//! Explicit synthetic preview fixtures, available only to screenshot builds.
use std::sync::Arc;

use crate::{
    ViewModel,
    ask::{AskEnd, AskPhase, AskUpdate},
    dashboard::{
        Branch, DaemonView, Section, Workspace,
        daemon_model::{
            DecisionRow, DshView, HealthView, PluginRow, RouteRow, SlotsView, TaskList, TaskRow,
        },
    },
    gui::{AskAvailability, AskState, ConsoleData},
    supervisor_model::*,
};
use vibemux_probe::{
    A2aSelfTestProbe, AgentKind, AgentProbe, GatewayProbe, LauncherKind, ProbeReport, ProbeState,
    RouteKind,
};

pub fn probe_view() -> ViewModel {
    let report = ProbeReport {
        schema_version: 1,
        observed_at_epoch_seconds: 0,
        platform: "Windows · preview fixture".into(),
        agents: AgentKind::all()
            .into_iter()
            .map(|agent| AgentProbe {
                agent,
                launcher_state: ProbeState::NotRun,
                authentication_state: ProbeState::NotRun,
                inference_state: ProbeState::NotRun,
                launcher: LauncherKind::Unavailable,
                path: None,
                version: None,
                route: RouteKind::Unknown,
                endpoints: vec![],
                code: "preview_fixture".into(),
            })
            .collect(),
        gateway: GatewayProbe {
            state: ProbeState::NotRun,
            host: String::new(),
            port: 0,
            tcp_reachable: false,
            health_status: None,
            telemetry_state: ProbeState::NotRun,
            telemetry: vec![],
            code: "preview_fixture".into(),
        },
        a2a: A2aSelfTestProbe {
            state: ProbeState::NotRun,
            correlation_preserved: false,
            listener_closed: true,
            code: "preview_fixture".into(),
        },
    };
    let mut view = ViewModel::from_report(&report);
    view.env_allowlist.clear();
    view
}

/// Synthetic Ask state: the setup card of a folder without routes, or a
/// finished Codex reply followed by a Claude Ask still streaming.
pub fn ask_state(state: &mut AskState, setup: bool) {
    if setup {
        state.set_availability(AskAvailability::NotConfigured);
        return;
    }
    state.set_availability(AskAvailability::Ready(vec![
        AgentKind::Claude,
        AgentKind::Codex,
    ]));
    let asks = [
        (
            "Codex",
            "Where does the daemon decide that a run succeeded?",
            "RunStatus::Succeeded is set only by the writer worker, after a structured adapter, a verifier, or an explicit user decision reports success. PTY text never completes a run.",
            Some(AskEnd::Completed),
        ),
        (
            "Claude",
            "Summarize what changed in the last three commits.",
            "The last commits add the read-only Ask core: reply text per protocol,",
            None,
        ),
    ];
    for (harness, prompt, reply, end) in asks {
        if state.launch(harness.into(), prompt.into()).is_err() {
            return;
        }
        let _ = state.take_launch();
        state.apply_update(AskUpdate {
            request_id: uuid::Uuid::nil(),
            phase: if end.is_some() {
                AskPhase::Finished
            } else {
                AskPhase::Running
            },
            reply: reply.into(),
            truncated: false,
            end,
        });
    }
}

