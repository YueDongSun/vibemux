# VibeMux Engineering Progress

> **Document role:** repository-wide engineering status and delivery plan.
> **Scope:** product goals, architecture migration, implementation status, quality gates, risks, and release criteria.
> **Out of scope:** personal learning plans, tutorials, agent-specific teaching prompts, and hidden reasoning.
> **Update rule:** every pull request that changes a milestone, public contract, protocol, persistence schema, platform support claim, or release gate **MUST** update this file.

---

## 1. Project Snapshot

| Field | Value |
|---|---|
| Project | VibeMux |
| Repository | `YueDongSun/vibemux` |
| Audited branch | `main` |
| Audited commit | `99d1f8ebcf9e25248eb2af1eb6d64b530e51f848` |
| Package version | `0.1.0a0` |
| Product target | Windows 10/11 first; WSL2/Linux development and compatibility |
| Current implementation | Python 3.12 executable prototype |
| Target core implementation | Rust 2024 edition |
| Current maturity | Executable architecture skeleton; **not yet a functional multi-harness alpha** |
| Current release posture | Pre-alpha; no stability or compatibility guarantee |

### Status vocabulary

| Status | Meaning |
|---|---|
| `VERIFIED` | Implemented and covered by repeatable automated or explicitly documented live validation |
| `PARTIAL` | A usable skeleton exists, but the contract, safety, or verification is incomplete |
| `PLANNED` | Accepted scope with an implementation path, but no production code yet |
| `BLOCKED` | Work cannot safely proceed until a named dependency or decision is resolved |
| `DEFERRED` | Intentionally excluded from the current release line |
| `REMOVED` | Previously present behavior that has been deliberately retired |

Progress is evidence-based. A feature is not `VERIFIED` merely because a class, command, or document exists.

---

## 2. Product Contract

VibeMux is a Windows-first, local-first coordination environment for multiple coding-agent harnesses. It must allow several agents to work against one logical Git repository while each run has an independent worktree, process, terminal location, state record, event stream, and reviewable output.

### Required long-term capabilities

1. Run OpenCode, Claude Code, GitHub Copilot CLI, Pi, Grok, Gemini CLI, and future harnesses through explicit adapters.
2. Keep each writable run in an independent Git worktree and branch.
3. Separate terminal presentation, process execution, harness control, sandboxing, and inter-agent protocol.
4. Maintain a canonical, replayable event stream independent of terminal text and vendor-specific protocol payloads.
5. Support local and remote A2A interoperability without making A2A responsible for local process, terminal, or worktree lifecycle.
6. Make Windows native operation a first-class product path; WSL must not be required for end users.
7. Keep optional integrations replaceable and independently versioned.
8. Default to inspectable, reversible, fail-closed operations.
9. Never infer task completion from PTY text, pane exit, or idle state.
10. Never automatically merge, force-clean, rewrite history, or push agent branches without an explicit future policy and user authorization.

### Explicit non-goals for the current release line

- A custom terminal renderer.
- A custom model runtime.
- Kubernetes or distributed consensus.
- In-process third-party native plugins.
- Automatic merge to the user’s primary branch.
- Treating Git worktrees as a security sandbox.
- Storing full prompts or secrets in the default audit log.
- Maintaining two independent state writers during the Python-to-Rust transition.

---

## 3. Accepted Architecture Direction

### 3.1 Decision

The final orchestration and A2A data path will be implemented as a long-lived **Rust core**. Optional and vendor-specific functionality will run as **out-of-process plugins**.

The current Python implementation remains valuable as:

- a behavior prototype;
- a compatibility oracle for CLI and state semantics;
- a temporary implementation while Rust parity is built;
- a future Python plugin SDK and integration host.

It must not continue growing into the authoritative A2A router, event broker, scheduler, or state writer.

### 3.2 Process topology

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

### 3.3 Core versus plugin boundary

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
| Harness-specific launch and structured protocol | No | Yes |
| Sandbox implementation | No | Yes |
| UI, notifications, GitHub integrations | No | Yes |
| Model-provider configuration helpers | No | Yes |
| Benchmark reporters/exporters | No | Yes |
| Direct database writes | Yes, single writer | Never |

### 3.4 Plugin model

V1 plugins are out-of-process executables. The project will not expose a Rust `cdylib` ABI as the public plugin interface.

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

### 3.5 External A2A transport policy

- External compatibility: JSON-RPC and HTTP+JSON/REST.
- High-throughput VibeMux-to-VibeMux path: gRPC when both Agent Cards advertise it.
- Streaming: transport-native streaming with bounded buffering and cancellation propagation.
- The core canonical event model remains independent of A2A wire objects.
- Internal `Task` and external A2A `Task` are related through an explicit binding table; they are not assumed to be the same object.
- Protocol types from the official Rust SDK must be wrapped behind a VibeMux-owned adapter crate so SDK churn does not leak through the whole codebase.

---

## 4. Current Repository Audit

### 4.1 What exists

| Area | Status | Current evidence | Assessment |
|---|---|---|---|
| Python package and Typer CLI | `PARTIAL` | `pyproject.toml`, `src/vibemux/cli.py` | Installable command skeleton exists; command surface is much smaller than the intended product contract |
| Windows-first declaration | `PARTIAL` | README, platform docs, CI matrix | Goal is explicit, but live Windows/WezTerm validation is not yet demonstrated |
| Domain models | `PARTIAL` | `models.py` | Task, Run, Event, TerminalLocation exist; lifecycle and fields are not sufficient for replay or review gates |
| SQLite persistence | `PARTIAL` | `storage.py` | State and events are persisted; migrations, WAL policy, richer IDs, and safe update semantics are incomplete |
| Git worktree manager | `PARTIAL` | `workspace.py` | Worktree creation and dirty cleanup refusal exist; base-commit and artifact semantics are incorrect/incomplete |
| Path safety helper | `PARTIAL` | `paths.py` | Common-path checks exist; Windows junction/reparse and Git ownership validation need full coverage |
| Terminal abstraction | `PARTIAL` | `terminal.py` | Mock, WezTerm, and tmux classes exist; project/task/run topology and ownership are incomplete |
| Harness profiles | `PARTIAL` | `harness.py` | Generic profiles and a mock adapter exist; no structured real-harness adapter exists |
| Canonical event log concept | `PARTIAL` | Event dataclass and SQLite event table | Events exist, but naming, correlation, migrations, replay projections, and invariants are incomplete |
| Spawn orchestration | `PARTIAL` | `RunService.spawn` | Basic worktree → terminal flow exists; no complete saga/compensation behavior |
| Reconciliation | `PLANNED` | Error type and design text | `status` currently lists records but does not reconcile terminal/worktree reality |
| Safe cleanup CLI | `PLANNED` | `WorkspaceManager.cleanup` helper | No complete immutable cleanup plan or CLI workflow |
| Tests | `PARTIAL` | one `test_core.py`, mock smoke | Core concepts are smoke-tested; security, integration, migration, platform, and live backend coverage are missing |
| CI | `PARTIAL` | Windows/Ubuntu matrix | Runs pytest, ruff, and mock smoke; no mypy, coverage gate, package build, Rust, TCK, or live backend gate |
| Rust workspace | `PLANNED` | none | Not started |
| Core daemon | `PLANNED` | current ADR rejects daemon for MVP | Must be introduced and the old decision superseded for the A2A/plugin architecture |
| Plugin protocol/host | `PLANNED` | none | Not started |
| A2A gateway | `PLANNED` | protocol mentioned only in docs | Not started |
| ACP/RPC/stream-json adapters | `PLANNED` | enum values only | Not started |
| Scheduler, artifacts, review gates | `PLANNED` | none | Not started |

