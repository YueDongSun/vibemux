# Desktop App Design — Tauri Shell First, Harness Chat Second

Status: design sections approved by the operator in conversation on
2026-10-24; this written spec awaits the operator's review. The decision
record for phase 1 is [ADR 043](adr/043_tauri_desktop_app.md)
(`Proposed`). Phase 2 (harness chat) gets its own ADR before implementation.
Nothing here changes code, protocols, or schemas, and nothing described here is
implemented.

Operator decisions (2026-10-24):

1. Replace the egui Supervisor Chat GUI with a Claude Desktop style app.
2. Design it around real multi-turn chat with Claude Code and Codex (streaming
   replies, tool calls, approvals), but ship the new shell on today's
   capabilities first and add chat after.
3. A chat that edits files or runs commands does so in its own Git worktree,
   and every edit or command waits for an approval card.
4. Chat transcripts persist in daemon-owned storage, with the vendor's session
   or thread ID kept for resume.
5. The sidebar lists projects, and each project expands to its chats.
6. The stack is Tauri 2 with a Svelte 5 UI.

---

## 1. Problem and goals

The egui GUI (`crates/vibemux_frontend/src/gui/`, ADRs 028 and 030) already
uses Claude's palette pair, a serif greeting, a sidebar, a rounded composer,
and a docked task drawer. It still looks rough, and the limits are egui's: text
rendering, markdown, selection, IME, smooth motion, and rich transcripts are
weak or hand-painted. `gui/supervisor_app.rs` alone is 2,763 lines. A further
restyle in egui would hit the same ceiling.

The goal is a desktop app that looks and behaves like Claude Desktop for the
harnesses VibeMux supervises:

- **Phase 1:** a new shell with today's features: project folders and their
  daemons, a chat screen whose Send runs today's read-only one-shot Ask, the
  task list and detail, the console, and settings.
- **Phase 2:** multi-turn chat with Claude Code and Codex: streaming replies,
  tool-call cards, approval cards, saved history, and resume.

Success criteria:

- Phase 1: every item in the parity checklist (§5.7) works in the new app;
  light and dark screenshots of every screen pass review; the webview lockdown
  tests (§5.5) pass; the egui GUI is then removed in a separate change.
- Phase 2: fixture-verified chat with approval, deny, expiry, cancel, resume,
  and limits; writable chat for a harness only after its live certification
  (§6.5).

Non-goals:

- An installer, code signing, or auto-update. Phase 1 runs from a local build.
- Changes to the mobile web surface (ADR 033) or the terminal UIs.
- Chat with ACP harnesses (OpenCode, Copilot, Grok). Their routes stay
  probe-only.
- Coordinator chat (ADR 028). The composer's targets are harnesses.
- Merging a chat's branch from the app (ADR 009).
- The five non-Claude palettes in the desktop app. They remain for the TUIs
  and the `vibemux theme` snippets.

## 2. Current state

| Area | Today | Source |
|---|---|---|
| GUI | egui 0.34 Supervisor Chat: welcome, task list, docked drawer, detached task windows, project folders with daemon Start/Stop, read-only Ask, Console (7 sections), Settings (5 sections), `Ctrl+K`, `Ctrl+/` | ADRs 028, 030, 042 |
| Web UI | Svelte 5.57.2, Vite 8.3.3, TypeScript 5.9.3, Node 22.22.0; read-only mobile surface | ADR 033, `crates/vibemux_web/web` |
| Design tokens | snake_case CSS variables from `claude_light` with a `claude` dark variant and AA-checked derived colors | commit `0f08aa5`, only on `origin/fix/web_design_tokens` |
| Task reads | Control v4 `frontend_tasks` / `frontend_task`, content-free, bounded | ADR 028 |
| Ask | one read-only structured turn through ADR 029 dispatch; reply lives in daemon memory | ADRs 029, 030 amendment |
| Console reads | content-free daemon feed, probe view model, Git inventory | ADR 042 |
| Approvals | `vibemux_harness::dispatch` recognizes Codex `item/commandExecution/requestApproval`, `item/fileChange/requestApproval`, `item/permissions/requestApproval`, Claude `control_request` `can_use_tool`, and ACP `session/request_permission`, and declines them | `dispatch/observation.rs` |
| Multi-turn | not implemented; ADR 040 excludes multi-turn resume and approval execution | ADR 040 |
| Streaming | no push channel on the control pipe; clients page with `after_sequence` | ADR 033 context |

