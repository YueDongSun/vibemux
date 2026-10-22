#![forbid(unsafe_code)]
//! Native Supervisor Chat application shell and immutable snapshot bridge.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};

use eframe::egui::{self, Key, Modifiers, RichText, ViewportBuilder};
#[cfg(test)]
use vibemux_probe::AgentKind;

use crate::{
    daemon_control::DaemonCommand,
    project_folder,
    quick_search::{self, QuickAction, SearchTarget},
    supervisor_model::{
        ConnectionStatus, SnapshotMode, SupervisorAction, SupervisorSnapshot, TaskView,
    },
    theme::{
        Density, StartPage, TextSize, ThemeId, palette_for,
        serialize::{UserConfig, WindowSize, save_user_config},
    },
    view_model::ViewModel,
};

use super::{
    C,
    ask_state::AskState,
    ask_view::{self, AskViewActions, ConfirmChoice, ConfirmState},
    chat, composer,
    console::{self, ConsoleActions, ConsoleData, ConsoleSection},
    daemon_card::{self, DaemonJob},
    design, diagnostics, folders,
    quick_switcher::{self, QuickSwitcherState},
    settings, shortcuts,
    shortcuts::ShortcutAction,
    sidebar,
    supervisor_state::{ComposerTarget, MainPage, SupervisorUiState, UiActionQueue},
    task_detail, task_labels,
};

const WRITE_DEBOUNCE: Duration = Duration::from_millis(250);
pub(crate) const REFRESH_BUTTON_ID: &str = "topbar_refresh";
const SETTINGS_BUTTON_ID: &str = "topbar_settings";
const DIAGNOSTICS_BUTTON_ID: &str = "topbar_diagnostics";
const CONSOLE_BUTTON_ID: &str = "topbar_console";
const TOPBAR_MIN_HEIGHT: f32 = 52.0;
pub(crate) const DRAWER_PANEL_ID: &str = "task_details_drawer";
const DRAWER_CLOSE_ID: &str = "task_details_close";
const DRAWER_DEFAULT_WIDTH: f32 = 420.0;
const DRAWER_MIN_WIDTH: f32 = 360.0;
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(500);
const TASK_WINDOW_SIZE: [f32; 2] = [1080.0, 760.0];
const TASK_WINDOW_MIN_SIZE: [f32; 2] = [720.0, 520.0];

pub struct SupervisorApp {
    view_model: ViewModel,
    /// Shared so a frame reads the snapshot without deep-cloning every task,
    /// run, and event it holds. The GUI repaints continuously, so a per-frame
    /// clone of the whole snapshot was pure allocation churn.
    snapshot: Arc<RwLock<Arc<SupervisorSnapshot>>>,
    user_config: UserConfig,
    ui_state: Arc<Mutex<SupervisorUiState>>,
    actions: UiActionQueue,
    /// The full-page settings replace the main area while this is set.
    settings_open: bool,
    settings_section: settings::SettingsSection,
    /// The text size last applied as the egui zoom factor. Applied only
    /// when it changes, so the built-in `Ctrl` `+`/`-` zoom keeps working.
    applied_text_size: Option<TextSize>,
    /// The folder this window observes. The live adapter connects to its
    /// daemon; `None` until a folder is open.
    project_folder: Option<PathBuf>,
    /// A folder switch for the live adapter to pick up.
    project_switch: Option<PathBuf>,
    folder_dialog: folders::FolderDialogState,
    /// A start or stop for the live adapter to run.
    daemon_request: Option<DaemonCommand>,
    daemon_job: DaemonJob,
    /// Ask: what the folder's daemon can run, this window's Asks, consent,
    /// and the one-click setup.
    ask: AskState,
    /// What the Console shows besides the probe view model.
    console: ConsoleData,
    diagnostics_open: bool,
    /// The `Ctrl+/` shortcut overlay.
    shortcuts_open: bool,
    quick_switcher: QuickSwitcherState,
    last_size: [f32; 2],
    pending_write: Option<Instant>,
    pending_resize: Option<Instant>,
    /// Last laid-out height of the auto-sized composer panel.
    composer_panel_height: f32,
}

/// The UI state a new window opens with: the saved start page.
fn start_state(mut state: SupervisorUiState, start_page: StartPage) -> SupervisorUiState {
    match start_page {
        StartPage::Conversation => state.show_coordinator_chat(),
        StartPage::Console => state.show_console(),
    }
    state
}

