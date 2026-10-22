#![forbid(unsafe_code)]
//! Full-page settings in the Claude style: a section list on the left and
//! rounded cards of rows on the right, each row with its label and
//! description on the left and its control on the right.
//!
//! Saved preferences change only through the returned [`SettingsChanges`].
//! The Connection section reads the supervisor snapshot and never starts,
//! stops, or changes the daemon.

use std::path::Path;

use eframe::egui::{self, RichText, Sense, Stroke, Ui, vec2};

use crate::{
    daemon_control::DaemonCommand,
    supervisor_model::{ConnectionStatus, SnapshotMode, SupervisorSnapshot},
    theme::{ALL_DENSITIES, Density, StartPage, TextSize, ThemeId, UserConfig, WindowSize},
};

use super::{
    C,
    daemon_card::{self, DaemonJob},
    design, shortcuts, typography,
};

/// Width of the section list.
const NAV_WIDTH: f32 = 196.0;
/// The content column never grows wider than this.
const CONTENT_MAX_WIDTH: f32 = 720.0;
/// Width reserved for a row's control.
const CONTROL_WIDTH: f32 = 300.0;
/// Size of one theme preview card.
const THEME_CARD_SIZE: egui::Vec2 = egui::Vec2::new(152.0, 104.0);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SettingsSection {
    #[default]
    General,
    Appearance,
    Connection,
    Shortcuts,
    About,
}

impl SettingsSection {
    pub const ALL: [Self; 5] = [
        Self::General,
        Self::Appearance,
        Self::Connection,
        Self::Shortcuts,
        Self::About,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Appearance => "Appearance",
            Self::Connection => "Connection",
            Self::Shortcuts => "Keyboard shortcuts",
            Self::About => "About",
        }
    }
}

/// What the page reads to draw itself.
pub struct SettingsView<'a> {
    pub section: SettingsSection,
    pub config: &'a UserConfig,
    pub snapshot: &'a SupervisorSnapshot,
    /// The folder the GUI observes, if any.
    pub project_folder: Option<&'a Path>,
    pub daemon_job: &'a DaemonJob,
}

/// What the user asked for this frame. The caller applies and persists it.
#[derive(Debug, Default, PartialEq)]
pub struct SettingsChanges {
    pub section: Option<SettingsSection>,
    pub close: bool,
    pub theme: Option<ThemeId>,
    pub window_size: Option<WindowSize>,
    pub start_page: Option<StartPage>,
    pub density: Option<Density>,
    pub text_size: Option<TextSize>,
    pub open_folder_dialog: bool,
    pub daemon: Option<DaemonCommand>,
}

pub fn render_page(ui: &mut Ui, c: &C, view: &SettingsView<'_>) -> SettingsChanges {
    let mut changes = SettingsChanges::default();
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            vec2(NAV_WIDTH, ui.available_height()),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(NAV_WIDTH);
                if back_button(ui, c).clicked() {
                    changes.close = true;
                }
                ui.add_space(18.0);
                for section in SettingsSection::ALL {
                    if nav_item(ui, c, section.label(), section == view.section) {
                        changes.section = Some(section);
                    }
                    ui.add_space(2.0);
                }
            },
        );
        ui.add_space(32.0);
        egui::ScrollArea::vertical()
            .id_salt("settings_content")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // The scroll area sits in a horizontal row; its content is a
                // top-down column of its own.
                ui.vertical(|ui| {
                    ui.set_max_width(CONTENT_MAX_WIDTH);
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(view.section.label())
                            .font(typography::display_font(ui.ctx(), typography::TITLE_SIZE))
                            .color(c.txt),
                    );
                    ui.add_space(18.0);
                    match view.section {
                        SettingsSection::General => general(ui, c, view, &mut changes),
                        SettingsSection::Appearance => {
                            appearance(ui, c, view.config, &mut changes);
                        }
                        SettingsSection::Connection => connection(ui, c, view, &mut changes),
                        SettingsSection::Shortcuts => keyboard_shortcuts(ui, c),
                        SettingsSection::About => about(ui, c),
                    }
                    ui.add_space(24.0);
                });
            });
    });
    changes
}