### 4.2 Strengths worth preserving

1. The repository already states that Windows is the primary product platform.
2. `TerminalLocation` is backend-neutral rather than hard-coded to tmux.
3. User text is sent to WezTerm through stdin rather than interpolated into a shell string.
4. Worktrees are treated as collaboration isolation rather than a security sandbox.
5. Events are modeled separately from terminal output.
6. Cleanup refuses a dirty worktree in the current helper.
7. The project is already Apache-2.0 and organized as an open-source repository.
8. The initial commit is small enough that architectural correction is still inexpensive.

### 4.3 Critical defects and technical debt

#### P0 — must be resolved before adding real harnesses

- [x] **Persist the actual base commit for every Run.**
  `Run` currently does not store `base_commit`; `RunService.diff` reconstructs a record with `"HEAD"`, while `WorkspaceManager.diff` only runs a working-tree diff. Committed agent changes and replay semantics are therefore not represented correctly.

- [x] **Introduce one injected command runner.**
  Git, WezTerm, and tmux currently call `subprocess.run` directly. This prevents reliable contract testing, centralized redaction, deadline handling, process-group policy, and Windows launcher control.

- [x] **Remove shell-mediated tmux launch construction.**
  The current tmux backend converts an argv array to one command string. A controlled cross-platform agent host must receive a validated launch spec and spawn the harness with `shell=false`.

- [x] **Implement complete spawn compensation.**
  If terminal creation fails after a worktree is created, the current flow marks the Run failed but does not reliably unwind the worktree, branch, launch spec, or terminal resources.

- [x] **Stop fabricating missing mock panes.**
  The mock backend currently recreates a pane record during `send_text`, masking stale-resource behavior. Mock state must be persisted or run inside an actual supervised process.

- [x] **Implement terminal resource ownership.**
  A pane ID alone is insufficient. Stop/send/activate must verify project workspace, task container, run identity, backend, and recorded ownership before operating.

- [x] **Implement reconciliation before real concurrency.**
  The system must detect missing panes, missing worktrees, unknown resources, backend outages, and mismatched ownership without treating any of them as task success.

- [x] **Eliminate automatic success semantics from PTY runs.**
  `RunStatus.SUCCEEDED` must not be reachable from process exit or terminal evidence. Completion requires a structured adapter event or explicit verifier/user transition.

#### P1 — required for Rust-core parity

- [ ] Replace `INSERT OR REPLACE` state writes with explicit insert/update operations.
- [ ] Add migration history, schema versioning, WAL, busy timeout, and transaction tests.
- [ ] Add `updated_at`, `stopped_at`, `failure_code`, `base_commit`, `correlation_id`, `causation_id`, and idempotency fields.
- [ ] Make every state transition and corresponding event one atomic transaction.
- [ ] Expand task lifecycle to include review, verified, merged, failed, and cancelled semantics.
- [ ] Implement project → task → run topology for WezTerm workspace/window/tab/pane.
- [ ] Implement project → task → run topology for tmux session/window/pane.
- [ ] Replace process-local mock state with a real testable mock plugin and mock terminal inventory.
- [ ] Add immutable cleanup plans and fail-closed execution.
- [ ] Add task/run show, list, filters, manual transitions, trace filters, and JSON output contracts.
- [ ] Add Windows path tests for drive letters, case folding, spaces, Unicode, junctions, reparse points, and cross-drive refusal.
- [ ] Add package build, mypy, coverage, security, integration, and platform jobs to CI.
- [ ] Split the current monolithic Python modules before freezing them as the reference implementation.

#### P2 — required before public plugin API

- [ ] Versioned plugin manifest and protocol schema.
- [ ] Capability negotiation and permission model.
- [ ] Heartbeat, deadlines, cancellation, crash recovery, and restart policy.
- [ ] Plugin SDKs for Rust and Python.
- [ ] Protocol compatibility matrix.
- [ ] A2A conformance and cross-language integration tests.
- [ ] Structured telemetry with secret redaction.
- [ ] Artifact store and content-addressed references.
- [ ] Deterministic scheduler contract and conflict prediction input schema.

---

## 5. Repository Target Layout

```text
vibemux/
├── Cargo.toml
├── rust-toolchain.toml
├── crates/
│   ├── vibemux-types/          # IDs, domain objects, state machines
│   ├── vibemux-events/         # canonical envelopes, projections, idempotency
│   ├── vibemux-store/          # SQLite migrations and repositories
│   ├── vibemux-plugin-api/     # generated protocol types and manifest model
│   ├── vibemux-plugin-host/    # process supervision, handshake, routing
│   ├── vibemux-a2a/            # official SDK adapter and A2A mappings
│   ├── vibemux-workspace/      # Git worktrees and artifacts
│   ├── vibemux-platform/       # Windows/POSIX process and IPC primitives
│   ├── vibemuxd/               # long-lived local core
│   └── vibemux-cli/            # thin CLI client and daemon bootstrap
├── proto/
│   └── plugin/v1/plugin.proto
├── plugins/
│   ├── terminal-wezterm/
│   ├── terminal-tmux/
│   ├── harness-mock/
│   └── harness-*/
├── sdk/
│   ├── python/
│   └── rust/
├── python/
│   └── prototype/              # frozen initial implementation during migration
├── tests/
│   ├── contract/
│   ├── integration/
│   ├── conformance/
│   ├── fixtures/
│   └── performance/
├── benches/
├── docs/
├── AGENTS.md
└── PROGRESS.md
```

This layout is a target, not permission to perform a single destructive rewrite. Migration must preserve a runnable branch and pass parity tests at each step.

---

