# Desktop App Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the egui Supervisor Chat GUI with a Tauri 2 + Svelte 5 desktop app that offers today's features (projects and their daemons, chat as the read-only one-shot Ask, tasks, console, settings), then remove the egui GUI.

**Architecture:** A new GUI-neutral `vibemux_client` crate takes the daemon, probe, Ask, and Git-inventory client code out of `vibemux_frontend`. A new `vibemux_desktop` Tauri binary drives it, holds all daemon credentials, and exposes a fixed list of typed commands to a locked-down webview running a Svelte UI. Shared design tokens move to `config/design_tokens.css`.

**Tech Stack:** Rust 1.92 (pinned), Tauri 2.12.5, tauri-build 2.7.4, Svelte 5.57.2, Vite 8.3.3, TypeScript 5.9.3, Node 22.22.0, markdown-it 15.0.3, highlight.js 11.12.0, @lucide/svelte 1.61.0, Playwright 1.64.1 (dev only).

**Spec:** [docs/desktop_app_design.md](desktop_app_design.md) and [ADR 043](adr/043_tauri_desktop_app.md). Read both before any task.

## Global Constraints

- Authored names use snake_case with underscores only: files, directories, Rust and TypeScript functions and variables, npm script names, JSON keys, CSS custom properties. Type, trait, and Svelte component identifiers use the capitalization the language requires (`ChangeNotifier`, `import Sidebar from './sidebar.svelte'`). External names stay as published (`tauri.conf.json`, `@tauri-apps/api`, `markdown-it`).
- Every new Rust library crate starts with `#![forbid(unsafe_code)]`. No `unwrap`/`expect` on untrusted input, no unbounded channels, no async mutex held across `.await`.
- Exact versions are pinned with `=` in `Cargo.toml` (workspace dependencies) and exactly in `package.json`.
- The desktop app never opens SQLite, adds no listener, and never passes daemon credentials or filesystem paths to the webview.
- Events to the webview carry no content. `ask_reply` is the only command that returns vendor text.
- CSP string, verbatim: `default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self'; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; frame-ancestors 'none'`.
- No Tauri plugin is registered (no `tauri-plugin-*` crate), so none is exposed to the webview. `withGlobalTauri` is false; `freezePrototype` is true.
- Markdown: `html: false`, image rule disabled, links limited to `https:`, `http:`, `mailto:`.
- Settings file `desktop_settings.json`, schema 1, at most 16 project folders and 64 Ask-confirmed folders.
- Polling: tasks every 5 s with `detail_revision` gating; the Ask worker's existing 400 ms cadence.
- Commit messages: conventional prefix (`feat`, `fix`, `refactor`, `docs`, `test`, `ci`), no AI attribution.
- `PROGRESS.md` changes in every PR that changes implementation status; planned behavior is never described as implemented.
- Disk: before Task 5, the drive holding the cargo target directory needs at least 20 GB free (on 2026-10-24 C: had 2.2 GB, I: 15 GB). If it does not, stop and ask the operator.
- Task 3 starts only after `origin/fix/web_design_tokens` is merged into `main`.
- The egui GUI is frozen during phase 1: bug fixes only, no new features.

## Review Focus

1. **A project's daemon is not running, or stops mid-session.** Expect a `stopped` or `unreachable` row and a Start action within about 2 s, never a hang. Pinned by `status_of_a_folder_without_a_daemon_is_stopped` (Task 6) and `tasks_snapshot_without_a_daemon_reports_the_code` (Task 8).
2. **Hostile or odd vendor text:** raw HTML, `<script>`, `javascript:` and `data:` links, 10,000-character lines, CJK, control characters. Expect escaped text and wrapped lines. Pinned by the `markdown_render` tests (Task 9).
3. **Enter during IME composition.** Expect no send; Shift+Enter inserts a newline. Pinned by `enter_during_composition_does_nothing` (Task 7).
4. **Double actions:** Start clicked twice, Send clicked twice. Expect one job and a `daemon_job_busy` or `ask_busy` code for the second. Pinned by `a_second_start_while_one_runs_is_busy` (Task 6) and `a_second_ask_in_the_same_project_is_busy` (Task 9).
5. **A saved project folder was deleted or renamed.** Expect a row marked missing, commands returning `project_folder_missing`, and Forget still working. Pinned by `a_deleted_folder_is_listed_as_missing` (Task 6).

---

## File Structure

```text
crates/vibemux_client/                 # PR 1: GUI-neutral daemon/probe/Ask/Git client
  Cargo.toml
  src/lib.rs                           # module list, ChangeNotifier
  src/change_notifier.rs               # ChangeNotifier trait + NoopNotifier
  src/display_text.rs                  # safe_text (from dashboard/model.rs)
  src/{cli_runner,daemon_control,daemon_session,project_folder}.rs
  src/{supervisor_client,supervisor_mapping,supervisor_model,quick_search}.rs
  src/{probe_env,view_model,workspace_inventory}.rs
  src/{daemon_feed,daemon_model}.rs    # from dashboard/
  src/ask/{mod,reply_text,routes,session,worker}.rs
  tests/daemon_feed.rs                 # moved from vibemux_frontend/tests
config/design_tokens.css               # PR 2: shared tokens
crates/vibemux_frontend/tests/design_tokens_parity.rs
crates/vibemux_desktop/                # PR 3-4
  Cargo.toml, build.rs, tauri.conf.json
  capabilities/main_window.json
  icons/icon.ico, icons/icon.png
  src/main.rs                          # calls vibemux_desktop::run()
  src/lib.rs                           # Builder, managed state, handler list
  src/command_names.rs                 # COMMAND_NAMES, shared with build.rs
  src/command_error.rs                 # CommandError {code, message}
  src/navigation.rs                    # navigation_allowed
  src/desktop_settings.rs              # DesktopSettings, SettingsPatch, load/save
  src/event_sink.rs                    # EventSink, TauriEventSink, RecordingEventSink
  src/ui_contract.rs                   # DTOs and UiEvent sent to the webview
  src/app_state.rs                     # AppState: owns everything below
  src/project_registry.rs              # project_key <-> folder path
  src/project_status.rs                # DaemonState per project, start/stop jobs
  src/ask_registry.rs                  # Asks by ask_id, consent, reply access
  src/console_service.rs               # feed, Git inventory, probe workers
  src/windows.rs                       # main and task windows with the navigation guard
  src/commands.rs                      # thin #[tauri::command] wrappers
  tests/lockdown.rs                    # config, capability, CSP checks
  tests/services_daemon.rs             # services against a real test daemon
  ui/package.json, ui/package-lock.json, ui/tsconfig.json, ui/vite_config.ts, ui/index.html, ui/.gitignore
  ui/src/main.ts, ui/src/app.svelte, ui/src/style.css
  ui/src/lib/{ui_contract,backend,api_client,fixture_backend,markdown_render,key_bindings,format_text,toast_queue,app_state.svelte}.ts
  ui/src/components/*.svelte
  ui/tests/*_test.ts
  ui/scripts/capture_screens.ts
```

---

## PR 1 — Extract `vibemux_client`

Branch: `refactor/desktop_client_extraction`. No behavior change.

### Task 1: Create `vibemux_client` and move the data and I/O modules

