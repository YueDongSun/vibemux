#![forbid(unsafe_code)]
//! TUI dashboard renderer: audited themes, the six-column ten-agent
//! table ported from the single-shell frontend, and the per-theme
//! golden fixture machinery. No per-harness detail; that lives in the
//! GUI now.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap},
};

use crate::tui::theme::{
    Theme, health_style, muted_style, route_style, state_style, telemetry_style, title_style,
};
use crate::view_model::ViewModel;
use vibemux_probe::{ProbeState, RouteKind};

pub fn render_debug_dashboard(frame: &mut Frame<'_>, model: &ViewModel, theme: Theme) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        // Title + diagnostics + agent table; per-harness detail (slots)
        // stays in the GUI. The stacked body needs 13 rows for ten agent
        // rows + header + borders plus 5 for diagnostics, so 18 guarantees
        // none of the ten rows is cut.
        .constraints([
            Constraint::Length(3),
            Constraint::Min(18),
            Constraint::Length(3),
        ])
        .split(frame.area());
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            &model.title,
            title_style(theme).add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("  [{}]  {}  ", model.platform, model.observed_at)),
        Span::styled(
            &model.overall_status,
            health_style(theme, model.health).add_modifier(Modifier::BOLD),
        ),
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
            .constraints([Constraint::Min(13), Constraint::Min(5)])
            .split(areas[1])
    };
    // Fixed columns (agent + three state columns + route) take 55 cells and
    // the six columns consume five 1-cell gaps inside the bordered table
    // interior; whatever remains belongs to Version. Ellipsize against that
    // real budget so narrow terminals show a clean ellipsis instead of a
    // hard mid-token cut, while wide tables keep full version strings.
    let version_budget = body[0].width.saturating_sub(2 + 55 + 5).max(10) as usize;
    let rows = model.agents.iter().map(|agent| {
        // A failed launcher jumps out when the whole row is bold; per-cell
        // state colors still apply on top for the state columns.
        let row_emphasis = if agent.launcher_state == ProbeState::Failed {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        Row::new(vec![
            Cell::from(agent.name.clone()),
            Cell::from(state_label(agent.launcher_state).to_string())
                .style(state_style(theme, agent.launcher_state)),
            Cell::from(state_label(agent.authentication_state).to_string())
                .style(state_style(theme, agent.authentication_state)),
            Cell::from(state_label(agent.inference_state).to_string())
                .style(state_style(theme, agent.inference_state)),
            Cell::from(ellipsize(&agent.version, version_budget)),
            Cell::from(route_label(agent.route).to_string()).style(route_style(theme, agent.route)),
        ])
        .style(row_emphasis)
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
        Line::from(vec![
            Span::raw("Gateway:   "),
            Span::styled(
                model.gateway_summary.clone(),
                state_style(theme, model.gateway_state),
            ),
        ]),
        Line::from(vec![
            Span::raw("A2A:       "),
            Span::styled(
                model.a2a_summary.clone(),
                state_style(theme, model.a2a_state),
            ),
        ]),
        Line::from(vec![
            Span::raw("Telemetry: "),
            Span::styled(
                model.telemetry_summary.clone(),
                telemetry_style(theme, model.telemetry_available, model.telemetry_failures),
            ),
        ]),
        Line::from(Span::styled(
            "env        (allowlisted only)",
            muted_style(theme),
        )),
        if model.env_allowlist.is_empty() {
            Line::from(Span::styled("<allowlist empty>", muted_style(theme)))
        } else {
            let line: Vec<Span> = model
                .env_allowlist
                .iter()
                .map(|(key, value)| {
                    Span::styled(
                        format!("{}={} ", key, value.as_deref().unwrap_or("<unset>")),
                        muted_style(theme),
                    )
                })
                .collect();
            Line::from(line)
        },
    ])
    .wrap(Wrap { trim: true })
    .block(Block::default().title("Diagnostics").borders(Borders::ALL));
    frame.render_widget(diagnostics, body[1]);

    let footer = Paragraph::new(Line::from(vec![
        Span::raw("q/Esc: quit"),
        Span::raw(" | "),
        Span::styled("c: copy snapshot", muted_style(theme)),
        Span::raw(" | "),
        Span::styled(
            format!("theme: {} (t to cycle)", theme.name()),
            muted_style(theme),
        ),
        Span::raw(" | "),
        Span::styled(
            model.overall_status.clone(),
            health_style(theme, model.health),
        ),
    ]))
    .block(Block::default().borders(Borders::ALL));
    frame.render_widget(footer, areas[2]);
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

/// Truncate `text` to `max_width` visible cells with an ellipsis so a
/// clipped cell announces its own truncation instead of ending mid-token.
fn ellipsize(text: &str, max_width: usize) -> String {
    if text.chars().count() <= max_width {
        return text.to_string();
    }
    let kept: String = text.chars().take(max_width.saturating_sub(1)).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view_model::ViewModel;
    use ratatui::{Terminal, backend::TestBackend};
    use std::path::{Path, PathBuf};
    use vibemux_probe::{
        A2aSelfTestProbe, AgentKind, AgentProbe, GatewayProbe, LauncherKind, ProbeReport,
        ProbeState, RouteKind,
    };

    fn report() -> ProbeReport {
        ProbeReport {
            schema_version: 1,
            observed_at_epoch_seconds: 0,
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
        probe.observed_at_epoch_seconds = 1_767_225_600;
        probe.agents = versions
            .into_iter()
            .map(|(agent, version, route)| AgentProbe {
                agent,
                launcher_state: ProbeState::Verified,
                authentication_state: ProbeState::NotRun,
                inference_state: ProbeState::NotRun,
                launcher: LauncherKind::DirectExecutable,
                path: None,
                version: Some(version.to_string()),
                route,
                endpoints: Vec::new(),
                code: "version_verified".to_string(),
            })
            .collect();
        // The five domestic agents are not installed on the capture machine:
        // their probes report an unavailable launcher with no version and an
        // unknown route, exactly matching the live snapshot output.
        let domestic = [
            AgentKind::Qwen,
            AgentKind::Iflow,
            AgentKind::Trae,
            AgentKind::Codebuddy,
            AgentKind::Kimi,
        ];
        for agent in domestic {
            probe.agents.push(AgentProbe {
                agent,
                launcher_state: ProbeState::Unavailable,
                authentication_state: ProbeState::NotRun,
                inference_state: ProbeState::NotRun,
                launcher: LauncherKind::Unavailable,
                path: None,
                version: None,
                route: RouteKind::Unknown,
                endpoints: Vec::new(),
                code: "launcher_unavailable".to_string(),
            });
        }
        probe
    }

    fn render_text_with_theme(model: &ViewModel, theme: Theme, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| render_debug_dashboard(frame, model, theme))
            .expect("render dashboard");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn render_text(model: &ViewModel, width: u16, height: u16) -> String {
        render_text_with_theme(model, Theme::Classic, width, height)
    }

    #[test]
    fn real_probe_states_and_versions_are_never_clipped() {
        let model = ViewModel::from_report(&real_report());
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
    fn stacked_layout_renders_all_ten_agent_rows() {
        // Regression: Min(12) fit only nine data rows (header + borders), so
        // the tenth agent (Kimi) was silently cut. The Min(13) table plus
        // the Min(18) outer body guarantee all ten rows at the golden sizes.
        let model = ViewModel::from_report(&real_report());
        for (width, height) in [(80u16, 24u16), (120, 32)] {
            let rendered = render_text(&model, width, height);
            for name in [
                "Claude",
                "Codex",
                "OpenCode",
                "Copilot",
                "Grok",
                "Qwen",
                "iFlow",
                "TRAE",
                "CodeBuddy",
                "Kimi",
            ] {
                assert!(
                    rendered.contains(name),
                    "missing agent {name} at {width}x{height}"
                );
            }
        }
    }

    #[test]
    fn render_debug_dashboard_does_not_mention_native_tui() {
        let model = ViewModel::from_report(&report());
        let rendered = render_text(&model, 120, 32);
        assert!(!rendered.contains("reserved"));
        assert!(!rendered.contains("Native CLI TUI surfaces"));
        assert!(!rendered.contains("native TUI"));
    }

    #[test]
    fn every_theme_keeps_real_data_unclipped_at_common_sizes() {
        let model = ViewModel::from_report(&real_report());
        for theme in Theme::ALL {
            // 80 columns physically cannot fit the full Copilot version next
            // to the state columns; state words and short versions must fit.
            let narrow = render_text_with_theme(&model, theme, 80, 24);
            for expected in ["verified", "not_run", "local_gateway", "2.1.259"] {
                assert!(
                    narrow.contains(expected),
                    "missing {expected} for theme {} at 80x24",
                    theme.name()
                );
            }
            for (width, height) in [(120u16, 32u16), (160, 40)] {
                let rendered = render_text_with_theme(&model, theme, width, height);
                for expected in [
                    "verified",
                    "not_run",
                    "2.1.259 (Claude Code)",
                    "grok 1.0.13 (5e9a58528b76) [stable]",
                    "local_gateway",
                ] {
                    assert!(
                        rendered.contains(expected),
                        "missing {expected} for theme {} at {width}x{height}",
                        theme.name()
                    );
                }
            }
        }
    }

    #[test]
    fn side_by_side_layout_pins_the_table_to_one_row_band() {
        let model = ViewModel::from_report(&report());
        let rows_of = |width: u16, height: u16| {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).expect("test terminal");
            terminal
                .draw(|frame| render_debug_dashboard(frame, &model, Theme::Classic))
                .expect("render dashboard");
            terminal
                .backend()
                .buffer()
                .content()
                .chunks(width as usize)
                .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
                .collect::<Vec<_>>()
        };
        // At 160 columns the diagnostics panel shares rows with agent rows.
        let wide = rows_of(160, 40);
        assert!(
            wide.iter()
                .any(|row| row.contains("Launcher") && row.contains("127.0.0.1:15721")),
            "wide layout must render diagnostics beside the agent table"
        );
        // At and below 120 columns the panels stack: no row mixes both.
        for narrow in [rows_of(80, 24), rows_of(120, 32)] {
            assert!(
                !narrow
                    .iter()
                    .any(|row| row.contains("Launcher") && row.contains("127.0.0.1:15721")),
                "narrow layouts must stack the panels on separate rows"
            );
        }
    }

    #[test]
    fn footer_shows_active_theme_name() {
        let model = ViewModel::from_report(&report());
        for theme in Theme::ALL {
            let rendered = render_text_with_theme(&model, theme, 160, 40);
            assert!(
                rendered.contains(&format!("theme: {} (t to cycle)", theme.name())),
                "footer missing theme label {}",
                theme.name()
            );
        }
    }

    #[test]
    fn failure_state_renders_failed_agent_in_dashboard() {
        let mut probe = report();
        probe.agents[0].launcher_state = ProbeState::Failed;
        let model = ViewModel::from_report(&probe);
        let rendered = render_text(&model, 120, 32);
        assert!(rendered.contains("failed"));
        assert!(model.overall_status.contains("1 failed"));
    }

    #[test]
    fn ellipsize_ends_with_an_ellipsis_announcement() {
        assert_eq!(ellipsize("short", 10), "short");
        assert_eq!(ellipsize("exactlyten", 10), "exactlyten");
        assert_eq!(ellipsize("elevenchars", 10).chars().count(), 10);
        assert!(ellipsize("elevenchars", 10).ends_with('…'));
    }

    fn golden_fixture_path(theme: Theme, width: u16, height: u16) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/golden")
            .join(format!(
                "dashboard_{}_{}x{}.txt",
                theme.name(),
                width,
                height
            ))
    }

    fn canonical_render(model: &ViewModel, theme: Theme, width: u16, height: u16) -> String {
        // Trailing spaces come from the right border padding; stripping them
        // keeps fixtures stable against buffer-width noise.
        render_text_with_theme(model, theme, width, height)
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    /// Regenerate the golden fixtures after an intentional visual change:
    /// `cargo test -p vibemux_frontend write_golden_fixtures -- --ignored --nocapture`
    #[test]
    #[ignore = "generator for golden render fixtures"]
    fn write_golden_fixtures() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/golden");
        std::fs::create_dir_all(&directory).expect("create golden fixture directory");
        for theme in Theme::ALL {
            for (width, height) in [(80u16, 24u16), (120, 32)] {
                let path = golden_fixture_path(theme, width, height);
                std::fs::write(&path, canonical_render(&model(), theme, width, height))
                    .expect("write golden fixture");
                println!("wrote {}", path.display());
            }
        }
    }

    fn model() -> ViewModel {
        let mut vm = ViewModel::from_report(&real_report());
        // `ViewModel::from_report` reads the live process environment for
        // the allowlisted diagnostics, so golden snapshots built from it
        // would embed machine-specific values (PATH, HOME, ...) and fail
        // on every other machine, platform, or CI runner — exactly what
        // happened when the fixtures first met Linux. Pin a deterministic
        // sample instead: the same allowlisted keys in allowlist order,
        // fixed cross-platform values, every third key unset (renders as
        // `<unset>`), and one long PATH to exercise paragraph wrapping.
        vm.env_allowlist = crate::probe_env::PROBE_ENVIRONMENT_ALLOWLIST
            .iter()
            .enumerate()
            .map(|(idx, key)| {
                let value = match *key {
                    "PATH" => Some(
                        "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/usr/games"
                            .to_string(),
                    ),
                    _ if idx % 3 == 2 => None,
                    _ => Some(format!("/opt/fixture/{}", key.to_lowercase())),
                };
                ((*key).to_string(), value)
            })
            .collect();
        vm
    }

    #[test]
    fn golden_render_snapshots_match_fixtures() {
        for theme in Theme::ALL {
            for (width, height) in [(80u16, 24u16), (120, 32)] {
                let path = golden_fixture_path(theme, width, height);
                let expected =
                    std::fs::read_to_string(&path).unwrap_or_else(|error| {
                        panic!(
                            "missing golden fixture {} (run the ignored write_golden_fixtures test): {error}",
                            path.display()
                        )
                    });
                assert_eq!(
                    canonical_render(&model(), theme, width, height),
                    expected,
                    "golden fixture mismatch for theme {} at {width}x{height}",
                    theme.name()
                );
            }
        }
    }
}
