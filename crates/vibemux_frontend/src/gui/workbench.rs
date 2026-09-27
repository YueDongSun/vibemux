#![forbid(unsafe_code)]
//! Seat view (ADR 027 shell): a Primer-style page header for the
//! focused harness, a bordered transcript card, a Claude-style rounded
//! composer, and a right-hand inspector. Pure painter + egui widgets;
//! no exotic glyphs.

use eframe::egui::{self, Align2, Color32, FontId, RichText, Sense, Stroke, Ui, vec2};

use crate::view_model::{ViewModel, route_label, state_label};

use super::C;

/// One live transcript for one harness; owned by the app.
#[derive(Clone, Debug)]
pub struct Session {
    pub text: String,
    pub composer: String,
}

impl Session {
    #[must_use]
    pub fn new(seed: &str) -> Self {
        Self {
            text: seed.to_string(),
            composer: String::new(),
        }
    }
}

/// Append a line to a transcript (newline-delimited, no trailing blank).
pub fn push(text: &mut String, line: &str) {
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(line);
    if !text.ends_with('\n') {
        text.push('\n');
    }
}

/// Keyboard accelerator label for seat `idx` (0-based): seats 1-9 map to
/// `Ctrl+1..9` and the tenth seat wraps to `Ctrl+0`, because the digit row
/// has exactly ten keys and every harness seat must be keyboard-reachable
/// (issue #5). Seats past ten fall back to their ordinal.
#[must_use]
pub fn seat_accelerator(idx: usize) -> String {
    if idx == 9 {
        "0".to_string()
    } else {
        (idx + 1).to_string()
    }
}

fn state_color(c: &C, state: vibemux_probe::ProbeState) -> Color32 {
    match state {
        vibemux_probe::ProbeState::Verified => c.ok,
        vibemux_probe::ProbeState::Failed => c.danger,
        vibemux_probe::ProbeState::Unavailable => c.warn,
        vibemux_probe::ProbeState::NotRun => c.muted,
    }
}

/// Primer-style status pill: bordered rounded label.
fn state_badge(ui: &mut Ui, c: &C, state: vibemux_probe::ProbeState) {
    let label = state_label(state);
    let color = state_color(c, state);
    let w = 14.0 + label.chars().count() as f32 * 6.2;
    let (rect, _) = ui.allocate_exact_size(vec2(w, 20.0), Sense::hover());
    let p = ui.painter();
    p.rect_stroke(
        rect,
        egui::CornerRadius::same(10),
        Stroke::new(
            1.0,
            if state == vibemux_probe::ProbeState::NotRun {
                c.border
            } else {
                color
            },
        ),
        egui::StrokeKind::Inside,
    );
    p.text(
        rect.center(),
        Align2::CENTER_CENTER,
        format!("● {label}"),
        FontId::monospace(9.5),
        color,
    );
}

/// Page header strip for the seat: name, badge, mono meta.
pub fn seat_header(ui: &mut Ui, c: &C, vm: &ViewModel, idx: usize) {
    let Some(agent) = vm.agents.get(idx) else {
        return;
    };
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(agent.name.clone())
                .color(c.txt)
                .size(17.0)
                .strong(),
        );
        ui.add_space(6.0);
        state_badge(ui, c, agent.launcher_state);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new("esc → overview")
                    .color(c.faint)
                    .size(9.5)
                    .monospace(),
            );
        });
    });
    ui.add_space(2.0);
    ui.label(
        RichText::new(format!(
            "{} · v{} · route {}",
            agent.name.to_lowercase(),
            agent.version,
            route_label(agent.route)
        ))
        .color(c.faint)
        .size(10.5)
        .monospace(),
    );
}