fn general(ui: &mut Ui, c: &C, view: &SettingsView<'_>, changes: &mut SettingsChanges) {
    let config = view.config;
    let folder = folder_text(view.project_folder);
    card(ui, c, |ui| {
        row(ui, c, "Project folder", &folder, |ui| {
            if design::action_button(ui, c, "Change…", true).clicked() {
                changes.open_folder_dialog = true;
            }
        });
        separator(ui, c);
        row(ui, c, "Start page", "The page VibeMux opens on.", |ui| {
            changes.start_page =
                segmented(ui, c, &StartPage::ALL, config.start_page, StartPage::label);
        });
        separator(ui, c);
        row(
            ui,
            c,
            "Startup window size",
            "Used the next time VibeMux opens.",
            |ui| {
                // A right-to-left row: widgets are added from the right.
                let mut size = config.window_size;
                let height = ui.add(
                    egui::DragValue::new(&mut size.height)
                        .range(WindowSize::MIN_HEIGHT..=WindowSize::MAX_HEIGHT)
                        .suffix(" px"),
                );
                ui.label(RichText::new("Height").size(13.0).color(c.muted));
                ui.add_space(10.0);
                let width = ui.add(
                    egui::DragValue::new(&mut size.width)
                        .range(WindowSize::MIN_WIDTH..=WindowSize::MAX_WIDTH)
                        .suffix(" px"),
                );
                ui.label(RichText::new("Width").size(13.0).color(c.muted));
                if (width.changed() || height.changed()) && size != config.window_size {
                    changes.window_size = Some(size);
                }
            },
        );
    });
}

fn appearance(ui: &mut Ui, c: &C, config: &UserConfig, changes: &mut SettingsChanges) {
    sub_heading(ui, c, "Theme");
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(12.0, 12.0);
        for theme in ThemeId::ALL {
            if theme_card(ui, c, theme, theme == config.theme).clicked() {
                changes.theme = Some(theme);
            }
        }
    });
    ui.add_space(22.0);
    card(ui, c, |ui| {
        row(ui, c, "Density", "Spacing between controls.", |ui| {
            changes.density = segmented(ui, c, &ALL_DENSITIES, config.density, density_label);
        });
        separator(ui, c);
        row(
            ui,
            c,
            "Text size",
            "Scales text and controls together.",
            |ui| {
                changes.text_size =
                    segmented(ui, c, &TextSize::ALL, config.text_size, TextSize::label);
            },
        );
    });
}

fn connection(ui: &mut Ui, c: &C, view: &SettingsView<'_>, changes: &mut SettingsChanges) {
    let snapshot = view.snapshot;
    card(ui, c, |ui| {
        value_row(ui, c, "Folder", &folder_text(view.project_folder));
        separator(ui, c);
        let (description, control) = daemon_control_state(snapshot, view.daemon_job);
        row(ui, c, "VibeMux", description, |ui| match control {
            DaemonControl::Busy(label) => {
                ui.label(RichText::new(label).size(13.0).color(c.muted));
                ui.spinner();
            }
            DaemonControl::Start => {
                if daemon_card::start_button(ui, c).clicked() {
                    changes.daemon = Some(DaemonCommand::Start);
                }
            }
            DaemonControl::Stop => {
                if design::action_button(ui, c, "Stop VibeMux", true).clicked() {
                    changes.daemon = Some(DaemonCommand::Stop);
                }
            }
            DaemonControl::None => {}
        });
        if let DaemonJob::Failed(message) = view.daemon_job {
            ui.add(egui::Label::new(RichText::new(message).size(12.5).color(c.danger)).wrap());
            ui.add_space(10.0);
        }
    });
    ui.add_space(14.0);
    let status = format!("{:?}", snapshot.connection.status);
    let data = match snapshot.mode {
        SnapshotMode::Live => "Live daemon data",
        SnapshotMode::Demo => "Demo data",
    };
    card(ui, c, |ui| {
        value_row(ui, c, "Project", &snapshot.project_name);
        separator(ui, c);
        value_row(ui, c, "Status", &status);
        separator(ui, c);
        value_row(
            ui,
            c,
            "Details",
            snapshot
                .connection
                .detail
                .as_deref()
                .unwrap_or("None reported"),
        );
        separator(ui, c);
        value_row(ui, c, "Data", data);
        separator(ui, c);
        value_row(
            ui,
            c,
            "Last update",
            snapshot.observed_at.as_deref().unwrap_or("Not yet"),
        );
    });
    ui.add_space(10.0);
    footnote(
        ui,
        c,
        "These rows only report what the daemon says. VibeMux starts or stops only when you click.",
    );
}

