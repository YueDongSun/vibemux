use vibemux_harness::{
    dispatch::{AdapterCapability, adapter_capability},
    terminal_text::display_character,
};
use vibemux_probe::{AgentKind, ProbeReport, ProbeState, RouteKind};

use crate::view_model::{route_label, state_label};

const MAX_DISPLAY_CHARACTERS: usize = 160;

/// One display line: every whitespace or terminal-unsafe character becomes a
/// space, and the text is cut at `MAX_DISPLAY_CHARACTERS`.
pub fn safe_text(value: &str) -> String {
    value
        .chars()
        .take(MAX_DISPLAY_CHARACTERS)
        .map(display_character)
        .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeTuiSlotState {
    Reserved,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeTuiSlot {
    pub agent: AgentKind,
    pub title: String,
    pub state: NativeTuiSlotState,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentView {
    pub name: String,
    pub launcher_state: String,
    pub authentication_state: String,
    pub inference_state: String,
    pub version: String,
    pub route: String,
    pub code: String,
    /// Structured adapter support compiled into this build; independent of
    /// whether the harness was observed.
    pub adapter: AdapterCapability,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DashboardModel {
    pub title: String,
    pub platform: String,
    pub agents: Vec<AgentView>,
    pub gateway_summary: String,
    pub telemetry_summary: String,
    pub a2a_summary: String,
    pub native_tui_slots: Vec<NativeTuiSlot>,
}

impl DashboardModel {
    #[must_use]
    pub fn from_report(report: &ProbeReport) -> Self {
        let agents = AgentKind::all()
            .into_iter()
            .map(
                |agent| match report.agents.iter().find(|probe| probe.agent == agent) {
                    Some(probe) => AgentView {
                        name: agent.display_name().to_string(),
                        launcher_state: state_label(probe.launcher_state).to_string(),
                        authentication_state: state_label(probe.authentication_state).to_string(),
                        inference_state: state_label(probe.inference_state).to_string(),
                        version: probe
                            .version
                            .as_deref()
                            .map(safe_text)
                            .unwrap_or_else(|| "-".to_string()),
                        route: route_label(probe.route).to_string(),
                        code: safe_text(&probe.code),
                        adapter: adapter_capability(agent),
                    },
                    None => AgentView {
                        name: agent.display_name().to_string(),
                        launcher_state: state_label(ProbeState::NotRun).to_string(),
                        authentication_state: state_label(ProbeState::NotRun).to_string(),
                        inference_state: state_label(ProbeState::NotRun).to_string(),
                        version: "-".to_string(),
                        route: route_label(RouteKind::Unknown).to_string(),
                        code: "observation_missing".to_string(),
                        adapter: adapter_capability(agent),
                    },
                },
            )
            .collect();
        let native_tui_slots = AgentKind::all()
            .into_iter()
            .map(|agent| {
                let available = report
                    .agents
                    .iter()
                    .find(|probe| probe.agent == agent)
                    .is_some_and(|probe| probe.launcher_state == ProbeState::Verified);
                NativeTuiSlot {
                    agent,
                    title: format!("{} native TUI", agent.display_name()),
                    state: if available {
                        NativeTuiSlotState::Reserved
                    } else {
                        NativeTuiSlotState::Unavailable
                    },
                    reason: if available {
                        "reserved for supervised PTY/ConPTY attach".to_string()
                    } else {
                        "no verified launcher observation; attach disabled".to_string()
                    },
                }
            })
            .collect();
        let telemetry_summary = if report.gateway.telemetry.is_empty() {
            "telemetry unavailable".to_string()
        } else {
            report
                .gateway
                .telemetry
                .iter()
                .map(|item| {
                    format!(
                        "{}:{} req/{} fail",
                        safe_text(&item.app_type),
                        item.requests,
                        item.failures
                    )
                })
                .collect::<Vec<_>>()
                .join(" | ")
        };
        Self {
            title: "VibeMux Unified Frontend".to_string(),
            platform: safe_text(&report.platform),
            agents,
            gateway_summary: format!(
                "{} health={} (endpoint withheld)",
                state_label(report.gateway.state),
                report
                    .gateway
                    .health_status
                    .map_or_else(|| "-".to_string(), |status| status.to_string())
            ),
            telemetry_summary,
            a2a_summary: format!(
                "{} correlation={} listener_closed={}",
                state_label(report.a2a.state),
                report.a2a.correlation_preserved,
                report.a2a.listener_closed
            ),
            native_tui_slots,
        }
    }

    #[must_use]
    pub fn plain_snapshot(&self) -> String {
        let mut lines = vec![
            self.title.clone(),
            format!("platform: {}", self.platform),
            format!("gateway: {}", self.gateway_summary),
            format!("telemetry: {}", self.telemetry_summary),
            format!("a2a: {}", self.a2a_summary),
            "agents:".to_string(),
        ];
        lines.extend(self.agents.iter().map(|agent| {
            format!(
                "- {} launcher={} auth={} inference={} version={} route={} code={} adapter={} protocols={}",
                agent.name,
                agent.launcher_state,
                agent.authentication_state,
                agent.inference_state,
                agent.version,
                agent.route,
                agent.code,
                agent.adapter.support.as_str(),
                agent.adapter.protocol_list()
            )
        }));
        lines.push("native_tui_slots:".to_string());
        lines.extend(self.native_tui_slots.iter().map(|slot| {
            format!(
                "- {} state={} reason={}",
                slot.title,
                slot_state_label(slot.state),
                slot.reason
            )
        }));
        lines.join("\n")
    }
}

pub(crate) const fn slot_state_label(state: NativeTuiSlotState) -> &'static str {
    match state {
        NativeTuiSlotState::Reserved => "reserved",
        NativeTuiSlotState::Unavailable => "unavailable",
    }
}
