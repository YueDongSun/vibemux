#![forbid(unsafe_code)]
//! Concrete `ThemePalette` values and palette resolution helpers.

use serde::{Deserialize, Serialize};

use super::{ThemeId, parse_hex_rgb};

/// Comfortable = relaxed spacing, slightly larger fonts, used by Claude
/// and VSCode. Compact = denser packing, slightly smaller fonts, used
/// by GitHub.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    Comfortable,
    Compact,
}

pub const ALL_DENSITIES: [Density; 2] = [Density::Comfortable, Density::Compact];
pub const DENSITY_DEFAULT: Density = Density::Comfortable;

/// Visual specification for one theme. All hex values are stored as
/// 6-digit `#RRGGBB` strings for portable JSON; conversion to
/// `egui::Color32` happens at the GUI boundary.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ThemePalette {
    pub name: String,
    pub bg: String,
    pub surface: String,
    pub surface_alt: String,
    pub border: String,
    pub text_primary: String,
    pub text_muted: String,
    pub accent: String,
    pub accent_alt: String,
    pub success: String,
    pub warning: String,
    pub danger: String,
    pub terminal_bg: String,
    pub terminal_fg: String,
    pub terminal_cursor: String,
    pub font_family: String,
    pub font_size: f32,
    pub corner_radius: f32,
    pub border_width: f32,
    pub density: Density,
}

impl ThemePalette {
    /// Lookup the field on this palette by hex name and convert to
    /// `(r, g, b)`. Returns `None` if the stored string is not a valid
    /// `#RRGGBB`. This is the only place where a bad hex literal would
    /// silently fall back to black; tests assert each hex string is
    /// valid.
    #[must_use]
    pub fn rgb(&self, field: &str) -> Option<(u8, u8, u8)> {
        let hex = match field {
            "bg" => &self.bg,
            "surface" => &self.surface,
            "surface_alt" => &self.surface_alt,
            "border" => &self.border,
            "text_primary" => &self.text_primary,
            "text_muted" => &self.text_muted,
            "accent" => &self.accent,
            "accent_alt" => &self.accent_alt,
            "success" => &self.success,
            "warning" => &self.warning,
            "danger" => &self.danger,
            "terminal_bg" => &self.terminal_bg,
            "terminal_fg" => &self.terminal_fg,
            "terminal_cursor" => &self.terminal_cursor,
            _ => return None,
        };
        parse_hex_rgb(hex)
    }
}

/// Return the canonical palette for a theme id.
#[must_use]
pub fn palette_for(id: ThemeId) -> ThemePalette {
    match id {
        ThemeId::Claude => claude_palette(),
        ThemeId::Github => github_palette(),
        ThemeId::Vscode => vscode_palette(),
    }
}

/// All three palettes in canonical order. Useful for the cycle helper
/// and for tests.
#[must_use]
pub fn all_palettes() -> [ThemePalette; 3] {
    [claude_palette(), github_palette(), vscode_palette()]
}

fn claude_palette() -> ThemePalette {
    ThemePalette {
        name: "claude".to_string(),
        bg: "#0F0E0C".to_string(),
        surface: "#171512".to_string(),
        surface_alt: "#211F1A".to_string(),
        border: "#2B2822".to_string(),
        text_primary: "#EAE6DC".to_string(),
        text_muted: "#9C958A".to_string(),
        accent: "#D97757".to_string(),
        accent_alt: "#E7C6B4".to_string(),
        success: "#8AB487".to_string(),
        warning: "#D9A85F".to_string(),
        danger: "#CB6D63".to_string(),
        terminal_bg: "#0A0908".to_string(),
        terminal_fg: "#E8E4D8".to_string(),
        terminal_cursor: "#D97757".to_string(),
        font_family: "Cascadia Code".to_string(),
        font_size: 13.0,
        corner_radius: 8.0,
        border_width: 1.0,
        density: Density::Comfortable,
    }
}

fn github_palette() -> ThemePalette {
    ThemePalette {
        name: "github".to_string(),
        bg: "#0B0B0F".to_string(),
        surface: "#111318".to_string(),
        surface_alt: "#1A1D24".to_string(),
        border: "#262B33".to_string(),
        text_primary: "#E6EDF3".to_string(),
        text_muted: "#8B949E".to_string(),
        accent: "#58A6FF".to_string(),
        accent_alt: "#A5D6FF".to_string(),
        success: "#3FB950".to_string(),
        warning: "#D29922".to_string(),
        danger: "#F85149".to_string(),
        terminal_bg: "#08090C".to_string(),
        terminal_fg: "#E6EDF3".to_string(),
        terminal_cursor: "#58A6FF".to_string(),
        font_family: "Cascadia Code".to_string(),
        font_size: 13.0,
        corner_radius: 6.0,
        border_width: 1.0,
        density: Density::Compact,
    }
}

