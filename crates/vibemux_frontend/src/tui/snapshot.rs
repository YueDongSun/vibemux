#![forbid(unsafe_code)]
//! ASCII-only plain-text snapshot for the TUI debug view. Used both
//! by the `c` action key inside the TUI and by the
//! `vibemux_frontend_dump` binary.

use crate::view_model::{
    ViewModel, route_label as view_route_label, state_label as view_state_label,
};

#[must_use]
pub fn debug_snapshot(model: &ViewModel) -> String {
    let mut lines = Vec::new();
    lines.push(model.title.clone());
    lines.push(format!("platform: {}", model.platform));
    lines.push(format!("schema_version: {}", model.schema_version));
    lines.push(format!(
        "observed_at_epoch_seconds: {}",
        model.observed_at_epoch_seconds
    ));
    lines.push(String::new());
    lines.push(format!("gateway: {}", model.gateway_summary));
    lines.push(format!("a2a: {}", model.a2a_summary));
    lines.push(format!("telemetry: {}", model.telemetry_summary));
    lines.push(String::new());
    lines.push("env (allowlisted only):".to_string());
    if model.env_allowlist.is_empty() {
        lines.push("- <allowlist empty>".to_string());
    } else {
        for (key, value) in &model.env_allowlist {
            lines.push(format!(
                "- {}={}",
                key,
                value.as_deref().unwrap_or("<unset>")
            ));
        }
    }
    lines.push(String::new());
    lines.push("agents:".to_string());
    for agent in &model.agents {
        lines.push(format!(
            "- {} launcher={} auth={} inference={} version={} route={} code={}",
            agent.name,
            view_state_label(agent.launcher_state),
            view_state_label(agent.authentication_state),
            view_state_label(agent.inference_state),
            agent.version,
            view_route_label(agent.route),
            agent.code
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use vibemux_probe::{
        A2aSelfTestProbe, AgentKind, AgentProbe, GatewayProbe, LauncherKind, ProbeReport,
        ProbeState, RouteKind,
    };

    fn fixture() -> ProbeReport {
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
                    path: None,
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
    fn debug_snapshot_is_ascii_only() {
        let model = ViewModel::from_report(&fixture());
        let snapshot = debug_snapshot(&model);
        for byte in snapshot.bytes() {
            assert!(byte <= 0x7E || byte == b'\n', "non-ASCII byte 0x{byte:02x}");
        }
    }

    #[test]
    fn debug_snapshot_lists_agents_in_order() {
        let model = ViewModel::from_report(&fixture());
        let snapshot = debug_snapshot(&model);
        // Find each agent by its unique "- <name> launcher=" prefix so
        // that substrings elsewhere (e.g. env values containing
        // "Codex" or "Claude") don't perturb the ordering.
        let claude = snapshot.find("- Claude launcher=").expect("Claude line");
        let codex = snapshot.find("- Codex launcher=").expect("Codex line");
        let opencode = snapshot
            .find("- OpenCode launcher=")
            .expect("OpenCode line");
        let copilot = snapshot.find("- Copilot launcher=").expect("Copilot line");
        let grok = snapshot.find("- Grok launcher=").expect("Grok line");
        assert!(claude < codex);
        assert!(codex < opencode);
        assert!(opencode < copilot);
        assert!(copilot < grok);
    }

    #[test]
    fn debug_snapshot_contains_diagnostics_lines() {
        let model = ViewModel::from_report(&fixture());
        let snapshot = debug_snapshot(&model);
        assert!(snapshot.contains("gateway:"));
        assert!(snapshot.contains("a2a:"));
        assert!(snapshot.contains("telemetry:"));
        assert!(snapshot.contains("env (allowlisted only):"));
        assert!(snapshot.contains("agents:"));
    }

    #[test]
    fn debug_snapshot_is_deterministic() {
        let model = ViewModel::from_report(&fixture());
        let a = debug_snapshot(&model);
        let b = debug_snapshot(&model);
        assert_eq!(a, b);
    }
}
