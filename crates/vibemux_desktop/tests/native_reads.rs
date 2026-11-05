//! Read services through a fixed fake boundary and a real temporary daemon.
use serde_json::json;
use std::time::SystemTime;
use uuid::Uuid;
use vibemux_client::{
    daemon_model::{DecisionRow, DshView, HealthView, PluginRow, RouteRow, SlotsView},
    workspace_inventory::Workspace,
};
use vibemux_desktop::{read_contract::*, read_services::*};
use vibemux_harness::{HarnessProtocol, HarnessRow};
use vibemux_types::{
    ArtifactId, ProjectId, Run, RunId, RunSpec, RunStatus, Task, TaskId, TaskSpec, TaskStatus,
    a2a::{A2aRunStart, RunWorkspace},
    frontend::{
        FrontendArtifactSummary, FrontendEventSummary, FrontendRunSummary, FrontendTaskDetail,
        FrontendTaskSummary, FrontendTasksPage, FrontendTasksQuery,
    },
};
use vibemuxd::{
    WriterWorker,
    control::{ControlClient, DaemonControlServer},
    process::DaemonPaths,
};

fn summary(id: u128) -> FrontendTaskSummary {
    FrontendTaskSummary {
        task_id: TaskId::from_uuid(Uuid::from_u128(id)).expect("task"),
        project_id: ProjectId::from_uuid(Uuid::from_u128(1)).expect("project"),
        title: "Task <script>not executable</script>\n中文".into(),
        title_truncated: false,
        status: TaskStatus::InProgress,
        latest_sequence: 7,
        detail_revision: 9,
    }
}

struct FakeReads {
    calls: Vec<&'static str>,
    page: FrontendTasksPage,
    detail: Option<FrontendTaskDetail>,
    plugins: Vec<PluginRow>,
    next_decision: Option<u64>,
}

impl Default for FakeReads {
    fn default() -> Self {
        Self {
            calls: Vec::new(),
            page: FrontendTasksPage {
                project_id: Some(summary(10).project_id),
                tasks: vec![summary(10), summary(11)],
                next_cursor: Some(summary(11).task_id),
            },
            detail: None,
            plugins: Vec::new(),
            next_decision: Some(10),
        }
    }
}

impl ReadPort for FakeReads {
    fn task_page(&mut self, _: FrontendTasksQuery) -> Result<FrontendTasksPage, String> {
        self.calls.push("tasks");
        Ok(self.page.clone())
    }
    fn task_detail(&mut self, _: TaskId) -> Result<Option<FrontendTaskDetail>, String> {
        self.calls.push("detail");
        Ok(self.detail.clone())
    }
    fn health(&mut self) -> Result<HealthView, String> {
        self.calls.push("health");
        Ok(HealthView {
            status: "healthy".into(),
            schema: "9".into(),
            queue: "0/64".into(),
        })
    }
    fn harnesses(&mut self) -> Result<Vec<HarnessRow>, String> {
        self.calls.push("harnesses");
        Ok(vec![HarnessRow {
            name: "claude".into(),
            command: vec!["PRIVATE_COMMAND".into()],
            protocol: HarnessProtocol::Pty,
            provider: "configured provider".into(),
            available: true,
            path: Some("PRIVATE_EXECUTABLE_PATH".into()),
            roles: Vec::new(),
            default: true,
            launcher: None,
            version: Some("2.1.fixture".into()),
        }])
    }
    fn plugins(&mut self) -> Result<Vec<PluginRow>, String> {
        self.calls.push("plugins");
        Ok(self.plugins.clone())
    }
    fn routes(&mut self) -> Result<Vec<RouteRow>, String> {
        self.calls.push("routes");
        Err("harness_dispatch_unconfigured".into())
    }
    fn dsh(&mut self) -> Result<DshView, String> {
        self.calls.push("dsh");
        Err("dsh_session_not_found".into())
    }
    fn slots(&mut self) -> Result<SlotsView, String> {
        self.calls.push("slots");
        Err("workflow_unconfigured".into())
    }
    fn decisions(&mut self, _: u64) -> Result<DecisionReadPage, String> {
        self.calls.push("decisions");
        Ok(DecisionReadPage {
            rows: vec![DecisionRow::new(
                "decision_fixture",
                "recorded",
                Some("Claude"),
                "local_rule",
                "2026-10-31T00:00:00Z",
                false,
                10,
            )],
            next_after_sequence: self.next_decision,
        })
    }
    fn workspace(&mut self) -> Result<Workspace, String> {
        self.calls.push("workspace");
        Ok(Workspace {
            current_branch: "feature/read_views".into(),
            head: "abc12345".into(),
            worktrees: vec!["PRIVATE_WORKTREE_PATH".into()],
            ..Default::default()
        })
    }
}