## 6. Milestones

### M0 — Audit baseline and architecture lock

**Status:** `PARTIAL`

Scope:

- [x] Public repository and Python prototype exist.
- [x] Windows-first direction is documented.
- [x] Basic Task/Run/Event, worktree, terminal, and mock concepts exist.
- [x] Add this `PROGRESS.md`.
- [x] Replace the current minimal `AGENTS.md`.
- [x] Add ADR: Rust core and out-of-process plugins.
- [x] Add ADR: supersede “no daemon in MVP”.
- [x] Add ADR: plugin wire protocol and compatibility policy.
- [ ] Create issues for every P0 item.

Exit criteria:

- Architecture decisions are explicit.
- Current claims match current evidence.
- No contributor can mistake the Python prototype for the final core.
- P0 defects have owners and acceptance tests.

### M1 — Freeze and stabilize the Python behavior reference

**Status:** `PARTIAL`

Scope:

- [ ] Split command runner, domain, storage, workspace, terminal, harness, and services.
- [x] Persist `base_commit` and correct diff semantics.
- [ ] Add migrations and atomic transition/event tests.
- [x] Implement reconciliation and immutable cleanup plan.
- [ ] Replace fake mock persistence with a supervised mock process.
- [ ] Add platform and security test suites.
- [x] Record stable JSON fixtures for domain, events, cleanup, terminal inventory, and send receipts.

Exit criteria:

- Python prototype passes Windows and Ubuntu CI.
- No shell-mediated user input path exists.
- Spawn failure leaves no unknown resource without an audit event.
- State and event invariants are enforced.
- The behavior fixtures are sufficient to test Rust parity.

Constraint:

- After M1, Python core receives correctness fixes only. New orchestration features target Rust.

### M2 — Rust workspace and canonical core

**Status:** `PARTIAL`

Scope:

- [x] Add Rust 2024 workspace with MSRV aligned to the official A2A Rust SDK.
- [x] Implement typed IDs and state machines.
- [x] Implement canonical event envelopes and error taxonomy.
- [x] Implement deterministic transition tests and property tests.
- [ ] Add structured tracing with redaction.
- [x] Add Windows and Linux CI for formatting, clippy, and tests.
- [ ] Add dependency policy and vulnerability checks to CI.

Exit criteria:

- `vibemux-types` and `vibemux-events` contain no terminal, Git, network, or SDK-specific types.
- No production panic path for untrusted input.
- State-machine parity fixtures pass.
- Public types have documented compatibility rules.

### M3 — Rust persistence and single-writer daemon

**Status:** `PARTIAL`

Scope:

- [x] SQLite migration framework and WAL configuration.
- [x] Atomic state + event transactions.
- [x] Idempotency keys, sequence ordering, projections, and replay foundation.
- [ ] On-demand user-mode `vibemuxd`.
- [ ] Windows named-pipe and POSIX Unix-domain-socket CLI IPC.
- [ ] Daemon lifecycle, lock, health, and graceful shutdown.

Exit criteria:

- Exactly one authoritative writer owns project state.
- Crash/restart preserves event order and projections.
- CLI can start, query, and stop the daemon.
- No unauthenticated local TCP control endpoint is required.
- Python and Rust are never concurrent writers to one database.

### M4 — Plugin protocol and supervisor

**Status:** `PLANNED`

Scope:

- [ ] Protobuf plugin protocol v1.
- [ ] Manifest, handshake, capabilities, permissions, and version negotiation.
- [ ] Bounded request/event channels.
- [ ] Deadlines, cancellation, heartbeat, shutdown, and crash reporting.
- [ ] Rust and Python plugin SDKs.
- [ ] Mock plugins for every plugin kind.
- [ ] Contract and fuzz tests for framing and malformed input.

Exit criteria:

- A plugin cannot directly mutate core state.
- Plugin crash cannot crash the core.
- Unsupported protocol versions fail with a structured error.
- Backpressure is observable and bounded.
- Plugin stderr cannot corrupt the wire protocol.

### M5 — Workspace and terminal parity in Rust

**Status:** `PLANNED`

Scope:

- [ ] Git repository discovery and clean-base validation.
- [ ] Worktree create/diff/cleanup/reconciliation.
- [ ] Windows path, junction, Unicode, and drive safety.
- [ ] WezTerm terminal plugin.
- [ ] tmux terminal plugin.
- [ ] Mock terminal plugin.
- [ ] Process-group and cancellation behavior on Windows/POSIX.

Exit criteria:

- Project/task/run topology is consistent across terminal plugins.
- Stop/send/activate validate ownership.
- Cleanup is plan-first and fail-closed.
- Live Windows WezTerm and WSL/Linux tmux smoke tests are documented and repeatable.
- No terminal evidence changes Task to completed.

### M6 — Structured harness plugins

**Status:** `PLANNED`

Implementation order:

1. OpenCode ACP or Gemini ACP.
2. Pi RPC.
3. Claude stream-json / Agent SDK subprocess protocol.
4. Copilot ACP.
5. Grok structured/headless protocol.
6. Generic PTY fallback.

Scope:

- [ ] Per-harness capability probes.
- [ ] Session create/resume/cancel/approval mappings where supported.
- [ ] Canonical event normalization.
- [ ] PTY fallback capability downgrade.
- [ ] Adapter contract tests and fixture replay.

Exit criteria:

- At least two structured harnesses can run concurrently.
- Approval and cancellation are not inferred from terminal text.
- PTY runs are visibly marked as reduced-reliability.
- Harness version changes are caught by contract tests.

#### M5.0 — Read-only probe and unified frontend shell

**Slice status:** `PARTIAL — PROBE AND FRONTEND SHELL VERIFIED` on 2026-08-24. Native CLI TUI attachment remains disabled.

Objective:

- Replace ad hoc machine probes with a stable Rust report and render that evidence in one Windows-first terminal dashboard while reserving, not recreating, every supported CLI's native TUI.

Probe contract:

- schema version and observation timestamp;
- platform and probe mode;
- per-agent launcher availability, resolved launcher kind, bounded version result, route classification, and safe endpoint host/port;
- local gateway listener and health result;
- allowlisted CC Switch request aggregates only: app type, request count, failure count, latest timestamp, status/model/latency summary;
- A2A loopback self-test result;
- explicit `verified`, `failed`, `unavailable`, and `not_run` states;
- no key, token, authorization header, cookie, environment dump, prompt, response body, repository content, or raw error payload.

Probe implementation phases:

