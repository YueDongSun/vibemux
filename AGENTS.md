# AGENTS.md

## 1. Scope and Authority

This file defines repository-wide engineering rules for human contributors and coding agents.

- These rules apply to every file in the repository unless a more specific nested `AGENTS.md` explicitly narrows a rule for its subtree.
- A nested file may add constraints but may not weaken security, state-integrity, compatibility, or release gates defined here.
- `PROGRESS.md` is the source of truth for implementation status and milestone sequencing.
- Architecture decisions belong in ADRs. Chat history, issue comments, and agent memory are not authoritative architecture records.
- Personal learning plans, tutorial prompts, and private workflow instructions do not belong in this file.

Normative terms `MUST`, `MUST NOT`, `SHOULD`, and `MAY` are used deliberately.

---

## 2. Product Invariants

Contributors and agents **MUST** preserve all of the following:

1. VibeMux is Windows-first. WSL2 is a development and optional execution environment, not an end-user prerequisite.
2. A writable Run owns an independent Git worktree and branch.
3. Git worktree isolation is not represented as a security sandbox.
4. Terminal presentation, process execution, harness control, sandboxing, and A2A communication are separate abstractions.
5. Terminal text, pane exit, process exit, and idle state do not prove task completion.
6. The canonical Task/Run/Event model is independent of WezTerm, tmux, ACP, A2A, MCP, and any vendor payload.
7. State transitions and their audit events are committed atomically.
8. Events are append-only.
9. The core database has exactly one authoritative writer.
10. Cleanup is plan-first, fail-closed, and dry-run by default.
11. User messages are data. They are never interpolated into shell command strings.
12. VibeMux does not automatically stash, reset, clean, commit, merge, force-push, or push agent branches.
13. Unknown, dirty, mismatched, or externally owned resources are not deleted or terminated.
14. Secrets and full prompts are not stored in default logs or events.
15. A plugin failure may degrade one capability but must not corrupt or crash the core.
16. A2A wire objects do not directly become internal state without validation and explicit mapping.
17. Optional integrations do not gain direct database access.
18. Claims in documentation must match implementation and verification evidence.

Any change that violates an invariant requires an explicit architecture proposal and is presumed rejected until accepted.

---

## 3. Target Architecture

### 3.1 Rust core

The authoritative orchestration and A2A path is implemented in Rust.

Core responsibilities:

- typed IDs and canonical domain objects;
- Task, Run, Review, Artifact, and Event state machines;
- event sequencing, idempotency, projections, and replay;
- SQLite schema, migrations, transactions, and single-writer enforcement;
- bounded async routing, backpressure, deadlines, and cancellation;
- plugin discovery, launch, capability negotiation, health, and shutdown;
- policy enforcement;
- worktree safety invariants and resource ownership;
- A2A Agent Card, task binding, protocol mapping, streaming, and transport selection;
- local daemon lifecycle and authenticated local IPC;
- reconciliation and audit;
- stable CLI-facing control API.

### 3.2 Out-of-process plugins

Vendor-specific and optional capabilities run out of process.

Initial plugin kinds:

- `harness`
- `terminal`
- `sandbox`
- `integration`
- `ui`
- `reporter`
- optional `policy-extension`

The core remains authoritative when a policy extension is present.

Plugins may implement:

- OpenCode, Claude, Copilot, Pi, Grok, and Gemini adapters;
- WezTerm and tmux control;
- Docker, Podman, VM, or future sandbox backends;
- GitHub, notification, editor, and observability integrations;
- optional user interfaces;
- benchmark export.

Plugins **MUST NOT**:

- open or mutate the core SQLite database;
- invent canonical sequence numbers;
- perform unrecorded state transitions;
- bypass permission checks;
- terminate resources that the core has not assigned to them;
- assume their process lifetime equals the lifetime of a Task;
- treat terminal output as a structured completion event unless the adapter contract explicitly defines and tests that event.

### 3.3 No public in-process native ABI

The project **MUST NOT** expose a public Rust/C dynamic-library plugin ABI in the first stable plugin API.

Reasons:

- Rust ABI is not stable across compiler versions;
- native plugins can crash or corrupt the host;
- dependency and allocator coupling makes open-source compatibility difficult;
- permission isolation is weak.

Use an out-of-process, versioned protocol. WASM components may later be considered for deterministic policy or transformation plugins, but they are not a substitute for OS-accessing harness and terminal plugins.

