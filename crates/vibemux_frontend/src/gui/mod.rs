#![forbid(unsafe_code)]
//! egui-based VibeMux shell. Two switchable shells:
//!
//! - **Overview** (canvas): an editorial landing that shows every
//!   harness at once plus system facts. Clicking a harness enters it.
//! - **Workbench** (seat): an icon rail, a large terminal "stage" for
//!   the focused harness, and a right-hand inspector.
//!
//! Settings and diagnostics open as lightweight overlays; there are no
//! navigation pages. All interactive elements are stubs.

mod app;
mod overview;
mod stub_bank;
mod theme;
mod topbar;
mod workbench;

pub use app::VibeMuxApp;
pub use theme::apply_theme;

use eframe::egui::{Color32, ViewportBuilder};

/// Shared palette-derived colors plus helper conversions. Rebuilt each
/// frame from the active `ThemePalette`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct C {
    pub bg: Color32,
    pub alt: Color32,
    pub txt: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub hair: Color32,
    pub hair2: Color32,
    pub accent: Color32,
    pub accent_2: Color32,
    pub accent_bg: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub term_bg: Color32,
    pub term_fg: Color32,
}

pub(crate) fn pal(p: &crate::theme::ThemePalette) -> C {
    let bg = hex(&p.bg);
    let accent = hex(&p.accent);
    C {
        bg,
        alt: hex(&p.surface_alt),
        txt: hex(&p.text_primary),
        muted: hex(&p.text_muted),
        faint: mix(&bg, &hex(&p.text_muted), 0.45),
        hair: Color32::from_rgba_unmultiplied(255, 255, 255, 20),
        hair2: Color32::from_rgba_unmultiplied(255, 255, 255, 9),
        accent,
        accent_2: hex(&p.accent_alt),
        accent_bg: mix(&bg, &accent, 0.13),
        ok: hex(&p.success),
        warn: hex(&p.warning),
        term_bg: hex(&p.terminal_bg),
        term_fg: hex(&p.terminal_fg),
    }
}

pub(crate) fn hex(s: &str) -> Color32 {
    match crate::theme::parse_hex_rgb(s) {
        Some((r, g, b)) => Color32::from_rgb(r, g, b),
        None => Color32::BLACK,
    }
}

/// Linear blend `t` of `b` over `a` (both opaque, straight alpha).
pub(crate) fn mix(a: &Color32, b: &Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

/// Run the GUI shell with a pre-built view model + persisted config.
pub fn run_gui(
    vm: crate::view_model::ViewModel,
    config: crate::theme::serialize::UserConfig,
) -> eframe::Result<()> {
    let title = "VibeMux Central Console".to_string();
    let viewport = ViewportBuilder::default()
        .with_title(&title)
        .with_inner_size([
            config.window_size.width as f32,
            config.window_size.height as f32,
        ])
        .with_min_inner_size([960.0, 600.0]);
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        &title,
        options,
        Box::new(|cc| Ok(Box::new(VibeMuxApp::new(cc, vm, config)))),
    )
}

/// Monogram letters per harness index (drawn in a small rounded tile).
pub(crate) fn monogram(idx: usize) -> &'static str {
    ["C", "X", "O", "K", "G"].get(idx).copied().unwrap_or("?")
}