**Files:**
- Create: `crates/vibemux_client/Cargo.toml`, `crates/vibemux_client/src/lib.rs`, `crates/vibemux_client/src/display_text.rs`
- Move (with their `#[cfg(test)]` modules, via `git mv`): `cli_runner.rs`, `daemon_control.rs`, `daemon_session.rs`, `project_folder.rs`, `supervisor_mapping.rs`, `supervisor_model.rs`, `quick_search.rs`, `probe_env.rs`, `view_model.rs`, `workspace_inventory.rs` from `crates/vibemux_frontend/src/`; `dashboard/daemon_feed.rs` and `dashboard/daemon_model.rs` to `crates/vibemux_client/src/daemon_feed.rs` and `daemon_model.rs`
- Modify: `Cargo.toml` (workspace `members`), `crates/vibemux_frontend/Cargo.toml`, `crates/vibemux_frontend/src/lib.rs`, `crates/vibemux_frontend/src/dashboard/mod.rs`, `crates/vibemux_frontend/src/dashboard/model.rs`, `crates/vibemux_frontend/src/dashboard/state.rs`

**Interfaces:**
- Produces: `vibemux_client::display_text::safe_text(value: &str) -> String` (cut at `MAX_DISPLAY_CHARACTERS = 160`, as today); `vibemux_client::workspace_inventory::{Workspace, Branch, load, parse, arguments}` (the two structs move here from `dashboard/state.rs`, unchanged fields); `vibemux_client::daemon_model::{DaemonView, Section, Unavailable, unavailable, …}`; `vibemux_client::daemon_feed::{DaemonFeed, FeedRequest}`; every other moved module keeps its public API and becomes `pub mod` in the client.
- Produces: feature `folder_dialog` on `vibemux_client` that enables `rfd = { version = "=0.17.2", default-features = false }` on Windows and `project_folder::pick_folder_native`.

- [ ] **Step 1: Record the baseline**

Run: `cargo test -p vibemux_frontend --all-features 2>&1 | tail -5`
Expected: the current pass count, recorded in the PR description.

- [ ] **Step 2: Create the crate and move the files**

`Cargo.toml` dependencies: `serde`, `serde_json`, `time` (`local-offset`), `tokio`, `uuid`, `vibemux_harness`, `vibemux_probe`, `vibemux_types`, `vibemuxd`, all `workspace = true` or by path as `vibemux_frontend` has them; dev-dependencies `tempfile` and `vibemuxd` with `test_helpers`. `safe_text` and `MAX_DISPLAY_CHARACTERS` move from `dashboard/model.rs` into `display_text.rs`. Items that were `pub(crate)` and are still called from `vibemux_frontend` become `pub` (for example `DaemonSession::connect` and `invalidate`). Fix intra-crate paths (`crate::dashboard::safe_text` becomes `crate::display_text::safe_text`; `crate::dashboard::{Branch, Workspace}` becomes `crate::workspace_inventory::{Branch, Workspace}`).

- [ ] **Step 3: Re-export from `vibemux_frontend` so its paths keep resolving**

In `vibemux_frontend/src/lib.rs` replace the moved `mod` lines with `pub use vibemux_client::{daemon_control, probe_env, project_folder, quick_search, supervisor_model, view_model};` and `use vibemux_client::{cli_runner, daemon_session, supervisor_mapping, workspace_inventory};`. `ask` and `supervisor_client` stay in `vibemux_frontend` until Task 2; their `crate::daemon_session` and similar paths resolve through these imports. In `dashboard/mod.rs`: `pub use vibemux_client::{daemon_feed, daemon_model};`. In `dashboard/model.rs`: `pub use vibemux_client::display_text::safe_text;`. In `dashboard/state.rs`: `pub use vibemux_client::workspace_inventory::{Branch, Workspace};`. Enable `vibemux_client/folder_dialog` from `vibemux_frontend` and drop `vibemux_frontend`'s own `rfd` dependency.

- [ ] **Step 4: Run both crates' tests**

Run: `cargo test -p vibemux_client --all-features && cargo test -p vibemux_frontend --all-features`
Expected: PASS; the two counts add up to the Step 1 baseline (moved tests now count under `vibemux_client`).

- [ ] **Step 5: Lint**