#[test]
fn task_queries_are_typed_bounded_and_keep_canonical_page_identity() {
    let mut reads = FakeReads::default();
    let result = read_tasks(
        &mut reads,
        TaskReadQuery::List {
            after: None,
            limit: 2,
        },
    )
    .expect("page");
    let TaskReadResult::Page {
        tasks,
        next_cursor,
        project_id,
        ..
    } = result
    else {
        panic!("page")
    };
    assert_eq!(tasks.len(), 2);
    assert_eq!(next_cursor, Some(summary(11).task_id));
    assert_eq!(project_id, Some(summary(10).project_id));
    assert_eq!(tasks[0].status, TaskStatus::InProgress);
    assert!(!tasks[0].title.contains('\n'));
    assert_eq!(
        read_tasks(
            &mut reads,
            TaskReadQuery::List {
                after: None,
                limit: 33
            }
        )
        .expect_err("bounded page")
        .code,
        "read_invalid_query"
    );
    assert_eq!(
        reads.calls,
        ["tasks"],
        "invalid arguments must not reach the read boundary"
    );
    for bad in [
        json!({"kind":"detail","task_id":"C:\\private"}),
        json!({"kind":"detail","task_id":"task_00000000-0000-0000-0000-000000000000"}),
        json!({"kind":"list","cwd":"forged"}),
        json!({"kind":"run","method":"shutdown"}),
    ] {
        assert!(serde_json::from_value::<TaskReadQuery>(bad).is_err());
    }
}

#[test]
fn a_cross_project_or_looping_page_is_rejected_without_relabeling_it() {
    let mut reads = FakeReads::default();
    reads.page.tasks[0].project_id =
        ProjectId::from_uuid(Uuid::from_u128(99)).expect("other project");
    assert_eq!(
        read_tasks(
            &mut reads,
            TaskReadQuery::List {
                after: None,
                limit: 32
            }
        )
        .expect_err("different project")
        .code,
        "read_contract_invalid"
    );
    let mut reads = FakeReads::default();
    let previous_cursor = reads.page.next_cursor;
    assert_eq!(
        read_tasks(
            &mut reads,
            TaskReadQuery::List {
                after: previous_cursor,
                limit: 32
            }
        )
        .expect_err("cursor loop")
        .code,
        "read_contract_invalid"
    );
}

#[test]
fn detail_returns_bounded_run_event_artifact_facts_without_paths_or_digests() {
    let task = summary(10);
    let run_id = RunId::from_uuid(Uuid::from_u128(20)).expect("run");
    let mut reads = FakeReads {
        detail: Some(FrontendTaskDetail {
            task: task.clone(),
            runs: (0..15)
                .map(|index| FrontendRunSummary {
                    run_id: RunId::from_uuid(Uuid::from_u128(20 + index)).expect("run"),
                    task_id: task.task_id,
                    harness: "claude".into(),
                    role: "worker".into(),
                    status: RunStatus::Running,
                    worktree_path: Some("PRIVATE_WORKTREE_PATH".into()),
                    worktree_path_truncated: false,
                    branch: Some("feature/read_views".into()),
                })
                .collect(),
            events: (0..26)
                .map(|index| FrontendEventSummary {
                    sequence: index,
                    event_type: "harness.started".into(),
                    at: "2026-10-31T00:00:00Z".into(),
                    run_id: Some(run_id),
                })
                .collect(),
            artifacts: (0..26)
                .map(|index| FrontendArtifactSummary {
                    run_id,
                    artifact_id: ArtifactId::from_uuid(Uuid::from_u128(100 + index))
                        .expect("artifact"),
                    sha256: "PRIVATE_ARTIFACT_DIGEST".into(),
                    size_bytes: 17,
                    media_type: "text/plain".into(),
                })
                .collect(),
            has_more_runs: false,
            has_more_events: false,
            has_more_artifacts: false,
        }),
        ..Default::default()
    };
    let result = read_tasks(
        &mut reads,
        TaskReadQuery::Detail {
            task_id: task.task_id,
        },
    )
    .expect("detail");
    let serialized = serde_json::to_string(&result).expect("DTO");
    assert!(!serialized.contains("PRIVATE_"));
    let TaskReadResult::Detail { task: detail, .. } = result else {
        panic!("detail")
    };
    assert_eq!(detail.runs.len(), 12);
    assert_eq!(detail.events.len(), 24);
    assert_eq!(detail.artifacts.len(), 24);
    assert!(detail.has_more_runs && detail.has_more_events && detail.has_more_artifacts);
    assert!(detail.runs[0].has_worktree);
    assert_eq!(detail.runs[0].branch.as_deref(), Some("feature/read_views"));
    assert_eq!(detail.runs[0].status, RunStatus::Running);
    assert_eq!(detail.artifacts[0].size_bytes, 17);
}

