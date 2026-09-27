#![forbid(unsafe_code)]
//! Overview content view (ADR 027 shell): a GitHub-dashboard-style
//! page — page header with a count badge, a row of bordered summary
//! cards, and the agent table inside a bordered Primer box with
//! hairline row separators. Pure painter rows so it does not depend on
//! glyph coverage in egui's default fonts.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Ui, vec2};

use crate::view_model::{ViewModel, route_label, state_label};

use super::{C, monogram};

pub fn render(ui: &mut Ui, c: &C, vm: &ViewModel, on_open: &mut dyn FnMut(usize)) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            page_header(ui, c, vm);
            ui.add_space(14.0);
            summary_cards(ui, c, vm);
            ui.add_space(14.0);
            agents_box(ui, c, vm, on_open);
            ui.add_space(24.0);
        });
}

fn page_header(ui: &mut Ui, c: &C, vm: &ViewModel) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Overview").color(c.txt).size(17.0).strong());
        ui.add_space(6.0);
        badge(
            ui,
            &format!("{} agents", vm.agents.len()),
            c.muted,
            c.border,
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(format!("{} · observed {}", vm.platform, vm.observed_at))
                    .color(c.faint)
                    .size(10.0)
                    .monospace(),
            );
        });
    });
    ui.add_space(2.0);
    ui.label(
        RichText::new("A quiet place to see what each coding agent is doing, and to sit down at any one of them.")
            .color(c.muted)
            .size(12.0),
    );
}

/// One bordered Primer card: uppercase mono label on top, mono body.
fn summary_cards(ui: &mut Ui, c: &C, vm: &ViewModel) {
    let gap = 10.0;
    let width = (ui.available_width() - 3.0 * gap) / 4.0;
    let agent_color = if vm
        .agents
        .iter()
        .any(|a| matches!(a.launcher_state, vibemux_probe::ProbeState::Failed))
        || vm.gateway_state == vibemux_probe::ProbeState::Failed
    {
        c.danger
    } else if vm.health == crate::view_model::Health::Warning {
        c.warn
    } else {
        c.ok
    };
    let cards = [
        ("agents", agent_card_body(vm), agent_color),
        (
            "gateway",
            vm.gateway_summary.clone(),
            state_color(c, vm.gateway_state),
        ),
        ("a2a", vm.a2a_summary.clone(), state_color(c, vm.a2a_state)),
        (
            "telemetry",
            vm.telemetry_summary.clone(),
            if vm.telemetry_available && vm.telemetry_failures == 0 {
                c.ok
            } else {
                c.muted
            },
        ),
    ];
    ui.horizontal(|ui| {
        for (label, body, accent) in cards {
            let (rect, _) = ui.allocate_exact_size(vec2(width, 64.0), Sense::hover());
            let p = ui.painter_at(rect);
            p.rect_filled(rect, egui::CornerRadius::same(6), c.surf);
            p.rect_stroke(
                rect,
                egui::CornerRadius::same(6),
                Stroke::new(1.0, c.border),
                egui::StrokeKind::Inside,
            );
            p.text(
                Pos2::new(rect.left() + 12.0, rect.top() + 18.0),
                Align2::LEFT_CENTER,
                label.to_uppercase(),
                FontId::monospace(9.0),
                c.muted,
            );
            p.text(
                Pos2::new(rect.left() + 12.0, rect.top() + 40.0),
                Align2::LEFT_CENTER,
                truncate(&body, (width - 24.0) / 6.0),
                FontId::monospace(10.5),
                accent,
            );
            ui.add_space(gap);
        }
    });
}

fn agent_card_body(vm: &ViewModel) -> String {
    let mut verified = 0;
    let mut failed = 0;
    let mut other = 0;
    for agent in &vm.agents {
        match agent.launcher_state {
            vibemux_probe::ProbeState::Verified => verified += 1,
            vibemux_probe::ProbeState::Failed => failed += 1,
            _ => other += 1,
        }
    }
    format!("{verified} verified · {failed} failed · {other} other")
}

fn state_color(c: &C, state: vibemux_probe::ProbeState) -> Color32 {
    match state {
        vibemux_probe::ProbeState::Verified => c.ok,
        vibemux_probe::ProbeState::Failed => c.danger,
        vibemux_probe::ProbeState::Unavailable => c.warn,
        vibemux_probe::ProbeState::NotRun => c.muted,
    }
}

/// Bordered Primer box with a header strip; the closure draws the body.
fn primer_box<R>(
    ui: &mut Ui,
    c: &C,
    header: &str,
    hint: &str,
    body: impl FnOnce(&mut Ui) -> R,
) -> R {
    egui::Frame::new()
        .fill(c.surf)
        .stroke(Stroke::new(1.0, c.border))
        .corner_radius(6)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            // Header strip.
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.label(
                    RichText::new(header.to_uppercase())
                        .color(c.txt)
                        .size(10.0)
                        .strong()
                        .monospace(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(12.0);
                    ui.label(RichText::new(hint).color(c.faint).size(9.5).monospace());
                });
            });
            ui.add_space(2.0);
            body(ui)
        })
        .inner
}

fn agents_box(ui: &mut Ui, c: &C, vm: &ViewModel, on_open: &mut dyn FnMut(usize)) {
    primer_box(ui, c, "Agents", "ctrl+1..0 opens a seat", |ui| {
        for (idx, agent) in vm.agents.iter().enumerate() {
            if agent_row(ui, c, idx, agent) {
                on_open(idx);
            }
        }
    });
}