Run: `cargo clippy -p vibemux_client -p vibemux_frontend -p vibemux_cli --all-targets --all-features -- -D warnings`
Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock crates/vibemux_client crates/vibemux_frontend
git commit -m "refactor(client): move daemon, probe, and Git client modules into vibemux_client"
```

### Task 2: `ChangeNotifier`, move the supervisor client and Ask, update docs

**Files:**
- Create: `crates/vibemux_client/src/change_notifier.rs`
- Move: `crates/vibemux_frontend/src/supervisor_client.rs`, `crates/vibemux_frontend/src/ask/` to `crates/vibemux_client/src/`; `crates/vibemux_frontend/tests/daemon_feed.rs` to `crates/vibemux_client/tests/daemon_feed.rs`
- Create: `crates/vibemux_frontend/src/gui/egui_notifier.rs`
- Modify: `crates/vibemux_frontend/src/{lib.rs,live_app.rs,live_ask.rs,live_console.rs}`, `crates/vibemux_frontend/src/gui/mod.rs`, `AGENTS.md` §4.2, `CLAUDE.md` (layout map and dependency line), `docs/architecture.md`, `PROGRESS.md`

**Interfaces:**
- Produces:
  ```rust
  pub trait ChangeNotifier: Send + Sync + 'static { fn notify(&self); }
  pub struct NoopNotifier;               // impl ChangeNotifier, for tests and headless use
  pub struct CountingNotifier;           // #[cfg(any(test, feature = "test_support"))]; count() -> usize
  ```
- Changed signatures (the only API change in PR 1): `FrontendClient::start(project_root: PathBuf, notifier: Arc<dyn ChangeNotifier>) -> io::Result<FrontendClient>`; `ask::worker::spawn_ask(project_root: PathBuf, harness: AgentKind, prompt: String, notifier: Arc<dyn ChangeNotifier>) -> io::Result<AskHandle>`; `spawn_catalog(project_root, notifier) -> Receiver<Result<Vec<AgentKind>, String>>`; `spawn_setup(project_root, notifier) -> Receiver<SetupOutcome>`.
- Produces in `vibemux_frontend`: `pub(crate) struct EguiNotifier(pub egui::Context)` implementing `ChangeNotifier` with `request_repaint()` (a newtype because of the orphan rule).

- [ ] **Step 1: Write the failing test** in `change_notifier.rs`

```rust
#[test]
fn counting_notifier_counts_each_notify() {
    let notifier = CountingNotifier::default();
    notifier.notify();
    notifier.notify();
    assert_eq!(notifier.count(), 2);
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p vibemux_client counting_notifier_counts_each_notify`
Expected: FAIL, `CountingNotifier` not found.

- [ ] **Step 3: Implement `change_notifier.rs`**; `CountingNotifier` wraps an `AtomicUsize`.

- [ ] **Step 4: Move `supervisor_client.rs` and `ask/`, replacing every `egui::Context` with `Arc<dyn ChangeNotifier>` and `request_repaint()` with `notify()`.** Their tests that built `egui::Context::default()` use `Arc::new(NoopNotifier)`; the feed test moves unchanged except for its import path (`vibemux_client::daemon_feed`). The egui call sites (`live_app.rs`, `live_ask.rs`, `live_console.rs`) pass `Arc::new(EguiNotifier(context.clone()))`.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p vibemux_client --all-features && cargo test -p vibemux_frontend --all-features && cargo test -p vibemux_cli --all-features`
Expected: PASS, same totals as Task 1 Step 4.

- [ ] **Step 6: Check that the client has no GUI dependency**

Run: `cargo tree -p vibemux_client -e normal | grep -E "egui|eframe|ratatui|crossterm|winit"`
Expected: no output.

- [ ] **Step 7: Update docs.** AGENTS.md §4.2 and CLAUDE.md gain `vibemuxd → vibemux_client → vibemux_frontend` in the dependency line, and `crates/vibemux_client/` in the layout map ("GUI-neutral daemon, probe, Ask, and Git client shared by the frontends"). `docs/architecture.md` gets the same line. `PROGRESS.md` gets a dated entry: refactor only, no status change, with the test commands and counts from Step 5.

- [ ] **Step 8: Format, lint, commit**

Run: `cargo fmt --all -- --check && cargo clippy -p vibemux_client -p vibemux_frontend -p vibemux_cli --all-targets --all-features -- -D warnings`
Expected: clean.

```bash
git add -A crates/vibemux_client crates/vibemux_frontend AGENTS.md CLAUDE.md docs/architecture.md PROGRESS.md Cargo.lock
git commit -m "refactor(client): replace the egui repaint context with ChangeNotifier"
```

---

## PR 2 — Shared design tokens

Branch: `refactor/shared_design_tokens`. Starts after `fix/web_design_tokens` is on `main`.

### Task 3: `config/design_tokens.css` with a parity and contrast test

**Files:**
- Create: `config/design_tokens.css`, `crates/vibemux_frontend/tests/design_tokens_parity.rs`
- Modify: `crates/vibemux_web/web/src/style.css` (token blocks removed, `@import` added), `crates/vibemux_web/web/vite_config.ts` (`server.fs.allow` gains the repo `config` directory)

**Interfaces:**
- Produces: `config/design_tokens.css` with three blocks: `:root { … }` (light colors from `claude_light`, plus every non-color token: fonts, sizes, radii); `@media (prefers-color-scheme: dark) { :root:not([data-theme="light"]) { … } }`; `:root[data-theme="dark"] { … }`. The two dark blocks are identical and use the `claude` palette. Token names and values are exactly those in `style.css` on `fix/web_design_tokens`. Without a `data-theme` attribute the page follows the OS, so the mobile web surface behaves as before.

- [ ] **Step 1: Write the failing test** `design_tokens_parity.rs`

Parse `--name: #RRGGBB;` pairs per block with plain string splitting (no regex dependency) and reuse `vibemux_frontend::theme::contrast::contrast_ratio`.

```rust
const TOKENS: &str = include_str!("../../../config/design_tokens.css");
const PALETTES: &str = include_str!("../../../config/theme_palettes.json");

#[test]
fn base_tokens_match_the_exported_palettes() {
    // light block vs "claude_light", each dark block vs "claude":
    // bg, surface, surface_alt, border, text_primary, accent, success, warning, danger
    // equal case-insensitively; --muted_text equals text_muted.
}

#[test]
fn the_two_dark_blocks_are_identical() { /* same name -> value map */ }

#[test]
fn every_text_pair_meets_wcag_aa() {
    // For light and dark, each pair >= 4.5:
    // text_primary, muted_text, accent_text on bg, surface, surface_alt;
    // on_accent on accent; success_text/success_tint; warning_text/warning_tint;
    // danger_text/danger_tint; success_text, warning_text, danger_text on bg;
    // accent_text on accent_tint.
}
```

Current lowest values, for the failure message: light `accent_text/accent_tint` 4.54, dark `danger_text/danger_tint` 4.63.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p vibemux_frontend --test design_tokens_parity`
Expected: FAIL, `config/design_tokens.css` missing.

- [ ] **Step 3: Create `config/design_tokens.css`; replace the token blocks in `style.css` with `@import '../../../../config/design_tokens.css';`**

- [ ] **Step 4: Run the parity test and the web checks**

Run: `cargo test -p vibemux_frontend --test design_tokens_parity` then, in `crates/vibemux_web/web`: `npm run typecheck && npm test && npm run build`
Expected: PASS; `dist/assets/*.css` contains `--accent_text`.

- [ ] **Step 5: Commit**

```bash
git add config/design_tokens.css crates/vibemux_frontend/tests/design_tokens_parity.rs crates/vibemux_web/web/src/style.css crates/vibemux_web/web/vite_config.ts
git commit -m "refactor(web): share design tokens through config/design_tokens.css"
```

---

## PR 3 — `vibemux_desktop` skeleton and lockdown

Branch: `feat/desktop_app_skeleton`. Check free disk space first (Global Constraints).

### Task 4: UI scaffold and build guard

**Files:**
- Create: `crates/vibemux_desktop/ui/{package.json,package-lock.json,tsconfig.json,vite_config.ts,index.html,.gitignore}`, `ui/src/{main.ts,app.svelte,style.css}`, `ui/src/lib/{ui_contract.ts,backend.ts,api_client.ts,fixture_backend.ts}`, `ui/tests/fixture_guard_test.ts`

**Interfaces:**
- Produces (`backend.ts`):
  ```ts
  export interface Backend {
    invoke<T>(command: CommandName, args?: Record<string, unknown>): Promise<T>;
    listen(event: UiEventName, handler: (payload: unknown) => void): Promise<() => void>;
  }
  export async function load_backend(): Promise<Backend>; // fixture backend only when import.meta.env.MODE === 'fixture'
  ```
- Produces (`ui_contract.ts`): `CommandName` (string union of the 23 names in Task 5) and `UiEventName` (`'projects_changed' | 'snapshot_changed' | 'ask_changed' | 'console_changed'`), plus the DTO types added by later tasks.
- `package.json`: name `vibemux_desktop_ui`, private, `"type": "module"`, `engines.node` `22.22.0`; scripts `dev` (`vite --config vite_config.ts --port 5180 --strictPort`), `dev_fixture` (same, `--mode fixture`), `typecheck` (`svelte-check --tsconfig ./tsconfig.json`), `build` (`vite build --config vite_config.ts`), `test` (`node --experimental-strip-types --test` with an explicit file list), `audit` (`npm audit --audit-level=low`), `capture` (Task 12). Dependencies: `svelte` 5.57.2, `@tauri-apps/api` 2.12.4. Dev: `@sveltejs/vite-plugin-svelte` 7.3.1, `@tauri-apps/cli` 2.12.4, `@types/node` 26.6.4, `svelte-check` 4.7.6, `typescript` 5.9.3, `vite` 8.3.3.
- `fixture_backend.ts` exports `FIXTURE_MARKER = 'VIBEMUX_FIXTURE_DATA'` and labels all data `mode: 'demo'`.

- [ ] **Step 1: Write the failing test** `fixture_guard_test.ts`

```ts
test('production build contains no fixture data', () => {
  const files = list_files_recursive('dist');
  assert.ok(files.some((file) => file.endsWith('.js')));
  for (const file of files) assert.ok(!readFileSync(file, 'utf8').includes('VIBEMUX_FIXTURE_DATA'), file);
});
```

- [ ] **Step 2: Run it to verify it fails**

Run (in `ui/`): `npm ci && node --experimental-strip-types --test tests/fixture_guard_test.ts`
Expected: FAIL, `dist` missing.

- [ ] **Step 3: Write the scaffold.** `app.svelte` renders the empty shell (sidebar column and main column) with tokens from `config/design_tokens.css`; `vite_config.ts` sets `server.fs.allow` to include `../../../config`, `build.outDir` `dist`, and `build.target` `chrome120`.

- [ ] **Step 4: Build and rerun**

Run: `npm run typecheck && npm run build && npm test`
Expected: PASS; also run `npm run build -- --mode fixture --outDir dist_fixture`, confirm `dist_fixture` contains the marker, then delete `dist_fixture`.

- [ ] **Step 5: Commit**

```bash
git add crates/vibemux_desktop/ui
git commit -m "feat(desktop): add the Svelte UI scaffold and the fixture build guard"
```

### Task 5: Tauri crate, settings, navigation guard, lockdown test, CI, ADR acceptance

**Files:**
- Create: `crates/vibemux_desktop/{Cargo.toml,build.rs,tauri.conf.json}`, `capabilities/main_window.json`, `icons/icon.ico`, `icons/icon.png`, `src/{main.rs,lib.rs,command_names.rs,command_error.rs,navigation.rs,desktop_settings.rs,event_sink.rs,ui_contract.rs,app_state.rs,windows.rs,commands.rs}`, `tests/lockdown.rs`
- Modify: `Cargo.toml` (member, workspace deps `tauri = "=2.12.5"`, `tauri-build = "=2.7.4"`), `.gitignore` (`crates/vibemux_desktop/gen/`), `deny.toml`, `.github/workflows/ci.yml`, `docs/adr/043_tauri_desktop_app.md`, `PROGRESS.md`, `CLAUDE.md`, `README.md`

**Interfaces:**
- Produces (`command_names.rs`, included by `build.rs` with `include!`):
  ```rust
  pub const COMMAND_NAMES: [&str; 23] = [
      "projects_list", "project_pick", "project_select", "project_forget",
      "daemon_start", "daemon_stop", "tasks_snapshot", "supervisor_action", "task_window_open",
      "ask_targets", "ask_setup", "ask_confirm", "ask_submit", "ask_cancel", "ask_status", "ask_reply",
      "console_section", "probe_rerun", "quick_search", "open_link",
      "settings_get", "settings_set", "about_info",
  ];
  ```
- Produces: `#[derive(Serialize)] pub struct CommandError { pub code: String, pub message: String }` with `CommandError::new(code: &str, message: impl Into<String>)`; every command returns `Result<T, CommandError>`.
- Produces: `pub fn navigation_allowed(url: &url::Url, dev_url: Option<&url::Url>) -> bool`. Allowed: `tauri://localhost`, `http://tauri.localhost`, `https://tauri.localhost`, and the dev URL's origin when given.
- Produces (`desktop_settings.rs`):
  ```rust
  pub struct DesktopSettings { pub schema_version: u16, pub appearance: Appearance, pub text_size: TextSize,
      pub density: Density, pub reduce_motion: bool, pub sidebar_collapsed: bool, pub start_page: StartPage,
      pub project_folders: Vec<PathBuf>, pub ask_confirmed_projects: Vec<PathBuf> }
  pub enum Appearance { Light, Dark, System }          // serde snake_case; default System
  pub const MAX_PROJECT_FOLDERS: usize = 16;
  pub const MAX_ASK_CONFIRMED_PROJECTS: usize = 64;
  pub fn load_settings_at(path: &Path) -> DesktopSettings;          // missing or invalid file -> default
  pub fn save_settings_at(path: &Path, settings: &DesktopSettings) -> io::Result<()>; // temp file + rename
  pub struct SettingsPatch { /* every field Option<…> except the two folder lists */ }
  ```
  The desktop crate defines its own enums (it must not depend on `vibemux_frontend`, which pulls in egui until PR 5), all serde snake_case: `TextSize { Small, Default, Large }` (default `Default`), `Density { Comfortable, Compact }` (default `Comfortable`), `StartPage { Chat, Tasks, Console }` (default `Chat`).
- `Cargo.toml` dependencies: `tauri`, `serde`, `serde_json`, `sha2`, `url`, `uuid`, `vibemux_client` (feature `folder_dialog`), `vibemux_types`, `vibemuxd`; build dependency `tauri-build`; dev dependencies `tempfile` and `vibemuxd` with `test_helpers`. No `tauri-plugin-*` crate.
- Produces: `pub trait EventSink: Send + Sync { fn emit(&self, event: UiEvent); }`, `TauriEventSink(AppHandle)`, and `RecordingEventSink` (test support; `events() -> Vec<UiEvent>`).
- Produces (`ui_contract.rs`): `#[serde(tag = "event", rename_all = "snake_case")] pub enum UiEvent { ProjectsChanged, SnapshotChanged { project_key: String, revision: u64 }, AskChanged { ask_id: String, phase: String }, ConsoleChanged { project_key: String } }`, plus `AboutInfo { app_version: String, webview_version: String }`.
- Produces: `pub struct AppState` constructed by `AppState::new(settings_path: PathBuf, sink: Arc<dyn EventSink>)`; later tasks add fields.
- `tauri.conf.json`: `productName` `VibeMux`, `identifier` `io.github.yuedongsun.vibemux`, `build.frontendDist` `ui/dist`, `build.devUrl` `http://localhost:5180`, `app.windows` `[]` (windows are created in Rust so the navigation guard applies), `app.withGlobalTauri` false, `app.security.csp` the Global Constraints string, `app.security.freezePrototype` true, `bundle.active` false, `bundle.icon` `["icons/icon.ico", "icons/icon.png"]`.
- `capabilities/main_window.json`: identifier `main_window`, windows `["main", "task_*"]`, permissions `core:event:allow-listen`, `core:event:allow-unlisten`, and one `allow-…` entry per command name.
- Every `#[tauri::command]` uses `rename_all = "snake_case"`, so the UI passes `{ project_key }`. Commands are `async` and run blocking service calls through `tauri::async_runtime::spawn_blocking`.

