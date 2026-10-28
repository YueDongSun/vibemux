# Workflow lease-loss cancellation validation

Date: 2026-10-27. Platform: Windows, Rust 1.92.0, locked dependencies.
Evidence class: synthetic daemon/workflow fixtures; no model or live provider.

## Reproduction

The new tests quarantine an active worker or reviewer lease through the running
daemon's authoritative writer. Each fixture turn delays for sixty seconds. The
test gives lease-loss cancellation ten seconds to settle and always shuts down
the owned fixture before asserting, including on the failing baseline.

Before the production fix, both tests failed with
"a quarantined lease left its turn running: Elapsed(())". No assertion or timeout
was relaxed to obtain the passing result.

## Focused validation

~~~text
cargo test --locked --offline -p vibemuxd --features test_helpers --lib harness_dispatch::lease_watch -- --test-threads=1
cargo test --locked --offline -p vibemuxd --features test_helpers --test workflow_lease_loss -- --test-threads=1
~~~

- Owner tests: 4 passed. They cover loss before registration, unexpected heartbeat
  owner termination, loss during pending admission with cleanup joined and the
  cancellation state retained, and healthy completion.
- End-to-end workflow tests: 2 passed. Both the worker and independent reviewer
  stop after quarantine; the long-running fixture emits no finished record.
  The workflow reports workflow_lease_lost, no review/selection/integration is
  published, and leases settle after dispatch completion. The worker case also
  produces no candidate receipt.

The first production fix passed both regressions. The final implementation then
moved interruption forwarding into the registered dispatch owner to close the
pending-admission/caller-wait race; the four owner tests and both workflow
regressions passed again.

## Scope of the evidence

These tests establish daemon cancellation and result refusal for the specified
fixtures. They do not qualify native Claude shutdown, sandbox startup or
termination, gateway-only egress, or the proposed closed broker profile. They do
not simulate an operating-system process-reap failure or a database outage.
Writer unavailability remains subject to bounded cancellation/finish calls and
existing startup reconciliation.

## Related checks and review

~~~text
cargo test --locked --offline -p vibemuxd --features test_helpers --test workflow_control -- --test-threads=1
cargo clippy --locked --offline -p vibemuxd --all-targets --features test_helpers -- -D warnings
cargo check --locked --offline -p vibemuxd
~~~

All five workflow-control tests passed: explicit cancellation, pause and prompt
inspection, restart without replaying completed work, old-contract discovery,
and slot eligibility. Strict all-target Clippy and the default build passed.
The default build has no fixture quarantine hook.

An independent source reviewer returned PASS for the bounded fix, including
pending admission, joined interruption, result refusal, and the writer's
unsettled-attempt reservation guard. Test execution remains parent-observed.
Existing completed-review receipt semantics after deliberate lease settlement
are unchanged.
