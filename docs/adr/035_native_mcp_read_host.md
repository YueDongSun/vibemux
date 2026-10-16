# ADR 035: Native MCP read host

Status: Bounded pre-alpha implementation with native Windows focused evidence;
whole-workspace qualification and operator acceptance pending

## Context

Agent clients need a bounded metadata view of the existing daemon through MCP.
The plugin supervisor implements VibeMux's Protobuf protocol, which is a distinct
wire contract from MCP. The daemon remains the single authoritative state writer.
The multi-producer Hub companion also serves AAG and Mesh, so a VibeMux-only host
does not replace it.

## Decision

Add a first-party frontend client, `vibemux_mcp`, with two process roles. The
foreground process owns the newline JSON-RPC MCP session, permission reduction,
request validation and tool discovery. It lazily starts one same-executable IPC
reader child after an authorized metadata request. The child uses only the
existing `ControlClient` read methods. It never opens SQLite or launches a daemon.

The launch requires an explicit descriptor path and the optional, default-denied
`--allow_metadata_read` grant. Initialization can reduce that grant through the
documented experimental permission field; client input cannot create a grant.
Initialization, discovery and host status perform no descriptor or producer I/O.
These restrictions are a first-party frontend boundary over the existing daemon
bearer. They do not claim to be the daemon's authoritative plugin permissions.

Read tools cover health, bounded task pages, task/run/event/artifact metadata and
dispatch status. Each result uses an explicit output projection. Worktree paths,
branch names, process identities, prompts, captures, source bodies and credentials
are excluded. Titles are intentionally disclosed metadata and are bounded.
Daemon-declared status does not establish model completion or verification.

One retained launch thread prepares and contains the child with `ProcessTree`
before any request input. The protocol task stays responsive while OS process
creation is pending. A fixed executable and argv, cleared environment and
protocol-only pipes avoid a general executable launcher. The lifecycle has
explicit ready, launching, active, cleanup, backoff and exhausted states.
Cancellation or timeout unloads daemon tools immediately; cleanup retains the
launch thread and any eventual child, terminates the tree and then reaps the
leader. No replacement can start until cleanup is terminal and the full backoff
has elapsed. Host-only status and ping remain available during cleanup.

OS process creation is not forcibly cancellable. A stalled launch may delay
final host exit, including the owner-drop fallback, until the retained thread
returns and its child is reconciled. This is a resource-ownership limitation,
not a reason to abandon the thread or claim cleanup from a timeout. Once a
child is known, its shutdown uses one absolute two-second deadline. Reads are
never silently replayed. At most three restarts follow the first launch attempt
over the entire MCP session. Exhaustion stays unloaded until a new session.
Tool changes are announced only after `notifications/initialized`.

MCP protocol version 2025-06-18 is supported. Unknown requested versions receive
that supported version for client negotiation, never an invented future version.
Control IPC remains v7 and the store remains schema 5. Rust workspace version
`0.2.0-alpha.1` identifies this additive pre-alpha frontend surface.

## Threat and failure boundary

The host limits frames before allocation, output before writing, concurrent
producer calls, worker lifetime and restart attempts. Client-provided arguments
cannot select new operations, processes or environment variables. Errors contain
fixed codes only. Descriptor safety and control version validation stay owned by
`ControlClient`. A malicious same-user executable or compromised daemon remains
outside this containment claim. `ProcessTree` is process ownership, not an OS
sandbox; POSIX does not provide Windows kill-on-close job semantics. Worker idle
and request deadlines bound an orphaned reader that loses its parent pipe.

## Alternatives and compatibility

- Reuse the plugin supervisor unchanged: rejected because its Protobuf lifecycle
  and permission registry do not implement MCP.
- Read the database directly: rejected because it bypasses the daemon contract.
- Add the MCP surface to the daemon: rejected because client crashes and framing
  failures should remain independent from the authoritative writer.
- Retire the Hub companion: rejected because its three-producer aggregation has
  a different owner and purpose.

Existing clients and descriptors need no migration. Operators opt in by launching
the new binary with the explicit descriptor and grant. Removing that launch and
reverting the additive commit rolls back the host without migrating state.

The existing dependency gate required a separate, approved frontend compatibility
migration: egui/eframe 0.34.3 replaces the unmaintained font-parser chain, and
ratatui 0.30.2 replaces the unmaintained macro dependency and old LRU cache.
Crossterm 0.29 matches the ratatui backend boundary. This raises the workspace
MSRV, pinned compiler and Windows/Ubuntu CI target to Rust 1.92. The presentation
adapters preserve themes, layout, current action callbacks, demo isolation
and existing assertions while adopting the new root-`Ui` API. The earlier time
0.3.47 and rustls 0.23.45 security patches remain. `deny.toml` contains the exact
approved ten-license allowlist; advisory and source checks keep their defaults.
No ignore, dependency exclusion or lint reduction is part of this migration.

Qualification requires real stdio, disposable daemon IPC, process cleanup,
permission, framing, projection and restart tests, followed by all producer gates
and independent acceptance. Fixture results do not establish live deployment,
operator acceptance, Linux process behavior or real harness completion.

## References

- [MCP lifecycle](https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle)
- [MCP stdio transport](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports)
- [MCP tools](https://modelcontextprotocol.io/specification/2025-06-18/server/tools)
- [MCP cancellation](https://modelcontextprotocol.io/specification/2025-06-18/basic/utilities/cancellation)
- [MCP 2025-06-18 request and notification schema](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/schema/2025-06-18/schema.ts)
- [ADR 022: plugin supervisor](022_plugin_process_supervisor_foundation.md)
- [ADR 029: contained harness dispatch](029_daemon_harness_dispatch.md)
