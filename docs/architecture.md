# Architecture

> **Document role:** canonical description of current modules, data flow, and contracts.
> **Scope:** current implementation and accepted design; no status vocabulary here.
> **Out of scope:** milestone evidence and release plan ([PROGRESS.md](../PROGRESS.md)); engineering rules ([AGENTS.md](../AGENTS.md)); accepted decisions ([docs/adr/](adr/)).

## Canonical model

`Task / Run / Event` is the terminal-independent domain model. Five orthogonal abstractions own distinct concerns:

| Abstraction | Owns | Does NOT do |
|---|---|---|
| `TerminalBackend` | pane lifecycle (WezTerm / tmux / mock) | decide Task success |
| `ExecutionBackend` | runtime that executes commands | own canonical state |
| `HarnessAdapter` | controlled argv / launch spec | execute shell strings |
| `WorkspaceManager` | Git worktree + branch per Run | merge, push, or clean automatically |
| `Run orchestrator` | spawn → terminal → harness | infer completion from PTY text |

"Same folder" means one logical repo; each Run uses a different physical worktree. A worktree only isolates concurrent modifications and is not a sandbox. User messages are data; they never enter a shell command string. Events are append-only; only the canonical writer may commit state and event transitions.

### Run execution layering

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
   Generic PTY / mock / structured dispatch
                    |
           controlled agent_host
                    |
        injected shell-free CommandRunner
```

Git, WezTerm, and tmux share one injected `CommandRunner`; `agent_host` only receives controlled argv/environment and never executes a shell command string. The Rust daemon's structured harness dispatch (ADR 029, below) executes Codex and Claude one-shot turns; ACP is used only for initialize-only probes. ACP execution, MCP, and remote A2A deployment remain unimplemented. The local stateful A2A gateway and supervisor use the Rust daemon writer, independent model-peer processes and owned Run worktrees; see [ADR 024](adr/024_stateful_a2a_supervisor.md).

## Process topology

```text
PowerShell / shell / future GUI
              |
              | local authenticated IPC
              v
        +-------------+
        | vibemux CLI |
        +-------------+
              |
              v
+--------------------------------------------------+
|                  vibemuxd                         |
|                                                  |
| Rust core:                                       |
| - canonical IDs, Task/Run/Event state machines   |
| - append-only event sequencing and projections   |
| - SQLite migrations and single-writer policy     |
| - bounded async routing and backpressure          |
| - plugin supervision and capability registry      |
| - A2A client/server gateway                       |
| - policy, cancellation, reconciliation, audit     |
+-------------+-------------------+----------------+
              |                   |
      plugin protocol             | A2A v1 transports
  framed messages over stdio      | gRPC / JSON-RPC / REST
              |                   |
      +-------+--------+      remote/local A2A peers
      |       |        |
  harness  terminal  sandbox
  plugins   plugins   plugins
      |
  OpenCode / Claude / Copilot / Pi / Grok / Gemini
