#![forbid(unsafe_code)]
//! Native Supervisor Chat application shell and immutable snapshot bridge.

use std::{
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};

use eframe::egui::{self, Key, Modifiers, RichText, ViewportBuilder};
#[cfg(test)]
use vibemux_probe::AgentKind;

use crate::{
    supervisor_model::{
        ConnectionStatus, SnapshotMode, SupervisorAction, SupervisorSnapshot, TaskView,
    },
    theme::{
        ThemeId, palette_for,
        serialize::{UserConfig, WindowSize, save_user_config},
    },
    view_model::ViewModel,
};

use super::{
    C, agents, chat, composer, design, diagnostics, settings, sidebar,
    supervisor_state::{MainPage, SupervisorUiState, UiActionQueue},
    task_detail,
};

const WRITE_DEBOUNCE: Duration = Duration::from_millis(250);
pub(crate) const REFRESH_BUTTON_ID: &str = "topbar_refresh";
const SETTINGS_BUTTON_ID: &str = "topbar_settings";
const DIAGNOSTICS_BUTTON_ID: &str = "topbar_diagnostics";
const AGENTS_BUTTON_ID: &str = "topbar_agents";
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
    settings_open: bool,
    diagnostics_open: bool,
    last_size: [f32; 2],
    pending_write: Option<Instant>,
    pending_resize: Option<Instant>,
    /// Last laid-out height of the auto-sized composer panel.
    composer_panel_height: f32,
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
        Self {
            view_model,
            snapshot: Arc::new(RwLock::new(Arc::new(SupervisorSnapshot::unavailable()))),
            user_config,
            ui_state: Arc::new(Mutex::new(SupervisorUiState::default())),
            actions,
            settings_open: false,
            diagnostics_open: false,
            last_size: [0.0, 0.0],
            pending_write: None,
            pending_resize: None,
            composer_panel_height: 0.0,
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
            if let Some(task) = snapshot.tasks.iter().find(|task| task.task_id == task_id) {
                if let Ok(state) = self.ui_state.lock() {
                    state.with_task_window_selection_mut(task_id, task, |selection| {
                        selection.tab = tab
                    });
                }
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
        let width = size.width.clamp(800, 3840);
        let height = size.height.clamp(600, 2160);
        let size = WindowSize { width, height };
        if self.user_config.window_size != size {
            self.user_config.window_size = size;
            self.pending_resize = Some(Instant::now());
        }
    }

    fn handle_keyboard(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() || ime_composition_active(ctx) {
            return;
        }
        if ctx.input(|input| input.key_pressed(Key::Escape)) {
            if let Ok(mut state) = self.ui_state.lock() {
                if state.discard_prompt_open() {
                    state.keep_draft();
                    return;
                }
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
        if let Some((_, index)) = DIGITS.into_iter().find(|(key, _)| input.key_pressed(*key)) {
            if let Ok(mut state) = self.ui_state.lock() {
                state.select_agent(index, self.view_model.agents.len());
            }
        }
    }

    fn render_topbar(
        &self,
        ctx: &egui::Context,
        colors: &C,
        snapshot: &SupervisorSnapshot,
    ) -> Option<HeaderAction> {
        let mut requested = None;
        egui::TopBottomPanel::top("supervisor_header")
            .show_separator_line(true)
            .min_height(TOPBAR_MIN_HEIGHT)
            .frame(
                egui::Frame::new()
                    .fill(colors.bg)
                    .inner_margin(egui::Margin::symmetric(20, 12)),
            )
            .show(ctx, |ui| {
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
                        // Refresh · Agents · Diagnostics · Settings on screen.
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
                                "Agents",
                                AGENTS_BUTTON_ID,
                                HeaderAction::Agents,
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

    fn render_drawer(&mut self, ctx: &egui::Context, colors: &C, viewport_width: f32) {
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
            egui::SidePanel::right(DRAWER_PANEL_ID)
                .resizable(true)
                .default_width(DRAWER_DEFAULT_WIDTH.min(max_width))
                .width_range(DRAWER_MIN_WIDTH..=max_width)
                .frame(
                    egui::Frame::new()
                        .fill(colors.bg)
                        .inner_margin(egui::Margin::symmetric(18, 14)),
                )
                .show(ctx, |ui| {
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
        if rendered {
            if let Ok(mut state) = self.ui_state.lock() {
                state.close_details();
            }
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
                snapshot
                    .tasks
                    .iter()
                    .find(|task| task.task_id == task_id)
                    .map_or_else(
                        || "Task observation".to_owned(),
                        |task| window_task_title(task).to_owned(),
                    )
            });
            let viewport_id = chat::task_viewport_id(&task_id);
            let title = format!("{title} · VibeMux");
            let viewport = ViewportBuilder::default()
                .with_title(title)
                .with_inner_size(TASK_WINDOW_SIZE)
                .with_min_inner_size(TASK_WINDOW_MIN_SIZE);
            let snapshot_arc = Arc::clone(&self.snapshot);
            let state_arc = Arc::clone(&self.ui_state);
            let actions = self.actions.clone();
            let root_ctx = ctx.clone();
            let task_colors = colors;
            ctx.show_viewport_deferred(viewport_id, viewport, move |child_ctx, _class| {
                let close_requested = child_ctx.input(|input| input.viewport().close_requested());
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
                    egui::CentralPanel::default().show(child_ctx,|ui| {
                        ui.heading("Task observation unavailable");
                        ui.label("This task is absent from the current page or connection. Its execution has not been changed.");
                        if ui.button("Refresh task").clicked() {
                            actions.enqueue(SupervisorAction::LoadTask {task_id:task_id.clone()});
                            root_ctx.request_repaint();
                        }
                    });
                    return;
                };
                super::apply_theme(child_ctx, &palette_for(theme));
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::new()
                            .fill(task_colors.bg)
                            .inner_margin(egui::Margin::same(18)),
                    )
                    .show(child_ctx, |ui| {
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
                width: (size[0] as u32).clamp(800, 3840),
                height: (size[1] as u32).clamp(600, 2160),
            });
        }
        self.last_size = size;
    }
}

impl eframe::App for SupervisorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.render_frame(ctx);
    }
}

impl SupervisorApp {
    fn render_frame(&mut self, ctx: &egui::Context) {
        if self.actions.take_overflowed() {
            if let Ok(mut snapshot) = self.snapshot.write() {
                Arc::make_mut(&mut snapshot).connection.detail = Some(
                    "Action queue is full. The last action was not sent; please try again.".into(),
                );
            }
        }
        let palette = palette_for(self.user_config.theme);
        super::apply_theme(ctx, &palette);
        ctx.style_mut(|style| {
            style
                .text_styles
                .insert(egui::TextStyle::Body, egui::FontId::proportional(15.0));
        });
        let colors = super::pal(&palette);
        let target_options = composer::target_options(&self.view_model);
        self.persist_if_due();
        self.handle_keyboard(ctx);

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
        let mut selected_agent_change = None;
        // A poisoned lock means no readable snapshot; drop the frame rather than
        // drawing from a fabricated one.
        let snapshot_arc = match self.snapshot.read() {
            Ok(guard) => Arc::clone(&guard),
            Err(_) => return,
        };
        let header_action;
        {
            // One shared borrow for the whole frame: the panels below read the
            // same snapshot instead of each cloning it.
            let snapshot = snapshot_arc.as_ref();
            egui::SidePanel::left(sidebar::SIDEBAR_PANEL_ID)
                .exact_width(sidebar::width_for(collapsed))
                .resizable(false)
                .frame(egui::Frame::new().fill(colors.surf))
                .show(ctx, |ui| {
                    let view = sidebar::SidebarView {
                        snapshot,
                        page: active_page,
                        welcome_active,
                        collapsed,
                        selected_task_id: selected_task_id.as_deref(),
                    };
                    sidebar_actions = Some(sidebar::render(ui, &colors, &view));
                });

            // Applied here, before the panels that read `page`, so a sidebar
            // navigation lands in the frame it was clicked rather than the next.
            if let Some(actions) = sidebar_actions.take() {
                if let Some(page) = actions.page {
                    if let Ok(mut state) = self.ui_state.lock() {
                        match page {
                            MainPage::CoordinatorChat => state.show_coordinator_chat(),
                            MainPage::Agents => state.show_agents(),
                        }
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
            }

            header_action = self.render_topbar(ctx, &colors, snapshot);
            if let Some(action) = header_action {
                match action {
                    HeaderAction::Settings => self.settings_open = true,
                    HeaderAction::Diagnostics => self.diagnostics_open = true,
                    HeaderAction::Agents => {
                        if let Ok(mut state) = self.ui_state.lock() {
                            state.show_agents();
                        }
                    }
                }
            }
            self.render_drawer(ctx, &colors, viewport_width);

            let page = self
                .ui_state
                .lock()
                .map_or(MainPage::CoordinatorChat, |state| state.page());
            let mut selected_agent = self
                .ui_state
                .lock()
                .ok()
                .and_then(|state| state.selected_agent());
            let welcome = page == MainPage::CoordinatorChat
                && (self
                    .ui_state
                    .lock()
                    .is_ok_and(|state| state.welcome_active())
                    || snapshot.tasks.is_empty());

            if page == MainPage::CoordinatorChat && !welcome {
                let composer_panel = egui::TopBottomPanel::bottom("coordinator_composer")
                    .show_separator_line(false)
                    .frame(
                        egui::Frame::new()
                            .fill(colors.bg)
                            .inner_margin(egui::Margin::symmetric(18, 10)),
                    )
                    .show(ctx, |ui| {
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

            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new()
                        .fill(colors.bg)
                        .inner_margin(egui::Margin::symmetric(18, 12)),
                )
                .show(ctx, |ui| match page {
                    MainPage::CoordinatorChat if welcome => {
                        // Centre the greeting/composer block against the
                        // composer height measured on the previous pass, so the
                        // empty space above and below stays balanced at any
                        // window size instead of being a fixed fraction.
                        let top_space =
                            welcome_top_space(ui.available_height(), self.composer_panel_height);
                        ui.add_space(top_space);
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
                        if let Ok(mut state) = self.ui_state.lock() {
                            chat::render_conversation(
                                ui,
                                ctx,
                                &colors,
                                snapshot,
                                &mut state,
                                &self.actions,
                            );
                        }
                    }
                    MainPage::Agents => {
                        if let Ok(state) = self.ui_state.lock() {
                            selected_agent = state.selected_agent();
                        }
                        let actions = agents::render(ui, &colors, &self.view_model, selected_agent);
                        if actions.back_to_list {
                            selected_agent_change = Some(None);
                        } else if let Some(agent) = actions.selected_agent {
                            selected_agent_change = Some(Some(agent));
                        }
                    }
                });

            if self.settings_open {
                let changes = settings::render(
                    ctx,
                    &colors,
                    &mut self.settings_open,
                    self.user_config.theme,
                    self.user_config.window_size,
                    snapshot.mode == SnapshotMode::Demo,
                );
                if let Some(theme) = changes.theme {
                    self.set_theme(theme);
                }
                if let Some(size) = changes.window_size {
                    self.set_window_size(size);
                }
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
        }
        if let Some(selected) = selected_agent_change {
            if let Ok(mut state) = self.ui_state.lock() {
                if let Some(index) = selected {
                    state.select_agent(index, self.view_model.agents.len());
                } else {
                    state.return_to_agents_list();
                }
            }
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
                        RichText::new(window_task_title(task))
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
    Agents,
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

fn window_task_title(task: &TaskView) -> &str {
    if task.title.trim().is_empty() {
        "Untitled task"
    } else {
        &task.title
    }
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
    use super::*;
    use crate::{
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
            diagnostics_open: false,
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
        let _ = context.run(
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
            assert_eq!(app.ui_state.lock().unwrap().selected_agent(), Some(index));
        }
        app.settings_open = true;
        key_event(&context, &mut app, Key::Escape, false, false);
        assert!(!app.settings_open);
        assert_eq!(app.ui_state.lock().unwrap().page(), MainPage::Agents);
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
        let _ = context.run(raw.clone(), |context| app.render_frame(context));
        let output = context.run(raw, |context| app.render_frame(context));
        (context, output)
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
        let _ = context.run(raw, |context| app.render_frame(context));
    }

    fn focused_composer_app() -> (egui::Context, SupervisorApp) {
        let mut app = test_app();
        app.ui_state
            .lock()
            .unwrap()
            .set_composer_draft("draft".to_string());
        let context = egui::Context::default();
        let _ = context.run(egui::RawInput::default(), |context| {
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
        let _ = context.run(raw.clone(), |context| app.render_frame(context));
        let output = context.run(raw, |context| app.render_frame(context));
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
        let _ = context.run(raw.clone(), |context| app.render_frame(context));
        let output = context.run(raw, |context| app.render_frame(context));
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
        let _ = context.run(raw.clone(), |context| app.render_frame(context));
        context.memory_mut(|memory| memory.request_focus(egui::Id::new(REFRESH_BUTTON_ID)));
        let output = context.run(raw, |context| app.render_frame(context));
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
        let _ = context.run(raw.clone(), |context| app.render_frame(context));
        let output = context.run(raw, |context| app.render_frame(context));
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
                    let _ = context.run(raw.clone(), |context| app.render_frame(context));
                    let _ = context.run(raw, |context| app.render_frame(context));
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
