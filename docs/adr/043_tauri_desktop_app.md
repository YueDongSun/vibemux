# ADR 043: Tauri desktop app replaces the egui GUI

Status: Accepted (2026-10-24) with the `vibemux_desktop` skeleton PR (design
§10, PR 3). The design and the operator's decisions are in
[docs/desktop_app_design.md](../desktop_app_design.md).

On acceptance this ADR supersedes the egui presentation decisions of
[ADR 028](028_supervisor_chat_frontend.md) and
[ADR 030](030_claude_style_gui.md). ADR 028's Control v4 task reads, content
limits, task window semantics, and WezTerm observation rules stay in force, as
do ADR 016's native terminal boundary and ADR 026's palette contract. It
reopens the M5 deferral of a graphical web or Tauri frontend for the local
desktop only; remote access stays under ADR 033.

## Context

The egui Supervisor Chat GUI already follows Claude Desktop's layout and
palette (ADR 030) but still looks rough. egui's text rendering, markdown,
selection, IME, and motion are weak or hand-built, and `gui/supervisor_app.rs`
has grown to 2,763 lines. The operator asked for a full rewrite as a Claude
Desktop style app, designed around multi-turn harness chat (streaming replies,
tool calls, approvals) and shipped in two phases: a new shell on today's
capabilities, then chat.

The repository already has a Svelte 5, Vite, and TypeScript UI for the mobile
surface (ADR 033) on Node 22.22.0, and ADR 033 names a Tauri 2 shell as a
future track. Only `egui::Context`, used to request repaints, ties the daemon
client code to egui.

## Decision

### 1. Stack

A Tauri 2 desktop app with a Svelte 5 UI, hosted in the system WebView2 on
Windows. Versions checked on 2026-10-24: `tauri` 2.12.5 and `tauri-build`
2.7.4 (MSRV 1.90, within the pinned 1.92), `@tauri-apps/api` 2.12.4, Svelte
5.57.2, Vite 8.3.5. Tauri 3 is alpha and needs Rust 1.95, so it is excluded.
UI libraries: `markdown-it` (MIT), `highlight.js` (BSD-3-Clause), and
`@lucide/svelte` (ISC).

### 2. Crates

- `vibemux_client`: a new library with no GUI or terminal dependency. It takes
  the daemon, probe, Ask, and Git-inventory client code from
  `vibemux_frontend` (design §5.2). A `ChangeNotifier` trait replaces
  `egui::Context` for repaint requests.
- `vibemux_desktop`: the Tauri binary. It owns the window, polling, daemon
  credentials, and its settings file.
- `crates/vibemux_desktop/ui`: the Svelte app.
- Dependency direction: `vibemuxd → vibemux_client → vibemux_frontend,
  vibemux_desktop`.

### 3. Command boundary and content rule

- The webview reaches the daemon only through a fixed list of typed Tauri
  commands (design §5.3). It never sees the control descriptor, pipe, or token.
- Every command validates its arguments: typed IDs and byte-limited strings.
  The webview never passes a filesystem path; folders come from the native
  folder dialog, run on the Rust side, or from the saved project list.
- Events to the UI are content-free. In phase 1, `ask_reply` is the only
  command that returns vendor text.
- The app never opens SQLite and adds no listener.

### 4. Webview lockdown

- Bundled assets only. CSP: `default-src 'self'; script-src 'self';
  style-src 'self'; img-src 'self'; connect-src ipc: http://ipc.localhost;
  object-src 'none'; base-uri 'none'; frame-ancestors 'none'`, confirmed
  against Tauri's own CSP injection when this ADR is accepted.
- `withGlobalTauri` off, `freezePrototype` on, navigation away from the app
  origin refused.
