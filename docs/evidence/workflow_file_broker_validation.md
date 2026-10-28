# Workflow file-broker integration validation

Date: 2026-10-27. Windows; Cargo/Rust 1.92.0; Node 24.18.0 for trusted fixture
verification. Source is the uncommitted task diff over VibeMux branch
codex/rd_execution_owner_20261026, base 45308fc. No commit, push or deployment
was performed by this increment.

## Reproduced failures and corrections

The first real workflow run passed one negative-output test and failed both
positive workflows. A content-free dispatch diagnostic showed a normal zero
process exit and protocol completion followed by harness_broker_candidate_invalid.
The owner had decoded Candidate directly, while the actual broker publishes a
strict status/candidate envelope. Typed envelope decoding fixed the mismatch.
The early negative test alone did not establish discrimination; positive and
negative workflows passed together after the correction, with exact refusal-code
assertions added.

Independent source reviews found and closed recovery and cancellation gaps:
private output deleted before durable finish; prepared input deleted on an
unpolled/unknown claim; create-only import capable of replacement; a detached
blocking importer outliving its registered task; lease forwarding stalled during
inline import; and interruption checks preceding slow staging or final input
reads. Current code retains preparation through confirmed completion, uses
no-clobber creation, joins inline import and checks both signals immediately before
publication. Existing-file content is rechecked after staging.

Strict Clippy initially reported three needless borrows in the extracted
publication helper. They were corrected without allow attributes or weaker gates.
The validator worker's first locked build refused because its copied dependency
manifest was incomplete; the matching manifest was supplied and the unchanged
locked/offline tests passed. No dependency download or lock relaxation occurred.

## Commands and observed results

All Cargo commands used the existing explicit runtime, an external task build
target, and the existing task temporary directory. Commands below run at the
VibeMux repository root.

| Command | Observed result | What it establishes |
| --- | --- | --- |
| cargo test --locked --offline -p vibemux_worker_tools --test owner_admission --test grants | 23 passed: 13 admission and 10 grant tests | Pure identity/scope/grant/receipt validation and existing broker behavior |
| cargo test --locked --offline -p vibemux_harness --features test_helpers --lib dispatch::fixture_guard:: -- --test-threads=1 | 7 passed | Existing fixture compatibility, broker image pinning and command-grant refusal |
| cargo test --locked --offline -p vibemuxd --features test_helpers --lib harness_dispatch:: -- --test-threads=1 | 37 passed | Owner retention/import checks, direct lease-loss observation, held inline-import join, and existing dispatch units |
| cargo test --locked --offline -p vibemuxd --features test_helpers --test workflow_file_broker -- --test-threads=1 | 5 passed | Actual broker subprocesses through daemon candidate, review and trusted verifier paths |
| cargo test --locked --offline -p vibemuxd --features test_helpers --test harness_dispatch --test workflow_lease_loss -- --test-threads=1 | 26 dispatch and 2 lease-loss tests passed in the combined related run | Process lifecycle, cancellation and lease compatibility |
| cargo test --locked --offline -p vibemuxd --features test_helpers --test workflow_gates one_bounded_repair -- --test-threads=1 | 1 passed | Legacy direct fixture repair remains functional |
| cargo clippy --locked --offline -p vibemuxd -p vibemux_harness -p vibemux_store -p vibemux_worker_tools --features vibemuxd/test_helpers --all-targets -- -D warnings | Passed | Strict changed-package static gate |
| cargo check --locked --offline -p vibemuxd | Passed | Default daemon build remains valid |

The five workflow tests cover: a successful write/create plus read-only review
and trusted verification; missing, partial, stale-attempt and stale-input owner
publications; a repair preserving an earlier created file and cumulative
checkpoint; cancellation after the real broker has published; and refusal of
run_dev_tests grants before controller launch.

The final post-read interruption fence was followed by another 37-test unit run
and strict all-target Clippy pass. The qualified independent source review then
reported no remaining actionable defect. Reviewers inspected source; command
results are parent-observed.

Retention tests model dropped/unpolled owners and late interruption. The held
inline-import test verifies that an aborted executor cannot complete its join
before the held operation returns. These are deterministic ownership controls,
not injected OS disk hangs, writer crashes or native isolation tests.

## Limits and reproducibility

There is no live Claude/provider acceptance, MXC qualification, remote execution,
external delivery, OS compare-and-swap guarantee, multi-file transaction or hard
filesystem wall-clock deadline in these results. File-only fixture profiles are
Windows/multithreaded-runtime only. Existing native startup/containment/release
failure reconciliation remains separate. No full workspace gate is claimed.

Private failed/unknown attempt inputs and results remain for owner reconciliation;
successful confirmed temporary directories are removed. The completed validator
checkout was removed after comparing every owned/copied source file with the
retained primary files. Its pre-integration lock context was preserved with local
validation logs; the branch and other worktrees were not removed.

Final artifact checks also passed: cargo fmt --all -- --check; git diff --check;
702 publishable text files with zero findings and 18 existing reviewed negative
fixtures; and all 217 Hub artifacts. No artifact exclusions were added.
