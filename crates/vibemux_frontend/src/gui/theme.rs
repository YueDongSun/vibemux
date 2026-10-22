#![forbid(unsafe_code)]
//! Theme application: convert `ThemePalette` (hex strings) into egui
//! visual style.

use eframe::egui::{self, Color32, Context, Style, Visuals};

use crate::theme::{Density, ThemePalette};

/// Apply the active palette to the egui context. Idempotent: safe to
/// call on every frame.
pub fn apply_theme(ctx: &Context, palette: &ThemePalette) {
    let visuals = build_visuals(palette);
    let mut style = Style {
        visuals,
        ..Default::default()
    };
    let (item_spacing, button_padding) = density_spacing(Density::Comfortable);
    style.spacing.item_spacing = item_spacing;
    style.spacing.button_padding = button_padding;
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(15.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Heading, egui::FontId::proportional(20.0));
    style
        .text_styles
        .insert(egui::TextStyle::Monospace, egui::FontId::monospace(13.0));
    ctx.set_global_style(style);
    // Let egui honor the OS display scale (do not pin pixels_per_point
    // to 1.0): on high-DPI monitors the UI would otherwise render far
    // too small.
}

/// Item spacing and button padding for a density.
#[must_use]
pub fn density_spacing(density: Density) -> (egui::Vec2, egui::Vec2) {
    match density {
        Density::Comfortable => (egui::vec2(8.0, 8.0), egui::vec2(12.0, 6.0)),
        Density::Compact => (egui::vec2(6.0, 4.0), egui::vec2(8.0, 3.0)),
    }
}

/// Applies the saved density. Call after `apply_theme`, which resets the
/// style to comfortable spacing every frame.
pub fn apply_density(ctx: &Context, density: Density) {
    if density == Density::Comfortable {
        return;
    }
    let (item_spacing, button_padding) = density_spacing(density);
    ctx.global_style_mut(|style| {
        style.spacing.item_spacing = item_spacing;
        style.spacing.button_padding = button_padding;
    });
}

fn build_visuals(p: &ThemePalette) -> Visuals {
    let light = p.is_light();
    let mut visuals = if light {
        Visuals::light()
    } else {
        Visuals::dark()
    };
    visuals.override_text_color = Some(color(&p.text_primary));
    let channel = |field: &str| p.rgb(field).unwrap_or((0, 0, 0));
    let link = crate::theme::contrast::readable_text_color(
        channel("accent"),
        &[channel("bg"), channel("surface"), channel("surface_alt")],
        channel("text_primary"),
        crate::theme::contrast::TEXT_CONTRAST_MINIMUM,
    );
    visuals.hyperlink_color = Color32::from_rgb(link.0, link.1, link.2);
    // Window chrome: 1px border-colored stroke. Windows and menus take the
    // shared card radius; widgets take the palette radius (the button scale).
    visuals.window_stroke = egui::Stroke::new(p.border_width, color(&p.border));
    visuals.window_corner_radius = egui::CornerRadius::same(super::design::CARD_RADIUS);
    visuals.menu_corner_radius = egui::CornerRadius::same(super::design::CARD_RADIUS);
    visuals.text_cursor.stroke = egui::Stroke::new(1.0, color(&p.accent));

    visuals.widgets.noninteractive.bg_fill = color(&p.surface);
    visuals.widgets.noninteractive.weak_bg_fill = color(&p.surface);
    visuals.widgets.noninteractive.bg_stroke.color = color(&p.border);
    visuals.widgets.noninteractive.fg_stroke.color = color(&p.border);
    visuals.widgets.noninteractive.bg_stroke.width = p.border_width;
    visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::same(p.corner_radius as u8);

    visuals.widgets.inactive.bg_fill = color(&p.surface_alt);
    visuals.widgets.inactive.weak_bg_fill = color(&p.surface_alt);
    visuals.widgets.inactive.bg_stroke.color = color(&p.border);
    visuals.widgets.inactive.bg_stroke.width = 0.0;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_density_is_tighter_than_comfortable() {
        let (comfortable_items, comfortable_buttons) = density_spacing(Density::Comfortable);
        let (compact_items, compact_buttons) = density_spacing(Density::Compact);
        assert!(compact_items.x < comfortable_items.x && compact_items.y < comfortable_items.y);
        assert!(
            compact_buttons.x < comfortable_buttons.x && compact_buttons.y < comfortable_buttons.y
        );
    }
}