fn keyboard_shortcuts(ui: &mut Ui, c: &C) {
    card(ui, c, |ui| {
        let other = shortcuts::OTHER_KEYS
            .iter()
            .map(|(keys, description)| (*keys, *description));
        let rows: Vec<(&str, &str)> = shortcuts::SHORTCUTS
            .iter()
            .map(|shortcut| (shortcut.keys_label, shortcut.description))
            .chain(other)
            .collect();
        for (index, (keys, description)) in rows.iter().enumerate() {
            if index > 0 {
                separator(ui, c);
            }
            row(ui, c, description, "", |ui| key_chip(ui, c, keys));
        }
    });
}

fn about(ui: &mut Ui, c: &C) {
    let build = if cfg!(debug_assertions) {
        "Debug"
    } else {
        "Release"
    };
    let platform = format!("{} · {}", std::env::consts::OS, std::env::consts::ARCH);
    let settings_file = crate::theme::serialize::config_path();
    let error_log = crate::startup_log::log_path();
    card(ui, c, |ui| {
        value_row(ui, c, "Version", env!("CARGO_PKG_VERSION"));
        separator(ui, c);
        value_row(ui, c, "Build", build);
        separator(ui, c);
        value_row(ui, c, "Platform", &platform);
        separator(ui, c);
        value_row(ui, c, "Settings file", &settings_file.display().to_string());
        separator(ui, c);
        value_row(ui, c, "Startup error log", &error_log.display().to_string());
    });
}

/// What the VibeMux row offers for this connection and job.
enum DaemonControl {
    Busy(&'static str),
    Start,
    Stop,
    None,
}

fn daemon_control_state(
    snapshot: &SupervisorSnapshot,
    job: &DaemonJob,
) -> (&'static str, DaemonControl) {
    if let DaemonJob::Running(command) = job {
        return (
            "Working on it.",
            DaemonControl::Busy(command.progress_label()),
        );
    }
    if daemon_card::daemon_missing(&snapshot.connection) {
        return ("Not running for this folder.", DaemonControl::Start);
    }
    match snapshot.connection.status {
        ConnectionStatus::Connected => ("Running for this folder.", DaemonControl::Stop),
        _ => ("Connecting or unavailable.", DaemonControl::None),
    }
}

fn folder_text(folder: Option<&Path>) -> String {
    folder.map_or_else(
        || "No folder open".to_string(),
        |folder| folder.display().to_string(),
    )
}

const fn density_label(density: Density) -> &'static str {
    match density {
        Density::Comfortable => "Comfortable",
        Density::Compact => "Compact",
    }
}

fn back_button(ui: &mut Ui, c: &C) -> egui::Response {
    let response = ui.add(
        egui::Button::new(RichText::new("←  Back").size(14.0).color(c.muted))
            .frame(false)
            .corner_radius(design::BUTTON_RADIUS)
            .min_size(vec2(0.0, 32.0)),
    );
    design::paint_focus_ring(ui, &response, design::BUTTON_RADIUS, c);
    response
}