1. Define `ProbeConfig`, versioned `ProbeReport`, stable error codes, size limits, and default-disabled inference mode.
2. Resolve Windows direct executables and same-name PowerShell companions without executing `.cmd` content.
3. Run bounded `--version` probes with `shell=false`; classify authentication/inference separately and never infer them from version success.
4. Parse only allowlisted endpoint/model fields from Claude, Codex, OpenCode, Grok, and Copilot configuration locations.
5. Detect loopback routes and CC Switch health; open `cc-switch.db` with SQLite read-only flags and fixed aggregate SQL.
6. Invoke the local `vibemux_a2a` self-test and include only correlation/status evidence.
7. Expose a JSON-producing `vibemux-probe` binary and fixture-backed unit/integration tests.

Frontend implementation phases:

1. Define a backend-neutral dashboard view model derived from `ProbeReport`.
2. Define stable `NativeTuiSlot` records for Claude, Codex, OpenCode, Copilot, Grok, and future harnesses.
3. Render overview, gateway, A2A, alerts, and native-TUI reservation panels through Ratatui.
4. Provide a deterministic in-memory render test and a non-interactive `--once` mode for Windows smoke/CI.
5. Keep interactive attach/launch actions disabled with an explicit `reserved` reason until terminal ownership, ConPTY, resize, focus, cancellation, and teardown contracts pass.
6. Later route slot activation through daemon control API and terminal plugins; the frontend never writes SQLite directly.

Expected owned files:

```text
docs/adr/016_probe_and_unified_frontend.md
crates/vibemux_probe/
crates/vibemux_frontend/
Cargo.toml
Cargo.lock
PROGRESS.md
README.md
CHANGELOG.md
```

Acceptance gate for this slice:

- A real local probe reports the five installed agent families without printing secrets.
- Claude and Grok are attributed to the detected CC Switch route when their safe configuration points to loopback; direct providers remain distinct.
- CC Switch health and aggregate telemetry are read successfully when available and degrade to structured `unavailable` otherwise.
- The A2A self-test shares the fixed information marker and shuts down cleanly.
- JSON serialization round-trips under a versioned fixture.
- The frontend render contains every agent, gateway/A2A status, and five native-TUI slots using a Ratatui test backend.
- Native slots are visibly `reserved` and cannot start a process in this slice.
- `--once` exits without leaving raw terminal mode enabled, child processes, listeners, files, or database writes.
- Rust format, Clippy, workspace tests, Python regression, and `git diff --check` pass.

Explicitly deferred:

- model inference smoke and paid request execution;
- login or provider mutation;
- prompt/response body capture or TLS interception;
- daemon control API and authoritative task mutation;
- real PTY/ConPTY attachment, native TUI embedding, focus/resize/input forwarding, and process teardown;
- graphical web/Tauri frontend, remote browser access, and multi-user authentication.

Rollback:

- Both crates are non-authoritative and have no daemon consumer in this slice.
- If live probing or terminal rendering cannot remain read-only and bounded, keep the ADR and tests, remove the unsafe path, and leave M5.0 `PLANNED`.

### M7 — A2A core gateway

**Status:** `PARTIAL`

Scope:

- [x] Minimal Agent Card generation for the loopback information-share capability.
- [ ] External A2A Task ↔ internal binding model.
- [x] Bounded Message/DataPart mapping for the M7.0 information-share contract.
- [ ] Task, Artifact, status, cancellation, and subscription mapping.
- [x] HTTP+JSON local loopback exchange through the official Rust SDK.
- [ ] JSON-RPC and HTTP+JSON interoperability.
- [ ] gRPC fast path.
- [ ] Streaming, backpressure, reconnection, and task resubscription.
- [ ] Authentication, TLS, URL validation, SSRF protection, and audit.
- [ ] Official conformance and cross-language integration tests.

Exit criteria:

- A VibeMux agent is discoverable and callable by an independent A2A client.
- VibeMux can call independent Rust, Go, and Python reference agents.
- All supported transports preserve task identity and cancellation.
- TCK/ITK results are archived as CI artifacts.
- Transport choice does not change canonical state semantics.

#### M7.0 — Basic A2A information-share slice

**Slice status:** `VERIFIED — LOCAL LOOPBACK ONLY` on 2026-08-24. M7 remains `PARTIAL`; no remote or production support is advertised.

Objective:

- Prove that two independent local VibeMux A2A peers can discover each other and exchange one bounded, non-secret information message through an official A2A v1 Rust SDK boundary.
- Produce implementation evidence without claiming task orchestration, persistence, remote deployment, authentication, streaming, cancellation, gRPC, or TCK conformance.

Information contract:

- sender-owned correlation ID;
- backend-neutral sender peer ID;
- information kind and UTF-8 text;
- optional small structured metadata map;
- receiver acknowledgement carrying the same correlation ID;
- no local repository file, environment variable, credential, prompt history, tool result, or artifact body is shared implicitly.

Authority boundary:

- The slice is an ephemeral read-only adapter and does not create or transition canonical Task/Run state.
- It does not open the Python or Rust state database and therefore cannot become a second writer.
- External A2A SDK objects are contained inside `vibemux_a2a`; core domain crates remain SDK-independent.
- Loopback bind is the only enabled runtime path in this slice. Non-loopback bind, TLS, and authentication remain fail-closed and unadvertised.

Implementation phases:

1. **R0 — source-of-truth research**
   - Record the current A2A specification version separately from Rust crate versions.
   - Inspect the official Rust SDK Agent Card, Message, Part, client, server, and transport APIs.
   - Pin compatible official crates that compile on the workspace MSRV; do not copy protocol types.
2. **R1 — VibeMux-owned information mapping**
   - Define a size-bounded `InformationShare` and acknowledgement contract.
   - Map it explicitly to official Message/DataPart or TextPart objects and back.
   - Reject missing correlation IDs, unsupported roles/parts, oversized text/metadata, and unknown required fields.
3. **R2 — discovery**
   - Generate a minimal Agent Card from explicit VibeMux capabilities.
   - Advertise only the binding exercised by the integration test.
   - Validate the served card through the SDK parser before the server reports ready.
4. **R3 — local client/server exchange**
   - Start one owned loopback server on an OS-assigned port.
   - Discover it from a separately constructed client.
   - Send one information message and return a deterministic acknowledgement without invoking a model or tool.
   - Give the server an owner, cancellation signal, readiness barrier, bounded request size, deadline, join path, and graceful shutdown.
5. **R4 — verification and evidence**
   - Unit-test mapping success and malformed/oversized failure paths.
   - Run a real loopback integration test using the network stack rather than calling the handler directly.
   - Assert correlation preservation, exact information payload, no workspace/database writes, clean shutdown, and no leaked listener.
   - Re-run Rust format, Clippy, workspace tests, Python regression, and `git diff --check`.
