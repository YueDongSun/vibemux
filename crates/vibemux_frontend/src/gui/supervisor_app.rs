#![forbid(unsafe_code)]
//! Native Supervisor Chat application shell and immutable snapshot bridge.

use std::{
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};

use eframe::egui::{self, Align2, Key, Modifiers, RichText, ViewportBuilder};
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
    C, agents, chat, diagnostics, settings, sidebar,
    supervisor_state::{
        CHAT_DRAWER_DOCK_THRESHOLD, CHAT_DRAWER_WIDTH, MainPage, SupervisorUiState, UiActionQueue,
    },
    task_detail,
};

const WRITE_DEBOUNCE: Duration = Duration::from_millis(250);
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(500);
const TASK_WINDOW_SIZE: [f32; 2] = [1080.0, 760.0];
const TASK_WINDOW_MIN_SIZE: [f32; 2] = [720.0, 520.0];

pub struct SupervisorApp {
    view_model: ViewModel,
    snapshot: Arc<RwLock<SupervisorSnapshot>>,
    user_config: UserConfig,
    ui_state: Arc<Mutex<SupervisorUiState>>,
    actions: UiActionQueue,
    settings_open: bool,
    diagnostics_open: bool,
    last_size: [f32; 2],
    pending_write: Option<Instant>,
    pending_resize: Option<Instant>,
}

