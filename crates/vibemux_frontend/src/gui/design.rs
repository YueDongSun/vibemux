//! Shared visual primitives for the conversation workspace.
use super::C;
use egui::{self, Color32, Painter, Pos2, Rect, Response, RichText, Sense, Stroke, Ui, vec2};

#[derive(Clone, Copy)]
pub enum Icon {
    Chat,
    Grid,
    Terminal,
    Arrow,
    Settings,
    Refresh,
    Plus,
    SidebarToggle,
    Send,
    Close,
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
        Icon::Refresh => {
            painter.circle_stroke(rect.center(), rect.width() * 0.36, stroke);
            painter.line_segment([position(0.72, 0.05), position(0.86, 0.18)], stroke);
            painter.line_segment([position(0.86, 0.18), position(0.66, 0.26)], stroke);
        }
        Icon::Plus => {
            painter.line_segment([position(0.5, 0.15), position(0.5, 0.85)], stroke);
            painter.line_segment([position(0.15, 0.5), position(0.85, 0.5)], stroke);
        }
        Icon::SidebarToggle => {
            painter.rect_stroke(rect, 3, stroke, egui::StrokeKind::Inside);
            painter.line_segment([position(0.38, 0.0), position(0.38, 1.0)], stroke);
        }
        Icon::Send => {
            painter.line_segment([position(0.5, 0.85), position(0.5, 0.18)], stroke);
            painter.line_segment([position(0.22, 0.45), position(0.5, 0.18)], stroke);
            painter.line_segment([position(0.5, 0.18), position(0.78, 0.45)], stroke);
        }
        Icon::Close => {
            painter.line_segment([position(0.2, 0.2), position(0.8, 0.8)], stroke);
            painter.line_segment([position(0.8, 0.2), position(0.2, 0.8)], stroke);
        }
    }
}

pub const COMPOSER_RADIUS: u8 = 20;
pub const BUTTON_RADIUS: u8 = 8;
pub const CARD_RADIUS: u8 = 12;

/// Fade `rect` from transparent at the top to `color` at the bottom.
pub fn paint_bottom_fade(painter: &Painter, rect: Rect, color: Color32) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), Color32::TRANSPARENT);
    mesh.colored_vertex(rect.right_top(), Color32::TRANSPARENT);
    mesh.colored_vertex(rect.left_bottom(), color);
    mesh.colored_vertex(rect.right_bottom(), color);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    painter.add(egui::Shape::mesh(mesh));
}
const ICON_BUTTON_SIZE: f32 = 30.0;
const SPARK_SHORT_RAY: f32 = 0.62;
const SPARK_INNER_GAP: f32 = 0.16;
const SPARK_STROKE: f32 = 0.2;

/// Eight rays of a generic spark, alternating long and short, starting at
/// `angle` radians. It is not a reproduction of any trademarked logo.
#[must_use]
pub fn spark_segments(center: Pos2, radius: f32, angle: f32) -> [(Pos2, Pos2); 8] {
    std::array::from_fn(|index| {
        let direction = angle + index as f32 * std::f32::consts::FRAC_PI_4;
        let unit = vec2(direction.cos(), direction.sin());
        let length = if index % 2 == 0 {
            radius
        } else {
            radius * SPARK_SHORT_RAY
        };
        (
            center + unit * radius * SPARK_INNER_GAP,
            center + unit * length,
        )
    })
}

pub fn paint_spark(painter: &Painter, center: Pos2, radius: f32, angle: f32, color: Color32) {
    let width = (radius * SPARK_STROKE).max(1.2);
    for (inner, outer) in spark_segments(center, radius, angle) {
        painter.line_segment([inner, outer], Stroke::new(width, color));
        painter.circle_filled(outer, width / 2.0, color);
    }
}

pub fn spark(ui: &mut Ui, color: Color32, size: f32, angle: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    paint_spark(ui.painter(), rect.center(), size / 2.0, angle, color);
    response
}

/// A square icon button with a tooltip and an explicit, stable `id`.
pub fn icon_button(ui: &mut Ui, c: &C, kind: Icon, tooltip: &str, id: egui::Id) -> Response {
    let (_, rect) = ui.allocate_space(vec2(ICON_BUTTON_SIZE, ICON_BUTTON_SIZE));
    let response = ui.interact(rect, id, Sense::click());
    if response.hovered() || response.has_focus() {
        ui.painter().rect_filled(rect, BUTTON_RADIUS, c.raised);
    }
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(6.0)));
    icon(
        &mut child,
        kind,
        if response.hovered() { c.txt } else { c.muted },
        18.0,
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tooltip));
    response.on_hover_text(tooltip)
}

fn shadow_color(c: &C) -> Color32 {
    if c.light {
        Color32::from_black_alpha(18)
    } else {
        Color32::from_black_alpha(64)
    }
}

pub fn composer_frame(c: &C) -> egui::Frame {
    egui::Frame::new()
        .fill(c.raised)
        .stroke(Stroke::new(1.0, c.border))
        .corner_radius(COMPOSER_RADIUS)
        .inner_margin(egui::Margin::symmetric(18, 14))
        .shadow(egui::Shadow {
            offset: [0, 2],
            blur: 12,
            spread: 0,
            color: shadow_color(c),
        })
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
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spark_has_eight_rays_alternating_long_and_short() {
        let center = egui::pos2(50.0, 50.0);
        let segments = spark_segments(center, 10.0, 0.0);
        assert_eq!(segments.len(), 8);
        for (index, (inner, outer)) in segments.iter().enumerate() {
            let expected = if index % 2 == 0 {
                10.0
            } else {
                10.0 * SPARK_SHORT_RAY
            };
            assert!((outer.distance(center) - expected).abs() < 1e-3);
            assert!((inner.distance(center) - 10.0 * SPARK_INNER_GAP).abs() < 1e-3);
        }
        let rotated = spark_segments(center, 10.0, std::f32::consts::FRAC_PI_2);
        assert!((rotated[0].1.y - 60.0).abs() < 1e-3);
    }
}