---

## 4. Repository and Migration Rules

### 4.1 Python prototype

The current Python implementation is a temporary behavior reference.

- Correctness and safety defects **MAY** be fixed.
- New orchestration, A2A routing, event-broker, scheduler, or public plugin features **MUST** target Rust.
- Python **MUST NOT** remain a concurrent authoritative state writer after the Rust daemon becomes default.
- Python may remain as:
  - a plugin SDK;
  - compatibility tooling;
  - test fixtures;
  - migration utilities;
  - optional integrations.

Do not delete the Python prototype until Rust parity tests and a state migration path exist.

### 4.2 Expected Rust workspace

New Rust code should follow the responsibility split recorded in `PROGRESS.md`. Do not create circular dependencies or a single "god crate."

Dependency direction:

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

Platform and transport crates may depend on domain interfaces; domain crates must not depend on platform, database, terminal, or network implementations.

### 4.3 One writer

Only `vibemuxd` may perform authoritative state transitions after daemon cutover.

- CLI, plugins, and UI clients send requests.
- They do not write state tables directly.
- Read-only diagnostic database access is also discouraged; use the control API or exported support bundle.
- A migration tool may write the database only while the daemon is stopped and an exclusive migration lock is held.

---

## 5. Mandatory Agent Workflow

Before editing:

1. Read this file.
2. Read `PROGRESS.md`.
3. Read relevant ADRs and public protocol/schema files.
4. Run `git status --short`.
5. Confirm the current branch and worktree.
6. Inspect nearby tests before changing behavior.
7. Identify whether the task changes:
   - state semantics;
   - persistence schema;
   - plugin protocol;
   - A2A mapping;
   - platform behavior;
   - security boundary;
   - public CLI/API.
8. State the intended file ownership for the task.
9. Do not overwrite unrelated user or agent changes.

During editing:

- Keep one issue or narrowly defined task per branch/worktree.
- Prefer small, reviewable commits.
- Do not mix broad refactoring with behavioral change unless the issue explicitly requires both.
- Preserve a runnable repository.
- Add tests with the behavior, not in a later unspecified step.
- Update schemas, fixtures, docs, and compatibility notes in the same change.
- Do not silently weaken a test to make a change pass.
- Do not replace an implementation with a stub unless the removal is explicit and documented.
- Do not add broad abstractions without at least two concrete consumers or an accepted ADR.

Before handoff:

1. Run all required checks for the touched area.
2. Record exact commands and outcomes.
3. Review the diff for secrets, generated noise, and unrelated edits.
4. Update `PROGRESS.md` if implementation status changed.
5. Update an ADR if an architecture decision changed.
6. Report unverified platform paths explicitly.
7. Leave the worktree clean or clearly describe remaining changes.
8. Never say "tests pass" when they were not executed.

---

## 6. Multi-Agent / Vibecoding Coordination

When several agents work in parallel:

- Each agent **MUST** use a separate Git worktree and branch.
- The task assignment **MUST** identify owned files or modules.
- Two agents **MUST NOT** concurrently edit:
  - the same migration;
  - the same Protobuf/WIT schema;
  - the same public state machine;
  - the same lock file;
  - the same release manifest.
- Schema changes require a single designated owner.
- Generated code is committed by the schema owner only.
- Review agents do not rewrite the implementation branch silently; they produce findings or a separate patch.
- Integration occurs through commits, diffs, test reports, and artifacts, not shared uncommitted files.
- A failed implementation attempt remains auditable; do not rewrite its history to look successful.
- Repair work should be a new commit or Run, not mutation of recorded evidence.
- Use deterministic task IDs or issue references in branch and commit metadata where practical.
- Conflicts are resolved by the designated integrator, not by whichever agent finishes last.

Recommended branch format:

```text
feat/<issue>-<short-name>
fix/<issue>-<short-name>
refactor/<issue>-<short-name>
docs/<issue>-<short-name>
```

---

## 7. Rust Engineering Rules

### 7.1 Toolchain

- Rust edition: 2024.
- MSRV: declared in the workspace and tested in CI; initially align it with the minimum required by the selected official A2A Rust SDK.
- `rust-toolchain.toml` and `Cargo.lock` **MUST** be committed for reproducible application builds.
- Libraries use semantic versioning; internal crates may remain unpublished.