#[test]
fn all_seven_console_sections_use_only_their_fixed_read_operations() {
    for (section, expected) in [
        (ConsoleSection::Overview, vec!["health"]),
        (ConsoleSection::Harnesses, vec!["harnesses", "routes"]),
        (ConsoleSection::Workspace, vec!["workspace"]),
        (ConsoleSection::Tasks, vec!["tasks"]),
        (ConsoleSection::Daemon, vec!["health", "routes", "plugins"]),
        (ConsoleSection::Sessions, vec!["dsh", "slots"]),
        (ConsoleSection::Decisions, vec!["decisions"]),
    ] {
        let mut reads = FakeReads::default();
        let snapshot = read_console(
            &mut reads,
            ConsoleQuery {
                section,
                after_sequence: None,
            },
        )
        .expect("section");
        assert_eq!(snapshot.section, section);
        assert_eq!(reads.calls, expected);
        assert!(
            !serde_json::to_string(&snapshot)
                .expect("DTO")
                .contains("PRIVATE_")
        );
    }
}

#[test]
fn console_preserves_partial_unavailability_and_bounded_display_rows() {
    let mut reads = FakeReads {
        plugins: (0..70)
            .map(|index| PluginRow {
                plugin_id: format!("plugin_{index}"),
                state: "ready".into(),
                restarts: "0/3".into(),
                last_error: "-".into(),
            })
            .collect(),
        ..Default::default()
    };
    let snapshot = read_console(
        &mut reads,
        ConsoleQuery {
            section: ConsoleSection::Daemon,
            after_sequence: None,
        },
    )
    .expect("daemon view");
    assert_eq!(snapshot.blocks[0].availability, ReadAvailability::Loaded);
    assert_eq!(
        snapshot.blocks[1].availability,
        ReadAvailability::Unavailable
    );
    assert_eq!(
        snapshot.blocks[1].code.as_deref(),
        Some("harness_dispatch_unconfigured")
    );
    assert_eq!(snapshot.blocks[2].rows.len(), 64);
    assert!(snapshot.blocks[2].truncated);
    let error = ReadError::unavailable("PRIVATE_PATH\\token\nsecret");
    assert_eq!(error.code, "read_operation_failed");
    assert!(!error.message.contains("PRIVATE"));
}

