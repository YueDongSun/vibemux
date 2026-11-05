# ADR 070: Private command tool records

- Status: Accepted and locally implemented; contained synthetic service verified, native card interaction and installed-provider acceptance open
- Date: 2026-11-03
- Related: [ADR 057](057_persistent_harness_sessions.md), [ADR 058](058_writable_chat_ownership.md), [ADR 059](059_interactive_chat_approvals.md)

## Context

Saved and Workspace transcripts already retain Tool records through bounded
private content reads. Their current runtime sinks publish assistant text only.
Codex command items are ignored, so renderer parsing cannot recover their
command, result or correlation. Approval decisions have their own canonical
authority; a displayed tool observation must not substitute for that authority.

## Decision

Map completed `commandExecution` items from the pinned Codex 0.166.0 app-server
contract into a private command record. Require the existing exact active
thread/turn binding before publishing from either item completion or terminal
turn items. Identical replay is inert; conflicting IDs or bodies refuse.
Unknown tool kinds keep their existing handling. Profiles and permission
denials remain unchanged.

Display projection loss is sticky for the current turn. Budget exhaustion,
unrepresentable optional tool data or conflicting tool replay withholds the
affected observation and marks output incomplete. It does not change an
otherwise correlated vendor outcome. Existing thread/turn correlation failures
retain their protocol rejection behavior.

The private version-1 body has exactly these fields:

```json
{
  "format": "vibemux_command_tool_record",
  "schema_version": 1,
  "call_id": "command_1",
  "status": "completed",
  "command": "read README.md",
  "command_original_bytes": 14,
  "command_truncated": false,
  "working_directory": "C:\\example",
  "output": "example output",
  "output_original_bytes": 14,
  "output_truncated": false,
  "exit_code": 0,
  "duration_ms": 42
}
```

Status is Completed, Failed or Declined. Null output means unavailable; empty
output means an observed empty string. Retain UTF-8 prefixes of at most 8 KiB
for command and 16 KiB for output, with original byte counts and visible
truncation. Bound working-directory text to 32 KiB, each encoded body to
256 KiB, and one turn's retained tool bodies to 64 records and 256 KiB.
Private body serialization is explicit. Debug and ordinary events contain no
commands, paths, output or vendor call IDs.

The actor forwards terminal observations to the actual Saved or Workspace
writer sink. Each sink checks its generation/fence, current durable
turn/attempt, Codex harness and acknowledged vendor binding. It reserves one
record UUID and exact body before awaiting append. Lost/failed responses do
not allocate a replacement or replay an effect. Unsettled display writes make
output incomplete without changing the separately correlated vendor outcome.
The existing protected immutable private writer owns all storage.
Optional tool callbacks share one absolute batch deadline. A timeout retires
further tool capture for that turn and still permits its correlated terminal
callback; the next successful turn claim resets capture. Binding errors retain
their fatal behavior.

Both transcript views render a collapsed, keyboard-accessible card with literal
command, directory, reported status, output, exit code and duration. Interpretation
requires a complete, untruncated Tool record with the exact known schema.
Legacy, malformed and partially read bodies remain escaped expandable text.
No displayed value becomes a link, executable action or approval. Existing
assistant aggregation is unchanged; this increment adds terminal command
observations, not a new live tool-progress stream.

## Compatibility and authority

This adds a versioned private Tool body, not a canonical state, metadata DTO,
SQL column or Control operation. Existing opaque Tool text remains readable;
old clients display new bodies as literal text. Exports retain the exact private
body through their existing text field. Control v16, store schema 14, native
command count 32 and Task capabilities remain unchanged. No data migration is
needed. Future body versions require explicit readers and compatibility tests.

Tool observations do not prove Task completion or permission to run a command.
The later [ADR 071](071_private_file_change_records.md) adds reported file-change
cards through the same ordered collection and shared budget, while preserving
this command body byte for byte. Claude remains tool-free; writable profiles,
affirmative approvals and installed-version qualification retain their separate
requirements.

## Verification

Require parser correlation, replay/conflict, malformed and budget regressions;
private writer round trips for Saved and Workspace attempts; dropped/unknown
append handling; bounded content parsing; and escaped, accessible presentation.
Use contained or in-memory synthetic vendors only. Source review and local
tests do not qualify native interaction or installed-provider behavior.

Executed Windows evidence includes pure protocol/codec and private-writer
tests, an owned in-memory callback-timeout regression, real contained synthetic
vendors through paged private reads and restart, renderer parsing, actual Svelte
server rendering and disclosure-aware Find. Independent bounded protocol/runtime
and UI source reviews passed. Commands and limitations are in the
[execution record](../evidence/native_harness_m1.md#private-command-tool-records-and-cards).

The protocol input is the official pinned
[ThreadItem schema](https://github.com/openai/codex/blob/rust-v0.166.0/codex-rs/app-server-protocol/schema/typescript/v2/ThreadItem.ts)
and [command statuses](https://github.com/openai/codex/blob/rust-v0.166.0/codex-rs/app-server-protocol/schema/typescript/v2/CommandExecutionStatus.ts).
