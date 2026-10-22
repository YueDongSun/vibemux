#![forbid(unsafe_code)]
//! The "Open a folder" dialog: a path field (typed or pasted), Browse on
//! platforms with a native picker, and the recent folders. It only reports
//! what the user chose; the app validates and switches folders.

use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText, Sense, Stroke, Ui, vec2};

use crate::{
    project_folder::{self, ProjectFolderError},
    theme::serialize::same_folder,
};

use super::{C, design, typography};

const DIALOG_WIDTH: f32 = 520.0;
const RECENT_ROW_HEIGHT: f32 = 44.0;

/// Open or closed, the typed path, and the last validation error.
#[derive(Debug, Default)]
pub struct FolderDialogState {
    pub open: bool,
    pub input: String,
    pub error: Option<ProjectFolderError>,
}

impl FolderDialogState {
    pub fn show(&mut self) {
        self.open = true;
        self.error = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.input.clear();
        self.error = None;
    }
}

/// What the user chose this frame.
#[derive(Debug, Default, PartialEq)]
pub struct FolderDialogActions {
    /// A path to open: typed and parsed, or a recent folder.
    pub open: Option<PathBuf>,
    pub forget: Option<PathBuf>,
    pub browse: bool,
    pub close: bool,
}

pub fn render(
    ctx: &egui::Context,
    c: &C,
    state: &mut FolderDialogState,
    recent: &[PathBuf],
    active: Option<&Path>,
) -> FolderDialogActions {
    let mut actions = FolderDialogActions::default();
    let response = egui::Modal::new(egui::Id::new("open_folder_dialog"))
        .frame(
            egui::Frame::new()
                .fill(c.raised)
                .stroke(Stroke::new(1.0, c.border))
                .corner_radius(design::CARD_RADIUS)
                .inner_margin(egui::Margin::same(22)),
        )
        .show(ctx, |ui| {
            ui.set_width(DIALOG_WIDTH);
            ui.label(
                RichText::new("Open a folder")
                    .font(typography::display_font(ui.ctx(), typography::TITLE_SIZE))
                    .color(c.txt),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new("VibeMux observes one project folder at a time.")
                    .size(13.0)
                    .color(c.muted),
            );
            ui.add_space(14.0);
            path_row(ui, c, state, &mut actions);
            if let Some(error) = state.error {
                ui.add_space(6.0);
                ui.label(RichText::new(error.message()).size(12.5).color(c.danger));
            }
            if !recent.is_empty() {
                ui.add_space(16.0);
                ui.label(RichText::new("Recent folders").size(12.0).color(c.muted));
                ui.add_space(4.0);
                for folder in recent {
                    let is_active = active.is_some_and(|active| same_folder(active, folder));
                    match recent_row(ui, c, folder, is_active) {
                        RecentChoice::Open => actions.open = Some(folder.clone()),
                        RecentChoice::Forget => actions.forget = Some(folder.clone()),
                        RecentChoice::None => {}
                    }
                }
            }
            ui.add_space(16.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if design::action_button(ui, c, "Cancel", true).clicked() {
                    actions.close = true;
                }
            });
        });
    if response.should_close() {
        actions.close = true;
    }
    actions
}

fn path_row(ui: &mut Ui, c: &C, state: &mut FolderDialogState, actions: &mut FolderDialogActions) {
    ui.horizontal(|ui| {
        let browse_width = if project_folder::native_picker_available() {
            92.0
        } else {
            0.0
        };
        let field_width = (ui.available_width() - browse_width - 78.0).max(160.0);
        let field = ui.add(
            egui::TextEdit::singleline(&mut state.input)
                .hint_text("C:\\Projects\\app")
                .desired_width(field_width)
                .margin(egui::Margin::symmetric(10, 7)),
        );
        if field.changed() {
            state.error = None;
        }
        let submitted = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if design::action_button(ui, c, "Open", !state.input.trim().is_empty()).clicked()
            || submitted
        {
            match project_folder::parse_folder_input(&state.input) {
                Ok(path) => actions.open = Some(path),
                Err(error) => state.error = Some(error),
            }
        }
        if project_folder::native_picker_available()
            && design::action_button(ui, c, "Browse…", true).clicked()
        {
            actions.browse = true;
        }
    });
}

enum RecentChoice {
    None,
    Open,
    Forget,
}

fn recent_row(ui: &mut Ui, c: &C, folder: &Path, active: bool) -> RecentChoice {
    let mut choice = RecentChoice::None;
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width(), RECENT_ROW_HEIGHT),
        Sense::click(),
    );
    if active || response.hovered() {
        ui.painter().rect_filled(
            rect,
            design::BUTTON_RADIUS,
            if active { c.accent_bg } else { c.surf },
        );
    }
    let left = rect.left() + 12.0;
    ui.painter().text(
        egui::pos2(left, rect.top() + 14.0),
        egui::Align2::LEFT_CENTER,
        project_folder::display_name(folder),
        egui::FontId::proportional(14.0),
        c.txt,
    );
    ui.painter().text(
        egui::pos2(left, rect.top() + 31.0),
        egui::Align2::LEFT_CENTER,
        folder.display().to_string(),
        egui::FontId::proportional(11.5),
        c.muted,
    );
    let forget_rect = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 20.0, rect.center().y),
        vec2(24.0, 24.0),
    );
    let forget = ui.interact(
        forget_rect,
        egui::Id::new(("forget_folder", folder)),
        Sense::click(),
    );
    if response.hovered() || forget.hovered() {
        ui.painter().text(
            forget_rect.center(),
            egui::Align2::CENTER_CENTER,
            "×",
            egui::FontId::proportional(16.0),
            if forget.hovered() { c.danger } else { c.muted },
        );
    }
    design::paint_focus_ring(ui, &response, design::BUTTON_RADIUS, c);
    let label = project_folder::display_name(folder);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
    let forget = forget.on_hover_text("Remove from recent folders");
    if forget.clicked() {
        choice = RecentChoice::Forget;
    } else if response.clicked() {
        choice = RecentChoice::Open;
    }
    choice
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn showing_and_closing_reset_the_dialog() {
        let mut state = FolderDialogState::default();
        state.show();
        assert!(state.open);
        state.input = "C:\\Projects".into();
        state.error = Some(ProjectFolderError::NotADirectory);
        state.close();
        assert!(!state.open);
        assert!(state.input.is_empty());
        assert_eq!(state.error, None);
    }
}
