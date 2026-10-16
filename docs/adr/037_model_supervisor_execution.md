# ADR 037: Observation-bound model supervisor execution

Status: Accepted for the offline increment. Extends ADR 031 and ADR 036.
Live model routing remains subject to the existing attestation gate; this
decision does not certify a live provider, vendor process, or native terminal.

## Context

The typed supervisor tool schema did not itself drive the daemon's workflow
effects. A usable model driver needs to propose actions against committed facts,
reserve each model request once, and dispatch the existing worker, reviewer,
verifier and integration owners. The deterministic coordinator remains useful
and must not silently become model-driven when a model route is configured.

## Decision

The operator explicitly chooses a driver. Existing `workflow_start` continues
to run the deterministic coordinator. New `workflow_supervisor_start` selects
the model driver. The in-memory run registry and the single-writer execution
generation jointly enforce one driver per workflow. The supervisor binds the
exact immutable recovery-plan digest, configuration digest, contracts, task
assignments, reviewer slot and harness identities before starting.

Each model round follows this order:

1. Read a bounded observation through the writer. It contains workflow and
   execution versions, per-task eligible slots, collected candidate gate facts,
   deterministic selection and recent content-free action results.
2. In one transaction, verify the complete observation digest and reserve the
   existing model-call ledger entry plus a supervisor round. Reservation debits
   the workflow's model allowance once and records both the observed workflow
   version and its own resulting version.
3. Persist submission, make one bounded `ModelGateway` call, and persist the
   terminal call receipt before inspecting its text. The model request body is
   `{"schema_version":1,"observation":...}` under a fixed supervisor prompt.
   The gateway's optional route-observation digest keeps its existing meaning;
   the workflow observation is a separate round identity and is also included
   in the fingerprinted model context.
4. Parse at most 16 KiB of strict JSON. A proposal may contain at most four
   decisions. Validate the original expected version rather than rewriting it
   to the post-reservation value. Only the round's own model debit, version,
   timestamp and round bookkeeping are compensated when checking target facts.
   Other workflow changes, eligibility changes, candidate changes and existing
   action results invalidate the proposal.
5. Commit a complete validated batch of action intents before any effect.
   Claim each intent once. Independent assignments run concurrently within the
   admitted width; all effects settle before the next model observation.
6. Finish each intent against exact owned evidence. Worker and reviewer actions
   bind a preallocated dispatch request ID, task, contract, lease and session.
   Verifier and integration results bind exact subjects, contracts and trusted
   verifier identity. Referenced receipts must follow the action claim. An
   unrelated receipt from the same workflow cannot prove this action ran.

The supported effects are inspect/list, dependency-safe task ordering, one
worker turn, independent review, trusted verification, deterministic comparison
selection, gated integration, task pause, task/global cancellation and a blocked
report. A plan is a permutation of admitted tasks, not a new contract. Assigning
again requests a bounded repair; dependencies must already be accepted.
Protected/forbidden writes preserve the existing run-failure behavior.

Read-artifact, query-context, message, context-share and native-handoff tools
receive a typed `supervisor_tool_unavailable` rejection in this increment. A
comparison requesting the trusted negative-control path is refused at start;
the deterministic comparison path retains that capability. There is no tool to
mark success, edit policy, widen permissions, choose arbitrary routes, execute
shell text, or mutate global context/compaction settings.

## Accounting, concurrency and recovery

Every new observed proposal consumes decision allowance, including rejected
proposals. Malformed text and schema-valid rejected proposals consume the
separate response-repair allowance (the existing `malformed_repairs_remaining`
field). A response seen with no remaining repair allowance is retained and
stops the driver; two repairs therefore permit at most three unsuccessful
responses. Repeated authority or unavailable-tool refusals cannot burn the
whole model-call allowance. Neither path charges the same model request twice. Action fingerprints bind the
full decision, expected version, execution generation, plan, configuration and
contract authority. Reusing an idempotency key in another round is a conflict.
Only the identical payload replayed against the same recorded round returns
its original result without another effect or decision charge.

A task pause changes only that task's gate. Operator pause/cancel reaches the
model call and child dispatch through the existing control signal. No late
model response is committed after an operator stop. A lost model response, a
claimed effect without terminal evidence, or an uncertain submission is retained
as unknown and fences a new generation. A pause intersecting such work reports
`blocked` with `supervisor_recovery_required`, rather than implying the workflow
can immediately resume. A quiescent pause remains resumable. Restart never replays it. Repeated end
and recovery calls preserve that state and retain all charges. Startup recovery
uses keyset pages, so the number of workflows is not limited to one status page.

Task acceptance and workflow acceptance stay with the existing writer gates.
Review and verification are real isolated effects over collected bytes.
Integration materializes the exact accepted candidate set and runs the trusted
suites before acceptance. Worktree isolation remains distinct from a security
sandbox, and cleanup retains dirty or uncertain workspaces.

## Protocol and migration

SQLite advances from schema 6 to schema 7 atomically, adding
`workflow_supervisors`, `workflow_supervisor_rounds`, and
`workflow_supervisor_actions`. Existing workflow, dispatch, model-call, contract
and receipt bytes are preserved. A failed migration rolls back its partial
tables and marker. Older binaries cannot open schema 7. Rollback uses the
matching pre-upgrade database and private-state backup; it never recreates or
downgrades a live database.

The Control descriptor advances to v8. Previous operations retain their
existing minimum versions; v1-v7 requests remain accepted for those operations.
The two new operation names require v8:

```json
{"operation":"workflow_supervisor_start","argument":"{\"contract\":\"<prepared start contract>\",\"request_id\":\"<uuid>\"}"}
```

```json
{"operation":"workflow_supervisor_status","argument":"{\"workflow_id\":\"<uuid>\"}"}
```

These are operation/argument fragments within the existing authenticated
Control envelope. The start response retains the ordinary workflow-start
shape. Supervisor status uses schema 1 and returns the execution binding and
the most recent eight rounds/actions, so it is not a complete event export.
The CLI exposes `workflow supervisor_start --contract ... --request-id ...`
and `workflow supervisor_status <workflow-id>` as thin clients. Ordinary
`workflow start` behavior remains deterministic.

## Threat model and verification boundary

The model may emit stale versions, another task's slot, unknown digests,
duplicate keys, forged success claims, incompatible action batches, or text
that attempts to alter runtime instructions. The writer treats these as data,
validates the complete batch, and records a content-free refusal. Durable
intents and fixed dispatch IDs prevent a retry from launching an extra child.
No model callback receives SQL, credentials, unrestricted filesystem access or
permission authority. Raw prompts, proposed message bodies and model rationale
are not persisted in the supervisor ledger; only bounded identifiers, digests,
rejection codes and result references are retained.

Production migration and store tests cover original-version compensation,
external version changes, exact same-round replay, cross-round key conflicts,
exhausted budgets, changed plans, receipt ownership, repeated end/recovery,
lost responses and recovery beyond 512 workflows. Daemon fixture integration
exercises the model loop with real guarded worker/reviewer dispatch, verifier
execution and integration. Test definitions describe intended coverage;
executed results and platform qualification belong in `PROGRESS.md` and the
task's final validation receipt. No live AAG, native TUI or Linux/WSL claim is
made by the existence of this implementation.
