# Model supervisor and Windows launch repair

## Scope

- Task: continue the runtime enhancement after the DSH/dual-track merge and
  investigate the reported `node`/`ruff.exe` Windows application error.
- Base: `a639ed5befdf232af4f4aa0f2d7d4dc55cf3c146`; owned integration branch:
  `codex/remaining_runtime`. The original main checkout and `ui_check/` are preserved.
- Owned modules: pure supervisor contracts, writer migration/action accounting,
  daemon model effects, Control/CLI adapters, acceptance launch helpers and the
  trusted browser prerequisite fixture. No unrelated process owner is modified.

This record describes the 6 October 2026 increment. The earlier
[ADR 033 execution record](runtime_enhancement_increment.md) remains historical
evidence; its schema-6/Control-v7 and unfinished-supervisor descriptions are
superseded by this increment. The original 37-case benchmark is unchanged.
[Machine-readable results](model_supervisor_increment.json) retain executed
commands, source stages and failed/blocked/not-run distinctions.

## Implemented

- An explicit model driver observes committed facts and proposes bounded JSON
  actions through `ModelGateway`. Ordinary workflow start remains deterministic.
  Configuring an alias alone never changes the selected driver.
- The writer binds immutable plan/configuration/contract identity, model-call
  reservations, observed versions, rounds and claimed action intents. It validates
  the original observation and compensates only that round's own reservation.
- Model decisions drive actual admitted worker turns, independent reviews,
  exact-candidate verification and isolated integration. Task ordering respects
  dependencies; assignments use only eligible owned slots and existing grants.
  Worker timestamp evidence verifies concurrent execution of two assignments.
- Same-round replay is idempotent; conflicting or cross-round keys refuse.
  Malformed and policy-rejected responses share a persisted repair ceiling.
  Unknown submissions/effects keep their charges and fence another execution.
  Pause during unsettled inference reports recovery required; cancellation
  prevents a delayed model response from dispatching a worker.
- Schema 7 and Control v8 add supervisor start/status. DSH operations retain
  their v7 minimum and old operations retain their previous version requirements.
- Windows acceptance helpers refuse restricted or unknown launch-token state
  before native child creation. Initialization status remains actionable; bounded
  token queries close their owned handles. Noninteractive error-mode bits are
  process-scoped and preserve existing bits. Report validation remains read-only.
- A blank-page browser preflight checks required CDP domains, emulation and
  evaluation before candidate testing. Infrastructure refusal is recorded as
  blocked, while the original candidate checks run unchanged after a usable
  preflight. Browser sandbox flags and Job containment are preserved.

## Behavior and invariants

Only the daemon writer commits canonical state and audit events. Effects follow
durable intents and one claimant; unknown delivery is not replayed automatically.
Model output cannot grant permissions, change routes/contracts, issue shell text
or mark success. Applied actions require exact post-claim owned evidence rather
than arbitrary receipts from the same workflow. Review and verification remain
independent acceptance gates. Worktree isolation is not an OS sandbox.

No new controller/provider, authentication, context-window or compaction setting
is applied to the user's global configuration. Prompts and model rationale do not
enter the content-free supervisor ledger. Synthetic gateway output establishes
offline behavior, not upstream model/provider identity or live certification.

## Files changed

The new model effect modules are daemon `workflow/model_supervisor.rs` and
`workflow/supervisor_actions.rs`; persistence lives in
`vibemux_store/src/workflow/supervisor_commands.rs` and its responsibility-based
submodules. Pure types remain in `vibemux_workflow/src/supervisor.rs`. Existing
writer, service, run lifecycle, Control and CLI boundaries integrate these modules.

Behavioral regressions live in `workflow_supervisor.rs`, `model_call_ledger.rs`
and `workflow_model_supervisor.rs`. Windows handling lives in acceptance
`windows_launch_context.py`, process execution, version probes and their tests.
The trusted browser suite adds `browser_preflight.mjs` and its focused tests.
[ADR 034](../adr/034_model_supervisor_execution.md), architecture, PROGRESS and
[Windows launch documentation](../windows_tool_launch.md) describe compatibility
and verification limits.

## Validation executed

- `cargo test -p vibemux_store -p vibemux_workflow --all-features --no-fail-fast`:
  257 passed, zero failed, including migration and durable supervisor regressions.
- `cargo test -p vibemuxd --lib --test workflow_model_supervisor --all-features
  --no-fail-fast`: 116 passed, zero failed. This includes all 111 daemon unit tests
  and five model-driver effect/failure tests, with observed worker overlap.