- [ ] **Step 1: Write the failing tests**

`navigation.rs`:
```rust
#[test]
fn only_the_app_origin_and_dev_server_are_allowed() {
    assert!(navigation_allowed(&url("http://tauri.localhost/index.html"), None));
    assert!(navigation_allowed(&url("tauri://localhost/"), None));
    assert!(!navigation_allowed(&url("https://example.com/"), None));
    assert!(!navigation_allowed(&url("file:///C:/Windows/"), None));
    assert!(!navigation_allowed(&url("http://localhost:5180/"), None));
    assert!(navigation_allowed(&url("http://localhost:5180/x"), Some(&url("http://localhost:5180"))));
}
```
`desktop_settings.rs`: `missing_file_gives_defaults` (appearance `System`, empty lists); `invalid_json_gives_defaults`; `save_then_load_round_trips`; `unknown_schema_version_gives_defaults`.
`tests/lockdown.rs`:
```rust
#[test] fn csp_is_exactly_the_reviewed_string() { /* tauri.conf.json app.security.csp == constant */ }
#[test] fn no_remote_urls_in_config() { /* every string value starting with http(s):// is the devUrl */ }
#[test] fn global_tauri_is_off_and_prototype_frozen() { /* withGlobalTauri false, freezePrototype true */ }
#[test] fn capability_grants_exactly_the_command_list() {
    // permissions == {"core:event:allow-listen", "core:event:allow-unlisten"}
    //   ∪ { format!("allow-{}", name.replace('_', "-")) for name in COMMAND_NAMES }
}
#[test] fn no_plugins_are_configured() { /* tauri.conf.json has no "plugins" entries; Cargo.toml has no tauri-plugin-* dependency */ }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p vibemux_desktop`
Expected: FAIL to compile (crate missing pieces).

- [ ] **Step 3: Implement the crate.** `build.rs` first asserts `ui/dist/index.html` exists and is non-empty, with the panic message `vibemux_desktop_frontend_missing: run npm ci && npm run build in crates/vibemux_desktop/ui before cargo`, then calls `tauri_build::try_build(Attributes::new().app_manifest(AppManifest::new().commands(&COMMAND_NAMES)))`. `lib.rs::run()` loads settings from the Tauri app config directory, creates `AppState`, opens the main window through `windows::open_main_window` (1280×820, minimum 900×600, title `VibeMux`, `on_navigation` calling `navigation_allowed`), and registers all 23 handlers; handlers added by later tasks start as stubs returning `CommandError::new("not_implemented", …)`, so the list is complete from this task on. `settings_get`, `settings_set`, and `about_info` (`tauri::webview_version()`) are real here. Icons: `npx tauri icon ../../vibemux_web/web/public/vibemux_icon.svg -o ../icons` from `ui/`, keeping only `icon.ico` and `icon.png`.