fn vscode_palette() -> ThemePalette {
    ThemePalette {
        name: "vscode".to_string(),
        bg: "#0E0F11".to_string(),
        surface: "#16181C".to_string(),
        surface_alt: "#1F2328".to_string(),
        border: "#2B313A".to_string(),
        text_primary: "#E8EAED".to_string(),
        text_muted: "#9DA5AE".to_string(),
        accent: "#007ACC".to_string(),
        accent_alt: "#75BEFF".to_string(),
        success: "#16825D".to_string(),
        warning: "#CCA700".to_string(),
        danger: "#E51400".to_string(),
        terminal_bg: "#0A0B0D".to_string(),
        terminal_fg: "#E8EAED".to_string(),
        terminal_cursor: "#007ACC".to_string(),
        font_family: "Cascadia Code".to_string(),
        font_size: 13.5,
        corner_radius: 4.0,
        border_width: 1.0,
        density: Density::Comfortable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_valid_hex(p: &ThemePalette, field: &str) {
        let rgb = p
            .rgb(field)
            .unwrap_or_else(|| panic!("palette {} has invalid hex for {field}", p.name));
        let (r, g, b) = rgb;
        let _ = (r, g, b);
    }

    #[test]
    fn themes_share_grayscale_base() {
        for palette in all_palettes() {
            assert_eq!(palette.surface_alt.len(), 7);
            assert_eq!(palette.text_primary.len(), 7);
            assert_eq!(palette.text_muted.len(), 7);
            assert_eq!(palette.terminal_bg.len(), 7);
            assert_eq!(palette.terminal_fg.len(), 7);
            // Surface is strictly lighter than bg; muted text strictly
            // darker than primary text (per-theme contrast invariant).
            let (br, bg, bb) = palette.rgb("bg").expect("bg");
            let surface = palette.rgb("surface").expect("surface");
            let primary = palette.rgb("text_primary").expect("primary");
            let muted = palette.rgb("text_muted").expect("muted");
            let (sr, sg, sb) = surface;
            let lum_bg = 0.2126 * br as f32 + 0.7152 * bg as f32 + 0.0722 * bb as f32;
            let lum_surface = 0.2126 * sr as f32 + 0.7152 * sg as f32 + 0.0722 * sb as f32;
            assert!(lum_surface > lum_bg, "surface must be lighter than bg");
            let (pr, pg, pb) = primary;
            let (mr, mg, mb) = muted;
            let lum_primary = 0.2126 * pr as f32 + 0.7152 * pg as f32 + 0.0722 * pb as f32;
            let lum_muted = 0.2126 * mr as f32 + 0.7152 * mg as f32 + 0.0722 * mb as f32;
            assert!(
                lum_primary > lum_muted,
                "primary text must be lighter than muted"
            );
        }
    }

    #[test]
    fn claude_uses_warm_accent() {
        let p = palette_for(ThemeId::Claude);
        assert_eq!(p.accent, "#D97757");
        assert_eq!(p.accent_alt, "#E7C6B4");
    }

    #[test]
    fn github_uses_blue_accent() {
        let p = palette_for(ThemeId::Github);
        assert_eq!(p.accent, "#58A6FF");
        assert_eq!(p.accent_alt, "#A5D6FF");
    }

    #[test]
    fn vscode_uses_electric_blue_accent() {
        let p = palette_for(ThemeId::Vscode);
        assert_eq!(p.accent, "#007ACC");
        assert_eq!(p.accent_alt, "#75BEFF");
    }

    #[test]
    fn every_hex_field_is_valid() {
        for p in all_palettes() {
            for field in [
                "bg",
                "surface",
                "surface_alt",
                "border",
                "text_primary",
                "text_muted",
                "accent",
                "accent_alt",
                "success",
                "warning",
                "danger",
                "terminal_bg",
                "terminal_fg",
                "terminal_cursor",
            ] {
                assert_valid_hex(&p, field);
            }
        }
    }

    #[test]
    fn every_palette_name_matches_id() {
        assert_eq!(palette_for(ThemeId::Claude).name, "claude");
        assert_eq!(palette_for(ThemeId::Github).name, "github");
        assert_eq!(palette_for(ThemeId::Vscode).name, "vscode");
    }
}