Only `egui::Context`, used to request repaints, ties the daemon client code to
egui (`supervisor_client.rs`, `ask/worker.rs`). The dashboard's daemon feed,
daemon model, and workspace types have no Ratatui or egui dependency.

## 3. Constraints from the engineering rules

- The GUI is a client. It never opens SQLite; `vibemuxd`'s `WriterWorker` is
  the only writer.
- Lifecycle, status, events, and errors stay content-free. Vendor text reaches
  a client only through an explicit read.
- User messages are data and never enter a shell string.
- A writable Run owns its own worktree and branch. Worktrees are concurrency
  isolation, not a sandbox. Nothing merges automatically.
- `RunStatus::Succeeded` needs structured-adapter, verifier, or explicit user
  authority.
- No unbounded channels, no async mutex held across `.await`, no `unwrap` or
  `expect` on untrusted input; `#![forbid(unsafe_code)]` in library crates.
- New dependencies go through AGENTS.md §16 (license, maintenance, size,
  security history, `deny.toml`).
- Authored names use snake_case with no hyphens.
- Planned behavior is never described as implemented; `PROGRESS.md` changes
  with each implementation PR.

## 4. Stack

| | Tauri 2 + Svelte 5 (chosen) | egui rewrite | Electron |
|---|---|---|---|
| Text, markdown, IME, selection, a11y | browser engine | hand-built | browser engine |
| Toolchain | Rust + the existing Node 22 web toolchain | Rust only | Node + a Rust sidecar or a second IPC client |
| Daemon client | existing Rust client, in process | existing | reimplemented or proxied |
| Runtime size | system WebView2 | none | bundled Chromium, about 150 MB |
| Fit with Rust-core direction | yes | yes | no |

Versions checked on 2026-10-24 (crates.io and npm):

| Package | Version | License | Note |
|---|---|---|---|
| `tauri` | 2.12.5 | Apache-2.0 OR MIT | MSRV 1.90; the repo pins 1.92 |
| `tauri-build` | 2.7.4 | Apache-2.0 OR MIT | MSRV 1.90 |
| `@tauri-apps/api`, `@tauri-apps/cli` | 2.12.4 | Apache-2.0 OR MIT | |
| `svelte` | 5.57.2 | MIT | same as `vibemux_web` |
| `@sveltejs/vite-plugin-svelte` | 7.3.1 | MIT | same as `vibemux_web` |
| `vite` | 8.3.5 | MIT | `vibemux_web` pins 8.3.3; align both in one change |
| `markdown-it` | 15.0.3 | MIT | `html: false`, image rule disabled |
| `highlight.js` | 11.12.0 | BSD-3-Clause | core plus an explicit language list |
| `@lucide/svelte` | 1.61.0 | ISC | icons, tree-shaken |

`tauri` 3.0.0-alpha.5 exists but needs Rust 1.95, above the pinned toolchain,
and is not stable. It is out of scope.

`tauri-utils` pulls `kuchikiki` and `html5ever` through its build features, so
`cargo deny` is expected to need per-crate MPL-2.0 exceptions like the existing
`ece` entry. PR 3 (§10) records the exact list.

DOMPurify is not used: markdown-it with `html: false` escapes all raw HTML, a
custom `validateLink` accepts only `https:`, `http:`, and `mailto:` links (the
default would still allow `data:image` URLs), the image rule is off, and the
CSP forbids inline script. Tests pin each of these (§8). Clicking a link calls
`open_link`, which opens only `https://` URLs.

