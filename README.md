# NDDev OpenNetwork · device agent

AGPL-3.0-only device integration runtime for NDS. [NDDev OpenNetwork](https://nddev.ai).

`crates/io` owns bounded native process and local-socket I/O used by the
separate sysinfo, clipboard, GDS, cleaner and updater adapters. It admits at
most 16 concurrent operations, rejects overload, bounds output and deadlines,
and contains owned child processes on cancellation. Raw tool output, arguments
and paths never enter its structured events.

This source provides integration I/O; it is not a remote-control daemon.
Native tools remain owners of their state and policy. No deployment,
backup/recovery, shell-command API or private estate configuration is included.

Run `just check`, `cargo nextest run --locked --workspace`, `cargo deny check`
and `cargo audit`. OS transport tests exercise actual local sockets and child
processes; module repositories own real-provider acceptance. Dependencies are
locked; runtime libraries are consumed by immutable Git revision.
