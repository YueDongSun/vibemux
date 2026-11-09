# Operator intake and worker follow-ups

These commands need the existing daemon workflow configuration and admitted
harness routes. Current writable execution evidence is from fixture workers;
this increment does not certify live vendor execution. See [ADR 057](../adr/057_superagent_workflow_intake.md).

## Admit a work request

Wrap an existing workflow request and policy in one intake document:

```json
{
  "schema_version": 1,
  "event_id": "27cbb2a2-4198-4eec-bb70-77717a3152ef",
  "request": { "...": "existing workflow request fields" },
  "policy": { "...": "existing operator policy fields" }
}
```

The placeholders above describe the wrapper, not an executable request.
Use the complete request and policy described in the [workflow section](../../README.md#dual-track-workflows-pre-alpha-adr-031-offline-only)
of README. Set each worker TaskSpec's `template_version` to `worker_v2`
to require structured reports. The daemon derives the request key from the
event UUID; keep the UUID for retries and use a new one for separate work.

```powershell
vibemuxctl workflow intake --request-file intake.json --project-root .
vibemuxctl workflow start --contract <intake.prepared.start_contract> --request-id <new-uuid> --project-root .
vibemuxctl workflow reports <workflow_id> --project-root .
vibemuxctl workflow reports <workflow_id> --include-content --project-root .
```

Intake returns `prepared` with the same start contract as existing preparation,
and a content-free `mandate` identifying the original request digest, policy,
base and contracts. It prepares the work for review before explicit start.
Raw report content is available only with both operator opt-in retention and
`--include-content`. Continue a report page with `--after-sequence <next>`
when `more` is true. Missing, expired or purged content is omitted.

## Send a follow-up

Read the current version and contracts with `workflow status` and the worker
session with `session inspect` or the status session list. Build a document
with those exact identities:

```json
{
  "schema_version": 1,
  "event_id": "90b1ed36-b048-442c-a48b-6268f38e8579",
  "workflow_id": "<workflow UUID>",
  "expected_workflow_version": 7,
  "task_key": "track_a",
  "session_id": "<worker session UUID>",
  "contract_id": "<64 hex characters from status>",
  "contract_version": 1,
  "kind": "question",
  "text": "Keep the admitted interface; clarify how its existing error is handled."
}
```

```powershell
vibemuxctl workflow followup --request-file followup.json --project-root .
```

The follow-up waits for the worker's next admitted turn. Running work
receives it at a turn boundary. Paused or blocked work needs the existing
explicit start command. Follow-ups cannot revive terminal work, change
contracts or authorize broader work. A stale version is refused; refresh
status and review the current identities before submitting a new event.
A retry of the same admitted event returns its original receipt.

The endpoint accepts only `question`, `progress` and `finding`, and only a
known worker of a cooperative workflow. It does not accept sender names,
permissions, shell commands or external identities as authority.

## Structured worker output

Schema 2 adds one required object to the existing `vibemux_checkpoint`:

```json
{
  "worker_report": {
    "state": "blocked",
    "summary": "The admitted implementation needs one interface clarification.",
    "human_action": "Which of the two existing error behaviors should be preserved?"
  }
}
```

This fragment belongs inside a complete checkpoint with `schema_version: 2`
and the existing task digest, changed-file, test, issue, context and message
fields. `worker_v2` renders the complete contract into each turn prompt.

`waiting` and `blocked` require a concrete question. Any question, or a
`waiting`, `blocked`, `failed` or `dropped` state, blocks automatic progress.
The operator can inspect retained content, clarify within scope and resume
explicitly. A later worker report must resolve the attention claim. A
`ready` claim still goes through independent review and trusted verification.
A `progress` claim keeps the work unsettled; it cannot stand in for readiness
when the writer evaluates acceptance.

No GitHub/Feishu watcher or external reply sender is installed by this
feature. Those prototype capabilities require authenticated source adapters
and separately authorized communication.

## Privacy and local data

Default events and report queries contain identifiers, hashes and byte counts.
Treat that metadata as sensitive: identifiers and hashes permit correlation,
and hashes of short or predictable text can be guessed.

Private local workflow plans retain narrowed TaskSpecs, including quoted
request spans, even when optional content retention is disabled. Follow-up
text is stored in the private outbox until acknowledgement or explicit
cleanup. Expiry stops delivery; it does not guarantee deletion of every
staged file, and uncertain writer outcomes retain bytes for reconciliation.

Opt-in report retention stores plaintext in the daemon user's private content
directory. An explicit `--include-content` read can reveal request excerpts,
repository content or sensitive text repeated by a worker. Its output can
also be captured by redirected output or surrounding tooling. Enable it only
when that local retention and disclosure are acceptable. The current boundary
is OS-user access control and checked ownership, with no application-layer
encryption or automatic redaction of arbitrary report text.

Purging retained content preserves immutable audit metadata and private
workflow plans; it is not complete erasure of a workflow. Keep runtime state,
outboxes, databases, provider settings and raw transcripts out of source
control and PR attachments. Examples in this guide and the tests use
synthetic text and identities.
