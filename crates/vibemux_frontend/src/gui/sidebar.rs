#![forbid(unsafe_code)]
//! Claude Desktop style sidebar: brand row, New task, navigation, Recents,
//! and a workspace menu. Collapses to a 56 px icon rail (ADR 030 §4).
use super::{
    C,
    design::{self, Icon},
    supervisor_state::MainPage,
    typography,
};
use crate::supervisor_model::SupervisorSnapshot;
use egui::{self, RichText, Sense, Ui, vec2};

pub const SIDEBAR_WIDTH: f32 = 260.0;
pub const SIDEBAR_RAIL_WIDTH: f32 = 56.0;
pub const SIDEBAR_PANEL_ID: &str = "supervisor_sidebar";
pub const NEW_TASK_BUTTON_ID: &str = "sidebar_new_task";
pub const TOGGLE_BUTTON_ID: &str = "sidebar_toggle";
pub const WORKSPACE_BUTTON_ID: &str = "sidebar_workspace";
const ROW_HEIGHT: f32 = 34.0;
const RECENT_ROW_HEIGHT: f32 = 30.0;

pub struct SidebarView<'a> {
    pub snapshot: &'a SupervisorSnapshot,
    pub page: MainPage,
    pub welcome_active: bool,
    pub collapsed: bool,
    pub selected_task_id: Option<&'a str>,
}

#[derive(Default)]
pub struct SidebarActions {
    pub page: Option<MainPage>,
    pub open_settings: bool,
    pub open_diagnostics: bool,
    pub task_id: Option<String>,
    pub new_task: bool,
    pub toggle_collapsed: bool,
}

#[must_use]
pub const fn width_for(collapsed: bool) -> f32 {
    if collapsed {
        SIDEBAR_RAIL_WIDTH
    } else {
        SIDEBAR_WIDTH
    }
}

pub fn render(ui: &mut Ui, c: &C, view: &SidebarView<'_>) -> SidebarActions {
    let mut actions = SidebarActions::default();
    if view.collapsed {
        render_rail(ui, c, &mut actions);
    } else {
        render_expanded(ui, c, view, &mut actions);
    }
    actions
}

fn render_expanded(ui: &mut Ui, c: &C, view: &SidebarView<'_>, actions: &mut SidebarActions) {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(12, 14))
        .show(ui, |ui| {
            ui.set_min_width(SIDEBAR_WIDTH - 24.0);
            egui::TopBottomPanel::bottom("sidebar_workspace_panel")
                .frame(egui::Frame::NONE)
                .show_separator_line(false)
                .show_inside(ui, |ui| {
                    ui.add_space(8.0);
                    workspace_button(ui, c, view.snapshot, actions);
                });
            ui.horizontal(|ui| {
                design::spark(ui, c.accent, 22.0, 0.0);
                ui.add_space(4.0);
                ui.label(
                    RichText::new("VibeMux")
                        .font(typography::display_font(ui.ctx(), 19.0))
                        .color(c.txt),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let toggle = design::icon_button(
                        ui,
                        c,
                        Icon::SidebarToggle,
                        "Collapse sidebar",
                        egui::Id::new(TOGGLE_BUTTON_ID),
                    );
                    if toggle.clicked() {
                        actions.toggle_collapsed = true;
                    }
                });
            });
            ui.add_space(14.0);
            if new_task_row(ui, c) {
                actions.new_task = true;
            }
            ui.add_space(4.0);
            let conversation = view.page == MainPage::CoordinatorChat && !view.welcome_active;
            if design::nav_row(ui, c, "Conversation", Icon::Chat, conversation) {
                actions.page = Some(MainPage::CoordinatorChat);
            }
            if design::nav_row(ui, c, "Agents", Icon::Grid, view.page == MainPage::Agents) {
                actions.page = Some(MainPage::Agents);
            }
            ui.add_space(18.0);
            ui.label(RichText::new("Recents").size(12.0).color(c.muted));
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .id_salt("sidebar_recents")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if view.snapshot.tasks.is_empty() {
                        ui.label(
                            RichText::new("Tasks appear here when assigned.")
                                .size(12.0)
                                .color(c.muted),
                        );
                    }
                    for task in &view.snapshot.tasks {
                        let selected = view.selected_task_id == Some(task.task_id.as_str());
                        if recent_row(ui, c, &task.title, &task.state, selected).clicked() {
                            actions.task_id = Some(task.task_id.clone());
                        }
                    }
                });
        });
}

