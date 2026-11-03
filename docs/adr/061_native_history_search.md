# ADR 061: Project conversation search and result navigation

- Status: Accepted for the pre-alpha native desktop
- Date: 2026-11-02
- Related: [ADR 043](043_tauri_desktop_app.md), [ADR 057](057_persistent_harness_sessions.md), [ADR 058](058_writable_chat_ownership.md), [ADR 060](060_native_conversation_export.md)

## Context

Find in transcript can inspect the currently loaded text, whose display is
bounded. Users also need to locate retained titles and messages across their
selected project's Saved chats and Workspaces. Search must use the existing
authenticated private reads, preserve gaps and source changes, and open the
matching canonical record without starting or resuming work.

## Decision

Add the thirty-first fixed native command, `chat_search_request`. Its closed
arguments contain a registered project key and:

```json
{
  "query": "literal text",
  "case_sensitive": false,
  "scope": "all",
  "include_archived": false,
  "limit": 25,
  "cursor": null
}
```

Scope is `all`, `saved` or `workspace`. The archive option includes active plus
archived Saved chats and active plus finalized Workspace owners. Saved discovery
already excludes owner-backed chats in the authoritative producer query, so the
two sources do not duplicate a conversation. The renderer supplies no path,
database, provider, endpoint or replacement history. Rust resolves the registered
project before and after the read.

The native-only Search history dialog reads only on explicit Search or Load
more. Opening, typing, option changes and rendering do not search. It clears
query text, titles, previews and cursors on close or project change, suppresses
old-epoch replies, and keeps one pending request through navigation. An owned
atomic native guard excludes duplicate searches independently of project jobs;
no mutex is held through async history reads, and conversation Cancel remains
available. Search creates no index, database mutation, model request or vendor
process and starts no daemon implicitly.

## Shared reader and matching

`vibemux_client::chat_history::HistoryClient` owns authenticated normalization of
Saved and Workspace status, title, record and turn/attempt reads. It reuses the
existing Control operations. Export's `ExportKind` and `ExportTurn` remain public
aliases, with their schema-1 fields and serialization unchanged. Export retains
its own quiescent snapshot policy; search may read committed retained history
while a conversation is active and does not inspect its live replacement text.

Queries are literal, with a 256-byte UTF-8 limit. The matcher escapes query syntax
and uses the already locked `regex` 1.13.1 package. Case-insensitive matching uses
[Unicode simple case folding](https://github.com/rust-lang/regex/blob/master/UNICODE.md),
without normalization or multi-code-point expansions. Returned offsets refer to
the original UTF-8 text. One hit represents a matching title or record, carries
its occurrence count, and previews the first match with at most 80 Unicode
scalar values of context on each side. Previews render as escaped text and
`mark`, without Markdown, HTML, URL execution or raw content logs.

The direct dependency adds one edge to the existing locked graph; it adds no
package or version. Its pinned manifest declares MIT OR Apache-2.0 and Rust 1.65,
within the workspace's Rust 1.92 toolchain. The workspace dependency enables
`std`, `perf` and `unicode-case` without unrelated default features.

## Continuation and consistency

Each scan page is bounded by 25 hits, 4 MiB of returned private text, 256 records, 128
conversation visits, a 30-second scan deadline and a 256 KiB encoded response.
The dialog displays at most 250 hits and then asks for a refined query. Connection
timeouts remain owned by the existing Control client.

The schema-1 stateless cursor binds the exact query/options, source kind,
conversation, generation/version, record cutoff/count, completed-record count,
last completed sequence and title progress. It is a read position, not authority.
Global sequences need not be consecutive. Continuation completes whole records;
retained body chunks must have exact UTF-8 offsets and stable metadata. A later
unavailable chunk discards the entire copied prefix for that record. Reaching
the cutoff requires the exact canonical record count.

The complete session projection is checked before and after the current page's
reads for a conversation. A changed source loses its newly collected previews
and is counted as a gap. Purged conversations and unavailable titles/records are
also counted. `complete` means no continuation remains; gap counts distinguish
an exhaustive available scan from unavailable history. Search is a per-page,
per-conversation observation, not an atomic project snapshot. Earlier results
can become stale; a new search observes newer history.

## Opening a result

Opening a hit performs existing project-bound reads only. Saved results can
resolve a chat outside the first listed page, including an archived chat. Status
must match its chat/generation/version, and the project projection must agree.
Workspace navigation obtains the actual validated owner before selecting its
execution history. Record hits start at `sequence - 1`, require the exact record
ID and sequence to load without content error, and revalidate final status.
Title hits open the normal conversation beginning.

Drafts, attachments and exact uncertain mutation payloads remain retained.
Opening a result issues no Send, Start, Resume, Cancel, consent or owner mutation.
Deep history explicitly says earlier records are not loaded and retains the
normal display caps. The selected archived result is temporarily pinned in the
sidebar through polling; ordinary navigation releases that pin.

Reveal uses the nearest marked transcript scroll container, shared with Find,
instead of scrolling all ancestors. Native testing exposed header clipping in
the earlier call. A real browser regression preserves the header and outer scroll
while centering the selected record. A renewed native check also retained the
header after Saved, archived and Workspace deep navigation.

## Compatibility and verification

Control remains v14, store schema remains 13, and no daemon operation or
migration is added. Matching native assets, typed commands and permissions must
ship together. The result/cursor schema is new and versioned independently;
the pre-alpha makes no durable cursor or index compatibility promise.

Pure matcher, paging/privacy and UI/navigation tests cover bounds, literal and
Unicode behavior, incomplete histories, stale results, exact navigation and
preserved unsent/uncertain state. Actual Tauri invokes exercise closed arguments;
real authenticated daemon fixtures search Saved and Workspace history, including
continuation, with no canonical mutation. Source reviews remain distinct from
executed checks. See the [execution evidence](../evidence/native_harness_m1.md#project-conversation-search).

Bounded native Search and deep navigation into Saved, archived and Workspace
records have been observed. The renewed native check retained the header after
each deep open. Linux/WSL, Windows 10 and full desktop release acceptance remain
separate. This feature grants no writable-profile qualification
or installed-vendor diagnostic allowance.

### Retained transcript Find follow-on

The shell's header, shortcut and palette now share a selected-transcript identity
gate for Saved, Workspace and retained Asks. Workspace eligibility uses the
current registered project, owner, execution chat and unpurged session, without
requiring an active process or an Open owner phase. Optional Find props reach
the actual Workspace execution transcript, and target-keyed teardown clears
old ranges/highlights. Search still uses the existing bounded displayed-text
matcher and selected-range reveal helper.

Owner context and consent information scroll with their corresponding history;
the bounded inner transcript owns movement. Inactive retained drafts use an
expandable composer with user-owned expansion and controller-owned data. These
presentation changes add no daemon operation, native command or migration.
Native observation and browser red/green results are recorded in the
[retained Find repair evidence](../evidence/native_harness_m1.md#retained-find-and-history-layout-repair).
