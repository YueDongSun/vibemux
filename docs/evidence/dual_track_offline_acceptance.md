# Dual-track workflow offline acceptance (ADR 031)

Date: 2026-10-03. Platform: native Windows 11 Pro 10.0.26200, Rust 1.85.0
(MSVC, debug builds), Python 3.12.10 with pytest 9.1.1, Node v24.18.0, Git
2.51.1.windows.1, and Microsoft Edge 154.0.4258.48 for the browser suite.

## Scope and verdict

Code under test: `feat/dual_track_workflow` at `7de3aa5`. The runner recorded
a clean tree. The branch starts from the research baseline `2445a01`. It
carries the PR #14 and PR #15 test fixes as `c944699` and `0588620`.

Benchmark: `tests/fixtures/dual_track_benchmark/dual_track_benchmark_v1.json`,
SHA-256 `e74d65f0a5b691bfea54a9fff4f5813ea1ca05f7a9ce6224d9e2af094ceab3a8`.
The runner refuses a catalog that does not cover exactly the benchmark's
(scenario, execution class) pairs.

**Verdict: `BLOCKED` (exit code 2). The evidence class is
`offline_fixture`.** Of the 37 scenarios, 22 are `PASS`, 11 are `BLOCKED`, 4
are `NOT_RUN`, and none are `FAIL`.

- The verdict is not `OFFLINE_PASS`. That needs every non-live class to pass,
  but four fixture classes are `NOT_RUN` because their behavior is not
  implemented: the G01 and G03 gateway fixtures and the T02 and T03 native
  adapter fixtures.
- Every live class is `BLOCKED`. Live mode was not requested because no budget
  or configuration authorization was given. Live mode is also not
  implemented beyond its gate.
- This is not dual-TUI programming acceptance. No model, AAG gateway, or
  vendor CLI was contacted, and `model_calls` is 0. Every worker, reviewer,
  and compiler turn was a synthetic fixture process in structured mode. No
  native TUI session was owned or attached.

## Commands

From the repository root, with `CARGO_TARGET_DIR` outside the repository:

```powershell
python scripts/verify_dual_track.py --mode offline --output <new_dir_outside_the_repo>
python scripts/verify_dual_track.py --validate <dir>\dual_track_report.json
```

| Runner command | Exit | Duration | Outcome |
|---|---|---|---|
| `cargo test --workspace --all-features --no-fail-fast --message-format=json` | 0 | 348 s | 708 passed, 0 failed, 2 ignored in 86 test targets |
| `python -m pytest -p no:cacheprovider --basetemp <private root>` | 0 | 10 s | 85 passed |
| `node tests/fixtures/taskboard_lite/calibration/calibrate.mjs` | 0 | 147 s | all calibration expectations met (below) |

- The standalone `--validate` run was started from the report's parent
  directory with a relative path. It printed `dual_track_report.json: valid
  (BLOCKED)` and exited 0.
- The runner also re-derives the report itself (scenario E01). Its
  `validation.problems` list is empty.

Checks on the same head, outside the runner:

- `cargo fmt --all -- --check`: clean.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`:
  clean.
- `ruff check .` and `ruff format --check .`: clean.
- `mypy --strict` on `scripts/dual_track_acceptance`,
  `scripts/verify_dual_track.py`, and `tests/test_verify_dual_track.py`:
  clean.
- `git diff --check 2445a01..HEAD`: clean.
- Repository-wide `mypy .` reports 20 errors in `tools/a2a_python_interop.py`,
  `tools/a2a_tck_auth.py`, and `tests/test_harness.py`. These are the same 20
  errors recorded on `main`, and this branch does not change those files.

## Scenario results

The class results come from the run's own logs and evidence files. A
scenario is `FAIL` if any class fails. Otherwise it is `NOT_RUN` if any class
was not run, then `BLOCKED` if any class is blocked, and `PASS` only if
every class passes.

| ID | Scenario (abridged) | Status | Execution classes | Evidence or reason |
|---|---|---|---|---|
| B01 | Baseline behavior stays compatible | PASS | repository_test PASS | full cargo and pytest suites |
| B02 | Windows runtime isolation, no shared ACL-marker mutation | PASS | native_windows_test PASS | 9 tests |
| P01 | Byte-identical rendering, replay reuses accepted artifact | PASS | deterministic_test PASS | 7 tests |
| P02 | Dropped prohibitions, changed numbers, invented scope rejected | PASS | deterministic_test PASS | 9 tests |
| P03 | Literals and adversarial text cannot change policy | PASS | deterministic_test PASS, adapter_fixture PASS | 10 + 2 tests |
| P04 | Semantic edits need a new contract generation | PASS | deterministic_test PASS | 4 tests |
| G01 | Two role routes through AAG without cross-talk | NOT_RUN | gateway_fixture NOT_RUN, live_gateway BLOCKED | no daemon AAG client; only the pure route contract is unit-tested |
| G02 | Unsupported capability refused before admission | PASS | adapter_fixture PASS | 5 tests |
| G03 | Pre-output failover vs post-output failure | NOT_RUN | gateway_fixture NOT_RUN | no daemon gateway client; only the pure retry and stream rules are unit-tested |
| G04 | Live requests reach AAG with truthful usage | BLOCKED | live_gateway BLOCKED | live only |
| S01 | Two slots, separate workspaces, overlapping work | BLOCKED | deterministic_fixture PASS, live_coding BLOCKED | fixture: two worker sessions on `slot_a` and `slot_b`, structured mode |
| S02 | Zero slots blocks, one slot runs sequentially | PASS | deterministic_fixture PASS | 3 tests |
| S03 | Lease loss cannot cause double ownership | PASS | deterministic_fixture PASS | 7 tests |
| S04 | No duplicated effects across restart or retry | PASS | deterministic_fixture PASS | 4 tests |
| T01 | Two owned native TUI sessions on Windows | BLOCKED | live_native_windows BLOCKED | live only; no native sessions exist |
| T02 | Prompt submission, human input ownership, interruption | NOT_RUN | adapter_fixture NOT_RUN, live_native_windows BLOCKED | native input ownership and unknown-delivery handling not implemented |
| T03 | Structured/native handoff never runs two controllers | NOT_RUN | adapter_fixture NOT_RUN, live_native_windows BLOCKED | handoff and context-transfer resume not implemented; attach is refused |
| T04 | Two distinct harness kinds perform writable turns | BLOCKED | live_mixed_harness BLOCKED | live only |
| C01 | Scoped bundle travels A to B with a delivery receipt | BLOCKED | deterministic_fixture PASS, live_coding BLOCKED | fixture: bundle from track A's accepted snapshot delivered and acknowledged |
| C02 | Forged, stale, duplicate, out-of-order messages handled | PASS | deterministic_test PASS | 7 tests |
| C03 | Secrets excluded, retention and deletion boundaries | PASS | deterministic_test PASS, native_windows_test PASS | 6 + 3 tests |
| C04 | Restart loses no acknowledged share, forges no content | PASS | deterministic_fixture PASS | 2 tests |
| W01 | Out-of-scope and protected-path changes rejected | PASS | deterministic_test PASS | 6 tests |
| W02 | Untracked files collected, later mutation detected | PASS | deterministic_test PASS | 6 tests |
| W03 | Dirty or foreign resources retained | PASS | deterministic_test PASS | 6 tests; 7 of 7 workspaces retained as `workspace_unsafe_cleanup` |
| V01 | Protocol-complete but wrong candidate cannot pass | PASS | deterministic_fixture PASS | 3 tests; both-fail selects no winner |
| V02 | Review and verifier receipts bind the same candidate | BLOCKED | deterministic_fixture PASS, live_coding BLOCKED | fixture: 2 accepted candidates with independent reviews and pinned-verifier receipts |
| V03 | Integration passes without touching the checkout | BLOCKED | deterministic_fixture PASS, live_coding BLOCKED | fixture: integration of 2 candidates passed `api`, `store`, `browser` |
| V04 | Selector refuses the defect; both-fail has no winner | BLOCKED | deterministic_fixture PASS, live_coding BLOCKED | fixture: defect and mutation excluded, verified winner selected |
| V05 | A late child result cannot promote a cancelled workflow | PASS | deterministic_fixture PASS | 3 tests |
| A01 | TaskBoard API passes the trusted suite | BLOCKED | trusted_api_test PASS, live_coding BLOCKED | calibration plus integrated tree: `api` 25/25, `store` 13/13 |
| A02 | Real-browser TaskBoard checks pass | BLOCKED | trusted_browser_test PASS, live_coding BLOCKED | calibration plus integrated tree: `browser` 16/16 in Edge |
| A03 | One bounded repair closes an injected defect | PASS | deterministic_fixture PASS | 2 tests |
| O01 | Optimizer changes only allowlisted fields | PASS | deterministic_test PASS | 4 tests |
| O02 | Fixed dev inputs, protected holdout, contracts unchanged | PASS | deterministic_fixture PASS | 6 tests |
| O03 | Harmful candidates rejected; versions roll back | BLOCKED | deterministic_fixture PASS, live_optimizer BLOCKED | fixture: 3 rejections, version 2 promoted within 14 of 64 fixture requests, rolled back to version 1 |
| E01 | Report covers every scenario, command, and identity | PASS | evidence_validation PASS | all statuses, references, and digests re-derived from the run's artifacts |

`scripts/dual_track_acceptance/scenario_catalog.py` (`RULES`) is the
authoritative map from each scenario and class to the exact tests,
calibration checks, and evidence checks that can satisfy it. It also lists
the related tests that cannot satisfy a `NOT_RUN` class, such as the pure
gateway contract tests for G01 and G03.

## Calibration

The TaskBoard Lite trusted suites were calibrated before they were trusted:

- The reference solution passed `api`, `store`, `browser`, and
  `browser_frontend_only`.
- All 9 mutants were caught: `api` 4/4, `store` 2/2, and `browser` 3/3,
  including an XSS through `innerHTML`.
- The unmodified base failed `api`, as expected. The contract stub passed
  it, which is what track B's frontend-only suite runs against.
- The verifier sources were unchanged afterwards.

## Workflow evidence exported by the run

- The report exports six of the workflows the tests ran: five `accepted` and
  one `failed`, which was the both-defective compare.
- Those workflows hold 22 sessions, 15 candidate artifacts, 11 review
  receipts, 16 verifier receipts, and 1 integration receipt.
- `route_generation` is null and the gateway commit is null.
- `accounting`: 0 model calls and a known cost of 0.
- `optimization_improved` is true, but only on the fixture cases.
- Seven evidence files are digest-bound in the report: `cooperate_flagship`,
  `compare_selection`, `compare_both_fail`, `bounded_repair`, `pause_inspect`,
  `share_walkthrough`, and `optimizer_cycle`.

## Walkthroughs

Each walkthrough is an integration test that drives the real `vibemuxd` over
Control v6. The listed CLI commands call the same operations.

- **Pause and inspect the next prompt**
  - Test: `workflow_control::a_paused_session_shows_its_exact_next_prompt_and_one_slot_runs_sequentially`.
    Evidence: `pause_inspect`.
  - With one slot the plan is `sequential`.
  - `workflow pause` lets the running turn finish and starts no other turn.
  - `session inspect` shows the session.
  - `session inspect --prompts` reports the sent prompt by digest only
    (`prompt_not_retained`), and says that A's next prompt depends on the
    gate outcome. It returns B's exact next prompt text.
  - After `workflow start` resumes the workflow, B runs after A with exactly
    the inspected prompt. The operator's checkout stays clean.
- **Share context from A to B**
  - Test: `workflow_share::an_operator_share_reaches_the_other_track_and_survives_restarts`.
    Evidence: `share_walkthrough`.
  - `context share --bundle <id> --to <session>` admits an operator context
    grant to `worker:track_b`.
  - Repeating the share is answered as a duplicate. A share back to the
    bundle's source session, to an unknown session, or of an unknown bundle
    is refused (`workflow_share_invalid`).
  - The grant and bundle survive two daemon restarts.
  - B's next turn receives the bundle with its source text.
  - `workflow purge` deletes the retained text but keeps the content-free
    evidence.
- **Compare two candidates**
  - Test: `workflow_compare::the_trusted_suite_selects_the_correct_candidate_and_refuses_the_defect`.
    Evidence: `compare_selection`.
  - Both candidates complete the protocol and pass review. Only the trusted
    suite separates them.
  - The daemon-made negative control fails the trusted suite and is excluded
    as a mutation artifact.
  - The selector picks the verified candidate.
  - `two_defective_candidates_select_no_winner` (evidence: `compare_both_fail`)
    ends `failed` with no winner and no integration.
- **Policy promotion and rollback**
  - Test: `workflow_optimizer::only_a_verified_improvement_is_promoted_and_it_can_be_rolled_back`.
    Evidence: `optimizer_cycle`.
  - `prompt evaluate` selects on the dev split. The holdout runs only for
    the baseline and the final candidate, and the cycle consults it once.
  - Version 2 is promoted and applies only to future admissions. A workflow
    admitted earlier keeps its wording.
  - `prompt rollback` restores version 1, and `prompt versions` then shows
    version 2 as `rolled_back`.

## Cleanup

- All owned processes were joined.
- The runner removed its private temporary root. This includes the test
  projects, their retained workflow worktrees, pytest's base directory, and
  read-only Git objects.
- The suites left `pytest_basetemp`, `node-compile-cache`, `opencode`, and
  eight Edge `Importer_0_4` directories in that root. The report lists them
  as `suite_leftovers`, and they were removed with the root.
- Inside each test project, the daemon retained every workflow worktree as
  `workspace_unsafe_cleanup`. It removes none on its own (W03).

## Baseline and gap inventory

- Before any workflow code existed, the full workspace suite on `0588620`
  (baseline plus the two carried test fixes) passed: 531 passed, 0 failed,
  and 2 ignored in 74 test targets.
- The baseline claims verified against the source are listed in
  [ADR 031](../adr/031_dual_track_workflow.md#baseline-verification). In
  summary, dispatch keeps one project working directory, its profiles are
  read-only, submit returns only an admission receipt, the dispatch and A2A
  records keep their owners, and the AAG proxy pins one global provider with
  no request-scoped routing receipt.

## Not verified and blocked

- **No AAG gateway client:** there is no live supervisor or compiler, and
  `aag_only` is declarative. A `live` workflow config keeps every slot
  `declared`, never eligible to code. This blocks G01, G03, G04, and every
  `live_coding` and `live_optimizer` class.
- **No native TUI session:** none is owned or attached, `session attach` is
  refused, and native handoff is not implemented. This blocks T01 through
  T04.
- **Live mode stops at its gate:** `--mode live` needs an explicit budget and
  configuration authorization, and no live run happened.
- **Fixture-only optimizer gain:** the improvement is measured on fixture
  cases and implies no model gain.
- **Remaining limits:** the remaining known limitations are listed in
  [ADR 031](../adr/031_dual_track_workflow.md#not-implemented-known-limitations).
  They include the read-only prompt inspection, the unpersisted
  negative-control receipt, the crash simulated through the writer, and
  writable profiles that are cooperative admission rather than an OS
  sandbox.
- **Excluded claims:** the report excludes full A2A ITK conformance, remote
  or TLS deployment, compatibility with every harness, model parity or
  superiority, hostile same-user isolation, and N_qubit scientific
  validation.