fn render_rail(ui: &mut Ui, c: &C, actions: &mut SidebarActions) {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(0, 14))
        .show(ui, |ui| {
            ui.set_min_width(SIDEBAR_RAIL_WIDTH);
            ui.vertical_centered(|ui| {
                let buttons = [
                    (Icon::SidebarToggle, "Expand sidebar", TOGGLE_BUTTON_ID),
                    (Icon::Plus, "New task", NEW_TASK_BUTTON_ID),
                    (Icon::Chat, "Conversation", "sidebar_rail_conversation"),
                    (Icon::Grid, "Agents", "sidebar_rail_agents"),
                    (Icon::Settings, "Settings", "sidebar_rail_settings"),
                ];
                for (kind, tooltip, id) in buttons {
                    if design::icon_button(ui, c, kind, tooltip, egui::Id::new(id)).clicked() {
                        match kind {
                            Icon::SidebarToggle => actions.toggle_collapsed = true,
                            Icon::Plus => actions.new_task = true,
                            Icon::Chat => actions.page = Some(MainPage::CoordinatorChat),
                            Icon::Grid => actions.page = Some(MainPage::Agents),
                            _ => actions.open_settings = true,
                        }
                    }
                    ui.add_space(6.0);
                }
            });
        });
}

fn new_task_row(ui: &mut Ui, c: &C) -> bool {
    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), ROW_HEIGHT));
    let response = ui.interact(rect, egui::Id::new(NEW_TASK_BUTTON_ID), Sense::click());
    if response.hovered() || response.has_focus() {
        ui.painter()
            .rect_filled(rect, design::BUTTON_RADIUS, c.raised);
    }
    let circle = egui::pos2(rect.left() + 18.0, rect.center().y);
    ui.painter().circle_filled(circle, 10.0, c.accent);
    let plus_color = if c.light { egui::Color32::WHITE } else { c.bg };
    let plus = egui::Stroke::new(1.6, plus_color);
    ui.painter()
        .line_segment([circle - vec2(4.5, 0.0), circle + vec2(4.5, 0.0)], plus);
    ui.painter()
        .line_segment([circle - vec2(0.0, 4.5), circle + vec2(0.0, 4.5)], plus);
    ui.painter().text(
        egui::pos2(rect.left() + 38.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "New task",
        egui::FontId::proportional(14.0),
        c.txt,
    );
    design::paint_focus_ring(ui, &response, design::BUTTON_RADIUS, c);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "New task"));
    response.clicked()
}

fn recent_row(ui: &mut Ui, c: &C, title: &str, state: &str, selected: bool) -> egui::Response {
    let title = if title.trim().is_empty() {
        "Untitled task"
    } else {
        title
    };
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width(), RECENT_ROW_HEIGHT),
        Sense::click(),
    );
    if selected || response.hovered() {
        ui.painter().rect_filled(
            rect,
            design::BUTTON_RADIUS,
            if selected { c.accent_bg } else { c.raised },
        );
    }
    if matches!(
        state,
        "in_progress" | "running" | "blocked" | "input_required"
    ) {
        ui.painter().circle_filled(
            egui::pos2(rect.left() + 9.0, rect.center().y),
            3.0,
            design::state_color(c, state),
        );
    }
    let text_left = rect.left() + 20.0;
    let galley = egui::WidgetText::from(RichText::new(title).size(13.0).color(if selected {
        c.txt
    } else {
        c.muted
    }))
    .into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        (rect.right() - 6.0 - text_left).max(0.0),
        egui::TextStyle::Button,
    );
    ui.painter().galley(
        egui::pos2(text_left, rect.center().y - galley.size().y / 2.0),
        galley,
        c.muted,
    );
    design::paint_focus_ring(ui, &response, design::BUTTON_RADIUS, c);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, title));
    response.on_hover_text(title)
}

fn workspace_button(
    ui: &mut Ui,
    c: &C,
    snapshot: &SupervisorSnapshot,
    actions: &mut SidebarActions,
) {
    let name = if snapshot.project_name.trim().is_empty() {
        "Local workspace"
    } else {
        snapshot.project_name.as_str()
    };
    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), 42.0));
    let response = ui.interact(rect, egui::Id::new(WORKSPACE_BUTTON_ID), Sense::click());
    if response.hovered() || response.has_focus() {
        ui.painter()
            .rect_filled(rect, design::BUTTON_RADIUS, c.raised);
    }
    let initial: String = name
        .chars()
        .next()
        .map_or_else(|| "V".to_string(), |first| first.to_uppercase().collect());
    let badge = egui::pos2(rect.left() + 18.0, rect.center().y);
    ui.painter().circle_filled(badge, 13.0, c.accent_bg);
    ui.painter().text(
        badge,
        egui::Align2::CENTER_CENTER,
        initial,
        egui::FontId::proportional(13.0),
        c.accent_text,
    );
    let text_left = rect.left() + 40.0;
    ui.painter().text(
        egui::pos2(text_left, rect.center().y - 7.0),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(13.0),
        c.txt,
    );
    ui.painter().text(
        egui::pos2(text_left, rect.center().y + 9.0),
        egui::Align2::LEFT_CENTER,
        "Local workspace",
        egui::FontId::proportional(11.0),
        c.muted,
    );
    design::paint_focus_ring(ui, &response, design::BUTTON_RADIUS, c);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Workspace menu")
    });
    egui::Popup::menu(&response).show(|ui| {
        ui.set_min_width(200.0);
        if ui.button("Settings").clicked() {
            actions.open_settings = true;
        }
        if ui.button("Diagnostics").clicked() {
            actions.open_diagnostics = true;
        }
    });
}