## 5. Phase 1 architecture

### 5.1 Crates

```text
vibemuxd → vibemux_client → vibemux_frontend (TUIs, dashboard; egui GUI until retired)
                          → vibemux_desktop  (Tauri 2 binary) ⇄ ui/ (Svelte 5)
```

The workspace dependency direction gains `vibemux_client` between `vibemuxd`
and the frontends. `vibemux_cli` is unchanged.

### 5.2 `vibemux_client`

A library crate with no GUI or terminal dependency. It owns every read of daemon,
probe, and Git state that a frontend needs. Moved from `vibemux_frontend` with
their tests:

- `daemon_session`, `daemon_control`, `cli_runner`, `project_folder`
- `supervisor_client`, `supervisor_mapping`, `supervisor_model`
- `ask/` (`reply_text`, `routes`, `session`, `worker`)
- `quick_search`
- `probe_env`, `view_model`
- `workspace_inventory`, plus `dashboard::daemon_feed`, `dashboard::daemon_model`,
  and the `Workspace`, `Branch`, and `safe_text` items they use
- the native folder picker, behind an optional `folder_dialog` feature that
  enables `rfd` on Windows

The repaint dependency becomes a narrow trait:

```rust
pub trait ChangeNotifier: Send + Sync {
    fn notify(&self);
}
```

The egui GUI implements it with `Context::request_repaint`; the desktop app implements it by waking its
event forwarder. `vibemux_frontend` re-exports the moved items during the
transition so the TUIs and the egui GUI build unchanged.

### 5.3 `vibemux_desktop` (Rust side)

Owns the window, the polling loops, the daemon credentials, and the settings
file. The webview never sees the control descriptor, the pipe, or a token.

Commands (phase 1). Every argument is validated: IDs parse as typed UUIDs and
strings have byte limits. The webview never passes a filesystem path: folders
come from the native folder dialog, run on the Rust side, or from the saved
project list.

| Command | Returns |
|---|---|
| `projects_list`, `project_pick()`, `project_select(project_key)`, `project_forget(project_key)` | project rows with daemon state; `project_pick` runs the native folder dialog |
| `daemon_start(project_key)`, `daemon_stop(project_key)` | job state; one job per project at a time |
| `tasks_snapshot(project_key)` | the existing `SupervisorSnapshot`: Control v4 bounded summaries |
| `supervisor_action(project_key, action)` | the existing `SupervisorAction`: refresh, load more, load task, and the ADR 028 WezTerm inspect, link, focus, and unlink actions |
| `task_window_open(project_key, task_id)` | opens a task window |
| `ask_targets(project_key)` | Claude Code and Codex availability with reasons |
| `ask_setup(project_key)` | writes default routes on this explicit call only, as today |
| `ask_confirm(project_key)` | records the once-per-folder paid-inference consent |
| `ask_submit(project_key, harness, prompt)`, `ask_cancel(ask_id)`, `ask_status(ask_id)` | Ask ID, phase, and end state, without content |
| `ask_reply(ask_id)` | the reply text; the only phase 1 command that returns vendor text |
| `console_section(project_key, section)`, `probe_rerun()` | content-free console data |
| `quick_search(query)` | at most 8 ranked entries |
| `open_link(url)` | opens an `https://` URL in the default browser; anything else is refused |
| `settings_get`, `settings_set(patch)` | settings |
| `about_info` | app and WebView2 versions |

Events to the UI carry no content: `projects_changed`,
`snapshot_changed { project_key, revision }`, `ask_changed { ask_id, phase }`,
and `console_changed { project_key, section }`. The UI fetches what it shows.

Polling keeps today's cadence: tasks every 5 s with `detail_revision` gating,
metadata every 12 ticks, and the Ask worker's existing cadence while an Ask
runs.

Settings live in `desktop_settings.json` (schema 1, snake_case keys) in the
Tauri app config directory: `appearance` (`light`, `dark`, `system`),
`text_size`, `density`, `reduce_motion`, `sidebar_collapsed`, `start_page`,
`project_folders` (at most 16), and `ask_confirmed_projects` (at most 64; the
once-per-folder Ask consent). There is no import from the egui GUI's settings.