### 7.2 Safety

- Library crates **SHOULD** use `#![forbid(unsafe_code)]`.
- `unsafe` is permitted only in a narrowly scoped platform module when no safe alternative is practical.
- Every `unsafe` block requires:
  - an accepted ADR;
  - a `// SAFETY:` explanation;
  - focused tests;
  - platform CI;
  - reviewer approval.
- No untrusted input may trigger a panic.
- Avoid `unwrap()` and `expect()` in production paths. If an invariant makes one unavoidable, document the invariant and test it.
- Use checked conversions for sizes, sequence numbers, timestamps, and protocol fields.

### 7.3 Errors

- Library crates use typed errors, normally with `thiserror`.
- Binary entry points may use `anyhow` for top-level context.
- External errors are translated at crate boundaries.
- Public errors have stable machine-readable codes.
- Error text must not contain secrets, bearer tokens, full prompts, or raw environment dumps.
- Plugin and A2A errors preserve correlation IDs.

### 7.4 Async and concurrency

- Tokio is the default async runtime.
- All channels are bounded.
- Queue capacity and overflow behavior are explicit.
- Use cancellation tokens and deadlines.
- Never hold a mutex across `.await`.
- Blocking Git, filesystem, process, compression, or SQLite operations run through a blocking boundary or dedicated worker.
- Do not spawn detached tasks without ownership and shutdown behavior.
- Every long-lived task has:
  - an owner;
  - cancellation;
  - structured error reporting;
  - a join or supervision path.
- Backpressure must propagate rather than silently accumulating memory.
- Ordering guarantees are documented per stream.

### 7.5 Domain types

- Use newtypes for `ProjectId`, `TaskId`, `RunId`, `EventId`, `PluginId`, and `ArtifactId`.
- IDs are opaque outside their defining crate.
- Store wall-clock timestamps in UTC/RFC 3339.
- Use monotonic time for durations, timeouts, and heartbeat decisions.
- State transitions are pure and deterministic.
- External SDK types must not appear in the core domain public API.
- Enums received from external protocols must tolerate future unknown values where the protocol permits extension.

### 7.6 Serialization

- Canonical persisted/event JSON must have explicit schema/version rules.
- Plugin wire messages use generated Protobuf types.
- Do not use Rust-specific binary serialization as a public cross-language format.
- Preserve unknown A2A extension metadata when safely possible.
- Enforce maximum frame, field, file, and artifact-reference sizes before allocation.
- Parsing untrusted messages must be fuzzed.

### 7.7 Observability

- Use `tracing` with structured fields.
- Include correlation, task, run, plugin, and transport identifiers.
- Do not log full prompt/message content by default.
- Redact authorization headers, cookies, tokens, environment secrets, and signed URLs.
- High-cardinality payloads and file content do not belong in normal logs.
- Audit events and diagnostic logs are separate concepts.

---

## 8. Python and Other Plugin Languages

Python is supported for plugins and tooling, not as a second core.

Python requirements:

- Python 3.12+.
- Full type checking for public plugin SDK code.
- `ruff`, `mypy`, and `pytest`.
- No shell-interpolated user input.
- No direct SQLite core writes.
- Async plugins must implement cancellation and shutdown.
- stdout is reserved for the plugin protocol; diagnostics use stderr.
- Dependencies are isolated per plugin.
- Plugin manifests declare entry point, API version, capabilities, platforms, and permissions.

Go, TypeScript, C#, or other plugin SDKs may be added after the wire contract is stable. Do not maintain language SDKs that have no contract-test coverage.

---

## 9. Plugin Protocol Rules

### 9.1 Framing and lifecycle

Every plugin session follows:

```text
spawn
  → Hello
  → CoreHello / negotiated version
  → capability registration
  → Ready
  → request/event/stream traffic
  → Drain
  → Shutdown
  → exit
```

Protocol requirements:

- length-delimited frames;
- explicit protocol version;
- request ID and correlation ID;
- typed request, response, event, and error envelopes;
- bounded frame size;
- heartbeat/health contract;
- deadline and cancellation fields;
- graceful drain;
- structured incompatibility error;
- feature/capability negotiation;
- deterministic handling of duplicate requests;
- no semantic dependence on message arrival timing beyond documented ordering.

### 9.2 Permissions

A plugin manifest declares requested permissions, including:

- executable launch;
- worktree read/write;
- network access;
- terminal control;
- Git read/write;
- secret access;
- external endpoint access;
- artifact access.

The core validates and records the granted subset. Absence of a permission means denial.

### 9.3 Failure behavior

- Malformed frames fail the plugin session, not the core.
- Excessive output is rate-limited or terminated.
- Missed heartbeats mark the plugin unhealthy.
- Core requests are retried only when idempotency semantics allow it.
- Restart policy is bounded and observable.
- A plugin crash never marks a Task successful.
- Partial side effects require reconciliation and audit events.

### 9.4 Compatibility

- Plugin API versions use explicit major/minor semantics.
- Breaking schema changes require a new major protocol version.
- Additive fields require default behavior and compatibility tests.
- Generated fixtures from previous supported versions stay in the repository.
- Core supports only a documented compatibility window.
- Public compatibility is not promised before the milestone recorded in `PROGRESS.md`.

---

## 10. A2A Engineering Rules

### 10.1 SDK isolation

Use the official A2A Rust SDK through a dedicated adapter crate.

- SDK types do not leak into core domain crates.
- SDK upgrades occur in one boundary crate.
- Pin or constrain versions deliberately.
- Record protocol specification compatibility separately from crate version.
- Add fixtures for every supported transport and protocol version.
- Do not fork or copy protocol types unless an accepted issue demonstrates that the SDK cannot meet a required contract.

### 10.2 Task mapping

Internal VibeMux tasks and external A2A tasks are distinct.

The mapping must record:

- local project/task/run IDs;
- peer identity;
- external task ID;
- transport/binding;
- protocol version;
- creation and update times;
- idempotency key;
- last accepted remote state;
- cancellation state;
- artifact bindings.

A remote state cannot bypass local transition validation.

### 10.3 Transport policy

- Advertise only transports that pass conformance tests.
- Prefer gRPC for a mutually supported VibeMux fast path.
- Retain JSON-RPC and HTTP+JSON/REST for interoperability.
- Transport choice must come from Agent Card capabilities and local policy.
- Canonical behavior must be transport-independent.
- Streaming uses bounded buffers.
- Disconnect, retry, resubscription, and cancellation are explicit states.
- Push endpoints and Agent Card URLs are untrusted inputs.

### 10.4 Security

- Validate scheme, host, port, redirects, and resolved addresses.
- Apply SSRF controls before fetching Agent Cards or push endpoints.
- Remote bind is disabled by default or requires explicit configuration.
- Non-loopback servers require authentication and TLS policy.
- Authorization is checked per task and artifact.
- Signed Agent Cards are verified when required by policy.
- Do not forward local secrets, environment variables, repository files, or prompts unless the task explicitly authorizes the data.
- Record peer identity and authorization decisions in audit events.
- Enforce body, frame, stream, and artifact limits.

### 10.5 Conformance

A2A changes require:

- unit tests for mapping;
- transport integration tests;
- official TCK for every advertised binding;
- cross-language ITK scenarios;
- Agent Card validation;
- cancellation, resubscription, push, malformed payload, and timeout tests.

No endpoint is documented as supported before these gates pass.

---

## 11. Persistence and Event Rules

### 11.1 SQLite

- Use schema migrations; never recreate a user database to "fix" a mismatch.
- Enable foreign keys.
- Configure WAL and busy timeout deliberately.
- Use explicit `INSERT` and `UPDATE`; avoid `INSERT OR REPLACE` for state entities.
- State and event changes use one transaction.
- Migration is forward-only by default; rollback policy must be documented.
- Database paths are project-owned and normalized.
- The daemon holds the authoritative writer lock.

### 11.2 Event model

Events are immutable.

Required envelope fields:

- event ID;
- sequence;
- event type;
- schema version;
- project/task/run correlation;
- causation ID;
- actor;
- timestamp;
- idempotency key where applicable;
- sanitized payload.

Rules:

- Sequence allocation is owned by the store.
- Duplicate idempotency keys do not create duplicate semantic effects.
- Projections can be rebuilt from events or verified against them.
- Event names are backend-neutral.
- Terminal and plugin details live in payloads, not core event names.
- Prompt/message content is hashed and sized by default, not stored verbatim.
- Migration tests must cover historical event fixtures.

---

## 12. Process, Terminal, and Windows Rules

### 12.1 Command execution

