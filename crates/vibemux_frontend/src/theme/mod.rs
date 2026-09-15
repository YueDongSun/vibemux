#![forbid(unsafe_code)]
//! Theme system: 3 switchable palettes on a shared black-and-white
//! grayscale ramp.
//!
//! - `ThemeId` selects which palette is active.
//! - `ThemePalette` is the value stored on disk and applied to the
//!   GUI's `egui::Context`.
//! - `serialize` owns the on-disk JSON layout and read/write
//!   lifecycle for `UserConfig`.
//!
//! All three palettes share the same grayscale ramp (the "B&W base").
//! The differences are entirely in the accent color, accent_alt, font,
//! corner radius, density, and font size.

mod palette;
pub mod serialize;

pub use palette::{
    ALL_DENSITIES, DENSITY_DEFAULT, Density, ThemePalette, all_palettes, palette_for,
};
pub use serialize::{UserConfig, WindowSize, load_user_config, save_user_config};

use serde::{Deserialize, Serialize};

/// Which palette is currently selected.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeId {
    Claude,
    Github,
    Vscode,
}

impl ThemeId {
    pub const ALL: [Self; 3] = [Self::Claude, Self::Github, Self::Vscode];

    /// Cycle to the next theme in the canonical order:
    /// `Claude -> GitHub -> VSCode -> Claude`.
    #[must_use]
    pub const fn cycle(self) -> Self {
        match self {
            Self::Claude => Self::Github,
            Self::Github => Self::Vscode,
            Self::Vscode => Self::Claude,
        }
    }

    /// Stable lowercase token for serialization and on-disk persistence.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Github => "github",
            Self::Vscode => "vscode",
        }
    }
}

impl Default for ThemeId {
    fn default() -> Self {
        Self::Claude
    }
}

/// Parse a `#RRGGBB` (uppercase or lowercase) hex string into a
/// `(r, g, b)` triple. Returns `None` for any other format.
#[must_use]
pub fn parse_hex_rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let trimmed = hex.trim();
    let body = trimmed.strip_prefix('#')?;
    if body.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&body[0..2], 16).ok()?;
    let g = u8::from_str_radix(&body[2..4], 16).ok()?;
    let b = u8::from_str_radix(&body[4..6], 16).ok()?;
    Some((r, g, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_id_cycle_wraps() {
        assert_eq!(ThemeId::Claude.cycle(), ThemeId::Github);
        assert_eq!(ThemeId::Github.cycle(), ThemeId::Vscode);
        assert_eq!(ThemeId::Vscode.cycle(), ThemeId::Claude);
    }

    #[test]
    fn theme_id_as_str_is_lowercase() {
        for id in ThemeId::ALL {
            assert!(id.as_str().chars().all(|c| c.is_ascii_lowercase()));
        }
    }

    #[test]
    fn theme_id_default_is_claude() {
        assert_eq!(ThemeId::default(), ThemeId::Claude);
    }

    #[test]
    fn parse_hex_rgb_handles_uppercase_and_lowercase() {
        assert_eq!(parse_hex_rgb("#000000"), Some((0, 0, 0)));
        assert_eq!(parse_hex_rgb("#FFFFFF"), Some((255, 255, 255)));
        assert_eq!(parse_hex_rgb("#f2f2f2"), Some((0xf2, 0xf2, 0xf2)));
        assert_eq!(parse_hex_rgb("#D97757"), Some((0xD9, 0x77, 0x57)));
    }

    #[test]
    fn parse_hex_rgb_rejects_invalid() {
        assert_eq!(parse_hex_rgb("000000"), None);
        assert_eq!(parse_hex_rgb("#FFF"), None);
        assert_eq!(parse_hex_rgb("#GGGGGG"), None);
        assert_eq!(parse_hex_rgb("#1234567"), None);
        assert_eq!(parse_hex_rgb(""), None);
    }

    #[test]
    fn all_palettes_returns_three_in_canonical_order() {
        let palettes = all_palettes();
        assert_eq!(palettes.len(), 3);
        assert_eq!(palettes[0].name, "claude");
        assert_eq!(palettes[1].name, "github");
        assert_eq!(palettes[2].name, "vscode");
    }
}