fn nav_item(ui: &mut Ui, c: &C, label: &str, selected: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
    if selected || response.hovered() {
        ui.painter().rect_filled(
            rect,
            design::BUTTON_RADIUS,
            if selected { c.accent_bg } else { c.raised },
        );
    }
    ui.painter().text(
        rect.left_center() + vec2(12.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.0),
        if selected { c.txt } else { c.muted },
    );
    design::paint_focus_ring(ui, &response, design::BUTTON_RADIUS, c);
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, label)
    });
    response.clicked()
        || (response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)))
}

fn sub_heading(ui: &mut Ui, c: &C, text: &str) {
    ui.label(RichText::new(text).size(15.0).strong().color(c.txt));
    ui.add_space(10.0);
}

fn footnote(ui: &mut Ui, c: &C, text: &str) {
    ui.label(RichText::new(text).size(12.5).color(c.muted));
}

fn card(ui: &mut Ui, c: &C, body: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(c.raised)
        .stroke(Stroke::new(1.0, c.border))
        .corner_radius(design::CARD_RADIUS)
        .inner_margin(egui::Margin::symmetric(20, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            body(ui);
        });
}

fn separator(ui: &mut Ui, c: &C) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter()
        .hline(rect.x_range(), rect.center().y, Stroke::new(1.0, c.border));
}

/// One settings row: title and description on the left; `control` draws in
/// a right-to-left area on the right, so it adds its widgets from the right.
fn row(ui: &mut Ui, c: &C, title: &str, description: &str, control: impl FnOnce(&mut Ui)) {
    ui.add_space(12.0);
    let total = ui.available_width();
    let control_width = CONTROL_WIDTH.min(total * 0.55);
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            vec2(total - control_width, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(total - control_width);
                ui.label(RichText::new(title).size(14.0).color(c.txt));
                if !description.is_empty() {
                    ui.add_space(2.0);
                    ui.label(RichText::new(description).size(12.5).color(c.muted));
                }
            },
        );
        ui.allocate_ui_with_layout(
            vec2(control_width, 0.0),
            egui::Layout::right_to_left(egui::Align::Center),
            control,
        );
    });
    ui.add_space(12.0);
}

/// A read-only row whose value is selectable text on the right.
fn value_row(ui: &mut Ui, c: &C, title: &str, value: &str) {
    row(ui, c, title, "", |ui| {
        ui.add(
            egui::Label::new(RichText::new(value).size(13.0).color(c.muted))
                .wrap()
                .selectable(true),
        );
    });
}

fn key_chip(ui: &mut Ui, c: &C, keys: &str) {
    egui::Frame::new()
        .fill(c.surf)
        .stroke(Stroke::new(1.0, c.border))
        .corner_radius(design::PILL_RADIUS)
        .inner_margin(egui::Margin::symmetric(10, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(keys).size(12.5).monospace().color(c.txt));
        });
}

/// A pill-shaped segmented choice. Returns the option clicked, if it differs
/// from `current`. Works in left-to-right and right-to-left layouts.
fn segmented<T: Copy + Eq>(
    ui: &mut Ui,
    c: &C,
    options: &[T],
    current: T,
    label: fn(T) -> &'static str,
) -> Option<T> {
    let mut picked = None;
    egui::Frame::new()
        .fill(c.surf)
        .stroke(Stroke::new(1.0, c.border))
        .corner_radius(design::PILL_RADIUS)
        .inner_margin(egui::Margin::same(3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let right_to_left = ui.layout().prefer_right_to_left();
                let ordered: Vec<T> = if right_to_left {
                    options.iter().rev().copied().collect()
                } else {
                    options.to_vec()
                };
                for option in ordered {
                    let selected = option == current;
                    let text = RichText::new(label(option)).size(13.0).color(if selected {
                        c.txt
                    } else {
                        c.muted
                    });
                    let mut button = egui::Button::new(text)
                        .corner_radius(design::PILL_RADIUS)
                        .min_size(vec2(0.0, 26.0));
                    // An unselected option keeps a transparent frame, so every
                    // option has the same padding and baseline.
                    button = if selected {
                        button.fill(c.raised).stroke(Stroke::new(1.0, c.border))
                    } else {
                        button.fill(egui::Color32::TRANSPARENT).stroke(Stroke::NONE)
                    };
                    let response = ui.add(button);
                    design::paint_focus_ring(ui, &response, design::PILL_RADIUS, c);
                    if response.clicked() && !selected {
                        picked = Some(option);
                    }
                }
            });
        });
    picked
}