/// Right-hand inspector for the focused harness.
pub fn inspector(ui: &mut Ui, c: &C, vm: &ViewModel, idx: usize) {
    let Some(agent) = vm.agents.get(idx) else {
        return;
    };

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(16.0);
            ui.label(
                RichText::new("DETAILS")
                    .color(c.muted)
                    .size(9.0)
                    .strong()
                    .monospace(),
            );
            ui.add_space(10.0);

            def(ui, c, "launcher", state_label(agent.launcher_state), true);
            def(
                ui,
                c,
                "auth",
                state_label(agent.authentication_state),
                false,
            );
            def(
                ui,
                c,
                "inference",
                state_label(agent.inference_state),
                false,
            );
            def(ui, c, "version", &format!("v{}", agent.version), true);
            def(ui, c, "route", route_label(agent.route), true);
            def(ui, c, "schema", &format!("{}", vm.schema_version), true);

            ui.add_space(10.0);
            hairline(ui, c);
            ui.add_space(8.0);
            ui.label(
                RichText::new("ENVIRONMENT")
                    .color(c.muted)
                    .size(9.0)
                    .strong()
                    .monospace(),
            );
            ui.add_space(4.0);
            for (key, value) in vm.env_allowlist.iter().take(5) {
                env_row(ui, c, key, value.as_deref());
            }

            ui.add_space(10.0);
            hairline(ui, c);
            ui.add_space(8.0);
            ui.label(
                RichText::new("REACH")
                    .color(c.muted)
                    .size(9.0)
                    .strong()
                    .monospace(),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!("route {}", route_label(agent.route)))
                    .color(c.faint)
                    .size(10.0)
                    .monospace(),
            );
            ui.label(
                RichText::new(vm.gateway_summary.clone())
                    .color(c.faint)
                    .size(10.0)
                    .monospace(),
            );
            ui.label(
                RichText::new(format!("a2a {}", vm.a2a_summary))
                    .color(c.faint)
                    .size(10.0)
                    .monospace(),
            );
            ui.add_space(20.0);
        });
}

fn def(ui: &mut Ui, c: &C, k: &str, v: &str, neutral: bool) {
    ui.horizontal(|ui| {
        ui.add_space(2.0);
        ui.label(RichText::new(k).color(c.faint).size(11.5));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let col = if neutral {
                c.muted
            } else if v == "verified" {
                c.ok
            } else {
                c.faint
            };
            ui.label(RichText::new(v).color(col).size(11.0).monospace());
        });
    });
    ui.add_space(6.0);
}

fn env_row(ui: &mut Ui, c: &C, k: &str, value: Option<&str>) {
    ui.horizontal(|ui| {
        ui.add_space(2.0);
        ui.label(RichText::new(k).color(c.faint).size(10.0).monospace());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(if value.is_none() { "unset" } else { "set" })
                    .color(c.faint)
                    .size(10.0)
                    .monospace(),
            );
        });
    });
    ui.add_space(3.0);
}

fn hairline(ui: &mut Ui, c: &C) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
    ui.painter().line_segment(
        [rect.left_center(), rect.right_center()],
        Stroke::new(1.0, c.border.gamma_multiply(0.55)),
    );
}

/// Bottom composer (Send / interrupt / Clear) in the Claude-Desktop
/// style: a wide rounded input with an accent send button on the right.
/// Returns true if the user asked to clear the transcript.
pub fn composer(ui: &mut Ui, c: &C, session: &mut Session, can_send: bool) -> bool {
    let mut cleared = false;
    egui::Frame::new()
        .fill(c.surf)
        .stroke(Stroke::new(1.0, c.border))
        .corner_radius(12)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let edit = egui::TextEdit::singleline(&mut session.composer)
                    .font(egui::TextStyle::Monospace)
                    .hint_text("Message this agent … (stub)")
                    .text_color(c.txt)
                    .desired_width((ui.available_width() - 250.0).max(120.0));
                ui.add(edit);
                let sendable = can_send && !session.composer.trim().is_empty();
                if ui
                    .add_enabled(
                        sendable,
                        egui::Button::new(RichText::new("Send").color(c.bg).strong())
                            .fill(c.accent)
                            .corner_radius(8)
                            .stroke(Stroke::NONE),
                    )
                    .clicked()
                {
                    let line = session.composer.trim().to_string();
                    push(&mut session.text, &format!("you> {line}"));
                    session.composer.clear();
                }
                if ui
                    .add(
                        egui::Button::new("Ctrl-C")
                            .fill(c.alt)
                            .corner_radius(8)
                            .stroke(Stroke::new(1.0, c.border)),
                    )
                    .clicked()
                {
                    push(&mut session.text, "^C (interrupt)");
                }
                if ui
                    .add(
                        egui::Button::new("Clear")
                            .fill(c.alt)
                            .corner_radius(8)
                            .stroke(Stroke::new(1.0, c.border)),
                    )
                    .clicked()
                {
                    cleared = true;
                }
            });
        });
    cleared
}

