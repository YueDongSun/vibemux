#![forbid(unsafe_code)]
//! Selectable palettes for the TUI debug view.
//!
//! Ported from the single-shell frontend's audited `Theme` system
//! (classic / high-contrast / mono / light) with the WCAG AA contrast
//! audit, extended with the `nord` and `gruvbox` brand palettes. The
//! TUI owns this ratatui-named-color system; the GUI keeps its own
//! persisted hex `ThemeId` palettes, so the debug view no longer
//! mirrors the GUI's theme file.

use ratatui::style::{Color, Modifier, Style};
use vibemux_probe::{ProbeState, RouteKind};

use crate::view_model::Health;

/// Selectable visual palette for the TUI renderer.
///
/// `Classic` targets 8-color dark terminals, `HighContrast` uses light
/// variants for bright environments and low-vision users, `Mono` drops
/// foreground colors entirely (emphasis via bold/dim only), `Light`
/// inverts to dark variants tuned for white backgrounds, and `Nord` /
/// `Gruvbox` map the brand palettes onto xterm-256 indices so the
/// dashboard renders the same hues on every supported backend
/// (WezTerm on Windows, tmux on POSIX) without a truecolor dependency.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Theme {
    #[default]
    Classic,
    HighContrast,
    Mono,
    Light,
    Nord,
    Gruvbox,
}

impl Theme {
    /// Every theme in CLI listing order.
    pub const ALL: [Theme; 6] = [
        Self::Classic,
        Self::HighContrast,
        Self::Mono,
        Self::Light,
        Self::Nord,
        Self::Gruvbox,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::HighContrast => "high-contrast",
            Self::Mono => "mono",
            Self::Light => "light",
            Self::Nord => "nord",
            Self::Gruvbox => "gruvbox",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "classic" => Some(Self::Classic),
            "high-contrast" => Some(Self::HighContrast),
            "mono" => Some(Self::Mono),
            "light" => Some(Self::Light),
            "nord" => Some(Self::Nord),
            "gruvbox" => Some(Self::Gruvbox),
            _ => None,
        }
    }

    /// The next theme in CLI listing order, wrapping around.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Classic => Self::HighContrast,
            Self::HighContrast => Self::Mono,
            Self::Mono => Self::Light,
            Self::Light => Self::Nord,
            Self::Nord => Self::Gruvbox,
            Self::Gruvbox => Self::Classic,
        }
    }

    /// Resolve the theme from `VIBEMUX_FRONTEND_THEME`, defaulting to `Classic`.
    #[must_use]
    pub fn from_environment() -> Self {
        std::env::var("VIBEMUX_FRONTEND_THEME")
            .ok()
            .as_deref()
            .and_then(Self::from_name)
            .unwrap_or_default()
    }
}

pub fn state_style(theme: Theme, state: ProbeState) -> Style {
    let (ok, failure, degraded, idle) = match theme {
        // Classic failure uses the bright red: the standard dark red only
        // reaches 3.6:1 against black, below WCAG AA (4.5:1).
        Theme::Classic => (
            Color::Green,
            Color::LightRed,
            Color::Yellow,
            Color::DarkGray,
        ),
        Theme::HighContrast => (
            Color::LightGreen,
            Color::LightRed,
            Color::LightYellow,
            Color::White,
        ),
        Theme::Mono => (Color::Reset, Color::Reset, Color::Reset, Color::Reset),
        // Light: dark variants tuned for white backgrounds; every value
        // clears WCAG AA (4.5:1) against white in the audit test. 16-color
        // yellow/gray fail on white, so degraded and idle use dark amber and
        // a darker gray instead.
        Theme::Light => (
            Color::Rgb(0, 110, 0),
            Color::Red,
            Color::Rgb(150, 75, 0),
            Color::Rgb(96, 96, 96),
        ),
        // Nord: xterm-256 indices resolved from the brand hex values
        // (snow-storm greens and yellows, frost blues). Idle recedes as
        // neutral gray. The nearest index to the brand red measures only
        // 4.64:1 against black — inside the audit gate but too close to
        // trust across terminals — so failure takes the brighter 167.
        Theme::Nord => (
            Color::Indexed(144),
            Color::Indexed(167),
            Color::Indexed(186),
            Color::Indexed(244),
        ),
        // Gruvbox: olive green, bright red, and yellow resolved from the
        // brand hex values; muted foregrounds use the bg4 gray ramp.
        Theme::Gruvbox => (
            Color::Indexed(142),
            Color::Indexed(203),
            Color::Indexed(214),
            Color::Indexed(245),
        ),
    };
    let style = match state {
        ProbeState::Verified => Style::default().fg(ok),
        ProbeState::Failed => Style::default().fg(failure).add_modifier(Modifier::BOLD),
        ProbeState::Unavailable => Style::default().fg(degraded),
        ProbeState::NotRun => Style::default().fg(idle),
    };
    match theme {
        // Mono keeps a strict emphasis ladder so states stay distinguishable
        // without any color: verified and failure jump out bold, degraded is
        // plain, and idle recedes dim.
        Theme::Mono => match state {
            ProbeState::Verified | ProbeState::Failed => style.add_modifier(Modifier::BOLD),
            ProbeState::Unavailable => style,
            ProbeState::NotRun => style.add_modifier(Modifier::DIM),
        },
        _ => style,
    }
}