- [ ] **Step 4: Verify the generated permission names**

Run: `cargo build -p vibemux_desktop`, then search `crates/vibemux_desktop/gen/schemas/` for `allow-projects-list`.
Expected: found. If Tauri generates a different form, change the lockdown test's mapping and the capability file to match, and note it in the commit message.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p vibemux_desktop`
Expected: PASS.

- [ ] **Step 6: Supply chain and size**

Run: `cargo deny check` and `cargo audit`. Add per-crate `[[licenses.exceptions]]` entries for each MPL-2.0 crate reported (expected: `cssparser`, `selectors`, and similar via `kuchikiki`), each with a reason line. Then `cargo build --release -p vibemux_desktop` and record the clean build time and the size of `vibemux_desktop.exe`.
Expected: `cargo deny check` passes; the numbers go in `PROGRESS.md`.

- [ ] **Step 7: CI.** In `.github/workflows/ci.yml` (job `rust`), add before the Rust steps: on Linux only (`if: runner.os == 'Linux'`), `sudo apt-get update && sudo apt-get install -y libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev librsvg2-dev`; then in `crates/vibemux_desktop/ui`: `npm ci`, `npm run typecheck`, `npm run build`, `npm test`, `npm run audit`.

- [ ] **Step 8: Run the app once**

Run: `cargo run -p vibemux_desktop`
Expected: a window titled VibeMux shows the empty shell in the OS appearance; navigating the devtools console to `location.href = 'https://example.com'` leaves the page unchanged.

- [ ] **Step 9: Docs.** ADR 043 status becomes `Accepted (2026-MM-DD)` with the confirmed CSP and permission-name form. `PROGRESS.md`: new row "Desktop app (ADR 043)" `PARTIAL — SKELETON`, plus a dated entry with commands, results, build time, and binary size. `CLAUDE.md` and `README.md`: layout line for `crates/vibemux_desktop/` and the build commands (`npm ci && npm run build` in `crates/vibemux_desktop/ui`, then `cargo run -p vibemux_desktop`), with the note that workspace-wide cargo commands need the UI built or `--exclude vibemux_desktop`.

- [ ] **Step 10: Commit**

```bash
git add Cargo.toml Cargo.lock .gitignore deny.toml .github/workflows/ci.yml crates/vibemux_desktop docs/adr/043_tauri_desktop_app.md PROGRESS.md CLAUDE.md README.md
git commit -m "feat(desktop): add the Tauri shell with settings, navigation guard, and lockdown tests"
```

---

## PR 4 — Screens

Branch: `feat/desktop_app_screens`. Each task adds its Rust service, its commands (replacing the stubs), its DTOs in `ui_contract.rs` and `ui_contract.ts`, and its components. Services are plain functions over `&AppState`; command wrappers only convert arguments and errors.

### Task 6: Projects, daemon state, and the sidebar

**Files:**
- Create: `src/project_registry.rs`, `src/project_status.rs`, `tests/services_daemon.rs`, `ui/src/components/{sidebar,project_row,top_bar,spark_mark,toast_stack}.svelte`, `ui/src/lib/{toast_queue,format_text}.ts`, `ui/tests/{toast_queue,format_text}_test.ts`
- Modify: `src/app_state.rs`, `src/commands.rs`, `src/ui_contract.rs`, `ui/src/lib/ui_contract.ts`, `ui/src/app.svelte`, `crates/vibemux_client/src/daemon_control.rs` (adds `DaemonCommand::Inspect` and `parse_inspection`)

**Interfaces:**
- Consumes: `vibemux_client::{daemon_control, daemon_session, project_folder}` (Task 1).
- Produces:
  ```rust
  pub fn project_key_for(path: &Path) -> String;   // first 16 hex chars of SHA-256 of the canonical path's UTF-8 (lossy) bytes
  #[serde(rename_all = "snake_case")] pub enum DaemonState { Stopped, Starting, Running, Stopping, Unreachable, Recoverable }
  pub struct ProjectRow { pub project_key: String, pub name: String, pub daemon: DaemonState,
      pub code: Option<String>, pub missing: bool, pub active: bool, pub ask_confirmed: bool }
  pub fn projects_list(state: &AppState) -> Vec<ProjectRow>;
  pub fn project_pick(state: &AppState) -> Result<Option<ProjectRow>, CommandError>;      // native dialog on Windows; elsewhere `folder_dialog_unsupported`
  pub fn project_select(state: &AppState, project_key: &str) -> Result<(), CommandError>; // starts FrontendClient for it, stops the previous one
  pub fn project_forget(state: &AppState, project_key: &str) -> Result<(), CommandError>;
  pub fn daemon_start(state: &AppState, project_key: &str) -> Result<(), CommandError>;   // one job per project: else `daemon_job_busy`
  pub fn daemon_stop(state: &AppState, project_key: &str) -> Result<(), CommandError>;
  pub trait DaemonJobRunner: Send + Sync {
      fn run(&self, command: DaemonCommand, project_root: &Path) -> DaemonControlResult;
  }
  pub struct CliJobRunner;   // production: vibemux_client::daemon_control::run
  // daemon_control.rs:
  pub fn parse_inspection(exit_success: bool, stdout: &[u8]) -> Option<String>; // the "status" field only
  ```
- Status worker: one thread checks every non-active project every 15 s with `DaemonSession::connect` (`Running`, or `Stopped` for `daemon_not_running`, else `Unreachable`); for `Unreachable` it runs `vibemuxctl --json daemon inspect --project-root <path>` at most once per 60 s per project and shows `Recoverable` when `status` is `recoverable`. It reads only `status` and `reason_code`, never the confirmation. It emits `ProjectsChanged` on any change.
- UI: `toast_queue.ts` exports `push_toast(queue, toast)`, at most 4 visible, newest first, each with `code` and `message`; `format_text.ts` exports `relative_time(iso: string, now: Date) -> string` (`now`, `2m`, `1h`, `3d`) and `short_id(id: string) -> string` (8 chars).

- [ ] **Step 1: Write the failing tests**

`project_registry.rs`: `project_key_is_stable_and_16_hex` (same path twice gives the same key; `len() == 16`; all hex); `a_deleted_folder_is_listed_as_missing` (save a temp folder, delete it, `projects_list` row has `missing: true`; `project_select` returns code `project_folder_missing`; `project_forget` succeeds); `the_list_keeps_at_most_16_folders` (adding a 17th drops the oldest).
`project_status.rs`: `status_of_a_folder_without_a_daemon_is_stopped` (temp folder, no daemon; state `Stopped` and code `daemon_not_running` within 2 s); `a_second_start_while_one_runs_is_busy` (inject a job runner that blocks; second `daemon_start` returns `daemon_job_busy`).
`daemon_control.rs`: `inspection_status_is_read_without_the_confirmation` (stdout `{"ok":true,"status":"recoverable","reason_code":"x","confirmation":"abc"}` gives `Some("recoverable")`; invalid JSON gives `None`).
`tests/services_daemon.rs`: `a_running_test_daemon_shows_as_running` (start a `DaemonControlServer` the way `vibemux_client/tests/daemon_feed.rs` does; register its folder; select it; `projects_list` shows `Running` and a `SnapshotChanged` event reaches the `RecordingEventSink` within 10 s).
`ui/tests/toast_queue_test.ts` and `format_text_test.ts` for the functions above.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p vibemux_desktop -p vibemux_client` and, in `ui/`, `npm test`
Expected: FAIL (missing items).

- [ ] **Step 3: Implement the services and commands.** `AppState` holds an `Arc<dyn DaemonJobRunner>` (`CliJobRunner` in production) so the busy test does not spawn `vibemuxctl`.

- [ ] **Step 4: Implement the components.** Sidebar per the spec §5.6 (New chat, project rows with the state dot and a hover menu with Start/Stop/Forget, footer Tasks/Console/Settings, collapse to a 56 px rail); top bar with breadcrumbs and the connection dot; toasts show `code` in monospace.

- [ ] **Step 5: Run the tests and the type check**

Run: `cargo test -p vibemux_desktop -p vibemux_client` and `npm run typecheck && npm test`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/vibemux_desktop crates/vibemux_client/src/daemon_control.rs
git commit -m "feat(desktop): list projects with daemon state and start or stop their daemons"
```