impl SupervisorApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        view_model: ViewModel,
        user_config: UserConfig,
    ) -> Self {
        super::typography::install_fonts(&cc.egui_ctx);
        let actions = UiActionQueue::default();
        actions.enqueue(SupervisorAction::Refresh);
        let ui_state = SupervisorUiState::default();
        let project_folder = project_folder::startup_folder(
            user_config.active_project.as_deref(),
            std::env::current_dir().ok(),
        );
        Self {
            view_model,
            snapshot: Arc::new(RwLock::new(Arc::new(SupervisorSnapshot::unavailable()))),
            ui_state: Arc::new(Mutex::new(start_state(ui_state, user_config.start_page))),
            user_config,
            actions,
            settings_open: false,
            settings_section: settings::SettingsSection::default(),
            applied_text_size: None,
            project_folder,
            project_switch: None,
            folder_dialog: folders::FolderDialogState::default(),
            daemon_request: None,
            daemon_job: DaemonJob::Idle,
            ask: AskState::default(),
            console: ConsoleData::default(),
            diagnostics_open: false,
            shortcuts_open: false,
            quick_switcher: QuickSwitcherState::default(),
            last_size: [0.0, 0.0],
            pending_write: None,
            pending_resize: None,
            composer_panel_height: 0.0,
        }
    }

    /// The folder this window observes.
    #[must_use]
    pub fn active_project(&self) -> Option<&Path> {
        self.project_folder.as_deref()
    }

    /// A folder switch the user made since the last call.
    pub fn take_project_switch(&mut self) -> Option<PathBuf> {
        self.project_switch.take()
    }

    /// A start or stop the user asked for since the last call.
    pub fn take_daemon_request(&mut self) -> Option<DaemonCommand> {
        self.daemon_request.take()
    }

    /// Records how the live adapter's start or stop is going.
    pub fn set_daemon_job(&mut self, job: DaemonJob) {
        self.daemon_job = job;
    }

    /// The window's Ask state, which the live adapter drives.
    pub fn ask_state(&mut self) -> &mut AskState {
        &mut self.ask
    }

    /// The Console's data, which the live adapter fills.
    pub fn console_data(&mut self) -> &mut ConsoleData {
        &mut self.console
    }

    /// Whether the Console page is on screen, so its reads should run.
    #[must_use]
    pub fn console_open(&self) -> bool {
        !self.settings_open
            && self
                .ui_state
                .lock()
                .is_ok_and(|state| state.page() == MainPage::Console)
    }

    /// Opens the Console on `section` (sidebar, preview scenes).
    pub fn open_console(&mut self, section: ConsoleSection) {
        self.settings_open = false;
        if let Ok(mut state) = self.ui_state.lock() {
            state.show_console_section(section);
        }
    }

    /// Replaces the probe view model after a re-probe.
    pub fn set_view_model(&mut self, view_model: ViewModel) {
        self.view_model = view_model;
    }

    /// Opens `folder` when it is an existing directory: it becomes the
    /// active project, moves to the front of the recent folders, and the
    /// live adapter reconnects. Otherwise the dialog shows the error.
    pub fn open_project(&mut self, folder: PathBuf) {
        match project_folder::existing_folder(&folder) {
            Ok(folder) => {
                let changed = !self
                    .project_folder
                    .as_deref()
                    .is_some_and(|current| crate::theme::serialize::same_folder(current, &folder));
                self.user_config.remember_project(folder.clone());
                self.pending_write = Some(Instant::now());
                self.folder_dialog.close();
                if changed {
                    self.project_folder = Some(folder.clone());
                    self.project_switch = Some(folder);
                    self.daemon_job = DaemonJob::Idle;
                    // Asks and Console reads belong to the old folder.
                    self.ask = AskState::default();
                    self.console = ConsoleData::default();
                    self.settings_open = false;
                    if let Ok(mut state) = self.ui_state.lock() {
                        *state =
                            start_state(SupervisorUiState::default(), self.user_config.start_page);
                    }
                }
            }
            Err(error) => {
                self.folder_dialog.show();
                self.folder_dialog.input = folder.display().to_string();
                self.folder_dialog.error = Some(error);
            }
        }
    }

    /// Asks the live adapter to start or stop the active folder's daemon,
    /// unless one is already in flight or no folder is open.
    pub fn request_daemon(&mut self, command: DaemonCommand) {
        if self.daemon_job.is_running() {
            return;
        }
        if self.project_folder.is_none() {
            self.daemon_job = DaemonJob::Failed("Open a folder first.".to_string());
            return;
        }
        self.daemon_request = Some(command);
        self.daemon_job = DaemonJob::Running(command);
    }

    fn apply_folder_dialog(&mut self, actions: folders::FolderDialogActions) {
        if let Some(folder) = actions.forget {
            self.user_config.forget_project(&folder);
            self.pending_write = Some(Instant::now());
        }
        if actions.browse
            && let Some(folder) = project_folder::pick_folder_native(self.project_folder.as_deref())
        {
            self.open_project(folder);
            return;
        }
        if let Some(folder) = actions.open {
            self.open_project(folder);
            return;
        }
        if actions.close {
            self.folder_dialog.close();
        }
    }

    /// Mirrors the facts the composer gates Send on.
    fn sync_ask_gate(&self) {
        let ready = self.ask.availability.ready_names();
        let busy = self.ask.busy();
        if let Ok(mut state) = self.ui_state.lock() {
            state.set_ask_ready(ready);
            state.set_ask_busy(busy);
        }
    }

    /// Runs the prompt the composer queued: at once in a folder whose Ask
    /// is confirmed, otherwise after the confirmation dialog.
    fn route_pending_ask(&mut self) {
        let Some(pending) = self
            .ui_state
            .lock()
            .ok()
            .and_then(|mut state| state.take_pending_ask())
        else {
            return;
        };
        let Some(folder) = self.project_folder.clone() else {
            self.restore_draft(pending.prompt);
            return;
        };
        if self.user_config.ask_confirmed(&folder) {
            self.launch_ask(pending.harness, pending.prompt);
        } else {
            self.ask.confirm = ConfirmState {
                open: true,
                harness: pending.harness,
                prompt: pending.prompt,
                folder: folder.display().to_string(),
            };
        }
    }

    fn launch_ask(&mut self, harness: String, prompt: String) {
        match self.ask.launch(harness, prompt) {
            Ok(()) => {
                if let Ok(mut state) = self.ui_state.lock() {
                    state.show_coordinator_chat();
                }
            }
            Err(prompt) => self.restore_draft(prompt),
        }
    }

    fn restore_draft(&self, prompt: String) {
        if let Ok(mut state) = self.ui_state.lock() {
            state.restore_draft(prompt);
        }
    }

    /// Send records consent for this folder and starts the Ask; Cancel puts
    /// the prompt back into the composer.
    fn apply_ask_confirm(&mut self, choice: ConfirmChoice) {
        if choice == ConfirmChoice::None {
            return;
        }
        let confirm = std::mem::take(&mut self.ask.confirm);
        if choice == ConfirmChoice::Cancel {
            self.restore_draft(confirm.prompt);
            return;
        }
        if let Some(folder) = self.project_folder.clone() {
            self.user_config.confirm_ask(folder);
            self.pending_write = Some(Instant::now());
        }
        self.launch_ask(confirm.harness, confirm.prompt);
    }

    fn apply_console_actions(&mut self, actions: ConsoleActions) {
        if let Ok(mut state) = self.ui_state.lock() {
            if let Some(section) = actions.section {
                state.show_console_section(section);
            }
            match actions.harness {
                Some(Some(index)) => {
                    state.select_harness(index, self.view_model.agents.len());
                }
                Some(None) => state.return_to_harness_list(),
                None => {}
            }
            if let Some(harness) = actions.ask {
                state.set_composer_target(ComposerTarget::Harness(harness));
                state.show_coordinator_chat();
                state.request_composer_focus();
            }
        }
        if actions.reprobe {
            self.console.request_reprobe();
        }
        if actions.refresh {
            self.console.request_refresh();
        }
        if let Some(task_id) = actions.open_task {
            self.console.request_task(task_id);
        }
        if let Some(task_id) = actions.task_window {
            self.open_task_window(&task_id);
        }
    }

    fn apply_ask_actions(&mut self, actions: AskViewActions) {
        if actions.cancel {
            self.ask.request_cancel();
        }
        if actions.enable {
            self.ask.request_setup();
        }
        if actions.dismiss_setup {
            self.ask.dismiss_setup();
        }
        if actions.recheck {
            self.ask.request_recheck();
        }
    }

    /// Replace the displayed immutable supervisor snapshot.
    pub fn set_supervisor_snapshot(&mut self, snapshot: SupervisorSnapshot) {
        self.replace_snapshot(Arc::new(snapshot));
    }

    /// Publish an already-shared snapshot without copying it again.
    pub(crate) fn replace_snapshot(&mut self, snapshot: Arc<SupervisorSnapshot>) {
        if let Ok(mut current) = self.snapshot.write() {
            let published = Arc::clone(&snapshot);
            *current = snapshot;
            if let Ok(mut state) = self.ui_state.lock() {
                state.reconcile_snapshot(&published);
            }
        }
    }

    /// Borrow the current snapshot without cloning its contents.
    fn with_snapshot<R>(&self, body: impl FnOnce(&SupervisorSnapshot) -> R) -> R {
        let guard = self.snapshot.read();
        match guard {
            Ok(snapshot) => body(snapshot.as_ref()),
            Err(_) => body(&SupervisorSnapshot::unavailable()),
        }
    }

    /// Drain bounded requests for the parent runtime adapter.
    #[must_use]
    pub fn take_supervisor_actions(&self) -> Vec<SupervisorAction> {
        self.actions.drain()
    }

    /// Select a task and open its in-app detail drawer.
    pub fn select_task(&mut self, task_id: &str) -> bool {
        let mut selected = false;
        self.with_snapshot(|snapshot| {
            if let Ok(mut state) = self.ui_state.lock() {
                selected = state.select_task(snapshot, task_id);
            }
        });
        if selected {
            self.actions.enqueue(SupervisorAction::LoadTask {
                task_id: task_id.to_string(),
            });
        }
        selected
    }

    /// Open one native task viewport, reusing the stable task viewport ID.
    pub fn open_task_window(&mut self, task_id: &str) -> bool {
        let mut opened = false;
        self.with_snapshot(|snapshot| {
            if let Ok(mut state) = self.ui_state.lock() {
                opened = state.open_task_window(snapshot, task_id);
            }
        });
        if opened {
            self.actions.enqueue(SupervisorAction::LoadTask {
                task_id: task_id.to_string(),
            });
        }
        opened
    }

    /// Select the content tab of an existing native task view.
    pub fn set_task_window_tab(&mut self, task_id: &str, tab: super::TaskDetailTab) {
        self.with_snapshot(|snapshot| {
            if let Some(task) = snapshot.tasks.iter().find(|task| task.task_id == task_id)
                && let Ok(state) = self.ui_state.lock()
            {
                state
                    .with_task_window_selection_mut(task_id, task, |selection| selection.tab = tab);
            }
        });
    }

    /// Collapse or expand the sidebar and persist the choice.
    pub fn set_sidebar_collapsed(&mut self, collapsed: bool) {
        if self.user_config.sidebar_collapsed != collapsed {
            self.user_config.sidebar_collapsed = collapsed;
            self.pending_write = Some(Instant::now());
        }
    }

    /// Points the composer at `target` (preview scenes).
    pub fn set_composer_target(&mut self, target: super::ComposerTarget) {
        if let Ok(mut state) = self.ui_state.lock() {
            state.set_composer_target(target);
        }
    }

    /// Start a new task (sidebar button, `Ctrl+N`, quick switcher, preview).
    pub fn request_new_task(&mut self) {
        if let Ok(mut state) = self.ui_state.lock() {
            let _ = state.request_new_task();
        }
    }

    fn enqueue_refresh(&self) {
        self.actions.enqueue(SupervisorAction::Refresh);
    }

    fn persist_if_due(&mut self) {
        if self.with_snapshot(|snapshot| snapshot.mode) == SnapshotMode::Demo {
            return;
        }
        let (pending_write, pending_resize, should_save) =
            consume_due_persistence(self.pending_write, self.pending_resize, Instant::now());
        self.pending_write = pending_write;
        self.pending_resize = pending_resize;
        if should_save {
            save_user_config(&self.user_config);
        }
    }

    fn set_theme(&mut self, theme: ThemeId) {
        if self.user_config.theme != theme {
            self.user_config.theme = theme;
            self.pending_write = Some(Instant::now());
        }
    }

    fn set_window_size(&mut self, size: WindowSize) {
        let size = size.clamped();
        if self.user_config.window_size != size {
            self.user_config.window_size = size;
            self.pending_resize = Some(Instant::now());
        }
    }

    fn set_start_page(&mut self, start_page: StartPage) {
        if self.user_config.start_page != start_page {
            self.user_config.start_page = start_page;
            self.pending_write = Some(Instant::now());
        }
    }

    fn set_density(&mut self, density: Density) {
        if self.user_config.density != density {
            self.user_config.density = density;
            self.pending_write = Some(Instant::now());
        }
    }

    fn set_text_size(&mut self, text_size: TextSize) {
        if self.user_config.text_size != text_size {
            self.user_config.text_size = text_size;
            self.pending_write = Some(Instant::now());
        }
    }

    /// Opens the full-page settings at `section`.
    pub fn open_settings(&mut self, section: settings::SettingsSection) {
        self.settings_section = section;
        self.settings_open = true;
    }

    fn apply_settings_changes(&mut self, changes: settings::SettingsChanges) {
        if let Some(section) = changes.section {
            self.settings_section = section;
        }
        if changes.close {
            self.settings_open = false;
        }
        if let Some(theme) = changes.theme {
            self.set_theme(theme);
        }
        if let Some(size) = changes.window_size {
            self.set_window_size(size);
        }
        if let Some(start_page) = changes.start_page {
            self.set_start_page(start_page);
        }
        if let Some(density) = changes.density {
            self.set_density(density);
        }
        if let Some(text_size) = changes.text_size {
            self.set_text_size(text_size);
        }
        if changes.open_folder_dialog {
            self.folder_dialog.show();
        }
        if let Some(command) = changes.daemon {
            self.request_daemon(command);
        }
    }

    /// Applies the saved density and text size to this frame's style.
    fn apply_display_preferences(&mut self, ctx: &egui::Context) {
        super::theme::apply_density(ctx, self.user_config.density);
        if self.applied_text_size != Some(self.user_config.text_size) {
            ctx.set_zoom_factor(self.user_config.text_size.zoom_factor());
            self.applied_text_size = Some(self.user_config.text_size);
        }
    }

    /// Applies a table chord (ADR 030 §5). Each only changes what this
    /// window shows; none sends anything.
    fn apply_shortcut(&mut self, action: ShortcutAction) {
        match action {
            ShortcutAction::QuickSwitcher => self.quick_switcher.open(),
            ShortcutAction::NewTask => self.request_new_task(),
            ShortcutAction::ToggleSidebar => {
                self.set_sidebar_collapsed(!self.user_config.sidebar_collapsed);
            }
            ShortcutAction::Settings => self.settings_open = true,
            ShortcutAction::Overlay => self.shortcuts_open = !self.shortcuts_open,
        }
    }

    /// Opens what the quick switcher picked. A task opens its drawer.
    pub(crate) fn apply_search_target(&mut self, target: SearchTarget) {
        match target {
            SearchTarget::Task(task_id) => {
                let _ = self.select_task(&task_id);
            }
            SearchTarget::Agent(index) => {
                if let Ok(mut state) = self.ui_state.lock() {
                    state.select_harness(index, self.view_model.agents.len());
                }
            }
            SearchTarget::Action(action) => match action {
                QuickAction::NewTask => self.request_new_task(),
                QuickAction::Settings => self.settings_open = true,
                QuickAction::Diagnostics => self.diagnostics_open = true,
                QuickAction::KeyboardShortcuts => self.shortcuts_open = true,
                QuickAction::ToggleSidebar => {
                    self.set_sidebar_collapsed(!self.user_config.sidebar_collapsed);
                }
                QuickAction::Light => self.set_theme(ThemeId::ClaudeLight),
                QuickAction::Dark => self.set_theme(ThemeId::Claude),
            },
        }
    }

    fn handle_keyboard(&mut self, ctx: &egui::Context) {
        if ime_composition_active(ctx) {
            return;
        }
        // Table chords are consumed before any text widget sees them, so
        // they also work while the composer has focus.
        if let Some(action) = shortcuts::consume(ctx) {
            self.apply_shortcut(action);
            return;
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        if ctx.input(|input| input.key_pressed(Key::Escape)) {
            if self.folder_dialog.open {
                self.folder_dialog.close();
                return;
            }
            if self.quick_switcher.open {
                self.quick_switcher.close();
                return;
            }
            if self.shortcuts_open {
                self.shortcuts_open = false;
                return;
            }
            if let Ok(mut state) = self.ui_state.lock()
                && state.discard_prompt_open()
            {
                state.keep_draft();
                return;
            }
            if self.diagnostics_open {
                self.diagnostics_open = false;
                return;
            }
            if self.settings_open {
                self.settings_open = false;
                return;
            }
            if let Ok(mut state) = self.ui_state.lock() {
                if state.details_open() {
                    state.close_details();
                    return;
                }
                if state.page() != MainPage::CoordinatorChat {
                    state.show_coordinator_chat();
                }
            }
            return;
        }

        let input = ctx.input(|input| input.clone());
        let ctrl = input.modifiers == Modifiers::CTRL || input.modifiers == Modifiers::COMMAND;
        if !ctrl {
            return;
        }
        const DIGITS: [(Key, usize); 10] = [
            (Key::Num1, 0),
            (Key::Num2, 1),
            (Key::Num3, 2),
            (Key::Num4, 3),
            (Key::Num5, 4),
            (Key::Num6, 5),
            (Key::Num7, 6),
            (Key::Num8, 7),
            (Key::Num9, 8),
            (Key::Num0, 9),
        ];
        if let Some((_, index)) = DIGITS.into_iter().find(|(key, _)| input.key_pressed(*key))
            && let Ok(mut state) = self.ui_state.lock()
        {
            state.select_harness(index, self.view_model.agents.len());
        }
    }

    fn render_topbar(
        &self,
        root_ui: &mut egui::Ui,
        colors: &C,
        snapshot: &SupervisorSnapshot,
    ) -> Option<HeaderAction> {
        let mut requested = None;
        egui::Panel::top("supervisor_header")
            .show_separator_line(true)
            .min_size(TOPBAR_MIN_HEIGHT)
            .frame(
                egui::Frame::new()
                    .fill(colors.bg)
                    .inner_margin(egui::Margin::symmetric(20, 12)),
            )
            .show_inside(root_ui, |ui| {
                ui.horizontal(|ui| {
                    let title = snapshot
                        .coordinator
                        .as_deref()
                        .filter(|name| !name.trim().is_empty())
                        .unwrap_or("Coordinator");
                    ui.label(
                        RichText::new("Main conversation")
                            .size(15.0)
                            .strong()
                            .color(colors.txt),
                    );
                    ui.label(
                        RichText::new(format!("/  {title}"))
                            .size(12.0)
                            .color(colors.muted),
                    );
                    ui.add_space(12.0);
                    connection_indicator(ui, colors, snapshot.connection.status);
                    if snapshot.mode == SnapshotMode::Demo {
                        ui.add_space(10.0);
                        chat::demo_badge(ui, colors);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Added right-to-left, so this order reads
                        // Refresh · Console · Diagnostics · Settings on screen.
                        if design::icon_button(
                            ui,
                            colors,
                            design::Icon::Refresh,
                            "Refresh now",
                            egui::Id::new(REFRESH_BUTTON_ID),
                        )
                        .clicked()
                        {
                            self.enqueue_refresh();
                        }
                        for (kind, tooltip, id, action) in [
                            (
                                design::Icon::Settings,
                                "Settings",
                                SETTINGS_BUTTON_ID,
                                HeaderAction::Settings,
                            ),
                            (
                                design::Icon::Pulse,
                                "Diagnostics",
                                DIAGNOSTICS_BUTTON_ID,
                                HeaderAction::Diagnostics,
                            ),
                            (
                                design::Icon::Grid,
                                "Console",
                                CONSOLE_BUTTON_ID,
                                HeaderAction::Console,
                            ),
                        ] {
                            if design::icon_button(ui, colors, kind, tooltip, egui::Id::new(id))
                                .clicked()
                            {
                                requested = Some(action);
                            }
                        }
                    });
                });
                ui.horizontal_top(|ui| {
                    if let Some(detail) = snapshot.connection.detail.as_deref() {
                        chat::wrapped_label(
                            ui,
                            RichText::new(detail).size(12.0).color(colors.muted),
                        );
                    }
                    // Data age is the one thing a polling reader cannot infer,
                    // so it belongs beside the control that refreshes it.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(observed) = freshness_label(snapshot.observed_at.as_deref()) {
                            ui.label(RichText::new(observed).size(12.0).color(colors.faint));
                        }
                    });
                });
            });
        requested
    }

    fn render_drawer(&mut self, root_ui: &mut egui::Ui, colors: &C, viewport_width: f32) {
        let rendered = {
            let snapshot = match self.snapshot.read() {
                Ok(guard) => Arc::clone(&guard),
                Err(_) => return,
            };
            // Resolve the selection before opening the panel so a selection that
            // no longer resolves does not leave an empty drawer docked.
            let open = self.ui_state.lock().is_ok_and(|state| {
                state.details_open() && state.selected_task(snapshot.as_ref()).is_some()
            });
            if !open {
                return;
            }
            let mut close_clicked = false;
            let max_width = (viewport_width * 0.5).max(DRAWER_MIN_WIDTH);
            egui::Panel::right(DRAWER_PANEL_ID)
                .resizable(true)
                .default_size(DRAWER_DEFAULT_WIDTH.min(max_width))
                .size_range(DRAWER_MIN_WIDTH..=max_width)
                .frame(
                    egui::Frame::new()
                        .fill(colors.bg)
                        .inner_margin(egui::Margin::symmetric(18, 14)),
                )
                .show_inside(root_ui, |ui| {
                    let Some(task) = self
                        .ui_state
                        .lock()
                        .ok()
                        .and_then(|state| state.selected_task(snapshot.as_ref()))
                    else {
                        return;
                    };
                    drawer_contents(
                        ui,
                        colors,
                        snapshot.as_ref(),
                        task,
                        &self.ui_state,
                        &self.actions,
                        &mut close_clicked,
                    );
                });
            close_clicked
        };
        if rendered && let Ok(mut state) = self.ui_state.lock() {
            state.close_details();
        }
    }

    fn render_task_viewports(&self, ctx: &egui::Context, theme: ThemeId) {
        let open_tasks = self
            .ui_state
            .lock()
            .map_or_else(|_| Vec::new(), |state| state.open_task_windows());
        let colors = super::pal(&palette_for(theme));
        for task_id in open_tasks {
            let title = self.with_snapshot(|snapshot| {
                task_labels::task_window_title(
                    snapshot
                        .tasks
                        .iter()
                        .find(|task| task.task_id == task_id)
                        .map(|task| task.title.as_str()),
                )
            });
            let viewport_id = chat::task_viewport_id(&task_id);
            let viewport = ViewportBuilder::default()
                .with_title(title)
                .with_inner_size(TASK_WINDOW_SIZE)
                .with_min_inner_size(TASK_WINDOW_MIN_SIZE);
            let snapshot_arc = Arc::clone(&self.snapshot);
            let state_arc = Arc::clone(&self.ui_state);
            let actions = self.actions.clone();
            let root_ctx = ctx.clone();
            let task_colors = colors;
            ctx.show_viewport_deferred(viewport_id, viewport, move |child_ui, _class| {
                let close_requested = child_ui.input(|input| input.viewport().close_requested());
                if close_requested {
                    actions.report_task_window_closed(task_id.clone());
                    root_ctx.request_repaint();
                    return;
                }
                // Clone the shared handle, never the snapshot body: this runs
                // on every frame of every open task window.
                let snapshot = match snapshot_arc.read() {
                    Ok(snapshot) => Arc::clone(&snapshot),
                    Err(_) => Arc::new(SupervisorSnapshot::unavailable()),
                };
                let Some(task) = snapshot.tasks.iter().find(|task| task.task_id == task_id) else {
                    egui::CentralPanel::default().show_inside(child_ui,|ui| {
                        ui.heading("Task observation unavailable");
                        ui.label("This task is absent from the current page or connection. Its execution has not been changed.");
                        if ui.button("Refresh task").clicked() {
                            actions.enqueue(SupervisorAction::LoadTask {task_id:task_id.clone()});
                            root_ctx.request_repaint();
                        }
                    });
                    return;
                };
                super::apply_theme(child_ui.ctx(), &palette_for(theme));
                child_ui.set_style(child_ui.ctx().global_style());
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::new()
                            .fill(task_colors.bg)
                            .inner_margin(egui::Margin::same(18)),
                    )
                    .show_inside(child_ui, |ui| {
                        let Ok(state) = state_arc.lock() else {
                            ui.label("Task window state is temporarily unavailable.");
                            return;
                        };
                        state.with_task_window_selection_mut(&task_id, task, |selection| {
                            // Same reading measure as the conversation surface,
                            // so a detached window is not a wider, looser page.
                            chat::content_column(ui, |ui| {
                                task_detail::render(
                                    ui,
                                    &task_colors,
                                    snapshot.as_ref(),
                                    task,
                                    selection,
                                    &actions,
                                    task_detail::TitleDisplay::Shown,
                                );
                            });
                        });
                    });
            });
        }
        if let Ok(mut state) = self.ui_state.lock() {
            for task_id in self.actions.take_closed_task_windows() {
                state.close_task_window(&task_id);
            }
        }
    }

    fn track_window_size(&mut self, ctx: &egui::Context) {
        let size = ctx
            .input(|input| input.viewport().inner_rect)
            .map_or([0.0, 0.0], |rect| [rect.width(), rect.height()]);
        if self.last_size[0] > 0.0 && size != self.last_size {
            self.set_window_size(WindowSize {
                width: size[0] as u32,
                height: size[1] as u32,
            });
        }
        self.last_size = size;
    }
}

