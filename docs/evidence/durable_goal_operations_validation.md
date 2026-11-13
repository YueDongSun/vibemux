# Durable goal operation validation

Date: 2026-11-13. Environment: managed Linux executor, Rust 1.92.0. Scope:
the current main plus PR 52 intake/report behavior, selected PR 56 execution
cleanup fixes, and [ADR 074](../adr/074_durable_goal_operations.md). The native
conversation branch was not imported; its incompatible store lineage is refused.

## Executed checks

The local validation used the locked dependency graph, two Cargo build jobs and
no incremental compilation. The following are distinct checks; some unit-test
filters overlap, so their counts should not be added as a unique test total.

| Check | Result |
|---|---|
| `cargo check -p vibemuxd -p vibemux_cli --all-targets --features vibemuxd/test_helpers --locked` | Passed |
| `cargo test -p vibemux_store --test workflow_operations --locked` | 6 passed |
| Existing supervisor reopen test `schema_eight_adds_owned_tables_and_survives_reopen`, updated for current schema 15 | 1 passed |
| Existing ledger migration test `a_v6_marker_without_ledger_tables_migrates_forward_when_reopened`, with the owned newer operation table removed from its synthetic v6 fixture | 1 passed |
| `cargo test -p vibemuxd --test workflow_goal_operations --features test_helpers --locked` before the additional raw-goal case | 2 passed |
| The same integration target filtered to `raw_goal_compiles_once` after adding that case | 1 passed |
| `cargo test -p vibemuxd --test model_runtime_output --features test_helpers --locked` | 4 passed |
| `cargo test -p vibemuxd --lib --features test_helpers --locked workflow::` | 40 passed |
| The daemon unit target filtered to `qualification` after the final canary-path fix | 7 passed |
| The daemon unit target filtered to `native_provider::tests` after the private-error-size fix | 6 passed |
| The daemon unit target filtered to `control_contract_accepts` and `protocol_boundaries` | 1 and 2 passed |
| The daemon unit target filtered to `native_process::tests::cleanup` | 3 passed |
| `cargo test -p vibemux_cli --lib --locked workflow_commands::tests` | 5 passed |
| The CLI unit target filtered to `expected_daemon_owner` | 1 passed |
| `cargo clippy -p vibemuxd -p vibemux_cli -p vibemux_store -p vibemux_workflow -p vibemux_harness --all-targets --all-features --locked -- -D warnings` | Passed |
| `cargo fmt --all -- --check`, `git diff --check` | Passed |
| `python scripts/check_public_artifacts.py` | 716 publishable text files, 0 findings; 18 reviewed negative fixtures |

The first Clippy pass identified large error return values in four new native
parser/configuration helpers. Those private helpers now return bounded static
codes and construct the existing public gateway failure at the async boundary.
No lint was suppressed and the public receipt contract did not change. A test
slice clone was also replaced with a borrowed slice.

## What the goal tests establish

The asynchronous submission is committed through the actual writer before work
starts. The acceptance tests launch built fixture child executables, create
independent Git worktrees and run the normal coordinator, independent reviewer,
candidate collection and trusted Node verifier. The final workflow phase is
`accepted` with verification receipts, not an injected terminal state.

The additional raw-goal case begins with empty `task_specs`. A fixture compiler
receives the complete format reference and source byte facts, returns candidate
specs through the ordinary model gateway, and the durable model-call record
finishes `completed` with exactly one call used. The same goal then reaches
verified workflow acceptance. Repeating the event returns the existing
operation without invoking the compiler again. The compiler itself is synthetic;
this proves the integrated admission path, not vendor model quality.

Other tests cover immutable duplicate receipts, changed-body conflicts,
prepare-only behavior, bounded workflow pagination, projection tampering,
schema-10 data preservation and transaction rollback, foreign-schema refusal,
and recovery across more than one page of interrupted admissions. Cancellation
and deadline tests observe gateway cleanup finishing before the model runtime
returns unknown and prove that a later call does not replay the effect.

The qualification tests require the actual tool inventory, correlated successful
tool results, the exact allowed test command, narrowly identified canary write
targets, unchanged proof/input/test files, and unexpired evidence tied to the
configured route. Files or a final model message alone cannot certify a slot.
These are parser/file-boundary tests, not a live Claude qualification.

## Environment and remaining validation

The existing `control::tests::local_control_health_auth_version_and_shutdown_round_trip`
was run once. It failed while starting the control server with
`EndpointUnavailable`, before any health/auth request. The managed executor
denies AF_UNIX socket creation. No TCP fallback, permission relaxation, ignored
test or transport substitution was added to turn this into a pass. The original
IPC tests remain required on a native host.

Codex 0.167.0-alpha.4 reported an existing ChatGPT login on this host, but the
bounded native compiler-profile probes did not produce a model completion.
Claude was not installed. Configured or logged-in status is therefore not
reported as successful native compilation, qualification, task execution or
acceptance. The [native setup section](../adr/074_durable_goal_operations.md#native-host-setup)
describes the installed-binary checks and explicit qualification required on the
target host, including the three-binary build, environment names and expiry.

Full workspace tests, nextest, dependency-policy/audit commands, native Windows
IPC/containment, browser transport and live vendor qualification were not
established by these scoped checks. Existing merge/CI/release gates remain in
force. The security dependency fix changes the pinned rustix 1.1.4 to 1.1.6;
it adds no advisory exemption.