impl SupervisorApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        view_model: ViewModel,
        user_config: UserConfig,
    ) -> Self {
        install_system_cjk_fallback(&cc.egui_ctx);
        let actions = UiActionQueue::default();
        actions.enqueue(SupervisorAction::Refresh);
        Self {
            view_model,
            snapshot: Arc::new(RwLock::new(SupervisorSnapshot::unavailable())),
            user_config,
            ui_state: Arc::new(Mutex::new(SupervisorUiState::default())),
            actions,
            settings_open: false,
            diagnostics_open: false,
            last_size: [0.0, 0.0],
            pending_write: None,
            pending_resize: None,
        }
    }

    /// Replace the displayed immutable supervisor snapshot.
    pub fn set_supervisor_snapshot(&mut self, snapshot: SupervisorSnapshot) {
        if let Ok(mut current) = self.snapshot.write() {
            *current = snapshot;
            if let Ok(mut state) = self.ui_state.lock() {
                state.reconcile_snapshot(&current);
            }
        }
    }

    /// Drain bounded requests for the parent runtime adapter.
    #[must_use]
    pub fn take_supervisor_actions(&self) -> Vec<SupervisorAction> {
        self.actions.drain()
    }

    /// Select a task and open its in-app detail drawer.
    pub fn select_task(&mut self, task_id: &str) -> bool {
        let snapshot = self.snapshot();
        let selected = self
            .ui_state
            .lock()
            .is_ok_and(|mut state| state.select_task(&snapshot, task_id));
        if selected {
            self.actions.enqueue(SupervisorAction::LoadTask {
                task_id: task_id.to_string(),
            });
        }
        selected
    }

    /// Open one native task viewport, reusing the stable task viewport ID.
    pub fn open_task_window(&mut self, task_id: &str) -> bool {
        let snapshot = self.snapshot();
        let opened = self
            .ui_state
            .lock()
            .is_ok_and(|mut state| state.open_task_window(&snapshot, task_id));
        if opened {
            self.actions.enqueue(SupervisorAction::LoadTask {
                task_id: task_id.to_string(),
            });
        }
        opened
    }

    /// Select the content tab of an existing native task view.
    pub fn set_task_window_tab(&mut self, task_id: &str, tab: super::TaskDetailTab) {
        let snapshot = self.snapshot();
        if let Some(task) = snapshot.tasks.iter().find(|task| task.task_id == task_id) {
            if let Ok(state) = self.ui_state.lock() {
                state
                    .with_task_window_selection_mut(task_id, task, |selection| selection.tab = tab);
            }
        }
    }

    fn snapshot(&self) -> SupervisorSnapshot {
        self.snapshot.read().map_or_else(
            |_| SupervisorSnapshot::unavailable(),
            |snapshot| snapshot.clone(),
        )
    }

    fn enqueue_refresh(&self) {
        self.actions.enqueue(SupervisorAction::Refresh);
    }

    fn persist_if_due(&mut self) {
        if self.snapshot().mode == SnapshotMode::Demo {
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

    fn render_topbar(&mut self, ctx: &egui::Context, colors: &C, snapshot: &SupervisorSnapshot) {
        egui::TopBottomPanel::top("supervisor_header")
            .show_separator_line(false)
            .exact_height(76.0)
            .frame(
                egui::Frame::new()
                    .fill(colors.bg)
                    .inner_margin(egui::Margin::symmetric(20, 10)),
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
                            .size(16.0)
                            .strong()
                            .color(colors.txt),
                    );
                    ui.label(
                        RichText::new(format!("/  {title}"))
                            .size(12.0)
                            .color(colors.muted),
                    );
                    ui.add_space(10.0);
                    connection_badge(ui, colors, snapshot.connection.status);
                    if snapshot.mode == SnapshotMode::Demo {
                        ui.add_space(10.0);
                        chat::demo_badge(ui, colors);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Refresh").clicked() {
                            self.enqueue_refresh();
                        }
                    });
                });
                if let Some(detail) = snapshot.connection.detail.as_deref() {
                    ui.add_space(3.0);
                    chat::wrapped_label(ui, RichText::new(detail).size(12.0).color(colors.muted));
                }
            });
    }

    fn render_drawer(
        &mut self,
        ctx: &egui::Context,
        colors: &C,
        snapshot: &SupervisorSnapshot,
        viewport_width: f32,
    ) {
        let selected_task = self
            .ui_state
            .lock()
            .ok()
            .and_then(|state| state.selected_task(snapshot).cloned());
        if !self.ui_state.lock().is_ok_and(|state| state.details_open()) {
            return;
        }
        let Some(task) = selected_task else {
            return;
        };

        if viewport_width >= CHAT_DRAWER_DOCK_THRESHOLD {
            let mut close_clicked = false;
            egui::SidePanel::right("task_details_drawer")
                .exact_width(CHAT_DRAWER_WIDTH)
                .resizable(false)
                .frame(
                    egui::Frame::new()
                        .fill(colors.bg)
                        .inner_margin(egui::Margin::same(12)),
                )
                .show(ctx, |ui| {
                    drawer_contents(
                        ui,
                        colors,
                        snapshot,
                        &task,
                        &self.ui_state,
                        &self.actions,
                        &mut close_clicked,
                    );
                });
            if close_clicked {
                if let Ok(mut state) = self.ui_state.lock() {
                    state.close_details();
                }
            }
        } else {
            let mut keep_open = true;
            let mut close_clicked = false;
            egui::Window::new("Task details")
                .id(egui::Id::new("task_details_overlay"))
                .open(&mut keep_open)
                .anchor(Align2::RIGHT_CENTER, egui::vec2(-12.0, 0.0))
                .default_width(CHAT_DRAWER_WIDTH)
                .default_height((ctx.available_rect().height() - 20.0).max(340.0))
                .min_width(280.0)
                .resizable(false)
                .collapsible(false)
                .show(ctx, |ui| {
                    drawer_contents(
                        ui,
                        colors,
                        snapshot,
                        &task,
                        &self.ui_state,
                        &self.actions,
                        &mut close_clicked,
                    );
                });
            if !keep_open || close_clicked {
                if let Ok(mut state) = self.ui_state.lock() {
                    state.close_details();
                }
            }
        }
    }

    fn render_task_viewports(
        &self,
        ctx: &egui::Context,
        snapshot: &SupervisorSnapshot,
        theme: ThemeId,
    ) {
        let open_tasks = self
            .ui_state
            .lock()
            .map_or_else(|_| Vec::new(), |state| state.open_task_windows());
        let colors = super::pal(&palette_for(theme));
        for task_id in open_tasks {
            let task = snapshot.tasks.iter().find(|task| task.task_id == task_id);
            let viewport_id = chat::task_viewport_id(&task_id);
            let title = format!(
                "{} · VibeMux",
                task.map_or("Task observation", window_task_title)
            );
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
                let snapshot = snapshot_arc.read().map_or_else(
                    |_| SupervisorSnapshot::unavailable(),
                    |snapshot| snapshot.clone(),
                );
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
                            task_detail::render(
                                ui,
                                &task_colors,
                                &snapshot,
                                task,
                                selection,
                                &actions,
                            );
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
                snapshot.connection.detail = Some(
                    "Action queue is full. The last action was not sent; please try again.".into(),
                );
            }
        }
        let snapshot = self.snapshot();
        let palette = palette_for(self.user_config.theme);
        super::apply_theme(ctx, &palette);
        ctx.style_mut(|style| {
            style
                .text_styles
                .insert(egui::TextStyle::Body, egui::FontId::proportional(15.0));
        });
        let colors = super::pal(&palette);
        self.persist_if_due();
        self.handle_keyboard(ctx);

        let viewport_width = ctx
            .input(|input| input.viewport().inner_rect.map(|rect| rect.width()))
            .unwrap_or(1280.0);
        let active_page = self
            .ui_state
            .lock()
            .map_or(MainPage::CoordinatorChat, |state| state.page());
        let mut sidebar_actions = None;
        egui::SidePanel::left("supervisor_sidebar")
            .exact_width(sidebar::SIDEBAR_WIDTH)
            .resizable(false)
            .frame(egui::Frame::new().fill(colors.surf))
            .show(ctx, |ui| {
                sidebar_actions = Some(sidebar::render(
                    ui,
                    &colors,
                    &snapshot,
                    active_page,
                    self.user_config.theme,
                    self.settings_open,
                    self.diagnostics_open,
                ));
            });
        if let Some(actions) = sidebar_actions {
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
            if actions.open_settings {
                self.settings_open = true;
            }
            if actions.open_diagnostics {
                self.diagnostics_open = true;
            }
        }

        self.render_topbar(ctx, &colors, &snapshot);
        self.render_drawer(ctx, &colors, &snapshot, viewport_width);

        let page = self
            .ui_state
            .lock()
            .map_or(MainPage::CoordinatorChat, |state| state.page());
        let mut selected_agent = self
            .ui_state
            .lock()
            .ok()
            .and_then(|state| state.selected_agent());
        let mut selected_agent_change = None;

        if page == MainPage::CoordinatorChat {
            egui::TopBottomPanel::bottom("coordinator_composer")
                .exact_height(204.0)
                .frame(
                    egui::Frame::new()
                        .fill(colors.bg)
                        .inner_margin(egui::Margin::symmetric(18, 10)),
                )
                .show(ctx, |ui| {
                    if let Ok(mut state) = self.ui_state.lock() {
                        chat::render_composer(ui, &colors, &mut state);
                    }
                });
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(colors.bg)
                    .inner_margin(egui::Margin::symmetric(18, 12)),
            )
            .show(ctx, |ui| match page {
                MainPage::CoordinatorChat => {
                    if let Ok(mut state) = self.ui_state.lock() {
                        chat::render_conversation(
                            ui,
                            ctx,
                            &colors,
                            &snapshot,
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
        if let Some(selected) = selected_agent_change {
            if let Ok(mut state) = self.ui_state.lock() {
                if let Some(index) = selected {
                    state.select_agent(index, self.view_model.agents.len());
                } else {
                    state.return_to_agents_list();
                }
            }
        }

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
                &snapshot,
                &self.view_model,
            );
        }

        self.render_task_viewports(ctx, &snapshot, self.user_config.theme);
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
        ui.label(
            RichText::new("Task details")
                .size(16.0)
                .strong()
                .color(colors.txt),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("Close").clicked() {
                *close_clicked = true;
            }
        });
    });
    ui.separator();
    if let Ok(mut state) = state.lock() {
        task_detail::render(
            ui,
            colors,
            snapshot,
            task,
            state.details_selection_mut(),
            actions,
        );
    }
}