### Task 7: Composer, welcome screen, and key bindings

**Files:**
- Create: `ui/src/lib/key_bindings.ts`, `ui/tests/key_bindings_test.ts`, `ui/src/components/{welcome_view,composer,target_chip,shortcut_overlay}.svelte`
- Modify: `ui/src/app.svelte`

**Interfaces:**
- Produces:
  ```ts
  export type ShortcutAction = 'quick_switcher' | 'new_chat' | 'toggle_sidebar' | 'settings' | 'shortcut_overlay' | 'close_overlay';
  export const SHORTCUTS: ReadonlyArray<{ keys: string; action: ShortcutAction; label: string }>;
  // Ctrl+K, Ctrl+N, Ctrl+B, Ctrl+, Ctrl+/ ; Esc
  export function shortcut_for(event: KeyLike): ShortcutAction | null;   // null while event.isComposing
  export function composer_key(event: KeyLike): 'send' | 'newline' | 'none'; // Enter send, Shift+Enter newline, composing none
  export function greeting(now: Date): string; // 'Good morning' 05:00-11:59, 'Good afternoon' 12:00-17:59, 'Good evening' otherwise
  ```
  `KeyLike` is `{ key: string; ctrlKey: boolean; shiftKey: boolean; isComposing: boolean }`.

- [ ] **Step 1: Write the failing tests** `key_bindings_test.ts`: `enter_during_composition_does_nothing` (`composer_key({key:'Enter', isComposing:true, …}) === 'none'`, and `shortcut_for` with `isComposing` returns `null` for Ctrl+K); `enter_sends_and_shift_enter_adds_a_line`; `each_chord_maps_to_one_action` (no duplicate `keys` in `SHORTCUTS`; Ctrl+K gives `quick_switcher`); `greeting_follows_local_time` (05:00 morning, 11:59 morning, 12:00 afternoon, 18:00 evening, 04:59 evening).

- [ ] **Step 2: Run them to verify they fail**

Run: `npm test`
Expected: FAIL.

- [ ] **Step 3: Implement `key_bindings.ts` and the components.** Welcome: spark mark and serif greeting above a centered composer. Composer: 20 px radius, soft shadow, placeholder `How can I help today?`, target chip on the left, circular send button on the right. `Ctrl+/` opens the overlay rendered from `SHORTCUTS`. New chat with a non-empty draft asks `Discard current draft?` with Discard and Keep editing.

- [ ] **Step 4: Run tests and type check**

Run: `npm run typecheck && npm test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/vibemux_desktop/ui
git commit -m "feat(desktop): add the welcome screen, composer, and shortcut table"
```

### Task 8: Tasks page, task panel, task windows, terminal actions

**Files:**
- Create: `ui/src/components/{tasks_page,task_panel,run_details,task_window_root}.svelte`
- Modify: `src/app_state.rs`, `src/commands.rs`, `src/windows.rs`, `tests/services_daemon.rs`, `ui/src/lib/ui_contract.ts`, `ui/src/app.svelte`

**Interfaces:**
- Consumes: `FrontendClient::{start, take_snapshot, submit}`, `SupervisorSnapshot`, `SupervisorAction` (Tasks 1-2).
- Produces:
  ```rust
  pub fn tasks_snapshot(state: &AppState, project_key: &str) -> Result<SupervisorSnapshot, CommandError>;
  pub fn supervisor_action(state: &AppState, project_key: &str, action: SupervisorAction) -> Result<(), CommandError>;
  pub fn task_window_open(app: &AppHandle, state: &AppState, project_key: &str, task_id: &str) -> Result<(), CommandError>;
  ```
  `supervisor_action` validates every ID field as a UUID before submitting (`invalid_identifier` otherwise). `task_window_open` creates window label `task_<first 8 of task_id>` with the same navigation guard and the URL `index.html#/task/<project_key>/<task_id>`. The snapshot is serialized as-is; `SupervisorSnapshot` already bounds its content.
- `ChangeNotifier` for the active project emits `SnapshotChanged { project_key, revision }`, where `revision` is the highest `detail_revision` in the snapshot.