impl eframe::App for SupervisorApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.render_frame(ui);
    }
}

impl SupervisorApp {
    fn render_frame(&mut self, root_ui: &mut egui::Ui) {
        let context = root_ui.ctx().clone();
        let ctx = &context;
        if self.actions.take_overflowed()
            && let Ok(mut snapshot) = self.snapshot.write()
        {
            Arc::make_mut(&mut snapshot).connection.detail = Some(
                "Action queue is full. The last action was not sent; please try again.".into(),
            );
        }
        let palette = palette_for(self.user_config.theme);
        super::apply_theme(ctx, &palette);
        ctx.global_style_mut(|style| {
            style
                .text_styles
                .insert(egui::TextStyle::Body, egui::FontId::proportional(15.0));
        });
        self.apply_display_preferences(ctx);
        root_ui.set_style(ctx.global_style());
        let colors = super::pal(&palette);
        let target_options = composer::target_options(&self.view_model);
        self.persist_if_due();
        self.handle_keyboard(ctx);
        self.sync_ask_gate();

        let viewport_width = ctx
            .input(|input| input.viewport().inner_rect.map(|rect| rect.width()))
            .unwrap_or(1280.0);
        let active_page = self
            .ui_state
            .lock()
            .map_or(MainPage::CoordinatorChat, |state| state.page());
        let (welcome_active, selected_task_id) =
            self.ui_state.lock().map_or((false, None), |state| {
                (
                    state.welcome_active(),
                    state.selected_task_id().map(str::to_owned),
                )
            });
        let collapsed = self.user_config.sidebar_collapsed;
        let mut sidebar_actions = None;
        let mut console_actions = None;
        // A poisoned lock means no readable snapshot; drop the frame rather than
        // drawing from a fabricated one.
        let mut switcher_target = None;
        let snapshot_arc = match self.snapshot.read() {
            Ok(guard) => Arc::clone(&guard),
            Err(_) => return,
        };
        let header_action;
        {
            // One shared borrow for the whole frame: the panels below read the
            // same snapshot instead of each cloning it.
            let snapshot = snapshot_arc.as_ref();
            egui::Panel::left(sidebar::SIDEBAR_PANEL_ID)
                .exact_size(sidebar::width_for(collapsed))
                .resizable(false)
                .frame(egui::Frame::new().fill(colors.surf))
                .show_inside(root_ui, |ui| {
                    let view = sidebar::SidebarView {
                        snapshot,
                        page: active_page,
                        welcome_active,
                        collapsed,
                        selected_task_id: selected_task_id.as_deref(),
                        settings_open: self.settings_open,
                        active_folder: self.project_folder.as_deref(),
                        recent_folders: &self.user_config.recent_projects,
                    };
                    sidebar_actions = Some(sidebar::render(ui, &colors, &view));
                });

            // Applied here, before the panels that read `page`, so a sidebar
            // navigation lands in the frame it was clicked rather than the next.
            if let Some(actions) = sidebar_actions.take() {
                if actions.page.is_some() {
                    // Choosing a page leaves the full-page settings.
                    self.settings_open = false;
                }
                if let Some(page) = actions.page
                    && let Ok(mut state) = self.ui_state.lock()
                {
                    match page {
                        MainPage::CoordinatorChat => state.show_coordinator_chat(),
                        MainPage::Console => state.show_console(),
                    }
                }
                if let Some(task_id) = actions.task_id {
                    self.select_task(&task_id);
                }
                if actions.new_task {
                    self.request_new_task();
                }
                if actions.toggle_collapsed {
                    self.set_sidebar_collapsed(!collapsed);
                }
                if actions.open_settings {
                    self.settings_open = true;
                }
                if actions.open_diagnostics {
                    self.diagnostics_open = true;
                }
                if actions.open_folder_dialog {
                    self.folder_dialog.show();
                }
                if let Some(folder) = actions.switch_folder {
                    self.open_project(folder);
                }
            }

            header_action = self.render_topbar(root_ui, &colors, snapshot);
            if let Some(action) = header_action {
                match action {
                    HeaderAction::Settings => self.settings_open = true,
                    HeaderAction::Diagnostics => self.diagnostics_open = true,
                    HeaderAction::Console => {
                        if let Ok(mut state) = self.ui_state.lock() {
                            state.show_console();
                        }
                    }
                }
            }
            if !self.settings_open {
                self.render_drawer(root_ui, &colors, viewport_width);
            }

            let page = self
                .ui_state
                .lock()
                .map_or(MainPage::CoordinatorChat, |state| state.page());
            let welcome = page == MainPage::CoordinatorChat
                && (self
                    .ui_state
                    .lock()
                    .is_ok_and(|state| state.welcome_active())
                    || (snapshot.tasks.is_empty() && self.ask.asks.is_empty()));

            if page == MainPage::CoordinatorChat && !welcome && !self.settings_open {
                let composer_panel = egui::Panel::bottom("coordinator_composer")
                    .show_separator_line(false)
                    .frame(
                        egui::Frame::new()
                            .fill(colors.bg)
                            .inner_margin(egui::Margin::symmetric(18, 10)),
                    )
                    .show_inside(root_ui, |ui| {
                        if let Ok(mut state) = self.ui_state.lock() {
                            chat::content_column(ui, |ui| {
                                composer::render(ui, &colors, &mut state, &target_options);
                            });
                        }
                    });
                // The panel sizes to its content one pass late; redo the pass
                // when its height changes so no frame shows an overflowing composer.
                let height = composer_panel.response.rect.height();
                if (height - self.composer_panel_height).abs() > 0.5 {
                    self.composer_panel_height = height;
                    ctx.request_discard("composer panel height changed");
                }
            }

            let mut settings_changes = None;
            let mut daemon_click = None;
            let mut ask_actions = AskViewActions::default();
            let harness_target =
                self.ui_state
                    .lock()
                    .ok()
                    .and_then(|state| match state.composer_target() {
                        ComposerTarget::Harness(name) => Some(name.clone()),
                        ComposerTarget::Coordinator => None,
                    });
            let show_setup = ask_view::setup_card_visible(
                &self.ask.availability,
                &self.ask.setup,
                harness_target.is_some(),
            );
            let ask_note =
                ask_view::availability_note(&self.ask.availability, harness_target.as_deref());
            let daemon_missing = daemon_card::daemon_missing(&snapshot.connection);
            let folder_name = self.project_folder.as_deref().map_or_else(
                || snapshot.project_name.clone(),
                project_folder::display_name,
            );
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new()
                        .fill(colors.bg)
                        .inner_margin(egui::Margin::symmetric(18, 12)),
                )
                .show_inside(root_ui, |ui| match page {
                    _ if self.settings_open => {
                        let view = settings::SettingsView {
                            section: self.settings_section,
                            config: &self.user_config,
                            snapshot,
                            project_folder: self.project_folder.as_deref(),
                            daemon_job: &self.daemon_job,
                        };
                        ui.add_space(8.0);
                        settings_changes = Some(settings::render_page(ui, &colors, &view));
                    }
                    MainPage::CoordinatorChat if welcome => {
                        // Centre the greeting/composer block against the
                        // composer height measured on the previous pass, so the
                        // empty space above and below stays balanced at any
                        // window size instead of being a fixed fraction.
                        let top_space =
                            welcome_top_space(ui.available_height(), self.composer_panel_height);
                        ui.add_space(top_space);
                        if daemon_missing {
                            chat::content_column(ui, |ui| {
                                daemon_click = daemon_card::render(
                                    ui,
                                    &colors,
                                    &folder_name,
                                    &self.daemon_job,
                                );
                            });
                            ui.add_space(18.0);
                        }
                        if show_setup || ask_note.is_some() {
                            chat::content_column(ui, |ui| {
                                ask_actions.merge(ask_view::render_notices(
                                    ui,
                                    &colors,
                                    &self.ask.setup,
                                    show_setup,
                                    ask_note.as_deref(),
                                ));
                            });
                            ui.add_space(6.0);
                        }
                        let hint = super::welcome::workspace_hint(snapshot.tasks.len());
                        chat::content_column(ui, |ui| {
                            super::welcome::render_heading(
                                ui,
                                &colors,
                                super::welcome::greeting_for_hour(
                                    super::welcome::current_local_hour(),
                                ),
                                Some(&hint),
                            );
                        });
                        if let Ok(mut state) = self.ui_state.lock() {
                            chat::content_column(ui, |ui| {
                                composer::render(ui, &colors, &mut state, &target_options);
                            });
                        }
                    }
                    MainPage::CoordinatorChat => {
                        if daemon_missing {
                            chat::content_column(ui, |ui| {
                                daemon_click = daemon_card::render(
                                    ui,
                                    &colors,
                                    &folder_name,
                                    &self.daemon_job,
                                );
                            });
                            ui.add_space(12.0);
                        }
                        if show_setup || ask_note.is_some() {
                            chat::content_column(ui, |ui| {
                                ask_actions.merge(ask_view::render_notices(
                                    ui,
                                    &colors,
                                    &self.ask.setup,
                                    show_setup,
                                    ask_note.as_deref(),
                                ));
                            });
                        }
                        if let Ok(mut state) = self.ui_state.lock() {
                            let asks = &self.ask.asks;
                            let mut ask_tail = |ui: &mut egui::Ui| {
                                ask_actions.merge(ask_view::render_asks(ui, &colors, asks));
                            };
                            let tail = (!asks.is_empty())
                                .then_some(&mut ask_tail as &mut dyn FnMut(&mut egui::Ui));
                            chat::render_conversation(
                                ui,
                                ctx,
                                &colors,
                                snapshot,
                                &mut state,
                                &self.actions,
                                tail,
                            );
                        }
                    }
                    MainPage::Console => {
                        let (section, selected_harness) = self
                            .ui_state
                            .lock()
                            .map_or((ConsoleSection::default(), None), |state| {
                                (state.console_section(), state.selected_harness())
                            });
                        let ask_ready = self.ask.availability.ready_names();
                        let view = console::ConsoleView {
                            section,
                            selected_harness,
                            view_model: &self.view_model,
                            ask_ready: &ask_ready,
                            folder: self.project_folder.as_deref(),
                            snapshot,
                            data: &self.console,
                        };
                        console_actions = Some(console::render_page(ui, &colors, &view));
                    }
                });

            if let Some(changes) = settings_changes {
                self.apply_settings_changes(changes);
            }
            if let Some(command) = daemon_click {
                self.request_daemon(command);
            }
            self.apply_ask_actions(ask_actions);
            self.route_pending_ask();
            if self.ask.confirm.open {
                let choice = ask_view::render_confirm(ctx, &colors, &self.ask.confirm);
                self.apply_ask_confirm(choice);
            }
            if self.folder_dialog.open {
                let actions = folders::render(
                    ctx,
                    &colors,
                    &mut self.folder_dialog,
                    &self.user_config.recent_projects,
                    self.project_folder.as_deref(),
                );
                self.apply_folder_dialog(actions);
            }
            if self.diagnostics_open {
                diagnostics::render(
                    ctx,
                    &colors,
                    &mut self.diagnostics_open,
                    snapshot,
                    &self.view_model,
                );
            }
            if self.shortcuts_open {
                shortcuts::render_overlay(ctx, &colors, &mut self.shortcuts_open);
            }
            if self.quick_switcher.open {
                let agent_names: Vec<String> = self
                    .view_model
                    .agents
                    .iter()
                    .map(|agent| agent.name.clone())
                    .collect();
                let entries = quick_search::build_entries(&snapshot.tasks, &agent_names);
                switcher_target =
                    quick_switcher::show(ctx, &colors, &mut self.quick_switcher, &entries);
            }
        }
        if let Some(target) = switcher_target {
            self.apply_search_target(target);
        }
        if let Some(actions) = console_actions {
            self.apply_console_actions(actions);
        }