6. **R5 — status update**
   - Move only this M7.0 slice to `PARTIAL` after fresh evidence.
   - Keep every unsupported M7 transport/security/conformance item unchecked.
   - Record exact crate/spec versions, commands, platform, limitations, and residual risks.

Expected owned files:

```text
Cargo.toml
Cargo.lock
crates/vibemux_a2a/Cargo.toml
crates/vibemux_a2a/src/lib.rs
crates/vibemux_a2a/tests/basic_information_share.rs
docs/protocol_boundaries.md
PROGRESS.md
CHANGELOG.md
```

Acceptance gate:

- An SDK-backed client discovers a valid Agent Card from the owned loopback server.
- The client sends `VIBEMUX_A2A_INFO_SHARE_OK` with a generated correlation ID.
- The independent server receives the exact information contract and returns an acknowledgement with the same correlation ID.
- The client validates that acknowledgement and the server records only bounded in-memory test evidence.
- Malformed and oversized messages fail with structured errors and do not crash the server.
- The server shuts down within its deadline, its listener closes, and the repository plus both state databases remain unchanged.
- The gate is not satisfied by serialization-only, handler-only, command-construction, or mock-transport tests.

Explicitly deferred:

- canonical A2A Task binding and state transitions;
- SQLite persistence and daemon writer integration;
- JSON-RPC plus REST plus gRPC parity beyond the one selected binding;
- streaming, push notifications, resubscription, cancellation, artifacts, and remote peers;
- non-loopback exposure, authentication, TLS, signed Agent Cards, SSRF controls, TCK, and ITK.

Rollback:

- The new crate is dependency-isolated and has no runtime consumer until its acceptance gate passes.
- If the official SDK cannot satisfy the bounded local exchange on MSRV, keep the research evidence, remove the unverified runtime advertisement, and leave M7 `PLANNED` rather than substituting a fake protocol.

### M8 — Collaboration workflow

**Status:** `PLANNED`

Scope:

- [ ] Planner–Worker–Reviewer–Verifier workflow.
- [ ] Artifact store and content-addressed references.
- [ ] Review records and verification gates.
- [ ] DAG dependencies.
- [ ] Conflict prediction and file-ownership hints.
- [ ] Repair runs rather than mutation of historical runs.
- [ ] Manual merge gate.

Exit criteria:

- A multi-agent task can be replayed from task specification, base commit, events, artifacts, and verifier result.
- Reviewer and implementer identity constraints are enforceable.
- Merge remains explicitly user-controlled.
- Failed verification produces a new repair Run.

### M9 — Windows native alpha

**Status:** `PLANNED`

Scope:

- [ ] Signed Windows binaries or reproducible unsigned preview packages.
- [ ] PowerShell installer path.
- [ ] WezTerm live acceptance suite.
- [ ] Native process lifecycle and named-pipe reliability.
- [ ] Upgrade, rollback, and state migration.
- [ ] Security review and threat model.
- [ ] Crash diagnostics and support bundle with redaction.

Exit criteria:

- A new Windows 11 machine can install and complete a two-harness workflow without WSL.
- Upgrade preserves state.
- Uninstall does not delete project worktrees or repositories.
- Known limitations are explicit.
- No P0/P1 security issue remains open.

### M10 — Plugin API beta and remote nodes

**Status:** `DEFERRED`

Scope:

- Stable plugin API policy.
- Plugin registry/signing policy.
- Remote VibeMux nodes over authenticated A2A.
- Tailnet integration.
- Sandboxed WASM policy/transform plugins where suitable.
- OpenClaw, Ollama, and provider integrations.

Entry condition:

- M7 and M9 are verified.
- Plugin API has survived at least two minor versions without an emergency break.

---

## 7. Immediate Execution Order

The next implementation work should follow this order:

1. **Merge `PROGRESS.md` and the replacement `AGENTS.md`.**
2. **Create ADR-013:** Rust core daemon and out-of-process plugin architecture; supersede ADR-011.
3. **Create ADR-014:** plugin protocol v1, framing, compatibility, and permission model.
4. **Open one issue per P0 defect.**
5. **Fix and test base-commit/diff semantics in the Python prototype.**
6. **Introduce the injected command runner and controlled agent host.**
7. **Implement spawn compensation and reconciliation.**
8. **Freeze Python behavior fixtures.**
9. **Bootstrap the Rust workspace without deleting Python.**
10. **Implement canonical Rust state machines before any A2A endpoint.**

Do not begin real harness integration or A2A routing before steps 1–8 are complete. Otherwise defects in lifecycle, ownership, and state semantics will be replicated into the new core.

---

## 8. Quality Gates

### 8.1 Pull request gate

Every non-documentation PR must provide:

- linked issue and milestone;
- exact acceptance criteria;
- tests that fail before and pass after the change;
- platform impact statement;
- security impact statement;
- protocol/schema compatibility statement;
- benchmark result when touching hot paths;
- `PROGRESS.md` update when status changes;
- no unreviewed generated code;
- no claim that a test passed unless it was executed.

### 8.2 Core Rust gate