/// A clickable preview of `theme`: a miniature window in its own colors,
/// with its readable name below.
fn theme_card(ui: &mut Ui, c: &C, theme: ThemeId, selected: bool) -> egui::Response {
    let palette = crate::theme::palette_for(theme);
    let colors = super::pal(&palette);
    let (rect, response) = ui.allocate_exact_size(THEME_CARD_SIZE, Sense::click());
    let painter = ui.painter();
    let ring = if selected {
        Stroke::new(2.0, c.accent)
    } else if response.hovered() {
        Stroke::new(1.0, c.accent)
    } else {
        Stroke::new(1.0, c.border)
    };
    painter.rect_filled(rect, design::CARD_RADIUS, c.raised);
    // The miniature: the theme's canvas with a sidebar, two text lines, and
    // an accent button.
    let preview =
        egui::Rect::from_min_size(rect.min + vec2(8.0, 8.0), vec2(rect.width() - 16.0, 62.0));
    painter.rect_filled(preview, design::BUTTON_RADIUS, colors.bg);
    let sidebar = egui::Rect::from_min_size(preview.min, vec2(34.0, preview.height()));
    painter.rect_filled(
        sidebar,
        egui::CornerRadius {
            nw: design::BUTTON_RADIUS,
            sw: design::BUTTON_RADIUS,
            ne: 0,
            se: 0,
        },
        colors.surf,
    );
    let text_left = sidebar.right() + 10.0;
    painter.hline(
        text_left..=preview.right() - 26.0,
        preview.top() + 18.0,
        Stroke::new(4.0, colors.txt),
    );
    painter.hline(
        text_left..=preview.right() - 46.0,
        preview.top() + 30.0,
        Stroke::new(4.0, colors.muted),
    );
    let pill = egui::Rect::from_min_size(
        egui::pos2(text_left, preview.bottom() - 20.0),
        vec2(40.0, 12.0),
    );
    painter.rect_filled(pill, design::PILL_RADIUS, colors.accent);
    painter.text(
        egui::pos2(rect.left() + 12.0, rect.bottom() - 17.0),
        egui::Align2::LEFT_CENTER,
        theme.display_name(),
        egui::FontId::proportional(13.0),
        if selected { c.txt } else { c.muted },
    );
    painter.rect_stroke(rect, design::CARD_RADIUS, ring, egui::StrokeKind::Inside);
    design::paint_focus_ring(ui, &response, design::CARD_RADIUS, c);
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::RadioButton,
            true,
            selected,
            theme.display_name(),
        )
    });
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_section_has_a_distinct_label() {
        let labels: std::collections::BTreeSet<_> = SettingsSection::ALL
            .iter()
            .map(|section| section.label())
            .collect();
        assert_eq!(labels.len(), SettingsSection::ALL.len());
        assert_eq!(SettingsSection::default(), SettingsSection::General);
    }

    #[test]
    fn theme_names_are_readable_and_distinct() {
        let names: std::collections::BTreeSet<_> = ThemeId::ALL
            .iter()
            .map(|theme| theme.display_name())
            .collect();
        assert_eq!(names.len(), ThemeId::ALL.len());
        assert!(names.iter().all(|name| !name.contains('_')));
        assert_eq!(ThemeId::ClaudeLight.display_name(), "Claude Light");
        assert_eq!(ThemeId::Claude.display_name(), "Claude Dark");
    }

    #[test]
    fn densities_have_labels() {
        assert_eq!(density_label(Density::Comfortable), "Comfortable");
        assert_eq!(density_label(Density::Compact), "Compact");
    }
}
