# ADR 022: Plugin process supervisor foundation

Status: Accepted for pre-alpha implementation

## Context

M4.0 validates the wire contract without executing plugins. M4 now needs a process boundary that proves malformed stdout, stderr flooding, queue saturation, handshake timeout, and child crash cannot corrupt the core or grow memory without bound. Launching arbitrary manifest text through a shell or inheriting the full core environment would undermine that isolation.

## Decision

- Add an isolated Rust supervisor crate that consumes a validated manifest plus a separately resolved launch specification. The executable is an explicit path, arguments are a vector, and no shell command string exists.
- Child stdin/stdout carry only framed protocol envelopes. Stderr is drained independently into bounded metadata: observed byte count and truncation flag; raw diagnostics are not returned by default.
- Clear the child environment and add only an explicit bounded allowlist plus a non-secret session ID. A working directory is explicit and canonical.
- Windows launches headless protocol plugins with no visible console and a new process group. POSIX uses a separate process group. The supervisor retains the exact child handle and may terminate only that child on its own deadline.
- Handshake is deadline-bound: receive Hello, negotiate, send CoreHello, receive matching Ready. Any malformed/out-of-phase frame or timeout terminates the one session.
- Reader and writer tasks communicate through bounded Tokio channels. `try_send` exposes stable queue-full backpressure; a full inbound queue stops pipe reads rather than allocating more memory.
- The session exposes bounded receive deadlines, heartbeat sequence/freshness, explicit cancellation messages, drain, shutdown, and a stable crash/exit report.
- Graceful shutdown sends Drain then Shutdown and waits for a session-bound Shutdown acknowledgement or child exit. Deadline expiry kills and reaps the exact child.
- Initial process fixtures implement only a mock harness plus malformed, hanging, stderr-flood, and crash modes. Automatic restart budgets, daemon routing, and other plugin kinds remain later work.

## Consequences

- The core can prove process and transport containment before accepting real vendor plugins.
- Environment clearing may require future permission-aware variables or brokers; silent inheritance is not restored.
- Stderr content remains local to future opt-in diagnostics and cannot corrupt stdout framing.
- A bounded channel can deliberately stall a plugin; overload is visible instead of hidden in memory growth.

## Alternatives

- Spawn the manifest entry point directly with inherited environment: rejected because manifests are declarations, not resolved authority, and inherited secrets violate least privilege.
- Merge stdout and stderr: rejected because diagnostics could corrupt Protobuf framing.
- Use unbounded Tokio channels: rejected because a plugin can outpace the core indefinitely.
- Add automatic restart immediately: rejected until crash classification and restart budgets have separate tests and policy.
- Integrate Task/Run mutation in the first supervisor slice: rejected because process containment must be proven independently of canonical state.
