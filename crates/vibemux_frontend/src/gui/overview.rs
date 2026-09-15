#![forbid(unsafe_code)]
//! Overview shell (canvas): an editorial landing listing every harness
//! as a hairline row plus a system-facts strip. Pure painter layout so
//! it does not depend on glyph coverage in egui's default fonts.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Ui, vec2};

use crate::view_model::{ViewModel, route_label, state_label};

use super::{C, monogram};

pub fn render(ui: &mut Ui, c: &C, vm: &ViewModel, on_open: &mut dyn FnMut(usize)) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let avail = ui.available_width();
        // Centered editorial column.
        let col_w = avail.min(960.0);
        let pad = ((avail - col_w) * 0.5).max(0.0);
        ui.add_space(pad.min(70.0));
        ui.add_space(26.0);

        // Kicker + headline.
        ui.label(RichText::new("COMMAND").color(c.accent).size(10.0).strong().monospace());
        ui.add_space(8.0);
        ui.label(
            RichText::new("Five agents, one machine.")
                .color(c.txt)
                .size(30.0)
                .strong(),
        );
        ui.add_space(6.0);
        ui.label(
            RichText::new(
                "A quiet place to see what each coding agent is doing, and to sit down at any one of them.",
            )
            .color(c.muted)
            .size(13.0),
        );
        ui.add_space(26.0);

        // Rows (hairline only, no boxes).
        for (idx, agent) in vm.agents.iter().enumerate() {
            if row(ui, c, idx, agent) {
                on_open(idx);
            }
        }
        ui.add_space(6.0);

        // System facts strip.
        system_strip(ui, c, vm);
        ui.add_space(40.0);
    });
}

fn row(ui: &mut Ui, c: &C, idx: usize, agent: &crate::view_model::AgentView) -> bool {
    let h = 58.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
    let p = ui.painter_at(rect);

    // Hover underlay (very subtle).
    if resp.hovered() || resp.highlighted() {
        p.rect_filled(
            rect,
            egui::CornerRadius::same(6),
            Color32::from_rgba_unmultiplied(255, 255, 255, 8),
        );
    }

    let cx = rect.center().y;

    // Monogram tile.
    let tile = Rect::from_center_size(Pos2::new(rect.left() + 34.0, cx), vec2(38.0, 38.0));
    p.rect_filled(tile, egui::CornerRadius::same(8), c.alt);
    p.text(
        tile.center(),
        Align2::CENTER_CENTER,
        monogram(idx),
        FontId::monospace(16.0),
        c.accent_2,
    );

    // Name + bin.
    p.text(
        Pos2::new(rect.left() + 66.0, cx - 9.0),
        Align2::LEFT_BOTTOM,
        agent.name.clone(),
        FontId::proportional(13.5),
        c.txt,
    );
    p.text(
        Pos2::new(rect.left() + 66.0, cx + 9.0),
        Align2::LEFT_TOP,
        line_bin(agent),
        FontId::monospace(9.5),
        c.faint,
    );

    // State (dot + word).
    let verified = matches!(agent.launcher_state, vibemux_probe::ProbeState::Verified);
    let dot_c = if verified { c.accent } else { c.faint };
    let dot_x = rect.left() + 260.0;
    let state_txt = state_label(agent.launcher_state);
    p.circle_filled(Pos2::new(dot_x, cx), 3.2, dot_c);
    p.text(
        Pos2::new(dot_x + 12.0, cx),
        Align2::LEFT_CENTER,
        state_txt,
        FontId::monospace(10.5),
        if verified { c.muted } else { c.faint },
    );

    // Live excerpt (mono), truncated to avoid colliding with the right
    // meta block.
    let mut line = excerpt(agent);
    line.truncate(52);
    p.text(
        Pos2::new(rect.left() + 372.0, cx),
        Align2::LEFT_CENTER,
        line,
        FontId::monospace(11.0),
        c.muted,
    );

    // Right aligned: route + version in a small fixed-width block.
    let block_right = rect.right() - 14.0;
    p.text(
        Pos2::new(block_right, cx),
        Align2::RIGHT_CENTER,
        format!("v{}", agent.version),
        FontId::monospace(10.0),
        c.faint,
    );
    p.text(
        Pos2::new(block_right - 64.0, cx),
        Align2::RIGHT_CENTER,
        route_label(agent.route),
        FontId::monospace(10.0),
        c.faint,
    );

    // Hairline under row.
    p.line_segment(
        [
            Pos2::new(rect.left() + 66.0, rect.bottom()),
            Pos2::new(rect.right() - 6.0, rect.bottom()),
        ],
        egui::Stroke::new(1.0, c.hair2),
    );

    resp.clicked()
}

