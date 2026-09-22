#![forbid(unsafe_code)]
//! App state + per-frame orchestration for the two-shell GUI.

use std::time::{Duration, Instant};

use eframe::egui::{self, Key, Modifiers};
use vibemux_probe::AgentKind;

use crate::theme::{
    ThemeId, palette_for,
    serialize::{UserConfig, WindowSize, save_user_config},
};
use crate::view_model::ViewModel;

use super::overview;
use super::stub_bank;
use super::topbar;
use super::workbench::{self, Session};

const WRITE_DEBOUNCE: Duration = Duration::from_millis(250);
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(500);

/// Live per-harness transcripts/composers, indexed exactly like
/// `ViewModel::agents` (which follows `AgentKind::all()` order).
struct Sessions {
    list: Vec<Session>,
    seeds: Vec<String>,
}

impl Sessions {
    fn seed_for(vm: &ViewModel) -> Self {
        let kinds = AgentKind::all();
        let list = vm
            .agents
            .iter()
            .enumerate()
            .map(|(i, _)| Session::new(&stub_bank::stub_lines(kinds[i])))
            .collect::<Vec<_>>();
        let seeds = kinds
            .iter()
            .map(|kind| stub_bank::stub_lines(*kind))
            .collect::<Vec<_>>();
        Self { list, seeds }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Shell {
    Overview,
    Workbench { agent: usize },
}

impl Shell {
    fn is_overview(self) -> bool {
        matches!(self, Shell::Overview)
    }
}

pub struct VibeMuxApp {
    view_model: ViewModel,
    user_config: UserConfig,
    theme_id: ThemeId,
    shell: Shell,
    sessions: Sessions,
    settings: Settings,
    settings_open: bool,
    diag_open: bool,
    last_size: [f32; 2],
    pending_write: Option<Instant>,
    pending_resize: Option<Instant>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BackendKind {
    Mock,
    WezTerm,
    Tmux,
}

#[derive(Clone, Debug)]
struct Settings {
    theme: ThemeId,
    default_backend: BackendKind,
    window_size: WindowSize,
    telemetry_opt_in: bool,
}

impl VibeMuxApp {
    pub fn new(
        _cc: &eframe::CreationContext<'_>,
        view_model: ViewModel,
        user_config: UserConfig,
    ) -> Self {
        let sessions = Sessions::seed_for(&view_model);
        let theme_id = user_config.theme;
        let settings = Settings {
            theme: theme_id,
            default_backend: BackendKind::Mock,
            window_size: user_config.window_size,
            telemetry_opt_in: false,
        };
        Self {
            view_model,
            user_config,
            theme_id,
            shell: Shell::Overview,
            sessions,
            settings,
            settings_open: false,
            diag_open: false,
            last_size: [0.0, 0.0],
            pending_write: None,
            pending_resize: None,
        }
    }

    // ── test/access surface ─────────────────────────────
    #[must_use]
    pub fn theme_id(&self) -> ThemeId {
        self.theme_id
    }
    #[must_use]
    pub fn overview(&self) -> bool {
        matches!(self.shell, Shell::Overview)
    }
    #[must_use]
    pub fn focused(&self) -> Option<usize> {
        match self.shell {
            Shell::Overview => None,
            Shell::Workbench { agent } => Some(agent),
        }
    }
    #[must_use]
    pub fn harness_terminal(&self, agent: AgentKind) -> String {
        let kinds = AgentKind::all();
        kinds
            .iter()
            .position(|k| *k == agent)
            .and_then(|i| self.sessions.list.get(i))
            .map_or_else(String::new, |s| s.text.clone())
    }
    pub fn handle_stub_send(&mut self, agent: AgentKind, line: impl Into<String>) {
        let kinds = AgentKind::all();
        if let Some(i) = kinds.iter().position(|k| *k == agent) {
            let text = &mut self.sessions.list[i].text;
            workbench::push(text, &line.into());
        }
    }

    // ── mutators ─────────────────────────────────────────
    fn set_theme(&mut self, id: ThemeId) {
        if self.theme_id != id {
            self.theme_id = id;
            self.user_config.theme = id;
            self.settings.theme = id;
            self.pending_write = Some(Instant::now());
        }
    }
    fn maybe_persist(&mut self) {
        for (when, dur) in [
            (self.pending_write, WRITE_DEBOUNCE),
            (self.pending_resize, RESIZE_DEBOUNCE),
        ] {
            if let Some(t) = when {
                if t.elapsed() >= dur {
                    save_user_config(&self.user_config);
                }
            }
        }
        self.pending_write = None;
        self.pending_resize = None;
    }
}

impl eframe::App for VibeMuxApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Theme.
        let palette = palette_for(self.theme_id);
        super::apply_theme(ctx, &palette);
        let c = super::pal(&palette);

        self.maybe_persist();
        self.handle_keyboard(ctx);
        let vm = self.view_model.clone();

        // Top chrome.
        egui::TopBottomPanel::top("chrome").show(ctx, |ui| {
            ui.add_space(5.0);
            let acts = topbar::render(
                ui,
                &c,
                self.theme_id,
                "E:\\lab\\vibemux",
                self.shell.is_overview(),
                self.diag_open,
                self.settings_open,
            );
            if let Some(id) = acts.picked_theme {
                self.set_theme(id);
            }
            if acts.toggle_settings {
                self.settings_open = !self.settings_open;
            }
            if acts.toggle_diag {
                self.diag_open = !self.diag_open;
            }
            ui.add_space(5.0);
        });

        match self.shell {
            Shell::Overview => {
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::new()
                            .fill(c.bg)
                            .inner_margin(egui::Margin::symmetric(120, 12)),
                    )
                    .show(ctx, |ui| {
                        let mut open: Option<usize> = None;
                        overview::render(ui, &c, &vm, &mut |i| open = Some(i));
                        if let Some(i) = open {
                            self.shell = Shell::Workbench { agent: i };
                        }
                    });
            }
            Shell::Workbench { agent } => {
                self.render_workbench(ctx, &c, &vm, agent);
            }
        }

        // Overlays.
        if self.settings_open {
            self.render_settings_window(ctx, &c);
        }
        if self.diag_open {
            self.render_diag_window(ctx, &c);
        }

        // Track window size for persistence.
        let size = ctx
            .input(|i| i.viewport().inner_rect)
            .unwrap_or_else(|| egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(0.0, 0.0)));
        let now = [size.width(), size.height()];
        if self.last_size[0] > 0.0 && now != self.last_size {
            let w = (now[0] as u32).clamp(800, 3840);
            let h = (now[1] as u32).clamp(600, 2160);
            if self.user_config.window_size.width != w || self.user_config.window_size.height != h {
                let s = WindowSize {
                    width: w,
                    height: h,
                };
                self.user_config.window_size = s;
                self.settings.window_size = s;
                self.pending_resize = Some(Instant::now());
            }
        }
        self.last_size = now;

        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

impl VibeMuxApp {
    fn handle_keyboard(&mut self, ctx: &egui::Context) {
        // Escape (no modifier): close the topmost overlay first, then
        // leave the seat for the overview. Guarded so a focused text
        // field (the composer) keeps Escape for itself.
        if ctx.input(|i| i.key_pressed(Key::Escape)) && !ctx.wants_keyboard_input() {
            if self.settings_open {
                self.settings_open = false;
                return;
            }
            if self.diag_open {
                self.diag_open = false;
                return;
            }
            if !self.overview() {
                self.shell = Shell::Overview;
            }
            // In the overview with nothing open, Escape is a no-op; fall
            // through so the ctrl-gated keys below still see their input.
        }
        let input = ctx.input(|i| i.clone());
        let ctrl = input.modifiers == Modifiers::CTRL || input.modifiers == Modifiers::COMMAND;
        if !ctrl {
            return;
        }
        if input.key_pressed(Key::T) {
            self.set_theme(self.theme_id.cycle());
            return;
        }
        // The digit row has exactly ten keys and there are ten harness
        // seats: Ctrl+1..9 open seats 1..9 and Ctrl+0 opens the tenth
        // (issue #5; previously only seats 1-5 were reachable).
        const DIGITS: [(Key, u8); 10] = [
            (Key::Num1, 1),
            (Key::Num2, 2),
            (Key::Num3, 3),
            (Key::Num4, 4),
            (Key::Num5, 5),
            (Key::Num6, 6),
            (Key::Num7, 7),
            (Key::Num8, 8),
            (Key::Num9, 9),
            (Key::Num0, 10),
        ];
        for (key, seat) in DIGITS {
            if input.key_pressed(key) {
                let idx = (seat as usize) - 1;
                if idx < self.view_model.agents.len() {
                    self.shell = Shell::Workbench { agent: idx };
                }
                return;
            }
        }
        if input.key_pressed(Key::Comma) {
            self.settings_open = !self.settings_open;
            return;
        }
        if input.key_pressed(Key::Semicolon) {
            self.diag_open = !self.diag_open;
        }
    }