- Every app command is declared in the app manifest. The main capability grants
  its command list only to `main`; the separate task capability grants bound
  Detail reads and public settings reads to `task_*` windows ([ADR 063](063_read_only_task_windows.md)). No Tauri plugin is
  registered, so no `fs`, `shell`, `http`, or `opener` plugin can reach the
  webview; `open_link` opens `https://` URLs from Rust with the `open` crate.
  [ADR 065](065_native_links_and_worktree_access.md) extends this slot with
  checked Task/Run worktree presentation, native clipboard ownership and
  main-only HTTPS dispatch.
  The generated permission name is `allow-<command>` with the command's
  underscores replaced by dashes (`allow-projects-list`), and the command
  list must be a `static`, because the app manifest takes a
  `&'static [&'static str]`.
- Markdown renders with raw HTML disabled, only `https:`, `http:`, and
  `mailto:` links kept, and images off.

### 5. Screens and parity

The sidebar lists project folders with their daemon state, and each project
expands to its chats. In phase 1 a chat is a read-only one-shot Ask through
ADR 029 dispatch, with today's setup and consent gates. Tasks, Console (seven
sections), and Settings (five sections) carry over. Appearance offers the
Claude light and dark pair and Match system. The egui GUI is removed only
after the design's parity checklist (§5.7) passes, in a separate change; it
is frozen to bug fixes until then.

### 6. Design tokens

`config/design_tokens.css` becomes the one token stylesheet for the desktop UI
and `vibemux_web`. A test pins it to `config/theme_palettes.json` and checks
WCAG AA for every text pair.

### 7. Phase 2 reservation

Multi-turn harness chat needs its own ADR, Control v11, and store schema 10
before implementation (design §6). This ADR only reserves the UI shape. Chats
that edit files will run in their own worktree with an approval for every edit
and command, and writable chat for a harness will need live certification on
its installed version.

## Compatibility and rollback

- No control operation, protocol version, store schema, or plugin payload
  changes in phase 1.
- `vibemux_client` extraction keeps the moved APIs re-exported from
  `vibemux_frontend` during the transition, so the TUIs, the dashboard, and
  the egui GUI build unchanged.
- The desktop app keeps its own settings file; the egui GUI's settings are not
  read or changed.
- Until the egui GUI is removed, rollback is not running the new binary. After
  removal, rollback is reverting the removal PR. No state needs cleanup.

## Security and privacy

- Same-user trusted-operator boundary as the egui GUI: the app is a client of
  the local daemon behind the existing descriptor and ACL checks.
- The webview is treated as less trusted than the Rust side: it holds no
  credentials, can call only the listed commands, and receives content only
  through explicit reads.
- No remote content loads. Vendor text is escaped markdown under a strict CSP.
- No telemetry, crash upload, or update check.

## Verification

- `vibemux_client`: the moved tests, plus the unchanged egui and TUI tests.
- `vibemux_desktop`: command handlers against a real test daemon, and a
  lockdown test over `tauri.conf.json` and the capability files.
- UI: `svelte-check`; `node --test` for markdown safety, key bindings, and
  formatting; a guard that the production build contains no fixture data.
- Token parity and contrast test.
- Light and dark screenshots of every screen from the fixture backend for
  design review.
- Gates: `cargo fmt --check`, workspace clippy with `-D warnings`, the affected
  crate tests, `cargo deny`, `npm audit`, on Windows and Ubuntu.

These prove contracts and layout at tested sizes. They do not prove WebView2
behavior on every Windows build, IME behavior with every input method, or any
vendor behavior.

The skeleton PR that accepts this ADR adds the shell, the settings file, the
navigation guard, and the lockdown tests. Its build gates (`cargo fmt`,
`cargo clippy`, `cargo test -p vibemux_desktop`, and the UI `typecheck`,
`test`, and `build`) were still outstanding when the ADR was accepted, so
the skeleton is not yet a qualified build.

## Alternatives

- Rewrite in egui: rejected; the rendering limits are the problem.
- Electron: rejected; a second control-pipe client or a Rust sidecar, a
  bundled Chromium, and a move away from the Rust core.
- Serving the UI from `vibemux_web`: rejected; it is read-only and
  bearer-authenticated by design, and approvals over HTTP widen the attack
  surface.
