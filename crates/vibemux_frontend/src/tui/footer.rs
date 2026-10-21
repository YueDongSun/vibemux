#![forbid(unsafe_code)]
//! Bottom-of-screen TUI footer: action keys, active theme, health.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::tui::theme::{Theme, health_style, muted_style, title_style};
use crate::view_model::{AgentView, ViewModel, route_label, state_label};

/// The keys sit in the block title; the line names the selected harness, a
/// running re-probe, and the overall status.
pub fn render_footer(
    frame: &mut Frame<'_>,
    model: &ViewModel,
    theme: Theme,
    selected: Option<usize>,
    probing: bool,
) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(3)])
        .split(frame.area());
    let mut spans = Vec::new();
    if probing {
        spans.push(Span::styled("probing (read-only) | ", title_style(theme)));
    }
    if let Some(agent) = selected.and_then(|index| model.agents.get(index)) {
        spans.push(Span::raw(agent_details(agent)));
        spans.push(Span::raw(" | "));
    }
    spans.push(Span::styled(
        model.overall_status.clone(),
        health_style(theme, model.health),
    ));
    let keys = format!(
        " q quit | r re-probe | j/k select | c snapshot | t theme: {} ",
        theme.name()
    );
    let paragraph = Paragraph::new(Line::from(spans)).block(
        Block::default()
            .title(Line::styled(keys, muted_style(theme)))
            .borders(Borders::ALL),
    );
    frame.render_widget(paragraph, areas[1]);
}

/// Every observation of one harness, in the order of the agent table.
fn agent_details(agent: &AgentView) -> String {
    format!(
        "{} launcher={} auth={} inference={} version={} route={} code={} adapter={}",
        agent.name,
        state_label(agent.launcher_state),
        state_label(agent.authentication_state),
        state_label(agent.inference_state),
        agent.version,
        route_label(agent.route),
        agent.code,
        agent.adapter.support.as_str()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::render::render_debug_dashboard;
    use ratatui::{Terminal, backend::TestBackend};
    use vibemux_probe::{
        A2aSelfTestProbe, AgentKind, AgentProbe, GatewayProbe, LauncherKind, ProbeReport,
        ProbeState, RouteKind,
    };

    fn model() -> ViewModel {
        ViewModel::from_report(&ProbeReport {
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
        })
    }

    fn render(selected: Option<usize>, probing: bool) -> String {
        let model = model();
        let mut terminal = Terminal::new(TestBackend::new(120, 32)).expect("test terminal");
        terminal
            .draw(|frame| {
                render_debug_dashboard(frame, &model, Theme::Classic, selected);
                render_footer(frame, &model, Theme::Classic, selected, probing);
            })
            .expect("draw");
        let buffer = terminal.backend().buffer();
        buffer
            .content()
            .chunks(120)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn selected_row_has_marker_and_footer_details() {
        let rendered = render(Some(1), false);
        assert!(rendered.contains("│> Codex"), "{rendered}");
        assert!(!rendered.contains("│> Claude"), "{rendered}");
        assert!(
            rendered.contains("Codex launcher=verified auth=not_run inference=not_run"),
            "{rendered}"
        );
        assert!(rendered.contains("adapter=execute"), "{rendered}");
        assert!(rendered.contains("r re-probe"), "{rendered}");
        assert!(rendered.contains("j/k select"), "{rendered}");
    }

    #[test]
    fn probing_footer_says_probing() {
        assert!(render(Some(0), true).contains("probing"));
        assert!(!render(Some(0), false).contains("probing"));
    }

    #[test]
    fn no_selection_draws_no_marker() {
        let rendered = render(None, false);
        assert!(
            !rendered.lines().any(|line| line.starts_with("│> ")),
            "{rendered}"
        );
    }
}
