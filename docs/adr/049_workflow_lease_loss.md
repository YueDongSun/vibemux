# ADR 049: Interrupt owned workflow turns when lease renewal fails

Status: Accepted for the daemon lifecycle contract; fixture validation only.

Extends ADR 031. This does not qualify a native harness or connect the closed
worker-tools broker to live workflow slots.

## Context

The workflow heartbeat task stopped after a rejected renewal, but the active
worker or reviewer turn observed only workflow cancellation. Store admission and
candidate fences could reject stale work while its process continued running.
Two synthetic workflow regressions reproduced this: after the single writer
quarantined an active lease, the corresponding long-running turn remained active
past the cancellation test's ten-second bound.

Broker integration must preserve the existing attempt and lease ownership. It
cannot depend only on rejecting a candidate after an owner has lost authority.

## Decision

Each heartbeat holder exposes a sticky loss signal. Failed renewal or unexpected
termination of the heartbeat task invalidates its observers. The coordinator
checks this state before a turn and after its execution has joined.

The exact admitted dispatch receives that same signal. Its registered execution
owner forwards loss to its cancellation channel independently of the
coordinator's durable cancellation call. Loss that occurs while admission is
pending remains visible when the execution owner begins. The owner awaits the
attempt's existing process cleanup and fenced finish; it never abandons that
future when cancellation is requested.

The coordinator still asks the writer to record cancellation. Failure or an
ambiguous response does not manufacture a persisted terminal state. After joining
a lost-lease turn it returns the content-free workflow_lease_lost error before
candidate or review publication. Lease settlement continues through the writer's
existing checks that all attempts under the generation have settled.

## Compatibility and limits

- No database schema, Control protocol, profile identity, or slot eligibility
  changes.
- Healthy turns, operator pause, explicit workflow cancellation, and restart
  continue to use their existing semantics.
- The failure signal detects rejected/failed renewal and a lost heartbeat owner.
  This change does not add an independent monotonic lease-expiry watchdog or
  change the existing renewal interval and bounded writer-call retry policy.
- Process interruption is separate from durable cancellation. Unavailable state
  storage can still require startup reconciliation.
- The native process and sandbox qualification gaps remain open. Synthetic
  fixture cancellation is not native vendor or sandbox qualification.

## Verification

The regression uses only registered fixture executables and the real daemon
writer. A test-only hook is available only when the server was started with a
fixture execution guard; there is no new Control request or production mutation
surface. It applies the existing missed-heartbeat event to a selected fixture
lease.

See [the validation record](../evidence/workflow_lease_loss_validation.md) for the
red/green evidence and exact commands.