- Slint, Iced, or GPUI: not chosen; markdown, highlighting, selection, and IME
  would still be custom work, and GPUI's API is not stable.
- Tauri 3: excluded until it is stable and the pinned toolchain reaches its
  MSRV.

## Amendment (2026-10-31): native one-turn conversation M1

The native conversation integration implements the bounded project-to-Ask
journey before the full screen-parity milestone. Its source and verification
status live in [the M1 acceptance record](../evidence/native_harness_m1.md);
[the use guide](../native_harness_conversations.md) records the capability and
retention limits. The accepted M1 contract refines the earlier desktop plan:

- Claude and Codex visual presentations share the same native project, route,
  draft and Ask ownership. Appearance has no execution authority. The
  standalone browser API app remains separate and cannot become a privileged
  webview or a fallback provider for native mode.
- Rust resolves opaque registered project handles through the existing
  `DaemonPaths` canonical identity rules and a held directory identity. This
  replaces the plan's independently computed path-hash key. Unknown handles,
  missing/changed identities and forged paths refuse. Directory-identity
  receipts bind saved consent; path-only legacy consent is reconfirmed.
- Public settings DTOs exclude project folders, confirmation records and
  identity receipts. Both settings commands return the public projection.
  Settings patches cannot register projects or manufacture consent.
- Each explicit Send is one independent ADR 029 Ask. Claude remains tool-free;
  Codex remains configured read-only and ephemeral. Native attachments,
  unsupported model/thinking controls, writable coding and tool approvals are
  unavailable. Existing egui behavior is retained until its separate removal
  gate; M1 does not claim complete phase-1 parity.
- Startup/listing/style changes never perform setup. Project lifecycle and
  setup are explicit, per-project serialized mutations that preserve app-owned
  active/uncertain ownership. Setup checks existing route configuration first
  and requires the daemon to be stopped (`daemon_stop_required` otherwise);
  an operator explicitly stops an existing daemon before setup. It adds no
  cross-client lock or new Control operation. Selecting another project/Ask
  does not cancel execution or discard its owner.
- A local submission receipt is distinct from confirmed daemon admission.
  The desktop reserves the request ID before the worker starts, atomically
  binds a submission key, and preserves daemon-provided Task/Run identity.
  Uncertain delivery reconciles that request without a replacement send.
- Rust continuously drains owned Ask workers and stores bounded snapshots,
  independently of webview polling or visibility. Status/events remain
  content-free; `ask_reply` is the vendor-text accessor. Monotonic revisions
  prevent stale responses from replacing newer state. Canonical terminal
  evidence, local observation errors, reply availability/completeness and
  display truncation remain separate facts.
- Display history is 32 Asks per project, evicting only oldest settled entries.
  Replay protection is a separate process-lifetime budget of 1,024 submission
  bindings; exhaustion refuses new sends as `ask_dedup_capacity`. A display
  eviction cannot erase a binding. This is not durable exactly-once behavior.
- Webview reload can rediscover retained Rust snapshots and owners. Desktop
  exit loses native display history and local deduplication bindings. Daemon
  restart loses raw output but retains canonical attempts and their recovery
  semantics. Native text does not enter browser localStorage by default.
- App shutdown requests cancellation, drains for a 3-second reconciliation
  period, then stops observers/closes receivers and uses a 5-second bounded
  join budget outside locks. Unknown terminal state remains unknown; the
  daemon keeps authority over reservations, deadlines and process containment.
- Bundled assets, CSP, navigation restrictions, prototype freezing,
  `withGlobalTauri: false` and the fixed command permissions remain. M1 adds
  no renderer shell, filesystem, HTTP or generic opener plugin; no active
  HTML/SVG artifact preview or generic RPC bridge is permitted.

M1 added no daemon listener, Control operation or database migration. At that
stage Control v10 remained current and Ask's v5 minimum was not a downgrade target.
[ADR 057](057_persistent_harness_sessions.md) proposes persistent sessions
separately and reconciles the earlier section-6 shape; it does not allocate a
new protocol/schema number or claim that persistent sessions are implemented.

