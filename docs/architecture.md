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

Git, WezTerm, and tmux share one injected `CommandRunner`; `agent_host` only receives controlled argv/environment and never executes a shell command string. The Rust daemon's structured harness dispatch (ADR 029, below) executes Codex and Claude one-shot turns and uses ACP only for initialize-only probes. The CLI-process native dispatcher (ADR 040, below) can run an ACP turn on a route that sets `allow_execution`; that path is fixture-verified only, and whether it should follow the daemon's probe-only rule is an open decision. Remote A2A deployment remains unimplemented. Native MCP exposes read-only daemon metadata through the separate frontend described below (ADR 035). The local stateful A2A gateway and supervisor use the Rust daemon writer, independent model-peer processes and owned Run worktrees; see [ADR 024](adr/024_stateful_a2a_supervisor.md).

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

The root `Cargo.toml` is authoritative for workspace membership (`version = "0.2.0-alpha.1"`, Rust edition `2024`, `rust-version = "1.92"`).

```text
crates/
├── vibemux_types/              # opaque IDs, domain objects, state machines, frontend query types, persistent-chat metadata types
├── vibemux_events/             # canonical envelopes, idempotency, payload invariants
├── vibemux_harness/            # AgentKind/LauncherKind/ProbeState, harness registry/profiles/rows, dispatch + DSH session state machines (pure logic, no I/O)
├── vibemux_harness_native/     # standalone native vendor stream adapters and bounded request dispatcher over the plugin supervisor (ADR 040)
├── vibemux_workflow/           # dual-track workflow contracts, gates, broker rules, optimizer (pure logic, no I/O; ADR 031)
├── vibemux_decision/           # decision port: questions, facts, answer validation, local rules, fallback resolver, config, records, peer wire (pure logic, no I/O; ADR 034)
├── vibemux_decision_peer/      # one-shot out-of-process decision peer: the only decision-API network client (TypeSafe Jev; ADR 034)
├── vibemux_store/              # SQLite migrations (schema 14) and repositories (bundled)
├── vibemux_platform/           # Windows/POSIX process, IPC, process-tree containment, and launch trampoline primitives
├── vibemux_a2a/                # official A2A Rust SDK adapter (loopback verified)
├── vibemux_model_peer/         # optional, out-of-process model-provider peer
├── vibemux_plugin_protocol/    # M4.0 Protobuf v1 wire + TOML manifest (proto/vibemux_plugin_v1.proto)
├── vibemux_plugin_supervisor/  # M4.1 child-process supervision
├── vibemux_terminal_observer/  # out-of-process WezTerm observation plugin (list/focus native panes)
├── vibemux_workspace/          # Git worktree lifecycle and cleanup
├── vibemux_probe/              # M5.0 read-only launcher/gateway probe + trusted probe cache
├── vibemux_dsh/                # DSH session driver: config trust, process containment, framing, deadlines, records
├── vibemux_client/             # GUI-neutral daemon, probe, Ask, and Git client shared by the frontends
├── vibemux_frontend/           # egui Supervisor Chat GUI + Ratatui diagnostic TUI + navigable read-only dashboard + DSH terminal surface + ASCII dump
├── vibemux_desktop/           # ADR 043 Tauri 2 desktop shell; the Svelte 5 UI is crates/vibemux_desktop/ui
├── vibemux_mcp/                # first-party MCP stdio frontend + contained metadata reader (ADR 035)
├── vibemuxd/                   # M3 daemon (writer worker + control IPC v17 + plugin registry + harness dispatch + persistent chat service + workflow + DSH session + decision service)
└── vibemux_cli/                # pre-alpha vibemuxctl: unified command tree (status/workspace/probe/ui/harness, --format), daemon lifecycle, harnesses/switch + dispatch + workflow/DSH/decision client (ADR 041)
```

Outside `crates/`: `src/vibemux/` is the Python behavior-reference package, `tests/` holds Python tests and fixtures, `scripts/` holds smoke and benchmark scripts, and `docs/` holds architecture, ADRs, boundaries, and labs.

### Native MCP frontend

`vibemux_mcp` has a foreground protocol owner and one lazy, same-executable reader
child. `config`, `protocol` and `tools` define launch grants, lifecycle and fixed
argument schemas; `framing` bounds transport; `projection` selects metadata;
`control_reader` uses the existing four read-only `ControlClient` methods; and
`reader_owner` retains the synchronous launch thread and contained child;
`supervisor`/`session` own lifecycle transitions, cancellation, deadlines and
bounded restarts. No MCP module accesses the database or registers a new control
operation.

The explicit launch grant is required for metadata and client negotiation can
reduce it. Discovery and host status are configuration-only. Reader failures
unload daemon-backed tools while preserving the MCP session; a bounded backoff
restores availability for a fresh explicit call, without replaying a read. The
session permits the initial launch attempt and at most three restart attempts.
Process-tree termination precedes leader reap. Pending launch cleanup keeps
host-only requests responsive and fences replacement until terminal cleanup plus
backoff. Final shutdown joins the retained launch owner; a stalled OS launch can
delay exit rather than be abandoned. See the protocol's separate request and
cleanup deadlines.

This first-party frontend uses the existing daemon bearer boundary. It does not
represent itself as a plugin permission registry: the existing plugin supervisor
speaks VibeMux's Protobuf contract, not MCP. The Hub companion continues to own
its multi-producer view. See [ADR 035](adr/035_native_mcp_read_host.md) and the
[wire contract](native_mcp_protocol.md).

### Dependency direction

Domain crates must not depend on platform, database, terminal, or network implementations (ADR 013). `vibemux_harness` is pure logic (no I/O); detection inputs are injected by the daemon, which reads the trusted `vibemux_probe` cache. The agent/launcher/probe-state vocabulary lives in `vibemux_harness` and is re-exported by `vibemux_probe`, which depends on it. The harness dispatch request, route config, launch argv, protocol session machines, framer, capture budget, and attempt phases are also pure `vibemux_harness` logic; `vibemuxd` performs every effect (ADR 029). The dual-track workflow contracts in `vibemux_workflow` depend only on `vibemux_harness`; `vibemux_store` applies them inside writer transactions, and `vibemuxd` performs every effect (ADR 031). The decision port in `vibemux_decision` depends only on `vibemux_types`, `vibemux_events`, and `vibemux_harness`; `vibemux_store`, `vibemuxd`, and `vibemux_decision_peer` depend on it, and `vibemuxd` does not depend on the peer crate (ADR 034). The normative layering from `AGENTS.md` §4.2:

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
vibemux_client
  ↑
vibemux_frontend / vibemux_desktop
  ↑
