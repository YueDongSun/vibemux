# Multi-pane Console and UI Functions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement ADR 042: a daemon-backed multi-pane console in `vibemuxctl ui`/`vibemux_dashboard`, re-probe and selection in the diagnostic TUI, transcript search in the DSH TUI, and the ADR 030 phase 2 shortcut table and quick switcher in the GUI.

**Architecture:** Pure state, filtering, projection, ranking, and search logic lives in egui-free and I/O-free modules with unit tests. The daemon feed owns one background thread and publishes an immutable `DaemonView` that renderers consume through `dashboard::Context`. The GUI client's connection cache moves to `daemon_session.rs`, shared by the GUI and the feed.

**Tech Stack:** Rust 1.92.0 (edition 2024, let chains allowed), ratatui, crossterm, egui/eframe, tokio, the `vibemuxd` control client.

**Spec:** `docs/adr/042_multi_pane_console_and_ui_functions.md` (and `docs/adr/030_claude_style_gui.md` §5 for the GUI subset).

## Global Constraints

- Every new Rust file starts with `#![forbid(unsafe_code)]` where it is a crate root or follows the module's existing header style.
- snake_case for every new file, module, function, variable, and JSON key; no hyphens in project-controlled names.
- Keys never start a daemon, harness, dispatch, DSH session, workflow, or decision. The feed calls only `health`, `plugin_status`, `frontend_tasks`, `frontend_task`, `harness_dispatch_catalog`, `dsh_session_status`, `workflow_slots`, `decision_list`.
- Every daemon string reaches a renderer only through `dashboard::safe_text`.
- No new control operation, protocol version, schema, or persisted setting. `--once` output of `vibemuxctl ui` and `vibemux_dashboard` is unchanged.
- No unbounded channels; the feed's request channel has capacity 8.
- Send stays unavailable in the GUI; Native TUI attach stays disabled.
- The diagnostic TUI golden fixtures stay byte-identical.
- Commits: plain messages, no Claude attribution or Co-Authored-By trailer.
- Windows builds: `CARGO_TARGET_DIR` under `%TEMP%`, `OPENSSL_SRC_PERL=F:/texlive/2025/tlpkg/tlperl/bin/perl.exe`, `cargo +1.92.0 ... -j 6 --locked`.

## Review Focus

1. A daemon that stops or restarts while the console is open must turn its panes into `daemon_not_running` and recover on the next poll, never freeze the UI. Test: Task 5 `feed_reports_not_running_then_recovers_after_start`.
2. A filter that hides the selected row must not leave the selection out of bounds or panic on `Enter`. Test: Task 2 `filter_clamps_selection_and_enter_on_empty_list_is_noop`.
3. Grid on a small terminal (80x24) must fall back instead of rendering unreadable panes. Test: Task 1 `layout_falls_back_by_terminal_size`.
4. Control characters or bidi overrides in task titles, plugin errors, or route labels must not reach the screen. Test: Task 3 `daemon_strings_are_sanitized`.
5. In the DSH TUI, `Esc` while search is open must close search and never cancel a running turn. Test: Task 7 `esc_in_search_closes_search_without_consuming_turn_cancel`.

---

### Task 1: Views, layouts, and focus state

**Files:**
- Modify: `crates/vibemux_frontend/src/dashboard/state.rs`
- Create: `crates/vibemux_frontend/src/dashboard/panes.rs`
- Modify: `crates/vibemux_frontend/src/dashboard/mod.rs`

**Interfaces:**
- Produces:
  - `Tab` gains `Tasks`, `Daemon`, `Sessions`, `Decisions` (indices 3-6); `Tab::ALL: [Tab; 7]`, `Tab::label(self) -> &'static str` ("Overview", "Harnesses", "Workspace", "Tasks", "Daemon", "Sessions", "Decisions"), `Tab::is_daemon(self) -> bool`.
  - `panes::LayoutMode { Single, Split, Grid }` with `next(self)`.
  - `panes::effective_layout(mode: LayoutMode, width: u16, height: u16) -> LayoutMode`: Grid needs width >= 120 and height >= 32, else Split; Split needs width >= 90, else Single.
  - `panes::PaneState { mode: LayoutMode, views: [Tab; 4], focus: usize, zoomed: bool }`, default views `[Overview, Tasks, Daemon, Sessions]`, `pane_count(mode) -> usize` (1, 2, 4), `focused_view(&self, effective: LayoutMode) -> Tab`.
  - `State` gains `panes: PaneState`, `selected_task: usize`, `selected_decision: usize`, `opened_task: Option<usize>`; `State::view(&self, width, height) -> Tab` returns `tab` in Single or zoomed, else the focused pane's view.
  - `pub enum Command { Continue, Quit, Refresh, OpenTask(usize), CycleTheme }` and `pub struct ListCounts { agents, branches, tasks, decisions: usize }`.
  - `State::handle_key(&mut self, key: KeyEvent, counts: ListCounts, size: (u16, u16)) -> Command` replaces the old signature.

