#![forbid(unsafe_code)]
//! Persisted theme and startup-window preferences.

use eframe::egui::{self, RichText};

use crate::localization::Language;
use crate::theme::{ThemeId, WindowSize};

use super::C;

#[derive(Default)]
pub struct SettingsChanges {
    pub theme: Option<ThemeId>,
    pub window_size: Option<WindowSize>,
    pub language: Option<Language>,
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
    egui::Window::new(colors.tr("Settings"))
        .id(egui::Id::new("workspace_settings"))
        .open(open)
        .collapsible(false)
        .resizable(false)
        .default_width(400.0)
        .show(ctx, |ui| {
            if demo_mode {
                ui.label(
                    RichText::new(colors.tr("DEMO DATA"))
                        .size(11.0)
                        .strong()
                        .color(colors.warn),
                );
                ui.add_space(8.0);
            }
            ui.label(
                RichText::new(colors.tr("Interface language"))
                    .size(14.0)
                    .strong()
                    .color(colors.txt),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                for (language, label) in [
                    (Language::English, "English"),
                    (Language::Chinese, "Chinese (Simplified)"),
                ] {
                    if ui
                        .add(
                            egui::Button::new(colors.tr(label))
                                .selected(language == colors.language)
                                .corner_radius(u8::MAX)
                                .min_size(egui::vec2(110.0, 32.0)),
                        )
                        .clicked()
                    {
                        changes.language = Some(language);
                    }
                }
            });
            ui.label(
                RichText::new(
                    colors.tr(
                        "Applies immediately. Your task content stays in its original language.",
                    ),
                )
                .size(12.0)
                .color(colors.muted),
            );
            ui.add_space(18.0);
            ui.label(
                RichText::new(colors.tr("Appearance"))
                    .size(14.0)
                    .strong()
                    .color(colors.txt),
            );
            ui.add_space(6.0);
            ui.label(
                RichText::new(colors.tr("Choose a palette for your workspace."))
                    .size(12.0)
                    .color(colors.muted),
            );
            ui.horizontal_wrapped(|ui| {
                for theme in ThemeId::ALL {
                    let active = theme == theme_id;
                    if ui
                        .add(
                            egui::Button::new(theme.as_str())
                                .selected(active)
                                .corner_radius(u8::MAX)
                                .min_size(egui::vec2(72.0, 30.0)),
                        )
                        .clicked()
                    {
                        changes.theme = Some(theme);
                    }
                }
            });
            ui.add_space(18.0);
            ui.label(
                RichText::new(colors.tr("Startup window size"))
                    .size(14.0)
                    .strong()
                    .color(colors.txt),
            );
            ui.add_space(5.0);
            let mut width = window_size.width;
            let mut height = window_size.height;
            ui.horizontal(|ui| {
                ui.label(colors.tr("Width"));
                ui.add(egui::DragValue::new(&mut width).range(800..=3840));
                ui.add_space(8.0);
                ui.label(colors.tr("Height"));
                ui.add(egui::DragValue::new(&mut height).range(600..=2160));
            });
            if width != window_size.width || height != window_size.height {
                changes.window_size = Some(WindowSize { width, height });
            }
            ui.add_space(5.0);
            ui.label(
                RichText::new(colors.tr("Saved for the next launch."))
                    .size(12.0)
                    .color(colors.faint),
            );
        });
    changes
}