        if self
            .ui_state
            .lock()
            .is_ok_and(|state| state.discard_prompt_open())
        {
            let modal = egui::Modal::new(egui::Id::new("discard_draft_prompt")).show(ctx, |ui| {
                ui.set_width(320.0);
                ui.label(
                    RichText::new("Discard current draft?")
                        .size(16.0)
                        .color(colors.txt),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new("The draft has not been sent and will be removed.")
                        .size(13.0)
                        .color(colors.muted),
                );
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    let discard = ui.button("Discard").clicked();
                    let keep = ui.button("Keep editing").clicked();
                    (discard, keep)
                })
                .inner
            });
            let (discard, keep) = modal.inner;
            if let Ok(mut state) = self.ui_state.lock() {
                if discard {
                    state.confirm_discard_draft();
                } else if keep || modal.should_close() {
                    state.keep_draft();
                }
            }
        }

        self.render_task_viewports(ctx, self.user_config.theme);
        self.track_window_size(ctx);
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

fn drawer_contents(
    ui: &mut egui::Ui,
    colors: &C,
    snapshot: &SupervisorSnapshot,
    task: &TaskView,
    state: &Arc<Mutex<SupervisorUiState>>,
    actions: &UiActionQueue,
    close_clicked: &mut bool,
) {
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let close = design::icon_button(
                ui,
                colors,
                design::Icon::Close,
                "Close task details",
                egui::Id::new(DRAWER_CLOSE_ID),
            );
            if close.clicked() {
                *close_clicked = true;
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(task_labels::task_title(&task.title))
                            .font(super::typography::display_font(
                                ui.ctx(),
                                super::typography::TITLE_SIZE,
                            ))
                            .color(colors.txt),
                    )
                    .truncate(),
                );
            });
        });
    });
    ui.add_space(6.0);
    if let Ok(mut state) = state.lock() {
        task_detail::render(
            ui,
            colors,
            snapshot,
            task,
            state.details_selection_mut(),
            actions,
            task_detail::TitleDisplay::InHeader,
        );
    }
}

fn connection_indicator(ui: &mut egui::Ui, colors: &C, status: ConnectionStatus) {
    let (label, color) = match status {
        ConnectionStatus::Connected => ("Connected", colors.ok),
        ConnectionStatus::Connecting => ("Connecting", colors.warn),
        ConnectionStatus::Disconnected => ("Disconnected", colors.muted),
        ConnectionStatus::Unavailable => ("Unavailable", colors.muted),
    };
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, color);
    ui.label(RichText::new(label).size(12.0).color(colors.muted));
}