```

## Current crate layout

The root `Cargo.toml` is authoritative for workspace membership (`version = "0.2.0-alpha.0"`, Rust edition `2024`, `rust-version = "1.85"`).

```text
crates/
├── vibemux_types/              # opaque IDs, domain objects, state machines, frontend query types
├── vibemux_events/             # canonical envelopes, idempotency, payload invariants
├── vibemux_harness/            # AgentKind/LauncherKind/ProbeState, harness registry/profiles/rows, dispatch protocol state machines (pure logic, no I/O)
├── vibemux_store/              # SQLite migrations (schema 4) and repositories (bundled)
├── vibemux_platform/           # Windows/POSIX process, IPC, process-tree containment, and launch trampoline primitives
├── vibemux_a2a/                # official A2A Rust SDK adapter (loopback verified)
├── vibemux_model_peer/         # optional, out-of-process model-provider peer
├── vibemux_plugin_protocol/    # M4.0 Protobuf v1 wire + TOML manifest (proto/vibemux_plugin_v1.proto)
├── vibemux_plugin_supervisor/  # M4.1 child-process supervision
├── vibemux_terminal_observer/  # out-of-process WezTerm observation plugin (list/focus native panes)
├── vibemux_workspace/          # Git worktree lifecycle and cleanup
├── vibemux_probe/              # M5.0 read-only launcher/gateway probe + trusted probe cache
├── vibemux_frontend/           # egui Supervisor Chat GUI + Ratatui diagnostic TUI + ASCII dump
├── vibemuxd/                   # M3 daemon (writer worker + control IPC v5 + plugin registry + harness dispatch)
└── vibemux_cli/                # pre-alpha vibemuxctl lifecycle + harnesses/switch + dispatch client
```

Outside `crates/`: `src/vibemux/` is the Python behavior-reference package, `tests/` holds Python tests and fixtures, `scripts/` holds smoke and benchmark scripts, and `docs/` holds architecture, ADRs, boundaries, and labs.

### Dependency direction

Domain crates must not depend on platform, database, terminal, or network implementations (ADR 013). `vibemux_harness` is pure logic (no I/O); detection inputs are injected by the daemon, which reads the trusted `vibemux_probe` cache. The agent/launcher/probe-state vocabulary lives in `vibemux_harness` and is re-exported by `vibemux_probe`, which depends on it. The harness dispatch request, route config, launch argv, protocol session machines, framer, capture budget, and attempt phases are also pure `vibemux_harness` logic; `vibemuxd` performs every effect (ADR 029). The normative layering from `AGENTS.md` §4.2:

```text
types
  ↑
events / plugin-api
  ↑
store / workspace / platform / a2a
  ↑
plugin-host
  ↑
vibemuxd
  ↑
