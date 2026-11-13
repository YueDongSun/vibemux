# ADR 057: Operator work intake, follow-ups and structured worker reports

Status: Accepted for pre-alpha implementation, 2026-11-09.

## Context

The operator requested functions from DeepSeek's
[dsh-superagent research prototype](https://github.com/deepseek-ai/dsh-superagent-prototype),
then selected authorized work intake, follow-ups and structured worker reports.
The reference inspected was commit `8c32a9dc5d72c033816c6a8110b37f40cf94fba4`
(2026-10-31), specifically `src/core/model.ts`, `src/core/engine.ts`,
`src/app/tools/executor.ts` and `docs/001-domain.md`–`003-execution.md`.
This change independently adapts those behaviors in Rust. It does not import
the prototype's TypeScript runtime, dependencies, prompts, deployment scripts
or source credentials.

VibeMux already has admitted workflow contracts, isolated writable Runs,
fenced attempts, a context broker and deterministic acceptance gates. A
second ticket scheduler or state writer would duplicate that authority.

## Decision

Use the existing workflow as the work item and its admitted contract set as
the immutable mandate. Control v11 adds three operations:

| Operation | Input | Behavior |
|---|---|---|
| `workflow_intake` | Schema-1 event UUID, workflow request and operator policy | Explicit authenticated operator intake; derives `intake_<event UUID>` as the request key and calls existing prepare admission |
| `workflow_followup` | Schema-1 event UUID, exact workflow version, task, worker session, contract ID/version, kind and text | Admits context at the worker's next turn boundary; does not change scope or resume a terminal workflow |
| `workflow_reports` | Workflow ID, event-sequence cursor and optional explicit content read | Pages immutable, content-free worker claims; returns retained report text only when requested and owned by this workflow |

Intake is an explicit local operator action. There is no source adapter in
this increment that can authenticate a GitHub or Feishu actor. Supplied
`actor`, `source`, `authority` or permission fields are refused. Message
text never determines who is authorized. Automatic monitoring, inferred
requests and external replies need a separately admitted integration.

Intake prepares a reviewable workflow and does not start execution. The
existing explicit `workflow start` or `workflow supervisor_start` and all
slot, route, ownership and execution gates still apply. A duplicate event
returns the same admitted workflow; a conflicting admission under that
event is refused. The source text, narrowed specs, policy, base and contract
identity remain pinned by the existing preparation path.

Follow-ups are operator-authenticated `question`, `progress` or `finding`
messages for one known worker of a cooperative workflow. The writer checks
workflow version, phase, task progress, contract identity/version and
admitted worker-session ownership in the message/event transaction. A
recipient may hold at most 32 outstanding messages. Replays compare the
complete immutable envelope; they do not overwrite or restage an
acknowledged body. A definitely failed admission removes only the newly
staged body; an uncertain writer receipt retains it for reconciliation.
Compare-mode follow-ups, permissions, contract edits and arbitrary RPCs are
refused. A broader request requires separate intake; an existing follow-up
cannot widen the worker's mandate.

`worker_v2` is an explicit template version for checkpoint schema 2. The
new required `worker_report` holds `state`, a non-empty `summary`, and an
optional `human_action`. States are `progress`, `waiting`, `ready`,
`blocked`, `failed` and `dropped`; `waiting` and `blocked` require a concrete
question. Summaries are bounded to 4096 UTF-8 bytes, questions to 1000 bytes,
and the encoded report to 6144 bytes. Unknown fields/states are refused.
The schema-1 parser and `worker_v1` rendered bytes stay unchanged.

Reports are claims bound by the daemon and writer to a completed owned
worker attempt under its active lease. Workers cannot supply their own
Task, Run, session, contract or lease identity. Only metadata, content
digests and sizes enter the database and audit event. An attention report
blocks automatic progress, and the stored latest report for that exact
session/contract fences acceptance after a crash or restart. A follow-up
can prompt another bounded turn; a later report must resolve the attention
claim before the candidate may be accepted. `ready` does not bypass review,
verification or integration; a progress claim cannot finish the work.
Missing/malformed required checkpoints block
automatic progress.

## Persistence and compatibility

Store schema 10 adds `workflow_worker_reports`, indexed by workflow and
event sequence. Each immutable row and its `workflow_worker_reported` event
commit in one immediate transaction. Rows refer to existing workflows,
attempts and events. The migration is forward-only and rewrites no old row
or receipt; a failure rolls back the new table, index and marker together.

Default report queries carry schema 1 with `reports`, `next` and `more`.
Each report has an event sequence and a content-free `claim`. An explicit
`include_content` query may add `content`, independently checked against
its byte digest and claim digest, the workflow's opt-in policy, its owned
content index and retention deadline. Purging removes text while retaining
the immutable claim. Pages contain at most eight reports and fit Control's
64 KiB frame even at the accepted encoded-report ceiling.

Old: `workflow prepare` plus a schema-1 checkpoint.
New: `workflow intake` plus an opt-in `worker_v2` task and schema-2 checkpoint.
Both paths share the same admission and acceptance machinery. Established
operations keep their previous minimum protocol versions; new ones require
v11. Clients and daemon must upgrade together because older clients refuse
the new descriptor. Older stores refuse schema 10. Rollback means restoring
a separately retained schema-9 backup with the daemon stopped; never remove
tables or recreate a user database. Changelog and progress record this
private pre-alpha format change; no stable compatibility is promised.

## Threat model and limits

Requests, follow-ups and reports are untrusted data. The control boundary
authenticates the operator; owned attempts authenticate worker claims.
Strict envelopes, finite sizes, queue caps, version/fence checks and
append-only events guard malformed input, impersonation, stale writes,
duplicate delivery and optimistic success claims. Prompts and report text
never enter shell command strings or default events. Retained text stays
in the existing private, opt-in content store. Identical content in another
workflow cannot satisfy this workflow's retention permission or ownership.

This boundary minimizes default disclosure but does not anonymize metadata:
identifiers/digests permit correlation and predictable text may be guessed.
Existing private plans retain TaskSpecs and source quotations independently
of optional content retention; pending follow-ups retain plaintext outbox
bytes. Optional report content is plaintext, protected by OS-user access
control, and may include sensitive material repeated by the worker. Explicit
content reads can expose that material to stdout consumers. There is no
application-layer encryption or general secret-redaction guarantee. Content
purge preserves audit metadata and plans; expiry alone does not prove all
outbox files were removed. Publication includes source and synthetic fixtures
only, excluding runtime data, raw captures, credentials and machine settings.

These features do not certify writable live vendor routes, add DSH
multi-session persistence, implement source subscriptions/hand-back,
bridge approvals or publish messages. Native Windows and Linux/WSL paths
require their platform checks; fixture execution on macOS is offline
evidence only. See the validation record and guide below.

## Validation and usage

- [Operator workflow guide](../guides/superagent_workflow.md)
- [Implementation validation](../evidence/superagent_workflow_validation.md)
