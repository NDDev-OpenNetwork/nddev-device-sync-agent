# NDDev OpenNetwork · device agent

AGPL-3.0-only native integration for NDS. [NDDev OpenNetwork](https://nddev.ai).

`crates/agent` composes the six compiled-in native adapters: sysinfo, clipboard,
cleaner, updater, GDS and RDS. Each adapter owns its manifest and provider
contract. The Flutter client calls the library to read actual native state;
configuration, untested reachability and a completed observation are distinct.
Mobile consumers share portable view types; local native tools require a
supported desktop host.

`crates/io` provides separate bounded socket and process capabilities. An
agent shares eight admission slots across its modules, uses finite deadlines
and output limits, and cancels owned work when closed. Native providers retain
their state and policy. Private paths, arguments and response content never
enter telemetry. Module operations carry real OpenTelemetry contexts through
the caller-installed shared telemetry SDK; its local logs do not imply remote
export or delivery.

Run `just check`, `cargo nextest run --locked --workspace`, `cargo deny check`
and `cargo audit`. OS transport checks exercise real sockets and child
processes. Module repositories own isolated real-provider acceptance; the
agent's Linux acceptance verifies composition against the pinned sysinfo
provider. Dependencies use immutable Git revisions.

This library has no standalone background daemon, remote-command endpoint,
maintenance mutation, backup/recovery or private estate configuration.