vibemux-cli
```

With the current crates placed in that layering: `types → events → harness → store / workspace / platform / a2a / probe → plugin-api → plugin-host → vibemuxd → vibemux-cli`.

## Core versus plugin boundary

| Capability | Core | Plugin |
|---|---:|---:|
| Canonical Task/Run/Event types | Yes | No |
| State transition validation | Yes | No |
| Event sequencing and idempotency | Yes | No |
| SQLite schema and migrations | Yes | No |
| A2A Agent Card, task mapping, streaming, routing | Yes | No |
| Backpressure, cancellation, deadlines | Yes | No |
| Plugin process supervision | Yes | No |
| Policy enforcement and permission decisions | Yes | Optional policy extension, core remains authoritative |
| Git worktree safety invariants | Yes | Optional platform helper only |
| Terminal-specific commands | No | Yes |
| Harness-specific launch and structured protocol | First-party structured adapters only (ADR 029); the vendor CLI still runs as a separate contained process | Yes, for third-party and optional adapters |
| Sandbox implementation | No | Yes |
| UI, notifications, GitHub integrations | No | Yes |
| Model-provider configuration helpers | No | Yes |
| Benchmark reporters/exporters | No | Yes |
| Direct database writes | Yes, single writer | Never |

## Plugin model

V1 plugins are out-of-process executables. The project does not expose a Rust `cdylib` ABI as the public plugin interface (ADR 013, ADR 021).

Initial plugin transport:

- child process managed by `vibemuxd`;
- length-delimited, versioned Protobuf frames over stdin/stdout;
- stderr reserved for human-readable diagnostics;
- optional JSON debug codec for development only;
- capability negotiation during handshake;
- bounded frame size, bounded queues, deadlines, cancellation, heartbeat, and structured shutdown;
- no direct access to the core SQLite database;
- explicit permission manifest and platform declaration.

Future long-lived plugins may use Windows named pipes or Unix domain sockets. Local unauthenticated TCP is not the default plugin transport.

## External A2A transport policy

- External compatibility: JSON-RPC and HTTP+JSON/REST.
- High-throughput VibeMux-to-VibeMux path: gRPC when both Agent Cards advertise it.
- Streaming: transport-native streaming with bounded buffering and cancellation propagation.
- The core canonical event model remains independent of A2A wire objects.
- Internal `Task` and external A2A `Task` are related through an explicit binding table; they are not assumed to be the same object.
- Protocol types from the official Rust SDK are wrapped behind a VibeMux-owned adapter crate so SDK churn does not leak through the whole codebase (ADR 024).

## Daemon writer ownership

`vibemuxd::WriterWorker` constructs and exclusively owns `SqliteStore` inside a dedicated blocking thread. Callers only hold a bounded queue handle; when the queue is full, an explicit backpressure error is returned. The nonce-bearing lock next to the database is established through an atomic `create_new`; a second writer fails closed. After shutdown, the holder verifies the nonce before deletion.

`vibemuxd::control` uses a per-instance named pipe on Windows and a randomized per-instance Unix-domain socket on POSIX. The listener is bound before the Git-ignored runtime descriptor containing the protocol version, endpoint, and OS-CSPRNG bearer token is published. The four-byte big-endian length prefix enforces a 64 KiB ceiling before the JSON payload is allocated, and connection reads/writes and peer-close have deadlines. The descriptor owner token also guards descriptor/socket cleanup; an old instance cannot delete the runtime artifact of a replacement. IPC v5 is current: v2 added read-only `plugin_status`, v3 added `harness_refresh`/`harness_snapshot`/`harness_switch`, v4 added bounded read-only `frontend_tasks`/`frontend_task` queries plus `terminal_inspect`/`terminal_link`/`terminal_focus`/`terminal_unlink` observation operations (see [terminal observation protocol](terminal_observer_protocol.md)), and v5 adds the six `harness_dispatch_*` operations (see below); v1 `health`/`shutdown` and v2–v4 requests remain accepted for their operations. A pre-v5 client refuses a v5 descriptor, so `vibemuxctl` and `vibemuxd` are upgraded together. Shutdown acceptance signals in-flight harness dispatches and probes before the reply, then joins dispatch tasks and plugin cleanup before writer shutdown. Blocking writer calls are isolated through `spawn_blocking`; no async mutex guard is held across an await. There is no TCP fallback.

The `vibemuxd` binary is an unprivileged foreground process owner; `vibemuxctl daemon start|health|stop` is responsible for the on-demand lifecycle. Readiness must come from authenticated health; the PID is only used to compare this spawn's identity. Windows uses a fixed, non-user-code-interpolated system PowerShell companion that holds the exact process handle and severs captured-stdout inheritance; POSIX uses direct argv spawn with a separate process group. During migration the path layer is fixed to `.vibemux/vibemux_rust.sqlite3`, and runtime symlinks and symlink/hardlink aliases that point at the Python database are refused.

`vibemuxctl daemon inspect` first attempts authenticated health and then reads bounded, Debug-redacted descriptor/writer-lock snapshots. It returns the domain-separated SHA-256 confirmation only when every recorded PID is absent, recorded identities agree across the artifacts that are present, and all present artifacts parse successfully. Descriptor-only and lock-only stale states can also be recoverable. `daemon recover` re-checks the confirmation, PID, and original bytes, and then removes only unchanged descriptor/UDS socket/writer lock; there is no force flag, no PID kill, and no database operation. Python database cutover remains unimplemented; same Windows logon SID is the collaboration trust boundary; inter-process ACL isolation is not promised.

Windows control metadata now lives under `%LOCALAPPDATA%\VibeMux\runtime\<domain-separated project hash>`. The protected parent DACL and the leaf/file effective ACL contain only the current SID, `SYSTEM`, and Administrators; the named pipe explicitly sets `PIPE_REJECT_REMOTE_CLIENTS` and continues to require the bearer token. New daemons hold both the protected lock and the legacy project lock as an upgrade compatibility gate, preventing an old binary from racing a new binary for the same SQLite writer. A healthy legacy daemon can still be queried and stopped; stale legacy artifacts still flow through the same confirmation recovery. POSIX path semantics, `0600`, and UDS behavior are unchanged.

## Plugin protocol and supervisor

`vibemux_plugin_protocol` is the isolated M4 wire boundary: the checked-in Protobuf schema is generated with pinned Prost and vendored protoc; the stdin/stdout frame uses four-byte big-endian length with a fixed ceiling. The manifest only stores argv, kind, platform, capabilities, and requested permissions; the core negotiation grants only the intersection of manifest declarations, the Hello request, and core policy. The codec and state machine alone are implemented: it does not spawn a child, does not read stderr, depends on no database, and does not map opaque payloads to Task/Run mutations.

`vibemux_plugin_supervisor` owns a single child on top of the protocol: canonical executable/cwd, argv, clean environment, Windows hidden process group / POSIX process group, bounded stdin/stdout channels, and an independent stderr drain. Handshake, receive, and shutdown have deadlines; queue full, heartbeat, cancel, drain, and exit/crash are all structured. The daemon owns a bounded in-memory registry with lifetime restart budgets, backoff, quarantine, and read-only status. Explicit startup configuration is optional; default startup loads no plugins. There are no real vendor plugins or canonical-state mutations. See [ADR 023](adr/023_daemon_plugin_registry.md) and the [registry protocol](plugin_registry_protocol.md).

`vibemux_terminal_observer` is a bounded WezTerm observer plugin. It uses an optional versioned terminal observation payload inside the unchanged v1.0 envelope; the daemon routes each request only after checking the plugin's effective terminal grant (startup configuration v2; v1 grants nothing), session, request/correlation identity, deadline, and response size. Run-scoped pane links are observation-only and never imply execution ownership or task completion. See the [terminal observation protocol](terminal_observer_protocol.md) and [ADR 028](adr/028_supervisor_chat_frontend.md).

## Harness dispatch (ADR 029)

`vibemuxd` can run one structured vendor turn per request and record its outcome canonically. The pure protocol logic lives in `vibemux_harness::dispatch`; the daemon owns config loading, Git `HEAD` reads, process launch, pipe I/O, deadlines, cancellation, the in-memory transcript, and every writer call. Store schema 4 adds the versioned `harness_dispatches` record, written only by `WriterWorker` in the same transaction as its event.

- **Opt-in.** Dispatch is disabled unless the operator creates `.vibemux/harness_dispatch.json` (schema 1). The daemon reads it once at startup and pins its SHA-256; a missing or invalid file makes dispatch operations fail with a fixed code without blocking startup. Routes name an absolute vendor executable (a `.exe` on Windows, outside the project root, whose file stem is the harness command name), an environment-variable allowlist, and an optional model. The config carries no operator argv; every argument comes from the protocol profile.
- **Protocols.** Codex `exec --json` and `app-server`, and Claude stream-json, can execute. ACP (OpenCode, Copilot, Grok) is initialize-only probe support; the config, admission, and launch builder all refuse ACP execution.
- **Gates.** A valid request, an enabled execution route, a harness `detected` in the persisted registry (ADR 013), and no other active reservation. At most one dispatch is active per project, and vendor processes run in the project root with read-only protocol profiles, which are not an OS sandbox.
- **Process.** The executor spawns the first-party `vibemux_launch_trampoline` with a cleared environment, contains it with `vibemux_platform::ProcessTree` (a kill-on-close Job Object on Windows, a process group on POSIX), then releases it to start the vendor CLI, so the vendor and its descendants are members from their first instruction. Prompts reach the vendor only as JSON-string data or stdin, never argv or a shell.
- **Outcome.** `RunStatus::Succeeded` requires a correlated structured terminal record and a clean exit (`StructuredAdapter` authority). Deadlines, cancellation, protocol violations, and capture overflow end the attempt with a fixed error code. Startup recovery moves interrupted attempts to a conservative state before the writer reports ready.
- **Capture.** Raw vendor records are kept byte-exact in a bounded in-memory transcript and are returned only by `harness_dispatch_output`; events, logs, status, and errors stay content-free (prompt hash and size only). Transcripts are lost on daemon restart.
- **Control.** `harness_dispatch_{catalog, probe, submit, status, output, cancel}` require Control v5; `vibemuxctl dispatch` is the thin client.

Live vendor CLIs have not been exercised yet; see [ADR 029](adr/029_daemon_harness_dispatch.md) and [PROGRESS.md](../PROGRESS.md) for the evidence and remaining gates.

## Probe and unified frontend boundary

`vibemux_probe` only produces versioned, content-free, read-only diagnostics: launcher/version, explicit provider endpoint, CC Switch health/aggregate telemetry, and the A2A self-test. Launcher verification is not the same as authentication/inference verification.

`vibemux_frontend` consumes probe reports and authenticated Control v4 snapshots; it never opens the core SQLite database. Its native Supervisor Chat workspace (ADR 028) has one main conversation, real task activity, secondary Agents/Settings/Diagnostics, and docked or independent task windows. The Studio light theme is the default for new profiles; five existing dark themes remain compatible. Task queries run through the daemon writer channel and return bounded metadata, not prompt bodies. A separately configured WezTerm observation plugin may list matching native panes and focus an explicitly linked pane after identity validation. Native TUI content stays in WezTerm. There is no continuous coordinator chat adapter, harness launch/input forwarding, embedded terminal renderer, or terminal-text completion inference. Ratatui remains the diagnostic surface, with existing classic/high-contrast/mono/light/nord/gruvbox themes. Shared palette export and terminal theme generation remain governed by ADR 026.

## Stateful local A2A and supervisor

`vibemux_a2a` isolates official wire types and all three local transports from canonical state. Its bounded backend/runtime contracts authorize each subject, retain task identity, and supervise cancellation and subscriptions. The daemon injects a non-owning `WriterHandle`; schema-2 transactions validate state versions and verifier receipts before committing Task/Run bindings plus events. Optional `vibemux_model_peer` processes read selected CC Switch profiles read-only and return data artifacts. `vibemux_workspace` binds role worktrees to private owner receipts. The current supervisor recipe does not execute generated code or provide terminal/harness parity. See [ADR 024](adr/024_stateful_a2a_supervisor.md) and the [supervisor protocol](a2a_supervisor_protocol.md) for limits and startup commands.

## Python and CLI boundaries

The Python package (`src/vibemux/`) is the behavior reference and complete prototype entry point. It owns Mock, a WezTerm command adapter, a tmux adapter, native/POSIX-compatible execution modeling, Git worktree management, an append-only SQLite event log, safe diff/stop/cleanup primitives, an offline mock harness, resource ownership, and reconciliation. ACP/MCP vendor adapters, ConPTY, the scheduler, and automatic merge/push are not implemented. Python and Rust use separate databases (`.vibemux/vibemux.sqlite3` and `.vibemux/vibemux_rust.sqlite3`), preventing an accidental dual-writer cutover.

## Linked authorities

- Engineering rules: [AGENTS.md](../AGENTS.md)
- Implementation status and evidence: [PROGRESS.md](../PROGRESS.md)
- Architecture decisions: [docs/adr/](adr/)
- Harness dispatch: [ADR 029](adr/029_daemon_harness_dispatch.md)
- Platform support matrix: [docs/platform_support.md](platform_support.md)
- Wire/protocol boundaries: [docs/protocol_boundaries.md](protocol_boundaries.md)
- Plugin registry protocol: [docs/plugin_registry_protocol.md](plugin_registry_protocol.md)
- A2A supervisor protocol: [docs/a2a_supervisor_protocol.md](a2a_supervisor_protocol.md)
- A2A conformance procedure: [docs/a2a_conformance_validation.md](a2a_conformance_validation.md)
- User entry / quick start: [README.md](../README.md)
