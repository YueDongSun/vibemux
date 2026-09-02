#![forbid(unsafe_code)]
//! Theme application: convert `ThemePalette` (hex strings) into egui
//! visual style.

use eframe::egui::{self, Color32, Context, Style, Visuals};

use crate::theme::ThemePalette;

/// Apply the active palette to the egui context. Idempotent: safe to
/// call on every frame.
pub fn apply_theme(ctx: &Context, palette: &ThemePalette) {
    let visuals = build_visuals(palette);
    let mut style = Style {
        visuals,
        ..Default::default()
    };
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 4.0);
    ctx.set_style(style);
    // Let egui honor the OS display scale (do not pin pixels_per_point
    // to 1.0): on high-DPI monitors the UI would otherwise render far
    // too small.
}

fn build_visuals(p: &ThemePalette) -> Visuals {
    let mut visuals = Visuals::dark();
    visuals.dark_mode = true;
    visuals.override_text_color = Some(color(&p.text_primary));
    visuals.hyperlink_color = color(&p.accent);

    visuals.widgets.noninteractive.bg_fill = color(&p.surface);
    visuals.widgets.noninteractive.weak_bg_fill = color(&p.surface);
    visuals.widgets.noninteractive.bg_stroke.color = color(&p.border);
    visuals.widgets.noninteractive.bg_stroke.width = p.border_width;
    visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::same(p.corner_radius as u8);

    visuals.widgets.inactive.bg_fill = color(&p.surface_alt);
    visuals.widgets.inactive.weak_bg_fill = color(&p.surface_alt);
    visuals.widgets.inactive.bg_stroke.color = color(&p.border);
    visuals.widgets.inactive.bg_stroke.width = p.border_width;
    visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(p.corner_radius as u8);

    visuals.widgets.hovered.bg_fill = color(&p.surface_alt);
    visuals.widgets.hovered.weak_bg_fill = color(&p.surface_alt);
    visuals.widgets.hovered.bg_stroke.color = color(&p.accent);
    visuals.widgets.hovered.bg_stroke.width = p.border_width;
    visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(p.corner_radius as u8);

    visuals.widgets.active.bg_fill = color(&p.accent);
    visuals.widgets.active.weak_bg_fill = color(&p.accent);
    visuals.widgets.active.bg_stroke.color = color(&p.accent);
    visuals.widgets.active.bg_stroke.width = p.border_width;
    visuals.widgets.active.corner_radius = egui::CornerRadius::same(p.corner_radius as u8);

    visuals.selection.bg_fill = color(&p.accent).linear_multiply(0.4);
    visuals.selection.stroke.color = color(&p.accent);
    visuals.selection.stroke.width = 1.0;

    visuals.window_fill = color(&p.bg);
    visuals.panel_fill = color(&p.bg);
    visuals.faint_bg_color = color(&p.surface_alt);
    visuals.extreme_bg_color = color(&p.bg);
    visuals.code_bg_color = color(&p.surface_alt);

    visuals.warn_fg_color = color(&p.warning);
    visuals.error_fg_color = color(&p.danger);

    visuals
}

fn color(hex: &str) -> Color32 {
    match crate::theme::parse_hex_rgb(hex) {
        Some((r, g, b)) => Color32::from_rgb(r, g, b),
        None => Color32::BLACK,
    }
}