Required commands:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo nextest run --workspace --all-features
cargo deny check
cargo audit
```

Additional gates:

- property tests for state machines;
- fuzz targets for plugin framing and external protocol decoding;
- no unbounded async channel;
- no blocking filesystem/database/process operation on async executor threads;
- no `unsafe` outside an approved platform module;
- no `unwrap`/`expect` on untrusted runtime input.

### 8.3 Python/plugin gate

```text
python -m ruff format --check .
python -m ruff check .
python -m mypy .
python -m pytest
```

Python plugins may not directly open or mutate the core database.

### 8.4 Platform gate

- Windows CI: build, unit, storage, path, process, plugin contract, and mock integration.
- Ubuntu CI: same core suite plus tmux integration.
- Live Windows release gate: real WezTerm, real Git for Windows, Unicode path, spaces, junction refusal, process cancellation.
- WSL smoke is not evidence of native Windows support.

### 8.5 A2A gate

- official TCK for each advertised transport;
- cross-language ITK against stable Rust, Go, and Python agents;
- Agent Card validation;
- cancellation and resubscription tests;
- malformed payload, oversized frame, timeout, disconnect, and replay tests;
- transport-independent canonical-state assertions.

---

## 9. Performance Targets

These are engineering targets for the Rust core, not claims about the current Python prototype and not end-to-end LLM latency targets.

| Metric | Target |
|---|---|
| Core cold start to healthy local IPC | p95 < 500 ms on a typical Windows developer machine |
| Local CLI → daemon no-op round trip | p99 < 10 ms |
| Spawned plugin control round trip, 1 KiB payload | p99 < 10 ms excluding plugin work |
| Canonical in-memory event routing | ≥ 10,000 1 KiB events/s in synthetic benchmark |
| Concurrent active streams | ≥ 100 without unbounded memory growth |
| SQLite durable event append | measurable p50/p95/p99; batching allowed, ordering and durability must remain correct |
| Idle daemon RSS | target ≤ 50 MiB in release build |
| Plugin crash detection | < 2 heartbeat intervals |
| Cancellation propagation | p99 < 250 ms to supervised local plugin |
| Queue behavior | all queues bounded; saturation returns or records backpressure |

Performance rules:

1. Benchmark before optimizing.
2. Never trade state correctness or cancellation safety for throughput.
3. Separate protocol overhead from model/harness execution time.
4. Record payload size, concurrency, platform, build profile, and persistence mode.
5. Keep regression thresholds in CI only after a stable benchmark baseline exists.
6. Avoid zero-copy complexity until profiling proves serialization is material.
7. Prefer batching SQLite writes over weakening durability.
8. A2A transport selection must be capability-driven, not hard-coded solely for speed.

---

## 10. Release Plan

| Version | Intended scope | Required gate |
|---|---|---|
| `0.1.x` | Python executable prototype and architecture correction | M1 |
| `0.2.0-alpha` | Rust types, event core, store, daemon, local IPC | M3 |
| `0.3.0-alpha` | plugin protocol and terminal/workspace parity | M5 |
| `0.4.0-alpha` | structured harnesses and A2A gateway | M7 |
| `0.5.0-alpha` | Windows native multi-harness workflow | M9 |
| `0.6.0-beta` | collaboration workflow and plugin API candidate | M8 + compatibility evidence |
| `1.0.0` | stable state schema, plugin API, upgrade policy, and security model | separate release review |

No date is promised by this file. Milestones are gated by evidence, not elapsed time.

---

## 11. Main Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Premature full rewrite | Long period with no runnable product | parity fixtures; incremental Rust replacement |
| Dual Python/Rust state writers | corruption and divergent semantics | single-writer rule; explicit migration cutover |
| Plugin API frozen too early | permanent compatibility burden | private API until M7; version negotiation; fixtures |
| Rust SDK churn | leaks changes across core | isolate in `vibemux-a2a` adapter crate |
| Terminal backend mistaken for agent protocol | false completion and unsafe automation | capability model; structured adapters first |
| Windows-specific process/path failures | product unusable on target platform | Windows CI plus live release gate |
| Unbounded streaming | memory exhaustion | bounded queues, backpressure, deadlines |
| Plugin compromise | state or credential exposure | out-of-process boundary, permission manifest, no DB access |
| A2A remote endpoint abuse | SSRF, data exfiltration, identity confusion | URL policy, TLS, authentication, signed-card support, audit |
| Documentation ahead of implementation | contributors make unsafe assumptions | evidence-based statuses and mandatory progress updates |
| Over-engineered plugin system | delays core parity | stdio protocol first; defer registry/WASM |
| Performance optimization without workload | complexity with no benefit | benchmark-first policy |

---

## 12. Decision Log Required

The following ADRs must exist before the corresponding implementation merges:

- Rust core and daemon.
- Superseding the no-daemon MVP decision.
- Core/plugin responsibility boundary.
- Plugin framing and compatibility.
- Local IPC transport.
- SQLite single-writer and migration policy.
- A2A SDK isolation and transport policy.
- Canonical Task versus A2A Task mapping.
- Artifact storage.
- Windows process and path security.
- Public plugin API stability policy.
- Any approved use of `unsafe`.

---

## 13. Progress Update Template

Use this template in pull requests that update this document:

```markdown
### YYYY-MM-DD — <milestone / issue>

**Status change**
- `<area>`: `PLANNED` → `PARTIAL`

**Implemented**
- ...

**Evidence**
- tests:
- benchmark:
- live validation:
- commit/PR:

**Remaining**
- ...

**Compatibility / migration**
- ...

