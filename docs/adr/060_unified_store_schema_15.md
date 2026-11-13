# ADR 060: Unified store schema 15 (chat 10-14 + worker reports 15)

Status: Accepted, 2026-11-13.

## Context

Two historical branches extended the store independently:

- PR #49 (`codex/native_harness_conversations`) used schema versions 10-14
  for chat sessions, turns, content, records, operations, execution owners,
  attempts, approvals, and workspace cleanup.
- PR #52 (`codex/dsh-superagent-enhancements`) used schema version **10**
  for `workflow_worker_reports`.

Both branches are fast-forward extensions of main's schema 9, so they never
collided in their own histories, but their version 10 means different DDL.
An integrated database must contain both families and must open either
historical layout without losing rows.

## Decision

The unified current schema is **15**:

| Version | Content |
|---|---|
| 1-9 | Main schema: events, projections, A2A, dispatch, workflows, decisions, supervisor, contract edits |
| 10-14 | PR #49 chat family, unchanged DDL |
| 15 | PR #52 `workflow_worker_reports` (+ page index) |

`SqliteStore::open` classifies an existing database read-only first:

- a main schema 1-9 database runs every missing migration;
- a chat-10..14 database gains only the later chat steps and version 15;
- a **reports-10** database already has marker 10 and the reports table, so
  the chat DDL runs without a second marker 10, chat versions 11-14 follow,
  and version 15 is recorded without recreating the table;
- a unified 15 database is opened as-is;
- a foreign Python store, an unrecognized layout, and a future marker are
  refused with their distinct codes and left byte-identical.

Each step keeps the existing immediate-transaction boundary, so a failed
reconciliation (for example an occupied index name) rolls back to the
original marker and object set.

## Alternatives

- Renumber chat to 11-15 and reports to 16: rejected; existing chat-10..14
  databases would need marker rewriting, which risks corrupting the history.
- Keep reports at 10 and refuse reports-10 databases: rejected; the
  integrated schema would permanently strand PR #52 databases.
- Merge chat and reports into one migration: rejected; it would duplicate
  large DDL and make per-step rollback harder.

## Consequences

- `STORE_SCHEMA_VERSION` is 15; daemon health and docs report 15.
- The store now contains chat and cleanup tables without the chat service
  code in this branch; they are schema-ready for the integration, and no
  runtime path writes them yet.
- A reports-10 database is reconciled once; reopening is idempotent.
- Legacy Python stores and unknown layouts remain refused, not guessed.

## Validation

`cargo test -p vibemux_store --all-features` (147 tests including six lineage
tests) covers: current recognition and idempotent reopen, reports-10
reconciliation, chat-10 and chat-14 upgrades, rollback on a conflicting
index, foreign-store byte preservation, future and incomplete layouts.
