#![forbid(unsafe_code)]
//! Read-only harness details sourced from the local probe view model.

use eframe::egui::{self, RichText, Ui};
use vibemux_probe::ProbeState;

use crate::{view_model::ViewModel, view_model::route_label};

use super::C;

#[derive(Default)]
pub struct AgentsActions {
    pub selected_agent: Option<usize>,
    pub back_to_list: bool,
}

pub fn render(
    ui: &mut Ui,
    c: &C,
    view_model: &ViewModel,
    selected: Option<usize>,
) -> AgentsActions {
    let mut actions = AgentsActions::default();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_max_width(900.0);
            if let Some(index) = selected {
                actions.back_to_list = render_detail(ui, c, view_model, index);
            } else {
                render_list(ui, c, view_model, &mut actions);
            }
        });
    actions
}

fn render_list(ui: &mut Ui, c: &C, view_model: &ViewModel, actions: &mut AgentsActions) {
    ui.label(RichText::new("Agents").size(20.0).strong().color(c.txt));
    ui.add_space(4.0);
    ui.label(
        RichText::new("Read-only launcher and route diagnostics from the local probe.")
            .size(13.0)
            .color(c.muted),
    );
    ui.add_space(18.0);

    if view_model.agents.is_empty() {
        ui.label(RichText::new("No agent probe results are available.").color(c.muted));
        return;
    }

    for (index, agent) in view_model.agents.iter().enumerate() {
        egui::Frame::group(ui.style())
            .fill(c.surf)
            .stroke(egui::Stroke::new(1.0, c.border))
            .corner_radius(egui::CornerRadius::same(7))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(agent.name.as_str())
                                .size(15.0)
                                .strong()
                                .color(c.txt),
                        );
                        ui.add_space(3.0);
                        ui.label(
                            RichText::new(format!(
                                "Launcher: {}  ·  Route: {}",
                                probe_state_label(agent.launcher_state),
                                route_label(agent.route)
                            ))
                            .size(12.0)
                            .color(c.muted),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Details").clicked() {
                            actions.selected_agent = Some(index);
                        }
                    });
                });
            });
        ui.add_space(8.0);
    }
}

fn render_detail(ui: &mut Ui, c: &C, view_model: &ViewModel, index: usize) -> bool {
    let Some(agent) = view_model.agents.get(index) else {
        return true;
    };
    if ui.button("←  All agents").clicked() {
        return true;
    }
    ui.add_space(12.0);
    ui.label(
        RichText::new(agent.name.as_str())
            .size(20.0)
            .strong()
            .color(c.txt),
    );
    ui.add_space(14.0);
    egui::Frame::group(ui.style())
        .fill(c.surf)
        .stroke(egui::Stroke::new(1.0, c.border))
        .corner_radius(egui::CornerRadius::same(7))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            detail_row(ui, c, "Launcher", probe_state_label(agent.launcher_state));
            detail_row(
                ui,
                c,
                "Authentication",
                probe_state_label(agent.authentication_state),
            );
            detail_row(
                ui,
                c,
                "Inference probe",
                probe_state_label(agent.inference_state),
            );
            detail_row(ui, c, "Version", &agent.version);
            detail_row(ui, c, "Route", route_label(agent.route));
            detail_row(ui, c, "Result code", &agent.code);
        });
    ui.add_space(14.0);
    ui.label(
        RichText::new("Probe results do not start an agent or open a terminal.")
            .size(12.0)
            .color(c.faint),
    );
    false
}

fn detail_row(ui: &mut Ui, c: &C, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.set_min_width(ui.available_width());
        ui.label(RichText::new(label).size(12.0).color(c.muted));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(
                egui::Label::new(RichText::new(value).size(12.0).color(c.txt).monospace()).wrap(),
            );
        });
    });
    ui.add_space(5.0);
}

fn probe_state_label(state: ProbeState) -> &'static str {
    match state {
        ProbeState::Verified => "Verified",
        ProbeState::Failed => "Failed",
        ProbeState::Unavailable => "Unavailable",
        ProbeState::NotRun => "Not run",
    }
}