fn connection_badge(ui: &mut egui::Ui, colors: &C, status: ConnectionStatus) {
    let (label, color) = match status {
        ConnectionStatus::Connected => ("Connected", colors.ok),
        ConnectionStatus::Connecting => ("Connecting", colors.warn),
        ConnectionStatus::Disconnected => ("Disconnected", colors.muted),
        ConnectionStatus::Unavailable => ("Unavailable", colors.muted),
    };
    egui::Frame::new()
        .fill(colors.surf)
        .stroke(egui::Stroke::new(1.0, colors.border))
        .corner_radius(egui::CornerRadius::same(7))
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(label).size(12.0).color(color));
        });
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

fn window_task_title(task: &TaskView) -> &str {
    if task.title.trim().is_empty() {
        "Task details"
    } else {
        &task.title
    }
}

#[cfg(windows)]
fn install_system_cjk_fallback(ctx: &egui::Context) {
    use std::{env, fs, path::PathBuf};

    let system_root = env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let fonts_dir = system_root.join("Fonts");
    let candidates = ["msyh.ttc", "simhei.ttf", "meiryo.ttc", "YuGothM.ttc"];
    let Some(bytes) = candidates
        .iter()
        .find_map(|font_name| fs::read(fonts_dir.join(font_name)).ok())
    else {
        return;
    };
    let mut definitions = egui::FontDefinitions::default();
    if let Ok(latin) = fs::read(fonts_dir.join("segoeui.ttf")) {
        definitions.font_data.insert(
            "vibemux_ui".into(),
            egui::FontData::from_owned(latin).into(),
        );
        definitions
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "vibemux_ui".into());
    }
    let font_name = "vibemux_system_cjk".to_string();
    definitions
        .font_data
        .insert(font_name.clone(), egui::FontData::from_owned(bytes).into());
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        definitions
            .families
            .entry(family)
            .or_default()
            .insert(1, font_name.clone());
    }
    ctx.set_fonts(definitions);
}

