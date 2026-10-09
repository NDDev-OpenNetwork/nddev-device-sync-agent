#!/usr/bin/env python3
"""Fetch a pinned public native test dependency into an isolated CI directory."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import time
import urllib.request

def require(condition, message="invalid native provider specification"):
    if not condition:
        raise ValueError(message)

spec = json.loads(Path(sys.argv[1]).read_text())
root = Path(sys.argv[2]).resolve()
root.mkdir(mode=0o700, parents=True, exist_ok=False)
repository, release, asset = (spec[key] for key in ("repository", "release", "linux_asset"))
require(re.fullmatch(r"NDDev-OpenNetwork/[a-z0-9-]+", repository))
require(re.fullmatch(r"[a-zA-Z0-9._-]+", release))
require(re.fullmatch(r"[a-zA-Z0-9._-]+", asset))
require(re.fullmatch(r"[a-f0-9]{64}", spec["sha256"]))
require(0 < spec["size"] <= 100_000_000)
archive = root / asset
url = f"https://github.com/{repository}/releases/download/{release}/{asset}"
started = time.monotonic()
digest = hashlib.sha256()
size = 0
with urllib.request.urlopen(url, timeout=20) as source, archive.open("xb") as target:
    while chunk := source.read(65536):
        size += len(chunk)
        if size > spec["size"] or time.monotonic() - started > 180:
            raise RuntimeError("native provider download exceeded its bound")
        target.write(chunk)
        digest.update(chunk)
require(size == spec["size"] and digest.hexdigest() == spec["sha256"], "native provider digest/size mismatch")
if spec.get("attestation"):
    subprocess.run(["gh", "attestation", "verify", str(archive), "--repo", repository,
                    "--signer-workflow", repository + "/.github/workflows/release.yml",
                    "--source-ref", "refs/tags/" + release, "--deny-self-hosted-runners"], check=True, timeout=60)
extracted = root / "extracted"
extracted.mkdir(mode=0o700)
if asset.endswith(".tar.gz"):
    with tarfile.open(archive) as bundle:
        members = bundle.getmembers()
        require(sum(member.size for member in members) <= 1_000_000_000)
        bundle.extractall(extracted, filter="data")
else:
    archive.rename(extracted / asset)
for variable, relative in spec["executables"].items():
    require(re.fullmatch(r"NDS_TEST_[A-Z_]+", variable))
    executable = (extracted / relative).resolve(strict=True)
    require(executable.is_relative_to(extracted) and executable.is_file())
    executable.chmod(0o755)
    if "GITHUB_ENV" in os.environ:
        with open(os.environ["GITHUB_ENV"], "a") as output:
            output.write(f"{variable}={executable}\n")
    else:
        print(f"{variable}={executable}")
