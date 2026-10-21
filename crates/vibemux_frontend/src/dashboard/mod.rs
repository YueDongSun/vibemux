//! Read-only dashboard model, navigation and renderers: the navigable inspector
//! behind `vibemuxctl ui` and the `vibemux_dashboard` binary. It consumes a
//! probe report plus optional local Git inventory and never starts a daemon,
//! a harness, or a model turn.
pub mod daemon_model;
mod daemon_render;
pub mod filter;
mod model;
pub mod panes;
mod render;
mod runtime;
mod state;
pub use daemon_model::{DaemonView, Section};
pub use daemon_render::{visible_decisions, visible_task_ids, visible_tasks};
pub use model::{AgentView, DashboardModel, NativeTuiSlot, NativeTuiSlotState, safe_text};
pub use render::{list_counts, render_dashboard, render_with_state};
pub use runtime::{run_interactive, snapshot};
pub use state::{Branch, Command, Context, ListCounts, State, Tab, Workspace};

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
    fn narrow_windows_terminal_names_every_harness_and_the_rows_in_view() {
        // Regression: at 80x24 the overview table showed five of eleven
        // harnesses with no sign of the rest, and the slot panel's
        // `name [state]` labels overflowed before CodeBuddy, Kimi, and DSH.
        let mut report = report();
        for probe in report.agents.iter_mut().skip(5) {
            probe.launcher_state = ProbeState::Unavailable;
        }
        let model = DashboardModel::from_report(&report);
        let total = AgentKind::all().len();
        let context = Context::default();
        let first = snapshot(&model, &State::default(), &context, 80, 24).unwrap();
        assert!(
            first.contains(&format!("Agent probes 1-4/{total}")),
            "{first}"
        );
        assert!(first.contains("> Claude"));
        assert!(first.contains("reserved: Claude, Codex, OpenCode, Copilot, Grok"));
        assert!(first.contains("unavailable: Qwen, iFlow, TRAE, CodeBuddy, Kimi, DSH"));
        let last = State {
            selected_agent: total - 1,
            ..State::default()
        };
        let scrolled = snapshot(&model, &last, &context, 80, 24).unwrap();
        assert!(
            scrolled.contains(&format!("Agent probes {}-{total}/{total}", total - 3)),
            "{scrolled}"
        );
        assert!(scrolled.contains("> DSH"));
        assert!(!scrolled.contains("> Claude"));
        // Below the two-pane width the Harnesses page has no table, so the
        // details title carries the position.
        let details = State {
            tab: Tab::Harnesses,
            selected_agent: 3,
            ..State::default()
        };
        let narrow = snapshot(&model, &details, &context, 70, 24).unwrap();
        assert!(narrow.contains(&format!("Selected harness 4/{total} / j,k")));
        assert!(narrow.contains("Copilot / version"));
    }

    #[test]
    fn every_slot_name_fits_the_slot_panel_at_every_supported_width() {
        let model = DashboardModel::from_report(&report());
        for (width, height) in [(30u16, 24u16), (59, 24), (60, 24), (80, 24)] {
            let rendered = snapshot(
                &model,
                &State::default(),
                &Context::default(),
                width,
                height,
            )
            .unwrap();
            let rows = rendered.lines().collect::<Vec<_>>();
            let top = rows
                .iter()
                .position(|row| row.contains("Native CLI TUI surfaces"))
                .unwrap_or_else(|| panic!("no slot panel at width {width}"));
            let bottom = top
                + rows[top..]
                    .iter()
                    .position(|row| row.contains('└'))
                    .expect("slot panel bottom border");
            let panel = rows[top + 1..bottom].join(" ");
            for agent in AgentKind::all() {
                assert!(
                    panel.contains(agent.display_name()),
                    "width {width}: {} missing from {panel}",
                    agent.display_name()
                );
            }
        }
    }

    #[test]
    fn filtered_agents_match_name_state_and_adapter() {
        let mut report = report();
        report.agents[1].launcher_state = ProbeState::Unavailable;
        let model = DashboardModel::from_report(&report);
        let names = |query: &str| {
            model
                .filtered_agents(query)
                .into_iter()
                .map(|agent| agent.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names("claude"), ["Claude"]);
        assert_eq!(names("UNAVAILABLE"), ["Codex"]);
        assert!(names("probe_only").contains(&"OpenCode".to_string()));
        assert_eq!(names("").len(), AgentKind::all().len());
        assert!(names("no such harness").is_empty());
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
            ..Context::default()
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
    fn every_agent_table_width_keeps_adapter_support() {
        let model = DashboardModel::from_report(&report());
        let context = Context::default();
        // The overview table spans the terminal below the wide layout; the
        // Harnesses page gives the table 45% of the width beside the details.
        for (tab, width, route_column) in [
            (Tab::Overview, 60, true),
            (Tab::Overview, 44, false),
            (Tab::Harnesses, 80, false),
            (Tab::Harnesses, 120, true),
        ] {
            let state = State {
                tab,
                ..State::default()
            };
            let rendered = snapshot(&model, &state, &context, width, 24).unwrap();
            let header = rendered
                .lines()
                .find(|line| line.contains("Agent") && line.contains("Launcher"))
                .unwrap_or_else(|| panic!("no agent table header at width {width}"));
            assert!(header.contains("Adapter"), "width {width}: {header}");
            assert_eq!(
                header.contains("Route"),
                route_column,
                "width {width}: {header}"
            );
            for label in ["execute", "probe"] {
                assert!(rendered.contains(label), "width {width} lacks {label}");
            }
        }
    }

    #[test]
    fn overview_keeps_every_diagnostics_line_and_grows_the_table_with_height() {
        // Regression: a fixed eight-row table cut the telemetry line at 80x24
        // and kept five agents on screen however tall the terminal was.
        let model = DashboardModel::from_report(&report());
        let total = AgentKind::all().len();
        let context = Context::default();
        for (width, height, title) in [
            (80u16, 24u16, format!("Agent probes 1-4/{total}")),
            (60, 24, format!("Agent probes 1-4/{total}")),
            (45, 30, format!("Agent probes 1-5/{total}")),
            (100, 28, format!("Agent probes 1-8/{total}")),
            (100, 40, "Agent probes─".to_string()),
        ] {
            let rendered = snapshot(&model, &State::default(), &context, width, height).unwrap();
            for label in ["Gateway:", "A2A:", "Telemetry:"] {
                assert!(
                    rendered.contains(label),
                    "{width}x{height} lacks {label}\n{rendered}"
                );
            }
            assert!(
                rendered.contains(&title),
                "{width}x{height} lacks {title}\n{rendered}"
            );
        }
        let tall = snapshot(&model, &State::default(), &context, 100, 40).unwrap();
        for agent in AgentKind::all() {
            let name = agent.display_name();
            assert!(
                tall.lines().any(|line| {
                    line.starts_with(&format!("│  {name} "))
                        || line.starts_with(&format!("│> {name} "))
                }),
                "{name} hidden at 100x40"
            );
        }
    }

    #[test]
    fn details_state_the_selected_harness_slot_not_a_fixed_reserved_line() {
        // Regression: every harness's details said "Reserved is not running."
        // even when its slot was unavailable for lack of a verified launcher.
        let mut report = report();
        report.agents.truncate(AgentKind::all().len() - 1);
        report.agents[1].launcher_state = ProbeState::Unavailable;
        let model = DashboardModel::from_report(&report);
        let context = Context::default();
        // Claude is verified, Codex unavailable, and DSH has no observation.
        for (selected, expected, absent) in [
            (0, "reserved, not running", "unavailable"),
            (1, "unavailable, no verified", "reserved, not running"),
            (
                AgentKind::all().len() - 1,
                "unavailable, no verified",
                "reserved, not running",
            ),
        ] {
            let state = State {
                tab: Tab::Harnesses,
                selected_agent: selected,
                ..State::default()
            };
            for width in [70u16, 100] {
                let rendered = snapshot(&model, &state, &context, width, 28).unwrap();
                let details = rendered
                    .lines()
                    .skip_while(|line| !line.contains("Selected harness"))
                    .take_while(|line| !line.contains("Native CLI TUI surfaces"))
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(
                    details.contains(&format!("Native TUI slot: {expected}")),
                    "width {width}, agent {selected}: {details}"
                );
                assert!(
                    !details.contains(&format!("Native TUI slot: {absent}")),
                    "width {width}, agent {selected}: {details}"
                );
                assert!(!details.contains("Reserved is not running"));
            }
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

    mod console {
        use super::*;
        use crate::dashboard::{
            daemon_model::{
                DecisionRow, DshView, HealthView, PluginRow, RouteRow, SlotsView, TaskDetailView,
                TaskList, TaskRow,
            },
            panes::LayoutMode,
        };
        use crossterm::event::KeyCode;
        use std::sync::Arc;

        fn task(title: &str) -> TaskRow {
            TaskRow {
                task_id: format!("task_{title}"),
                title: title.to_string(),
                status: "in_progress".to_string(),
            }
        }

        fn loaded() -> DaemonView {
            DaemonView {
                connection: "connected".to_string(),
                observed_at: Some("2026-10-21T10:00:00Z".to_string()),
                health: Section::Loaded(HealthView {
                    status: "healthy".to_string(),
                    schema: "9".to_string(),
                    queue: "0/64 (peak 1)".to_string(),
                }),
                plugins: Section::Loaded(vec![PluginRow {
                    plugin_id: "wezterm_observer".to_string(),
                    state: "active".to_string(),
                    restarts: "0/5".to_string(),
                    last_error: "-".to_string(),
                }]),
                routes: Section::Loaded(vec![RouteRow {
                    harness: "Codex".to_string(),
                    protocol: "codex_exec".to_string(),
                    mode: "executes".to_string(),
                }]),
                tasks: Section::Loaded(TaskList {
                    rows: vec![task("alpha"), task("beta")],
                    more: false,
                }),
                task_detail: Some(Section::Loaded(TaskDetailView {
                    task_id: "task_alpha".to_string(),
                    title: "alpha".to_string(),
                    runs: vec!["abc12345 codex implementer running".to_string()],
                    events: vec!["#3 run_started 2026-10-21T10:00:00Z".to_string()],
                    artifacts: Vec::new(),
                    truncated: true,
                })),
                dsh: Section::Loaded(DshView::new(
                    "ready",
                    "deepseek",
                    "chat",
                    [0, 0, 0, 0, 0],
                    true,
                )),
                slots: Section::Loaded(SlotsView {
                    evidence_class: "fixture".to_string(),
                    rows: vec!["track_a codex executes".to_string()],
                }),
                decisions: Section::Loaded(vec![DecisionRow::new(
                    "0123456789abcdef",
                    "resolved",
                    Some("Codex"),
                    "local_rule",
                    "2026-10-21T10:00:00Z",
                    false,
                    4,
                )]),
            }
        }

        fn context(daemon: Option<DaemonView>) -> Context {
            Context {
                source: "local_probe".to_string(),
                daemon: daemon.map(Arc::new),
                ..Context::default()
            }
        }

        fn showing(view: Tab, mode: LayoutMode) -> State {
            let mut state = State {
                tab: view,
                ..State::default()
            };
            state.panes.mode = mode;
            state.panes.views[0] = view;
            state
        }

        #[test]
        fn every_view_renders_in_every_layout() {
            let model = DashboardModel::from_report(&report());
            for (daemon, connected) in [
                (loaded(), true),
                (DaemonView::not_running("daemon_not_running"), false),
            ] {
                let context = context(Some(daemon));
                for mode in [LayoutMode::Single, LayoutMode::Split, LayoutMode::Grid] {
                    for (width, height) in [(130u16, 40u16), (80, 24)] {
                        for view in Tab::ALL {
                            let state = showing(view, mode);
                            let rendered =
                                snapshot(&model, &state, &context, width, height).unwrap();
                            let at = format!("{view:?} {mode:?} {width}x{height}\n{rendered}");
                            assert!(rendered.contains("READ ONLY"), "{at}");
                            assert!(rendered.contains(view.label()), "{at}");
                            if view.is_daemon() && !connected {
                                assert!(rendered.contains("daemon_not_running"), "{at}");
                            }
                        }
                    }
                }
            }
            // The Daemon view's lines span the terminal, so the hint is whole.
            let state = showing(Tab::Daemon, LayoutMode::Single);
            let not_running = context(Some(DaemonView::not_running("daemon_not_running")));
            let wide = snapshot(&model, &state, &not_running, 130, 40).unwrap();
            assert!(
                wide.contains("start it with vibemuxctl daemon start"),
                "{wide}"
            );
        }

        #[test]
        fn grid_shows_four_titled_panes() {
            let model = DashboardModel::from_report(&report());
            let mut state = State::default();
            state.panes.mode = LayoutMode::Grid;
            let rendered = snapshot(&model, &state, &context(Some(loaded())), 130, 40).unwrap();
            for title in ["1 Overview", "4 Tasks", "5 Daemon", "6 Sessions"] {
                assert!(rendered.contains(title), "{title}\n{rendered}");
            }
            assert!(
                rendered.contains("> 1 Overview"),
                "the focused pane is marked"
            );
            assert!(rendered.contains("layout grid"), "{rendered}");
        }

        #[test]
        fn loaded_daemon_views_show_their_rows() {
            let model = DashboardModel::from_report(&report());
            let context = context(Some(loaded()));
            for (view, expected) in [
                (
                    Tab::Tasks,
                    vec!["alpha", "beta", "run_started", "truncated"],
                ),
                (
                    Tab::Daemon,
                    vec!["healthy", "wezterm_observer", "codex_exec"],
                ),
                (
                    Tab::Sessions,
                    vec!["ready", "deepseek", "track_a codex executes"],
                ),
                (
                    Tab::Decisions,
                    vec!["01234567", "resolved", "Codex", "local_rule"],
                ),
            ] {
                let rendered = snapshot(
                    &model,
                    &showing(view, LayoutMode::Single),
                    &context,
                    130,
                    40,
                )
                .unwrap();
                for text in expected {
                    assert!(rendered.contains(text), "{view:?} lacks {text}\n{rendered}");
                }
            }
        }

        #[test]
        fn filter_title_and_rows() {
            let model = DashboardModel::from_report(&report());
            let context = context(Some(loaded()));
            let mut state = showing(Tab::Tasks, LayoutMode::Single);
            state.filter.start(Tab::Tasks);
            for character in "alp".chars() {
                state
                    .filter
                    .handle_editing_key(KeyCode::Char(character).into());
            }
            state.filter.editing = None;
            let rendered = snapshot(&model, &state, &context, 130, 40).unwrap();
            assert!(rendered.contains("filter: alp"), "{rendered}");
            assert!(rendered.contains("alpha"));
            assert!(!rendered.contains("beta"), "{rendered}");
            let counts = list_counts(&model, &context, &state);
            assert_eq!(counts.tasks, 1);
            assert_eq!(counts.agents, AgentKind::all().len());
            let daemon = context.daemon.as_deref().unwrap();
            assert_eq!(visible_task_ids(daemon, "alp"), ["task_alpha"]);
            assert_eq!(visible_task_ids(daemon, "").len(), 2);
        }

        #[test]
        fn no_feed_message_without_daemon_view() {
            let model = DashboardModel::from_report(&report());
            let rendered = snapshot(
                &model,
                &showing(Tab::Tasks, LayoutMode::Single),
                &context(None),
                100,
                28,
            )
            .unwrap();
            assert!(rendered.contains("Daemon views need the interactive console"));
        }

        #[test]
        fn help_lists_every_console_key() {
            let model = DashboardModel::from_report(&report());
            let state = State {
                help_visible: true,
                ..State::default()
            };
            let rendered = snapshot(&model, &state, &context(None), 130, 40).unwrap();
            for key in [
                "L: cycle layouts",
                "z: zoom",
                "r: refresh",
                "/: filter",
                "t: theme",
                "1-7: views",
                "Tab: next",
                "q/Esc: quit",
            ] {
                assert!(rendered.contains(key), "{key}\n{rendered}");
            }
        }
    }
}