Subsequent conversation increments are recorded in ADRs 057–068. The current
desktop has 32 fixed commands, including `chat_approval_request` over Control
v14. Its mounted Workspace review panel consumes existing offers with explicit
private input reads, human choices and read-only receipt recovery. Schema 13 and
the production read-only profile remain unchanged by this surface; writable
profile qualification remains pending. The latest contracts and verification
are in [ADR 059](059_interactive_chat_approvals.md) and the
[surface evidence](../evidence/native_harness_m1.md#control-v14-and-desktop-tool-approvals).

[ADR 063](063_read_only_task_windows.md) implements bound independent Task
windows. [ADR 064](064_task_terminal_observation.md) adds a shared Terminal tab
and activates the existing `supervisor_action` with four closed terminal actions.
Task children receive that command beside their two existing reads. Per-view
observations and cleanup receipts preserve exact lease identity after lost
replies. That terminal increment left Control v14, store schema 13 and the
31-command list unchanged;
native unavailable-plugin handling is verified in docked/child views; real WezTerm actions remain separate acceptance work.

[ADR 066](066_explicit_worktree_removal.md) adds Main-only command 32 and a
reviewed Windows worktree-removal service over Control v16 and schema 14.
Task-window permissions remain unchanged. Source and owned-fixture integration
checks passed; native cleanup-panel interaction remains unverified.

[ADR 060](060_native_conversation_export.md) adds `chat_export_request` and the
shared saved/Workspace Export control. Native Windows selection precedes
authenticated history reads, and the bounded snapshot is written only to a new
selected file. It changes no Control operation or storage schema and grants no
execution authority. A bounded native check opened and cancelled the Save dialog;
An initial renewed check selected JSON and opened its Save dialog without
completing the write. A later approved batch saved the Workspace JSON artifact
with two complete records and verified owned shutdown.

[ADR 061](061_native_history_search.md) adds project-wide retained-history
search and read-only result navigation through `chat_search_request`. Search
composes existing authenticated reads, returns bounded private previews and
continuation, and preserves gaps and source versions. Opening a result never
starts or resumes a turn. Control v14 and store schema 13 remain unchanged.

[ADR 062](062_native_gui_acceptance_fixture.md) adds a nondefault, Rust-only
native GUI QA runner using the same bundled application builder and command
boundary. Its synthetic project, application settings and WebView data belong
to one fresh trusted namespace. Fixture provenance, observed identity checks,
nonce stop and explicit shutdown are separate from live GUI acceptance; normal
launch selection and production-profile permissions are unchanged.

### Native GUI repairs observed on 2026-11-02

Deep record reveal and transcript Find now scroll only their marked transcript
container. The old `scrollIntoView` call also moved an outer overflow-hidden
container and clipped the header during native acceptance. Source/unit and real
browser geometry checks cover the repair. A renewed native check kept the header
visible after Saved, archived and Workspace deep navigation.

The fallback ICO writer now emits the complete 40-byte BITMAPINFOHEADER with
BI_RGB before the image-size field. The earlier 36-byte encoding was embedded
under the correct Windows icon resource ID but rejected by Windows decoders.
Build preparation creates individually missing assets and repairs only the
entire exact known legacy encoding. Existing PNGs and custom ICOs are preserved.
The normal/QA executable icon resources pass non-GUI Windows decoding; no plugin,
image library, command permission or runtime execution profile is added.

Observed native results, original failures, decoder checks and the remaining
interaction scope are in [the repair record](../evidence/native_harness_m1.md#native-gui-search-and-export-observation-and-scroll-repair).


The command palette now uses loaded Saved/Workspace metadata and cached titles
alongside its existing local entries. It delegates complete retained-history
queries to the explicit history-search surface, with no new native command or
database reader. Read-only navigation revalidates the current project/target and
caller lifetime; positive receipts may settle existing uncertainty without replay.
See the [palette guide](../native_harness_conversations.md#command-palette-conversations)
and [local verification](../evidence/native_harness_m1.md#saved-and-workspace-command-palette).