fn line_bin(agent: &crate::view_model::AgentView) -> String {
    format!(
        "{} · v{} · {}",
        agent.name.to_lowercase(),
        agent.version,
        route_label(agent.route)
    )
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

fn system_strip(ui: &mut Ui, c: &C, vm: &ViewModel) {
    let top = ui.cursor().top();
    let _ = top;
    ui.add_space(14.0);
    ui.separator();
    ui.add_space(12.0);

    // Three equal columns.
    let cols = [
        ("telemetry", &vm.telemetry_summary),
        ("gateway", &vm.gateway_summary),
        (
            "runtime",
            &format!("schema {} · {}", vm.schema_version, vm.platform),
        ),
    ];
    let mut laid = [false; 3];
    ui.columns(3, |col_uis| {
        for (i, (title, body)) in cols.iter().enumerate() {
            let cui = &mut col_uis[i];
            cui.add_space(2.0);
            cui.label(
                RichText::new(title.to_uppercase())
                    .color(c.muted)
                    .size(9.0)
                    .strong()
                    .monospace(),
            );
            cui.add_space(4.0);
            for line in body.split("health=").collect::<Vec<_>>() {
                let text =
                    if body.contains("health=") && line != body.split("health=").next().unwrap() {
                        format!("health={line}")
                    } else {
                        line.to_string()
                    };
                cui.label(
                    RichText::new(text.trim())
                        .color(c.faint)
                        .size(10.0)
                        .monospace(),
                );
            }
            laid[i] = true;
        }
    });
    let _ = laid;
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

    /// Headless render of the overview inside a CentralPanel, mirroring
    /// the top-level structure of `VibeMuxApp::update`, then count the
    /// text shapes painted into the background layer. If the overview
    /// body silently paints nothing (the "empty body" bug), this fails.
    #[test]
    fn overview_body_paints_text_shapes() {
        let vm = crate::view_model::ViewModel::from_report(&fixture_report());
        let palette = palette_for(ThemeId::Claude);
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
                // Mirror VibeMuxApp::update exactly, theme included.
                super::super::apply_theme(ctx, &palette);
                egui::TopBottomPanel::top("chrome").show(ctx, |ui| {
                    ui.add_space(5.0);
                    let _ = super::super::topbar::render(
                        ui,
                        &c,
                        ThemeId::Claude,
                        "E:\\lab\\vibemux",
                        true,
                        false,
                        false,
                    );
                    ui.add_space(5.0);
                });
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::new()
                            .fill(c.bg)
                            .inner_margin(egui::Margin::symmetric(120, 12)),
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

    /// Headless render of the workbench (rail + inspector + composer +
    /// stage) mirroring `VibeMuxApp::render_workbench`. Guards against
    /// the same class of silent-empty-render bugs in the seat shell.
    #[test]
    fn workbench_body_paints_text_shapes() {
        let vm = crate::view_model::ViewModel::from_report(&fixture_report());
        let palette = palette_for(ThemeId::Claude);
        let c = super::super::pal(&palette);
        let ctx = egui::Context::default();
        let mut session = super::super::workbench::Session::new("stub line one\nstub line two\n");
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
                egui::SidePanel::left("rail")
                    .exact_width(64.0)
                    .resizable(false)
                    .show(ctx, |ui| {
                        let _ = super::super::workbench::rail(ui, &c, &vm, Some(0));
                    });
                egui::SidePanel::right("inspector")
                    .exact_width(280.0)
                    .resizable(false)
                    .show(ctx, |ui| {
                        super::super::workbench::inspector(ui, &c, &vm, 0);
                    });
                egui::TopBottomPanel::bottom("composer").show(ctx, |ui| {
                    ui.add_space(6.0);
                    let _ = super::super::workbench::composer(ui, &c, &mut session, true);
                    ui.add_space(6.0);
                });
                egui::CentralPanel::default().show(ctx, |ui| {
                    super::super::workbench::stage_body(ui, &c, &vm, 0, &mut session);
                });
            },
        );
        let shapes = out.shapes.len();
        assert!(
            shapes > 5,
            "workbench painted only {shapes} shapes; seat appears empty"
        );
    }
}