pub fn route_style(theme: Theme, route: RouteKind) -> Style {
    let (direct, gateway, unknown) = match theme {
        Theme::Classic => (Color::Green, Color::Cyan, Color::DarkGray),
        Theme::HighContrast => (Color::LightGreen, Color::LightCyan, Color::White),
        Theme::Mono => (Color::Reset, Color::Reset, Color::Reset),
        Theme::Light => (
            Color::Rgb(0, 110, 0),
            Color::Rgb(0, 110, 110),
            Color::Rgb(96, 96, 96),
        ),
        // Nord keeps the frost-blue gateway distinct from the direct green;
        // Gruvbox pairs the olive direct with the aqua gateway.
        Theme::Nord => (
            Color::Indexed(144),
            Color::Indexed(110),
            Color::Indexed(244),
        ),
        Theme::Gruvbox => (
            Color::Indexed(142),
            Color::Indexed(108),
            Color::Indexed(245),
        ),
    };
    match route {
        RouteKind::Direct => Style::default().fg(direct),
        RouteKind::LocalGateway => Style::default().fg(gateway),
        RouteKind::Unknown => Style::default().fg(unknown),
    }
}

pub fn health_style(theme: Theme, health: Health) -> Style {
    let (ok, warning, failure) = match theme {
        Theme::Classic => (Color::Green, Color::Yellow, Color::LightRed),
        Theme::HighContrast => (Color::LightGreen, Color::LightYellow, Color::LightRed),
        Theme::Mono => (Color::Reset, Color::Reset, Color::Reset),
        // Terminal Green/Yellow on a white background measure ~1.71:1, far
        // below the WCAG AA 4.5:1 audit the light theme advertises; reuse the
        // dark variants already audited for the light state cells.
        Theme::Light => (Color::Rgb(0, 110, 0), Color::Rgb(150, 75, 0), Color::Red),
        // Nord and Gruvbox reuse their state ramps: brand greens for ok,
        // brand yellows for warning, bright brand reds for failure.
        Theme::Nord => (
            Color::Indexed(144),
            Color::Indexed(186),
            Color::Indexed(167),
        ),
        Theme::Gruvbox => (
            Color::Indexed(142),
            Color::Indexed(214),
            Color::Indexed(203),
        ),
    };
    match health {
        Health::Ok => Style::default().fg(ok),
        Health::Warning => Style::default().fg(warning),
        Health::Failure => Style::default().fg(failure).add_modifier(Modifier::BOLD),
    }
}

pub fn title_style(theme: Theme) -> Style {
    match theme {
        Theme::Classic => Style::default().fg(Color::Cyan),
        Theme::HighContrast => Style::default().fg(Color::White),
        Theme::Mono => Style::default(),
        Theme::Light => Style::default().fg(Color::Rgb(0, 110, 110)),
        // The title takes each brand's signature hue: Nord's frost blue
        // and Gruvbox's bright orange.
        Theme::Nord => Style::default().fg(Color::Indexed(110)),
        Theme::Gruvbox => Style::default().fg(Color::Indexed(208)),
    }
}