/// Terminal body inside a bordered card. Rendered in the central panel.
pub fn stage_body(ui: &mut Ui, c: &C, _vm: &ViewModel, _idx: usize, session: &mut Session) {
    let height = (ui.available_height() - 8.0).max(120.0);
    let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area));
    {
        let painter = child.painter();
        painter.rect_filled(area, egui::CornerRadius::same(6), c.term_bg);
        painter.rect_stroke(
            area,
            egui::CornerRadius::same(6),
            Stroke::new(1.0, c.border),
            egui::StrokeKind::Inside,
        );
    }
    egui::ScrollArea::vertical()
        .id_salt("term")
        .max_height(area.height() - 16.0)
        .auto_shrink([false, false])
        .show(&mut child, |ui| {
            ui.add_space(10.0);
            ui.spacing_mut().item_spacing.y = 1.0;
            for line in session.text.split('\n') {
                if line.trim().is_empty() {
                    ui.add_space(2.0);
                    continue;
                }
                let color = line_color(line, c);
                ui.label(RichText::new(line).monospace().size(11.5).color(color));
            }
            ui.add_space(10.0);
        });
}

fn line_color(line: &str, c: &C) -> Color32 {
    let t = line.trim_start();
    if t.starts_with("you>") || t.starts_with("Claude>") || t.starts_with('>') {
        c.term_fg
    } else if t.starts_with("^C") || t.contains("interrupt") {
        c.warn
    } else {
        c.muted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ThemeId, palette_for};
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

    #[test]
    fn seat_accelerator_maps_the_digit_row_onto_seats() {
        // Seats 1-9 map to Ctrl+1..9; the tenth seat wraps to Ctrl+0 so
        // every harness seat has a keyboard path (issue #5).
        let expected = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"];
        for (idx, want) in expected.iter().enumerate() {
            assert_eq!(&seat_accelerator(idx), want, "seat {}", idx + 1);
        }
        // Out-of-range fallback: ordinal (no seat exists past ten today).
        assert_eq!(seat_accelerator(10), "11");
    }

    /// Headless render of the seat view mirroring
    /// `VibeMuxApp::render_workbench` (sidebar + inspector + composer +
    /// header/stage). Guards against silent-empty-render bugs in the
    /// seat shell.
    #[test]
    fn seat_body_paints_text_shapes() {
        let vm = crate::view_model::ViewModel::from_report(&fixture_report());
        let palette = palette_for(ThemeId::Github);
        let c = crate::gui::pal(&palette);
        let ctx = egui::Context::default();
        let mut session = Session::new("stub line one\nstub line two\n");
        let out = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                ..Default::default()
            },
            |ctx| {
                crate::gui::apply_theme(ctx, &palette);
                egui::SidePanel::left("sidebar")
                    .exact_width(crate::gui::sidebar::SIDEBAR_WIDTH)
                    .resizable(false)
                    .show(ctx, |ui| {
                        let _ = crate::gui::sidebar::render(
                            ui,
                            &c,
                            &vm,
                            ThemeId::Github,
                            Some(0),
                            false,
                            false,
                        );
                    });
                egui::SidePanel::right("inspector")
                    .exact_width(280.0)
                    .resizable(false)
                    .show(ctx, |ui| {
                        inspector(ui, &c, &vm, 0);
                    });
                egui::TopBottomPanel::bottom("composer").show(ctx, |ui| {
                    let _ = composer(ui, &c, &mut session, true);
                });
                egui::CentralPanel::default().show(ctx, |ui| {
                    seat_header(ui, &c, &vm, 0);
                    ui.add_space(12.0);
                    stage_body(ui, &c, &vm, 0, &mut session);
                });
            },
        );
        let shapes = out.shapes.len();
        assert!(
            shapes > 5,
            "seat painted only {shapes} shapes; seat appears empty"
        );
    }
}
