# ADR 043: Tauri desktop app replaces the egui GUI

Status: Proposed (draft, 2026-10-24). Becomes Accepted with the
`vibemux_desktop` skeleton PR (design §10, PR 3). The design and the operator's
decisions are in [docs/desktop_app_design.md](../desktop_app_design.md).

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
- Every app command is declared in the app manifest and granted by one
  capability file for the main window and `task_*` windows. No Tauri plugin is
  registered, so no `fs`, `shell`, `http`, or `opener` plugin can reach the
  webview; `open_link` opens `https://` URLs from Rust with the `open` crate.
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