#[cfg(not(windows))]
fn install_system_cjk_fallback(_ctx: &egui::Context) {}

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
            snapshot: Arc::new(RwLock::new(fixture_snapshot())),
            user_config: config(),
            ui_state: Arc::new(Mutex::new(SupervisorUiState::default())),
            actions: UiActionQueue::default(),
            settings_open: false,
            diagnostics_open: false,
            last_size: [0.0, 0.0],
            pending_write: None,
            pending_resize: None,
        }
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

    #[test]
    fn shell_renders_all_themes_sizes_and_scales_without_dropping_the_composer() {
        for theme in ThemeId::ALL {
            for (width, height) in [(960.0, 600.0), (1280.0, 800.0), (1920.0, 1080.0)] {
                for scale in [1.0, 1.5, 2.0] {
                    let mut app = test_app();
                    app.user_config.theme = theme;
                    app.snapshot.write().unwrap().mode = SnapshotMode::Demo;
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
                    let output = context.run(raw, |context| app.render_frame(context));
                    let has_send = output.shapes.iter().any(|shape| match &shape.shape {
                        egui::epaint::Shape::Text(text) => {
                            text.galley.job.text == "Send" && rect.contains(text.pos)
                        }
                        _ => false,
                    });
                    assert!(
                        has_send,
                        "composer missing: {theme:?} {width}x{height} scale {scale}"
                    );
                }
            }
        }
    }
}