    fn render_workbench(
        &mut self,
        ctx: &egui::Context,
        c: &super::C,
        vm: &ViewModel,
        agent: usize,
    ) {
        // Left rail.
        let mut shell_change: Option<Shell> = None;
        egui::SidePanel::left("rail")
            .exact_width(64.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(c.bg)
                    .inner_margin(egui::Margin::symmetric(13, 14)),
            )
            .show(ctx, |ui| {
                let out = workbench::rail(ui, c, vm, Some(agent));
                if let Some(i) = out.open {
                    shell_change = Some(Shell::Workbench { agent: i });
                }
                if out.to_overview {
                    shell_change = Some(Shell::Overview);
                }
            });
        if let Some(s) = shell_change {
            self.shell = s;
            return;
        }

        // Right inspector.
        egui::SidePanel::right("inspector")
            .exact_width(280.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(c.bg)
                    .inner_margin(egui::Margin::symmetric(6, 0)),
            )
            .show(ctx, |ui| {
                workbench::inspector(ui, c, vm, agent);
            });

        // Composer pinned above the central terminal.
        let can_send = vm
            .agents
            .get(agent)
            .is_some_and(|a| matches!(a.launcher_state, vibemux_probe::ProbeState::Verified));
        let mut clear_now = false;
        egui::TopBottomPanel::bottom("composer").show(ctx, |ui| {
            ui.add_space(6.0);
            if let Some(s) = self.sessions.list.get_mut(agent) {
                clear_now = workbench::composer(ui, c, s, can_send);
            }
            ui.add_space(6.0);
        });
        if clear_now {
            if let Some(s) = self.sessions.list.get_mut(agent) {
                s.text = self.sessions.seeds[agent].clone();
            }
        }

        // Central terminal stage.
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(c.bg)
                    .inner_margin(egui::Margin::symmetric(20, 10)),
            )
            .show(ctx, |ui| {
                if let Some(s) = self.sessions.list.get_mut(agent) {
                    workbench::stage_body(ui, c, vm, agent, s);
                }
            });
    }

