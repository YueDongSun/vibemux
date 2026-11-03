# ADR 072: Private live command progress

- Status: Accepted design after bounded architecture review; implementation and acceptance pending
- Date: 2026-11-03
- Related: [ADR 057](057_persistent_harness_sessions.md), [ADR 070](070_private_command_tool_records.md), [ADR 071](071_private_file_change_records.md)

## Context

Saved and Workspace chats retain completed command/file observations but ignore
command starts and output deltas. The assistant-only Live contract cannot carry
this activity. Its closed JSON shapes and immutable Tool-v1 history must retain
their current meanings. A command item can start before an approval decision;
its presence cannot prove OS execution or grant permission.

The pinned Codex contract provides `item/started` and ordered
`item/commandExecution/outputDelta` notifications; `item/completed` carries the
authoritative final item. The delta has thread, turn, item and literal text, with
no event ID or offset. Repeated equal deltas can be legitimate output.

## Decision

Implement command progress as private, bounded, replacement-only observations
from the pure adapter through the existing daemon-owned runtime to both chat
views. This first live projection covers commands; completed file-change cards
remain supported and live file-change projection remains subsequent work.
Profiles, approval decisions and process permissions are unchanged.

### Pure projection

Keep a separate per-turn command-progress ledger, with at most 64 cumulative identities
and 256 KiB retained text, plus checked monotonic revisions. Retain command/cwd
prefixes within the existing terminal field ceilings and a bounded output tail
with observed/omitted byte counts. Append deltas in arrival order; never dedup
them by content. Duplicate identical starts are inert. Conflicting metadata,
unknown item deltas or optional malformed data limit progress capture without
changing a correlated vendor outcome or valid terminal capture.

Existing thread/turn/retired-turn checks apply before projection, including
bounded pre-ack buffering. Completed items retain the original Tool-v1 codec,
full-source replay fingerprint and once-only terminal ledger. Progress snapshots
coalesce by call ID in a transition. They use redacted Debug and no ordinary
event/log publication.

### Runtime ownership and handoff

`ActiveOutput` owns a separate progress ledger with cumulative identity accounting;
payload trimming or retirement does not reopen identity capacity. Its budget is
independent of the unchanged terminal ledger. First accepted command progress
reserves a generic Tool observation UUID without creating a database row. Later
snapshots reuse it. A terminal Tool with the same qualified binding, canonical
group and call ID reuses that UUID in the existing immutable write reservation;
provisional command/cwd text is not part of identity. Final Command-v1 bytes
remain completion-authoritative even after a progress conflict. If final kind
is FileChange, retire the command-progress payload and write the authoritative
File-v1 body under the same Tool UUID, preventing two lingering identities.
Progress-only entries never
enter the terminal-write ledger and never make `output_available()` false.

Reserve the final body/UUID before awaiting the existing writer. Pending or
unknown writes retain that exact reservation and cannot allocate/reappend.
Known append completion updates the transient row; late deltas cannot resurrect
an already terminal-reserved call. Completed transient rows may remain until
the active output is released so a history/progress polling race does not create
an empty gap. Capture budgets and omission remain explicit.

Deliver a coalesced progress batch through a synchronous, nonblocking,
first-party sink hook. Actual Saved/Workspace sinks use short `try_lock` memory
merges; contention may skip an intermediate replacement. There is no await,
writer operation, retry queue or detached task on this path. The next replacement
can refresh the observation. This introduces no renewed callback allowance or
delay in the existing cancellation/close/shutdown deadlines. Transient delivery
loss remains separate from terminal capture and canonical outcomes.

### Versioned private read

Add outer Control operation `chat_command_progress`, minimum protocol v17.
Its closed identifier-only request selects Saved or Workspace, chat ID,
generation and existing request ID. Service reads validate the correct family,
canonical turn/attempt and active binding before copying private memory, then
recheck current binding afterward. No renderer supplies a canonical group ID.