The bundle identifier uses reverse-DNS form without hyphens or underscores,
for example `io.github.yuedongsun.vibemux`; PR 3 fixes it.

### 5.4 UI (`crates/vibemux_desktop/ui`)

Svelte 5, Vite (`vite_config.ts`), TypeScript, and Node 22.22.0. Logic lives in
plain TypeScript modules so it can be tested with `node --test`; components
stay thin.

- `src/lib/api_client.ts`: typed wrappers over `invoke` and `listen`
- `src/lib/markdown_render.ts`: markdown-it and highlight.js configuration
- `src/lib/key_bindings.ts`: one shortcut table for handling and the overlay
- `src/lib/format_text.ts`: times, IDs, sizes
- `src/lib/fixture_backend.ts`: dev-only fixture data labeled DEMO (§8)
- `src/components/*.svelte`: sidebar, composer, chat view, task panel,
  console tabs, settings, switcher, toasts

Shared tokens move to `config/design_tokens.css`, next to the exported
`config/theme_palettes.json`. `vibemux_web` and the desktop UI both import it.
This depends on `fix/web_design_tokens` reaching `main` first.

### 5.5 Webview lockdown

- Only bundled assets load. `devUrl` is used only by `tauri dev`.
- CSP in `app.security.csp`: `default-src 'self'; script-src 'self';
  style-src 'self'; img-src 'self'; connect-src ipc: http://ipc.localhost;
  object-src 'none'; base-uri 'none'; frame-ancestors 'none'`. PR 3 confirms
  the string against Tauri 2.12's own CSP injection.
- `withGlobalTauri` is off; `freezePrototype` is on.
- Every app command is declared in the `tauri_build` app manifest, so it is
  denied unless a capability grants it. One capability file,
  `capabilities/main_window.json`, grants exactly the command list in §5.3 plus
  event listening, for the main window and windows labeled `task_*`.
- No Tauri plugin is registered, so none is exposed to the webview. `open_link`
  checks the scheme and opens the default browser from Rust with the `open`
  crate (5.4.5, MIT).
- Navigation away from the app origin is refused.

### 5.6 Screens

```text
┌──────────────────────┬──────────────────────────────────────────────┐
│ ✳ VibeMux        [≡] │ VibeMux / Why is the probe cache stale?   ● │
│ + New chat    Ctrl+N │                                              │
│                      │        ┌ You ───────────────────────┐        │
│ ▾ VibeMux  ● running │        │ Why is the probe cache…    │        │
│   ✳ Probe cache  2m  │        └────────────────────────────┘        │
│     ADR 040 review   │   ✳ Claude Code · read-only · one turn       │
│ ▸ N_qubit  ○ stopped │     The cache is written by… (markdown,      │
│                      │     code blocks with copy)                   │
│                      │   ┌────────────────────────────────────────┐ │
│                      │   │ Reply…                                 │ │
│ Tasks Console Settings   │ [Claude Code ▾]                    (↑) │ │
└──────────────────────┴───┴────────────────────────────────────────┘─┘
```

- **Sidebar.** New chat (`Ctrl+N`) opens the welcome screen. Projects show
  their daemon state, with Start and Stop in a hover menu. Each project
  expands to the chats sent in this app session; in phase 1 these are Asks
  marked "one-shot · not saved". Footer: Tasks, Console, Settings. `Ctrl+B`
  collapses to an icon rail.
- **Chat (phase 1 = Ask).** The target chip lists Claude Code and Codex when
  detected with an executing route; other harnesses appear disabled with the
  reason. Send runs one read-only Ask with today's gates: route setup only on
  an explicit click and the existing paid-inference confirmation. A running
  Ask shows a rotating spark (static with Reduce motion) and Cancel. The end
  state is Completed, Unverified, Cancelled, or an error code. The reply
  renders as markdown with highlighted, copyable code blocks. Sending again
  starts a new Ask row, and the composer says so.
