#!/usr/bin/env python3
"""Qualify the assembled agent against an isolated real sysinfo daemon."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time

if sys.platform != "linux":
    raise SystemExit("This provider qualification requires Linux")
root = Path(__file__).resolve().parent.parent
provider = Path(os.environ["NDS_TEST_SYSINFO_DAEMON"]).resolve(strict=True)
build = subprocess.run(
    ["cargo", "test", "--locked", "-p", "nddev-device-sync-agent", "--test", "lifecycle",
     "--no-run", "--message-format=json"],
    cwd=root, text=True, capture_output=True, check=True, timeout=180,
)
executables = [item["executable"] for line in build.stdout.splitlines()
               if (item := json.loads(line)).get("reason") == "compiler-artifact" and item.get("executable")]
if len(executables) != 1:
    raise RuntimeError("expected one agent acceptance executable")
with tempfile.TemporaryDirectory(prefix="nds-agent-provider-") as temporary:
    fixture = Path(temporary)
    fixture.chmod(0o700)
    environment = {**os.environ, "XDG_RUNTIME_DIR": temporary, "NDS_ISOLATED_PROVIDER": "1"}
    for key in ["LISTEN_PID", "LISTEN_FDS", "LISTEN_FDNAMES"]:
        environment.pop(key, None)
    daemon = subprocess.Popen([str(provider)], env=environment, stdin=subprocess.DEVNULL,
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        deadline = time.monotonic() + 10
        while not (fixture / "rldyour-sysinfo.sock").exists():
            if daemon.poll() is not None or time.monotonic() >= deadline:
                raise RuntimeError("native provider did not become ready")
            time.sleep(0.02)
        subprocess.run([executables[0], "--ignored", "native_query_updates_only_its_own_observation"],
                       env=environment, check=True, timeout=30)
        subprocess.run([executables[0], "--ignored", "cached_missing_provider_is_not_a_fresh_native_observation"],
                       env={**environment, "PATH": ""}, check=True, timeout=30)
        # Pause only this owned daemon to exercise a genuine stalled local I/O.
        daemon.send_signal(signal.SIGSTOP)
        deadline = time.monotonic() + 2
        while True:
            status = Path(f"/proc/{daemon.pid}/status").read_text()
            if any(line.startswith("State:") and "T (stopped)" in line for line in status.splitlines()):
                break
            if time.monotonic() >= deadline or daemon.poll() is not None:
                raise RuntimeError("owned native provider did not pause")
            time.sleep(0.02)
        subprocess.run([executables[0], "--ignored", "closing_an_in_flight_query_preserves_unobserved_state"],
                       env={**environment, "NDS_PAUSED_PROVIDER": "1"}, check=True, timeout=15)
    finally:
        daemon.send_signal(signal.SIGCONT)
        daemon.terminate()
        try:
            daemon.wait(timeout=3)
        except subprocess.TimeoutExpired:
            daemon.kill()
            daemon.wait(timeout=3)
print("Assembled agent observation and shutdown acceptance passed")
