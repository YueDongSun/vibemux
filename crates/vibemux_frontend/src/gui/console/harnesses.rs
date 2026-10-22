#![forbid(unsafe_code)]
//! Harnesses: the read-only probe facts for each harness, the re-probe, and
//! a jump to the conversation for a harness that can be asked here.

use eframe::egui::{RichText, Ui};
use vibemux_probe::ProbeState;

use crate::view_model::{AgentView, route_label};

use super::{
    super::{C, design, section_page},
    ConsoleActions, ConsoleView, ProbeJob, toolbar,
};

pub(super) fn render(ui: &mut Ui, c: &C, view: &ConsoleView<'_>, actions: &mut ConsoleActions) {
    let probing = view.data.probe == ProbeJob::Running;
    let status = if probing {
        "Probing…".to_string()
    } else {
        format!("Probed {}", view.view_model.observed_at)
    };
    if toolbar(ui, c, &status, "Re-probe", !probing) {
        actions.reprobe = true;
    }
    if let ProbeJob::Failed(code) = &view.data.probe {
        ui.label(
            RichText::new(format!("The probe could not run ({code})."))
                .size(12.5)
                .color(c.danger),
        );
        ui.add_space(10.0);
    }
    match view
        .selected_harness
        .and_then(|index| view.view_model.agents.get(index))
    {
        Some(agent) => render_detail(ui, c, view, agent, actions),
        None => render_list(ui, c, view, actions),
    }
}

fn render_list(ui: &mut Ui, c: &C, view: &ConsoleView<'_>, actions: &mut ConsoleActions) {
    if view.view_model.agents.is_empty() {
        super::notice(ui, c, "No probe results are available.");
        return;
    }
    section_page::card(ui, c, |ui| {
        for (index, agent) in view.view_model.agents.iter().enumerate() {
            if index > 0 {
                section_page::separator(ui, c);
            }
            let ready = ask_ready(view, agent);
            section_page::row(ui, c, &agent.name, &agent_summary(agent), |ui| {
                if design::action_button(ui, c, "Details", true).clicked() {
                    actions.harness = Some(Some(index));
                }
                if ready {
                    ui.label(RichText::new("Ask ready").size(12.0).color(c.ok));
                }
            });
        }
    });
    ui.add_space(12.0);
    section_page::footnote(
        ui,
        c,
        "Probe results come from read-only --version checks. They never start a harness or open a terminal.",
    );
}

fn render_detail(
    ui: &mut Ui,
    c: &C,
    view: &ConsoleView<'_>,
    agent: &AgentView,
    actions: &mut ConsoleActions,
) {
    if design::quiet_button(ui, c, "←  All harnesses").clicked() {
        actions.harness = Some(None);
    }
    ui.add_space(8.0);
    section_page::sub_heading(ui, c, &agent.name);
    let rows = agent_details(agent);
    section_page::value_card(ui, c, &rows);
    ui.add_space(14.0);
    if ask_ready(view, agent) {
        if design::accent_button(ui, c, &format!("Ask {}", agent.name)).clicked() {
            actions.ask = Some(agent.name.clone());
        }
        ui.add_space(8.0);
        section_page::footnote(
            ui,
            c,
            "Opens the conversation with this harness selected. Nothing is sent until you press Send.",
        );
    } else if askable(agent) {
        section_page::footnote(
            ui,
            c,
            "Ask is not set up for this harness in this folder. Pick it in the conversation's composer to set it up.",
        );
    }
}

fn ask_ready(view: &ConsoleView<'_>, agent: &AgentView) -> bool {
    view.ask_ready.iter().any(|name| name == &agent.name)
}

/// A detected harness a structured adapter can run a turn for.
fn askable(agent: &AgentView) -> bool {
    agent.launcher_state == ProbeState::Verified
        && agent.adapter.support == vibemux_harness::dispatch::AdapterSupport::Execute
}

/// The line the list shows under a harness name.
fn agent_summary(agent: &AgentView) -> String {
    format!(
        "Launcher: {}  ·  Route: {}  ·  Adapter: {}",
        probe_state_label(agent.launcher_state),
        route_label(agent.route),
        agent.adapter.support.as_str()
    )
}

/// The labelled values the detail view shows, in order.
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

const fn probe_state_label(state: ProbeState) -> &'static str {
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

    use super::{agent_details, agent_summary, askable, probe_state_label};
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
    fn the_harness_views_state_every_probe_fact_in_order() {
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
    fn only_detected_executable_harnesses_are_askable() {
        assert!(askable(&agent(AgentKind::Claude)));
        assert!(!askable(&agent(AgentKind::Kimi)), "terminal only");
        let mut missing = agent(AgentKind::Codex);
        missing.launcher_state = ProbeState::Unavailable;
        assert!(!askable(&missing));
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
