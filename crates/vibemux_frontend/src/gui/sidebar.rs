#![forbid(unsafe_code)]
//! Project navigation and real task shortcuts; appearance lives in Settings.
use super::{
    C,
    design::{self, Icon},
    supervisor_state::MainPage,
    typography,
};
use crate::{supervisor_model::SupervisorSnapshot, theme::ThemeId};
use egui::{self, RichText, Ui};
pub const SIDEBAR_WIDTH: f32 = 236.0;
#[derive(Default)]
pub struct SidebarActions {
    pub page: Option<MainPage>,
    pub open_settings: bool,
    pub open_diagnostics: bool,
    pub task_id: Option<String>,
}
pub fn render(
    ui: &mut Ui,
    c: &C,
    snapshot: &SupervisorSnapshot,
    page: MainPage,
    _theme: ThemeId,
    settings_open: bool,
    diagnostics_open: bool,
) -> SidebarActions {
    let mut actions = SidebarActions::default();
    egui::Frame::new()
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_min_width(SIDEBAR_WIDTH - 32.0);
            egui::TopBottomPanel::bottom("workspace_sidebar_utilities")
                .exact_height(130.0)
                .frame(egui::Frame::NONE)
                .show_inside(ui, |ui| {
                    ui.add_space(14.0);
                    actions.open_diagnostics =
                        design::nav_row(ui, c, "Diagnostics", Icon::Activity, diagnostics_open);
                    actions.open_settings =
                        design::nav_row(ui, c, "Settings", Icon::Settings, settings_open);
                    ui.add_space(8.0);
                    ui.label(RichText::new("Local workspace").size(11.0).color(c.muted));
                });
            ui.horizontal(|ui| {
                design::spark(ui, c.accent, 22.0, 0.0);
                ui.add_space(4.0);
                ui.label(
                    RichText::new("VibeMux")
                        .font(typography::display_font(ui.ctx(), 19.0))
                        .color(c.txt),
                );
            });
            ui.add_space(28.0);
            egui::Frame::new()
                .fill(c.raised)
                .corner_radius(8)
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.label(RichText::new("WORKSPACE").size(10.0).color(c.muted));
                    ui.add_space(4.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new(&snapshot.project_name)
                                .size(14.0)
                                .strong()
                                .color(c.txt),
                        )
                        .wrap(),
                    );
                });
            ui.add_space(26.0);
            if design::nav_row(
                ui,
                c,
                "Main conversation",
                Icon::Chat,
                page == MainPage::CoordinatorChat,
            ) {
                actions.page = Some(MainPage::CoordinatorChat);
            }
            if design::nav_row(ui, c, "Agents", Icon::Grid, page == MainPage::Agents) {
                actions.page = Some(MainPage::Agents);
            }
            ui.add_space(25.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("TASKS").size(10.5).color(c.muted));
                ui.label(
                    RichText::new(snapshot.tasks.len().to_string())
                        .size(10.5)
                        .color(c.muted),
                );
            });
            ui.add_space(8.0);
            egui::ScrollArea::vertical()
                .id_salt("sidebar_tasks")
                .show(ui, |ui| {
                    if snapshot.tasks.is_empty() {
                        ui.label(
                            RichText::new("Tasks appear here when assigned.")
                                .size(12.0)
                                .color(c.muted),
                        );
                    }
                    for task in &snapshot.tasks {
                        ui.horizontal(|ui| {
                            let (rect, _) =
                                ui.allocate_exact_size(egui::vec2(6.0, 6.0), egui::Sense::hover());
                            ui.painter().circle_filled(
                                rect.center(),
                                2.5,
                                design::state_color(c, &task.state),
                            );
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(&task.title).size(12.0).color(c.muted),
                                    )
                                    .frame(false)
                                    .wrap(),
                                )
                                .clicked()
                            {
                                actions.task_id = Some(task.task_id.clone());
                            }
                        });
                        ui.add_space(5.0);
                    }
                });
        });
    actions
}
