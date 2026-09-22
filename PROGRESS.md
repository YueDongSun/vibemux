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
| Historical audited branch | `main` |
| Current implementation branch | `main` (feature branches merge back via direct merge; PR #3 and the two-shell frontend branch land through `feat/3-two-shell-frontend-integration`); prior baseline `codex/m7_a2a_supervisor` per `85697d6743b01b8e0aa10c48d79837dcc56c007c` with its tested input fingerprint in `docs/evidence/a2a_source_manifest.json` and the 2026-08-31 publication audit |
| Audited implementation baseline | PR #1 head `fce05cffe628ce65c49620a06f6bd1ef7eaadb89`, merged as `6498e6aa711f751d9f9e50e37034d61c17d4b341` |
| Python package version | `0.1.0a0` |
| Rust workspace version | `0.2.0-alpha.0` |
| Product target | Windows 10/11 first; WSL2/Linux development and compatibility |
| Current implementation | Python 3.12 behavior-reference CLI plus a Rust 2024 pre-alpha core workspace |
| Current Rust scope | Typed domain/events, SQLite store, single-writer daemon, authenticated local IPC/lifecycle, authenticated local stateful A2A and supervisor workflow, read-only probe/frontend shell, plugin protocol v1 foundation, plugin supervisor, and daemon-owned registry with bounded recovery and read-only IPC v2 status |
| Current default entry point | Python `vibemux` for the complete prototype workflow; Rust `vibemuxctl` for daemon lifecycle and the harness registry/switch surface (control protocol v3) |
| Current maturity | M3 is `VERIFIED`; M4.0/M4.1 foundations and the M4.2 registry/status slice have bounded verification; M4.2 and M7.1/M8.0 local supervisor evidence is native Windows only; **not yet a functional multi-harness alpha** |
| Current release posture | Pre-alpha; no stable CLI, schema, protocol, or plugin compatibility guarantee |

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
- Automatic merge to the user's primary branch.
- Treating Git worktrees as a security sandbox.
- Storing full prompts or secrets in the default audit log.
- Maintaining two independent state writers during the Python-to-Rust transition.

---

## 3. Accepted Architecture Direction

### 3.1 Decision

The accepted authoritative architecture is a long-lived **Rust core** with optional and vendor-specific functionality running as **out-of-process plugins**.

The migration is already in progress rather than merely planned:

- M2 domain/event foundations exist in Rust.
- M3 persistence, single-writer daemon, authenticated local IPC, lifecycle, stale recovery, and the declared Windows control-runtime boundary are implemented and verified within scope.
- M4.0 protocol and M4.1 process-supervisor foundations are implemented as isolated crates; M4.2 integrates the supervisor into the daemon registry without Task/Run routing.
- Full command parity, Python-to-Rust state migration/cutover, operator plugin recovery and writable plugin control, Rust workspace/terminal parity, real vendor CLI plugins, remote A2A, full ITK, and automatic in-flight workflow recovery remain incomplete.

The current Python implementation remains valuable as:

- the current complete prototype CLI;
- a behavior and compatibility reference;
- a source of parity fixtures;
- a future Python plugin SDK and integration host.

Python must not receive new authoritative orchestration, A2A routing, event-broker, scheduler, or public plugin behavior. It must also not become a concurrent writer to the Rust database.

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
| Python package and Typer CLI | `PARTIAL` | `pyproject.toml`, `src/vibemux/`, Python reference fixtures | The current complete prototype workflow remains runnable; original P0 correctness fixes are implemented, but Python is not the future authoritative core |
| Windows-first product path | `PARTIAL` | README, platform docs, Windows CI, protected control runtime | Native daemon lifecycle and the declared ACL boundary have evidence; live WezTerm/ConPTY multi-harness operation is not implemented |
| Rust workspace and canonical domain/event core | `PARTIAL` | root `Cargo.toml`, `vibemux_types`, `vibemux_events` | Rust 2024/MSRV workspace, typed IDs, state machines, events, and property tests exist; tracing and public compatibility policy remain incomplete |
| Rust SQLite store and single-writer daemon | `VERIFIED` | `vibemux_store`, `vibemuxd`, `vibemux_cli`, `vibemux_platform` | M3 is implemented within scope: migrations/WAL, atomic state+event, replay/idempotency, bounded writer, authenticated IPC, lifecycle, explicit recovery, and Windows protected runtime |
| Rust CLI | `PARTIAL` | `vibemuxctl daemon` with `start`, `health`, `inspect`, `recover`, or `stop` | Lifecycle commands exist; Task/Run/workspace/plugin command parity does not |
| Plugin protocol v1 foundation | `VERIFIED` | `vibemux_plugin_protocol`, checked-in Protobuf/TOML fixtures | M4.0 is verified as an isolated private pre-alpha wire/manifest/negotiation/lifecycle foundation; it does not spawn or mutate state |
| Plugin process supervisor foundation | `VERIFIED` | `vibemux_plugin_supervisor`, real mock-child process tests | M4.1 is verified as an isolated supervisor foundation with bounded channels, deadlines, cancellation, heartbeat, stderr cap, shutdown, and crash containment |
| Daemon-owned plugin registry and read-only status | `VERIFIED` | `vibemuxd::plugin_registry`, explicit startup config, IPC v2 `plugin_status` | Native Windows real mock-child/standalone-daemon evidence; lifetime budgets/backoff/quarantine; no writable plugin API, operator recovery, SDK, vendor plugin, or Task/Run routing |
| A2A adapter and supervisor | `PARTIAL — LOCAL STATEFUL` | `vibemux_a2a`, daemon supervisor, optional model peers | Authenticated HTTP+JSON/JSONRPC/gRPC, task/artifact/status/cancel/subscription mapping, atomic canonical bindings and local supervisor verification are implemented; remote/TLS, full ITK, history and restart recovery remain incomplete |
| Probe and frontend shell | `PARTIAL` | `vibemux_probe`, `vibemux_frontend` | Read-only evidence collection and reserved Ratatui slots exist; real PTY/ConPTY attachment and native-TUI lifecycle do not |
| Python workspace/terminal safety | `PARTIAL` | `workspace.py`, `terminal.py`, `services.py` | Base-commit diff, cleanup plan, compensation, ownership, and reconciliation are implemented; live backend and Windows path-edge coverage remain incomplete |
| Rust workspace/worktree parity | `PLANNED` | no Rust workspace crate | Git worktree lifecycle, cleanup, artifacts, and reconciliation are not implemented in Rust |
| Rust terminal plugins and native-TUI attachment | `PLANNED` | no terminal plugin/ConPTY implementation | Mock/WezTerm/tmux plugin parity and ownership are not implemented |
| Structured vendor harness plugins | `PLANNED` | mock fixture only | No OpenCode/Claude/Copilot/Pi/Grok/Gemini production adapter is integrated |
| Python-to-Rust state migration and default cutover | `PLANNED` | separate Python and Rust databases | No state migration, default CLI cutover, or shared-schema compatibility promise exists |
| Tests and CI | `PARTIAL` | Python/Rust Windows+Ubuntu workflow; PR #1 head CI run #8 | pytest/Ruff/smoke and Rust fmt/Clippy/workspace tests run cross-platform; mypy/format/package/coverage/nextest/deny/audit/fuzz/live-backend gates are not all enforced in CI |
| Scheduler, artifacts, review gates | `PLANNED` | design text only | No production implementation |

### 4.2 Strengths worth preserving

1. Windows remains the primary product platform, and the Rust daemon has a verified current-user control-runtime boundary.
2. The Python prototype remains runnable and has versioned behavior fixtures instead of being deleted during migration.
3. Domain, event, store, daemon, platform, A2A, protocol, supervisor, probe, and frontend responsibilities are separated into Rust crates.
4. Python and Rust use separate databases, preventing an accidental dual-writer cutover.
5. Plugin framing, queues, deadlines, cancellation, stderr handling, and child lifecycle are bounded and fail closed in the implemented foundations.
6. Plugin protocol and supervisor crates cannot directly mutate canonical Task/Run state or SQLite.
7. A2A, probe, and frontend slices are explicitly non-authoritative and do not silently become state writers.
8. PR #1 head passed the configured Windows/Ubuntu Python and Rust CI jobs.

### 4.3 Critical defects and technical debt

#### Resolved P0 — Python behavior-reference safety baseline

- [x] **Persisted the actual base commit and implemented authoritative Run diffs.**
  `Run.base_commit` is persisted; diff covers committed, staged, unstaged, and untracked changes relative to that commit.

- [x] **Introduced one injected, shell-free command runner.**
  Git, WezTerm, tmux, and harness-host paths use controlled argv/stdin boundaries.

- [x] **Removed shell-mediated tmux target construction.**
  The controlled agent host receives validated argv and launches with `shell=false`.

- [x] **Added plan-first spawn compensation.**
  Owned terminal/worktree resources are stopped or removed only after safety revalidation, and outcomes are recorded.

- [x] **Persisted mock terminal inventory and rejected unknown panes.**
  Cross-process mock state no longer fabricates missing resources and does not persist message content.

- [x] **Validated terminal resource ownership before operations.**
  Project, backend, Run identity, workspace, cwd, and pane inventory are checked before send/stop.

- [x] **Reconciled missing or mismatched resources.**
  Affected running Runs become `STALE`; terminal/worktree evidence never implies success.

- [x] **Required explicit Run completion authority.**
  Only a structured adapter, verifier, or user transition may produce `SUCCEEDED`.

These defects are resolved in the current Python reference. Remaining work is Rust parity, integration, and cutover, not continued present-tense description of the old bugs.

#### P1 — active parity, migration, and platform work

Completed foundations:

- [x] Python schema migration history, WAL, busy timeout, and atomic Task/Run save-plus-event paths.
- [x] Python immutable cleanup plans, spawn compensation, ownership, reconciliation, and compatibility fixtures.
- [x] Rust migrations/WAL, atomic state+event, idempotency, projections/replay, and single-writer daemon lifecycle.
- [x] Rust authenticated local IPC and explicit stale-runtime recovery.

Remaining:

- [ ] Replace `INSERT OR REPLACE` state writes with explicit insert/update operations.
- [ ] Add failure-injection transaction tests for every remaining Python state/event path.
- [ ] Complete persisted domain fields and lifecycle semantics needed for review, verification, merge, failure, cancellation, correlation, and causation.
- [ ] Add Rust Task/Run control API and machine-readable CLI parity.
- [ ] Design and test Python-to-Rust state migration, exclusive cutover, rollback, and version compatibility.
- [ ] Implement Rust Git worktree lifecycle, diff, cleanup, artifacts, ownership, and reconciliation.
- [ ] Implement mock, WezTerm, tmux, and Windows ConPTY terminal plugins.
- [ ] Add Windows path tests for drive letters, case folding, spaces, Unicode, junctions, reparse points, and cross-drive refusal.
- [ ] Replace the Python mock runtime path with a daemon-supervised mock plugin before claiming Rust end-to-end parity.
- [ ] Enforce mypy, Ruff format, package build, coverage, security, integration, and live platform gates in CI.
- [x] Port the Python-side harness orchestration surface added after the migration boundary (harness registry, detection-gated spawn, `vibemux harnesses`/`vibemux switch`; introduced by `2cdc3ed` and re-applied by PR #3) to Rust per AGENTS.md §4.1 — **done 2026-09-16 (entry 2026-09-16 (2))**: new crate `vibemux_harness`, `vibemux_store` schema 3 (harness registry/config projections + `v3_harness_seed`), control protocol v3 (`HarnessRefresh`/`HarnessSnapshot`/`HarnessSwitch`, daemon-owned via the single `WriterWorker`), and `vibemuxctl harnesses [--cached]` / `vibemuxctl switch <harness>`. Detection comes from the trusted `vibemux_probe` cache; gating rejects undetected/unknown harnesses. Closes issue #4. Remaining scope note: Rust launch/attach execution is still not implemented (the `pty` protocol label is persisted for parity only), and the Python prototype remains the installed behavior reference for full spawn orchestration.

#### P2 — required before public plugin API

Implemented private foundations:

- [x] Versioned Protobuf plugin schema, bounded framing, and validated TOML manifest.
- [x] Capability, permission, platform, and protocol negotiation.
- [x] Session-bound handshake/lifecycle validation.
- [x] Shell-free child spawn, clean environment, bounded queues, deadlines, heartbeat, cancellation, stderr cap, graceful shutdown, and crash containment.
- [x] Contract fixtures, malformed-input tests, and cross-platform real-process tests.

M4.2 delivery and remaining gates:

- [x] Daemon-owned ephemeral plugin registry and lifecycle snapshots (M4.2).
- [x] Authenticated read-only plugin status with bounded snapshots and redacted diagnostics.
- [ ] Writable plugin start/stop API with authorization gates.
- [x] Lifetime restart budget, capped exponential backoff, and terminal quarantine.
- [ ] Operator-controlled quarantine recovery and cross-daemon persistence.
- [ ] Plugin SDKs for Rust and Python.
- [ ] Public compatibility policy, previous-version fixtures, and version matrix.
- [ ] Coverage-guided fuzz targets for framing, manifests, negotiation, and lifecycle transitions.
- [ ] Daemon-owned mock plugins and compatibility tests for each intended plugin kind.
- [ ] Real vendor plugins only after M4.2/M5 integration gates pass.
- [ ] Sandbox enforcement and external-endpoint permission brokers.

---

## 5. Current Repository Layout and Remaining Target Additions

### 5.1 Current `main`

```text
vibemux/
├── Cargo.toml
├── rust-toolchain.toml
├── crates/
│   ├── vibemux_types/          # IDs, domain objects, state machines
│   ├── vibemux_events/         # canonical envelopes and invariants
│   ├── vibemux_store/          # SQLite migrations and repositories
│   ├── vibemux_platform/       # Windows/POSIX process and IPC primitives
│   ├── vibemux_a2a/            # official SDK adapter; loopback slice only
│   ├── vibemux_probe/          # read-only launcher/gateway probes
│   ├── vibemux_frontend/       # Ratatui shell with reserved native-TUI slots
│   ├── vibemux_plugin_protocol/
│   │   └── proto/vibemux_plugin_v1.proto
│   ├── vibemux_plugin_supervisor/
│   ├── vibemuxd/               # long-lived local core
│   └── vibemux_cli/             # thin lifecycle client and daemon bootstrap
├── src/vibemux/                 # Python behavior-reference package
├── tests/                       # Python tests and fixtures
├── scripts/                     # smoke and benchmark scripts
├── docs/                        # architecture, ADRs, boundaries, and labs
├── AGENTS.md
└── PROGRESS.md
```

The root `Cargo.toml` is authoritative for current workspace membership.

### 5.2 Planned additions

```text
vibemux/
├── crates/
│   └── vibemux_workspace/       # Rust Git worktrees, artifacts, cleanup
├── plugins/
│   ├── terminal-wezterm/
│   ├── terminal-tmux/
│   ├── harness-mock/
│   └── harness-*/
├── sdk/
│   ├── python/
│   └── rust/
├── tests/
│   ├── contract/
│   ├── integration/
│   ├── conformance/
│   ├── fixtures/
│   └── performance/
└── benches/
```

Planned paths are not implemented claims or permission for a destructive rewrite. Migration must preserve a runnable branch and pass parity tests at each step.

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
- [x] Add ADR: supersede "no daemon in MVP".
- [x] Add ADR: plugin wire protocol and compatibility policy.
- [ ] Create issues with named owners for the remaining M4.2, M5, CI-hardening, migration, and P1/P2 work.

Exit criteria:

- Architecture decisions are explicit.
- Current claims match current evidence.
- No contributor can mistake the Python prototype for the final core.
- Remaining integration and migration risks have owners and acceptance tests.

### M1 — Freeze and stabilize the Python behavior reference

**Status:** `PARTIAL`

Scope:

- [x] Split command runner, domain, storage, workspace, terminal, harness, and services.
- [x] Persist `base_commit` and correct diff semantics.
- [x] Add schema migrations, WAL/busy-timeout configuration, and atomic state/event foundations.
- [x] Implement reconciliation and immutable cleanup plan.
- [ ] Replace fake mock persistence with a supervised mock process.
- [ ] Add complete failure-injection, live-backend, and Windows path-edge suites.
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

**Status:** `VERIFIED`

Scope:

- [x] SQLite migration framework and WAL configuration.
- [x] Atomic state + event transactions.
- [x] Idempotency keys, sequence ordering, projections, and replay foundation.
- [x] Dedicated daemon writer worker with bounded queue and explicit backpressure.
- [x] On-demand user-mode `vibemuxd` process and CLI bootstrap.
- [x] Authenticated Windows named-pipe and POSIX Unix-domain-socket control transport library.
- [x] CLI start/query/stop wiring over local IPC.
- [x] Daemon writer-core lifecycle, exclusive lock, health, and graceful shutdown.
- [x] Explicit stale-instance inspect/recover workflow with confirmation-bound cleanup.
- [x] Windows protected current-user control runtime and cross-user/remote access boundary.

Exit criteria:

- Exactly one authoritative writer owns project state.
- Crash/restart preserves event order and projections.
- CLI can start, query, and stop the daemon.
- No unauthenticated local TCP control endpoint is required.
- Python and Rust are never concurrent writers to one database.

### M4 — Plugin protocol and supervisor

**Status:** `PARTIAL`

Scope:

- [x] Protobuf plugin protocol v1 wire/framing foundation.
- [x] Manifest, handshake, capabilities, permissions, and version negotiation foundation.
- [x] Bounded request/event channel foundation with observable backpressure.
- [x] Handshake/receive/shutdown deadlines, cancellation, heartbeat, and crash-reporting foundation.
- [x] Daemon-owned ephemeral plugin registry and lifecycle snapshots.
- [x] Authenticated read-only plugin status API.
- [ ] Writable plugin control API.
- [x] Restart budget, bounded backoff, and quarantine.
- [ ] Operator recovery.
- [ ] Daemon-owned mock manifests and plugins.
- [ ] Rust and Python plugin SDKs.
- [x] Contract fixtures and property tests for framing and malformed input.
- [ ] Coverage-guided fuzz targets.

Exit criteria:

- A plugin cannot directly mutate core state.
- Plugin crash cannot crash the core.
- Unsupported protocol versions fail with a structured error.
- Backpressure is observable and bounded.
- Plugin stderr cannot corrupt the wire protocol.

#### M4.0 — Protocol, manifest, and handshake foundation

**Slice status:** `VERIFIED — ISOLATED PROTOCOL FOUNDATION`

Acceptance gate:

- [x] Checked-in `vibemux.plugin.v1` Protobuf schema generates deterministically with pinned Prost and vendored `protoc` on Windows/Linux MSRV-compatible toolchains.
- [x] Four-byte big-endian framing rejects zero, oversized, truncated, malformed, and missing-body envelopes without allocation above the configured maximum or panic.
- [x] Envelope version/ID/correlation/causation grammar validation returns stable structured errors.
- [x] TOML manifest validation requires a shell-free argv entry point, known kind, bounded/deduplicated capabilities and permissions, supported platform list, and compatible protocol range.
- [x] Core negotiation grants only the intersection of manifest declarations, Hello requests, and core policy; omitted permission is denied.
- [x] Core lifecycle accepts only Hello -> CoreHello -> Ready -> Active -> Drain -> Shutdown -> Closed, binds Ready to the negotiated session ID, and rejects duplicate/out-of-phase transitions.
- [x] Canonical round-trip fixtures preserve message/correlation identity and demonstrate additive unknown-field tolerance under the same accepted version.
- [x] The crate contains no database, terminal, Git, A2A, vendor SDK, process-spawn, shell, or canonical-state mutation dependency.
- [x] Windows/Linux tests, workspace MSRV/current-stable Clippy/tests, and Python reference regressions remain green.

Implementation order:

1. Add the isolated `vibemux_plugin_protocol` crate, `.proto`, vendored code generation, error taxonomy, and validated fixed framing.
2. Add manifest types/TOML validation and bounded identifier sets.
3. Add core-side negotiation and lifecycle state machine with deterministic tests.
4. Add malformed/truncated/oversized/property tests and canonical wire fixtures.
5. Update architecture, protocol boundaries, changelog, and this ledger before committing.

Non-goals for M4.0:

- Child spawn/supervision, queues, heartbeat timers, restart budget, stderr capture, SDK publication, real plugin behavior, or Task/Run mutation.

#### 2026-08-24 — M4.0 protocol foundation implementation

**Status change**
- M4: `PLANNED` -> `PARTIAL`. The isolated wire/manifest/handshake foundation is implemented; process supervision and plugin behavior remain pending.

**Implemented**
- Added `vibemux_plugin_protocol` with no dependency on state, SQLite, Git, terminal, A2A, vendor SDK or process-spawn crates.
- Added checked-in `vibemux.plugin.v1` schema for Hello/CoreHello/Ready, request/response/event, heartbeat/cancel/drain/shutdown and structured protocol errors.
- Added pinned Prost 0.14.4 generation with vendored `protoc` 3.2.0; Windows and Linux generate the same checked-in canonical frame fixture.
- Added configurable four-byte big-endian framing with a 1 MiB default/16 MiB hard ceiling and prefix validation before payload allocation.
- Added bounded envelope/body/ID/version validation, deterministic unknown-field tolerance and stable error codes with no raw decoder/I/O text.
- Added deny-unknown TOML manifest validation, SemVer, known platform/kind, bounded unique identifiers and shell-executable rejection for argv entry points.
- Added policy negotiation that enforces manifest/Hello identity, protocol/platform overlap and deterministic capability/permission intersection; omitted permission is denied.
- Added a core lifecycle machine with session-bound Ready, active/drain traffic allowlists and fail-closed duplicate/out-of-phase transitions.

**Evidence**
- Protocol crate: 16 unit/property tests plus 1 checked-in manifest/wire fixture test passed on Windows and Linux Rust 1.85-compatible toolchains.
- Oversized-prefix tests provide only the four-byte prefix and receive `plugin_frame_too_large`, proving rejection before payload read/allocation.
- Malformed, zero, truncated, trailing-byte, missing-body, unsupported-version, duplicate-ID, undeclared-capability, platform-mismatch and session-mismatch cases fail with stable errors.
- Windows Rust 1.85.0 MSRV and stable 1.97.0: full workspace 92 tests passed on each; workspace Clippy with `-D warnings` passed on each.
- Linux Docker Rust 1.85.1: vendored code generation and all 17 protocol contract tests passed.
- Python reference: 27 pytest tests passed; Ruff lint/format and mypy passed.
- Direct dependency audit: Prost, SemVer, Serde, Tokio I/O, TOML and error handling only; build/dev dependencies are vendored protoc, Proptest and Tempfile.

**Compatibility boundary**
- Protocol v1 remains private pre-alpha. Unknown Protobuf fields are tolerated, but a newer minor is rejected until an explicit additive compatibility policy is accepted.
- Opaque request/event payloads are not authoritative Task/Run mutations. The core database remains unreachable from this crate.

**Next slice**
- M4.1 process supervisor: shell-free child spawn, stdout-only frame transport, bounded queues, stderr cap, handshake/deadline/heartbeat/cancel/drain/shutdown and crash reporting using mock plugins.

#### M4.1 — Process supervisor foundation plan

**Slice status:** `VERIFIED — ISOLATED PROCESS SUPERVISOR FOUNDATION`

Acceptance gate:

- [x] A resolved canonical executable + argv launch spec spawns without a shell, clears inherited environment, applies only bounded explicit variables, and uses a separate process group/no visible Windows console.
- [x] Real mock-plugin stdin/stdout completes Hello/CoreHello/session-bound Ready and echo traffic over the M4.0 codec on Windows and Linux.
- [x] Handshake timeout, malformed stdout, invalid transition, oversized frame, and early exit terminate only that child and return stable errors.
- [x] Bounded outbound/inbound channels expose queue-full backpressure and never use an unbounded buffer.
- [x] Stderr flood is drained independently; only bounded byte-count/truncation metadata crosses the supervisor API and stdout framing remains valid.
- [x] Heartbeat sequence/freshness, receive deadlines and explicit Cancel messages are observable with deterministic tests.
- [x] Drain -> Shutdown -> acknowledgement exits cleanly; shutdown timeout kills and reaps only the exact child.
- [x] Abrupt nonzero child exit produces a stable crash report and does not crash the test/core process.
- [x] Supervisor and fixtures have no SQLite/Git/terminal/A2A/vendor SDK/canonical-state mutation dependency.
- [x] Windows/Linux real-process tests, workspace MSRV/current-stable Clippy/tests, and Python reference regressions remain green.

Implementation order:

1. Add `vibemux_plugin_supervisor` launch/config/error/report types and bounded stderr collector.
2. Add handshake and bounded reader/writer tasks over `vibemux_plugin_protocol`.
3. Add session send/receive/heartbeat/cancel/drain/shutdown/crash APIs.
4. Add mock/malformed/hang/stderr/crash fixture binary and cross-platform process tests.
5. Update docs/evidence and commit after sanitization.

Non-goals for M4.1:

- Automatic restart policy, daemon/control-API integration, SDK publication, non-harness plugin kinds, sandbox enforcement, or Task/Run mutation.

#### 2026-08-24 — M4.1 process supervisor implementation

**Implemented**
- Added isolated `vibemux_plugin_supervisor` with canonical executable/cwd resolution, argv-only spawn, shell-executable rejection, clean environment and bounded explicit variables.
- Added Windows hidden/new-process-group and POSIX process-group launch while retaining the exact child handle; Drop starts exact-child kill plus background reap.
- Added deadline-bound Hello/CoreHello/session-ready handshake over child stdin/stdout and bounded reader/writer Tokio channels.
- Added public queue-full/closed errors, receive deadlines, monotonic heartbeat sequence/freshness, explicit Cancel, Drain/Shutdown acknowledgement and exact-child timeout termination.
- Added independent stderr draining that retains no raw content and reports only capped byte count plus truncation.
- Added stable graceful/crash exit reports with exit code and bounded stderr metadata.
- Added a feature-gated mock harness binary with normal echo/environment/cancel, quiet saturation, duplicate heartbeat, malformed, oversized, wrong Ready, handshake hang, stderr flood, crash and shutdown-hang modes.

**Evidence**
- Supervisor crate: 3 unit tests plus 8 real-process integration tests passed on Windows and Linux Rust 1.85-compatible toolchains.
- Real normal session proved inherited `PATH` absent, explicit `ALLOWED_VALUE` present, correlation-preserving echo, Cancel event, heartbeat freshness and graceful exit code 0.
- A quiet plugin saturated the real outbound path and returned `plugin_supervisor_queue_full`; a separate 50 ms receive returned `plugin_supervisor_receive_timeout`.
- A 128 KiB stderr flood reported exactly the configured 1 KiB captured count with `truncated=true` while stdout response framing remained valid.
- Malformed/oversized stdout, wrong-session Ready, handshake hang, duplicate heartbeat, exit code 7 crash and shutdown hang all produced stable errors/reports without crashing the supervisor test process.
- Windows Rust 1.85.0 MSRV and stable 1.97.0: full workspace 103 tests passed on each; workspace Clippy with `-D warnings` passed on each.
- Linux Docker Rust 1.85.1: all 11 supervisor unit/process tests passed with POSIX process-group behavior.
- Python reference: 27 pytest tests passed; Ruff lint/format and mypy passed.

**Security boundaries**
- `ResolvedPluginLaunch` Debug redacts executable, cwd, arguments and environment values. Stderr content never crosses the API.
- Environment allowlisting is process hygiene, not a sandbox; filesystem/network enforcement still requires future permission-aware sandbox plugins.
- No supervisor dependency reaches SQLite, Git, terminal, A2A, vendor SDK or canonical Task/Run state.

**Next slice**
- Add daemon-owned supervisor registry, restart budgets/quarantine, plugin status/control API, and mock manifests for the remaining declared plugin kinds before SDK publication.

#### M4.2 - Daemon registry and read-only status slice

**Slice status:** `VERIFIED - NATIVE WINDOWS MOCK INTEGRATION`; M4 remains `PARTIAL`.

**Implemented scope**

- `vibemuxd` owns the existing Rust supervisor through a bounded in-memory registry; the default daemon loads no plugins. An explicit `--plugin-config` JSON file supplies validated local startup registrations.
- Maximum 32 entries, lifetime restart budget of at most 16 additional launches, capped exponential backoff, unique attempt/session IDs, heartbeat watchdog with negotiated-interval margin, and terminal quarantine. Status reads never launch, reset budgets, clear quarantine, or contact a child.
- Only heartbeat envelopes are accepted after Ready. Unsolicited request/response/event payloads quarantine and never reach Task/Run state or the database. Registry code has no writer/store handle.
- IPC v2 adds authenticated read-only `plugin_status`. v1 health/shutdown requests and their error versions remain compatible; old v1 clients must be upgraded to accept v2 descriptors. Plugin wire/manifest and SQLite schemas are unchanged.
- Cancellation during active execution, handshake, and backoff prevents further restarts. Normal daemon exit joins exact-child cleanup and I/O workers before releasing its writer. Unconfirmed reap preserves PID evidence in quarantine. Shutdown acknowledgement is acceptance only.
- See [ADR 023](docs/adr/023_daemon_plugin_registry.md) and [registry protocol](docs/plugin_registry_protocol.md) for defaults, compatibility, redaction, and threat model.

**Evidence and limits**

- Real Rust mock-child and separate-daemon integration scenarios exercise crash exhaustion, permanent protocol quarantine, healthy sibling visibility, authentication, unchanged seeded Task/events, cancellation, periodic/missing heartbeat, forced shutdown, and post-join OS process absence.
- Final commands and outcomes are recorded in the dated M4.2 ledger below. These are local native Windows results, not Linux, CI, live terminal, vendor-plugin, sandbox, fuzz, soak, or production evidence.
- Quarantine/budgets are ephemeral to a daemon lifetime. Writable plugin control, operator recovery, other plugin-kind fixtures, SDKs, vendor integrations, process-tree containment, and abrupt-daemon-exit child cleanup remain outside this slice.

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

### M7 — A2A core gateway

**Status:** `PARTIAL`

Scope:

- [x] Minimal Agent Card generation for the loopback information-share capability.
- [x] External A2A Task ↔ internal Run binding with optimistic versions and atomic audit events.
- [x] Bounded Message/DataPart mapping for the M7.0 information-share contract.
- [x] Bounded Task, Artifact, status, cancellation, and subscription mapping for local gateways.
- [x] HTTP+JSON local loopback exchange through the official Rust SDK.
- [x] JSON-RPC and HTTP+JSON local interoperability.
- [x] Official SDK gRPC adapter over the same daemon backend; local conformance/integration scope.
- [x] Bounded streaming, explicit subscriber-lag errors, and explicit reconnect/resubscribe while the owner is alive.
- [ ] Automatic in-flight recovery after daemon restart.
- [x] Bearer-to-subject authorization, explicit loopback-origin allowlists, redirect refusal, bounded payloads and canonical audit.
- [ ] Non-loopback TLS/auth deployment, broader endpoint/SSRF policy, signed cards and push delivery.
- [x] Official TCK and custom bidirectional tests against pinned official Python/Go SDKs for the local gateway, with explicit exclusions.
- [ ] Full official cross-language ITK traversal.

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

#### M7.1 / M8.0 - Local stateful gateway and supervisor acceptance

**Slice status:** `VERIFIED - LOCAL STRUCTURED-DATA SUPERVISOR` on 2026-08-30. Overall M7 and M8 remain `PARTIAL`.

- The daemon owns one canonical writer and delegates through bounded SDK-independent A2A backends. HTTP+JSON, JSONRPC and gRPC share authorization and state contracts; plugin registry/status still cannot mutate Task/Run state.
- SQLite schema 2 adds validated A2A bindings, optimistic versions, idempotency conflicts, independent review receipts and immutable audit records. Schema-1 history migrates forward. Remote Completed cannot declare canonical success.
- Writable role Runs use independent worktrees with private Git-directory owner receipts. Optional model peers read explicitly selected CC Switch profiles in read-only mode; they never receive a core writer or execute model-generated shell/code.
- Three real provider combinations completed planner-worker-reviewer-verifier loops on native Windows, each reaching canonical `done` and joined shutdown. A fresh loop after the final provider API-root correction also passed. The synthetic integer task returned sorted unique values `[-3, 0, 1, 5, 9]` and sum `12`.
- Selected CC Switch profile probes: **9 of 14 passed**. The remaining five were HTTP 401, 403, 404, 429, or invalid structured output. This counts configured profile entries, not distinct suppliers or model-quality rankings.
- Final Rust workspace: **184 passed, 0 failed, 1 ignored** across 49 groups; the ignored child helper is explicitly invoked by its parent test. Format and Clippy passed. Python: 27 pytest tests, Ruff format/lint and mypy passed.
- Final official TCK across three bindings/all levels: **207 passed, 53 skipped, 5 xfailed**, zero failures/errors, pytest and fixture exit 0. Pinned official Python and Go SDK custom bilateral tests passed. Full official ITK, remote/TLS, complete history and cross-restart recovery remain unverified or incomplete.
- Exact commands, source/binary identities, failure history and residual gates are in the dated ledger, [handoff](docs/m7_a2a_handoff.md), [sanitized evidence](docs/evidence/a2a_supervisor_validation.json), [protocol](docs/a2a_supervisor_protocol.md), and [conformance procedure](docs/a2a_conformance_validation.md).

### M8 — Collaboration workflow

**Status:** `PARTIAL`

Scope:

- [x] Bounded local Planner–Worker–Reviewer–Verifier structured-data workflow; real provider acceptance recorded separately from fixtures.
- [x] Immutable Run artifact files and SHA-256 references.
- [ ] General content-addressed artifact store and remote artifact lifecycle.
- [x] Independent reviewer binding and local verifier receipts gate canonical completion.
- [ ] DAG dependencies.
- [ ] Conflict prediction and file-ownership hints.
- [x] Rejected verification creates new repair Runs without rewriting prior evidence.
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

1. Close remaining M4.2 platform, plugin-kind, operator-recovery, and writable-control acceptance gaps before exposing a broader plugin API.
2. Add previous-version protocol fixtures, fuzz gates, and missing CI tooling.
3. Extend the verified supervisor workspace ownership slice to complete Rust workspace/worktree parity.
4. Implement terminal plugins in the order mock, WezTerm/ConPTY, then tmux.
5. Add real vendor harness plugins only after the M4.2 and M5 gates pass together.
6. Perform Python-to-Rust state cutover and expand remote A2A only after the preceding ownership, migration, and conformance gates pass; the explicitly requested local stateful supervisor slice is already verified within its narrower contract.

The M4.2 registry/status slice is integrated and locally tested with mock children. The user-requested M7.1/M8.0 path now proves a local structured-data supervisor loop using real providers, with the necessary owned-worktree subset. Neither establishes full workspace/terminal parity, vendor CLI readiness, remote readiness, or overall M7/M8 completion.

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
| Core cold start to healthy local IPC | p95 < 500 ms after one-time platform security bootstrap on a typical Windows developer machine |
| First Windows control-runtime ACL bootstrap | one-time p95 < 1,000 ms; measured separately from core cold start |
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
- Recorded commit `99d1f8ebcf9e25248eb2af1eb6d64b530e51f848` as the original Python audit baseline.

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

**Status refresh (2026-09-13)**
- The probe and frontend now cover ten agent families (adding qwen, iflow, trae, codebuddy, kimi) with live-verified unavailable reporting for uninstalled launchers; the dashboard ships four selectable themes (classic, high-contrast, mono, light) whose every accent color passes a WCAG AA contrast audit test against its background, supports `t`-key live theme switching and `--json` machine output, and holds golden render fixtures per theme at two sizes. Native-TUI attachment stays intentionally disabled as deferred.

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

### 2026-08-24 — M3 dedicated writer worker

**Status change**
- M3 remains `PARTIAL`; the daemon writer ownership core is implemented, while the user-mode process and IPC transports remain pending.

**Implemented**
- Added the `vibemuxd` library crate with one dedicated named blocking thread that constructs and exclusively owns `SqliteStore`.
- Added an atomic `create_new` lifecycle lock containing only a process/nonce token; a second writer fails closed.
- Added nonce verification before lock removal so one process cannot remove a replaced lock file.
- Added a bounded `sync_channel`, non-blocking enqueue, stable `writer_queue_full` backpressure, response deadlines, and structured worker/store errors.
- Added health, Task/Run commit, projection, ordered event replay, explicit shutdown, restart, and Drop-triggered eventual shutdown paths.
- Kept all SQLite objects inside the writer thread; handles exchange owned domain values and bounded responses only.

**Evidence**
- `vibemuxd`: 7 tests passed
- full Rust workspace: 40 tests passed on both MSRV 1.85 and current stable; rustfmt and Clippy with `-D warnings` passed
- Python regression: 27 tests, Ruff format/lint, and mypy passed
- exclusive ownership: a simultaneous second writer was rejected with `writer_lock_held`
- backpressure: a test-only barrier filled a capacity-one queue and the next request returned `writer_queue_full`
- commit/replay: Task projection and event sequence committed through the worker and survived shutdown/restart
- lifecycle: explicit shutdown removed the lock; dropping the handle released it within the bounded test window
- health JSON round-tripped without exposing a database path

**Compatibility / migration**
- The worker uses the existing private Rust store schema and does not open the Python prototype database.
- No public IPC or daemon command contract is introduced by this slice.

**Security impact**
- Lock contents contain no credential, database path, prompt, environment, or request payload.
- Queue saturation is reported rather than accumulating unbounded memory.
- Store error text is reduced to stable VibeMux error codes across the thread boundary.

**Remaining**
- Add the on-demand `vibemuxd` process owner and authenticated local control protocol.
- Implement Windows named pipe and POSIX UDS transports without unauthenticated TCP fallback.
- Add stale-lock support-bundle diagnostics and an explicit recovery workflow; never delete a stale lock automatically.
- Add crash/restart and abrupt-process termination tests at the process boundary.

**Known risks**
- A process crash intentionally leaves a stale lock and requires explicit diagnosis/recovery before another writer may start.
- Drop requests shutdown but cannot forcibly terminate a blocked OS thread; only explicit shutdown provides joined completion evidence.

### 2026-08-24 — M3 local control IPC plan

**Planning status:** `IMPLEMENTED — TRANSPORT LIBRARY SLICE`

Acceptance gate:

- [x] Windows health/shutdown crosses a real named pipe; POSIX health/shutdown crosses a real UDS in a Linux container.
- [x] No TCP listener is created.
- [x] Descriptor publication occurs after bind and is removed only by its matching owner token.
- [x] A valid client receives versioned health without database path or secret fields.
- [x] Wrong token and wrong protocol version return stable errors without dispatch.
- [x] Oversized frames are rejected before allocation beyond the configured maximum.
- [x] Shutdown responds, closes the listener, joins the server, shuts down the writer, and removes descriptor plus writer lock.
- [x] A second client cannot connect after shutdown.
- [x] `PROGRESS.md` keeps on-demand process/bootstrap unchecked until an actual standalone daemon binary exists.

### 2026-08-24 — M3 authenticated local control transport

**Status change**
- M3 remains `PARTIAL`: the reusable authenticated transport is implemented on both platform families, while a standalone on-demand daemon process and CLI lifecycle wiring remain pending.

**Implemented**
- Added `vibemuxd::control::DaemonControlServer` and `ControlClient` with Windows named-pipe and POSIX UDS backends; no TCP fallback exists.
- Added four-byte big-endian, length-prefixed JSON frames with protocol v1, a 64 KiB frame ceiling, bounded descriptor reads, request IDs, and stable error codes.
- Added a 256-bit bearer token sourced directly from the OS CSPRNG. Descriptor and request `Debug` output redact it, and authentication comparison covers content plus length before dispatch.
- Bound the local listener before publishing `control.json`; POSIX creates that descriptor with mode `0600` and uses a randomized per-instance UDS path. Owner-token verification prevents one server instance from deleting a replacement descriptor or socket.
- Applied bounded read, write, request, and peer-close deadlines so an idle or non-reading local client cannot hold the single control listener indefinitely.
- Routed blocking writer health and shutdown through `spawn_blocking`; no async mutex guard is held across an await.
- Added a bounded peer-close handshake before recycling a Windows pipe instance, preventing `DisconnectNamedPipe` from truncating a response that the client has not consumed yet.
- Limited the initial protocol to `health` and `shutdown`; state mutation remains behind the writer API and is not exposed over IPC in this slice.

**Evidence**
- Windows MSRV 1.85.0 and current stable 1.97.0: the full Rust workspace passed 47 tests on each toolchain; `vibemuxd` contributed 14 tests, and workspace Clippy with `-D warnings` passed on both.
- Windows named-pipe health/auth/version/shutdown round trip passed 20 consecutive stress repetitions after the response-lifecycle fix.
- Linux Docker on the local machine: `vibemuxd` passed 15 tests on Rust 1.85.1 using a real UDS; an earlier 12-test slice also passed on exact MSRV 1.85.0. Unix `0600` descriptor mode, owner-safe socket replacement, and socket cleanup were asserted.
- Python reference regression remained green: 27 pytest tests, Ruff lint/format, and mypy passed.
- Oversized frame tests reject the length prefix before allocating a payload buffer; descriptor replacement and post-shutdown connection refusal are covered.

**Compatibility / migration**
- The protocol is private pre-alpha v1 and has no standalone CLI contract yet.
- Existing Python behavior and database remain reference-only; the control server owns the Rust writer and does not introduce concurrent Python/Rust writes.

**Security impact**
- Token values are absent from `Debug` and error output; unknown peer error text is collapsed to a fixed local code instead of being reflected.
- In this historical transport slice the descriptor remained under ignored `.vibemux/` state; ADR 020 later moved Windows control metadata to a protected per-user runtime. Same-SID process isolation is not claimed.
- Malformed, oversized, unauthenticated, and version-mismatched input cannot reach writer dispatch.

**Remaining**
- Add an explicit inspect/recover command with process-identity proof before stale descriptor/lock removal.
- Add Windows protected-runtime DACL hardening and process-boundary recovery tests.
- Expose state mutation only after authorization, compatibility, cancellation, and bounded concurrency contracts are accepted.

### 2026-08-24 — M3 standalone daemon process and CLI plan

**Planning status:** `IMPLEMENTED — PROCESS AND LIFECYCLE CLI SLICE`

Architecture and migration boundaries:

- `vibemuxd` is an unprivileged foreground binary internally; `vibemuxctl daemon start` is responsible for controlled background launch and readiness polling.
- All paths derive from one canonical project root. Rust writes `.vibemux/vibemux_rust.sqlite3`; the Python reference database `.vibemux/vibemux.sqlite3` is never opened by this slice.
- `vibemuxctl` remains a pre-alpha lifecycle client and does not replace the installed Python `vibemux` executable.
- A valid authenticated health response is the readiness authority. PID and descriptor existence are diagnostics only.
- Existing descriptor/lock artifacts fail closed if they do not converge to health. No automatic delete, PID kill, reset, migration, or database reuse is permitted.

Acceptance gate:

- [x] A real `vibemuxd` child publishes authenticated IPC health, remains alive after its launcher releases the child handle, accepts shutdown, exits successfully, and removes owned descriptor/socket/writer lock artifacts.
- [x] `vibemuxctl daemon start`, `health`, and `stop` work against the real child using controlled argv and emit bounded JSON without path, endpoint, token, prompt, or raw OS-error content.
- [x] A second start returns `already_running`; it does not create a second authoritative writer.
- [x] Simultaneous starts converge on one healthy daemon through the exclusive writer lock.
- [x] Invalid/stale descriptor and lock-only states return stable fail-closed errors and are not automatically modified.
- [x] Startup timeout terminates and reaps only the exact child launched by that call.
- [x] An existing Python `vibemux.sqlite3` sentinel remains byte-identical while Rust creates and uses only `vibemux_rust.sqlite3`.
- [x] Windows launch uses a hidden new process group plus a fixed companion with no user-code interpolation; POSIX uses direct argv and a separate process group.
- [x] Windows named-pipe and Linux-container UDS process tests pass on MSRV-compatible Rust; workspace Rust and Python regressions remain green.

Implementation order:

1. Add canonical daemon project paths and a waitable server lifecycle API.
2. Add the `vibemuxd` binary and process-boundary integration tests.
3. Add the `vibemux_cli` crate and `vibemuxctl` lifecycle commands with injectable executable path and bounded bootstrap configuration.
4. Exercise real Windows CLI start/health/second-start/stop plus stale cases; repeat the process test in Linux Docker.
5. Update architecture, protocol, changelog, and this ledger with exact evidence before committing the implementation slice.

Non-goals for this slice:

- Python database migration or command-parity cutover.
- Automatic stale-artifact deletion or PID-based recovery.
- State mutation, plugin supervision, A2A routing, terminal ownership, autostart, or a privileged system service.

### 2026-08-24 — M3 standalone daemon and lifecycle CLI implementation

**Status change**
- M3 remains `PARTIAL`: normal on-demand process lifecycle is implemented, while explicit stale recovery and the Windows cross-user control-runtime boundary remain release gates.

**Implemented**
- Added canonical `DaemonPaths` rooted at one existing project directory. The Rust process uses `.vibemux/vibemux_rust.sqlite3`; Python remains on `.vibemux/vibemux.sqlite3`.
- Rejected runtime-directory symlinks and Rust database symlink/hardlink aliases to the Python database before the process opens SQLite.
- Added the foreground `vibemuxd` binary, waitable control-server lifecycle, process ID in authenticated health, and process-boundary normal/abrupt-exit tests.
- Added the `vibemux_cli` crate and pre-alpha `vibemuxctl daemon start|health|stop`. Outputs contain only allowlisted status, PID, schema, queue, and healthy fields.
- Added bounded startup/health/shutdown polling, second-start convergence, stale descriptor/lock diagnosis, and a test-only hanging executable proving timeout termination.
- Added a Windows system-PowerShell companion encoded from fixed project source. Paths cross only as environment values; stdin accepts only `release`/`terminate`. `ProcessStartInfo` retains the exact child handle and hidden launch semantics.
- Kept POSIX launch direct and shell-free with null stdio plus a separate process group.

**Root-cause repair**
- A direct Windows `std::process::Command` child retained an unrelated captured-stdout handle, so `vibemuxctl daemon start | ConvertFrom-Json` did not receive EOF until daemon shutdown.
- The fixed companion launches the daemon through hidden `ProcessStartInfo`/`ShellExecute`, holds the exact process until authenticated health, and then releases it. The original captured pipeline now returns immediately while the daemon remains healthy.
- Companion PID read, release, and terminate paths have independent deadlines; readiness timing no longer depends on an unbounded helper read.

**Evidence**
- Windows Rust 1.85.0 MSRV and stable 1.97.0: full workspace 59 tests passed on each; workspace Clippy with `-D warnings` passed on each.
- Linux Docker Rust 1.85.1: 29 current-tree `vibemuxd`/`vibemux_cli` tests passed, including real UDS processes, POSIX process groups, symlink/hardlink rejection, abrupt exit, and timeout termination.
- Windows real CLI pipeline: start -> health -> second start -> stop completed in one captured PowerShell command with one consistent PID and statuses `started`, `running`, `already_running`, `stopped`.
- Windows simultaneous two-client start: both returned successfully against one PID; exactly one reported `started` and one `already_running`.
- Abrupt process termination preserved descriptor and writer lock byte-for-byte; a replacement daemon failed closed without changing either artifact.
- Windows release benchmark, existing Rust database, 1 warmup + 20 samples: start-to-authenticated-health p50 `355.38 ms`, p95 `383.31 ms`, max `403.25 ms`; stop p95 `57.40 ms`.
- Windows release benchmark, 10 fresh project databases: start-to-authenticated-health p50 `369.73 ms`, p95/max `385.14 ms`. Both workloads meet the `< 500 ms` cold-start budget; no speculative optimization was applied.
- Python reference: 27 pytest tests passed; Ruff lint/format and mypy passed.

**Security and migration impact**
- Neither lifecycle binary accepts a public database override, and process/CLI JSON omits paths, endpoint, token, prompt, message, and raw OS errors.
- Startup kills only the process represented by the exact child/companion handle it created. Existing PIDs and stale artifacts are never modified automatically.
- The Windows companion source is fixed and base64-encoded only for transport to system PowerShell; no user value is concatenated into executable code.

**Remaining**
- Harden the Windows descriptor/lock runtime against other standard local users before release claims.
- Keep Task/Run mutation out of lifecycle IPC until authorization and compatibility gates are accepted.

### 2026-08-24 — M3 explicit stale-runtime recovery plan

**Planning status:** `IMPLEMENTED — EXPLICIT METADATA RECOVERY SLICE`

Safety contract:

- `daemon inspect` is read-only and attempts authenticated health before classifying artifacts as stale.
- Recovery requires a separate `daemon recover --confirmation <sha256>` invocation bound to the exact descriptor/lock snapshot.
- A live or merely PID-present owner, malformed artifact, descriptor/lock PID mismatch, symlink, inaccessible path, or changed snapshot always refuses recovery.
- Recovery removes runtime metadata only; it never kills a process and never opens or changes Python/Rust SQLite files.
- Normal `start`, `health`, and `stop` remain fail-closed and never call recovery implicitly.

Acceptance gate:

- [x] Live daemon inspection reports `running`, exposes no confirmation, and recovery refuses without changing artifacts.
- [x] Abruptly terminated daemon inspection reports `recoverable` only after its recorded PID is absent.
- [x] Wrong/missing confirmation leaves descriptor, socket, lock, and databases byte-identical.
- [x] Correct confirmation removes only unchanged stale descriptor/socket/lock artifacts and permits a normal restart with the existing Rust database.
- [x] Descriptor-only and lock-only stale states are recoverable when valid; malformed or descriptor/lock PID-mismatched states fail closed.
- [x] Artifact replacement between inspection and recovery invalidates the confirmation or unchanged-content check.
- [x] A snapshot that names the current/live process is never recoverable; no production PID termination API exists.
- [x] Inspect/recover JSON contains no token, endpoint path, project/database path, lock nonce, prompt, message, or raw OS error.
- [x] Windows named-pipe and Linux UDS process recovery tests pass; MSRV/current-stable Rust and Python regressions remain green.

Implementation order:

1. Add bounded redacted descriptor and writer-lock snapshot APIs in `vibemuxd`.
2. Add process-liveness classification, domain-separated confirmation hashing, and compare-before-delete recovery planning in `vibemux_cli`.
3. Add `daemon inspect` and `daemon recover --confirmation` parsing and allowlisted JSON output.
4. Exercise live, abrupt, wrong-confirmation, changed-artifact, PID-present, and successful restart paths on Windows and Linux.
5. Update architecture, security boundaries, changelog, and this ledger with exact evidence before committing.

Non-goals:

- Force recovery, PID kill, database repair/migration, Windows hostile same-user atomicity, or automatic startup cleanup.

### 2026-08-24 — M3 explicit stale-runtime recovery implementation

**Status change**
- M3 remains `PARTIAL`: normal lifecycle and explicit stale metadata recovery are implemented; the Windows cross-user control-runtime boundary remains a release gate.

**Implemented**
- Added bounded `ControlArtifactSnapshot` and `WriterLockSnapshot` APIs. Debug output redacts descriptor token, endpoint and raw lock nonce; snapshot hashes bind the exact original bytes.
- Added cross-platform PID presence checks pinned to the latest MSRV-compatible `sysinfo` (`0.36.1`). PID presence always blocks and no production kill API exists.
- Added domain-separated SHA-256 recovery confirmations covering descriptor and lock presence/content. `recover` recomputes the plan and rejects missing, malformed, stale, or changed confirmations.
- Added unchanged-content cleanup for descriptor, POSIX socket and writer lock. Cleanup never opens either database and preserves partial-failure evidence for a fresh inspection.
- Added `vibemuxctl daemon inspect` and `daemon recover --confirmation`; JSON exposes only allowlisted booleans, status/reason, protocol/endpoint kind, PID presence and confirmation.
- Added a POSIX reaper thread so library-mode launchers do not leave a released child as a zombie that would falsely block recovery.

**Evidence**
- Windows Rust 1.85.0 MSRV and stable 1.97.0: full workspace 69 tests passed on each; workspace Clippy with `-D warnings` passed on each.
- Windows real process: live inspect returned `running` with no confirmation and recovery was blocked.
- After exact test-daemon termination, inspect returned `recoverable` with `process_present=false`; wrong confirmation returned `daemon_recovery_confirmation_mismatch` and preserved both artifacts.
- Correct confirmation removed descriptor and writer lock, left the Rust database SHA-256 unchanged, then the same database restarted and stopped normally.
- Windows automated tests cover live/PID-present blocking, descriptor-only, lock-only, PID mismatch, changed artifact, malformed artifact, wrong confirmation, successful recovery/restart and database preservation.
- Linux Docker Rust 1.85.1: 39 current-tree daemon/CLI tests passed, including real UDS abrupt recovery, socket cleanup, POSIX reaping and database-preserving restart.
- Post-recovery-dependency release benchmark, 1 warmup + 20 samples: start-to-authenticated-health p50 `354.97 ms`, p95 `383.48 ms`, max `425.21 ms`; stop p95 `64.25 ms`. The `< 500 ms` budget remains met without optimization.
- Python reference: 27 pytest tests passed; Ruff lint/format and mypy passed.

**Security boundaries**
- Confirmation is a non-secret checksum; authentication token, endpoint path, project/database path and raw nonce remain absent from reports and errors.
- Recovery is compare-before-delete under ordinary same-user filesystem assumptions. It does not claim hostile same-user atomicity or Windows ACL isolation.
- There is no force flag, implicit recovery, PID kill, database deletion, reset or migration path.

**Remaining**
- Move Windows control metadata to a protected current-user runtime and verify cross-user DACL/remote-client boundaries before changing the release claim.
- Keep Task/Run mutation out of lifecycle IPC until authorization, compatibility and cancellation gates are accepted.

### 2026-08-24 — M3 Windows control-runtime ACL plan

**Planning status:** `IMPLEMENTED — WINDOWS PER-USER CONTROL BOUNDARY`

Threat-model correction:

- Windows ACLs isolate SIDs, not processes under one SID. Same-logon agents are the intended collaboration domain and are not treated as mutually hostile.
- The implementable release boundary is: current user + `SYSTEM` + administrators may access control metadata; other standard local users and remote named-pipe clients may not.
- Project/database filesystem confidentiality remains governed by the repository/drive ACL and is not silently redefined by VibeMux.

Acceptance gate:

- [x] Windows control descriptor and cooperative writer lock live under a canonical `%LOCALAPPDATA%\VibeMux\runtime\<sha256>` leaf; the hash output contains no project path text.
- [x] The runtime root DACL is protected and contains only current-user, `LOCAL_SYSTEM`, and built-in-administrator allow rules; hashed leaves inherit no broad principals.
- [x] The actual descriptor and lock inherit only allowlisted effective identities; bearer token/path/SID/raw ACL text remain absent from CLI JSON and logs.
- [x] Named-pipe construction explicitly rejects remote clients and still requires the bearer token for same-SID clients.
- [x] A healthy legacy project-local daemon remains discoverable and stoppable; stale legacy artifacts are inspectable/recoverable with the existing confirmation flow.
- [x] New daemon startup refuses while any unresolved legacy descriptor exists and holds both protected and legacy locks, preventing old/new concurrent writers.
- [x] Windows real start/health/inspect/stop/recover tests pass against the protected runtime; a structural ACL test proves broad principals are absent.
- [x] Linux UDS paths and `0600` semantics remain unchanged; cross-platform process/recovery tests pass.
- [x] Post-bootstrap release startup p95 remains below `500 ms`; first-machine ACL initialization is separately measured and documented. MSRV/current-stable Rust and Python regressions remain green.

Implementation order:

1. Add a small `vibemux_platform` crate for fixed Windows PowerShell execution and protected-user-directory ACL creation/inspection.
2. Split daemon state paths from control-runtime paths and add a domain-separated project hash.
3. Move process-owned descriptor/cooperative lock to the protected runtime while retaining the project-local Rust database.
4. Add legacy descriptor/lock discovery to start, health, stop, inspect, and recover without automatic deletion.
5. Run actual ACL, lifecycle, upgrade, recovery, Linux, performance, and regression checks; update the ledger before committing.

Non-goals:

- Same-SID process isolation, admin/SYSTEM exclusion, repository data encryption, native unsafe ACL APIs, or automatic legacy cleanup.

### 2026-08-24 — M3 Windows per-user control-runtime implementation

**Status change**
- M3: `PARTIAL` -> `VERIFIED`. Persistence, one-writer ownership, process lifecycle, authenticated local IPC, explicit stale recovery and the declared Windows cross-user boundary now have repeatable evidence.

**Implemented**
- Added `vibemux_platform` as an `unsafe`-free platform boundary with fixed, encoded system-PowerShell ACL construction and verification plus bounded helper execution.
- Protected `%LOCALAPPDATA%\VibeMux\runtime` once with exactly three full-control allow identities: current user SID, `SYSTEM`, and built-in Administrators. Hashed project leaves, descriptor and protected lock inherit only those effective rules.
- Added a domain-separated SHA-256 project runtime key; CLI/health/recovery output never contains the project path, runtime path, SID or raw ACL.
- Moved the Windows descriptor and primary writer lock out of broadly inherited project storage. The Rust/Python databases remain separate and project-local.
- Explicitly configured named pipes with remote-client rejection and a bounded two-instance handoff; stable state has one listener while handoff avoids response truncation.
- Added dual-generation locks. New daemons acquire legacy then protected locks, so an old binary cannot race a new binary to the same SQLite writer.
- Added healthy legacy health/stop compatibility, confirmation-bound stale legacy recovery, mixed-generation refusal and direct-daemon legacy descriptor refusal.
- Kept POSIX project-local descriptor mode `0600`, randomized UDS, socket cleanup and single lock unchanged.

**Evidence**
- Windows ACL before change: project `.vibemux` inherited `Authenticated Users: Modify`; `%LOCALAPPDATA%` still exposed a sandbox-group read ACE.
- Windows ACL after change: protected root, hash leaf, live descriptor and live protected lock each had exactly three allow identities and zero Everyone/Authenticated Users/Users/sandbox principals.
- Windows real lifecycle returned `started -> running -> stopped` on one PID; live inspection reported `runtime_generation=current` and a valid compatibility lock.
- Automated upgrade tests proved a healthy legacy daemon is discoverable/stoppable, stale legacy state recovers into protected runtime, mixed generations fail closed, and dual locks reject a legacy writer.
- Windows Rust 1.85.0 MSRV and stable 1.97.0: full workspace 75 tests passed on each; workspace Clippy with `-D warnings` passed on each.
- Linux Docker Rust 1.85.1: 40 current-tree daemon/CLI tests passed; Linux Clippy with `-D warnings` passed. POSIX path, `0600`, UDS, recovery and process semantics remained green.
- Python reference: 27 pytest tests passed; Ruff lint/format and mypy passed.
- Release after security bootstrap, existing database, 20 samples: start-to-health p50 `372.02 ms`, p95 `429.75 ms`, max `457.13 ms`; stop p95 `70.52 ms`.
- Release on 10 fresh projects under an already protected root: p50 `366.23 ms`, p95/max `438.26 ms`, within the `< 500 ms` budget.
- First machine ACL-root initialization, 5 isolated samples: median `852.32 ms`, mean `858.60 ms`, max `896.06 ms`; this one-time security bootstrap is tracked against the separate `< 1,000 ms` budget.

**Security claim**
- Other standard local users and remote named-pipe clients are outside the allowed control plane. Same-logon-SID agents are trusted collaborators; administrators and `SYSTEM` retain OS authority.
- Bearer authentication remains required for every request. No token, endpoint path, project path, SID or ACL text is emitted by lifecycle/recovery JSON or errors.
- No core crate contains `unsafe`; native handle-level same-SID isolation is neither implemented nor claimed.

**Next milestone**
- Begin M4 plugin protocol/supervisor work. Task/Run mutation remains absent from lifecycle IPC until M4 authorization, compatibility, bounded-channel and cancellation contracts are accepted.

### 2026-08-24 — Post-PR #1 documentation reconciliation

**Status reconciliation**

- Updated the audited implementation baseline to PR #1 head `fce05cffe628ce65c49620a06f6bd1ef7eaadb89`, merged as `6498e6aa711f751d9f9e50e37034d61c17d4b341`. The earlier `99d1f8e` commit remains only the original Python audit baseline.
- Recorded Python package version `0.1.0a0`, Rust workspace version `0.2.0-alpha.0`, Rust edition 2024, and MSRV 1.85.
- Reconciled the current overview with the later evidence ledger: M3 is `VERIFIED`; M4 remains `PARTIAL`; M4.0 protocol and M4.1 process-supervisor foundations are verified only within their isolated scopes.
- Recorded PR #1 head GitHub Actions run #8 as successful for the configured Windows/Ubuntu Python and Rust jobs. This does not imply that nextest, deny, audit, fuzz, or live-backend gates ran.
- Confirmed that `vibemuxd` does not depend on `vibemux_plugin_supervisor`; daemon-owned plugin registry/control integration, restart budget/quarantine, and real vendor plugins remain unimplemented.
- Replaced the obsolete execution order with M4.2 daemon integration and M5 workspace/terminal parity as the next implementation priorities.

**Impact**

- Documentation only. No runtime, SQLite schema, plugin wire protocol, A2A behavior, or CLI behavior changed in this reconciliation.

### 2026-08-30 — Documentation branch integration review

**Scope and corrections**

- Reviewed documentation commit `9df83b36e9d7ea9532a44b1ede078bb12d7b4e9e` against `main` at `6498e6aa711f751d9f9e50e37034d61c17d4b341`; the integration changes 14 Markdown files and no runtime sources, dependencies, or schemas.
- Corrected the recovery confirmation to 64 hexadecimal characters representing SHA-256, made the PowerShell placeholder parseable, and documented the project-local compatibility writer lock.
- Fixed the Rust CLI table row, distinguished planned architecture from implemented behavior, and clarified the existing loopback A2A and descriptor-only/lock-only recovery scopes.

**Validation executed**

- Native Windows, pinned Rust 1.85.0: `cargo test --locked -p vibemux_plugin_protocol -p vibemux_plugin_supervisor --all-features` passed 28 tests, including real mock-child process and failure-path tests.
- `cargo test --locked -p vibemux_cli --lib recovery::tests::confirmation_is_stable_and_fixed_length -- --exact` passed the targeted confirmation-format test; the other 12 library tests were filtered out.
- `cargo fmt --all -- --check`, `git diff --check`, and `git diff --cached --check` passed. Local Markdown link/table checks, PowerShell example syntax checks, and an added-line credential/private-path signature scan also passed.
- Freshly inspected [PR #1 head CI run #8](https://github.com/YueDongSun/vibemux/actions/runs/32689344465): all four configured Python/Rust Windows/Ubuntu jobs succeeded for `fce05cffe628ce65c49620a06f6bd1ef7eaadb89`. Its implementation tree matches the merge baseline; this is historical CI evidence, not a new CI run for the documentation integration.

**Limits**

- This documentation review adds no runtime capability or platform-support claim. The full workspace/Python suites, Linux/WSL tests, live terminal backends, benchmarks, and A2A conformance were not rerun in this review.

### 2026-08-30 - M4.2 daemon registry and read-only plugin status

**Baseline and scope**

- Implemented on `codex/m4_2_plugin_registry` from `85697d6`; local uncommitted delivery. Existing untracked `.weave/` is excluded and untouched. No commit, push, installation, or deployment was performed.
- Added explicit daemon-owned startup registration, bounded per-plugin lifetime restart budgets/backoff/quarantine, heartbeat monitoring, read-only authenticated IPC v2 status, and joined shutdown before writer release. Task/Run operations remain prohibited.
- Supervisor cleanup now explicitly kills/reaps and joins I/O on failure, accepts queued drain-legal traffic before shutdown acknowledgement, and validates shutdown session correlation. Handshake failure reaping is deadline-bound.
- Independent review identified and tests reproduced a queued-frame shutdown false timeout and a v1 authentication error incorrectly returned as a version error; both regression tests now pass. Heartbeat grace is three negotiated intervals in startup configuration, and registry validation rejects deadlines no greater than the negotiated interval.

**Validation executed (native Windows, pinned Rust 1.85.0)**

- `cargo test --locked --offline --workspace --all-features`: **126 passed, 0 failed, 0 ignored** across 36 test/doc-test groups. Includes 9 daemon registry integration tests, 13 supervisor process integration tests, the real standalone daemon/config/plugin path, existing writer/CLI/platform tests, and protocol fixtures.
- `cargo clippy --locked --offline --workspace --all-targets --all-features -- -D warnings`: passed on the final Rust implementation.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed; final added-line privacy and local Markdown link checks passed.
- `cargo nextest run --workspace --all-features`, `cargo deny check`, and `cargo audit`: each attempted and returned exit 101 because the corresponding cargo subcommand is not installed. These gates are **unverified**, not passed.

**Observed boundaries**

- Real crashing mock children exhausted exactly the configured restart counter and remained quarantined across status reads. A healthy sibling retained its session and the writer health endpoint remained responsive.
- Malformed frames and unsolicited Task/Run-shaped plugin events quarantined without retries; seeded canonical Task projection/events remained unchanged, and no Run projection appeared. A second writer remained refused while the daemon held the lock.
- Active, handshake, and backoff cancellation prevented new restarts. A cancelled daemon wait requested cleanup without aborting its owner. Graceful and forced cleanup tests inspected OS process absence after join; forced shutdown preserved its error.
- Periodic heartbeat traffic survived multiple deadlines; a silent mock quarantined at zero restart budget. Invalid configuration batches created no descriptor, database, or writer lock; duplicate registration did not replace a session or reset policy.
- Read-only status rejects unauthenticated access and mutation operation names; worst-bound snapshots fit within the existing 64 KiB frame limit. Status contains no prompt, environment values, argv, or workspace path.

**Limits / next work**

- M4 remains `PARTIAL`. M4.2 evidence is native Windows only; Linux/WSL, live WezTerm/tmux/ConPTY, vendor plugins, fuzz/soak, additional plugin kinds, sandbox enforcement, and release/production readiness are not claimed.
- No Python source changed; Python checks were not rerun. No new CI run was triggered. Required nextest/deny/audit tooling remains unavailable locally.
- Budgets/quarantine are ephemeral and reset with a new daemon lifetime. Operator quarantine recovery, writable plugin control, persisted lifecycle audit, descendant-process cleanup, and abrupt-daemon-exit cleanup remain future work.
- Local IPC advances to v2 with v1 health/shutdown request compatibility. Old v1 clients reject v2 descriptors and require upgrade. SQLite and plugin wire/manifest schemas are unchanged; rollback is normal daemon stop and restart without plugin configuration.

### 2026-08-30 - M7.1/M8.0 local A2A and real supervisor acceptance

**Scope and authority**

- Implemented on `codex/m7_a2a_supervisor` over `85697d6`, retaining the earlier uncommitted M4.2 work. This is an uncommitted source delivery, not a release or CI result. The [source manifest](docs/evidence/a2a_source_manifest.json) fingerprints the tested implementation inputs.
- Local machine/model testing was explicitly authorized. Only selected CC Switch API profiles and synthetic task data were used; provider defaults, credentials and global settings were not changed. Plugin status remains read-only; canonical A2A commands use the daemon's existing single writer.
- Added official-SDK task adapters, owned bounded execution/streams, subject authorization, schema-2 binding and verification transactions, owned Run workspaces, separate model peers and explicit supervisor startup/one-shot CLI. See [ADR 024](docs/adr/024_stateful_a2a_supervisor.md).

**Validation executed**

- Native Windows 11 Pro build 26200, x86_64, Rust 1.85.0: `cargo test --locked --offline --workspace --all-features -j 2` passed **184 tests**, with **0 failures and 1 ignored standalone child helper** across 49 test/doc-test groups. The parent invokes that helper explicitly.
- `cargo fmt --all -- --check` and `cargo clippy --locked --offline --workspace --all-targets --all-features -j 2 -- -D warnings`: passed.
- Task-isolated CPython 3.13.12: `python -m pytest` passed **27**; `python -m mypy .` passed **21 source files**; `python -m ruff check .` and `python -m ruff format --check .` passed, with **64 files formatted**. No ignores, exclusions or weakened assertions were introduced.
- `cargo nextest run --workspace --all-features`, `cargo deny check`, and `cargo audit`: attempted, each exit 101 because its cargo subcommand is absent. These gates remain unverified.
- Final `tools/run_a2a_tck.py` against unmodified official TCK commit `5996b79f9cefa6fc390980e383e358a66fb9e49e`: **207 passed, 53 skipped, 5 xfailed**, all requirement levels, HTTP+JSON/JSONRPC/gRPC, no failures/errors; pytest/fixture exit 0, no early fixture exit or forced cleanup. Exclusions and binary hashes are in [conformance evidence](docs/a2a_conformance_validation.md).
- Custom official Python SDK v1.1.3 and Go SDK v2.5.0 bilateral tests passed all three bindings. Go ran three tests without skips, plus `go vet .` and `go mod verify`. These are not the full official ITK.
- Real process regression tests cover valid completion, rejected review/new repair Runs, active and review-stage cancellation, gRPC daemon calls, sibling fault isolation, bounded queues/streams, shutdown, migration/authority, real Git ownership and dirty/mismatched cleanup refusal. Existing plugin crash-budget/quarantine/status tests remain green.

**Live provider and supervisor evidence**

- Fourteen selected API profile entries were probed; nine passed. Final negative outcomes are HTTP 401/403/404/429 and one strict-JSON contract failure. OAuth-only, unknown APIs and the local large-model profile were not tested. Requested model IDs do not attest to the provider's actual weights or routing.
- Three real loops used independent configured provider combinations; public evidence now uses opaque model/profile aliases, while original selection details remain private. All reached canonical `done`, one attempt each, with independent review, exact local verification and joined daemon/peer shutdown.
- Final provider adapter repair preserved configured `/v3` and `/v4` API roots instead of inserting `/v1`. Its wrong-path regression was reproduced; corrected live probes yielded one pass and one HTTP 429. The final rebuilt binaries then completed another real supervisor loop. Earlier timeout/404/numeric-contract failures remain separate local receipts, not rewritten successes.
- Live tests used a task-created synthetic repository, with unchanged main HEAD and tracked files. Run artifacts and private raw receipts are intentionally retained as evidence. No user repository content or credentials were sent as task data.

**Limits and delivery**

- Only the local structured-data M7.1/M8.0 slice is verified. Overall M4/M7/M8 remain partial: no complete official ITK, remote/TLS deployment, full history, inbound restart recovery, live terminal/vendor CLI, sandbox enforcement, fuzz/soak or new CI/publication claim.
- SQLite schema migration is forward-only; do not run an old writer against a schema-2 database. Python migration/default cutover and stable public API compatibility are not implemented.
- An unrelated concurrent `rust-analyzer` component addition in `rust-toolchain.toml` was preserved and is excluded from this task's authorship. The initial excessive-parallelism build failed under memory pressure; final builds used `-j 2`, without stopping unrelated processes.
- Detailed delivery, resource ownership, security boundaries and reproduction commands are in [the handoff](docs/m7_a2a_handoff.md). No commit or push was performed; `.weave/` remains ignored and untouched.

- Final handoff audit: diff/cached-diff whitespace checks passed; 13 changed Markdown files had no broken local links; five JSON artifacts parsed; no added private-path/key signatures were found; all 102 implementation fingerprints matched. Three task-created developer worktrees were archived and removed without force, with branches retained. The implementation worktree and intentionally retained private live-test evidence remain.

### 2026-08-31 - Feature-branch publication preparation and privacy audit

**Authorized scope**

- The user authorized committing and pushing the M4.2 plus M7.1/M8.0 implementation. Destination: `origin`, branch `codex/m7_a2a_supervisor`. This is feature-branch source publication, not a main merge, tagged release, deployment or declaration that remaining milestone gates passed.
- The unrelated local `rust-toolchain.toml` addition of the editor component remains unstaged and excluded. Rust edition/MSRV/runtime source are unchanged. Earlier dated uncommitted/no-push statements describe those earlier handoffs, not the later publication decision.

**Privacy and evidence handling**

- Reviewed the entire staged source/docs/tools snapshot for credential patterns, private paths, private endpoint literals and accidentally tracked runtime/config/database files. Initial matches were synthetic security fixtures: rejected URL userinfo, wildcard/test-network IPs and test bearer constants, not operational credentials.
- Public supervisor evidence schema 2 replaces actual configured model values and generated reviewer Run identity with opaque aliases. Real selection mappings, provider URLs/keys, private configuration, raw reports and artifacts remain local. Receipt hashes still identify original private receipt bytes; redaction does not alter success/failure counts.
- The source manifest records the original tested filesystem bytes. Publication evidence separately fingerprints normalized staged implementation inputs and identifies the excluded local toolchain difference. This prevents a historical working-tree hash from being presented as a final commit hash.
- `.weave/`, local provider/runtime directories, secrets and database files remain ignored; shareable example JSON remains trackable and uses placeholders. No history rewriting, credential rotation or global provider changes were performed.

**Fresh validation**

- `cargo test --locked --offline --workspace --all-features -j 2`: **184 passed, 0 failed, 1 ignored** across 49 groups; the ignored helper is invoked by its parent. `cargo fmt --all -- --check` and workspace/all-targets/all-features Clippy with `-D warnings`: passed.
- Existing isolated CPython environment: pytest **27 passed**; mypy **21 source files** passed; Ruff check and format passed, **65 files formatted**. No model calls were needed for this publication check.
- The 102 historical implementation fingerprints matched before publication preparation. Runtime source has not changed since the recorded final TCK, bilateral interoperability and real-model acceptance; those remain explicitly dated evidence, not rerun claims.
- `cargo nextest run --workspace --all-features`, `cargo deny check`, and `cargo audit` were freshly attempted; each returned exit 101 because the command is not installed. No full release-gate pass or new CI success is claimed.
- The repository-specific `scripts/inspect_git_release.py` inspector is absent. Explicit Git status/branch/worktree/remote inspection, exact-index scans and independent isolated privacy review are used instead; they are not a claim that the absent inspector ran.

Publication authorization does not resolve the documented Linux/WSL, full ITK, remote/TLS, terminal/vendor-CLI, recovery, fuzz/soak or auxiliary-tooling gaps. See [publication evidence](docs/evidence/publication_validation.json) for the prepared snapshot and validation identity; the observed remote SHA is reported only after the push is verified.

- Final exact-index checks: 77 intended files; 13 Markdown files and six JSON files validated; no broken staged local links; diff/cached-diff whitespace and ignore/example checks passed. Independent review in a separate worktree found no newly introduced operational credentials, private endpoints/profile IDs, machine-user paths or raw runtime/config artifacts. The only unstaged file is the unrelated toolchain component edit.

<<<<<<< HEAD
### 2026-09-04 - Frontend agent-table truncation fix

**Bug**
- On any machine with verified agents, the unified dashboard's combined `L:.. A:.. I:..` cell (30-31 chars of real data) rendered into a fixed 28-char column, truncating the inference state to `I:not_r` for all five agents; long vendor version strings were clipped as well. Unit tests masked this because their toy strings were exactly 28 chars.

**Fix**
- Split the agent table into per-state columns (`Launcher`/`Auth`/`Infer`, each 11 chars so `unavailable` fits whole) with `Version` taking remaining width and `Route` unchanged.
- Raised the side-by-side layout threshold from 110 to 160 columns so common 80-132 column terminals stack vertically and the table keeps full width.
- Added `real_probe_states_and_versions_are_never_clipped` regression test covering real version shapes at 80/120/160 widths; verified live on a 120-column Windows console.

**No contract change**: `ProbeReport`, `DashboardModel::plain_snapshot`, `--once` output, and slot semantics are untouched.

### 2026-09-15 (1) - Unix control socket moved to system temp (deep-root bind fix)
- On Linux, the unix control socket was bound inside the project runtime dir as `.vibemux/vibemux_control_<32 hex>.sock`. Linux caps unix socket paths at 107 usable bytes (`sun_path` is 108 including NUL), so any project root nested deeper than ~50 characters failed at `UnixListener::bind` with `EndpointUnavailable`. Reproduced on WSL2 (Ubuntu 22.04, the documented development environment): the supervisor integration tests run under ~100-character tempdir prefixes and failed 3/3 with the same signature as the rust-ubuntu CI runner; Windows named pipes are unaffected.

**Fix**
- `endpoint_name()` (unix arm) now returns a fixed system-temp location: `<temp_dir>/vibemux_ctl_<key16>_<uuid32>.sock`, where the key is the first 16 hex of the existing `project_runtime_key(runtime_dir)` SHA-256 (project-scoped) and the UUID preserves per-instance isolation. Total length is ~66 bytes under `/tmp`, depth-independent.
- The bound socket is chmod 0600 (fail-closed on chmod error) since the shared temp dir is multi-user; the per-frame auth token in the 0600 descriptor remains the security boundary. The descriptor (still in the runtime dir) is the single source of truth for endpoint + token, and clients read the endpoint from it — no client change.
- Regression tests: endpoint length/location for a 200-char-deep runtime dir, per-instance and per-project uniqueness, and socket-file mode asserted in the unix health round-trip test.

**Evidence**
- WSL2 before fix: `cargo test -p vibemuxd --all-features` — supervisor_grpc 3/3 failed (`EndpointUnavailable`), matching rust-ubuntu CI byte-for-byte. After fix: `default_daemon_has_no_a2a_endpoints` passes on WSL2; full WSL workspace gate runs at the pre-push validation step. Windows `cargo test -p vibemuxd --all-features` on this change: all control unit tests (33), daemon_process (2), plugin_registry (9), supervisor_grpc (3), supervisor_workflow (4) passed; the unix-gated tests compile only on unix.

### 2026-09-15 (2) - Worktree inventory parses without the Git 2.37-only -z flag

**Bug**
- `WorkspaceManager` ran `git worktree list --porcelain -z`, but `-z` for worktree list requires Git 2.37+. WSL2/Ubuntu 22.04 — the documented development environment — ships Git 2.34 as its LTS default, which fails with `error: unknown switch 'z'`, surfacing as `WorkspaceError::Git` and failing the supervisor workflow/grpc integration tests (4/4 and 2/3 on WSL2 after the socket fix above). `git status --porcelain -z` is supported since Git 1.7 and is unchanged.

**Fix**
- Dropped `-z` from the worktree-list invocation only; `inventory_matches` now parses LF-delimited porcelain records (blank line separates entries).
- Fail-closed safety: a record path or branch containing `\n` is rejected before matching (a newline could otherwise forge field lines). Owned worktree paths are generated UUID-suffixed names and never contain a newline, so no legitimate record is affected.
- New unit tests: LF inventory exact-match / duplicate / locked / detached rejection, missing trailing blank line, and newline-path/branch rejection.

### 2026-09-12 - Frontend themes, writer observability, concurrent control plane

**Implemented**
- Added `vibemux_frontend` dashboard themes: classic, high-contrast, and mono (no foreground colors), selectable via `--theme` or `VIBEMUX_FRONTEND_THEME`; footer shows the active theme.
- Added writer scheduling observability: `WriterHealth` reports `queue_depth`, `queue_high_watermark`, and `queue_saturated`; a saturated queue answers health from an atomic snapshot without a worker round trip; `vibemuxctl daemon health` exposes the metrics.
- Fixed control-plane head-of-line blocking: control connections are handled concurrently with a per-connection task; graceful shutdown drains in-flight connections via `JoinSet` before the writer closes; Windows pipe instance ceiling raised to 16.

**Evidence**
- Frontend: golden render fixtures for all themes at 80x24 and 120x32 guard against visual drift; real-terminal ANSI captures verified per-theme color signatures; Rust workspace 174 tests green.
- Daemon: regression test `idle_client_does_not_block_other_control_traffic` fails against the old serial accept and passes after the fix.
- Live Windows e2e via `vibemuxctl`: daemon start reported `queue_depth=0`, `queue_high_watermark=1` (sampling floor), `queue_saturated=false`; five concurrent health clients all answered with identical metrics; stop removed the endpoint and subsequent health returned `daemon_not_running`.

### 2026-09-12 (2) - Scheduling audit rounds 5-8

**Implemented**
- Event-driven `wait_remote`: supervisor waits on the A2A SSE subscription instead of polling; subscription rejection, transport errors, and streams ending without a terminal event degrade to the bounded polling path (250ms), preserving the wait contract.
- Extracted `POLL_INTERVAL` (250ms) for the fallback polling cadence, cutting poll RPC volume roughly 60% versus 100ms.
- `vibemux-frontend --json` emits the versioned probe report for scripts and CI; CLI integration tests cover the flag matrix and error paths.
- Live Windows e2e recorded: five concurrent `vibemuxctl daemon health` clients with consistent queue metrics; clean stop.

**Reviewed, no change needed**
- Supervisor repair-loop budget: `max_repairs <= 2` is enforced in work-order validation (at most three worker/reviewer rounds), each role run carries the 300s deadline, cancellation is checked per round, and peer cleanup failures surface as errors. No unbounded-loop risk.

### 2026-09-12 (3) - Probe parallelism, visual polish, mock terminal audit

**Implemented**
- `run_probe` runs the five agent version probes concurrently (`join_all`, order-preserving) instead of serially; worst-case startup drops from the sum of five 10s deadlines to the slowest single probe. Measured `--once` wall time on a live Windows machine: 2.1-3.1s.
- Diagnostics panel colors the Gateway and A2A summary lines by probe state; `DashboardModel` retains both states for the renderer. Text content unchanged, so golden fixtures still pass.
- Agent table version cells ellipsize against the real column budget (fixed columns 55 + five 1-cell gaps + borders subtracted from the table interior) instead of a hard mid-token cut; wide tables keep full version strings. 80-column golden fixtures regenerated.
- Frontend `--json` mode and CLI integration tests recorded under the frontend theme entry.

**Reviewed, no change needed**
- Python mock terminal backend: pane inventory persists through a temp-file atomic replace with schema validation and explicit corruption errors; concurrent last-writer-wins is acceptable for the offline mock test facility and does not touch authoritative SQLite state.

### 2026-09-12 (4) - Terminal backend command scheduling review

**Reviewed, no change needed**
- WezTerm backend: every operation is one `wezterm cli` invocation (spawn/list/send/kill/activate), which is the supported control path; `stop` verifies pane existence via `list` before killing, and probes never run user text through a shell.
- `send_text` with `submit=True` issues two invocations (text, then Enter). Merging into one `send-text` call with an appended carriage return would halve submit latency but changes byte-chunk boundaries seen by the target program; the Python reference and Rust backend lock this contract, so the change needs a dedicated cross-language review rather than a loop iteration.
- Mock terminal backend: temp-file atomic replace with schema validation (audited separately, no change needed).

### 2026-09-12 (5) - Probe endpoint discovery audit

**Reviewed, no change needed**
- Endpoint discovery reads only fixed, allowlisted configuration paths per agent (Claude/Codex/OpenCode/Grok) and only allowlisted keys; Copilot declares none. `SafeEndpoint` parsing rejects credential-bearing URLs, and route classification treats loopback case-insensitively via `IpAddr::is_loopback` with empty sets mapping to `unknown`.
- Gateway probing targets fixed `127.0.0.1` with `no_proxy` and a bounded timeout; no user-controlled URL enters the transport. Consistent with the M5.0 acceptance gate on SSRF-safe, read-only evidence collection.

### 2026-09-12 (6) - Visual polish rounds 18-22 summary

**Implemented**
- Telemetry line colored by failure semantics (absent muted, zero failures green, any failure bright red); failed agent rows render bold for scan visibility.
- WCAG AA contrast audit as a permanent test: every state color, title, tabs, route, and muted color must hold 4.5:1 on dark backgrounds. The audit caught the classic dark red at 3.60:1, fixed to the bright red (5.25:1) across agent states, health, and telemetry.
- Mono theme emphasis ladder extended from the state columns to the slot panel (reserved bold, unavailable dim), keeping the no-foreground-color invariant.
- README documents themes, `--json`/`--once` output, and golden fixture regeneration.
- Light theme live-verified on Windows: real-terminal ANSI capture shows the truecolor dark palette (`38;2;0;110;0`, `38;2;150;75;0`, `38;2;96;96;96`) instead of the 256-color indices the dark themes use, confirming the palette ships as designed.

### 2026-09-12 (7) - Ten-agent probe live verification

**Evidence**
- `vibemux-frontend --once` on Windows: Claude/Codex/OpenCode/Copilot/Grok verified with real versions and routes; Qwen/iFlow/TRAE/CodeBuddy/Kimi report `launcher_unavailable` + `unknown` (not installed on this machine); overall aggregates `5 verified, 5 unavailable`; ten reserved native-TUI slots render.
- Ledger catch-up: rounds 18-25 committed telemetry failure coloring, failed-row emphasis, WCAG contrast audit (classic dark red fixed, light theme), mono emphasis ladder, light theme, README theme docs, and ten-agent probe extension.
- Round 28-29 fixes: narrow-layout height rebalance keeps all ten agent rows visible at 120x32+ (80x24 fits six, physical limit recorded in golden fixtures), and the slot panel renders wrapped per-slot styled lines at every width (the single-row Tabs truncated ten reservations).
- Round 37: interactive theme hot-switching via the `t` key (`Theme::next` cycles all four themes in place; footer shows the hint), plus the flag-matrix CLI tests.
- Rounds 30-32: light theme completed with the dark-cyan correction (title/route/slot cyan at 1.57:1 on white replaced by Rgb(0,110,110) at 7.4:1), the WCAG audit now covers every accent of all four themes against their backgrounds, and CLI integration tests pin the `--theme` x `--json`/`--once` flag matrix.

### 2026-09-12 (8) - Supervisor service and model peer audit

**Reviewed, no change needed**
- `SupervisorService`: dual HTTP+gRPC transports start with full cleanup on partial failure and shut down in order (accept surfaces, then executor runtime); credentials validate before use; the supervisor config reader enforces a 64 KiB bound and rejects non-regular files.
- `TaskRuntime::start` concurrency of 1 serializes supervisor workflows by design; raising it is a configuration decision, not a defect.
- `vibemux_model_peer` bootstrap: single-line stdin with a 32 KiB take plus 16 KiB line cap, `deny_unknown_fields`, and CC Switch reads off-runtime via `spawn_blocking`.

### 2026-09-12 (9) - Python command boundary audit

**Reviewed, no change needed**
- `command_runner.py`: the single execution boundary enforces shell=False, argv/stdin null-byte rejection, an environment-key allowlist pattern, positive deadlines, and typed OSError vs timeout errors.
- `agent_host.py`: harness hosting passes argv directly (no shell), caps environment JSON at 16 KiB with full type validation, and documents its two deliberate compat decisions (inherited environment, no timeout for long-lived harness processes) as temporary boundaries the Rust plugin permission model will replace.

### 2026-09-12 (10) - Model peer process lifecycle audit

**Reviewed, no change needed**
- `model_peer_process.rs`: peer processes spawn with `kill_on_drop`, the bootstrap handshake validates peer_id and OS process identity against a 4 KiB response cap under a start deadline, graceful shutdown escalates to kill after `PEER_STOP_DEADLINE`, stderr diagnostics tasks are aborted and joined on every exit path, and `Drop` is a backstop rather than the cleanup plan. Ownership, cancellation, and join paths all meet the repository's long-lived-task rules.
- With this audit the scheduling review matrix covers every lifecycle owner in the workspace: writer, control plane, supervisor workflow/service/runtime, model peer process and provider, probe, terminal backends, and both Python boundaries.

### 2026-09-12 (11) - Live daemon start benchmark on Windows

**Measured**
- `scripts/benchmark_daemon_start.ps1` with release binaries, 10 samples plus warmup, on Windows (I:/VibeMux): daemon start p50 473.23 ms, p95 550.99 ms, mean 476.14 ms, min 407.82 ms, max 550.99 ms; daemon stop p95 52.07 ms; all start/stop status checks passed.
- The workload covers process spawn, lifecycle locks, SQLite migration check, writer startup, named-pipe listener, and authenticated health. Values are first-party measurements on the developer machine, not performance targets.
- Disk note: the workspace build cache had grown to 44 GB and filled the drive; it was removed and rebuilt cleanly (all 184 Rust tests still green). Drive I: remains at 82% occupancy from other data; large builds need headroom.

### 2026-09-12 (12) - Workspace crate audit

**Reviewed, no change needed**
- `vibemux_workspace` ownership validation: run worktree paths must sit directly under the managed directory (parent equality, not prefix matching), both the worktree path and its git-dir canonicalize to their recorded forms, the `.git` pointer file is size-capped UTF-8 whose target resolves to the expected git-dir, and the managed directory itself must be a plain directory (junction/symlink refusal has a dedicated Windows junction fixture test).
- All nine workspace-safety tests pass; the crate meets the repository's path-safety rules including the worktrees-vs-worktrees-evil prefix trap.

### 2026-09-12 (13) - Plugin protocol limits audit

**Reviewed, no change needed**
- `vibemux_plugin_protocol`: frame sizing is double-bounded (1 MiB default, 16 MiB hard ceiling that rejects oversized configuration outright), in-flight requests default to 32 with a 4096 hard cap, heartbeat intervals cap at 300 s, and manifests use `deny_unknown_fields` with policy-identifier and entry-point validation.
- A proptest drives every length above the default limit through the codec and asserts rejection, so the bound cannot regress silently. All 16 contract-fixture tests plus the property suite pass.

### 2026-09-12 (14) - Events crate audit

**Reviewed, no change needed**
- `vibemux_events` enforces the canonical event discipline at the wire: tiered byte limits (type/actor 128, idempotency key 256, payload 64 KiB, envelope 96 KiB), mandatory lowercase snake_case for event types and actors, hard rejection of unknown schema versions, and recursive payload validation that case-insensitively rejects forbidden keys - the executable form of the "secrets never enter the default log" invariant.
- The Python reference event fixture round-trips through the Rust decoder, keeping the cross-language contract locked.

### 2026-09-12 (15) - A2A client transport audit

**Reviewed, no change needed**
- `vibemux_a2a` task client: the base URL must normalize to an entry in `allowed_origins` (loopback-normalized) or the connect is forbidden; redirects are disabled outright so a response cannot pull a request off the allowlist; proxying is off; three timeout layers cover connect, call, and per-chunk stream reads; responses are double-bounded by content-length precheck and `bounded_json`; the bearer token travels only in the authorization header.
- Subscribe semantics reviewed earlier (terminal-event termination, explicit disconnect errors, no silent retry) hold for the rest of the client surface.

### 2026-09-12 (16) - Recovery artifact audit

**Reviewed, no change needed**
- `vibemuxd::recovery`: writer-lock snapshots bind the exact bytes inspected, `remove_if_unchanged` re-reads and compares before deleting (TOCTOU closed), symlinks/non-regular/empty/oversized artifacts are all rejected, the lock format parses as exactly three strict fields with a non-zero PID, the SHA-256 confirmation digest is domain-separated, and Debug output is redacted with a test asserting the nonce never appears.
- Recovery remains fail-closed end to end: any deviation surfaces as a typed error instead of a best-effort cleanup.

### 2026-09-12 (17) - Daemon spawn and paths audit

**Reviewed, no change needed**
- `vibemux_cli::spawn_daemon_process`: Unix detaches via `process_group(0)` with all stdio nulled; Windows launches the checked-in PowerShell companion through a base64 `-EncodedCommand` (no string concatenation into the script), non-interactive, in a new process group without a window, and the helper protocol (PID line + terminate control) has timeouts and a kill escalation on both startup and shutdown.
- `vibemuxd::process::DaemonPaths`: runtime/state directories validate as plain directories, legacy dual-runtime compatibility is explicit, and the control runtime plus artifacts verify Windows ACLs before use.

### 2026-09-12 (18) - Store transactional audit

**Reviewed, no change needed**
- `vibemux_store`: every commit runs in an `Immediate` transaction; the idempotency key is required for state-changing commits, UNIQUE at the schema level, and checked inside the transaction so duplicate requests deterministically return the original event with `duplicate: true`; WAL and busy_timeout are set deliberately; projections upsert by (kind, id) as rebuildable state; A2A run updates use expected-version CAS and reject double binding; sequences validate through `EventSequence`.
- This is the executable form of the atomic state-and-event invariant: a crash between the event insert and the projection update rolls both back together.

### 2026-09-12 (19) - Types crate audit

**Reviewed, no change needed**
- `vibemux_types` defines every domain identifier through one `define_id!` macro: opaque newtypes with a private inner UUID, construction only via `new()` (v4) or `from_uuid` (nil-rejecting), serde-transparent string representation, and `FromStr` routed through the validating constructor - exactly the opaque-ID discipline the architecture rules require.

### 2026-09-12 (20) - Plugin registry audit

**Reviewed, no change needed**
- `vibemuxd::plugin_registry`: lifetime restart budgets default to 3 (hard cap 16) and successful handshakes never reset consumed budget, so a crash loop cannot extend its own lease; exponential backoff uses a checked shift and validates against a 60 s ceiling; registration enforces capacity, duplicate, and stopped-state checks; shutdown signals every lifetime before joining each one and cleanup continues past individual errors; cancelled joins retain the worker as ownership evidence and failed joins quarantine it - both pinned by tests.

### 2026-09-13 - Tmux backend deep audit

**Reviewed, no change needed**
- `TmuxBackend`: sessions live on a named `-L` socket (the user's default server is never touched), `new-session` separates the command with `--` so command text cannot become tmux options, `send_text` stages text through a `NamedTemporaryFile` (0600) that is unlinked in a `finally`, and `list` parses tmux's dead-pane flag.
- Observation (kept as-is): `stop` does not check the `kill-pane` return code; the operation is intentionally idempotent - killing an already-dead pane reports failure to tmux but means success for the caller, so an error check would add noise without changing semantics.

### 2026-09-13 (2) - Workspace cleanup audit

**Reviewed, no change needed**
- `vibemux_workspace::plan_cleanup/cleanup`: the plan verifies ownership, then runs `git status --porcelain -z --untracked-files=all --ignored=matching` and refuses on ANY output - ignored and untracked files count as user data; cleanup re-inspects ownership immediately before `git worktree remove --` (closing the plan-to-execute window), and the branch is always retained.

### 2026-09-13 (3) - gRPC transport audit

**Reviewed, no change needed**
- `vibemux_a2a::task_grpc`: the listener binds `Ipv4Addr::LOCALHOST` on an ephemeral port only; a connection-budget semaphore rejects overflow immediately and each held permit lives exactly as long as its socket; decoding/encoding sizes, per-connection concurrency, stream count, and a server-level timeout are all bounded; the client validates endpoints with no discovery, no implicit redirection, no non-loopback targets, and no automatic retry; shutdown joins under a deadline with abort and Drop backstops.

### 2026-09-13 (4) - A2A wire layer audit

**Reviewed, no change needed**
- `vibemux_a2a::task_wire`: every wire-to-native conversion (request, part, snapshot, artifact, reply) validates after mapping, so no wire object becomes internal state unchecked. Stream application requires an existing previous snapshot, binds task_id and context_id on every update, merges artifacts by id instead of appending, and the SSE loop rejects any event whose task id differs from the subscription.

### 2026-09-13 (5) - A2A server admission and types restore audit

**Reviewed, no change needed**
- `task_server::begin`: stopped-state rejection, explicit a2a-version header check, exactly-one authorization header with constant-time bearer comparison (subtle), and a 32-permit admission semaphore returning Busy when saturated - permits are owned for the request lifetime.
- `vibemux_types::restore`: persisted records deserialize with `deny_unknown_fields` and explicit per-field length validation, preserving the stored JSON shape without weakening domain invariants.

### 2026-09-13 (6) - Task runtime audit

**Reviewed, no change needed**
- `vibemux_a2a::task_runtime`: every channel and pool is bounded (requests 32, events 16, tasks 64, cancel waiters 8, subscriptions 32) and overflow rejects with Busy instead of queueing; the state machine is Submitted -> Working -> terminal with idempotent cancellation; entries are subject-scoped so one caller cannot observe another's tasks; the crate documents itself as non-authoritative - executors receive explicit capabilities and canonical state never flows through it, consistent with the single-writer architecture.

### 2026-09-13 (7) - Worktree creation audit

**Reviewed, no change needed**
- `vibemux_workspace::create`: the target path must not exist (symlink metadata checked), the branch name derives from the RunId so user input never enters branch names, `git worktree add` pins the explicit base commit, an ownership token is generated at creation, and the owner receipt is written with create-new semantics, synced to disk, and verified by a final inspect. Windows extended-length path prefixes are stripped before invoking Git.

### 2026-09-13 (8) - Control frame protocol audit

**Reviewed, no change needed**
- `vibemuxd::control`: length-prefixed frames reject zero and oversize lengths (64 KiB cap with a dedicated test), the bearer token is 32 OS-random bytes rendered as 64 hex chars and compared in constant time, protocol versions are negotiated explicitly with unsupported versions rejected, every request runs under a 10 s deadline, and descriptor files cap at 4 KiB.
- Combined with the concurrent-connection fix (round 44) and the writer metrics (round 45), the control plane's security and scheduling surfaces are fully audited.

### 2026-09-13 (9) - A2A server send semantics audit

**Reviewed, no change needed**
- `task_server::send_message` and `send_streaming_message`: admission through `begin`, wire-to-native validation, response context_id must match the request or the exchange fails as a protocol error, and non-immediate sends subscribe to the task stream until a terminal state, validating every update's task and context binding. The streaming variant moves the admission permit into the response stream, and the server's stop watch terminates streams on shutdown.

### 2026-09-13 (10) - Supervisor finalization audit

**Reviewed, no change needed**
- `supervisor_workflow::fail_open_runs/finalize_cancelled_task`: only non-terminal runs (Preparing/Running/Stale) are marked failed with the `supervisor_not_verified` code, terminal records are never rewritten, cancellation finalization is idempotent, all canonical writes go through the single writer via spawn_blocking, and errors propagate instead of being swallowed.

### 2026-09-13 (11) - Artifact write audit

**Reviewed, no change needed**
- `vibemux_workspace::write_artifact`: artifact names are a three-value whitelist (result/review/verification.json) so path injection is structurally impossible, payloads pass a 32 KiB JSON bound, ownership is verified immediately before writing, files open with create-new semantics (no overwrite or clobber race), and each reference records sha256 and size with a sync-to-disk completion.

### 2026-09-13 (12) - Push notifications and status reader audit

**Reviewed, no change needed**
- Push notification configs are explicitly rejected at the wire layer (`Unsupported`) and the gRPC surface maps that to the standard `Unimplemented`/`PUSH_NOTIFICATION_NOT_SUPPORTED` status - an explicit capability boundary instead of a silent ignore. Tenant fields are rejected the same way.
- `plugin_registry::status_reader` exposes read-only snapshots through a watch channel; plugin cancellation is deliberately not exposed over IPC, and terminal entries cannot be silently re-registered.

### 2026-09-13 (13) - A2A domain types audit

**Reviewed, no change needed**
- `vibemux_types::a2a`: cross-entity identity consistency (task/run/binding/workspace must agree on ids and base_commit), schema-version enforcement, ordered timestamps, `Succeeded` requiring a verification receipt (matching the RunCompletionAuthority rule), `A2aRunStart` restricted to Preparing status with a role allowlist, bounded artifact and run counts, and `deny_unknown_fields` throughout.

### 2026-09-13 (14) - A2A contract audit

**Reviewed, no change needed**
- `vibemux_a2a::task_contract`: tiered size bounds (wire 64 KiB, payload 16 KiB, 16 parts, 16 artifacts, 32 identities), bounded JSON payloads, media-type validation, and peer credentials constrained to 32-256 ASCII alphanumeric/hyphen/underscore/dot characters (blocking header injection and control characters) with a redacted Debug implementation.

### 2026-09-13 (15) - Python diff semantics audit

**Reviewed, no change needed**
- `workspace.py::diff`: tracked changes diff against the pinned base commit (`--no-ext-diff --binary` blocks external diff tools and preserves binaries), untracked files are enumerated NUL-delimited (`-z` blocks newline injection) and patched via `--no-index` against `/dev/null` with exit code 1 treated as success, and the base commit is verified to exist before any diffing. Ownership is checked first.

### 2026-09-13 (16) - Context inference audit

**Reviewed, no change needed**
- `task_server::infer_context`: task identifiers validate before lookup, the snapshot is fetched by the authenticated subject (other subjects' tasks are simply not found), the fetched task id is re-verified, and a client-supplied context_id that disagrees with the real snapshot is rejected as an invalid request - blocking forged-context attachment to another conversation.

### 2026-09-13 (17) - Git process execution audit

**Reviewed, no change needed**
- `workspace::git_output`: git runs with a fully cleared environment (only SystemRoot/WINDIR/TEMP/TMP/PATH re-imported), system and global git configs blocked, hooks disabled via `core.hooksPath`, autocrlf and fsmonitor pinned off, `kill_on_drop` plus Windows no-window flags set; stdout and stderr are read concurrently under a 1 MiB bound each (no pipe-full deadlock), the whole operation runs under a 30 s deadline, and every failure path terminates and reaps the child within `REAP_DEADLINE`.

### 2026-09-13 (18) - Python path safety audit

**Reviewed, no change needed**
- `vibemux::paths`: containment uses `os.path.commonpath` equality (prefix matching and the worktrees-evil trap are structurally excluded, cross-drive ValueError fails closed), case normalization runs on Windows via `normcase`, and managed-worktree checks layer normalized containment, main-repository protection, symlink/reparse refusal, and a resolve-based re-verification that tolerates not-yet-created paths by validating the parent.

### 2026-09-13 (19) - gRPC status compatibility audit

**Reviewed, no change needed**
- `compatible_grpc_status`: SDK-internal error messages map explicitly to the A2A-standard gRPC codes (TASK_NOT_FOUND, PUSH_NOTIFICATION_NOT_SUPPORTED, UNSUPPORTED_OPERATION, VERSION_NOT_SUPPORTED, TASK_NOT_CANCELABLE), auth failures map to unauthenticated/permission-denied so they cannot be downgraded to generic Unknown, unmatched statuses pass through unchanged, and every rewritten status carries standard error-info details.

### 2026-09-13 (20) - Probe version parsing audit

**Reviewed, no change needed**
- `vibemux_probe` version extraction is deliberately generic: the first non-empty line of stdout (falling back to stderr) becomes the version text, so the ten registered agents - including the Chinese CLI families - are all handled by one code path with no per-vendor format assumptions. Output size is bounded on both streams, the version text is length-capped and sanitized, and Windows launcher resolution distinguishes executables it may run (.exe/.com) from scripts it never executes (.cmd/.ps1).

### 2026-09-13 (21) - Mock harness audit

**Reviewed, no change needed**
- `mock_harness.py`: the WRITE command applies the same three-layer path defense as production (absolute-path rejection, `..` segment rejection, resolve-then-commonpath equality), unknown commands answer with an explicit UNKNOWN line instead of silence, every response flushes for pipe interaction, and EXIT carries an explicit status code. The offline harness therefore exercises the real safety semantics rather than a lax copy.

### 2026-09-13 (22) - PowerShell companion launcher audit

**Reviewed, no change needed**
- `harness.py::_resolve_command`: Windows script launchers (.cmd/.bat/.ps1) are never executed directly. A .ps1 is wrapped as `powershell -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File <script> -- <args>`, .cmd/.bat require a same-name .ps1 companion or registration fails, and only .exe/.com resolve as direct executables. The whole launch then flows through `vibemux.agent_host` (-- separated argv, JSON environment), so no user-controlled text ever reaches a shell string. The probe path mirrors the same resolution, so availability reflects the controlled launcher, not a raw script path.

### 2026-09-13 (23) - A2A store CAS audit

**Reviewed, no change needed**
- `vibemux_store::a2a`: run updates carry double-layer compare-and-swap - an application-level `expected_version` check plus the SQL `UPDATE ... WHERE version=?` conditional whose zero-row result maps to `A2aVersionConflict`, closing the read-check-write race even under concurrent writers. Idempotency fingerprints are domain-separated SHA-256 (kind, NUL, payload) so a reused key with different content surfaces as `A2aIdempotencyConflict` rather than silently matching. Terminal records reject every action except `FinalizeCancellation`, timestamps must be monotonic, and the whole path runs in an `Immediate` transaction.

### 2026-09-13 (24) - Control descriptor security audit

**Reviewed, no change needed**
- Descriptor files carrying the bearer token are created with `create_new` (an existing descriptor blocks a second daemon), cap at 4 KiB, and open with 0o600 permissions on Unix; on Windows the control runtime directory is secured through `vibemux_platform::secure_user_directory` with an ACL marker file that is validated and persisted so the tightening is enforced on every start.

### 2026-09-13 (25) - Platform ACL hardening audit

**Reviewed, no change needed**
- `vibemux_platform::windows_security`: the ACL script runs base64-encoded through the system PowerShell resolved by absolute path (PATH hijacking is irrelevant); the target path travels only via an environment variable, never through script text. The directory drops inheritance and is rewritten with exactly three FullControl ACEs (current user, SYSTEM, Administrators); the script then reads the ACL back and verifies protection, rule count, non-inheritance, allow-type, and rights before reporting success. The helper runs under a 5 s deadline with kill-and-reap, bounded output, and staged error codes for diagnostics.

### 2026-09-13 (26) - Plugin supervisor audit

**Reviewed, no change needed**
- `vibemux_plugin_supervisor`: plugin processes spawn with a cleared environment plus an explicit session variable, `kill_on_drop`, and platform detachment flags; all three pipes are owned (stderr is collected under a byte limit with an atomic truncation flag); the Hello -> CoreHello -> Ready handshake runs through the lifecycle state machine under a deadline, and every handshake failure terminates and reaps the child plus aborts and joins the stderr task; reader/writer tasks communicate through bounded mpsc channels; the config rejects zero-valued timeouts.
- With this, every crate in the workspace has been audited or improved at least once by the optimization loop.

### 2026-09-13 (27) - CI parity verification

**Verified**
- Local runs now match the CI Rust job exactly: `cargo clippy --workspace --all-targets --all-features -- -D warnings` reports zero diagnostics and `cargo test --workspace --all-features` passes 217 tests - 31 feature-gated tests beyond the 186 exercised by the loop's default-feature runs. No divergence existed; the loop's verification baseline now uses the all-features variant.

### 2026-09-13 (28) - Theme signature refresh after layout changes

**Verified**
- Live Windows ANSI captures with the current binary (after the slot-panel wrap and height rebalance): classic emits 256-color indices (2/3/6/8), high-contrast the bright variants (10/11/14/15), mono emits no foreground sequences at all, and light emits the WCAG-corrected truecolor palette (0,110,0 / 0,110,110 / 150,75,0 / 96,96,96). All four signatures match the designed palettes; the layout changes did not disturb theme styling.

### 2026-09-15 (3) - PR #3 merge into main (scheduling and visual optimizations)

**Scope**
- Merged `origin/feat/scheduling-and-visual-optimizations` (PR #3, head `4d96fc2`, 11 commits) into the integration branch on top of `origin/main` (`91eaa6d`) plus the C0a/C0b Linux fixes. Conflict resolution: `CHANGELOG.md` (union, both entry sets kept), `tests/test_harness.py` (PR side = main + 2 registry-cache tests, verified by diff), `crates/vibemux_frontend/src/lib.rs` (PR side; it is a strict superset of the 91eaa6d truncation fix — same 160-column threshold, same six-column layout, regression test carried verbatim and extended to ten agents), `README.md` (union: Harness switching + Dashboard themes sections), `PROGRESS.md` (chronological union).
- The Python side is conflict-free: the PR's `9aa0f4f` is a byte-for-byte re-application of main's `2cdc3ed`.
- Seven Codex review findings on PR #3 are tracked for follow-up commits on this branch (Kimi table rows, light-theme health colors, writer queue telemetry order, SSE/polling deadline sharing, legacy run-role tolerance, spawn detection ordering ×2); the Python-orchestration architecture debt (AGENTS.md §4.1) is recorded and tracked in a GitHub issue.

**Validation of the merged state** is recorded after the follow-up fixes land; see the pre-push validation entry.

### 2026-09-04 (2) - GUI chrome topbar layout fix

**Problem**

- The two-shell egui GUI opened with an effectively empty window: only the `VIBEMUX` wordmark was visible and every other chrome control plus the overview body failed to display. Reproduced by running `target\debug\vibemux_frontend.exe` on native Windows and capturing the window.

**Root cause**

- `topbar::render` assumed its parent `Ui` had a horizontal layout, but egui 0.32 `TopBottomPanel` always creates its child `Ui` with `Layout::top_down(Align::Min)` (`containers/panel.rs::show_inside_dyn`). The topbar widgets therefore stacked vertically inside the default `interact_size` chrome height and were clipped; the overflowing chrome also displaced the central panel so the overview body ended up invisible.

**Fix**

- `crates/vibemux_frontend/src/gui/topbar.rs`: wrap the topbar contents in an explicit `ui.horizontal(|ui| …)` block so the brand mark, workspace label, and the right-aligned `right_to_left` toolbar (theme switch, settings/diag icons, mode hint) land on a single chrome line.
- `crates/vibemux_frontend/src/gui/overview.rs`: add headless egui render tests for both shells (`overview_body_paints_text_shapes`, `workbench_body_paints_text_shapes`) that mirror `VibeMuxApp::update`/`render_workbench` and assert non-trivial painted shape counts, guarding against silent empty-body regressions.
- `crates/vibemux_frontend/Cargo.toml`: add the off-by-default `gui_screenshot` feature forwarding eframe's `__screenshot` test feature; used only for local visual verification via `EFRAME_SCREENSHOT_TO`.

**Validation (native Windows, this change)**

- `cargo build -p vibemux_frontend --bin vibemux_frontend`: passed.
- Live window observation with the fix: the chrome renders VIBEMUX + workspace + overview hint + claude/github/vscode theme buttons + settings/diag icons, and the overview body renders the COMMAND kicker, "Five agents, one machine." headline, all five agent rows (Claude/Codex/OpenCode/Copilot/Grok with state dots, excerpts, route/version), and the telemetry/gateway/runtime facts strip. Captured both via `EFRAME_SCREENSHOT_TO` framebuffer dump and via Win32 window capture.
- `cargo test -p vibemux_frontend`: 45 passed, 0 failed (including the two new headless render tests).
- `cargo test --workspace --all-features`: all groups `ok` (same set as the 2026-08-31 run plus the two new tests).
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --all-features -- -D warnings`: passed.

**Notes**

- No behavior contracts changed: the fix is layout-only inside the chrome; probe/env/privacy contracts are untouched. No PTY/ConPTY attach was added.
- The unrelated local `rust-toolchain.toml` `rust-analyzer` component addition remains unstaged and excluded, as before.

### 2026-09-15 (4) - Two-shell frontend branch merge (Merge B)

**Scope**
- Merged `codex/m7_a2a_supervisor` (head `7ed2b9f`, two-shell GUI refactor + topbar fix) into the integration branch after PR #3 and the C0-C6 review fixes. Conflict resolution: `crates/vibemux_frontend/src/lib.rs` and `src/bin/vibemux_frontend.rs` resolved to the two-shell side (module declaration + egui GUI); `Cargo.toml` unioned (egui/eframe/serde/serde_json + three bins + `gui_screenshot` feature, exactly one `serde_json`); `Cargo.lock` taken from the two-shell side and regenerated by `cargo check`; `crates/vibemux_probe/src/lib.rs` auto-merged (BOM tolerance vs ten-agent `AgentKind`); `PROGRESS.md` unioned chronologically.
- The PR dashboard renderer (six-column ten-agent table, audited Theme system, golden fixtures, CLI flags) is **ported forward** into the two-shell TUI in the follow-up commits (C7/C8) rather than resurrecting the single-shell `lib.rs`; `vibemux_frontend_dump` supersedes the removed `--once` flag.
- Immediately after this merge the tree has known red tests (stub bank 5-of-10, `app.rs` session-count assertion, `view_model_has_five_agents`, `tests/cli.rs` pointing at the GUI binary); they are fixed by C7-C10 before any push.

### 2026-09-15 (5) - TUI port and ten-agent GUI adaptation (C7-C10)

**Scope**
- C7/C8/C10 (`a54c225`): ported the single-shell PR dashboard forward into the two-shell TUI — new `src/tui/theme.rs` owns the audited `Theme` system (classic/high-contrast/mono/light with the WCAG AA 4.5:1 audit against the xterm-256 palette, including the C2 light-health fix); `src/tui/render.rs` was rewritten around the six-column ten-agent table (per-state cells, ellipsize, version_budget, 160-column side-by-side threshold, stacked `Min(13)`/`Min(18)` layout carrying the C1 fix); `view_model.rs` gained `Health`, `overall_status`, ISO `observed_at`, and `format_epoch_seconds`; `run_tui(view_model, theme)` cycles themes with `t` (no AtomicU8/ThemeId file mirroring — decision D3: the TUI no longer mirrors the GUI's persisted theme); `bin/vibemux_frontend_tui.rs` implements `--json`/`--theme` with exit-4 error paths; `tests/cli.rs` retargeted to `_tui` and `_dump` (dump supersedes `--once` — decision D2). Golden fixtures regenerated under the ported renderer and reviewed per AGENTS.md §14.2 (Kimi visible at 80x24 and 120x32, 120x32 fixtures distinct per theme via the footer label, 80x24 theme-identical because the footer folds, no ANSI escapes in snapshot output).
- C9 (`36c208e`): GUI stub bank extended from 5 to all 10 harnesses (Qwen/iFlow/TRAE/CodeBuddy/Kimi, ASCII, ~40 lines each); `app.rs` session-count assertion and `view_model_has_ten_agents` bumped to 10; overview headline now "Ten agents, one machine.".
- Native-TUI slots were deliberately **not** ported back into the TUI (slots/seats belong to the GUI; `render_debug_dashboard_does_not_mention_native_tui` guards this).

**Validation (native Windows, this change)**
- `cargo test -p vibemux_frontend --all-features`: 53 lib + 5 CLI integration tests, all green.
- `cargo test --workspace --all-features`: three consecutive clean runs (the known `bootstrap_process::startup_timeout_terminates_only_the_spawned_fixture` Windows flake did not recur; it spawns real timed-out children and is machine-load sensitive — isolated reruns pass every time).
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --all-features -- -D warnings`: passed.

**Known limitations**
- GUI keyboard accelerators `Ctrl+0..5` reach only 6 of the 10 harness seats; the remaining seats are mouse-selectable. Tracked in GitHub issue #5.

### 2026-09-15 (6) - Documentation sync for the two-shell frontend (C11)

**Scope**
- `README.md`: "Dashboard themes and machine-readable output" rewritten as "Frontend binaries, dashboard themes, and machine-readable output" describing the three binaries (`vibemux_frontend` egui GUI without flags, `vibemux_frontend_tui` with `--json`/`--theme`/`t`/`c`, `vibemux_frontend_dump` replacing `--once`); fixed PR #3's `vibemux-frontend` hyphen typo; the probe-preview section now lists all three cargo run commands and names all ten harnesses with the stub-transcript/no-PTY caveat.
- `CLAUDE.md`: layout map line, the non-interactive smoke command (`vibemux_frontend_dump`), and the native-TUI caution updated to the two-shell/ten-harness reality.
- `docs/architecture.md`: frontend boundary paragraph updated (two shells, GUI seats with stub transcripts, ten harnesses, no attach).
- `CHANGELOG.md`: added two-shell split and ten-harness GUI entries; corrected the now-stale "+N more slots" and "ten reserved native-TUI slots" phrasing (slots do not exist in the final tree).
- `docs/adr/016_probe_and_unified_frontend.md`: added "Amendment (2026-09-15): two-shell frontend split" (theme ownership, light theme addition, dump superseding `--once`, rejected single-shell alternative).
- `PROGRESS.md` §4.3 P1: added the Python harness-orchestration architecture debt item (AGENTS.md §4.1; `2cdc3ed` + PR #3 surface belongs in Rust), tracked in GitHub issue #4.

**Validation**
- `grep` sweep for `vibemux.frontend|--once|vibemux-frontend` across `*.md`/`*.yml`/`*.ps1`: remaining hits are historical PROGRESS/CHANGELOG ledger entries (intentionally preserved as dated records) and the ADR 016 original text describing the pre-amendment decision.

**Notes**
- The unrelated local `rust-toolchain.toml` `rust-analyzer` component addition remains unstaged and excluded from every commit.
- Local Python validation in this cycle ran on Anaconda 3.11.5 via `PYTHONPATH=src python -m pytest` (the package requires >=3.12 so `pip install -e` is unavailable locally); the GitHub Actions Python 3.12 job remains the authoritative gate. This caveat applies to the C5/C6 test runs recorded in their commit messages.

### 2026-09-15 (7) - winit Linux backend fix (unblocks rust-ubuntu CI)

**Problem**
- The WSL full-workspace gate (Ubuntu 22.04, Rust 1.85.0) failed to compile at `9ebde41` with `error: The platform you're compiling for is not supported by winit` (winit 0.30.13, `platform_impl/mod.rs:78`). Because `vibemux_frontend` is part of `--workspace --all-targets`, this single compile error would fail the entire rust-ubuntu CI job — the two-shell merge would have landed main's Linux CI red again.

**Root cause**
- The two-shell branch (`4458db8`) pins eframe as `default-features = false, features = ["default_fonts", "glow"]`. eframe's default feature set forwards `wayland`/`x11` to winit; omitting them leaves winit with no Linux platform backend (`x11_platform`/`wayland_platform` cfgs unset → the unconditional `compile_error!`). The defect was invisible pre-merge because the two-shell branch was only ever built on Windows, and the risk-table assumption "eframe-on-Ubuntu builds with pure-Rust backends" was true of the *backends* but never exercised because the feature flags that enable them were missing.

**Fix**
- Root `Cargo.toml`: eframe features extended to `["default_fonts", "glow", "wayland", "x11"]` (commit `a35d9b1`, lockfile regenerated). Both backends are pure Rust at build time (x11rb, vendored wayland-sys); no system X11/wayland dev packages are required, and the frontend tests stay headless.

**Validation**
- `cargo check -p winit` on Ubuntu 22.04 (WSL2) with the fix: green (both backends resolve).
- `cargo check --workspace --all-features` on Windows after the lockfile regen: green.
- Full pre-push gates on both platforms recorded in the next entry.

### 2026-09-15 (8) - Golden fixtures decoupled from the machine environment

**Problem**
- The first full Linux run of the WSL gate (Ubuntu 22.04, after the winit fix) failed `tui::render::tests::golden_render_snapshots_match_fixtures`: the golden model was built with `ViewModel::from_report`, whose diagnostics section embeds `collect_allowlisted_env()` — the live process environment. The fixtures therefore contained the generating machine's PATH (including local build dirs), HOME, APPDATA, and locale values, and could never match on any other machine, platform, or CI runner. This would have failed both the rust-ubuntu and rust-windows GitHub jobs on the very first push.

**Fix**
- The golden-test `model()` now pins a deterministic env sample: the same `PROBE_ENVIRONMENT_ALLOWLIST` keys in allowlist order, fixed cross-platform values (`/opt/fixture/<key>`), every third key unset (rendering `<unset>`), and one fixed long PATH to keep paragraph wrapping exercised. All four 120x32 fixtures regenerated (the 80x24 snapshots contain no env lines at that height, so they were already platform-independent). Fixture diff reviewed per AGENTS.md §14.2: only the env lines changed; title, agent table, gateway/A2A/telemetry lines, and footer are byte-identical, and no machine-specific value remains (`grep` for the user name, local paths, and Anaconda finds nothing).
- `cargo test -p vibemux_frontend --all-features` on Windows: 53 lib + 5 CLI green with the new fixtures; fmt/clippy green.

### 2026-09-15 (9) - Pre-push validation of the integration branch tip (`9c2b0a4`)

**Windows (native, tip `9c2b0a4`)**
- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: pass.
- `cargo test --workspace --all-features`: pass (52 test-group results ok; frontend 53 lib + 5 CLI; no failures). Note: an earlier full-suite attempt that ran concurrently with the WSL gate showed 2 real-process lifecycle-test failures (`startup_timeout_terminates_only_the_spawned_fixture`, `abrupt_process_recovery_preserves_database_and_allows_restart`) under the doubled machine load; both pass in every non-concurrent rerun, consistent with the documented load-sensitive flake class.
- `PYTHONPATH=src python -m pytest`: 41 passed in 15.12s (Anaconda 3.11.5 caveat per entry (6)).
- `PYTHONPATH=src python -m ruff check .`: pass. `python scripts/smoke_test.py`: PASS mock workflow.
- Runtime smokes: `vibemux_frontend_dump` prints the Central Console snapshot (exit 0); `vibemux_frontend_tui --json` emits `schema_version 1` with 10 agents; `--theme mono --json` valid; `--bogus` exits 4 with `unknown argument: --bogus` on stderr; `cargo build -p vibemux_frontend --bin vibemux_frontend` (GUI) pass.

**Linux (Ubuntu 22.04, WSL2, Rust 1.85.0, fresh shallow clone at `9c2b0a4`)**
- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: pass — including the egui/eframe GUI with the winit x11/wayland backends enabled (entry (7)); no system X11 dev packages were needed.
- `cargo test --workspace --all-features`: pass — all 52 group results ok including the golden fixture snapshots (entry (8) fix verified cross-platform) and the previously Linux-blocking grpc/workflow suites (C0a/C0b). One intermediate run hit 10 failures in the real-process `vibemux_plugin_supervisor` suite while the harness session was being torn down; isolated rerun of that crate (13/13) and the subsequent full-suite rerun were both green — same load-contention flake class already documented, no code change.

**Gate decision**: both platform gates green at the integration branch tip; proceeding to the main push.

### 2026-09-15 (10) - CI golden-fixture line-ending fix (rust-windows runner)

**Problem**
- The first CI run on the pushed main (`34984067454`) passed `rust (ubuntu-latest)` for the first time since the sun_path defect — both C0 fixes and the winit/golden work held — but failed `rust (windows-latest)` on `golden_render_snapshots_match_fixtures` (classic@80x24), a combination that was green on both local gates.

**Root cause**
- The repository has no `.gitattributes`; the fixtures are stored LF. GitHub's windows runners default to `core.autocrlf=true`, so checkout rewrote every fixture to CRLF, and the character-exact snapshot comparison failed before any product code ran. The local Windows clone has autocrlf off (working tree LF), which is why the identical suite passed locally. Ubuntu runners are unaffected (`i/lf` both sides).

**Fix**
- Added `.gitattributes` pinning `crates/vibemux_frontend/tests/fixtures/golden/*.txt` to `text eol=lf`, which is the documented remedy for byte-exact fixtures on autocrlf runners (fixtures are text, so `-text`/LFS are not applicable).

**Validation**
- `git ls-files --eol` shows `i/lf` for all eight fixtures (repository side already correct; the attribute fixes the checkout side). Fixtures and tests unchanged; no rerun of the local suites was needed beyond confirming the working tree stayed byte-identical. The next CI run on main is the authoritative confirmation.

### 2026-09-15 (11) - Main push outcome: CI green on both platforms; PR #3 closed as landed

**Outcome**
- `feat/3-two-shell-frontend-integration` pushed to `main` (`91eaa6d..4718c64`, then the `.gitattributes` fix `3ae7cb5`). The second CI run on the tip (`34985544437`) is green on all four jobs: `python (windows-latest)`, `python (ubuntu-latest)`, `rust (windows-latest)`, `rust (ubuntu-latest)`. `rust (ubuntu-latest)` passes for the first time since the unix-socket `sun_path` defect entered the branch — C0a (system-temp control socket), C0b (Git 2.34-compatible worktree inventory), the winit `wayland`/`x11` backend fix, and the deterministic golden env sample all held under CI.
- PR #3 was closed as superseded rather than MERGED: Merge A landed as a content-equivalent squash (single-parent `3261f82`; `git diff 4d96fc2 <landing>` shows zero product-code delta, only the C0a/C0b changes), so `4d96fc2` is not on main's first-parent chain and GitHub cannot auto-mark it. Closure rationale posted on the PR; the review triage comment maps all seven Codex findings to their fix commits or tracking issues (#4, #5).
- Verification trail: run `34984067454` (first push) — rust-ubuntu green, rust-windows red on fixture CRLF (entry (10) root cause); run `34985544437` (after `.gitattributes`) — all four green.

**Notes**
- Issues #4 (Python harness-orchestration architecture debt, AGENTS.md §4.1) and #5 (GUI accelerators reach 6 of 10 seats) were created and backfilled into entries (5)/(6) and §4.3.
- The unrelated local `rust-toolchain.toml` modification remains uncommitted by design.

### 2026-09-15 (12) - Atomic fixture receipt publication (CI race fix)

**Problem**
- CI run `34988675997` on the main tip (`a4b3127`, PROGRESS-docs-only over the green `3ae7cb5` run) failed `rust (windows-latest)` on `cancel_during_review_preserves_worker_completion_observation_and_cancels_local_task`: `fixture_receipts` read a zero-byte receipt and `serde_json` panicked with `EOF while parsing a value` at supervisor_workflow.rs:272.

**Root cause**
- `vibemux_model_fixture`'s `receipt()` helper opened the final path with `create_new` and wrote in place, while the workflow tests poll the receipts directory every 20 ms. A slow CI runner can observe the file between creation and `write_all`, i.e. zero bytes. A classic non-atomic-publish race that the faster local machines and the previous run happened to miss; the same helper backs the grpc suite's receipts.

**Fix**
- `receipt()` now writes to a sibling `<name>.tmp-<pid>` file, syncs, and `rename`s into place. Rename within a directory is atomic on POSIX and Windows, and Windows' rename-refuses-to-replace preserves the old create_new semantics of failing when the receipt already exists.

**Validation**
- Windows: `cargo test -p vibemuxd --test supervisor_workflow --all-features` three consecutive runs, 4/4 each. Clippy over the crate's bins green.
- Ubuntu 22.04 (WSL probe at `a4b3127` + this fix): `supervisor_workflow` 4/4 and `supervisor_grpc` 3/3 green.
- CI confirmation: run `34990558402` on main tip `8509b02` is green on all four jobs (python/rust × windows-latest/ubuntu-latest) — the first fully green main CI since PR #1. `rust (ubuntu-latest)` has now passed three consecutive runs.

### 2026-09-16 (1) - GUI keyboard accelerators reach all ten seats (issue #5)

**Problem**
- After the ten-harness GUI extension, the keyboard digit map still covered only `Ctrl+0..5` (6 of 10 destinations: Overview + seats 1-5), and the workbench rail hover text advertised `Ctrl+6..Ctrl+10` for seats 6-10 — keys that did nothing. The topbar hint also promised `esc → overview` but no Escape handler existed.

**Fix**
- Digit row mapped one-to-one onto the ten seats: `Ctrl+1..9` open seats 1-9, `Ctrl+0` opens the tenth (Kimi). `Escape` (unmodified, and only when no text field holds keyboard focus via `ctx.wants_keyboard_input()`) closes the topmost overlay first and otherwise returns a seat to the overview — making the existing topbar hint true. The rail hover labels now come from a `seat_accelerator(idx)` helper that matches the key map exactly, and the overview-mode hint documents the digit keys (`overview · ctrl+1..0 → seat`). `Ctrl+T`/`Ctrl+,`/`Ctrl+;` are unchanged.
- Tests: `ctrl_digits_open_all_ten_seats` (headless egui `Context::run` with injected key events, one assertion per seat), `escape_closes_overlays_then_returns_to_overview` (overlay-close precedence, seat → overview, no-op in overview), and a `seat_accelerator` unit test pinning the digit-row mapping. The headless key-injection helper sets both the event modifiers and `RawInput::modifiers` — `InputState::modifiers` is sourced from the latter.

**Validation (native Windows)**
- `cargo test -p vibemux_frontend --all-features`: 56 lib + 5 CLI green (three new tests). `cargo fmt --all -- --check`, `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings`, and the GUI binary build: all green. Closes issue #5.

### 2026-09-16 (2) - Rust harness-orchestration surface (issue #4)

**Scope**
- Ports the Python harness-orchestration surface (`2cdc3ed` + PR #3) into the Rust core per AGENTS.md §4.1, so the Rust core — not the Python prototype — now carries harness registry, detection, the `harnesses`/`switch` commands, and detection gating. The single-writer rule is preserved: all harness state changes go through `vibemuxd`'s `WriterWorker`; no parallel Python writer is introduced.

**Implementation**
- New crate `vibemux_harness` (pure logic, `#![forbid(unsafe_code)]`, no I/O): the ten-harness profile registry (command/protocol/provider, protocol label `pty` retained from the Python reference for parity — no Rust launch/attach in this slice), row/snapshot building with roles carry-over, `harness_probed`/`harness_switched`/`v3_harness_seed` canonical event drafts, and bounded path/role validation. Detection inputs are injected so the types → harness → store dependency direction stays acyclic.
- `vibemux_store` schema **3** migration: seeds `harness_registry` and `harness_config` projections plus a `v3_harness_seed` event. `commit_harness_snapshot` (atomic registry upsert + `harness_probed` event; seeds the default only when absent so a refresh never clobbers a `switch`) and `commit_harness_switch` (detection-gated; rejects unknown names and undetected harnesses before any state change) plus validated `harness_registry`/`harness_config` reads.
- `vibemuxd`: control protocol **v3** adds `HarnessRefresh`/`HarnessSnapshot`/`HarnessSwitch` (v1/v2 remain accepted; the new ops require v3, mirroring the PluginStatus legacy gate). Note: the tree at this commit actually accepted only v1/v3 — the hand-synced version `matches!` list had dropped v2; v2 acceptance was restored by the per-operation version table in the (3) review-fix round below. The daemon executes **no** harness binaries itself — it consumes the trusted `vibemux_probe` cache (`<project>/.vibemux/probe_cache.json`, validated as a regular owner-readable bounded UTF-8 JSON file) inside `spawn_blocking`, and only the probe's `verified` `--version` state counts as detected (a bare PATH hit never does). Probe-cache path is derived from the database's state dir, fixing the Windows case where the control runtime lives under `%LOCALAPPDATA%` but project state stays under `<project>/.vibemux`.
- `vibemuxctl harnesses [--cached]` and `vibemuxctl switch <harness>`: thin clients over the new control ops. Harness gating codes (`harness_not_detected`, `store_unknown_harness`, `harness_probe_cache_missing`) round-trip the wire so the CLI prints actionable errors instead of a generic remote-error.

**Live validation (native Windows, real installed harnesses)**
- Real `vibemux_probe` run detected `claude` (2.1.217) and `grok` (1.0.30) verified, `codex`/`opencode`/`copilot` launcher=`power_shell_companion` with `--version` probe `failed` (correctly **not** counted as available), and qwen/iflow/trae/codebuddy/kimi unavailable.
- `vibemuxctl harnesses` (live refresh) returned all ten rows with detected/version/launcher for the verified two; `switch grok` succeeded (`from none`), `switch qwen` was gated (`harness_not_detected`), `switch bogus-harness` rejected (`store_unknown_harness`), `harnesses --cached` read back the persisted `default=grok` and detection after a daemon restart, and a missing probe cache surfaced `harness_probe_cache_missing`. `daemon health` reported `store_schema_version: 3`.

**Validation**
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-features`: all green (new tests across `vibemux_harness`, `vibemux_store`, `vibemuxd` control integration, and `vibemux_cli`). Closes issue #4.

**Merge outcome**
- Landed on `main` as `8cc394b` (feature commit `273e041` + probe-cache path fix `8cc394b`; authorized direct push). WSL (Ubuntu 22.04, Rust 1.85.0, probe clone @ `8cc394b`) re-ran the three gates green: 54 test suites ok, zero failures, including the three harness control tests fixed for Linux (runtime_dir ≠ state_dir). CI on `main` run `35077253232`: python (ubuntu/windows) + rust (ubuntu/windows) all **success**.
- Follow-up docs commit `494c40e` (this entry's merge-outcome note) also green on `main` (run `35078695714`) after one re-run: the first attempt hit a load-sensitive `vibemux_cli::recovery` flake (`ControlRuntimeSecurityInvalid` in four tests) unrelated to the docs diff — the identical code passed at `8cc394b` and passed on re-run. Tracked as issue #6.

### 2026-09-16 (3) - Harness surface review-fix round

**Scope**
- Post-merge multi-angle review of the harness-orchestration surface (`273e041..0bfe966`) found 15 findings across the probe, harness, store, daemon, and CLI crates. All fixed in this review-fix round except the documented deferrals below.

**Implementation**
- **Crate hygiene / vocabulary ownership**: `AgentKind`/`LauncherKind`/`ProbeState` move from `vibemux_probe` into pure-logic `vibemux_harness` (new `crates/vibemux_harness/src/agent.rs` for the enums + serde round-trip tests); `vibemux_probe` re-exports them so every historical consumer compiles unchanged. `vibemux_harness` drops its `vibemux_probe` dependency (no tokio/reqwest/rusqlite in the crate — the "pure logic, no I/O" claim is now enforcement-level) and `vibemux_probe` gains a `vibemux_harness` dependency. Wire serde names stay byte-identical (`open_code`, `power_shell_companion`, `verified`, ...).
- **Probe cache owner + binary**: new `vibemux_probe::cache` module owns `PROBE_CACHE_FILE_NAME` (`probe_cache.json`), `default_cache_path(project_root)` (= `.vibemux/probe_cache.json`), atomic `write_cache` (creates `.vibemux` with 0700 on unix, temp file 0600 in the same directory, sync, rename — a torn cache is never observable), and hardened `read_cache` (Missing/UnsafeArtifact/TooLarge/Invalid/IO with the size re-checked after a bounded read). The probe binary gains `--write-cache` / `--project-root <dir>` (default `.`); it writes the cache via `cache::write_cache` and still prints the report JSON to stdout; unknown/malformed flags print usage to stderr and exit 4. `AgentProbe` gains `path: Option<String>` (`#[serde(default)]`), the resolved executable path the PATH scan actually found and used (Windows `PATHEXT`-style scan incl. `.cmd`/`.ps1` PowerShell-companion shims, mirrors the launcher detection; unix executable-regular-file scan); caches written by older builds parse with `path: None`.
- **Daemon paths**: `DaemonPaths::probe_cache_path()` (`state_dir()/probe_cache.json`); both Windows and unix `ServerState` construct the cache path from it; the old `database_path.parent()` derivation in `control.rs` is deleted, leaving one owner of the location.
- **`vibemux_harness` model**: `HarnessState` gains `launcher: Option<LauncherKind>` and `version: Option<String>` (`#[serde(default)]`; `version` validated non-empty, `<= MAX_HARNESS_VERSION_BYTES`, no NUL); `build_refresh` returns `(snapshot, payload)` from ONE private classifier (`classify_detections`) whose `detected`/`missing` arrays are sorted alphabetically (Python `services.py` parity); `build_rows_from_registry(registry, config)` derives rows from persisted state (used by the `--cached` path and the post-commit live path) while `build_rows(registry, config, detections)` remains for pre-persistence uses; `HarnessEntry` switches from the inert `deny_unknown_fields`+`flatten` combination to a strict manual `Deserialize` (private flat helper with explicit key list). Dead API deleted: `HarnessError::UnknownHarness` (and its `harness_unknown` arm), `profile_for`, `MAX_HARNESS_LAUNCHER_BYTES`, `HarnessProfile.agent`.
- **`vibemux_store`**: `project_id() -> Result<Option<ProjectId>, StoreError>` via `SELECT envelope_json FROM events ORDER BY sequence LIMIT 1` + `EventEnvelope::from_json_slice` (typed, O(1)); `seed_harness_projections` uses it. `commit_harness_snapshot(detections, checked_at, idempotency_key)` and `commit_harness_switch(harness_name, idempotency_key)` each run ONE transaction: `project_id` minted inside (only when the events table is empty), `build_refresh` for the snapshot, `find_duplicate` → return the in-scope previous registry/config/payload on duplicate, else insert event + upsert registry/config projection (config seeded only when absent). The switch gates unknown name (`store_unknown_harness`) and undetected (`harness_not_detected`) BEFORE any write, and reads `from` inside the same transaction that writes so concurrent switches can never record a lying `from`. `harness_config()` self-heals an unknown persisted `default_harness` back to the `NO_DEFAULT_HARNESS` snapshot (write-path gating stays strict). MIGRATION_V3 is now atomic: the version-3 marker insert and the harness projection seed share one transaction, so a seed failure leaves `schema_migrations` without version 3 and a reopen retries, never a half-migrated store (with a corrupt-first-event + reopen regression test). Reused `projection`/`find_duplicate`/`insert_event`/`upsert_harness_projection` internally; removed the dead `detected()` test helper and its call sites.
- **`vibemuxd`**: new `WriterRequest::CommitHarnessSnapshot` / `CommitHarnessSwitch` arms route to the store methods via the existing `store_error` helper; `WriterRequest::ProjectId` is backed by `store.project_id()` (O(1)) — the old `events()`-scan `WriterHandle::project_id` is deleted, and the unused writer forwarding methods from the earlier round that became caller-less are removed. `load_probe_detections` uses `cache::read_cache`; a single agent's invalid version/path DEGRADES only that agent to `detected = false` (fail-closed; a bad row never aborts the refresh), and detection path = `agent.path` (None when absent, never `command_name`). `harness_refresh` is ONE writer round-trip (commit) then rows built from `outcome.registry` + `outcome.config` via `build_rows_from_registry()` (the separate config pre-read, the `detections.clone()`, and the 4th `harness_rows` call are deleted); the cached arm uses the same row builder and stops shipping a synthetic detections payload. Control versioning is now `ControlOperation::minimum_protocol_version()` (Health/Shutdown=1, PluginStatus=2, Harness*=3) with `supported_control_version` accepting v1/v2/v3; the hand-synced `matches!` list and the PluginStatus special case are deleted. Remote-error fidelity: codes with known namespaces (`store_`/`writer_`/`harness_`/`control_`/`a2a_`/`plugin_`) round-trip as `ControlError::Remote{code}`, everything else collapses to `control_remote_error` (store-side error round-trip test added).
- **`vibemux_cli`**: `harness_list` maps errors through `map_harness_control_error`, which puts the harness NAME into `harness_not_detected`/`store_unknown_harness` user messages and the REAL cache path (from `DaemonPaths::probe_cache_path()`) into `harness_probe_cache_missing` with the `vibemux_probe --write-cache --project-root <root>` remediation (the hardcoded "." path is deleted). The `harness_unknown` mapping arm is gone with the variant. `harness_client`'s doc is corrected: the harness surface requires a running daemon and never auto-spawns it. Main JSON shapes unchanged.

**Live validation (native Windows, real installed harnesses)**
- Fresh, empty temp project; dev-profile `vibemuxd`/`vibemuxctl`/`vibemux_probe` at `target/debug`. Daemon start with **no cache**: `{"healthy":true,"ok":true,...,"store_schema_version":3}`. `harnesses` with no cache → exit 4, `harness_probe_cache_missing`, message naming `<tmp>\.vibemux\probe_cache.json` and `--write-cache`.
- `vibemux_probe --write-cache --project-root <tmp>` → exit 0; cache created with `claude` (`path=C:\Users\...\.local\bin\claude.exe`, `launcher=direct_executable`, `version=2.1.217 (Claude Code)`, `code=version_verified`) and `grok` (`path=C:\Users\...\.grok\bin\grok.exe`, `version=grok 1.0.30 (...)`, `code=version_verified`) — both resolved `path` values non-null.
- `harnesses` → exit 0, 10 rows, claude/grok `available=true` with `launcher`/`path`/`version`; `switch grok` → `{"default_harness":"grok","from":"none","ok":true,"to":"grok"}`; `switch qwen` → exit 4 `harness_not_detected` naming qwen; `switch bogus-harness` → exit 4 `store_unknown_harness` naming bogus-harness.
- Daemon stop then start (new PID), `harnesses --cached` → 10 rows with persisted `default=true` on grok and persisted `launcher`/`version`; `daemon health` keeps `store_schema_version: 3`.
- JSON-leak scan over the emitted JSON (live + cached harness rows, health, probe cache): no `vibemux_rust.sqlite3`, no database path, full environment, or secret string in any JSON; the only path named is the probe-cache path in the spec-mandated cache-missing error. Cleanup: daemon stop, no `vibemuxd.exe` processes remain, temp dir removed.

**Validation**
- `cargo fmt --all -- --check` — clean.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — clean.
- `cargo test --workspace --all-features` — 54 suites, `293 passed; 0 failed; 2 ignored` (the two ignored are the standard `--ignored`-gated exceptions).

**Deferrals**
- `NO_DEFAULT_HARNESS` sentinel kept as-is for wire-shape stability.
- `parse_harness_arguments` (CLI harness subcommand parsing) left on its existing implementation; not unified this round.
- `commit_projection` shared-tail reuse (expressing the a2a commit path through `find_duplicate`/`insert_event`/`upsert_harness_projection`) not performed — the only `crates/vibemux_store/src/a2a.rs` change is test adjustments for the v3 seed-event baseline.

### 2026-09-23 - Windows ACL-marker publication race (issue #6)

**Scope**
- Fix the load-sensitive `rust-windows` CI flake (issue #6): four `vibemux_cli::recovery` tests intermittently panicked at `ensure_runtime_dir()` with `ControlRuntimeSecurityInvalid` on loaded runners while identical code was green on adjacent commits.

**Root cause**
- `ensure_windows_control_acl` published the shared `%LOCALAPPDATA%\VibeMux\runtime\.acl_v1` marker with `create_new` + `write_all` — not atomic. `vibemux_cli` is the second workspace member, so its lib test binary is the first process in a CI job to touch the marker, and its parallel recovery tests all raced first-touch initialization: the winner created an EMPTY marker and every loser hitting `AlreadyExists` inside the create→write window (widened to seconds by runner scheduling load) read empty contents and hard-failed. The same window permanently poisoned the marker when a writer died mid-publish (`AlreadyExists` + invalid was a hard error that never self-healed). Secondary: each racing caller spawned its own `powershell.exe` secure+verify pair, multiplying helper load toward the 5 s `HELPER_TIMEOUT`.

**Implementation**
- `crates/vibemuxd/src/process.rs`: marker publication is now atomic — contents are written to a per-pid temporary file (`.acl_v1.<pid>.tmp`), synced, then `std::fs::rename`d onto the marker, so readers see either no marker or the complete contents, never a half-written one, and a crashed writer leaves at most a stale temporary (overwritten by the next same-pid attempt) instead of a poisoned marker. `ensure_windows_control_acl` serializes the secure→publish→verify sequence behind a process-wide, poisoning-tolerant mutex (double-checked around the existing fast path), so a first-touch pays one secure+verify pair and every concurrent caller early-returns on the valid marker. A stale or invalid existing marker is healed by renaming over it — only after re-running `secure_user_directory` and re-verifying the marker ACL, so the trust chain is unchanged.
- `crates/vibemux_platform/src/windows_security.rs`: `HELPER_TIMEOUT` 5 s → 15 s (a liveness knob for one helper on a loaded runner, not a security check).

**Validation**
- Deterministic regression test demonstrated pre-fix-red / post-fix-green: `ensure_runtime_dir_heals_a_stale_acl_marker` writes an empty marker and expects ensure to heal it; run against `328e72c`'s implementation it failed with exactly `ControlRuntimeSecurityInvalid`. Plus `acl_marker_publication_replaces_stale_markers_without_debris` (atomic replacement of a stale marker, no temporary left behind).
- First-touch concurrency smoke reproducing the CI scenario (global marker deleted before each round, then `cargo test -p vibemux_cli --lib recovery::` — the exact binary and tests that flaked, in parallel): 5/5 rounds green.
- Windows gates on this change: `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean; `cargo test --workspace --all-features` — 54 suites, 297 passed / 0 failed / 2 ignored. The entire diff is `#[cfg(windows)]`-gated (the unix compile surface is unchanged); `rust-ubuntu` CI re-validates the unix side.
