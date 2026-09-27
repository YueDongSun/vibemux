//! Explicit synthetic preview fixtures, available only to screenshot builds.
use crate::{ViewModel, supervisor_model::*};
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
                "Preview: daemon disconnected. Last observations may be stale.".into()
            } else {
                "Preview: task status connected; continuous coordinator chat is unavailable.".into()
            }),
        },
        tasks,
        next_cursor: None,
        observed_at: Some("2026-09-27T10:24:00Z".into()),
        mode: SnapshotMode::Demo,
    }
}
