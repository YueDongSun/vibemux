#![forbid(unsafe_code)]
//! egui-based Supervisor Chat shell. The coordinator conversation is the
//! default view; probe-backed agent details and task detail windows remain
//! separate, read-only surfaces.

mod agents;
mod chat;
mod design;
mod diagnostics;
mod settings;
mod sidebar;
mod supervisor_app;
mod supervisor_state;
mod task_detail;
mod theme;

pub use chat::wrapped_label as render_wrapped_label;
pub use supervisor_app::SupervisorApp as VibeMuxApp;
pub use supervisor_state::{MainPage, SupervisorUiState, TaskDetailSelection, TaskDetailTab};
pub use theme::apply_theme;

use eframe::egui::{Color32, ViewportBuilder};

/// Shared palette-derived colors plus helper conversions. Rebuilt each
/// frame from the active `ThemePalette`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct C {
    pub bg: Color32,
    /// Mid surface (Primer "canvas subtle"): sidebar and box headers.
    pub surf: Color32,
    pub raised: Color32,
    pub border: Color32,
    pub txt: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub accent: Color32,
    pub accent_bg: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub danger: Color32,
}

pub(crate) fn pal(p: &crate::theme::ThemePalette) -> C {
    let bg = hex(&p.bg);
    let accent = hex(&p.accent);
    C {
        bg,
        surf: hex(&p.surface),
        raised: hex(&p.surface_alt),
        border: hex(&p.border),
        txt: hex(&p.text_primary),
        muted: hex(&p.text_muted),
        faint: hex(&p.text_muted),
        accent,
        accent_bg: mix(&bg, &accent, 0.17),
        ok: hex(&p.success),
        warn: hex(&p.warning),
        danger: hex(&p.danger),
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
        Box::new(|cc| {
            let app = VibeMuxApp::new(cc, vm, config);
            Ok(Box::new(crate::live_app::LiveApp::new(
                app,
                cc.egui_ctx.clone(),
            )))
        }),
    )
}
