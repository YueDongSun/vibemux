# ADR 035: Claimed supervisor context actions

Status: Accepted for the offline runtime continuation. Extends ADR 034.
Live gateway/vendor qualification remains a separate gate.

## Decision

`ReadArtifact`, `QueryContext`, `SendMessage`, and `ShareContext` use the
existing supervisor round/action ledger, writer, private content store and
broker. Every effect follows durable proposal admission and a single winning
claim. Native handoff remains unavailable.

- `read_artifact` accepts an admitted candidate digest or a workflow-owned
  retained bundle content digest. Candidate reads use immutable collected
  manifests and typed blob identities, never model-supplied paths or live
  worktrees. They create an action-owned scoped bundle. Retained-bundle reads
  create an action-owned receipt alias without renewing retention.
- `query_context` retrieves a task's permitted collected candidate excerpts
  and public messages involving its exact admitted worker sessions. Empty,
  unavailable, expired, purged or forbidden sources do not become successful
  retrievals. No arbitrary SQL, raw transcript, contract prompt, URL or file
  query is exposed.
- `send_message` queues one question to one uniquely admitted worker session.
  The original model body stays transient through proposal commitment; its
  persisted action has empty `SendMessage.text` and an explicit
  `message_body: {body_sha256, body_bytes}` binding. The action fingerprint
  still covers the full original proposal. A missing or changed transient body
  cannot execute the action. Recovery never reconstructs or replays it.
  Free-form sends are permitted only before any applied `read_artifact` or
  `query_context` in this workflow's complete action history. After a private
  retrieval, the supervisor must use the scoped `share_context` path. This
  conservative rule has no content-classification exception: paraphrasing,
  claiming that text is public, or selecting an otherwise valid recipient
  cannot declassify retrieved data. Both proposal admission and atomic message
  admission enforce it; a read applied after a send batch was committed still
  prevents that send. The rule persists across generations and beyond the
  bounded recent-status window.
- `share_context` answers one existing, unexpired question/context request
  from the recipient to the source in the same cooperative workflow. Both
  communication policies must admit the peer. The source's immutable candidate
  is filtered through both tasks' read scopes, protected/forbidden paths and
  context limits. A supervisor grant conveys this bundle without changing any
  TaskSpec, contract, tool permission or worker role. Comparison competitors
  cannot receive this tool's shares.

The existing recipient delivery protocol renders admitted questions/grants at
the next bounded worker turn and acknowledges them only when that attempt
completes. An applied send/share proves durable admission, not worker delivery,
acknowledgement or task completion. Fixed message and bundle IDs derive from
the durable action identity. Message admission occurs in one writer transaction
that rechecks the running generation, winning claim, exact full-plan digest,
target contract/session, source request, bounded outstanding-message allowance
and effect identity. Applied results require matching post-claim evidence,
including the action/generation in the immutable message-admission event.

## Retrieval and retention

Only applied read actions from the immediately preceding model round populate
`tool_results`. Before the next model call, their references are resolved again
through current ownership, policy, retention and tombstone checks. Text is
explicitly labeled `untrusted_data`; it is never added to system instructions,
ordinary status, audit events or the supervisor ledger. The exact resulting
prompt remains covered by the existing model-call context fingerprint.

A message query accepts only pending `admitted` records with no prior delivery
or acknowledgement, exact current source and recipient sessions, the current
recipient contract (or sender contract for the supervisor inbox), exact
audience and a live creation/expiry interval. The daemon records a completed
read only after verifying the private bytes. The writer then appends a
content-free `workflow_supervisor_context_read` event bound to the action,
generation, round, plan, exact result references and their metadata fingerprint.
Only that exact post-claim evidence can finish the read as applied. Finish
rechecks current references, including expiry and message state; pre-existing
messages, changed references, old sessions or an earlier read event cannot
stand in for a new action's actual retrieval.

One round permits at most eight data items and 32 KiB of serialized item data.
Each task's smaller context budget still applies. Candidate bundles also use
the existing selection-count and redaction limits; oversized data is refused.
Messages keep their existing body/TTL/depth bounds. Questions/grants additionally
consume the recipient's bounded outstanding supervisor-message allowance;
their eventual worker turn consumes the existing task/model allowances.

Operational candidate blobs remain governed by their admitted manifest and
active workflow. Opt-in retained bundle text requires a present writer record
and an unexpired per-workflow content-index owner. Another workflow's identical
digest supplies no authority. Missing legacy expiry, tombstones, absent bytes,
digest mismatch, oversized bytes, links and Windows reparse points fail closed.
The shared content gate serializes retained reads/aliases with purge. A retained
read does not re-create purged content or extend its lifetime.

In a running model-driven workflow, supervisor-addressed worker replies remain
pending and queryable until their TTL; they are not acknowledged as model-read
merely because the worker finished. The deterministic coordinator retains its
existing immediate settlement behavior. Pending replies carry no permission to
resume after unknown outcomes.

## Compatibility and recovery

No new database table or Control operation is required by this slice. The
optional typed action `message_body` field defaults to absent when reading
historical actions and is omitted for other tools. New send records extend the
persisted/status JSON shape; older strict readers cannot consume those records.
This change ships with the continuation's forward schema/version boundary.
Rollback therefore follows its matching database/private-state backup procedure,
not merely checking out an older binary.

The same round and identical proposal reuse their action IDs without another
effect or budget charge. Changed bodies/keys, cross-round replays and duplicate
claims are refused. A claimed send/share with an uncertain outcome remains
unknown and fences restart, preserving the existing recovery rule. A failed
read is a content-free blocked result; absence is never promoted to success.

## Threat model and evidence

The model may name foreign digests, forged recipients, old contracts, expired
requests, conflicting idempotency keys, source paths outside the recipient's
scope or text that impersonates runtime instructions. Typed identities, exact
writer joins, scoped snapshot reads and the untrusted data envelope preserve
these boundaries. Git worktree isolation remains separate from a security
sandbox, and private state does not isolate hostile processes running as the
same OS user.

Focused production-store cases cover body-free persistence, exact replay and
conflict, winning claims, wrong body/recipient/contract/plan/generation/expiry,
omitted policy, pre-claim receipts, outstanding-message limits and unknown
restart fencing. A synthetic `ModelGateway` scenario drives actual guarded
worker turns, sends a question before private retrieval, reads candidate bytes
into its next context, retrieves a worker request, shares permitted source
bytes, verifies recipient prompts/acknowledgements and retrieves the worker's
response. A second guarded scenario retrieves source-only candidate bytes and
attempts to forward them to a task without shared read scope; it requires a
typed refusal and zero admitted messages. Additional store cases cover complete
history rather than recent status, a read applying after a send batch was
committed, missing/conflicting read evidence, wrong source identities, dead
messages and expiry between read and finish. Test definitions
are not execution evidence; the parent integration receipt records executed
commands, outcomes and platform limits.