/// Telemetry line semantics: absent data stays muted, zero failures are
/// healthy green, and any failure count turns red regardless of theme.
pub fn telemetry_style(theme: Theme, available: bool, failures: u64) -> Style {
    if !available {
        return muted_style(theme);
    }
    match theme {
        Theme::Mono => {
            if failures > 0 {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            }
        }
        // Light uses the dark red (clears WCAG AA on white); bright red fails
        // there, so the light theme flips the pairing. Green is likewise the
        // dark variant on light backgrounds.
        Theme::Light => {
            let color = if failures > 0 {
                Color::Red
            } else {
                Color::Rgb(0, 110, 0)
            };
            Style::default().fg(color)
        }
        // Nord and Gruvbox mirror the state ramps: brand reds on failure,
        // brand greens for a healthy line.
        Theme::Nord => {
            let color = if failures > 0 {
                Color::Indexed(167)
            } else {
                Color::Indexed(144)
            };
            Style::default().fg(color)
        }
        Theme::Gruvbox => {
            let color = if failures > 0 {
                Color::Indexed(203)
            } else {
                Color::Indexed(142)
            };
            Style::default().fg(color)
        }
        _ => {
            let color = if failures > 0 {
                Color::LightRed
            } else {
                Color::Green
            };
            Style::default().fg(color)
        }
    }
}