- [ ] **Step 1: Write the failing tests** in `state.rs`/`panes.rs`:
  - `layout_falls_back_by_terminal_size`: `effective_layout(Grid,120,32)==Grid`, `(Grid,119,40)==Split`, `(Grid,100,24)==Split`, `(Split,89,40)==Single`, `(Grid,80,24)==Single`.
  - `tab_and_digits_follow_the_layout`: in Single, `Tab` moves `tab` Overview→Harnesses and `4` selects Tasks; after `L` at 130x40 (Split), `Tab` moves `panes.focus` 0→1 and `7` sets `panes.views[1]=Decisions` without changing `tab`; `z` toggles `zoomed` and `view()` then returns the focused view.
  - `keys_only_request_reads`: `r` returns `Command::Refresh`, `t` returns `CycleTheme`, `Enter` on Tasks with 3 tasks and `selected_task=2` returns `OpenTask(2)`, `q` returns `Quit`; the existing `navigation_is_bounded_and_never_executes_an_action` assertions keep passing under the new signature.
- [ ] **Step 2: Run** `cargo test -p vibemux_frontend dashboard::` — expect compile failure / FAIL.
- [ ] **Step 3: Implement** the types above; `j`/`k` move the selection of `view()`'s list (agents for Overview/Harnesses, branches for Workspace, tasks, decisions; Daemon and Sessions have none). Update the existing `Tab::from_index` to modulo 7 and the existing render match arms so the crate compiles (daemon tabs render a placeholder until Task 4).
- [ ] **Step 4: Run** `cargo test -p vibemux_frontend dashboard::` — expect PASS, including every existing dashboard test.
- [ ] **Step 5: Commit** `feat(frontend): add console views, layouts, and pane focus state`.

### Task 2: Filter

**Files:**
- Create: `crates/vibemux_frontend/src/dashboard/filter.rs`
- Modify: `crates/vibemux_frontend/src/dashboard/state.rs`

**Interfaces:**
- Produces: `filter::FilterState { queries: [String; 7], editing: Option<Tab> }`, `filter::matches(query: &str, fields: &[&str]) -> bool` (empty query matches; case-insensitive substring via `to_lowercase`), `FilterState::query(&self, tab) -> &str`, `MAX_FILTER_CHARS = 64`. `State` gains `filter: FilterState`. `DashboardModel::filtered_agents(&self, query) -> Vec<&AgentView>` (match on name, launcher state, adapter label).
- Keys: `/` sets `editing = Some(view)`; while editing, printable characters append (bounded), `Backspace` pops, `Enter` stops editing (keeps query), `Esc` clears the query and stops editing; `q` is a character while editing.