- **Tasks.** The project's Control v4 tasks with All, Running, Blocked, and
  Done filters. A task opens a resizable right panel with Overview, Activity,
  Artifacts, and Terminal tabs. "Open in window" opens a task window.
- **Console.** Tabs for Overview, Harnesses, Workspace, Tasks, Daemon,
  Sessions, and Decisions, with today's content-free reads.
- **Settings.** General (start page), Appearance (Light, Dark, Match system,
  text size, density, Reduce motion; the OS reduced-motion setting is
  honored), Connection, Shortcuts, About.
- **Everywhere.** `Ctrl+K` switcher over projects, chats, tasks, and actions;
  `Ctrl+/` shortcut overlay; `Ctrl+,` Settings; a top bar with breadcrumbs and
  a connection dot; toasts that always show the error code.

Visual language: warm canvas from the shared tokens; Georgia/Cambria serif for
the greeting and titles, Segoe UI for the interface, Cascadia Code for code; a
centered conversation column about 760 px wide; a 20 px composer radius with a
soft shadow; hover tints instead of borders; 150 to 200 ms transitions.

### 5.7 Parity checklist (retiring the egui GUI)

Carried over:

- welcome state and new chat (was New task), with the draft-discard prompt
- sidebar collapse and the icon rail
- project folders: open, remember, forget, missing-folder error, daemon
  Start/Stop with one job at a time, daemon card details
- task list with filters, status dots, and freshness label
- task panel tabs and aligned run details; copyable IDs, branches, and paths
- task windows
- WezTerm link, focus, and unlink
- Ask: targets, setup, consent, submit, cancel, end states, reply
- Console: all seven sections
- Settings: all five sections
- `Ctrl+K`, `Ctrl+/`, `Ctrl+N`, `Ctrl+B`, `Ctrl+,`, `Esc`; ignored during IME
  composition
- the DEMO tag whenever fixture data is shown
- light and dark Claude themes; Match system

Dropped on purpose: the five non-Claude palettes in the GUI, and the
`Ctrl+1`…`Ctrl+0` agent shortcuts (the Agents page no longer exists).

## 6. Phase 2 — harness chat (shape)

This section fixes the shape so phase 1 does not block it. Its own ADR, with
Control v11 and store schema 10, comes before implementation, and pins exact
vendor flags against the installed versions.

### 6.1 Canonical mapping

- A chat is a canonical Task with one writable Run. The Run owns the chat's
  worktree and branch (`chat/<short_id>`) for the chat's lifetime, created
  through `WorkspaceManager`.
- Each turn is a set of events on that Run.
- Archiving stops the Run. A chat never claims `Succeeded`. Nothing merges
  automatically.

### 6.2 Daemon service

| Operation | Effect |
|---|---|
| `chat_open(project, harness, base)` | worktree, Task, and Run; returns the chat ID |
| `chat_send(chat_id, text)` | starts a turn |
| `chat_cancel_turn(chat_id)` | protocol cancellation, then process cleanup |
| `chat_poll(chat_id, after_sequence, wait_ms ≤ 1000)` | waits up to 1 s for records |
| `chat_decide(chat_id, approval_id, allow \| deny)` | answers a held approval; a second answer gets `approval_already_decided` |
| `chat_list(project, include_titles)` | content-free rows; titles only when asked |
| `chat_archive(chat_id)` | stops the Run; keeps the worktree |
| `chat_purge(chat_id)` | deletes transcript content; keeps content-free events |
| `chat_remove_worktree(chat_id)` | explicit, through the existing checked cleanup |

`vibemuxctl chat …` exposes the same operations for scripting and tests.

### 6.3 Vendor processes

- **Codex:** one app-server thread per chat: `thread/start` once, `turn/start`
  per message, `thread/resume` after a restart.
- **Claude:** one stream-json process per chat with stdin left open; resume with
  `--resume <session_id>`. Chat mode drops `--no-session-persistence`.
