# Native harness conversations

This guide covers the related native conversation surfaces.

- **Chats (ADR 057, the default).** A persistent, daemon-owned conversation
  that survives a restart, keeps private history, and admits one canonical
  Task/Run per turn. Bounded native navigation, settings and draft checks
  passed. Both pinned vendors passed bounded follow-up and restart-resume
  checks. Full native saved-history interaction remains open.
- **Independent Ask (M1).** The one-turn path, retained unchanged. One
  prompt, one independent attempt, bounded display history.
- **Workspaces (ADR 058).** Create and inspect lifetime chat workspaces with
  explicit consent, receipt reconciliation and file-retaining Stop. Owner-backed
  conversations now have explicit Start/Resume, Send, Cancel and Close controls
  with private transcripts and text attachments. Tool grants remain empty.
  Native acceptance of that execution path passed with synthetic vendors;
  live-vendor and interactive OS-UI qualification remain open. A human tool
  approval surface now exists under Control v14, while the production approval
  flow with writable profiles is still pending.
- **History search (ADR 061).** A project-wide search over retained
  conversations, covering saved chats and workspaces, with an explicit search
  and an explicit load-more, and with opening a result that selects the exact
  matching record without sending anything.

The Conversation view opens on **Chats**. Independent Ask remains available as
a separate, explicitly chosen path.

The Ask execution path is:

```text
Desktop Svelte presentation
  -> fixed typed Tauri commands
  -> vibemux_client Ask
  -> authenticated ControlClient
  -> vibemuxd harness dispatch
  -> contained vendor process
```

The persistent chat path is:

```text
Desktop Svelte presentation
  -> fixed typed Tauri commands (chat_confirm, chat_request)
  -> authenticated typed Control client
  -> vibemuxd chat service
  -> contained vendor process held across turns
```

The [acceptance record](evidence/native_harness_m1.md) separates implemented
source, automated tests, native Windows interaction and real vendor inference.
The persistent-session decision is recorded in
[ADR 057](adr/057_persistent_harness_sessions.md), which accepts a read-only
scope and defers writable worktrees and affirmative tool approvals.

## Build and launch locally

Use the checked-in Rust toolchain and UI lockfile. The UI must be built first:
`crates/vibemux_desktop/build.rs` requires `ui/dist/index.html`.

```powershell
Set-Location crates/vibemux_desktop/ui
npm ci
npm run typecheck
npm run build
npm test
Set-Location ../../..
cargo build -p vibemux_desktop -p vibemux_cli -p vibemuxd
cargo run -p vibemux_desktop
```

`npm test` includes the production-asset guard and therefore needs the built
`dist` on a fresh checkout. Build first, then run all tests before Cargo.

The default build embeds the bundled assets. The daemon lifecycle helper needs
`vibemuxctl`, and dispatch needs `vibemuxd` and
`vibemux_launch_trampoline`, from the same build. Do not substitute the
standalone `vibemux_harness_native` executable: that path has no daemon
admission or canonical Task/Run binding.

The supported user journey targets native Windows and WebView2, without WSL.
Compilation on one Windows build does not establish interaction acceptance on
another. See the exact OS/build and outstanding interaction checks in the
acceptance record. Installing dependencies, credentials or WebView2 remains
an operator setup action.

The persistent-chat path adds no new build requirement and no new
configuration file. It uses the same `vibemuxd`, `vibemuxctl`, and
`vibemux_launch_trampoline` binaries and the same
`.vibemux/harness_dispatch.json` routes. The read-only chat operation requires
Control v11. The current source uses Control v16 and store schema 14, so the daemon and
the desktop app must be upgraded
together; an older daemon that does not understand the migrated database
refuses it.

## Open a project and send an Ask

1. Pick a folder with the native folder picker or select a registered project.
   Cancelling the picker changes nothing. Rust resolves a registered opaque
   handle and verifies the folder identity; the renderer cannot submit a path.
2. Inspect the project's daemon state and the Claude Code/Codex availability
   reasons. Starting the app, refreshing targets or changing presentation does
   not start a daemon, configure routes or submit a prompt.
3. Start the daemon explicitly when needed. Use explicit route setup when the
   configuration is absent. Setup requires the authenticated daemon state to be
   stopped; otherwise it returns `daemon_stop_required`. Stop an already
   running daemon explicitly before setup. Setup then performs discovery,
   starts the initially unconfigured daemon to record detection, validates
   and creates routes, and reloads that daemon. Existing
   `harness_dispatch.json` is preserved (`ask_config_exists`) before entering
   the shared setup helper. App-owned active or uncertain Asks block setup
   and Forget. Explicit Stop remains a deliberate lifecycle action and can
   interrupt active work after the UI's confirmation.
4. Accept the per-project inference consent. The consent explains that the
   installed harness may consume subscription or paid inference. Consent and
   project registration are Rust-owned; a settings patch cannot create them.
   Legacy path-only consent must be confirmed again for the registered
   directory identity.
5. Select an available Claude Code or Codex route and enter bounded text.
   Empty/whitespace input, more than 32 KiB UTF-8, unsupported targets, malformed
   correlation IDs and escaped Control frames beyond the transport limit are
   refused. Prompt text is protocol/stdin data, never a command string.
6. Send. A local receipt means the desktop retained the submission; it is not
   proof of inference or canonical admission. Confirmed daemon admission adds
   the daemon's request/Task/Run correlation. The text and lifecycle then update
   independently until an authoritative terminal outcome or explicit
   observation uncertainty is available.
7. Stop requests cancellation. Keep `Cancelling` or unresolved state visible
   until daemon evidence settles it. Selecting another Ask, changing visual
   style, hiding a panel or minimizing the window does not cancel the work.
8. A subsequent explicit Send creates another independent Ask. Retry of an
   uncertain submission reconciles its original key and request ID; it never
   silently creates a replacement attempt.

Lifecycle jobs serialize inside this desktop process. There is no new global
cross-client resource lock or Control operation: another same-user client can
act independently, and daemon admission/reservations remain authoritative.
The stopped-state setup gate prevents implicit takeover of an existing daemon;
it does not claim atomic exclusion against another control client.

The two presentation choices control appearance only. Their selected project,
route, editable draft and active Ask ownership are the same. Native availability
comes from the backend, not `navigator.onLine` or the browser app's model list.

The exact capability labels are:

- **Claude Code: tool-free, one turn.** It does not offer arbitrary repository
  inspection through the current profile. ADR 029 documents the remaining
  vendor context-discovery and standalone-login qualifications after removing
  `--bare`.
- **Codex: configured read-only, one turn.** The existing profile is ephemeral,
  with no approval escalation. Process containment and a worktree are distinct
  from a filesystem/network sandbox.
- **Each Send starts a new independent Ask.** UI history does not give the
  vendor shared conversation memory.

Attachments, unsupported model/thinking controls, writable coding, tool
approvals and active HTML/SVG previews are disabled in native M1. Browser
attachments stay in their separate browser conversation and are never
converted into paths or silently inserted into native prompts. Unrelated
unfinished desktop actions remain visibly unavailable.

## Persistent conversations (ADR 057, read-only scope)

A persistent chat is a daemon-owned session: it survives a daemon restart,
keeps private history, and admits one canonical Task/Run per turn. The
Conversation view opens on Chats; the one-turn Independent Ask above is
retained unchanged as a separate choice.

### Setup

1. Open the project. The daemon must be running; a stopped daemon reports
   `chat_daemon_unavailable` and the desktop never starts one implicitly.
2. Accept the chat's private-storage consent with `chat_confirm`. This is a
   separate grant from the one-turn Ask consent. Registering the project is an
   identity boundary, not a consent of its own: it binds the app to the
   selected directory and carries no separate consent grant. Opening a chat
   without the chat consent is refused.
3. Open the chat for the Claude Code or Codex route you want. The daemon
   launches the vendor process under a read-only profile and binds the exact
   vendor session or thread id it returns.

The daemon reads the same `.vibemux/harness_dispatch.json` routes as the
one-shot path and applies the same detection and execution gates. A route
without `allow_execution`, an undetected harness, or a mismatched protocol
refuses the chat. No separate configuration file exists for persistent chats.

The chat route is chosen independently of the Ask picker. A chat target does
not follow whatever the Ask composer currently names, and changing the Ask
target does not redirect a chat.

### Using a chat

1. **Start a new chat.** New chat creates a local draft. Review and accept the
   saved-chat consent, which covers inference and private storage together.
   This consent is separate from Independent Ask consent. Send opens the
   vendor session and submits the draft.
2. **Send.** A new send first waits for the chat to become ready, bounded at
   **30 seconds**, polling status only with a **250 ms** yielded interval. It
   then sends exactly one turn. The readiness wait never sends input and never
   starts a second turn.
3. **Read the history.** Stored records are read by paging; live text arrives
   in bounded chunks with a replacement revision bound to the cursor. Copy,
   find, and code-artifact actions work on the retrieved text.
4. **Manage the chat.** Rename, close, archive, resume, and purge are
   available per chat. Saved chats are listed per project, with **Show
   archived** and **Load more chats** for the older pages.
5. **Resume explicitly.** Resume is an explicit action on a known chat, not
   something a reload triggers. A chat being resumed shows as opening while
   the desktop polls until it settles.
6. **Recover an uncertain action.** See "Recovering an uncertain action"
   below.

Project lifecycle actions act on the project they belong to. A late response
for a chat you already navigated away from preserves the selected chat's
details and any unsent draft rather than replacing them with the arriving
chat.

### Behavior

- **The chat is a session, not a Task.** It has its own states — opening,
  ready, busy, suspended, recovery-required, closed, archived — and never
  becomes completed because one turn finished.
- **Each turn gets its own Task and Run.** The turn records the Git HEAD
  observed when it was admitted. A chat does not own a single lifetime
  Task/Run pair.
- **Resuming is exact.** A Claude chat resumes by its exact stored vendor
  session id; a Codex chat resumes its exact stored thread id. A resume is
  refused when the operator route profile or the workspace resource key
  changed since the chat was opened.
- **Startup, listing, and status are read-only.** The daemon creates no
  private storage and launches no vendor process at startup, and listing or
  reading a chat's status never starts one. Only explicit private reads
  return text.
- **Private history is protected.** Titles, prompts, and assistant output are
  stored as immutable private bodies behind an opaque blob reference, a
  digest, and a byte count. The reference is never a filesystem path, and
  the body is published before the reference that names it commits.
- **Reads are bounded.** A stored record is read through a cursor that names
  the record after a sequence plus a byte offset inside it; a live read also
  binds a replacement revision. Text arrives in chunks of at most 8,192 bytes.
- **Replay is idempotent.** A repeated send after a restart, or after the
  chat's private content was purged, returns the original receipt instead of
  starting a second attempt. A purge never recreates content it removed.
- **Close reserves before it stops.** A close records its operation before
  the vendor process owner is stopped, so an interrupted close cannot leave a
  chat that no operation id can address.

### Archive, purge, and what they leave behind

- **Archive** stops accepting new turns for that chat and keeps its history
  and its canonical metadata. Archived chats stay out of the default list
  until you choose **Show archived**.
- **Purge** removes the chat's **private bodies** — the stored titles, prompts,
  and assistant output. The chat's **canonical metadata** is retained: its
  identity, generations, turn and Task/Run links, and the receipts that prove
  which turn ran. A purged chat therefore still shows what happened, but it
  no longer holds the text.
- **Vendor-side history is separate.** Purging removes VibeMux's local private
  content. It does not erase the vendor's own session or thread files, any
  provider-side retention, backups, or content you already exported. Deleting
  vendor-side history needs its own supported vendor operation.
- **Closing** stops the chat's process and stops accepting turns; it is not a
  deletion and it does not purge anything.
- **An exported file is a private copy.** Purging removes the stored private
  bodies, but it cannot remove a file you already exported. An export is not
  importable state and cannot resume the conversation.

### Recovering an uncertain action

When an action's outcome is uncertain — the daemon did not confirm it — the
desktop keeps the **exact identifiers** involved and offers two ways forward:

- a **read-only lookup** of the current state, which changes nothing; or
- an **explicit retry of the same action**, using the same identifiers.

The desktop never auto-replays an uncertain action. A lost response, a
reloaded view, or a restored draft is not a reason to send again. Recover the
state first, then decide. An unsent draft is retained across the recovery, so
a text you had not yet sent is not lost while you investigate.

### Limits

- **Read-only.** Claude is tool-free; Codex is configured read-only with approval
  requests denied. Command observations display what Codex reports and add no
  execution authority. The profile keeps its existing posture, including
  `--restricted` and `--strict-mcp-config` for Claude.
- **No chat-owned worktree.** A chat runs in the project workspace under the
  daemon's containment. Writable chats, chat-owned branches, affirmative tool grants, and
  the allow-once/deny approval contract are a separate future design that
  needs its own accepted decision.
- **One live vendor process per chat**, with the daemon owning it. The chat
  process is contained, not sandboxed.
- **Attachments stay unsupported.** A browser attachment is never converted
  into a path or silently inserted into a chat prompt.
- The native shell and draft interactions have bounded Windows evidence. Saved-history and live persistent-vendor qualification remain open.

### Verification status

The integrated desktop UI passed 53 tests, typechecking and a production build. The Windows desktop, daemon and CLI binaries build successfully. Authenticated native fixtures verify repeated turns, exact resume, private-history reload, duplicate handling, cancellation and cleanup. The [acceptance record](evidence/native_harness_m1.md#2026-11-01-continuation-evidence) contains exact commands and outcomes.

The approved native check verified Settings, Tasks, Console, Chats and Independent Ask navigation, Light/Dark rendering and restoration, project selection, and bilingual draft retention and clearing. The daemon stayed stopped throughout that UI check. Saved-history interaction against a running vendor, IME composition, DPI variants, Windows 10, Linux/WSL and live persistent-vendor qualification remain open.

## Receipts, output and uncertainty

### Streaming assistant text

Saved and Workspace chats keep their private assistant stream while ordinary
Tool, User, Notice or Approval records become durable. A fully read, error-free
assistant record for the same canonical turn or attempt takes over. Partial,
unavailable or other-turn assistant records preserve the last good live text
when a subsequent read fails. Once the matching assistant body is loaded, a
stale successful Live reply cannot restore a duplicate streaming row.

Existing project/chat, generation, active-request, empty-Live and purge resets
still discard obsolete private output. Polling and failed reads do not issue
Send, Start, Resume or automatic mutation retries. This is assistant-display
handoff; live tool-progress capture remains separate future work.
This display handoff does not change the turn's or Task's canonical outcome.

See the [handoff evidence](evidence/native_harness_m1.md#live-assistant-handoff-repair).

The desktop reserves a dispatch request ID before launching its Ask worker.
A submission key binds atomically to project identity, harness, prompt
fingerprint and request ID. Repeating the same binding returns the same Ask;
changed reuse is a conflict. Canonical Task/Run IDs always come from the daemon.

Submission mappings are distinct from display rows. The desktop process keeps
at most **1,024** deduplication bindings; reaching the limit refuses new sends
with `ask_dedup_capacity`. Removing an old settled display row cannot make its
submission key launchable again. This budget and its protection last only for
the desktop process. It is not durable exactly-once delivery after restart.
The daemon retains its own admission/idempotency rules independently.

For each Ask, distinguish:

| Fact | Meaning |
|---|---|
| Local receipt | Rust retained the validated submission and its request ID. |
| Confirmed admission | The daemon returned or reconciled canonical identity. |
| Last canonical phase/outcome | Last authoritative daemon observation; it is not replaced by a local transport error. |
| Canonical terminal observed | The daemon confirmed a terminal attempt; `recovery_pending` remains unresolved ownership. |
| Observer state/error | Whether the desktop is still following, stopped, timed out or lost contact. |
| Reply availability/completeness | Whether private output can be read and whether all retained records were delivered. |
| Display truncation | Text exceeded the bounded display snapshot; distinct from failed execution. |

A completed canonical attempt can have unavailable output after daemon restart
or eviction. A local timeout or failed best-effort cancel does not prove the
vendor stopped. Partial text, EOF, idle, process exit or a rendered answer alone
cannot establish `Completed`.

Rust continuously drains each owned worker's bounded update channel, even with
no visible webview. It retains the latest bounded snapshot. Content-free events
carry identity, phase and a monotonic revision; the UI fetches text through
`ask_reply`, replaces cumulative snapshots and rejects stale revisions. Record
fragments are reassembled before assistant-text extraction.

## State ownership and retention

| Owner | State |
|---|---|
| Renderer | Appearance, layout, selection, unsent draft and rendering state. |
| Standalone browser app | Browser history/attachments/artifacts and its memory-only Anthropic key. |
| Desktop Rust | Registered handles and directory identities, consent, submission bindings, owned Ask workers/collectors and bounded display snapshots. |
| Daemon | Authentication, admission, canonical Task/Run/Event transitions, reservations, cancellation, deadlines and contained execution. |
| Vendor protocol under daemon ownership | The current one-turn protocol state. |
| Daemon chat service (ADR 057) | Persistent chat sessions, per-turn canonical Task/Run, private history, generation fences and exact vendor resume. Read-only scope. |

The display history is capped at **32 Asks per project**. Only the oldest
settled display entries can be evicted. Active, pending-admission and uncertain
ownership is retained; reaching a limit must not erase it to admit more work.
Replies use the existing **256 KiB** assistant-text display bound. Native prompt
and reply text is not stored in browser `localStorage` by default.

| Event | What survives |
|---|---|
| Webview reload | Desktop Rust workers, submission bindings and retained snapshots survive. The UI rediscovers bounded Ask IDs and rereads snapshots without submitting again. Unsent renderer-only drafts do not survive a reload. |
| Desktop process exit | Registered projects/settings persist. Consent is restored only when its stored directory identity receipt still matches; filesystems without that receipt require confirmation again. Native display text and local deduplication bindings do not persist. Canonical daemon attempts may remain; a lost mapping never authorizes replay of an old draft. |
| Daemon restart | Canonical admission/status records persist under existing recovery rules. Memory-only raw output is lost. Running attempts may become `recovery_pending`; opening the UI never acknowledges them or releases their reservation. A still-running desktop may retain its already-collected display snapshot, labeled with current observation status. |

App shutdown requests cancellation for app-owned active Asks and keeps draining
for a **3-second** reconciliation period. It then stops observers and closes
receivers as needed to unblock terminal delivery, with a **5-second** bounded
join budget and no registry lock held during joins. Unreconciled terminal state
remains unknown. The daemon's own reservation, deadline and process ownership
continue to govern execution; the app never kills an unrelated daemon or
process by name/PID.

## Command and privacy boundary

The Rust definitions are in `crates/vibemux_desktop/src/ui_contract.rs` and
`command_arguments.rs`; the mirrored TypeScript `CommandMap` and `EventMap`
are in `ui/src/lib/ui_contract.ts`. Invoke payloads use these exact flat
`snake_case` keys. Rust deserializes the complete body to a closed argument DTO,
so extra keys are rejected. Frontend types assist callers but grant no authority.

| Command | Arguments | Result |
|---|---|---|
| `projects_list` | `{}` | `ProjectRow[]` |
| `project_pick` | `{}` | `ProjectRow` or `null` for user cancellation |
| `project_select`, `project_forget` | `{ project_key }` | `void` |
| `daemon_start`, `daemon_stop` | `{ project_key }` | `void` after the bounded lifecycle action |
| `ask_targets` | `{ project_key }` | `AskTargets` |
| `ask_setup`, `ask_confirm` | `{ project_key }` | `void` |
| `ask_submit` | `{ project_key, harness, prompt, submission_key }` | Rust `AskReceipt` / TypeScript `SubmissionReceipt` |
| `ask_cancel` | `{ ask_id }` | `void`; confirmation is observed through status |
| `ask_status` | `{ ask_id, reconcile?: boolean }` | `AskStatus` |
| `ask_reply` | `{ ask_id }` | `AskReply` |
| `chat_owner_request` | `{ project_key, request }` | Closed `ChatOwnerResponse` |
| `chat_export_request` | `{ project_key, request: { kind, format, chat_id, generation, expected_version } }` | Closed `ChatExportReceipt` |
| `chat_search_request` | `{ project_key, request: { query, case_sensitive, scope, include_archived, limit, cursor } }` | Closed `ChatSearchPage` |
| `workspace_cleanup_request` | `{ request: { operation, registration_key, task_id, run_id, ...operation_fields } }` | Closed target and optional cleanup view; Main only |
| `probe_rerun` | `{ project_key, request: { operation, operation_id } }` | Schema-1 local-check snapshot; Main only |
| `settings_get` | `{}` | `PublicSettings` |
| `settings_set` | `{ patch }` | `PublicSettings` |
| `about_info` | `{}` | `{ app_version, webview_version }` |

The fixed list has 32 entries, including `workspace_cleanup_request`, `chat_confirm`, `chat_request`,
`chat_pick_files`, `chat_owner_request`, `chat_execution_request`,
`chat_approval_request`, `chat_export_request` and `chat_search_request`. The file picker takes `{ project_key }` and returns
`TextAttachment[] | null`. Rust obtains the paths from the native dialog.
`tasks_snapshot` and `console_section` were previously stubs; they now take
the strict `{ project_key, query }` shape and reject anything else. The
remaining existing commands keep their names and arguments.

### Mounted read surfaces and the chat commands

The desktop also exposes the Tasks and Console read surfaces, the settings
page, search, and the shortcut set. Their renderer contracts are fixed and
bounded in the desktop crate:

- `tasks_snapshot` takes a strict `{ project_key, query }` argument and
  returns a task page or a task detail. A query is either a list with an
  optional cursor and a limit of at most 32 tasks, or a detail for one task.
  The result carries the project, the task cards, the runs, events, and
  artifacts of the selected task, and explicit "has more" flags for each of
  the three collections.
- `console_section` takes a strict `{ project_key, query }` argument and
  returns a snapshot of one section. The sections are Overview, Harnesses,
  Workspace, Tasks, Daemon, Sessions, and Decisions. A section returns at most
  64 rows, and only the Decisions section accepts an audit-sequence cursor.
  Every block reports whether it is loaded or unavailable, with a stable code
  and a hint when it is not. A console row may carry a task id, which is the
  only navigation a row offers into a task detail.
- `chat_confirm` and `chat_request` carry the persistent-chat path. `chat_confirm`
  records the private-storage consent for a project; `chat_request` carries a
  typed chat request to the daemon and returns its typed response. The desktop
  boundary around it is blocking and runs the authenticated typed control
  client on its own runtime; a daemon that is not running reports
  `chat_daemon_unavailable`, and an unavailable client runtime reports
  `desktop_runtime_unavailable`. A failed request reports the daemon's own
  error code and tells the operator to check the chat's status before retrying
  a send.

The important DTO shapes are:

- `ProjectRow`: opaque `project_key`, bounded `name`, `daemon`
  (`running | stopped | unreachable`), optional `code`, `missing`, `active`,
  `ask_confirmed`, and bounded `ask_ids`.
- `AskTargets`: `targets` with `harness`, `label`, `available`, optional `reason`;
  `setup_offered` and optional `code`. Harness is exactly `claude | codex`.
- Receipt: `ask_id`, reserved `request_id`, `submission_key`, `duplicate`.
  `duplicate` here reports the desktop binding result, not a new inference.
- `AskStatus`: identity/project/harness, `revision`, local `phase`
  (`submitting | running | cancelling | finished | uncertain | rejected`),
  nullable `canonical`, `terminal_observed`, `observer_state`
  (`observing | stopped | failed`), optional `observer_error`, `output_state`
  (`pending | partial | complete | unavailable | failed`), `reply_available`,
  `delivery_complete`, and `cancel_requested`.
- `canonical`: daemon `request_id`, `task_id`, `run_id`, `protocol`, `phase`,
  optional `sequence`, and optional `error_code`. Canonical phases retain
  `admitted`, `running`, `cancel_requested`, `completed`, `cancelled`, `failed`,
  `unverified` and `recovery_pending`.
- `AskReply`: `ask_id`, `revision`, retained `prompt`, cumulative `reply`,
  `truncated`, and `delivery_complete`. It supplies explicit content recovery;
  none of these texts appears in events or `ask_status`.
- `PublicSettings`: schema version, appearance, text size, density, reduced
  motion, sidebar collapse and start page. A patch can change only the public
  preferences, not schema version, paths, registration or consent.

`ask_changed` carries `{ event, ask_id, phase, revision }`;
`projects_changed` carries only its event tag. The existing task/console events
retain content-free shapes. There is no generic RPC or HTTP bridge.
The task and console read surfaces are mounted with fixed, bounded renderer
contracts instead of placeholders, and settings, search, and the shortcut set
are integrated. Their reads stay content-free. `projects_list` supplies safe
rows and a bounded set of owned Ask IDs for reload
recovery. `ask_status` defaults `reconcile` to `false`; normal polling reads
snapshots only. Startup/webview reload and explicit Refresh request
`reconcile: true` once, which may attach one observation-only collector to a
known request whose collector has stopped. It never submits or cancels, and
cannot create a second concurrent collector for the Ask. `ask_status` and events are content-free. `ask_reply` is the phase-1
vendor-text accessor. Internal settings include project paths, consent and
identity receipts; `settings_get` and `settings_set` return only the public
projection, and patches reject unknown fields.

A command that is absent from the fixed list is denied to the webview. The
list is the single source of truth: the build declares it in the app manifest,
the capability file grants exactly these commands, and a lockdown test checks
all three against each other.

No JavaScript result exposes daemon tokens/descriptors/pipe addresses,
executable paths, argv, environment maps or unrestricted filesystem paths.
The app retains its bundled-assets CSP, navigation restriction,
`withGlobalTauri: false`, prototype freezing and narrow capability manifest.
It adds no renderer shell/filesystem/HTTP/opener plugin. Markdown escapes or
sanitizes active content; code artifacts remain text. Link actions are disabled in this milestone.

The standalone browser API app still sends explicit requests to its fixed
Anthropic origin. It has no native authority. Native mode never silently falls
back to that provider. The mobile monitor remains read-only. The existing egui
frontend remains until its separate parity/removal gate passes.

## If a step is unavailable

Use the reported stable reason to distinguish a stopped/unreachable daemon,
unsupported Control version, missing route configuration, disabled execution,
missing detection, unsupported protocol, busy reservation, missing output or
recovery-required state. Executable setup accepts the established trusted
native-image policy; installing a script shim is not remedied by adding a shell.
Authenticate the standalone vendor CLI through its supported operator flow;
launcher detection does not prove authentication or inference access.

New Claude route setup includes supported authentication, provider and model
environment references already present in the desktop process. It writes names
such as `ANTHROPIC_AUTH_TOKEN` and `ANTHROPIC_BASE_URL`, and the daemon resolves
their values at launch. It keeps an existing route file intact. For an older
operator config, add the required existing names to that Claude route's
`environment_names` and restart the stopped project daemon. Keep authentication
and its configured provider endpoint together. Credential values stay outside
the route file.

Persistent Claude uses the v2 safe-mode profile qualified with CLI 2.1.304.
Its exact builtin-plugin source allowlist retains the tool/MCP denial checks.
Claude v1 chats stay readable, but need a new chat to use v2. Codex's profile
binding is retained.

After an uncertain send, reconcile its existing identity. Do not resend merely
because the UI reloaded or the response disappeared. After the desktop has
lost its mapping, make a new independent attempt only through an explicit new
Send and preserve prior canonical provenance. Recovery acknowledgement and
worktree/record deletion are separate operator actions, outside M1's UI flow.

For a persistent chat, the equivalent reconciliation is the chat's own
submission lookup: it returns the original turn for a request id that the
daemon already admitted, including after a restart. Do not open a second chat
to work around an uncertain send, because that creates a second vendor
session rather than recovering the first one.

## Native text attachments

Use **Attach text** in a persistent chat to select UTF-8 text or code files.
Each selected file appears with its name, byte count, a literal preview, and
a remove button. Files belong to that project's chat draft. Cancel keeps the
current draft, and a selection that finishes after you switch chats is discarded.

The reader accepts up to four files, 16 KiB per file and 24 KiB together. It
strips a leading UTF-8 byte-order mark and rejects NUL bytes, invalid UTF-8,
directories, and observed Windows reparse points. Filenames have a 240-byte
UTF-8 limit. Windows directory handles prevent ancestor rename/replacement
while the selected file is read. Concurrent in-place reparse mutation and
atomic capture of a changing file remain unverified.

Picked content stays in the desktop draft until **Send**. Disk edits after
capture leave that draft copy intact. Sending uses the project's existing
persistent inference/private-storage consent and the read-only chat profile.
The renderer receives only `attachment_id`, `name`, `content`, and `byte_count`.
The native command grants no renderer path-reading or shell capability.

An attached message uses a versioned text envelope. Attachment-only sends are
supported. The entire encoded prompt, including JSON escaping, fits within
32 KiB.

```text
VIBEMUX_TEXT_ATTACHMENTS_V1
{"schema_version":1,"message":"Explain this file","files":[{"name":"example.rs","content":"fn main() {}"}]}
```

A message without attachments retains its original text. Saved history parses
the envelope strictly and displays its contents as literal text. Malformed,
unknown, or oversized envelopes display as ordinary text. The sent envelope
uses the existing private prompt retention and purge rules.

An uncertain Send retains its exact composed payload and identifiers. An
explicit retry uses that captured payload. Confirmation clears only the
matching submitted draft, preserving subsequent edits and file selections.

Image, PDF and binary inputs remain future work. A bounded native Windows 11
check verified selection, literal bilingual preview, tab-switch retention and
removal of a task-created text fixture. Sending attached content to a live
vendor remains unverified.

## Lifetime owner workspace foundation

Control v12 adds a separate `chat_owner` API for a workspace chat. `Open`
requires explicit private-storage and workspace consent, pins the base commit
and profile, and admits an empty tool set. Store schema 11 binds one lifetime
Task/Run to the chat. The daemon persists a workspace plan and creation claim,
creates an independent branch/worktree under `.vibemux/worktrees`, and verifies
the Git inventory and ownership receipt. Its owner phase becomes `Open` while
the canonical Run stays `Preparing`.

The typed requests are `List`, `Open`, `LookupOpen`, `Status`, `Title`,
`Reconcile` and `Stop`. Duplicate operations return their existing receipt.
Restart retains unfinished provisioning for explicit `Reconcile`. A matching
receipt can settle an interrupted creation. An unresolved claim prevents a
second creation and Stop. Known unverified process cleanup requires operator
investigation. Stop marks the preparing Run `Failed` and Task `Cancelled`,
retaining candidate files and the branch for separate checked cleanup.

Default status contains IDs, lifecycle, branch and verification state. The
daemon keeps full paths, ownership tokens, resource/profile bindings and title
content private. `Title` is an explicit content read. The legacy `chat` API
continues to serve read-only chats and rejects owner access with
`chat_owner_control_required`. Historical request frames and read-only schemas
remain accepted, but already-built v11 clients reject a v12 descriptor. Upgrade
the daemon and clients together.

This foundation has native Windows real-Git fixture coverage. The desktop
controls are described below. Writable attempts, affirmative tool execution, approval transport
and success finalization remain open. See [ADR 058](adr/058_writable_chat_ownership.md) and
the [execution record](evidence/native_harness_m1.md#lifetime-owner-workspace-foundation).

### Native workspace controls

Choose **Workspaces** beside **Chats** and **Independent Ask** for the selected
project. **Create workspace** opens a dialog with the harness, optional title
and explicit consent for private title storage and a separate Git branch and
worktree. Select a saved workspace to inspect its title, branch and verification
state. Start or Resume its execution session separately, then Send a message.

**Reconcile** verifies an idle owner's persisted receipt, including provisioning
left unfinished by a restart. Known cleanup uncertainty requires operator
investigation. **Stop** asks for confirmation and retains the files and branch.
Creation and Stop dialogs are bound to the project and owner shown when opened.

An unconfirmed operation retains its exact request through project navigation.
Refresh performs reads. **Retry same action** replays that request, and a later
retry error retains any earlier uncertainty. Loaded list pages remain visible
across refresh, including their continuation cursor. The view retains up to
128 owners per project and releases settled caches for forgotten registrations.
Forget is disabled while known provisioning or unresolved owner operations
remain. Forgetting an idle project removes its app registration and retains
the daemon, saved records, files and branches.

`chat_owner_request` accepts the strict `{ project_key, request }` shape. Rust
resolves the project, validates the closed request and invokes authenticated
Control v12 off the UI thread. The same typed interface handles explicit title
reads. This owner command retains its Control v12 minimum; current builds publish a
Control v16 descriptor and use store schema 14.

Daemon Stop waits for confirmed service and process completion. The CLI has a
900-second observation window, and the desktop wrapper allows 960 seconds.
The window accommodates the daemon's 600-second provisioning drain and other
service cleanup. An expired observation reports an unconfirmed shutdown.
Normal completion returns as soon as the process exits.

The integrated UI passed typechecking, a production build and 85 tests under
Node 22.22.2. The native bridge fixture verifies creation, saved-project reopening,
private title reads and file retention after Stop. An approved Windows interaction
check verified bilingual title entry, both consent gates, cancellation/reset,
focus restoration and tab switching while the project daemon was stopped.
Exact evidence is in the
[execution record](evidence/native_harness_m1.md#native-workspace-controls).

### Owner-backed attempt persistence

Store schema 12 adds a separate attempt and record ledger under each lifetime
owner. Each submission retains its request ID, attempt ID, process generation
and effective grants. The first admission moves the canonical owner Run from
`Preparing` to `Running`, and individual results leave that Run open. Process
and workspace creation have separate claims and cleanup proofs.

The private-writer facade stores prompts and output through the existing
immutable body channel. Exact replay returns the retained receipt. Missing
private text remains unavailable instead of being republished. Startup and
unverified cleanup retain the old generation's reservation until cleanup proof
allows recovery. Historical read-only turns keep their own records and validators.

This persistence stage is database/writer verified. The subsequent public
execution stage below connects the runtime and desktop transcript with empty
owner grants and Control v13. The current daemon source supports the
forward-only schema 12 migration. Older schema-11 daemon binaries refuse that
database. The open native preview was built before this ledger increment and
uses the preceding daemon/CLI build.

The complete store suite passed 190 tests and the final daemon library suite
passed 194, with two ignored. Ten focused attempt tests and six private-writer
tests are included in that evidence. See [ADR 058](adr/058_writable_chat_ownership.md)
and the [execution record](evidence/native_harness_m1.md#owner-backed-attempt-persistence).

### Owner runtime service

`ChatService` now drives owner submissions through the existing supervised
runtime actor. Internal Rust methods cover Start, Send, cancellation, process
closure and private reads. Start validates the pinned profile. Fresh launch and
input check the stored workspace receipt, inventory, branch, resource and HEAD.
Exact Start and Send retries resolve retained bindings first and never repeat
the effect.

The sink binds each protocol turn to its durable attempt, compares the exact
private prompt before a one-use claim, and persists bounded output. Verified
release settles the attempt while retaining its lifetime Run. Unknown cleanup
keeps the fence. Exact pending identity can reconcile cleanup after a lost
claim response without establishing success. Claude permits one first input
per generation for its pinned new or retained UUID; current root/profile proof
is required before acknowledgement and completion.

The service uses empty grants and read-only profiles. The public execution
stage below adds Control v13 without another schema migration. Writable profiles,
affirmative approvals and success finalization remain open.
See the [execution record](evidence/native_harness_m1.md#owner-runtime-service).

### Workspace conversations

With a source-matched daemon and desktop build, select **Workspaces**, choose a
prepared owner, and open its conversation. **Start…** or **Resume…**
asks for execution/private-storage consent and records the native project consent
before calling the daemon. Start does not send the draft. Once the session is
ready, **Send** admits one attempt bound to its generation and owner version.
The current profiles admit no tools; workspace ownership does not enable edits
or command approvals.

Text attachments use the same native picker and literal prompt envelope as saved
chats. The combined prompt stays within 32 KiB of UTF-8 text. The transcript loads
private records in bounded chunks, shows attempt state and live output, and offers
**Load more** for retained history. Switching projects clears active private history;
drafts and unresolved actions remain bound to their original project and owner.

**Cancel** addresses the active request and generation. **Close session** ends that
process generation while retaining the lifetime owner, Task/Run, workspace files
and branch. Close the execution session before **Reconcile** or **Stop workspace**.
Stop finalizes the lifetime record without declaring a successful coding result.
A reload, selection or poll reads state and never starts a process or sends a turn.

An uncertain action keeps its original payload and IDs. **Refresh** uses read-only
receipt/submission queries; **Retry** explicitly replays the same action. A positive
Start receipt confirms the old action even if its process has since exited.
The returned current runtime determines whether Start/Resume or Send is available.
A null lookup retains uncertainty.

The native command is `chat_execution_request({ project_key, request })`, the
28th fixed command. Its closed Control v13 operation supports `status`, `start`,
`lookup_start`, `send`, `submission`, `cancel`, `close`, `attempts`, `content`
and `live`. Only Content and Live return private text. Execution consent is
required for native Start/Send; the renderer cannot choose paths, profiles or grants.
Older clients reject the v13 descriptor, so upgrade clients and daemon together.
The legacy chat/owner operations retain their v11/v12 minima. Store schema 13 adds
the separate approval ledger described below. Detailed execution wire and replay rules are in
[ADR 058](adr/058_writable_chat_ownership.md#control-v13-execution-contract).

Accepted Start and Send actions remain daemon-owned after a client acknowledgement
timeout. The daemon retains their dependent launch or input step, and shutdown
settles the accepted work before releasing its ownership. The existing recovery
controls keep the same request IDs and request shapes.

The mounted UI and public boundary checks are locally verified; the current UI
run passes 99 tests, typechecking and a production build. The native execution
matrix passed 4/4 across both synthetic vendors after the test-only refusal
assertion was corrected to the existing client code contract. Native desktop
acceptance passed one case covering consent denial and acceptance, Start, Send,
the correlated result, private content, replay, Close and the final Stop with
file retention. The daemon library suite passed 211 tests with two ignored, and
Clippy over the daemon, store and desktop crates with all targets, all features
and `-D warnings` passed, as did formatting and the diff check.

That evidence uses synthetic vendors and an automated native acceptance test, so
it does not qualify a live vendor, the whole repository or the interactive OS
UI. Linux/WSL and Windows 10 remain unqualified. A subsequent complete daemon
package run passed 363 tests with three ignored, including both previously failing
workflow compare cases. Their original failure cause remains unproven, and the
earlier results are retained. Existing live
read-only chat results do not certify this owner execution path. Tool grants
stay empty and profiles remain read-only; writable profiles, affirmative approvals and success
finalization remain open.
See the [execution record](evidence/native_harness_m1.md#public-owner-execution).

### Human tool approvals (Control v14)

Control is now v14. The older chat, workspace and execution operations keep
their shapes and their v11, v12 and v13 minima, and a new closed approval
operation requires v14. The daemon, the client and the desktop app are upgraded
together, because an older client refuses the v14 descriptor. The stored
records are unchanged, so this is not a data migration.

In a workspace conversation, a pending tool request appears as an approval card
that opens a review dialog. **Review** fetches the request text into a
read-only box. **Allow once** is offered only when the complete current
request is present and the challenge is still live, decidable and unexpired.
**Deny** can be sent without reading the request text, and it is the control
that holds focus when the dialog opens. Nothing sends a decision for you:
opening, rendering, closing, an expiry and a recovery never choose, and the
immutable request and version stay bound to the choice. Allow once applies
to the unchanged input you reviewed. A recovery pass only reads.

If receipt recovery finds no decision for the saved operation and a newer
final record for that same approval proves it cannot commit, the card reports
the choice as not recorded and never sends again. When
the request is still pending, or the same operation is still in flight, the
card stays uncertain instead of guessing. The request text is cleared whenever
the selection changes, the dialog closes, an error occurs, the challenge
expires, the detail is fetched again or the decision is submitted, and the
ordinary poll waits while you are reviewing. A forced refresh clears the text
and asks for a fresh review. The controller compares the current approval ID and version before applying
metadata, so a delayed response cannot replace newer delivery evidence.

If the local wall clock moves backward during review, elapsed time can no
longer be trusted. The card clears private text, reports that the deadline
needs verification, and blocks choices until fresh server calibration.
The daemon stays the final authority on what is expired.

This surface consumes approvals that the runtime already offers. It does not
yet produce them in production: the default services still deny, the profiles
are still read-only and empty, and writable workspace admission, a narrower
send, the production profile and an installed-vendor qualification are still
pending.

Exact commands are in the
[control v14 and desktop tool approvals record](evidence/native_harness_m1.md#control-v14-and-desktop-tool-approvals).

### Next: interactive approvals (proposed)

[ADR 059](adr/059_interactive_chat_approvals.md) is proposed; the approval
surface described above is a partial consumer, and the remaining scope below is
still open. It
sketches the first candidate approval model: Claude `manual` permission mode with `host` prompts and four independent
file-tool grants (read, search, edit and write). It records that RunDevTests is unsupported and
that Codex workspace-write stays uncertified, and it defines schema 13 and
Control v14. That approval increment introduced schema 13; current builds use
schema 14 for the separate cleanup ledger. The
sections below describe the durable ledger and the remaining scope rather than
shipped behavior.

The groundwork is a private receipt for a submitted input. The daemon queues a
tracked input once and reports it as written only after the bytes are written
and flushed. An I/O failure, task abort or lost completion sender reports it as
unknown. The plain send path keeps its earlier queue-only behavior and drops
the receipt. No product feature consumes that receipt yet.

A test-only peer fixture exercises the Claude approval request shape. It emits
a root assistant write request, waits for the matching approval, and writes a
fixed artifact only when the request ID matches exactly, the input is
unchanged, no new permissions are requested and the answer allows it. Foreign, duplicate or stale replies do not cause an additional effect.
Denied or changed input, interruption and EOF do not release a pending effect. The fixture runs as a temporary local process rather than the
daemon's contained runtime, and it is not a real vendor, so it proves the
request contract only. It invents no assistant user identity, and native
correlation and ordering stay unqualified.

The durable ledger now exists in the store. A challenge records canonical
approval metadata and a private reference to its evidence, and its decision is
recorded once, as either allow-once or deny. Replaying the same operation
returns the recorded receipt without the capability to deliver it again, and
reusing the same identity with different content is a conflict. The capability
itself stays private: it is not serializable and cannot be copied.

The recorded state is explicit about ordering. Cancelling before the delivery
is claimed invalidates a pending challenge; cancelling afterwards keeps the
recorded decision. Releasing the process or restarting the daemon invalidates a
pending challenge and turns an unobserved claimed challenge into an unknown
one, and nothing is sent again. A terminal success cannot be recorded while a
challenge is pending, claimed or unknown. A challenge expires at most 120
seconds after it was recorded, the boundary is exact, and a later challenge
cannot be backdated behind an earlier tool call or across the chat lifecycle.
The guarded fixture runtime caps approval expiry by the remaining turn deadline
using a monotonic clock.

Each attempt keeps a single outstanding challenge, including one that was
already claimed or became unknown, and the write, edit, read and search grants
are independent. `RunDevTests` and non-Claude requests are refused in this
store slice. Private evidence reuses the existing byte quotas, a chat may hold
at most 4096 challenges, and no raw vendor input reaches the event stream or
the debug output.

The private approval writer now exists in the daemon. It connects protected
immutable private evidence bodies to the ledger through the daemon's single
authoritative writer queue. A replay is answered by an exact read-only lookup
before any private read happens, a new evidence body is published before its
metadata, and a rejected admission removes only the owned body that nothing
references.

The evidence binds the actual attempt, chat, request and generation, the
vendor session, the canonical tool and its native mapping, digests of the raw
request and tool identifiers, and the exact original input. A fresh decision
checks the body and that binding, takes one snapshot before it claims the
challenge, and returns a single redacted dispatch that cannot be copied or
serialized. A historical decision is replayed without reading or rebuilding any
private body and without granting a capability. Missing, altered, out-of-scope,
oversized or changed evidence fails closed. Private text is reachable only
through an explicit content read; the normal status output, the debug output
and the event stream stay content-free.

A test-only observer checks what a denial actually proves. It reports fixed
booleans, counters and errors, keeps raw identifiers and input out of its
output, and bounds the encoded JSON, the input and the pending bytes. A tool
request that matches without the user echo is only an observation; proof needs
that tool's own message to be bound to the exact user echo, an unrelated echo
cannot bind an earlier unbound tool, and the proof snapshot is fixed at the
denial. It runs as a pure test helper, so it made no model or vendor call and
qualified nothing.

The guarded offline runtime now exists behind the test helpers. Its fresh Claude
fixture protocol permits exactly one user input with the `WriteFiles` grant
and has no serialized opt-in. The request, the tool and the input are
correlated exactly, for complete and for streamed responses. The input is
flushed before the challenge becomes pending, the response is prepared once
before the durable claim, and the physical dispatch still requires the
canonical one-use capability.

The actor keeps ownership of its callback futures when the client observer is
lost: it continues output reading and turn cancellation while callbacks wait,
holds back the terminal state
until the durable response is written, never resends a response with Unknown delivery,
and limits the two-minute approval expiry by the original turn deadline while
pausing the idle clock during a human wait. The fixture's fixed effect is a
single new file created only after one matching allow; a denial, a
cancellation, an expiry or an identity failure creates nothing. The factory
pins the actual built vendor and trampoline, the configuration and working
directory, the managed worktree ownership and the HEAD, and it refuses to
enqueue after the HEAD moved.

The separate diagnostic observer reports what a denial proved and has no native
runner. The guarded runtime drives a contained fixture process. The deny-only
qualification diagnostic below has a native path for an installed vendor, but
that path has not been executed and it never carries production authority.

A deny-only qualification diagnostic exists behind the test helpers. It never
grants a permission. Preparing a candidate starts no program and reads no
credentials; it opens a fresh empty namespace and pins the vendor, the built
trampoline and a fixed configuration. A separate native step would check the
exact installed version in its own contained process before one fixed input,
using a manual host profile limited to writing, with restricted settings, strict
MCP configuration, no browser, no slash commands and no session persistence.
The answer is always a fixed denial, and it never changes the input or the
permissions.

The driver uses the normal session machine only for the exact initialize and
generated user frame, and the observer takes over from there. It tracks the
initialize, user and denial flush slots with output, cancellation, root and
deadline handling, the terminal step waits for the receipts it needs, an abort
joins standard input before the contained finish, and a closed cancellation
owner refuses to start. The run is bounded to 15 seconds of setup, 60 seconds
of a turn and 3 seconds of shutdown, with an 8 MiB output cap, and asking for
one turn is a request rather than proof that only one request was made. The
report holds fixed observations, receipt flags, counters and cleanup state
only; it carries no identifiers, paths, raw input or provider values, and it
always reports that neither the policy sources nor the production profile are
qualified. Unknown files and cleanup keep the exact recovery namespace.

The installed-vendor case is ignored by default and needs a separate explicit
allowance plus a surviving-policy review; ordinary test runs use the guarded
built fixture only. At that stage no native vendor run had been performed.

Four focused input/output tests and three peer integration tests passed, and
the daemon library suite passed 215 tests with two ignored after these
changes in the earlier foundation artifact. Eleven new store API tests cover
the ledger's replay, cancellation ordering, expiry, concurrency, restart, grant
narrowing, settlement, clock watermark, corrupt bindings and migration rollback.
The complete current store suite passed 202 tests and the types suite passed 23.
Fresh native Windows regression passed 215 daemon library tests with two
existing ignores and four contained owner-execution cases; affected-package
Clippy and formatting also passed. Eight new writer boundary tests passed, the
store approval tests passed eleven including the new read-only lookup
assertion, and the first full daemon library run passed 223 tests with two
existing ignores before the observer was added. The observer's fifteen pure tests passed after parent and independent review
corrected origin, stream-confirmation and duplicate-block cases, including an
echoed snapshot omitting its streamed tool. The final daemon library run passed
238 tests with two existing ignores. Affected-package Clippy and the rebuilt
desktop/CLI/daemon bundle also passed.

The Windows offline native ten-case matrix passed: allow and deny, cancellation,
parked admission with continued output, a parked claim followed by cancellation,
a delayed durable write, a post-claim same-branch HEAD move, a no-dispatch
unknown, a lost client observer, an idle pause with absolute expiry, and a wrong
vendor or trampoline. The full harness package passed 278 tests
(217 + 12 + 22 + 8 + 15 + 4). The full daemon package passed 404 tests with three existing explicit ignores.
After the lint-only condition/initializer and boxed-guard refinements, the
current source passed the harness 278, daemon library 239 with two existing
ignores, and all ten native approval cases. Affected-package Clippy with
`-D warnings` and the desktop/CLI/daemon bundle build also passed. Nothing here qualifies an installed vendor version, an
effective policy or a writable native profile, and no vendor or model call was
made.

The parent executed eleven injected tests. The full daemon library passed 250
tests with two existing ignores, and the synthetic integration run passed five
with one explicit installed-case ignore, including the closed-owner and
no-start-marker regression. The final driver cases passed again after the lint-only condition refinement;
affected all-target Clippy and the rebuilt desktop/CLI/daemon bundle passed. Read-only host metadata identifies 2.1.304.0; the managed local files and
the four registry views were inspected and are absent, but the remote and
runtime sources remain unqualified. At that stage no installed-Claude diagnostic process or qualification input was
started; these checks used synthetic fixtures, and existing provider values were never
recorded. No new live allowance was granted; the prior budgets stood at 0.

The guarded runtime exists only behind the test helpers, so the app still has
no production approvals: production profiles deny by default, writable profiles
remain unbuilt, and there is no production approval route. The deny-only
diagnostic is a qualification aid that grants no permission and carries no
production authority. A local
native CLI flag parse with no input and an invalid `--max-turns` value
confirmed that the numeric option is recognized; that is option-parsing
evidence only; it does not qualify the native limit. A fresh single-input process is a proposed alternative for origin proof,
with strict closure, no resume, single input and policy gates, and it still
needs independent native qualification; the existing persistent parser is
unchanged.

Exact commands and the distinction between pipe, peer and vendor evidence are in
the [approval foundation record](evidence/native_harness_m1.md#interactive-approval-foundation).
The ledger implementation and its store evidence are in the
[durable chat approval ledger record](evidence/native_harness_m1.md#durable-chat-approval-ledger).
The writer and the diagnostic observer are in the
[private approval writer and diagnostic observer record](evidence/native_harness_m1.md#private-approval-writer-and-diagnostic-observer).
The guarded offline runtime and its ten-case matrix are in the
[guarded fixture approval runtime record](evidence/native_harness_m1.md#guarded-fixture-approval-runtime).
The deny-only qualification diagnostic is in the
[deny-only approval diagnostic record](evidence/native_harness_m1.md#deny-only-approval-diagnostic).
The human approval surface and its evidence are in the
[control v14 and desktop tool approvals record](evidence/native_harness_m1.md#control-v14-and-desktop-tool-approvals).
The installed-vendor run and the redacted init checks are in the
[installed Claude diagnostic record](evidence/native_harness_m1.md#installed-claude-diagnostic-and-redacted-init-checks).

### Explicit installed-vendor diagnostic

The normal test command exercises the guarded synthetic process and skips the
installed-vendor case:

```text
cargo test -p vibemuxd --test approval_diagnostic --features test_helpers --locked --offline
```

The installed case consumes provider resources. Invoke it after a separate
scoped live allowance and surviving-policy review, using the reviewed executable
and a new absolute recovery directory. The directory must not already exist.
Forward only the names of credentials/provider values already present in the
calling process; keep their values out of commands, reports and repository files.
For a process already configured with the token/base-URL provider:

```powershell
$env:VIBEMUX_APPROVAL_PROBE_EXECUTABLE = 'C:\reviewed\claude.exe'
$env:VIBEMUX_APPROVAL_PROBE_ROOT = 'C:\owned\new_probe_run'
$env:VIBEMUX_APPROVAL_PROBE_ENVIRONMENT_NAMES = 'ANTHROPIC_AUTH_TOKEN,ANTHROPIC_BASE_URL'
cargo test -p vibemuxd --test approval_diagnostic --features test_helpers --locked --offline installed_claude_deny_only_diagnostic -- --ignored --exact --nocapture
```

This selects the fixed one-input, deny-only diagnostic; it imports no repository
content. The host sends Deny when a permission callback appears. An earlier
native allow rule may bypass that callback, so the report separately records
whether the synthetic artifact appeared. A green test means that the diagnostic
returned with confirmed cleanup; it does not certify native origin, permissions,
the requested model-step limit or a production profile. Preserve the named
namespace after a failed run or retained-file report, and reconcile that attempt
before another invocation. The current implementation has run this installed
case twice, once per separately approved input; the next two sections record
those results.

### Installed-vendor run result and report limits

On 2026-11-02 the ignored installed-vendor case was run exactly once, after
the explicit one-input allowance was confirmed. The installed Claude 2.1.304
version was verified. The report recorded the initialize and input frames as
written and the denial frame as not written. The observer saw 5 records and
6360 JSON bytes, none of its root-init, input-echo, tool, request or terminal
flags were set, and the error was `init_mismatch`; stop = `protocol_rejected`.
The process root was reaped, its tree was empty, cleanup was
verified, forced termination was used, the exit code was 1 and stderr was
empty. Workspace identity was verified, the fixed synthetic artifact was
absent, the workspace was not retained, and the fresh root was removed.

A passing test means the diagnostic completed and its own cleanup was
verified. It does not qualify a native policy or profile, and it does not
prove that a denial would have been delivered to a real vendor. A false
`input_written` on its own cannot show that no input reached the vendor, and it
does not authorize a retry; `input_queued` records queue acceptance, not
delivery.

The raw output was not logged, so the first report cannot say which
initialization field disagreed. The report is now extended for diagnostics
only: it separates acceptance and queueing from the write/flush receipts
(`initialize_accepted`, `input_queued`, `denial_queued`) and carries
content-free initialization checks for the permission mode, the working
directory and the tool list. A serialized `init_checks` of `null` means no
identity-qualified initialization reached the checks. An object carries all
three booleans, and `false` means the field was compared and mismatched.
The checks are computed only after the root identity and duplicate validation,
and all three are evaluated before any mismatch is rejected. The acceptance
rules are unchanged. The affected fourteen driver tests failed against the
first report in eight of fourteen cases and passed in all fourteen against the
extended report. The full daemon library passed 256 tests with two existing
ignores, the synthetic process diagnostic passed five with the installed case
left ignored, and affected-package Clippy, formatting and diff checks passed.

One provider input was spent for that run and none remained, so the diagnostic
does not retry by itself. Run it again only with a new explicit allowance, a reviewed
executable, a fresh absolute recovery directory and a surviving-policy review,
and reconcile the previous attempt first.

Exact commands are in the
[installed Claude diagnostic record](evidence/native_harness_m1.md#installed-claude-diagnostic-and-redacted-init-checks).

### Installed-vendor report v2 run result

The user separately approved one further input, and the report v2 diagnostic
ran exactly once more with the same existing provider, the installed Claude
2.1.304 version, `--permission-mode manual`, `--permission-prompts host` and
`--tools=Write`, with its fixed input restricted to requesting the synthetic artifact; no artifact was created.

The report now separates acceptance from delivery. The initialize frame was
accepted and written, the input was queued and written, and the denial was
neither queued nor written. The initialization checks compared the permission
mode, the working directory and the tool list: the working directory and the
tools matched, and the permission mode did not, so the run stopped with
`init_mismatch` and stop `protocol_rejected`. The observer accepted no root
init, and it saw no input echo, tool, matching request, denial or terminal
record. It
counted 5 records, 6366 record bytes, 6371 stdout bytes and 297 pending bytes.
The process root was reaped, its tree was empty, cleanup was verified, forced
termination was used, the exit code was 1 and stderr was empty, workspace
identity was verified, the probe artifact was absent, the workspace was not
retained, and the fresh namespace was removed.

A test exit of 0 with one pass means the diagnostic cleanup succeeded. It is
not a profile qualification: the candidate allows a single user input, and
whether an affirmative response is available, whether the policy sources are
qualified and whether the production profile is qualified are all false. The
parser is unchanged, and the mismatching permission-mode value was not logged,
so it cannot be named.

Two installed diagnostic inputs have now been used in total, one per run, and
no installed input remains. A further run needs a new explicit allowance, the
reviewed executable, a fresh absolute recovery directory and a surviving-policy
review, and the previous attempt has to be reconciled first.

Exact commands are in the
[installed Claude diagnostic record](evidence/native_harness_m1.md#installed-claude-diagnostic-and-redacted-init-checks).

### Diagnostic report v3 and the canonical permission mode

The test-only diagnostic report is now version 3. It keeps the version 2 JSON
fields, and `permission_mode_matches` now compares a closed set of spellings
for the same semantic mode. The recorded version 2 booleans keep their
meaning and are not reinterpreted.

The change follows the vendor's public documentation, retrieved on 2026-11-02:

- the CLI reference states that `--permission-mode manual` is an alias for
  `default`, introduced in 2.1.200, and that the help text lists `manual` in
  place of `default`;
- the permission-modes page names the mode "Manual" for the user interface
  and the command line, while the configuration and SDK value is `default`;
- the agent SDK reference defines the permission-mode values as `default`,
  `acceptEdits`, `bypassPermissions`, `plan`, `dontAsk` and `auto`, and types
  the system message permission mode with them.

The exact installed 2.1.304 `--help` was also inspected locally: stdin was
closed, the read had its own 15-second deadline, it exited 0 with empty
stderr, and no prompt or model input was submitted.

The observer previously accepted only the literal string `manual`, in the test
helpers only, and rejected the canonical `default` on its own. A new pure
regression reproduced that as an initialization mismatch before the fix. The
predicate now accepts exactly `manual` and `default` and nothing else, with no
trimming, no case folding and no coercion. The identity, working-directory,
sole Write tool, duplicate, proof and deny-guard checks are unchanged, the
production session machine and its `dontAsk` mode are untouched, the fixture
state machine is untouched, and the launch arguments still request `manual`
with host prompts.

Two clarifications about the recorded results. First, when `init_checks` is
populated, a false `permission_mode_matches` and false `root_init_seen` mean the
initialization reached the comparison and was not accepted; it does not mean
that no initialization was seen. Second, the raw permission-mode value of the
earlier live runs was discarded, so the canonical spelling is a plausible
explanation for those mismatches and not a recorded fact. Both earlier live
reports are unchanged.

The verification is offline. The native daemon library passed 259 tests with
two ignored, the synthetic process diagnostic passed five with the installed
case still ignored, and the affected daemon, harness and desktop packages
passed Clippy with all targets and all features. A first broader approval
subset run inside the sandbox passed 35 and failed eight Windows cases that
depend on private blob handling; the native full suite then passed without
any test being weakened.

Three new tests cover the repair: the alias with a full proof and a closed
deny, the rejection of other and malformed modes, and a duplex driver run in
the canonical default mode with one handshake, one input and one fixed
denial. No installed run has been executed against version 3.

The normal and QA dependency rebuilds and the independent Sol source gate
passed. The unchanged harness protocol subset passed 52 tests. The 2026-11-02
native GUI observation and its open items are recorded in the native acceptance
section below: the renewed batch verified Saved, archived and Workspace deep opens and the
transcript-scroll repair. The JSON file save remains incomplete. The prepared acceptance root is retained. No further installed input
is available: the two earlier runs consumed both approved inputs, none remain
and none is reserved.

Exact commands are in the
[canonical manual-mode diagnostic repair record](evidence/native_harness_m1.md#canonical-manual-mode-diagnostic-repair).

## Conversation export

A saved chat or a workspace can be exported to a file from the native desktop
app. Choose **Export**, pick JSON or text, and confirm the Windows save dialog.
The dialog opens first with a generic name, `conversation.json` or
`conversation.txt`; choose your own folder and name. Cancelling the dialog
reads no history and creates no file.

The export needs the daemon connected and a quiet conversation: the session
must be exactly Ready, Suspended, Closed or Archived, and no request may be in
flight. The conversation's generation and version are pinned when the export
starts, and the same metadata is checked again after the content is collected
and again before the file is written. If the conversation changed in between,
the export is refused rather than mixed.

Export reads through authenticated daemon calls only. It never opens the
database or the private store directly.

Every exported record carries its exact metadata. If some records are
unavailable, purged, truncated or incomplete, the export is still written and
is marked partial, so a partial file is never presented as a complete one.

The JSON file uses export schema 1. It keeps the text exactly as stored, in
UTF-8, together with the canonical task, run and correlation facts. It carries
no vendor identifiers, no private-body references, no digests and no file
paths. The text file is meant to be read: terminal and bidirectional control
characters are shown visibly instead of being acted on, and the file contains
no active markup.

The exported file is a private copy. It is not state you can import, and it
cannot resume a conversation. Purging the conversation later does not remove a
file you already exported.

The bounds are 4096 records, 65536 turns, 256 KiB of retained content per
record, 32 MiB retained in total, a 512-byte title and a 120-second collection
timeout. The output file is capped at 343937024 bytes, which covers escaping
and metadata; content is never silently cut to fit.

The destination must be an ordinary absolute path ending in the extension you
chose. Devices, alternate data streams, reparse points and existing files are
refused, and nothing is overwritten. The ancestors of the destination are held
open while the file is created, so another program holding a writable handle
there can make the save fail; close it and start a new explicit selection. The
file is created new, then written and flushed to disk.

If the write or the flush fails, the file can be left incomplete. When the
result is unconfirmed, check the destination you selected before doing
anything else; the app does not retry on its own. Cancelling stays responsive
because export ownership is separate from the daemon’s per-project job.

The native save dialog was opened in the 2026-11-02 native run: cancelling it
with Escape returned `Export cancelled` and left no file behind. Saving a JSON
file to an exact owned path was not completed, so the native write path is
still unverified.

Exact commands are in the
[native conversation export record](evidence/native_harness_m1.md#native-conversation-export).

## Project conversation search

The native desktop app can search the retained conversations of the project you
are working in. Press **Search history** to open the search panel. It covers
Saved chats and Workspaces. Opening the panel, typing and changing options
read nothing. Use **Search** to run a query and **Load
more** to continue a result set.

The query is plain text of at most 256 bytes, matched literally: the text is
escaped before it is used as a pattern, and case is folded in the simple way
that keeps different scripts apart. There is no normalization and no wider
case-folding expansion, so a match is what you typed rather than a normalized
variant of it.

The scope starts at all conversations. You can narrow it to saved chats or to
workspaces. **Match case** makes the comparison exact, and the checkbox
includes archived and finalized conversations. Archived conversations are
searched but stay hidden from the ordinary lists.

Each result is one matching title or message, with how many times it matched
and a short preview of the text around the first match. The preview shows up to
80 characters on each side of the match, and the offsets are counted in UTF-8
bytes.

A result page holds at most 25 hits. The whole read is bounded as well: 4 MiB
of history, 256 records, 128 visited conversations and 30 seconds, with the
response capped at 256 KiB. When that cap is reached the app shows 250 hits
and asks you to narrow the query instead of returning a partial page.

**Load more** continues the same query. The continuation is bound to the exact
query, options, conversation kind, generation, version and cut-off that
produced it, so it resumes at a whole record and never continues in the middle
of one. A page is complete only when no further page is offered.

Search observes each conversation as it reads it. It is not one snapshot of the
whole project, so a conversation can change between pages. If one changed, its
newer text is dropped from that page and the gap is reported rather than
hidden. Purged conversations and unavailable title or record content are
counted and reported too. Earlier results can therefore go stale, so run the
search again when you need a fresh view.

The query, the titles, the previews and the continuation positions are private.
The panel’s in-memory query and result cache are cleared when you close it or
switch projects. An outstanding bounded read may finish, but its late results
are discarded. Nothing is written to disk and nothing is added to the
default logs.

Opening a result selects the matching conversation and, when you open it, the
exact record that matched. The app re-checks the conversation's status,
generation and version before loading, so a conversation that moved on is not
opened as if it had not. An archived or otherwise unlisted conversation is
resolved and selected for you. A deep record view states that earlier history is not loaded and retains the
normal display caps. If the exact record or version no longer matches, refresh
the search before opening it again.

Opening a result never sends anything. It does not resume or start a
conversation, cancel one, ask for consent or change the owner. Drafts,
attachments and any uncertain pending action are kept as they were.

Exact commands are in the
[project conversation search record](evidence/native_harness_m1.md#project-conversation-search).

## Native GUI QA runner

A nondefault `native_qa` feature builds a separate desktop binary that drives
the production desktop app inside an isolated, prepared namespace. The feature
also pulls in the bundled custom protocol handler and the daemon test helpers,
and is selected only by an explicit feature-enabled build.

Build the two sides:

```powershell
cargo build -p vibemuxd --features test_helpers --bin vibemux_chat_fixture --bin vibemux_launch_trampoline --locked --offline
cargo build -p vibemux_desktop --features native_qa --bin vibemux_desktop_qa --locked --offline
```

The runner takes a subcommand and a root directory:

```text
vibemux_desktop_qa <prepare|prepare_cleanup|prepare_surfaces|check|run|stop> --root <fresh absolute local directory>
vibemux_desktop_qa --help
```

- **prepare** claims an exclusive, fresh root and builds a synthetic-only
  namespace with a real Git repository, a real daemon and three synthetic
  conversations. The baseline is 4 Saved, 2 archived Saved and 2 Workspace
  records across those three conversations, with no process claim.
- **check** starts its own daemon and reopens the production `AppState`, the
  settings and the descriptor, then repeats the history read-only. It joins
  its daemon before it exits.
- **prepare_cleanup** creates the same retained history and finalizes its
  synthetic Workspace owner through file-retaining Stop. It adds one fixed
  first-party empty-inventory terminal peer so the Main removal review can be
  exercised. Build `vibemux_registry_mock` alongside the chat fixture and
  trampoline first. The runner seeds no cleanup Plan or confirmation.
- **prepare_surfaces** retains command and file-change cards in Saved and
  Workspace history: 8 Saved, 2 archived and 4 Workspace records, with an Open
  owner, Closed session and no process claim. A unique late marker,
  `DEMO_SURFACE_FIND_TARGET`, stays inside a complete 300-line diff. Initial
  preparation and later check/run read back bounded private bodies through
  authenticated Control and verify canonical grouping, exact tools, complete
  outcomes and unchanged statuses. No export file or terminal plugin is created.
- **run** starts the shared production Tauri builder and UI with all 32 fixed
  commands, the content security policy and the navigation guard. It uses a
  separate root, profile and `desktop_settings.json`, plus a root-scoped
  webview, and the window title reads `VibeMux · DEMO · isolated native QA`.
  One namespace is run once.
- **stop** requests owned window exit using its root and run nonce. It never kills a
  process by PID.

The runner retains directory identities and preserves atomic settings saves.
It holds immutable pins for the built fixture, trampoline, copied executables
and fixed configuration throughout the run.

Complete the builds before preparing a namespace. Later check/run operations
require its copies to match the current built fixture. A rebuild can invalidate
the old copies even when their byte length is unchanged. Keep the refused
namespace and prepare a fresh root; do not overwrite its copies to force a pass.

New private plans use schema 2 with an explicit `history`, `cleanup_review` or `surfaces`
scenario. A legacy schema-1 plan without a scenario remains History; unknown or
missing version-2 selections are refused. Root/run/stop/report schema 1 remains
unchanged. The cleanup peer is a pinned copy of the built mock, with verified
fixed startup/manifest bytes and no restart or terminal-focus grant. Its empty
result is synthetic evidence, not live WezTerm observation. All three daemon
entry paths wait for that peer to become Active and join owned shutdown on
readiness failure. Ordinary History remains independent of the mock. See
[ADR 062](adr/062_native_gui_acceptance_fixture.md#cleanup-review-scenario).

Native QA windows and their non-GUI AppState observations use a fixed in-process
diagnostic runner. Run local checks takes eight seconds unless cancelled;
Check status observes the same owned job and cancellation preserves its previous
completed result. Every environment/launcher/authentication/inference check is
Not checked. No installed tool, gateway or model is contacted. This Rust-only
composition is unavailable to renderer input, and the normal desktop keeps its
existing diagnostic runner. See the
[Surfaces scenario](adr/062_native_gui_acceptance_fixture.md#tool-card-palette-and-console-scenario).

This is an exclusive, trusted QA environment. Changing the namespace from
outside is unsupported. The checks can detect an observed change, but they are
not a security sandbox and they are not a guarantee against a hostile
concurrent writer.

A monitor samples every 100 ms for up to 20 minutes, and it starts after the
app is constructed, so it does not bound a hung native startup. Cleanup joins
the monitor and the daemon, and explicitly shuts down owned Ask and probe
registries. The actual managed AppState is captured during setup before window
creation, so cleanup observation does not depend on reaching Ready. Ask counts
remain separate; any unresolved probe causes `cleanup_unconfirmed` before
startup, ownership or monitor errors are classified. Later Stop refuses that
uncertain cleanup.
Anything unknown is retained rather than discarded.

`gui_ready_observed` means the Tauri application signalled Ready. It does not
mean that the DOM is present or that a scenario ran. A replay validates the
schema, fixture-only and qualification-false fields, exact identifiers, terminal
phase and cleanup. Altered fixed claims and unconfirmed cleanup are rejected.

Driving the prepared window needs operating-system input, and the bounded
desktop-input sequence needs explicit consent before it starts. On 2026-11-02
that consent covered one ten-minute window, which expired and was not renewed;
a further run needs a new approval. This acceptance
sequence uses no installed vendor or model input. The installed diagnostic allowance is separate: one
approved input was spent and none remains.

Exact commands are in the
[native GUI QA preparation record](evidence/native_harness_m1.md#native-gui-qa-preparation),
and the decision is recorded in
[ADR 062](adr/062_native_gui_acceptance_fixture.md).

## Native GUI acceptance observation (2026-11-02)

The user explicitly approved one ten-minute DEMO native window with the Save
dialogs, the search DEMO, opening a Saved chat, an archived chat and a
Workspace, cancelling one export, saving one JSON file to an exact owned path,
and closing the app. No send, start or resume was approved, and no provider
setup was performed.

The native QA session launched at 06:23:59.395Z and the allowed input window
ended at 06:33:59.395Z. Binding the first window took 110 seconds.

Search DEMO reported a complete scan of two conversations, six records and
241 bytes, with five hits: the Saved title, two user records, the Workspace
title and its user record. Archived conversations were excluded by default, as intended. Opening
the first Saved user record loaded four canonical records: two bilingual user
messages and synthetic answers 1 and 2, with the deep-history notice. No
conversation execution or canonical mutation was requested.

The export opened the owned operating-system Save dialog. Escape cancelled it,
the command returned `Export cancelled`, and no file was created.

A defect was then observed: after the deep record loaded, the header and the
top navigation became clipped. `Ctrl+Home`, the mouse wheel and a direct index
attempt did not restore them. The cause was in the application code. The deep
result in `app.svelte` and `TranscriptFind` both called
`scrollIntoView(center, instant)`, which moved the outer container that hides
its overflow. The browser DOM regression measured outer scroll 76 and header top 1 to -75
with the old call. With the helper, outer scroll stays 0, header top stays 1,
transcript scroll is 872 and document scroll stays 0.

The repair adds a `reveal_transcript_element` helper that scrolls only the
nearest marked `HTMLElement` ancestor with `scrollTo`, using the element
rectangle, `clientTop`, the current scroll and the viewport centre together
with the range bounds. There is no outer or global fallback. It is applied to
the three actual scroll roots, the Ask container in `app.svelte` rather than
the inner NativeTranscript section, the Saved container and the Workspace
container, and it has two consumers. Six new unit cases cover it.

The UI then passed 159 tests with a typecheck of zero errors and zero warnings
and a production build of 526.36 kB advisory, and both independent UI and
source gates passed. At the end of the first batch, the native retest was unverified because
the approved input window had expired. Normal and QA builds containing the
repair passed. The browser regression HTML at
`ui/tests/transcript_scroll_browser.html` is an engineer fixture for the build
and serve path, not native proof.

The JSON save was not completed. The Text and JSON choices were shown, but the
direct menu click was refused by the automation target guard because the
webview is an msedgewebview2 Chrome Legacy child; that is a guard refusal and
not a product save failure. The archived deep open and the Workspace deep open
were not finished and remain unverified.

The attempted post-deadline key action was refused before producing OS input.
No native input was sent after the ten-minute deadline.
The owned nonce stop at 06:35:02 returned runner exit 0, and the report showed
the GUI exited, Ready, the event loop, the joined daemon, the joined monitor
and zero unresolved asks. The stop replay reported `AlreadyStopped` and the
root process was gone. The nonfatal default window icon resource warning and
the WebView class unregister error 1412 are retained.

The icon warning was independently traced to the generated ICO's missing BI_RGB
DWORD. The corrected writer repairs only the exact known fallback and preserves
custom assets. Five public-I/O tests, affected Clippy and both builds passed.
Windows decoders loaded the ICO and both executable icon resources as data;
native appearance confirmation was pending until the renewed batch below.

Control v14, store schema 13, export schema 1, the 31 native commands and the
profiles are unchanged. The TaskWindow feature stays design only and
is deferred until the native defect is repaired, and the original parity gaps
remain open.

Exact commands are in the
[native GUI search and export observation record](evidence/native_harness_m1.md#native-gui-search-and-export-observation-and-scroll-repair),
and the decisions are recorded in
[ADR 043](adr/043_tauri_desktop_app.md),
[ADR 061](adr/061_native_history_search.md) and
[ADR 062](adr/062_native_gui_acceptance_fixture.md).


### Renewed native retest and interrupted final export

The renewed ten-minute batch ran from 08:38:22.377Z to its input deadline at
08:48:22.377Z on Windows 11 build 26300. Search with archived history included
scanned three conversations, eight records and 324 bytes. Saved deep navigation
loaded four records, archived navigation loaded two, and Workspace navigation
loaded two retained records with its execution session Closed. The application
header and Search control stayed visible after each deep open. The repaired
application icon rendered in the title bar. No Send, Start, Resume, provider
setup or installed-vendor inference was requested.

JSON was selected with the native keyboard and the operating-system Save dialog
showed `JSON conversation (*.json)`. Save was not confirmed before the input
deadline; the exact approved destination remained absent. No post-deadline
desktop input was sent. The owned nonce stop returned runner exit 0 with
`gui_ready_observed=true`, `event_loop_returned=true`, `daemon_joined=true`,
`monitor_joined=true` and zero unresolved Asks. The nonfatal WebView unregister
diagnostic 1412 remains recorded separately from that successful lifecycle.

A separately approved final five-minute export attempt launched at 08:53:02.676Z
in a fresh synthetic namespace. The first Computer Use observation reported
that the user pressed physical Escape and stopped Computer Use. No further
Computer Use call or export interaction was made. The owner requested the
root/run-bound stop. The runner exited with code 2 and retained an explicit
`cleanup_unconfirmed` outcome: Ready and event-loop-returned true, daemon joined
true, monitor joined false, unresolved Asks zero. This last attempt is not a
successful cleanup acceptance result and does not replace the preceding batch's
clean shutdown. The JSON destination is still absent. No automatic retry is
authorized by this interrupted attempt.

At this retest cutoff, TaskWindow remained design-only; its later implementation is recorded separately. Control v14, store schema 13,
export schema 1, the 31-command boundary and production profiles are unchanged.
The repaired native scroll and icon observations establish their bounded GUI
result; JSON save, the interrupted monitor-join failure and broader parity and
release acceptance remain open.

## Bound read-only task windows

In Tasks, select a task and choose **Open in window**. The separate view shows
the same Overview, Activity and Artifact metadata as the docked details, with
copyable IDs and manual Refresh. It reads again five seconds after the previous
observation finishes. Appearance is read when the window opens; only the main
window writes settings. Reopening an existing view shows and focuses it.

The view keeps its original task when the main window selects another project.
Closing it does not cancel a task, run, terminal or daemon. If a task window is
still open after the main window closes, the app remains alive; final app exit
uses the existing app-owned Ask shutdown behavior.

Rust checks the registered project and authenticated task before creating the
window, reserves at most 16 opening/open views, and binds each native label to
the exact registration, ProjectId and TaskId. An opening duplicate returns busy.
The initial task capability granted bound Detail reads and public settings
reads. [Task Terminal observation](#task-terminal-observation) adds one closed,
task-bound terminal command alongside those reads.
List queries, another task/project, a forgotten registration and changed directory
identity are refused. URL bootstrap values supply data and cannot grant access.
The task entry initializes no main, chat, Ask or consent controller.

Local verification covers 19 new parser/controller tests, all 178 UI tests,
typecheck with zero errors/warnings, the production UI build, four registry
tests, a MockRuntime URL/reuse check, generated-capability denial, strict invoke
arguments and an authenticated daemon fixture. The latter kept task/run state
and its descriptor unchanged across main selection changes and registration
revocation. The initial shell-sandbox token failed the existing Windows runtime
ACL check; the unchanged fixture passed under the normal host token. No ACL was
weakened. A later approved Windows 11 fixture batch also exercised the native
task window: Overview, Activity and Artifact metadata, duplicate-open reuse,
close/reopen, and continued observation after the main window closed. Closing
the last child then joined the QA daemon and monitor with zero unresolved Asks.

No new command name, dependency, listener, production profile or schema is added.
Control v14, store 13, export 1 and the 31 native commands remain unchanged.
Terminal controls, artifact contents and broader desktop parity remain open.
See [ADR 063](adr/063_read_only_task_windows.md) and the
[execution record](evidence/native_harness_m1.md#bound-read-only-task-windows).

### Native QA stop publication

The isolated QA runner publishes a complete stop record after closing its
exclusive staging writer. Publication does not replace an existing destination;
matching concurrent stop requests remain idempotent. It requires hard-link
support on the local filesystem and fails without a replacement fallback.
Directory identity pins continue to deny delete sharing, while permitting the
directory writes required for publication. File readers and immutable fixture
pins retain their prior sharing restrictions.

If a staging link cannot be removed, `native_qa_stop_stage_retained` names that
owned recovery artifact; a valid published stop remains successful. Monitor
failure now emits its inner stable error code. The historical third run's
`monitor_joined=false` can also represent an error returned by a joined thread;
it does not establish a leaked thread or prove which failure branch ran.
The original outcome is preserved. See [ADR 062](adr/062_native_gui_acceptance_fixture.md)
and the [repair evidence](evidence/native_harness_m1.md#qa-stop-publication-and-monitor-diagnostics).

### Native Workspace JSON export acceptance

The approved Windows 11 fixture batch selected JSON, entered the exact approved
destination in the native Save dialog and saved one Workspace artifact. Its
2,065-byte schema-1 file contains one completed turn and two complete records,
with no truncation/content errors and `partial=false`. Independent artifact
checks matched its session/project/task/run identities and record cutoff to the
QA plan. This is Workspace JSON-save evidence; Saved/chat JSON save remains a
separate scenario. The batch used no Send, Start, Resume, provider setup or
installed-vendor inference. Details and native images are in the
[acceptance record](evidence/native_harness_m1.md#native-task-window-and-workspace-json-acceptance).

## Worktree actions and HTTPS links

In a task's **Overview**, each Run has **Open worktree folder**, **Copy worktree
path** and **Copy branch**. The selected Workspace view has the same controls.
Branch copying uses the complete recorded branch, even when its display label
was shortened. It can remain available when a directory is missing. Folder and
path actions require current managed-worktree ownership; a recorded path alone
does not grant access. Verified retained Finalized workspaces remain browseable,
including dirty files and an advanced owned HEAD. Execution and cleanup retain
their separate checks.

An explicit HTTPS link click in an Independent Ask, Saved conversation or
Workspace transcript asks the native host to open the default browser. Other
schemes, credentials, whitespace/control characters and oversized URLs are
refused. The app blocks WebView navigation away from its own origin. An open
acknowledgement means the launcher accepted the request; it does not prove a
remote page loaded or a particular file-manager window exists. Task child
windows can use only their bound worktree actions, with no HTTPS capability.

Both features use the existing closed `open_link` command and require the
bundled Control v16 daemon/client. These actions retain their v15 minimum;
current builds use store schema 14, export schema 1 and 32 native command names.
No Tauri shell/opener/clipboard plugin is exposed. Paths, full branch
values and native handles remain in the native/Core channel. Clipboard actions
replace text explicitly and never read or restore the previous clipboard. Linux
clipboard availability after app exit depends on the desktop clipboard manager.

Directory actions perform two complete fresh Core inspections while native
directory/ancestry handles remain held, then recheck identity before OS handoff.
Windows pins deny delete sharing through handoff; compatible writes remain
possible. These checks do not freeze Git state or provide an OS sandbox. A
selection change cancels queued frontend work and ignores late results, but
cannot recall an already accepted native effect. Failed or unconfirmed actions
are never replayed automatically. DEMO fixture mode keeps the controls inert.

Local Windows checks cover authenticated Core reads, actual Tauri main/child
arguments and capability enforcement, managed Open/Finalized ownership,
clipboard values through fake effects, directory pins and frontend disposal.
A bounded Windows 11 synthetic native check verified both copied values by
explicit paste into the QA filter, the exact worktree folder in File Explorer,
the fixed HTTPS browser tab, and branch/path acknowledgements in the Task child.
Browser page-content inspection could not finish because the browser tools were
unavailable. When tool waits exhausted desktop consent, the owned QA stop command
closed/joined the session with zero unresolved Asks and no remaining QA windows.
The ordinary manual close sequence and broader platform acceptance remain open.
See [ADR 065](adr/065_native_links_and_worktree_access.md) and the
[executed evidence](evidence/native_harness_m1.md#native-worktree-and-https-presentation).

## Task Terminal observation

Open a task's **Terminal** tab in either the docked details or its independent
window, select one of its loaded Runs and choose **Find terminals**. With the
existing WezTerm observation plugin explicitly configured and granted, the view
shows at most four matching panes. Choose **Link** for an offered pane, then
**Focus** to raise that linked pane or **Unlink** to remove its observation link.
The existing [plugin setup](terminal_observer_protocol.md#configuration) applies.
Find does not start a daemon, plugin, terminal or harness.

Terminal observations have their own freshness and do not depend on a changed
task revision. The view shows only sanitized pane identifiers, observation time
and status; cwd, terminal content and titles are excluded. A missing plugin,
denied grant, stale pane or changed session produces an unavailable/error state
without disrupting the task's metadata tabs.

Each native window receives its own observation token. Link and Focus require
its latest reviewed observation and are retired before submission. After an
uncertain effect, use Find before another Link or Focus. If the view already
reviewed a linked pane, explicit cleanup-only Unlink remains available through
failed effects, repeated lost Find replies and plugin outage. It targets exactly
that reviewed link and can be explicitly retried after a lost acknowledgement.
A successful Find advances the view's acknowledgement. Selecting another Run,
closing the view or forgetting the project clears its cached observations.

Both A2A-backed Runs and Open Workspace chat-owner Runs can resolve a managed
workspace. Workspace observation rechecks existing managed directories, branch,
Git inventory, ownership receipt and resource identity. Dirty files or an
advanced owned HEAD remain observable; fresh execution retains its stricter
empty-grant and original-HEAD policy. Provisioning, recovery, finalized, foreign
and corrupt owner bindings are refused.

This surface uses existing Control terminal operations and sends no terminal
text. Native command permissions and bundled UI assets must be upgraded together.
Local bridge/controller, owner, protocol and capability checks are recorded in
the [Terminal evidence](evidence/native_harness_m1.md#task-terminal-observation).
An approved Windows 11 fixture batch rendered Terminal in both docked and
independent Task views and returned `terminal_plugin_unavailable` from explicit
Find in each. The child completed another Find and refreshed revision 42 after
the main window closed; final child close joined the daemon and monitor with
zero unresolved Asks. Real WezTerm Link/Focus/Unlink remains a separate acceptance
check; the earlier three-tab native images do not establish it. See
[ADR 064](adr/064_task_terminal_observation.md).


## Command palette conversations

Press **Ctrl+K** and enter a title or identifier to find loaded Saved and
Workspace conversations alongside projects, retained Asks, Tasks and actions.
Entries show the project and conversation kind. A title that has not been loaded
uses a short conversation-ID label. Long titles remain searchable beyond the
shortened display label.

The palette uses metadata already loaded by the conversation views; opening or
typing in it fetches no Saved/Workspace transcript. Choose **Search conversation
history** to search other retained records in the selected project. That action
opens the existing explicit history-search surface.

Opening a conversation selects its currently registered project and uses the
existing read selectors. Missing folders, forgotten projects, evicted targets and
purged Saved content cannot become enabled conversation targets. A changed or
disposed selection does not publish late navigation. Fixture mode keeps these
actions unavailable.

Drafts and attachments remain with their conversations, and navigation does not
Start, Resume, Send, Cancel or retry work. Ordinary read-only reconciliation can
confirm an earlier uncertain operation and settle its pending slot. Confirmation
consumes current input only when the raw text and complete formatted attachment
payload still match the submitted prompt. Changed input remains available.

Local catalogue, navigation and acknowledgement regressions, typechecking and
production assets passed. A synthetic native sequence opened both Saved and
Workspace palette entries. See the
[palette execution record](evidence/native_harness_m1.md#saved-and-workspace-command-palette)
and the [native observation](evidence/native_harness_m1.md#native-surfaces-interaction-and-observed-gaps).

## Find in retained conversations

Use **Ctrl+F**, the header Find button or the palette's Find action in the
selected Saved, Workspace or retained Ask. Find searches displayed transcript
text, opens the selected tool disclosure and centers the matching text range
inside that transcript. Saved/Workspace eligibility requires the current
registered project/chat/session binding; unavailable, mismatched or purged
private history stays ineligible.
Closed Workspace sessions and finalized owners remain searchable without
starting a process. Selecting another Workspace/project closes or recreates
Find for the new target; ordinary history updates refresh matches.

Closed/archived Saved history and inactive Workspace sessions keep the empty
composer under **Draft message**. Expand it to edit a local draft. Existing text
or attachments open that control on context entry; clearing the text or removing
the last file keeps the user's current expansion. Selection preserves the
controller-owned draft and files. Resume/Send and consent gates remain unchanged.
Saved storage-consent information scrolls with history and its explicit Review
action remains available.

The actual components, production styles and export controls passed late-diff
Find, fixed-header, bounded-scrolling and draft regressions at CSS viewports
1280×850 and 900×600. The repaired native window remains a separate retest.
See the [repair evidence](evidence/native_harness_m1.md#retained-find-and-history-layout-repair).

## Explicit worktree removal

Close the execution session, then **Stop workspace** to finalize it. Stop keeps
the worktree, branch and conversation history. For a verified finalized Workspace,
the Main window also provides **Review removal**. Review the recorded branch and
choose **Keep worktree** to dismiss the preview, or **Remove worktree** to confirm.
The confirmed operation removes the managed worktree and preserves its branch
and chat history. Task-child windows have no removal command.

The preview lasts at most five minutes and expires across daemon restart. Any
tracked, untracked or ignored data, changed HEAD/binding, uncertain ownership,
conflicting Run/workflow lease or matching terminal pane prevents admission.
Clean committed work can be removed while its commits remain on the retained
branch. A configured compatible observer must explicitly advertise and receive
`terminal:cleanup_inventory_v1`, `terminal:observe` and `process:execute`; enable
`cleanup_inventory_enabled` in its own configuration too. Missing, ambiguous,
unavailable or incomplete observation is a refusal. See the
[terminal setup](terminal_observer_protocol.md#opt-in-cleanup-subtree-observation).

After confirmation, read-only status follows the owned operation. Closing its
request/view does not re-send or drop it. A lost reply retains the original
target and request; use **Refresh status**, then explicit **Check outcome** when
needed. That action persists evidence/observes postconditions and never replays
deletion. A Planned status alone does not prove that a queued confirmation cannot
act later. Automatic polling is status-only, bounded and stops with the view.

Unknown remains reserved until positive executor evidence permits reconciliation.
Missing PIDs, elapsed time, an unavailable observer or daemon death cannot supply
that proof. A moved Git registration or the Run branch attached elsewhere is
not observed removal. Removed results refer to the historical workspace; later
reuse of its old path cannot restore directory actions for that old binding.
Recorded branch metadata stays readable.

The current effect implementation is Windows-only. Local tests use real owned
temporary Git repositories and synthetic terminal peers; they do not qualify live
WezTerm, Windows 10, other filesystems or native desktop removal. A bounded
Windows 11 synthetic review/Keep/review check passed, including a Task child
without removal controls and joined shutdown; it pressed no Remove control.
Control v16,
store schema 14 and native command 32 require coordinated binaries/assets. See
[ADR 066](adr/066_explicit_worktree_removal.md) for the authority and recovery limits.

## Console local checks

On native Windows, open Console → Harnesses, select a registered project and
choose **Run local checks**. The app runs its fixed sibling diagnostic helper
through the contained trampoline. The desktop bundle must include matching
`vibemux_probe.exe` and `vibemux_launch_trampoline.exe` in the desktop executable's
directory; build `vibemux_probe`, `vibemuxd` and `vibemux_desktop` before packaging.
Missing or mismatched helpers refuse. The app installs no tool and opens no
login or provider-setup flow.

The report shows launcher/version observations, gateway health and an A2A
loopback self-test. Authentication and model responses remain Not checked.
**Last checked** is the report's own timestamp. Failed or cancelled runs leave
the previous completed report visible. Ordinary Console Refresh reads recorded
data and starts no local checks. Browser fixture mode has no diagnostic runner;
other native platforms refuse this execution action explicitly.

**Cancel** requests cancellation of the original operation. Wait for observed
status before treating it as stopped. **Check status** reads the retained
receipt; bounded one-second polling also observes status while the panel is
mounted, for at most 60 ticks. Navigation and polling never start another check.
An uncertain reply stays bound to its original project. Return there and use
Check status; a missing receipt does not prove that a delayed Start cannot run.
Unconfirmed cleanup blocks new checks. Closing Main stops admission and requests
cancellation; application shutdown joins owned workers within one absolute
35-second budget and preserves unresolved cleanup.

The Main-only command is `probe_rerun({ project_key, request })`. Requests are
closed objects with `operation: "start" | "status" | "cancel"` and a non-nil
`operation_id`; Status alone also accepts `null` to read the latest operation
for that registration. Start replay returns the original receipt without rerun.
Snapshot schema 1 has exactly `schema_version`, `project_key`, `job`, `result`,
`can_start` and `blocking_code`. Job fields are `operation_id`, `phase` and
`error_code`; result fields are `operation_id` and the public metadata `report`.
Phases are Running, Completed, Failed, Cancelled and Unconfirmed, serialized in
`snake_case`. The app permits one active job globally and retains at most 64
operation bindings without eviction. Reopen the app when receipt capacity is
exhausted. Task children have no probe capability. Renderer input cannot choose
an executable, arguments, environment, endpoint, provider or model.

The host/renderer contracts and contained synthetic process path are locally
verified. Native Console completion, cancellation, status and retained results
were observed with the fixed in-process NotRun runner. Installed diagnostic
behavior remains unqualified. See [ADR 069](adr/069_native_console_diagnostics.md) and its
[execution record](evidence/native_harness_m1.md#native-console-local-checks-integration).

## Command tool cards

Completed Codex command observations appear as collapsed cards in Saved and
Workspace transcripts. Expand a card to read the literal command and working
directory, reported Completed/Failed/Declined status, output, exit code and
duration. An unavailable output differs from an observed empty output. Command
and output prefixes carry visible truncation labels. Find in transcript opens
the selected hit's card and scrolls only that transcript.

Cards appear after the vendor's terminal command item arrives. They do not add
a command runner, new permission, approval decision or live tool-progress stream.
Tool status is an observation and does not determine the turn or Task outcome.
Claude remains tool-free. File-change inspection is described below; installed
writable profiles remain unfinished.

The daemon validates exact active thread/turn correlation and writes each
command observation once through the protected private writer. Legacy Tool
text, unsupported schemas, incomplete reads and externally truncated records
remain expandable literal text. All displayed strings stay inert; paths and
URLs are not activated. Existing assistant-text aggregation is unchanged.

Retained command text is at most 8 KiB and output 16 KiB, at UTF-8 boundaries.
Each private envelope is at most 256 KiB, with at most 64 tool records and
256 KiB of tool bodies per turn. A display-budget or projection failure marks
capture incomplete and preserves the separately correlated vendor outcome.
The optional writer callbacks share one absolute batch deadline. A timeout
retires further tool capture for that turn; it never resends or allocates a
replacement record for an unknown append.

No Control, SQL or metadata DTO migration is required: the new version-1 body
uses existing private Tool content. Older clients display it as text. Existing
exports keep that exact body in the text field. The contract and verification
limits are in [ADR 070](adr/070_private_command_tool_records.md).

## File-change tool cards

Saved and Workspace transcripts also show completed Codex file-change
observations. Expand a File changes card to inspect its reported status,
ordered paths, Add/Delete/Update kinds, optional move paths and literal diff
text. Repeated paths retain their order. These are vendor observations: the
card does not read files, apply/revert a patch or verify an on-disk effect.
Existing read-only profiles and approval denials remain unchanged.

Each body retains at most 16 changes, 16 KiB per diff and 32 KiB of diff text
in total. Source counts describe omitted entries, and diff prefixes have their
own truncation flags. Empty reported lists, omitted details, empty diffs and
truncated empty prefixes have distinct labels. Commands and file changes share
the same 64-record/256-KiB turn budget and call-ID namespace. Display loss marks
capture incomplete without changing a correlated vendor outcome.

Paths and diff strings remain inert, including URLs and executable-looking
text. Complete known private bodies get structured cards; partial, malformed,
unknown or externally truncated bodies remain literal text. Messages and
reported tool activity use the project's private conversation history, as the
consent copy states. Exports preserve the exact body text.

Find opens the selected disclosure and centers the matching text range.
Bounded tool blocks use transcript scrolling, so long output/diff text has no
second clipped vertical viewport. An actual-component Chrome fixture reproduced
the former whole-block failure and verified range visibility without moving
the header or page. A synthetic native sequence expanded Saved/Workspace cards
and selected a Saved late-diff match. Workspace Find and compact-history repairs
have component browser evidence; their native retest and installed writable
qualification remain open. See [ADR 071](adr/071_private_file_change_records.md)
and the [execution record](evidence/native_harness_m1.md#private-file-change-records-and-cards).

For the standalone browser regression, start the existing UI development server
from `crates/vibemux_desktop/ui`:

```text
node node_modules/vite/bin/vite.js --config vite_config.ts --host 127.0.0.1 --port 5180 --strictPort
```

Open `http://127.0.0.1:5180/tests/tool_record_browser.html`. Reveal whole block
reproduces the offscreen matching line; Reveal selected text must report
`range_visible: true`, preserved header and zero outer/page scrolling. The
fixture mounts the actual component with synthetic data and no native bridge.
Stop the owned development server after the check. A busy port is a refusal;
this command does not replace an existing listener.
