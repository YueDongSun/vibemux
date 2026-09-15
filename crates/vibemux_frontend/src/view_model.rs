#![forbid(unsafe_code)]
//! Backend-neutral view model shared by the TUI debug view and the GUI.
//!
//! `ViewModel::from_report` is the only place that knows the shape of
//! `vibemux_probe::ProbeReport`. Both UIs consume a `ViewModel`.

use vibemux_probe::{
    A2aSelfTestProbe, AgentKind, AgentProbe, GatewayProbe, ProbeReport, ProbeState, RouteKind,
};

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
    /// ISO-8601 UTC rendering of `observed_at_epoch_seconds` for display.
    pub observed_at: String,
    pub health: Health,
    /// Aggregate one-line status: agent state counts plus gateway and A2A
    /// probe states. Surfaced in the TUI title and footer.
    pub overall_status: String,
    pub agents: Vec<AgentView>,
    pub gateway_summary: String,
    pub gateway_state: ProbeState,
    pub telemetry_summary: String,
    pub telemetry_failures: u64,
    pub telemetry_available: bool,
    pub a2a_summary: String,
    pub a2a_state: ProbeState,
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
        let counts = StateCounts::from_agents(&report.agents);
        let health = compute_health(&counts, report.gateway.state, report.a2a.state);
        Self {
            title: "VibeMux Central Console".to_string(),
            platform: report.platform.clone(),
            schema_version: report.schema_version,
            observed_at_epoch_seconds: report.observed_at_epoch_seconds,
            observed_at: format_epoch_seconds(report.observed_at_epoch_seconds),
            health,
            overall_status: build_overall_status(&counts, &report.gateway, &report.a2a),
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
            gateway_state: report.gateway.state,
            telemetry_summary,
            telemetry_failures: report
                .gateway
                .telemetry
                .iter()
                .map(|item| item.failures)
                .sum(),
            telemetry_available: !report.gateway.telemetry.is_empty(),
            a2a_summary: format!(
                "{} correlation={} listener_closed={}",
                state_label(report.a2a.state),
                report.a2a.correlation_preserved,
                report.a2a.listener_closed
            ),
            a2a_state: report.a2a.state,
            env_allowlist: collect_allowlisted_env(),
        }
    }
}

/// Aggregate health computed from every probe dimension.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Health {
    Ok,
    Warning,
    Failure,
}

#[derive(Default)]
struct StateCounts {
    verified: usize,
    failed: usize,
    unavailable: usize,
    not_run: usize,
}

impl StateCounts {
    fn from_agents(agents: &[AgentProbe]) -> Self {
        let mut counts = Self::default();
        for agent in agents {
            match agent.launcher_state {
                ProbeState::Verified => counts.verified += 1,
                ProbeState::Failed => counts.failed += 1,
                ProbeState::Unavailable => counts.unavailable += 1,
                ProbeState::NotRun => counts.not_run += 1,
            }
        }
        counts
    }
}

fn compute_health(counts: &StateCounts, gateway: ProbeState, a2a: ProbeState) -> Health {
    if counts.failed > 0 || gateway == ProbeState::Failed || a2a == ProbeState::Failed {
        Health::Failure
    } else if counts.unavailable > 0
        || gateway == ProbeState::Unavailable
        || a2a == ProbeState::Unavailable
    {
        Health::Warning
    } else {
        Health::Ok
    }
}

fn build_overall_status(
    counts: &StateCounts,
    gateway: &GatewayProbe,
    a2a: &A2aSelfTestProbe,
) -> String {
    let agent_summary = if counts.failed == 0 && counts.unavailable == 0 && counts.not_run == 0 {
        format!("{} verified", counts.verified)
    } else {
        format!(
            "{} verified, {} failed, {} unavailable, {} not_run",
            counts.verified, counts.failed, counts.unavailable, counts.not_run
        )
    };
    format!(
        "agents: {} | gateway: {} | a2a: {}",
        agent_summary,
        state_label(gateway.state),
        state_label(a2a.state),
    )
}

/// Render a Unix epoch-seconds value as ISO-8601 UTC (e.g. `2026-09-06T03:34:56Z`).
#[must_use]
pub fn format_epoch_seconds(epoch_seconds: u64) -> String {
    let days = i64::try_from(epoch_seconds / 86_400).unwrap_or(0);
    let secs_today = epoch_seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = secs_today / 3600;
    let minute = (secs_today % 3600) / 60;
    let second = secs_today % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Howard Hinnant's date algorithm: convert days since 1970-01-01 to civil date.
const fn civil_from_days(days_since_1970: i64) -> (i64, u32, u32) {
    let z = days_since_1970 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u32, d as u32)
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
    use vibemux_probe::LauncherKind;

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
    fn view_model_has_ten_agents() {
        let vm = ViewModel::from_report(&report_with_env());
        assert_eq!(vm.agents.len(), 10);
    }

    #[test]
    fn view_model_computes_health_and_overall_status() {
        let vm = ViewModel::from_report(&report_with_env());
        assert_eq!(vm.health, Health::Ok);
        assert_eq!(
            vm.overall_status,
            "agents: 10 verified | gateway: verified | a2a: verified"
        );
        assert_eq!(vm.observed_at, "1970-01-01T00:00:01Z");

        let mut degraded = report_with_env();
        degraded.agents[0].launcher_state = ProbeState::Unavailable;
        let vm = ViewModel::from_report(&degraded);
        assert_eq!(vm.health, Health::Warning);
        assert!(vm.overall_status.contains("1 unavailable"));

        let mut failed = report_with_env();
        failed.a2a.state = ProbeState::Failed;
        let vm = ViewModel::from_report(&failed);
        assert_eq!(vm.health, Health::Failure);
    }

    #[test]
    fn format_epoch_seconds_renders_civil_dates() {
        assert_eq!(format_epoch_seconds(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_epoch_seconds(1_757_636_096), "2025-09-12T00:14:56Z");
        // Leap-day crossing and a far-future year boundary.
        assert_eq!(format_epoch_seconds(1_582_963_200), "2020-02-29T08:00:00Z");
        assert_eq!(format_epoch_seconds(4_102_444_800), "2100-01-01T00:00:00Z");
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