- An idle process exits after 10 minutes; the next message resumes it.
- At most 3 live chat processes per project.
- Job Object containment, per-turn deadlines, and frame and record caps stay.

### 6.4 Approvals

- In chat mode the engine holds approval requests instead of declining them.
- It records a content-free `chat_approval_requested` event with the kind
  (`command`, `file_change`, `other`) and a transcript record with the
  details (command text, or file path and diff).
- The UI shows Allow once and Deny inline.
- An unanswered request is denied after 30 minutes, and the expiry is recorded.
- Only control-pipe clients (the desktop app and `vibemuxctl`) can answer. The
  mobile web surface stays read-only.
- Every decision is an event recorded on the operator's authority.

### 6.5 Certification gate (ADR 036)

- A harness runs writable chats only after a recorded live check on its
  installed version shows that it asks before every edit and command and obeys
  Deny. Until then its chats run read-only and the header says so.
- Repository auto-approval stays blocked: `--strict-mcp-config` and
  `--restricted` for Claude; a workspace-write sandbox rooted at the worktree
  for Codex.
- The live check sends real prompts and runs only with the operator's explicit
  authorization. Its results go to `docs/evidence/`.
- A worktree is not a sandbox: the harness can read anything the user can and
  has whatever network access the vendor allows.

### 6.6 Persistence

- `chat_sessions`: IDs, harness, branch, vendor session or thread ID, state,
  timestamps.
- `chat_records`: one row per piece of a turn: user message, assistant text
  block, tool call, tool result, approval request, decision, end of turn.
- Titles are content: derived from the first line of the first message and
  returned only when a client asks.
- Limits: 256 KiB per record (truncated with a visible marker) and 32 MiB per
  chat (a full chat refuses new turns).
- During a turn, text streams from an in-memory tail and is saved when each
  block ends. A daemon crash loses the partial block and marks the turn
  interrupted.
- All writes go through the single `WriterWorker`.

### 6.7 Chat screen

- The phase 1 Ask rows become chats; the composer stays enabled for follow-ups;
  Stop cancels the turn.
- The header shows the harness, the branch, and a "writable · approvals" or
  "read-only" badge.
- Tool calls render as collapsible cards (command and output, or edited files
  with diffs); approval cards appear inline.
- The chat menu has Rename, Archive, Open worktree folder, Copy branch, and
  Remove worktree (with confirmation). There is no merge button.

### 6.8 Questions for the phase 2 ADR

- Exact Codex approval and sandbox policy values, and Claude's permission
  prompt mechanism, for the installed versions.
- Whether to add "Allow for this chat".
- The default base for `chat_open` (current branch head is the proposal).
- How chats appear on the Tasks page (a `chat` filter is the proposal).

## 7. Error handling

- Every command returns a value or `{ code, message }` with the daemon's error
  code. The UI shows a toast with the code and never reports success it did
  not observe.
- Each project row shows its daemon state: stopped, starting, unreachable, or
  recoverable. Recovery stays manual: the app shows the
  `vibemuxctl daemon recover` step and never automates the SHA-256
  confirmation.
- An older daemon shows the missing capability by name: Control v4 for tasks,
  v5 for Ask, v11 for chat.
- After a daemon restart, an open chat reloads saved history and marks an
  interrupted turn.
- A webview reload rebuilds state from the Rust side; only an unsent draft is
  lost.

## 8. Testing

- **`vibemux_client`:** the moved unit tests and real-daemon feed tests. The
  egui GUI and TUI tests stay green, which shows the extraction changed no
  behavior.
- **`vibemux_desktop` (Rust):** command handlers tested as plain functions
  against a real test daemon, without a webview. A lockdown test parses
  `tauri.conf.json` and the capability files and fails on a remote URL, an
  extra plugin, a capability broader than the command list, or a weakened CSP.