vibemux-cli
```

With the current crates placed in that layering: `types → events → harness → workflow / decision → store / workspace / platform / a2a / probe → plugin-api → plugin-host → vibemuxd → vibemux_client → vibemux_frontend / vibemux_desktop → vibemux-cli`. `vibemux_decision_peer` sits beside `vibemuxd` at the process level and depends only on `vibemux_decision`. `vibemux_desktop` is the Tauri shell (ADR 043): it drives `vibemux_client` and exposes a fixed command list to a locked-down webview that never holds a daemon credential or a filesystem path.

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

`vibemuxd::WriterWorker` constructs and exclusively owns `SqliteStore` inside a dedicated blocking thread. Callers only hold a bounded queue handle; when the queue is full, an explicit backpressure error is returned. The nonce-bearing lock next to the database is established through an atomic `create_new`; a second writer fails closed. After shutdown, the holder verifies the nonce before deletion. That check reads at most 256 bytes of a regular file, so a link found at the path, an oversized file, or another nonce reads as not held and is left in place.

Schema 14 adds the private workspace-cleanup ledger and operation index
([ADR 066](adr/066_explicit_worktree_removal.md)). Plan records a nonreserving,
five-minute preview; fresh Confirm atomically validates the settled Finalized
owner and reserves the resource/path/branch. Dispatch, chat, owner, workflow
lease and A2A admission enforce reciprocal fences. Separate linear registration,
Arm, release and completion tokens prevent replay from returning execution
authority. Completion retires pending release deliveries before writing evidence,
and definite storage errors retain only the retired proof for retry. Unknown
reserves the resource until positive durable quiescence and explicit settlement;
reconciliation performs no execution. Cleanup writes leave historical owner,
session, Task and Run rows unchanged. Current directory presentation checks active
cleanup and historical Removed tombstones, while metadata reads retain the branch.
The 64 KiB private records and their indexed columns are validated together;
events omit paths, tokens and object identities. The Windows service now uses
retained immutable image/ancestor pins, live-held process audit, fixed no-force
Git argv and joined root/tree/pipe evidence. A bounded evidence custodian keeps
retired proof across writer queue/read failures; only the executing writer takes
custody. Explicit reconciliation flushes that proof and observes postconditions
without replaying deletion. Status remains read-only. Control v16 and Main-only
native command 32 expose review/confirm/status/reconciliation; the native UI
mount requires a verified Finalized Workspace. Local owned-Git and mock-native
checks are separate from pending native desktop/live-terminal acceptance.

The narrow Windows file-identity adapter ([ADR 068](adr/068_windows_file_identity.md))
queries `FILE_ID_INFO` from an already-open borrowed handle. It preserves the
full volume serial and 128-bit identifier, rejects unsupported/zero results and
redacts diagnostics. It is the third reviewed unsafe module alongside ACL and
Job containment. It opens or mutates no path and supplies no process/restart
authority; callers still own immutable executable pins and physical validation.

`vibemuxd::control` uses a per-instance named pipe on Windows and a randomized per-instance Unix-domain socket on POSIX. The listener is bound before the Git-ignored runtime descriptor containing the protocol version, endpoint, and OS-CSPRNG bearer token is published. The four-byte big-endian length prefix enforces a 64 KiB ceiling before the JSON payload is allocated, and connection reads/writes and peer-close have deadlines. The descriptor owner token also guards descriptor/socket cleanup; an old instance cannot delete the runtime artifact of a replacement. IPC v17 is current in this draft. v2 added read-only `plugin_status`, v3 added `harness_refresh`/`harness_snapshot`/`harness_switch`, v4 added bounded read-only `frontend_tasks`/`frontend_task` queries plus `terminal_inspect`/`terminal_link`/`terminal_focus`/`terminal_unlink` observation operations (see [terminal observation protocol](terminal_observer_protocol.md)), v5 added the six `harness_dispatch_*` operations, v6 added the 15 `workflow_*` operations, v7 added the six `dsh_session_*` operations (see below), v8 added the four `decision_*` operations (ADR 034), v9 adds `workflow_supervisor_start`/`workflow_supervisor_status` (ADR 037), v10 adds explicit operator `workflow_contract_edit` (ADR 039), v11 adds the persistent-chat operation (ADR 057), v12 adds lifetime chat ownership, and v13 adds owner execution (ADR 058), v14 adds interactive approval, v15 adds checked workspace presentation reads (ADR 065), v16 adds explicit owner cleanup (ADR 066), and v17 adds the private `chat_command_progress` read (ADR 072). The progress increment has incomplete authenticated-routing and end-to-end acceptance. Requests from v1 through v17 remain accepted at their operations' minimum versions. A client that predates the daemon's version refuses its descriptor, so `vibemuxctl` and `vibemuxd` are upgraded together. Shutdown acceptance signals in-flight harness dispatches and probes before the reply, then joins dispatch tasks and plugin cleanup before writer shutdown. Blocking writer calls are isolated through `spawn_blocking`; no async mutex guard is held across an await. There is no TCP fallback.

DSH session operations require Control v7, decision operations require Control v8, model-supervisor start/status requires Control v9, operator contract edits require Control v10, the persistent-chat operation requires Control v11, lifetime chat ownership requires Control v12, owner execution requires Control v13, and established workflow operations retain v6.

The `vibemuxd` binary is an unprivileged foreground process owner; `vibemuxctl daemon start|health|stop` is responsible for the on-demand lifecycle. Readiness must come from authenticated health; the PID is only used to compare this spawn's identity. Windows uses a fixed, non-user-code-interpolated system PowerShell companion that holds the exact process handle and severs captured-stdout inheritance; POSIX uses direct argv spawn with a separate process group. During migration the path layer is fixed to `.vibemux/vibemux_rust.sqlite3`, and runtime symlinks and symlink/hardlink aliases that point at the Python database are refused.

Control v7 adds the six DSH session operations (ADR 032 §9) to the v6 surface.
The allowlisted remote-error namespace set includes `dsh_`, so a DSH failure
reaches a client as its own actionable code rather than collapsing to
`control_remote_error`.

`vibemuxctl daemon inspect` first attempts authenticated health and then reads bounded, Debug-redacted descriptor/writer-lock snapshots. It returns the domain-separated SHA-256 confirmation only when every recorded PID is absent, recorded identities agree across the artifacts that are present, and all present artifacts parse successfully. Descriptor-only and lock-only stale states can also be recoverable. `daemon recover` re-checks the confirmation, PID, and original bytes, and then removes only unchanged descriptor/UDS socket/writer lock; there is no force flag, no PID kill, and no database operation. Python database cutover remains unimplemented; same Windows logon SID is the collaboration trust boundary; inter-process ACL isolation is not promised.

Windows control metadata now lives under `%LOCALAPPDATA%\VibeMux\runtime\<domain-separated project hash>`. The protected parent DACL and the leaf/file effective ACL contain only the current SID, `SYSTEM`, and Administrators; the named pipe explicitly sets `PIPE_REJECT_REMOTE_CLIENTS` and continues to require the bearer token. New daemons hold both the protected lock and the legacy project lock as an upgrade compatibility gate, preventing an old binary from racing a new binary for the same SQLite writer. A healthy legacy daemon can still be queried and stopped; stale legacy artifacts still flow through the same confirmation recovery. Each start creates the per-project leaf. After a clean shutdown of a trusted start, the daemon removes its owned descriptor and writer lock and then removes the leaf. It removes the leaf only when the leaf is an empty real directory at its canonical path. Links, junctions, and leaves holding any other entry are left alone. The shared root and its `.acl_v1` marker always remain. Failed starts and crashes keep the leaf: `daemon inspect`/`recover` and the CLI's post-mortem ACL classification read it, and `recover` does not remove it. Leaf ownership is bound to one daemon generation through the project-local compatibility lock: only its holder may create, validate, or remove the leaf. A start acquires that lock before it creates the leaf, and the writer then acquires the protected lock inside the leaf. A clean shutdown runs in reverse order: it joins the writer, which removes the protected lock; removes the descriptor; releases the leaf, but only while the compatibility lock still carries this generation's nonce; and removes the compatibility lock last. A start that finds the compatibility lock held fails with `writer_lock_held` before it touches the leaf. A start that runs concurrently with another client's stop fails closed in the same way, or with `daemon_control_runtime_unavailable` when the release removes the leaf during its pre-lock `ensure_runtime_dir`, and it can be retried. A stopping daemon therefore never removes a leaf that another start is using. `vibemuxctl daemon stop` waits until the compatibility lock is gone and the daemon process has exited. If a failed start leaves no leaf, the CLI's post-mortem reports a plain `daemon_start_failed`, while every surviving path still has to verify. POSIX path semantics, `0600`, and UDS behavior are unchanged; on POSIX the runtime directory is the project state directory and is never removed.

## Plugin protocol and supervisor

`vibemux_plugin_protocol` is the isolated M4 wire boundary: the checked-in Protobuf schema is generated with pinned Prost and vendored protoc; the stdin/stdout frame uses four-byte big-endian length with a fixed ceiling. The manifest only stores argv, kind, platform, capabilities, and requested permissions; the core negotiation grants only the intersection of manifest declarations, the Hello request, and core policy. The codec and state machine alone are implemented: it does not spawn a child, does not read stderr, depends on no database, and does not map opaque payloads to Task/Run mutations.

`vibemux_plugin_supervisor` owns a single child on top of the protocol: canonical executable/cwd, argv, clean environment, Windows hidden process group / POSIX process group, bounded stdin/stdout channels, and an independent stderr drain. Handshake, receive, and shutdown have deadlines; queue full, heartbeat, cancel, drain, and exit/crash are all structured. The daemon owns a bounded in-memory registry with lifetime restart budgets, backoff, quarantine, and read-only status. Explicit startup configuration is optional; default startup loads no plugins. There are no real vendor plugins or canonical-state mutations. See [ADR 023](adr/023_daemon_plugin_registry.md) and the [registry protocol](plugin_registry_protocol.md).

`vibemux_terminal_observer` is a bounded WezTerm observer plugin. It uses an optional versioned terminal observation payload inside the unchanged v1.0 envelope; the daemon routes each request only after checking the plugin's effective terminal grant (startup configuration v2; v1 grants nothing), session, request/correlation identity, deadline, and response size. Run-scoped pane links are observation-only and never imply execution ownership or task completion. See the [terminal observation protocol](terminal_observer_protocol.md) and [ADR 028](adr/028_supervisor_chat_frontend.md).

The optional `terminal:cleanup_inventory_v1` method separately validates the
complete bounded configured-instance inventory and matches real directory
ancestry, including descendants. It fails on unknown evidence and never falls
back to exact-directory inventory. A default-false observer config flag, manifest
declaration and Core policy/session grants preserve existing configurations.
This read-only foundation ([ADR 067](adr/067_terminal_cleanup_inventory.md))
changes no Control/store/native-command contract and grants no deletion
authority. The separate Windows cleanup service consumes it under
[ADR 066](adr/066_explicit_worktree_removal.md), together with canonical ownership,
immutable executable pins and joined process evidence.

## Project conversation search

The native command palette builds a local catalogue from registered projects,
loaded Saved sessions, loaded Workspace owners, cached titles, retained Asks and
Task metadata. Workspace snapshots copy the bounded per-project owner/title
catalogue without fetching content. Display labels are shortened while title
matching uses the full cached title. The explicit history-search action handles
records outside those loaded lists; the native `quick_search` stub is not used.

`palette_navigation` rechecks registration, missing-folder state, cached target,
project selection and caller lifetime around existing read selectors. App
admission serializes navigation and suppresses late presentation. Existing
read-only reconciliation may settle an uncertain operation from a positive
receipt; it never resends. Workspace acknowledgement consumes current input only
when both the raw draft and complete formatted attachment payload still match
the immutable submitted prompt. See the
[palette guide](native_harness_conversations.md#command-palette-conversations).

`vibemux_client::chat_history::HistoryClient` owns authenticated normalization for
Saved and owner-backed histories. Export and `chat_search` are its two consumers;
each owns its snapshot or paging policy. Search composes existing list/status/
title/content operations without an index or database reader. Bounded literal
matching produces private previews, exact record targets and option-bound
continuation. Source changes and unavailable content remain explicit gaps.

The native `chat_search_request` command resolves the registered project and
owns one scan with an atomic guard independently of daemon jobs. The renderer's
modal reads only on Search/Load more and clears private results on close. Opening
a retained result uses existing read-only controllers, exact generation/version
and record identity, final status and selection epochs. It preserves drafts and
pending mutations; it cannot start, resume or cancel execution. See
[ADR 061](adr/061_native_history_search.md) and the
[search evidence](evidence/native_harness_m1.md#project-conversation-search).

Record reveal and transcript Find share `transcript_scroll::reveal_transcript_element`.
Only the nearest marked Ask, Saved or Workspace scroll container moves; the
application shell and header do not receive a scroll request. A real browser
regression reproduces the former ancestor-scroll failure and checks the scoped
replacement. A renewed native check also retained the header after Saved,
archived and Workspace deep navigation; the browser fixture remains separate
evidence.

Header, shortcut and palette Find share one selected-transcript gate.
Saved/Workspace eligibility checks registered project/chat/session identity and
purge state. Closed or finalized
Workspace history needs no live process. Workspace Find is keyed to its target;
the owner context sits inside the bounded transcript, with a separately scrolling
fallback when execution detail is unavailable. Saved consent copy shares its
transcript scroller. Retained history puts inactive drafts behind a disclosure
whose user state resets on context entry, independently of draft edits. Existing
controllers retain the text/files and all consent/mutation authority.

`task_windows` owns a process-local, bounded reservation registry for independent
task views. `task_window_open` authenticates the task before creating a native
label; reads check that label's registration, canonical project/task identity
and generation. Only `main` retains the full command capability. Auxiliary
`task_*` views receive Detail reads, public settings reads, the closed
terminal-only `supervisor_action` and bound worktree `open_link` actions, load a separate frontend entry and share the
docked detail presenter. Window construction uses
the trusted launcher profile and navigation guard. Closing a view releases its
reservation without execution changes. See [ADR 063](adr/063_read_only_task_windows.md).

`terminal_bridge` owns bounded per-window observations for the shared Task
Terminal tab. Native code fixes the WezTerm plugin identity, validates the actual
caller, registration and canonical Task/Run, and composes existing Control v4
Inspect/Link/Focus/Unlink operations. Replies use the path-free terminal
projection. One current observation plus one cleanup receipt per view preserves
explicit Unlink of the exact reviewed lease after failed effects or lost replies;
Inspect echoes the last received token to acknowledge it. Link/Focus authority
is retired before submission, and close/revocation clears cached authority.
There is no automatic terminal input, pane creation or plugin startup.

The observer resolves Workspace chat-owner Runs through the writer's indexed,
fully validated owner read, or uses the existing A2A binding when no owner exists.
The shared physical validator checks existing managed directories, Git inventory,
branch, receipt and resource identity without creating a namespace. Observation
permits dirty or advanced owned worktrees; execution keeps its empty-grant and
base-HEAD admission checks. See [ADR 064](adr/064_task_terminal_observation.md).

## Native worktree and HTTPS presentation

Control v15 adds the authenticated `workspace_read` boundary. The single writer
loads complete Workspace-owner or A2A authority and validates canonical
Project/Task/Run agreement. Metadata mode returns a complete bounded recorded
branch without filesystem access. Directory mode checks eligible ownership,
noncreating managed-worktree inspection and the exact physical Run slot, then
reloads current authority after I/O. Verified, settled Open and Finalized owners
are browseable; recovery projections are refused. Terminal observation remains
Open-only and execution keeps its separate admission policy.

The existing native `open_link` command accepts a closed HTTPS/worktree enum.
Tauri supplies the actual caller label. Main-only HTTPS uses a bounded parsed URL;
Task children can request only their bound worktree presentation actions. Folder
and path actions retain native directory/ancestry handles across a second
complete Core read, compare its result and recheck file-object identity before
the effect. Windows handles deny delete sharing through handoff. This does not
freeze in-place filesystem or Git mutations and is not an OS sandbox.

`link_effects` owns default-application handoff and one lazy, serialized text
clipboard. Only the native host receives paths and complete branch values;
renderer replies contain scope and acknowledgement. `WorkspaceAccessState`
provides shared explicit controls in Task and Workspace views. `NativeLinks`
routes transcript anchor clicks through the same closed command, prevents
WebView navigation and suppresses queued/late work after disposal. Neither flow
automatically retries an effect. Fixture mode is inert. See
[ADR 065](adr/065_native_links_and_worktree_access.md).

## Native private conversation export

The desktop's `chat_export_request` command composes the existing authenticated
Chat/Owner/Execution reads through `vibemux_client::chat_export`. Collection owns
generation/version and full-session consistency checks, UTF-8 cursors, canonical
metadata and private-content gaps. Separate pure formatters produce bounded
schema-1 JSON or readable text. The native Save boundary owns the selected path,
checked ancestor handles and exclusive file creation. The renderer receives only
receipt metadata, and one export operation is independent of the project's
daemon job. Cancellation of the Save dialog reads no history. This adds no
daemon Control operation, database access or canonical event mutation; see
[ADR 060](adr/060_native_conversation_export.md) and its
[execution evidence](evidence/native_harness_m1.md#native-conversation-export).

## Harness dispatch (ADR 029)

`vibemuxd` can run one structured vendor turn per request and record its outcome canonically. The pure protocol logic lives in `vibemux_harness::dispatch`; the daemon owns config loading, Git `HEAD` reads, process launch, pipe I/O, deadlines, cancellation, the in-memory transcript, and every writer call. Store schema 4 adds the versioned `harness_dispatches` record, written only by `WriterWorker` in the same transaction as its event.

- **Opt-in.** Dispatch is disabled unless the operator creates `.vibemux/harness_dispatch.json` (schema 1). The daemon reads it once at startup and pins its SHA-256; a missing or invalid file makes dispatch operations fail with a fixed code without blocking startup. Routes name an absolute vendor executable (a `.exe` on Windows, outside the project root, whose file stem is the harness command name), an environment-variable allowlist, and an optional model. The config carries no operator argv; every argument comes from the protocol profile.
- **Protocols.** Codex `exec --json` and `app-server`, and Claude stream-json, can execute. ACP (OpenCode, Copilot, Grok) is initialize-only probe support; the config, admission, and launch builder all refuse ACP execution.
- **Gates.** A valid request, an enabled execution route, a harness `detected` in the persisted registry (ADR 013), and no other active reservation. At most one dispatch is active per project, and vendor processes run in the project root with read-only protocol profiles, which are not an OS sandbox.
- **Process.** The executor spawns the first-party `vibemux_launch_trampoline` with a cleared environment, contains it with `vibemux_platform::ProcessTree` (a kill-on-close Job Object on Windows, a process group on POSIX), then releases it to start the vendor CLI, so the vendor and its descendants are members from their first instruction. Prompts reach the vendor only as JSON-string data or stdin, never argv or a shell.
- **Held launch.** The private `spawn_held` / consuming `release_after` seam can
  retain containment across an asynchronous authorization callback before go.
  Its opaque failure retains the process for joined cleanup and offers no
  release retry. Normal spawn still auto-releases. Failed release now performs
  additional bounded Job/stdout checks before returning, with up to roughly
  four seconds of extra failure-path latency; legacy callers discard the
  stronger receipt. PID observation and Drop are not durable recovery authority.
  The Windows cleanup service binds the live held trampoline to immutable image
  handles, commits executor registration and Arm before consuming one release
  permit, and persists the joined receipt through a bounded evidence custodian.
  The custodian retains retired proof across definite queue/read/write failures;
  explicit reconciliation never recreates execution authority. Its shutdown
  flush deadline bounds waiting separately from the writer's owned join.
  See [ADR 066](adr/066_explicit_worktree_removal.md).
- **Outcome.** `RunStatus::Succeeded` requires a correlated structured terminal record and a clean exit (`StructuredAdapter` authority). Deadlines, cancellation, protocol violations, and capture overflow end the attempt with a fixed error code. Startup recovery moves interrupted attempts to a conservative state before the writer reports ready.
- **Capture.** Raw vendor records are kept byte-exact in a bounded in-memory transcript and are returned only by `harness_dispatch_output`; events, logs, status, and errors stay content-free (prompt hash and size only). Transcripts are lost on daemon restart.
- **Control.** `harness_dispatch_{catalog, probe, submit, status, output, cancel}` require Control v5; `vibemuxctl dispatch` is the thin client.

Live vendor CLIs have been exercised only through initialize-only probes on native Windows ([Stage 7 evidence](evidence/harness_dispatch_validation.md)); no live execution turn has run. See [ADR 029](adr/029_daemon_harness_dispatch.md) and [PROGRESS.md](../PROGRESS.md) for the remaining gates.

## Persistent native conversations (ADR 057)

The daemon can own a persistent Claude or Codex chat: a session aggregate that
survives a restart, keeps private history, and admits one canonical Task/Run
per turn. This is the accepted **read-only** scope. It is separate from the
one-shot Ask path, which is unchanged.

`vibemux_types::chat` holds the chat metadata types and their bounds: a
`ChatSession`, a `ChatTurn`, a `ChatRecord`, a `ContentRef`, the session and
turn state machines, and the fixed error-code vocabulary. `vibemuxd::chat_session`
holds the service; `vibemuxd::chat_writer` holds the metadata writer; and
`vibemuxd::control_chat` is the authenticated Control v11 adapter.

- **One bounded command owner.** The chat service serializes open, send,
  cancel, rename, close, archive, resume, purge, and private live-text reads
  through a bounded command queue with an explicit request deadline. A full
  queue refuses work instead of accumulating it.
- **Read-only startup.** Starting the chat service creates no private storage
  and launches no vendor process. Listing and status read committed metadata
  only.
- **Separate consent.** Opening a chat requires an explicit
  `consent_private_storage` grant on the request. It is distinct from the
  one-shot Ask consent. Project registration is an identity boundary rather
  than a consent of its own: it binds the app to a directory and carries no
  separate consent grant.
- **Private bodies.** Titles, prompts, and assistant output are stored as
  immutable private bodies referenced by an opaque blob id, a SHA-256 digest,
  and a byte count. A reference is never a path. The writer publishes a
  private body before the reference naming it commits, so a reference never
  points at missing content; a failed write keeps the body for a later
  cleanup pass.
- **Bounded reads.** Only explicit `title`, `content`, and `live` requests
  return text. A content cursor names the record after `after_sequence` plus
  a byte offset inside that record; a live cursor also binds a replacement
  revision. Text is delivered in chunks of at most 8,192 bytes, and a chat
  response is size-checked before it leaves the daemon.
- **Exact resume.** A Claude chat resumes by its exact stored vendor session
  id; a Codex chat resumes its exact stored thread id through the app-server
  protocol. A resume refuses when the route profile or workspace resource key
  changed since the chat was opened.
- **Per-turn Task/Run.** Each admitted turn carries its own `task_id` and
  `run_id` and the Git HEAD observed at admission. A chat never becomes a
  completed Task because one turn finished.
- **Replay safety.** Session operations bind to an operation id and sends bind
  to a request id. The writer consults the durable binding first, so a
  repeated send after a restart, or after a purge, returns the original
  receipt rather than creating a second effect.
- **Close and namespace ordering.** A close reserves its operation before it
  stops the process owner, so an interrupted close cannot leave a session that
  no operation id can address. Chat operation ids and one-shot Ask request ids
  are checked against each other in both directions, so neither namespace can
  be mistaken for the other.
- **Control.** The chat surface is one authenticated Control v11 operation
  carrying a fixed request enum; the `operation` tag selects `list`, `open`,
  `lookup_open`, `status`, `submission`, `send`, `cancel`, `turns`, `title`,
  `content`, `live`, `rename`, `close`, `archive`, `resume`, or `purge`.

The desktop chat surface opens the Conversation view on Chats and keeps the
one-turn Independent Ask as a separate, unchanged choice. The chat route is
chosen independently of the Ask picker. The saved-chat consent dialog covers
inference and private storage together, separately from Independent Ask consent.
A new send waits for the chat to become ready — bounded at 30 seconds, polling
status only with a 250 ms yielded interval — and then sends exactly one turn.
Saved chats are listed per project with rename, close, archive, resume, and
purge, plus **Show archived** and **Load more chats** for older pages;
history is read by paging and live text arrives as bounded, revision-bound
chunks. An uncertain operation retains its exact identifiers and offers a
read-only lookup or an explicit same-action retry; the desktop never
auto-replays. A late response preserves the selected chat's details and any
retained draft.

Saved and Workspace renderer controllers retire an assistant snapshot only
after a complete, error-free Assistant body for its canonical turn/attempt is
loaded. Other record kinds and incomplete/unavailable bodies do not establish
that handoff. Their existing bounded record lists suppress a late same-ID Live
response after handoff; no separate cache or wire field is added. Existing
scope/generation/request and purge resets remain authoritative. See the
[handoff evidence](evidence/native_harness_m1.md#live-assistant-handoff-repair).

Not part of this scope: writable chats, chat-owned worktrees, affirmative tool execution, and
in-chat approvals. See [ADR 057](adr/057_persistent_harness_sessions.md) for
the accepted scope, the allocated versions, and the compatibility boundary.

### Private command observations (ADR 070)

The Codex session adapter maps correlated completed command and file-change
items to private, bounded `ToolObservation` values. It deduplicates exact replay, retains
full-tail replay identity and carries sticky projection loss independently of
the vendor outcome. The actor forwards these observations before the terminal
callback. Optional display callbacks share one absolute batch deadline; after
uncertainty the actor retires tool capture for that turn and still forwards its
correlated terminal.

Saved and owner-backed sinks validate their actual generation/fence, durable
turn/attempt and acknowledged Codex binding. They reserve a fixed record UUID
and private body before append, then use the existing immutable private writer.
An unknown append cannot mint a replacement or become complete capture. Tool
bodies use a closed version-1 envelope under the existing Tool kind; no store,
Control or public metadata change is required. The shared collapsed card reads
only complete, untruncated Tool bodies and preserves literal legacy fallback.
It adds no execution or approval capability. See
[ADR 070](adr/070_private_command_tool_records.md).

[ADR 071](adr/071_private_file_change_records.md) adds a distinct version-1
file-change body while preserving command version 1. Both variants use the
same ordered ledger, ID namespace, record/byte budgets, callback deadline and
immutable writer reservation. Source file lists are validated before bounded
prefix selection; replay identity includes omitted entries and discarded
diff tails. File cards display opaque reported paths/kinds/moves/diffs and gain
no filesystem or approval authority. Find reveals selected text ranges, with
one transcript-owned vertical viewport for bounded literal content.

## Native harness request dispatch (ADR 040)

`vibemux_harness_native` is a second, independent dispatch path next to the daemon-owned one (ADR 029). It is a standalone vendor plugin and dispatcher over `vibemux_plugin_supervisor`: native JSON framing, conservative observations, the adapter over the shared protocol engine, and OS process ownership are separate modules, and it supports Codex app-server and exec JSONL, Claude stream-json, and ACP (OpenCode, Copilot, Grok). Complete vendor stdout records stay in bounded private memory; public reports contain only metadata. The dispatcher revalidates terminal evidence and configuration identity (SHA-256 pinning) without writing canonical Task/Run state, and it never talks to `vibemuxd`.

The two paths differ on purpose:

| | `vibemuxctl dispatch` (ADR 029) | `vibemuxctl harness` (ADR 040) |
|---|---|---|
| Owner | `vibemuxd` | the CLI process or a standalone binary |
| Canonical state | records Run outcome via the single writer | none |
| Config | `.vibemux/harness_dispatch.json` | `.vibemux/harness_config.json` (`examples/harness/`) |
| ACP | probe only | probe and dispatch |
| Crate | `vibemux_harness::dispatch` (pure logic) plus daemon I/O | `vibemux_harness_native` (I/O) over the same `vibemux_harness::dispatch` engine |

Both paths drive one vendor posture: `vibemux_harness_native` builds its argv with `launch_spec::structured_arguments`, runs its handshake through `ProtocolSession`, and classifies Codex, Claude, and ACP records with the shared `observation::classify`. Direct OpenCode, Copilot, and Grok JSON routes are replay-only. This crate classifies direct OpenCode records itself; direct Copilot and Grok records stay `opaque`. `dispatch::capability` derives, from `NativeProtocol`, which structured adapters exist for each harness and whether the daemon path can execute through them. The dashboard, GUI, TUI diagnostics panel and snapshot, and `vibemuxctl probe` text all display that table.

Their configuration files, error namespaces, and protocol versions are separate; do not conflate them with each other or with the A2A wire version `1.0` or the internal plugin envelope version. See [ADR 040](adr/040_native_harness_request_dispatch.md) for the trust boundary and [the core guide](guides/harness_core_guide.md) for commands and live limits.

## Decision port and harness routing (ADR 034)

`vibemuxd` can record an advisory decision about which harness should run a prompt. The pure question, facts, answer validation, local rules, fallback resolver, config schema, record, event drafts, and peer wire live in `vibemux_decision`; the daemon's decision service owns config loading, facts assembly, the peer process, and every writer call. Store schema 6 adds the `decision_records` projection, written only by `WriterWorker` in the same transaction as its `decision_recorded` or `decision_consumed` event.

- **Opt-in.** Decisions are disabled unless the operator creates `.vibemux/decision_config.json` (schema 1). The daemon reads it once at startup and pins its SHA-256; a missing or invalid file makes `decide route` fail with a fixed code without blocking startup, while listing, showing, and dispatching recorded decisions still work.
- **Options.** The harnesses with an enabled, executing ADR 029 route that the probe detected, plus `escalate`. Zero eligible harnesses is an error; one is decided locally.
- **Advice only.** A decision runs nothing. `decision_dispatch_submit` builds an ordinary `DispatchRequest`, runs every ADR 029 gate, and in one writer transaction checks that the decision is resolved, unconsumed, unexpired (15 minutes), and matches the harness, prompt digest, and dispatch-config digest before it admits the dispatch and marks the decision consumed.
- **Fallback.** An accepted external answer at or above the threshold, else the highest-priority local rule, else the eligible project default, else `escalated`.
- **External backend.** A decision-API call needs both `jev.enabled` in the config and `allow_external` on the request. The daemon then spawns the one-shot `vibemux_decision_peer` next to its own executable with a cleared environment plus the system variables and the one credential variable the config names, writes one request line of at most 16 KiB, and reads one result line of at most 16 KiB under a deadline; the child is killed on drop, and daemon shutdown kills and reaps it before the writer is released. Only enumerated facts (option ids, operator option descriptions, allowlisted labels, a prompt size bucket, the project default) leave the machine. The peer is the only decision-API network client: fixed HTTPS endpoint, no redirects, no proxy, a 16 KiB response cap, and at most one retry of 429 or 529.
- **Content.** Records and the `decision_recorded` event carry identities, the prompt SHA-256 and byte count, the dispatch- and decision-config SHA-256 digests, option ids, allowlisted labels, probabilities and confidences in integer millionths, model ids, and fixed codes. The `decision_recorded` and `decision_consumed` events hold the complete record, so the projection can be rebuilt from them. Views returned by `decisions list|show` omit the prompt and config digests. The prompt, the credential, and provider usage are never stored.
- **Control.** `decision_{advise, list, show, dispatch_submit}` require Control v8; `vibemuxctl decide|decisions` and `vibemuxctl dispatch submit --decision` are the thin client.

The external path is verified only against fixture peers and a loopback test server; no live decision-API call has run. See [ADR 034](adr/034_decision_port.md) and the [decision orchestration roadmap](decision_orchestration_roadmap.md).

## DSH session substrate (ADR 032)

DeepSeek Harness (`dsh`) is the eleventh registered harness. It differs from
the other adapter targets in one decisive way: it publishes a first-party
machine protocol — newline-delimited JSON-RPC 2.0 over stdio
(`@deepseek-ai/dsh-sdk-protocol`, served by `@deepseek-ai/dsh-sdk-jsonrpc-server`
in the `sdk` profile bundle) — and it serves many turns from one long-lived
process.

`vibemux_harness::dsh` is the pure half: the wire vocabulary with strict
bounded decoders, the client-side session state machine (handshake, one turn at
a time, notification correlation, teardown), the record classifier, and the
bounded record ring. It performs no I/O and reuses the ADR 029 JSON-line framer.

`vibemux_dsh` is the effect half: it verifies and canonicalizes the operator
configuration, spawns the vendor launcher pair (`node` native image plus the
`@deepseek-ai/dsh` entry script) through `vibemux_platform::ProcessTree`,
frames stdout, enforces the initialize and per-turn deadlines, and terminates
the contained tree on cancel or close. It depends only on `vibemux_harness` and
`vibemux_platform`.

Two rules shape the substrate:

- **Idle is not completion.** A `session.status: idle` notification is rendered
  as a diagnostic and never ends a turn. Only a correlated `turn/end` event
  with a recognized `reason.kind` produces a terminal outcome; an unrecognized
  one degrades to `unverified`, which is never success.
- **The notification stream is unfiltered.** DSH delivers every session in the
  runtime to every client, so notifications are matched against the session's
  own id and everything else is counted and dropped. `subagent.*` payloads are
  keyed by parent and child session ids instead, so they are never correlated
  through the session-id path.

The SDK protocol publishes no cancel and no session-close method, so
cancellation terminates the contained process tree and `close` asks politely
with `shutdown` before escalating. The runtime is contained for lifetime, not
confined: it is not a sandbox, and its own profile decides what it may read and
write.

`vibemux_frontend`'s `vibemux_dsh_tui` binary is the operator surface: it
drives `vibemux_dsh` directly, so an operator needs no daemon. It never opens
the core database, writes no canonical state, reuses the audited `tui::Theme`
registry (ADR 026), and is a separate binary from the read-only diagnostic TUI
because it owns a live session and an input loop.

`vibemuxd::dsh_session::DshSessionService` holds at most one live session per
project and exposes it through Control v7
(`dsh_session_{open,prompt,poll,status,cancel,close}`). It performs no framing,
containment, or record bookkeeping itself — those come from `vibemux_dsh` — and
owns only the control-frame page budget, because a poll page must fit the
64 KiB IPC frame even when the driver's own page bound is larger. A page's
status is built from the driver snapshot that produced its records, never
from a second read, so it cannot run ahead of them. The daemon
joins the session at shutdown, so no vendor process outlives it.
`vibemuxctl dsh` is the thin Control v7 client; a prompt comes from a file or
stdin, never from argv.

Not implemented: a canonical session record (a session lives in daemon memory
and is lost on restart), multi-session support, resume, and image prompts. No
prompt has been sent to a real DSH runtime. See
[ADR 032](adr/032_dsh_harness_adapter.md) and the
[DSH session protocol](dsh_session_protocol.md).

## Dual-track workflows (ADR 031)

`vibemuxd` can run a supervised two-worker coding workflow on one repository: `cooperate` (disjoint owned paths of one task, integrated together) or `compare` (two competitors on one task, at most one selected). The pure contracts, gates, broker rules, and optimizer live in `vibemux_workflow`. Store schema 9 additionally persists execution authority, model-call accounting, fenced controllers and outboxes, plan bindings, recovery checkpoints, supervisor executions/rounds/action intents, and immutable contract-edit provenance through the single writer. The daemon's workflow service owns every effect.

- **Opt-in and pinned.** The service needs `.vibemux/workflow_config.json` (schema 1), read once at startup and pinned by digest. The Git executable and the trusted verifier must be absolute and outside the project root; the verifier directory is hashed at startup. Without the file, workflow operations answer `workflow_unconfigured`.
- **Admission.** Prepare validates each TaskSpec candidate against the exact request bytes and narrows it with the operator policy. Each contract is rendered once; its identity pins the TaskSpec, policy, template, harness profile, and base commit.
- **Turns.** Each worker turn holds a fenced lease and is admitted through the existing dispatch admission inside the workflow transaction. It runs with a writable profile in its own owned worktree, which is not an OS sandbox. ADR 029's read-only profiles and one-per-project reservation are unchanged.
- **Gates.** The daemon collects each candidate from Git and admits only regular text files inside the owned paths. Review and trusted verification run on materialized copies of the collected bytes in fresh worktrees. Integration applies accepted candidates to a fresh worktree at the base commit and verifies it again. Only the writer's gate marks a workflow `accepted`; nothing is committed, merged, or pushed.
- **Context.** Worker checkpoints are claims. The broker binds messages to authenticated sessions and delivers them at turn boundaries. Bundles carry snapshot ranges and attributed claims, redacted and bounded.
- **Private state.** `.vibemux/workflow_state` holds candidate blobs, the opt-in content store, plans, ledgers, and outboxes. It is restricted to the daemon's user (Windows protected ACL, POSIX `0700`).
- **Control.** The 15 `workflow_*` operations require Control v6. Answers are content-free except `workflow_session_prompt`. `vibemuxctl workflow|slots|context|session|prompt` is the thin client.

The [ADR 036](adr/036_runtime_execution_authority.md) increment adds a bounded daemon AAG HTTP client and a candidate compiler. The compiler enters the existing deterministic admission path; it is not deterministic interpretation. Test startup can inject an opaque fixture gateway retaining its owned loopback listener. Production has no certified live factory: request-scoped upstream attestation is unavailable, so live calls refuse before HTTP submission. A local health/model fingerprint is only an observation. A `live` label cannot authorize a writable slot.

Each launch derives independent read, search, edit, write, and development-test grants from its admitted contract and purpose. Unsupported vendor profiles refuse; fixture profiles need a nonserialized guard bound to copies of the built worker. Attempts pin their effective route, profile, and grants. Review identity is checked against persisted attempts, and derived negative-control verification receipts cannot become worker candidates. Owned-path admission requires source-requirement or exact operator-policy provenance. Optimizer parameters without operational effects refuse evaluation and promotion while historical policy rows remain readable.

The native-controller runtime sends literal input only through an explicitly owned adapter. The writer commits a reservation and lets one outbox claimant submit it; uncertain delivery is not replayed. This increment supplies no production native terminal backend and exposes no native controller mutation over Control v10. Affirmative approval transport, standalone review mode, semantic prompt editing, and full active workflow recovery remain unimplemented. Checkpoint projection refuses missing attempt/controller joins, participant cursors, unsettled effects, or artifact ownership. It never modifies global context or compaction settings. Current evidence and limits are recorded in the [runtime enhancement evidence](evidence/runtime_enhancement_increment.md); inherited acceptance remains separate.

## Model workflow supervisor (ADR 037)

The operator selects the model driver explicitly with `workflow_supervisor_start`; ordinary `workflow_start` retains the deterministic coordinator. Both share the same supervised run registry, cancellation path and single writer. Configuration of a supervisor alias alone does not change the driver. Control v9 adds the start operation and bounded, content-free `workflow_supervisor_status`; existing DSH operations retain their v7 minimum.

The writer supplies a typed observation and atomically reserves a model call and round against its complete digest. Proposal validation keeps the model's original observed version and compensates only that round's own reservation; any other changed target fact refuses. Bounded JSON proposals commit action intents before effects and are claimed once. Per-task eligible slots, immutable plan/configuration/contract identity, fixed dispatch request IDs and post-claim owned receipts govern worker, reviewer, verifier and integration effects. Schema 8 adds three supervisor tables without rewriting old receipts. Unknown calls or effects retain their charges, require reconciliation and fence a new generation.

Supported actions inspect/list, reorder admitted tasks without widening contracts, assign one dependency-ready worker turn, request independent review or exact-candidate verification, select by existing deterministic gates, propose gated integration, pause/cancel and report blocked. Independent assignments run concurrently within the admitted width. Read-artifact/query-context actions retrieve bounded admitted bytes; message/share actions use writer-bound, scoped broker effects (ADR 038). After any applied private read or query, arbitrary model messages refuse and retrieved data can travel only through scoped shares. Native-handoff actions still return typed refusal. A negative-control comparison uses the existing deterministic driver. Malformed or policy-rejected responses share a persistent repair ceiling; restart and repair never reset call/decision budgets. A pause overlapping unsettled inference reports `supervisor_recovery_required`, while a quiescent pause remains resumable.

The fixture gateway drives real guarded worker/reviewer processes and trusted verifier/integration work, including observed worker overlap. These executions establish offline behavior only. Live gateway attribution, installed vendor certification, native terminal operation and the complete Version 2 scenario gates remain unqualified. See [ADR 037](adr/037_model_supervisor_execution.md) and [current supervisor evidence](evidence/model_supervisor_increment.md).

## Operator contract editing (ADR 039)

Control v10 and `vibemuxctl prompt edit --request-file ...` admit an explicit operator edit of a quiescent workflow. Exact parent version/contracts and original source bytes are required. Wording changes and authority narrowing produce an immutable successor with generation links; source, base, roles, routes and original hard requirements remain pinned. A broader task uses separate admission. One writer transaction retires the quiescent parent along legal phase edges, creates one successor and records content-free provenance. Active or uncertain effects refuse. Remaining allowances, spent task counters and the original time grant are preserved; replay compares the complete immutable successor/artifact payload. A crash during later immutable binding leaves the successor non-executable until exact replay restores its bindings. Schema 9 adds the provenance table; older binaries refuse it. See [ADR 039](adr/039_operator_contract_edits.md).

## Windows acceptance tool launch

The acceptance CLI and owned command/version-probe helpers check `TokenRestrictedSids` before native child creation. Restricted or unknown token state returns `BLOCKED`; it never retries under another token. Noninteractive error-mode bits are scoped to the runner and preserve existing bits. Initialization status `0xc0000142` remains an actionable typed failure, distinct from a missing tool. Read-only report validation launches no tools. See [Windows tool launch checks](windows_tool_launch.md) for the paired Ruff/Node observations and the direct-host launch boundary.

## Probe and unified frontend boundary

The fixed Windows `vibemuxd::local_probe` boundary runs the bundled diagnostic
CLI with empty argv through the existing held trampoline/tree owner. Immutable
bundle handles survive final verification and cleanup. Bounded stdout/stderr,
cancellation, root exit and actual EOF/Job-empty evidence precede publication.
The pure `vibemux_probe::snapshot::project_report` entry validates completed
metadata without file I/O and withholds paths/endpoints/arbitrary text. This
Rust boundary is locally verified with fixed children. The desktop AppState
owns one global job, 64 retained operation bindings and joined shutdown with one
absolute deadline. Main-only `probe_rerun` accepts closed start/status/cancel
requests and returns a schema-1 metadata snapshot. The Harnesses Local checks
panel uses explicit start and status-only recovery, preserves previous results
and fences uncertain work across project changes. Ordinary Refresh remains a
recorded-data read. Host/renderer tests and source review passed. A native QA
sequence observed completion, cancellation, status and prior-result retention
using the fixed NotRun runner; installed-tool qualification remains open.
See [ADR 069](adr/069_native_console_diagnostics.md).

`vibemux_probe` only produces versioned, content-free, read-only diagnostics: launcher/version, explicit provider endpoint, CC Switch health/aggregate telemetry, and the A2A self-test. Launcher verification is not the same as authentication/inference verification.

`vibemux_probe::snapshot` loads either a bounded, validated, redacted saved report or the fixed local probe, and the `vibemux_frontend::dashboard` module renders it (with an optional read-only local Git inventory from `vibemuxctl workspace status`) as a navigable dashboard behind `vibemuxctl ui` and the `vibemux_dashboard` binary. It starts no daemon, harness, or model turn. See [ADR 041](adr/041_unified_cli_and_workspace_layout.md). ADR 042 makes it a multi-pane console: `daemon_feed` owns one background thread that polls the running daemon's content-free reads (health, plugins, dispatch catalog, task list and detail, DSH status, workflow slots, decisions) through the `daemon_session` connection cache it shares with the GUI client, and publishes an immutable `DaemonView`; `daemon_model` projects and sanitizes. Both `daemon_feed` and `daemon_model` live in `vibemux_client` and are re-exported by `vibemux_frontend::dashboard`. `daemon_render`, `panes`, and `filter` are pure presentation. The diagnostic TUI re-runs the probe on a worker thread (`tui::reprobe`), the DSH surface searches its in-memory transcript (`dsh_tui::search`), and the GUI's shortcut table and quick switcher are `gui::shortcuts`, `gui::quick_switcher`, and the egui-free `quick_search`.

`vibemux_frontend` consumes probe reports and authenticated Control v4 snapshots; it never opens the core SQLite database. Its native Supervisor Chat workspace (ADR 028) has one main conversation, real task activity, secondary Agents/Settings/Diagnostics, and docked or independent task windows. The Claude light theme is the default for new profiles (ADR 030); Studio and the five dark themes remain available. Task queries run through the daemon writer channel and return bounded metadata, not prompt bodies. A separately configured WezTerm observation plugin may list matching native panes and focus an explicitly linked pane after identity validation. Native TUI content stays in WezTerm. There is no continuous coordinator chat adapter, harness launch/input forwarding, embedded terminal renderer, or terminal-text completion inference. Ratatui remains the diagnostic surface, with existing classic/high-contrast/mono/light/nord/gruvbox themes. Shared palette export and terminal theme generation remain governed by ADR 026. The `vibemux_dsh_tui` surface independently owns an SDK runtime through `vibemux_dsh`; it writes no canonical state.

## Stateful local A2A and supervisor

`vibemux_a2a` isolates official wire types and all three local transports from canonical state. Its bounded backend/runtime contracts authorize each subject, retain task identity, and supervise cancellation and subscriptions. The daemon injects a non-owning `WriterHandle`; schema-2 transactions validate state versions and verifier receipts before committing Task/Run bindings plus events. Optional `vibemux_model_peer` processes read selected CC Switch profiles read-only and return data artifacts. `vibemux_workspace` binds role worktrees to private owner receipts. The current supervisor recipe does not execute generated code or provide terminal/harness parity. See [ADR 024](adr/024_stateful_a2a_supervisor.md) and the [supervisor protocol](a2a_supervisor_protocol.md) for limits and startup commands.

## Python and CLI boundaries

The Python package (`src/vibemux/`) is the behavior reference and complete prototype entry point. It owns Mock, a WezTerm command adapter, a tmux adapter, native/POSIX-compatible execution modeling, Git worktree management, an append-only SQLite event log, safe diff/stop/cleanup primitives, an offline mock harness, resource ownership, and reconciliation. ACP/MCP vendor adapters, ConPTY, the scheduler, and automatic merge/push are not implemented. Python and Rust use separate databases (`.vibemux/vibemux.sqlite3` and `.vibemux/vibemux_rust.sqlite3`), preventing an accidental dual-writer cutover.

## Linked authorities

- Engineering rules: [AGENTS.md](../AGENTS.md)
- Implementation status and evidence: [PROGRESS.md](../PROGRESS.md)
- Architecture decisions: [docs/adr/](adr/)
- Harness dispatch: [ADR 029](adr/029_daemon_harness_dispatch.md)
- DSH harness adapter and session substrate: [ADR 032](adr/032_dsh_harness_adapter.md)
- DSH session protocol: [docs/dsh_session_protocol.md](dsh_session_protocol.md)

- Dual-track workflows: [ADR 031](adr/031_dual_track_workflow.md)
- Platform support matrix: [docs/platform_support.md](platform_support.md)
- Wire/protocol boundaries: [docs/protocol_boundaries.md](protocol_boundaries.md)
- Plugin registry protocol: [docs/plugin_registry_protocol.md](plugin_registry_protocol.md)
- A2A supervisor protocol: [docs/a2a_supervisor_protocol.md](a2a_supervisor_protocol.md)
- A2A conformance procedure: [docs/a2a_conformance_validation.md](a2a_conformance_validation.md)
- User entry / quick start: [README.md](../README.md)

## Optional browser-local conversational workspace

`crates/vibemux_chat/web` is an independent static Svelte app with Claude and
Codex presentations. It stores versioned conversation records in its own browser
namespace and sends explicit chat/API-validation requests directly to the fixed
Anthropic HTTPS origin. Provider keys remain only in page memory. It has no
control descriptor, database writer, canonical Run state or harness execution.
The read-only monitor and native desktop retain their existing boundaries.
[ADR 056](adr/056_browser_local_chat.md) records storage, preview and provider
limits; [the guide](conversational_web_ui.md) gives startup commands.

## Native desktop one-turn Ask integration

Native conversation M1 connects the fixed Tauri command boundary to
`vibemux_client::ask` and authenticated daemon dispatch. Both visual
presentations share project/route/Ask ownership. Desktop Rust owns registered
opaque project handles, identity-bound consent, submission deduplication,
continuously drained worker collectors and bounded display snapshots; the
daemon remains the canonical Task/Run/Event and process owner.

A local receipt is separate from canonical admission. Status, observer health,
terminal evidence and output completeness are independent; lost observation
cannot invent success or cancellation. Display retention is 32 Asks per project;
only settled entries may be evicted, with active/uncertain ownership protected. The separate 1,024-key
process-lifetime deduplication budget refuses new work when full. Renderer
reload recovers Rust-held state without submitting again; desktop exit does
not preserve native text or replay bindings. Daemon raw output remains bounded
memory-only under ADR 029.

The command path uses the existing Control protocol and no new listener or
store migration. Claude stays tool-free/one-turn and Codex configured
read-only/ephemeral/one-turn. The standalone browser API app and read-only
mobile monitor retain their own boundaries; egui remains pending its separate
parity/removal gate. Implementation and exact validation status are recorded
in [native M1 evidence](evidence/native_harness_m1.md), the contract amendment
in [ADR 043](adr/043_tauri_desktop_app.md), and setup/retention behavior in
[the native guide](native_harness_conversations.md).

The native desktop now also mounts **Tasks** and **Console** read surfaces, the
settings page, search, and the shortcut set. Their renderer contracts are
fixed and bounded in the desktop crate: a task page returns at most 32 tasks, a
console section at most 64 rows, a decision page at most 32 entries, and only
the Decisions section accepts an audit-sequence cursor. Task rows carry an
optional task id so the console can offer read-only navigation to a task
detail.

Persistent Claude/Codex sessions are accepted for a **read-only** scope in
[ADR 057](adr/057_persistent_harness_sessions.md), with Control v11 and store
schema 10 allocated. The daemon backend and the desktop chat surface are
implemented and locally tested. Bounded native navigation, settings and draft
checks passed. Claude 2.1.304 and Codex 0.166.0 passed bounded follow-up and
restart-resume checks. Bounded native Saved, archived and Workspace history
interaction has also been observed. A bounded Workspace JSON save and native
task-window lifecycle check passed in the isolated Windows fixture.
The writable profile, affirmative tool/approval path, image/PDF inputs and
remaining platform/live qualification are open. This remains a pre-release.

The desktop imports bounded UTF-8 text/code snapshots through the native
`chat_pick_files` command. The per-chat draft owns the selected contents and
encodes them into the existing private prompt on Send. The command accepts
an opaque project key and obtains file paths only from the native dialog.
See the [attachment contract](native_harness_conversations.md#native-text-attachments).

### Lifetime chat ownership

`chat_session::owner_api` and `control_chat_owner` expose the Control v12
ownership API. `owner_service` supervises provisioning through an actor-owned
`JoinSet`. `ChatWriter` commits owner metadata through the authoritative writer.
Schema 11 adds the lifetime Task/Run binding and workspace reservation.
`WorkspaceManager::plan` and `create_planned` separate durable intent from Git
effects, and restart recovery verifies the persisted ownership receipt.
The owner API admits empty grants. The desktop's typed `chat_owner_request`
bridge and Workspaces controller expose creation, bounded owner reads,
reconciliation and Stop. Pending mutations retain their request binding across
project navigation, and periodic refresh preserves the loaded pagination window.
Public owner submission is described below. Writable profiles remain a subsequent stage. See [ADR 058](adr/058_writable_chat_ownership.md) and
the [native guide](native_harness_conversations.md#lifetime-owner-workspace-foundation).

The schema-12 increment adds `chat_execution_attempts` and
`chat_execution_records` under that owner. `vibemux_store::chat` now has
owner-specific admission, one-use input claim, result, record and process cleanup
transactions. The private `attempt_rows` loader checks the immutable admission
binding and owner identity independently of the lifetime Run's later state.
The authoritative writer fences unfinished execution generations at startup.
`chat_execution_writer` reuses the private-body store and publication ordering.
`chat_session::execution_service` now serializes internal Start/Send/cancel/close
operations around the existing runtime actor. `ChatExecutionSink` maps its facts
to the owner-specific writer transactions, while `sink_state` shares bounded
private output bookkeeping with the read-only sink. Workspace receipt, inventory,
branch, resource and HEAD checks precede fresh effects. Receipt replay precedes
those new-work checks. Native synthetic-vendor tests exercise repeated turns,
exact resume and failure/shutdown paths without ending the lifetime Run.
`chat_session::execution_api` and `control_chat_execution` expose closed Control
v13 requests and allowlisted runtime, attempt and record metadata. Runtime status
reads owner/session/process-claim state through one writer closure. Content and
Live use bounded UTF-8 chunks without exposing private body references.
The registered-project `chat_execution_request` native bridge checks consent and
frame bounds before connecting. The mounted Workspaces controller retains one
exact uncertain action per target, polls read-only, and requires explicit
Start/Resume and Send. Process Close retains lifetime ownership; final Stop remains
a separate owner operation.

Accepted chat-writer callbacks now return their actual completion through a
crate-private Tokio oneshot on the existing bounded writer queue. The supervising
chat actor retains the dependent launch/input step when its client stops waiting.
The synchronous writer API and external acknowledgement deadlines are unchanged.
Post-commit shutdown checks release fresh process claims or cancel attempts before
vendor effects. This preserves the admission-to-execution handoff without retries.

### Interactive approval groundwork

`chat_session::input_writer` has an owned per-frame `InputReceipt`. It reports
`Written` after write/flush and `Unknown` after lost or interrupted completion;
legacy `send` remains queue-only. No production approval consumer exists yet.
The test-only Claude peer proves exact one-use response gating of its artifact
through a directly owned subprocess. The proposed authority, correlation and
durable-delivery contract is in [ADR 059](adr/059_interactive_chat_approvals.md).
Schema 13 now adds `chat_approvals`, linked to the lifetime owner and attempt,
with private evidence references, exact request/tool identity keys, decision
receipts in `chat_operations`, and a one-use outbox claim. Decision, claim and
audit event commit atomically. Only a fresh commit returns an opaque delivery
capability; replay and restart cannot reconstruct one. Pending invalidation and
claimed Unknown outcomes share cancellation/process transactions. Unresolved
approvals block ordinary terminal completion. Production profiles stay read-only
and Control now exposes the approval consumer at v14. `chat_approval_writer` publishes protected evidence
before metadata, resolves replay without filesystem effects and verifies private
identity/input snapshots before minting a fresh dispatch claim. It refreshes UTC
at actual decision commit after file reads. The runtime actor now consumes these
claims through a guarded `test_helpers` fixture factory. The closed Control
approval operation and mounted desktop panel consume existing offers; qualified
production writable profiles and their grant admission remain pending. A separate test-only field observer
prepares a diagnostic without launching a vendor or qualifying a native profile.

The fixture protocol is a fresh Claude session with one input and a fixed
WriteFiles effect. The factory binds the built vendor and trampoline, fixed
configuration, CWD, managed worktree receipt and pinned HEAD at launch and before
dispatch. No serialized setting selects this mode. Actor-owned futures preserve
admission, decision and delivery work after the client stops observing; a separate
cancellation slot keeps output and interruption responsive. Input flush precedes
the human offer, the one-use canonical claim precedes response enqueue, and
durable Written precedes terminal release. A human wait pauses idle expiry while
the original total-turn deadline remains active. Unknown delivery is never
resent. See the [fixture runtime evidence](evidence/native_harness_m1.md#guarded-fixture-approval-runtime).
`approval_diagnostic` is a separate `test_helpers` path. Candidate preparation
starts no vendor; explicit execution pins the source/trampoline and fresh empty
namespace, checks the installed version in its own contained process, and drives
one fixed input with deny-only responses. The initialization/user/denial receipts
remain owned while output, cancellation and deadlines progress. Reports expose
fixed observations and cleanup evidence, with policy and production qualification
false. Normal tests use guarded fixture copies; installed execution stays ignored
until separately authorized and reviewed. This path opens no Control or UI route.
Two separately authorized installed-Claude 2.1.304 inputs stopped at `init_mismatch`
with verified process cleanup and no fixed test artifact. The v2 observation
isolates the permission-mode mismatch; working directory and tools match.
Qualification remains false. Diagnostic report v2 preserves queue acceptance separately from flush
receipts and adds optional redacted initialization comparisons; strict mode,
working-directory, tool and identity validation are unchanged. The original v1
observation and subsequent synthetic repair have separate
[evidence](evidence/native_harness_m1.md#installed-claude-diagnostic-and-redacted-init-checks).
The later [v2 installed observation](evidence/native_harness_m1.md#installed-claude-report-v2-observation)
records the separately approved input and spent allowance.

The current test-only report is v3. Its observer recognizes only canonical SDK
`default` and the CLI/fixture `manual` alias as Manual mode. The old literal-only
predicate rejected the documented canonical value. Report fields, deny-only
delivery and the production `dontAsk` parser are unchanged. Historical v2 checks
cannot reveal the discarded native value. See the
[source contract and offline repair](adr/059_interactive_chat_approvals.md#canonical-manual-mode-in-diagnostic-report-v3).

### Test-only native GUI QA

The nondefault `native_qa` feature adds the closed `vibemux_desktop_qa`
developer binary. It prepares/checks synthetic history through the real daemon
and launches the same production Tauri builder with separate owned settings and
WebView data. A private Rust launch mode supplies the DEMO title; no renderer
selector or additional command is exposed. Fixed fixture images/configuration
remain pinned through daemon shutdown. Compatible directory handles and identity
checks support production atomic settings saves within an exclusive trusted
namespace. Observed external changes fail the run; this is not an OS sandbox.
The owned monitor, nonce stop requests and explicit daemon/Ask reconciliation
have separate report fields. Tauri Ready does not prove DOM/scenario readiness.
The Surfaces scenario retains 8 Saved, 2 archived and 4 Workspace records and
verifies complete private command/file bodies through the authenticated reader.
NativeQa startup supplies a fixed in-process diagnostic runner: Console checks
exercise progress, cancellation and retained results with every check marked
NotRun. Standard startup retains its ordinary runner. The fixture does not
launch installed diagnostics or qualify a production profile.
See [ADR 062](adr/062_native_gui_acceptance_fixture.md), the
[developer guide](native_harness_conversations.md#native-gui-qa-runner), and
[executed evidence](evidence/native_harness_m1.md#native-gui-qa-preparation).

### Public approval consumer and desktop review

Control v14 adds `chat_approval` with mapped metadata, explicit input-only UTF-8
chunks, human AllowOnce/Deny and read-only receipt recovery. Raw vendor identity,
blob references, profile fingerprints and delivery capabilities stay core-owned.
Historical replay reads the original operation binding and current delivery;
it cannot dispatch again or require a live actor. The service waits outside its
serialized command queue, and the native bridge avoids the exclusive project job
during that wait. Cancel/close still reach the owned runtime.

The desktop's 29th fixed command, `chat_approval_request`, reaches the mounted
Workspace approval panel through the typed bridge. Private input loads after
Review and clears on selection, close, expiry, error, forced refetch or decision.
Scope/detail epochs and current-row version checks suppress stale responses.
Clock rollback requires fresh calibration. The dialog focuses Deny and submits
only explicit choices; recovery reads receipts/metadata without resending. A
final canonical row can prove an uncertain operation was not recorded without
being presented as a successful decision. See the
[surface evidence](evidence/native_harness_m1.md#control-v14-and-desktop-tool-approvals).