- [ ] **Step 1: Write the failing tests** in `tests/services_daemon.rs`: `tasks_snapshot_without_a_daemon_reports_the_code` (selected folder without a daemon; `connection.status` `unavailable` and `connection.code` `daemon_not_running` within 2 s); `tasks_snapshot_lists_a_recorded_task` (record a Task the way `daemon_feed.rs` does; the snapshot contains its `task_id` within 10 s); `supervisor_action_rejects_a_non_uuid_task_id` (`LoadTask { task_id: "x" }` returns `invalid_identifier`).

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p vibemux_desktop --test services_daemon`
Expected: FAIL.

- [ ] **Step 3: Implement the services, commands, and components.** Tasks page: filters All, Running, Blocked, Done applied in TypeScript over `snapshot.tasks`; "Load more" sends `LoadMoreTasks { cursor }` when `next_cursor` is set; a DEMO tag whenever `mode === 'demo'`; a freshness label from `observed_at` showing only the local clock time; when `connection.code` is `control_unsupported_version`, the page says `This daemon is too old for the task list (needs Control v4).` Task panel: resizable right panel (default 420 px, minimum 360 px, maximum half the window), tabs Overview, Activity, Artifacts, Terminal; run details in a two-column grid of muted labels and monospace values; copy buttons on IDs, branches, and paths showing `Copied` for 1.5 s. Terminal tab: Inspect, Link, Focus, Unlink through `supervisor_action`.

- [ ] **Step 4: Run tests and type check**

Run: `cargo test -p vibemux_desktop` and `npm run typecheck && npm test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/vibemux_desktop
git commit -m "feat(desktop): add the tasks page, task panel, task windows, and terminal actions"
```

### Task 9: Chat as Ask, with the markdown renderer

**Files:**
- Create: `src/ask_registry.rs`, `ui/src/lib/markdown_render.ts`, `ui/tests/markdown_render_test.ts`, `ui/src/components/{chat_view,reply_body,setup_card,consent_dialog}.svelte`, `ui/src/lib/ask_copy.ts`
- Modify: `src/app_state.rs`, `src/commands.rs`, `src/ui_contract.rs`, `ui/src/lib/ui_contract.ts`, `ui/package.json` (adds `markdown-it` 15.0.3, `highlight.js` 11.12.0, `@lucide/svelte` 1.61.0), root `Cargo.toml` and `crates/vibemux_desktop/Cargo.toml` (adds `open = "=5.4.5"`, MIT), `crates/vibemux_client/src/ask/worker.rs` (adds `AskHandle::new`)

**Interfaces:**
- Consumes: `vibemux_client::ask::{worker::{spawn_ask, spawn_catalog, spawn_setup}, AskHandle, AskUpdate, AskPhase, AskEnd, SetupOutcome, UNCONFIGURED}` (Task 2).
- Produces:
  ```rust
  pub struct AskTarget { pub harness: String, pub label: String, pub available: bool, pub reason: Option<String> }
  pub struct AskTargets { pub targets: Vec<AskTarget>, pub setup_offered: bool, pub code: Option<String> }
  pub struct AskStatus { pub ask_id: String, pub project_key: String, pub harness: String,
      pub phase: String, pub end: Option<String>, pub end_code: Option<String>, pub truncated: bool }
  pub struct AskReply { pub reply: String, pub truncated: bool }
  pub fn ask_targets(state: &AppState, project_key: &str) -> Result<AskTargets, CommandError>;
  pub fn ask_setup(state: &AppState, project_key: &str) -> Result<(), CommandError>;
  pub fn ask_confirm(state: &AppState, project_key: &str) -> Result<(), CommandError>;      // records consent in settings
  pub fn ask_submit(state: &AppState, project_key: &str, harness: &str, prompt: &str) -> Result<String, CommandError>; // ask_id
  pub fn ask_cancel(state: &AppState, ask_id: &str) -> Result<(), CommandError>;
  pub fn ask_status(state: &AppState, ask_id: &str) -> Result<AskStatus, CommandError>;
  pub fn ask_reply(state: &AppState, ask_id: &str) -> Result<AskReply, CommandError>;
  pub fn open_link(url: &str) -> Result<(), CommandError>;   // https only, else `link_scheme_refused`; open::that_detached
  pub trait AskSpawner: Send + Sync {
      fn spawn(&self, project_root: PathBuf, harness: AgentKind, prompt: String,
               notifier: Arc<dyn ChangeNotifier>) -> io::Result<AskHandle>;
  }
  pub struct WorkerAskSpawner;   // production: vibemux_client::ask::worker::spawn_ask
  // vibemux_client, for alternative spawners:
  impl AskHandle { pub fn new(updates: Receiver<AskUpdate>, cancel: Arc<AtomicBool>) -> Self; }
  ```
  Rules: `ask_submit` returns `ask_consent_required` until `ask_confirm` ran for the folder; `ask_busy` while another Ask runs in the same project; `harness_not_available` for a harness not in `targets` as available; `prompt_too_large` above the dispatch prompt limit; `invalid_harness` for an unknown name. When `ask_targets` reports `control_unsupported_version`, the chat screen says `This daemon is too old for Ask (needs Control v5).` Asks are kept in memory for the app session, at most 32 per project (oldest finished dropped). `AskChanged` carries `ask_id` and `phase` only.
- Produces (`markdown_render.ts`): `export function render_markdown(source: string): string;` markdown-it with `html: false`, `linkify: false`, `typographer: false`, `disable('image')`, `validateLink` allowing only `https:`, `http:`, `mailto:`; fenced code highlighted with highlight.js core and the languages `bash`, `powershell`, `rust`, `typescript`, `javascript`, `python`, `json`, `toml`, `yaml`, `diff`, `markdown`; unknown languages are escaped plain text.
- `ask_copy.ts` holds the exact Ask copy from `crates/vibemux_frontend/src/gui/ask_view.rs` (lines 135-425: the unavailable, setup, consent, end-state, and per-harness reach sentences), so it survives the removal in Task 13.

- [ ] **Step 1: Write the failing tests**

`markdown_render_test.ts`:
```ts
test('raw html is escaped', () => {
  const html = render_markdown('<script>alert(1)</script><img src=x onerror=alert(1)>');
  assert.ok(!html.includes('<script') && !html.includes('<img'));
  assert.ok(html.includes('&lt;script&gt;'));
});
test('only https, http, and mailto links survive', () => {
  for (const bad of ['javascript:alert(1)', 'data:image/png;base64,AA==', 'file:///C:/x', 'vbscript:x'])
    assert.ok(!render_markdown(`[x](${bad})`).includes('href='), bad);
  assert.ok(render_markdown('[x](https://example.com)').includes('href="https://example.com"'));
});
test('images are not rendered', () => { assert.ok(!render_markdown('![a](https://example.com/a.png)').includes('<img')); });
test('code fences are highlighted or escaped', () => {
  assert.ok(render_markdown('```rust\nfn main() {}\n```').includes('hljs'));
  assert.ok(render_markdown('```nosuchlang\n<b>x</b>\n```').includes('&lt;b&gt;'));
});
test('long lines and CJK pass through', () => {
  const long = 'a'.repeat(10_000);
  assert.ok(render_markdown(long).includes(long));
  assert.ok(render_markdown('主控工作台').includes('主控工作台'));
});
```
`ask_registry.rs`: `submit_without_consent_is_refused` (`ask_consent_required`); `a_second_ask_in_the_same_project_is_busy`; `an_unknown_harness_is_refused` (`invalid_harness`); `status_and_events_carry_no_reply_text` (with an injected Ask spawner that yields a reply `SECRET_REPLY`, serialize `ask_status` and every recorded `UiEvent` to JSON and assert neither contains `SECRET_REPLY`, while `ask_reply` does).

- [ ] **Step 2: Run them to verify they fail**

Run: `npm test` and `cargo test -p vibemux_desktop ask_registry`
Expected: FAIL.

- [ ] **Step 3: Implement.** `AppState` holds an `Arc<dyn AskSpawner>` (`WorkerAskSpawner` in production), called with a `ChangeNotifier` that emits `AskChanged`. Chat view: the user message, then the harness line (`Claude Code · read-only · one turn`), a rotating spark while running (static when Reduce motion or `prefers-reduced-motion`), Cancel, the end state, and `reply_body` rendering `render_markdown(reply)` with copy buttons on code blocks; the `Replies live in the daemon's memory and are lost when it restarts.` note; the cut notice when `truncated`. Sending again starts a new Ask row, and the composer caption says `Each message starts a new one-turn Ask`. Clicking a rendered link calls `open_link`, which opens the default browser from Rust; no Tauri plugin is registered. Add a test `open_link_refuses_other_schemes` (`http://`, `file:///`, `javascript:` all return `link_scheme_refused`; the opener is not called for them).

- [ ] **Step 4: Run tests and type check**

Run: `cargo test -p vibemux_desktop` and `npm run typecheck && npm test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/vibemux_desktop
git commit -m "feat(desktop): send read-only Asks from the chat screen and render replies as safe markdown"
```

### Task 10: Console page

**Files:**
- Create: `src/console_service.rs`, `ui/src/components/{console_page,console_section_view}.svelte`
- Modify: `src/app_state.rs`, `src/commands.rs`, `src/ui_contract.rs`, `ui/src/lib/ui_contract.ts`

**Interfaces:**
- Consumes: `DaemonFeed`, `DaemonView`, `Section`, `workspace_inventory::load`, `view_model::ViewModel`, `probe_env` (Task 1).
- Produces:
  ```rust
  #[serde(rename_all = "snake_case")] pub enum ConsoleSectionName { Overview, Harnesses, Workspace, Tasks, Daemon, Sessions, Decisions }
  #[serde(tag = "state", rename_all = "snake_case")]
  pub enum SectionDto<T> { Waiting, Loaded { data: T }, Unavailable { code: String, hint: String } }
  pub struct ConsoleSection { pub section: ConsoleSectionName, pub observed_at: Option<String>, pub body: serde_json::Value }
  pub fn console_section(state: &AppState, project_key: &str, section: ConsoleSectionName) -> Result<ConsoleSection, CommandError>;
  pub fn probe_rerun(state: &AppState) -> Result<(), CommandError>;   // one probe at a time: `probe_busy`
  ```
  `body` holds the section's DTO built from `DaemonView`, `Workspace`, or `ViewModel` with the same fields the egui console shows (`gui/console/*.rs`). The feed runs only while the Console page is open: the UI calls `console_section` on open and the service stops the feed 30 s after the last call.