pub fn muted_style(theme: Theme) -> Style {
    match theme {
        Theme::Classic => Style::default().fg(Color::DarkGray),
        Theme::HighContrast => Style::default().fg(Color::White),
        Theme::Mono => Style::default().add_modifier(Modifier::DIM),
        Theme::Light => Style::default().fg(Color::Rgb(96, 96, 96)),
        // Neutral grays that pass AA on black while staying clearly
        // dimmer than any state color in the same theme.
        Theme::Nord => Style::default().fg(Color::Indexed(244)),
        Theme::Gruvbox => Style::default().fg(Color::Indexed(245)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_names_round_trip_and_cycle_covers_all() {
        assert_eq!(Theme::ALL.len(), 6);
        for theme in Theme::ALL {
            assert_eq!(Theme::from_name(theme.name()), Some(theme));
        }
        let mut theme = Theme::Classic;
        for _ in 0..Theme::ALL.len() {
            theme = theme.next();
        }
        assert_eq!(theme, Theme::Classic);
        assert_eq!(Theme::from_name("neon-does-not-exist"), None);
    }

    /// Map named colors to the xterm-256 palette values real terminals
    /// emit (e.g. ANSI green is xterm's 0/205/0, not CSS's 0/128/0), so
    /// the audit measures what users actually see.
    fn color_rgb(color: Color) -> (u8, u8, u8) {
        match color {
            Color::Black => (0, 0, 0),
            Color::Red => (205, 0, 0),
            Color::Green => (0, 205, 0),
            Color::Yellow => (205, 205, 0),
            Color::Cyan => (0, 205, 205),
            Color::DarkGray => (128, 128, 128),
            Color::LightRed => (255, 0, 0),
            Color::LightGreen => (0, 255, 0),
            Color::LightYellow => (255, 255, 0),
            Color::LightCyan => (0, 255, 255),
            Color::White => (229, 229, 229),
            Color::Gray => (229, 229, 229),
            Color::Rgb(r, g, b) => (r, g, b),
            Color::Indexed(index) => indexed_rgb(index),
            _ => (229, 229, 229),
        }
    }

    /// Resolve an xterm-256 index to the RGB value standard terminals
    /// emit: the 16-color base ramp, the 6x6x6 color cube, then the
    /// grayscale ramp.
    fn indexed_rgb(index: u8) -> (u8, u8, u8) {
        match index {
            0..=15 => ansi_base_rgb(index),
            16..=231 => {
                let offset = u16::from(index - 16);
                let levels = [0u8, 95, 135, 175, 215, 255];
                let red = levels[usize::from(offset / 36)];
                let green = levels[usize::from((offset / 6) % 6)];
                let blue = levels[usize::from(offset % 6)];
                (red, green, blue)
            }
            _ => {
                // The `_` arm only receives 232..=255, so the gray step
                // 8 + 10 * n stays within u8 (max 238).
                let gray = 8 + 10 * (index - 232);
                (gray, gray, gray)
            }
        }
    }

    fn ansi_base_rgb(index: u8) -> (u8, u8, u8) {
        match index {
            0 => (0, 0, 0),
            1 => (205, 0, 0),
            2 => (0, 205, 0),
            3 => (205, 205, 0),
            4 => (0, 0, 238),
            5 => (205, 0, 205),
            6 => (0, 205, 205),
            7 => (229, 229, 229),
            8 => (127, 127, 127),
            9 => (255, 0, 0),
            10 => (0, 255, 0),
            11 => (255, 255, 0),
            12 => (92, 92, 255),
            13 => (255, 0, 255),
            14 => (0, 255, 255),
            _ => (255, 255, 255),
        }
    }

    fn relative_luminance(rgb: (u8, u8, u8)) -> f64 {
        let channel = |value: u8| {
            let v = f64::from(value) / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(rgb.0) + 0.7152 * channel(rgb.1) + 0.0722 * channel(rgb.2)
    }

    fn contrast_ratio(first: Color, second: Color) -> f64 {
        let lighter =
            relative_luminance(color_rgb(first)).max(relative_luminance(color_rgb(second)));
        let darker =
            relative_luminance(color_rgb(first)).min(relative_luminance(color_rgb(second)));
        (lighter + 0.05) / (darker + 0.05)
    }

    #[test]
    fn theme_state_colors_meet_wcag_aa_on_dark_backgrounds() {
        // The dashboard targets dark terminals; every state color must keep
        // at least AA (4.5:1) against black so text stays readable.
        let classic_pairs = [
            (ProbeState::Verified, Color::Green),
            (ProbeState::Failed, Color::LightRed),
            (ProbeState::Unavailable, Color::Yellow),
            (ProbeState::NotRun, Color::DarkGray),
        ];
        for (_, color) in classic_pairs {
            let ratio = contrast_ratio(color, Color::Black);
            assert!(ratio >= 4.5, "classic {color:?} contrast {ratio:.2} < 4.5");
        }
        let high_contrast_pairs = [
            (ProbeState::Verified, Color::LightGreen),
            (ProbeState::Failed, Color::LightRed),
            (ProbeState::Unavailable, Color::LightYellow),
            (ProbeState::NotRun, Color::White),
        ];
        for (state_probe, color) in high_contrast_pairs {
            let ratio = contrast_ratio(color, Color::Black);
            assert!(
                ratio >= 4.5,
                "high-contrast {color:?} contrast {ratio:.2} < 4.5"
            );
            let classic_twin = classic_pairs
                .iter()
                .find(|(state, _)| *state == state_probe)
                .map(|(_, color)| contrast_ratio(*color, Color::Black))
                .unwrap_or(0.0);
            assert!(
                ratio >= classic_twin,
                "high-contrast must not be weaker than classic"
            );
        }
        // Light variants must be strictly brighter than their classic twins.
        assert!(
            contrast_ratio(Color::LightGreen, Color::Black)
                > contrast_ratio(Color::Green, Color::Black)
        );
        assert!(
            contrast_ratio(Color::LightRed, Color::Black)
                > contrast_ratio(Color::Red, Color::Black)
        );
        // The light theme inverts the background: its dark colors must clear
        // AA against white, with the red pairing flipped (dark red passes on
        // white, bright red fails there).
        let light_pairs = [
            (ProbeState::Verified, Color::Rgb(0, 110, 0)),
            (ProbeState::Failed, Color::Red),
            (ProbeState::Unavailable, Color::Rgb(150, 75, 0)),
            (ProbeState::NotRun, Color::Rgb(96, 96, 96)),
        ];
        for (_, color) in light_pairs {
            let ratio = contrast_ratio(color, Color::White);
            assert!(
                ratio >= 4.5,
                "light {color:?} on white contrast {ratio:.2} < 4.5"
            );
        }
        assert!(contrast_ratio(Color::Red, Color::White) >= 4.5);
        assert!(contrast_ratio(Color::LightRed, Color::White) < 4.5);
        // The naive light palette would have failed: green-on-white 1.71,
        // yellow-on-white 1.71, gray-on-white 3.95.
        assert!(contrast_ratio(Color::Green, Color::White) < 4.5);
        assert!(contrast_ratio(Color::Yellow, Color::White) < 4.5);
        // Light theme accents against white: title/route cyan and the dark
        // gray muted/route-unknown clear AA too.
        for (label, color) in [
            ("light title dark cyan", Color::Rgb(0, 110, 110)),
            ("light route direct", Color::Rgb(0, 110, 0)),
            ("light route gateway", Color::Rgb(0, 110, 110)),
            ("light muted gray", Color::Rgb(96, 96, 96)),
        ] {
            let ratio = contrast_ratio(color, Color::White);
            assert!(ratio >= 4.5, "{label} contrast {ratio:.2} < 4.5");
        }
        // Every other accent color in the renderer: title cyan, route colors,
        // and the muted gray must also clear AA on black.
        for (label, color) in [
            ("title cyan", Color::Cyan),
            ("high-contrast title white", Color::White),
            ("route direct green", Color::Green),
            ("route gateway cyan", Color::Cyan),
            ("muted dark gray", Color::DarkGray),
        ] {
            let ratio = contrast_ratio(color, Color::Black);
            assert!(ratio >= 4.5, "{label} contrast {ratio:.2} < 4.5");
        }
        // The aggregate health label in the title/footer follows the same
        // audit: dark themes use the classic ramps on black; the light theme
        // uses the WCAG-corrected dark variants on white (plain terminal
        // Green/Yellow on white measure ~1.71:1 and would fail).
        for (label, color) in [
            ("classic health ok", Color::Green),
            ("classic health warning", Color::Yellow),
            ("classic health failure", Color::LightRed),
            ("high-contrast health ok", Color::LightGreen),
            ("high-contrast health warning", Color::LightYellow),
            ("high-contrast health failure", Color::LightRed),
        ] {
            let ratio = contrast_ratio(color, Color::Black);
            assert!(ratio >= 4.5, "{label} on black contrast {ratio:.2} < 4.5");
        }
        for (label, color) in [
            ("light health ok", Color::Rgb(0, 110, 0)),
            ("light health warning", Color::Rgb(150, 75, 0)),
            ("light health failure", Color::Red),
        ] {
            let ratio = contrast_ratio(color, Color::White);
            assert!(ratio >= 4.5, "{label} on white contrast {ratio:.2} < 4.5");
        }
        // The brand themes map their hex palettes onto xterm-256 indices;
        // the audit measures the RGB real terminals emit for those
        // indices, so every accent must clear AA on black exactly like
        // the classic ramp. Nord first.
        for (label, color) in [
            ("nord state ok", Color::Indexed(144)),
            ("nord state failure", Color::Indexed(167)),
            ("nord state degraded", Color::Indexed(186)),
            ("nord state idle", Color::Indexed(244)),
            ("nord health ok", Color::Indexed(144)),
            ("nord health warning", Color::Indexed(186)),
            ("nord health failure", Color::Indexed(167)),
            ("nord title", Color::Indexed(110)),
            ("nord route direct", Color::Indexed(144)),
            ("nord route gateway", Color::Indexed(110)),
            ("nord route unknown", Color::Indexed(244)),
            ("nord muted", Color::Indexed(244)),
        ] {
            let ratio = contrast_ratio(color, Color::Black);
            assert!(ratio >= 4.5, "{label} on black contrast {ratio:.2} < 4.5");
        }
        // Then Gruvbox. The muted gray (245) is deliberately brighter
        // than classic's DarkGray because the olive/aqua accents leave
        // less headroom; the audit is the same gate either way.
        for (label, color) in [
            ("gruvbox state ok", Color::Indexed(142)),
            ("gruvbox state failure", Color::Indexed(203)),
            ("gruvbox state degraded", Color::Indexed(214)),
            ("gruvbox state idle", Color::Indexed(245)),
            ("gruvbox health ok", Color::Indexed(142)),
            ("gruvbox health warning", Color::Indexed(214)),
            ("gruvbox health failure", Color::Indexed(203)),
            ("gruvbox title", Color::Indexed(208)),
            ("gruvbox route direct", Color::Indexed(142)),
            ("gruvbox route gateway", Color::Indexed(108)),
            ("gruvbox route unknown", Color::Indexed(245)),
            ("gruvbox muted", Color::Indexed(245)),
        ] {
            let ratio = contrast_ratio(color, Color::Black);
            assert!(ratio >= 4.5, "{label} on black contrast {ratio:.2} < 4.5");
        }
    }
}