- **UI:** `svelte-check`; `node --test` for the markdown renderer (raw HTML
  escaped, links other than `https:`, `http:`, and `mailto:` dropped, images
  off), key bindings, formatting, and the phase 2 record reducer.
- **Fixture build guard:** a test fails if the production `dist` contains the
  fixture backend's marker string.
- **Design review loop:** `vite dev` in a normal browser with the fixture
  backend. A local Playwright script captures every screen in light and dark;
  the captures replace `docs/frontend_previews/`. Playwright is a dev-only
  dependency and is not in CI at first.
- **Tokens:** a parity test pins `config/design_tokens.css` to
  `config/theme_palettes.json` and checks WCAG AA for every text pair.
- **Phase 2:** `vibemux_native_fixture` gains multi-turn streams and approval
  requests in the Codex and Claude shapes; tests cover deny, expiry, cancel,
  resume after restart, and the limits; a migration test covers schema 9 to 10.

Automated tests prove contracts and layout at tested sizes. They do not prove
WebView2 behavior on every Windows build, IME behavior with every input method,
or vendor behavior. Those are recorded separately as live evidence.

## 9. CI, dependencies, and disk

- CI builds the desktop UI the same way it builds `vibemux_web`, before the
  Rust steps.
- The Ubuntu Rust job installs Tauri's WebKitGTK development packages. The app
  targets Windows; Linux only has to build and pass tests.
- `vibemux_desktop`'s `build.rs` requires `ui/dist`, so workspace-wide cargo
  commands can `--exclude vibemux_desktop` when the UI is not built.
- `cargo deny` and `npm audit` must pass; new exceptions are per crate with a
  reason.
- Disk: on 2026-10-24 C: had 2.2 GB free and I: 15 GB. A Tauri build plus
  `node_modules` needs several more GB; free space before PR 3.

## 10. Rollout

Phase 1 PRs, in order:

1. Extract `vibemux_client` with no behavior change.
2. Move the tokens to `config/design_tokens.css` with the parity test (after
   `fix/web_design_tokens` merges).
3. `vibemux_desktop` skeleton: window, settings, lockdown, CI, `cargo deny`
   exceptions, measured build time and binary size; ADR 043 accepted.
4. Screens: sidebar and projects, chat (Ask), tasks, console, settings,
   switcher, toasts.
5. Parity check against §5.7, then remove the egui GUI and its binary in a
   separate PR. The TUIs stay.

The egui GUI is frozen during phase 1: bug fixes only.

Phase 2 PRs: store schema 10; protocol engine multi-turn and approval hold
(fixtures); daemon service, Control v11, and `vibemuxctl chat`; desktop chat
UI; live certification checks with authorization.

`PROGRESS.md` changes in every implementation PR. Status stays
`PARTIAL — FIXTURE ONLY` until live evidence exists.

## 11. Risks

- **WebView2 missing or old** on some Windows 10 machines. Windows 11 ships the
  evergreen runtime. The About page reports the runtime version.
- **Build cost.** Tauri adds many crates and compile time. PR 3 measures and
  records both.
- **Two GUIs during phase 1.** Freezing the egui GUI limits the cost.
- **Vendor protocol drift.** Approval method names and flags change between
  vendor releases; fixtures pin the shapes we support, and certification is
  per installed version.
- **Disk space** blocks builds until freed.

## 12. Alternatives

- **Rewrite in egui:** rejected; the rendering limits are the problem.
- **Electron:** rejected; it needs a second control-pipe client or a sidecar,
  bundles Chromium, and moves away from the Rust core.
- **Serve the UI from `vibemux_web` in a browser:** rejected; it is read-only
  and bearer-authenticated by design, and approvals over an HTTP listener widen
  the attack surface.
- **Other native toolkits (Slint, Iced, GPUI):** not chosen; each would need
  markdown, code highlighting, selection, and IME work that the webview already
  provides, and GPUI's API is not stable.
- **Tauri's isolation pattern:** not needed in phase 1; the webview loads no
  remote content and the command surface is small. Reconsider if third-party
  UI dependencies grow.
