//! Shared visual primitives for the conversation workspace.
use super::C;
use egui::{self, Align2, Color32, FontId, Response, RichText, Sense, Stroke, Ui, vec2};

#[derive(Clone, Copy)]
pub enum Icon {
    Chat,
    Grid,
    Terminal,
    Arrow,
    Settings,
    Activity,
}

pub fn icon(ui: &mut Ui, kind: Icon, color: Color32, size: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let rect = rect.shrink(2.0);
    let painter = ui.painter();
    let stroke = Stroke::new(1.5, color);
    let position = |x: f32, y: f32| {
        egui::pos2(
            rect.left() + rect.width() * x,
            rect.top() + rect.height() * y,
        )
    };
    match kind {
        Icon::Chat => {
            painter.rect_stroke(rect, 3, stroke, egui::StrokeKind::Inside);
            painter.line_segment([position(0.22, 0.35), position(0.78, 0.35)], stroke);
            painter.line_segment([position(0.22, 0.62), position(0.6, 0.62)], stroke);
        }
        Icon::Grid => {
            for x in [0.0, 0.58] {
                for y in [0.0, 0.58] {
                    painter.rect_stroke(
                        egui::Rect::from_min_size(position(x, y), rect.size() * 0.4),
                        2,
                        stroke,
                        egui::StrokeKind::Inside,
                    );
                }
            }
        }
        Icon::Terminal => {
            painter.line_segment([position(0.1, 0.25), position(0.4, 0.5)], stroke);
            painter.line_segment([position(0.4, 0.5), position(0.1, 0.75)], stroke);
            painter.line_segment([position(0.52, 0.75), position(0.9, 0.75)], stroke);
        }
        Icon::Arrow => {
            painter.line_segment([position(0.15, 0.85), position(0.85, 0.15)], stroke);
            painter.line_segment([position(0.4, 0.15), position(0.85, 0.15)], stroke);
            painter.line_segment([position(0.85, 0.15), position(0.85, 0.6)], stroke);
        }
        Icon::Settings => {
            painter.circle_stroke(rect.center(), rect.width() * 0.38, stroke);
            painter.circle_stroke(rect.center(), rect.width() * 0.12, stroke);
        }
        Icon::Activity => {
            painter.line_segment([position(0.1, 0.5), position(0.3, 0.5)], stroke);
            painter.line_segment([position(0.3, 0.5), position(0.45, 0.1)], stroke);
            painter.line_segment([position(0.45, 0.1), position(0.65, 0.85)], stroke);
            painter.line_segment([position(0.65, 0.85), position(0.8, 0.5)], stroke);
            painter.line_segment([position(0.8, 0.5), position(0.95, 0.5)], stroke);
        }
    }
}

pub fn nav_row(ui: &mut Ui, c: &C, label: &str, kind: Icon, selected: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
    if selected || response.hovered() {
        ui.painter()
            .rect_filled(rect, 7, if selected { c.accent_bg } else { c.raised });
    }
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(vec2(10.0, 7.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    icon(
        &mut child,
        kind,
        if selected { c.accent } else { c.muted },
        19.0,
    );
    child.add_space(7.0);
    child.label(
        RichText::new(label)
            .size(14.0)
            .color(if selected { c.txt } else { c.muted }),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, label)
    });
    response.clicked() || (response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
}

pub fn quiet_button(ui: &mut Ui, c: &C, label: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(label).size(13.0).color(c.muted))
            .frame(false)
            .corner_radius(6),
    )
}

pub fn status(ui: &mut Ui, c: &C, state: &str) {
    let color = state_color(c, state);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(7.0, 7.0), Sense::hover());
        ui.painter().circle_filled(rect.center(), 3.0, color);
        ui.label(
            RichText::new(state_text(state))
                .size(12.0)
                .color(state_text_color(c, state)),
        );
    });
}
pub fn state_color(c: &C, state: &str) -> Color32 {
    match state {
        "done" | "succeeded" => c.ok,
        "blocked" | "input_required" => c.warn,
        "failed" => c.danger,
        "in_progress" | "running" => c.accent,
        _ => c.muted,
    }
}
/// Text color for a state label; accent states use the readable accent.
pub fn state_text_color(c: &C, state: &str) -> Color32 {
    match state {
        "in_progress" | "running" => c.accent_text,
        _ => state_color(c, state),
    }
}
pub fn state_text(state: &str) -> String {
    match state {
        "in_progress" => "In progress".into(),
        "done" => "Completed".into(),
        "open" => "Open".into(),
        "input_required" => "Needs input".into(),
        other => {
            let mut chars = other.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().to_string() + chars.as_str())
                .unwrap_or_else(|| "Unknown".into())
        }
    }
}
pub fn avatar(ui: &mut Ui, c: &C, label: &str, size: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    ui.painter().rect_filled(rect, 8, c.accent_bg);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(size * 0.42),
        c.accent,
    );
}
