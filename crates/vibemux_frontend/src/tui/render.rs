#![forbid(unsafe_code)]
//! TUI dashboard renderer. Slimmed-down view: title, diagnostics
//! (gateway / A2A / telemetry / env allowlist), one line per agent,
//! and a footer. No per-harness detail; that lives in the GUI now.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap},
};

use crate::theme::ThemePalette;
use crate::view_model::{ViewModel, route_label, state_label};

pub fn render_debug_dashboard(frame: &mut Frame<'_>, model: &ViewModel, palette: &ThemePalette) {
    let bg = hex_color(&palette.bg, Color::Black);
    let surface = hex_color(&palette.surface, Color::Black);
    let border_color = hex_color(&palette.border, Color::DarkGray);
    let text_primary = hex_color(&palette.text_primary, Color::White);
    let text_muted = hex_color(&palette.text_muted, Color::Gray);
    let accent = hex_color(&palette.accent, Color::Cyan);

    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(bg)), area);

    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(8),
            Constraint::Min(7),
            Constraint::Length(3),
        ])
        .split(area);

    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            format!("{} (TUI debug)", model.title),
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  [{}]", model.platform),
            Style::default().fg(text_muted),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .style(Style::default().bg(surface).fg(text_primary)),
    );
    frame.render_widget(title, outer[0]);

    render_diagnostics(
        frame,
        outer[1],
        model,
        border_color,
        text_primary,
        text_muted,
        surface,
    );
    render_agents(
        frame,
        outer[2],
        model,
        border_color,
        text_primary,
        text_muted,
        surface,
    );
}

fn render_diagnostics(
    frame: &mut Frame<'_>,
    area: ratatui::layout::Rect,
    model: &ViewModel,
    border_color: Color,
    text_primary: Color,
    text_muted: Color,
    surface: Color,
) {
    let mut lines = vec![
        Line::from(vec![
            Span::styled("gateway  ", Style::default().fg(text_muted)),
            Span::styled(
                model.gateway_summary.clone(),
                Style::default().fg(text_primary),
            ),
        ]),
        Line::from(vec![
            Span::styled("a2a      ", Style::default().fg(text_muted)),
            Span::styled(model.a2a_summary.clone(), Style::default().fg(text_primary)),
        ]),
        Line::from(vec![
            Span::styled("telemetry", Style::default().fg(text_muted)),
            Span::styled(
                model.telemetry_summary.clone(),
                Style::default().fg(text_primary),
            ),
        ]),
        Line::from(Span::styled(
            "env      (allowlisted only)",
            Style::default().fg(text_muted),
        )),
    ];
    if model.env_allowlist.is_empty() {
        lines.push(Line::from(Span::styled(
            "<allowlist empty>",
            Style::default().fg(text_muted),
        )));
    } else {
        let line: Vec<Span> = model
            .env_allowlist
            .iter()
            .map(|(key, value)| {
                Span::styled(
                    format!("{}={} ", key, value.as_deref().unwrap_or("<unset>")),
                    Style::default().fg(text_muted),
                )
            })
            .collect();
        lines.push(Line::from(line));
    }
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: true }).block(
        Block::default()
            .title("Diagnostics")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .style(Style::default().bg(surface).fg(text_primary)),
    );
    frame.render_widget(paragraph, area);
}

fn render_agents(
    frame: &mut Frame<'_>,
    area: ratatui::layout::Rect,
    model: &ViewModel,
    border_color: Color,
    text_primary: Color,
    text_muted: Color,
    surface: Color,
) {
    let rows = model.agents.iter().map(|agent| {
        Row::new(vec![
            Cell::from(agent.name.clone()).style(Style::default().fg(text_primary)),
            Cell::from(format!(
                "{}/{}/{}",
                state_label(agent.launcher_state),
                state_label(agent.authentication_state),
                state_label(agent.inference_state),
            ))
            .style(Style::default().fg(text_muted)),
            Cell::from(agent.version.clone()).style(Style::default().fg(text_primary)),
            Cell::from(route_label(agent.route)).style(Style::default().fg(text_muted)),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Length(30),
            Constraint::Min(8),
            Constraint::Length(16),
        ],
    )
    .header(
        Row::new(["Agent", "Launcher/Auth/Inference", "Version", "Route"])
            .style(Style::default().fg(text_muted).add_modifier(Modifier::BOLD)),
    )
    .block(
        Block::default()
            .title("Agents")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .style(Style::default().bg(surface).fg(text_primary)),
    );
    frame.render_widget(table, area);
}

fn hex_color(hex: &str, fallback: Color) -> Color {
    match crate::theme::parse_hex_rgb(hex) {
        Some((r, g, b)) => Color::Rgb(r, g, b),
        None => fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ThemeId, palette_for};
    use crate::view_model::ViewModel;
    use ratatui::{Terminal, backend::TestBackend};
    use vibemux_probe::{
        A2aSelfTestProbe, AgentKind, AgentProbe, GatewayProbe, LauncherKind, ProbeReport,
        ProbeState, RouteKind,
    };

    fn fixture_report() -> ProbeReport {
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

    fn rendered(model: &ViewModel, palette: &ThemePalette, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| render_debug_dashboard(frame, model, palette))
            .expect("render");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn ratatui_test_backend_renders_agents_and_diagnostics() {
        let model = ViewModel::from_report(&fixture_report());
        let palette = palette_for(ThemeId::Claude);
        let rendered = rendered(&model, &palette, 120, 32);
        assert!(rendered.contains("Claude"), "missing Claude");
        assert!(rendered.contains("Diagnostics"));
    }

    #[test]
    fn render_debug_dashboard_does_not_mention_native_tui() {
        let model = ViewModel::from_report(&fixture_report());
        let palette = palette_for(ThemeId::Claude);
        let rendered = rendered(&model, &palette, 120, 32);
        assert!(!rendered.contains("reserved"));
        assert!(!rendered.contains("Native CLI TUI surfaces"));
        assert!(!rendered.contains("native TUI"));
    }

    #[test]
    fn render_debug_dashboard_mentions_one_line_per_agent() {
        let model = ViewModel::from_report(&fixture_report());
        let palette = palette_for(ThemeId::Claude);
        let rendered = rendered(&model, &palette, 120, 32);
        for name in ["Claude", "Codex", "OpenCode", "Copilot", "Grok"] {
            assert!(rendered.contains(name), "missing {name}");
        }
    }

    #[test]
    fn render_debug_dashboard_snapshot_is_well_formed_utf8() {
        // The on-disk snapshot (`debug_snapshot`) is ASCII-only; the
        // live render is allowed to use the box-drawing Unicode that
        // ratatui emits by default. We assert the buffer decodes as
        // well-formed UTF-8 to catch encoding regressions, but do not
        // pin it to ASCII.
        let model = ViewModel::from_report(&fixture_report());
        let palette = palette_for(ThemeId::Github);
        let rendered = rendered(&model, &palette, 120, 32);
        // Strip control characters; anything that survives must be
        // well-formed UTF-8 (guaranteed by Rust's `String`) and not a
        // stray C0 control other than `\n`.
        for byte in rendered.bytes() {
            assert!(byte >= 0x20 || byte == b'\n', "control byte 0x{byte:02x}");
        }
    }

    #[test]
    fn narrow_terminal_keeps_all_agent_lines_visible() {
        let model = ViewModel::from_report(&fixture_report());
        let palette = palette_for(ThemeId::Vscode);
        let rendered = rendered(&model, &palette, 80, 24);
        for name in ["Claude", "Codex", "OpenCode", "Copilot", "Grok"] {
            assert!(rendered.contains(name), "missing {name}");
        }
    }
}
