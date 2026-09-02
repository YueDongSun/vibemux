#![forbid(unsafe_code)]
//! Slim hairline top chrome shared by both shells: brand wordmark on
//! the left, theme switch and utility icons on the right.

use eframe::egui::{self, RichText, Sense, Ui, vec2};

use crate::theme::ThemeId;

use super::C;

/// What the top bar signals back to the app this frame.
pub struct TopActions {
    pub picked_theme: Option<ThemeId>,
    pub toggle_settings: bool,
    pub toggle_diag: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn render(
    ui: &mut Ui,
    c: &C,
    theme_id: ThemeId,
    workspace: &str,
    overview: bool,
    diag_open: bool,
    settings_open: bool,
) -> TopActions {
    let mut acts = TopActions {
        picked_theme: None,
        toggle_settings: false,
        toggle_diag: false,
    };

    // Brand mark: tiny accent square + wordmark.
    let (mark, _) = ui.allocate_exact_size(vec2(9.0, 9.0), Sense::hover());
    ui.painter()
        .rect_filled(mark, egui::CornerRadius::same(2), c.accent);
    ui.add_space(2.0);
    ui.label(RichText::new("VIBEMUX").color(c.muted).strong().size(11.0));
    ui.add_space(6.0);
    ui.label(RichText::new(workspace).color(c.faint).size(10.5));
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.add_space(4.0);

        // Diagnostics icon (outlined circle + inner dot).
        let diag_fill = if diag_open { c.accent_bg } else { c.bg };
        let (dr, dresp) = ui.allocate_exact_size(vec2(26.0, 22.0), Sense::click());
        if diag_fill != c.bg {
            ui.painter()
                .rect_filled(dr, egui::CornerRadius::same(6), diag_fill);
        }
        ui.painter().rect_stroke(
            dr.shrink(4.0),
            egui::CornerRadius::same(8),
            egui::Stroke::new(1.0, c.muted),
            egui::StrokeKind::Inside,
        );
        ui.painter().circle_filled(dr.center(), 1.6, c.muted);
        if dresp.clicked() {
            acts.toggle_diag = true;
        }

        // Settings icon (three horizontal rules).
        let set_fill = if settings_open { c.accent_bg } else { c.bg };
        let (sr, sresp) = ui.allocate_exact_size(vec2(26.0, 22.0), Sense::click());
        if set_fill != c.bg {
            ui.painter()
                .rect_filled(sr, egui::CornerRadius::same(6), set_fill);
        }
        for i in 0..3 {
            let y = sr.top() + 5.0 + i as f32 * 6.0;
            ui.painter().line_segment(
                [
                    egui::pos2(sr.left() + 6.0, y),
                    egui::pos2(sr.right() - 6.0, y),
                ],
                egui::Stroke::new(1.4, c.muted),
            );
        }
        if sresp.clicked() {
            acts.toggle_settings = true;
        }
        ui.add_space(2.0);

        // Theme switch: quiet segments.
        for candidate in ThemeId::ALL.iter().rev() {
            let active = *candidate == theme_id;
            let text = RichText::new(candidate.as_str())
                .color(if active { c.txt } else { c.faint })
                .size(10.5)
                .strong();
            let fill = if active { c.alt } else { c.bg };
            let stroke = if active {
                egui::Stroke::new(1.0, c.accent)
            } else {
                egui::Stroke::new(1.0, c.hair2)
            };
            if ui
                .add(
                    egui::Button::new(text)
                        .fill(fill)
                        .corner_radius(6)
                        .stroke(stroke)
                        .min_size(vec2(54.0, 22.0)),
                )
                .clicked()
            {
                acts.picked_theme = Some(*candidate);
            }
        }
        ui.add_space(6.0);

        // Shell mode hint.
        if overview {
            ui.label(
                RichText::new("overview")
                    .color(c.faint)
                    .size(9.5)
                    .monospace(),
            );
        } else {
            ui.label(
                RichText::new("seat · esc → overview")
                    .color(c.faint)
                    .size(9.5)
                    .monospace(),
            );
        }
    });
    acts
}
