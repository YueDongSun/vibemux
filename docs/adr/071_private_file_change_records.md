# ADR 071: Private file-change observations

- Status: Accepted and locally implemented; contained synthetic service and browser Find verified, native app and installed-provider acceptance open
- Date: 2026-11-03
- Related: [ADR 070](070_private_command_tool_records.md), [ADR 057](057_persistent_harness_sessions.md), [ADR 059](059_interactive_chat_approvals.md)

## Context

Command observations now have an owned private producer, writer and transcript
card. Completed Codex file-change items still disappear at the adapter boundary.
The desktop design also requires file/diff inspection. That presentation must
preserve the existing profile and approval restrictions and must not interpret
reported paths or diff text as filesystem authority.

## Decision

Extend the same ordered tool collection and private writer to completed
`fileChange` items from the pinned Codex 0.166.0 schema. Both completion
notifications and terminal turn-item arrays use the existing active thread/turn
checks. Started and patch-update notifications remain outside this increment.
The non-tool item count excludes both supported tool kinds; display overflow
cannot turn an otherwise valid vendor terminal into a protocol failure.

Use one shared call-ID namespace, 64-record/256-KiB per-turn budget, full-tail
replay identity, sticky capture loss and absolute callback batch deadline for
Command and FileChange. Cross-kind reuse of an ID is a conflict. The generic
in-process observation changes no permission or canonical state. The existing
13-field command version-1 body stays unchanged.

The new private body has exactly seven fields:

```json
{
  "format": "vibemux_file_change_tool_record",
  "schema_version": 1,
  "call_id": "file_change_1",
  "status": "completed",
  "changes": [{
    "path": "src/example.rs",
    "kind": "update",
    "move_path": null,
    "diff": "-old\n+new\n",
    "diff_original_bytes": 10,
    "diff_truncated": false
  }],
  "original_change_count": 1,
  "changes_truncated": false
}
```

Status is Completed, Failed or Declined. Each change has exactly the six fields
shown; its kind is Add, Delete or Update. A non-null move path is permitted only
for Update. Omitted and null vendor move paths map to the mandatory nullable
private field. Paths and move paths are nonempty literal text bounded to 4 KiB;
they are not resolved or opened. Empty change lists and empty diff strings are
valid observations. Repeated paths retain their reported order.

Validate all bounded source entries before selecting a display prefix. Source
change count is at most 1,024, with at most 1 MiB of original diff text. Retain
at most 16 changes, 16 KiB per diff and 32 KiB of diff text per body, using UTF-8
boundaries. If the aggregate diff budget or encoded 256-KiB body ceiling would
be exceeded, omit the next entry and remaining tail. Original counts and exact
truncation flags describe
both list omission and shorter diff prefixes. Any omission or inner truncation
makes capture incomplete. Full normalized source identity includes omitted
entries and discarded tails, so they cannot hide a replay conflict.

Saved and Workspace sinks reuse their durable turn/attempt, generation/fence,
acknowledged Codex binding and pre-append UUID/body reservation. Unknown appends
retain that identity and are never reissued as a replacement. Tool display loss
remains separate from the correlated vendor outcome.

## Presentation and compatibility

The existing inert tool card gains a File changes variant showing reported
status, ordered paths, change kinds, move paths and literal diff text. It does
not assume that the string is a unified patch or prove an on-disk effect.
List omission and diff truncation are visible. Zero retained changes with a
positive original count indicates omitted details, not a report of no changes.

All content stays escaped. No patch application, revert, path link, file read,
copy action or approval decision is added. Bounded literal blocks use the
transcript's vertical scrolling so selected Find hits can become visible after
their disclosure opens. No separate clipped vertical viewport hides a hit.
Find measures the selected text range after opening disclosures; centering the
entire long text block can still leave the matching line outside the viewport.

Interpretation requires a complete, externally untruncated Tool body with an
exact known schema. Command version 1 keeps its strict existing reader. Legacy,
malformed, unknown and partial bodies retain literal fallback. Older clients
and existing exports keep the new body's exact text. No SQL, Control or public
metadata migration is required; Control v16, store schema 14, native count 32
and existing profiles/approval denials remain unchanged.

## Verification

Require strict private-codec and Unicode/aggregate/list-budget checks, mixed-kind
deduplication and hidden-tail conflicts, wrong/retired correlation, sticky-loss
reset, both private sinks, bounded paging and history after restart. Synthetic
vendors only report file observations; guarded project files and HEAD bytes
must remain unchanged. Actual component rendering must keep paths/diffs inert
and distinguish empty, omitted and truncated data. Native card interaction and
installed writable-profile qualification remain separate gates.

Local acceptance includes both private sinks and the contained service's mixed
command/file, overflow and truncation cases, with paging/restart and unchanged
guarded files/HEAD. A browser fixture mounts the actual Svelte component and
imports the production search/reveal helpers. It reproduces the block-centering
failure and verifies selected-range visibility with the header and page fixed.
This is Chrome component evidence, separate from native VibeMux app interaction.
See the [execution record](../evidence/native_harness_m1.md#private-file-change-records-and-cards).

Protocol inputs are the official pinned
[FileUpdateChange](https://github.com/openai/codex/blob/rust-v0.166.0/codex-rs/app-server-protocol/schema/typescript/v2/FileUpdateChange.ts),
[PatchChangeKind](https://github.com/openai/codex/blob/rust-v0.166.0/codex-rs/app-server-protocol/schema/typescript/v2/PatchChangeKind.ts)
and [PatchApplyStatus](https://github.com/openai/codex/blob/rust-v0.166.0/codex-rs/app-server-protocol/schema/typescript/v2/PatchApplyStatus.ts)
schemas.