/// Synthetic Console data: a connected daemon with routes, tasks, a DSH
/// session, and decisions, and a small Git inventory.
pub fn console_data(data: &mut ConsoleData) {
    let mut daemon = DaemonView::waiting();
    daemon.connection = "connected".into();
    daemon.observed_at = Some("18:24:00".into());
    daemon.health = Section::Loaded(HealthView {
        status: "healthy".into(),
        schema: "9".into(),
        queue: "0 of 64 (high 1)".into(),
    });
    daemon.plugins = Section::Loaded(vec![PluginRow {
        plugin_id: "terminal_observer".into(),
        state: "running".into(),
        restarts: "0".into(),
        last_error: "-".into(),
    }]);
    daemon.routes = Section::Loaded(vec![
        RouteRow {
            harness: "claude".into(),
            protocol: "claude_stream_json".into(),
            mode: "executes".into(),
        },
        RouteRow {
            harness: "codex".into(),
            protocol: "codex_app_server".into(),
            mode: "executes".into(),
        },
    ]);
    daemon.tasks = Section::Loaded(TaskList {
        rows: vec![
            TaskRow {
                task_id: "task_ui".into(),
                title: "Build the supervisor workspace".into(),
                status: "running".into(),
            },
            TaskRow {
                task_id: "task_contract".into(),
                title: "Verify task and terminal ownership".into(),
                status: "blocked".into(),
            },
        ],
        more: false,
    });
    daemon.dsh = Section::Loaded(DshView::new(
        "ready",
        "deepseek",
        "deepseek-chat",
        [2, 2, 0, 0, 0],
        true,
    ));
    daemon.slots = Section::Loaded(SlotsView {
        evidence_class: "offline_fixture".into(),
        rows: vec![
            "coder claude executes".into(),
            "reviewer codex probe only".into(),
        ],
    });
    daemon.decisions = Section::Loaded(vec![DecisionRow::new(
        "4f2a9c1e7b3d4e5f",
        "recorded",
        Some("claude"),
        "local_rules",
        "2026-10-23T18:20:00Z",
        false,
        3,
    )]);
    data.daemon = Some(Arc::new(daemon));
    data.set_workspace(Ok(Workspace {
        current_branch: "feat/gui_console_page".into(),
        head: "b68f381f".into(),
        baseline: "origin/main".into(),
        ahead: 3,
        behind: 0,
        staged: 0,
        unstaged: 2,
        untracked: 1,
        change_detection: "external_filters_disabled".into(),
        branches: vec![
            Branch {
                name: "main".into(),
                role: "baseline".into(),
                commit: "b68f381f".into(),
                upstream: "origin/main".into(),
            },
            Branch {
                name: "feat/gui_console_page".into(),
                role: "current".into(),
                commit: "c0ffee12".into(),
                upstream: "-".into(),
            },
        ],
        worktrees: vec!["feat/gui_console_page -> C:\\work\\vibemux".into()],
    }));
}

pub fn supervisor_snapshot(disconnected: bool) -> SupervisorSnapshot {
    let tasks = [
        (
            "task_ui",
            "Build the supervisor workspace · 主控工作台",
            "in_progress",
            "Claude Code",
            "Worker started; waiting for a structured result",
        ),
        (
            "task_contract",
            "Verify task and terminal ownership",
            "blocked",
            "Codex",
            "Review needs input before the next Run",
        ),
        (
            "task_theme",
            "Unify the five supported themes",
            "done",
            "OpenCode",
            "Local verification accepted the recorded artifact",
        ),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (task_id, title, state, harness, update))| {
        let run_id = format!("run_{index}");
        TaskView {
            task_id: task_id.into(),
            title: title.into(),
            state: state.into(),
            executor: harness.into(),
            latest_update: update.into(),
            latest_sequence: index as u64 + 1,
            runs: vec![RunView {
                run_id: run_id.clone(),
                harness: harness.into(),
                role: "worker".into(),
                state: if index == 2 {
                    "succeeded".into()
                } else {
                    "running".into()
                },
                worktree: "C:\\Projects\\VibeMux\\.vibemux\\worktrees\\supervisor_chat_多语言测试"
                    .into(),
                branch: "codex/supervisor_chat".into(),
                artifacts: vec![ArtifactView {
                    artifact_id: "artifact_preview".into(),
                    name: "Recorded change summary".into(),
                    kind: "application/json".into(),
                    state: "Recorded reference".into(),
                    sha256: Some("a".repeat(64)),
                    size_bytes: Some(2048),
                    media_type: Some("application/json".into()),
                }],
                terminal: TerminalView {
                    status: if disconnected {
                        "Disconnected — inspect again".into()
                    } else {
                        "Linked terminal — observation only".into()
                    },
                    binding_id: (!disconnected).then(|| "preview_binding".into()),
                    plugin_id: Some("wezterm_observer".into()),
                    pane_id: Some("7".into()),
                    instance_id: Some("preview_instance".into()),
                    checked_at: Some("2026-09-27T10:24:00Z".into()),
                    candidates: vec![],
                },
            }],
            recent_events: vec![TaskEventView {
                event_id: "event_preview".into(),
                sequence: 42,
                kind: "a2a_run_started".into(),
                occurred_at: "2026-09-27T10:24:00Z".into(),
                run_id: Some(run_id),
                summary: update.into(),
            }],
            ..Default::default()
        }
    })
    .collect();
    SupervisorSnapshot {
        project_id: "preview_project".into(),
        project_name: "VibeMux".into(),
        coordinator: Some("Codex".into()),
        connection: ConnectionState {
            status: if disconnected {
                ConnectionStatus::Disconnected
            } else {
                ConnectionStatus::Connected
            },
            detail: Some(if disconnected {
                "Preview: the daemon is not running for this project.".into()
            } else {
                "Preview: task status connected; continuous coordinator chat is unavailable.".into()
            }),
            code: disconnected.then(|| "daemon_not_running".into()),
        },
        tasks,
        next_cursor: None,
        observed_at: Some("2026-09-27T10:24:00Z".into()),
        mode: SnapshotMode::Demo,
    }
}
