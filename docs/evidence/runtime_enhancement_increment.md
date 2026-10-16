# Runtime enhancement increment (ADR 036)

> Numbering: this record was written on `codex/remaining_runtime` before it was merged with the decision port. Its store schemas 6 to 8 are now 7 to 9, its Control v8 and v9 are now v9 and v10, and its ADRs 033 to 036 are now ADRs 036 to 039. Measured results below keep the original wording.

This is the historical ADR 036 execution record. The subsequent
[model supervisor and Windows launch increment](model_supervisor_increment.md)
updates current implementation, schema, Control version and test evidence.

## Scope

- Task: the next offline implementation increment after the DSH/dual-track merge.
- Base HEAD: `a639ed5befdf232af4f4aa0f2d7d4dc55cf3c146`.
- Branch: `codex/remaining_runtime`; the enhancement is uncommitted in its owned worktree.
- Owned modules: CLI bootstrap, harness grant/fixture profiles, pure workflow contracts,
  writer persistence, daemon model/controller effects, acceptance mapping and tests.

The original 37-scenario benchmark is unchanged. Historical acceptance counts
belong to their recorded revisions. The accompanying
[machine-readable evidence](runtime_enhancement_increment.json) separates source
observations, executed checks, and unexecuted scenario acceptance.

## Implemented

- Five independent launch grants derived from each narrowed contract and turn purpose.
  Fixture launch authority is an opaque startup guard; vendor profiles that cannot
  enforce the grants refuse. Dispatch attempts pin their route and execution authority.
- Bounded loopback AAG HTTP/SSE transport, owned fake-server permits, durable call
  reservations/submission/terminal records and an empty-TaskSpec candidate compiler.
  Its output enters existing source, path, policy and contract admission.
- Compiler allowance association during preparation, without rewriting prior call
  identity or resetting the allowance on duplicate preparation. Oversized or internally
  inconsistent completed results are persisted as rejected and cannot be replayed.
- Fenced native-controller reducers and an owned-adapter runtime with single-claim
  literal input, unknown delivery, human ownership, interruption and terminal handoff.
- Schema-6 authority, call ledger, controller/outbox, plan binding and checkpoint
  persistence. Identical immutable binding replay remains available after cancellation.
- Source/operator provenance for owned paths, persisted derived negative-control
  verifier receipts, and refusal of optimizer parameters without operational effects.
- Direct Windows daemon bootstrap with typed arguments and an owned Child handle.

## Behavior and invariants

Only the daemon writer mutates canonical state. Submission intent and accounting
precede external effects. Unknown delivery is never blindly replayed. Fixture
authority cannot be supplied through a JSON label. Reviewer routes must match their
persisted dispatch identity. Derived negative-control subjects cannot be promoted.
Read/search omission remains omission on answer and review turns. Worktree isolation
is not an OS sandbox. No workflow result follows from pane text or process exit alone.

## Files changed

The implementation is grouped in `vibemux_workflow/{execution_authority,model_call,
native_controller,path_authority,checkpoint,optimizer,gates}`, the store's
`workflow/{model_calls,recovery_store}` plus migration/transaction entry points,
daemon `workflow/{aag_gateway,model_compiler,model_runtime,native_runtime}` and
existing admission/runtime services. Harness fixture/grant and CLI bootstrap
modules have focused tests. Architecture, ADR 036, PROGRESS and the acceptance
catalog describe these boundaries.

## Validation executed

Exact commands, exit codes, test counts and final-state reruns are in the JSON
evidence. Tests use owned databases/processes/worktrees and synthetic HTTP/vendor
fixtures. A test reference is a coverage pointer, not a scenario PASS.

- Final formatting and workspace Clippy with `-D warnings`: passed.
- Final `vibemux_store` + `vibemux_workflow` tests: 242 passed, zero failed,
  including 144 pure workflow tests and 13 model-call ledger tests.
- Final eight daemon runtime targets: 26 passed, zero failed. These include the
  gateway/compiler, narrowed grants, native adapter, optimizer and share/export paths.
- Final Python tests: 114 passed. Ruff formatting/lint and package/acceptance-runner
  type checking passed (30 source files).
