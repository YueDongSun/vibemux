#![forbid(unsafe_code)]
//! The Claude-style section page shared by Settings and the Console: a
//! section list on the left and a scrolling column of rounded cards on the
//! right, each card made of rows with a label on the left and a value or
//! control on the right.

use eframe::egui::{self, RichText, Sense, Stroke, Ui, vec2};

use super::{C, design, typography};

/// Width of the section list.
pub const NAV_WIDTH: f32 = 196.0;
/// The content column never grows wider than this.
pub const CONTENT_MAX_WIDTH: f32 = 720.0;
/// Width reserved for a row's control.
const CONTROL_WIDTH: f32 = 300.0;
/// In a narrow column the control gets at most this share of a row; the
/// window size pair in Settings needs about 240 px.
const CONTROL_SHARE: f32 = 0.6;

/// Draws the page: `nav` fills the section list, `body` the scrolling
/// content column under `title`.
pub fn shell(
    ui: &mut Ui,
    c: &C,
    scroll_id: &str,
    title: &str,
    nav: impl FnOnce(&mut Ui),
    body: impl FnOnce(&mut Ui),
) {
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            vec2(NAV_WIDTH, ui.available_height()),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(NAV_WIDTH);
                nav(ui);
            },
        );
        ui.add_space(32.0);
        egui::ScrollArea::vertical()
            .id_salt(scroll_id)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // The scroll area sits in a horizontal row; its content is a
                // top-down column of its own.
                ui.vertical(|ui| {
                    // `set_max_width` can widen past the viewport; a narrow
                    // window would then clip every right-aligned value.
                    ui.set_max_width(CONTENT_MAX_WIDTH.min(ui.available_width()));
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(title)
                            .font(typography::display_font(ui.ctx(), typography::TITLE_SIZE))
                            .color(c.txt),
                    );
                    ui.add_space(18.0);
                    body(ui);
                    ui.add_space(24.0);
                });
            });
    });
}

/// One entry of the section list. Returns true when it is chosen.
pub fn nav_item(ui: &mut Ui, c: &C, label: &str, selected: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
    if selected || response.hovered() {
        ui.painter().rect_filled(
            rect,
            design::BUTTON_RADIUS,
            if selected { c.accent_bg } else { c.raised },
        );
    }
    ui.painter().text(
        rect.left_center() + vec2(12.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.0),
        if selected { c.txt } else { c.muted },
    );
    design::paint_focus_ring(ui, &response, design::BUTTON_RADIUS, c);
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, label)
    });
    response.clicked()
        || (response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)))
}

pub fn sub_heading(ui: &mut Ui, c: &C, text: &str) {
    ui.label(RichText::new(text).size(15.0).strong().color(c.txt));
    ui.add_space(10.0);
}

pub fn footnote(ui: &mut Ui, c: &C, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).size(12.5).color(c.muted)).wrap());
}

pub fn card(ui: &mut Ui, c: &C, body: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(c.raised)
        .stroke(Stroke::new(1.0, c.border))
        .corner_radius(design::CARD_RADIUS)
        .inner_margin(egui::Margin::symmetric(20, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            body(ui);
        });
}

pub fn separator(ui: &mut Ui, c: &C) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter()
        .hline(rect.x_range(), rect.center().y, Stroke::new(1.0, c.border));
}

/// One row: title and description on the left; `control` draws in a
/// right-to-left area on the right, so it adds its widgets from the right.
pub fn row(ui: &mut Ui, c: &C, title: &str, description: &str, control: impl FnOnce(&mut Ui)) {
    ui.add_space(12.0);
    let total = ui.available_width();
    let control_width = CONTROL_WIDTH.min(total * CONTROL_SHARE);
    // The gap between the two columns counts too, or every row would widen
    // its card by one item spacing.
    let label_width = (total - control_width - ui.spacing().item_spacing.x).max(0.0);
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            vec2(label_width, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(label_width);
                ui.label(RichText::new(title).size(14.0).color(c.txt));
                if !description.is_empty() {
                    ui.add_space(2.0);
                    ui.label(RichText::new(description).size(12.5).color(c.muted));
                }
            },
        );
        ui.allocate_ui_with_layout(
            vec2(control_width, 0.0),
            egui::Layout::right_to_left(egui::Align::Center),
            control,
        );
    });
    ui.add_space(12.0);
}

/// A read-only row whose value is selectable text on the right.
pub fn value_row(ui: &mut Ui, c: &C, title: &str, value: &str) {
    row(ui, c, title, "", |ui| {
        ui.add(
            egui::Label::new(RichText::new(value).size(13.0).color(c.muted))
                .wrap()
                .selectable(true),
        );
    });
}

/// Rows separated by hairlines inside one card.
pub fn value_card(ui: &mut Ui, c: &C, rows: &[(&str, String)]) {
    card(ui, c, |ui| {
        for (index, (title, value)) in rows.iter().enumerate() {
            if index > 0 {
                separator(ui, c);
            }
            value_row(ui, c, title, value);
        }
    });
}
