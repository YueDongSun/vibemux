#![forbid(unsafe_code)]
//! Read-only harness details sourced from the local probe view model.

use eframe::egui::{self, RichText, Ui};
use vibemux_probe::ProbeState;

use crate::view_model::{AgentView, ViewModel, route_label};

use super::{C, design};

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
            .corner_radius(design::CARD_RADIUS)
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
                            RichText::new(agent_summary(agent))
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
        .corner_radius(design::CARD_RADIUS)
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            for (label, value) in agent_details(agent) {
                detail_row(ui, c, label, &value);
            }
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

/// The line the agent list shows under a harness name.
fn agent_summary(agent: &AgentView) -> String {
    format!(
        "Launcher: {}  ·  Route: {}  ·  Adapter: {}",
        probe_state_label(agent.launcher_state),
        route_label(agent.route),
        agent.adapter.support.as_str()
    )
}

/// The labelled values the agent detail view shows, in order.
fn agent_details(agent: &AgentView) -> [(&'static str, String); 7] {
    [
        (
            "Launcher",
            probe_state_label(agent.launcher_state).to_string(),
        ),
        (
            "Authentication",
            probe_state_label(agent.authentication_state).to_string(),
        ),
        (
            "Inference probe",
            probe_state_label(agent.inference_state).to_string(),
        ),
        ("Version", agent.version.clone()),
        ("Route", route_label(agent.route).to_string()),
        ("Structured adapter", agent.adapter.summary()),
        ("Result code", agent.code.clone()),
    ]
}

fn probe_state_label(state: ProbeState) -> &'static str {
    match state {
        ProbeState::Verified => "Verified",
        ProbeState::Failed => "Failed",
        ProbeState::Unavailable => "Unavailable",
        ProbeState::NotRun => "Not run",
    }
}

#[cfg(test)]
mod tests {
    use vibemux_harness::dispatch::adapter_capability;
    use vibemux_probe::{AgentKind, ProbeState, RouteKind};

    use super::{agent_details, agent_summary, probe_state_label};
    use crate::view_model::AgentView;

    fn agent(kind: AgentKind) -> AgentView {
        AgentView {
            name: kind.display_name().to_string(),
            launcher_state: ProbeState::Verified,
            authentication_state: ProbeState::Failed,
            inference_state: ProbeState::NotRun,
            version: "1.2.3".to_string(),
            route: RouteKind::LocalGateway,
            code: "launcher_verified".to_string(),
            adapter: adapter_capability(kind),
        }
    }

    #[test]
    fn the_agent_views_state_every_probe_fact_in_order() {
        let codex = agent(AgentKind::Codex);
        assert_eq!(
            agent_summary(&codex),
            "Launcher: Verified  ·  Route: local_gateway  ·  Adapter: execute"
        );
        assert_eq!(
            agent_details(&codex).map(|(label, value)| format!("{label}={value}")),
            [
                "Launcher=Verified",
                "Authentication=Failed",
                "Inference probe=Not run",
                "Version=1.2.3",
                "Route=local_gateway",
                "Structured adapter=execute via codex_exec,codex_app_server",
                "Result code=launcher_verified",
            ]
        );
        // The adapter is a build fact, shown even for a harness with none.
        let kimi = agent(AgentKind::Kimi);
        assert!(agent_summary(&kimi).ends_with("Adapter: terminal_only"));
        assert_eq!(
            agent_details(&kimi)[5],
            ("Structured adapter", "terminal_only".to_string())
        );
    }

    #[test]
    fn probe_state_labels_are_pinned() {
        let labels = [
            (ProbeState::Verified, "Verified"),
            (ProbeState::Failed, "Failed"),
            (ProbeState::Unavailable, "Unavailable"),
            (ProbeState::NotRun, "Not run"),
        ];
        for (state, expected) in labels {
            assert_eq!(probe_state_label(state), expected);
        }
    }
}
