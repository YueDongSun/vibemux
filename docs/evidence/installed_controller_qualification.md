# Installed-controller daemon qualification

Date: 2026-10-27. Native Windows, installed Claude Code 2.1.304, Rust/Cargo 1.92.0,
Node 24.18.0 for the trusted TaskBoard fixture verifier. Source is the uncommitted
task diff on codex/rd_execution_owner_20261026 over 45308fc.

## What ran

The explicit ignored tests ran the installed Claude executable through the daemon
trampoline and Windows Job, with the real file broker as its MCP child. Inference
came from a bounded task-owned loopback Anthropic fixture using a fixed synthetic
key and model. No account credential or live/paid inference was used.

The worker positive case read, edited and created files through the broker, then
reached accepted workflow state with a separately pinned synthetic Codex reviewer
and the trusted local store verifier. The reverse-role case used a synthetic
Codex worker and the installed Claude reviewer; the latter exposed exactly the
read/search broker tools. This is protocol/effect integration, not model-quality
or independent human review evidence.

Six refusal workflows covered edit-on-missing, create-on-existing, an ungranted
edit tool, an unavailable native Edit tool, an unknown MCP operation, and an extra
command argument. Each required an actual error tool result as well as no accepted
candidate. The applicable cases also required the broker's exact denial code.

Cancellation occurred after the synthetic API observed a successful broker effect
but before the next completion response. The workflow settled cancelled with no
candidate promotion. The existing ownership barrier remained in force.

## Failure history and review corrections

The first installed test produced two Completed Claude dispatches and an admitted
two-file candidate. It failed with store_workflow_review_not_independent because
both roles used Claude. The fixture was corrected to use distinct pinned
controllers; the independence rule was preserved.

The revised canary setup initially failed compilation because a PathBuf was moved
before later use. Borrowing that value fixed the setup. A source review also found
that token-count requests skipped authentication/context observations. All accepted
message and token-count requests are now recorded, and API-key matching checks the
exact header value. A default regression sends a context marker and a key with a
suffix to prove both observations detect their respective conditions.

Project CLAUDE.md, SessionStart-hook and alternate-MCP canaries are committed
task-owned fixture data. Their fixed canary program has an executed positive
control before the marker is removed. During the installed runs, the marker stayed
absent and the context string did not appear in observed API input. These controls
cover the tested project/ancestor paths, not every managed or user configuration.

Strict Clippy reported bool::then in filter_map and a large guard enum variant.
Both were corrected without suppressions. A default-only unused import was moved
under its existing feature condition; the final default build is warning-free.

## Executed commands and results

All commands below ran at the repository root with locked offline dependencies,
the existing task build target and task temporary directory. The installed tests
received explicit verified absolute VIBEMUX_QUALIFY_CLAUDE and
VIBEMUX_QUALIFY_GIT_BASH values. The installed binary was not upgraded or replaced.

| Command | Result |
| --- | --- |
| cargo test --locked --offline -p vibemuxd --features test_helpers --test workflow_installed_controller -- --ignored --test-threads=1 | Initial three-test matrix passed: worker acceptance, six refusal workflows and cancellation; 153.45 seconds |
| cargo test --locked --offline -p vibemuxd --features test_helpers --test workflow_installed_controller installed_claude_review -- --ignored --test-threads=1 | Native read-only reviewer passed; 32.55 seconds |
| cargo test --locked --offline -p vibemuxd --features test_helpers --test workflow_installed_controller -- --test-threads=1 | API-observation regression passed; four native tests explicitly ignored in this invocation |
| cargo test --locked --offline -p vibemux_store --test workflow installed_controller_profile -- --test-threads=1 | 1 passed: Fixture-only profile and immutable replay binding |
| cargo test --locked --offline -p vibemux_harness --features test_helpers --lib guard -- --test-threads=1 | 10 passed, including original guard compatibility and new installed guard tests |
| cargo test --locked --offline -p vibemuxd --features test_helpers --lib harness_dispatch:: -- --test-threads=1 | 37 passed |
| cargo test --locked --offline -p vibemuxd --features test_helpers --test harness_dispatch --test workflow_file_broker -- --test-threads=1 | 26 dispatch and 5 broker workflow tests passed |
| cargo clippy --locked --offline -p vibemuxd -p vibemux_harness -p vibemux_store --features vibemuxd/test_helpers --all-targets -- -D warnings | Passed on final source |
| cargo check --locked --offline -p vibemuxd | Passed without warnings on final source |

The four installed test functions were executed across the matrix and the later
reviewer invocation. They cover nine workflow cases, including expected refusals.
The complete target now contains those four opt-in tests plus one default API test.
The distinct targeted test count for this stage is 84; this is not a full-workspace
or release gate.

A GPT-6.1 Sol final source review reported no actionable findings, including the
new store regression. Review was read-only; test results are parent-observed.
At reviewer handoff, two restricted read-tool handles remained unresolved in the
tool backend after interruption attempts, while exact-command process inspection
found no corresponding shell. Parent reconciliation subsequently returned Unknown
process id for both handles. No known handle remains; these observations are not
native-job or test-completion evidence.

## Remaining boundary

The profile is explicit test-helper Fixture evidence, not a Live capability.
It establishes the tested installed CLI, closed broker, daemon ownership/import,
independent fixture-role selection and trusted verifier flow. It does not establish
live model behavior, request-bound production gateway routing, gateway-only egress,
OS/MXC isolation, arbitrary managed customization exclusion, or remote execution.

MXC command-startup and backpressure cleanup remain unresolved. A read-only audit
found that the earlier passing full probe and later failing startup controls used
different declared bounds; no deadline or acceptance assertion was changed and no
native sandbox matrix was rerun here. The next diagnostic must bind the exact
artifact/context and observe startup phases before attributing a cause.

No commit, push, deployment, global runtime/settings mutation, or native slot
promotion occurred in this stage. See ADR 052 for design and migration details.


Final artifact checks passed: cargo fmt --all -- --check, git diff --check,
708 publishable text files with zero findings and 18 existing exact reviewed
negative fixtures, and all 217 Hub artifacts. No new artifact exclusion was added.
