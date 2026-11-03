# ADR 060: Native conversation export

- Status: Accepted for the pre-alpha native desktop
- Date: 2026-11-02
- Related: [ADR 043](043_tauri_desktop_app.md), [ADR 057](057_persistent_harness_sessions.md), [ADR 058](058_writable_chat_ownership.md)

## Context

The desktop can read saved chats and owner-backed Workspace history, but users
need an explicit way to retain a private copy outside the application. Export
was planned in ADR 057. The existing authenticated Control reads already expose
bounded titles, canonical turn/attempt metadata and private record chunks.
Adding a second database reader or a new daemon mutation would duplicate that
boundary.

## Decision

Add the thirtieth fixed native command, `chat_export_request`. The renderer sends
only a registered project key, conversation kind, format and canonical selection:

```json
{
  "project_key": "registered-project-key",
  "request": {
    "kind": "chat",
    "format": "text",
    "chat_id": "11111111-1111-4111-8111-111111111111",
    "generation": 1,
    "expected_version": 4
  }
}
```

`kind` is `chat` or `workspace`; `format` is `text` or `json`. Unknown fields,
including destination paths, titles, body text and replacement records, are
rejected by the typed native invoke decoder. The response contains only
saved/cancelled status, the same selection, record/byte counts and `partial`.
It contains no destination or private body.

Native Rust resolves the registered project and opens the existing Windows
Save dialog, with a generic `conversation.txt` or `conversation.json` filename.
Cancellation returns before connecting to Control or reading private history.
The selected path remains in Rust. An application-wide owned operation guard excludes
duplicate dialogs while leaving project daemon jobs and conversation Cancel
available. The dialog uses the existing pinned `rfd` dependency, whose
[Save API](https://docs.rs/rfd/0.17.2/rfd/struct.FileDialog.html#method.save_file)
returns the native selection.

## Snapshot and artifact contract

Collection requires the selected generation/version, no active request and a
Ready, Suspended, Closed or Archived session. It reads through the authenticated
Control client, compares complete session metadata after collection, and checks
it again after formatting immediately before file creation. A changed session
aborts the export. This defines a captured copy; it grants no continuing read
authority and makes no claim that future turns or purges cannot occur.

Turn and record pagination must progress, return stable canonical identities
and preserve all record metadata through the captured cutoff. Global sequence
numbers need not be consecutive. Private chunks use exact UTF-8 byte offsets;
an unavailable later chunk discards that record's previously collected body
and produces an explicit error placeholder. No record is silently omitted or
body silently clipped. Truncation, private-content errors, purge and incomplete
turn output make `partial` true. Outcome, terminal correlation and output
completeness remain independent facts.

JSON uses export schema version 1 and carries capture time, conversation kind,
the public session projection, cutoff, optional private title, canonical
turn/attempt Task/Run correlations and records with explicit availability and
truncation facts. It preserves copied UTF-8 text exactly. Text renders the same
facts for reading and makes terminal and bidi control characters visible.
Neither format includes internal blob references, fingerprints, vendor session
IDs, profiles, fences or provider values. Both are private user-selected copies;
neither is an import format, a resume receipt or an execution capability.

Limits reuse the stored-history quotas: 4096 records, 65,536 turns, 256 KiB
retention per record, 32 MiB retained content and a 512-byte title. Original byte
counts may exceed per-record retention and remain visible. Collection has a
120-second deadline. Checked output growth is capped at 343,937,024 bytes to
cover JSON escaping and bounded metadata without clipping.

## Destination ownership and failure semantics

The native-selected destination must be an ordinary absolute Windows drive or
UNC path with the selected extension. Devices, alternate data streams,
ambiguous names, relative traversal, symlinks and reparse points are refused.
Every checked ancestor directory stays open without write or delete sharing
through exclusive leaf creation, writing and syncing. This also refuses an
existing writable directory handle that could mutate a checked ancestor into
a reparse point. Leaf `OPEN_REPARSE_POINT` alone would not protect traversal
through changed ancestors.

The file uses `create_new`, `write_all` and `sync_all`. Existing files are never
overwritten. A write/sync failure may leave a partial new file, which is retained
for the user to inspect. Errors contain fixed codes and messages. An unknown
response does not cause an automatic retry; users should check their selected
destination before another explicit export. Export makes no canonical state or
audit-event mutation. Purging application content cannot erase an earlier
exported copy.

## Compatibility and migration

There was no previous export artifact or command. This adds schema-1 files and
the new native command; no storage migration or Control operation is added.
Control remains v14 and store schema remains 13, with legacy operation minima
unchanged. Matching native assets and command permissions must ship together.
The pre-alpha does not promise import or long-term artifact compatibility;
future incompatible export changes require a new export schema version.

Native selection and writing are Windows-only in this implementation. Other
platforms return the explicit unsupported error. Exported files inherit the
user-selected directory's filesystem policy; protected application storage is
not a claim about that destination.

## Verification and remaining limits

Collector, formatter, file, invoke and UI tests cover bounds, identity changes,
UTF-8 cursors, partial histories, private-data redaction, cancellation and
overwrite/path refusal. A Windows handle regression reproduces and closes the
ancestor write-sharing race. Real authenticated daemon acceptance checks export
saved chats in both formats and owner-backed Workspace history to new files,
using contained synthetic vendor processes. Exact commands and outcomes are in
the [execution evidence](../evidence/native_harness_m1.md#native-conversation-export).

Those engineering checks remain separate from native acceptance. A subsequent
bounded Windows 11 fixture batch selected JSON in the native Save dialog and
saved a Workspace export: schema 1, one completed turn, two complete records,
2,065 bytes and `partial=false`. Independent artifact checks matched its QA plan
and canonical identities. Saved/chat JSON save, Linux/WSL, Windows 10,
installed-vendor policy qualification and full desktop completion remain
unverified. See the [native acceptance record](../evidence/native_harness_m1.md#native-task-window-and-workspace-json-acceptance).