**Known risks**
- ...
```

A status must not move to `VERIFIED` without a repeatable evidence path.

### 2026-08-23 — M0 governance baseline

**Status change**
- Governance documents and architecture decisions: `PLANNED` -> `VERIFIED`
- M0 remains `PARTIAL` because P0 issues do not yet have repository issue records.

**Implemented**
- Replaced the minimal repository guidance with the Rust-core, single-writer, and out-of-process plugin rules.
- Added the evidence-based migration plan and accepted ADRs 013 through 015.
- Recorded commit `99d1f8ebcf9e25248eb2af1eb6d64b530e51f848` as the Python behavior-reference source snapshot.

**Evidence**
- baseline tests: `python -m pytest` -> 6 passed
- baseline lint: `python -m ruff check .` -> passed
- baseline smoke: `python scripts/smoke_test.py` -> passed
- baseline typing: `python -m mypy .` -> 4 existing errors; not passed
- live validation: WezTerm and tmux unavailable on the audited Windows host

**Remaining**
- Open and implement one focused change for each P0 defect.
- Freeze stable Python parity fixtures before Rust becomes authoritative.
- Bootstrap Rust domain and event crates without deleting the Python prototype.

**Compatibility / migration**
- This baseline changes architecture governance only; it does not change the Python CLI, schema, or runtime behavior.

**Known risks**
- The Python P0 items listed above were completed on the later reference branch; the prototype remains non-authoritative and still requires a supervised mock plugin before M1 exit.

### 2026-08-23 — M1 Python reference P0 command and diff boundary

**Status change**
- M1 Python behavior reference: `PLANNED` -> `PARTIAL`

**Implemented**
- Persisted each Run's actual Git base commit through a forward SQLite migration.
- Made diff output cover committed, staged, unstaged, and untracked changes relative to that base.
- Injected one shell-free command runner into Git, WezTerm, tmux, and service orchestration.
- Added a controlled agent host and removed tmux command-string construction.
- Added a Windows launcher path that prefers direct executables and otherwise invokes a same-name PowerShell companion with fixed flags and separate arguments.

**Evidence**
- tests: 16 pytest tests passed at commit `2b8d871`, including real Git for Windows worktree and diff coverage
- typing: mypy completed with no errors
- lint: Ruff completed with no errors
- smoke: offline mock CLI workflow passed
- local CLI probes: Claude 2.1.224, Copilot 1.0.75, OpenCode 1.18.21, and Grok 0.2.114 returned versions through the controlled runner
- live terminal backend: unavailable; WezTerm and tmux are not installed on the audited host

**Remaining**
- Replace the persisted mock inventory with a supervised mock plugin process.
- Freeze the expanded Python compatibility fixtures.

**Compatibility / migration**
- Legacy databases migrate forward by adding nullable `runs.base_commit`; legacy rows remain readable and fail closed when an operation requires a missing base commit.
- CLI command names and JSON output are unchanged.

**Known risks**
- The Python reference still inherits the harness process environment until the Rust plugin permission model is available.
- PowerShell companion launch is a compatibility boundary, not a general-purpose script execution API.

### 2026-08-23 — M1 spawn compensation and reconciliation

**Status change**
- Python P0 defect list: all listed items now have implementations and focused acceptance tests

**Implemented**
- Added immutable cleanup plans with pre-execution revalidation and dirty-worktree refusal.
- Added spawn compensation that stops an owned terminal, removes only a clean registered worktree, preserves the branch, and records sanitized outcomes.
- Replaced fabricated mock panes with a persisted resource inventory that never stores message content.
- Added project/backend/run/workspace/cwd/pane ownership checks before send and stop.
- Added reconciliation that marks a running Run stale for missing or mismatched resources without treating expected worktree changes as failure.
- Required structured-adapter, verifier, or explicit user authority for a Run to become succeeded.

**Evidence**
- tests: 26 pytest tests passed, including compensation, cleanup-plan drift, persistent inventory, ownership, reconciliation, and success-authority cases
- typing and style: mypy, Ruff lint, and Ruff format checks passed
- smoke and package: cross-process mock CLI smoke and isolated wheel build passed
- live terminal backend: unavailable; contracts are tested but WezTerm and tmux are not installed

**Remaining**
- Freeze versioned Python CLI/event/cleanup/terminal/error fixtures.
- Move mock process behavior to the future plugin protocol rather than extending the Python core.

**Compatibility / migration**
- Mock inventory is stored under ignored `.vibemux/` state; no prompt or message content is persisted.
- Existing stopped Runs remain idempotent; legacy Runs without base commit remain readable but cannot produce authoritative diffs.

**Known risks**
- Compensation cannot identify a terminal resource if a backend creates it and then fails before returning its identity; later inventory reconciliation must preserve such unknown resources.

### 2026-08-23 — M1 Python compatibility fixture freeze

**Status change**
- Immediate execution order step 8: completed for implemented Python contracts

**Implemented**
- Added a versioned fixture tied to the original Python prototype base commit.
- Frozen Task/Run transitions, completion authorities, Event fields, worktree and cleanup records, mock terminal inventory, and send receipt fields.
- Added a contract test that compares the checked-in fixture directly with current definitions.

**Evidence**
- tests: 27 pytest tests passed, including the fixture-to-implementation contract check
- typing and style: mypy, Ruff lint, and Ruff format checks passed
- smoke: cross-process mock CLI workflow passed after fixture freeze

**Remaining**
- CLI machine-readable fixtures should expand when the Rust thin CLI contract is designed.
- Plugin handshake fixtures remain deferred until the Protobuf schema exists; planned fields are not presented as implemented.

**Compatibility / migration**
- Fixture schema v1 is additive test evidence and does not change runtime storage or CLI output.

**Known risks**
- This fixture freezes implemented Python shapes, not the future Rust persistence or plugin wire schema.
### 2026-08-23 — M2 canonical Rust core foundation

**Status change**
- M2 Rust workspace and canonical core: `PLANNED` -> `PARTIAL`

**Implemented**
- Added the Rust 2024 workspace with `vibemux_types` and `vibemux_events` crates.
- Added opaque IDs, deterministic Task/Run transition tables, explicit run-success authority, canonical event envelopes, payload limits, and plaintext-sensitive-field rejection.
- Added Python-reference transition and event fixtures plus Windows/Ubuntu Rust CI commands.

**Evidence**
- MSRV checks: Rust 1.85.0 `cargo fmt --all -- --check`, Clippy with `-D warnings`, and 10 tests passed
- current stable check: Rust 1.97.0 ran the same 10 tests successfully
- Python regression at the original Rust commit: 6 pytest tests, Ruff lint, mock smoke, and wheel build passed
- The later Python P0 branch resolved the recorded formatting and mypy gaps before integration.
- unavailable local gates: cargo-nextest, cargo-deny, and cargo-audit are not installed
- benchmark: not applicable to this domain-only slice
- live validation: not applicable; no terminal or daemon implementation changed

**Remaining**
- Add structured tracing and dependency policy gates.
- Implement Rust persistence and local IPC in the M3 slices.

**Compatibility / migration**
- Rust packages remain private pre-alpha interfaces; Python schema migration and compatibility details are recorded in the M1 entries above.

**Known risks**
- Event payload key rejection is a defense-in-depth schema check, not a general secret detector.
- Rust CI configuration is present but remote GitHub Actions has not been observed in this session.

### 2026-08-23 — M3 Rust SQLite store foundation

**Status change**
- M3 Rust persistence and single-writer daemon: `PLANNED` -> `PARTIAL`

**Implemented**
- Added `vibemux_store` as the blocking SQLite boundary with bundled SQLite for repeatable Windows/Linux builds.
- Added forward schema versioning, WAL, explicit busy timeout, ordered event sequences, JSON projections, and reopen replay.
- Committed state projection and canonical event envelope in one immediate transaction.
- Made idempotency-key duplicates return the original event without applying a second state change.

**Evidence**
- Rust tests: 16 passed across types, events, and store crates on both MSRV 1.85 and current stable
- Rust quality: MSRV 1.85 rustfmt and Clippy with `-D warnings` passed
- Atomicity: a forced projection trigger failure left the event log empty
- Replay: a committed Run projection and event survived store close/reopen

**Remaining**
- Add the daemon-owned dedicated writer worker and exclusive lifecycle lock.
- Add crash/restart projection verification and historical migration fixtures.
- Add Windows named-pipe and POSIX Unix-domain-socket local control transport.

**Compatibility / migration**
- Rust store schema v1 is private pre-alpha and separate from the Python prototype database.
- No Python/Rust dual-writer cutover occurred.

**Known risks**
- `rusqlite::Connection` is intentionally blocking and must remain behind the future daemon writer worker.
- The current projection format is a private JSON snapshot contract and is not yet a public migration promise.

### 2026-08-24 — M7.0 basic A2A information share

**Status change**
- M7 A2A core gateway: `PLANNED` -> `PARTIAL`
- M7.0 local HTTP+JSON information share: `VERIFIED — LOCAL LOOPBACK ONLY`

**Research baseline**
- A2A specification: released protocol `1.0.0`; wire service version `1.0`
- official Rust types: `a2a-lf 0.3.0`
- official Rust client: `a2a-client-lf 0.2.1`
- official Rust server: `a2a-server-lf 0.4.1`
- all selected official crates declare Rust 1.85 MSRV

**Implemented**
- Added the dependency-isolated `vibemux_a2a` crate; no SDK type leaked into types/events/store crates.
- Added a VibeMux-owned, size-bounded information-share and acknowledgement mapping over official Message/DataPart types.
- Added a minimal Agent Card advertising only HTTP+JSON and the basic information-share skill.
- Added an owned loopback server with OS-assigned port, readiness after bind, body limit, request deadline, bounded in-memory capture, graceful shutdown, join path, and Drop abort fallback.
- Added an SDK Agent Card resolver and negotiated client that sends a direct Message response without creating canonical VibeMux Task/Run state.

**Evidence**
- Rust workspace: 23 tests passed on both MSRV 1.85 and current stable; A2A crate contributed 5 unit and 2 real-network integration tests
- Client/server integration: `VIBEMUX_A2A_INFO_SHARE_OK` crossed a real TCP loopback connection and preserved its generated correlation ID
- Discovery: the official SDK resolver fetched and parsed `/.well-known/agent-card.json`
- Transport: the official HTTP+JSON client called the official server `/message:send` router
- Failure paths: invalid contract returned a client error; an oversized HTTP body returned 413; a valid request still succeeded afterward
- Lifecycle: graceful shutdown completed and the listener rejected a new connection afterward
- Quality: Rust 1.85 rustfmt, Clippy with `-D warnings`, workspace tests, Python 27-test regression, Ruff, and mypy passed

**Compatibility / migration**
- This slice does not open either state database, create a canonical Task, or change the single-writer cutover.
- The information contract is private pre-alpha and may change before M7 conformance.
- Only the selected HTTP+JSON binding was exercised; JSON-RPC and gRPC remain unadvertised.

**Security impact**
- Bind is hard-coded to `127.0.0.1`; non-loopback URLs are rejected by the client and Agent Card validator.
- Request bodies are limited to 16 KiB; information text, metadata count, keys, values, and identifiers have smaller explicit limits.
- No environment, credential, repository file, prompt history, tool output, URL part, raw file part, or database content is shared implicitly.

**Remaining**
- Integrate the handler with the daemon-owned writer only after M3 lifecycle and IPC gates.
- Add explicit internal/external Task binding before any stateful A2A operation.
- Add authentication, TLS policy, SSRF controls, JSON-RPC, gRPC, streaming, cancellation, artifacts, TCK, ITK, and independent cross-language peers.

**Known risks**
- The official REST-only dependency graph still includes generated Protobuf and tonic support transitively through the SDK packages, increasing build time and binary dependency surface.
- Local loopback success is not remote interoperability, deployment, security, or conformance evidence.

### 2026-08-24 — M5.0 fixed probe and unified frontend shell

**Status change**
- M5.0 read-only probe and unified frontend shell: `PLANNED` -> `PARTIAL`

**Implemented**
- Added `vibemux_probe` with a versioned JSON report for five agent launchers, safe endpoint routing, CC Switch health/telemetry, and the A2A loopback self-test.
- Added Windows direct-executable and same-name PowerShell-companion resolution; `.cmd` contents are never executed.
- Added bounded version probes with deadlines, child kill-on-drop, and an explicit environment allowlist that excludes provider keys and tokens.
- Added allowlisted JSON/TOML endpoint parsing; credential-bearing URLs and non-HTTP schemes are discarded.
- Added read-only CC Switch SQLite aggregation that never selects request/response body or error-message columns.
- Added `vibemux_frontend` with a backend-neutral dashboard model, Ratatui renderer, deterministic `--once` output, and five `NativeTuiSlot` reservations.
- Kept native TUI launch, attach, input, focus, resize, and teardown code absent from this slice.

**Live evidence on Windows**
- Claude `2.1.224`, Codex `0.137.0`, OpenCode `1.18.21`, Copilot `1.0.75`, and Grok `0.2.114` launchers were version-verified without inference.
- Claude and Grok were classified as `local_gateway`; OpenCode as `direct`; Codex and Copilot as `unknown` because no safe explicit endpoint was present.
- CC Switch `127.0.0.1:15721` was TCP-reachable, `/health` returned 200, and aggregate telemetry reported `grokbuild` failures without exposing body content.
- A2A self-test preserved correlation and closed its listener.
- Frontend `--once` rendered gateway/A2A summaries, all five agents, and five reserved native-TUI slots, then exited cleanly.

**Automated evidence**
- Full Rust workspace: 33 tests passed on both MSRV 1.85 and current stable; rustfmt and Clippy with `-D warnings` passed
- Probe: 6 unit tests, including bounded/allowlisted config parsing, read-only aggregate telemetry, JSON fixture round-trip, and repeatable A2A self-test
- Frontend: 4 tests, including wide/narrow Ratatui TestBackend rendering and deterministic snapshot output
- Native slot tests prove reservation state only; they do not prove PTY/ConPTY attachment
- Python regression: 27 tests, Ruff format/lint, and mypy passed

**Frontend dependency baseline**
- Ratatui `0.29.0` (Rust 1.74 MSRV)
- Crossterm `0.28.1` (Rust 1.63 MSRV)
- TOML parser `0.8.23` (Rust 1.66 MSRV)
- Interactive Windows smoke entered alternate-screen/raw mode, rendered the dashboard, accepted `q`, and restored the terminal successfully

**Compatibility / migration**
- Probe schema v1 and the frontend model are private pre-alpha contracts.
- Neither crate opens the VibeMux state database or performs canonical transitions.
- Authentication and inference states are separately reported as `not_run`; launcher verification is not promoted to model-health evidence.

**Security impact**
- Default probes perform no paid inference, login, provider switch, process termination, config write, or database write.
- Child process environments are allowlisted; report endpoints omit credentials, query strings, and fragments.
- CC Switch is opened with SQLite read-only/no-mutex flags and fixed aggregate SQL.

**Remaining**
- Move probe execution behind the daemon support-bundle/control API when M3 IPC is ready.
- Add fixture versioning for backward-compatible probe report evolution.
- Implement terminal plugin and Windows ConPTY ownership before changing any native TUI slot from `reserved` to attachable.
- Add focus, resize, input, clipboard, approval, cancellation, crash recovery, and teardown tests before interactive native surfaces are enabled.
- Build the graphical frontend only after the terminal-surface and daemon APIs stabilize.

**Known risks**
- Version probes prove launcher startup only; provider authentication and inference require separate explicit probes.
- Reading live third-party configuration remains best-effort and must degrade to `unknown` when formats change.
- Ratatui shell rendering is verified, but live embedded agent TUIs remain deliberately unimplemented.
