//! Read-only dashboard model, navigation and renderers: the navigable inspector
//! behind `vibemuxctl ui` and the `vibemux_dashboard` binary. It consumes a
//! probe report plus optional local Git inventory and never starts a daemon,
//! a harness, or a model turn.
mod model;
mod render;
mod runtime;
mod state;
pub use model::{AgentView, DashboardModel, NativeTuiSlot, NativeTuiSlotState, safe_text};
pub use render::{render_dashboard, render_with_state};
pub use runtime::{run_interactive, snapshot};
pub use state::{Branch, Context, State, Tab, Workspace};

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};
    use vibemux_probe::{
        A2aSelfTestProbe, AgentKind, AgentProbe, GatewayProbe, ProbeReport, ProbeState, RouteKind,
    };

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
    fn dashboard_reserves_every_native_tui() {
        let model = DashboardModel::from_report(&report());
        assert_eq!(model.native_tui_slots.len(), AgentKind::all().len());
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

    #[test]
    fn plain_snapshot_is_deterministic_and_read_only() {
        let snapshot = DashboardModel::from_report(&report()).plain_snapshot();
        assert!(snapshot.contains("Claude native TUI state=reserved"));
        assert!(snapshot.contains("a2a: verified correlation=true listener_closed=true"));
    }

    #[test]
    fn adapter_support_is_a_build_fact_shown_even_without_an_observation() {
        let mut report = report();
        report.agents.clear();
        let model = DashboardModel::from_report(&report);
        let snapshot = model.plain_snapshot();
        assert!(snapshot.contains("launcher=not_run"));
        assert!(snapshot.contains("adapter=execute protocols=claude_stream_json"));
        assert!(snapshot.contains("adapter=probe_only protocols=acp"));
        assert!(snapshot.contains("adapter=terminal_only protocols=-"));
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
        for expected in ["Adapter", "execute", "probe", "terminal"] {
            assert!(rendered.contains(expected), "missing {expected}");
        }
    }

    #[test]
    fn navigation_views_and_small_terminals_render_without_authority_changes() {
        let model = DashboardModel::from_report(&report());
        let workspace = Workspace {
            current_branch: "codex/topic".to_string(),
            head: "12345678".to_string(),
            baseline: "origin/main".to_string(),
            change_detection: "external_filters_disabled".to_string(),
            branches: vec![Branch {
                name: "codex/topic".to_string(),
                role: "current".to_string(),
                commit: "12345678".to_string(),
                upstream: "-".to_string(),
            }],
            worktrees: vec!["codex/topic -> <primary>".to_string()],
            ..Workspace::default()
        };
        let context = Context {
            source: "saved_report".to_string(),
            workspace: Some(workspace),
        };
        for tab in [Tab::Overview, Tab::Harnesses, Tab::Workspace] {
            let state = State {
                tab,
                ..State::default()
            };
            for (width, height) in [(100, 28), (80, 24)] {
                let rendered = snapshot(&model, &state, &context, width, height).unwrap();
                assert!(rendered.contains("READ ONLY"));
                if tab == Tab::Workspace {
                    assert!(rendered.contains("codex/topic"));
                    assert!(rendered.contains("Changes: staged="));
                    assert!(rendered.contains("External filters disabled"));
                    assert!(rendered.contains("current"), "a branch row must be visible");
                }
                if tab == Tab::Harnesses {
                    assert!(rendered.contains("Native attach: disabled"));
                    assert!(rendered.contains("Inference: not_run"));
                    assert!(rendered.contains("Adapter: "));
                }
            }
            assert!(snapshot(&model, &state, &context, 20, 6).is_ok());
        }
    }

    #[test]
    fn standalone_views_retain_source_without_inventing_workspace_metadata() {
        let model = DashboardModel::from_report(&report());
        for source in ["saved_report", "local_probe"] {
            let context = Context {
                source: source.to_string(),
                ..Context::default()
            };
            for tab in [Tab::Overview, Tab::Harnesses, Tab::Workspace] {
                let state = State {
                    tab,
                    ..State::default()
                };
                let rendered = snapshot(&model, &state, &context, 100, 28).unwrap();
                assert!(rendered.contains(&format!("READ ONLY / {source}")));
                if tab == Tab::Workspace {
                    assert!(rendered.contains("Workspace inventory not supplied"));
                }
            }
        }
    }

    #[test]
    fn control_characters_and_missing_observations_are_not_rendered_as_readiness() {
        let mut report = report();
        report.agents.truncate(1);
        report.agents[0].version = Some("fixture\u{1b}[31m\u{202e}version".to_string());
        let model = DashboardModel::from_report(&report);
        assert_eq!(model.agents.len(), AgentKind::all().len());
        assert_eq!(model.agents[1].launcher_state, "not_run");
        let text = model.plain_snapshot();
        assert!(!text.contains('\u{1b}'));
        assert!(!text.contains('\u{202e}'));
        assert!(!text.contains("127.0.0.1"));
    }
}