- [ ] **Step 1: Write the failing tests** in `console_service.rs`: `section_dto_maps_each_state` (`Section::Waiting` gives `{"state":"waiting"}`; `Unavailable` gives `code` and `hint`); `a_second_probe_while_one_runs_is_busy`; `the_feed_stops_after_the_page_closes` (inject a clock; no feed 30 s after the last call).

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p vibemux_desktop console_service`
Expected: FAIL.

- [ ] **Step 3: Implement the service and the page** (one tab per section, the same sections and labels as `gui/console`).

- [ ] **Step 4: Run tests and type check**

Run: `cargo test -p vibemux_desktop` and `npm run typecheck && npm test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/vibemux_desktop
git commit -m "feat(desktop): add the console page over content-free daemon, probe, and Git reads"
```

### Task 11: Settings, appearance, and the quick switcher

**Files:**
- Create: `ui/src/components/{settings_page,quick_switcher}.svelte`, `ui/src/lib/app_state.svelte.ts`
- Modify: `src/commands.rs` (`quick_search`), `src/ui_contract.rs`, `ui/src/lib/ui_contract.ts`, `ui/src/app.svelte`

**Interfaces:**
- Consumes: `vibemux_client::quick_search` (Task 1), `DesktopSettings` and `SettingsPatch` (Task 5).
- Produces:
  ```rust
  pub struct SearchEntry { pub kind: String, pub label: String, pub detail: String, pub target: String }
  pub fn quick_search(state: &AppState, query: &str) -> Result<Vec<SearchEntry>, CommandError>; // at most 8; query at most 256 bytes
  ```
  Entries: tasks of the active snapshot, projects, Asks of this session, and the actions New chat, Settings, Console, Keyboard shortcuts, Toggle sidebar, Light, Dark, Match system. Ranking is `quick_search`'s (prefix above word start above substring; ties keep source order).
- `app_state.svelte.ts` applies appearance by setting `document.documentElement.dataset.theme` to `light` or `dark`, or removing it for `system`.

- [ ] **Step 1: Write the failing test** in `commands.rs` tests: `quick_search_caps_results_and_query_length` (more than 8 matches gives 8; a 300-byte query gives `query_too_long`; an empty query lists tasks first, then actions).

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p vibemux_desktop quick_search`
Expected: FAIL.

- [ ] **Step 3: Implement.** Settings sections General (start page), Appearance (Light, Dark, Match system; text size; density; Reduce motion), Connection (each project's daemon state with Start/Stop), Shortcuts (from `SHORTCUTS`), About (`about_info`). Switcher: centered popup, focused input, Up/Down, Enter opens, Esc closes.

- [ ] **Step 4: Run tests and type check**

Run: `cargo test -p vibemux_desktop` and `npm run typecheck && npm test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/vibemux_desktop
git commit -m "feat(desktop): add settings, appearance, and the quick switcher"
```

### Task 12: Fixture scenes, screenshots, and PR 4 docs

**Files:**
- Create: `ui/scripts/capture_screens.ts`, `docs/frontend_previews/desktop_<scene>_<appearance>.png`
- Modify: `ui/src/lib/fixture_backend.ts`, `ui/package.json` (dev dependency `playwright` 1.64.1, script `capture`), `PROGRESS.md`, `README.md`

**Interfaces:**
- Fixture mode reads `?scene=<name>&appearance=<light|dark>`. Scenes: `welcome`, `chat_running`, `chat_reply`, `chat_setup`, `tasks_panel`, `console`, `settings`, `switcher`, `daemon_stopped`. Every scene shows the DEMO tag.
- `capture_screens.ts` starts `vite --mode fixture --port 5181`, opens each scene in both appearances at 1440×900 with Chromium, and writes the PNG files. `npx playwright install chromium` is a one-time local step (about 150 MB).

- [ ] **Step 1: Capture**

Run (in `ui/`): `npm run capture`
Expected: 18 PNG files in `docs/frontend_previews/`.

- [ ] **Step 2: Review the captures** against the spec §5.6 and fix what does not match. Record in the PR description what was reviewed.

- [ ] **Step 3: Docs.** `PROGRESS.md`: the desktop row becomes `PARTIAL — FIXTURE AND LOCAL DAEMON` with a dated entry (commands, results, which screens were checked against a real local daemon, and what was not checked: no live vendor Ask). `README.md`: a "Desktop app" section with the build and run commands.

- [ ] **Step 4: Full gates**

Run: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test -p vibemux_client -p vibemux_frontend -p vibemux_desktop --all-features`, and in `ui/`: `npm run typecheck && npm test && npm run audit && npm run build`
Expected: all clean.

- [ ] **Step 5: Commit**

```bash
git add crates/vibemux_desktop/ui docs/frontend_previews PROGRESS.md README.md
git commit -m "docs(desktop): add fixture scenes and light and dark captures of every screen"
```

---

## PR 5 — Parity check and egui removal

Branch: `refactor/remove_egui_gui`.

### Task 13: Check parity, then remove the egui GUI

**Files:**
- Delete: `crates/vibemux_frontend/src/gui/`, `src/live_app.rs`, `src/live_ask.rs`, `src/live_console.rs`, `src/preview.rs`, `src/bin/vibemux_frontend.rs`, `src/bin/vibemux_frontend_preview.rs`, `tests/supervisor_ui.rs`, the old egui captures in `docs/frontend_previews/` (`supervisor_*.png`, `A-ledger.png`, `B-workbench.png`, `C-canvas.png`)
- Modify: `crates/vibemux_frontend/Cargo.toml` (drop `egui`, `eframe`, `image`, the `vibemux_client/folder_dialog` feature, the `gui_screenshot` feature, and the two `[[bin]]` entries), `crates/vibemux_frontend/src/lib.rs`, root `Cargo.toml` (drop `egui`/`eframe` workspace deps if unused), `deny.toml` (drop exceptions only egui needed), `docs/adr/028_supervisor_chat_frontend.md` and `030_claude_style_gui.md` (status line: "Presentation superseded by ADR 043 on <date>"), `CLAUDE.md`, `README.md`, `PROGRESS.md`

- [ ] **Step 1: Walk the parity checklist** (spec §5.7) in the running desktop app against a real local daemon. For each item record pass or fail in the PR description. Stop if any carried-over item fails; fix it in PR 4's branch first.

- [ ] **Step 2: Delete the files and dependencies listed above.** Remove any remaining item the compiler then reports as unused (`dead_code` under `-D warnings`), and nothing else.

- [ ] **Step 3: Verify nothing else used them**

Run: `cargo build --workspace --all-features` and `cargo tree -p vibemux_frontend -e normal | grep -E "egui|eframe|winit"`
Expected: builds; no output from the grep.

- [ ] **Step 4: Full gates**

Run: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-features`, `cargo deny check`
Expected: clean. The TUIs, the dashboard, `vibemuxctl ui`, and `vibemux_frontend_dump` still run.

- [ ] **Step 5: Docs.** CLAUDE.md drops the GUI command and the egui notes and points to the desktop app; README replaces the GUI section; PROGRESS.md records the removal and the parity results.

- [ ] **Step 6: Commit**

```bash
git add -A crates/vibemux_frontend Cargo.toml Cargo.lock deny.toml docs CLAUDE.md README.md PROGRESS.md
git commit -m "refactor(frontend): remove the egui GUI now that the desktop app covers it"
```