fn ime_composition_active(ctx: &egui::Context) -> bool {
    ctx.input(|input| {
        input
            .events
            .iter()
            .any(|event| matches!(event, egui::Event::Ime(_)))
    })
}

fn consume_due_persistence(
    pending_write: Option<Instant>,
    pending_resize: Option<Instant>,
    now: Instant,
) -> (Option<Instant>, Option<Instant>, bool) {
    let write_due = pending_write
        .is_some_and(|pending| now.saturating_duration_since(pending) >= WRITE_DEBOUNCE);
    let resize_due = pending_resize
        .is_some_and(|pending| now.saturating_duration_since(pending) >= RESIZE_DEBOUNCE);
    (
        if write_due { None } else { pending_write },
        if resize_due { None } else { pending_resize },
        write_due || resize_due,
    )
}

/// A control the main-page header asked for. These are existing capabilities
/// that were previously reachable only from the sidebar footer popup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HeaderAction {
    Settings,
    Diagnostics,
    Console,
}

/// Human clock time from the snapshot's RFC 3339 observation stamp.
///
/// Formats the clock explicitly instead of slicing a formatted string, so no
/// date or numeric offset can leak into the label. Returns `None` when the
/// stamp is absent, unparseable, or the local offset is unknown rather than
/// guessing: a freshness indicator that invents a time is worse than none.
#[must_use]
fn freshness_label(observed_at: Option<&str>) -> Option<String> {
    let stamp = observed_at?;
    let parsed =
        time::OffsetDateTime::parse(stamp, &time::format_description::well_known::Rfc3339).ok()?;
    let local = parsed.to_offset(time::UtcOffset::current_local_offset().ok()?);
    Some(format!(
        "Updated {:02}:{:02}:{:02}",
        local.hour(),
        local.minute(),
        local.second()
    ))
}

/// Height of everything in the welcome block that is not the composer: the
/// spark, its gaps, the greeting line, the workspace hint, and the draft note.
const WELCOME_HEADING_HEIGHT: f32 = 34.0 + 10.0 + 42.0 + 8.0 + 18.0 + 22.0 + 20.0;

/// Top padding that centres the welcome block in the available height.
///
/// The composer is measured one pass late (see the height-tracking above), so a
/// zero/unmeasured height falls back to a conservative fixed block instead of
/// jumping to the top edge on the first frame.
#[must_use]
fn welcome_top_space(available_height: f32, composer_height: f32) -> f32 {
    let composer = if composer_height > 0.0 {
        composer_height
    } else {
        120.0
    };
    let block = WELCOME_HEADING_HEIGHT + composer;
    ((available_height - block) / 2.0).clamp(16.0, available_height * 0.34)
}

