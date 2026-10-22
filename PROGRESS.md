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
| Current implementation branch | `codex/remaining_runtime` for the owned follow-up; local `main` remains at `a639ed5` (feature branches previously merged back via direct merge; PR #3 and the two-shell frontend branch land through `feat/3-two-shell-frontend-integration`); prior baseline `codex/m7_a2a_supervisor` per `85697d6743b01b8e0aa10c48d79837dcc56c007c` with its tested input fingerprint in `docs/evidence/a2a_source_manifest.json` and the 2026-08-31 publication audit |
| Audited implementation baseline | PR #1 head `fce05cffe628ce65c49620a06f6bd1ef7eaadb89`, merged as `6498e6aa711f751d9f9e50e37034d61c17d4b341` |
| Python package version | `0.1.0a0` |
| Rust workspace version | `0.2.0-alpha.1` (MSRV and pinned compiler 1.92) |
| Product target | Windows 10/11 first; WSL2/Linux development and compatibility |
| Current implementation | Python 3.12 behavior-reference CLI plus a Rust 2024 pre-alpha core workspace |
| Current Rust scope | Typed domain/events, SQLite store, single-writer daemon, authenticated local IPC/lifecycle, authenticated local stateful A2A and supervisor workflow, read-only probe/frontend shell, plugin protocol v1 foundation, plugin supervisor, and daemon-owned registry with bounded recovery and read-only IPC v2 status, plus opt-in daemon-owned harness dispatch (ADR 029: Codex/Claude one-shot turns, ACP initialize-only probes, store schema 4, Control v5; fixture evidence plus live initialize-only probes on native Windows), and offline-only dual-track coding workflows (ADR 031: `cooperate`/`compare` over owned worktrees with review, verifier, and integration gates, store schema 5, Control v6; fixture workers only), and the DSH SDK session substrate (ADR 032: pure session state machine, shared driver, daemon service, Control v7, CLI and terminal UI; no live vendor or persistence evidence), and the advisory decision port with per-task harness routing (ADR 034: pure `vibemux_decision`, one-shot `vibemux_decision_peer` for TypeSafe Jev, local rules, store schema 6, Control v8; fixture peers and a loopback test server only, no live Jev call), plus runtime authority/gateway/controller foundations (ADR 036: schema 7), an explicit model supervisor driver (ADR 037: schema 8, Control v9, offline fixtures; live and native acceptance incomplete), scoped supervisor context actions (ADR 038), and operator contract edits (ADR 039: schema 9, Control v10), plus the standalone native harness crate `vibemux_harness_native` (ADR 040: bounded Codex/Claude/ACP request dispatch with Job Object containment; no daemon, store, or Task/Run binding) and the unified `vibemuxctl` command tree with a read-only workspace inventory and navigable dashboard (ADR 041) |
| Current default entry point | Python `vibemux` for the complete prototype workflow; Rust `vibemuxctl` for daemon lifecycle, the harness registry/switch surface (control protocol v3), harness dispatch (control protocol v5), and dual-track workflows (control protocol v6), DSH sessions (control protocol v7), and advisory decisions (control protocol v8), plus explicit model-supervisor start/status (control protocol v9) and operator semantic contract edits (control protocol v10) |
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

### 3.2 Process topology, crate layout, and contracts

Detailed process topology, current crate responsibilities, dependency direction, core/plugin boundary, plugin model, and A2A transport policy live in [docs/architecture.md](docs/architecture.md). The decision summary in §3.1 above captures what is accepted and what remains unimplemented.

### 3.3 Core versus plugin boundary

See [docs/architecture.md §Core versus plugin boundary](docs/architecture.md#core-versus-plugin-boundary).

### 3.4 Plugin model

See [docs/architecture.md §Plugin model](docs/architecture.md#plugin-model) and [ADR 013](docs/adr/013_rust_core_plugins.md) and [ADR 021](docs/adr/021_plugin_protocol_v1_foundation.md).

### 3.5 External A2A transport policy

See [docs/architecture.md §External A2A transport policy](docs/architecture.md#external-a2a-transport-policy) and [ADR 024](docs/adr/024_stateful_a2a_supervisor.md).

---

## 4. Current Repository Audit

### 4.1 What exists

| Area | Status | Current evidence | Assessment |
|---|---|---|---|
| Python package and Typer CLI | `PARTIAL` | `pyproject.toml`, `src/vibemux/`, Python reference fixtures | The current complete prototype workflow remains runnable; original P0 correctness fixes are implemented, but Python is not the future authoritative core |
| Windows-first product path | `PARTIAL` | README, platform docs, Windows CI, protected control runtime | Native daemon lifecycle and the declared ACL boundary have evidence; live WezTerm/ConPTY multi-harness operation is not implemented |
| Rust workspace and canonical domain/event core | `PARTIAL` | root `Cargo.toml`, `vibemux_types`, `vibemux_events` | Rust 2024/MSRV workspace, typed IDs, state machines, events, and property tests exist; tracing and public compatibility policy remain incomplete |
| Rust SQLite store and single-writer daemon | `VERIFIED` | `vibemux_store`, `vibemuxd`, `vibemux_cli`, `vibemux_platform` | M3 is implemented within scope: migrations/WAL, atomic state+event, replay/idempotency, bounded writer, authenticated IPC, lifecycle, explicit recovery, and Windows protected runtime |
| Rust CLI | `PARTIAL` | `vibemuxctl daemon` with `start`, `health`, `inspect`, `recover`, or `stop`; `vibemuxctl harnesses`, `switch`, `dispatch`, `decide`, and `decisions`; the unified `status`, `workspace status`, `probe`, `ui`, and standalone `harness` commands with `--format auto|text|json` (ADR 041) | Lifecycle, harness registry, one-shot harness dispatch, advisory decision commands, the read-only workspace/probe/dashboard views, read-only `tasks list`/`show` and `plugins status`, and observation-only `terminal inspect`/`link`/`focus`/`unlink` exist, with `scripts/install_local_tools.ps1` to install the release binaries together; standalone task creation and `send` (pane input) have no Rust equivalent |
| Plugin protocol v1 foundation | `VERIFIED` | `vibemux_plugin_protocol`, checked-in Protobuf/TOML fixtures | M4.0 is verified as an isolated private pre-alpha wire/manifest/negotiation/lifecycle foundation; it does not spawn or mutate state |
| Plugin process supervisor foundation | `VERIFIED` | `vibemux_plugin_supervisor`, real mock-child process tests | M4.1 is verified as an isolated supervisor foundation with bounded channels, deadlines, cancellation, heartbeat, stderr cap, shutdown, and crash containment |
| Daemon-owned plugin registry and read-only status | `VERIFIED` | `vibemuxd::plugin_registry`, explicit startup config, IPC v2 `plugin_status` | Native Windows real mock-child/standalone-daemon evidence; lifetime budgets/backoff/quarantine; no writable plugin API, operator recovery, SDK, vendor plugin, or Task/Run routing |
| A2A adapter and supervisor | `PARTIAL — LOCAL STATEFUL` | `vibemux_a2a`, daemon supervisor, optional model peers | Authenticated HTTP+JSON/JSONRPC/gRPC, task/artifact/status/cancel/subscription mapping, atomic canonical bindings and local supervisor verification are implemented; remote/TLS, full ITK, history and restart recovery remain incomplete |
| Probe and frontend shell | `PARTIAL` | `vibemux_probe`, `vibemux_frontend` | Read-only evidence collection, the Ratatui diagnostic dashboard (with re-probe and harness selection), the read-only multi-pane console with content-free daemon views (ADR 042), DSH transcript search, and the egui Supervisor Chat workspace over Control v4 task queries (ADR 028) with the ADR 030 shortcut table and quick switcher exist; continuous coordinator chat, real PTY/ConPTY attachment, and native-TUI lifecycle do not |
| Python workspace/terminal safety | `PARTIAL` | `workspace.py`, `terminal.py`, `services.py` | Base-commit diff, cleanup plan, compensation, ownership, and reconciliation are implemented; live backend and Windows path-edge coverage remain incomplete |
| Rust workspace/worktree parity | `PLANNED` | no Rust workspace crate | Git worktree lifecycle, cleanup, artifacts, and reconciliation are not implemented in Rust |
| Rust terminal plugins and native-TUI attachment | `PLANNED` | `vibemux_terminal_observer` (observation-only WezTerm list/focus; not live-verified against a running WezTerm) | Mock/WezTerm/tmux plugin parity, pane ownership, and ConPTY attach are not implemented |
| Standalone native harness dispatch | `PARTIAL — FIXTURE ONLY` | `vibemux_harness_native`, `vibemuxctl harness`, `examples/harness`, fixture-binary tests (ADR 040) | Bounded one-shot Codex (`app-server`/`exec`), Claude stream-json, and ACP (OpenCode/Copilot/Grok) request dispatch with Windows Job Object containment and configuration SHA-256 pinning, run in the CLI process with no daemon admission, store records, or Task/Run binding; branch evidence is initialize-plus-clean-exit for Codex, Claude, OpenCode, and Copilot, Grok shutdown unverified, no model generation executed; ADR 029 remains the daemon-owned path |
| Structured vendor harness adapters | `PARTIAL` | `vibemux_harness::dispatch`, `vibemuxd::harness_dispatch`, fixture-binary tests (ADR 029) | Daemon-owned one-shot Codex (`exec --json`, `app-server`) and Claude stream-json execution plus ACP initialize-only probes (OpenCode/Copilot/Grok), verified against a fixture binary on Windows and Linux and by live initialize-only probes of the installed Codex, Claude, OpenCode, Copilot, and Grok CLIs on native Windows; no live execution turn, ACP execution, concurrency, approvals, or Pi/Gemini adapter |
| Decision port and per-task harness routing | `PARTIAL — FIXTURE ONLY` | `vibemux_decision`, `vibemux_decision_peer`, `vibemuxd::decision`, store schema 6, Control v8, `vibemuxctl decide`/`decisions`/`dispatch submit --decision` (ADR 034) | Advisory, content-free decisions over the detected executing harnesses with a threshold, local label rules, project-default fallback, and escalation; a decided submit re-runs every ADR 029 gate and runs once. The Jev peer needs both the config gate and `--allow-external` and receives enumerated facts only. Verified against fixture peers and a loopback test server; no live Jev call, calibrated threshold, or other decision kind |
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

The current crate responsibilities, process topology, and dependency direction live in [docs/architecture.md](docs/architecture.md). The root `Cargo.toml` remains the authoritative source for workspace membership.

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
- [x] Windows protected current-user control runtime and cross-user/remote access boundary (effective root/leaf/artifact ACLs re-verified natively — read-only Win32, no helper process — in two phases on every trusted daemon start: pre-descriptor-publication and post-publication — issue #7 Stage 1 + issue #8 Stage 2 / ADR 025, 2026-09-23).

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

**Status:** `PARTIAL` — [ADR 029](docs/adr/029_daemon_harness_dispatch.md) implements first-party structured adapters inside `vibemuxd` (the vendor CLI still runs as a separate contained process) for one-shot Codex and Claude turns, with ACP initialize-only probes. Evidence is fixture tests plus live initialize-only probes on native Windows, with no live execution turn, and the exit criteria below are not yet demonstrated.

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

ADR 031 adds an offline-verified dual-track slice to this milestone: TaskSpec-owned paths per worker, daemon-collected candidates, independent review and trusted verifier receipts per candidate, bounded repair, comparison selection, and integration into a fresh worktree without commit or merge. Live workers, DAG dependencies, and the manual merge gate remain open.

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

The user-requested ADR 029 harness dispatch slice (one daemon-owned dispatch per project, read-only protocol profiles, Codex/Claude execution, ACP probes) was implemented ahead of item 5 and has only fixture evidence and live initialize-only probes. It does not establish vendor plugin readiness, concurrent multi-harness execution, or the M6 exit criteria.

The user-requested ADR 031 dual-track workflow slice (two workers in owned worktrees, independent review, trusted verification, integration into a fresh worktree) was also implemented ahead of item 5. Its evidence is offline fixture evidence only: there is no AAG gateway client, live worker, or native TUI session, so it establishes neither vendor readiness nor any live acceptance.

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

> **Deviation resolved (2026-09-23, issue #8 Stage 2):** the Stage 1 batched-PowerShell verification pass (which added ~400 ms per trusted start, see the 2026-09-23 (3) entry) was replaced by native read-only Win32 verification (ADR 025), measured back-to-back on the same machine and project: Stage 1 p50 970.98 ms / p95 1062.64 ms → Stage 2 p50 ~517 ms / p95 ~582 ms (median of three 20-sample runs; range p50 510.65–564.93, p95 573.09–603.74), removing the ~480 ms helper-spawn cost. The remaining absolute p95 sits at this benchmark machine's pre-existing level — the pre-change baseline measured 482.80/558.06 ms in the Stage 1 session, already above the 500 ms target, which was set from earlier, less loaded measurement windows (e.g. 355.38/383.31 ms). The verification itself now costs ~15 ms per start (two native phases, no process spawn); the two-phase ordering additionally closes the Stage 1 token-exposure window (see the 2026-09-23 (4) entry).

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

### 2026-10-01 — Frontend daemon-call efficiency (Supervisor Chat)

**Status change**
- Supervisor Chat daemon-call path: unchanged behavior, materially fewer control round-trips and no per-frame snapshot copy. The GUI remains `PARTIAL`; this changes no milestone gate, adds no capability, and changes no user-visible surface.

**Problem**
Three costs scaled with workspace size rather than with what actually changed:
1. **Every poll re-verified the runtime.** `apply_action` called `connect()` per action, and on Windows that re-walked the protected control root, the runtime leaf, the ACL marker, the descriptor, and the writer lock through native `GetNamedSecurityInfoW`, then re-read the descriptor — on every 5 s tick and every user click.
2. **Every poll refetched every task detail.** The refresh path issued one `frontend_task` read per listed task (bounded at 256) regardless of whether anything had changed, sequentially.
3. **Every frame deep-copied the whole snapshot.** `render_frame` cloned the entire `SupervisorSnapshot` — all tasks, runs, events, and terminal candidates — and `render_task_viewports` cloned it again per open window per frame, at the existing 100 ms repaint cadence. `persist_if_due` performed a second full clone merely to read `mode`.

**Implemented**
- **`vibemux_types` / `vibemux_store` — a complete change signal.** `FrontendTaskSummary` gains `detail_revision` (`#[serde(default)]`): the highest sequence across the Task projection, every Run projection belonging to the task, and every event carrying its `task_id`. `latest_sequence` alone is provably insufficient — `commit_transition` writes the Task projection only when the transition carries a `task_status`, so an admitted dispatch claiming its Run (`attempt.rs`, `task_status: None`) moves the Run projection and leaves the task sequence untouched. `resolve_detail_revisions` fills the field for a whole page in two grouped scans instead of one query per task, and the list and detail paths publish the same value for the same task so a client never compares two different meanings of "changed". `0` means "unknown" and forces a fetch, so a client meeting an older daemon degrades to the previous unconditional behavior.
- **`vibemuxd` — non-secret generation.** `ControlClient::generation()` returns `ControlGeneration { version, process_id, endpoint }` (a local IPC name, never a credential) so a client can detect that the daemon behind the descriptor was replaced. `same_generation` is unchanged.
- **`vibemux_frontend::supervisor_client` — reuse, skip, bound.** A `ClientSession` carries the verified client and its generation across polls: an unchanged generation keeps its existing verification, a republished descriptor forces re-verification, and any failed call invalidates the cache. Detail reads are gated on `detail_revision` (a carried-forward view with an unchanged revision is not refetched) and run through a `MAX_CONCURRENT_DETAIL_READS = 8` semaphore, applying results in task order so the view never depends on completion order. `harness_snapshot` / `plugin_status` moved to a 12-tick cadence (still forced on the first successful poll and after any failure). The reader publishes `Arc<SupervisorSnapshot>` and mutates through `Arc::make_mut`, so an idle poll moves the existing allocation instead of rebuilding the task tree.
- **`vibemux_frontend::gui` — no per-frame copy.** The app holds `Arc<RwLock<Arc<SupervisorSnapshot>>>`; `render_frame` takes one shared borrow for the whole frame and hands it to the sidebar, top bar, drawer, conversation, and diagnostics, and each deferred task window clones the handle rather than the body. `persist_if_due` reads `mode` in place. `LiveApp` publishes by moving the shared handle.

**Behavior preserved**
- No new capability, no new wire operation, and no protocol version bump: `detail_revision` is an additive field on an existing v4 payload, and the control surface, sanitization rules, and content-free event/status/output contracts are untouched.
- An explicit `LoadTask` still always reads the detail even when the revision is unchanged, because the operator asked for that task specifically.
- The optimistic-concurrency guards are unchanged: a detail never moves a task backwards, a cross-project detail is refused, and a missing task is still dropped.
- A daemon restart is still detected and still clears terminal associations and the observed-terminal set.

**Evidence**
- `cargo fmt --all -- --check` — clean.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — clean (exit 0).
- `cargo test --workspace --all-features` — the crates this change touches are green: `--exclude vibemux_cli` exits 0 with no failures. `vibemux_frontend` alone is 93 unit tests (1 pre-existing ignored) + 17 integration across `cli`, `palette_parity`, and `supervisor_ui`; the `vibemux_store` frontend module is 7. See the pre-existing `vibemux_cli` failures recorded below.
- New tests: `vibemux_store` `detail_revision_is_stable_when_nothing_changes_and_moves_when_a_run_changes` (pins revision stability across identical reads, asserts the run-only commit leaves `latest_sequence` pinned while the revision moves, and asserts list/detail agreement), `detail_revision_ignores_other_projects_and_unrelated_tasks`; `vibemux_frontend` `unchanged_tasks_keep_their_detail_and_changed_ones_are_refetched` (end-to-end against a real `DaemonControlServer`: idle refresh does not move the revision, a recorded transition does, and the new run state reaches the view across a daemon restart), `client_session_reuses_one_verified_generation_and_reverifies_after_a_change` (same generation is retained; a republished descriptor yields a new generation; a failure clears the cache).
- Not measured: no wall-clock or round-trip-count benchmark was captured. The reduction is structural (an idle poll performs zero detail reads instead of one per task, and one runtime verification instead of one per tick) and is argued from the code path, not from a timing run.

**Pre-existing failures observed in `vibemux_cli` (not caused by this change)**
`vibemux_cli` is not modified by this change, and the rest of the workspace is green: `cargo test --workspace --all-features --exclude vibemux_cli` exits 0 with no failures. The full `--workspace --all-features` run does not reach green on this machine, for reasons in `vibemux_cli` only:
- `vibemux_cli --test bootstrap_process` / `startup_timeout_terminates_only_the_spawned_fixture` fails at `assertion failed: started.is_file()` (`crates/vibemux_cli/tests/bootstrap_process.rs:54`). Reproduced **3/3 on the stashed clean tree**, and 1/3 with the change applied — a load-sensitive assertion against a 150 ms startup deadline and a spawned hang fixture, not a regression.
- `vibemux_cli --lib` `recovery::tests::{descriptor_only_recovery_and_pid_mismatch_fail_closed, live_daemon_is_never_recoverable}` fails intermittently during a full parallel workspace run. The failing **set is not stable**: one run failed 2 of them, a later run failed only 1. Both pass **3/3 in isolation on the clean tree and 3/3 in isolation with this change applied**, matching the documented load-sensitive recovery/ACL-marker test class.

Conclusion: the frontend, store, types, and control-path changes are green. The remaining red is confined to the pre-existing `vibemux_cli` timing/ACL-marker flake class and belongs on its own branch; the current workaround is `--test-threads=1` or re-running those two binaries.

**Remaining**
- The per-task detail read is still one round-trip per changed task; a batched v4 detail query would remove the remaining N+1 when many tasks change at once. That is a protocol change and is deferred.
- `MAX_LOADED_TASKS` remains 256 with a "first N tasks" notice; there is no virtualized list, so a very large workspace still builds 256 rows per frame.
- Terminal observation is still re-inspected on every poll for each observed run.

#### Integration into main (2026-10-17, native Windows, Rust 1.92.0)
- Ported onto main's egui 0.34 shell: `eframe::App::ui`, `egui::Panel::*::show_inside`, and `render_topbar`/`render_drawer` taking the root `Ui`. `vibemux_mcp` test fixtures gained the new `detail_revision` field. The Control version is unchanged by this entry: `detail_revision` is additive on the existing frontend payload, and main's `CONTROL_PROTOCOL_VERSION` stays V10.
- `cargo fmt --all -- --check` and `cargo clippy --locked --workspace --exclude vibemux_web --all-targets --all-features -- -D warnings` are clean.
- `cargo test --all-features` for `vibemux_types`, `vibemux_store`, `vibemux_frontend`, `vibemux_mcp`, `vibemuxd`, and `vibemux_cli` passed with 0 failures (`vibemux_frontend` lib: 120 passed, 1 ignored; `vibemuxd` plus `vibemux_cli`: 310 passed, 1 ignored). The first parallel build hit rustc allocation failures under memory pressure; it was rerun with `-j 4`.
- `vibemuxd` `workflow_cooperate` (which needs `test_helpers`, enabled by `--all-features`) passed, including `two_tracks_build_taskboard_with_a_scoped_bundle_and_isolated_integration`. That test failed on main before the rendezvous fixture step from `feba1cb` was merged with this entry.
- Not run: `vibemux_web`, the Python suite for this entry, and any interactive GUI session. The GUI is verified only by the egui frame tests.

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

### 2026-09-23 (2) - Issue #6 review round: ensure-path straddle + hardening

**Scope**
- Three-angle review (security semantics / concurrency / tests+conventions) of the `f18db99` issue-#6 fix. Security verdict: **no weakening** — the removed `AlreadyExists`+invalid hard-fail defended nothing (the 22-byte marker constant is public in the binary and the fast path always trusted it contents-only), and the heal path is strictly stronger for corrupted markers (it re-secures the root and ACL-verifies the republished marker instead of merely refusing). The concurrency review **reproduced** one real defect (2/20 on a warm marker) and the tests review confirmed it independently; both are fixed here, plus two hardening items.

**Findings fixed**
- **Test-side straddle (reproduced)**: `ensure_runtime_dir_heals_a_stale_acl_marker` wrote an empty marker into the shared global root while sibling tests' `ensure_runtime_dir` re-read the marker lock-free at the tail (`validate_runtime_dir` → `validate_windows_acl_marker`) — the exact issue-#6 signature, reintroduced by the regression test itself on warm/self-hosted machines (fresh hosted CI starts marker-absent, so it was low-risk there). Fix: the ensure path now validates locations only via a private `validate_runtime_locations` split; the marker re-read was redundant there because `ensure_windows_control_acl` has just established (and, when healing, ACL-verified) the marker under the init lock. Public `validate_runtime_dir` keeps the full check for read-only inspection callers (`vibemuxctl daemon inspect/recover`). Post-fix warm-marker repro: **0/20**.
- **Init lock had no regression test**: `publish_acl_marker`'s temporary name is per-PROCESS, so `control_acl_init_lock` is load-bearing for intra-process publication (without it, threads interleave truncate/write/rename on the same temporary and can publish an empty marker or fail a rename). New `concurrent_first_touch_serializes_marker_publication` (global marker removed, 4 threads × distinct project roots, every ensure must succeed) — verified to pin the lock: with the lock temporarily removed it failed **8/10** runs; with the lock, 0/20.
- **Comment accuracy**: the `HELPER_TIMEOUT` comment no longer claims all callers serialize (the budget is per-invocation; verify callers and separate processes may run helpers concurrently); the init-lock comment no longer conflates cross-process first-starts (made safe by the atomic rename, not by the lock); `publish_acl_marker`'s doc now states the in-process lock dependency explicitly.

**Hardening**
- `publish_acl_marker` opens its temporary with `FILE_FLAG_OPEN_REPARSE_POINT`: a link planted at the temporary path now fails closed instead of letting the writer truncate an arbitrary target. The security review rated the planted-link vector low and out-of-model (a same-user attacker can already write user-writable files; it would only matter if `vibemuxd` were run elevated) and noted it is not a regression — the pre-fix `create_new` on the marker path followed dangling symlinks and CREATED the target.
- A test-only `marker_test_lock` serializes the two tests that deliberately invalidate the shared global marker, so neither can land an empty-marker write inside the other's read-then-assert window.

**Deferred (follow-up issue filed)**
- The marker is a contents-only trust anchor: `verify_control_runtime_acl`/`verify_control_artifact_acls` have no production call sites, so daemon start trusts a contents-valid marker without re-verifying the root ACL ("verify once, trust forever"). Pre-existing in both old and new code and NOT made more reachable by this fix (the constant is public; forging it directly achieves the same state on either version). Wiring root/artifact ACL verification into daemon start is tracked separately.
- A publisher killed between temp-write and rename leaves a ~20-byte `.acl_v1.<deadpid>.tmp` in the control root (no foreign-pid sweep; pid reuse overwrites it harmlessly). Cosmetic; not fixed.

**Validation**
- Windows: `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean; `cargo test --workspace --all-features` — 54 suites, **298 passed / 0 failed / 2 ignored**.
- Reviewer repro re-run: warm-marker `cargo test -p vibemuxd --lib process::` **20/20 green** (was 2/20 failing before this round); lock-removal check **8/10 red** (the new test pins the lock); first-touch smoke (global marker deleted, then `cargo test -p vibemux_cli --lib recovery::`) **5/5 green**. The diff remains `#[cfg(windows)]`-gated except comment text; unix compile surface unchanged.

### 2026-09-23 (3) - Issue #7 Stage 1: per-start control-runtime ACL verification

**Scope**
- Close the "verify once, trust forever" gap filed as issue #7: the `.acl_v1` marker is a contents-only idempotency record (22 public bytes, no secret/MAC), so `ensure_runtime_dir`'s fast path accepted any start whose marker contents matched while `verify_control_runtime_acl`/`verify_control_artifact_acls` had **zero production callers**. A root/leaf/artifact ACL loosened after bootstrap (manual `icacls`, same-SID malware, misconfiguration) was never re-checked. ADR-020 line 18 already mandates "Verify the protected root and effective leaf/artifact ACLs" — this completes an accepted ADR; it is defense-in-depth, not a privilege-boundary fix (an attacker able to loosen the ACL is same-SID or higher, the accepted limitation in ADR-020 line 26).

**Implementation (Stage 1: batched PowerShell, no unsafe, no new dependencies)**
- `crates/vibemux_platform/src/windows_security.rs`: new `verify_restricted_path_acls(&[&Path])` verifies up to 16 paths in **ONE** `powershell.exe` invocation (paths cross only through numbered `VIBEMUX_VERIFY_PATH_<i>` env vars; empty/oversized batches fail closed with `HelperRejected`). The batched script is now the single implementation of the per-path rule set (exactly three Allow ACEs for current user/`SYSTEM`/Builtin Administrators, current user FullControl; first failure prints `stage_<s>_index_<i>` ≤ 16 bytes, no path text — ADR-020 lines 17/21). `verify_restricted_path_acl` delegates to a batch-of-1 and preserves its stage-only error shape. New `PlatformError::AccessControlInvalidAt { stage, index }` (same wire code `platform_access_control_invalid`). New `helper_slot()` mutex serializes helper invocations per process (bounds `powershell.exe` fan-out from parallel callers — the issue-#6 load class; lock order is always `control_acl_init_lock` → `helper_slot`). The old single-path verify script is deleted.
- `crates/vibemuxd/src/process.rs`: new `DaemonPaths::verify_control_security()` — one batched call over the index-contract order [1 root, 2 runtime leaf, 3 marker, 4 descriptor, 5 writer.lock]; any failure → `ControlRuntimeSecurityInvalid`. The two existing verifiers are rebased onto the batch API. Legacy compatibility artifacts stay project-local by design and are not part of this surface. Nothing in the issue-#6 ensure/marker logic was touched.
- `crates/vibemuxd/src/control.rs`: `start_with_writer_locks` gains a `security: Option<&DaemonPaths>` parameter — `Some` only from the three trusted `start_for_paths*` constructors (reached after `ensure_runtime_dir`), `None` from the raw-path constructors (plain test tempdirs). Verification runs right after `DescriptorGuard::publish` (all five paths exist) and before any plugin/supervisor machinery, through `spawn_blocking`. New `ControlError::ControlRuntimeSecurityInvalid` (code string identical to the `DaemonPathError` variant). **Consistency fix**: all four pre-existing constructor error paths (registry register / supervisor start, both cfg branches) now join the writer via `WriterWorker::shutdown` through `spawn_blocking` instead of relying on `Drop`, which only `try_send`s a shutdown signal and DETACHES — leaving stale lifecycle locks behind.
- `crates/vibemux_cli/src/lib.rs`: `classify_daemon_exit` — the Windows launcher detaches the daemon so its stderr never reaches the CLI; on the child-exited-without-artifacts failure path only, one read-only `verify_control_runtime_acl` re-check distinguishes `daemon_control_runtime_security_invalid` from the generic `daemon_start_failed`. Zero helper spawns on the happy path. Unix behavior unchanged.

**Launcher watchdog fix (pre-existing defect, discovered by the real-machine smoke and required for the CLI classification to be reachable)**
- The Windows launcher helper read its control stdin via `[Console]::In.ReadLineAsync()`. On Windows PowerShell (.NET Framework), `Console.In` is a `SyncTextReader` whose `ReadLineAsync()` **blocks the calling thread** until a line arrives — so the helper never reached its `while (-not $control_task.Wait(25)) { if ($process.HasExited) { exit 5 } }` watchdog loop. Consequence (pre-existing, independent of issue #7): a daemon that exited before becoming healthy was never observed; `vibemuxctl daemon start` always burned the full startup deadline and reported `daemon_startup_timeout`, making the fast `child_exited` classification path dead code on Windows. Isolated repro: the real helper over held-open Windows pipes produced zero watchdog ticks in 120 s while the daemon died in ~0.5 s; with the fix (`New-Object IO.StreamReader([Console]::OpenStandardInput())` — a genuinely async reader) the helper detected the exit at tick 18 (~450 ms) and exited 5. Pinned by `early_daemon_exit_fails_fast_instead_of_timing_out` (new `vibemux_exit_fixture` bin): red without the fix (10.5 s → `StartupTimeout`), green with it (1.0 s → `StartFailed`). The healthy path is unchanged (release/terminate still flow through the same stdin line).

**Tests**
- Platform unit: batched accept (secured dir + 2 inherited files), batched failing index/stage (`icacls` broadening the 3rd path → `AccessControlInvalidAt { stage: 7, index: 3 }`), empty/oversized → `HelperRejected`, single-path stage-only shape preserved.
- `process.rs` unit: `verify_control_security` green on a secured runtime with artifact stand-ins; red (`ControlRuntimeSecurityInvalid`) after loosening the project-private runtime leaf.
- Integration (`daemon_process.rs`): trusted start verifies the full surface while running and shuts down clean; with a loosened leaf, `start_for_paths` fails closed with `daemon_control_runtime_security_invalid`, leaves no descriptor/writer.lock/legacy lock, keeps the database, fails identically on retry, and the standalone `vibemuxd` binary reports the same code on stderr.
- CLI unit: `classify_daemon_exit` both arms.

**Performance deviation (temporary, Stage 2 will remove it)**
- One extra PowerShell spawn per trusted daemon start. Release benchmark with the final binaries, existing database, 1 warmup + 20 samples, same scratch project as the pre-change baseline: start-to-authenticated-health p50 **903.22 ms** / p95 **920.31 ms** / mean 900.41 ms / min 867.53 ms / max 939.32 ms (baseline on this machine this session: p50 482.8 / p95 558.06 / mean 503.38 ms; Δ ≈ +400 ms = one helper spawn). Stop p95 56.96 ms unchanged. This **exceeds the §9 "p95 < 500 ms after one-time bootstrap" target** — the target exempts only the one-time bootstrap, not per-start verification. Recorded here as a known, accepted, temporary deviation per §9 rule 1 (benchmark before optimizing): Stage 2 (native read-only ACL verification via `GetNamedSecurityInfoW`, no process spawn, tracked as a follow-up issue with a new ADR amending ADR-020's read clause) is the planned remedy, with these numbers as its justification.

**Validation**
- Real-machine smoke (release binaries): fresh temp project → `vibemuxctl daemon start` healthy, `stop` clean; `icacls /grant *S-1-5-32-545:(OI)(CI)F` on the project's runtime leaf → start refused in **1662 ms** with `{"error_code":"daemon_control_runtime_security_invalid"}` (before the launcher fix: 5561 ms `daemon_startup_timeout`), leaf left empty (no descriptor/writer.lock residue), error JSON carries no path/SID/ACL text; ACL restored → healthy start again.
- Windows gates on this change: `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean; `cargo test --workspace --all-features` — 55 suites, **308 passed / 0 failed / 2 ignored** (+10 tests over the pre-change baseline; the extra suite line is the new fixture binary's empty test target). All new production code is `#[cfg(windows)]`-gated; the unix compile surface is unchanged.

### 2026-09-23 (4) - Issue #8 Stage 2: native read-only ACL verification (ADR 025)

**Scope**
- Remove the Stage 1 performance deviation (issue #8): the batched `powershell.exe` verification pass cost ~400 ms on every trusted daemon start, above the §9 "p95 < 500 ms after one-time bootstrap" budget. Replace the **read-only** verification with native Win32 calls (~ms, no process spawn) and close the Stage 1 token-exposure window via two-phase ordering. ADR 025 amends ADR 020's "PowerShell until a separately reviewed native platform crate is justified" clause for the read path only; ACL **writes** (`secure_user_directory`) stay on the reviewed fixed PowerShell script.

**Implementation**
- `crates/vibemux_platform/src/windows_acl_native.rs` (new): the single implementation of the ADR 020 rule set — effective DACL must contain exactly three Allow ACEs, every principal in {current token user, `S-1-5-18` LOCAL_SYSTEM, `S-1-5-32-544` Builtin Administrators}, current user holding FullControl (`FILE_ALL_ACCESS` 0x001F01FF). Read via `GetNamedSecurityInfoW(SE_FILE_OBJECT, DACL_SECURITY_INFORMATION)` + `GetAclInformation`/`GetAce` walk + `EqualSid`; the descriptor is freed with `LocalFree` and the token handle closed. Stage/index failure semantics are unchanged from Stage 1 (4 = missing, 5 = disallowed principal/non-Allow, 6 = no current-SID FullControl, 7 = ACE count ≠ 3, 9 = API failure) and errors carry no path/SID/ACL text. `windows-sys =0.59.0` (already a transitive workspace dependency; exact pin adds no new external surface). The Stage 1 batched PowerShell verify script is **deleted** (no dual implementation of a security rule set); `verify_restricted_path_acl(s)` API shapes and all Stage 1 callers are unchanged. `helper_slot` now guards only the write path.
- Unsafe containment (AGENTS §7.2 + ADR 025): `vibemux_platform`'s crate root moves `#![forbid(unsafe_code)]` → `#![deny(unsafe_code)]`; `unsafe` is allowed in exactly this one module with a `// SAFETY:` comment on every block. All other workspace crates keep `forbid`.
- `crates/vibemuxd/src/process.rs`: new `verify_control_pre_publish()` — root, runtime leaf, marker, writer lock (everything that exists before the descriptor does). `crates/vibemuxd/src/control.rs`: trusted starts now verify in TWO phases — phase 1 before the listener/descriptor publication (a loosened runtime refuses the start while the token-bearing descriptor does not exist yet, closing Stage 1's sub-second exposure window), phase 2 (`verify_control_artifact_acls`) after publication. Both run through `spawn_blocking`; failure paths keep the Stage 1 `writer.shutdown()` join discipline. `verify_control_security` (all five paths) remains the single-call full-surface inspection API.
- `crates/vibemux_cli/src/lib.rs`: `classify_daemon_exit` re-check is now native too (no helper spawn on any CLI path); behavior unchanged.

**Use-after-free found and fixed during development (recorded honestly)**
- The first version of the native module returned the current-user SID as a raw pointer into the `TOKEN_USER` buffer but dropped the buffer — a dangling pointer. Because the allocator usually handed back the same untouched block, it failed only intermittently: 2 of 6 full-suite rounds rejected healthy runtimes with stage 5, and a high-rate sampler caught 3 violations in 3000 rounds while `Get-Acl` simultaneously showed a clean 3-ACE DACL. Fix: the guard OWNS the buffer (`CurrentUserSid { _token, _buffer, sid }`). Pinned by `concurrent_verification_with_heap_churn_is_stable` (4 threads interleaving same-size-class poisoned allocations with verifications): **red** against the reintroduced dangling version (fails at worker 1, round 0), **green** after the fix; post-fix `cargo test -p vibemuxd --lib` ran **8/8 rounds green** (pre-fix: ~2/3 rounds failed).

**Performance (back-to-back, same machine, same scratch project, release binaries, 1 warmup + 20 samples)**
- Stage 1 (HEAD `1a112d6`, re-measured in this window): start p50 **970.98 ms** / p95 **1062.64 ms**. Stage 2: three runs p50 510.65/517.55/564.93 ms, p95 573.09/581.74/603.74 ms (median ≈ 517/582; stop p95 ~57–62 ms unchanged). Δ ≈ **−460 ms p50 / −480 ms p95** — the full helper-spawn cost removed; verification itself now costs ~15 ms (two native phases). The absolute p95 sits at this machine's pre-existing level (the Stage 1 session's pre-change baseline was 482.80/558.06 ms, already above the 500 ms target that was set from earlier less-loaded windows, e.g. 355.38/383.31 ms). The §9 deviation note is updated to "resolved" accordingly.

**Tests**
- Platform: 5 native-module tests (fixed-SID wire layouts round-trip through `ConvertSidToStringSidW` to `S-1-5-18`/`S-1-5-32-544`; token SID readable; missing path → stage 4; an `icacls`-broadened directory (Everyone) rejected at stage 7; concurrency/churn stress pin) + 1 new batched missing-index test (`stage 4, index 3`); all 6 Stage 1 windows_security tests green **unchanged** (they pin fail-closed behavior, not the implementation), including the `icacls`-loosened `AccessControlInvalidAt { stage: 7, index: 3 }` contract. CI compatibility note: the rejection test first assumed "a plain tempdir is always loosely ACLed" — true on the development machine, false on the GitHub Windows runner whose temp tree is itself restricted to exactly the three allowed ACEs (the verifier correctly ACCEPTED it and the environment-dependent assertion failed, `35869167559`); it now grants Everyone explicitly, which takes the effective count off exactly three on any machine.
- `process.rs`: new `verify_control_pre_publish_covers_the_pre_descriptor_surface` (accepts the secured pre-publish surface without a descriptor; fails closed after loosening the project-private leaf). All Stage 1 process/control/integration/CLI tests green unchanged.

**Validation**
- Real-machine smoke (release binaries): fresh temp project → start healthy / health running / stop clean; `icacls /grant *S-1-5-32-545:(OI)(CI)F` on the project's runtime leaf → start refused in **600 ms** (Stage 1: 1662 ms) with `{"error_code":"daemon_control_runtime_security_invalid"}` and — the two-phase evidence — the leaf left **completely empty: `control.json` was never created**, so the token never existed on disk; no path/SID/ACL text in output; ACL restored → healthy start/health/stop again; smoke state cleaned.
- Windows gates on this change: `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean; `cargo test --workspace --all-features` — 55 suites, **315 passed / 0 failed / 2 ignored** (+7 over Stage 1). All new production code is `#[cfg(windows)]`-gated; the unix compile surface is unchanged.

### 2026-09-24 - Issue #8 code-review fix round (all 12 findings)

**Scope**
- Close every finding of the max-effort review of `1a112d6..790953e` (10 finder angles, 13 candidates, per-finding verification: 11 CONFIRMED, 1 PLAUSIBLE, 1 REFUTED). V12 (opt-in `security: Option<&DaemonPaths>` design seam) was REFUTED by the verifier — `tests/daemon_process.rs::trusted_start_fails_closed_when_the_runtime_acl_is_loosened` distinguishes verified from unverified construction in CI — and stays an observation note only: if a future caller passes `None` outside a raw test-tempdir constructor, the seam deserves its own gate.

**Fixes (review finding ids)**
- V1 (critical, PR-introduced): `windows_acl_native::verify_path` now maps a NULL `ppDacl` out-parameter (object has NO DACL — the maximally loose state) to the stage-7 fail-closed answer BEFORE `GetAclInformation` could dereference it; the first version crashed the process with STATUS_ACCESS_VIOLATION instead of refusing. Pinned by `null_dacl_is_a_count_violation_not_a_crash` (true NULL DACL via `SetNamedSecurityInfoW(pDacl=NULL)` inside the unsafe-reviewed module test — icacls cannot create one; the first attempt with `icacls /inheritance:r` silently left the explicit ACEs and stayed green, an environment-dependent no-op).
- V2 (pre-existing): CLI `classify_daemon_exit` now verifies the phase-1 **surviving** surface (root/leaf/marker — descriptor and writer.lock are gone by definition on a failed start) instead of root+leaf only, so marker-only drift classifies as `daemon_control_runtime_security_invalid`, not the generic `daemon_start_failed`. Pinned by extending `classify_daemon_exit_distinguishes_security_failures` (healthy → marker-only loosen → restore → leaf loosen).
- V3 (PLAUSIBLE): `broadened_directory_fails_closed` is now environment-independent — it secures the directory to exactly 3 ACEs first, then grants Everyone (deterministic stage 7 on any machine, including CI runners whose temp tree is already restricted).
- V4 (PR-introduced): the second `GetTokenInformation` SAFETY comment corrected — the returned SID lives inline at the end of the caller buffer (the guard must own both buffer and token handle); the old text claimed token-owned storage.
- V5 (pre-existing): the Windows trusted-start branch now holds the writer in `Option<WriterWorker>` and joins it through `spawn_blocking` on EVERY failure path (listener create, descriptor publish, phase failure, registry new/register, supervisor start/handle) — previously the listener/publish `?` returns dropped the writer, whose `Drop` only try_sends and detaches, stranding lifecycle locks until process exit. `control_token()` was hoisted before the writer spawn so the fallible step sits outside the writer-ownership window.
- V6 (PR-introduced): ADR 025's consequence bullet no longer claims the §9 p95 < 500 ms budget is met; it records the return to the machine-specific baseline and points at the rewritten §9 deviation note.
- V7: the two phase blocks collapsed into a shared `verify_control_phase` helper (verify → on failure join writer + fail closed); rationale text lives once on the helper and is cross-referenced, not copied.
- V8: `DaemonPaths` verification surface consolidated — one private `verify_control_acls(&[&Path])` helper; `verify_control_security` composes pre-publish + artifact surfaces instead of repeating a fifth path array; docs corrected (tests are the callers). New `verify_control_surviving_acls` (CLI classify surface) and `verify_control_descriptor_acl` (phase 2: descriptor only — the writer lock was already verified by phase 1, closing V10's redundant re-read).
- V9: one shared `vibemux_platform::test_helpers::{loosen_with_icacls, restore_with_icacls}` (behind a `test_helpers` feature enabled by consumer dev-dependencies) replaces the six hand-spelled icacls recipes across platform/daemon/CLI tests; the no-fallback rationale lives once in the helper.
- V11: the use-after-free churn pin runs 4×200 rounds instead of 4×1500 (red run fired at round 0, so the regression pin keeps a ~200× margin at ~7.5× less CI time).
- V13: new `vibemux_platform/tests/unsafe_containment.rs` mechanically enforces the ADR 025 containment: crate root keeps exactly one `#![deny(unsafe_code)]`, `#![allow(unsafe_code)]` appears exactly once workspace-wide inside `windows_acl_native.rs`, and every other workspace library crate keeps exactly one `#![forbid(unsafe_code)]`.

**Validation**
- Windows gates on this change: `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean; `cargo test --workspace --all-features` — 56 suites (55 + the new `unsafe_containment` integration target), **319 passed / 0 failed / 2 ignored** (+4 over Stage 2). All new production code paths are `#[cfg(windows)]`-gated; the unix compile surface is unchanged.
- Real-machine smoke (release binaries): healthy start/health/stop on a fresh scratch project; marker-only loosen (`icacls /grant *S-1-1-0:F`) → CLI classify reports `daemon_control_runtime_security_invalid` (V2 evidence); marker restored, NULL DACL applied via `SetNamedSecurityInfoW` on the runtime leaf → daemon refuses WITHOUT a crash (V1 evidence) and a later healthy start/stop confirms recovery; smoke state cleaned.

### 2026-09-26 - Nord and Gruvbox themes across GUI, TUI, Python CLI, and terminal backends (ADR 026)

**Change**
- The egui GUI (`ThemeId`) gains `nord` and `gruvbox` as full persisted hex palettes (background ramp, text pair, accent pair, success/warning/danger, terminal colors) following the existing palette invariants; the cycle order becomes `claude -> github -> vscode -> nord -> gruvbox -> claude`, and the default stays `claude`. Adding serde enum variants keeps old persisted configs loading (unknown names fall back to defaults).
- `config/theme_palettes.json` (schema_version 1) now exports all five GUI palettes; the new `vibemux_frontend` `palette_parity` integration test pins the file to the serialized `all_palettes()` output, so the Rust palettes stay the single canonical source and the file is the shared consumer boundary.
- The Ratatui TUI `Theme` gains `nord` and `gruvbox` expressed as xterm-256 `Color::Indexed` values (chosen over `Rgb` so WezTerm on Windows and tmux on POSIX render the same hues without a truecolor dependency). Nearest-index mapping was audit-driven: the nearest index to Nord's brand red measures 4.64:1 against black — inside the gate but too close to trust — so the failure accent uses the brighter index 167. The TUI WCAG AA audit test now resolves `Color::Indexed` through the standard xterm-256 table (base ramp, 6x6x6 cube, grayscale) and covers every new accent pair; the `--theme` usage hint is derived from `Theme::ALL` instead of a hardcoded list; four new golden fixtures (nord/gruvbox at 80x24 and 120x32) pin the layouts, and the eight existing fixtures are byte-identical.
- The Python prototype gains `src/vibemux/theme.py` (loads and validates the exported palette file, resolves `VIBEMUX_THEME` with a stderr warning and fallback to `claude` for unknown values, degrades to built-in default colors when the file is unreadable, maps roles onto rich styles) and `src/vibemux/terminal_theme.py` (renders a palette as a WezTerm `config.colors` lua table or a tmux `set -g` option block using nearest xterm-256 `colour<N>` values). `vibemux theme --list` and `vibemux theme --backend wezterm|tmux --theme <name>` print to stdout; generation only — VibeMux never writes terminal configuration or mutates a running terminal. Human-facing CLI output (tables, error lines, init/switch/stop) is role-styled; `--json` outputs stay theme-independent.

**Validation**
- `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean; `cargo test --workspace --all-features` — **324 passed / 0 failed / 2 ignored**. (One run showed 2 load-flake failures in `vibemux_cli` recovery tests; both pass in isolation and in the re-run, consistent with the known issue #6 load sensitivity.)
- Python gates: `python -m ruff format --check .` clean; `python -m ruff check .` clean; `python -m mypy .` — 20 errors, identical to the pre-change baseline (tools/a2a_* optional-stub gaps and one pre-existing `tests/test_harness.py` finding; no new findings); `python -m pytest` — **61 passed / 0 failed** (14 new theme tests).
- Live Windows render: `vibemux_frontend` built with `--features gui_screenshot` and rendered via `EFRAME_SCREENSHOT_TO` with a scratch `APPDATA` config per theme — nord and gruvbox screenshots verified visually (correct ramps, accents, picker state, no layout drift) against a claude baseline; scratch configs and screenshots deleted afterwards. TUI live ANSI emission for the new themes is covered by the audit test's index assertions and the `--theme nord|gruvbox --json` integration runs; no separate manual ANSI capture was recorded for this change.
- tmux snippets are generated and pinned by unit tests but not live-applied on POSIX in this change (Windows-first repository); the WezTerm snippet uses hex values directly and was not loaded into a user WezTerm config (generation-only by design).

**Docs**: ADR 026 records the decision; README frontend/theme section, `docs/architecture.md` frontend boundary, and CHANGELOG updated in the same change.

### 2026-09-26 (2) - GitHub-Primer sidebar shell GUI redesign (ADR 027)

**Change**
- The egui GUI is redesigned around a persistent left sidebar (ADR 027): brand, Overview entry, ten harness rows with live status dots and `Ctrl+<digit>` accelerator hints, and a pinned footer (theme ComboBox, Settings/Diagnostics toggles, shortcut hint). The main area switches between an Overview dashboard (page header with count badge, four bordered summary cards, agent table in a bordered box with hairline separators and bordered state pills) and a Seat page (header with state pill, bordered transcript card, Claude-style rounded composer, right inspector), with a thin probe/health status bar spanning the bottom. The `topbar` module (theme chips, hardcoded workspace label) is deleted.
- The `github` palette is now the exact GitHub Primer dark token set (`#0d1117`/`#161b22`/`#21262d`/`#30363d`, `#e6edf3`/`#8b949e`, `#2f81f7` accent, `#3fb950`/`#d29922`/`#f85149` semantics, `#010409` terminal canvas), and the **default theme changes from `claude` to `github`** so the stock look matches the shell (persisted user configs unaffected; `config/theme_palettes.json` updated in the same commit under the `palette_parity` pin). The shared palette color struct gained the mid-surface and danger colors.
- Preserved contracts: Ctrl+1..0 opens all ten seats (issue #5), Escape closes overlays before leaving a seat, Ctrl+T cycles themes, Ctrl+,/Ctrl+; toggle overlays, theme persistence via debounced `UserConfig` writes, `handle_stub_send`/`harness_terminal` semantics, stub-only transcripts, no PTY attach.

**Validation**
- `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean; `cargo test --workspace --all-features` — **324 passed / 0 failed / 2 ignored** (headless render guards rewritten to mirror the new shell: sidebar+overview and sidebar+seat; all keyboard-accelerator, theme-persistence, and stub-send tests pass unchanged).
- Live Windows render: `gui_screenshot` builds rendered github/nord/gruvbox/claude/vscode at 1280x800 via a scratch `APPDATA`; github and nord screenshots verified visually (Primer tokens, sidebar active state, footer picker, cards, pills, status bar). Three rendering defects found and fixed during screenshot review: the sidebar footer content clipped under a content-sized nested bottom panel (pinned `min_height`), the Diagnostics footer button label clipped because the second half-width was computed from the already-shrunk available width (widths captured up front), and long version strings collided with the route label in agent rows (single truncated `route · version` string). Scratch configs and screenshots deleted afterwards.

**Docs**: ADR 027 records the redesign; ADR 026's default-theme sentence is amended; README, `docs/architecture.md`, and CHANGELOG updated in the same change.

### 2026-09-27 - Supervisor Chat workspace and explicit terminal observation (ADR 028)

**Implemented**
- Replaced the runtime dashboard/fixture seats with a single Supervisor Chat home, continuous task activity, project/task sidebar, docked details and independent native task windows. The final user-requested visual revision introduces Studio (light canvas, restrained green, Segoe UI/CJK fonts); all five existing dark palettes remain supported. Existing appearance preferences are preserved. Old dirty GUI source files were preserved but are no longer imported by the normal runtime.
- Added the production `LiveApp`/`FrontendClient` bridge: authenticated Control v4 task/run/event/artifact reads, bounded pagination and action queues, daemon-generation invalidation, explicit disconnected/capability states, and no simulated send/reply. The primary composer retains a local draft while continuous coordinator chat is unavailable.
- Added optional versioned terminal observation payloads and a standalone Rust WezTerm observer; daemon request routing checks effective grants, session, request/correlation identity, deadlines and bounded responses. Explicit Run-scoped links are observation-only. Focus revalidates project, workspace, plugin and native endpoint generation; closing a task window never cancels execution.
- Control v1-v3 operations remain compatible, store schema stays v3, and plugin envelope stays v1.0. Startup configuration v2 adds explicit terminal grants; v1 keeps no grants. Terminal setup and failure codes are documented in `docs/terminal_observer_protocol.md`.

**Validation executed**
- `cargo fmt --all -- --check`: pass. `cargo clippy --workspace --all-targets --all-features -- -D warnings`: pass.
- `cargo test --workspace --all-features -j 2 --target-dir target/supervisor_delivery -- --test-threads=1`: **340 passed / 0 failed / 2 ignored**, 63 groups. This run used `CARGO_PROFILE_TEST_DEBUG=0`, `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_INCREMENTAL=0` after an initial debug build exhausted disk space and hit MSVC PDB LIMIT. Only task-created duplicate build caches were removed.
- Ordinary parallel workspace tests hit two pre-existing CLI recovery failures while another test loosened the shared ACL marker. Source inspection confirmed the cross-test interference; the final serial run preserves every assertion. Parallel Windows suite readiness remains unverified.
- Python project interpreter: `python -m pytest` 61 passed; Ruff lint and format checks passed; `python -m mypy src/vibemux` passed (17 files). Full-tree `mypy .` still reports optional A2A dependency/stub gaps and an existing test typing error. `cargo nextest`, `cargo deny`, `cargo audit` are unavailable.
- Native egui preview: 54 captures across six themes, three logical sizes (960x600/1280x800/1920x1080) and 100/150/200 percent renderer scaling, plus independent task/terminal and disconnected views. Representative frames visually inspected; five final DEMO screenshots retained under `docs/frontend_previews`. IME shortcut interception and Unicode layout have automated coverage; real IME candidate windows and physical monitor DPI transitions remain manual gates.

**Limits**
- No WezTerm installation/running GUI was found in inspected paths/processes, so real pane focus/native TUI integration is not live-verified. No software or global terminal configuration was installed/changed.
- Continuous coordinator chat, automatic coding-harness dispatch and embedded terminal rendering remain separate future capabilities. No model inference, deployment, remote A2A or hosted CI was performed.
- Full evidence and screenshots: `docs/evidence/supervisor_frontend_validation.md`. Existing uncommitted theme/CLI changes and `.zcode` were preserved; this task did not commit or push them.

### 2026-09-27 (2) - Branch audit and open-work integration (`feat/integrate-open-work`)

**Branch dispositions** (checked against `origin/main` at `b577c11`)
- Fully contained in `origin/main` (0 commits ahead, nothing to merge): `a2a_basic_share`, `codex/supervisor_chat_ui`, `codex/supervisor_state_api`, `daemon_local_ipc`, `daemon_process_cli`, `daemon_stale_recovery`, `daemon_windows_security`, `daemon_writer_worker`, `feat/gui-theme-nord-gruvbox`, `integration_core`, `main` (local ref is 34 behind `origin/main`), `plugin_protocol_core`, `plugin_supervisor_core`, `probe_unified_frontend`, `python_reference_p0`, `python_source_reference`, `rust_core_refactor`, `rust_store_daemon`, and every `origin/*`/`linux/*` remote branch except the one below.
- `feat/scheduling-and-visual-optimizations`: its PR (#3) was closed as superseded after head `4d96fc2` landed on main as a content-equivalent squash. The only unlanded commit, `a698f64` (architecture docs consolidation), is ported here as a curated merge: newer main content it would have dropped (CLAUDE.md command reference, README supervisor workflow commands, Control v4/terminal observer/Supervisor Chat text) is kept, and the architecture reference is brought up to date. A stray conflict marker committed to this file on main is removed.
- `codex/harness_request_dispatch` and `codex/harness_request_dispatch_latest` (same commit `a955484`, 40 behind): **not merged at the time** (merged later; see the 2026-10-17 entry, ADR 040/041). A trial merge conflicted in 12 files (including add/add on `crates/vibemux_harness`), the branch puts process/session I/O inside `vibemux_harness` against the "pure logic, no I/O" boundary, and main's issue #4 harness port supersedes its registry. Its uncommitted worktree ADR draft also reuses number 025. Harness dispatch/capture should be re-ported onto the daemon-owned architecture as a separate task.
- A detached Codex worktree held uncommitted "Document role" headers in five docs; they are superseded by the `a698f64` cross-references and not included.

**Integrated in this branch**
- The previously uncommitted work-area changes: ADR 026 Python theme CLI and terminal snippets, the Control v4 task queries plus the bounded WezTerm terminal observer (ADR 028), the Supervisor Chat frontend with Nord/Gruvbox/Studio palettes (ADR 026-028), their ledger entries, and the docs consolidation above. No behavior was changed while splitting them into commits.

**Validation executed on the integrated tree**
- `cargo fmt --all -- --check`: pass. `cargo clippy --workspace --all-targets --all-features -j 2 -- -D warnings`: pass.
- `cargo test --workspace --all-features -j 2 -- --test-threads=1` (debug info and incremental builds disabled because of disk space): **340 passed / 0 failed / 2 ignored**.
- Project interpreter: `python -m pytest` 61 passed; `ruff check` and `ruff format --check` clean; `mypy src/vibemux` clean (17 files); `scripts/smoke_test.py` PASS (mock workflow).
- Relative links and anchors in AGENTS.md, CLAUDE.md, PROGRESS.md, README.md and `docs/architecture.md`: 0 broken.

**Known follow-ups**
- `crates/vibemux_frontend/src/gui/{app,overview,workbench,stub_bank}.rs` are no longer in the module tree; they reference only each other and their tests do not compile. Delete them or bring them back in a dedicated change.
- Port harness request dispatch/capture from `a955484` onto `vibemuxd`-owned I/O.
- Stale local branches and worktrees listed above can be pruned after this merges. That needs a cleanup plan and was not done here.
- The dependency diagrams in AGENTS.md §4.2 and CLAUDE.md still disagree on where `harness`/`probe` sit.
- Parallel Windows test runs remain sensitive to shared ACL-marker interference (issue #6); WezTerm focus is still not live-verified.

### 2026-09-27 (3) - Daemon-owned harness dispatch port (ADR 029, `feat/harness_dispatch_port`)

**Status change**
- Structured vendor harness adapters (§4.1) and M6: `PLANNED` → `PARTIAL` (fixture evidence only).
- ADR 029: Proposed → Accepted for pre-alpha implementation. The `windows_job_object` `unsafe` module still needs reviewer approval on the pull request (AGENTS.md §7.2).

**Implemented** (the capability of `a955484` re-ported onto the daemon-owned architecture; the codex branches and their worktrees are unchanged)
- Stage 1 (`abee927`, `9b910e2`): pure `vibemux_harness::dispatch`. Request and route-config validation, the launch argv builder, Codex `exec --json`/`app-server`, Claude stream-json, and ACP session machines, the incremental JSON-line framer, the capture budget, the attempt phase machine with its Task/Run mapping, `decide_outcome`, and content-free event drafts. ACP routes are probe-only.
- Stage 2 (`0f882f4`, `cb63015`): `vibemux_platform::ProcessTree`. A kill-on-close Windows Job Object that refuses breakaway (the second reviewed `unsafe` module, pinned by `tests/unsafe_containment.rs`) and a POSIX process group in safe code.
- Stage 3 (`4951001`, `1b2f55b`): store schema 4 `harness_dispatches`. Idempotent admission with a store-derived fingerprint, one active reservation per project, a single-use fenced claim, fenced finish and cancel, and startup recovery with per-attempt quarantine. The generic projection commit and A2A binding refuse dispatch-owned Tasks and Runs.
- Stage 4 (`e62d867`): the `vibemux_launch_trampoline` binary; the `vibemuxd` dispatch service and executor with a bounded stdin queue, deadlines, cancellation, and a shutdown drain; the config loader with the executable trust rules; a Git `HEAD` reader; bounded in-memory transcripts; and recovery before the writer reports ready.
- Stage 5 (`e7e26d9`): Control IPC v5 with `harness_dispatch_{catalog, probe, submit, status, output, cancel}`, paged byte-exact output with `OutputReassembler`, the 60 s probe cap, the shutdown signal, and `vibemuxctl dispatch`.
- Stage 6 (this change): ADR 029 accepted; `docs/architecture.md`, `docs/protocol_boundaries.md`, `docs/platform_support.md`, README, CLAUDE.md, CHANGELOG, the AGENTS.md §3.2 exception, ADR 025's status line, and this ledger updated.

**Evidence**
- tests (native Windows, target directory outside the repository): `cargo test --workspace --all-features -- --test-threads=1`: **488 passed / 0 failed / 2 ignored** in 74 test binaries, on the Stage 5 code (Stage 6 changes documentation only). Dispatch binaries: `vibemux_harness` `dispatch_protocols` 14, `dispatch_framing` 8, `dispatch_config` 16, `dispatch_attempt` 12; `vibemux_platform` `process_tree` 7, `unsafe_containment` 3; `vibemux_store` `harness_dispatch` 18; `vibemuxd` `launch_trampoline` 6, `harness_dispatch` 23, `control_harness_dispatch` 3; plus unit tests.
- tests (Linux container `rust:1.85-slim-bookworm`, read-only source mount, at `e7e26d9`): `cargo test -p vibemux_harness -p vibemuxd -p vibemux_cli -p vibemux_platform --all-features --no-fail-fast`: **237 passed / 0 failed** in 33 test binaries.
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --all-features -- -D warnings`: pass.
- Python, project interpreter with the worktree sources: `python -m pytest` 63 passed; `ruff check` and `ruff format --check` clean; `mypy src/vibemux` clean (17 files); `scripts/smoke_test.py` PASS.
- Relative links and anchors in the ten edited documents: 80 checked, 0 broken.
- CLI smoke (real `vibemuxd`/`vibemuxctl`, scratch project, no vendor CLI): without a config, `dispatch catalog` returns `harness_dispatch_unconfigured`; an unknown request returns `harness_dispatch_not_found`; a blank prompt returns `harness_dispatch_invalid_prompt`; an unreadable prompt file returns `cli_prompt_unavailable`; an unknown harness returns `store_unknown_harness` with exit 2; health reports `store_schema_version: 4`; the daemon stops cleanly.
- live validation: **none**. No vendor CLI was launched. Every process test uses the first-party `vibemux_native_fixture` binary.
- commit/PR: branch `feat/harness_dispatch_port`, based on the PR #9 line (merge base with `origin/main` `b577c11`); not merged.

**Remaining**
- Stage 7: live native Windows `dispatch probe` against the installed CLIs (initialize only). A real `submit` needs explicit authorization of paid inference. Open decisions 3 (Claude `--strict-mcp-config`/`--restricted` with `--bare` stream-json) and 4 (forced exit after a terminal record) wait on that evidence.
- Rebase onto `main` after PR #9 lands. `main` now has `5873573` (generic Runs refused under A2A-owned Tasks), which touches the same projection-guard area as Stage 3.
- Reviewer approval of the `windows_job_object` `unsafe` module.
- ACP execution needs a verified per-vendor deny posture and a new decision. Writable dispatch, owned per-Run worktrees, and more than one dispatch per project need a separate ADR. Durable transcript artifacts, a Control operation for the recovery report, the frontend Send action, and A2A exposure are out of scope.

**Compatibility / migration**
- Store schema 3 → 4 is additive, atomic, and forward-only. A schema-3 binary cannot open a schema-4 store; to downgrade, restore a backup taken while the daemon was stopped.
- Control v5 only adds operations. A pre-v5 `vibemuxctl` refuses a v5 descriptor, so upgrade `vibemuxctl` and `vibemuxd` together and install `vibemux_launch_trampoline` next to `vibemuxd`.
- Rollback without a downgrade: remove `.vibemux/harness_dispatch.json` and restart the daemon; every dispatch operation then returns `harness_dispatch_unconfigured` and nothing is spawned.
- Plugin wire v1.0 and the Python reference are unchanged.

**Known risks**
- Read-only protocol profiles are not an OS sandbox; a vendor defect or a changed flag meaning could still write in the project root.
- A crash during `running` keeps the project's reservation until an operator cancels the attempt. A quarantined row blocks dispatch until a backup is restored; there is no repair tool.
- POSIX containment does not survive a daemon crash, and a member can leave its group with `setsid`. Windows containment does not cover processes that system services start on a member's behalf.
- Vendor protocol drift is caught only by fixture replay until Stage 7 records live evidence.
- The Windows evidence above is serial; parallel Windows runs remain sensitive to issue #6.

### 2026-09-27 (4) - Harness dispatch Stage 7 live probe evidence (ADR 029, `feat/harness_dispatch_port`)

**Status change**
- Structured vendor harness adapters (§4.1) and M6 stay `PARTIAL`. The evidence now includes live initialize-only probes on native Windows; no live execution turn has run.
- ADR 029 open decision 3 (Claude flags) is resolved for flag parsing. Open decision 4 (forced exit after a terminal record) stays open, now with live evidence.

**Implemented**
- [docs/evidence/harness_dispatch_validation.md](docs/evidence/harness_dispatch_validation.md) records the Stage 7 probes. ADR 029, README, CLAUDE.md, CHANGELOG, `docs/architecture.md`, `docs/protocol_boundaries.md`, `docs/platform_support.md`, and this ledger now say that live validation covers initialize-only probes. No code changed.

**Evidence** (native Windows 11 Pro 10.0.26200, debug binaries built from `4302c13`, a scratch project, every route `allow_execution: false`)
- live validation: `vibemuxctl dispatch probe` against Codex 0.157.1 (`codex_app_server`), Claude Code 2.1.283 (`claude_stream_json`), and three `acp` routes: OpenCode 1.18.32, GitHub Copilot CLI 1.0.87-0, and Grok 1.0.41. Initialize only; no prompt and no inference.
- Default limits: Codex, Claude, OpenCode, and Copilot are `probed` with exit code 0 and no forced termination. Grok is `failed` with `harness_process_failed`, exit code 1, and forced termination in 3 of 3 probes.
- A temporary replay outside the daemon showed that Grok answers initialize after about 0.3 s and exits with code 0 2.12 to 2.14 s after stdin closes, past the 2 s default grace. With `limits.shutdown_grace_ms` at 5,000, all five probes are `probed` with exit code 0 and no forced termination.
- Claude accepted the full route argv (`--bare` stream-json with `--strict-mcp-config`, `--restricted`, `--tools=`, `--permission-mode dontAsk`, and `--no-session-persistence`) and exited on its own.
- The 12 daemon probes left no `harness_dispatches` row, and no vendor process launched with a probe argv was left running.
- tests: not rerun, because this change is documentation only. Relative links and anchors in the eleven edited documents: 93 checked, 0 broken.
- commit/PR: branch `feat/harness_dispatch_port`; not merged.

**Remaining**
- A live `submit` (Codex and Claude execution, `output`, and `cancel`) needs explicit authorization of paid inference.
- Decide open decision 4: a larger default grace, a per-route grace, or a narrower rule for a forced kill after a correlated terminal record.
- Verify which forwarded environment names each vendor needs to authenticate during a real turn.
- As in entry (3): rebase onto `main` after PR #9 lands, and reviewer approval of the `windows_job_object` `unsafe` module.

**Known risks**
- The live evidence covers one host and the listed vendor versions. Protocol drift in later vendor versions shows only when the probes are run again.
- Until decision 4 is settled, a vendor that exits more slowly than the grace makes a probe or a turn report `failed` although it ended cleanly.

### 2026-09-27 (5) - Merge `main` into the harness dispatch branch (`feat/harness_dispatch_port`)

**Status change**
- None. The dispatch status from entries (3) and (4) is unchanged.

**Implemented**
- Merged `origin/main` at `5873573` (generic Runs refused under A2A-owned Tasks) into `feat/harness_dispatch_port` in `e9f1e0d`. The branch is published, so it takes a merge instead of the rebase that entries (3) and (4) planned.
- One conflict, in `SqliteStore::commit_projection` (`crates/vibemux_store/src/lib.rs`): both sides had moved the ownership checks into the Immediate write transaction. The resolution keeps both rules in order. First `main`'s A2A rule: a Task or Run bound in `a2a_runs`, or a new Run under an A2A-owned Task, is refused with `A2aBoundProjection`. Then the Stage 3 dispatch rule: a Task or Run owned by a harness dispatch, or a new Run under a dispatch-owned Task, is refused with `HarnessDispatchBoundProjection`. `crates/vibemux_store/src/a2a.rs` merged cleanly and keeps both new tests.

**Evidence**
- tests (native Windows, serial): `cargo test --workspace --all-features --no-fail-fast -- --test-threads=1`: **489 passed / 0 failed / 2 ignored** in 74 test binaries (the 488 of entry (3) plus `main`'s new store test).
- tests (Linux container `rust:1.85-slim-bookworm`, read-only source mount): `cargo test -p vibemux_store -p vibemux_harness -p vibemuxd -p vibemux_cli -p vibemux_platform --all-features --no-fail-fast`: **290 passed / 0 failed** in 36 test binaries.
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --all-features -- -D warnings`: pass.
- live validation: not applicable; no dispatch behavior changed.
- commit/PR: branch `feat/harness_dispatch_port`; PR #9 is still open, and this branch has no pull request yet.

**Remaining**
- Merge PR #9, then open this branch's pull request into `main`. Reviewer approval of the `windows_job_object` `unsafe` module and the Stage 7 items from entry (4) still apply.

### 2026-09-27 (6) - PR #9 merged; harness dispatch pull request (`feat/harness_dispatch_port`)

**Status change**
- PR #9 (`feat/integrate-open-work`) merged into `main` as the merge commit `6e6df7f`. The ADR 029 dispatch status from entries (3) to (5) is unchanged; it reaches `main` only when this branch's pull request merges.

**Implemented**
- Merged `origin/main` at `6e6df7f` into `feat/harness_dispatch_port` in `4d74a24`. Both parents of `6e6df7f` were already in the branch, so the tree is unchanged; the merge leaves one merge base with `main`, so the pull request shows only the ADR 029 work (71 files).
- This branch's pull request into `main` is opened with this change.

**Evidence**
- tests: not rerun. The tree of `4d74a24` is identical to the tree tested in entry (5); this change edits only this ledger and the ADR 029 plan line.
- commit/PR: PR #9 merged; this branch's pull request is open.

**Remaining**
- Review of the pull request, including reviewer approval of the `windows_job_object` `unsafe` module (AGENTS.md §7.2). The Stage 7 items from entry (4) still apply.

### 2026-09-28 (1) - Harness dispatch merged into `main` (ADR 029, PR #10)

**Status change**
- ADR 029 harness dispatch (stages 1 to 6, Stage 7 partial) is on `main` through PR #10, merge commit `273a176`. The feature stays `PARTIAL`; its evidence and gates are unchanged from the 2026-09-27 entries (3) to (6).
- The repository owner approved the Stage 2 `windows_job_object` `unsafe` module when merging PR #10, which meets the reviewer-approval requirement of AGENTS.md §7.2.

**Implemented**
- ADR 029's status line and implementation plan record the merge. No code changed.

**Evidence**
- CI on the PR head `123941c`: `python` (Ubuntu, Windows) and `rust (ubuntu-latest)` passed. The first attempts of both `rust (windows-latest)` jobs failed on runner timing, not on dispatch logic. In one, three `vibemuxd` writer tests hit the fixed 5 s writer startup bound during a runner I/O stall. In the other, `terminal_observer`'s `native_surface_observation_never_changes_execution_and_requires_identity` lost its plugin session (`terminal_plugin_unavailable`). A rerun of both jobs passed, and PR #10 merged with all eight checks green.
- tests: not run for this change; it edits only docs.
- commit/PR: PR #10 merged as `273a176`; this docs change is its own pull request.

**Remaining**
- The writer's startup wait shares the 5 s per-request response timeout, and the CLI's `daemon start` wait is also 5 s, so Windows CI can fail when the runner stalls on I/O. Hardening these bounds is a separate follow-up.
- ADR 029 open decision 4 (forced exit after a terminal record), a live `dispatch submit` (needs explicit authorization of paid inference), and the other Stage 7 items from the 2026-09-27 entry (4).

### 2026-09-28 (2) - Separate writer startup bound and derived `daemon start` wait (`fix/daemon_startup_deadline`)

**Status change**
- None. M3 stays `VERIFIED`. This change hardens the two startup bounds and the test runtime behind the Windows CI failures recorded in the 2026-09-28 (1) entry.

**Root cause**
- `WriterWorker` waited for writer readiness (store open, migrations, harness dispatch recovery) with the 5 s per-request `DEFAULT_RESPONSE_TIMEOUT`. On expiry it still joined the writer thread, so the bound never capped wall time. It only turned a slow but successful start into `writer_response_timeout`. Local reproduction: with a 1 ns request deadline, a fresh start that took 17 ms failed with `ResponseTimeout`, and the store reopened cleanly afterwards.
- `vibemuxctl daemon start` waited 5 s for health and then terminated the daemon, so it could kill a daemon that was still starting.
- `terminal_observer`'s `native_surface_observation_never_changes_execution_and_requires_identity` ran on a current_thread tokio runtime and made blocking writer calls while a plugin session was live. A 6 s blocking stall inserted after the registry became active reproduced the CI failure (`terminal_plugin_unavailable` at the first `inspect`). The same stall on a multi-thread runtime passed.

**Implemented**
- `vibemuxd`: `DEFAULT_WRITER_STARTUP_TIMEOUT` (20 s) bounds readiness, and `response_timeout` is now per-request only. Expiry returns the new `WriterError::StartupTimeout` (`writer_startup_timeout`) and still joins the writer thread, so the lifecycle locks are released before the error returns. Public constructor signatures are unchanged. The harness dispatch executor's retry classification is unchanged, because startup errors never reach it.
- `vibemux_cli`: `DEFAULT_STARTUP_TIMEOUT` is the writer startup bound plus a 10 s launch margin (30 s). A compile-time assertion keeps it longer than the writer bound. The Windows launcher helper's wait for the daemon PID follows it (`max(startup_timeout, 3 s)`). Early daemon exits are still reported as soon as they are observed. The one-time Windows ACL helper runs in `ensure_runtime_dir` before the spawn, outside this window.
- `terminal_observer`: that test now uses `multi_thread, worker_threads = 2`, matching the daemon's runtime and every test in `plugin_registry.rs`. No assertion changed.
- ADR 018 amendment (2026-09-28) records the relation between the two bounds.

**Evidence**
- New tests in `vibemuxd`: `startup_readiness_does_not_use_the_request_deadline` (a 1 ns request deadline; the start succeeds, shutdown joins, and the lock is released) and `expired_startup_bound_reports_startup_timeout_and_releases_lock` (a 1 ns startup bound; `writer_startup_timeout`, the lock is released, and a restart succeeds). The first reproduces the pre-fix failure mode directly.
- The two reproductions above used temporary diagnostic code that is not part of this change.
- commands (native Windows): `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean; `cargo test --workspace --all-features` (parallel, as in CI) exit 0, 491 passed, 0 failed, 2 ignored.

**Observation (not changed)**
- The plugin registry's session monitor uses a `biased` `tokio::select!` that polls the heartbeat deadline before buffered frames. After any runtime stall longer than the heartbeat receive deadline, it ends the session even when heartbeats are already queued. The daemon's multi-thread runtime makes such a stall unlikely. Changing the select order is out of scope here.

**Remaining**
- Windows CI evidence for this change comes from its pull request run.
- ADR 029 open decision 4 and a live `dispatch submit`, unchanged from the 2026-09-28 (1) entry.

---

### 2026-10-06 - DSH prerequisite provisioning and live runtime re-verification

**Scope and resulting contracts**
- Add the DeepSeek Harness runtime to the repository's prerequisite path: a checkable, provisionable setup step for the npm-global `@deepseek-ai/dsh` runtime, the `sdk` profile the ADR 032 adapter drives, and the optional out-of-tree `tui` profile.
- Re-verify the ADR 032 wire binding against the current vendor release, because `docs/dsh_session_protocol.md` bound the adapter to shapes read from `0.1.2-rc.1` and required re-verification before a later release is claimed to work.
- No canonical state, store schema, control protocol, event, or plugin contract changed, and no Rust source changed.

**Implemented**
- `scripts/setup_dsh_prereq.ps1`: read-only by default (exit 0 ready, 2 not ready, 1 unusable); `-Apply` performs only the documented operator commands (`npm install -g @deepseek-ai/dsh@<version>`, `dsh --profile sdk --help`, `dsh plugin --profile tui add <bundle>`); `-Json` emits one machine-readable object; `-SkipTui` limits the check to the `sdk` profile; `-WriteSessionConfig` writes `.vibemux\dsh_session.json` as UTF-8 without a BOM, refuses to replace it without `-Force`, and refuses a launcher path that resolves inside the project root; `-SkipProbes` runs the structural checks only. Tools are launched as `node <javascript entry>`, never through a command interpreter; the script prints no secret and no environment dump and deletes nothing.
- The launcher-pair trust rule is mirrored rather than assumed: the installed package must declare `"name": "@deepseek-ai/dsh"` and map its own `dsh` bin entry to the script that would be launched.
- `tests/test_dsh_prereq.py`: 17 fixture-driven tests (skipped with an explicit reason when `pwsh` is absent) covering the missing install, the version floor with prerelease ordering, a foreign package with the same layout, the ready path, a `tui` profile without its bundle, `-SkipTui`, the read-only guarantee, `-Apply -WriteSessionConfig` (BOM-free write, no clobber without `-Force`), and the `-Apply`/`-SkipProbes` refusal.
- The prerequisite is documented in README's DSH section and as CONTRIBUTING step 7.
- `crates/vibemux_harness/src/dispatch/launch_spec.rs`: `interpreter_script_path` restores the classic spelling of a Windows verbatim path for the launcher-pair script argument only (`\\?\C:\x` → `C:\x`, `\\?\UNC\s\share\x` → `\\s\share\x`; a volume GUID path is left unchanged so it fails loudly). The admitted canonical path stays the trust anchor, and the image argument is untouched because `CreateProcess` accepts the verbatim form.

**Evidence**
- commands (native Windows, PowerShell 7.6.6, Node v24.18.0): `python -m pytest` 128 passed; `python -m ruff format --check .` clean (109 files); `python -m ruff check .` clean; `mypy tests/test_dsh_prereq.py` clean; repository-wide `python -m mypy .` still reports the same 20 errors in three untouched `tools/` modules.
- `scripts\setup_dsh_prereq.ps1` on the reference host: `READY`, exit 0, ten checks (node, npm, pnpm, DSH install, DSH version, `sdk` profile, `tui` profile, `tui` composition, session configuration, API-key presence).
- Live wire re-verification against `@deepseek-ai/dsh` `0.2.0-rc.2`, initialize-only with no prompt on any path: `initialize` returned `serverInfo.name == "deepseek-harness-sdk-runtime"` and `serverInfo.version == "0.0.1"`, `shutdown` returned `{}`, the runtime exited 0 with empty stderr, and five runs — including the adapter's environment allowlist, its creation posture, and a verbatim `cwd` — produced byte-identical output. No difference from the documented shapes. Full record: [docs/evidence/dsh_runtime_validation.md](docs/evidence/dsh_runtime_validation.md).
- First live exercise of the adapter itself (`vibemux_dsh_tui` in a real ConPTY, and the Control v7 path through `vibemuxd`) failed before the handshake completed: `dsh_driver_transport_closed` (exit 1) and `dsh_session_refused` (exit 4). `--print-config` still returns the documented content-free projection with exit 0.
- `cargo test -p vibemux_harness -p vibemux_dsh --all-features`: every suite green, including three new unit tests for the path transformation and one Windows-gated integration test proving that a route whose `entry_script` is `\\?\C:\tools\dsh-package\lib\bin.js` yields the argv `["C:\\tools\\dsh-package\\lib\\bin.js", "--profile", "sdk"]` with no `\\?\` prefix. `cargo clippy -p vibemux_harness -p vibemux_dsh --all-targets --all-features -- -D warnings` clean.
- Live re-probe after the repair (rebuilt `vibemux_dsh_tui`, node-pty 120x32, real `dsh` 0.2.0-rc.2): the surface reaches `phase ready` with `1 handshake ok: deepseek-harness-sdk-runtime 0.0.1`, renders its status and record panels, and `Ctrl-Q` exits `0` after 21.1 s. The same probe exited `1` after ~1.16 s with no status line before the repair. This is the first live DSH session this adapter has completed.

**Defect found and repaired**
- `crates/vibemux_dsh/src/config.rs` (`canonical_outside_root`) stores the entry script in `std::fs::canonicalize` form — on Windows the verbatim `\\?\C:\...` form — and `crates/vibemux_harness/src/dispatch/launch_spec.rs` passed that string as Node's first argument. Node's loader refuses a verbatim script path (`EISDIR: illegal operation on a directory, lstat 'C:'`) and exits 1 before loading anything, and because the driver discards the child's stderr the failure surfaced as a closed transport. Reproduced directly: `node <plain path>` runs the script, `node \\?\<same path>` fails.
- The defect was version-independent — no DSH code ran — so it was not caused by the `0.2.0-rc.2` upgrade and would have failed identically on `0.1.2-rc.1`. It is repaired at the argv boundary, and `docs/dsh_session_protocol.md` §1 and §10 plus the validation record now carry both the defect and the repair.

**Remaining**
- Turn-level behavior against `0.2.0-rc.2` is unverified: no prompt was sent before or after the repair, so `session.event`/`session.status` handling, `turn/end` mapping, record paging, and the completion-authority table remain fixture-verified only. Sending one is the next live step and needs explicit authorization for vendor inference.
- The Control v7 daemon path (`vibemuxctl dsh open`) was not re-run against the repaired driver; it shares `build_launch_spec` with the terminal surface, which now completes the handshake.
- `vibemuxctl daemon start` reported `daemon_spawn_failed` while `vibemuxd.exe --project-root ...` started correctly; not root-caused.

**Known risks**
- `-Apply` installs third-party packages; the default check mode does not. CI never exercises a real install: the tests use fixture trees with `-SkipProbes`.
- The `tui` profile comes from a third-party community bundle. The script checks composition, not behavior, and never launches an interactive surface.

---

### 2026-10-04 - Dual-track workflow and DSH integration on main

**Scope and resulting contracts**
- Combine the reviewed dual-track branch at `6edb1aa` with the DSH SDK driver, daemon service, CLI, and terminal surface; preserve the newer GUI and the existing workflow hardening.
- Workflow operations retain Control v6; DSH operations require Control v7. The daemon accepts v1-v7 requests for their established operations. Older clients reject the new descriptor and must upgrade with the daemon. Store schema 5 is unchanged by this integration.
- Preserve GUI ADR 030 and workflow ADR 031; the DSH decision is ADR 032. DSH sessions remain in memory and are not canonical Task/Run records.
- Verify and pin the external Node/DSH package launcher pair. Generic ADR 029 DSH dispatch is initialize-only until a read-only profile is certified; operator consent and raw SDK protocol support cannot grant generic execution. Dedicated SDK sessions remain explicitly configured. DSH has no workflow writable profile and cannot acquire fixture write-mode certification or worker admission.
- Retain the intentional deletion of the uncompiled GUI stub bank and give the daemon DSH fixture a distinct binary name.

**Independently executed validation**
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`: passed on the final permission boundary.
- `cargo test -p vibemux_harness -p vibemux_dsh -p vibemux_frontend --all-features --locked`: 266 passed, 0 failed, 1 ignored.
- `cargo test -p vibemuxd --lib --test workflow_admission --test dsh_session --test control_harness_dispatch --test harness_dispatch --all-features --locked`: 152 passed, 0 failed. The subsequent actual-IPC DSH execution/slot refusal regression also passed after its final assertions were added.
- `python -m pytest`: 111 passed using the integration source via `PYTHONPATH` and a test directory outside that checkout. Repository Ruff format/lint passed; strict mypy for the 14 acceptance-tooling modules passed.
- `node --test tests/fixtures/taskboard_lite/calibration/calibration_integrity.test.mjs`: 5 passed. The pinned good frontend also passed all 16 direct browser checks.
- CLI help lists both workflow and DSH verbs; the DSH terminal surface refuses an unconfigured project without launching a runtime.
- Environment: native Windows, Rust 1.85.0/MSVC. Build/test debug information was disabled after an MSVC D8050 debug-record error; TEMP/TMP and LOCALAPPDATA were scoped to owned test directories because the managed environment denied normal runtime writes. Global settings were not changed.

**Remaining failures and qualification limits**
- The complete `cargo test --workspace --all-features --locked --no-fail-fast` run, before the final generic-dispatch permission fence, observed 819 passed, 5 failed, and 2 ignored across 93 targets. This is not a green full-workspace result. Final changes were verified with the affected suites and the explicit IPC regression above.
- Three unchanged `vibemux_cli::bootstrap_process` tests failed with `SpawnFailed`; an independent reproduction of the unchanged PowerShell detached-launch helper timed out. A direct immediate-exit fixture and CLI help succeeded. Detached daemon startup remains unqualified in this execution environment.
- The two unchanged `workflow_cooperate` tests failed after contained frontend browser verification, ending with `dispatch_not_completed`. The same good frontend passed all 16 checks directly; the contained verifier boundary remains unresolved. No test, acceptance gate, or cleanup rule was weakened.
- Repository-wide `python -m mypy .` still reports 20 errors in three untouched files. `cargo nextest`, `cargo deny`, and `cargo audit` are unavailable; no installation was performed.
- Linux/WSL, interactive DSH/vendor operation, AAG model routing, and live native terminal acceptance were not run. The pinned 37-case acceptance report was not regenerated, and its historical results do not certify this merged source. The Version 2 runtime enhancements and live gates remain unfinished.

### 2026-10-04 - DSH harness adapter and session interaction substrate (ADR 032)

**Status change**
- New adapter: DeepSeek Harness (`dsh`) is registered as the eleventh harness, and `NativeProtocol::DshSdkJsonRpc` describes its SDK JSON-RPC stdio protocol. `DshSdkJsonRpc` joins the ADR 029 dispatch path with a real initialize-only probe.
- New crate `vibemux_dsh`, new surface `vibemux_dsh_tui`, and a new daemon service with **Control v7** plus the `vibemuxctl dsh` client.
- Control protocol moves from v6 to v7. v1–v6 requests remain accepted for their operations; a pre-v7 client refuses a v7 descriptor, so `vibemuxctl` and `vibemuxd` are upgraded together.
- The substrate is `VERIFIED` at the fixture level: a pure state-machine suite plus a driver suite and a daemon-level end-to-end suite against `vibemux_dsh_fixture`. **No prompt has been sent to a real DSH runtime.**
- No milestone changes. No persistence schema change: a DSH session has **no canonical record** and lives in daemon memory (ADR 032 open decision 1).

**Implemented**
- `vibemux_harness::dsh` (pure, no I/O): the observed wire vocabulary with strict bounded decoders, the client-side session state machine, a record classifier, and a bounded sequenced record ring (`wire`, `session`, `records`).
- `AgentKind::Dsh` appended last, with the wire name `dsh`, command `dsh`, display `DSH`, and provider `deepseek`.
- `NativeProtocol::DshSdkJsonRpc` with `accepts`, `supports_probe`, `supports_execution`, and `requires_launcher_pair`; route config gains `entry_script`, `profile`, and `provider`, protocol-conditional in both directions.
- `vibemux_harness::dispatch::protocol_session`: a DSH arm for the one-shot path, including the `initialize` handshake, the `session/prompt` admission, notification correlation by session id, and structured `turn/end` terminal evidence.
- `vibemux_dsh`: operator config loading and trust verification, the Node launcher pair, `ProcessTree` containment, bounded JSON-line framing, initialize and turn deadlines, cancellation by tree termination, the record buffer, and a `summary()` accessor.
- `vibemuxd::dsh_session::DshSessionService`: one live session per project, opened lazily against `.vibemux/dsh_session.json`, joined at daemon shutdown so no vendor process outlives the daemon. It owns only the control-frame page budget; framing, containment, and records stay in `vibemux_dsh`.
- Control v7: `dsh_session_{open,prompt,poll,status,cancel,close}`, with `dsh_*` added to the allowlisted remote-error namespaces so an actionable DSH code reaches a client instead of collapsing to `control_remote_error`.
- `vibemuxctl dsh open|prompt|poll|status|cancel|close`: a prompt comes from a file or stdin, never argv; `dsh poll` is the only DSH command that prints runtime content.
- `vibemux_frontend::dsh_tui` and the `vibemux_dsh_tui` binary: a live session surface with an input editor, bounded transcript, cancel path, teardown, and the audited `tui::Theme` registry.
- Test fixture `vibemux_dsh_fixture` (modes `serve`, `hang`, `error-turn`, `foreign`, `garbage`, `no-turn-end`, `exit-early`), selectable through `--profile` so a daemon test picks a behaviour the way an operator picks a profile.

**Behaviour and invariants**
- Idle is not completion. `session.status: idle` is rendered as a diagnostic and never ends a turn; only a correlated `turn/end` with a recognized `reason.kind` produces a terminal outcome, and an unrecognized one resolves as `unverified`, which is never success (AGENTS.md §2 invariant 5).
- The notification stream is unfiltered by the vendor, so notifications are matched against the session's own id; foreign records are counted and dropped. `subagent.*` payloads carry `parentSessionId`/`childSessionId` and no `sessionId`, so they are never correlated through the session-id path.
- One turn at a time per session, and one session per project (`dsh_session_busy`). The wire carries no prompt-to-turn correlation id, so a second concurrent prompt would make turn attribution impossible.
- No shell and no shim: the route pairs a canonical `node` image with an entry script the installed `@deepseek-ai/dsh` package names in its own `bin` map, and neither may resolve inside the project root.
- Cancellation terminates the contained process tree, because the protocol publishes no cancel or session-close method.
- Prompts reach the vendor only as JSON string data on stdin and never from argv. Status views, events, and errors carry a prompt's size and SHA-256, never its text, and never a path.

**Evidence**
- commands (native Windows, MSVC, pinned 1.85.0):
  - `cargo fmt --all -- --check` clean (exit 0).
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean (exit 0).
  - `cargo test --workspace --all-features -j 2 --no-fail-fast`: every crate passes except 33 tests in `vibemux_cli` and `vibemuxd` that are blocked by this session's execution environment; see the note below.
- Passing suites that cover this change: `vibemux_harness` 123 (68 lib + 55 integration, including 21 route-config and 14 protocol-session tests); `vibemux_dsh` 18 (6 unit + 12 driver integration); `vibemuxd` 152 (78 lib including the DSH control tests, 9 `dsh_session` end-to-end, 8 plugin_registry, 23 harness_dispatch, 6 launch_trampoline, 3 terminal_observer, 3 control_harness_dispatch, 1 bin, plus the environment-blocked suites listed below); `vibemux_frontend` 80 (68 lib, 1 ignored, plus cli/palette_parity/supervisor_ui); `vibemux_cli` 18 (15 lib including 5 new `dsh` grammar/projection tests, 3 bin); `vibemux_probe` 18; `vibemux_store` 35.
- The `vibemuxd` `dsh_session` suite is the end-to-end gate: it drives a real contained fixture process over the real control IPC and proves the launcher-pair trust rules, the handshake gate, one-turn-at-a-time admission, structured completion, cancellation by tree termination, teardown on daemon shutdown, and the rule that no prompt text reaches a status view.
- CLI smoke on the built binary: `--help` lists all six commands; `dsh prompt` without `--prompt-file` and an unknown verb both return `cli_invalid_arguments` (exit 2); `dsh status` against a project with no daemon returns `daemon_not_running` (exit 4).
- Golden dashboard fixtures were regenerated for the intentional eleventh row and the one-line summary change from `5 unavailable` to `6 unavailable` (6 files, 6 changed lines).
- Regression coverage added for the subagent-correlation defect found during review: `subagent.*` payloads carry no `sessionId`, so correlating them through the session-id path would have failed the session on the first subagent event.

**Environment limitation (not a defect in this change)**
- 33 tests fail in this session with `ControlRuntimeUnavailable` from `DaemonPaths::ensure_runtime_dir`, whose protected `%LOCALAPPDATA%\VibeMux\runtime` DACL step cannot complete under the agent's process token: `vibemux_cli` lib 10 and `bootstrap_process` 3; `vibemux_frontend` lib 1 (`supervisor_client`); `vibemuxd` lib 7 (`process::tests`), `daemon_process` 4, `supervisor_grpc` 3, `supervisor_workflow` 4, `plugin_registry` 1 (`daemon exited early`, the same startup failure).
- Every one of them fails before any harness or DSH code runs; the raw-path control constructors used by the DSH tests do not touch that surface, which is why the DSH suites run here. They are expected to pass on a normal developer or CI token; that has not been re-verified here.
- Other environment workarounds used for this run only, none applied to the repository: `TMP`/`TEMP` redirected into `target/tmp` (the platform temp directory is not writable here, which breaks `prost-build` and `tempfile`), and `CARGO_PROFILE_DEV_DEBUG=0` (MSVC `D8050`, the C7 debug-record limit, is hit by this checkout's long paths when compiling bundled C dependencies). Separately, `target/` had grown to 71.8 GB and filled the drive; `cargo clean` reclaimed it.

**Remaining**
- A canonical persisted session record. The service keeps the session in memory, so a daemon restart loses it and its records (ADR 032 open decision 1).
- Live validation against a real DSH runtime. No prompt has been sent to the vendor: the adapter is verified against the wire shapes read from `@deepseek-ai/dsh-sdk-protocol` 0.1.2-rc.1 and against `vibemux_dsh_fixture`.
- Bootstrapping the `sdk` profile for the first time writes to the operator's DSH home; that side effect is documented in `docs/dsh_session_protocol.md` and README, and it did occur on the reference host during reconnaissance.
- ADR 032 open decisions 2 to 6, including consolidating the two trusted-launcher-pair loaders.

**Known risks**
- DSH's protocol carries no version negotiation and no compatibility promise, so this adapter binds to observed 0.1.2-rc.1 shapes. A later release may change them silently; the handshake checks only `serverInfo.name`.
- The vendor `sdk` profile defaults `DSH_MAX_TOKENS_AS_SUCCESS` to true, so a token-limited turn may reach VibeMux already reported as `completed`. VibeMux's own mapping stays conservative, but it cannot recover the distinction.
- The runtime is not a sandbox: containment governs lifetime only, and the vendor profile decides what the session may read and write.

### 2026-09-28 (3) - Claude-style GUI phase 1 (ADR 030, `feat/claude_style_gui`)

**Status change**
- M5.0 frontend stays `PARTIAL`. ADR 030 phase 1 is implemented; phase 2 (shortcuts, quick switcher, appearance modes, motion, copy buttons) is not.

**Implemented**
- `claude_light` palette as the default for new profiles and a retuned warm charcoal `claude` palette; `config/theme_palettes.json` and the Python fallback updated under the parity test; ADR 026's surface invariant amended to a 5.0 luma gap in either direction.
- WCAG contrast math, luminance-based light detection, and readable accent text in every palette.
- Serif display family from Georgia or Cambria, with sans, serif, and CJK faces loaded independently (a missing CJK font no longer skips Segoe UI).
- Collapsible sidebar with New task, Recents, and a workspace menu; welcome state with a local-time greeting; rounded composer with a target chip, a muted placeholder, and Enter/Shift+Enter handling that never sends; borderless serif task rows with a scroll fade; a quiet top bar; an always-docked task drawer with one header and an aligned details grid; deletion of four uncompiled shell files.
- Final whole-branch review (fresh reviewer): no Critical findings; two Important findings fixed with RED-to-GREEN tests. Long drafts now scroll inside a composer area capped at 35% of the window height (48 to 240 px), so they can no longer push the send row and the "not connected" caption off screen or collapse the conversation. Custom controls (icon buttons, New task, Recents rows, the workspace button, navigation rows) draw a 2 px accent focus ring when keyboard-focused.
- The frontend enables `time`'s `local-offset` feature for the greeting, which adds `num_threads` 0.1.7 (MIT or Apache-2.0, used on Unix only) to `Cargo.lock`.

**Evidence**
- commands on the final head (native Windows): `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean; `cargo test --workspace --all-features --no-fail-fast`: 529 passed, 1 failed, 2 ignored. The failure was `vibemux_cli` `startup_timeout_terminates_only_the_spawned_fixture`, a timing flake that also fails on `main` at `21447ac` (1 of 8 runs there, 1 of 5 on this branch). An earlier run on this branch instead failed the `vibemux_cli` recovery tests `live_daemon_is_never_recoverable` and `descriptor_only_recovery_and_pid_mismatch_fail_closed` (`ControlRuntimeSecurityInvalid`), which fail the same way on `main` on this machine. None of these tests or their crates change in this branch. Python: pytest 64 passed; ruff check and ruff format clean; mypy reports 20 errors in `tools/a2a_python_interop.py`, `tools/a2a_tck_auth.py`, and `tests/test_harness.py`, the same 20 as on `main`. `git diff --check` over the branch is clean.
- Screenshots: `claude_light` and `claude` at 1280x800, 960x600, and 1.5x scale for the chat, drawer, welcome, and collapsed scenes were reviewed; the committed previews in `docs/frontend_previews/` were regenerated with `claude_light` at 1440x900.

**Remaining**
- ADR 030 phase 2.
- At the 960x600 minimum with the drawer open, the conversation column is narrow and its header badge is clipped.
- Deferred review minors: the workspace badge letter reaches about 4.2:1 instead of 4.5:1; Esc closes an open popup and the drawer in one press; Recents keeps highlighting a task after its drawer closes; every disabled composer target says "Not detected" even when the version check failed; the Agents page title is still sans; the collapsed rail lacks Diagnostics and a current-page indicator; ADR 030's 3:1 control-boundary goal conflicts with its own border tokens and needs an ADR decision.
- The flaky `vibemux_cli` startup-timeout test and the recovery-test failures on this machine are pre-existing and tracked separately.
- No live check yet on a real display at scales other than 1.0 and 1.5, with other system fonts, or with IME input on a real keyboard.

### 2026-09-29 - Shared ACL-marker test isolation (`fix/acl_marker_test_isolation`)

**Status change**
- None. M3 stays `VERIFIED`. This is a test isolation fix plus a `DaemonPaths` constructor refactor that keeps production behavior the same. No ACL check, wire contract, schema, or CLI output changed.

**Symptom**
- On `main` at `21447ac`, `cargo test -p vibemux_cli --lib` failed in 12 of 15 back-to-back runs of the built test binary on the development machine. `recovery::tests::live_daemon_is_never_recoverable` and `recovery::tests::descriptor_only_recovery_and_pid_mismatch_fail_closed` panicked at `DaemonControlServer::start_for_paths` with `ControlRuntimeSecurityInvalid`. Each test passed when run alone. The "2 load-flake failures in `vibemux_cli` recovery tests" in the 2026-09-26 entry, attributed there to issue #6, match this signature, as do the recovery-test failures recorded as pre-existing in the 2026-09-28 (3) entry. That entry's `startup_timeout_terminates_only_the_spawned_fixture` flake is a different issue and is not addressed here.

**Root cause**
- `tests::classify_daemon_exit_distinguishes_security_failures` came from the issue #8 review round (V2). It granted Everyone on the real, shared `%LOCALAPPDATA%\VibeMux\runtime\.acl_v1` marker and restored the ACL afterwards. Its comment assumed no sibling test verifies the marker's ACL. That has been false since issue #7: every trusted start verifies the marker in phase 1 (`verify_control_pre_publish` covers root, leaf, marker, and writer lock). The two recovery tests start daemons in parallel with it, so a start that landed inside the loosen/restore window failed closed, as designed.
- Temporary diagnostics on the failing call reported `AccessControlInvalidAt { stage: 7, index: 3 }` for the four-path phase-1 batch. Index 3 is the marker and stage 7 means the effective ACE count was not 3. The checks behaved as specified. The failure was test isolation against shared user state.
- Skipping only the classify test gave 0 failures in 15 runs.
- The same design had a second hazard. A panic between the loosen and the restore left the user's shared marker loosened, which fails every later trusted start on the machine, including a real `vibemuxctl daemon start`. This happened during validation (below) and was repaired by hand.
- Two `vibemuxd` tests had the same problem across processes. `ensure_runtime_dir_heals_a_stale_acl_marker` emptied the shared marker and `concurrent_first_touch_serializes_marker_publication` deleted it. `marker_test_lock` serialized them inside the `vibemuxd` test binary, but not against other processes reading the marker at the same time, such as another worktree's test run or a live daemon start.

**Implemented**
- `crates/vibemuxd/src/process.rs`: `DaemonPaths::from_project_root` is split into `canonical_project_root`, `windows_control_runtime_root` (the `<LOCALAPPDATA>\VibeMux\runtime` derivation), and a shared `assemble`. Production paths, error codes, and error order are unchanged. The new test-only `DaemonPaths::from_project_root_with_local_app_data` is compiled under `cfg(all(windows, any(test, feature = "test_helpers")))`, the same gating pattern as `start_with_dispatch`. It roots the control runtime under a private base with the production layout, so the ensure path secures it with the same helper and the tampering tests still exercise the real code.
- The three tests that loosen, empty, or remove the marker now use private roots. `marker_test_lock` is removed because no test in the workspace mutates the shared marker any more.
- `crates/vibemux_cli/Cargo.toml`: adds a dev-dependency on `vibemuxd` with `test_helpers`.

**Evidence**
- Regression pin in the classify test: while its private marker is loosened, a sibling project's surviving surface under the real shared root must still verify. With the test pointed back at the shared root, it failed deterministically at that assertion even when run alone (`sibling control surface is unaffected by this test: ControlRuntimeSecurityInvalid`). With the fix it passes.
- New `isolated_local_app_data_relocates_the_whole_control_surface`: every control artifact (root, leaf, marker, descriptor, writer lock) is under the private base and none is under the shared root. Project-local state paths match production.
- After the fix, the `vibemux_cli` lib test binary failed 0 of 20 runs (before: 12 of 15). A full workspace run left the shared marker's modification time unchanged. Before the fix, the first-touch test deleted and republished the marker on every run.
- Commands (native Windows): `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean; `cargo test --workspace --all-features` (parallel, as in CI) exit 0, 492 passed, 0 failed, 2 ignored. The one extra test is the new contract test.

**Observations (not changed)**
- With `control_acl_init_lock` temporarily removed, `concurrent_first_touch_serializes_marker_publication` passed 10 of 10 runs. It therefore no longer pins the lock, although its comment says it does; the issue #6 review round measured 8 of 10 failing. The likely cause is issue #7 Stage 1's `helper_slot`, which serializes each thread's `secure_user_directory` helper and so staggers the publications. This predates this change and does not depend on which root the test uses. It is left for a separate change.
- Validation side effect: the deliberate pre-fix run above panicked before its restore and left the real marker with an extra Everyone ACE, the hazard described above. `icacls <marker> /remove *S-1-1-0` removed it, and the marker again has its three inherited ACEs. Those runs also left five empty per-project leaves under the shared root. The root already held many empty leaves from earlier test runs. None are removed here.

**Remaining**
- The `rust-ubuntu` compile of the refactored unix path comes from the pull request CI run. A local Linux cross-check stopped at the bundled SQLite C build.

### 2026-09-29 - Deterministic startup-timeout bootstrap test (`fix/startup_timeout_fixture_race`)

**Status change**
- None. Test-only change: no production code changed. It removes the known load-sensitive flake in `bootstrap_process::startup_timeout_terminates_only_the_spawned_fixture` recorded in the 2026-09-15 (5) and (9) entries, and again in the 2026-09-28 (3) entry.

**Root cause**
- The test gave `start_daemon` a real-time 150 ms startup deadline and then required the hang fixture's started marker. The deadline starts once the fixture's PID is known (on Windows, after the PowerShell launcher helper reports it), so it raced only the fixture's own launch. Temporary instrumentation on a loaded Windows host (not committed) showed the marker 50-590 ms after the PID was known. In every failing run, the deadline fired 150-161 ms after the PID with no marker, and the fixture was killed before it ran. Unmodified, the test failed 15 of 20 runs on that host at `started.is_file()`.
- A second mode appeared with the CPU saturated (24 busy loops on 24 logical CPUs). The 150 ms timeout also cut the Windows helper's PID wait to its `max(startup_timeout, 3 s)` floor. With three helpers starting at once, that wait sometimes expired (`SpawnFailed`, 3 of 12 full-binary runs).
- The termination check was a third real-time assumption. It slept 700 ms and required `completed` to be absent, against a 600 ms hang, so the kill had to land within 600 ms of the marker.

**Implemented**
- The test runs on a paused tokio clock (`start_paused`) with the production `DaemonBootstrapConfig` timing. A `spawn_blocking` task waits on real time for the marker. While it runs, the paused current_thread clock cannot auto-advance (tokio test-util, tokio-rs/tokio#5115), so the startup deadline cannot expire before the fixture has written its marker. After that, the clock jumps to the deadline without real waiting. The production timing gives the Windows helper its 30 s PID wait. The marker wait is bounded by the default start timeout plus 30 s.
- The test proves termination by process exit. It reads the fixture's PID from the marker and waits on real time (5 s) for that process to exit, under `FixtureProcessGuard`. It then requires `completed`, the descriptor, and the writer lock to be absent. The hang fixture now hangs for 30 s instead of 600 ms, so only a kill ends it within the bound.
- `vibemux_cli` dev-dependency: `tokio` with `test-util`. This adds no crates and leaves `Cargo.lock` unchanged; non-test builds are unaffected.

**Evidence**
- Under 24 CPU busy loops, full `bootstrap_process` binary: the original test failed 15 of 15 and 12 of 12 runs (the marker race plus `SpawnFailed`). The fixed test passed 20 of 20. The marker gate with the old 150 ms timeout passed 12 of 15, and its 3 failures were the `SpawnFailed` mode, which led to the production timing.
- Mutations (temporary, not committed): with the fixture delayed 2 s before its marker, the original test failed 3 of 3 and the fixed test passed 3 of 3. With a launcher helper that never kills the fixture, the fixed test failed with `fixture process did not exit`, and the guard removed the leaked fixture.
- The test alone now takes about 0.7 s; the fixed 700 ms sleep is gone.
- commands (native Windows): `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean; `cargo test --workspace --all-features --no-fail-fast` 490 passed, 1 failed, 2 ignored, and all three `bootstrap_process` tests passed.
- The one failure, `vibemux_cli` `recovery::tests::live_daemon_is_never_recoverable` (`ControlRuntimeSecurityInvalid`), is the pre-existing shared-marker race addressed by PR #14 and is unrelated to this change. `classify_daemon_exit_distinguishes_security_failures` loosens the per-user `.acl_v1` marker while sibling tests start control servers. Over eight `cargo test -p vibemux_cli --all-features --lib` runs, two failed; with that test skipped, none of eight failed.

**Known risks**
- The gate depends on tokio test-util not auto-advancing a paused clock while a `spawn_blocking` task runs (`tokio` is pinned at `=1.47.1`). If an upgrade dropped that, the deadline would expire before the marker. The test would then fail loudly at "fixture never wrote its started marker"; it would not pass silently.

**Remaining**
- Windows and Linux CI evidence comes from the pull request run.
- `abrupt_process_recovery_preserves_database_and_allows_restart` also failed once under doubled load (2026-09-15 (9)). It did not fail in any run here and is not changed.

### 2026-10-03 - Dual-track workflow (ADR 031) offline slice (`feat/dual_track_workflow`)

**Status change**
- New area: the dual-track workflow ([ADR 031](docs/adr/031_dual_track_workflow.md)) is `PARTIAL — OFFLINE ONLY`.
- Control IPC moves from v5 to v6 and the Rust store schema from 4 to 5. M3 stays `VERIFIED` and ADR 029 dispatch stays `PARTIAL`.

**Implemented**
- `vibemux_workflow` is a pure crate (no I/O). It covers:
  - TaskSpec validation against the source request, including prohibitions, numbers, literals, invented scope, and requirement coverage.
  - The deterministic English renderer and templates, plus contract identity and generations.
  - Gates, comparison selection, message broker rules, and context bundles with redaction.
  - Leases, slot planning, candidate snapshots, the gateway route contract, and the prompt optimizer.
- `vibemux_store` schema 5 is forward-only. It adds nine workflow tables with fenced leases, idempotent admission, a content index, and versioned prompt policies.
- `vibemux_harness` gains writable worker profiles, final-text extraction, and per-turn prompts.
- `vibemuxd` gains a workflow service with a deterministic coordinator (at most two broker rounds). The service:
  - gives every slot an owned worktree;
  - collects candidates itself;
  - runs an independent review and the pinned trusted verifier per candidate;
  - allows bounded repair;
  - selects between comparison candidates;
  - integrates into a fresh worktree.
- Control v6 adds 15 workflow operations. Private workflow state is restricted to the daemon's user. A pre-v6 client refuses a v6 daemon.
- `vibemuxctl` adds `workflow`, `slots`, `context share`, `session inspect`, and `prompt`.
- The TaskBoard Lite fixture adds a frozen base and contract, plus trusted `api`, `store`, and `browser` suites calibrated against a good solution and nine mutants.
- The `vibemux_worker_fixture` binary stands in for every worker, reviewer, and compiler turn.
- `scripts/verify_dual_track.py` is the acceptance runner, with Python as the test driver only. It pins the 37-scenario benchmark, re-derives every class status from logs and evidence digests, validates the report, and removes its private temporary root.

**Evidence**
- [docs/evidence/dual_track_offline_acceptance.md](docs/evidence/dual_track_offline_acceptance.md): the offline run on `7de3aa5` with a clean tree.
  - Verdict `BLOCKED`: 22 `PASS`, 11 `BLOCKED`, 4 `NOT_RUN`, 0 `FAIL`.
  - `cargo test --workspace --all-features`: 708 passed, 0 failed, 2 ignored in 86 targets.
  - pytest: 85 passed. Calibration: 9 of 9 mutants caught, browser suite in Edge.
  - The standalone `--validate` check reported the report valid.
- On the same head: `cargo fmt --check` and `cargo clippy --workspace --all-targets --all-features -- -D warnings` are clean. `ruff check` and `ruff format --check` are clean. `mypy --strict` on the runner is clean.
- Repository-wide mypy reports the 20 pre-existing errors in three files this branch does not touch.
- Baseline before any workflow code (`0588620`): 531 passed, 0 failed, 2 ignored in 74 targets.
- No live validation. No model, AAG gateway, or vendor CLI was contacted.

**Remaining**
- A daemon AAG gateway client, plus live supervisor and compiler routes. Without them G01 and G03 are `NOT_RUN` and G04 is `BLOCKED`, and a `live` config cannot make a slot eligible to code.
- Owned native TUI sessions, attach, and structured/native handoff. Without them T01 is `BLOCKED`, and T02 and T03 are `NOT_RUN`.
- A live run of every `live_coding`, `live_mixed_harness` (T04), and `live_optimizer` class, which needs an explicit budget and configuration authorization.
- The known limitations listed in ADR 031: read-only prompt inspection, the unpersisted negative-control receipt, a crash simulated through the writer, and writable profiles that are cooperative admission rather than an OS sandbox.

**Compatibility / migration**
- Schema 5 is not downgraded. Rolling back needs a pre-migration store, and `vibemuxctl` and `vibemuxd` are upgraded together.
- Without `.vibemux\workflow_config.json`, every workflow operation answers `workflow_unconfigured`. Existing dispatch, harness, frontend, and A2A behavior is unchanged.

**Known risks**
- Workers are same-user processes. Owned paths and scope checks reject a bad candidate when the daemon collects it, but they do not stop a vendor process from writing outside its worktree.

### 2026-10-03 - Dual-track source review repairs and publication preparation

**Scope and implementation**
- Reviewed the offline dual-track branch from `342535a` and repaired state,
  ownership, execution evidence, and acceptance-report defects. The final
  reviewed and tested source is `b6b490d525d8a7b876776635d91764c84fcbede2`.
- Message batches bind to the admitted recipient attempt before launch;
  acknowledgements bind to the same completed attempt. Failed-turn messages
  remain unacknowledged and do not poison a later repair prompt or batch.
- Substantive review verdicts require a matching completed dispatch. Base
  commits and accepted contracts are enforced for attempts, candidates, and
  integration receipts. Recovery and lookup use bounded keyset pagination.
- Workflow lifecycle controls serialize with start registration; content
  retention and purge preserve independent owners; integration rejects ASCII
  and Unicode case aliases. Ledger publication failure attempts verified
  cleanup and has a real-Git main-integrity regression.
- Train cases now gate optimizer eligibility. Reports reconcile artifact
  summaries as well as scenario statuses, reject malformed metadata, require
  real passed tests and the complete nine-mutant inventory, and verify source
  contents. Timed-out acceptance commands terminate their owned process trees.

**Validation executed**
- Fresh offline acceptance: `BLOCKED`, 22 PASS / 11 BLOCKED / 4 NOT_RUN /
  0 FAIL. Rust: 721 passed, 0 failed, 2 ignored in 86 targets. Python:
  111 passed. Edge calibration caught all nine mutants. Standalone report
  validation succeeded with no problems.
- Workspace formatting and Clippy with warnings denied passed; repository
  Ruff lint/format and strict acceptance-module mypy passed. Five Node
  calibration regressions passed. Repository-wide mypy reports the same
  20 errors in three untouched files.
- Independent final static review found no outstanding actionable regression
  in the repair delta. The exact commands and platform are recorded in
  [the evidence summary](docs/evidence/dual_track_offline_acceptance.md).
- An initial fresh build failed because drive space was exhausted. Only
  task-created caches were moved to another drive before successful rechecks;
  that failed attempt is not counted as a passing test run.
- `nextest`, `cargo-deny`, and `cargo-audit` are not installed locally and were
  not claimed. Linux/WSL and live terminal paths remain unverified here.

**Compatibility and remaining work**
- SQLite schema 5 and Control v6 are unchanged by the repairs. The private
  content index writes version 2 and reads legacy version 1 conservatively;
  rollback requires matching pre-upgrade private state (ADR 031).
- The workflow remains PARTIAL / OFFLINE ONLY. Gateway/native fixture gaps,
  live gateway, live coding, native TUI/handoff, and live optimizer acceptance
  remain blocked or not run. No live model calls, merge, release, or deployment
  occurred in this verification.
- Route labels are not independently bound by the store because dispatch
  route identity is not persisted. If both ledger publication and verified
  cleanup fail, the resource is retained for later cleanup; the double-failure
  path is not injected by the new regression.

> Integration note: the next three entries were written on `codex/remaining_runtime` as ADRs 033 to 036, store schemas 6 to 8, and Control v8 and v9. Merged with the decision port (ADR 034, schema 6, Control v8), they carry ADRs 036 to 039, schemas 7 to 9, and Control v9 and v10; the text below uses the merged numbers.

### 2026-10-05 - Runtime execution-authority increment (ADR 036)

**Status:** PARTIAL / OFFLINE INCREMENT. Full Version 2 acceptance remains BLOCKED.
Base HEAD is `a639ed5befdf232af4f4aa0f2d7d4dc55cf3c146`; source is uncommitted
on `codex/remaining_runtime` in an owned integration worktree. The original
37-scenario benchmark and historical evidence remain unchanged.

**Implemented**
- Independent read/search/edit/write/development-test grants, contract authority,
  guarded fixture profiles and persisted effective dispatch/reviewer identity.
- Bounded daemon AAG transport, fake-server permits and a candidate compiler using
  existing deterministic admission. Durable model-call accounting includes the
  pre-admission compiler charge; rejected/unknown results never authorize replay.
- Native-controller/owned-adapter effects with fenced ownership and single-claim
  input outboxes, plus writer-derived checkpoint foundations that refuse missing
  active-job, message or artifact joins.
- SQLite schema 7; historical attempt/receipt readability and fail-closed legacy
  execution. Workflow status/export schema 2; this increment adds no Control version.
- Source/operator owned-path provenance, persisted nonpromotable negative-control
  verifier receipts, operational optimizer validation and direct Windows bootstrap.

**Validation and limits**
- Exact executed commands, outcomes, platform coverage, requirement/test mapping
  and resource state are recorded in [runtime enhancement evidence](docs/evidence/runtime_enhancement_increment.md)
  and its [JSON record](docs/evidence/runtime_enhancement_increment.json).
- Fixtures do not certify live vendor grants, AAG request-scoped route identity,
  native Windows TUI sessions, browser containment or complete recovery.
- Live model supervisor, production native Control/UI/backend, affirmative approval
  delivery, standalone review, semantic prompt editing and full active workflow
  checkpoint/reconciliation remain future increments.
- No global context/compaction setting, live gateway default, account, deployment,
  publication or original-checkout source was changed by this increment. No commit,
  merge or push was performed for this follow-up.
- Schema-7 rollback requires matching pre-upgrade database/private-state backups;
  a source checkout does not downgrade state. See ADR 036.

### 2026-10-06 - Model supervisor and Windows launch repair (ADR 037)

**Status:** PARTIAL / OFFLINE INCREMENT. The model driver now executes genuine
admitted workflow effects. Full Version 2 acceptance remains BLOCKED.
Source continues on `codex/remaining_runtime`, based on `a639ed5`; the local main
checkout and pre-existing `ui_check/` are preserved.

**Implemented**
- Explicit model/deterministic driver selection with one supervised run owner.
- Writer-bound observations, durable model rounds and claimed action intents;
  real worker/reviewer/verifier/integration effects retain existing gates.
- Dependency-safe task eligibility, observed worker overlap, exact replay and
  cross-round conflict handling, bounded malformed/rejected-response repair,
  owned post-claim receipts and restart fencing for unknown outcomes.
- Additive store schema 8 and Control v9 supervisor start/status. DSH keeps its
  v7 minimum and old operations retain their previous protocol requirements.
- Windows token preflight before acceptance tool children, process-scoped error
  handling and explicit DLL-initialization status. Paired project Ruff probes
  fail under the restricted token and succeed under normal host access; paired
  Node probes succeed. The precise denied DLL/subsystem is unknown.

**Validation and limits**
- Store/workflow tests: 257 passed. Final daemon unit/supervisor regressions:
  116 passed, including an actual two-worker timestamp-overlap assertion.
- The complete Rust workspace run recorded 924 passed, 3 failed, 3 ignored.
  The stale Control-version assertion was repaired and its whole 111-test unit
  target passed. Two existing cooperative browser-verifier failures remain
  under diagnosis at that stage; these counts are not a green workspace claim.
- The final focused cooperative rerun records two acceptance failures with the
  exact prerequisite `browser_page_unavailable:page_enable`, zero candidate
  checks. Blank-page preflight tests pass (5); the unchanged good frontend passes
  all 16 checks directly. Containment is preserved and the host gate stays blocked.
- Python: 132 passed; Ruff formatting/lint and scoped mypy (32 source files)
  passed. Workspace formatting and all-target/all-feature Clippy passed.
- [Current evidence and remaining scope](docs/evidence/model_supervisor_increment.md)
  separates these executions from historical records and scenario acceptance.
- Production native backend/Control/UI, affirmative approval, standalone review,
  semantic prompt edit, complete recovery joins and live route/vendor qualification
  remain incomplete. Unsupported supervisor tools refuse with typed diagnostics.
- No live inference, global configuration, deployment or unrelated process change
  was performed. Rollback from schema 8 requires matching pre-upgrade backups.

**Local delivery and cleanup**
- Implementation saved as `a3c65c3`; the test-only naming correction is `df121b7`.
  Its two Windows test files passed again (20 tests and scoped Ruff checks).
- Completed worker edits are recoverable at `be33e12` and `aa8d24a`; their clean
  checkouts were removed with normal Git worktree removal. Task-created build/test
  scratch, raw logs, patch copies and source caches were removed.
- The older unregistered optimizer directory has unknown ownership/handle state
  and remains excluded from cleanup. Main remains `a639ed5`, with `ui_check/`
  preserved. No push, live inference or deployment was performed.

### 2026-10-06 - Scoped supervisor context and operator edits (ADRs 038/039)

**Status:** PARTIAL / OFFLINE INCREMENT. Full Version 2 acceptance remains BLOCKED.
Base is `c8ad7e09c582370943754b87dcc342be4c690d09` on `codex/remaining_runtime`.

**Implemented and focused evidence**
- Real admitted-artifact/context retrieval, question admission and bilateral scoped
  context sharing feed bounded untrusted tool results into the next model request.
  Private bodies stay outside ordinary status/audit records. Read completion requires
  exact post-claim proof and fresh source session/contract/audience/state/expiry.
- Free-form sends refuse after any applied private read/query in workflow history;
  this closes a cross-task disclosure path and leaves scoped shares available.
- Immutable semantic contract successors preserve source, requirements, checks,
  policy/profile identity, spent counters and remaining budgets. Quiescent parent
  retirement and successor provenance commit atomically; stale/active/uncertain
  parents refuse. Replay binds the complete immutable admitted payload.
- Store schema 9 adds edit provenance and guards extended strict supervisor JSON.
  Control v10 adds operator contract edit; previous operation minimums stay unchanged.
- Focused edit tests: 4 real store cases and 2 Control cases passed. Repaired context
  suites: 23 real store tests and 7 guarded model-driver cases passed, including zero
  admitted messages after an attempted source-only disclosure.
- Final affected regression suites passed: 274 store/domain, 139 daemon/CLI unit
  and 13 service tests. Workspace formatting and all-target/all-feature Clippy
  passed. [Source-bound evidence](docs/evidence/context_edit_increment.md) retains
  exact commands and limitations without inventing scenario PASS.

**Browser qualification and remaining scope**
- A bounded, content-free probe compared Edge through NativeProcess and a direct
  launcher, both inside owned Jobs, plus Chrome through NativeProcess. Every tested
  Job reported zero remaining members. Edge 154.0.4258.53 timed out at Runtime.enable
  with Windows access-denial markers through both launchers; Chrome exited before
  DevTools. These observations do not qualify browser support or establish a fix.
- The direct launcher uses the same verified non-verbatim Node entry path; its first
  empty observation was corrected and rerun. No sandbox, Job breakaway or host ACL
  change was introduced. The existing browser-dependent acceptance gate remains.
- Standalone review mode, production native backend/Control/UI, affirmative approval,
  complete recovery joins and live gateway/harness qualification remain unfinished.
  No live inference, deployment or global context/provider change was performed.

**Local delivery**
- Implementation committed as `3a17885` on the owned integration branch; default
  feature workspace build passed with fixture capabilities disabled.
- Four completed clean task checkouts were removed normally with snapshot branches
  retained; task-created build/test scratch and raw logs were removed.
- Main remains `a639ed5`, `ui_check/` intact, and the older unknown optimizer directory
  remains excluded. No push, live inference or deployment occurred.

---

### 2026-10-06 — Mobile access research and phased design proposal

**Scope and resulting contracts**
- Research-and-design deliverable only, owned by this session: two new documents and this amended progress log. No code, store schema, control protocol, plugin contract, A2A surface, or CI change.
- Files: [docs/mobile_access_design.md](docs/mobile_access_design.md) (new), [docs/adr/033_mobile_access_channels.md](docs/adr/033_mobile_access_channels.md) (new, `Proposed`), `PROGRESS.md` (this entry plus the §5.2 planned-additions tree).

**Status change**
- None. No milestone status moved. Mobile access is recorded as a proposed design, not accepted scope.

**Implemented**
- Documentation-only deliverable, no code: an industry survey of bot-class and remote-access patterns (Happy Coder, the OpenClaw gateway with its current release line and official iOS/Android companion apps, lightweight Telegram control bots, Vibe Kanban, the Tailscale serve exposure pattern, Feishu/DingTalk/WeCom bot APIs, the Rust bot crates, and Tauri 2 mobile), a current-state inventory of the existing control IPC and A2A surfaces, and a track-based mobile access design with threat-model notes.
- Operator priority recorded in the design (2026-10-06): tailnet-exposed read-only mobile PWA first (proposed `crates/vibemux_web`), an OpenClaw gateway connector second (proposed `crates/vibemux_bot`), a direct Feishu adapter third, and an independent iOS/Android app via a Tauri 2 shell reusing the same frontend last.
- All tracks stay within the accepted security posture: the only new listener is loopback and token-gated, connector channel traffic is outbound-only, every payload follows content-free defaults, and no git-mutation, prompt-relay, or auto-approval action exists at any phase.
- An October 2026 community-alignment pass (Claude Code Remote Control and mobile apps, Cursor web agents, Omnara, Happy, claude-code-webui, agentapi, ccgram, OpenCode serve/share, OpenClaw webhook plus MCP seams, self-hosted Web Push feasibility, Tauri push plugin status, Feishu long-connection card callbacks) fixed three seams: Track A gains self-hosted web push and one-time QR enrollment with an OpenAPI-described JSON API; Track B uses the gateway's token-protected webhook plus MCP tool registration; Track C gets interactive cards over the Feishu long connection without a public callback URL.
- A three-reviewer adversarial pass (red-team security, rules-compliance audit, consistency/feasibility) produced 35 findings, and the material ones are folded back into the two documents: the Track B MCP surface mandates bearer auth with constant-time comparison and bounded concurrency; the Track B relay payload is minimal-verbosity-only (never titles) against cross-tool prompt injection through the gateway LLM; the same-logon trust-boundary claim is restated as a deliberate bounded extension of the ADR 020 model (the phone is outside that boundary); the M5 "remote browser access" deferral is recorded as partially reopened; planned-crate labels are corrected from `PLANNED` to `PROPOSED`; enrollment redemption is POST-body-only with a failed-attempt limiter; push endpoints inherit webhook URL validation; daemon-restart generation semantics (`ControlClient::same_generation`) are specified for dedupe keys; and CI ownership of the future PWA build is flagged as an infrastructure change.

**Evidence**
- Documentation only. No build, test, or live validation was executed for this change; the design cites verified code paths (`ControlOperation` v7 table, dispatch/DSH `after_sequence` cursors, the GUI 5 s `POLL_INTERVAL` polling precedent) and public references listed in the design document.

**Remaining**
- Acceptance decision on ADR 033; PWA frontend stack and MCP transport dependency reviews under the §16 rules; the M9 security review before any release claim; the Tauri 2 app track requires its own ADR with signing and distribution decisions.

**Known risks**
- Publishing the Track A web surface to a tailnet is an operator action on the operator's trust boundary; VibeMux still binds loopback only, and no remote-readiness claim is made beyond that documented pattern.
- Outbound delivery to IM or assistant platforms intentionally exposes task metadata (IDs, states, optionally titles); the design's content policy and allowlists bound but do not eliminate this egress.

---


---

### 2026-10-08 — Track A read-only mobile web implementation

**Status:** `PARTIAL`. ADR 033 is Accepted by the operator and the §5.2
`vibemux_web` label is PLANNED (scope acceptance); implementation and release
qualification are recorded separately here. No milestone is promoted.

## Scope
- Task: Implement accepted Track A in the additive `vibemux_web` workspace member.
- Owned modules: web control projections, HTTP/API/authentication, enrollment,
  private web state, Web Push, frontend, API contract, build/CI, and these docs.
- Integration used one feature branch and separate bounded worker worktrees.
  Existing primary-checkout DSH and documentation edits were preserved.

## Implemented
- Operator-started, fail-closed loopback server with a 5-second configurable
  poller reusing existing authenticated control IPC; no daemon autostart.
- Bounded task/run and optional configured dispatch/workflow projections,
  optional DSH health, generation/race detection, and stale title suppression.
- Instance bearer on every route, including shell, assets, OpenAPI and
  enrollment; constant-time digest comparison, failed-attempt/concurrency/body
  limits, strict CSP/referrer/security headers, static MIME and ETag/304 handling.
- Random 256-bit, 120-second single-use enrollment tokens with bounded storage,
  POST-only redemption and SVG QR invitations. Rotation invalidates all HTTP
  credentials and prior push subscriptions while keeping permanent VAPID keys.
- Persistent VAPID P-256 keys and full bounded PushSubscription JSON, protected
  runtime files/instance lock, validated public/pinned HTTPS endpoints, no proxies
  or redirects, bounded retries/responses/sweeps, expiration/404/410 pruning and
  quarantine. Push wire JSON contains exactly kind/id/state; dedupe stays local.
- Svelte 5/Vite/TypeScript PWA, local bundled Workbox, explicit update activation,
  a bounded 90-second/64-entry/256-KiB authenticated cache, stale notices and
  cross-generation/refresh consistency checks. No cached title is current.
- Hand-written OpenAPI 3.1 API version 0.1.0 with route coverage. Exact Node
  22.22.0 frontend build/check/test/audit steps precede cargo in Windows/Ubuntu CI.
  Compile-time guard rejects missing/empty frontend output; dist is not tracked.

## Behavior and invariants
- Production web code never opens SQLite, writes canonical state, requests
  terminal output, invokes a write control operation, or mutates Git.
- Task links are validated configuration UUIDs because the current protocol
  cannot enumerate dispatch/workflow IDs by task. Missing links remain untracked.
- Every endpoint remains bearer-gated per the operator's explicit clarification.
  Ordinary browser/QR mobile bootstrap is unavailable unless the operator supplies
  authentication covering navigation, assets and service-worker/background fetches.
- State reports come from validated daemon projections, never terminal exit/idle.

## Files changed
- `crates/vibemux_web/` Rust modules, binary, tests, build guard and OpenAPI.
- `crates/vibemux_web/web/` frontend sources, npm package/lock and ignored build output.
- Root Cargo manifest/lock, `.github/workflows/ci.yml`, ADR 033, mobile design status,
  `docs/vibemux_web.md`, and this progress record. No baseline dependency versions
  were removed or changed; no daemon/control/store/A2A/Python source was modified.

## Validation executed
- Runtime: native Windows, Rust/Cargo 1.85.0, Node 22.22.0. Command-local npm cache,
  TEMP/TMP and build directories were used. The local Rust build used verified
  OpenSSL 3.0.17 through command-local OPENSSL_DIR/OPENSSL_NO_VENDOR; vendored CI
  OpenSSL compilation remains unverified. Global runtime/PATH/provider settings
  were not changed.
- `npm ci`: passed using the committed lockfile.
- `npm run typecheck`: passed, 0 errors/0 warnings.
- `npm test`: passed, 10 tests (cache bound/expiry/body limit, bearer partition,
  stale title/marker, push whitelist, cross-generation/refresh consistency).
- `npm run build`: passed; embedded shell/manifest/service worker/assets produced.
- `npm audit --audit-level=low`: passed, 0 vulnerabilities; no audit exclusions.
- `cargo fmt --all -- --check`: passed (exit 0).
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: passed (exit 0).
- `cargo test -p vibemux_web --all-features --locked`: passed, 30 tests total
  (21 module tests, 3 real control-pipe tests, 6 real HTTP/contract tests).
- `cargo test --workspace --all-features`: failed with 10 existing CLI unit
  `ControlRuntimeUnavailable` errors when using the restricted host's default
  LOCALAPPDATA. A single unchanged CLI test passed after assigning a writable
  task-local LOCALAPPDATA, confirming that environment cause.
- `cargo test --workspace --all-features -- --test-threads=2`, with task-local
  LOCALAPPDATA: failed in all 3 existing `vibemux_cli::bootstrap_process` tests
  with `SpawnFailed` (exit 101). Those source files were left unchanged.
- `cargo nextest run --workspace --all-features`: failed at the same default
  runtime-directory issue: 48 passed, 10 failed, 2 skipped; 795 tests not run.
- `cargo nextest run --workspace --all-features --test-threads=2`, with task-local
  LOCALAPPDATA: failed at existing bootstrap-process tests: 58 passed, 2 failed,
  2 skipped; 793 tests not run (exit 100). A skip is not release evidence.
- `cargo deny check`: failed initially on the restricted default advisory cache.
  Re-executed with a temporary config changing only advisory db-path: advisories
  FAILED, licenses FAILED (no repository license allow policy), bans/sources OK.
  No ignored advisories or relaxed policies were added.
- `cargo audit --no-fetch -d <fresh_task_local_rustsec_git_db> --json`: failed
  (exit 1): RSA 0.9.10 RUSTSEC-2023-0071 in the new push crypto chain, plus the
  existing time 0.3.44 RUSTSEC-2026-0009 and rustls 0.23.43 RUSTSEC-2026-0285.
- Production embedding guard, compiled directly and executed against real dist:
  present output passed (exit 0); missing dist rejected (exit 101), then restored.
- Fresh bundled Chromium against the real web binary with an authenticated,
  task-owned bootstrap fixture: phone 390px and desktop 1366px rendered without
  horizontal overflow; stale/unavailable notice, protected shell/API, initial SW
  installation/control passed; 0 page errors and 0 CSP errors. A page-only header
  fixture could not authenticate SW fetches; no public-route exception was added.
- `git diff --check`: passed before local delivery. Worker claims were followed by
  independently observed integrated checks. CI was configured, not executed remotely.

## Platform coverage
- Windows: native feature tests, IPC, HTTP, ACL and viewport/browser fixture evidence.
- Linux/WSL: not executed; Ubuntu CI build configured only.
- Live terminal backend: not applicable; no terminal route or proxy exists.
- Live tailnet/operator bootstrap, installed iOS/Android PWA push and actual vendor
  delivery, offline-device behavior and live SW replacement: unverified.

## Compatibility / migration
- No daemon/control/store/plugin/A2A migration or operation change.
- New pre-alpha API 0.1.0 and private web-state schema 1, with an additive credential
  epoch; older private state without an epoch revokes old subscriptions on startup
  while retaining VAPID keys. No stable API compatibility promise is made.
- Rollback is stopping the web process and removing its config; no canonical state
  is altered. Known web-local state is independently owned.

## Security impact
- Strict authentication, content limits, ACL/link checks, origin-local assets and
  validated/pinned outbound destinations preserve the accepted read-only boundary.
- Push carries no titles/branches/prompts/paths/tokens/free text. Token rotation
  revokes notification recipients as well as future HTTP access.
- The accepted mobile threat model and Track A residual risks are recorded in
  `docs/mobile_access_design.md` §7 and `docs/vibemux_web.md`.

## Remaining work
- Resolve the existing Windows process-bootstrap gate in its own bounded scope,
  establish the dependency/license policy and remediate the reported advisories.
- Qualify CI/Ubuntu, real operator bootstrap/tailnet and installed iOS/Android push;
  M9 review still precedes any release claim. Tracks B/C/D remain out of scope.

## Known risks
- No full-workspace green gate or release-readiness claim. New RSA dependency has
  an unresolved advisory even though VAPID request construction uses P-256.
- Push is best-effort: persisted pre-send claims can lose an uncertain delivery,
  and bounded in-call retries can duplicate at a vendor. No exactly-once claim.
- Shared bearer rotation is deliberately coarse. The auth limiter is global and
  can temporarily deny valid clients after enough failed attempts.
- The strict bootstrap prerequisite is real; the temporary authenticated test
  fixture is not a shipped bootstrap service or proof of an operator deployment.

### 2026-10-12 - Decision port and advisory harness routing (ADR 034, `feat/decision_port`)

**Scope and implementation**
- Status: `PARTIAL — FIXTURE ONLY`. New pure crate `vibemux_decision`
  (typed `harness_route` question, enumerated facts, answer validation,
  local label rules, the fallback resolver, config schema 1, content-free
  records and events, peer wire) and the one-shot `vibemux_decision_peer`
  binary, the only decision-API network client (TypeSafe Jev System One,
  pinned model `jev-1.13.0`, redirects refused, no proxy, 16 KiB caps, one
  429/529 retry inside a total budget).
- Store schema 6 adds `decision_records` with `decision_recorded` and
  `decision_consumed` events written by the single `WriterWorker`.
  `admit_decided_dispatch` checks the binding, admits the dispatch, and
  consumes the decision in one Immediate transaction.
- `vibemuxd` decision service: config pinned at start; options are the
  detected harnesses with an enabled executing route plus `escalate`; the
  peer gets a cleared environment plus one credential variable, only when
  both `jev.enabled` and `--allow-external` are set; one external exchange
  at a time, and a concurrent request records `decision_peer_busy` instead
  of queueing; daemon shutdown cancels the exchange and reaps its peer
  before the writer is released. Control v8 adds `decision_advise`,
  `decision_list`, `decision_show`, and `decision_dispatch_submit`; v1-v7
  clients keep their operations.
- `vibemuxctl decide route`, `decisions list|show`, and
  `dispatch submit --decision`. The roadmap for later decision-driven
  features is [docs/decision_orchestration_roadmap.md](docs/decision_orchestration_roadmap.md).

**Validation executed** (native Windows 11, pinned Rust 1.85.0)
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
  --all-features -- -D warnings`, and `git diff --check` passed.
- `cargo test --workspace --all-features --no-fail-fast` after the pull
  request review repairs: 923 passed, 0 failed, 2 ignored in 103
  result targets. Decision targets: `vibemux_decision` 47 unit tests;
  `vibemux_decision_peer` 4 unit, 13 loopback-HTTP client tests, and 4
  binary tests run under the daemon's exact environment without `PATH`;
  store `decision` 8; `vibemuxd` `decision_route` 10. With the control
  shutdown's peer cancellation removed, the new daemon shutdown test failed
  because the advice did not return within its limit.
- Offline end to end against the real daemon and CLI built from `8f22024`,
  in a scratch project outside the repository (schema 6 observed), with
  `vibemux_native_fixture` vendors and `vibemux_decision_fixture` as the
  peer. Passed: a local-rule decision; an unknown label refused; escalation
  with no default; project-default fallback after `switch`;
  `--allow-external` with Jev disabled recorded as `disabled`; `decisions
  show` output held neither the prompt, its digest, nor a path; list
  pagination; `--decision` with an explicit harness refused; another prompt
  refused with `decision_prompt_mismatch`; an escalated decision refused; a
  decided submit admitted and completed; the same request id replayed as a
  duplicate; another request id refused with `decision_already_consumed`.
  With Jev enabled and a dummy credential variable, the fixture answer was
  accepted from the pinned model with gated confidence 0.955 and the dummy
  value appeared in no output; without `--allow-external` the backend was
  `not_requested`; a daemon started without the variable recorded
  `decision_credential_unavailable` and used the local rule. This run
  predates the review repairs below, which are covered by the targeted and
  workspace tests instead.
- Independent read-only review of `a639ed5..8f22024` found no critical or
  high defect. Repaired with regressions: an advice could wait one peer
  deadline for the external permit and then run a second, outliving the
  client deadline and starting a paid call nobody received (now
  `decision_peer_busy`); a relative peer path was not refused; system
  variable names such as `PATH` were accepted as `credential_env`; a wall
  clock stepped behind the creation time extended the 15-minute window; the
  retry attempt could outlive the peer's total budget; and three doc claims
  overstated the code.
- The pull request review of `571152d` raised two more defects, both
  repaired with regressions:
  - The decision events could not rebuild `decision_records`. Now
    `decision_recorded` holds the complete sanitized record and
    `decision_consumed` holds its consumption time. A store test rebuilds
    the projection from the event log alone.
  - Daemon shutdown neither cancelled nor reaped an active peer, and a
    control shutdown drained the connection for up to the 50-second peer
    deadline. Now the peer is cancelled at once and reaped before the
    writer is released, and the advice records `decision_peer_cancelled`.
    This is covered at the service level and through both daemon shutdown
    paths.
- Building `aws-lc-sys` with MSVC failed with `D8050` under the long
  worktree target path; a short `CARGO_TARGET_DIR` outside the worktree
  built cleanly. One workspace run failed to compile because the build
  drive filled up; only the task's own incremental cache was removed before
  the rerun, and that failed attempt is not counted.

**Compatibility and remaining work**
- Store schema 6 and Control v8; older clients and the daemon must upgrade
  together for the decision operations. Decisions stay off unless
  `.vibemux/decision_config.json` exists, and the default config never
  calls the provider.
- No live Jev call has run; it needs the user's explicit authorization (paid
  API and key). The default threshold 0.9 is policy, not calibrated. HTTPS
  from the peer under the cleared environment is unverified because tests
  use loopback HTTP.
- Not implemented: redacted task-summary forwarding, other decision kinds
  and backends, per-repository threshold fitting, and roadmap items 3-10.

### 2026-10-12 - Release the empty Windows runtime leaf on clean daemon shutdown

**Scope and implementation**
- Problem: on Windows, every daemon start creates the per-project leaf
  `%LOCALAPPDATA%\VibeMux\runtime\<project hash>` (`ensure_runtime_dir` and
  `start_with_writer_locks`). Nothing removed it. The owner-matched guards
  remove only the descriptor and the writer lock. Every clean start/stop
  cycle therefore left one empty leaf. One machine held about 580 of them,
  and each workspace test run added more.
- Keeping the empty leaf was not intentional. ADR 020 puts the DACL and the
  `.acl_v1` marker on the shared root, and leaves only inherit them. The
  leaf holds no state of its own, and the next start recreates it.
- `crates/vibemuxd/src/process.rs`: new
  `DaemonPaths::release_runtime_dir()` (Windows). It removes the leaf only
  when the leaf is an empty real directory at its canonical path. A
  symlink, junction, or other reparse point is never followed or removed,
  and neither is a path reached through one. A non-empty leaf is left
  untouched. `RemoveDirectoryW` refuses a leaf that became non-empty after
  the check. The release is best-effort and returns nothing. A kept leaf
  costs one directory that the next start reuses, and the daemon has no
  channel left to report it: the writer is joined, and a CLI-started
  daemon's stderr is discarded. A panic in the blocking task is contained
  and never turns a clean shutdown into an error.
- `crates/vibemuxd/src/control.rs`: the trusted Windows start (`security`
  is `Some`) calls it after `serve_owned` returns Ok, once the descriptor
  guard is dropped. The call runs through `spawn_blocking`. By then
  `serve_owned` has joined the writer, which removed the nonce-matched
  writer lock. The following keep the leaf as before:
  - a shutdown error;
  - a failed start, whose leaf the CLI's `verify_control_surviving_acls`
    post-mortem needs;
  - a crash;
  - raw-path constructors, whose runtime directory is arbitrary;
  - POSIX, where the runtime directory is the project state directory.
- Test fixtures:
  - The two supervisor fixtures now assert that the leaf was released
    instead of removing it themselves.
  - One recovery test recreates the leaf before simulating a crashed
    daemon's stale descriptor.
- Review follow-up for the restart race (see Known risks):
  - `crates/vibemux_cli/src/lib.rs`: `stop_daemon` now also waits, under the
    same `DEFAULT_SHUTDOWN_TIMEOUT` deadline, until the daemon PID from the
    health response has exited. It reuses `recovery::process_is_present`,
    which is now `pub(crate)`. An in-process server sharing the CLI's PID
    (tests) is not awaited.
  - `classify_daemon_exit` (Windows) treats an absent runtime leaf as the
    plain `daemon_start_failed`. Only the root and the `.acl_v1` marker are
    then verified, through the new strict
    `DaemonPaths::verify_control_root_acls()`. Every path that exists stays
    fail-closed, including a missing control root.
- Docs: `docs/architecture.md` (Windows control metadata paragraph) and
  `CHANGELOG.md`.

**Validation executed** (native Windows 11, pinned Rust 1.85.0/MSVC, short
`CARGO_TARGET_DIR` outside the worktree)
- Regression tests, run before the fix:
  `cargo test -p vibemuxd --all-features --test daemon_process` gave
  4 passed and 2 failed. `clean_shutdown_releases_the_empty_runtime_leaf`
  failed with "cycle 0: the empty runtime leaf must be released", and
  `standalone_daemon_owns_rust_database_and_shuts_down_cleanly` failed on
  `!paths.runtime_dir().exists()`. After the fix both pass, together with
  the new `clean_shutdown_keeps_a_runtime_leaf_with_foreign_content` and
  the unit tests `release_runtime_dir_removes_only_an_empty_leaf` and
  `release_runtime_dir_never_follows_or_removes_a_junction`. The junction
  test covers a junction at the leaf and a junctioned control root.
- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`:
  passed.
- `cargo test --workspace --all-features` before the fix: 824 passed,
  0 failed, 2 ignored in 93 targets. The shared runtime root grew from 582
  to 583 entries, one new empty leaf. Its timing is consistent with the
  `vibemux_frontend` unit tests, whose `supervisor_client` test starts and
  stops a trusted daemon without a cleanup guard.
- After the fix, run in the shared root: 828 passed, 0 failed, 2 ignored in
  93 targets. One new empty leaf still appeared, but another session was
  concurrently running `cargo test --workspace` from a different target
  directory without this fix. The shared-root count therefore cannot
  attribute that leaf.
- After the fix, run with `LOCALAPPDATA` set to a private empty directory:
  828 passed, 0 failed, 2 ignored in 93 targets. The private runtime root
  afterwards held only `.acl_v1` and zero leaves.
- Review follow-up, run after merging `origin/main` (decision port):
  - The new absent-leaf case in
    `classify_daemon_exit_distinguishes_security_failures` failed against
    the old classification. It reported
    `Control { code: "daemon_control_runtime_security_invalid" }` where
    `StartFailed` was expected. It passes with the fix.
  - `abrupt_process_recovery_preserves_database_and_allows_restart` now
    asserts that the daemon PID is gone as soon as `stop_daemon` returns.
  - `cargo fmt --all -- --check` and
    `cargo clippy --workspace --all-targets --all-features -- -D warnings`
    passed.
  - `cargo test -p vibemux_cli`: 30 + 3 passed. `bootstrap_process` is gated
    on `test_helpers`, so it reports 0 there. With `--all-features --test
    bootstrap_process` it gave 3 passed.
  - `cargo test -p vibemuxd --all-features --test daemon_process --test
    decision_route`: 6 and 10 passed.
  - `cargo test --workspace --all-features` with a private `LOCALAPPDATA`:
    927 passed, 0 failed, 2 ignored in 103 targets. Afterwards the private
    runtime root again held only `.acl_v1` and zero leaves.

**Compatibility / remaining work / Known risks**
- There are no wire, schema, or CLI argument changes. `vibemuxctl daemon
  stop` now returns only after the daemon process has exited, so it can take
  slightly longer.
- Leaves left by crashes, `daemon recover`, failed starts, or earlier
  versions are not pruned. That includes the existing leaves on the
  observed machine, about 76 of which still hold stale artifacts. An
  explicit, confirmation-bound pruning of orphaned leaves is possible
  follow-up work.
- The restart race is closed by the 2026-10-13 follow-up below. Leaf
  ownership is now bound to the generation that holds the compatibility
  lock, so a stopping daemon can no longer remove a leaf that a concurrent
  start is using. Before that follow-up, a start between an old daemon's
  descriptor removal and its leaf removal could lose its recreated leaf.
  It then failed with `daemon_start_failed`, which the CLI's post-mortem
  had first misreported as `daemon_control_runtime_security_invalid`.
- Same-user replacement of the leaf by a junction between the check and
  the removal would at most delete that junction itself, never its target.
  Same-SID processes are trusted by ADR 020.
- Linux/WSL was not run. The POSIX path is unchanged by construction: the
  release code is Windows-only.

### 2026-10-12 — Native MCP frontend and dependency qualification (ADR 035)

**Status: PARTIAL — required local gates passed on native Windows;
independent programme acceptance and other platform qualification pending.**

The additive `vibemux_mcp` crate implements MCP 2025-06-18 over newline-delimited
stdio. Its foreground session keeps a default-denied metadata grant, validates
fixed schemas before producer I/O, and supervises one contained same-executable
reader using existing `ControlClient` read methods. Five tools expose host status,
daemon health, task pages, task/run/event/artifact metadata and dispatch status.
Explicit projections omit worktree/branch, prompt, source, capture, process and
credential fields. Declared daemon status does not establish model completion.

Reader errors, crash, malformed output and timeouts unload daemon-backed tools;
cancellation suppresses the cancelled response and EOF joins owned cleanup.
Generation/sequence checks fence stale completions. The session has four launch
attempts total (initial plus three restarts), with no automatic replay or budget
replenishment after a successful read. Initialization, discovery and host status
have no descriptor, reader-spawn or producer side effect. Control IPC v7 and
store schema 5 remain unchanged. The Hub companion retains its separate
multi-producer responsibility.

**Approved compatibility prerequisites.** Rust workspace `0.2.0-alpha.1` uses
MSRV/pinned compiler/Windows-Ubuntu CI target 1.92. Egui/eframe 0.34.3 and ratatui
0.30.2/crossterm 0.29 replace the old unmaintained dependency chains; time 0.3.47
and rustls 0.23.45 retain the security fixes. Root `deny.toml` contains the exact
ten-license policy; no advisory ignore, source exclusion or weakened lint was
introduced. The frontend root-`Ui` migration preserves themes, action ownership,
layout and the existing placeholder-color assertion. A strict frontend-only
loopback port override isolates TUI/dump CLI fixtures without changing probe-core
or default runtime behavior.

**Focused verification completed on native Windows with Rust 1.92.0.**

- `cargo test --locked --offline -p vibemux_mcp --all-features`: 27 tests passed,
  zero failed or ignored. This includes real stdio, disposable daemon/control
  IPC, legacy-v1 health and unsupported-version rejection, framing properties,
  bounds, permission reduction, cancellation, busy responses, expired-backoff
  fairness, restart exhaustion, stale generations, actual child/descendant
  containment and retained-child terminal observations.
- Subsequent reviews identified path-shaped artifact media labels and non-object
  successful private-reader results. Each regression failed before repair and
  passed afterward. The integrated focused suite then passed 29 tests. This
  result precedes the additional launch-ownership repair described below.
- `cargo test --locked --offline -p vibemux_frontend --all-features`: 122 tests
  passed, zero failed, with one existing ignored golden-fixture generator. No
  existing assertion or golden rendering fixture was removed or weakened.
- `cargo fmt -p vibemux_mcp -p vibemux_frontend -- --check`: passed.
- `cargo build --locked --offline -p vibemux_frontend --features gui_screenshot
  --bin vibemux_frontend_preview`: passed. Eight synthetic native captures were
  inspected across light/dark chat, minimum-size drawer/welcome/disconnected
  views, collapsed sidebar at 1.5 scale, and independent task/terminal views.
  These used demo data; they establish no live daemon or harness acceptance.
- The dependency policy/audit gates passed on the migrated graph. Necessary
  Rust 1.92 compatibility changes preserve condition order, assertions and
  defaults; strict workspace/all-target/all-feature Clippy passed with
  `-D warnings` unchanged. No warning was disabled.
- The first workspace test run stopped after an existing plugin-supervisor
  `HandshakeTimeout`; the unchanged exact case passed alone. A later
  no-fail-fast run passed 853 tests across 82 completed binaries, with no
  failures and two existing ignored test utilities, but reached its 30-minute
  external limit before all targets and doc tests completed. It is an incomplete
  gate. Compilation consumed 15m47s; final validation reuses the warm target.
- Every Python gate passed in the isolated qualified development environment:
  Ruff formatting/lint, mypy (43 source files), pytest (111 tests) and the
  documented mock-workflow smoke. These do not establish native Rust acceptance.

**Additional launch-ownership repair verified.** Independent source review
found that synchronous OS launch could block the foreground's first read poll.
A controlled slow-launch regression reproduced a 2.38-second ping delay and
failed before implementation. The repair retains a dedicated launch thread and
uses explicit lifecycle states: cancellation unloads tools promptly while any
eventual child remains owned until tree termination and reap. Host-only requests
remain available during cleanup; replacement waits for terminal cleanup and a
complete backoff. A stalled OS launch may delay final exit, which must not be
misreported as bounded or complete cleanup. The final focused suite passed
39 tests, including 22 lifecycle cases; production-default compilation and
strict scoped Clippy passed. Independent read-only review confirmed the
launch-blocking finding closed. Parent workspace formatting and strict
all-target/all-feature Clippy also passed on the final source.

**Final workspace test result.** `cargo test --locked --offline --workspace
--all-features --no-fail-fast -- --test-threads=2` completed with exit 0 on
native Windows at 03:40 on 2026-10-12: 865 passed, zero failed, two existing
test utilities ignored, no filtered tests. All 83 test binaries and 18
doc-test phases completed within the 30-minute bound (9m36s elapsed,
51.52s compilation). Earlier failed and incomplete runs remain recorded above.
`cargo nextest run --locked --offline --workspace --all-features --test-threads
2 --no-fail-fast` also completed with exit 0 at 03:47:31: 865 passed, two
existing utilities skipped, two slow workflow tests passed, 395.235s test time.
No tests were excluded or retried to obtain these passes.

**Documentation and CI reconciliation.** The repository-root launch recipes now
select the production binary through `cargo run --locked -p vibemux_mcp --bin
vibemux_mcp -- ...`. CI retains both Windows/Ubuntu Rust jobs and now runs
nextest with pinned 0.9.98; a separate Ubuntu job runs deny 0.20.2 and audit
0.22.2. YAML parsing, gate-presence inspection and independent source review
passed. Hosted installation and execution of these CI jobs remain unverified.

**Failure chronology.** The first stdio test failed against the empty host before
its denied initialization/status path passed. Subsequent contemporary reds
covered granted discovery, bounded projections, real reader operation, restart
exhaustion, owner denial, cancellation/EOF/idle exit, recovery scheduling, path
bounds and standard `_meta` compatibility. The frontend migration first failed
compilation, then exposed the actual placeholder-color regression before its
unchanged assertion passed. The loopback-port adapter had its own behavioral red.
Two fixture corrections are kept separate from product failures: A2A setup must
use an empty task description, and a split duplex writer needs explicit shutdown
to signal EOF. Additional property, compatibility and process-cleanup coverage
that passed on its first run is recorded as coverage, not retrospectively claimed
as test-first evidence.

**Remaining acceptance.** Independent programme review and operator acceptance.
Native Linux/WSL process behavior, the configured Ubuntu CI job, live
MCP deployment and operator acceptance remain unverified. No live model call,
daemon installation, publication, merge or deployment is implied by these tests.
Rollback removes the additive host/client launch without a state migration;
rolling back the dependency migration restores the previous compiler/frontend
versions together. Historical evidence elsewhere in this document is unchanged.

### 2026-10-13 - Bind the Windows runtime leaf to its compatibility-lock generation

**Scope and implementation**
- Codex review on PR #17 (P2, `crates/vibemuxd/src/process.rs`): the
  2026-10-12 leaf release could remove the leaf of a start that ran
  concurrently with a stop. AGENTS.md invariant 13 forbids removing a
  resource the caller does not own, so a known-limit note was not enough.
- Ownership rule: only the holder of the project's compatibility lock may
  create, validate, or remove the per-project runtime leaf. That lock is
  `legacy_writer_lock_path()`, the existing upgrade gate, with the same
  file and contents. A trusted Windows start always has one. It is held
  for the daemon's whole life, as before.
- `crates/vibemuxd/src/lib.rs`:
  - New `pub(crate)` `CompatibilityLock`, an `Arc`-shared `LifecycleLock`.
  - `WriterWorker::start_with_held_compatibility_lock` takes a lock the
    caller already holds and acquires only the primary lock. The writer
    thread keeps its reference until it has closed the database and
    dropped the primary lock. The writer therefore never opens the
    database without both locks, even when a failed start detaches it.
  - `Drop` and the release now share the nonce check
    (`LifecycleLock::is_held`).
- `crates/vibemuxd/src/control.rs` (`start_with_writer_locks`), start order:
  1. A read-only check for an existing descriptor. It keeps a live or
     crashed daemon reported as `control_descriptor_exists` ahead of its
     locks.
  2. The compatibility lock, when it is distinct from the primary lock,
     acquired before the leaf is created. A held lock fails the start with
     the existing `writer_lock_held`.
  3. Leaf creation, canonicalization, and the authoritative descriptor
     check.
  4. The writer, which acquires the primary `writer.lock` inside the leaf.
- Shutdown order of a clean trusted Windows start:
  1. The writer is joined, which removes the primary lock.
  2. The descriptor guard is dropped.
  3. `release_runtime_dir()` runs, still for an empty real leaf only. It
     runs only while the compatibility lock carries this generation's
     nonce (new `release_owned_runtime_dir`).
  4. The compatibility lock is dropped.
  The lock moves into the shutdown task and then into the blocking release
  closure. A shutdown error, a join error, or a cancelled release therefore
  still drops it, after everything else. On failed starts the writer is
  joined first and the lock is dropped when the start returns. The leaf is
  kept, as before.
- Unchanged: POSIX, where `has_distinct_legacy_runtime()` is false and no
  compatibility lock is taken, and raw-path constructors without a
  compatibility lock. `start_with_writer_and_compatibility_lock`, the one
  raw-path constructor with a compatibility lock, has no callers. It now
  also takes that lock first, and it never releases a leaf.
- Test hook: `vibemuxd::control::test_hooks` exists on Windows with
  `test_helpers` only. It pauses a clean shutdown right before the leaf
  release. It is keyed by the leaf path, so parallel tests never share a
  gate, and it lets the shutdown continue by itself after 30 s.
- `crates/vibemux_cli/src/lib.rs`: comment only. `stop_daemon`'s artifact
  loop already waits for the compatibility lock, because `select_runtime`
  reports artifacts while it exists. The lock now goes only after the
  release. The PID wait from the 2026-10-12 follow-up stays.
- Docs: `docs/architecture.md` (Windows control metadata paragraph),
  `CHANGELOG.md`, and the 2026-10-12 Known risks above. ADR 020 and ADR 025
  do not describe the lock order and are unchanged.

**Validation executed** (native Windows 11, pinned Rust 1.85.0/MSVC, short
`CARGO_TARGET_DIR` outside the worktree, private `LOCALAPPDATA`)
- New tests in `crates/vibemuxd/tests/daemon_process.rs`. Each was run
  against 414c317's `control.rs` and `lib.rs` with only the test hook added:
  3 failed and 6 passed. All pass after the fix. They synchronize through
  channels with deadlines and use no sleep-based race.
  - `a_start_inside_the_leaf_release_window_is_refused_while_the_leaf_is_owned`
    (`test_helpers`). It pauses a clean shutdown between its descriptor
    removal and its leaf release. It then runs `ensure_runtime_dir` and
    `start_for_paths`, as the daemon binary does. Before the fix the second
    start succeeded inside the window ("a start inside the release window
    must be refused"). After the fix it gets `writer_lock_held` while the
    compatibility lock still exists. The release then completes, and a
    retry starts and releases its own leaf.
  - `a_start_refused_by_a_held_compatibility_lock_never_touches_the_leaf`.
    Before the fix the refused start created the missing leaf ("a start
    that does not own the compatibility lock must not create the leaf").
    After the fix it gets `writer_lock_held` without creating the leaf.
    A leaf that already exists is kept, empty. The database is never
    opened, and the held lock file is unchanged.
  - `clean_shutdown_keeps_the_leaf_once_the_compatibility_lock_is_not_its_own`.
    Before the fix the shutdown removed the leaf. After the fix the leaf
    and the foreign lock are both kept.
- The first run after the fix failed
  `abrupt_exit_leaves_owned_artifacts_for_fail_closed_diagnosis`. A
  replacement start after a crash reported `writer_lock_held` instead of
  `control_descriptor_exists`, because the stale compatibility lock was now
  checked first. The read-only descriptor check in start step 1 fixes this,
  and the test passes unchanged.
- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`:
  passed.
- `cargo test -p vibemux_cli`: 30 + 3 passed. With `--all-features --test
  bootstrap_process` it gave 3 passed, including the PID-gone assertion.
- `cargo test -p vibemuxd --all-features --lib --test daemon_process --test
  decision_route`: 123, 9, and 10 passed.
- `cargo test --workspace --all-features`, with `LOCALAPPDATA` set to a new
  empty private directory and no other build running: 930 passed, 0 failed,
  2 ignored in 103 targets. That is the earlier 927 plus the 3 new tests.
  Afterwards the private runtime root held only `.acl_v1` and zero leaves.
- The workflow test `a_restart_stops_the_workflow_and_the_resumed_run_repeats_no_completed_turn`
  has been seen failing under load on another host. It passed here.

**Compatibility / remaining work / Known risks**
- There are no wire, schema, CLI argument, or lock-file format changes. A
  stale or live descriptor is still reported first. A held compatibility
  lock now fails the start before the leaf is created, not after.
- A start that runs concurrently with a stop fails closed and can be
  retried. While the stopping generation holds the lock, it fails with
  `writer_lock_held`. It fails with `daemon_control_runtime_unavailable`
  when the release removes the leaf while the new process's pre-lock
  `ensure_runtime_dir` is still validating it. That pre-lock call stays
  because it bootstraps the shared root and its ACL marker. Neither case
  loses state.
- Only the compatibility lock exists in two short windows: at start,
  before the writer takes the primary lock, and at shutdown, during the
  release. `select_runtime` reports artifacts in both. `stop` waits through
  them, and a CLI `start` waits for health instead of spawning, as before.
- If an external same-user process deletes the compatibility lock while a
  daemon runs, another start can acquire it. The running daemon then keeps
  its leaf on shutdown, as pinned by
  `clean_shutdown_keeps_the_leaf_once_the_compatibility_lock_is_not_its_own`.
  The nonce check and the removal are not atomic against a process that
  deletes and recreates the lock in between. ADR 020 trusts same-SID
  processes.
- Linux/WSL was not run. The release and the test hook are Windows-only.
  On POSIX no compatibility lock is taken.

### 2026-10-13 - Take a DSH poll page's status from its records' snapshot

**Scope and implementation**
- Found while diagnosing the flaky DSH integration test fixed on the test
  side in PR #19. `DshSessionService::poll` read the page's records with
  `DshDriver::poll`, then called `refresh()`. That read `summary()` and
  `transport_open()` under two more acquisitions of the driver's state lock.
  The runtime reader appends each record and advances the session machine
  under that lock, so it could run between the reads. A page could then
  carry a status that counted records the page did not reach, including a
  settled turn without its `TurnEnd`.
- `DshDriver::poll` already took its `summary` under the same lock as the
  records. The service discarded it.
- `crates/vibemux_dsh/src/driver.rs`: `DshPoll` gains `transport_open`, read
  under the same lock. Its doc states the invariant: when `more` is false,
  `next` equals `summary.records`.
- `crates/vibemuxd/src/dsh_session.rs`: `poll` builds the page status from
  `poll.summary` and `poll.transport_open`. `apply_summary` now takes a
  summary and a transport flag instead of the driver. The other operations
  return no records and still refresh from the driver.
- Consumers checked:
  - `vibemuxctl dsh poll` (`poll_to_tail`) stops on `more == false`, not on
    the status. It prints the last page's status beside the records, so
    before the fix it could print a settled turn whose `TurnEnd` it had not
    printed. No CLI change.
  - `vibemux_dsh_tui` drives the driver directly and already used
    `poll.summary`. Unchanged.
  - No other caller of `dsh_session_poll` exists outside tests.
- Test fixture: `vibemux_dsh_fixture` gains a `flood` mode. It streams 400
  text chunks in one buffered burst before each turn ends.
- Docs: `docs/dsh_session_protocol.md` (Control IPC v7 snapshot contract and
  stop rule; CLI), `docs/architecture.md`, `CHANGELOG.md`.

**Validation executed** (native Windows 11, pinned Rust 1.85.0/MSVC, short
`CARGO_TARGET_DIR` outside the worktree)
- New test `a_poll_page_status_never_runs_ahead_of_its_records` in
  `crates/vibemuxd/tests/dsh_session.rs`. It runs 20 `flood` turns and polls
  without sleeping. On every page with `more == false` it asserts that
  `next == status.records` and that `status.completed_turns` equals the
  `TurnEnd` records read so far.
  - Against the unfixed service, with only the fixture and the test added:
    10 of 10 runs failed, each within turns 1-4, with the status one record
    ahead of the page (for example 185 against 186). An earlier variant,
    with 3 turns of 4,000 chunks, failed 7 of 10 runs. The final test
    failed 5 more of 5 runs against the old `dsh_session.rs`.
  - With the fix: 20 of 20 runs passed, 1.3-2.1 s each.
- Driver tests now assert `next == summary.records` on a drained page, and
  check `DshPoll.transport_open` for a live runtime and an exited one.
- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`:
  passed.
- `cargo test -p vibemuxd --all-features --test dsh_session`: 10 runs, each
  10 passed.
- `cargo test -p vibemuxd --all-features --lib`: 123 passed.
- `cargo test -p vibemux_dsh --all-features`: 6 + 12 passed.
- `cargo test -p vibemux_cli --all-features`: 30 + 3 + 3 passed.
- `cargo test -p vibemux_frontend --all-features`: all passed, 1 ignored.

**Compatibility / remaining work / Known risks**
- No wire change: `DshSessionPollPage` keeps its fields. The contract is
  stronger, because a page's status now agrees with its records.
  `DshPoll`, a `vibemux_dsh` Rust type that is not on the wire, gains a
  public field. Its only constructor is the driver.
- The new test detects the old ordering only when the reader runs on
  another thread while a poll is served. That is likely but not certain on a
  small runner.
- `status`, `prompt`, `cancel`, and `close` still read `summary()` and
  `transport_open()` under separate acquisitions. They return no records, so
  they cannot run ahead of records, but one could briefly show
  `transport_open: false` with a non-terminal phase.
- The PR #19 test helper's comment says a page reads its status after its
  records. That is no longer true. The helper still drains until
  `more == false`, which stays correct.
- Observed and not changed: `fit_page` re-encodes a page once per trimmed
  record. A full page of small records took about 250 ms in a debug build.
- Linux/WSL was not run.

### 2026-10-17 - Native harness dispatch and unified CLI integrated (ADR 040/041)

**Scope**
- Integrates `codex/harness_request_dispatch` (identical to `codex/harness_request_dispatch_latest`, commit `a955484`, base `6498e6a`) into `main` after the 2026-09-27 audit left it unmerged. The user asked for all of the branch's functions to be kept, so this is a port, not a record-only merge.
- The branch's crate name collided with main's pure-logic registry crate, so it is renamed before merging: crate `vibemux_harness_native`; binaries `vibemux_harness_native`, `vibemux_harness_native_plugin`, `vibemux_harness_native_fixture`. Main's `vibemux_harness` (registry, ADR 029 dispatch protocols, DSH substrate) is unchanged.
- The branch's ADR 023 and ADR 024 are renumbered to [ADR 040](docs/adr/040_native_harness_request_dispatch.md) and [ADR 041](docs/adr/041_unified_cli_and_workspace_layout.md), because main's ADR 023/024 are the daemon plugin registry and the stateful A2A supervisor. The branch's "M4.2"/"M4.3" labels are not main milestones; the records below keep them as historical names only.

**Integration decisions**
- Two dispatch paths now coexist and are independent. `vibemuxctl dispatch` (ADR 029) is daemon-owned, admitted, and recorded in the single-writer store (schema 9). `vibemuxctl harness` (ADR 040) runs bounded request dispatch in the CLI process from `.vibemux/` configuration with SHA-256 pinning; it writes no canonical state, takes no daemon admission, and is not a Task/Run.
- `vibemuxctl` keeps main's lifecycle client and control-verb contracts (`daemon`, `harnesses`, `switch`, `dispatch`, `decide`, `decisions`, `dsh`, `workflow`, `slots`, `context`, `session`, `prompt`) and gains the branch's `status`, `workspace status`, `probe`, `ui`, and `harness` commands plus `--format auto|text|json`. The branch's `daemon_client.rs`, `error.rs`, and `parser.rs` were an extraction of code main already has in a richer form and are not carried over. Control verbs keep their JSON output and their actionable `error` text; `daemon` prints text on a terminal and JSON when piped.
- The branch carried its own Windows Job Object FFI and a POSIX `kill` call. `main` confines `unsafe` to reviewed `vibemux_platform` modules and requires `#![forbid(unsafe_code)]` in every other library crate (pinned by a test), so the native process now uses `vibemux_platform::ProcessTree` (ADR 029 section 6) and the crate forbids unsafe code. Behavior is unchanged: kill-on-close Job Object on Windows, process group on POSIX. Three `harness_job_*` error codes become `harness_containment_failed`.
- The branch's CI step `scripts/check_public_artifacts.py` reported 13 findings on `main`'s own content. All were synthetic (fake credentials in rejection and redaction tests, RFC 1918 and CGNAT samples in address-policy tests, one elided `C:\Users\...` path in this ledger). The script's single-entry exception table now holds 14 exact-line hashes; no detection rule was changed.
- The branch dashboard is namespaced as `vibemux_frontend::dashboard` with its own `vibemux_dashboard` binary. Main's egui `vibemux_frontend` GUI, `vibemux_frontend_tui`, and `vibemux_dsh_tui` are untouched.

**Evidence labels**
- Live-vendor evidence for the native slice is the branch's own 2026-08-27 record (initialize plus clean exit for Codex, Claude, OpenCode, and Copilot; Grok shutdown unverified; no model generation). It was not re-run during this integration, and no vendor CLI, Jev, or paid API was invoked.
- The branch's Windows counts below predate the merge and are not evidence for the merged tree.

#### Branch record: unified CLI and workspace layout (labelled M4.3 on the branch, 2026-08-28)

- `vibemuxctl` now groups daemon, harness, probe, status, workspace, and UI
  commands. Legacy daemon exports/JSON and standalone binaries are retained.
- CLI parsing, lifecycle I/O, application orchestration, Git inventory, and
  output formatting are separate modules. Frontend model, navigation,
  rendering, and terminal ownership are also separate.
- The CLI and frontend share a bounded saved-report validation/redaction
  boundary in `vibemux_probe::snapshot`. Saved data is labeled, endpoint details
  are omitted, and control characters cannot become terminal instructions.
- The UI has Overview, Harnesses, and Workspace sections with bounded
  selection, help, and offline snapshots. It does not launch native TUIs,
  approve tools, start daemons, or invoke models through key presses.
- Guides, workflow policy, dated reports, and harness examples have distinct
  directories. Existing worktree locations and historical branches are
  preserved; build caches are ignored rather than deleted.
- Rust package version advances to `0.2.0-alpha.1`; no database, A2A, or plugin
  wire version changes. See [ADR 041](docs/adr/041_unified_cli_and_workspace_layout.md)
  and the [unified CLI guide](docs/guides/unified_cli_guide.md).
- Branch-time Windows validation: 160 Rust tests, 30 Python tests, Clippy, formatting,
  Mypy, mock smoke, and the public-artifact scan passed. Actual offline terminal
  sessions covered navigation, 80 x 24 layout, source labels, and normal exit.
  Git filter side effects are disabled and explicitly qualified in the output.
  [Current evidence](docs/reports/workspace_cli_validation_20260828.md) records
  the environment corrections, independent review, Git state, and limits.
- Nextest, cargo-deny, and cargo-audit are not installed locally; their declared
  additional gates remain unverified. Secondary-remote synchronization could
  not be refreshed. These limits are not release-gate waivers.

The native harness slice evidence below is a historical snapshot. New local
checks and any unavailable tooling gates are recorded separately for the
unified CLI; neither slice claims Linux qualification, production deployment,
or model-generation success.

#### Branch record: local native harness slice (labelled M4.2 on the branch, 2026-08-27)

- Added typed text requests, complete JSON record retention, native protocol
  sessions, clean environment launch, and a bounded dispatcher using M4.1.
- Added independent terminal-evidence checks, configuration-digest binding,
  duplicate/resource admission, cancellation-aware backpressure, and default
  content-free reports. No canonical database contract changed.
- Focused Windows verification: 37 Rust tests passed, including five synthetic
  native routes and malformed/forged/blocked/cancelled cases.
- Live initialization plus clean exit passed: Codex 0.149.1, Claude 2.1.224,
  OpenCode 1.18.21, and Copilot 1.0.75. Grok 0.2.114 received two JSON records but
  failed the shutdown deadline; that route remains live-unverified.
- Real model generation was not authorized or executed. No Linux run, remote
  A2A task, deployment, commit, or publication is claimed for this slice.
- The public `origin/main` and secondary `linux/main` differ; feature-ref and
  language evidence is in [the branch inventory](docs/reports/branch_inventory_20260827.md).

The branch named a loopback A2A task through daemon-owned admission and the
single writer as its next boundary. This slice does not implement it; main's
daemon-owned ADR 029 dispatch is the separate path that exists. Details and
non-goals are in
[ADR 040](docs/adr/040_native_harness_request_dispatch.md). The separate
[learning path](docs/guides/a2a_implementation_learning_path.md) is not an automatic
execution or deployment plan.

Branch-time local regression: 140 Rust workspace tests and 30 Python tests passed,
along with Clippy, formatting, mock smoke, and the public-artifact gate.
Exact commands and live qualification boundaries are recorded in the
[validation record](docs/reports/harness_validation_20260827.md).

#### Merge-time validation (2026-10-17, native Windows, Rust 1.92.0)

Run on the integrated tree before it was committed; `CARGO_TARGET_DIR` was a
scratch directory outside the repository.

- `cargo fmt --all -- --check`: clean.
- `cargo clippy --locked --workspace --exclude vibemux_web --all-targets --all-features -- -D warnings`:
  clean after fixing four findings in the merged code (two `collapsible_if` and
  two `cmp_owned`).
- `cargo test --locked --workspace --exclude vibemux_web --all-features --no-fail-fast`,
  final run on an otherwise idle machine: 1156 passed, 2 failed, 3 ignored.
  - `vibemuxd` `workflow_cooperate::two_tracks_build_taskboard_with_a_scoped_bundle_and_isolated_integration`
    fails on the unmodified main checkout with the same assertion
    (`workflow_cooperate.rs:172`, overlap of the two tracks), so it predates
    this integration and is not a regression of it.
  - `vibemuxd` `plugin_registry::periodic_heartbeats_survive_multiple_deadlines_and_silence_exhausts_budget`
    is a wall-clock test in main's code. It failed once in that run and passed
    twice when rerun alone; treat it as timing-sensitive, not as proven stable.
- Defect found by the gates and fixed: two dashboard tests asserted the
  branch's five-agent registry; they now follow `AgentKind::all()`, which has
  eleven entries on main.
- An earlier full run overlapped other heavy work and reported 21 failures in
  seven targets (native dispatch, DSH session, MCP lifecycle, `vibemuxd` runtime
  ACL tests, one `vibemux_cli` test, and the two items above). Everything except
  `workflow_cooperate` passed when rerun alone and in the idle full run, so
  those were load effects, but they show that several suites have tight
  deadlines.
- Python: `pytest` exits 0 on the whole suite, `ruff check` and
  `ruff format --check` are clean, mypy is clean on the changed Python files,
  and `scripts/check_public_artifacts.py` reports 0 findings over 589 files
  with the 14 exact reviewed fixtures. The project virtual environment's
  interpreter could not create files during this run, so Python used the base
  3.12 interpreter with the environment's site-packages; a whole-repository mypy
  run in that setup reports errors in six files this merge did not touch and is
  not claimed clean.
- Not run: any real vendor CLI, `live_codex_smoke`, a live Jev call, Linux,
  nextest, cargo-deny, or cargo-audit. The native dispatch's live evidence above
  is the branch-time record and was not repeated. `vibemux_web` is outside these
  workspace gates.

### 2026-10-17 - Shared harness protocol engine and adapter capability display

**Scope**
- A harness-design review of the two dispatch paths and of the frontends. After the ADR 040 port, `vibemux_harness_native` carried its own copy of the Codex, Claude, and ACP state machines and vendor argv, so the two paths could drift apart, and Claude ran under a weaker posture on the native path. No frontend showed which harnesses VibeMux can drive through a structured adapter.
- Backend: `vibemux_harness` (shared argv and session constructors, new `dispatch::capability` table) and `vibemux_harness_native` (now a thin wrapper over the shared engine). Frontend: `vibemux_frontend` (dashboard, TUI debug snapshot, egui Agents view and composer) and `vibemux_cli` (`probe` text). Docs: [docs/architecture.md](docs/architecture.md) and the "Integration with main" section of [ADR 040](docs/adr/040_native_harness_request_dispatch.md).
- No store schema, control protocol, plugin wire, `HarnessRow` wire, or route-config schema change. No milestone status moves.

**Implemented**
- `vibemux_harness::dispatch::structured_arguments` is now the only place that defines each vendor's structured argv: Codex app-server and exec, Claude stream-json, and ACP for OpenCode, Copilot, and Grok. The daemon's `build_launch_spec` delegates to it, and DSH keeps its launcher pair. ACP model selection is refused there as well as in route config.
- `SessionRoute` with `ProtocolSession::probe` and `ProtocolSession::execution` builds a session from route facts without a daemon `DispatchRoute`. The daemon's `for_probe` and `for_execution` keep their signatures and the 128 KiB prompt limit.
- `vibemux_harness_native` drives Codex, Claude, and ACP routes through the shared `ProtocolSession` and `observation::classify`, and its observation `Kind` is now the shared `ObservationKind`.
  - Only the direct OpenCode JSON classifier stays local; direct Copilot and Grok JSON stay opaque and non-executable as before.
  - A fixed 13-entry table maps shared protocol errors to the crate's existing `harness_*` codes and keeps the source error.
- `vibemux_harness::dispatch::capability` (`AdapterSupport`, `AdapterCapability`, `adapter_capability`, `adapter_capabilities`) derives each harness's support from `NativeProtocol`. The value is `execute`, `probe_only`, or `terminal_only`. It describes what this build can do, not what the operator has configured or authorized.
- The frontends show it:
  - Dashboard: an Adapter column in the wide Harnesses table, and an `Adapter:` line in the details pane.
  - Plain snapshot and TUI debug snapshot: `adapter=… protocols=…` per agent.
  - GUI: the support level in the Agents list and the full summary in its detail view.
  - GUI composer: a detected harness that cannot take a prompt now says why ("Structured adapter is probe-only", "No structured adapter (terminal UI only)") instead of appearing available.
  - `vibemuxctl probe` text: `adapter=` per row, and `unknown` for an agent this build does not know.

**Behavior changes on the native path (ADR 040)**
- Native Claude routes also get `--strict-mcp-config` and `--restricted`, so project and user MCP servers and settings files cannot re-enable tools. The bare, tool-free `dontAsk` profile is unchanged.
- Native Codex exec puts `--model` before the stdin marker `-`, which stays last.
- A duplicate Codex exec terminal record reports `harness_unexpected_terminal` instead of `harness_duplicate_terminal`.
- `clientInfo.version` in initialize frames is the workspace version `0.2.0-alpha.1` instead of `0.2.0`.
- The plugin error allowlist accepts all 13 mapped protocol codes. Five of them used to be masked as `harness_vendor_failed`.
- The dispatcher builds its validator session before it takes a slot or lease or spawns the plugin, so a route the engine cannot drive is refused without starting a process.

**Evidence** (native Windows, Rust 1.92.0, `-j 4`, no vendor CLI, Jev, or paid API invoked)
- `cargo fmt --all -- --check`: clean.
- `cargo clippy -p vibemux_harness -p vibemux_harness_native -p vibemux_frontend -p vibemux_cli --all-targets --all-features -- -D warnings`: clean.
- `cargo test -p vibemux_harness -p vibemux_harness_native -p vibemux_cli -p vibemux_frontend --all-features --no-fail-fast`, every target passes except one:
  - `vibemux_harness`: lib 85, `dispatch_attempt` 12, `dispatch_config` 22, `dispatch_framing` 8, `dispatch_protocols` 14, `writable_fixture_guard` 4.
  - `vibemux_harness_native`: lib 7, `normalization` 18, `process_dispatch` 19.
  - `vibemux_frontend`: lib 123 (1 ignored), `cli` 6, `palette_parity` 2, `supervisor_ui` 10.
  - `vibemux_cli`: lib 40, `bootstrap_process` 4, `unified_commands` 4 of 5.
  - The exception is `unified_commands::workspace_inventory_disables_external_git_filters_and_labels_the_comparison`. It fails at a fixture `git` call (`unified_commands.rs:40`), and it fails identically on unmodified `HEAD` `5f82aa7` in a scratch worktree, so it predates this change. Its root cause in this environment was not found.
- `cargo test -p vibemuxd --all-features --lib --test harness_dispatch --test control_harness_dispatch --test dsh_session --test launch_trampoline --test decision_route`: lib 123, `harness_dispatch` 23, `control_harness_dispatch` 3, `dsh_session` 10, `launch_trampoline` 6, `decision_route` 10, all passed.
- New tests:
  - Shared Claude argv posture and Codex exec stdin-marker order.
  - Refused mismatched, launcher-pair, and ACP-model routes.
  - Capability table order, ownership, and summaries.
  - Native argv composition, error-code mapping, and duplicate-terminal handling.
  - Composer reasons, dashboard and TUI adapter display (including with no probe observation), and CLI `probe` text.

**Remaining**
- Policy split, not changed: ADR 040's native path still executes ACP routes, while ADR 029 keeps ACP probe-only because some ACP vendors grant their own tool permissions. The shared engine declines every `session/request_permission` it sees, but that cannot restrain a vendor that never asks. An operator decision is needed: align the native path with ADR 029, or record a deny posture per vendor.
- Not done: multi-turn or resumed sessions, an approval bridge for vendor permission requests, contract-drift telemetry for unknown vendor event types, and the vendor CLI version on dispatch records.
- Live vendor evidence is unchanged: initialize-only probes on the daemon path, and the 2026-08-27 branch record on the native path. No real prompt turn was run.

**Known risks**
- The stricter native Claude argv and the shared Codex exec argv were checked against fixtures only. A vendor release that rejects `--restricted` or `--strict-mcp-config` would now fail on both paths, not just the daemon path.

### 2026-10-17 - Hermetic Git filter fixture for the workspace inventory test

**Scope**
- Fixes the pre-existing `vibemux_cli` failure recorded in the previous entry: `unified_commands::workspace_inventory_disables_external_git_filters_and_labels_the_comparison`. Test-only change: a new `test_helpers` fixture binary, the test, and its fixture helper. No product code changed.

**Cause**
- The test configured the shell filter `echo invoked > filter_marker; cat`. Git for Windows runs such a command through MSYS `sh.exe`. Launched from the Rust test process, that `sh.exe` aborted before running anything with `NtCreateDirectoryObject(\BaseNamedObjects\msys-2.0S5-…): 0xC0000022` (access denied), so `git add` failed with filter exit 66.
- The same sequence succeeded from Git Bash and from PowerShell, with inherited or redirected stdio. It also failed outside the tool sandbox, so the sandbox is not the cause. Why this one launch context is denied was not determined.
- The flaw that matters: the test's "no filter ran" assertions only prove something while that shell can start.

**Implemented**
- New `vibemux_git_filter_fixture` binary (`required-features = ["test_helpers"]`, like the other CLI fixtures). It writes `filter_marker` in its working directory and copies stdin to stdout.
- The test configures the fixture's forward-slash path as both the `clean` and the `process` filter. With no shell metacharacters, Git runs it directly instead of through `sh -c`. The positive control (`git add` must leave the marker) and the negative checks (inventory must not run a clean or a process filter, and must not change the index) are kept. The test is now gated on `test_helpers`, which CI and the documented `--all-features` commands enable.
- The test's `git()` helper now reports the failing fixture arguments and Git's stderr. The arguments are constants, so no private data is printed.

**Evidence** (native Windows, Rust 1.92.0)
- `cargo test -p vibemux_cli --all-features --test unified_commands`: 5 passed, including the previously failing test.
- `cargo test -p vibemux_cli --test unified_commands` with default features: 4 passed, and the gated test compiles out cleanly.
- `cargo clippy -p vibemux_cli --all-targets -- -D warnings`, with and without `--all-features`: clean. `cargo fmt --all -- --check`: clean.

**Remaining**
- Without `test_helpers`, the filter test no longer runs. A filter path that contains a space or another shell metacharacter would make Git use `sh -c` again.

### 2026-10-17 - Pin the vendor-request refusal posture and the native completion-code allowlist

**Scope**
- Test-only. Coverage review of how both dispatch paths answer vendor-to-client requests, and of which plugin error codes the native dispatcher passes through. No behavior change.

**Findings**
- The native fixture already checks end to end that an ACP `session/request_permission` is answered `cancelled`, a Codex `item/commandExecution/requestApproval` is answered `decline`, and a Claude permission control request is answered with an error.
- Untested shared-engine branches before this change:
  - The Codex `item/fileChange/requestApproval` decline.
  - The JSON-RPC `-32601` refusal for every other vendor request, which covers the filesystem and terminal requests behind ACP's disabled client capabilities.
  - Rejection of a request whose id cannot be echoed back.
- The native `safe_error_code` allowlist had no test. Removing a mapped protocol code from it would have silently relabeled that violation as `harness_vendor_failed`.

**Implemented**
- `vibemux_harness` `dispatch_protocols::every_vendor_request_is_refused_and_none_is_granted`:
  - A Codex app-server session declines a `fileChange` approval and refuses an unknown request without a `jsonrpc` member.
  - An ACP session refuses `fs/read_text_file`, `fs/write_text_file`, `terminal/create`, and an unknown request with a `jsonrpc: "2.0"` `-32601` error that echoes no request parameters.
  - Both reject a non-scalar or empty request id with `InvalidRpcId` and leave the session non-terminal.
- `vibemux_harness_native` `dispatch::tests::completion_codes_survive_only_when_fixed_and_content_free`: all 13 mapped protocol codes plus representative transport codes are accepted. Empty, case-changed, padded, content-bearing, and vendor-defined codes are rejected.

**Evidence** (native Windows, Rust 1.92.0)
- `cargo test -p vibemux_harness --all-features --test dispatch_protocols`: 15 passed.
- `cargo test -p vibemux_harness_native --all-features --lib`: 8 passed.
- `cargo clippy -p vibemux_harness -p vibemux_harness_native --all-targets --all-features -- -D warnings`: clean. `cargo fmt --all -- --check`: clean.

**Remaining**
- Approval method names are pinned only to the ones the engine knows. A vendor that renames its approval request would get `-32601` rather than a decline. That is still a refusal, but its effect on the vendor turn was not verified against a live CLI.

### 2026-10-17 - Show what each native harness route can do in the catalog text

**Scope**
- CLI text rendering only. `vibemuxctl --format text harness catalog` previously listed each route's `enabled` and `allow_execution` settings, but not whether this build can probe or execute that route's protocol. The JSON catalog contract is unchanged.

**Implemented**
- `vibemux_cli` `output::route_actions` derives an `Actions` column from fields the catalog already returns:
  - `disabled` when the route is not enabled, because route lookup skips it.
  - `probe` when the shared engine has an initialize-only handshake for the protocol (`structured_probe`).
  - `execute` only when the route has a shared-engine session (`native_execution`) and the operator set `allow_execution`.
  - `none` for an enabled route that offers neither, such as a direct replay protocol, or a `codex` route without `allow_execution`.
- A footer states that the column combines route settings with this build's protocol engine and that a probe sends no prompt. The text keeps the "configuration only, not a live readiness check" header.

**Evidence** (native Windows, Rust 1.92.0)
- `cargo test -p vibemux_cli --all-features`: lib 41 passed (including `output::tests::harness_catalog_text_states_what_each_route_can_do`), `bootstrap_process` 4 passed, `unified_commands` 5 passed.
- `unified_commands::harness_catalog_stays_metadata_only_through_unified_entrypoint` now also renders the example configuration as text. It checks that every disabled route shows `disabled`, that no row offers `execute`, and that no `native_bin` path appears.
- `cargo clippy -p vibemux_cli --all-targets --all-features -- -D warnings`: clean. `cargo fmt --all -- --check`: clean.

**Remaining**
- The column reports configuration and build capability only. It does not check that the vendor CLI is installed, and it does not run a probe.
- ACP routes still show `execute` under the native CLI when `allow_execution` is set. Whether native ACP execution should follow the daemon's probe-only policy is still an open decision.

### 2026-10-17 - Tally observation kinds in the native harness report text

**Scope**
- CLI text rendering only. The `vibemuxctl harness` probe, dispatch, and replay reports list at most 32 observation rows in text mode. A run with more records hid how many records the classifier left `opaque`, which is the earliest sign that a vendor changed its output. The JSON reports are unchanged and already carry every observation.

**Implemented**
- `vibemux_cli` `output::kind_tally` adds a `Kinds:` line that counts every observation, including the truncated rows. It uses the shared ADR 029 kind order. A kind outside that vocabulary is counted as `unrecognized` and is never echoed.
- When any record is `opaque`, the text says that such records are kept intact but not classified: RPC replies, or vendor events without a verified contract. It also says that a growing share can mean the vendor output changed.
- A replay report now starts with `Replay <protocol> | replayed` and `Records: <n>`. It no longer prints `Harness -` and placeholder process fields for a replay that ran no process.

**Evidence** (native Windows, Rust 1.92.0)
- `cargo test -p vibemux_cli --all-features`: lib 42 passed (including `output::tests::harness_report_text_tallies_every_kind_including_truncated_rows`), `bootstrap_process` 4 passed, `unified_commands` 6 passed.
- New `unified_commands::harness_replay_text_tallies_kinds_without_vendor_content` replays the native `codex_app_server.jsonl` fixture through `vibemuxctl --format text harness replay`. It checks the header, `Records: 10`, and `Kinds: started 1, text 2, tool 1, approval 1, usage 1, completed 1, opaque 3`, and that no fixture content appears.
- `cargo clippy -p vibemux_cli --all-targets --all-features -- -D warnings`: clean. `cargo fmt --all -- --check`: clean.

**Remaining**
- The tally is a count, not a drift detector. Some opaque records are expected for every protocol (RPC replies), and every record of a direct `copilot_json` or `grok_stream_json` replay is opaque. No baseline share is recorded or compared.
- The daemon dispatch status already returns per-kind counts (`CaptureSummary.kinds`). This change does not touch the daemon or its control protocol.

### 2026-10-17 - Document harness adapter capability and protocol error codes

**Scope**
- Documentation, plus one test that keeps it current. `docs/protocol_boundaries.md` was stale for the harness surface:
  - It still described control IPC v6.
  - It said ACP prompt execution was unimplemented, although the ADR 040 native CLI executes ACP routes that set `allow_execution`.
  - It said MCP was unimplemented, although ADR 035 adds `vibemux_mcp`.
  - It did not mention the native harness CLI, adapter capability, or the native error codes.

**Implemented**
- The control IPC bullet now lists v1–v10 from the constants in `vibemuxd::control`.
- The ACP/MCP bullet now states the daemon's probe-only rule and the native CLI's ACP execution path. It names that divergence as an open decision and links the native MCP read frontend.
- A new "Native harness CLI" bullet describes `vibemuxctl harness`. It runs on the shared `ProtocolSession` engine, writes no canonical state, and produces content-free reports. Execution needs `allow_execution`, and its status is `PARTIAL — FIXTURE ONLY`.
- A new "Harness adapter capability" section has the per-harness table from `adapter_capability`: execute, probe_only, or terminal_only, and through which protocols. It also explains the catalog `Actions` column.
- A new "Harness protocol error codes" section:
  - Maps the 13 shared `DispatchError` protocol violations to their daemon (`harness_protocol_*`) and native (`harness_*`) codes, and lists the other native mappings.
  - Documents the plugin completion-code allowlist, under which anything else becomes `harness_vendor_failed`.
  - Documents how each protocol answers vendor-to-client requests.
- `vibemux_harness_native` `session::tests::protocol_boundaries_document_every_protocol_code_pair` requires each code pair to appear on one table row of the document.

**Evidence** (native Windows, Rust 1.92.0)
- `cargo test -p vibemux_harness_native --all-features --lib`: 9 passed.
- Renaming one native code in the document made the new test fail with `harness_uncorrelated_turn is not documented beside harness_protocol_uncorrelated_turn`. Restoring the document made it pass again.
- Each table description and the vendor-request answers were checked against `vibemux_harness::dispatch::protocol_session` and `error_code`.
- `cargo clippy -p vibemux_harness_native --all-targets --all-features -- -D warnings`: clean. `cargo fmt --all -- --check`: clean.

**Remaining**
- Only the protocol code pairs are pinned by a test. The capability table and the prose can still drift from the code.
- Other `protocol_boundaries.md` bullets (A2A, plugin, workflow) were not re-audited.

### 2026-10-17 - Keep adapter support in every dashboard agent table width

**Scope**
- Read-only dashboard rendering (ADR 041). Below 72 columns the agent table switched to `Agent / Launcher / Route` and dropped the `Adapter` column. That happened on the overview in a terminal narrower than 72 columns, and on the Harnesses page at most widths, because the table gets 45% of the width there. No model, probe, or protocol change.

**Implemented**
- `vibemux_frontend` `dashboard::render::AgentColumns` chooses one of three column sets by table width:
  - Full (at least 72): every probe column.
  - Compact (50 to 71): `Agent / Launcher / Adapter / Route`. 50 is the narrowest width at which the route column still fits `local_gateway`.
  - Minimal (below 50): `Agent / Launcher / Adapter`.
- Adapter support is a build fact that no observation replaces, so every set keeps it. The minimal set drops the route, which the selected-harness pane still shows.

**Evidence** (native Windows, Rust 1.92.0)
- New `dashboard::tests::every_agent_table_width_keeps_adapter_support` covers the overview at 60 and 44 columns and the Harnesses page at 80 and 120 columns. It checks that the table header has `Adapter`, has `Route` only when the compact or full set applies, and that `execute` and `probe` labels are visible.
- `vibemuxctl --format text ui --once --report <probe fixture> --width 60|44 --height 30` shows the compact and minimal tables with each harness's adapter label.
- `cargo test -p vibemux_frontend --all-features`: lib 124 passed (1 ignored) and all integration targets passed. `cargo test -p vibemux_cli --all-features --test unified_commands`: 6 passed.
- `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings`: clean. `cargo fmt --all -- --check`: clean.

**Remaining**
- On the Harnesses page at 80 columns, the table now shows the adapter instead of the route. The route stays in the selected-harness pane.
- The overview's stacked agent table is 8 rows high, so at 24 rows only the first five harnesses are visible without scrolling. This is unchanged.

### 2026-10-17 - Pin the documented harness capability table to the code

**Scope**
- Test plus a one-cell documentation fix. The capability table that `c8512ce` added to `docs/protocol_boundaries.md` had no test, and it already had one error: DSH's protocol id was written as `dsh_sdk_jsonrpc`, but the code uses `dsh_sdk_json_rpc`.

**Implemented**
- `vibemux_harness` `dispatch::capability::tests::protocol_boundaries_document_every_harness_capability` parses the "Harness adapter capability" table. It requires every `AgentKind` display name exactly once, and the total count rules out stale or extra names. Each harness's row must list exactly the protocols and support that `adapter_capability` derives.
- `docs/protocol_boundaries.md`: the DSH row now names `dsh_sdk_json_rpc`.

**Evidence** (native Windows, Rust 1.92.0)
- Before the documentation fix, the new test failed with ``DSH protocols: left "`dsh_sdk_jsonrpc`", right "`dsh_sdk_json_rpc`"``. After the fix it passes.
- Mutation checks, each restored afterwards:
  - Documenting ACP as `execute` failed with `OpenCode support`.
  - Adding an unknown harness to a row failed with `stale or extra harness`.
- `cargo test -p vibemux_harness --all-features`: lib 86 passed and every integration target passed. `cargo test -p vibemux_harness_native --all-features --lib protocol_boundaries`: 1 passed.
- `cargo clippy -p vibemux_harness --all-targets --all-features -- -D warnings`: clean. `cargo fmt --all -- --check`: clean.

**Remaining**
- The prose around both tables is still unchecked.

### 2026-10-17 - Bring the native harness guide and README table up to date

**Scope**
- Documentation only, from a re-audit of the harness surfaces. `docs/guides/harness_core_guide.md` was stale:
  - It named a pinned Rust 1.85 toolchain, but `rust-toolchain.toml` pins 1.92.0.
  - It did not mention `vibemuxctl harness`, `codex_exec`, the shared protocol engine, or that the direct JSON protocols are replay-only.
  - It did not mention the daemon/native ACP difference.
- The README's ADR 029 / ADR 040 table did not say that native ACP routes can execute.

**Implemented**
- The guide now says:
  - All four structured protocols run on the shared `ProtocolSession` engine that the daemon also uses.
  - `opencode_json`, `copilot_json`, and `grok_stream_json` can be replayed but not run.
  - The native CLI can run an ACP prompt where the daemon only probes, and this is an open decision.
  - `vibemuxctl --format text` adds the catalog `Actions` column and the per-kind tally.
  - A Codex probe needs a `codex_app_server` route.
- The guide also names Rust 1.92.0 and links protocol boundaries.
- The README table's Routes row states native ACP execution under `allow_execution`, and a new Protocol engine row says both paths share the engine.

**Evidence** (native Windows, Rust 1.92.0)
- The guide's offline commands ran from the repository root:
  - `cargo build -p vibemux_harness_native --bins`.
  - `vibemux_harness_native.exe catalog --config examples/harness/harness_config.json` and the documented `replay`.
  - `vibemuxctl help harness`.
- `vibemuxctl --format text harness replay --protocol copilot_json --input .../unverified_stream.jsonl` showed `Kinds: opaque 3`.
- Each new statement was checked against `rust-toolchain.toml`, `contract::Protocol::native_protocol`, `session::Session::new`, and `ProtocolSession`'s probe handling.

**Remaining**
- The guide's "Current Live Boundary" section is the 2026-08-27 record and was not re-validated. No live probe was run.

### 2026-10-17 - Correct the architecture overview's harness statements

**Scope**
- Documentation only. `docs/architecture.md` had three statements that no longer matched the code:
  - The overview said ACP execution was unimplemented. The ADR 040 native dispatcher can already run an ACP turn on a route that sets `allow_execution`.
  - The ADR 040 section listed protocol correlation as a separate native module. Correlation now lives in the shared `ProtocolSession` engine, and the native crate only adapts to it.
  - It said only the direct OpenCode replay was the native crate's own. It did not say that all three direct JSON routes are replay-only, or that direct Copilot and Grok records stay `opaque`.

**Implemented**
- The overview now says the daemon uses ACP only for initialize-only probes. It adds that the native dispatcher can run a fixture-verified ACP turn under `allow_execution`, and that whether it should follow the daemon's rule is an open decision. Remote A2A deployment is still listed as unimplemented.
- The ADR 040 section names the adapter over the shared protocol engine and states which direct routes are replay-only and how each is classified.

**Evidence** (native Windows, Rust 1.92.0)
- Each corrected statement is backed by an existing test that was run:
  - `cargo test -p vibemux_harness_native --features test_helpers --test process_dispatch all_five_harnesses`: 1 passed. It runs OpenCode, Copilot, and Grok as fixture ACP turns.
  - `cargo test -p vibemux_harness_native --test normalization --lib`: 18 and 9 passed. These include `unverified_copilot_and_grok_streams_remain_opaque` and `unsupported_routes_keep_their_native_codes`.

**Remaining**
- No test pins `docs/architecture.md` prose, so it can drift again.

### 2026-10-17 - State which plugins speak the plugin wire

**Scope**
- Documentation only. In `docs/protocol_boundaries.md`, the plugin stdio bullet said real plugins were unimplemented, and the supervisor bullet said real vendor plugins were absent. Two first-party plugins already speak the Protobuf v1 wire under `vibemux_plugin_supervisor`:
  - `vibemux_terminal_observer` (plugin id `wezterm_observer`, kind terminal).
  - `vibemux_harness_native_plugin` (plugin id `vibemux.harness`).

**Implemented**
- The plugin stdio bullet names both plugins and says who starts each:
  - The daemon's plugin registry starts the observer from the operator's startup configuration.
  - The ADR 040 dispatcher starts the harness plugin once per probe or execution, in the CLI process.
- It says neither is live-validated, and that plugin SDKs and third-party plugins are still unimplemented.
- The supervisor bullet now says plugin SDKs and third-party plugins remain absent. Status rows and the "Real vendor plugins" checklist item are unchanged.

**Evidence** (native Windows, Rust 1.92.0)
- Each statement was checked against the code and existing records:
  - `vibemux_terminal_observer/src/main.rs` and `vibemux_harness_native/src/plugin.rs` both use `vibemux_plugin_protocol` framing.
  - `vibemux_harness_native::dispatch` calls `spawn_plugin` from its per-request `run`.
  - The observer's "not live-verified" status comes from the PROGRESS status table.
- Both tests that read this document still pass:
  - `cargo test -p vibemux_harness --lib protocol_boundaries`: 1 passed.
  - `cargo test -p vibemux_harness_native --lib protocol_boundaries`: 1 passed.

**Remaining**
- The A2A and workflow bullets were not re-audited in this change.

### 2026-10-17 - Show forced termination and stderr size in the native harness text view

**Scope**
- `vibemuxctl --format text harness probe|dispatch` printed only the record count, stdout bytes, and exit code from the native `Completion`. It dropped two fields:
  - `forced_termination`. When the native process is killed after a deadline, cancellation, protocol error, or missed shutdown grace period, the exit code comes from the kill, not the vendor, and the text view gave no hint of that.
  - `stderr_bytes`, the only content-free sign that the vendor wrote diagnostics.
- The existing unit test rendered hand-written JSON, so a renamed completion field would have printed `-` without failing.
- An audit of the GUI Agents list at narrow widths found nothing to fix. The window's minimum inner size is 960x600, which leaves room for the one-line metadata and the Details button.

**Implemented**
- `crates/vibemux_cli/src/output.rs`:
  - The completion line reads `Records: N   Stdout bytes: N   Stderr bytes: N   Native exit: N`.
  - A forced termination adds a `Forced termination: ...` line that says the exit code is not the vendor's.
  - JSON output is unchanged.
- The new test `harness_outcome_text_reads_every_native_completion_field` renders a real `vibemux_harness_native::contract::Outcome`, with records built by `Record::parse`. It pins both lines in the forced and normal cases, and checks that neither the vendor text nor the request id appears.
- `docs/protocol_boundaries.md`: the native CLI bullet mentions the forced-termination flag.

**Evidence** (native Windows, Rust 1.92.0)
- `cargo test -p vibemux_cli --all-features --lib`: 43 passed. `--test unified_commands`: 6 passed.
- Mutation check: reading `stderr_byte` instead of `stderr_bytes` failed the new test with `Stderr bytes: -`. The file was restored afterwards.
- `cargo clippy -p vibemux_cli --all-targets --all-features -- -D warnings`: clean. `cargo fmt --all -- --check`: clean.

**Remaining**
- No end-to-end `vibemuxctl harness probe` run against the fixture plugin. The plugin binary belongs to another crate, so `vibemux_cli` tests cannot locate it through `CARGO_BIN_EXE_*`.

### 2026-10-17 - Test the native route gates before any process starts

**Scope**
- Test only. `enabled` and `allow_execution` are the operator's only authorization for the ADR 040 native path. Both layers enforce them: `Dispatcher::run` in the CLI process and `native::run` in the plugin. No test exercised either `harness_route_unavailable` or `harness_execution_disabled`.

**Implemented**
- New test `route_gates_refuse_before_any_plugin_or_vendor_process_starts` in `crates/vibemux_harness_native/tests/process_dispatch.rs`. Every route names a missing executable and working directory, so a request that slipped past a gate would fail later with `harness_invalid_cwd` instead.
- Dispatcher cases:
  - Execute on an enabled route without `allow_execution` gives `harness_execution_disabled`.
  - Execute or probe on a disabled route, and a probe of a harness with no route, give `harness_route_unavailable`.
- Plugin cases, through `native::run`, give the same codes. One extra case: a route handed in for another harness gives `harness_route_unavailable`.
- Control rows: a probe of an enabled route without `allow_execution` passes both gates and reaches `harness_invalid_cwd`.
- No product code changed.

**Evidence** (native Windows, Rust 1.92.0)
- `cargo test -p vibemux_harness_native --all-features`: lib 9, normalization 18, and process_dispatch 20 passed.
- Mutation checks, each restored afterwards. Disabling the `allow_execution` check in `dispatch.rs` failed the test with `harness_invalid_cwd` instead of `harness_execution_disabled`. Disabling the check in `native.rs` did the same.
- `cargo clippy -p vibemux_harness_native --all-targets --all-features -- -D warnings`: clean. `cargo fmt --all -- --check`: clean.

**Remaining**
- Whether native ACP routes should execute at all is still the open policy decision. This test covers the existing gates only.

### 2026-10-17 - Diagnostic TUI shows every probed harness row

**Scope**
- The read-only diagnostic TUI (`vibemux_frontend_tui`, `tui/render.rs`). The ADR 041 dashboard is unchanged.

**Implemented**
- Bug: the probe reports eleven harnesses, including DSH, but the stacked agent table had a fixed `Min(13)` height. That fit ten rows, so DSH was cut at 80x24 and 120x32 even while the status line counted it. The golden fixtures had pinned the cut table.
- The stacked table's height now follows the agent count: top border, header, and one row per agent.
- Its bottom edge is now the diagnostics block's collapsed top border (`├Diagnostics┤`). At 80x24 all eleven rows and the gateway, A2A, and telemetry lines still fit.
- The side-by-side layout (160 columns and wider) keeps full borders.
- New test `every_probed_agent_row_and_diagnostic_line_renders_at_every_size`. At 80x24, 120x32, and 160x40 it requires one table row per `AgentKind::all()` harness and all three diagnostics lines, so a twelfth harness without room fails it. It replaces the ten-name check.
- Regenerated the twelve golden fixtures; each changed only by the DSH row and the collapsed border. Corrected the stale "ten-agent" module comments.

**Evidence** (native Windows, Rust 1.92.0)
- `cargo test -p vibemux_frontend --all-features`: lib 124 passed (1 ignored generator); integration 6, 2, and 10 passed.
- Mutation check, restored afterwards: restoring the stacked table's full bottom border failed the new test with `missing agent row DSH at 80x24`.
- `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings`: clean. `cargo fmt --all -- --check`: clean.

**Remaining**
- The ADR 041 dashboard's agent table scrolls with the selection, but at 80x24 it shows five of eleven harnesses and gives no hint that more rows exist.

### 2026-10-17 - Dashboard names the harnesses outside its 80x24 view

**Scope**
- The ADR 041 read-only dashboard (`vibemuxctl ui`, `vibemux_dashboard`; `dashboard/render.rs`). No model, snapshot text, or navigation change.

**Implemented**
- At 80x24 the overview's agent table holds five of the eleven harnesses and scrolls with the selection. Nothing said the other six existed. When rows are hidden, the table title now names the rows in view, for example `Agent probes 7-11/11`.
- The table now owns its scroll window (`agent_window`), so the title and the rows always agree.
- Below the two-pane width, the Harnesses page shows no table. Its details title now gives the selection's position, for example `Selected harness 4/11 / j,k`.
- The native-TUI slot panel printed one `name [state]` label per harness. At 80 columns it overflowed after TRAE and hid CodeBuddy, Kimi, and DSH. It now prints one line per state, naming that state's harnesses (`reserved: ...`, `unavailable: ...`), so every name fits from 30 columns up.
- Replaced `narrow_windows_terminal_keeps_all_agents_and_slots_visible`, which checked five names only. The replacement checks the first and scrolled range titles, the selected DSH row, the narrow details position, and both slot lines.
- A new test requires every harness name inside the slot panel at 30, 59, 60, and 80 columns. A unit test pins the scroll window, including clamping and the empty cases.

**Evidence** (native Windows, Rust 1.92.0)
- `cargo test -p vibemux_frontend --all-features`: lib 126 passed (1 ignored generator); cli 6, palette_parity 2, and supervisor_ui 10 passed.
- `cargo test -p vibemux_cli --all-features --test unified_commands`: 6 passed.
- Mutation checks, each restored afterwards:
  - A window that never scrolls failed the window unit test and the narrow test.
  - Per-slot labels instead of grouped names failed the slot panel test (`CodeBuddy missing` at 30 columns) and the narrow test.
- `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings`: clean. `cargo fmt --all -- --check`: clean.

**Remaining**
- At 80x24 the overview's diagnostics panel shows the gateway and A2A lines but not telemetry.
- The details pane says `Reserved is not running.` even when the harness's slot is `unavailable`.

### 2026-10-17 - Dashboard details state each harness's own native-TUI slot

**Scope**
- The ADR 041 dashboard's Harnesses details pane (`dashboard/render.rs`). No model or snapshot text change.

**Implemented**
- Bug: the details pane ended with `Reserved is not running.` for every harness. That included harnesses whose slot is `unavailable` because no launcher was verified, so a missing or failed CLI read as reserved.
- The line now reads that harness's own slot: `Native TUI slot: reserved, not running` or `Native TUI slot: unavailable, no verified launcher`. It finds the slot by harness kind, not by list position.
- An exhaustive match maps each slot state to its text, so a new state must be given its own text.
- New test `details_state_the_selected_harness_slot_not_a_fixed_reserved_line` covers three harnesses at 70 and 100 columns: a verified one, an unavailable one, and one with no observation.

**Evidence** (native Windows, Rust 1.92.0)
- `cargo test -p vibemux_frontend --all-features`: lib 127 passed (1 ignored generator); cli 6, palette_parity 2, and supervisor_ui 10 passed.
- `cargo test -p vibemux_cli --all-features --test unified_commands`: 6 passed.
- Mutation check, restored afterwards: printing the reserved text for an unavailable slot failed the new test at 70 columns.
- `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings`: clean. `cargo fmt --all -- --check`: clean.

**Remaining**
- At 80x24 the overview's diagnostics panel still omits the telemetry line.

### 2026-10-17 - Dashboard overview keeps every diagnostics line and grows its agent table

**Scope**
- The ADR 041 dashboard's stacked overview, below 110 columns (`dashboard/render.rs`). The side-by-side overview is unchanged.

**Implemented**
- Bug: the stacked overview gave the agent table a fixed eight rows (five agents) and the rest to diagnostics.
  - At 80x24 the diagnostics panel had two rows of text, so the telemetry line was cut.
  - On taller terminals the table still showed five agents above blank diagnostics rows.
- New split:
  - Diagnostics get the rows their word-wrapped lines need.
  - The table keeps at least its header and two agents, then takes the rest up to its full height.
  - Leftover rows go back to diagnostics.
- At 80x24 the table now shows four agents, titled `Agent probes 1-4/11`, plus all three diagnostics lines. At 100x28 it shows eight. At 100x40 it shows all eleven, with no range title.
- `wrapped_rows` estimates greedy word wrapping, splitting a word wider than a row. A unit test pins it.
- New test `overview_keeps_every_diagnostics_line_and_grows_the_table_with_height` covers 80x24, 60x24, 45x30, 100x28, and 100x40. The narrow-terminal test now expects the four-row window.

**Evidence** (native Windows, Rust 1.92.0)
- `cargo test -p vibemux_frontend --all-features`: lib 129 passed (1 ignored generator); cli 6, palette_parity 2, and supervisor_ui 10 passed.
- `cargo test -p vibemux_cli --all-features --test unified_commands`: 6 passed.
- Mutation check, restored afterwards: restoring the fixed eight-row split failed the new test with `80x24 lacks Telemetry:`.
- `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings`: clean. `cargo fmt --all -- --check`: clean.

**Remaining**
- Below about 35 columns, the wrapped diagnostics lines can still exceed the space the table leaves.

### 2026-10-17 - State adapter support in the diagnostic TUI

**Scope**: Ratatui diagnostic TUI (`vibemux_frontend` `tui/render.rs`), with `docs/protocol_boundaries.md` and `docs/architecture.md`.

**Implemented**:
- The diagnostics panel gains an `Adapters:` line. It groups every probed harness under the strongest thing this build's structured adapters can do for it, strongest support first and probe order within a group, for example `execute: Claude, Codex, DSH; probe_only: OpenCode, Copilot, Grok; terminal_only: ...`. `protocol_boundaries.md` already said the TUI showed adapter support, but only the copied snapshot carried it.
- It is a line, not a table column. At 80 columns the six-column table has no free cells. At 160 columns, a 13-cell column would clip the real Grok version string that the existing test keeps whole.
- The line follows the gateway, A2A, and telemetry lines, so the minimum 80x24 layout keeps every probe line and does not change. At 80x24 the line has no row, and the snapshot's `adapter=` field is the record there. The two docs now name the diagnostics line and the snapshot.

**Evidence** (native Windows, Rust 1.92.0):
- `cargo fmt --all -- --check` and `cargo clippy -p vibemux_frontend -p vibemux_harness --all-targets --all-features -- -D warnings` pass.
- `cargo test -p vibemux_frontend --all-features`: 131 library tests pass, with 1 ignored generator, plus the crate's integration tests. `cargo test -p vibemux_harness --lib capability`: 6 pass, including the documentation-table check.
- Two new tests pin the exact grouping and the empty case, and the line at 120x32 and 160x40. The six 120x32 golden fixtures were regenerated; the 80x24 fixtures are byte-identical.
- Mutation checks: dropping the line's content fails the render test and the goldens. Ordering the groups weakest first fails the grouping test as well.

**Remaining**: No live terminal run. The line is hidden at 80x24 for lack of rows.

### 2026-10-17 - Correct the dual-track protocol boundary

**Scope**: The dual-track workflow bullet in `docs/protocol_boundaries.md`, with a `vibemuxd` guard test. The A2A bullet was re-audited and left unchanged.

**Implemented**:
- The bullet said "There is no AAG gateway client". Since ADR 036, the daemon has a bounded loopback AAG transport, and its AAG adapter is the only production `ModelGateway`. The bullet now says that this transport refuses every live call before submission (`gateway_live_attestation_unavailable`). It also says that only `test_helpers` builds can construct the permit its fake-server calls need, so no model call is made.
- The bullet listed only the 15 Control v6 operations. It now names the v9 `workflow_supervisor_start` and `workflow_supervisor_status` (ADR 037) and the v10 `workflow_contract_edit` (ADR 039). Both newer answers were checked to be content-free (digests, versions, codes, IDs, and the prepare response), so "content-free except `workflow_session_prompt`" still holds.
- A2A audit: the information-share peer, the authenticated HTTP+JSON, JSON-RPC, and gRPC task gateway, and the stated non-claims all match `vibemux_a2a` and `docs/a2a_supervisor_protocol.md`.
- `protocol_boundaries_dual_track_bullet_matches_the_daemon` reads the bullet. It requires the v6 count to equal the test's `WORKFLOW_OPERATIONS` list, each later workflow operation's wire name to appear, and the refusal code to equal `Aag_Gateway_Error::LiveAttestationUnavailable.code()`.

**Evidence** (native Windows, Rust 1.92.0):
- `cargo fmt --all -- --check` and `cargo clippy -p vibemuxd --all-targets --all-features -- -D warnings` pass.
- `cargo test -p vibemuxd --all-features --lib control::tests`: 21 pass. The other two tests that read this document, the `vibemux_harness` capability-table check and the `vibemux_harness_native` error-code check, still pass.
- Mutation checks: dropping the v9 names, stating 14 operations, and renaming the refusal code each fail the new test at their own assertions.

**Remaining**: The guard's list of later operations is maintained by hand, like the existing per-version lists. `README.md`, `CLAUDE.md`, and ADR 031 still use the shorthand "no AAG gateway client", meaning no live-capable client. They were left for the owner to reword.

### 2026-10-17 - Guard the control IPC version range in the protocol boundary

**Scope**: The "Local control IPC" bullet in `docs/protocol_boundaries.md`, with a `vibemuxd` guard test.

**Implemented**:
- I verified the bullet against `ControlOperation::minimum_protocol_version` and `supported_control_version`. The current version (v10), the accepted range (v1–v10), and what each version added are correct.
- One omission is fixed. The bullet said `vibemuxctl` covers lifecycle, recovery, harness listing and switching, dispatch, and workflows. Its daemon-backed commands also include `Dsh` and `Decision`, so the bullet now names DSH sessions and advisory decisions.
- `protocol_boundaries_ipc_bullet_states_every_accepted_version` requires the bullet to name `CONTROL_PROTOCOL_VERSION` as current, to state the accepted range ending at it, and to name what every accepted version added. A version bump now fails until the documentation follows.

**Evidence** (native Windows, Rust 1.92.0):
- `cargo fmt --all -- --check` and `cargo clippy -p vibemuxd --all-targets --all-features -- -D warnings` pass.
- `cargo test -p vibemuxd --all-features --lib control::tests`: 22 pass.
- Mutation checks: a stale current version, a stale range, and a dropped v7 clause each fail the new test at their own assertions.

**Remaining**: The test checks that each version is named, not the operation names listed for it. Those come from `minimum_protocol_version`, which has no enumerable operation list.

### 2026-10-17 - Stop reporting native runner failures as vendor failures

**Scope**: `vibemux_harness_native` plugin completion error-code allowlist (`dispatch.rs` `safe_error_code`) and its paragraph in `docs/protocol_boundaries.md`. The "Harness dispatch" and "Native harness CLI" bullets were audited and left unchanged.

**Implemented**:
- The ADR 040 dispatcher keeps a plugin completion's `error_code` only if it is on a fixed allowlist, and otherwise reports `harness_vendor_failed`. Tracing `native::run` showed seven fixed, content-free codes that the runner can record after the vendor process starts but that were not on the list: `harness_read_failed`, `harness_wait_failed`, `harness_missing_pipe`, `harness_receiver_closed`, `harness_encode_failed`, `harness_invalid_limits`, and `harness_session_internal`. A stdout read error, a closed record channel, or an engine-internal error was therefore blamed on the vendor. They are now allowlisted. The list still admits only fixed constants, so vendor text still cannot pass through.
- `every_runner_code_after_spawn_survives_the_allowlist` scans `native.rs` and `json_lines.rs` for code literals. Each one must survive the allowlist unless it is one of the four checks the runner makes before a vendor process exists, which end the request without a completion. A new runner failure code fails this test instead of silently becoming `harness_vendor_failed`.
- Audit: the dispatch bullet's claims match `vibemuxd`: the trampoline, at most one dispatch per project, raw records only from `harness_dispatch_output`, and initialize-only live evidence. The native CLI bullet's commands (`catalog`, `replay`, `probe`, `dispatch`) match `vibemux_harness_native::cli::Command`.

**Evidence** (native Windows, Rust 1.92.0):
- `cargo fmt --all -- --check` and `cargo clippy -p vibemux_harness_native --all-targets --all-features -- -D warnings` pass.
- `cargo test -p vibemux_harness_native --all-features`: 10 library tests and the 18- and 20-test integration suites pass. These include the fixture-binary process dispatch tests and the protocol error-code documentation check.
- Mutation checks: dropping `harness_read_failed` from the allowlist fails both allowlist tests. Dropping `harness_missing_pipe`, which only the scan covers, fails the new test.

**Remaining**: The scan covers the runner and its line reader. Codes from `process.rs` and `contract.rs` that are reachable mid-exchange (`harness_wait_failed`, `harness_invalid_json`, `harness_invalid_record`) are pinned by the explicit list only. No live vendor run.

### 2026-10-18 - Native dispatcher keeps the plugin's pre-completion code

**Scope:** `vibemux_harness_native` dispatcher (ADR 040) and `docs/protocol_boundaries.md`. No wire, schema, or control change.

**Implemented:** when the native plugin answers without a completion, because it refused the request or its runner failed before the vendor process yielded one, the dispatcher discarded the plugin's code and reported `harness_plugin_rejected`. A route pointing at a missing vendor executable or a Windows `.cmd` shim therefore did not say why nothing ran. The dispatcher now keeps the plugin's code when the answer is an error and the code is on a fixed list (`REJECTION_CODES`: the plugin's request checks, the runner's checks before launch, and its launch, containment, and reaping failures); any other code, including a case-changed, padded, or content-bearing one, stays `harness_plugin_rejected`. The protocol boundary documents the second list.

**Evidence (native Windows, Rust 1.92.0):** `cargo +1.92.0 fmt -p vibemux_harness_native`, `cargo +1.92.0 clippy -j 4 -p vibemux_harness_native --all-targets --all-features -- -D warnings`, and `cargo +1.92.0 test -j 4 -p vibemux_harness_native --all-features` passed. New `every_code_before_a_completion_survives_the_rejection_list` scans the runner's process code, its pre-launch checks, `Session::new`, and the plugin's request checks; new `launch_failures_keep_the_runner_code` probes and executes a missing executable through the real plugin (`harness_invalid_launch`); the Windows batch-route test now asserts `harness_native_executable_required`. Mutations (always `harness_plugin_rejected`, dropping `harness_spawn_failed` from the list, ignoring the response status) each failed the new tests. Fixture vendors only.

**Remaining:** a dispatcher-built completion for a failed exchange still sets `forced_termination`, even when the plugin answered cleanly and no vendor process was killed.

### 2026-10-18 - Native completion-less answers claim no forced kill and skip the drain

**Scope:** `vibemux_harness_native` dispatcher (ADR 040) and `docs/protocol_boundaries.md`. No wire, schema, or control change.

**Implemented:** a plugin answer without a completion (the request refused, or the vendor process never launched) went through the dispatcher's failed-exchange path. That path claimed `forced_termination: true`, so `vibemuxctl --format text harness probe` printed the forced-termination line for a process that never started. It also cancelled and drained for `shutdown_ms` (2 s by default) waiting for an answer that had already arrived. The dispatcher now builds the `failed` completion directly, with no exit code and `forced_termination: false`, as the daemon reports a vendor process that never started. It returns without the drain. Failures inside an exchange still cancel, drain, and report a forced termination.

**Evidence (native Windows, Rust 1.92.0):** `cargo +1.92.0 fmt -p vibemux_harness_native`, `cargo +1.92.0 clippy -j 4 -p vibemux_harness_native --all-targets --all-features -- -D warnings`, `cargo +1.92.0 test -j 4 -p vibemux_harness_native --all-features`, and `cargo +1.92.0 test -j 4 -p vibemux_cli --all-features harness` passed. `launch_failures_keep_the_runner_code` now uses a 3 s grace period. It asserts no forced termination and that each probe and execution returns within that period. Mutations failed as expected: claiming a forced kill failed the forced-termination assertion. Restoring the old error path failed it too, and with that assertion disabled the timing assertion failed at 3.31 s. Fixture vendors only.

**Remaining:** none for this path.

### 2026-10-18 - Native plugin exit no longer hides the exchange's failure

**Scope:** `vibemux_harness_native` dispatcher (ADR 040) and `docs/protocol_boundaries.md`. No wire, schema, or control change.

**Implemented:** after every request the dispatcher overwrote the completion with `failed` / `harness_plugin_shutdown_failed` whenever the plugin did not shut down cleanly. That replaced a failure the exchange had already recorded, such as a protocol violation or `harness_dispatch_timeout`, which was usually the cause of the unclean exit. The new `apply_plugin_exit` folds the plugin's exit the way `vibemux_harness::dispatch::decide_outcome` folds the vendor's. An unclean exit still fails a success, a protocol terminal, or an unverified completion. An exchange failure keeps its status and code. The protocol boundary states the rule.

**Evidence (native Windows, Rust 1.92.0):** `cargo +1.92.0 fmt -p vibemux_harness_native`, `cargo +1.92.0 clippy -j 4 -p vibemux_harness_native --all-targets --all-features -- -D warnings`, and `cargo +1.92.0 test -j 4 -p vibemux_harness_native --all-features` passed. New `an_unclean_plugin_exit_never_hides_an_exchange_failure` covers clean and unclean exits for five completions that are demoted and five that are kept. Three mutations each failed it: the old unconditional overwrite, treating `harness_eof_without_terminal` as an exchange failure, and ignoring the exit. No fixture makes the real plugin exit uncleanly, so the rule is unit-tested only. `vibemuxctl --format text` renders a missing exit code as `-`, the same placeholder as other missing fields, so Backlog 22 needed no change.

**Remaining:** an integration fixture that makes the plugin exit uncleanly after an exchange failure.

### 2026-10-18 - Native route table pinned to the shared engine

**Scope:** `vibemux_harness_native` contract tests only. No behavior, wire, schema, or control change.

**Implemented:** the native CLI keeps its own `Harness` and `Protocol` tables (ADR 040) beside the shared engine's `AgentKind` and `NativeProtocol`, and nothing checked that they agree. If the shared engine gained an ACP harness, every frontend would show it as `probe_only` through `adapter_capability`, while `vibemuxctl harness` could not route it. Two new tests close that. `native_routes_agree_with_the_shared_engine` checks three things: every native harness is named by its command name, every engine-driven native protocol by the engine's id, and native `accepts` matches shared `accepts` for every pair. `every_shared_adapter_outside_dsh_has_a_native_route` walks every `AgentKind` and checks that each structured adapter has a native route, apart from DSH, which `vibemux_dsh` drives.

**Evidence (native Windows, Rust 1.92.0):** `cargo +1.92.0 fmt -p vibemux_harness_native`, `cargo +1.92.0 clippy -j 4 -p vibemux_harness_native --all-targets --all-features -- -D warnings`, and `cargo +1.92.0 test -j 4 -p vibemux_harness_native --all-features` passed. Three mutations each failed the tests: dropping native ACP for Grok, adding Qwen to the shared ACP harnesses, and mapping native `acp` to the Claude engine. Backlog checks with no change: the daemon-backed `vibemuxctl dispatch` verbs deliberately have no text view (their JSON is the contract), and the MCP host has no harness listing tool, which would be an ADR 035 tool-contract change.

**Remaining:** the native `HARNESSES` and `PROTOCOLS` test arrays are maintained by hand, so a new native variant must be added to them. The shared-to-native direction is exhaustive through `AgentKind::all()` and `NativeProtocol::ALL`.

### 2026-10-18 - Native route argument and environment refusals are pinned

**Scope:** `vibemux_harness_native` configuration validation and the harness core guide. No wire, schema, control protocol, or policy change.

**Implemented:**

- The native configuration's two security-relevant refusals had no test: a route prefix carrying an approval, sandbox, sharing, or debug-log flag (`harness_unsafe_arguments`), and a bad environment name (`harness_invalid_environment`). Removing either check would not have failed any test.
- The inline flag list is now the documented `UNSAFE_ARGUMENTS` constant near the top of `config.rs`, with unchanged contents and matching.
- New unit tests refuse every listed flag alone and as `flag=value`, keep the Codex, Claude, and Copilot bypass flags on the list, and accept a plain launcher prefix. Environment names must be non-empty, at most 128 bytes, uppercase ASCII letters, digits, and `_`, distinct, and not `VIBEMUX_PLUGIN_SESSION_ID`; 24 names are accepted and a 25th fails the route (`harness_invalid_route`).
- `docs/guides/harness_core_guide.md` now names both codes and says the flag list guards against mistakes and is not a sandbox.

**Evidence (native Windows, Rust 1.92.0):** `cargo +1.92.0 fmt -p vibemux_harness_native -- --check` clean; `cargo +1.92.0 clippy -p vibemux_harness_native --all-targets --all-features -- -D warnings` clean; `cargo +1.92.0 test -p vibemux_harness_native --all-features` passed (16 unit, 18 normalization, 21 process dispatch). Four mutations each failed a new test: dropping `--yolo` from the list, matching `flag:` instead of `flag=`, removing the reserved-name check, and ignoring duplicate names.

**Remaining:** The native model field accepts a value that starts with `-`, which the daemon's shared route label check refuses; that parity fix is a separate change. The flag list is a deny list, not an allow list, so an operator prefix stays operator-trusted.

### 2026-10-18 - Native model names can no longer be read as a flag

**Scope:** `vibemux_harness_native` route validation and the harness core guide. No wire, schema, control protocol, or ACP policy change.

**Implemented:**

- A native route's `model` follows `--model` in the Claude stream-json and Codex exec argv. The daemon's shared route label check refuses a model that starts with `-`, but native validation checked only emptiness, a 256-byte bound, and NUL, so a model such as `--dangerously-skip-permissions` loaded and reached argv outside the `UNSAFE_ARGUMENTS` check.
- Native validation now also refuses a leading `-` (`harness_invalid_route` at load, like the other model checks) and takes its length bound from the shared `MAX_MODEL_BYTES`.
- The native check does not adopt the daemon's full label alphabet, so a model the native path accepted before still loads unless it starts with `-`.
- A unit test accepts `sonnet`, `gpt-5.1-codex`, `claude-sonnet-4-5`, and `openai/gpt-5`, and refuses an empty name, `-m`, `--dangerously-skip-permissions`, a name one byte over the bound, and a name with NUL. The guide states the rule.

**Evidence (native Windows, Rust 1.92.0):** `cargo +1.92.0 fmt -p vibemux_harness_native -- --check` clean; `cargo +1.92.0 clippy -p vibemux_harness_native --all-targets --all-features -- -D warnings` clean; `cargo +1.92.0 test -p vibemux_harness_native --all-features` passed (17 unit, 18 normalization, 21 process dispatch). Two mutations each failed the new test: removing the leading-dash check and loosening the bound by one byte.

**Remaining:** Whether native should adopt the daemon's full model alphabet is an owner decision, because it would refuse names the native path accepts today. Native environment names may still start with a digit, which the daemon refuses; that difference is harmless and unchanged.

### 2026-10-18 - Native configuration and route-shape refusals are pinned

**Scope:** `vibemux_harness_native` configuration validation tests. No behavior, wire, schema, or control protocol change.

**Implemented:**

- Apart from the checks pinned earlier today, no test exercised the native configuration's shape refusals: a wrong schema version, no routes, or more than `MAX_ROUTES` (`harness_invalid_config`); invalid limits (`harness_invalid_limits`); and, per route, a duplicate harness, a protocol that does not accept the harness, an empty executable or working directory, or arguments over the count, per-argument, or total byte bound, or containing NUL (`harness_invalid_route`).
- The inline bounds are now named constants beside the other configuration bounds, with unchanged values: `MAX_ARGUMENTS` (24), `MAX_ARGUMENT_BYTES` (4096), `MAX_TOTAL_ARGUMENT_BYTES` (12 KiB), and `MAX_ENVIRONMENT_NAME_BYTES` (128).
- Two unit tests refuse each shape with its code and accept the bounds themselves: 24 arguments totalling exactly 12 KiB, one 4096-byte argument, and a second route for another harness.

**Evidence (native Windows, Rust 1.92.0):** `cargo +1.92.0 fmt -p vibemux_harness_native -- --check` clean; `cargo +1.92.0 clippy -p vibemux_harness_native --all-targets --all-features -- -D warnings` clean; `cargo +1.92.0 test -p vibemux_harness_native --all-features` passed (19 unit, 18 normalization, 21 process dispatch). Five mutations each failed a new test: loosening the total argument bound by one byte, dropping the protocol-accepts check, disabling the duplicate-harness check, dropping the route cap, and refusing an argument of exactly 4096 bytes.

**Remaining:** The file-level load refusals (a missing file, a file over `MAX_CONFIG_BYTES`, and malformed JSON or an unknown field) still have no test. The owner decisions on load-time refusal of Windows non-`.exe` executables, ACP routes with a model, and the daemon's full model alphabet are unchanged.

### 2026-10-18 - Native configuration file loading is pinned

**Scope:** `vibemux_harness_native` configuration loading tests. No behavior, wire, schema, or control protocol change.

**Implemented:**

- No test exercised `Config::load` itself: a missing file (`harness_config_unavailable`), a file over `MAX_CONFIG_BYTES` (`harness_config_too_large`), malformed JSON or an unknown field at the top level, in `limits`, or in a route (`harness_invalid_config`), and a structurally invalid route surfacing its validation code at load.
- Nothing pinned that relative `executable` and `working_directory` paths resolve against the configuration file's directory rather than the caller's current directory, as the harness core guide states, or that the snapshot digest is the SHA-256 of the exact file bytes.
- Two unit tests now load files from a temporary directory. They accept a file of exactly `MAX_CONFIG_BYTES` and refuse one byte more, check each refusal code, resolve a relative route under the configuration's canonical directory, leave an absolute route unchanged, and compare the digest with an independently computed SHA-256.

**Evidence (native Windows, Rust 1.92.0):** `cargo +1.92.0 fmt -p vibemux_harness_native -- --check` clean; `cargo +1.92.0 clippy -p vibemux_harness_native --all-targets --all-features -- -D warnings` clean; `cargo +1.92.0 test -p vibemux_harness_native --all-features` passed (21 unit, 18 normalization, 21 process dispatch). Four mutations each failed a new test: refusing a file of exactly the bound, dropping `deny_unknown_fields` from `Route`, not resolving a relative working directory, and hashing all but the first byte.

**Remaining:** Of the eight `Limits` ranges, only `timeout_ms` has a test; the others are the next coverage gap. The owner decisions on load-time refusal of Windows non-`.exe` executables, ACP routes with a model, and the daemon's full model alphabet are unchanged.

### 2026-10-18 - Native limit ranges are documented and pinned

**Scope:** `vibemux_harness_native` limit validation tests and the harness core guide. No behavior, wire, schema, or control protocol change.

**Implemented:**

- The native configuration's eight `limits` ranges were not documented, and only `timeout_ms` had a test. The guide said only that invalid limits are rejected.
- `docs/guides/harness_core_guide.md` now has a table of each field's default and accepted range, and says every field is optional and an out-of-range value rejects the whole configuration with `harness_invalid_limits`.
- A table-driven unit test checks each default against `Limits::default()`, accepts each field at its lowest and highest value, refuses one below and one above, and checks that the capture floor follows `frame_bytes`. The same test asserts that the guide contains each row, so the table cannot drift from the code.

**Evidence (native Windows, Rust 1.92.0):** `cargo +1.92.0 fmt -p vibemux_harness_native -- --check` clean; `cargo +1.92.0 clippy -p vibemux_harness_native --all-targets --all-features -- -D warnings` clean; `cargo +1.92.0 test -p vibemux_harness_native --all-features` passed (22 unit, 18 normalization, 21 process dispatch). Five mutations each failed the new test: raising the `record_count` ceiling, lowering the `shutdown_ms` floor, fixing the capture floor at 64, editing the guide's `queue_capacity` row, and changing the `shutdown_ms` default.

**Remaining:** None for native configuration validation coverage. The owner decisions on load-time refusal of Windows non-`.exe` executables, ACP routes with a model, and the daemon's full model alphabet are unchanged.

### 2026-10-18 - Native request validation is pinned and documented

**Scope:** `vibemux_harness_native` request contract tests and the harness core guide. No behavior, wire, schema, or control protocol change.

**Implemented:**

- `Request::validate` had no direct test, and the guide did not state the native prompt rules. It refuses a schema version other than 1 or a nil request UUID with `harness_invalid_request`, and a blank prompt or one over 128 KiB with `harness_invalid_prompt`. The 128 KiB bound is deliberately larger than the daemon's 32 KiB dispatch prompt limit, because each caller of the shared engine owns its prompt-size policy.
- A unit test accepts a prompt of exactly `MAX_PROMPT_BYTES`, including one made of two-byte characters, and refuses an empty prompt, a whitespace-only prompt, and one byte over the bound. It also refuses the next schema version and a nil UUID.
- The "Explicit Execution" section of `docs/guides/harness_core_guide.md` now states these rules and the separate daemon limit. The test asserts that the guide names the same bound.

**Evidence (native Windows, Rust 1.92.0):** `cargo +1.92.0 fmt --all -- --check` clean; `cargo +1.92.0 clippy -p vibemux_harness_native --all-targets --all-features -- -D warnings` clean; `cargo +1.92.0 test -p vibemux_harness_native --all-features` passed (23 unit, 18 normalization, 21 process dispatch). Six mutations each failed the new test: dropping the prompt `trim`, refusing a prompt of exactly the bound, dropping the nil UUID check, accepting any non-zero schema version, counting characters instead of bytes, and changing the guide's bound to 32 KiB.

**Remaining:** `Config::catalog()` has no direct test of its per-protocol `structured_probe` and `native_execution` flags or of the fields it omits; the CLI output tests build their catalog JSON by hand. The owner decisions on load-time refusal of Windows non-`.exe` executables, ACP routes with a model, and the daemon's full model alphabet are unchanged.

### 2026-10-18 - Native catalog output is pinned and its actions documented

**Scope:** `vibemux_harness_native` catalog tests, a `vibemuxctl` output test, the harness core guide, and `docs/protocol_boundaries.md`. No behavior, wire, schema, or control protocol change.

**Implemented:**

- `Config::catalog()` had no direct test. The `vibemuxctl` output tests build catalog JSON by hand, so nothing checked the real per-protocol `structured_probe` and `native_execution` flags, that disabled routes stay listed, or that a route's executable, working directory, arguments, environment names, and model stay out of the output.
- A unit test builds one route per native protocol, each with private paths, arguments, environment names, and a model, and compares every catalog row with an exact expected object. `codex_exec` executes without a probe; `codex_app_server`, `claude_stream_json`, and `acp` both probe and execute; the replay-only OpenCode, Copilot, and Grok JSON protocols do neither. The ACP row records today's native execution, which stays an open owner decision.
- The guide and `docs/protocol_boundaries.md` listed the text `Actions` column values as `disabled`, `probe`, `execute`, or `none`, but a route that can do both shows `execute,probe`. Both now list all five values, and the `vibemuxctl` output test asserts that each document names each value. The guide also states which fields the catalog reports and which route settings it never includes.

**Evidence (native Windows, Rust 1.92.0):** `cargo +1.92.0 fmt --all -- --check` clean; `cargo +1.92.0 clippy -p vibemux_harness_native -p vibemux_cli --all-targets --all-features -- -D warnings` clean; `cargo +1.92.0 test -p vibemux_harness_native -p vibemux_cli --all-features` passed (native 24 unit, 18 normalization, 21 process dispatch; CLI 43 unit, 4 bootstrap process, 6 unified commands). Seven mutations each failed a test: gating `native_execution` on the shared engine's execution support, reporting a probe for every engine protocol, adding the executable to a row, dropping disabled routes, reporting `enabled` as `allow_execution`, and removing `execute,probe` from either document.

**Remaining:** The owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, and the daemon's full model alphabet are unchanged.

### 2026-10-18 - Native frame and capture bounds are pinned at their edges

**Scope:** `vibemux_harness_native` JSON-lines reader tests and the harness core guide. No behavior, wire, schema, or control protocol change.

**Implemented:**

- The integration tests covered an oversized frame, invalid UTF-8, a blank line, a final line without a newline, CRLF, cancellation safety, and the record count refusal. Nothing pinned the exact edges: `frame_bytes` counts a record's line including its line ending, so the same JSON fits a bound with LF that it overruns with CRLF, while `capture_bytes` counts only the JSON text. The guide did not state either rule.
- Two unit tests in `json_lines.rs` check these edges:
  - LF and CRLF records at and one byte over the frame bound, and a final unterminated line that fills the whole bound.
  - A CRLF-only line refused as empty, and a zero bound refused as invalid limits.
  - A replay that reaches `capture_bytes` and `record_count` exactly and is refused one byte or one record past them, with the records returned whole and numbered from 1.
  - A replay whose limits are invalid, refused before anything is read.
- The guide now explains how `frame_bytes` and `capture_bytes` count, names the two refusal codes, and states that no record is truncated. The frame test asserts the guide's wording.

**Evidence (native Windows, Rust 1.92.0):** `cargo +1.92.0 fmt --all -- --check` clean; `cargo +1.92.0 clippy -p vibemux_harness_native --all-targets --all-features -- -D warnings` clean; `cargo +1.92.0 test -p vibemux_harness_native --all-features` passed (26 unit, 18 normalization, 21 process dispatch). The runner error code allowlist test, which scans `json_lines.rs`, still passes. Eight mutations each failed a new test:

- refusing a line of exactly the frame bound;
- not counting the LF;
- not stripping the CR;
- accepting a zero bound;
- refusing a capture of exactly the bound;
- allowing one record past `record_count`;
- ignoring invalid limits in replay;
- rewording the guide.

**Remaining:** The owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, and the daemon's full model alphabet are unchanged.

### 2026-10-18 - Native CLI parsing and request file bounds are pinned

**Scope:** `vibemux_harness_native` command-line tests and the harness core guide. No behavior, wire, schema, or control protocol change.

**Implemented:**

- The native parser, which `vibemuxctl harness` also uses, had two tests. They covered a missing option, a duplicate option, an unknown harness, and an unknown option.
- Nothing tested the following:
  - the help forms and `--version`;
  - an unknown command;
  - a missing or empty option value, or a stray trailing argument;
  - an option that belongs to another command;
  - protocol and harness names in the wrong case;
  - option order;
  - arguments that are not Unicode;
  - the bounded file reader;
  - the request file bound, `6 × MAX_PROMPT_BYTES + 4096`, which exists so that a maximum prompt written entirely in six-byte `\u00XX` escapes still fits;
  - the rule that a dispatch request is validated before its configuration is opened.
- Three unit tests now cover each of these:
  - **Parsing.** Six help forms and `--version` parse. Fourteen malformed commands are usage errors. Replay, probe, and dispatch options parse in any order, with an optional plugin.
  - **Non-Unicode arguments.** A configuration path that is not Unicode is kept as given. A command, option name, or harness id that is not Unicode is refused.
  - **Request files.** The reader accepts a file of exactly its bound and refuses one byte more or a missing file. A dispatch request is refused for malformed JSON, a nil UUID, a blank prompt, or a file one byte over its bound. A maximum prompt of escaped control characters reaches the missing configuration, which shows the bound fits it.
- The guide states that the request file is checked before the configuration is opened, and why its bound fits an all-escaped maximum prompt; the test pins that sentence.

**Evidence (native Windows, Rust 1.92.0):** `cargo +1.92.0 fmt --all -- --check` clean; `cargo +1.92.0 clippy -p vibemux_harness_native --all-targets --all-features -- -D warnings` clean; `cargo +1.92.0 test -p vibemux_harness_native --all-features` passed (29 unit, 18 normalization, 21 process dispatch). Nine mutations each failed a new test:

- accepting an empty value;
- skipping the per-command option list;
- ignoring argument parity;
- dropping `-h` after a command;
- refusing a file of exactly the bound;
- shrinking or growing the request bound;
- skipping request validation;
- rewording the guide.

**Remaining:** The owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, and the daemon's full model alphabet are unchanged.

### 2026-10-18 - Native plugin request admission is a pure, tested function

**Scope:** `vibemux_harness_native` plugin request admission, the shared request size bound, and their tests. No behavior, wire, schema, or control protocol change.

**Implemented:**

- The plugin's request checks sat inline in its stdio loop and had no test. They guard against a misbehaving core: a second request in one session, an ungranted or unknown method, under 100 ms left before the deadline, an oversized payload, and a payload whose UUID, correlation, or harness does not match. The dispatcher always sends well-formed requests, so the process tests never reach these checks. The response mapping from completion to wire status was untested too.
- The checks moved unchanged into a pure `admit` function, which the loop now calls. The dispatcher's error code allowlist test still scans that block.
- The request bound `MAX_PROMPT_BYTES * 6 + 4096` was written out separately in the CLI and the plugin. It is now one documented `MAX_REQUEST_BYTES` constant in the contract.
- Three unit tests cover the new structure:
  - **Admission.** An execute request and a blank-prompt probe are admitted; an execute request with a blank prompt is refused. Fourteen edits each yield their fixed code, and the refusal order holds when two checks fail together. A probe still refuses a wrong schema version or a nil UUID, because it skips `Request::validate`. Exactly 100 ms left and a payload of exactly `MAX_REQUEST_BYTES` are admitted.
  - **Completion mapping.** Completed and probed answer `Ok`, cancelled answers `Cancelled`, and failed or unverified answer `Error`. A failure without a code gets `harness_vendor_failed`, and the payload is the unchanged completion.
  - **Frame fit.** The largest admitted request fits the default plugin frame that the dispatcher's plugin policy uses.

**Evidence (native Windows, Rust 1.92.0):** `cargo +1.92.0 fmt --all -- --check` clean; `cargo +1.92.0 clippy -p vibemux_harness_native -p vibemux_cli --all-targets --all-features -- -D warnings` clean; `cargo +1.92.0 test -p vibemux_harness_native -p vibemux_cli --all-features` passed (native 32 unit, 18 normalization, 21 process dispatch through the real plugin binary; CLI 43 unit, 4 bootstrap process, 6 unified commands). Twelve mutations each failed a new test. They weakened, in turn, the used-session, method, deadline, size, schema, nil UUID, correlation, harness, and probe-only prompt checks; mapped unverified to cancelled; dropped the failure code fallback; and raised the shared bound past the plugin frame.

**Remaining:** The owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, and the daemon's full model alphabet are unchanged.

### 2026-10-18 - Native launch environment and executable check are pure and tested

**Scope:** The launch helpers in `vibemux_harness_native` process code, plus the harness core guide. No wire, schema, or control protocol change. There is one corner-case behavior alignment with the daemon, described below.

**Implemented:**

- The native path builds every launch environment the same way, for both the plugin and the vendor, but its environment builder had no test. It kept its own copy of the OS startup names, which now come from the shared engine's `SYSTEM_ENVIRONMENT_NAMES`. The builder is now a pure function that takes a lookup.
- The tested rules:
  - A launch inherits only the startup set (`SYSTEMROOT`, `WINDIR`, `TEMP`, `TMP`) and the route's named variables. No other variable passes, not even `PATH`.
  - A startup variable passes only when it is set and valid Unicode.
  - A named variable that is unset or not valid Unicode fails with `harness_environment_unavailable`.
- **Alignment:** a route that names a startup variable now treats it as optional, as the daemon's launch already did through `build_launch_spec`. Before, the native path failed when such a variable was unset.
- The Windows `.exe` requirement is now a pure `is_native_executable` check, tested on every platform. The test covers letter case, a double extension, and the `.cmd`, `.bat`, `.ps1`, `.com`, extensionless, trailing-dot, and bare-dot names. The check still runs only on Windows, and the process test for `.cmd` and `.bat` shims is unchanged.
- The guide now names the startup set and says what an unset or non-Unicode variable does. A test checks the guide for this.

**Evidence (native Windows, Rust 1.92.0):** `cargo +1.92.0 fmt --all -- --check` clean; `cargo +1.92.0 clippy -p vibemux_harness_native -p vibemux_cli --all-targets --all-features -- -D warnings` clean; `cargo +1.92.0 test -p vibemux_harness_native -p vibemux_cli --all-features` passed (native 34 unit, 18 normalization, 21 process dispatch; CLI 43 unit, 4 bootstrap process, 6 unified commands). Six mutations each failed a new test rather than compilation: requiring a route-named startup variable, forwarding an unset startup name as empty, forwarding `PATH`, skipping an unset route variable, a case-sensitive `.exe` match, and accepting a path with no extension.

**Remaining:** The plugin supervisor's launch contract holds only Unicode values, so the native path still refuses non-Unicode values that the daemon would pass through. The owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, and the daemon's full model alphabet are unchanged.

### 2026-10-18 - Native completion fold is pinned to the daemon's outcome rule

**Scope:** The completion fold in `vibemux_harness_native` native code and the harness core guide. No behavior, wire, schema, or control protocol change.

**Implemented:**

- At the end of a native exchange, the runner turns three things into the completion's status and code: the exchange's error, the protocol's terminal status, and the vendor's exit. This fold was inline and had no direct test. No test reached the `harness_process_failed` branch, where a vendor reports a terminal event but then exits unclean.
- The fold moved unchanged into a pure `settle` function.
- A parity test runs it against the shared engine's `decide_outcome`, the rule the daemon uses. It covers all 105 combinations of three exchange results, five terminal states, and seven exits, and the native status must match the daemon's outcome in every case.
- The codes match too, with two documented differences:
  - The native path uses its own names, `harness_eof_without_terminal` and `harness_process_failed`.
  - A vendor-reported failure carries its `failed` status without a code. The plugin's wire answer still supplies `harness_vendor_failed`.
- The guide now says that a terminal event stands only after a clean vendor exit, and that an exchange error keeps its own code. A test checks the guide for this.

**Evidence (native Windows, Rust 1.92.0):** `cargo +1.92.0 fmt --all -- --check` clean; `cargo +1.92.0 clippy -p vibemux_harness_native -p vibemux_cli --all-targets --all-features -- -D warnings` clean; `cargo +1.92.0 test -p vibemux_harness_native -p vibemux_cli --all-features` passed (native 35 unit, 18 normalization, 21 process dispatch; CLI 43 unit, 4 bootstrap process, 6 unified commands). Five mutations each failed the parity test:
- letting an unclean exit outrank an exchange error;
- ignoring forced termination;
- misspelling the cancellation code;
- turning end of input without a terminal into `failed`;
- coding vendor failures.

**Remaining:** The owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, and the daemon's full model alphabet are unchanged.

### 2026-10-18 - OpenCode replay classification is pinned record by record

**Scope:** The `vibemux_harness_native` normalization tests and the harness core guide. No behavior, wire, schema, or control protocol change.

**Implemented:**

- The direct `opencode_json` protocol is the only stream this crate classifies itself; every other protocol uses the shared ADR 029 classifiers. Its only test was one fixture, and that fixture had no `reasoning` or `error` record, no cost-only usage, and no malformed usage data.
- A table test now pins every classifier branch:
  - `step_start` is started.
  - `text` and `reasoning` are text.
  - `tool_use` is tool activity.
  - `error` is a failure.
  - A `step_finish` is usage only when its part carries an object `tokens` field or a numeric `cost`.
- The test also pins the opaque cases:
  - a step ending with no usage data, string or non-object usage, or usage outside `part`;
  - near-miss discriminants (case, padding, a shorter name, an array, a nested type);
  - final-sounding records such as `step_finish` with `end_turn`, `session.idle`, and `done`. No OpenCode record completes a request.
- The guide's replay paragraph now says this, and the test checks the guide for it.

**Evidence (native Windows, Rust 1.92.0):** `cargo +1.92.0 fmt --all -- --check` clean; `cargo +1.92.0 clippy -p vibemux_harness_native -p vibemux_cli --all-targets --all-features -- -D warnings` clean; `cargo +1.92.0 test -p vibemux_harness_native -p vibemux_cli --all-features` passed (native 35 unit, 19 normalization, 21 process dispatch; CLI 43 unit, 4 bootstrap process, 6 unified commands). Six classifier mutations failed only the new test:
- dropping `reasoning`;
- accepting any non-null `tokens`;
- accepting any non-null `cost`;
- dropping cost-only usage;
- making `error` opaque;
- completing on `end_turn`.

**Remaining:** The owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, and the daemon's full model alphabet are unchanged.

### 2026-10-18 - Frontend probe labels have one source

**Scope:** The `vibemux_frontend` view model, TUI table renderer, and read-only dashboard model. No behavior, output, wire, schema, or control protocol change.

**Implemented:**

- The view model's `state_label` and `route_label` are documented as the labels both UIs and the snapshot use. The TUI table renderer and the dashboard model each kept a private copy instead, so the TUI table and its copied snapshot printed the same words from two definitions. The dashboard's placeholder row for a harness with no observation spelled `not_run` and `unknown` as literals.
- Both copies are removed. The renderer and the dashboard now call the view model's functions, and the placeholder row asks them for the not-run and unknown labels.
- The view model's tests checked only that each label was lowercase ASCII. They now pin each label exactly, since operators read and copy them from all three views.
- The GUI's title-case `Not run` style labels are display text, not these machine labels, and are unchanged.

**Evidence (native Windows, Rust 1.92.0):** `cargo +1.92.0 fmt --all -- --check` clean; `cargo +1.92.0 clippy -p vibemux_frontend -p vibemux_cli --all-targets --all-features -- -D warnings` clean; `cargo +1.92.0 test -p vibemux_frontend --all-features` passed (131 unit with 1 pre-existing ignore, plus the integration suites) and `cargo +1.92.0 test -p vibemux_cli --all-features` passed (43 unit, 4 bootstrap process, 6 unified commands). Two mutation probes confirmed the views now share one source:
- Renaming `not_run` in the view model failed the pinned label test, three dashboard tests, and three TUI render tests, including the golden snapshots.
- Renaming `local_gateway` failed the pinned label test and three TUI render tests.

**Remaining:** The probe crate's snapshot formatter keeps its own inline state mapping in another crate, and is out of scope here. The owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, and the daemon's full model alphabet are unchanged.

### 2026-10-18 - One wire name for each probe state

**Scope:** `vibemux_harness` agent vocabulary, the `vibemux_probe` snapshot, and the frontend view model. No wire, store, or protocol change.

**Implemented:**

- `ProbeState::as_str` returns the name serde writes for each state. The existing wire-name test now checks it against the JSON encoding of every variant.
- The probe snapshot's `reported_<state>` fallback code and the frontend's `state_label`, which the TUI, its copied snapshot, and the dashboard use, now take the name from `as_str` instead of keeping their own copies of the mapping.

**Evidence (native Windows, Rust 1.92.0):** `cargo fmt --all -- --check` passed. `cargo clippy -p vibemux_harness -p vibemux_probe -p vibemux_frontend --all-targets --all-features -- -D warnings` passed. `cargo test -p vibemux_harness -p vibemux_probe -p vibemux_frontend --all-features` passed: harness 86 unit, frontend 131 unit with 1 pre-existing ignore, and the probe suites. A mutation that renamed `not_run` in `as_str` failed the harness wire-name test, the frontend label test, and the TUI and dashboard render tests; the file was restored.

**Remaining:** owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, and the daemon's full model alphabet. New open question: the TUI diagnostics pane and its copied snapshot print allowlisted environment values such as `PATH` and `USERPROFILE`, while the GUI shows only whether each is set.

### 2026-10-18 - One wire name for each route kind

**Scope:** `vibemux_probe` route vocabulary and the frontend view model. No wire, cache, or protocol change.

**Implemented:**

- `RouteKind::as_str` returns the name serde writes for each route kind. A new probe test pins the JSON names (`direct`, `local_gateway`, `unknown`), which the probe report and its trusted cache both carry and no test had pinned, and checks `as_str` against them.
- The frontend's `route_label`, which the TUI, its copied snapshot, and the dashboard use, now takes the name from `as_str` instead of keeping its own copy of the mapping.

**Evidence (native Windows, Rust 1.92.0):** `cargo fmt --all -- --check` passed. `cargo clippy -p vibemux_probe -p vibemux_frontend --all-targets --all-features -- -D warnings` passed. `cargo test -p vibemux_probe -p vibemux_frontend --all-features` passed: probe 22 unit plus its integration suites, frontend 131 unit with 1 pre-existing ignore. A mutation that renamed `local_gateway` in `as_str` failed the new probe test, the frontend label test, and the TUI render tests; the file was restored.

**Remaining:** owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, and whether the TUI diagnostics pane and its copied snapshot should show allowlisted environment variables as set or unset only, like the GUI.

### 2026-10-18 - Test the GUI agent view text

**Scope:** the egui Agents view in `vibemux_frontend`. No wire, store, or protocol change.

**Implemented:**

- The agent list's summary line and the agent detail rows are now built by two pure functions, `agent_summary` and `agent_details`, instead of inline in the egui layout. The rendered text is unchanged.
- New unit tests, the first for this view, pin the summary line and all seven detail rows in order for Codex, check that Kimi still shows its `terminal_only` adapter as a build fact, and pin the GUI's title-case probe state labels.

**Evidence (native Windows, Rust 1.92.0):** `cargo fmt --all -- --check` passed. `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings` passed. `cargo test -p vibemux_frontend --all-features` passed: 133 unit with 1 pre-existing ignore, plus the integration suites. A mutation that showed the result code in the Version row failed the new test; the file was restored.

**Remaining:** owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, and whether the TUI diagnostics pane and its copied snapshot should show allowlisted environment variables as set or unset only, like the GUI.

### 2026-10-18 - Correct two harness API doc comments

**Scope:** documentation comments in `vibemux_harness::dispatch`. No code, wire, store, or protocol change.

**Implemented:**

- `AdapterCapability::summary` no longer wraps its example across two comment lines, which made `codex_exec,codex_app_server` read as if it had a space after the comma. The comment now says the protocol list has none, which the existing summary test pins. It also no longer claims that every frontend uses this line.
- `Sha256Digest::of_fields` no longer links to the crate-private `hash_fields`. That broken link was the only thing stopping `cargo doc -p vibemux_harness --no-deps` from building with warnings denied.

**Evidence (native Windows, Rust 1.92.0):** `cargo fmt --all -- --check` passed. `cargo clippy -p vibemux_harness --all-targets --all-features -- -D warnings` passed. `cargo test -p vibemux_harness --all-features capability` passed: 6 tests. With `RUSTDOCFLAGS="-D warnings"`, `cargo doc -p vibemux_harness --no-deps` failed before this change on the private link and passed after it. CI does not build rustdoc. The generated `target/doc` output was removed.

**Remaining:** owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, and whether the TUI diagnostics pane and its copied snapshot should show allowlisted environment variables as set or unset only, like the GUI.

### 2026-10-18 - Workspace rustdoc builds with warnings denied

**Scope:** documentation comments in `vibemuxd` and `vibemux_platform`. No code, wire, store, or protocol change.

**Implemented:**

- Six public doc comments linked to private items: `WORKFLOW_EVALUATION_DEADLINE`, `SHUTDOWN_JOIN_MARGIN`, `release_runtime_dir`, and `SHUTDOWN_GRACE` in `vibemuxd`, and the `windows_acl_native` and `windows_security` modules in the `vibemux_platform` crate docs. Each link is now a plain code span. The wording is unchanged.

**Evidence (native Windows, Rust 1.92.0):** `cargo doc --workspace --exclude vibemux_web --no-deps --all-features` reported these six warnings and no others. After the change, the same command with `RUSTDOCFLAGS="-D warnings"` passed. `cargo fmt --all -- --check` passed, and `cargo clippy -p vibemuxd -p vibemux_platform --all-targets --all-features -- -D warnings` passed. The generated `target/doc` output was removed. CI still has no rustdoc step, so nothing stops a new broken link.

**Remaining:** owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, and whether the TUI diagnostics pane and its copied snapshot should show allowlisted environment variables as set or unset only, like the GUI. Whether CI should build rustdoc with warnings denied is also open.

### 2026-10-18 - Pin the GUI's presence-only environment display

**Scope:** the egui Diagnostics window in `vibemux_frontend`. No wire, store, or protocol change.

**Implemented:**

- The rows of the allowlisted environment section are now built by a pure `environment_presence` function instead of inline in the egui layout. The window still shows each name with only `set` or `unset`.
- The window's footnote promises that variable values are not displayed. A new unit test, the first for this window, pins that promise: a set `PATH` value, an unset `HOME`, and an empty `USERPROFILE` produce only names and `set` or `unset`, in order.

**Evidence (native Windows, Rust 1.92.0):** `cargo fmt --all -- --check` passed. `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings` passed. `cargo test -p vibemux_frontend --all-features` passed: 134 unit with 1 pre-existing ignore, plus the integration suites. A mutation that swapped `set` and `unset` failed the new test; the file was restored.

**Remaining:** owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, and whether the TUI diagnostics pane and its copied snapshot should also show allowlisted environment variables as set or unset only.

### 2026-10-18 - Test the documented native harness commands

**Scope:** `vibemux_harness_native` CLI tests. No code, wire, store, or protocol change.

**Implemented:**

- A new unit test reads the README, `docs/guides/harness_core_guide.md`, and `docs/guides/unified_cli_guide.md`, finds each `vibemux_harness_native.exe` and `vibemuxctl.exe harness` command line, and parses it with the real CLI parser. There are eight such lines today, and the test pins that count.
- The documented `catalog` and `replay` commands run against the public example configuration and fixture and must succeed. The documented `dispatch` command's public example request must pass every request check, so that only a missing configuration stops it. The probe and dispatch configurations the docs name are private `.vibemux/` paths, and the test never opens them; no vendor process starts.

**Evidence (native Windows, Rust 1.92.0):** `cargo fmt --all -- --check` passed. `cargo clippy -p vibemux_harness_native --all-targets --all-features -- -D warnings` passed. `cargo test -p vibemux_harness_native --all-features` passed: 36 unit, 19 normalization, 21 process dispatch. A README mutation that misspelled the replay protocol failed the test with `harness_cli_usage`. An example request mutated to schema version 2 failed it with `harness_invalid_request`. Both files were restored.

**Remaining:** owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, and whether the TUI diagnostics pane and its copied snapshot should show allowlisted environment variables as set or unset only.

### 2026-10-18 - Test the documented vibemuxctl commands

**Scope:** `crates/vibemux_cli/src/command.rs` tests only. No behavior, wire, schema, or documentation change.

**Implemented:**
- `every_documented_command_line_parses` reads `CLAUDE.md`, the README, `docs/guides/unified_cli_guide.md`, and `docs/workflow/branch_workflow.md`. It finds every `vibemuxctl.exe` command line (90 today) and parses each one with the real `vibemuxctl` parser, which is pure, so nothing runs and no path or daemon is touched. Each document must contain at least one such line.
- A `status|pause|cancel` token expands to each alternative, a `[--prompts]` token is tried present and absent, and each `<placeholder>` gets a well-formed value: a 64-character hex digest for `<sha256>` and `<confirmation_from_inspect>`, a UUID for the id placeholders, and a name for `<new_dir>`. A placeholder the test does not know fails it, so a new documented argument has to be classified.
- Every documented line parses today, so no document changed.

**Evidence (native Windows, Rust 1.92.0):** `cargo fmt --all -- --check` passed. `cargo clippy -p vibemux_cli --all-targets --all-features -- -D warnings` passed. `cargo test -p vibemux_cli --all-features --lib` passed: 44 tests. A README mutation that respelled `--prompt-file` as `--prompt_file` on a `dispatch submit` line failed the test and named the line's arguments. The README was restored and compared byte for byte.

**Remaining:** owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, and whether the TUI diagnostics pane and its copied snapshot should show allowlisted environment variables as set or unset only.

### 2026-10-18 - Test the DSH CLI synopsis

**Scope:** `crates/vibemux_cli/src/command.rs` tests only. No behavior, wire, schema, or documentation change.

**Implemented:**
- `every_documented_command_line_parses` now also reads `docs/dsh_session_protocol.md`. Besides each `vibemuxctl.exe` invocation, it parses each line that starts with a `vibemuxctl` synopsis, which covers the six `vibemuxctl dsh` synopsis lines there. Prose that only mentions `vibemuxctl` inside a sentence is not a command and is skipped.
- The synopsis grammar is expanded faithfully. An optional group that spans tokens, such as `[--after-sequence <n>]`, is tried present and absent as a whole. A placeholder that contains a bar, such as `<path|->`, is one argument and not two alternatives. The new placeholders are `<path>`, `<path|->`, and `<n>`.
- ADR synopses, such as ADR 029's dispatch grammar, are dated decision records and stay out of this check.
- Every line still parses, so no document changed.

**Evidence (native Windows, Rust 1.92.0):** `cargo fmt --all -- --check` passed. `cargo clippy -p vibemux_cli --all-targets --all-features -- -D warnings` passed. `cargo test -p vibemux_cli --all-features --lib` passed: 44 tests. A mutation that respelled `--session-id` as `--session_id` inside the `dsh open` synopsis's optional group failed the test on the group's present variant. The document was restored and compared byte for byte.

**Remaining:** owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, and whether the TUI diagnostics pane and its copied snapshot should show allowlisted environment variables as set or unset only.

### 2026-10-18 - One owner for GUI task labels

**Scope:** `crates/vibemux_frontend/src/gui/` display text only. No wire, schema, control, or canonical state change.

**Implemented:**
- The new `gui/task_labels.rs` owns the task title, state, and executor labels and the blank-text rule behind them. The chat task rows, the sidebar recents, the task window title, and the detail drawer all call it. Before, four copies disagreed:
  - The chat rows treated only an empty string as blank, and the other views also treated whitespace as blank.
  - The chat rows showed `Unassigned` for a blank executor, while the detail drawer fell back to the first Run's harness.
  - The chat rows passed a blank state straight to the status chip, while the detail drawer showed `Unknown state`.
- The shared rule:
  - A title, state, executor, or harness that is empty or only whitespace counts as missing.
  - The executor falls back to the first Run's harness, then to `Unassigned`.
  - Real text is shown verbatim.
- A test keeps the views from restating the `Untitled task` and `Unassigned` fallbacks themselves.

**Evidence (native Windows, Rust 1.92.0):** `cargo fmt --all -- --check` passed. `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings` passed. `cargo test -p vibemux_frontend --all-features` passed: 137 lib (1 ignored), 6 cli, 2 palette parity, 10 supervisor UI. Two mutations were each caught by their test:
- Restoring the chat row's own `Unassigned` fallback failed the view guard.
- Dropping the harness fallback from `executor_label` failed the executor test.

Both files were restored and compared byte for byte.

**Remaining:** owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, and whether the TUI diagnostics pane and its copied snapshot should show allowlisted environment variables as set or unset only.

### 2026-10-18 - GUI Run labels and terminal link enablement follow the blank-text rule

**Scope:** `crates/vibemux_frontend/src/gui/task_labels.rs` and `task_detail.rs` display text and button enablement only. No wire, schema, control, or canonical state change.

**Implemented:**
- `task_labels.rs` now also owns the Run labels. `single_run_label` is the caption for a task's only Run. Before, it printed a blank harness as an empty gap (`Run ·  · <id>`); now it shows the same `Harness` fallback as `run_label`, the Run picker entry that moved here from `task_detail.rs`.
- The detail drawer's `Link terminal` button is enabled by `candidate_is_linkable`. It requires the plugin, pane, and instance identifiers of the exact reported candidate, and an identifier that is only whitespace now counts as missing, as everywhere else in the GUI. The daemon's `link` still refuses a malformed pane or instance identifier with `terminal_invalid_request` and requires the instance and pane to match its live inventory; this change only stops the GUI from offering a link it cannot name.

**Evidence (native Windows, Rust 1.92.0):** `cargo fmt --all -- --check` passed. `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings` passed. `cargo test -p vibemux_frontend --all-features` passed: 139 lib (1 ignored), 6 cli, 2 palette parity, 10 supervisor UI. Two mutations were each caught by their test:
- Dropping the trim from `candidate_is_linkable` failed the linkability test.
- Dropping the fallback from `single_run_label` failed the Run label test.

Both files were restored and compared byte for byte.

**Remaining:** owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, and whether the TUI diagnostics pane and its copied snapshot should show allowlisted environment variables as set or unset only.

### 2026-10-18 - Probe text drops bidirectional marks

**Scope:** probe text sanitization and the one definition of terminal-unsafe characters, for `vibemux_harness`, `vibemux_probe`, `vibemux_cli`, and `vibemux_frontend`. No store schema, control protocol, plugin wire, or `HarnessRow` field changes.

**Implemented:**
- `vibemux_harness::terminal_text::is_terminal_unsafe` now owns the set of characters a terminal must never receive raw from untrusted text. The set is C0 and C1 controls, DEL, U+061C, U+200E-U+200F, U+2028-U+202E, and U+2066-U+2069. The module is pure logic, like the rest of the crate.
- The probe's `sanitize_text` used to drop only `char::is_control` characters, so a bidirectional override or isolate passed through. It now drops every terminal-unsafe character. A vendor `--version` line, a resolved launcher path, or a cc-switch `app_type` with such a mark now reaches the probe report, the probe cache, `vibemux_probe` stdout, the TUI, and the GUI without it. The byte bound still counts kept bytes and never splits a character.
- The CLI's JSON escaping and the dashboard's `safe_text` call the same predicate instead of keeping their own copies of the ranges. Their behaviour is unchanged: the CLI already escaped this set, and the dashboard's narrower range left out only U+2028 and U+2029, which `safe_text` already blanks as whitespace.

**Evidence (native Windows, Rust 1.92.0):** `cargo fmt --all -- --check` passed. `cargo clippy -p vibemux_harness -p vibemux_probe -p vibemux_frontend -p vibemux_cli --all-targets --all-features -- -D warnings` passed. `cargo test -p vibemux_harness -p vibemux_probe -p vibemux_frontend -p vibemux_cli` passed:

| Crate | Tests passed |
|---|---|
| `vibemux_harness` | 88 lib (2 new), plus 12, 22, 8, 15, and 4 integration |
| `vibemux_probe` | 23 lib (1 new) |
| `vibemux_frontend` | 139 lib (1 ignored), 6 cli, 2 palette parity, 10 supervisor UI |
| `vibemux_cli` | 44 lib, 5 unified commands |

Two mutations were each caught:
- Restoring the probe filter to `!character.is_control()` failed the new probe test.
- Narrowing the predicate's range to end at U+202D failed one test in each of the four crates: the harness predicate test, the probe sanitize test, the CLI JSON escaping test, and the dashboard control-character test.

Both files were restored and compared byte for byte.

**Remaining:** owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, and whether the TUI diagnostics pane and its copied snapshot should show allowlisted environment variables as set or unset only.

### 2026-10-18 - DSH terminal surface shows runtime text as one safe line

**Scope:** `vibemux_dsh_tui` rendering and the shared one-line display rule, for `vibemux_harness` and `vibemux_frontend`. No DSH wire, store schema, control protocol, or plugin wire changes. The handshake still accepts the same `serverInfo.version` strings.

**Implemented:**
- `vibemux_harness::terminal_text::display_character` now owns the one-line display rule: whitespace and terminal-unsafe characters show as a space. The dashboard's `safe_text` calls it in place of its own copy, with unchanged behaviour.
- The DSH surface's `display_text` reduces each runtime record to one display line before truncating it. Assistant text, tool names, approval tool names, status values, and error messages come from the runtime. A line break in them used to reach ratatui as text, and so did an ANSI escape or a bidirectional override. Each now shows as a space.
- The status line applies the same rule to the runtime's `serverInfo.version`. The handshake bounds that string's length but not its characters.

**Evidence (native Windows, Rust 1.92.0):** `cargo fmt --all -- --check` passed. `cargo clippy -p vibemux_harness -p vibemux_frontend -p vibemux_cli --all-targets --all-features -- -D warnings` passed. `cargo test -p vibemux_harness -p vibemux_frontend -p vibemux_cli` passed:

| Crate | Tests passed |
|---|---|
| `vibemux_harness` | 89 lib (1 new), plus 12, 22, 8, 15, and 4 integration |
| `vibemux_frontend` | 140 lib (1 new, 1 ignored), 6 cli, 2 palette parity, 10 supervisor UI |
| `vibemux_cli` | 44 lib, 5 unified commands |

Three mutations were each caught by their test:
- Truncating the raw record text failed the new DSH test's record assertion.
- Passing the raw version to the status line failed its status-line assertion.
- Dropping the whitespace check from `display_character` failed the harness test.

Both files were restored and compared byte for byte.

**Remaining:** owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, and whether the TUI diagnostics pane and its copied snapshot should show allowlisted environment variables as set or unset only.

### 2026-10-18 - GUI window size has one bounded owner

**Scope:** the GUI's persisted startup window size in `vibemux_frontend` (`theme/serialize.rs`, `gui/settings.rs`, `gui/supervisor_app.rs`). The `frontend.json` layout and schema version are unchanged.

An audit of `vibemuxctl`'s text output found nothing to change. Every daemon- or vendor-supplied string in the text views passes through `safe_text`, and so does the `ui --once` dashboard snapshot. Every other kind falls back to escaped JSON. The diagnostic TUI renders a live probe, whose text the probe already sanitizes.

**Implemented:**
- `WindowSize` now owns the settings range of 800-3840 by 600-2160 as constants, plus a `clamped` method. Before, three places repeated those literals: the settings dialog's two ranges and the two clamps in `supervisor_app.rs`. All of them now use the shared bounds.
- `load_user_config_at` clamps a loaded window size into that range. Before, a hand-edited or corrupt `frontend.json` with a width of 0 or 100000 passed straight to the viewport builder at startup, although the GUI never saves such a size. Only the size is touched; the theme and the sidebar setting load as before.
- The resize tracker no longer clamps on its own; `set_window_size` clamps once.

**Evidence (native Windows, Rust 1.92.0):** `cargo fmt --all -- --check` passed. `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings` passed. `cargo test -p vibemux_frontend --all-features` passed: 142 lib (2 new, 1 ignored), 6 cli, 2 palette parity, 10 supervisor UI. Two mutations were each caught by their test:
- Dropping the clamp on load failed the load test.
- Dropping the clamp in `set_window_size` failed the saved-size test.

Both files were restored and compared byte for byte.

**Remaining:** the window's minimum inner width is still 960, above the settings minimum of 800, so a saved width below 960 opens at 960. Changing either bound is a layout decision left to the owner. Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, and whether the TUI diagnostics pane and its copied snapshot should show allowlisted environment variables as set or unset only.

### 2026-10-18 - TUI binaries refuse non-Unicode arguments with the usage exit

**Scope:** the flag parsing of the two operator TUI binaries in `vibemux_frontend`: `vibemux_frontend_tui` and `vibemux_dsh_tui`. The flags, their messages, and exit code 4 are unchanged.

**Implemented:**
- Both binaries read their flags with `std::env::args`, which panics on an argument that is not valid Unicode. That meant exit code 101 and a panic message instead of the documented usage exit 4. Both now read `std::env::args_os` into a pure `parse_arguments` function, as `vibemuxctl` already does.
- A non-Unicode flag is reported as `unknown argument: ...` (lossy text), and a non-Unicode `--theme` value gets the existing theme error. Both exit 4.
- `vibemux_dsh_tui` keeps `--project-root` and `--config` values as `OsString` paths, so a project under a non-Unicode Windows path can be opened. Before, such a path panicked before the configuration was read.
- `vibemux_dsh_tui` now takes the working directory and the environment theme as parser inputs, so its tests read no process state.

**Evidence (native Windows, Rust 1.92.0):** `cargo fmt --all -- --check` passed. `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings` passed. `cargo test -p vibemux_frontend --all-features` passed:

| Target | Tests |
|---|---|
| lib | 142 (1 ignored) |
| `vibemux_dsh_tui` bin | 4 (new) |
| `vibemux_frontend_tui` bin | 2 (new) |
| cli | 6 (exit code 4 and stderr text unchanged) |
| palette parity | 2 |
| supervisor UI | 10 |

The built binaries were run with an argument holding a lone UTF-16 surrogate, both as a flag and as a `--theme` value. Each of the four runs exited 4 with the usage message.

Three mutations were each caught by their test:
- Converting the `--project-root` path through lossy text failed the path test.
- Accepting any non-Unicode `--theme` value failed the DSH usage-error test.
- Treating a non-Unicode flag as `--json` failed the diagnostic TUI usage-error test.

Both files were restored and compared byte for byte.

**Remaining:**
- `vibemux_frontend_preview`, `vibemux_harness_native_plugin`, the fixture binaries, and the `live_codex_smoke` example still read `std::env::args`.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800.

### 2026-10-18 - Native harness plugin refuses non-Unicode arguments with its usage code

**Scope:** the command line of `vibemux_harness_native_plugin`, the out-of-process plugin that the native dispatcher launches. The options, the `harness_plugin_usage` code, exit code 1, and the plugin wire are unchanged.

**Implemented:**
- The plugin read its options with `std::env::args`, which panics on an argument that is not valid Unicode. Run by hand with such an argument, it exited 101 with a panic message instead of its `harness_plugin_usage` code.
- It now parses `std::env::args_os` in a pure `parse_arguments` function. A non-Unicode option name, harness name, or digest returns `harness_plugin_usage`. The `--config` value is kept as an operating-system path.
- The parser had no direct tests; only the dispatcher's process tests exercised it. It now has four bin tests, covering:
  - options in any order;
  - a non-Unicode config path;
  - non-Unicode names and values;
  - a wrong count, a duplicate or unknown option, a short or non-hex digest, and an unknown harness.

**Evidence (native Windows, Rust 1.92.0):** `cargo fmt --all -- --check` passed. `cargo clippy -p vibemux_harness_native --all-targets --all-features -- -D warnings` passed. `cargo test -p vibemux_harness_native --all-features` passed: 36 lib, 4 plugin bin (new), 19 normalization, and 21 process dispatch. The process dispatch tests launch the real plugin through the dispatcher.

The built plugin was run with a lone UTF-16 surrogate, once as an option name and once as the harness name. Both runs exited 1 with `harness_plugin_usage`.

Three mutations were each caught by their test:
- Reading a non-Unicode option name as `--config` failed the non-Unicode test.
- Converting the config path through lossy text failed the path test.
- Dropping the digest's hex check failed the malformed-command-line test.

The file was restored and compared byte for byte.

**Remaining:**
- The dispatcher still passes the config path to the plugin as lossy text, because the plugin manifest's entry point is a list of strings. A config under a non-Unicode path therefore fails closed when the plugin reads it, rather than being misread. Changing that needs a plugin wire decision.
- `vibemux_frontend_preview`, the fixture binaries, and the `live_codex_smoke` example still read `std::env::args`.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800.

### 2026-10-18 - Screenshot preview refuses unknown scenes and themes

**Scope:** the command line of `vibemux_frontend_preview`, the screenshot-only binary built with `--features gui_screenshot`, plus one README sentence. The scenes, the defaults, the size and scale clamps, and the capture path are unchanged.

**Implemented:**
- An unknown scene used to fall back silently to `chat`, and an unknown theme to `claude_light`. A size or scale that was not a number fell back to its default. All of these exited 0. A typo, or one of the `switcher` and `shortcuts` scenes that plan 030 describes but the binary does not draw, therefore saved a screenshot of a different scene under the requested name.
- These cases now print a message naming the expected values and exit 4, before any window opens. So do a non-finite scale (`NaN`, `inf`) and a seventh argument.
- The binary read its arguments with `std::env::args`, which panics on an argument that is not valid Unicode. It now parses `std::env::args_os` in a pure `parse_arguments` function, and the screenshot path is kept as an operating-system path.
- The README now states the refusal.

**Evidence (native Windows, Rust 1.92.0):** `cargo fmt --all -- --check` passed. `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings` passed. `cargo test -p vibemux_frontend --all-features` passed: 142 lib (1 ignored), 6 preview bin (new), 4 DSH TUI bin, 2 diagnostic TUI bin, 6 cli, 2 palette parity, 10 supervisor UI.

The built binary was run with four refused argument lists: the `switcher` scene, the `solarized` theme, a width of `wide`, and a scene holding a lone UTF-16 surrogate. Each exited 4 with its message. No window was opened, and no screenshot capture was run for this change.

Five mutations were each caught, in two rounds so that no test masked another:
- Falling back to `chat` for an unknown scene failed the unknown scene or theme test.
- Falling back to `claude_light` for an unknown theme failed the same test.
- Accepting a non-finite scale failed the malformed-argument test.
- Allowing a seventh argument failed the same test.
- Converting the screenshot path through lossy text failed the path test.

The file was restored and compared byte for byte after each round.

**Remaining:**
- The preview's size clamps (720-3840 by 520-2160) are still separate from `WindowSize`'s 800-3840 by 600-2160. Aligning them is a choice about what the preview may show.
- The fixture binaries and the `live_codex_smoke` example still read `std::env::args`.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800.

### 2026-10-18 - Task window titles are one bounded line

**Scope:** the operating-system title of the GUI's detached task windows, in `vibemux_frontend` (`gui/task_labels.rs`, `gui/supervisor_app.rs`). The task title shown inside egui is unchanged.

An audit of the remaining frontend entry points found nothing else to change:
- `vibemux_dashboard` already parses `std::env::args_os`.
- `vibemux_frontend_dump` takes no arguments.
- The GUI's use of the default probe port is the documented scope of `VIBEMUX_FRONTEND_PROBE_PORT`, which the README limits to the TUI JSON and dump entry points.
- The welcome greeting and workspace hint are already tested.

**Implemented:**
- The GUI used to build a task window's title from the daemon-supplied task title with no change and no length limit. The operating system draws that title outside egui, in the title bar, the taskbar, and the window switcher. There a line break or a bidirectional override in a task title is honoured. For example, U+202E could make a title read as a different name.
- The new `task_window_title` in `task_labels.rs` owns that title. It maps every character through the shared `display_character` rule, so whitespace and terminal-unsafe characters become spaces. It cuts the title at 80 characters with an ellipsis and appends ` · VibeMux`.
- A title that is blank after mapping, such as one made only of bidirectional marks, falls back to `Untitled task`. A task the snapshot does not yet hold is titled `Task observation`, as before.

**Evidence (native Windows, Rust 1.92.0):** `cargo fmt --all -- --check` passed. `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings` passed. `cargo test -p vibemux_frontend --all-features` passed: 143 lib (1 new, 1 ignored), 4 DSH TUI bin, 6 preview bin, 2 diagnostic TUI bin, 6 cli, 2 palette parity, 10 supervisor UI.

Three mutations were each caught by the new test, one per round:
- Dropping the `display_character` mapping.
- Dropping the ellipsis.
- Skipping the blank-title fallback after mapping.

The file was restored and compared byte for byte after each round. The `supervisor_app.rs` call site was checked by review only. No GUI window was opened, so no title was observed in a real title bar.

**Remaining:**
- Task titles inside egui are still drawn verbatim, so a line break in a title still makes a multi-line row there. How egui draws bidirectional controls was not examined.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800.

### 2026-10-18 - GUI task and Run text is one display line

**Scope:** the free text the daemon returns for a task and its Runs, as it enters the GUI. The domain checks a task title, harness, role, branch, and worktree path only for blankness and size (`vibemux_types`), and the store's frontend projection only bounds bytes. So a line break split a chat row, a sidebar entry, or a detail-drawer field across lines, and a bidirectional control could reorder the text after it. The task window title was already one line (previous entry); the in-window views were not.

**Implemented:**

- `supervisor_mapping` maps the task title (before the truncation ellipsis), Run harness, role, branch, and worktree path through the shared `vibemux_harness::terminal_text::display_character` rule as it builds `TaskView` and `RunView`. This covers every view that reads them: chat task rows, sidebar recents, the task window, and the detail drawer. The executor label is formatted from the mapped harness and role.
- Ordinary text, including non-Latin script, is unchanged, and an absent worktree or branch keeps its placeholder.

**Evidence (native Windows, Rust 1.92.0):** `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings` clean. `cargo test -p vibemux_frontend --all-features`: lib 144 passed and 1 ignored, and every bin and integration target passed. The new test `task_and_run_text_becomes_one_display_line` failed under each of four separate mutations that dropped the mapping on the title, on the harness, on the worktree path, and on the branch; the source was restored byte-for-byte after each.

**Remaining:** the mapping replaces unsafe characters; it does not bound them further than the daemon already does. `task_labels::task_title` still keeps text verbatim, because the rule now applies one step earlier. Event types and artifact media types were not changed. How egui itself draws bidirectional text was not examined. Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800.

### 2026-10-18 - Terminal candidate captions are one display line

**Scope:** the WezTerm workspace name the terminal observer reports for each candidate pane, which the GUI shows as the candidate's caption in the Run's terminal panel. WezTerm lets the user name a workspace freely. `vibemux_plugin_protocol::terminal::valid_pane` refuses `char::is_control` characters in it, but bidirectional controls and the line and paragraph separators U+2028 and U+2029 are not control characters, so they reached the caption. Pane and window IDs must be ASCII digits, and the instance ID comes from the observer's own process-identity check, so neither needed a change.

**Implemented:**

- `supervisor_mapping::pane_display_name` builds the caption and maps the workspace through the same `display_character` rule the previous entry applied to task and Run text.
- A workspace that is blank, or blank after mapping, no longer leaves a trailing ` · ` after `Pane N · window M`.
- The pane, plugin, and instance IDs a link request sends back are unchanged.

**Evidence (native Windows, Rust 1.92.0):** `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings` clean. `cargo test -p vibemux_frontend --all-features`: lib 145 passed and 1 ignored, and every bin and integration target passed. The new test `a_terminal_candidate_caption_is_one_display_line` failed under each of three separate mutations: restoring the old inline caption, dropping the mapping, and dropping the blank-workspace branch. The source was restored byte-for-byte after each.

**Remaining:** the plugin protocol's own validation was not tightened, because that would change the plugin wire contract. No other surface renders the pane inventory: `vibemuxctl` has no terminal command, the dashboard's workspace tab lists Git branches, and `vibemux_mcp` and `vibemux_web` have no pane fields. Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800.

### 2026-10-18 - Artifact media types are one display line in the GUI

**Scope:** the remaining daemon text in the GUI's task detail drawer. Event types need no change: `vibemux_events::EventType` accepts only snake_case names, on construction and on deserialization. An artifact's ID is a UUID and its SHA-256 is validated hex. Its media type, however, passes the A2A `valid_label` rule (non-blank, at most 128 bytes, no `char::is_control`), which lets a line separator or bidirectional override through to the artifact row.

**Implemented:**

- `supervisor_mapping::task_detail` maps each artifact's media type through the shared `display_line` rule from the earlier entries today, for both the drawn `kind` and the stored `media_type`.

**Evidence (native Windows, Rust 1.92.0):** `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings` clean. `cargo test -p vibemux_frontend --all-features`: lib 146 passed and 1 ignored, and every bin and integration target passed. The new test `an_artifact_media_type_becomes_one_display_line` failed under each of two separate mutations, one dropping the mapping and one drawing the unmapped type, and the source was restored byte-for-byte after each.

**Remaining:** with this entry every free-text task, Run, artifact, and pane-caption field the supervisor mapping takes from the daemon passes through one display rule. Event times come from typed timestamps and binding IDs are daemon UUIDs. The terminal instance ID is still drawn verbatim, because a link request must echo it: the first-party WezTerm observer builds it from numbers (`wezterm_<pid>_<start>_<epoch>`), but the daemon accepts any terminal plugin's ID that passes the protocol's `valid_label`. The A2A `valid_label` rule itself was not tightened, because that would change accepted A2A artifact input. Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800.

### 2026-10-18 - Terminal candidate cards draw display copies of their identifiers

**Scope:** the one daemon field the previous entry left verbatim in the GUI, the terminal instance ID in each candidate card's "Instance" row. A link request must send it back exactly as reported, so it cannot be rewritten at the mapping boundary. The first-party WezTerm observer builds it from numbers, but the daemon accepts any terminal plugin's ID that passes the protocol's control-character check, which lets bidirectional controls and line separators through.

**Implemented:**

- `vibemux_harness::terminal_text::display_line` is the shared whole-string form of `display_character`. `supervisor_mapping` now uses it in place of its private copy.
- `gui::task_detail::candidate_rows` gives the card's Plugin, Pane, and Instance rows as display lines. The card draws those rows, while `candidate_is_linkable` and the `LinkTerminal` action still read the identifiers exactly as reported.

**Evidence (native Windows, Rust 1.92.0):** `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_harness -p vibemux_frontend --all-targets --all-features -- -D warnings` clean. `cargo test -p vibemux_harness --all-features`: lib 90 passed, and every integration target passed. `cargo test -p vibemux_frontend --all-features`: lib 147 passed and 1 ignored, and every bin and integration target passed. Mutations: drawing the raw instance ID failed `candidate_rows_draw_a_display_copy_of_each_identifier`. Making `display_line` return its input unchanged failed the new harness test `a_display_line_maps_every_character` and all three display tests in `supervisor_mapping`. Both sources were restored byte-for-byte.

**Remaining:** `dsh_tui::render::one_line` is the same mapping and could call `display_line`. That is a consolidation with no behavior change, so it was left out of this entry. The dashboard's `safe_text` and `task_labels::task_window_title` also cut at a length, so they keep their own chains. Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800.

### 2026-10-18 - DSH TUI uses the shared display line

**Scope:** the duplicate noted in the previous entry. `dsh_tui::render::one_line` was the same whole-string mapping as `vibemux_harness::terminal_text::display_line`.

**Implemented:**

- `dsh_tui::render` calls `display_line` for runtime record text and the runtime's version string, and its private `one_line` is gone. There is no behavior change.

**Evidence (native Windows, Rust 1.92.0):** `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings` clean. `cargo test -p vibemux_frontend --all-features`: lib 147 passed and 1 ignored, and every bin and integration target passed. Making `display_line` return its input unchanged failed the existing `runtime_text_is_one_line_without_terminal_controls`, so the TUI now depends on the shared rule. The source was restored byte-for-byte.

**Remaining:** the dashboard's `safe_text` and `task_labels::task_window_title` also cut at a length, so they keep their own chains. Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800.

### 2026-10-18 - Allowlisted environment values are one display line

**Scope:** text sources of the Ratatui diagnostic TUI and `vibemux_frontend_dump`. Both run the probe live and read no cache. Their probe text is already bounded: agent versions and gateway telemetry `app_type` pass the probe's `sanitize_text`, endpoint hosts come from `Url::host_str`, and the gateway host shown is the fixed `127.0.0.1`. The exception was the allowlisted environment display. The TUI's environment row, its `c` debug snapshot, and `vibemux_frontend_dump` print each value (PATH, HOME, the temp directories, and so on) after only a lossy UTF-8 conversion. On POSIX a directory name may hold a line break or an ANSI escape, so a value could start a new row or reach the terminal raw. The GUI shows set or unset only, and `--json` prints the probe report rather than these values.

**Implemented:**

- `probe_env::collect_allowlisted_env` reads through a new pure `allowlisted_values(read)`, which maps each value through the shared `vibemux_harness::terminal_text::display_line`. The keys, their order, unset values, and empty values are unchanged.
- The test injects a reader instead of changing the process environment.

**Evidence (native Windows, Rust 1.92.0):** `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings` clean. `cargo test -p vibemux_frontend --all-features`: lib 148 passed and 1 ignored, and every bin and integration target passed. Dropping the mapping failed the new test `allowlisted_values_are_one_display_line`, and the source was restored byte-for-byte.

**Remaining:** whether the diagnostics should show these values at all, or set or unset only as the GUI does, is still the owner's decision; this entry does not change what is shown, only how. Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800.

### 2026-10-18 - `dsh prompt` reads its prompt before selecting the daemon

**Scope:** how the CLI reads a prompt file. `dispatch submit` and `decide route` share `read_valid_prompt`, which reads on a blocking thread before `harness_client` selects the daemon and reports a source it cannot open or read as `cli_prompt_unavailable`. `dsh prompt` kept its own reader. It ran synchronously on the async runtime after the daemon had been selected, and it reported every failure as `cli_invalid_arguments`, including a missing file. Audits in this pass needed no change: every dashboard draw site goes through `safe_text`, CLI text output goes through `safe_text` and JSON through `escape_controls`, every production binary reads `args_os`, the probe cache read is bounded and re-checks the opened file, and the daemon validates each cached detection's path and version.

**Implemented:**

- `run_dsh` reads and checks the prompt first, through a new `read_prompt_off_runtime` (`spawn_blocking`), the same order `run_dispatch` and `run_decision` use.
- `read_prompt` reads at most one byte beyond the 32 KiB bound into bytes. A source it cannot open or read (a missing file, a directory) returns `cli_prompt_unavailable`. A blank, oversized, or non-UTF-8 prompt still returns `cli_invalid_arguments`.
- `docs/dsh_session_protocol.md` names both codes in its CLI section.
- No daemon, control protocol, or wire change.

**Evidence (native Windows, Rust 1.92.0):** `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_cli --all-targets --all-features -- -D warnings` clean. `cargo test -p vibemux_cli`: lib 45 passed, integration 5 passed. Two mutations were each caught and restored byte-for-byte. Selecting the daemon before the read failed `the_prompt_is_checked_before_the_daemon_is_selected`. Reporting an unreadable source as an invalid argument failed that test and `a_prompt_is_bounded_utf8_and_non_blank_and_an_unreadable_one_is_unavailable`.

**Remaining:** the DSH and dispatch readers are now near copies, each with its own `PromptSource` type and bound; one shared reader could own both. Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800.

### 2026-10-18 - MCP task titles follow the shared terminal-text rule

**Scope:** text boundaries of the native MCP read host (ADR 035). Every field in its results is typed or checked against an allowlist, except the task title. Harness, role, and event type must be ASCII tokens. Media types must be MIME-shaped, digests hexadecimal, and timestamps RFC 3339. Dispatch status fields are typed enums. `projection::task_summary` bounded the title to 240 bytes but replaced only characters for which `char::is_control` is true. So a right-to-left override, a bidirectional isolate or mark, or a line or paragraph separator reached the MCP client verbatim, where it could reorder or break the title as shown.

**Implemented:**

- The title projection replaces each character for which the shared `vibemux_harness::terminal_text::is_terminal_unsafe` is true with a space. That set includes every control character, so nothing the old rule changed is kept now. Other characters are kept, including a no-break space, so `title_truncated` is not set by ordinary text.
- `vibemux_mcp` depends directly on `vibemux_harness`, which it already reached through `vibemuxd`. `Cargo.lock` adds that one edge.
- `docs/native_mcp_protocol.md` states the rule.

**Evidence (native Windows, Rust 1.92.0):** `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_mcp --all-targets --all-features -- -D warnings` clean. `cargo test -p vibemux_mcp --all-features`: lib 5, framing 5, ipc 3, lifecycle 22, and stdio 5 passed. The I: drive was full (4.9 MB free), so the `ipc` test failed to link under the repository's `target`. The full run used a scratch target directory on C:. Restoring `char::is_control` failed the new test `a_title_loses_bidirectional_controls_and_separators_but_keeps_its_text`, and the source was restored byte-for-byte.

**Remaining:** the I: drive needs space before the repository `target` can build again; its existing build caches were not removed. Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800.

### 2026-10-18 - One CLI module reads every `--prompt-file`

**Scope:** the follow-up named by the `dsh prompt` entry above. `dispatch_commands` and `dsh_commands` each held a `PromptSource` type, a stdin marker, and a bounded reader. The readers had become near copies; only the bound and the error code for an invalid prompt differed.

**Implemented:**

- New private module `vibemux_cli::prompt_input` owns `PromptSource`, the `--prompt-file` argument rule (`-` reads stdin), and `read_prompt(source, max_bytes)`. The reader runs on a blocking thread and reads at most one byte beyond the bound. A two-variant `PromptReadError` reports either `Unavailable` or `Invalid` (oversized, non-UTF-8, or blank).
- `PromptReadError::into_cli_error` maps `Unavailable` to `cli_prompt_unavailable` and `Invalid` to the caller's code. `dispatch submit` and `decide route` keep `harness_dispatch_invalid_prompt`, and `dsh prompt` keeps `cli_invalid_arguments`. Each command passes its own 32 KiB constant.
- `dispatch_commands::PromptSource` and `dsh_commands::PromptSource` stay valid paths, now re-exports of the one type.
- No behavior, CLI output, daemon, or wire change.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):** `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_cli --all-targets --all-features -- -D warnings` clean. `cargo test -p vibemux_cli`: lib 48 passed, integration 5 passed. The reader tests moved to `prompt_input` with an 8-byte bound. The dispatch and DSH tests now check each command's bound and codes end to end. Two mutations were caught and restored byte-for-byte. Refusing a prompt of exactly the bound failed three tests, as did mapping every invalid prompt to `cli_invalid_arguments`.

**Remaining:** owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Bound the probe config read itself, not only the size check before it

**Scope:** `vibemux_probe` reads agent config files (Claude and OpenCode JSON, Codex and Grok TOML) to discover endpoints. `read_bounded_config` checked the path's size and then called `read_to_string`, which reads to the end. A file swapped or grown after the check could make the probe read past the 256 KiB `MAX_CONFIG_FILE_BYTES`.

**Implemented:**
- `read_bounded_config` still checks the path before opening, so a FIFO found there is never opened. It then checks the opened handle's metadata (a regular file within the bound), as `cache::read_cache` already does.
- New private `read_bounded_text(reader, max_bytes)` reads at most one byte past the bound through `take`. It refuses a longer or non-UTF-8 source rather than truncating it.
- No change to discovered endpoints, the probe report, the cache, or any wire format.
- Audit only, no change: `snapshot::load` (256 KiB, sanitized, stable codes) and gateway telemetry (the health check reads only the status code; the SQLite query is aggregate-only with `LIMIT 32` and a 64-byte `app_type`).

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):** `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_probe --all-targets --all-features -- -D warnings` clean. `cargo test -p vibemux_probe --all-features`: lib 24 passed. The new test `a_config_read_stops_one_byte_past_the_bound` covers an exact-bound read, an endless `std::io::repeat` source, non-UTF-8 bytes, a directory, and a missing file. One mutation was caught and restored byte-for-byte: removing the post-read length check failed the new test.

**Remaining:** owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - One bounded regular-file read for workflow contracts, verifier results, and state files

**Scope:** three ADR 031 workflow reads in `vibemuxd` checked a file's size on the path, then read it:
- `prepare::read_shared_contracts` read shared contract text from the project checkout with `read_to_string`.
- `verifier_runner::read_result` read the result file a verifier child process writes with `fs::read`.
- `state_files::read_bounded` read through `take(maximum + 1)` but never refused the extra byte.

A file swapped or grown after the size check could be read past its bound: 64 KiB for contracts, 1 MiB for results, or the state file's own bound.

**Implemented:**
- New private `workflow::bounded_file::read_regular_file(path, maximum)`. It checks the path without following a symlink, so a link, FIFO, or device found there is never opened (a link swapped in after that check is still followed by the open). It checks the opened handle again, then reads at most one byte past the bound and refuses rather than truncates.
- `BoundedReadError` has three variants: `Missing`, `Refused`, and `Io`.
- The three callers use it and keep their own error codes:
  - A contract that is unreadable, oversized, or not UTF-8 is still `RequestInvalid`.
  - An unreadable verifier result is still no result.
  - A state file that does not exist yet is still `None`, and every other failure is `Internal`.
- `prepare` keeps its symlink check before `canonicalize` and its containment check after it.
- No change to request, contract, receipt, or state-file formats, the control protocol, or the store.
- Not changed: the daemon lifecycle lock (`LifecycleLock::is_held`) and the Windows control ACL marker read whole files in the daemon's private runtime paths. They are left for a later pass.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt --all -- --check` clean. `cargo clippy -p vibemuxd --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemuxd --all-features --lib workflow::`: 25 passed. New tests:
  - `a_read_stops_one_byte_past_the_bound`: an endless `std::io::repeat` source.
  - `only_a_regular_file_within_the_bound_is_read`: exact bound, one byte over, a directory, and a missing file.
  - `a_shared_contract_is_bounded_utf8_text_inside_the_root`: a 64 KiB contract is read, while one byte over, non-UTF-8 bytes, a missing file, a directory, and `../outside.md` are refused.
  - A Unix-only test checks that a symlink is refused without being followed. It was not run here.
- `cargo test -p vibemuxd --all-features --test workflow_gates --test workflow_control --test workflow_admission`: 4, 5, and 6 passed. These ran on the final production code, before the `prepare` test was added.
- Two mutations were caught and restored byte-for-byte. Removing the post-read length check failed the bounded-read test. Passing the contract bound minus one failed the shared-contract test.

**Remaining:** owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Read worker worktree files through the shared bounded reader

**Scope:** the ADR 031 candidate collector (`vibemuxd::workflow::collector::observe`) reads the files a worker changed in its worktree. It checked each path with `symlink_metadata` and classified links and special files. It then used its own `read_bounded`: `File::open` plus `take(maximum + 1)` and a post-read check, but with no check of the opened handle.

**Implemented:**
- `observe` now reads through `workflow::bounded_file::read_regular_file`, and every failure stays `WorkflowError::Snapshot`. The private `collector::read_bounded` is gone, so the workflow has one owner for bounded file reads.
- The symlink and special-file classification, the manifest's per-file and total bounds (bytes over them are never read), and `BlobStore` are unchanged. `BlobStore::get` still reads a whole blob from the daemon's private state.
- The `bounded_file` and probe `read_bounded_config` doc comments, and the two earlier 2026-10-18 entries above, now say exactly what the path check prevents. A link, FIFO, or device found by the check is never opened, but a link swapped in between the check and the open is still followed. The size bound holds either way.
- No change to snapshot manifests, receipts, the control protocol, or the store.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt --all -- --check` clean. `cargo clippy -p vibemuxd -p vibemux_probe --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemuxd --all-features --lib workflow::`: 26 passed. The new test `a_worktree_file_is_read_within_its_bound_and_others_are_classified` covers a file at the bound being read, a directory classified `Special`, a missing path reported `Deleted`, and an oversized file kept out of the read with its size recorded.
- `cargo test -p vibemuxd --all-features --test workflow_cooperate --test workflow_compare`: 2 and 2 passed.
- One mutation was caught and restored byte-for-byte: passing the per-file bound minus one failed the new test.

**Remaining:**
- Owner decision: whether bounded reads should open without following links. On POSIX that means `O_NOFOLLOW`. On Windows it means `FILE_FLAG_OPEN_REPARSE_POINT`, which also changes how cloud-file and deduplication placeholders read. Until then, the bound, not the path check, is the guarantee against a swapped link.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Check the daemon lifecycle lock with a bounded read

**Scope:** `vibemuxd`'s `LifecycleLock::is_held` decides whether the writer lock and the project compatibility lock still carry this holder's nonce. `Drop` removes the file only then, and the Windows leaf release checks the compatibility lock the same way. It used `read_to_string` on the lock path, so a lock path replaced by a large file was read whole.

**Implemented:**
- `bounded_file` moved from `vibemuxd::workflow` to the crate root (`git mv`), since the lifecycle lock is not workflow code. The four workflow callers now import `crate::bounded_file`, with unchanged behavior.
- `is_held` reads the lock through `bounded_file::read_regular_file` with a new private `MAX_LOCK_FILE_BYTES = 256`. A nonce is `{pid}-{epoch nanoseconds}-{counter}`, under 80 bytes. A link found at the path, an oversized file, non-UTF-8 bytes, or another nonce now reads as not held, so `Drop` leaves that file in place.
- `docs/architecture.md` (writer section) says the nonce check is bounded and what it leaves in place.
- No change to the lock file contents, the lock path, recovery, the control protocol, or the store.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt --all -- --check` clean. `cargo clippy -p vibemuxd --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemuxd --all-features --lib`: 130 passed. The new test `a_lifecycle_lock_is_held_only_while_its_bounded_file_carries_the_nonce` covers the fresh lock, the nonce padded past 256 bytes, another holder's nonce, and the nonce restored. The padded case read as held under the old `read_to_string` and `trim`.
- `cargo test -p vibemuxd --all-features --test daemon_process`: 9 passed.
- `cargo test -p vibemux_cli --all-features`: lib 48, `bootstrap_process` 4, `unified_commands` 6 passed. `bootstrap_process` asserts the writer lock is gone after a daemon run, so the holder still recognizes its own lock.
- One mutation was caught and restored byte-for-byte: reading the lock with no bound (`u64::MAX`) failed the new test.

**Remaining:**
- The Windows control ACL marker check (`process.rs` `acl_marker_valid`) still reads its marker whole.
- Owner decision on no-follow opens for bounded reads (see the previous entry).
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Read the Windows control ACL marker through the bounded reader

**Scope:** on Windows, `vibemuxd::process::acl_marker_valid` decides whether the shared control runtime root has a published `.acl_v1` marker. Daemon start uses it to skip or redo ACL setup, and trusted-start validation uses it to refuse. It checked the path's file type and then read the whole marker with `fs::read` to compare it with the 22-byte `vibemux_control_acl_v1`.

**Implemented:**
- `acl_marker_valid` reads through `crate::bounded_file::read_regular_file`, bounded to the marker's own length, and compares the bytes. That reader's path check, which does not follow links, replaces the separate file-type check here.
- A link or directory found at the path, a missing file, or other bytes still read as invalid. A longer file is now refused one byte past the contents instead of being read whole.
- The `bounded_file` module doc lists the marker among its users. A grep of `vibemuxd` code, leaving out tests and the two fixture binaries, now finds every other `read_to_end` behind a `take` bound. The only whole-file read left is `BlobStore::get`.
- No change to the marker contents, its publication, the ACL helpers, or the control protocol.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt --all -- --check` clean. `cargo clippy -p vibemuxd --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemuxd --all-features --lib`: 131 passed. The new Windows test `an_acl_marker_is_valid_only_with_its_exact_contents` covers missing, exact, one byte longer, a same-length different marker, and a directory. The existing marker healing and concurrent first-touch tests also pass.
- `cargo test -p vibemuxd --all-features --test daemon_process`: 9 passed, including `trusted_start_verifies_the_full_control_runtime_surface`.
- One mutation was caught and restored byte-for-byte: a bound one byte shorter than the marker failed the new test, `ensure_runtime_dir_heals_a_stale_acl_marker`, and `concurrent_first_touch_serializes_marker_publication`.

**Remaining:**
- `BlobStore::get` still reads a whole blob from the daemon's private workflow state, because it has no bound of its own.
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Bound candidate blob reads to the manifest's byte count

**Scope:** `vibemuxd::workflow::collector::BlobStore::get` returns a candidate's collected bytes from the daemon's private workflow state. It serves materialization (`apply_manifests`), the negative-control mutation, reloading a candidate's texts for a resumed coordinator, and `put`'s check of an existing blob. It refused a non-regular file and re-hashed the bytes, but read the whole blob file first. The supervisor-context path already read the same blobs bounded by the manifest's byte count.

**Implemented:**
- `get` takes the `byte_count` the manifest recorded for the write. It reads through `crate::bounded_file::read_regular_file` bounded to that count, then requires exactly that many bytes and the digest. A blob file that has grown or been replaced is refused one byte past the count instead of being read whole.
- Every caller passes the count from its `SnapshotChange::Write` entry, and `put` passes the length of the bytes it stores. The collector records that count as the length of the bytes it read, so collected manifests and blobs agree.
- No change to the manifest format, the blob layout, the store schema, or the control protocol.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt --all -- --check` clean. `cargo clippy -p vibemuxd --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemuxd --all-features --lib`: 131 passed. `blobs_are_content_addressed_and_rehashed` now also covers a count one byte short or long, a tampered blob, and a blob grown past its count that `put` then heals.
- `cargo test -p vibemuxd --all-features --test workflow_compare --test workflow_cooperate --test workflow_gates`: 2, 2, and 4 passed. They drive the real daemon through collection, review, verification, and isolated integration.
- One mutation was caught and restored byte-for-byte: dropping the length comparison failed the off-by-one assertion. The bound by itself has no observable effect at this level once the length check stands; the shared reader's own tests cover the stop one byte past the bound.

**Remaining:**
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Bound the GUI user settings read

**Scope:** `vibemux_frontend::theme::serialize::load_user_config_at` loads `frontend.json`, the GUI's persisted theme, window size, and sidebar setting, when the `vibemux_frontend` GUI binary starts. It read the whole file with `fs::read_to_string` before parsing. A missing, unreadable, or corrupt file already fell back to the defaults.

**Implemented:**
- The file is read only if it is a regular file, and the read stops one byte past `MAX_USER_CONFIG_BYTES` (64 KiB). A larger file, a directory, or non-UTF-8 bytes fall back to the defaults like any other unreadable file. A saved settings document is a few hundred bytes.
- The file-type check follows a link, so a settings file a dotfiles manager placed as a link still loads.
- No change to the `frontend.json` layout, its schema version, the save path, or the window-size clamping.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemux_frontend --all-features --lib`: 150 passed, 1 ignored (the golden-fixture writer). New tests: `a_user_config_past_the_bound_returns_defaults` loads a valid document padded to exactly 64 KiB, and falls back for one byte more or a directory. `a_settings_read_stops_one_byte_past_the_bound` ends an endless source at the bound.
- One mutation was caught and restored byte-for-byte: accepting one byte past the bound failed both new tests.
- No GUI window was opened.

**Remaining:**
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Stage GUI settings saves under a unique name

**Scope:** `vibemux_frontend::theme::serialize::save_user_config_at` persists `frontend.json` after the GUI's theme, window size, or sidebar setting changes (debounced, and never in demo mode). It wrote the whole document to the fixed sibling `frontend.json.tmp` and renamed it over the settings file. Two GUI processes saving at once wrote through the same staged file, and a failed rename left it behind.

**Implemented:**
- Each save stages through a hidden sibling named for the process id and the current time, `.frontend.json.<pid>.<nanos>.tmp`. The staged file is opened only if it does not exist yet, so a name already taken fails that save instead of overwriting another save's file.
- A failed write removes the file it created, and a failed rename, after the existing single retry, removes the staged file. A name already taken by another save is left alone.
- The module doc still states that concurrent writers can lose a write. There is no lock, and the `frontend.json` layout, its schema version, and the read path are unchanged.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_frontend --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemux_frontend --all-features --lib`: 152 passed, 1 ignored (the golden-fixture writer). New tests: `a_save_leaves_only_the_settings_file` checks that a save leaves only `frontend.json`, and that a save blocked by a non-empty directory at the path fails, leaves no staged file, and leaves the directory's contents alone. `a_staged_file_is_a_hidden_sibling_named_for_this_process` pins the name.
- After a doc-comment rewording, fmt, clippy, and `cargo test -p vibemux_frontend --all-features --lib theme::serialize` (16 passed) were rerun.
- One mutation was caught and restored byte-for-byte: dropping the cleanup after a failed rename failed `a_save_leaves_only_the_settings_file`.
- No GUI window was opened.

**Remaining:**
- A `frontend.json.tmp` left behind by an earlier build is not removed; nothing reads it.
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Pin the MCP host's request handling with unit tests

**Scope:** `vibemux_mcp::protocol` decides how the native MCP read host (ADR 035) answers each JSON-RPC message, with no process or daemon I/O. That covers the lifecycle order, the read-tool gate, the in-flight rules while one read runs, and envelope checks. The crate's integration tests reach it through the stdio host. No `vibemux_mcp` test named the `already_initialized`, `method_not_found`, `duplicate_request_id`, `reader_unavailable`, or `invalid_request` codes. The framing test checks only the -32600 number for a batch.

**Implemented:**
- A unit-test module in `crates/vibemux_mcp/src/protocol.rs`. No production code changed, and the MCP wire is unchanged.
- `the_lifecycle_answers_each_request_in_order`: before `initialize`, only `ping` and `initialize` answer. A second `initialize` is `already_initialized` (-32600). The tools stay closed until `notifications/initialized` arrives. An unknown method then gets `method_not_found` (-32601) with its own id.
- `an_early_initialized_notification_is_ignored`: the notification sent before `initialize` does not open the tools later.
- `a_read_needs_the_host_grant_and_an_available_backend`: a granted `health` call becomes a read. An unavailable backend answers a tool error naming `reader_unavailable`, and an unknown tool is `unknown_tool` (-32602). Without the host grant, read tools are `metadata_read_denied` (-32001) and `tools/list` offers only `host_status`.
- `an_in_flight_read_admits_only_ping_and_its_own_cancellation`: a request reusing the active id is `duplicate_request_id` with a null id, and `ping` still answers. Another request is `request_busy` (-32000). Only a well-formed `notifications/cancelled` naming the active id cancels.
- `envelopes_outside_json_rpc_2_are_invalid_requests`: these all get `invalid_request` with a null id: a non-object, another `jsonrpc` version, a missing method, a boolean, fractional, or 129-byte string id, or an extra key. A 128-byte string id is accepted.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_mcp --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemux_mcp --all-features`: lib 10 passed (5 new), framing 5, ipc 3, lifecycle 22, stdio 5.
- Two mutations were caught and restored byte-for-byte. Disabling the second-`initialize` guard failed the lifecycle test, and disabling the duplicate-id check failed the in-flight test.

**Remaining:**
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Pin the MCP tool schemas to the argument parser

**Scope:** The native MCP read host (ADR 035) advertises an input schema for each read tool in `tools/list`, and `vibemux_mcp::tools::parse_operation` decides what a `tools/call` actually accepts. The two are written separately, and only the stdio integration tests sampled a few refused arguments. No test checked that each schema describes what the parser accepts.

**Implemented:**
- A unit-test module in `crates/vibemux_mcp/src/tools.rs`. No production code changed, and the MCP wire is unchanged.
- `listed_read_tools_are_the_tools_the_parser_knows`: with the host grant, the listed tools after `host_status` are `READ_TOOLS` in order. Without it, only `host_status` is listed, and the parser does not handle `host_status` (`unknown_tool`).
- `each_schema_declares_exactly_the_arguments_the_parser_accepts`: for every read tool, all declared arguments together are accepted, and so are the required ones alone. Dropping any required argument is refused. So are an undeclared argument (the schema sets `additionalProperties: false`) and a non-object.
- `the_limit_schema_states_the_parser_bounds_and_default`: the `limit` schema's `minimum` and `maximum` are 1 and `MAX_FRONTEND_TASKS`, both accepted. Omitting `limit` yields the schema's `default`. 0, one past the maximum, -1, 1.5, the string `"1"`, and null are refused.
- `identifier_schemas_admit_what_the_parser_admits`: `after`, `task_id`, and `request_id` share one schema: a 36-character string matching a lowercase hyphenated hex pattern that is not the nil UUID. For eight candidates, the parser accepts exactly those that a hand-written reading of that schema admits. The canonical form is accepted. Uppercase, nil, unhyphenated, braced, `urn:uuid:`, space-led, and non-hex forms are refused, and so is a number.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_mcp --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemux_mcp --all-features`: lib 14 passed (4 new), framing 5, ipc 3, lifecycle 22, stdio 5.
- Three mutations were caught and restored byte-for-byte. Each failed one new test: narrowing the parser's limit range to `1..MAX_FRONTEND_TASKS`, dropping `task_id` from the `frontend_task` schema's `required`, and dropping the canonical-text check from `canonical_uuid`.

**Remaining:**
- JSON Schema counts `1.0` as an integer, but the parser refuses a `limit` sent as a float. The parser is the stricter of the two, so this was left unchanged.
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Pin the MCP reader process's own request checks

**Scope:** The native MCP read host (ADR 035) does its daemon reads in a contained child process (`vibemux_mcp --ipc_reader`, which runs `vibemux_mcp::control_reader::run`). The reader owner sends that child one JSON request per line over a private pipe. The child decodes each request with serde, so `parse_operation` never runs there, and it repeats its own checks before reading the control descriptor. It also requires a stable non-zero generation and sequences that rise by one. The integration suites send the real reader only well-formed requests, and no `vibemux_mcp` test named `reader_request_invalid`.

**Implemented:**
- A unit-test module in `crates/vibemux_mcp/src/control_reader.rs`. No production code changed, and the reader's private wire and the MCP wire are unchanged.
- `invalid_operations_are_refused_before_the_descriptor_is_read`: limit 0, a limit one past `MAX_FRONTEND_TASKS`, and a nil `request_id` are `reader_request_invalid`. Each valid operation, including limits 1 and `MAX_FRONTEND_TASKS`, reaches a descriptor path where nothing exists and answers `producer_unavailable`. So the refusals happen before any descriptor I/O.
- `a_request_with_a_nil_task_id_or_an_extra_key_does_not_decode`: a nil `task_id` or `after` fails to decode, as does an unknown key inside the operation or at the top level.
- `each_request_is_answered_in_order_until_the_input_ends`: three requests in one generation get three replies carrying their own generation and sequence, and the end of input ends the run cleanly. Empty input writes nothing.
- `a_request_out_of_generation_or_sequence_stops_the_reader`: generation 0, a first sequence other than 1, a replayed or skipped sequence, a changed generation, and an undecodable line each stop the run with `reader_request_invalid`. No reply is written for the offending line.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_mcp --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemux_mcp --all-features`: lib 18 passed (4 new), framing 5, ipc 3, lifecycle 22, stdio 5.
- Four mutations were caught and restored byte-for-byte: dropping the zero-limit guard, dropping the nil `request_id` guard, accepting generation 0, and ignoring a changed generation.

**Remaining:**
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Pin the CLI failure type's public-code contract

**Scope:** Every `vibemuxctl` error reaches the operator as a `vibemux_cli::failure::Failure`. Its module promises that public error codes are independent of private paths and process diagnostics. `Failure::new` keeps a code only if it is 1 to 96 bytes of lowercase ASCII letters, digits, and underscores. Otherwise the code becomes `cli_operation_failed`. `Display` and `Debug` show only the code. `output::error_text` adds the remediation message from `with_message` when there is one. The wrapped source is reachable only through `Error::source`. No `vibemux_cli` test exercised the code filter or checked what `Display` and `Debug` show.

**Implemented:**
- A unit-test module in `crates/vibemux_cli/src/failure.rs`. No production code changed.
- `only_a_short_snake_case_code_is_kept`: real codes and a 96-byte code are kept. These are all replaced by `cli_operation_failed`, through both `new` and `with_source`: an empty code, a 97-byte code, one with an uppercase letter, a hyphen, a space, or a newline, a Windows path, and non-ASCII letters.
- `display_and_debug_show_the_code_and_never_the_source_or_message`: for a failure with a source, with a message, and with a path given as its code, `Display` is exactly the code and `Debug` is exactly `Failure { code: ..., .. }`. Neither shows the source's private path or the message.
- `the_message_and_source_stay_available_to_their_own_readers`: `message()` returns only what `with_message` set, and `Error::source` still returns the wrapped error.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_cli --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemux_cli --all-features --lib`: 51 passed (3 new). The CLI integration suites were not rerun, because no production code changed.
- Four mutations were caught and restored byte-for-byte: a 97-byte code limit, an accepted hyphen, a `Display` that appends the source, and a `Debug` that lists the message.
- `vibemux_workflow::receipts` was checked first and deferred. Its accessors feed the `vibemux_store` row checks on kind, task key, subject, and digest. The store integration tests build candidate, review, verification, integration, and selection receipts directly. How they reach bundle, decision, and model-call receipts was not traced.

**Remaining:**
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Pin decision and model-call workflow receipts

**Scope:** `vibemux_workflow::receipts::WorkflowReceipt` is the evidence the single writer persists for a workflow (ADR 031). `vibemux_store` files each receipt under its `kind()`, `receipt_id()`, `task_key()`, and `subject_digest()`, and checks those columns and `digest()` again when it recovers. The store tests build candidate, review, verification, integration, and selection receipts, and the daemon builds bundle receipts. Nothing in production or in any test builds the decision or model-call variants as a `WorkflowReceipt`. So those two arms of every accessor were never run.

**Implemented:**
- A unit-test module in `crates/vibemux_workflow/src/receipts.rs`. No production code changed, and the store schema and receipt JSON are unchanged.
- `decision_and_model_call_receipts_name_their_own_id_and_subject`: a decision receipt is kind `decision`, takes its id from `decision_id`, and is bound to `rationale_sha256`. A model-call receipt is kind `model_call`, takes its id from `call_id`, and is bound to `request_sha256`, not to the response or the route table. Neither claims a workflow or a task.
- `the_stored_kind_is_the_serialized_tag`: for both kinds, `kind()` equals the `kind` tag in the receipt JSON, and the decoded receipt equals the original and has the same digest.
- `the_receipt_digest_covers_the_kind_and_every_field`: the digest is taken over the tagged receipt, not the bare record. It changes when a decision is rejected instead of accepted, and when a model call's attempt number changes.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_workflow --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemux_workflow --all-features`: 149 passed (3 new).
- Three mutations were caught and restored byte-for-byte: the model-call kind spelled `model-call`, the model-call subject switched to `route_generation`, and the decision id shifted by one.

**Remaining:**
- `kind()` and the serde tag are still compared only for these two kinds. The other six are built only in store and daemon tests.
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Pin the A2A record rules in vibemux_types

**Scope:** `vibemux_types::a2a` holds the rules that `vibemux_store` applies to A2A run records. These include the remote-state transitions, the run workspace checks, the workspace key that the store compares workspaces by, and the artifact reference bounds. The module had no unit tests. Its rules were reached only through `vibemux_store` and daemon scenarios.

**Implemented:**
- A unit-test module in `crates/vibemux_types/src/a2a.rs`. No production code changed, no rule was tightened (including `valid_label`), and the record JSON is unchanged.
- `remote_states_never_return_to_submitted_or_leave_a_terminal_state`: checks all 36 pairs. Every state may repeat. `submitted` may move to any state. `working` and `input_required` may move to each other or to any terminal state, but never back to `submitted`. `completed`, `failed`, and `cancelled` move nowhere else.
- `a_run_workspace_needs_an_absolute_path_and_a_safe_branch`: POSIX, UNC, and drive paths with either separator are accepted, and so are 40- and 64-character base commits. These are refused: a relative path, a drive-relative `C:repo`, an empty path, a path with a control character, or one longer than 4096 bytes. So is a branch that starts with `-`, contains `..`, ends in `.lock`, contains a space, or is empty. So is a base commit of the wrong length or with a non-hex character, and an empty or spaced ownership token.
- `workspace_path_keys_ignore_case_separators_and_trailing_slashes`: `C:\Work\Repo\` and `c:/work/repo` share the key `c:/work/repo`, and a different directory keeps a different key.
- `artifacts_are_bounded_unique_and_lowercase_hex`: `MAX_A2A_ARTIFACTS` artifacts pass, and one more fails. A repeated artifact id fails. A size of exactly `MAX_A2A_ARTIFACT_BYTES` passes, and one byte more fails. So do an uppercase or short digest, and a media type that is blank or contains a control character.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_types --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemux_types --all-features`: 10 passed (4 new).
- Five mutations were caught and restored byte-for-byte: letting `working` return to `submitted`, accepting a drive-relative path, dropping the `.lock` branch rule, keeping case in the path key, and admitting a repeated artifact id.

**Remaining:**
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Pin the remaining A2A validators in vibemux_types

**Scope:** `vibemux_types::a2a` validates A2A run starts, updates, verification receipts, and stored run records. The store's A2A tests drive these checks only through its own scenarios. They never submit a run start with a transport other than `http_json`, a non-UTC or pre-1970 timestamp, a role outside worker/reviewer/verifier, a task with a description, or a stored record that fails validation as it decodes. This change pins those rules where they live. It is test-only.

**Implemented:**
- New tests in the existing unit-test module of `crates/vibemux_types/src/a2a.rs`. No production code changed, no rule was tightened (including `valid_label`), and the record JSON is unchanged.
- `a_run_start_is_a_preparing_worker_reviewer_or_verifier_run_of_its_task`: a preparing `worker`, `reviewer`, or `verifier` run of its own task is accepted. These are refused: any other role, a task with a description, a run that has already started, a run of another task, a workspace base commit that differs from the run's, and an invalid workspace.
- `binding_fields_name_a_known_transport_and_bounded_identifiers`: the transports `http_json`, `json_rpc`, and `grpc` are accepted, and `http`, `HTTP_JSON`, and `grpc ` are refused. The peer id (128 bytes), external task id (256), protocol version (32), and idempotency key (256) are accepted at their bound and refused one byte over it. An empty or spaced peer id and an idempotency key with `?` are refused.
- `timestamps_must_be_utc_and_not_before_1970`: in both a run start and a run update, the Unix epoch and 9999-12-31T23:59:59Z are accepted. One second before the epoch is refused, and so is the same instant written with a `+08:00` offset. The crate's `time` build cannot represent a year after 9999, so only the lower bound can be crossed.
- `a_verification_receipt_needs_a_reviewer_version_and_three_digests`: a receipt is valid whether it accepts or rejects. Reviewer version 0 is refused, and so is an uppercase, short, or non-hex value in any of the three digests.
- `a_run_update_checks_its_version_key_and_action`: every action kind is accepted with valid content. These are refused: expected version 0, an empty idempotency key, an observation with a repeated artifact id, a failure code that is empty or has a space, and a verification whose receipt is invalid.
- `a_run_record_binds_one_task_run_and_workspace`: a wrong schema version, version 0, a task, project, task id, or run id that does not match the run, and a workspace base commit that differs from the run's are each an identity mismatch. An unknown transport, an empty idempotency key, a non-UTC creation time, an update time before the creation time, an invalid workspace branch, an invalid artifact, and an invalid receipt are each invalid input.
- `a_succeeded_run_record_carries_a_verification_receipt`: a succeeded run without a receipt is an invalid state, and the same run with a receipt is accepted.
- `a_record_that_fails_validation_does_not_decode`: a valid record survives a JSON round trip unchanged. The same JSON with version 0, transport `http`, or a `+08:00` creation time does not decode.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_types --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemux_types --all-features`: 18 passed (8 new).
- Thirteen mutations were caught and restored byte-for-byte: admitting transport `http`, dropping the UTC check, moving the year floor to 1969, admitting role `planner`, dropping the empty-description check, dropping the update-after-creation order check, letting a succeeded run skip its receipt, decoding a record without validating it, accepting reviewer version 0, accepting a spaced failure code, skipping a stored receipt's validation, accepting expected version 0, and raising the peer id bound to 129.

**Remaining:**
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Pin task and run restoration in vibemux_types

**Scope:** `crates/vibemux_types/src/restore.rs` decodes stored `Task` and `Run` JSON, for example the A2A record and the store's run check. It runs the same field checks as construction, and it keeps the stored status. Its one test covered a nil task id, a malformed base commit, and an unknown run key. Nothing pinned the title, description, or name bounds on this path, the stored status being kept, missing fields, or that a restoration failure reports the same domain error as construction. This change is test-only.

**Implemented:**
- `task()` and `run()` fixtures, plus `restore_with` and `restore_without` helpers that set or remove one key of the stored JSON and restore it. They are added to the existing unit-test module. No production code changed, and the JSON shape is unchanged.
- `a_restored_task_keeps_its_stored_status_and_bounded_text`: an in-progress task restores equal with its status, and a stored `blocked` status is kept without any transition being replayed. A title of exactly 512 UTF-8 bytes made of two-byte characters is accepted. So are a 64 KiB description and a blank description. These are refused: a 513-byte title, a blank or empty title, a description one byte over 64 KiB, an unknown status, a nil task id, a malformed project id, an unknown key, and a missing title, description, or status.
- `a_restored_run_keeps_its_stored_status_and_bounded_names`: a running run restores equal with its status, and a stored `stale` status is kept. `harness`, `role`, and `protocol` are each accepted at 128 bytes. Each is refused at 129 bytes, when blank, when empty, and when missing. A base commit of 7 and of 128 hex characters is accepted. These are refused: 6 or 129 characters, a non-hex commit, an unknown status, a nil run id, and an unknown key.
- `restoration_reports_the_same_domain_error_as_construction`: an over-long title, an over-long description, and a short base commit each fail restoration with the message `Task::new` or `Run::new` gives for the same value.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_types --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemux_types --all-features`: 21 passed (3 new).
- Eleven mutations were caught and restored byte-for-byte. Ten were in `restore.rs`: a blank title accepted, the description bound raised by one, a blank role accepted, the protocol bound raised by one, an unbounded harness, the base commit left unchecked, the task status or the run status reset on restore, unknown task keys admitted, and the title error replaced by a generic message. The eleventh was in the shared `validate_required` in `lib.rs`: whitespace-only values accepted.

**Remaining:**
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Pin the supervisor context-action rules in vibemux_workflow

**Scope:** `crates/vibemux_workflow/src/supervisor/context_actions.rs` holds the pure least-privilege checks that the daemon (`vibemuxd/src/workflow/supervisor_context.rs`) and the store (`supervisor_commands/context_actions.rs`) apply before a supervisor context action reads a file, shares a bundle, or addresses a worker. Until now these checks were exercised only through those integration tests, and the module had no tests of its own. This change is test-only and reuses the crate's `task_spec::fixtures` Track A spec.

**Implemented:**
- A unit-test module in `context_actions.rs` with an execution fixture, a bundle fixture, and `*_with` helpers. No production code changed.
- `effect_ids_are_stable_and_distinct_per_action_and_kind`: the same action and kind give the same id, a different kind or action gives a different one, and the id is a version 4 UUID.
- `a_task_addressed_tool_reaches_only_a_single_slot_worker`: a task with exactly one allowed slot resolves to a worker of that task. The session id is stable, and it changes with the slot or the workflow. A comparison task with two slots, a task with no slots, and an unknown task resolve to nobody.
- `only_a_session_of_a_current_slot_is_a_current_worker`: the sessions of both slots of a comparison task are current, and so are the supervisor, operator, and broker. These are not current: a session of another slot, a session of another workflow, an arbitrary session id, and a real session id presented under another task key.
- `a_path_is_readable_only_in_scope_and_outside_forbidden_scopes_of_any_case`: owned and read-scope paths are readable. These are refused: paths outside every scope, an owned path in another letter case, a forbidden or protected path in its own case or in another case, an absolute path, a path with `..` or `.` (including `tests/worker_a/../../package.json` under a recursive owned scope), a backslash path, and an empty path. Without the `read_files` tool, nothing is readable.
- `a_bundle_is_shared_only_within_budget_policy_and_scope`: a bundle is accepted at the policy's byte limit, at the 32 KiB supervisor cap when the policy allows more, with a verifier-confirmed claim, with claims only, and with a worker claim once the policy retrieves worker claims. These are refused: an empty byte count, one byte over the policy limit, one byte over the cap, public excerpts disabled, candidate snapshots not in the retrieval order, no `read_files` tool, a source from another snapshot, line 0, an end line before the start line, a source outside scope or under a protected scope, a worker claim the policy does not retrieve, and a bundle with neither sources nor claims.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_workflow --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemux_workflow --all-features`: 154 passed (5 new).
- Eighteen mutations were caught and restored byte-for-byte: dropping the case-folded forbidden match, ignoring protected paths, skipping the `read_files` tool check, ignoring read scopes, skipping the relative-path check (caught only by the traversal cases), resolving a multi-slot task to its first slot, leaving the slot out of the session id, treating every worker as current, leaving the kind out of the effect id, dropping the 32 KiB cap, ignoring the public-excerpt flag, ignoring the snapshot retrieval order, admitting another snapshot digest, admitting line 0, ignoring the line order, mapping worker claims to the snapshot source, admitting an empty bundle, and admitting a zero byte count.

**Remaining:**
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Pin the A2A SDK task mapping in vibemux_a2a

**Scope:** `crates/vibemux_a2a/src/task_wire.rs` maps the local task contract to the official A2A SDK types and back. It covers task states, errors, parts, the request envelope, native messages, snapshots, replies, and stream updates. The module had no tests of its own. The integration tests in `crates/vibemux_a2a/tests` reach it only through the HTTP+JSON, JSON-RPC, and gRPC gateways and the crate's own server. That server sends only replacing artifact updates and status updates for the task being watched, so none of these tests sends an append chunk, an update for another task or context, a tenant, or an unspecified state. This change is test-only; no wire format or SDK use changed.

**Implemented:**
- A unit-test module in `task_wire.rs` with request, snapshot, native-message, status-update, and artifact-update fixtures. No production code changed.
- `every_local_state_maps_to_its_sdk_state_and_unspecified_is_refused`: each of the eight local states maps to the SDK state of the same name and back, and the SDK's `Unspecified` state is a protocol failure.
- `errors_keep_their_code_across_the_sdk_boundary`: invalid request, unauthorized, forbidden, not found, unsupported, busy, and deadline keep their JSON-RPC code (`-32602`, `-32040`, `-32041`, `-32001`, `-32004`, and `-32603` for the last two). The SDK message is the local error code, and each error maps back to itself. Protocol, transport, server, and internal failures leave as `-32603` and come back as a protocol failure. An error from another implementation maps by its code alone: task not found to not found, unsupported operation and task not cancelable to unsupported, and anything else to a protocol failure.
- `parts_round_trip_and_are_validated_when_read`: text, data, raw, and URL parts keep their media type and filename across the mapping. A part read from the wire is refused when its text is over 16 KiB, its URL is not http or https or carries user info, its filename has a newline, or its media type is over 128 bytes.
- `a_request_round_trips_in_its_envelope_and_mismatched_ids_are_refused`: a request travels as one data part in the `vibemux.task.request.v1` envelope with history length 0 and an immediate return, and it comes back unchanged. These are refused as invalid: a message id, context id, or task id that differs from the envelope, a tenant, an agent role, no parts, another contract version, an unknown envelope field, and an invalid request inside the envelope. A push notification configuration is refused as unsupported, and an invalid request is refused before it is sent.
- `a_native_message_becomes_bounded_part_data`: a message without the envelope becomes a request whose id, idempotency key, and default context are the message id, and whose payload is the parts and the request metadata. A given context and task id are kept. More than 16 parts, an invalid part, and an invalid message id are refused.
- `snapshots_and_replies_keep_their_error_code_and_artifacts`: a snapshot's error code travels in the task metadata, and the snapshot comes back unchanged. A snapshot with no artifacts and no error code sends neither. A task in the `Unspecified` state is a protocol failure, and a snapshot with an invalid error code is not sent. Task and message replies round trip, and a message reply is sent with the agent role. A reply message with the user role or without a context is a protocol failure.
- `stream_updates_apply_only_to_the_task_they_continue`: a status or artifact update before the first task snapshot is a protocol failure. A status update sets the state and the error code it carries, and clears a code it does not carry. An artifact chunk extends its artifact only when marked as an append, and otherwise replaces it; a chunk of a new artifact adds it. These are protocol failures that leave the stream unchanged: a status update for another task or context, an artifact update for another task, an unspecified state, a message event, and a snapshot of another task. A status update with an invalid error code is refused as invalid.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_a2a --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemux_a2a --all-features`: 13 library tests passed (7 new), and the 24 integration tests in `basic_information_share`, `task_gateway`, `task_grpc`, and `task_runtime` passed.
- Thirty-five mutations were caught and restored byte-for-byte. They were: accepting `Unspecified`; swapping two states in both directions; the wrong code for forbidden or unsupported; dropping the forbidden message, the task-not-found code, or the task-not-cancelable code from the reverse mapping; skipping the role, part-count, tenant, or push-configuration check; skipping the contract, context, or task id check of the envelope; skipping validation of the envelope request, the native request, or a read part; dropping the zero history length; not defaulting a native context; dropping a raw part's media type or a URL part's filename; moving the error code out of `error_code` metadata; sending empty artifacts; skipping the reply role check or sending a reply as the user; accepting a status update with no prior snapshot; refusing a status update only when both its task and context ids differ; skipping the identity check of an artifact update; keeping a stale error code; ignoring or always applying the append flag; dropping a new artifact; accepting a message event; skipping validation of the next snapshot; and skipping the final identity check.

**Remaining:**
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Pin the A2A subscription diff in vibemux_a2a

**Scope:** `crates/vibemux_a2a/src/task_server.rs` turns the backend's task snapshots into the A2A stream a subscriber sees. `snapshot_events` sends the first snapshot whole and later ones as artifact and status updates. `stream_finished` and the stream's `stop_on_interrupted` flag decide when the stream ends, and `validate_task_query` checks the task id and tenant of a get, cancel, or subscribe request. The module had no tests of its own; it was exercised only through the gateway integration tests. This change is test-only, and it is the server-side counterpart of the client mapping pinned in the previous entry.

**Implemented:**
- A unit-test module in `task_server.rs` with a subscription fixture (an empty backend stream, a stop channel, and one semaphore permit), plus snapshot, artifact, and expected-event helpers. No production code changed.
- `the_first_snapshot_is_sent_whole_and_later_ones_only_as_changes`: the first snapshot is sent as one task event, and an unchanged snapshot sends nothing. A changed or new artifact is sent whole as a replacing update (`append: false`, `last_chunk: true`) in snapshot order, with no status update while the state is unchanged. A change of state, of the error code alone, or a cleared error code each sends exactly one status update; the error code travels in `error_code` metadata.
- `applying_the_sent_events_reproduces_each_snapshot`: across six snapshots (submitted, working with a first artifact, a changed and an added artifact, input required with an error code, working again, and completed), the client's `apply_stream_response` rebuilds each snapshot exactly from the events sent. The stream ends only at the completed snapshot.
- `a_snapshot_of_another_task_an_invalid_one_or_a_shrinking_one_is_refused`: a snapshot of another task or context is a protocol failure, both before and after the first snapshot. So is a later snapshot that drops an artifact the subscriber already has. An invalid snapshot is refused as invalid, both first and after a snapshot was sent. A refused snapshot sends nothing and leaves the last sent snapshot in place.
- `a_terminal_state_ends_the_stream_and_an_interruption_ends_it_only_when_asked`: completed, failed, canceled, and rejected end the stream. Input required and auth required end it only when the stream stops on interruption. Submitted and working never end it.
- `a_task_query_needs_a_valid_id_and_no_tenant`: a valid id without a tenant passes. An invalid or empty id is refused with `-32602`, and a tenant is refused as unsupported (`-32004`).

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_a2a --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemux_a2a --all-features`: 18 library tests passed (5 new), and the 24 integration tests in `basic_information_share`, `task_gateway`, `task_grpc`, and `task_runtime` passed.
- Twenty mutations were caught and restored byte-for-byte. They were: dropping the up-front snapshot validation; refusing another task only when both ids differ, or not at all; allowing a dropped artifact; resending unchanged artifacts; setting the append flag; dropping the last-chunk flag; sending a status update only on a state change, or always; renaming the `error_code` metadata key; not sending the first snapshot; not keeping the last sent snapshot; ending the stream only on terminal states, on interruption regardless of the flag, or never on a terminal state; dropping auth required from the interrupted states, or adding working; skipping the query id check; allowing a tenant; and refusing a tenant as invalid instead of unsupported.
- In the first version of the refusal test, the invalid snapshot came only before the first one was sent. There `snapshot_to_wire` validates again, so dropping the up-front validation survived. The final test also sends an invalid snapshot after the first one, where that validation is the only check before a status update would carry the invalid error code.

**Remaining:**
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Pin the A2A client's origin, JSON-RPC, and event-frame checks in vibemux_a2a

**Scope:** `crates/vibemux_a2a/src/task_client.rs` has three pure helpers that the network client depends on. `loopback_origin` normalizes the allowed origins, the base URL, every interface an agent card advertises, and the server's gRPC base URL; it is the check that keeps the gateway loopback-only. `rpc_result` checks a JSON-RPC response before its result is used. `event_boundary` finds where a server-sent event ends in the subscription stream. The module had no tests of its own; the helpers were reached only through the gateway integration tests and their adversarial agent-card fixture. This change is test-only.

**Implemented:**
- A unit-test module in `task_client.rs`. No production code changed.
- `only_a_numeric_loopback_http_origin_with_an_explicit_port_is_allowed`: plain-http origins on IPv4 loopback (any `127.0.0.0/8` address) or IPv6 `::1`, with a port and at most a root path, are normalized to their origin without the trailing slash. These are refused as forbidden: a host name (including `localhost`), a private or unspecified address, another IPv6 address, https, a missing port, user info, a password, a query, a fragment, and a non-root path. The URL parser drops a scheme's default port, so `http://127.0.0.1:80` also counts as a missing port. Text that is not a URL is refused as invalid.
- `a_json_rpc_result_needs_its_id_and_exactly_one_outcome`: a `2.0` response whose numeric or string id matches yields its result. These are protocol failures: another version, another id, a string id where a number was sent, a response when no id is expected, a missing id, both a result and an error, neither, and a value that is not a response object. An error outcome maps by its local error code first (`a2a_task_not_found`, `a2a_task_busy`) and then by its JSON-RPC code (`-32004` is unsupported); any other error is a protocol failure.
- `an_event_ends_at_its_first_blank_line_in_any_newline_style`: an event ends at the first blank line written as LF LF, CR CR, or CRLF CRLF, and the helper returns both where the event data ends and where the next event starts. A blank line at the very start counts. A partial frame, including one cut inside a CRLF CRLF, has no boundary.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_a2a --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemux_a2a --all-features`: 21 library tests passed (3 new), and the 24 integration tests in `basic_information_share`, `task_gateway`, `task_grpc`, and `task_runtime` passed.
- Twenty-four mutations were caught and restored byte-for-byte. They were: mapping unparsable text to forbidden; accepting any non-multicast IPv4 address; refusing or accepting every IPv6 address; accepting host names; and skipping the scheme, port, user name, password, query, fragment, or path check. For the origin, keeping the trailing slash. For JSON-RPC: skipping the version, id, or single-outcome check; not mapping an error; and ignoring the error's JSON-RPC code. For event frames: shortening the CRLF CRLF frame, or dropping the CRLF CRLF, LF LF, or CR CR boundary; shifting where the data ends; and skipping the first byte.

**Remaining:**
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Pin the A2A gRPC endpoint, error, and status checks in vibemux_a2a

**Scope:** `crates/vibemux_a2a/src/task_grpc.rs` has three pure helpers on the gRPC binding. `validate_grpc_endpoint` checks the endpoint `GrpcTaskClient::connect` dials. `grpc_error` maps a gRPC status back to a local error on the client. `compatible_grpc_status` rewrites the statuses the server returns so they carry the A2A error reason. The module had no tests of its own; the helpers were reached only through the `task_grpc` integration tests. This change is test-only.

**Implemented:**
- A unit-test module in `task_grpc.rs`. No production code changed.
- `a_grpc_endpoint_is_a_numeric_loopback_http_address`: an endpoint without a scheme gets `http://`, and an accepted endpoint is returned otherwise unchanged. Plain http on IPv4 loopback or IPv6 `::1` is accepted. Unlike the HTTP client's origin check, a missing port means the default port 80. These are refused as invalid: an empty endpoint, a host name, a private address, another IPv6 address, https, user info, a password, a query, a fragment, a non-root path, port 0, and an endpoint over 2048 bytes. The length bound is tested with an endpoint whose port has leading zeros, so it is otherwise valid; it is accepted at exactly 2048 bytes and refused at 2049.
- `grpc_statuses_map_to_local_errors`: deadline exceeded, unauthenticated, permission denied, and resource exhausted map to deadline, unauthorized, forbidden, and busy. Unavailable and cancelled map to transport. Any other status maps by its local error code first (`a2a_task_unauthorized`, `a2a_task_busy`, `a2a_task_invalid_request`) and then by the SDK's mapping of its gRPC code. Through that mapping, not found becomes not found, a failed precondition becomes unsupported, and invalid argument, unimplemented, and internal become a protocol failure.
- `compatible_statuses_carry_the_a2a_error_reason`: these server statuses gain an A2A `ErrorInfo` with the `a2a-protocol.org` domain, and each keeps its message:
  - `a2a_task_not_found` keeps not found and gets the reason `TASK_NOT_FOUND`.
  - The push-notification, `a2a_task_unsupported`, and version failed preconditions become unimplemented, with `PUSH_NOTIFICATION_NOT_SUPPORTED`, `UNSUPPORTED_OPERATION`, and `VERSION_NOT_SUPPORTED`.
  - A task that cannot be canceled stays a failed precondition with `TASK_NOT_CANCELABLE`.
  - Unknown statuses carrying `a2a_task_unauthorized` and `a2a_task_forbidden` become unauthenticated and permission denied, without details.
  - Every other status passes through unchanged, including a local error code under a gRPC code it is not paired with.
  - Each rewritten local error still maps back to the same local error on the client.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_a2a --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemux_a2a --all-features`: 24 library tests passed (3 new), and the 24 integration tests in `basic_information_share`, `task_gateway`, `task_grpc`, and `task_runtime` passed.
- Thirty mutations were caught and restored byte-for-byte.
  - Endpoint check: raising the length bound, or refusing at exactly 2048 bytes; not adding the scheme; accepting any IPv4 address, any IPv6 address, or host names, or refusing IPv6 loopback; skipping the scheme, user name, password, query, fragment, or path check; allowing port 0; and requiring an explicit port.
  - Status mapping: mapping deadline exceeded to transport; dropping unauthenticated, permission denied, resource exhausted, or cancelled; and replacing the fallback with a protocol failure.
  - Status rewriting: dropping the not-found reason; keeping the push failure as a failed precondition; renaming the unsupported reason; matching the version message exactly instead of by prefix; changing the not-cancelable code; passing unauthorized through; giving forbidden the unauthenticated code; adding details to every status; and renaming the domain.

**Remaining:**
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Pin the A2A task contract bounds in vibemux_a2a

**Scope:** `crates/vibemux_a2a/src/task_contract.rs` defines the local A2A task types and their validators. These cover identifiers, requests, parts, artifacts, snapshots, messages, list requests, lists, and the peer credential. The other A2A modules rely on these validators, but the module had no tests of its own; its bounds were reached only through the integration tests. This change is test-only.

**Implemented:**
- A unit-test module in `task_contract.rs`. No production code changed.
- `task_identifiers_are_short_ascii_names`: one to 128 bytes of ASCII letters, digits, `_`, `-`, and `.` are accepted. An empty identifier, 129 bytes, a space, `/`, `:`, a non-ASCII letter, and NUL are refused.
- `a_request_bounds_its_identifiers_and_payload`: the request, context, optional task, and idempotency-key identifiers are each checked, and a request without a task id is accepted. A payload of exactly 16 KiB of serialized JSON is accepted, and one byte more is refused.
- `parts_bound_their_content_and_names`:
  - Text, data, and raw parts are accepted at 16 KiB and refused one byte over.
  - A media type is accepted at 128 bytes and refused at 129 bytes or with CR or LF. A filename is accepted at 256 bytes and refused at 257 bytes or with NUL, CR, or LF. Both checks apply to raw and URL parts.
  - A URL is accepted at 2048 bytes and refused at 2049. An unparsable URL, an ftp URL, a user name, and a password are refused.
  - A part list holds 1 to 16 parts, and one invalid part refuses the whole list.
- `artifacts_snapshots_and_messages_are_bounded`:
  - `TaskArtifact::data` builds an unnamed artifact with one data part. An artifact name is accepted at 256 bytes and refused at 257 bytes or with NUL. An invalid artifact id or an empty part list is refused.
  - A snapshot holds at most 16 artifacts and 32 KiB of serialized JSON: two 12 KiB artifacts fit and three do not. An invalid artifact, task id, context id, or error code refuses the snapshot.
  - A message has the same 32 KiB bound (two 12 KiB texts fit, three do not). An empty part list or an invalid id refuses it.
- `a_list_request_and_a_list_are_bounded`:
  - A list request's page size is 1 to 32. Its context id must be an identifier, and its page token is at most 128 bytes with no NUL.
  - A list holds at most 32 tasks and 32 KiB. Its total size is at most `i32::MAX`, its next-page token has the same bounds as the page token, and each task is validated.
- `a_peer_credential_is_validated_and_its_token_never_printed`: the subject must be an identifier. The bearer token is 32 to 256 bytes of ASCII letters, digits, `-`, `_`, and `.`, so a space or `+` refuses it. `Debug` prints the subject and `[redacted]`, never the token.
- `states_bindings_and_errors_keep_their_names`: completed, failed, canceled, and rejected are the terminal states, and all eight states serialize in snake_case. The bindings report the protocol names `HTTP+JSON` and `JSONRPC` and serialize as `http_json` and `json_rpc`. All eleven error codes keep their `a2a_task_*` strings.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_a2a --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemux_a2a --all-features`: 31 library tests passed (7 new), and the 24 integration tests in `basic_information_share`, `task_gateway`, `task_grpc`, and `task_runtime` passed.
- Fifty-three mutations were caught and restored byte-for-byte.
  - Identifiers and parts: raising the identifier bound, allowing `/`, or accepting an empty identifier; skipping the idempotency-key, task-id, or payload check; refusing text at exactly 16 KiB or raising the text, data, or raw bound; skipping the raw or URL media type check; raising the filename bound or allowing CR in a URL filename; raising the media type bound or allowing CR; raising the URL bound; ignoring an unparsable URL; allowing ftp, a user name, or a password; allowing an empty part list or 17 parts; and making the shared JSON bound inclusive.
  - Artifacts, snapshots, and messages: raising the artifact name bound or allowing NUL; skipping the artifact id or part check; allowing 17 artifacts; skipping the artifact or error-code check; and dropping the snapshot, message, or list size bound or the message part or id check.
  - Lists and credentials: allowing page size 0 or 33; skipping the list context check; raising the page-token bound or allowing NUL in the next-page token; allowing 33 tasks; dropping the total-size bound or the per-task check; skipping the subject check; allowing a 31- or 257-byte token or any byte but a space; and printing the token.
  - Names: making rejected non-terminal, renaming the JSON-RPC protocol or the server error code, and dropping the snake_case state names.

**Remaining:**
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Pin the A2A runtime's request identity and task listing in vibemux_a2a

**Scope:** `crates/vibemux_a2a/src/task_runtime.rs` owns the bounded ephemeral task runtime. Three of its pure helpers decide what callers see. `canonical_identity_payload` and `input_identity` build the identity that tells a repeated request apart from a conflicting one. `list_tasks` filters and pages a caller's tasks. The module's only unit test covered subscriber lag; these helpers were reached only through the `task_runtime` integration tests. This change is test-only.

**Implemented:**
- Five unit tests in the existing `task_runtime.rs` test module. No production code changed.
- `the_identity_payload_is_canonical`:
  - A float with no fractional part becomes an integer: `1.0`, `-0.0`, `1e3`, and `-2.0` become `1`, `0`, `1000`, and `-2`. `1.5` stays as it is.
  - Integers up to plus or minus 2^53 - 1 (`MAX_EXACT_INTEGER`) are kept, written either as integers or as floats. One past that bound in either direction is refused, as are `u64::MAX`, `1e300`, and an out-of-range number nested in an array or object.
  - Strings, booleans, and null pass through. Nested arrays and objects are canonicalized, and object keys come out sorted.
- `the_input_identity_ignores_request_ids_and_number_spelling`: the request id and idempotency key do not change the identity, nor does spelling `2` as `2.0` or reordering object keys. A different context id, a missing or different task id, or a different payload value does change it, and a payload with an out-of-range number is refused.
- `a_listing_keeps_only_the_callers_matching_tasks`: a listing holds only the calling subject's tasks, optionally narrowed by context id, state, or both. The total size counts only the matching tasks, and a subject with no tasks gets an empty page.
- `a_listing_pages_by_a_numeric_offset_token`: the page token is the offset of the next task. A page holds at most the page size, the last page has no next token, and a token equal to the number of matching tasks gives an empty page. A token past the end, a non-numeric token, a negative one, and an empty one are refused.
- `a_listing_strips_artifacts_unless_asked_and_stops_at_its_size_bound`:
  - Without `include_artifacts` the listed tasks have no artifacts, and the stored tasks keep theirs.
  - With artifacts, a page stops before the task that would take it over the 32 KiB list bound, and its next token points at that task. Three tasks of about 12 KiB give pages of two and one.
  - A task whose snapshot is exactly 32 KiB is valid on its own. A page stops before it rather than skipping it for a later task that would fit. A page that would start with it is refused as busy, and without artifacts it lists normally.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_a2a --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemux_a2a --all-features`: 36 library tests passed (5 new), and the 24 integration tests in `basic_information_share`, `task_gateway`, `task_grpc`, and `task_runtime` passed.
- Twenty-eight mutations were caught and restored byte-for-byte.
  - Identity payload: keeping integral floats as floats; refusing the largest exact integer; dropping the integer bound or only its negative side; making the float bound inclusive, dropping it, or applying it only to positive floats; and not recursing into arrays or objects.
  - Input identity: adding the request id; dropping the task id or the context id; and using the raw payload, with or without the canonical check.
  - Listing: dropping the subject, context, or state filter; treating a bad token as offset 0; refusing a token equal to the number of tasks; counting every task in the total size; never or always stripping artifacts; ignoring the page size; an off-by-one next token, or the token left by the task that did not fit; keeping the task that overflows the page, or skipping past it; and reporting an internal error instead of busy.
- This crate builds `serde_json` without `preserve_order`, so its maps already keep keys sorted. Removing the explicit key sort would therefore change nothing, and it was not counted as a mutation. The sorted-key assertions hold either way.

**Remaining:**
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Pin the A2A information-share contract in vibemux_a2a

**Scope:** `crates/vibemux_a2a/src/lib.rs` holds the loopback-only basic information-share boundary. It defines the share and acknowledgement types and their validators. It builds and parses the official A2A messages that carry them. It also checks a peer's agent card and URL before the client talks to it. Its five unit tests covered a round trip, oversized text, a correlation mismatch, two non-loopback URLs, and the SDK version. Everything else was reached only through the two `basic_information_share` integration tests. This change is test-only.

**Implemented:**
- Six unit tests in the existing `lib.rs` test module. No production code changed.
- `a_share_bounds_its_identifiers_text_and_metadata`:
  - Each identifier (correlation id, sender peer id, information kind) is 1 to 128 bytes of ASCII letters, digits, `_`, `-`, and `.`. A refusal names the field that failed.
  - Text is 1 byte to 4 KiB.
  - Metadata holds at most 16 entries. A key is 1 to 64 bytes and a value at most 256 bytes; a value may be empty. NUL in a key or a value is refused.
- `a_share_message_is_one_data_part_from_the_user`:
  - A share message comes from the user, carries the correlation id as its context id, and has exactly one data part: `{"contract": "vibemux.info.share.v1", "share": ...}`. An invalid share is refused before a message is built.
  - When parsing, missing metadata reads as empty. Refused as unsupported: an agent message, two parts, a text part, the acknowledgement contract name, and an unknown field in the envelope or the share.
  - An invalid share is refused before its context is compared. A different or missing context id is a correlation mismatch.
- `an_acknowledgement_message_is_one_data_part_from_the_agent`:
  - Accepted and declined acknowledgements both round trip. Each comes from the agent, with the correlation id as its context id and one data part under `vibemux.info.ack.v1`. Building one refuses an invalid correlation id or receiver id and names the field.
  - When parsing, these are refused as unsupported: a user message, two parts, a text part, the share contract name, and an unknown field. Invalid ids are refused by field, and a different or missing context id is a mismatch. A share message is never read as an acknowledgement.
- `only_a_loopback_card_offering_the_share_skill_is_accepted`:
  - The built card is named after the peer, offers only the share skill, and has one HTTP+JSON interface at the given URL.
  - A card may list other skills as long as the share skill is among them. A card is refused as unsupported without the share skill, with no interfaces or two of them, or with a JSON-RPC binding. A non-loopback interface URL is refused as such.
- `a_peer_url_is_plain_http_on_loopback_with_an_explicit_port`:
  - Accepted: `http://127.0.0.1` or `http://localhost` (in any case) with a port, with or without a trailing `/`.
  - Refused: an empty or unparsable URL, https, no port, other loopback or IPv6 addresses, a host name, a user name, a password, a query, a fragment, and a path.
  - Port 80 is refused as well, because the URL parser reports the http default port as no port.
- `information_errors_keep_their_codes`: all nine error codes keep their `a2a_*` strings.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_a2a --all-targets --all-features -- -D warnings` clean. Clippy's `type_complexity` lint asked for named types for the two case tables (`ShareCase`, `CardCase`).
- `cargo test -p vibemux_a2a --all-features`: 42 library tests passed (6 new), and the 24 integration tests in `basic_information_share`, `task_gateway`, `task_grpc`, and `task_runtime` passed.
- Fifty-seven mutations were caught and restored byte-for-byte.
  - Validation: skipping any of the three share identifier checks; allowing empty text or text one byte over 4 KiB; allowing 17 metadata entries, an empty key, a longer key or value, or NUL in a key or a value; and, in the shared identifier check, allowing an empty or 129-byte identifier or `/`, or dropping the field name.
  - Building messages: not validating the share; sending a share as the agent or an acknowledgement as the user; leaving out either context id; renaming either contract; and skipping the acknowledgement's correlation or receiver check.
  - Parsing messages: for both shares and acknowledgements, ignoring the role, the part count, the contract name, an invalid id or share, or the context id, and accepting unknown fields in the envelope or the payload; and requiring share metadata.
  - Card, URL, and codes: ignoring the share skill or requiring it to be the only skill; allowing two interfaces; ignoring the binding or the interface URL; renaming the card or its skill; allowing https, another loopback address, the default port, a user name, a password, a query, a fragment, or a path, or refusing `localhost`; and renaming an error code.

**Remaining:**
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Pin the harness dispatch route config bounds in vibemux_harness

**Scope:** `crates/vibemux_harness/src/dispatch/route_config.rs` validates the operator's `.vibemux/harness_dispatch.json` (ADR 029). It checks the config size and route count, the capture and timing limits, the protocol matrix, executable and entry-script paths, the DSH launcher pair, environment names, and model and provider labels. It also holds the execution and probe gates and the content-free catalog and `Debug` projections. The module had no unit tests; the 22 integration tests in `tests/dispatch_config.rs` covered it. A sweep of 182 mutants against the crate's tests left 52 alive. This change is test-only.

**Implemented:**
- A new test module in `route_config.rs` with 12 unit tests. No production code changed.
- `a_config_holds_at_most_64_kib_and_sixteen_routes`: a valid config padded with whitespace to exactly 65,536 bytes parses, and one more byte is `ConfigTooLarge`. Sixteen routes pass the count gate and reach the duplicate-harness check (`ConfigRouteInvalid`); seventeen are `ConfigInvalid`.
- `the_default_limits_keep_their_values`: omitted limits default to a 128 KiB frame, 16 MiB of capture, 16,384 records, a 600,000 ms deadline, and a 2,000 ms shutdown grace, and the defaults validate.
- `only_the_read_only_protocols_admit_execution_and_only_with_consent`:
  - Only `codex_exec`, `codex_app_server`, and `claude_stream_json` have a read-only dispatch posture.
  - Codex app-server and Claude stream-json routes are admitted for execution.
  - A DSH route with `allow_execution` is `ExecutionDisabled`, and so is a `codex_exec` route without `allow_execution`.
- `a_probe_refuses_an_unsupported_protocol_before_detection`: a `codex_exec` route on an undetected harness is `ProbeUnsupported`, not `NotDetected`. The catalog reports a disabled route as disabled.
- `the_host_policy_matches_the_platform`: Windows uses the native-image policy and refuses an extensionless executable; other hosts use the POSIX policy and accept it.
- `route_debug_shows_only_the_shape_and_counts`: a route's `Debug` output is exactly its harness, protocol, enabled and execution flags, whether it is a launcher pair, and its environment name count. The executable, entry script, provider, model, and environment names never appear.
- `a_launcher_pair_route_needs_a_valid_provider_and_a_model`: a DSH route whose provider is `-p`, `a b`, or empty is refused, and so is one without a model.
- `executable_and_entry_script_paths_hold_at_most_1024_bytes`: a 1,024-byte absolute executable path and entry script are accepted; 1,025 bytes are refused.
- `an_entry_script_refuses_nul_and_folds_the_extension_case`: NUL in an entry script is refused, and `BIN.JS` is accepted.
- `launcher_and_harness_stems_fold_case_only_on_windows`: `NODE.EXE` is the Node image under the Windows policy. Under the POSIX policy `node` is the Node image and `Node` is not. A path with no file stem (`..`) names neither the Node image nor a harness.
- `profiles_are_bounded_lowercase_names`: a profile is 1 to 64 bytes, starts with a lowercase letter, and continues with lowercase letters, digits, `_`, or `-`. `1sdk`, `sDk`, and `s.dk` are refused.
- `environment_names_and_route_labels_reach_their_exact_bounds`: 24 environment names and a 128-byte name are accepted. A 256-byte route label is accepted and a 257-byte one is refused.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_harness --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemux_harness --all-features`: 102 library tests passed (12 new), and the 61 integration tests in `dispatch_attempt`, `dispatch_config`, `dispatch_framing`, `dispatch_protocols`, and `writable_fixture_guard` passed.
- The sweep ran 182 mutants against that suite. The existing tests caught 129. One mutant (dropping the duplicate environment-name check) did not compile; rerun in a compiling form, it was caught by `environment_names_are_upper_snake_case_unique_and_not_reserved`. The source was restored byte-for-byte.
- The new tests catch all 50 non-equivalent survivors:
  - Bounds: a 64 KiB config, sixteen routes, a 1,024-byte executable path or entry script, a 64-byte profile, 24 environment names, a 128-byte environment name, and a 256-byte route label, each by its comparison and by its constant.
  - Defaults: each default limit, by its field and by its constant.
  - Gates: dropping either read-only protocol other than `codex_exec`, ignoring `allow_execution`, checking probe detection before probe support, and reporting every catalog entry as enabled.
  - Host policy: inverting the platform check.
  - Debug output: printing the model, the entry script, or the provider, inverting the launcher-pair flag, or zeroing the environment name count.
  - Launcher pair: accepting any provider label or a missing model.
  - Entry script: allowing NUL, or comparing the extension case-sensitively.
  - Stems: comparing the Node image case-sensitively on Windows or case-insensitively on POSIX, and accepting a path with no file stem as the Node image or as a harness.
  - Profiles: accepting a leading digit, an uppercase letter, or `.`.
- Two survivors are equivalent and not counted:
  - Dropping the non-empty file-stem check on an entry script: a `.js` extension always comes with a non-empty stem.
  - Accepting any launcher-pair model in the pair arm: the general model check that follows validates it again.

**Remaining:**
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Pin the harness dispatch request fingerprint and prompt bound in vibemux_harness

**Scope:** `crates/vibemux_harness/src/dispatch/request.rs` holds the typed dispatch request (ADR 029). It validates the request's schema version, id, and prompt; derives the content-free prompt digest; computes the idempotency fingerprint that the store persists; and prints a `Debug` form without the prompt. The module had no unit tests; two integration tests in `tests/dispatch_config.rs` covered it. A sweep of 30 mutants against the crate's tests left 8 alive. This change is test-only.

**Implemented:**
- A new test module in `request.rs` with four unit tests. No production code changed.
- `a_prompt_holds_at_most_32_kib`: a 32,768-byte prompt is valid, and 32,769 bytes is `InvalidPrompt`.
- `the_fingerprint_is_a_known_answer_over_its_fields`:
  - Pins the prompt digest of a synthetic prompt and the fingerprint of a fixed request, project, protocol, and config digest to literal SHA-256 values.
  - Both values were computed outside Rust from the layout that `request_fingerprint` and `hash_fields` implement. That layout is SHA-256 over the domain `vibemux.harness_dispatch.fingerprint.v1` and a NUL byte, followed by each field as an eight-byte big-endian length and its bytes.
  - The fields are, in order: the hyphenated project id, the hyphenated request id, the harness command name, the protocol name, the raw prompt digest, and the raw config digest. The Rust result matched on the first run.
- `request_debug_shows_the_id_harness_and_prompt_size_only`: a request's `Debug` output is exactly its request id, harness, and prompt byte count.
- `a_prompt_digest_refuses_unknown_fields`: a prompt digest round-trips through JSON, and an extra field is refused.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_harness --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemux_harness --all-features`: 106 library tests passed (4 new), and the 61 integration tests in `dispatch_attempt`, `dispatch_config`, `dispatch_framing`, `dispatch_protocols`, and `writable_fixture_guard` passed.
- The sweep ran 30 mutants against that suite. The existing tests caught 22, and the source was restored byte-for-byte.
- The new tests catch all 8 survivors:
  - Prompt bound: changing the 32 KiB constant.
  - Fingerprint: changing the domain, spelling the request id without hyphens, upper-casing the harness name, and swapping the prompt and config digests.
  - Debug output: dropping the request id or the harness.
  - Prompt digest: accepting unknown fields.

**Remaining:**
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Pin the harness dispatch capture budget in vibemux_harness

**Scope:** `crates/vibemux_harness/src/dispatch/capture_budget.rs` accounts for one dispatch attempt's stdout records (ADR 029). For each frame it enforces the capture byte and record limits before parsing, assigns the record sequence, classifies the record, and folds it into the per-kind counts and the transcript digest. Its content-free `CaptureSummary` is persisted. The module had no unit tests; two integration tests in `tests/dispatch_framing.rs` covered it, using only five of the nine observation kinds and a frame limit equal to the capture limit. A sweep of 33 mutants against the crate's tests left 15 alive. This change is test-only.

**Implemented:**
- A new test module in `capture_budget.rs` with five unit tests. No production code changed.
- `each_kind_lands_in_its_own_count_and_the_total`: one Codex app-server notification per observation kind is classified as that kind. Each of the nine counts is then 1, and the total and the record count are 9.
- `counts_and_their_total_saturate`: incrementing a count at `u64::MAX` keeps it there, and a total over a saturated count is `u64::MAX`.
- `the_capture_bound_is_the_byte_limit_and_admits_an_exact_fill`: with a 64-byte frame limit and a 128-byte capture limit, two 64-byte records fill the capture exactly and are accepted. The next record is `CaptureTooLarge`, and 128 bytes stay counted.
- `a_rejected_frame_leaves_no_trace`: a frame that is not JSON (`InvalidJson`) or not an object (`InvalidRecord`) leaves the record count, the byte count, and the transcript digest unchanged. The next accepted record is sequence 1, and the digest covers only that record and its LF.
- `summaries_round_trip_and_refuse_unknown_fields`: a capture summary round-trips through JSON. An extra field in the summary or in its kind counts is refused.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt --all -- --check` clean. `cargo clippy -p vibemux_harness --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemux_harness --all-features`: 111 library tests passed (5 new), and the 61 integration tests in `dispatch_attempt`, `dispatch_config`, `dispatch_framing`, `dispatch_protocols`, and `writable_fixture_guard` passed.
- The sweep ran 33 mutants against that suite. The existing tests caught 18, and the source was restored byte-for-byte.
- The new tests catch all 15 survivors:
  - Counting approval, usage, failed, or cancelled records in the wrong slot, or leaving any of those four out of the total.
  - Plain addition in place of saturation, in a count or in the total.
  - Bounding the capture by the frame limit instead of the capture limit, and refusing a capture that fills the limit exactly.
  - Counting a rejected frame's bytes.
  - Accepting unknown fields in the summary or in its kind counts.

**Remaining:**
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Pin the harness dispatch JSON line framer in vibemux_harness

**Scope:** `crates/vibemux_harness/src/dispatch/json_line_framer.rs`. It splits vendor stdout into LF-delimited UTF-8 frames under a byte limit. Its integration tests in `tests/dispatch_framing.rs` cover reassembly, the exact limit, and poisoning after a failed `push`. This change is test-only and has no wire or behavior change.

**Implemented:**
- A new test module in `json_line_framer.rs` with three unit tests. No production code changed.
- `a_failed_finish_poisons_the_framer`: an incomplete UTF-8 sequence left at `finish` fails with `InvalidUtf8`. A second `finish` and a later `push` of a valid frame return the same failure, and the later push emits no frame.
- `the_frame_limit_counts_a_trailing_cr`: with a 4-byte limit, `123` CR LF yields the frame `123`, and `1234` CR LF is `FrameTooLarge`, because the limit covers the bytes before LF, including the CR.
- `only_one_trailing_cr_is_removed`: `{}` CR CR LF yields `{}` CR, and a CR inside a frame is kept.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt -p vibemux_harness -- --check` clean. `cargo clippy -p vibemux_harness --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemux_harness --all-features`: 114 library tests passed (3 new), and the 61 integration tests in `dispatch_attempt`, `dispatch_config`, `dispatch_framing`, `dispatch_protocols`, and `writable_fixture_guard` passed.
- The sweep ran 19 mutants against that suite. The existing tests caught 16, and the source was restored byte-for-byte. A proptest regression file that a caught mutant wrote was removed.
- The new tests catch all 3 survivors:
  - Not poisoning the framer when `finish` fails.
  - Leaving a trailing CR out of the frame limit.
  - Removing every trailing CR instead of one.

**Remaining:**
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Pin the harness dispatch outcome wire names in vibemux_harness

**Scope:** `crates/vibemux_harness/src/dispatch/outcome.rs`. It holds `AttemptOutcome`, `ProcessExit`, `OutcomeDecision`, and `decide_outcome`, the single rule that turns exchange, protocol, and process evidence into an attempt outcome (ADR 029 §3). The integration test `outcomes_prefer_exchange_errors_then_process_exit_then_terminal_evidence` in `tests/dispatch_attempt.rs` covers the rule. This change is test-only and has no wire or behavior change.

**Implemented:**
- A new test module in `outcome.rs` with two unit tests. No production code changed.
- `outcome_names_match_their_serde_form`: each of the five outcomes has the `as_str` name `completed`, `failed`, `unverified`, `cancelled`, or `probed`. Each serializes to that same name and parses back from it, and the Rust variant spelling `Completed` is refused.
- `exits_and_decisions_round_trip_and_refuse_unknown_fields`: a `ProcessExit` and an `OutcomeDecision` round-trip through JSON, and an extra field in either is refused.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt -p vibemux_harness -- --check` clean. `cargo clippy -p vibemux_harness --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemux_harness --all-features`: 116 library tests passed (2 new), and the 61 integration tests in `dispatch_attempt`, `dispatch_config`, `dispatch_framing`, `dispatch_protocols`, and `writable_fixture_guard` passed.
- The sweep ran 18 mutants against that suite. The existing tests caught all 10 that changed `decide_outcome` or `is_clean`, and the source was restored byte-for-byte.
- The new tests catch all 8 survivors:
  - Changing any of the five `as_str` names.
  - Dropping the snake_case serde rename on `AttemptOutcome`.
  - Accepting unknown fields in `ProcessExit` or `OutcomeDecision`.

**Remaining:**
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Pin the harness dispatch resource key in vibemux_harness

**Scope:** `crates/vibemux_harness/src/dispatch/attempt.rs`. It holds the durable attempt phases, their Task/Run mapping, the `apply` transition table, and `resource_key`, the working-directory reservation hash the store keeps in place of the path (ADR 029 §3). The integration tests in `tests/dispatch_attempt.rs` cover the transition table, status pairs, fences, reservations, and phase names. This change is test-only and has no wire or behavior change.

**Implemented:**
- A new test module in `attempt.rs` with one unit test. No production code changed.
- `the_resource_key_is_a_known_answer_over_the_directory`: the keys for `C:\projects\synthetic` and for an empty directory string equal SHA-256 digests computed independently in Python. Each digest covers the domain `vibemux.harness_dispatch.resource.v1`, a NUL byte, and the directory with its u64 big-endian length prefix.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt -p vibemux_harness -- --check` clean. `cargo clippy -p vibemux_harness --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemux_harness --all-features`: 117 library tests passed (1 new), and the 61 integration tests in `dispatch_attempt`, `dispatch_config`, `dispatch_framing`, `dispatch_protocols`, and `writable_fixture_guard` passed.
- The sweep ran 22 mutants against that suite. The existing tests caught 19, and the source was restored byte-for-byte.
- The new test catches 2 of the 3 survivors: a changed key domain, and an extra hashed field.
- The third survivor is equivalent and is not counted. It drops the `from.holds_reservation()` check from `releases_reservation`, but every `Changed` transition starts in a non-terminal phase, and those phases all hold the reservation.

**Remaining:**
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Pin the harness dispatch event names and payloads in vibemux_harness

**Scope:** `crates/vibemux_harness/src/dispatch/events.rs`. It builds the content-free canonical event drafts for dispatch attempts, with their deterministic idempotency keys (ADR 029 §3). The integration tests in `tests/dispatch_attempt.rs` check that every draft is valid, scoped, and content-free. They compare event types against the module's own constants and check keys only by prefix. This change is test-only and has no wire or behavior change.

**Implemented:**
- A new test module in `events.rs` with four unit tests. No production code changed.
- `event_types_and_idempotency_keys_have_fixed_names`: the five event types are `harness_dispatch_admitted`, `harness_dispatch_started`, `harness_dispatch_cancel_requested`, `harness_dispatch_finished`, and `harness_dispatch_recovered`. Each idempotency key is exactly `harness_dispatch:<hyphenated request id>:<suffix>`, where the suffix is the type without the `harness_dispatch_` prefix. A type without that prefix is kept whole.
- `every_payload_refuses_unknown_fields`: each of the six payloads round-trips through JSON, and refuses an extra field.
- `constructors_copy_every_fact`: `AdmittedPayload::new` keeps the prompt digest, the prompt size, and the config digest. `ProcessSummary::new` keeps the exit code, forced termination, and stderr byte count.
- `a_cancel_request_records_its_from_phase_without_a_causation`: a cancel from `admitted` records `admitted`, and the draft has no causation id.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt -p vibemux_harness -- --check` clean. `cargo clippy -p vibemux_harness --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemux_harness --all-features`: 121 library tests passed (4 new), and the 61 integration tests in `dispatch_attempt`, `dispatch_config`, `dispatch_framing`, `dispatch_protocols`, and `writable_fixture_guard` passed.
- The sweep ran 22 mutants against that suite. The existing tests caught 3, and the source was restored byte-for-byte.
- The new tests catch 18 of the 19 survivors:
  - Renaming any of the five event types.
  - Keeping the `harness_dispatch_` prefix in the key suffix, or adding a trailing separator.
  - Accepting unknown fields in any payload.
  - Dropping the exit code, forced termination, or prompt size.
  - Recording a fixed from-phase in a cancel request.
  - Setting a causation id.
- The remaining survivor is equivalent and is not counted. It drops the final `EventDraft::validate` call, but `EventPayload::new` already validates the payload, and the fixed-suffix idempotency key is never empty or over the bound.

**Remaining:**
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Pin the writable fixture launch guard in vibemux_harness

**Scope:** `crates/vibemux_harness/src/dispatch/fixture_guard.rs`, which is compiled only with the `test_helpers` feature. It admits launches only of exact registered copies of the built synthetic worker, and builds their fixture argv. The integration tests in `tests/writable_fixture_guard.rs` cover profile and grant arguments, byte and path changes at launch, duplicate aliases, a source inside the vendor root, and symlinked copies. This change is test-only and has no wire or behavior change.

**Implemented:**
- A new test module in `fixture_guard.rs` with six unit tests. No production code changed. The tests write synthetic images under a unique directory in the system temporary directory and remove it on drop.
- `registration_needs_one_or_two_aliases_and_exact_nonempty_copies`: registration refuses an empty alias list, a copy whose bytes differ from the source, and an empty image.
- `a_route_must_be_enabled_executable_matching_and_its_own_copy`: a disabled route, a route without execution consent, and a protocol that does not accept the harness are `ExecutionDisabled`. A Codex route that points at the registered Claude copy is `ConfigExecutableInvalid`.
- `launch_argv_is_exact_per_protocol`: Codex gets exactly `exec --json --fixture_profile codex_fixture_writable_v1 --fixture_grant_json <grant> -`, with the prompt on stdin. Claude gets the stream-json input and output flags, its own profile, and the grant, with no stdin marker.
- `a_development_runtime_is_pinned_and_passed_by_flag`: a pinned runtime is passed as `--fixture_node <canonical path>` before the Codex stdin marker. Changing its bytes refuses the next launch.
- `canonical_helpers_refuse_relative_paths_and_the_wrong_kind`: the file helper refuses a directory, the directory helper refuses a file, and a relative path is refused.
- `a_drive_relative_path_is_refused` (Windows only): a drive-relative path such as `<drive>:Cargo.toml`, built at run time from the current drive, is refused. Its ancestors resolve to the current directory, so the absolute-path check is its only guard.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt -p vibemux_harness -- --check` clean. `cargo clippy -p vibemux_harness --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemux_harness --all-features`: 127 library tests passed (6 new), and the 61 integration tests in `dispatch_attempt`, `dispatch_config`, `dispatch_framing`, `dispatch_protocols`, and `writable_fixture_guard` passed.
- The sweep ran 25 mutants against that suite. The existing tests caught 4, and the source was restored byte-for-byte.
- The new tests catch 15 of the 21 survivors:
  - Admitting an empty alias list, a copy with different bytes, or an empty image.
  - Skipping the enabled, execution-consent, or protocol check.
  - Admitting another alias's copy.
  - Changing the Codex or Claude argv, or always adding the stdin marker.
  - Skipping the runtime recheck, or renaming its flag.
  - Admitting a drive-relative path.
  - Dropping the file or directory kind check.
- Four survivors are equivalent and are not counted:
  - Both copy-parent checks: a canonical path with no reparse ancestor that equals the expected copy is always directly under the vendor root.
  - The vendor-root recheck: the route executable's ancestor reparse check and exact path match already cover it.
  - The `is_symlink` check on Windows: the reparse-point attribute check covers it, and the Unix integration test covers it there.
- Two survivors need host features this machine does not provide in a test:
  - The source path recheck differs only when the source becomes a symlink, which needs symlink privilege.
  - The reparse-attribute check differs only for a reparse point that is not a link.

**Remaining:**
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Pin the harness dispatch observation classifier in vibemux_harness

**Scope:** test-only. `crates/vibemux_harness/src/dispatch/observation.rs` had no unit tests; the integration tests in `tests/dispatch_protocols.rs` replay four fixture streams and a few terminal tables. No production code, wire format, or classification behavior changed.

**Implemented:** a unit-test module in `observation.rs` with eight tests:
- `kind_names_match_their_serde_form_and_only_results_are_terminal`: each of the nine kinds has its exact name and serde form, and only `completed`, `failed`, and `cancelled` are terminal.
- `parse_keeps_the_exact_bytes_and_refuses_bad_frames_by_code`: a parsed record keeps its sequence, kind, exact bytes, and byte count. `Debug` prints only the sequence, kind, and byte count. Sequence 0 is `Internal`, malformed JSON is `InvalidJson`, and an array, number, string, or null is `InvalidRecord`.
- `codex_exec_classifies_every_verified_discriminant`: every Codex exec event type and every item type under each of the three item events, with error and future items `Opaque`.
- `codex_app_server_classifies_every_verified_discriminant`: every method, all five approval requests, each turn status, and every item type under `item/started` and `item/completed`.
- `claude_classifies_every_verified_discriminant`: only an `init` system record starts, only `can_use_tool` asks for approval, a user text block stays `Opaque`, and each stream event needs its own delta, tool block, or usage object.
- `claude_results_need_root_scope_and_a_clean_success`: a success needs no error flag and no unknown terminal reason. Each error subtype and each terminal reason maps to its result, and a nested result stays `Opaque`.
- `acp_classifies_every_verified_discriminant`: a message chunk needs text content, and a stop reason counts only when the record has no `method` key at all.
- `dsh_classifies_turn_events_and_never_ends_on_status`: every session event type and every `turn/end` reason. A status notification, a bare `turn/end`, and an unknown reason stay `Opaque`.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt -p vibemux_harness -- --check` clean. `cargo clippy -p vibemux_harness --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemux_harness --all-features`: 135 library tests passed (8 new), and the 61 integration tests in `dispatch_attempt`, `dispatch_config`, `dispatch_framing`, `dispatch_protocols`, and `writable_fixture_guard` passed.
- The sweep ran 177 mutants against that suite: 159 that rename one string literal and 18 that drop or loosen a guard. The existing tests caught 91, 85 survived, and one malformed mutant did not compile; the source was restored byte-for-byte.
- The new tests catch all 85 survivors and the corrected form of the malformed mutant. The survivors included every DSH discriminant, most Codex app-server methods and item types, the Claude `system`, `control_request`, stream-event, and success guards, the ACP `method` presence check, the kind names, `is_terminal` for `cancelled`, `byte_count`, and the `Debug` field names.

**Remaining:**
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

### 2026-10-18 - Pin the harness dispatch protocol sessions and module root in vibemux_harness

**Scope:** test-only. `crates/vibemux_harness/src/dispatch/protocol_session.rs` and `crates/vibemux_harness/src/dispatch/mod.rs` were the last dispatch files without unit tests. The integration tests in `tests/dispatch_protocols.rs` replay the Codex exec, Codex app-server, Claude, and ACP fixture streams and never drive a DSH session. No production code, wire format, handshake, or correlation rule changed.

**Implemented:** a unit-test module in `protocol_session.rs` with sixteen tests, and one in `mod.rs` with two:
- `terminal_names_and_debug_forms_are_pinned_and_content_free`: the four terminal serde names, and the exact `Debug` forms of `InitialInput` (the prompt's byte count only) and `ProtocolSession` (no prompt, working directory, or identifiers).
- `construction_checks_the_route_prompt_and_working_directory`: `SessionRoute::from` copies the model and provider, the daemon's prompt bound is exactly `MAX_PROMPT_BYTES`, and a caller's bound is inclusive. An empty or whitespace prompt, an empty or NUL working directory, a harness the protocol does not accept, and a Codex exec probe are each refused by code.
- `sessions_start_in_their_protocol_phase_and_dsh_ids_are_input_digests`: Codex exec starts at the prompt and no session starts finished. The DSH session id is `vibemux_` plus 32 hex digits of the domain-separated digest of the working directory and prompt, and it changes with either.
- `initial_inputs_pin_every_handshake_and_offer_no_client_capability`: the exact first frame of all five protocols, including the client name, the crate version, and ACP's disabled filesystem and terminal capabilities.
- `cancel_messages_address_only_an_identified_turn_or_session`: DSH and Codex exec have none, ACP needs its session, the app server needs both its thread and its turn, and Claude always interrupts.
- `codex_exec_maps_each_terminal_kind_once_and_keeps_observing_progress`: each terminal kind maps to its result once, progress records still pass after the finish, and a non-terminal kind never finishes.
- `codex_app_server_exchange_pins_every_reply` and `codex_app_server_rejects_uncorrelated_or_malformed_evidence`: every reply in order, approval declines for string and negative ids, and refusals for a non-object result, a missing thread or turn id, a turn result that contradicts a started turn, a foreign thread, a completion before any turn, and any record after the finish.
- `acp_exchange_pins_every_reply_and_maps_each_stop_reason`: the session and prompt requests, the permission refusal, a foreign session update, and each stop reason.
- `probes_finish_on_a_valid_handshake_and_send_nothing_more`: the app server, ACP, Claude, and DSH probes end `probed` and finished.
- `claude_exchange_sends_one_user_message_and_refuses_every_request` and `claude_handshake_needs_a_successful_correlated_response`: the exact user message, interrupt acknowledgements before and after the handshake, a second handshake, and every refused request id shape.
- `dsh_exchange_pins_the_prompt_and_ends_only_on_its_own_turn`, `dsh_turn_end_correlates_the_turn_and_waits_out_unknown_reasons`, and `dsh_rejects_malformed_or_uncorrelated_frames`: the crate's first DSH session coverage. Another session's turn end, status and subagent notifications, and unknown methods never end the turn. An unknown reason waits, a second turn number is uncorrelated, and each malformed frame fails with its own code.
- `identifiers_and_rpc_ids_are_bounded`: the 512-byte identifier and RPC id bounds, signed integer ids, and the refused id shapes.
- `root_reexports_name_their_module_items` and the feature-gated `fixture_guard_reexports_name_their_module_items` in `mod.rs`: each root re-export names the item its module defines.

**Evidence (native Windows, Rust 1.92.0; `CARGO_TARGET_DIR` on C: because I: is full):**
- `cargo fmt -p vibemux_harness -- --check` clean. `cargo clippy -p vibemux_harness --all-targets --all-features -- -D warnings` clean.
- `cargo test -p vibemux_harness --all-features`: 153 library tests passed (18 new), and the 61 integration tests in `dispatch_attempt`, `dispatch_config`, `dispatch_framing`, `dispatch_protocols`, and `writable_fixture_guard` passed. Without features, the 146 library tests passed.
- The `protocol_session.rs` sweep ran 386 mutants against the old suite: 213 that rename one string literal and 173 that drop or loosen a guard or change a value. The existing tests caught 196 and 190 survived, including all 68 mutants in the DSH receive path, most initialize-frame literals, the `Debug` forms, the DSH id derivation, and the identifier bounds. The source was restored byte-for-byte.
- The new tests catch 186 of the 190. The other four are equivalent: giving every session a DSH id (only DSH paths read it), dropping or loosening the length check on the echoed DSH id (it is first compared equal to the session's own 40-byte id), and dropping the `terminal.is_none()` check after a DSH turn end (only terminal kinds reach it, and those always set a terminal).
- The `mod.rs` sweep ran 47 mutants: 2 renames of the `test_helpers` feature literal, 27 dropped re-export items, 16 `pub mod` to `pub(crate) mod` narrowings, and 2 removed cfg gates. The compiler rejected 33 and 14 survived. The new tests make the 7 surviving dropped re-exports fail to compile: `AdapterCapability`, `AdapterSupport`, `adapter_capabilities`, `adapter_capability`, `OutcomeDecision`, `SessionRoute`, and `fixture_profile_id`.
- A unit module cannot see the other 7. Narrowing `capability`, `digest`, `error_code`, `final_text`, or `fixture_guard` changes nothing inside the crate: a `vibemuxd` build rejects the last two, and nothing names the first three by module path. Removing either `test_helpers` gate is equivalent under `--all-features`; a default-feature build rejects the ungated re-export, while the ungated module compiles either way.

**Remaining:**
- Owner decision on no-follow opens for bounded reads.
- Also open: owner decisions on ACP execution, load-time refusal of Windows non-`.exe` executables, ACP routes with a model, the daemon's full model alphabet, a CI rustdoc step, the TUI diagnostics environment display, and the GUI's minimum inner width of 960 against the settings minimum of 800. The I: drive still needs space before the repository `target` can build.

## DSH cancellation response race: local review and repair (2026-10-20)

**Scope:** the dedicated SDK session state machine and its local fixture coverage.
The cancellation fix in `0c7e116` was reviewed independently of the existing
runtime-exit close fixes. A prompt response that arrives after cancel or close
is correlated against the bounded abandoned-request set and consumed once;
unknown and duplicate response IDs still fail. Cancellation remains process-tree
termination and resolves the turn once, without turning a late completion into
success. Daemon ownership, Control v7 status, persistence, and the production
close path are unchanged by this follow-up.

**Local review repair:** the new late-turn allowance now validates the event's
turn number and matches it to a start observed before or after cancellation.
Missing, null, string, and negative numbers fail with `dsh_invalid_turn_end`;
a mismatched late end fails with `dsh_unexpected_turn_end`. Both new regression
tests failed before the repair and passed afterward. The original cancellation
ordering was also replayed against `94d2a0c` and reproduced
`UncorrelatedResponse`; the temporary baseline source was then restored.

The `late_prompt` synthetic runtime withholds prompt frames until it receives
shutdown. Driver tests cover cancellation before any receipt and closing after
abandoning a prompt, without repeated timing trials. Exact buffered-frame
acceptance is asserted by the pure-machine tests: driver close may stop the
reader after process exit, so the integration test does not require every
buffered frame to have been consumed.

**Validation executed locally on native Windows, Rust 1.92.0:**

- `cargo test --locked --offline -p vibemux_harness --lib dsh::session::tests -- --nocapture`: 39 passed.
- `cargo test --locked --offline --quiet -p vibemux_harness -p vibemux_dsh --all-features`: 249 passed, zero failures or ignored tests.
- `cargo test --locked --offline --quiet -p vibemuxd --features test_helpers --test dsh_session`: 10 passed, zero failures or ignored tests.
- `cargo clippy --locked --offline --quiet -p vibemux_harness -p vibemux_dsh -p vibemuxd --all-targets --all-features -- -D warnings`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.

Initial sandbox runs hit path-canonicalization permission errors. Normal local
execution resolved those; fixture tests also required command-local `TEMP` and
`TMP` under the task's `target/dsh_test_tmp`. The final broader checks used
`CARGO_INCREMENTAL=0`, `CARGO_PROFILE_TEST_DEBUG=0`,
`CARGO_PROFILE_DEV_DEBUG=0`, and `CARGO_BUILD_JOBS=2` after MSVC D8050 blocked
C dependency debug-record generation. These are invocation settings only; no
source guard, test assertion, dependency, or machine configuration was weakened.

**Coverage limits:** local source review and synthetic SDK processes only.
Hosted PR review/CI, the full workspace test/release matrix, Linux/WSL, live
vendor inference, and a live terminal backend were not run. This evidence does
not change the project's release or live-runtime qualification.

**Integration into PR #28:** this repair was written on `0c7e116` and then
cherry-picked onto `4dbaaac`, which lets a closed session keep draining late
frames for abandoned work. The fixture keeps both the `shutdown-first` and
`late_prompt` modes. The `shutdown-first` driver test keeps its repeated
rounds. On the combined commit, these checks passed on native Windows: fmt;
clippy for vibemux_harness and vibemux_dsh; 165 vibemux_harness library tests;
and 18 vibemux_dsh session tests. These passed in WSL Ubuntu 24.04: fmt;
clippy for vibemux_harness, vibemux_dsh, vibemuxd and vibemux_frontend; the
vibemux_harness and vibemux_dsh tests; `vibemuxd --test dsh_session` (10/10);
the vibemux_frontend dsh tests; and the vibemux_dsh session suite 60 times
pinned to one loaded core, with no failures. Hosted CI covers the full
workspace on the PR.

### 2026-10-21 - ADR 042 multi-pane console and terminal UI functions

**Status:** "Probe and frontend shell" and "Rust CLI" stay `PARTIAL`. No control
operation, protocol version, schema, plugin payload, or persisted setting
changed. See [ADR 042](docs/adr/042_multi_pane_console_and_ui_functions.md).

**Implemented (`vibemux_frontend`, plus the `vibemuxctl ui` wiring):**

- `vibemuxctl ui` and `vibemux_dashboard` are a read-only multi-pane console:
  seven views (Overview, Harnesses, Workspace, and the daemon views Tasks,
  Daemon, Sessions, Decisions), single, split, and grid layouts with a
  size-based fallback, pane focus and zoom, per-list filters, refresh, and the
  six terminal themes. `vibemux_dashboard` gains `--project-root`.
- `dashboard::daemon_feed` polls only content-free reads (health, plugins,
  dispatch catalog, task list and detail, DSH status, workflow slots,
  decisions) on one background thread, through the `daemon_session` cache now
  shared with the GUI client. Health has a 2-second deadline, so a crashed
  daemon shows "not answering" within one short poll.
- The diagnostic TUI re-runs the read-only probe on a worker thread (`r`) and
  selects harnesses (`j`/`k`) with a details footer; golden fixtures are
  unchanged.
- The DSH surface searches its in-memory transcript (`Ctrl-F`) and jumps with
  `Home`/`End`.
- The GUI gains ADR 030's shortcut table, `Ctrl+/` overlay, and `Ctrl+K` quick
  switcher. Send stays unavailable.
- Fixed in passing: the DSH surface and the diagnostic TUI ignore key release
  events, which Windows terminals report. Before, every key acted twice in the
  DSH prompt, and an Esc release could cancel a running turn.

**Not implemented:** dispatch or workflow list views (they need a new control
operation), PTY/ConPTY attach or embedded vendor TUIs, the GUI Send action,
and ADR 030's Match system, Reduce motion, spark animation, and copy buttons.
Interactive terminals and windows were not driven by hand: rendering is
verified with Ratatui `TestBackend` and headless egui frames only.

**Validation executed (Rust 1.92.0):**

- Native Windows 11, at `6c5d2e3`, with `CARGO_TARGET_DIR` under `%TEMP%`:
  - `cargo +1.92.0 fmt --all -- --check`: passed.
  - `cargo +1.92.0 clippy --workspace --all-targets --all-features --exclude vibemux_web -j 6 --locked -- -D warnings`: passed.
  - `cargo +1.92.0 test -p vibemux_frontend -p vibemux_cli --all-features -j 6 --locked`: passed. `vibemux_frontend` lib 202 passed (1 ignored: the golden fixture generator), `daemon_feed` 4, `cli` 6, `supervisor_ui` 10, `palette_parity` 2, binaries 2 + 4 + 6 + 2; `vibemux_cli` lib 59, `unified_commands` 7, `bootstrap_process` 4.
- WSL Ubuntu 24.04 (Git 2.43.0), a fresh clone at `6c5d2e3`: the same three
  commands passed with the same counts, except `vibemux_cli` lib 55 (its
  Windows-only tests do not build on Linux).
- `vibemux_web` was excluded because its web frontend was not built in these
  checkouts. No vendor CLI or paid API ran: the re-probe test uses an empty
  `PATH`, and the feed tests start a local test daemon in a temporary project.

**Review:** one fresh whole-branch review. It found one critical defect (the
Esc release above) and minor ones. The critical defect, a crash that kept the
console "connected" for the 10-second pipe retry, a search highlight that
drifted at the 2000-line cap, and a re-probe that could stay "probing" forever
were fixed with tests that failed first. Deferred minors: a feed thread that
exits early leaves the last view on screen; kept decision rows survive a
database reset; held `r` keys queue several polls; the diagnostic TUI cuts
long versions without an ellipsis at 80 columns; operator-typed filter text is
not sanitized; GUI chords repeat while held.

### 2026-10-22 - Web console views and opt-in local page mode in vibemux_web

**Status:** Track A (`vibemux_web`, ADR 033) stays `PARTIAL`. No control
operation, protocol version, store schema, or plugin payload changed. The web
API moves to 0.2.0 (pre-alpha, no compatibility promise). See the ADR 033
note of 2026-10-22 and [docs/vibemux_web.md](docs/vibemux_web.md).

**Implemented (`crates/vibemux_web`):**

- The board has five views: Tasks, Daemon, Sessions, Decisions, Device
  (tabs, or keys `1` to `5`; `R` refreshes). Daemon, Sessions, and Decisions
  are the ADR 042 console's daemon views with the same content-free fields.
  The Tasks detail adds the task's latest events (type, time, sequence) and
  artifacts (id, validated media type, size; no digest).
- `console_poller` reads health, plugin status, the dispatch catalog, DSH
  status, workflow slots, and the decision list on the configured interval;
  `console_model` projects them; `GET /api/v1/console` serves the result.
  Each section is `waiting`, `loaded`, or `unavailable` with a code and an
  operator hint, and fails on its own. An unreachable daemon makes every
  section unavailable instead of showing an earlier observation. Kept
  decisions (newest 64) are cleared when the daemon generation changes.
- `content_label` holds the label sanitizers (identifiers, timestamps, media
  types), so no path separator or free text passes whole.
- `local_page_without_token` (default `false`, operator decision 2026-10-22):
  GET/HEAD requests for the page and its static assets skip the bearer only
  when the request names the bound loopback listener literally, in `Host`
  and, for an absolute-form or HTTP/2 request, in the URI authority too.
  Every JSON route, including enrollment and OpenAPI, still needs the token,
  and token-free page loads do not use the failed-attempt budget. The
  service worker script needs it too, so a token-free page never installs a
  worker whose precached page would outlive turning the mode off (no offline
  cache or push in this mode).
- A failed refresh marks the views stale instead of leaving the last
  observation on screen as current, and a task detail read that finishes
  after another task was selected is dropped.
- Fixed in passing: the board's polling effect re-ran after every refresh, so
  it polled about 110 times a second instead of every 5 seconds (present on
  `main` before this change); event times lost their spaces; the enrollment
  panel overflowed at tablet width.

**Not implemented:** dispatch or workflow list views (no enumeration
operation exists), any write or execution control, prompt or transcript
views. The local page mode is not checked behind `tailscale serve`; a proxy
that rewrites `Host` to the loopback address would publish the page itself.

**Validation executed (Rust 1.92.0, Node 22.22.2; the package pins 22.22.0):**

- Native Windows 11, with `CARGO_TARGET_DIR` under `%TEMP%`:
  - `npm ci`, `npm run typecheck` (0 errors, 0 warnings), `npm test` (15
    passed: 10 existing, 5 new), `npm audit --audit-level=moderate` (0
    vulnerabilities), `npm run build`: passed.
  - `cargo +1.92.0 fmt --all -- --check`: passed.
  - `cargo +1.92.0 clippy -p vibemux_web --all-targets --all-features -j 6 --locked -- -D warnings`: passed.
  - `cargo +1.92.0 test -p vibemux_web --all-features -j 6 --locked`: passed.
    Library 32, `console_poller` 2, `control_bridge` 3, `http_contract` 9.
  - The new absolute-form test failed (200) with the earlier Host-only rule
    put back temporarily, and passes with the fix.
- WSL Ubuntu 24.04 (Git 2.43.0, Node 22.22.2), a fresh clone at `b844f78`:
  `npm ci`, typecheck, `npm test` (15), `npm run audit` (0 vulnerabilities),
  build, `cargo fmt --all -- --check`, the same clippy command, and the same
  test command passed: library 31 (one Windows-only test), `console_poller` 2,
  `control_bridge` 3, `http_contract` 9.
- Browser check, native Windows, the app's built-in browser: a scratch
  project with two fixture tasks seeded through the store writer (the same
  seeding as the integration tests), a daemon started with `vibemuxctl
  daemon start`, and `vibemux_web` with `local_page_without_token: true`.
  The page loaded without a token, the API answered 401 without one, and
  all five views rendered the daemon's data after the fixture token was
  entered. With the daemon stopped, every console section read "Start it with
  vibemuxctl daemon start (daemon_not_running)" and task titles were hidden.
  No horizontal overflow at 375, 768, or 1280 pixels after the panel fix; no
  CSP errors. After the review fixes: one refresh per 5 seconds, the task
  detail no longer blanks between refreshes, and with the web process
  stopped the views read "These views are not current". (Before `/sw.js`
  was made token-only, service-worker registration also failed in that pane,
  "unknown error when fetching the script", although the server returned
  `/sw.js` with 200.)
- No vendor CLI, Jev, or paid API ran. The daemon had no dispatch, DSH,
  workflow, or decision configuration.

**Review:** one fresh read-only review of the branch. No blocker or major
finding; it confirmed that no `/api/`, OpenAPI, or enrollment handler is
reachable on the token-free path (raw paths are matched without
normalization, so `//api`, `/./api`, `/%61pi` and similar fall through to the
static page). Fixed: absolute-form authority versus `Host`, stale marking on
a failed refresh, out-of-order task detail, path-leak checks that could not
fail on Windows (JSON escapes backslashes), and docs that still said every
route needs the bearer. Not covered: a poller test with recorded decisions
(catch-up and generation change are covered by the `merge_decisions` unit
test only); on a long decision history the view shows an older window for a
few polls, which its truncation note discloses. The hosted Codex review then
found two more: a service worker installed through the token-free page
would keep serving its precached page after the mode was turned off (fixed:
`/sw.js` keeps requiring the token), and the footer of the console views
showed the task poller's observation time (fixed: they show the console's).

### 2026-10-22 - GUI: console-less launch, rounder scale, full-page Settings, terminal link in Overview

**Scope:** `vibemux_frontend` GUI only (PR 1 of 2 agreed with the operator;
the Console page that absorbs Agents follows). No daemon, store, control
protocol, CLI, or observer contract changes. ADR 030 records the amendment.

**Implemented:**
- The GUI binary is a Windows GUI-subsystem binary in debug and release
  builds, so it never opens a console. Startup failures and panics go to a
  bounded `frontend_errors.log` next to the settings file (`startup_log`).
- One rounded scale for every theme (10 px buttons, 14 px cards and windows,
  20 px composer, pill chips); every palette's `corner_radius` is 10 and
  `config/theme_palettes.json` matches.
- Full-page Settings with General, Appearance, Connection (read-only),
  Keyboard shortcuts, and About; theme preview cards with readable names;
  new saved preferences `start_page`, `density`, and `text_size`, each
  defaulting when absent (`schema_version` stays 1).
- The task Terminal tab and per-task Native terminal action are removed; the
  Overview's Linked terminal section keeps Focus, Unlink, Find terminals, and
  Link for reported candidates.

**Validation executed (Rust 1.92.0):**
- Native Windows 11, `CARGO_TARGET_DIR` under `%TEMP%`: `cargo fmt --all --
  --check` passed; `cargo clippy -p vibemux_frontend --all-targets
  --all-features -- -D warnings` passed; `cargo test -p vibemux_frontend
  --all-features` passed (lib 214, 1 ignored golden-fixture generator;
  `supervisor_ui` 10, `daemon_feed` 4, `palette_parity` 2, binaries 2 + 4 +
  6 + 6). Python `tests/test_theme.py`: 23 passed.
- WSL Ubuntu 24.04 at the same commit: the same fmt, clippy, and test
  commands passed with the same counts.
- Preview screenshots (`settings`, `appearance` in Claude Light and Claude
  Dark, `chat`, `drawer`, `task`) were captured at 1280×800 and checked by
  eye; they are local review artifacts, not committed.

**Not verified:** the console-less launch and startup log were exercised by
unit tests and a manual launch, not by an automated end-to-end Windows test;
other display scales, IME input, and screen readers were not tested.

### 2026-10-22 - vibemuxctl tasks, plugins, and terminal verbs, and a local install script

Status: Rust CLI stays `PARTIAL`. No new control operation, schema, or ADR: the
verbs are thin clients of operations the daemon already served to the console,
GUI, web, and MCP frontends.

- `vibemuxctl tasks list [--after] [--limit]` and `tasks show <task_id>` print the
  daemon's bounded read models (Control v4). An unknown task fails with
  `frontend_task_not_found`. Neither creates a task.
- `vibemuxctl plugins status` prints the plugin registry state (read-only).
- `vibemuxctl terminal inspect|link|focus|unlink` drive the existing
  observation-only terminal operations. Nothing attaches to a pane, sends input,
  or launches a harness. The terminal verbs need a daemon the operator started
  with `--plugin-config`, which `vibemuxctl daemon start` does not pass.
- `scripts/install_local_tools.ps1` copies the release binaries into one
  directory (default `%LOCALAPPDATA%\VibeMux\bin`) and, only with
  `-add_to_user_path`, appends it to the user PATH.
- The unified CLI guide maps each Python `vibemux` command to its Rust verb.

**Not implemented:** standalone task creation and `send` (they would need a new
control operation and an ADR, and pane input and harness launch into native
terminals remain premature), `diff`, and passing `--plugin-config` through
`daemon start`. A populated task page was not read through the CLI: the live
check below saw an empty store, so the JSON shape of a real task rests on the
daemon's existing read-model types.

**Validation executed (Rust 1.92.0, native Windows 11, `CARGO_TARGET_DIR` under `%TEMP%`):**

- `cargo +1.92.0 fmt --all -- --check`: passed.
- `cargo +1.92.0 clippy -p vibemux_cli --all-targets --all-features --locked --offline -- -D warnings`: passed.
- `cargo +1.92.0 test -p vibemux_cli --all-features --locked --offline`: passed.
  `vibemux_cli` lib 72 (59 before), `bootstrap_process` 4, `unified_commands` 8
  (7 before). The documented-command parse test covers the new CLAUDE.md,
  README, and guide lines.
- A real local daemon in a scratch Git project (debug binaries, no vendor CLI or
  paid API): `tasks list` and `plugins status` returned empty pages; `tasks show`
  and `terminal inspect` of unknown ids failed with `frontend_task_not_found` and
  `terminal_run_unavailable`; `terminal focus` of an unknown binding failed with
  `terminal_link_expired`; `terminal unlink` of an unknown binding succeeded
  because the daemon's unlink is idempotent; `tasks list --limit 0` exited 2.
  No terminal plugin was configured, so `link` and a populated `inspect` did not
  run.
- The install script ran with `-dry_run`, then into a scratch directory, and
  twice (idempotent); the installed `vibemuxctl` started its sibling `vibemuxd`
  and read `tasks list`. `-add_to_user_path` was exercised only as a dry run,
  and the `-build` path only as `cargo check --release --locked` with the same
  package and binary arguments, not as a full release build.
- Observed, not changed here: piping `vibemuxctl daemon start` into `cut` from
  Git Bash did not return until the daemon stopped, as if the daemon inherited
  the pipe handle.
- Two hosted Codex review findings were fixed after the first push, the first
  with a test that failed before the fix: the shared scanner took an option as
  a flag's value (`plugins status --project-root --unknown` named a project
  `--unknown`), and the install script resolved a relative `CARGO_TARGET_DIR`
  against the caller's directory instead of the repository root where it
  builds (reproduced from another directory before the fix).
