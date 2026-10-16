# ADR 039: Operator edits as immutable workflow successors

Status: implemented with focused native Windows fixture verification; full product acceptance remains incomplete.

## Context

The existing contract reducer describes semantic generations, but prompt inspection
does not admit an edited contract. Replacing an active plan would detach attempts,
candidate receipts, permissions and accounting from their admitted authority. A
stopped workflow can also race an operator edit with a resume unless the writer
fences the old aggregate atomically.

## Decision

An authenticated operator submits schema-1 `workflow_contract_edit` with a stable
edit UUID, exact parent workflow/version/contract identities, the explicit
`retire_quiescent_parent` disposition and a complete ordinary workflow request.
The request retains the exact original source bytes and supplies typed TaskSpecs;
an edit never invokes the compiler implicitly. The CLI uses `prompt edit
--request-file <file>`; text travels as bounded Control data rather than argv.

This conservative edit surface changes wording or narrows authority. Original
requirements, checks and non-goals remain present. Owned/read paths and tools
cannot broaden, forbidden paths cannot weaken, and budgets cannot increase.
Source, base, task/workflow keys, role, dependencies, capabilities, communication,
context policy and admission policy remain immutable. A broader task or different
source requires separate ordinary admission. The existing structural, source,
policy and cross-task validators run before the edit is admitted.

A changed task advances its contract version by exactly one and records the
existing semantic diff and previous-contract link. Unchanged task artifacts remain
byte-identical. Template, harness profile, transformation and bundle identity cannot
drift during a wording edit. Old plans, contracts, attempts, receipts, messages and
workspaces are retained. Nothing is merged, promoted or submitted by editing.

The daemon lifecycle semaphore serializes edits with starts. A live driver is
refused; callers stop and settle it first. The writer independently checks the exact
parent version/plan and rejects active leases, unsettled dispatches/model rounds,
pending or unknown supervisor effects, delivered messages, any native controller,
and running or recovery-required supervisor executions. No unknown effect transfers
to the successor or becomes implicitly approved.

One immediate transaction retires a quiescent prepared/paused/blocked parent along
legal phase edges, admits one new prepared aggregate, and appends transition and
edit provenance. Terminal parents stay immutable. A unique parent association
prevents competing successors; further editing targets the successor. The successor
inherits spent task counters and remaining decision/repair/call allowances. Its
deadline cannot extend the original grant; narrower elapsed limits are anchored to
the parent's admission time. Replanning does not reset accounting.

Replay binds the complete request, parent plan and immutable successor/artifact
payload. Only generated preparation timestamps are excluded from that payload
identity. An exact retry returns the recorded successor even after parent retirement;
changed IDs, budgets, profile, contracts or content refuse without a second effect.

Contract authority, recovery binding and private plan publication follow admission
through the existing immutable binding APIs. A crash during those steps leaves an
admitted but non-executable successor. An exact retry restores the same bindings.
If Git HEAD or pinned configuration has changed, retry refuses rather than using
new authority; recovery then requires the original revision/configuration. The
operation never downgrades or recreates state.

## Compatibility and privacy

SQLite advances from schema 7 to 8 with one additive provenance table. The marker
and table creation share a transaction. Old rows and events remain unchanged;
failed migration leaves schema 7 intact. Older binaries refuse schema 8 before
reading newly supported supervisor message payloads (ADR 038). Rollback requires
matching pre-upgrade database/private-state backups and binaries.

Control v9 adds this operation. Existing operations retain their minimum versions,
including model supervisor v8, DSH v7 and deterministic workflows v6. All arguments
remain inside the authenticated bounded Control envelope. Audit rows retain only
identities, counts and digests; request/spec text is transient or private plan data.
This surface does not change global context-window, compaction or provider settings.

## Verification

Pure tests cover allowed wording changes and refusal of restored tools, dropped
checks, changed roles and widened budgets. Production-store tests cover atomic
retirement, original contract visibility, accounting preservation, exact replay,
conflicting input/plan/artifact/budget, stale versions and competing successors.
Control-level tests cover a real successor and no implicit turn submission.
Schema tests cover historical forward migration and failed-migration rollback.
Test definitions are not execution evidence; results belong in PROGRESS and the
source-bound continuation receipt after checks run.
