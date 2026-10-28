# ADR 051: Admit file-broker results inside the workflow attempt

Status: Implemented for explicit Windows synthetic fixture profiles.
Native controller and sandbox qualification remain open.

Extends ADR 031, ADR 044, ADR 049 and ADR 050.

## Context

The closed MCP broker already exported edited text, but its owner output did not
enter daemon workflows. A successful protocol terminal and process-tree cleanup
cannot establish that a broker result is complete or belongs to the admitted
attempt. Repair turns must also retain earlier accepted input changes.

## Auto-approved implementation decisions

1. Keep bootstrap/candidate schema 1. The daemon privately binds its unique
   request identity and input digest to workflow, lease generation, contract,
   task, session and profile. Candidate fields cannot replace that authority.
2. Add distinct codex_fixture_broker_v1 and claude_fixture_broker_v1 identities.
   The opaque fixture guard pins both the synthetic controller and actual broker
   executable and rechecks their images. No serialized label enables the profile.
3. Launch the broker as the controller's stdio child inside the already-contained
   Windows Job. The controller runs in a separate private directory; the original
   owned worktree remains the workflow's reserved resource.
4. Construct a bounded text snapshot from effective owned/read scopes and reject
   links, reparse points, special files, forbidden paths and invalid names.
   Snapshot the current turn input, including previous repair changes.
5. Consume output only after process settlement and before fenced dispatch
   completion. Decode the strict status/candidate envelope, validate exact attempt
   and snapshot identity, grants, paths, size limits and bounded receipt claims.
   Recheck the current worktree, then materialize into that same owned worktree.
   Existing candidate double collection, review, verifier and lease fences remain
   the authorities for workflow acceptance.
6. Retain private input, owner record and publication from preparation through a
   confirmed durable Completed finish. Unknown admission/claim/finish, cancellation
   and malformed output retain recovery evidence. No unknown effect is retried.

The pathless operation receipts are necessary consistency checks, not
authentication of individual file changes. Missing, partial, stale, extended or
outcome_unknown output refuses with the content-free
harness_broker_candidate_invalid code.

## Import ownership and cancellation

Import stays inside the registered executor task. On a multithreaded runtime,
block_in_place lets the runtime service other tasks without detaching the
importer from its join. The importer directly observes both cancellation and the
sticky heartbeat-loss receiver; its surrounding async forwarding loop cannot
poll while synchronous import runs.

Each file is staged and synced before another cancellation/lease check and an
existing-file content recheck. New files use no-clobber persistence, preserving
create-only authority. Existing replacements rely on the managed worktree's
single admitted writer and are not an OS compare-and-swap operation. The protocol
does not authorize external concurrent edits to that owned worktree.

File/count limits bound work and memory, not stalled filesystem latency.
Shutdown waits for a non-preemptible import to return before joining its owner.
Cancellation during multi-file import may leave partial changes in the isolated
worktree, but cannot produce an admitted candidate; recovery records remain.
The source checkout is not the import destination.

## Compatibility and migration

No database schema migration or Control operation is added. Existing writable
fixture profiles keep their behavior. Live profiles and default daemon admission
are not enabled by this change. The new profile requires Windows, a multithreaded
runtime, an opaque fixture guard and fixture evidence class.

The initial integration is explicitly file-only. An effective run_dev_tests grant
is refused before launch rather than silently removed. Deletions and arbitrary
commands are unsupported. Bootstrap v2 test execution remains a separate component
and qualification track.

To run this path, build the test_helpers targets and use the workflow_file_broker
integration suite. It builds the real broker entrypoint as vibemux_broker_fixture
and registers it through FixtureLaunchGuard::with_file_broker. There is no
user-facing JSON flag that upgrades an unqualified live slot.

Retained failed/unknown attempt directories are private workflow scratch. Inspect
their exact owner records during reconciliation; do not overwrite or relaunch a
request merely because a result is absent. Successful confirmed attempts remove
their temporary private broker directory. See the validation record for commands.

## Acceptance boundary

This connects the actual closed file broker to real daemon workflow admission,
candidate collection, synthetic independent review and trusted local verification.
The controller remains synthetic. Installed Claude behavior, native isolation,
remote execution, external delivery, non-Windows containment equivalence and the
full four-direction R&D workflow are not established by these tests.
