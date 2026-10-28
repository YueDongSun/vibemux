# Workflow start observation and cancellation validation

Date: 2026-10-28. Source: uncommitted task diff over VibeMux 45308fc29 on
codex/rd_execution_owner_20261026. Windows, explicit Rust/Cargo 1.92.0 and
Node 24.18.0. No live execution/profile deployment or native Mxc retry.

## Scope

- Task: recover exact workflow start identity after a lost reply; cancel only the
  matched generation; retain the full personal R&D goal.
- Owned modules: workflow service observation/cancel boundary, serialized store
  read, Control v11, thin CLI, and focused fixtures. Hub consumer work is in its
  separate repository and uses public owner APIs only.

## Implemented

Positive-only latest execution observation and owner-atomic conditional cancel.
The existing ID-only operator surface remains compatible. No store schema changed.

## Behavior and invariants

Reads do not start, resume, cancel, settle leases or append events. Missing or
mismatched latest identity never authorizes retry. Conditional cancellation holds
one lifecycle permit through checks and effects. Responses contain bounded metadata.

## Files changed

- crates/vibemuxd/src/workflow/service/start_observation.rs
- crates/vibemuxd/src/workflow/service.rs and workflow/error.rs
- crates/vibemuxd/src/workflow_writer.rs
- crates/vibemux_store/src/workflow/supervisor_commands.rs
- crates/vibemuxd/src/control.rs and control_workflow.rs
- crates/vibemux_cli/src/workflow_commands.rs and command.rs
- crates/vibemuxd/tests/workflow_start_observation.rs
- Protocol/ADR/changelog/progress documents for this increment.

## Validation executed

Final command outcomes are appended after all relevant checks complete. Initial
new-API builds failed on the missing read and conditional-cancel methods; subsequent
actual Control fixtures passed. This preserves the original red evidence rather
than treating compiler guidance to call an existing mutating method as a solution.

## Platform coverage

- Windows: actual private Control IPC with synthetic workflow controllers.
- Linux/WSL: not executed in this increment.
- Live terminal/backend: not qualified by these checks.

## Compatibility / migration

Control v11 adds two operations; earlier minimum versions and response echoes stay
intact. No database migration. New observation requires matching CLI/daemon support;
an unavailable operation is uncertainty, not permission to resend start.

## Security impact

Authenticated owner-only metadata, strict argument shapes, no new endpoint and no
caller-supplied executable/path/prompt. Conditional cancellation closes the observed
read-to-mutation generation race. Source review remains distinct from runtime tests.

## Remaining work

Remote execution admission/routing, Live qualification and external result-delivery
authority remain in the full-goal audit. This prerequisite does not complete them.

## Known risks

Only the latest supervisor row is retained. An earlier generation may become
unobservable. A cancel acknowledgment is not terminal process evidence. The legacy
ID-only path is not generation-fenced. Existing native startup/resource-settlement
failures remain open and were not retried here.


## Final observed checks

All Cargo invocations used the existing explicit Rust 1.92.0 runtime, locked/offline
resolution and the task's external build/temp directories. No ignored native Mxc
qualification was invoked.

| Command | Result |
| --- | --- |
| cargo test --locked --offline -p vibemuxd --features test_helpers --test workflow_start_observation -- --test-threads=1 --nocapture | 3 passed; actual Control IPC with synthetic workflows |
| cargo test --locked --offline -p vibemuxd --features test_helpers --lib control:: -- --test-threads=1 | 43 passed |
| cargo test --locked --offline -p vibemux_cli --lib workflow_commands:: -- --test-threads=1 | 3 passed |
| cargo test --locked --offline -p vibemuxd --features test_helpers --test workflow_control --test workflow_admission -- --test-threads=1 | 5 control and 6 admission tests passed |
| cargo clippy --locked --offline -p vibemuxd -p vibemux_cli -p vibemux_store --features vibemuxd/test_helpers --all-targets -- -D warnings | Passed |
| cargo build --locked --offline -p vibemux_cli --bin vibemuxctl | Passed; built workflow help lists both new commands |
| cargo check --locked --offline -p vibemuxd | Passed with default features |
| cargo fmt --all -- --check | Passed |
| python scripts/check_public_artifacts.py | 714 publishable text files; zero findings; 18 existing reviewed negative fixtures |
| git diff --check | Passed |

These are 60 distinct targeted Rust tests, not the complete workspace, platform,
release or Live gates. The new conditional test starts a second generation before
issuing old/mismatched cancellation identities, confirms unchanged status, then
cancels the exact current generation. The lookup tests include before-admission
miss, discarded reply, restart observation and repeat-read invariance.

Optional metadata capture from those actual Control responses was consumed by the
current Hub validator: four observations and one cancellation acknowledgment passed,
plus nine wrong-identity/retry negative checks. This is a serialized producer-to-
consumer contract check, not an end-to-end live vendor CLI or remote-network run.

The Hub final suite passed 1,688 tests, syntax, current-source integration,273-file
artifact verification and 207 browser checks/192 render claims. Its focused recovery
browser passed 12 checks with one start, one conditional cancellation and six reads,
plus desktop/mobile image inspection. The browser's CLI was synthetic; its first
runs exposed fixture assumptions about argument order and about unknown cancellation
versus a fresh running task observation. Those failed records remain retained.

Independent qualified source review closed the generation-cancel race and a second
consumer race where a late legacy reply could downgrade newer generation evidence
and charge the circuit. Actual-argument, stale-version and cross-instance regressions
were observed failing before the fixes. The final source review returned PASS with
latest-only, legacy-path and native/live limitations preserved.