    fn render_settings_window(&mut self, ctx: &egui::Context, c: &super::C) {
        let mut open = self.settings_open;
        let mut theme_pick: Option<ThemeId> = None;
        let mut wsize: Option<WindowSize> = None;
        let ui_out = egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(360.0);
        ui_out.show(ctx, |ui| {
            ui.label(
                egui::RichText::new("Theme")
                    .color(c.muted)
                    .size(10.0)
                    .strong()
                    .monospace(),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                for id in ThemeId::ALL {
                    let active = id == self.settings.theme;
                    if ui
                        .add(
                            egui::Button::new(egui::RichText::new(id.as_str()).color(if active {
                                c.bg
                            } else {
                                c.txt
                            }))
                            .fill(if active { c.accent } else { c.alt })
                            .stroke(egui::Stroke::new(1.0, c.hair)),
                        )
                        .clicked()
                    {
                        theme_pick = Some(id);
                    }
                }
            });
            ui.add_space(10.0);
            ui.label(
                egui::RichText::new("Default backend")
                    .color(c.muted)
                    .size(10.0)
                    .strong()
                    .monospace(),
            );
            ui.add_space(2.0);
            egui::ComboBox::from_id_salt("backend")
                .selected_text(backend_label(self.settings.default_backend))
                .show_ui(ui, |ui| {
                    for b in [BackendKind::Mock, BackendKind::WezTerm, BackendKind::Tmux] {
                        ui.selectable_value(
                            &mut self.settings.default_backend,
                            b,
                            backend_label(b),
                        );
                    }
                });
            ui.add_space(10.0);
            ui.label(
                egui::RichText::new("Window size")
                    .color(c.muted)
                    .size(10.0)
                    .strong()
                    .monospace(),
            );
            ui.add_space(2.0);
            let mut width = self.settings.window_size.width;
            let mut height = self.settings.window_size.height;
            ui.horizontal(|ui| {
                ui.label("w");
                ui.add(
                    egui::DragValue::new(&mut width)
                        .range(800..=3840)
                        .clamp_existing_to_range(true),
                );
                ui.add_space(8.0);
                ui.label("h");
                ui.add(
                    egui::DragValue::new(&mut height)
                        .range(600..=2160)
                        .clamp_existing_to_range(true),
                );
            });
            if width != self.settings.window_size.width
                || height != self.settings.window_size.height
            {
                wsize = Some(WindowSize { width, height });
            }
            ui.add_space(10.0);
            ui.checkbox(
                &mut self.settings.telemetry_opt_in,
                "Telemetry opt-in (in-memory only)",
            );
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("Backend + telemetry choices stay in-memory this slice.")
                    .color(c.faint)
                    .size(9.5)
                    .italics(),
            );
        });
        self.settings_open = open;
        if let Some(id) = theme_pick {
            self.set_theme(id);
        }
        if let Some(s) = wsize {
            self.set_window_size(s);
        }
    }

    fn render_diag_window(&mut self, ctx: &egui::Context, c: &super::C) {
        let vm = &self.view_model;
        let mut open = self.diag_open;
        egui::Window::new("Diagnostics")
            .open(&mut open)
            .collapsible(false)
            .default_width(460.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(420.0)
                    .show(ui, |ui| {
                        for (k, v) in [
                            ("gateway", vm.gateway_summary.clone()),
                            ("a2a", vm.a2a_summary.clone()),
                            ("telemetry", vm.telemetry_summary.clone()),
                        ] {
                            mono_def(ui, c, k, &v);
                        }
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("ENVIRONMENT (allowlisted)")
                                .color(c.muted)
                                .size(9.0)
                                .strong()
                                .monospace(),
                        );
                        for (key, value) in &vm.env_allowlist {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{}={}",
                                    key,
                                    value.as_deref().unwrap_or("<unset>")
                                ))
                                .color(c.faint)
                                .size(10.0)
                                .monospace(),
                            );
                        }
                        ui.add_space(8.0);
                        mono_def(ui, c, "schema_version", &format!("{}", vm.schema_version));
                        mono_def(
                            ui,
                            c,
                            "observed_at",
                            &format!("{}s", vm.observed_at_epoch_seconds),
                        );
                    });
            });
        self.diag_open = open;
    }

    fn set_window_size(&mut self, s: WindowSize) {
        if self.user_config.window_size != s {
            self.user_config.window_size = s;
            self.settings.window_size = s;
            self.pending_resize = Some(Instant::now());
        }
    }
}

