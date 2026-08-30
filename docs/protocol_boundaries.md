# Protocol boundaries

- **Internal Event**: append-only canonical SQLite model, independent of terminal output.
- **Local control IPC**: `vibemuxd::control` implements the private pre-alpha v1 authenticated Windows named-pipe/POSIX UDS transport and only exposes `health`/`shutdown`. On Windows the descriptor/lock lives in a protected per-user hashed runtime and the pipe rejects remote clients; on POSIX the descriptor is `0600`. There is no TCP fallback. `vibemuxctl` only manages the standalone daemon lifecycle and explicit stale metadata recovery; it does not directly touch SQLite and never exposes token/endpoint path.
- **PTY**: provides interactive input/output only and never infers completion/approval.
- **ACP/MCP**: future protocol boundary; the current code does not create a fake gateway.
- **Plugin stdio**: `vibemux_plugin_protocol` implements the private pre-alpha Protobuf v1 schema, fixed-ceiling framing, manifest/negotiation, and session lifecycle; the wire crate itself has no process dependency, and the SDK and real plugins are still unimplemented.
- **Plugin supervisor**: `vibemux_plugin_supervisor` has been validated with real Windows/Linux mock children for stdout-only framing, independent bounded stderr metadata, bounded queues, deadline/heartbeat/cancel/drain/shutdown/crash; no daemon registry, restart policy, SDK, or real plugin exists yet.
- **A2A**: `vibemux_a2a` has verified one isolated A2A v1 HTTP+JSON local-loopback information-share slice: official Agent Card discovery, official client/server transport, bounded Message/DataPart, and correlation-preserving acknowledgement. That slice does not read or write the canonical store, does not create a VibeMux Task/Run, and does not support remote bind, JSON-RPC, gRPC, streaming, artifacts, authentication, or TCK; these capabilities must not be inferred from the local smoke.
- **Git artifact**: worktree, diff, and commit are auditable artifacts; merge is never automatic.
