#![forbid(unsafe_code)]
//! Theme system: two light palettes (Studio, Claude light) and five dark palettes.
//! background-to-border ramp and semantic accent set.
//!
//! - `ThemeId` selects which palette is active.
//! - `ThemePalette` is the value stored on disk and applied to the
//!   GUI's `egui::Context`.
//! - `serialize` owns the on-disk JSON layout and read/write
//!   lifecycle for `UserConfig`.
//!
//! The compiled-in palettes are the canonical values; the exported
//! `config/theme_palettes.json` mirrors them exactly (pinned by the
//! `palette_parity` integration test) so the Python CLI and the
//! WezTerm/tmux theme generators consume the same colors.

pub mod contrast;
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
    Nord,
    Gruvbox,
    Studio,
    ClaudeLight,
}

impl ThemeId {
    pub const ALL: [Self; 7] = [
        Self::Claude,
        Self::Github,
        Self::Vscode,
        Self::Nord,
        Self::Gruvbox,
        Self::Studio,
        Self::ClaudeLight,
    ];

    /// Cycle to the next theme in the canonical order:
    /// `Claude -> GitHub -> VSCode -> Nord -> Gruvbox -> Studio -> ClaudeLight -> Claude`.
    #[must_use]
    pub const fn cycle(self) -> Self {
        match self {
            Self::Claude => Self::Github,
            Self::Github => Self::Vscode,
            Self::Vscode => Self::Nord,
            Self::Nord => Self::Gruvbox,
            Self::Gruvbox => Self::Studio,
            Self::Studio => Self::ClaudeLight,
            Self::ClaudeLight => Self::Claude,
        }
    }

    /// Stable snake_case token for serialization and on-disk persistence.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Github => "github",
            Self::Vscode => "vscode",
            Self::Nord => "nord",
            Self::Gruvbox => "gruvbox",
            Self::Studio => "studio",
            Self::ClaudeLight => "claude_light",
        }
    }
}

impl Default for ThemeId {
    fn default() -> Self {
        // New profiles use Claude light (ADR 030); saved theme IDs are preserved.
        Self::ClaudeLight
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
        assert_eq!(ThemeId::Vscode.cycle(), ThemeId::Nord);
        assert_eq!(ThemeId::Nord.cycle(), ThemeId::Gruvbox);
        assert_eq!(ThemeId::Gruvbox.cycle(), ThemeId::Studio);
        assert_eq!(ThemeId::Studio.cycle(), ThemeId::ClaudeLight);
        assert_eq!(ThemeId::ClaudeLight.cycle(), ThemeId::Claude);
    }

    #[test]
    fn theme_id_as_str_is_snake_case() {
        for id in ThemeId::ALL {
            assert!(
                id.as_str()
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_')
            );
            assert!(!id.as_str().starts_with('_') && !id.as_str().ends_with('_'));
        }
    }

    #[test]
    fn theme_id_default_is_claude_light() {
        assert_eq!(ThemeId::default(), ThemeId::ClaudeLight);
    }

    #[test]
    fn claude_light_serializes_as_snake_case_token() {
        assert_eq!(
            serde_json::to_string(&ThemeId::ClaudeLight).expect("serialize"),
            "\"claude_light\""
        );
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
    fn all_palettes_returns_seven_in_canonical_order() {
        let names: Vec<String> = all_palettes().into_iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            [
                "claude",
                "github",
                "vscode",
                "nord",
                "gruvbox",
                "studio",
                "claude_light"
            ]
        );
    }

    #[test]
    fn cycle_covers_every_theme_once() {
        let mut id = ThemeId::default();
        for _ in 0..ThemeId::ALL.len() {
            id = id.cycle();
        }
        assert_eq!(id, ThemeId::default());
        for candidate in ThemeId::ALL {
            assert!(
                ThemeId::ALL.iter().any(|seen| seen.cycle() == candidate),
                "cycle must reach {candidate:?}"
            );
        }
    }
}
