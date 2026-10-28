# ADR 050: Keep process ownership through the completion barrier

Status: Accepted for daemon dispatch and trusted-verifier lifecycle.
Windows synthetic validation; native broker/runtime qualification remains open.

Extends ADR 029, ADR 031, ADR 047 and ADR 049.

## Context

A protocol terminal, stdout EOF, controller exit, descendant publication and
process-tree settlement are different observations. The daemon previously waited
for its trampoline and then requested tree termination without checking Windows
Job emptiness. It also ignored cancellation after a successful protocol terminal,
allowing a publishing descendant to hold output open for the full grace.

A synthetic controller that exits with a publishing child reproduced the latter
failure. Pipe inheritance made the publication controls pass even before the fix;
that accidental ordering is not a process-ownership contract.

## Decision

Windows native dispatch now uses the existing safe, preconnected asynchronous
output-pipe factory. Containment still precedes release of the trusted trampoline.
Natural completion requires root exit and a successful query showing zero active
members in the exact owned Job. A terminal frame starts one shutdown grace; EOF
and the descendant wait retain its remaining deadline. An unterminated last
record parsed at EOF does not reset an already-started cancellation grace.

Cancellation after a successful terminal interrupts the barrier. A correlated
cancellation response may finish gracefully within the remaining deadline.
Cancellation and owner closure remain checked before durable finish, whose
existing writer fence retains precedence over a concurrent successful result.

Forced cleanup requests termination and then observes reaping, Windows Job
emptiness, and joined stderr draining under one six-second cleanup budget.
Timeout-aborted output tasks are joined. Requesting a kill is not evidence that
cleanup completed.

If cleanup cannot be confirmed, dispatch keeps the exact NativeProcess owner in
its service registry, stops new admission, and omits durable terminal finish.
Its existing reservation therefore remains for explicit reconciliation. Workflow
result use and automatic worktree cleanup refuse while an owner is unresolved.

Trusted verification uses the same barrier. An unresolved verifier returns its
owner to the service, stops the suite loop before reading or removing the output,
and reports workflow_process_cleanup_pending. It cannot create a verifier receipt
or proceed to another suite. Recovery output and worktrees remain available.

POSIX retains its existing process-group contract, without a new all-descendants
empty claim. Failed group signalling does not reap the leader. The process-group
owner drops before Child so any retry occurs while the leader identity remains
reserved.

## Compatibility and authority

No database migration, Control operation, launch-profile identity, slot promotion,
or vendor permission expansion is introduced. The new workflow error is a
content-free diagnostic. Existing graceful protocol cancellation remains valid.

The deterministic fault hook exists only with test_helpers and requires a server
holding a registered fixture execution guard. It discards one observed cleanup
receipt after a real synthetic process runs. It is not an injected operating-
system failure and is absent from the default daemon build.

The lifecycle barrier does not validate a broker candidate. A future broker
profile still needs complete atomic owner output bound to the admitted attempt,
lease generation, input and profile before terminal dispatch publication. Root
exit and Job emptiness alone cannot establish artifact validity.

This increment covers settlement after NativeProcess::spawn returns an owner.
Startup/containment/release failures retain their existing cleanup behavior.
No operating-system kill/query/reap failure, Linux/WSL execution, native vendor
qualification, sandbox qualification, or full live/remote workflow is certified.

## Verification

See [the validation record](../evidence/native_completion_barrier_validation.md).