#[test]
fn console_cursors_do_not_select_arbitrary_reads_or_loop() {
    let mut reads = FakeReads::default();
    assert_eq!(
        read_console(
            &mut reads,
            ConsoleQuery {
                section: ConsoleSection::Workspace,
                after_sequence: Some(1)
            }
        )
        .expect_err("invalid cursor scope")
        .code,
        "read_invalid_query"
    );
    assert!(reads.calls.is_empty());
    let snapshot = read_console(
        &mut reads,
        ConsoleQuery {
            section: ConsoleSection::Decisions,
            after_sequence: Some(0),
        },
    )
    .expect("page");
    assert_eq!(snapshot.next_after_sequence, Some(10));
    assert_eq!(
        read_console(
            &mut reads,
            ConsoleQuery {
                section: ConsoleSection::Decisions,
                after_sequence: Some(10)
            }
        )
        .expect_err("nonadvancing cursor")
        .code,
        "read_contract_invalid"
    );
    assert!(serde_json::from_value::<ConsoleQuery>(json!({"section":"shutdown"})).is_err());
    assert!(
        serde_json::from_value::<ConsoleQuery>(json!({"section":"overview","method":"execute"}))
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_control_reads_leave_canonical_state_and_runtime_descriptor_unchanged() {
    let temp = tempfile::tempdir().expect("temporary project");
    #[cfg(windows)]
    let paths = {
        let local = temp.path().join("private_local_app_data");
        std::fs::create_dir(&local).expect("private runtime root");
        DaemonPaths::from_project_root_with_local_app_data(temp.path(), &local).expect("paths")
    };
    #[cfg(not(windows))]
    let paths = DaemonPaths::from_project_root(temp.path()).expect("paths");
    paths.ensure_runtime_dir().expect("runtime");
    let writer = WriterWorker::start(paths.database_path()).expect("writer");
    let handle = writer.handle().expect("writer handle");
    let project_id = handle
        .project_id()
        .expect("project query")
        .expect("project");
    let task = Task::new(TaskSpec {
        project_id,
        title: "read fixture task".into(),
        description: String::new(),
    })
    .expect("task");
    let task_id = task.task_id();
    let run = Run::new(RunSpec {
        project_id,
        task_id,
        harness: "fixture".into(),
        role: "worker".into(),
        protocol: "a2a".into(),
        base_commit: "a".repeat(40),
    })
    .expect("run");
    let run_id = run.run_id();
    handle
        .start_a2a_run(A2aRunStart {
            task,
            run,
            peer_id: "fixture_peer".into(),
            external_task_id: "fixture_external".into(),
            transport: "http_json".into(),
            protocol_version: "1.0".into(),
            workspace: RunWorkspace {
                path: "/PRIVATE_PATH_MARKER/run".into(),
                branch: "fixture_branch".into(),
                base_commit: "a".repeat(40),
                ownership_token: "PRIVATE_OWNER_MARKER".into(),
            },
            timestamp: SystemTime::now().into(),
            idempotency_key: "fixture_start".into(),
        })
        .expect("seed canonical task");
    drop(handle);
    writer.shutdown().expect("stop setup writer");
    let server = DaemonControlServer::start_for_paths(&paths)
        .await
        .expect("server");
    let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
    let before = client
        .frontend_task(task_id)
        .await
        .expect("before")
        .expect("task");
    let descriptor = std::fs::read(server.descriptor_path()).expect("owned test descriptor");
    let read_paths = paths.clone();
    let result = tokio::task::spawn_blocking(move || {
        tasks_snapshot(&read_paths, TaskReadQuery::Detail { task_id })
    })
    .await
    .expect("blocking read")
    .expect("detail");
    let TaskReadResult::Detail { task: detail, .. } = result else {
        panic!("detail")
    };
    assert_eq!(detail.task.task_id, task_id);
    assert_eq!(detail.runs[0].run_id, run_id);
    assert!(
        !serde_json::to_string(&detail)
            .expect("renderer DTO")
            .contains("PRIVATE_")
    );
    let read_paths = paths.clone();
    let snapshot = tokio::task::spawn_blocking(move || {
        console_section(
            &read_paths,
            ConsoleQuery {
                section: ConsoleSection::Daemon,
                after_sequence: None,
            },
        )
    })
    .await
    .expect("blocking console")
    .expect("console");
    assert_eq!(snapshot.blocks[0].availability, ReadAvailability::Loaded);
    assert_eq!(
        snapshot.blocks[1].code.as_deref(),
        Some("harness_dispatch_unconfigured")
    );
    let after = client
        .frontend_task(task_id)
        .await
        .expect("after")
        .expect("task");
    assert_eq!(before.task.detail_revision, after.task.detail_revision);
    assert_eq!(before.task.latest_sequence, after.task.latest_sequence);
    assert!(
        descriptor == std::fs::read(server.descriptor_path()).expect("descriptor after"),
        "reads must not rewrite descriptor"
    );
    server.shutdown().await.expect("stop owned server");
}
