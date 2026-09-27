#![forbid(unsafe_code)]
//! Persisted theme and startup-window preferences.

use eframe::egui::{self, RichText};

use crate::theme::{ThemeId, WindowSize};

use super::C;

#[derive(Default)]
pub struct SettingsChanges {
    pub theme: Option<ThemeId>,
    pub window_size: Option<WindowSize>,
}

pub fn render(
    ctx: &egui::Context,
    colors: &C,
    open: &mut bool,
    theme_id: ThemeId,
    window_size: WindowSize,
    demo_mode: bool,
) -> SettingsChanges {
    let mut changes = SettingsChanges::default();
    egui::Window::new("Settings")
        .open(open)
        .collapsible(false)
        .resizable(false)
        .default_width(400.0)
        .show(ctx, |ui| {
            if demo_mode {
                ui.label(
                    RichText::new("DEMO DATA")
                        .size(11.0)
                        .strong()
                        .color(colors.warn),
                );
                ui.add_space(8.0);
            }
            ui.label(
                RichText::new("Appearance")
                    .size(14.0)
                    .strong()
                    .color(colors.txt),
            );
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                for theme in ThemeId::ALL {
                    let active = theme == theme_id;
                    if ui
                        .add(
                            egui::Button::new(theme.as_str())
                                .selected(active)
                                .corner_radius(7),
                        )
                        .clicked()
                    {
                        changes.theme = Some(theme);
                    }
                }
            });
            ui.add_space(18.0);
            ui.label(
                RichText::new("Startup window size")
                    .size(14.0)
                    .strong()
                    .color(colors.txt),
            );
            ui.add_space(5.0);
            let mut width = window_size.width;
            let mut height = window_size.height;
            ui.horizontal(|ui| {
                ui.label("Width");
                ui.add(egui::DragValue::new(&mut width).range(800..=3840));
                ui.add_space(8.0);
                ui.label("Height");
                ui.add(egui::DragValue::new(&mut height).range(600..=2160));
            });
            if width != window_size.width || height != window_size.height {
                changes.window_size = Some(WindowSize { width, height });
            }
            ui.add_space(5.0);
            ui.label(
                RichText::new("Saved for the next launch.")
                    .size(12.0)
                    .color(colors.faint),
            );
        });
    changes
}