- The broader Rust run before the final repairs recorded 890 passed, 8 failed and
  3 ignored. Focused reruns cleared six failures. Two cooperative TaskBoard tests
  still have unresolved `dispatch_not_completed` failures; the complete workspace
  was not rerun after the final repairs. This is not OFFLINE_PASS.
- Broad `mypy .` recorded 20 errors in unchanged integration tooling/tests;
  optional dependencies/stubs are absent. Nextest, deny and audit are unavailable
  on the checked PATH and were not installed.

The inherited offline acceptance runner and its report validator were not executed
for this increment. All 37 fresh end-to-end scenario statuses remain NOT_RUN; the
catalog now points G01/G03 and T02/T03 fixture classes to their applicable tests,
without converting those references into scenario results.

## Platform coverage

- Windows: native Rust checks, writer/control/transport fixtures and CLI bootstrap.
- Linux/WSL: execution unverified; platform-specific source retains its existing split.
- Live terminal backend: unverified. The controller tests use an owned adapter fixture.
- Contained browser: renderer initialization has previously emitted sandbox/access
  denial markers; direct browser lifecycle and contained cancellation are distinct
  observations. No sandbox bypass or Job breakaway is introduced.

## Compatibility / migration

Schema 5 migrates forward to 6, adding tables without rewriting historical receipts.
Attempt v1 and optional receipt fields remain readable; missing legacy authority
cannot authorize new execution. Workflow status/export is schema 2; Control remains
v7 and workflow operations retain their v6 minimum. Unsupported optimizer policies
remain readable as history but cannot be evaluated or promoted.

Before upgrading a real project, stop its daemon and back up the database together
with matching private workflow state. Rollback requires those pre-upgrade backups
and matching binaries; an older source checkout cannot downgrade schema 6. Never
delete or recreate state to bypass compatibility checks.

## Security impact

User text stays in typed protocol fields/stdin. Credentials are redacted and never
loaded from an ambient credential store. Transport rejects nonnumeric/nonloopback
origins, redirects and ambient proxies. Real AAG routing needs request-scoped
attestation; local health fingerprints are observations. Global context and
compaction configuration are not written by this increment.

## Remaining work

Full Version 2 acceptance is BLOCKED. Required future implementation includes a
model supervisor loop, production native Control/UI operations and a certified
owned terminal backend, durable affirmative approval delivery, standalone review
mode, semantic prompt editing and complete active-attempt/message recovery joins.
H01/H02 are partial: checkpoint projection refuses missing joins and unknown context
identity cannot establish the one-million-token preservation regression.

Live execution additionally requires an explicit gateway/route/harness policy with
concurrency, time/call/cost ceilings and retention choice; request-scoped AAG identity;
and installed-version certification of two writable harnesses. No such inference,
deployment or publication was performed. Fixture results do not establish these gates.

## Known risks

The source is an uncommitted integration increment, not a release or a new main HEAD.
Captured outer-stdio daemon survival remains a separate bootstrap limitation.
Windows symlink-denial coverage depends on host privilege. Broad tooling/check
failures and ignored tests are reported explicitly in the evidence, without turning
them into passes.

All five temporary worker worktrees were unregistered after integration. Test
caches, raw logs, patch copies and pytest scratch were removed. One unregistered,
empty `remaining_optimizer` directory remains held open by an unidentified process;
no unknown process was terminated. The owned integration source is retained for
review. Local `main` remains at the base HEAD with the pre-existing `ui_check/` intact.

## Walkthrough

From this integration worktree and its supported native Rust environment:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features --no-fail-fast
$env:PYTHONPATH = Join-Path $PWD 'src'
& 'I:\VibeMux\.venv\Scripts\python.exe' scripts/verify_dual_track.py --mode offline --output <new_directory_outside_this_worktree>
& 'I:\VibeMux\.venv\Scripts\python.exe' scripts/verify_dual_track.py --validate <output_directory>/dual_track_report.json
```

Use the existing project Python interpreter and explicitly bind `PYTHONPATH` to this
worktree's `src`. Keep temporary roots short on Windows. The offline runner and its
validator are not implied to have run by these example commands. Live mode remains
gated; do not invoke it without the required explicit execution policy.
