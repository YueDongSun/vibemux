#![forbid(unsafe_code)]
//! Unified read-only dashboard model and Ratatui renderer.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Tabs, Wrap},
};
use vibemux_probe::{AgentKind, ProbeReport, ProbeState, RouteKind};

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
        let agents = report
            .agents
            .iter()
            .map(|probe| AgentView {
                name: probe.agent.display_name().to_string(),
                launcher_state: state_label(probe.launcher_state).to_string(),
                authentication_state: state_label(probe.authentication_state).to_string(),
                inference_state: state_label(probe.inference_state).to_string(),
                version: probe.version.clone().unwrap_or_else(|| "-".to_string()),
                route: route_label(probe.route).to_string(),
                code: probe.code.clone(),
            })
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
                        "launcher unavailable or version probe failed".to_string()
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
                        item.app_type, item.requests, item.failures
                    )
                })
                .collect::<Vec<_>>()
                .join(" | ")
        };
        Self {
            title: "VibeMux Unified Frontend".to_string(),
            platform: report.platform.clone(),
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
                "- {} launcher={} auth={} inference={} version={} route={} code={}",
                agent.name,
                agent.launcher_state,
                agent.authentication_state,
                agent.inference_state,
                agent.version,
                agent.route,
                agent.code
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

pub fn render_dashboard(frame: &mut Frame<'_>, model: &DashboardModel) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(12),
            Constraint::Length(5),
            Constraint::Length(3),
        ])
        .split(frame.area());
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            &model.title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("  [{}]", model.platform)),
    ]))
    .block(Block::default().borders(Borders::ALL));
    frame.render_widget(title, areas[0]);

    // Side-by-side only when the table keeps enough width for the longest
    // real version strings; narrower terminals stack vertically so the table
    // spans the full width and no state text is clipped.
    let body = if areas[1].width >= 160 {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(64), Constraint::Percentage(36)])
            .split(areas[1])
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(8), Constraint::Min(4)])
            .split(areas[1])
    };
    let rows = model.agents.iter().map(|agent| {
        Row::new(vec![
            Cell::from(agent.name.clone()),
            Cell::from(agent.launcher_state.clone()),
            Cell::from(agent.authentication_state.clone()),
            Cell::from(agent.inference_state.clone()),
            Cell::from(agent.version.clone()),
            Cell::from(agent.route.clone()),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(9),
            Constraint::Length(11),
            Constraint::Length(11),
            Constraint::Length(11),
            Constraint::Min(14),
            Constraint::Length(13),
        ],
    )
    .header(
        Row::new(["Agent", "Launcher", "Auth", "Infer", "Version", "Route"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().title("Agent probes").borders(Borders::ALL));
    frame.render_widget(table, body[0]);

    let diagnostics = Paragraph::new(vec![
        Line::from(format!("Gateway: {}", model.gateway_summary)),
        Line::from(format!("A2A: {}", model.a2a_summary)),
        Line::from(format!("Telemetry: {}", model.telemetry_summary)),
    ])
    .wrap(Wrap { trim: true })
    .block(Block::default().title("Diagnostics").borders(Borders::ALL));
    frame.render_widget(diagnostics, body[1]);

    let slot_labels = model
        .native_tui_slots
        .iter()
        .map(|slot| {
            format!(
                "{} [{}]",
                slot.agent.display_name(),
                slot_state_label(slot.state)
            )
        })
        .collect::<Vec<_>>();
    let slot_block = Block::default()
        .title("Native CLI TUI surfaces")
        .borders(Borders::ALL);
    if areas[2].width >= 110 {
        let tabs = Tabs::new(slot_labels.iter().map(String::as_str))
            .block(slot_block)
            .style(Style::default().fg(Color::Yellow))
            .divider(" | ");
        frame.render_widget(tabs, areas[2]);
    } else {
        let slots = Paragraph::new(slot_labels.join(" | "))
            .style(Style::default().fg(Color::Yellow))
            .wrap(Wrap { trim: true })
            .block(slot_block);
        frame.render_widget(slots, areas[2]);
    }

    let footer = Paragraph::new("q/Esc: quit | native TUI launch/attach intentionally disabled")
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(footer, areas[3]);
}

const fn state_label(state: ProbeState) -> &'static str {
    match state {
        ProbeState::Verified => "verified",
        ProbeState::Failed => "failed",
        ProbeState::Unavailable => "unavailable",
        ProbeState::NotRun => "not_run",
    }
}

const fn route_label(route: RouteKind) -> &'static str {
    match route {
        RouteKind::Direct => "direct",
        RouteKind::LocalGateway => "local_gateway",
        RouteKind::Unknown => "unknown",
    }
}

const fn slot_state_label(state: NativeTuiSlotState) -> &'static str {
    match state {
        NativeTuiSlotState::Reserved => "reserved",
        NativeTuiSlotState::Unavailable => "unavailable",
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};
    use vibemux_probe::{A2aSelfTestProbe, AgentProbe, GatewayProbe, ProbeState};

    use super::*;

    fn report() -> ProbeReport {
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
                    launcher: vibemux_probe::LauncherKind::DirectExecutable,
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
    fn dashboard_reserves_every_native_tui() {
        let model = DashboardModel::from_report(&report());
        assert_eq!(model.native_tui_slots.len(), 5);
        assert!(
            model
                .native_tui_slots
                .iter()
                .all(|slot| slot.state == NativeTuiSlotState::Reserved)
        );
    }

    #[test]
    fn ratatui_test_backend_renders_agents_and_reserved_slots() {
        let model = DashboardModel::from_report(&report());
        let backend = TestBackend::new(120, 32);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| render_dashboard(frame, &model))
            .expect("render dashboard");
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        for expected in [
            "Claude",
            "Codex",
            "OpenCode",
            "Copilot",
            "Grok",
            "Native CLI TUI surfaces",
            "reserved",
            "Gateway",
            "A2A",
        ] {
            assert!(rendered.contains(expected), "missing {expected}");
        }
    }

    #[test]
    fn narrow_windows_terminal_keeps_all_agents_and_slots_visible() {
        let model = DashboardModel::from_report(&report());
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| render_dashboard(frame, &model))
            .expect("render dashboard");
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        for expected in ["Claude", "Codex", "OpenCode", "Copilot", "Grok", "reserved"] {
            assert!(rendered.contains(expected), "missing {expected}");
        }
    }

    fn render_text(model: &DashboardModel, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| render_dashboard(frame, model))
            .expect("render dashboard");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    fn real_report() -> ProbeReport {
        // Shapes captured from a real Windows probe run: verified launchers,
        // not_run auth/inference, and vendor-length version strings.
        let versions: [(AgentKind, &str, RouteKind); 5] = [
            (
                AgentKind::Claude,
                "2.1.259 (Claude Code)",
                RouteKind::LocalGateway,
            ),
            (AgentKind::Codex, "codex-cli 0.153.0", RouteKind::Unknown),
            (AgentKind::OpenCode, "1.18.21", RouteKind::Direct),
            (
                AgentKind::Copilot,
                "GitHub Copilot CLI 1.0.75.",
                RouteKind::Unknown,
            ),
            (
                AgentKind::Grok,
                "grok 1.0.13 (5e9a58528b76) [stable]",
                RouteKind::LocalGateway,
            ),
        ];
        let mut probe = report();
        probe.agents = versions
            .into_iter()
            .map(|(agent, version, route)| AgentProbe {
                agent,
                launcher_state: ProbeState::Verified,
                authentication_state: ProbeState::NotRun,
                inference_state: ProbeState::NotRun,
                launcher: vibemux_probe::LauncherKind::DirectExecutable,
                version: Some(version.to_string()),
                route,
                endpoints: Vec::new(),
                code: "version_verified".to_string(),
            })
            .collect();
        probe
    }

    #[test]
    fn real_probe_states_and_versions_are_never_clipped() {
        let model = DashboardModel::from_report(&real_report());
        // Regression: the combined "L:.. A:.. I:.." cell needed 30-31 chars
        // but had a fixed 28-char column, so verified agents rendered a
        // truncated inference state like "I:not_r" on real machines.
        let narrow = render_text(&model, 80, 24);
        for expected in [
            "verified",
            "not_run",
            "local_gateway",
            "2.1.259",
            "codex-cli 0.153.0",
            "GitHub Copilot",
        ] {
            assert!(narrow.contains(expected), "missing {expected} at 80x24");
        }
        for (width, height) in [(120u16, 32u16), (160, 40)] {
            let rendered = render_text(&model, width, height);
            for expected in [
                "verified",
                "not_run",
                "2.1.259 (Claude Code)",
                "codex-cli 0.153.0",
                "GitHub Copilot CLI 1.0.75.",
                "grok 1.0.13 (5e9a58528b76) [stable]",
                "local_gateway",
            ] {
                assert!(
                    rendered.contains(expected),
                    "missing {expected} at {width}x{height}"
                );
            }
        }
    }

    #[test]
    fn plain_snapshot_is_deterministic_and_read_only() {
        let snapshot = DashboardModel::from_report(&report()).plain_snapshot();
        assert!(snapshot.contains("Claude native TUI state=reserved"));
        assert!(snapshot.contains("a2a: verified correlation=true listener_closed=true"));
    }
}
