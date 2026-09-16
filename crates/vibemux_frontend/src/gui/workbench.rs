#![forbid(unsafe_code)]
//! Workbench shell (seat): a narrow icon rail of harnesses, a large
//! monospace terminal "stage" for the focused harness, and a right-hand
//! inspector. Pure painter + egui widgets; no exotic glyphs.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Ui, vec2};

use crate::view_model::{ViewModel, route_label, state_label};

use super::{C, monogram};

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

/// User intent from the rail.
pub struct RailOut {
    pub open: Option<usize>,
    pub to_overview: bool,
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

pub fn rail(ui: &mut Ui, c: &C, vm: &ViewModel, cur: Option<usize>) -> RailOut {
    let mut out = RailOut {
        open: None,
        to_overview: false,
    };

    // Top: "overview" glyph (2x2 grid of squares) returns to overview.
    let (r0, r0resp) = ui.allocate_exact_size(vec2(38.0, 38.0), Sense::click());
    if r0resp.hovered() {
        ui.painter().rect_filled(
            r0,
            egui::CornerRadius::same(9),
            Color32::from_rgba_unmultiplied(255, 255, 255, 10),
        );
    }
    for i in 0..2 {
        for j in 0..2 {
            let s = 5.0;
            let x = r0.left() + 9.0 + i as f32 * (s + 8.0);
            let y = r0.top() + 9.0 + j as f32 * (s + 8.0);
            ui.painter().rect_filled(
                Rect::from_min_size(Pos2::new(x, y), vec2(s, s)),
                egui::CornerRadius::same(1),
                if cur.is_none() { c.accent } else { c.faint },
            );
        }
    }
    if r0resp.clicked() {
        out.to_overview = true;
    }
    r0resp.on_hover_text("Overview (all agents)");
    ui.add_space(10.0);

    for (idx, agent) in vm.agents.iter().enumerate() {
        let active = cur == Some(idx);
        let (rect, resp) = ui.allocate_exact_size(vec2(38.0, 38.0), Sense::click());
        let base = if active {
            c.accent_bg
        } else if resp.hovered() {
            Color32::from_rgba_unmultiplied(255, 255, 255, 8)
        } else {
            c.bg
        };
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(9), base);
        if active {
            ui.painter().rect_stroke(
                rect,
                egui::CornerRadius::same(9),
                Stroke::new(1.0, c.accent),
                egui::StrokeKind::Inside,
            );
        }
        let color = if active { c.accent } else { c.faint };
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            monogram(idx),
            FontId::monospace(15.0),
            color,
        );
        let verified = matches!(agent.launcher_state, vibemux_probe::ProbeState::Verified);
        ui.painter().circle_filled(
            Pos2::new(rect.right() - 4.0, rect.bottom() - 4.0),
            3.4,
            if verified { c.ok } else { c.faint },
        );
        if resp.clicked() {
            out.open = Some(idx);
        }
        let _ = resp.on_hover_text(format!("{} (Ctrl+{})", agent.name, seat_accelerator(idx)));
        ui.add_space(6.0);
    }

    out
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
                RichText::new(agent.name.to_uppercase())
                    .color(c.accent)
                    .size(9.5)
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
            } else if matches!(v, "verified") {
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
        Stroke::new(1.0, c.hair2),
    );
}

/// Bottom composer (Send / interrupt / Clear). Returns true if the user
/// asked to clear the transcript.
pub fn composer(ui: &mut Ui, c: &C, session: &mut Session, can_send: bool) -> bool {
    let mut cleared = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(">").color(c.accent).size(14.0).monospace());
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
                egui::Button::new(
                    RichText::new("Send")
                        .color(Color32::from_rgb(28, 14, 9))
                        .strong(),
                )
                .fill(c.accent)
                .stroke(Stroke::NONE),
            )
            .clicked()
        {
            let line = session.composer.trim().to_string();
            push(&mut session.text, &format!("you> {line}"));
            session.composer.clear();
        }
        if ui
            .add(egui::Button::new("Ctrl-C").stroke(Stroke::new(1.0, c.hair)))
            .clicked()
        {
            push(&mut session.text, "^C (interrupt)");
        }
        if ui
            .add(egui::Button::new("Clear").stroke(Stroke::new(1.0, c.hair)))
            .clicked()
        {
            cleared = true;
        }
    });
    cleared
}

/// Stage header + terminal body. Rendered inside the central panel.
pub fn stage_body(ui: &mut Ui, c: &C, vm: &ViewModel, idx: usize, session: &mut Session) {
    let Some(agent) = vm.agents.get(idx) else {
        return;
    };

    // Header row.
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(agent.name.clone())
                .color(c.txt)
                .size(16.0)
                .strong(),
        );
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!(
                "{} · v{}",
                agent.name.to_lowercase(),
                agent.version
            ))
            .color(c.faint)
            .size(10.5)
            .monospace(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let pill = ui.add_enabled(
                true,
                egui::Button::new(RichText::new(" ● ready").color(c.ok).size(10.0).monospace())
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::new(1.0, c.hair2))
                    .corner_radius(12),
            );
            let _ = pill;
        });
    });
    ui.add_space(6.0);
    ui.separator();
    ui.add_space(6.0);

    // Terminal block.
    let height = (ui.available_height() - 8.0).max(120.0);
    let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area));
    {
        let painter = child.painter();
        painter.rect_filled(area, egui::CornerRadius::same(10), c.term_bg);
        painter.rect_stroke(
            area,
            egui::CornerRadius::same(10),
            Stroke::new(1.0, c.hair2),
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
}