fn agent_row(ui: &mut Ui, c: &C, idx: usize, agent: &crate::view_model::AgentView) -> bool {
    let h = 44.0;
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, h), Sense::click());
    let p = ui.painter_at(rect);

    // GitHub table row hover.
    if resp.hovered() || resp.highlighted() {
        p.rect_filled(rect, egui::CornerRadius::same(0), c.alt);
    }

    let cx = rect.center().y;

    // Monogram tile.
    let tile = Rect::from_center_size(Pos2::new(rect.left() + 28.0, cx), vec2(28.0, 28.0));
    p.rect_filled(tile, egui::CornerRadius::same(6), c.alt);
    p.rect_stroke(
        tile,
        egui::CornerRadius::same(6),
        Stroke::new(1.0, c.border),
        egui::StrokeKind::Inside,
    );
    p.text(
        tile.center(),
        Align2::CENTER_CENTER,
        monogram(idx),
        FontId::monospace(12.0),
        c.muted,
    );

    // Name + excerpt under it.
    p.text(
        Pos2::new(rect.left() + 54.0, cx - 8.0),
        Align2::LEFT_BOTTOM,
        agent.name.clone(),
        FontId::proportional(13.0),
        c.txt,
    );
    p.text(
        Pos2::new(rect.left() + 54.0, cx + 8.0),
        Align2::LEFT_TOP,
        truncate(&excerpt(agent), 46.0),
        FontId::monospace(9.5),
        c.faint,
    );

    // State badge pill.
    let label = state_label(agent.launcher_state);
    let (text_color, border_color) = match agent.launcher_state {
        vibemux_probe::ProbeState::Verified => (c.ok, c.ok),
        vibemux_probe::ProbeState::Failed => (c.danger, c.danger),
        vibemux_probe::ProbeState::Unavailable => (c.warn, c.warn),
        vibemux_probe::ProbeState::NotRun => (c.muted, c.border),
    };
    let badge_w = 82.0;
    let badge = Rect::from_center_size(Pos2::new(rect.left() + 360.0, cx), vec2(badge_w, 20.0));
    p.rect_filled(badge, egui::CornerRadius::same(10), Color32::TRANSPARENT);
    p.rect_stroke(
        badge,
        egui::CornerRadius::same(10),
        Stroke::new(1.0, border_color),
        egui::StrokeKind::Inside,
    );
    p.text(
        badge.center(),
        Align2::CENTER_CENTER,
        format!("● {label}"),
        FontId::monospace(9.5),
        text_color,
    );

    // Right aligned: route + version as one truncated string so long
    // version strings never collide with the route label.
    let meta = truncate(
        &format!("{} \u{b7} v{}", route_label(agent.route), agent.version),
        34.0,
    );
    p.text(
        Pos2::new(rect.right() - 16.0, cx),
        Align2::RIGHT_CENTER,
        meta,
        FontId::monospace(10.0),
        c.faint,
    );

    // Hairline under the row, inset like GitHub table separators.
    p.line_segment(
        [
            Pos2::new(rect.left() + 54.0, rect.bottom()),
            Pos2::new(rect.right() - 16.0, rect.bottom()),
        ],
        Stroke::new(1.0, c.border.gamma_multiply(0.55)),
    );

    resp.clicked()
}

/// Small pill with bordered text, used in the page header.
fn badge(ui: &mut Ui, text: &str, text_color: Color32, border: Color32) {
    let w = 12.0 + text.chars().count() as f32 * 6.2;
    let (rect, _) = ui.allocate_exact_size(vec2(w, 20.0), Sense::hover());
    let p = ui.painter();
    p.rect_stroke(
        rect,
        egui::CornerRadius::same(10),
        Stroke::new(1.0, border),
        egui::StrokeKind::Inside,
    );
    p.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::monospace(9.5),
        text_color,
    );
}

fn excerpt(agent: &crate::view_model::AgentView) -> String {
    let auth = state_label(agent.authentication_state);
    let inf = state_label(agent.inference_state);
    if matches!(agent.launcher_state, vibemux_probe::ProbeState::Verified) {
        if inf == "not_run" {
            "ready — inference not run yet".to_string()
        } else {
            format!("ready — auth {auth}, inference {inf}")
        }
    } else {
        "launcher unavailable".to_string()
    }
}

fn truncate(text: &str, max_chars: f32) -> String {
    let max = (max_chars.max(8.0)) as usize;
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let cut: String = text.chars().take(max - 1).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeId;
    use crate::theme::palette_for;
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

    /// Headless render of the redesigned shell mirroring the top-level
    /// structure of `VibeMuxApp::update` (sidebar + central overview),
    /// then count the shapes painted. If the overview body silently
    /// paints nothing (the "empty body" bug class), this fails.
    #[test]
    fn overview_body_paints_text_shapes() {
        let vm = crate::view_model::ViewModel::from_report(&fixture_report());
        let palette = palette_for(ThemeId::Github);
        let c = super::super::pal(&palette);
        let ctx = egui::Context::default();
        let mut opened: Option<usize> = None;
        let out = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                ..Default::default()
            },
            |ctx| {
                super::super::apply_theme(ctx, &palette);
                egui::SidePanel::left("sidebar")
                    .exact_width(super::super::sidebar::SIDEBAR_WIDTH)
                    .resizable(false)
                    .show(ctx, |ui| {
                        let _ = super::super::sidebar::render(
                            ui,
                            &c,
                            &vm,
                            ThemeId::Github,
                            None,
                            false,
                            false,
                        );
                    });
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::new()
                            .fill(c.bg)
                            .inner_margin(egui::Margin::same(20)),
                    )
                    .show(ctx, |ui| {
                        render(ui, &c, &vm, &mut |i| opened = Some(i));
                    });
            },
        );
        let shapes = out.shapes.len();
        assert!(
            shapes > 5,
            "overview painted only {shapes} shapes; body appears empty"
        );
        assert!(opened.is_none(), "no row should be clicked headless");
    }
}
