# Native completion barrier validation, 2026-10-27

Source: the task-owned VibeMux working tree on base 45308fc.
Environment: Windows, Rust 1.92.0, locked offline Cargo dependencies; synthetic
native workers and the existing Node 24.18.0 trusted fixture verifier.

## Observed regressions and repairs

Three publication lifecycle controls use a synthetic controller that exits while
its contained child waits to publish and then waits again before exiting.

~~~text
cargo test --locked --offline -p vibemuxd --features test_helpers --test harness_dispatch native_completion_barrier -- --test-threads=1
~~~

Before the fix, cancellation after controller exit failed its five-second bound
because the read loop waited through the ten-second grace. The publication and
stuck-child controls already passed: inherited output handles kept the exchange
open. Do not describe those two controls as failing baseline tests.

After the fix, all three passed. Windows completion explicitly waits for Job
emptiness, uses safe asynchronous output readers, and responds to cancellation.
The stuck publisher is terminated and is gone when terminal dispatch is observed.

The first broader run passed 24 of 26 dispatch tests. Two existing correlated
cancellation tests failed because an acknowledged cancellation was force-killed.
The repair preserves the remaining graceful interval only for a correlated
Cancelled terminal; the final broader run passed all 26 dispatch tests and all
6 launch-trampoline tests.

~~~text
cargo test --locked --offline -p vibemuxd --features test_helpers --test harness_dispatch --test launch_trampoline -- --test-threads=1
~~~

## Cleanup uncertainty

Independent review found a verifier path that discarded an unresolved process
owner and a POSIX path that reaped a leader after failed group signalling.
The verifier now returns its exact owner, stops before consuming output or
starting another suite, and blocks automatic worktree cleanup. POSIX preserves
the unreaped leader and group-before-child Drop ordering.

~~~text
cargo test --locked --offline -p vibemuxd --features test_helpers --test workflow_cleanup_loss -- --test-threads=1
~~~

Both new workflow regressions failed before the retention repairs. Dispatch
retained its owner but used ordinary worktree cleanup; verifier retained zero
owners and continued to the second suite. Both now pass.

The tests run real guarded fixture workers/verifiers and deliberately discard a
completion receipt. They establish:

- One exact process owner remains in the service, and new admission refuses.
- A worker's durable dispatch remains Running with its lease unsettled.
- No worker candidate or verifier promotion is created from the uncertain result.
- Automatic cleanup retains every recorded worktree with a diagnostic reason.
- Verifier uncertainty leaves exactly the first invocation and its result file;
  the second suite never starts and the first result is not consumed/removed.

This models lost confirmation, not an actual OS kill/query/reap failure. The
test-only hook requires the opaque fixture execution guard and adds no Control
wire operation.

## Related compatibility checks

~~~text
cargo test --locked --offline -p vibemuxd --features test_helpers --test control_harness_dispatch --test workflow_control --test workflow_lease_loss -- --test-threads=1
~~~

All three Control tests, all five workflow-control tests and both lease-loss
tests passed. They retain versioned Control behavior, explicit cancellation,
pause and next-prompt inspection, restart without replay, slot eligibility and
interruption after worker/reviewer lease quarantine.

## Static checks

~~~text
cargo clippy --locked --offline -p vibemuxd --all-targets --features test_helpers -- -D warnings
cargo check --locked --offline -p vibemuxd
~~~

Both passed after the owner-retention repair. A Clippy-requested nested-test
conditional was simplified without changing its assertions. The default build
excludes the fixture loss hooks.

## Final EOF check and review

~~~text
cargo test --locked --offline -p vibemuxd --features test_helpers --lib harness_dispatch::executor::completion_tests -- --test-threads=1
~~~

The one protocol-framing regression passed. It establishes a correlated Claude
cancellation acknowledgment without a trailing newline, parsed only at EOF, and
checks that the exact preexisting shutdown deadline is retained. It does not use
a timing tolerance to infer the deadline.

Strict all-target Clippy and the default build both passed again after this final
correction. Independent source review returned PASS after the verifier, POSIX and
EOF findings were repaired. Test execution is parent-observed; source review is
not native qualification.

## Limits

This stage is a daemon lifecycle prerequisite. It does not connect broker output
to candidate admission, qualify protected controller startup or an OS sandbox,
or enable a live workflow slot. It covers settlement after successful native
owner construction; earlier startup/containment/release cleanup is unchanged.
POSIX source ordering was reviewed but not executed on Linux/WSL. No injected
operating-system failure, whole-workspace gate, live provider, remote device,
deployment, or full four-direction R&D acceptance is claimed.