fn backend_label(b: BackendKind) -> &'static str {
    match b {
        BackendKind::Mock => "mock",
        BackendKind::WezTerm => "wezterm",
        BackendKind::Tmux => "tmux",
    }
}

fn mono_def(ui: &mut egui::Ui, c: &super::C, k: &str, v: &str) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(k).color(c.faint).size(10.5).monospace());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(v).color(c.txt).size(10.5).monospace());
        });
    });
    ui.add_space(3.0);
}

/// Build a `VibeMuxApp` without an eframe context (used by tests).
#[cfg(test)]
impl VibeMuxApp {
    fn new_for_test(view_model: ViewModel, user_config: UserConfig) -> Self {
        let sessions = Sessions::seed_for(&view_model);
        let theme_id = user_config.theme;
        let settings = Settings {
            theme: theme_id,
            default_backend: BackendKind::Mock,
            window_size: user_config.window_size,
            telemetry_opt_in: false,
        };
        Self {
            view_model,
            user_config,
            theme_id,
            shell: Shell::Overview,
            sessions,
            settings,
            settings_open: false,
            diag_open: false,
            last_size: [0.0, 0.0],
            pending_write: None,
            pending_resize: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::serialize::{SCHEMA_VERSION, WindowSize};
    use vibemux_probe::{
        A2aSelfTestProbe, AgentKind, AgentProbe, GatewayProbe, LauncherKind, ProbeReport,
        ProbeState, RouteKind,
    };

    fn fixture_report() -> ProbeReport {
        ProbeReport {
            schema_version: 1,
            observed_at_epoch_seconds: 1,
            platform: "windows".to_string(),
            agents: AgentKind::all()
                .into_iter()
                .map(|agent| AgentProbe {
                    agent,
                    launcher_state: ProbeState::Verified,
                    authentication_state: ProbeState::NotRun,
                    inference_state: ProbeState::NotRun,
                    launcher: LauncherKind::DirectExecutable,
                    path: None,
                    version: Some("1.0".to_string()),
                    route: RouteKind::Direct,
                    endpoints: Vec::new(),
                    code: "version_verified".to_string(),
                })
                .collect(),
            gateway: GatewayProbe {
                state: ProbeState::Verified,
                host: "127.0.0.1".to_string(),
                port: 15_721,
                tcp_reachable: true,
                health_status: Some(200),
                telemetry_state: ProbeState::Verified,
                telemetry: Vec::new(),
                code: "gateway_verified".to_string(),
            },
            a2a: A2aSelfTestProbe {
                state: ProbeState::Verified,
                correlation_preserved: true,
                listener_closed: true,
                code: "a2a_self_test_verified".to_string(),
            },
        }
    }

    fn cfg(theme: ThemeId) -> UserConfig {
        UserConfig {
            schema_version: SCHEMA_VERSION,
            theme,
            window_size: WindowSize {
                width: 1280,
                height: 800,
            },
        }
    }

    #[test]
    fn app_starts_in_overview() {
        let app = VibeMuxApp::new_for_test(
            ViewModel::from_report(&fixture_report()),
            cfg(ThemeId::Claude),
        );
        assert!(app.overview());
        assert_eq!(app.sessions.list.len(), 10);
    }

    #[test]
    fn send_appends_to_correct_harness_terminal() {
        let mut app = VibeMuxApp::new_for_test(
            ViewModel::from_report(&fixture_report()),
            cfg(ThemeId::Claude),
        );
        let before = app.harness_terminal(AgentKind::Claude).len();
        app.handle_stub_send(AgentKind::Claude, "hello");
        let after = app.harness_terminal(AgentKind::Claude);
        assert!(after.len() > before);
        assert!(after.ends_with("hello\n"));
        // Codex untouched.
        assert_eq!(
            app.harness_terminal(AgentKind::Codex),
            stub_bank::stub_lines(AgentKind::Codex)
        );
    }

    #[test]
    fn theme_change_updates_user_config() {
        let mut app = VibeMuxApp::new_for_test(
            ViewModel::from_report(&fixture_report()),
            cfg(ThemeId::Claude),
        );
        app.set_theme(ThemeId::Github);
        assert_eq!(app.theme_id(), ThemeId::Github);
        assert_eq!(app.user_config.theme, ThemeId::Github);
    }

    // ── keyboard accelerators (issue #5) ───────────────────

    fn press_key(app: &mut VibeMuxApp, key: Key, modifiers: egui::Modifiers) {
        let ctx = egui::Context::default();
        // InputState::modifiers comes from RawInput::modifiers (not from
        // the event's own modifiers field), so both must carry them.
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                modifiers,
                ..Default::default()
            },
            |ctx| app.handle_keyboard(ctx),
        );
    }

