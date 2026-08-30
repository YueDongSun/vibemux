# ADR 023: Daemon plugin registry and read-only status

Status: Accepted for the M4.2 pre-alpha slice

## Context

ADR 022 isolated plugin process supervision but did not assign daemon ownership or bound automatic recovery. A crashing child must not reset its own restart budget, block status queries, gain access to the writer, or leave unjoined cleanup on ordinary daemon shutdown.

## Decision

- `vibemuxd::plugin_registry` owns one worker and supervisor session per explicitly registered manifest identity. It has no writer, store, Task, or Run handle. Status is ephemeral process metadata, not a canonical event or a persisted state transition.
- A startup configuration is explicit and local. The default registry is empty. There is no discovery, installation, remote loading, automatic manifest permission grant, or plugin mutation IPC. Startup validation rejects duplicate identities and invalid capacity/policy before any child starts.
- Use bounded lifetime restart budgets, capped exponential backoff, unique session IDs on every attempt, and terminal quarantine. Successful handshake/heartbeat does not reset budget. Protocol/authority failures quarantine immediately; classified transient process/transport failures may consume the remaining budget.
- Registry workers accept only validated heartbeats after Ready. Unsolicited requests, responses, and events are rejected and quarantined. Plugin payloads never reach canonical state or a Task/Run mapper.
- Cancellation signals workers before joining them. In-progress handshake remains owned until its bounded deadline; active sessions drain/shutdown or are explicitly killed/reaped. Daemon cleanup joins plugins before shutting down the writer. Unknown reap outcome retains the recorded PID and quarantine rather than claiming stopped.
- Read-only status uses the existing authenticated local IPC, with a 64 KiB frame ceiling. IPC v2 adds `plugin_status`; v1 health/shutdown remain accepted and use v1 response envelopes. Updated clients may query/stop v1 daemons but refuse plugin status on v1. Old v1 clients reject v2 descriptors and need an upgrade.
- Shutdown IPC acknowledges acceptance, not completion. Process exit plus matching runtime cleanup, or the joined server result, establishes completion. Rust `Drop` only requests best-effort cancellation; callers requiring a receipt must await shutdown/wait.
- Registry state/budgets are scoped to one daemon lifetime and not persisted. Querying status never starts a child, clears quarantine, resets budget, opens SQLite, or invokes a plugin.

## Compatibility and migration

The local control version changes from 1 to 2; plugin Protobuf/manifest and SQLite versions remain unchanged. There is no database migration. See [plugin registry protocol](../plugin_registry_protocol.md) for old/new examples and limits. Existing daemon invocations without `--plugin-config` retain an empty registry. A stopped daemon can be restarted without that argument to disable this slice. No quarantine-clear/start/stop API or public plugin SDK compatibility promise is added.

## Threat model and limits

The IPC bearer token and OS transport restrictions remain mandatory. Status contains only bounded identity/lifecycle/error metadata, never argv, cwd, environment, raw stderr, prompts, or plugin payloads. Configured executables are operator-trusted code: clean environment and Git worktree isolation are not a filesystem/network sandbox. This slice provides no process-tree containment and cannot enforce a malicious executable's filesystem access. No database path or writer handle is supplied to plugins. Abrupt OS termination of the daemon does not have the same cleanup guarantee as joined shutdown. Same-user hostile code, sandbox enforcement, crash recovery across daemon lifetimes, operator quarantine recovery, and vendor plugins remain future work.

## Verification

Real child-process tests cover bounded crash exhaustion, independent healthy sibling visibility, permanent malformed/unsolicited-event quarantine, unchanged canonical events/projection, active/handshake/backoff cancellation, unresponsive shutdown, and standalone daemon startup/status/shutdown. IPC tests cover authentication, v1 health/shutdown compatibility, rejected mutation operation names, and maximum status payload size. Exact executed commands and platform limitations are recorded in `PROGRESS.md`.