- `shell=false` is mandatory.
- Commands are represented as executable plus argv.
- User text is passed through stdin or a dedicated protocol field.
- `.cmd`/`.bat` launchers require a narrow, tested Windows shim.
- User message, task title, description, role, file content, and remote payload must never enter that shim's command string.
- Environment inheritance is allowlisted or explicitly documented.
- Process groups/job objects are used for reliable cancellation.
- Child processes must be supervised and reaped.

### 12.2 Terminal ownership

A terminal resource record includes:

- backend;
- project workspace/session;
- task tab/window;
- run pane;
- backend resource IDs;
- ownership token or metadata;
- expected cwd;
- creation event.

Before send, activate, interrupt, or kill:

1. confirm the Run belongs to the active project;
2. confirm backend identity;
3. query current inventory;
4. confirm pane/resource ownership;
5. fail closed on mismatch.

Do not kill a pane merely because its numeric/string ID exists.

### 12.3 Path safety

Windows is a release platform.

Path checks must cover:

- case-insensitive comparison;
- drive letters;
- different-drive `commonpath` failure;
- spaces and Unicode;
- trailing separators;
- long paths within supported limits;
- symlinks;
- junctions and reparse points;
- UNC paths;
- missing paths;
- prefix traps such as `worktrees` versus `worktrees-evil`.

Never use string `startswith` as the sole containment test.

### 12.4 Worktrees

- Record `base_commit` at Run creation.
- Branch and path are unique per Run.
- Diff includes committed, staged, unstaged, and untracked information relative to `base_commit`.
- Cleanup verifies database ownership, Git worktree inventory, branch, path, Run state, terminal state, and cleanliness.
- Use `git worktree remove`; do not raw-delete registered worktrees.
- Do not delete the branch automatically.
- Do not prune unknown worktrees.
- Main worktree deletion is impossible by construction and tested.

---

## 13. Security Rules

The following are forbidden:

- secrets in repository files, fixtures, events, or normal logs;
- printing full environments;
- unbounded remote payloads;
- unauthenticated non-loopback control endpoints;
- direct plugin database access;
- arbitrary shell execution from protocol data;
- automatic trust of Agent Cards or remote artifact URLs;
- following redirects without policy;
- reading user SSH keys or credential stores without an explicit feature and permission;
- mounting the user home directory into a sandbox by default;
- auto-approving harness tool permissions;
- force cleanup to make a test pass;
- disabling a safety test without a linked security rationale.

Security-sensitive changes require a threat-model note and tests for failure paths.

---

## 14. Testing Requirements

### 14.1 Test layers

- **Unit:** state transitions, IDs, parsing, validation, mappings.
- **Property:** state-machine invariants, idempotency, ordering.
- **Contract:** plugin SDKs, terminal plugins, harness adapters.
- **Integration:** daemon, store, plugins, worktrees, process lifecycle.
- **Platform:** native Windows and Linux/WSL behaviors.
- **Conformance:** official A2A TCK/ITK.
- **Security:** path escape, command injection, SSRF, oversized frames, permission denial.
- **Fuzz:** plugin framing, A2A decoding, manifest parsing, event payloads.
- **Soak:** long-lived streams, plugin restarts, database growth, cancellation.
- **Benchmark:** routing, persistence, plugin IPC, startup, memory.

### 14.2 Test integrity

- Tests must not use the user's normal tmux/WezTerm namespace.
- Tests must not modify global Git configuration.
- Tests create local Git identities only in temporary repositories.
- Tests must clean only resources they created.
- Integration tests must verify main-repository integrity after cleanup.
- Platform-specific tests skip with an explicit reason; a skip is not a pass for release claims.
- Flaky tests are bugs. Do not add blind retries without diagnosing the cause.
- Golden fixtures are reviewed and versioned.
- A test that only asserts command construction does not prove a live backend works.

### 14.3 Required CI