The reply echoes the binding and returns an optional schema-1 snapshot with
the daemon-derived canonical group ID, monotonic revision, omitted-entry count,
capture-limited flag and at most eight recent command entries. Each entry has
its reserved record UUID, observed phase/status, literal command/cwd,
output tail and original/retained/omitted counts. Vendor call/session IDs are not
needed in this renderer projection. Null means no observation for that binding;
it proves neither completion nor non-execution. Private lock failure is an
error, distinct from valid absence. Observed purge returns a distinct error so
the renderer invalidates retained private progress immediately.

The complete serialized snapshot is at most 8 KiB. Build it from bounded memory,
reducing the largest retained text field at a UTF-8 boundary until it fits,
preserving prefix command/cwd and suffix output. Drop older entries only if
metadata itself cannot fit; report omissions. Byte counts/truncation remain
accurate. Worst-case full outer responses, escaping and 128-byte transport IDs
must remain below the existing 64 KiB frame limit. This latest-observation read
returns one whole snapshot and needs no paging cursor or snapshot lease.

Existing Tauri commands remain 32. Add read-only progress variants to the typed
Chat/Execution requests and route them through the new v17 outer operation in
the Control client. Legacy outer Chat/Execution dispatchers explicitly refuse
those variants, including manually crafted v11/v13/v16 frames. Existing Live
requests/responses and all old operation minima stay unchanged.

### Renderer

Poll explicit progress reads with the existing active-chat polling owner. Fence
project/chat/generation/request/canonical identity and monotonic revision; clear
obsolete context and retain the last good observation across read failures.
Render literal reported activity, never executable actions or permission grants.

Use the reserved record UUID to present one tool identity. A canonical row and
its transient counterpart must not create duplicate cards. While a final body
is still loading, the last progress view can supply the one card; a successfully
read final Tool-v1 body takes over. Late progress cannot restore that live card.
Turn Ready/no-active-request can precede final private-body loading. Keep an
already authorized same-group last observation through that reconciliation,
including null reads, skipped delivery and partial/failed final reads. Show it
as last observed, with no running indicator after the turn ends. Successful
final-body handoff or explicit project/chat/generation/new-request/purge/disposal
invalidation clears it. A non-null advancing empty snapshot explicitly retires
rows. Progress is not persisted/exported as a second history and does not mark
a turn or Task successful.

## Compatibility and authority

Control advances to v17 for this read. New clients retain v16 support for old
operations and refuse progress locally against old daemons. Older binaries
reject v17 descriptors, requiring a coordinated app/daemon upgrade. Store 14,
private Tool-v1 bodies, export 1, profiles, approvals and Task-child capabilities
are unchanged. No migration, dependency or provider setup is introduced.

## Acceptance

Require pure correlation/pre-ack/late/conflict/repeated-delta/UTF-8/budget and
redaction cases; sink UUID/unknown-write/handoff and progress-only completeness
cases; nonblocking contention/flood/cancellation regressions; v17/authentication,
legacy-bypass, wrong-family/stale-binding and full-frame size checks; contained
synthetic turns whose progress is readable before any durable Tool record and
whose completion appends exactly one unchanged Tool-v1 body; and both renderer
controllers plus actual component escaping/handoff checks. Include Ready before
body load with partial/read failure, authoritative changed command/cwd, command
start followed by FileChange completion under one UUID, and late-response
suppression. Use no installed
vendor/model input or desktop control. Native interaction and installed-version
qualification remain separate acceptance work.

## Sources

- [Pinned command delta](https://github.com/openai/codex/blob/rust-v0.166.0/codex-rs/app-server-protocol/schema/typescript/v2/CommandExecutionOutputDeltaNotification.ts)
- [Pinned ThreadItem](https://github.com/openai/codex/blob/rust-v0.166.0/codex-rs/app-server-protocol/schema/typescript/v2/ThreadItem.ts)
- [App Server lifecycle and ordered deltas](https://learn.chatgpt.com/docs/app-server)