- [ ] **Step 1: Failing tests:** `matches_is_case_insensitive_substring` (`matches("cod", &["Codex"])`, `!matches("x", &["Claude"])`, `matches("", &[])`); `filter_clamps_selection_and_enter_on_empty_list_is_noop` (Tasks view, `selected_task=2`, filter applied so `counts.tasks=0`: `j` leaves `selected_task=0`, `Enter` returns `Continue`); `typing_q_while_editing_does_not_quit`; `esc_clears_filter_before_quitting`.
- [ ] **Step 2: Run** `cargo test -p vibemux_frontend dashboard::filter dashboard::state` — FAIL.
- [ ] **Step 3: Implement.** Selection indices refer to the filtered list; `handle_key` clamps them to `counts` on every call.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(frontend): filter console lists`.

### Task 3: Daemon view model (pure)

**Files:**
- Create: `crates/vibemux_frontend/src/dashboard/daemon_model.rs`

**Interfaces:**
- Produces:
  - `pub enum Section<T> { Waiting, Loaded(T), Unavailable(Unavailable) }`; `pub struct Unavailable { pub code: String, pub hint: String }`; `pub fn unavailable(code: &str) -> Unavailable` mapping `daemon_not_running` → "start it with vibemuxctl daemon start", `control_unsupported_version` → "unsupported by this daemon", `harness_dispatch_unconfigured|dsh_session_unconfigured|workflow_unconfigured|decision_not_configured` → "not configured", `dsh_session_not_found` → "no DSH session is open", other → "see vibemuxctl daemon health".
  - Row types, all `String` fields already passed through `safe_text`: `HealthView { healthy, schema, queue }`, `PluginRow { plugin_id, state, restarts, last_error }`, `RouteRow { harness, protocol, enabled, executes, probe }`, `TaskRow { task_id, title, status }`, `TaskDetailView { task_id, runs: Vec<String>, events: Vec<String>, artifacts: Vec<String>, truncated: bool }`, `DshView { phase, provider, model, turns: String, transport_open: bool }`, `SlotsView { evidence_class, rows: Vec<String> }`, `DecisionRow { decision_id, status, chosen, source, created }`.
  - `pub struct DaemonView { pub connection: String, pub observed_at: Option<String>, pub health, plugins, routes, tasks, task_detail: Option<Section<TaskDetailView>>, dsh, slots, decisions }` with `DaemonView::waiting()` and `DaemonView::not_running(code)`.
  - Projection functions `project_health(&DaemonHealth)`, `project_plugins(&[PluginStatus])`, `project_routes(&[DispatchCatalogEntry])`, `project_tasks(&FrontendTasksPage)`, `project_task_detail(&FrontendTaskDetail)`, `project_dsh(&DshSessionStatus)`, `project_slots(&serde_json::Value)` (reads `evidence_class` and each `routes[]` entry's `slot_id`, `route_label`, `executes`; ignores other fields), `project_decision(&DecisionView)`.
  - `Context` gains `pub daemon: Option<std::sync::Arc<DaemonView>>` (`None` = no feed).

- [ ] **Step 1: Failing tests:** `unavailable_codes_get_operator_hints` (each code above → its hint); `daemon_strings_are_sanitized` (a task title and a plugin `last_error_code` containing `\u{1b}[31m` and `\u{202e}` project to strings without either); `slots_projection_reads_only_route_fields` (a JSON value with an extra `"prompt":"secret"` field projects without "secret" anywhere in `format!("{:?}")`); `task_detail_marks_truncation` (`has_more_events: true` → `truncated`).
- [ ] **Step 2: Run** `cargo test -p vibemux_frontend dashboard::daemon_model` — FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(frontend): project daemon reads into a content-free console view`.

### Task 4: Console rendering, layouts, and themes

**Files:**
- Create: `crates/vibemux_frontend/src/dashboard/daemon_render.rs`
- Modify: `crates/vibemux_frontend/src/dashboard/render.rs`, `crates/vibemux_frontend/src/dashboard/mod.rs` (tests)

**Interfaces:**
- Consumes: Tasks 1-3.
- Produces: `render_with_state` keeps its signature; it reads `state.panes`, `state.filter`, `state.theme: Theme` (new `State` field, default `Theme::from_environment()` at runtime, `Theme::Classic` in `State::default()`), and `context.daemon`. `daemon_render::render_view(frame, area, view: Tab, daemon: Option<&DaemonView>, state, theme)` draws the four daemon views; `Section::Unavailable` renders `"<label>: unavailable (<code>) - <hint>"`, `Waiting` renders `"<label>: waiting for the daemon"`, and `context.daemon == None` renders `"Daemon views need the interactive console"`.
- Layout: title, a 7-tab bar, the body (Single: the existing view plus the native slot panel; Split: two panes side by side; Grid: 2x2; zoomed: the focused pane as Single), and a footer naming the layout, theme, filter, and keys. Each pane is a bordered block titled `"<n> <label>"`; the focused pane's border uses `title_style(theme)`; a filtered list's title adds `filter: <query>`.

- [ ] **Step 1: Failing tests** (TestBackend via `snapshot`):
  - `every_view_renders_in_every_layout` for views 1-7, layouts Single/Split/Grid at 130x40 and 80x24, with a loaded fixture `DaemonView` and with `DaemonView::not_running("daemon_not_running")`: rendered text contains each pane's label, `READ ONLY`, and for not-running `"vibemuxctl daemon start"`.
  - `grid_shows_four_titled_panes` at 130x40: contains "1 Overview", "4 Tasks", "5 Daemon", "6 Sessions".
  - `filter_title_and_rows` : Tasks filter "alp" shows "filter: alp", task "alpha", hides "beta".
  - `no_feed_message_without_daemon_view`.
  - `help_lists_every_console_key`: with `help_visible`, the overlay names `L`, `z`, `r`, `/`, `t`, `1-7`, `Tab`, `q`.
  - The existing dashboard tests stay unchanged and pass.
- [ ] **Step 2: Run** `cargo test -p vibemux_frontend dashboard::` — FAIL.
- [ ] **Step 3: Implement**; replace hard-coded `Color::Cyan`/`Color::Yellow` with `tui::theme` styles.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(frontend): render console panes, layouts, and themes`.

### Task 5: Shared daemon session and feed

**Files:**
- Create: `crates/vibemux_frontend/src/daemon_session.rs`, `crates/vibemux_frontend/src/dashboard/daemon_feed.rs`, `crates/vibemux_frontend/tests/daemon_feed.rs`
- Modify: `crates/vibemux_frontend/src/supervisor_client.rs` (use `daemon_session`; behavior unchanged), `crates/vibemux_frontend/src/lib.rs`, `crates/vibemux_frontend/Cargo.toml` (dev-dependencies only if the test needs `vibemux_store`/`uuid`)

**Interfaces:**
- Produces:
  - `pub(crate) struct DaemonSession` with `connect(&mut self, root: &Path) -> Result<(ControlClient, bool), String>` and `invalidate(&mut self)` (moved verbatim from `ClientSession`/`client_for`/`read_published`/`verify_runtime`).
  - `pub struct DaemonFeed` with `DaemonFeed::start(project_root: PathBuf) -> io::Result<Self>`, `latest(&mut self) -> Option<Arc<DaemonView>>`, `request(&self, request: FeedRequest) -> bool` (`try_send`; false when full), `Drop` stops and joins. `pub enum FeedRequest { Refresh, OpenTask(String) }`; an opened task stays open until another one is opened.
  - Constants: `POLL_INTERVAL = 5s`, `REQUEST_CAPACITY = 8`, `MAX_DECISIONS = 64`, `MAX_DECISION_PAGES_PER_POLL = 4`, `DECISION_PAGE_LIMIT = 50`.

- [ ] **Step 1: Failing integration tests** in `tests/daemon_feed.rs` (pattern: `crates/vibemux_web/tests/control_bridge.rs`, `DaemonPaths::from_project_root` + `DaemonControlServer::start_for_paths`):
  - `feed_reads_a_real_daemon_without_writing`: seed one task, start the server, start the feed, wait (20 s deadline) for `tasks` to be `Loaded` with one row and `health` `Loaded`; `routes`, `dsh`, `slots`, `decisions` are `Unavailable` with "not configured"; the database file's task count and descriptor bytes are unchanged afterwards.
  - `feed_reports_not_running_then_recovers_after_start`: start the feed first, observe `connection` containing `daemon_not_running`; start the server; send `FeedRequest::Refresh`; observe `tasks` `Loaded` within 20 s.
- [ ] **Step 2: Run** `cargo test -p vibemux_frontend --test daemon_feed` — FAIL.
- [ ] **Step 3: Implement.** Each read maps its `ControlError` with `.code()` into `Section::Unavailable(unavailable(code))`; a connection failure produces `DaemonView::not_running(code)` and invalidates the session. Decisions: keep `after_sequence` from the last seen `created_sequence`, read up to 4 pages per poll, keep the newest 64.
- [ ] **Step 4: Run** the new test plus `cargo test -p vibemux_frontend` — PASS (the GUI tests prove the refactor kept behavior).
- [ ] **Step 5: Commit** `feat(frontend): poll content-free daemon state for the console`.

### Task 6: Console runtime and entry points

**Files:**
- Modify: `crates/vibemux_frontend/src/dashboard/runtime.rs`, `crates/vibemux_frontend/src/bin/vibemux_dashboard.rs`, `crates/vibemux_cli/src/application.rs`, `crates/vibemux_cli/src/command.rs` (help text only)

**Interfaces:**
- Consumes: Tasks 1-5.
- Produces: `run_interactive(model: DashboardModel, context: Context, feed: Option<DaemonFeed>) -> io::Result<()>`; it copies `feed.latest()` into `context.daemon` each loop, maps `Command::Refresh` → `FeedRequest::Refresh`, `OpenTask(i)` → `FeedRequest::OpenTask(filtered task id)`, `CycleTheme` → `state.theme = state.theme.next()`. `vibemux_dashboard` accepts `--project-root <path>` (default current directory). `vibemuxctl ui` starts the feed with its resolved project root. Help texts list `L`, `z`, `r`, `/`, `t`, `1-7`.

- [ ] **Step 1: Failing tests:** in `vibemux_dashboard.rs` a `parse_arguments` unit test for `--project-root` (missing value is a usage error); in `vibemux_cli` the existing ui help/parse tests updated for the new help text.
- [ ] **Step 2: Run** `cargo test -p vibemux_frontend --bin vibemux_dashboard` and `cargo test -p vibemux_cli` — FAIL.
- [ ] **Step 3: Implement.** `--once` paths do not start a feed.
- [ ] **Step 4: Run** — PASS; also `vibemuxctl --format text ui --once` prints the same lines as before.
- [ ] **Step 5: Commit** `feat(cli): open the live console from vibemuxctl ui`.

### Task 7: DSH transcript search and Home/End

**Files:**
- Create: `crates/vibemux_frontend/src/dsh_tui/search.rs`
- Modify: `crates/vibemux_frontend/src/dsh_tui/mod.rs`, `crates/vibemux_frontend/src/dsh_tui/render.rs`

**Interfaces:**
- Produces: `TranscriptSearch { open: bool, query: String, current: Option<usize> }` in `DshTuiState.search`; `find_older(lines: &[TranscriptLine], query: &str, before: usize) -> Option<usize>`, `find_newer(lines, query, after: usize) -> Option<usize>` (case-insensitive); `pub fn handle_search_key(state: &mut DshTuiState, key: KeyEvent) -> bool` returns true when consumed; `scroll_to(state, index)` sets `state.scroll = len - 1 - index`.
- Keys: `Ctrl-F` opens; while open, characters edit the query (max 128 bytes) and jump to the newest match, `Enter`/`Down` → older match, `Up` → newer match, `Esc` closes; `Home` → oldest line, `End` → `scroll = 0`.

- [ ] **Step 1: Failing tests:** `search_moves_between_matches_and_scrolls` (lines "alpha","beta","alpha2": query "alpha" lands on index 2, `Enter` → 0, `Up` → 2); `esc_in_search_closes_search_without_consuming_turn_cancel` (`handle_search_key` consumes `Esc` only while open, and returns false for `Esc` when closed); `home_and_end_jump`; `search_status_is_rendered` (the status line shows `search: alpha` and the matched line carries a `>` prefix).
- [ ] **Step 2: Run** `cargo test -p vibemux_frontend dsh_tui::` — FAIL.
- [ ] **Step 3: Implement**; `handle_key` calls `handle_search_key` first.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(frontend): search the DSH transcript`.

### Task 8: Diagnostic TUI re-probe and selection

**Files:**
- Create: `crates/vibemux_frontend/src/tui/state.rs`, `crates/vibemux_frontend/src/tui/reprobe.rs`
- Modify: `crates/vibemux_frontend/src/tui/mod.rs`, `crates/vibemux_frontend/src/tui/render.rs`, `crates/vibemux_frontend/src/tui/footer.rs`, `crates/vibemux_frontend/src/bin/vibemux_frontend_tui.rs`

**Interfaces:**
- Produces: `DebugState { selected: usize, probing: bool, theme: Theme }` with `handle_key(&mut self, key, agent_count) -> DebugCommand { Continue, Quit, Snapshot, Reprobe }` (`r` returns `Reprobe` only when `!probing`); `Reprobe::start(config: ProbeConfig) -> io::Result<Reprobe>` and `try_take(&mut self) -> Option<ProbeReport>` (one worker thread with a current-thread runtime, one result over a capacity-1 channel); `render_debug_dashboard(frame, model, theme, selected: Option<usize>)` (goldens pass `None`); `render_footer(frame, model, theme, selected: Option<usize>, probing: bool)`; `run_tui(view_model: ViewModel, theme: Theme, config: Option<ProbeConfig>)`.

- [ ] **Step 1: Failing tests:** `r_requests_one_probe_at_a_time`; `selection_wraps_and_q_quits`; `selected_row_has_marker_and_footer_details` (TestBackend 120x32: "> " before the selected harness name, footer line contains `launcher=` and the harness name, footer title contains "r re-probe"); `probing_footer_says_probing`. The golden test still passes untouched.
- [ ] **Step 2: Run** `cargo test -p vibemux_frontend tui::` — FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p vibemux_frontend tui::` — PASS, goldens unchanged (`git diff --exit-code crates/vibemux_frontend/tests/fixtures`).
- [ ] **Step 5: Commit** `feat(frontend): re-probe and select harnesses in the diagnostic TUI`.

### Task 9: GUI quick search, shortcuts, and switcher

**Files:**
- Create: `crates/vibemux_frontend/src/quick_search.rs`, `crates/vibemux_frontend/src/gui/shortcuts.rs`, `crates/vibemux_frontend/src/gui/quick_switcher.rs`
- Modify: `crates/vibemux_frontend/src/gui/supervisor_app.rs`, `crates/vibemux_frontend/src/gui/mod.rs`, `crates/vibemux_frontend/src/lib.rs`

**Interfaces:**
- Produces:
  - `quick_search::{SearchEntry { label: String, detail: String, target: SearchTarget }, SearchTarget { Task(String), Agent(usize), Action(QuickAction) }, QuickAction { NewTask, Settings, Diagnostics, KeyboardShortcuts, ToggleSidebar, Light, Dark }, build_entries(tasks: &[TaskView], agent_names: &[String]) -> Vec<SearchEntry>, rank<'a>(entries: &'a [SearchEntry], query: &str) -> Vec<&'a SearchEntry>, MAX_RESULTS = 8}`. Rank: label prefix (0) < label word-start (1) < label substring (2) < detail substring (3); ties keep source order; empty query keeps source order (tasks, actions, agents).
  - `shortcuts::{Shortcut { key: egui::Key, label: &'static str, action: ShortcutAction }, ShortcutAction { QuickSwitcher, NewTask, ToggleSidebar, Settings, Overlay }, SHORTCUTS: [Shortcut; 5], render_overlay(ctx, open: &mut bool, colors: &C)}`; the overlay also lists `Esc` and `Ctrl+1..0`.
  - `quick_switcher::{QuickSwitcherState { open, query, selected }, show(ctx, state, entries, colors) -> Option<SearchTarget>}`.
  - `SupervisorApp` consumes the chords with `input_mut(|i| i.consume_key(Modifiers::COMMAND, key))` before panels render, skipping during IME composition; applies `SearchTarget` (task → open drawer, agent → `select_agent`, actions → existing setters; Light → `ThemeId::ClaudeLight`, Dark → `ThemeId::Claude`).

- [ ] **Step 1: Failing tests:** `prefix_beats_word_start_beats_substring`; `ranking_handles_cjk_ids_and_caps_results` (CJK title prefix match, task id substring via detail, 20 entries → 8); `empty_query_lists_tasks_then_actions_then_agents`; `shortcut_table_has_no_duplicate_chords`; in `supervisor_app.rs` tests with the existing `key_event` helper: `ctrl_k_opens_the_switcher`, `ctrl_b_toggles_the_sidebar`, `ctrl_slash_opens_the_overlay`, `global_shortcuts_are_ignored_during_ime_composition`, `send_stays_disabled_after_shortcuts`.
- [ ] **Step 2: Run** `cargo test -p vibemux_frontend quick_search gui::` — FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(gui): add the shortcut table, overlay, and quick switcher`.

### Task 10: Documentation and status

**Files:**
- Modify: `docs/adr/041_unified_cli_and_workspace_layout.md` (pointer to ADR 042), `docs/adr/030_claude_style_gui.md` (phase 2 delivery note: shortcuts and switcher done; appearance, motion, copy buttons remain), `README.md` (console keys, diagnostic TUI `r`/`j`/`k`, DSH `Ctrl-F`/`Home`/`End`, GUI shortcuts), `CLAUDE.md` (inspector and DSH command lines), `docs/architecture.md` (frontend module list, if it lists dashboard modules), `PROGRESS.md` (dated entry; frontend row text; status stays `PARTIAL`).

- [ ] **Step 1:** Edit the docs; every command and key named matches the code.
- [ ] **Step 2:** Commit `docs: record ADR 042 console and UI functions`.

### Task 11: Verification

- [ ] Windows: `cargo +1.92.0 fmt --all -- --check`; `cargo +1.92.0 clippy --workspace --all-targets --all-features --exclude vibemux_web -j 6 --locked -- -D warnings`; `cargo +1.92.0 test -p vibemux_frontend -p vibemux_cli --all-features -j 6 --locked`.
- [ ] WSL Ubuntu, own clone of the branch: the same three commands.
- [ ] Smoke: `vibemuxctl --format text ui --once` unchanged output; `vibemux_dashboard --once` unchanged.
- [ ] Remove temporary build folders; push; open the PR.
