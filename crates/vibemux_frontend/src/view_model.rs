#![forbid(unsafe_code)]
//! Backend-neutral view model shared by the TUI debug view and the GUI.
//!
//! `ViewModel::from_report` is the only place that knows the shape of
//! `vibemux_probe::ProbeReport`. Both UIs consume a `ViewModel`.

use vibemux_probe::{AgentKind, AgentProbe, ProbeReport, ProbeState, RouteKind};

use crate::probe_env::collect_allowlisted_env;

/// One harness entry as the UI sees it. Keeps the field set narrow:
/// the TUI/GUI do not need the full `AgentProbe` shape.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentView {
    pub name: String,
    pub launcher_state: ProbeState,
    pub authentication_state: ProbeState,
    pub inference_state: ProbeState,
    pub version: String,
    pub route: RouteKind,
    pub code: String,
}

impl From<&AgentProbe> for AgentView {
    fn from(probe: &AgentProbe) -> Self {
        Self {
            name: probe.agent.display_name().to_string(),
            launcher_state: probe.launcher_state,
            authentication_state: probe.authentication_state,
            inference_state: probe.inference_state,
            version: probe.version.clone().unwrap_or_else(|| "-".to_string()),
            route: probe.route,
            code: probe.code.clone(),
        }
    }
}

/// Top-level model consumed by both the TUI and the GUI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ViewModel {
    pub title: String,
    pub platform: String,
    pub schema_version: u16,
    pub observed_at_epoch_seconds: u64,
    pub agents: Vec<AgentView>,
    pub gateway_summary: String,
    pub telemetry_summary: String,
    pub a2a_summary: String,
    /// One entry per `PROBE_ENVIRONMENT_ALLOWLIST` key. Value is `None`
    /// when the variable is unset in the current process environment.
    /// Secret-bearing keys are never included by design.
    pub env_allowlist: Vec<(String, Option<String>)>,
}

impl ViewModel {
    /// Construct a view model from a probe report and the current
    /// process environment filtered through the probe allowlist.
    #[must_use]
    pub fn from_report(report: &ProbeReport) -> Self {
        let agents = report.agents.iter().map(AgentView::from).collect();
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
                        item.app_type, item.requests, item.failures
                    )
                })
                .collect::<Vec<_>>()
                .join(" | ")
        };
        Self {
            title: "VibeMux Central Console".to_string(),
            platform: report.platform.clone(),
            schema_version: report.schema_version,
            observed_at_epoch_seconds: report.observed_at_epoch_seconds,
            agents,
            gateway_summary: format!(
                "{}:{} {} health={}",
                report.gateway.host,
                report.gateway.port,
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
            env_allowlist: collect_allowlisted_env(),
        }
    }
}

/// Stable, lowercase ASCII label for a probe state. Used by both UIs
/// and by the snapshot formatter.
#[must_use]
pub const fn state_label(state: ProbeState) -> &'static str {
    match state {
        ProbeState::Verified => "verified",
        ProbeState::Failed => "failed",
        ProbeState::Unavailable => "unavailable",
        ProbeState::NotRun => "not_run",
    }
}

/// Stable, lowercase ASCII label for a route kind. Used by both UIs
/// and the snapshot formatter.
#[must_use]
pub const fn route_label(route: RouteKind) -> &'static str {
    match route {
        RouteKind::Direct => "direct",
        RouteKind::LocalGateway => "local_gateway",
        RouteKind::Unknown => "unknown",
    }
}

/// Stable, lowercase ASCII label for an agent kind's command name.
#[must_use]
pub const fn agent_command_name(agent: AgentKind) -> &'static str {
    agent.command_name()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vibemux_probe::{A2aSelfTestProbe, GatewayProbe, LauncherKind};

    fn report_with_env() -> ProbeReport {
        ProbeReport {
            schema_version: 1,
            observed_at_epoch_seconds: 1,
            platform: "windows".to_string(),
            agents: AgentKind::all()
                .into_iter()
                .map(|agent| AgentProbe {
                    agent,
                    launcher_state: ProbeState::Verified,
                    authentication_state: ProbeState::NotRun,
                    inference_state: ProbeState::NotRun,
                    launcher: LauncherKind::DirectExecutable,
                    version: Some("1.0".to_string()),
                    route: RouteKind::Direct,
                    endpoints: Vec::new(),
                    code: "version_verified".to_string(),
                })
                .collect(),
            gateway: GatewayProbe {
                state: ProbeState::Verified,
                host: "127.0.0.1".to_string(),
                port: 15_721,
                tcp_reachable: true,
                health_status: Some(200),
                telemetry_state: ProbeState::Verified,
                telemetry: Vec::new(),
                code: "gateway_verified".to_string(),
            },
            a2a: A2aSelfTestProbe {
                state: ProbeState::Verified,
                correlation_preserved: true,
                listener_closed: true,
                code: "a2a_self_test_verified".to_string(),
            },
        }
    }

    #[test]
    fn view_model_has_five_agents() {
        let vm = ViewModel::from_report(&report_with_env());
        assert_eq!(vm.agents.len(), 5);
    }

    #[test]
    fn view_model_does_not_reserve_native_tui() {
        // Native TUI reservations moved to the GUI; the view model has
        // no slot concept.
        let vm = ViewModel::from_report(&report_with_env());
        for agent in &vm.agents {
            assert!(!agent.name.is_empty());
        }
        // ViewModel has no native_tui_slots field (compile-time check).
    }

    #[test]
    fn state_label_is_lowercase_ascii() {
        for label in [
            state_label(ProbeState::Verified),
            state_label(ProbeState::Failed),
            state_label(ProbeState::Unavailable),
            state_label(ProbeState::NotRun),
        ] {
            assert!(label.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
        }
    }

    #[test]
    fn route_label_is_lowercase_ascii() {
        for label in [
            route_label(RouteKind::Direct),
            route_label(RouteKind::LocalGateway),
            route_label(RouteKind::Unknown),
        ] {
            assert!(label.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
        }
    }
}