/// Build a minimal view model for GUI unit tests without invoking probes.
#[cfg(test)]
fn test_view_model() -> ViewModel {
    use crate::view_model::{AgentView, Health};
    use vibemux_probe::{ProbeState, RouteKind};
    ViewModel {
        title: "VibeMux".to_string(),
        platform: "test".to_string(),
        schema_version: 1,
        observed_at_epoch_seconds: 0,
        observed_at: "1970-01-01T00:00:00Z".to_string(),
        health: Health::Warning,
        overall_status: "no probe".to_string(),
        agents: AgentKind::all()
            .into_iter()
            .map(|agent| AgentView {
                name: agent.display_name().to_string(),
                launcher_state: ProbeState::Unavailable,
                authentication_state: ProbeState::NotRun,
                inference_state: ProbeState::NotRun,
                version: "-".to_string(),
                route: RouteKind::Unknown,
                code: "not_probed".to_string(),
                adapter: vibemux_harness::dispatch::adapter_capability(agent),
            })
            .collect(),
        gateway_summary: "unavailable".to_string(),
        gateway_state: vibemux_probe::ProbeState::Unavailable,
        telemetry_summary: "unavailable".to_string(),
        telemetry_failures: 0,
        telemetry_available: false,
        a2a_summary: "unavailable".to_string(),
        a2a_state: vibemux_probe::ProbeState::Unavailable,
        env_allowlist: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::ask_view::AskAvailability;
    use super::super::console::{ProbeJob, WorkspaceLoad};
    use super::*;
    use crate::{
        ask::{AskEnd, AskPhase, AskUpdate},
        dashboard::{
            DaemonView, Section, Workspace,
            daemon_model::{DecisionRow, HealthView, RouteRow, TaskList, TaskRow, unavailable},
        },
        supervisor_model::{RunView, TaskEventView},
        theme::serialize::{SCHEMA_VERSION, WindowSize},
    };

    fn fixture_snapshot() -> SupervisorSnapshot {
        SupervisorSnapshot {
            project_id: "project_1".to_string(),
            project_name: "VibeMux".to_string(),
            coordinator: Some("Coordinator".to_string()),
            connection: crate::supervisor_model::ConnectionState {
                status: ConnectionStatus::Connected,
                detail: None,
                code: None,
            },
            tasks: vec![
                TaskView {
                    task_id: "task_1".to_string(),
                    title: "First task".to_string(),
                    state: "running".to_string(),
                    executor: "Claude".to_string(),
                    latest_update: "Started".to_string(),
                    latest_sequence: 1,
                    runs: vec![RunView {
                        run_id: "run_1".to_string(),
                        harness: "Claude".to_string(),
                        role: "implementer".to_string(),
                        state: "running".to_string(),
                        ..RunView::default()
                    }],
                    recent_events: vec![TaskEventView {
                        event_id: "event_1".to_string(),
                        sequence: 1,
                        kind: "run_started".to_string(),
                        occurred_at: "now".to_string(),
                        run_id: Some("run_1".to_string()),
                        summary: "Started".to_string(),
                    }],
                    ..TaskView::default()
                },
                TaskView {
                    task_id: "task_2".to_string(),
                    title: "Second task".to_string(),
                    state: "completed".to_string(),
                    executor: "OpenCode".to_string(),
                    latest_update: "Finished".to_string(),
                    ..TaskView::default()
                },
            ],
            next_cursor: None,
            observed_at: Some("2026-09-27T00:00:00Z".to_string()),
            mode: SnapshotMode::Live,
        }
    }

    fn config() -> UserConfig {
        UserConfig {
            schema_version: SCHEMA_VERSION,
            theme: ThemeId::Github,
            window_size: WindowSize {
                width: 1280,
                height: 800,
            },
            ..UserConfig::default()
        }
    }

    #[test]
    fn fresh_app_starts_in_coordinator_chat_with_unavailable_runtime() {
        let snapshot = SupervisorSnapshot::unavailable();
        assert_eq!(snapshot.connection.status, ConnectionStatus::Unavailable);
        let view_model = test_view_model();
        assert_eq!(view_model.agents.len(), AgentKind::all().len());
        assert_eq!(config().theme, ThemeId::Github);
        let snapshot = fixture_snapshot();
        let mut state = SupervisorUiState::default();
        assert!(state.open_task_window(&snapshot, "task_1"));
        state.reconcile_snapshot(&SupervisorSnapshot::unavailable());
        assert!(
            state.task_window_is_open("task_1"),
            "a missing snapshot must not close a task window"
        );
    }

    #[test]
    fn freshness_label_shows_only_a_clock_time_or_nothing() {
        // No stamp, or an unparseable one, yields no label rather than a guess.
        assert_eq!(freshness_label(None), None);
        assert_eq!(freshness_label(Some("not a timestamp")), None);
        let label = freshness_label(Some("2026-09-27T10:24:00Z")).expect("label");
        // Exactly "Updated HH:MM:SS" and nothing else. The snapshot carries no
        // date contract, so a date or offset must never appear here — this also
        // catches a formatter whose output happens to contain two colons.
        let clock = label.strip_prefix("Updated ").expect("prefixed");
        assert!(!clock.contains(' '), "leaked a date or offset: {label}");
        let parts: Vec<&str> = clock.split(':').collect();
        assert_eq!(parts.len(), 3, "{label}");
        assert!(
            parts
                .iter()
                .all(|part| part.len() == 2 && part.bytes().all(|b| b.is_ascii_digit())),
            "{label} should be zero-padded HH:MM:SS"
        );
    }

    #[test]
    fn welcome_block_centers_against_the_measured_composer() {
        // A tall window leaves matching space above and below the block.
        let top = welcome_top_space(715.0, 120.0);
        let block = WELCOME_HEADING_HEIGHT + 120.0;
        assert!(
            (top - (715.0 - block) / 2.0).abs() < 0.01,
            "top padding {top} should centre a {block}px block"
        );
        // An unmeasured composer still yields a sane, non-negative offset
        // rather than pinning the block to the top edge.
        let unmeasured = welcome_top_space(715.0, 0.0);
        assert!(unmeasured > 0.0 && unmeasured < 715.0, "{unmeasured}");
        // A short window clamps instead of pushing the block off-screen.
        let short = welcome_top_space(120.0, 120.0);
        assert!(short >= 16.0, "{short}");
        assert!(short <= 120.0, "{short}");
    }

    #[test]
    fn debounce_retains_pending_write_until_elapsed() {
        let now = Instant::now();
        let pending = Some(now - Duration::from_millis(100));
        let (still_pending, _, should_save) = consume_due_persistence(pending, None, now);
        assert_eq!(still_pending, pending);
        assert!(!should_save);

        let due = Some(now - WRITE_DEBOUNCE - Duration::from_millis(1));
        let (cleared, _, should_save) = consume_due_persistence(due, None, now);
        assert!(cleared.is_none());
        assert!(should_save);
    }

    fn test_app() -> SupervisorApp {
        SupervisorApp {
            view_model: test_view_model(),
            snapshot: Arc::new(RwLock::new(Arc::new(fixture_snapshot()))),
            user_config: config(),
            ui_state: Arc::new(Mutex::new(SupervisorUiState::default())),
            actions: UiActionQueue::default(),
            settings_open: false,
            settings_section: settings::SettingsSection::default(),
            applied_text_size: None,
            project_folder: None,
            project_switch: None,
            folder_dialog: folders::FolderDialogState::default(),
            daemon_request: None,
            daemon_job: DaemonJob::Idle,
            ask: AskState::default(),
            console: ConsoleData::default(),
            diagnostics_open: false,
            shortcuts_open: false,
            quick_switcher: QuickSwitcherState::default(),
            last_size: [0.0, 0.0],
            pending_write: None,
            pending_resize: None,
            composer_panel_height: 0.0,
        }
    }

    /// Mutate the shared snapshot in place for a test, copying on write only
    /// when another holder still references it.
    fn edit_snapshot(app: &mut SupervisorApp, body: impl FnOnce(&mut SupervisorSnapshot)) {
        let mut guard = app.snapshot.write().expect("snapshot lock");
        let snapshot = Arc::make_mut(&mut guard);
        body(snapshot);
    }

    fn key_event(
        context: &egui::Context,
        app: &mut SupervisorApp,
        key: Key,
        ctrl: bool,
        ime: bool,
    ) {
        let modifiers = egui::Modifiers {
            ctrl,
            ..Default::default()
        };
        let mut events = vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }];
        if ime {
            events.push(egui::Event::Ime(egui::ImeEvent::Preedit("输入".into())));
        }
        let _ = context.run_ui(
            egui::RawInput {
                events,
                modifiers,
                ..Default::default()
            },
            |context| app.handle_keyboard(context),
        );
    }

    #[test]
    fn all_ten_harness_shortcuts_and_escape_precedence_work() {
        let mut app = test_app();
        let context = egui::Context::default();
        for (index, key) in [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
            Key::Num0,
        ]
        .into_iter()
        .enumerate()
        {
            key_event(&context, &mut app, key, true, false);
            assert_eq!(app.ui_state.lock().unwrap().selected_harness(), Some(index));
        }
        app.settings_open = true;
        key_event(&context, &mut app, Key::Escape, false, false);
        assert!(!app.settings_open);
        assert_eq!(app.ui_state.lock().unwrap().page(), MainPage::Console);
        key_event(
            &egui::Context::default(),
            &mut app,
            Key::Escape,
            false,
            false,
        );
        assert_eq!(
            app.ui_state.lock().unwrap().page(),
            MainPage::CoordinatorChat
        );
    }

    #[test]
    fn ctrl_k_opens_the_switcher() {
        let mut app = test_app();
        key_event(&egui::Context::default(), &mut app, Key::K, true, false);
        assert!(app.quick_switcher.open);
        // Esc closes the switcher before anything else.
        app.settings_open = true;
        key_event(
            &egui::Context::default(),
            &mut app,
            Key::Escape,
            false,
            false,
        );
        assert!(!app.quick_switcher.open);
        assert!(app.settings_open);
    }

    #[test]
    fn ctrl_b_toggles_the_sidebar() {
        let mut app = test_app();
        let collapsed = app.user_config.sidebar_collapsed;
        key_event(&egui::Context::default(), &mut app, Key::B, true, false);
        assert_eq!(app.user_config.sidebar_collapsed, !collapsed);
        key_event(&egui::Context::default(), &mut app, Key::B, true, false);
        assert_eq!(app.user_config.sidebar_collapsed, collapsed);
    }

    #[test]
    fn ctrl_slash_opens_the_overlay_and_ctrl_comma_settings() {
        let mut app = test_app();
        key_event(&egui::Context::default(), &mut app, Key::Slash, true, false);
        assert!(app.shortcuts_open);
        key_event(
            &egui::Context::default(),
            &mut app,
            Key::Escape,
            false,
            false,
        );
        assert!(!app.shortcuts_open);
        key_event(&egui::Context::default(), &mut app, Key::Comma, true, false);
        assert!(app.settings_open);
        key_event(&egui::Context::default(), &mut app, Key::N, true, false);
        assert!(app.ui_state.lock().unwrap().welcome_active());
    }

    #[test]
    fn global_shortcuts_are_ignored_during_ime_composition() {
        let mut app = test_app();
        for key in [Key::K, Key::B, Key::Slash, Key::Comma, Key::N] {
            key_event(&egui::Context::default(), &mut app, key, true, true);
        }
        assert!(!app.quick_switcher.open);
        assert!(!app.shortcuts_open);
        assert!(!app.settings_open);
        assert!(!app.ui_state.lock().unwrap().welcome_active());
    }

    #[test]
    fn switcher_targets_open_tasks_and_run_actions_but_never_send() {
        let mut app = test_app();
        app.apply_search_target(SearchTarget::Task("task_1".to_string()));
        assert_eq!(
            app.ui_state.lock().unwrap().selected_task_id(),
            Some("task_1")
        );
        assert!(app.ui_state.lock().unwrap().details_open());
        app.apply_search_target(SearchTarget::Action(QuickAction::Diagnostics));
        assert!(app.diagnostics_open);
        app.apply_search_target(SearchTarget::Action(QuickAction::Light));
        assert_eq!(app.user_config.theme, ThemeId::ClaudeLight);
        app.apply_search_target(SearchTarget::Action(QuickAction::Dark));
        assert_eq!(app.user_config.theme, ThemeId::Claude);
        app.apply_search_target(SearchTarget::Agent(2));
        assert_eq!(app.ui_state.lock().unwrap().selected_harness(), Some(2));
        assert!(!app.ui_state.lock().unwrap().send_enabled());
    }

    #[test]
    fn ime_preedit_does_not_trigger_harness_shortcuts() {
        let mut app = test_app();
        key_event(&egui::Context::default(), &mut app, Key::Num1, true, true);
        assert_eq!(
            app.ui_state.lock().unwrap().page(),
            MainPage::CoordinatorChat
        );
    }

    fn render_at(
        app: &mut SupervisorApp,
        width: f32,
        height: f32,
    ) -> (egui::Context, egui::FullOutput) {
        let context = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, height),
            )),
            ..Default::default()
        };
        let _ = context.run_ui(raw.clone(), |context| app.render_frame(context));
        let output = context.run_ui(raw, |context| app.render_frame(context));
        (context, output)
    }

    #[test]
    fn opening_a_folder_remembers_it_and_asks_the_live_adapter_to_switch() {
        let directory = tempfile::tempdir().expect("folder");
        let mut app = test_app();
        app.open_project(directory.path().to_path_buf());
        assert_eq!(app.active_project(), Some(directory.path()));
        assert_eq!(app.take_project_switch().as_deref(), Some(directory.path()));
        assert_eq!(app.take_project_switch(), None, "one switch per choice");
        assert_eq!(
            app.user_config.active_project.as_deref(),
            Some(directory.path())
        );
        assert_eq!(
            app.user_config
                .recent_projects
                .first()
                .map(PathBuf::as_path),
            Some(directory.path())
        );
        assert!(app.pending_write.is_some(), "the choice is saved");
        // Reopening the active folder reconnects nothing.
        app.open_project(directory.path().to_path_buf());
        assert_eq!(app.take_project_switch(), None);
    }

    #[test]
    fn a_missing_folder_reopens_the_dialog_with_its_error() {
        let directory = tempfile::tempdir().expect("folder");
        let mut app = test_app();
        app.open_project(directory.path().join("missing"));
        assert!(app.folder_dialog.open);
        assert_eq!(
            app.folder_dialog.error,
            Some(crate::project_folder::ProjectFolderError::NotADirectory)
        );
        assert_eq!(app.active_project(), None);
        assert_eq!(app.take_project_switch(), None);
    }

    #[test]
    fn daemon_requests_need_a_folder_and_never_overlap() {
        let mut app = test_app();
        app.request_daemon(DaemonCommand::Start);
        assert_eq!(app.take_daemon_request(), None);
        assert!(matches!(app.daemon_job, DaemonJob::Failed(_)));

        let directory = tempfile::tempdir().expect("folder");
        app.open_project(directory.path().to_path_buf());
        app.request_daemon(DaemonCommand::Start);
        assert_eq!(app.daemon_job, DaemonJob::Running(DaemonCommand::Start));
        app.request_daemon(DaemonCommand::Stop);
        assert_eq!(app.take_daemon_request(), Some(DaemonCommand::Start));
        assert_eq!(app.take_daemon_request(), None, "the second click waited");
        app.set_daemon_job(DaemonJob::Idle);
        app.request_daemon(DaemonCommand::Stop);
        assert_eq!(app.take_daemon_request(), Some(DaemonCommand::Stop));
    }

    #[test]
    fn the_daemon_card_and_folder_dialog_render_and_escape_closes_the_dialog() {
        let directory = tempfile::tempdir().expect("folder");
        let mut app = test_app();
        app.open_project(directory.path().to_path_buf());
        let _ = app.take_project_switch();
        edit_snapshot(&mut app, |snapshot| {
            snapshot.connection.status = ConnectionStatus::Disconnected;
            snapshot.connection.code = Some(daemon_card::DAEMON_NOT_RUNNING.into());
        });
        app.folder_dialog.show();
        for (width, height) in [(1280.0, 800.0), (960.0, 600.0)] {
            let _ = render_at(&mut app, width, height);
            assert!(
                app.folder_dialog.open,
                "rendering alone keeps the dialog open"
            );
        }
        app.settings_open = true;
        key_event(
            &egui::Context::default(),
            &mut app,
            Key::Escape,
            false,
            false,
        );
        assert!(!app.folder_dialog.open, "Esc closes the dialog first");
        assert!(app.settings_open);
    }

    /// An app with `folder` open whose daemon can run Codex Asks.
    fn ask_ready_app(folder: &Path) -> SupervisorApp {
        let mut app = test_app();
        app.open_project(folder.to_path_buf());
        let _ = app.take_project_switch();
        // Nothing in these tests may reach the real config file.
        app.pending_write = None;
        app.ask
            .set_availability(AskAvailability::Ready(vec![AgentKind::Codex]));
        app
    }

    /// Types `prompt` for Codex and presses Send, as the composer does.
    fn send_to_codex(app: &SupervisorApp, prompt: &str) -> bool {
        app.sync_ask_gate();
        let mut state = app.ui_state.lock().expect("ui state");
        state.set_composer_target(ComposerTarget::Harness("Codex".into()));
        state.set_composer_draft(prompt.into());
        state.try_send()
    }

    fn finished(reply: &str) -> AskUpdate {
        AskUpdate {
            request_id: uuid::Uuid::nil(),
            phase: AskPhase::Finished,
            reply: reply.into(),
            truncated: false,
            end: Some(AskEnd::Completed),
        }
    }

    fn shape_texts(output: &egui::FullOutput) -> Vec<String> {
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.job.text.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn send_stays_off_until_the_folder_can_run_the_harness() {
        let directory = tempfile::tempdir().expect("folder");
        let mut app = ask_ready_app(directory.path());
        app.ask.set_availability(AskAvailability::NotConfigured);
        assert!(!send_to_codex(&app, "Explain"));
        app.ask
            .set_availability(AskAvailability::Ready(vec![AgentKind::Claude]));
        assert!(!send_to_codex(&app, "Explain"), "only Claude has a route");
        app.ask
            .set_availability(AskAvailability::Ready(vec![AgentKind::Codex]));
        {
            let mut state = app.ui_state.lock().expect("ui state");
            state.set_composer_target(ComposerTarget::Coordinator);
            assert!(!state.try_send(), "the coordinator chat is not connected");
            assert_eq!(
                state.composer_draft(),
                "Explain",
                "a refused send keeps the draft"
            );
        }
        assert!(send_to_codex(&app, "Explain"));
        app.route_pending_ask();
        app.apply_ask_confirm(ConfirmChoice::Send);
        app.pending_write = None;
        assert!(!send_to_codex(&app, "Again"), "one Ask at a time");
    }

    #[test]
    fn a_harness_ask_needs_consent_once_per_folder() {
        let directory = tempfile::tempdir().expect("folder");
        let mut app = ask_ready_app(directory.path());
        assert!(send_to_codex(&app, "Explain the build"));
        app.route_pending_ask();
        assert!(app.ask.confirm.open, "the first Ask in a folder asks first");
        assert_eq!(app.ask.take_launch(), None);
        let (_, output) = render_at(&mut app, 1280.0, 800.0);
        assert!(
            app.ask.confirm.open,
            "rendering alone keeps the dialog open"
        );
        assert!(shape_texts(&output).iter().any(|text| text == "Ask Codex?"));
        app.apply_ask_confirm(ConfirmChoice::Send);
        assert!(app.user_config.ask_confirmed(directory.path()));
        assert!(app.pending_write.take().is_some(), "consent is saved");
        assert_eq!(
            app.ask.take_launch(),
            Some((AgentKind::Codex, "Explain the build".to_string()))
        );
        app.ask.apply_update(finished("It builds with cargo."));
        assert!(send_to_codex(&app, "And the tests?"));
        app.route_pending_ask();
        assert!(
            !app.ask.confirm.open,
            "consent is remembered for the folder"
        );
        assert!(app.ask.take_launch().is_some());
    }

    #[test]
    fn cancelling_the_consent_puts_the_prompt_back() {
        let directory = tempfile::tempdir().expect("folder");
        let mut app = ask_ready_app(directory.path());
        assert!(send_to_codex(&app, "Explain"));
        app.route_pending_ask();
        app.apply_ask_confirm(ConfirmChoice::Cancel);
        assert!(!app.ask.confirm.open);
        assert!(!app.user_config.ask_confirmed(directory.path()));
        assert_eq!(app.ask.take_launch(), None);
        assert!(app.ask.asks.is_empty());
        assert_eq!(
            app.ui_state.lock().expect("ui state").composer_draft(),
            "Explain"
        );
    }

    #[test]
    fn asks_and_the_setup_card_render_and_reset_with_the_folder() {
        let directory = tempfile::tempdir().expect("folder");
        let mut app = ask_ready_app(directory.path());
        app.ask.set_availability(AskAvailability::NotConfigured);
        app.ui_state
            .lock()
            .expect("ui state")
            .set_composer_target(ComposerTarget::Harness("Codex".into()));
        let (_, output) = render_at(&mut app, 1280.0, 800.0);
        assert!(
            shape_texts(&output)
                .iter()
                .any(|text| text == "Enable Claude and Codex (read-only)")
        );

        app.ask
            .set_availability(AskAvailability::Ready(vec![AgentKind::Codex]));
        app.user_config.confirm_ask(directory.path().to_path_buf());
        assert!(send_to_codex(&app, "Summarize the README"));
        app.route_pending_ask();
        let _ = app.ask.take_launch();
        app.ask.apply_update(finished("A local-first harness hub."));
        // Below the workspace's tasks, the conversation follows the newest
        // Ask once its scroll area has settled at the bottom.
        let context = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            ..Default::default()
        };
        let mut output = None;
        for _ in 0..4 {
            output = Some(context.run_ui(raw.clone(), |context| app.render_frame(context)));
        }
        let texts = shape_texts(&output.expect("rendered"));
        assert!(texts.iter().any(|text| text == "Summarize the README"));
        assert!(
            texts
                .iter()
                .any(|text| text == "A local-first harness hub.")
        );
        // An empty workspace shows the Asks alone.
        edit_snapshot(&mut app, |snapshot| snapshot.tasks.clear());
        let (_, output) = render_at(&mut app, 900.0, 600.0);
        let texts = shape_texts(&output);
        assert!(
            texts
                .iter()
                .any(|text| text == "A local-first harness hub.")
        );
        assert!(
            !texts
                .iter()
                .any(|text| text.ends_with("tasks in this workspace"))
        );

        let other = tempfile::tempdir().expect("other folder");
        app.open_project(other.path().to_path_buf());
        app.pending_write = None;
        assert!(
            app.ask.asks.is_empty(),
            "Asks belong to the old folder's daemon"
        );
        assert_eq!(app.ask.availability, AskAvailability::Unknown);
    }

    fn console_daemon_view() -> DaemonView {
        let mut daemon = DaemonView::waiting();
        daemon.connection = "connected".into();
        daemon.observed_at = Some("12:00:01".into());
        daemon.health = Section::Loaded(HealthView {
            status: "healthy".into(),
            schema: "9".into(),
            queue: "0 of 64".into(),
        });
        daemon.routes = Section::Loaded(vec![RouteRow {
            harness: "claude".into(),
            protocol: "claude_stream_json".into(),
            mode: "executes".into(),
        }]);
        daemon.plugins = Section::Loaded(Vec::new());
        daemon.tasks = Section::Loaded(TaskList {
            rows: vec![TaskRow {
                task_id: "task_1".into(),
                title: "Console task row".into(),
                status: "running".into(),
            }],
            more: false,
        });
        daemon.dsh = Section::Unavailable(unavailable("dsh_session_unconfigured"));
        daemon.slots = Section::Unavailable(unavailable("workflow_unconfigured"));
        daemon.decisions = Section::Loaded(vec![DecisionRow::new(
            "0123456789abcdef",
            "recorded",
            Some("claude"),
            "local_rules",
            "2026-10-23",
            false,
            1,
        )]);
        daemon
    }

    #[test]
    fn every_console_section_renders_its_data() {
        let directory = tempfile::tempdir().expect("folder");
        let mut app = test_app();
        app.open_project(directory.path().to_path_buf());
        let _ = app.take_project_switch();
        app.pending_write = None;
        app.console.daemon = Some(Arc::new(console_daemon_view()));
        app.console.set_workspace(Ok(Workspace {
            current_branch: "feat/console_branch".into(),
            head: "b68f381f".into(),
            baseline: "origin/main".into(),
            ..Workspace::default()
        }));
        let expected = [
            (ConsoleSection::Overview, "Detected"),
            (ConsoleSection::Harnesses, "Claude"),
            (ConsoleSection::Workspace, "feat/console_branch"),
            (ConsoleSection::Tasks, "Console task row"),
            (ConsoleSection::Daemon, "claude_stream_json · executes"),
            (
                ConsoleSection::Sessions,
                "Unavailable: not configured (dsh_session_unconfigured).",
            ),
            (ConsoleSection::Decisions, "01234567 · recorded"),
        ];
        for (section, text) in expected {
            app.ui_state
                .lock()
                .expect("ui state")
                .show_console_section(section);
            assert!(app.console_open());
            for (width, height) in [(1280.0, 800.0), (900.0, 600.0)] {
                let (_, output) = render_at(&mut app, width, height);
                assert!(
                    shape_texts(&output).iter().any(|shown| shown == text),
                    "{section:?} at {width}x{height} shows no {text:?}"
                );
            }
        }
        let other = tempfile::tempdir().expect("other folder");
        app.open_project(other.path().to_path_buf());
        app.pending_write = None;
        assert!(
            app.console.daemon.is_none(),
            "Console reads belong to the folder"
        );
        assert!(matches!(app.console.workspace, WorkspaceLoad::NotLoaded));
    }

    #[test]
    fn console_actions_navigate_and_ask_only_for_reads() {
        let mut app = test_app();
        app.apply_console_actions(ConsoleActions {
            harness: Some(Some(1)),
            ..ConsoleActions::default()
        });
        {
            let state = app.ui_state.lock().expect("ui state");
            assert_eq!(state.page(), MainPage::Console);
            assert_eq!(state.console_section(), ConsoleSection::Harnesses);
            assert_eq!(state.selected_harness(), Some(1));
        }
        app.apply_console_actions(ConsoleActions {
            harness: Some(None),
            ..ConsoleActions::default()
        });
        assert_eq!(
            app.ui_state.lock().expect("ui state").selected_harness(),
            None
        );
        app.apply_console_actions(ConsoleActions {
            ask: Some("Codex".into()),
            ..ConsoleActions::default()
        });
        {
            let state = app.ui_state.lock().expect("ui state");
            assert_eq!(state.page(), MainPage::CoordinatorChat);
            assert_eq!(
                state.composer_target(),
                &ComposerTarget::Harness("Codex".into())
            );
            assert!(state.composer_draft().is_empty(), "the jump never sends");
        }
        assert!(app.ask.asks.is_empty());
        app.apply_console_actions(ConsoleActions {
            refresh: true,
            reprobe: true,
            open_task: Some("task_1".into()),
            ..ConsoleActions::default()
        });
        assert!(app.console.take_refresh());
        assert!(app.console.take_reprobe());
        assert_eq!(app.console.take_task().as_deref(), Some("task_1"));
        assert_eq!(app.console.probe, ProbeJob::Running);
        app.apply_console_actions(ConsoleActions {
            task_window: Some("task_1".into()),
            ..ConsoleActions::default()
        });
        assert!(
            app.ui_state
                .lock()
                .expect("ui state")
                .task_window_is_open("task_1")
        );
    }

    #[test]
    fn settings_changes_apply_and_back_closes_the_page() {
        let mut app = test_app();
        app.open_settings(settings::SettingsSection::Appearance);
        assert!(app.settings_open);
        assert_eq!(app.settings_section, settings::SettingsSection::Appearance);
        app.apply_settings_changes(settings::SettingsChanges {
            section: Some(settings::SettingsSection::About),
            theme: Some(ThemeId::Nord),
            start_page: Some(StartPage::Console),
            density: Some(Density::Compact),
            text_size: Some(TextSize::Large),
            ..Default::default()
        });
        assert_eq!(app.settings_section, settings::SettingsSection::About);
        assert_eq!(app.user_config.theme, ThemeId::Nord);
        assert_eq!(app.user_config.start_page, StartPage::Console);
        assert_eq!(app.user_config.density, Density::Compact);
        assert_eq!(app.user_config.text_size, TextSize::Large);
        assert!(app.pending_write.is_some(), "changed preferences are saved");
        assert!(app.settings_open);
        app.apply_settings_changes(settings::SettingsChanges {
            close: true,
            ..Default::default()
        });
        assert!(!app.settings_open);
    }

    #[test]
    fn the_saved_start_page_opens_first() {
        let agents = start_state(SupervisorUiState::default(), StartPage::Console);
        assert_eq!(agents.page(), MainPage::Console);
        let conversation = start_state(SupervisorUiState::default(), StartPage::Conversation);
        assert_eq!(conversation.page(), MainPage::CoordinatorChat);
    }

    #[test]
    fn every_settings_section_renders_at_the_supported_sizes() {
        for section in settings::SettingsSection::ALL {
            for (width, height) in [(1280.0, 800.0), (960.0, 600.0)] {
                let mut app = test_app();
                app.open_settings(section);
                let (_, output) = render_at(&mut app, width, height);
                assert!(app.settings_open, "rendering alone keeps the page open");
                assert_eq!(app.settings_section, section);
                if section == settings::SettingsSection::General {
                    // Regression: at 960 px the content column grew past the
                    // viewport and clipped every right-aligned control.
                    let texts = shape_texts(&output);
                    assert!(texts.iter().any(|text| text == "Change…"), "{width}");
                    assert!(texts.iter().any(|text| text == "Console"), "{width}");
                }
            }
        }
    }

    #[test]
    fn switcher_and_shortcut_overlay_render_and_focus_the_query() {
        let mut app = test_app();
        app.quick_switcher.open();
        app.shortcuts_open = true;
        let (context, _) = render_at(&mut app, 1280.0, 800.0);
        assert!(app.quick_switcher.open, "rendering alone picks nothing");
        assert!(app.shortcuts_open);
        assert!(
            context.memory(|memory| memory.has_focus(egui::Id::new("quick_switcher_query"))),
            "the query field takes the keyboard"
        );
        // The smallest supported window still draws both.
        let _ = render_at(&mut app, 960.0, 600.0);
        assert!(!app.ui_state.lock().unwrap().send_enabled());
    }

    #[test]
    fn sidebar_width_follows_the_collapsed_setting() {
        for (collapsed, expected) in [
            (false, sidebar::SIDEBAR_WIDTH),
            (true, sidebar::SIDEBAR_RAIL_WIDTH),
        ] {
            let mut app = test_app();
            app.user_config.sidebar_collapsed = collapsed;
            let (context, _) = render_at(&mut app, 1280.0, 800.0);
            let panel = egui::containers::panel::PanelState::load(
                &context,
                egui::Id::new(sidebar::SIDEBAR_PANEL_ID),
            )
            .expect("sidebar panel");
            assert!((panel.rect.width() - expected).abs() < 1.0, "{collapsed}");
        }
    }

    /// A size from the settings dialog or a manual resize is saved inside the
    /// range `load_user_config` clamps to, and only a real change is saved.
    #[test]
    fn a_saved_window_size_stays_in_the_settings_range() {
        let mut app = test_app();
        app.set_window_size(WindowSize {
            width: 100_000,
            height: 1,
        });
        assert_eq!(
            app.user_config.window_size,
            WindowSize {
                width: 3840,
                height: 600
            }
        );
        assert!(app.pending_resize.is_some());
        app.pending_resize = None;
        app.set_window_size(WindowSize {
            width: 3841,
            height: 599,
        });
        assert!(app.pending_resize.is_none(), "same clamped size, no write");
    }

    #[test]
    fn recents_truncate_long_cjk_titles_inside_the_sidebar() {
        let long_title = "很长的任务标题需要截断".repeat(8);
        let mut app = test_app();
        edit_snapshot(&mut app, |snapshot| {
            snapshot.tasks[0].title = long_title.clone()
        });
        let (_, output) = render_at(&mut app, 1280.0, 800.0);
        let sidebar_titles: Vec<egui::Rect> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text)
                    if text.galley.job.text == long_title
                        && text.pos.x < sidebar::SIDEBAR_WIDTH =>
                {
                    Some(text.visual_bounding_rect())
                }
                _ => None,
            })
            .collect();
        assert_eq!(sidebar_titles.len(), 1);
        assert!(sidebar_titles[0].right() <= sidebar::SIDEBAR_WIDTH + 0.5);
    }

    fn composer_key(
        context: &egui::Context,
        app: &mut SupervisorApp,
        modifiers: egui::Modifiers,
        with_ime: bool,
        time: f64,
    ) {
        let mut events = Vec::new();
        if with_ime {
            events.push(egui::Event::Ime(egui::ImeEvent::Preedit("输入".into())));
        }
        events.push(egui::Event::Key {
            key: Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        });
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            events,
            modifiers,
            time: Some(time),
            ..Default::default()
        };
        let _ = context.run_ui(raw, |context| app.render_frame(context));
    }

    fn focused_composer_app() -> (egui::Context, SupervisorApp) {
        let mut app = test_app();
        app.ui_state
            .lock()
            .unwrap()
            .set_composer_draft("draft".to_string());
        let context = egui::Context::default();
        let _ = context.run_ui(egui::RawInput::default(), |context| {
            app.render_frame(context)
        });
        context
            .memory_mut(|memory| memory.request_focus(egui::Id::new(composer::COMPOSER_TEXT_ID)));
        (context, app)
    }

    #[test]
    fn enter_keeps_the_draft_and_shows_not_sent() {
        let (context, mut app) = focused_composer_app();
        composer_key(&context, &mut app, egui::Modifiers::NONE, false, 1.0);
        let state = app.ui_state.lock().unwrap();
        assert_eq!(state.composer_draft(), "draft");
        assert!(state.not_sent_hint_visible(1.5));
        drop(state);
        assert!(
            app.take_supervisor_actions()
                .iter()
                .all(|action| matches!(action, SupervisorAction::Refresh))
        );
    }

    #[test]
    fn shift_enter_inserts_a_newline() {
        let (context, mut app) = focused_composer_app();
        composer_key(&context, &mut app, egui::Modifiers::SHIFT, false, 1.0);
        assert!(app.ui_state.lock().unwrap().composer_draft().contains('\n'));
    }

    #[test]
    fn enter_during_ime_composition_is_ignored() {
        let (context, mut app) = focused_composer_app();
        composer_key(&context, &mut app, egui::Modifiers::NONE, true, 1.0);
        let state = app.ui_state.lock().unwrap();
        // egui shows the uncommitted preedit text inline; Enter itself must
        // neither attempt a send nor insert a newline.
        assert!(state.composer_draft().starts_with("draft"));
        assert!(!state.composer_draft().contains('\n'));
        assert!(!state.not_sent_hint_visible(1.5));
        drop(state);
        assert!(
            app.take_supervisor_actions()
                .iter()
                .all(|action| matches!(action, SupervisorAction::Refresh))
        );
    }

    #[test]
    fn task_titles_use_the_serif_family() {
        let mut app = test_app();
        let context = egui::Context::default();
        context.set_fonts(super::super::typography::build_font_definitions(|_| None));
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            ..Default::default()
        };
        let _ = context.run_ui(raw.clone(), |context| app.render_frame(context));
        let output = context.run_ui(raw, |context| app.render_frame(context));
        let serif_title = output.shapes.iter().any(|shape| match &shape.shape {
            egui::epaint::Shape::Text(text) => {
                text.galley.job.text == "Second task"
                    && text.galley.job.sections.first().is_some_and(|section| {
                        section.format.font_id.family == super::super::typography::serif_family()
                    })
            }
            _ => false,
        });
        assert!(serif_title);
    }

    #[test]
    fn task_drawer_docks_below_the_header_without_duplicate_chrome() {
        for (width, height) in [(960.0, 600.0), (1280.0, 800.0), (1920.0, 1080.0)] {
            let mut app = test_app();
            assert!(app.select_task("task_1"));
            let (context, output) = render_at(&mut app, width, height);
            let drawer =
                egui::containers::panel::PanelState::load(&context, egui::Id::new(DRAWER_PANEL_ID))
                    .expect("docked drawer");
            let header = egui::containers::panel::PanelState::load(
                &context,
                egui::Id::new("supervisor_header"),
            )
            .expect("header");
            assert!((drawer.rect.right() - width).abs() < 1.0, "{width}");
            assert!(drawer.rect.top() >= header.rect.bottom() - 0.5, "{width}");
            let texts: Vec<String> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(text) => Some(text.galley.job.text.clone()),
                    _ => None,
                })
                .collect();
            assert!(
                !texts
                    .iter()
                    .any(|text| text == "Task details" || text == "Close")
            );
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, height));
            let send = context
                .read_response(egui::Id::new(composer::SEND_BUTTON_ID))
                .expect("send button");
            assert!(screen.contains_rect(send.rect), "{width}");
        }
    }

    #[test]
    fn composer_placeholder_uses_the_muted_text_color() {
        let mut app = test_app();
        app.user_config.theme = ThemeId::ClaudeLight;
        let muted = super::super::pal(&palette_for(ThemeId::ClaudeLight)).muted;
        let (_, output) = render_at(&mut app, 1280.0, 800.0);
        let placeholder = output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(text)
                if text.galley.job.text == composer::COMPOSER_PLACEHOLDER =>
            {
                text.galley
                    .job
                    .sections
                    .first()
                    .map(|section| section.format.color)
            }
            _ => None,
        });
        assert_eq!(placeholder, Some(muted));
    }

    #[test]
    fn long_draft_keeps_the_send_button_caption_and_conversation_visible() {
        let mut app = test_app();
        let long_draft = (0..60)
            .map(|line| format!("line {line} of a pasted specification"))
            .collect::<Vec<_>>()
            .join("\n");
        app.ui_state.lock().unwrap().set_composer_draft(long_draft);
        assert!(app.select_task("task_1"));
        let (width, height) = (960.0, 600.0);
        let (context, _) = render_at(&mut app, width, height);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, height));
        // The draft's scroll area sizes itself from the previous frame, so a
        // freshly pasted long draft settles within two more frames.
        let raw = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        let _ = context.run_ui(raw.clone(), |context| app.render_frame(context));
        let output = context.run_ui(raw, |context| app.render_frame(context));
        let send = context
            .read_response(egui::Id::new(composer::SEND_BUTTON_ID))
            .expect("send button");
        assert!(screen.contains_rect(send.rect), "send button off screen");
        let caption_visible = output.shapes.iter().any(|shape| match &shape.shape {
            egui::epaint::Shape::Text(text) => {
                text.galley.job.text.starts_with("Draft only") && screen.contains(text.pos)
            }
            _ => false,
        });
        assert!(caption_visible, "caption missing");
        let composer_panel = egui::containers::panel::PanelState::load(
            &context,
            egui::Id::new("coordinator_composer"),
        )
        .expect("composer panel");
        assert!(
            composer_panel.rect.height() <= height * 0.6,
            "composer panel took {} of {height}",
            composer_panel.rect.height()
        );
    }

    #[test]
    fn focused_icon_button_draws_an_accent_focus_ring() {
        let mut app = test_app();
        app.user_config.theme = ThemeId::ClaudeLight;
        let accent = super::super::pal(&palette_for(ThemeId::ClaudeLight)).accent;
        let context = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            ..Default::default()
        };
        let _ = context.run_ui(raw.clone(), |context| app.render_frame(context));
        context.memory_mut(|memory| memory.request_focus(egui::Id::new(REFRESH_BUTTON_ID)));
        let output = context.run_ui(raw, |context| app.render_frame(context));
        let refresh = context
            .read_response(egui::Id::new(REFRESH_BUTTON_ID))
            .expect("refresh button");
        let ring = output.shapes.iter().any(|shape| match &shape.shape {
            egui::epaint::Shape::Rect(rect) => {
                rect.stroke.color == accent
                    && rect.stroke.width >= 2.0
                    && rect.rect.intersects(refresh.rect)
            }
            _ => false,
        });
        assert!(ring, "no accent focus ring on the focused Refresh button");
    }

    #[test]
    fn empty_workspace_renders_the_welcome_composer() {
        let mut app = test_app();
        edit_snapshot(&mut app, |snapshot| snapshot.tasks.clear());
        let context = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            ..Default::default()
        };
        let _ = context.run_ui(raw.clone(), |context| app.render_frame(context));
        let output = context.run_ui(raw, |context| app.render_frame(context));
        let texts: Vec<String> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.job.text.clone()),
                _ => None,
            })
            .collect();
        assert!(
            texts
                .iter()
                .any(|text| text.starts_with("Good ") || text == "Hello")
        );
    }

    #[test]
    fn shell_renders_all_themes_sizes_and_scales_without_dropping_the_composer() {
        for theme in ThemeId::ALL {
            for (width, height) in [(960.0, 600.0), (1280.0, 800.0), (1920.0, 1080.0)] {
                for scale in [1.0, 1.5, 2.0] {
                    let mut app = test_app();
                    app.user_config.theme = theme;
                    edit_snapshot(&mut app, |snapshot| snapshot.mode = SnapshotMode::Demo);
                    let context = egui::Context::default();
                    let rect =
                        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, height));
                    let mut raw = egui::RawInput {
                        screen_rect: Some(rect),
                        ..Default::default()
                    };
                    raw.viewports
                        .entry(egui::ViewportId::ROOT)
                        .or_default()
                        .native_pixels_per_point = Some(scale);
                    let _ = context.run_ui(raw.clone(), |context| app.render_frame(context));
                    let _ = context.run_ui(raw, |context| app.render_frame(context));
                    let send = context
                        .read_response(egui::Id::new(composer::SEND_BUTTON_ID))
                        .expect("send button rendered");
                    assert!(
                        rect.contains_rect(send.rect),
                        "send button off screen: {theme:?} {width}x{height} scale {scale}"
                    );
                }
            }
        }
    }
}