- The complete workspace run before the last compatibility/preflight repairs:
  924 passed, 3 failed, 3 ignored. The stale current-Control-version assertion
  was repaired and the entire affected unit target rerun successfully.
- The two original `workflow_cooperate` tests were rerun after browser preflight:
  both still fail their required acceptance assertions. Their trusted verifier
  now records `browser_page_unavailable:page_enable`, blocked with zero candidate
  checks. This is a host containment prerequisite failure, not a passing workflow.
- `node --test tests/fixtures/taskboard_lite/verifier/browser_preflight.test.mjs`:
  five passed, zero failed. The trusted good frontend also passed all 16 original
  browser checks directly, outside the contained workflow path.
- `cargo check --workspace --all-targets`: passed with default features, including
  production paths without fixture capabilities.
- Python: 132 passed. Ruff formatting/lint and scoped mypy (32 source files)
  passed. Broad mypy has 20 pre-existing errors in unchanged optional tooling.
- Workspace formatting and all-target/all-feature Clippy with `-D warnings`
  passed on the final Rust source; exact commands are in the JSON receipt.

The complete workspace was not rerun after those focused repairs. Results from
different runs are not added to invent a single green total. Nextest, deny and
audit were unavailable and were not installed. The original acceptance runner
and validator were not executed for this increment; all 37 fresh scenario results
remain NOT_RUN. Fixture test pointers are coverage references, not scenario PASS.

The first browser rerun was interrupted without a terminal test summary. Its
counts are unknown. Owned process identities were reconciled before the replacement
run; only the replacement's terminal results above are used.

## Platform coverage

- Windows: native Rust writer/control/effect tests and paired token-context probes.
  Project Ruff `0.16.1` fails with `0xc0000142` under the restricted token and
  succeeds under normal host access. Current paired Node `24.18.0` probes succeed.
  The exact denied DLL/subsystem and historical Node failures remain unknown.
- Linux/WSL: not executed for this increment; existing platform code is preserved.
- Live terminal backend: not implemented or verified by these fixtures.
- Contained browser: Page CDP enable is unavailable on the tested Windows host.
  Contained cancellation has separate active coverage; an ignored lifecycle
  success test does not establish browser support.

## Compatibility / migration

Schema 6 migrates atomically to 7 by adding supervisor execution, round and action
tables. Historical workflow/dispatch/call/receipt bytes remain unchanged. Schema-5
migration fixtures also exercise forward migration through 7. Failed migration
rolls back its partial tables and marker. An older binary cannot open schema 7.
Rollback needs matching pre-upgrade database/private-state backups and binaries;
never recreate, downgrade or discard a live database.

New supervisor operations require Control v8; prior operations retain their
minimum versions. Older clients that refuse a newer descriptor need a matching
binary upgrade. Workflow status/export remain schema 2; supervisor status is a
bounded schema-1 view, not a complete event export.

## Security impact

The model is an untrusted proposer. Writer validation, ownership and grants remain
authoritative. Fixed dispatch identities and post-claim receipts prevent unrelated
effects from proving success. Lost output and claimed effects require reconciliation.
No browser sandbox bypass, Job breakaway, global Error Reporting change, credential
access or unrelated process termination was introduced.

The project preflight governs its acceptance CLI and owned tool helpers. A direct
native command launched by the coding host is outside that boundary and must use
its already-authorized normal Windows execution path. The project does not silently
switch tokens or elevate. See the [paired launch evidence](windows_launch_diagnosis.json).

## Remaining work

Full Version 2 acceptance remains BLOCKED. Production native Control/UI/backend,
affirmative approval delivery, standalone review mode, semantic prompt editing,
complete active-attempt/message/controller recovery joins and live qualification
are unfinished. Artifact/context/message/share/native-handoff supervisor tools
currently return a typed unavailable refusal. Deterministic comparison retains
its negative-control path; that path is unavailable to the model driver.

Live execution needs explicit route/harness/concurrency/time/call/cost/retention
policy, request-scoped AAG identity and installed-version grant certification.
No live model request, vendor coding turn or deployment was performed. The present
Windows browser limitation also prevents complete offline workflow acceptance.

## Known risks

This is an integration increment, not a release or a claim of full Version 2
completion. Main promotion, remote delivery and live operation are separate Git
and execution actions. Cleanup retains dirty or uncertain resources. The earlier
unregistered empty `remaining_optimizer` directory has an unidentified handle
owner; no unknown process is terminated to remove it.