Rust:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo nextest run --workspace --all-features
cargo deny check
cargo audit
```

Python/plugin tooling:

```text
python -m ruff format --check .
python -m ruff check .
python -m mypy .
python -m pytest
```

CI must build packages and run on Windows and Ubuntu. Release gates additionally require documented live Windows/WezTerm validation.

---

## 15. Performance Rules

- Treat A2A and plugin routing as latency-sensitive control/data paths.
- Use bounded queues and explicit backpressure.
- Do not optimize terminal UI code before measuring core routing and persistence.
- Benchmarks exclude model inference time unless explicitly labeled end-to-end.
- Every hot-path optimization includes before/after benchmark data.
- Do not add unsafe code, custom allocators, zero-copy lifetime complexity, or bespoke codecs without profiling evidence.
- Batch durable event writes only when ordering and durability semantics remain explicit.
- Do not hold database transactions across network or plugin awaits.
- Avoid global locks.
- Keep per-stream state isolated where practical.
- Record benchmark platform, CPU, OS, build profile, payload, concurrency, and persistence configuration.
- Performance targets live in `PROGRESS.md`; documentation must not present targets as achieved measurements.

---

## 16. Dependency Rules

Before adding a dependency:

1. Explain the capability it provides.
2. Check maintenance and license compatibility.
3. Prefer official protocol SDKs and mature ecosystem crates.
4. Avoid overlapping libraries for the same role.
5. Disable unnecessary default features.
6. Assess binary size, compile time, transitive dependencies, and security history.
7. Add the dependency to allow/deny policy where applicable.
8. Do not introduce a framework solely to avoid writing a small, well-tested adapter.

Protocol and cryptography dependencies require extra review.

---

## 17. Public API, Schema, and Compatibility Changes

A change requires an ADR and migration/compatibility plan when it modifies:

- canonical domain states;
- event schema;
- SQLite schema;
- plugin protocol;
- plugin manifest;
- A2A mapping;
- CLI machine-readable output;
- local IPC;
- artifact format;
- authentication or permission semantics;
- platform support guarantees.

Required artifacts:

- old/new examples;
- migration;
- compatibility tests;
- version bump;
- changelog;
- `PROGRESS.md` update;
- deprecation period where the project has already promised compatibility.

Do not promise compatibility before the corresponding milestone declares the interface public.

---

## 18. Git and Review Rules

Forbidden Git actions unless the user explicitly requests them for a known safe resource:

- `git reset --hard`
- `git clean -fd` / `-fdx`
- `git checkout -- .`
- `git restore .`
- force push
- deleting unknown branches
- rewriting published history
- deleting worktrees outside a validated cleanup plan

Commit rules:

- Conventional Commit style.
- One coherent concern per commit.
- Generated files and source schema are committed together.
- No secrets, local databases, `.vibemux/`, virtual environments, caches, logs, or benchmark dumps.
- Run `git diff --check`.
- Review staged content before commit.
- Do not claim GitHub push, CI, or release success without verification.

PR review priority:

1. correctness and state integrity;
2. security and resource ownership;
3. compatibility and migration;
4. cancellation and failure behavior;
5. Windows behavior;
6. tests and observability;
7. performance;
8. style.

---

## 19. Documentation Rules

- `PROGRESS.md` records status, milestones, evidence, and risks.
- `AGENTS.md` records engineering constraints.
- ADRs record accepted architecture decisions.
- README records user-facing behavior only.
- Protocol docs record wire contracts.
- Security docs record threat model and reporting.
- Do not copy private prompts or personal learning instructions into public project files.
- Do not describe planned code as implemented.
- Commands in documentation must match the current CLI.
- Platform claims name the exact validation environment.
- Superseded ADRs remain in history and point to the replacing ADR.

---

## 20. Definition of Done

A task is complete only when all applicable items are true:

- acceptance criteria are met;
- implementation is not a stub;
- state and failure semantics are explicit;
- tests cover success and failure paths;
- platform impact is tested or explicitly unverified;
- security boundaries are preserved;
- compatibility/migration is handled;
- observability is sufficient to diagnose failure;
- docs and examples match behavior;
- `PROGRESS.md` reflects any status change;
- required checks were actually executed;
- the diff contains no unrelated edits;
- no unknown resource or dirty worktree is left behind;
- handoff lists residual risks accurately.

---

## 21. Required Handoff Format

Every coding-agent handoff must include:

```markdown
## Scope
- issue/task:
- owned modules:

## Implemented
- ...

## Behavior and invariants
- ...

## Files changed
- ...

## Validation executed
- command:
- result:

## Platform coverage
- Windows:
- Linux/WSL:
- live terminal backend:

## Compatibility / migration
- ...

## Security impact
- ...

## Remaining work
- ...

## Known risks
- ...
```

Do not include hidden chain-of-thought. Provide decisions, evidence, and reproducible results.