    #[test]
    fn ctrl_digits_open_all_ten_seats() {
        let digits = [
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
        ];
        for (idx, key) in digits.iter().enumerate() {
            let mut app = VibeMuxApp::new_for_test(
                ViewModel::from_report(&fixture_report()),
                cfg(ThemeId::Claude),
            );
            assert!(
                app.overview(),
                "test starts in overview for seat {}",
                idx + 1
            );
            press_key(&mut app, *key, egui::Modifiers::CTRL);
            assert_eq!(
                app.focused(),
                Some(idx),
                "Ctrl+{} must open seat {} (0-based {})",
                if idx == 9 { 0 } else { idx + 1 },
                idx + 1,
                idx
            );
        }
    }

    #[test]
    fn escape_closes_overlays_then_returns_to_overview() {
        let mut app = VibeMuxApp::new_for_test(
            ViewModel::from_report(&fixture_report()),
            cfg(ThemeId::Claude),
        );
        // From the overview with an overlay open, Escape closes the
        // overlay without changing the shell.
        app.settings_open = true;
        press_key(&mut app, Key::Escape, egui::Modifiers::NONE);
        assert!(!app.settings_open, "Escape must close the settings overlay");
        assert!(app.overview());

        // From a seat, Escape closes the diag overlay first and keeps
        // the seat; a second Escape returns to the overview.
        app.diag_open = true;
        app.shell = Shell::Workbench { agent: 7 };
        press_key(&mut app, Key::Escape, egui::Modifiers::NONE);
        assert!(!app.diag_open, "Escape must close the diag overlay");
        assert_eq!(app.shell, Shell::Workbench { agent: 7 });

        press_key(&mut app, Key::Escape, egui::Modifiers::NONE);
        assert!(app.overview(), "Escape from a seat must return to overview");

        // In the overview with nothing open, Escape is a no-op.
        press_key(&mut app, Key::Escape, egui::Modifiers::NONE);
        assert!(app.overview());
    }
}
