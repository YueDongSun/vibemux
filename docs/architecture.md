# Architecture

```text
PowerShell / CLI / Future TUI
             |
      Application Services
             |
   +---------+----------+
   |         |          |
 Domain    Storage    Policy
   |         |          |
   +---- Canonical Event Model ----+
   |                                |
   v                                v
WorkspaceManager              TerminalBackend
Git worktree                  WezTerm / tmux / mock
   |                                |
   +---------- Run Orchestrator ----+
                    |
              ExecutionBackend
          Native Windows / POSIX
                    |
              HarnessAdapter
       Generic PTY / mock / future ACP
                    |
           controlled agent_host
                    |
        injected shell-free CommandRunner
```

"Same folder" means one logical repo; each Run uses a different physical worktree. A worktree only isolates concurrent modifications and is not a sandbox. `TerminalBackend` manages pane lifecycle and does not decide Task success; `ExecutionBackend` describes the runtime; `HarnessAdapter` only produces a controlled launch spec. Git, WezTerm, and tmux share one injected `CommandRunner`; `agent_host` only receives controlled argv/environment and never executes a shell command string. ACP/MCP and stateful/remote A2A gateways remain unimplemented; the isolated loopback-only A2A information-share slice is described in `docs/protocol_boundaries.md`.

## Probe and unified frontend boundary

`vibemux_probe` only produces versioned, content-free, read-only diagnostics: launcher/version, explicit provider endpoint, CC Switch health/aggregate telemetry, and the A2A self-test. Launcher verification is not the same as authentication/inference verification.

`vibemux_frontend` consumes the probe report and the future daemon control API; it never reads or writes the core SQLite directly. The unified dashboard reserves a `NativeTuiSlot` for Claude, Codex, OpenCode, Copilot, and Grok respectively. These slots are ultimately carried by the terminal plugin / ConPTY for each CLI's own TUI; the dashboard does not parse terminal text, does not redraw the vendor TUI, and does not infer completion from pane exit. Today the slots are merely `reserved`; there is no launch or input-forwarding behavior.

## Daemon writer ownership

`vibemuxd::WriterWorker` constructs and exclusively owns `SqliteStore` inside a dedicated blocking thread. Callers only hold a bounded queue handle; when the queue is full, an explicit backpressure error is returned. The nonce-bearing lock next to the database is established through an atomic `create_new`; a second writer fails closed. After shutdown, the holder verifies the nonce before deletion.

`vibemuxd::control` uses a per-instance named pipe on Windows and a randomized per-instance Unix-domain socket on POSIX. The listener is bound before the Git-ignored runtime descriptor containing the protocol version, endpoint, and OS-CSPRNG bearer token is published. The four-byte big-endian length prefix enforces a 64 KiB ceiling before the JSON payload is allocated, and connection reads/writes and peer-close have deadlines. The descriptor owner token also guards descriptor/socket cleanup; an old instance cannot delete the runtime artifact of a replacement. Only the versioned `health` and `shutdown` calls are open today. Blocking writer calls are isolated through `spawn_blocking`; no async mutex guard is held across an await. There is no TCP fallback.

The `vibemuxd` binary is now an unprivileged foreground process owner; `vibemuxctl daemon start|health|stop` is responsible for the on-demand lifecycle. Readiness must come from authenticated health; the PID is only used to compare this spawn's identity. Windows uses a fixed, non-user-code-interpolated system PowerShell companion that holds the exact process handle and severs captured-stdout inheritance; POSIX uses direct argv spawn with a separate process group. During migration the path layer is fixed to `.vibemux/vibemux_rust.sqlite3`, and runtime symlinks and symlink/hardlink aliases that point at the Python database are refused.

`vibemuxctl daemon inspect` first attempts authenticated health and then reads bounded, Debug-redacted descriptor/writer-lock snapshots. It returns the domain-separated SHA-256 confirmation only when every recorded PID is absent, recorded identities agree across the artifacts that are present, and all present artifacts parse successfully. Descriptor-only and lock-only stale states can also be recoverable. `daemon recover` re-checks the confirmation, PID, and original bytes, and then removes only unchanged descriptor/UDS socket/writer lock; there is no force flag, no PID kill, and no database operation. Python database cutover remains unimplemented; same Windows logon SID is the collaboration trust boundary; inter-process ACL isolation is not promised.

Windows control metadata now lives under `%LOCALAPPDATA%\VibeMux\runtime\<domain-separated project hash>`. The protected parent DACL and the leaf/file effective ACL contain only the current SID, `SYSTEM`, and Administrators; the named pipe explicitly sets `PIPE_REJECT_REMOTE_CLIENTS` and continues to require the bearer token. New daemons hold both the protected lock and the legacy project lock as an upgrade compatibility gate, preventing an old binary from racing a new binary for the same SQLite writer. A healthy legacy daemon can still be queried and stopped; stale legacy artifacts still flow through the same confirmation recovery. POSIX path semantics, `0600`, and UDS behavior are unchanged.

`vibemux_plugin_protocol` is the isolated M4 wire boundary: the checked-in Protobuf schema is generated with pinned Prost and vendored protoc; the stdin/stdout frame uses four-byte big-endian length with a fixed ceiling. The manifest only stores argv, kind, platform, capabilities, and requested permissions; the core negotiation grants only the intersection of manifest declarations, the Hello request, and core policy. Today only the codec and state machine are implemented: it does not spawn a child, does not read stderr, depends on no database, and does not map opaque payloads to Task/Run mutations.

`vibemux_plugin_supervisor` owns a single child on top of the protocol: canonical executable/cwd, argv, clean environment, Windows hidden process group / POSIX process group, bounded stdin/stdout channels, and an independent stderr drain. Handshake, receive, and shutdown have deadlines; queue full, heartbeat, cancel, drain, and exit/crash are all structured. Daemon registry integration and restart/quarantine policy are not implemented, and there are no real vendor plugins or canonical-state mutations.
