use std::time::Duration;

use vibemux_types::{
    Run, RunSpec, Task, TaskSpec,
    a2a::{A2aRunStart, RunWorkspace},
};
use vibemux_web::control_bridge::BridgePoller;
use vibemuxd::{WriterWorker, control::DaemonControlServer, process::DaemonPaths};

async fn seed_daemon(root: &std::path::Path, task_count: usize) -> DaemonPaths {
    #[cfg(windows)]
    let paths = {
        let local_app_data = root.join("private_local_app_data");
        std::fs::create_dir_all(&local_app_data).expect("private local app data");
        DaemonPaths::from_project_root_with_local_app_data(root, &local_app_data)
            .expect("daemon paths")
    };
    #[cfg(not(windows))]
    let paths = DaemonPaths::from_project_root(root).expect("daemon paths");
    paths.ensure_runtime_dir().expect("runtime directory");
    let writer = WriterWorker::start(paths.database_path()).expect("writer");
    let handle = writer.handle().expect("writer handle");
    let project_id = handle
        .project_id()
        .expect("project query")
        .expect("project");
    for index in 0..task_count {
        let task = Task::new(TaskSpec {
            project_id,
            title: format!("Task {index} PRIVATE_TITLE_MARKER"),
            description: String::new(),
        })
        .expect("task");
        let run = Run::new(RunSpec {
            project_id,
            task_id: task.task_id(),
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
                external_task_id: format!("external_{run_id}"),
                transport: "http_json".into(),
                protocol_version: "1.0".into(),
                workspace: RunWorkspace {
                    path: format!("/vibemux_test/{run_id}"),
                    branch: format!("codex/run_{run_id}"),
                    base_commit: "a".repeat(40),
                    ownership_token: format!("owner_{run_id}"),
                },
                timestamp: time::OffsetDateTime::now_utc(),
                idempotency_key: format!("start_{run_id}"),
            })
            .expect("seed task and run");
    }
    writer.shutdown().expect("stop setup writer");
    paths
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refresh_uses_real_control_pipe_and_projects_only_allowlisted_fields() {
    let temp = tempfile::tempdir().expect("temporary project");
    let paths = seed_daemon(temp.path(), 2).await;
    let server = DaemonControlServer::start_for_paths(&paths)
        .await
        .expect("control server");
    let mut poller = BridgePoller::new(paths, Duration::from_secs(5));
    let snapshot = tokio::time::timeout(Duration::from_secs(20), poller.refresh())
        .await
        .expect("refresh deadline")
        .expect("refresh");
    assert!(!snapshot.stale);
    assert_eq!(snapshot.tasks.len(), 2);
    assert!(
        snapshot
            .generation
            .as_deref()
            .is_some_and(|value| !value.is_empty())
    );
    assert_eq!(snapshot.tasks[0].runs.len(), 1);
    assert_eq!(snapshot.activity.len(), 2);
    let first = &snapshot.tasks[0];
    let activity = &snapshot.activity[&first.task_id];
    assert!(!activity.events.is_empty());
    assert!(activity.events.iter().all(|event| {
        event.run_id.is_none() || event.run_id.as_deref() == Some(first.runs[0].run_id.as_str())
    }));
    let encoded = serde_json::to_string(&snapshot).expect("serialize snapshot");
    let project_path = temp.path().to_string_lossy().into_owned();
    let escaped_path = serde_json::to_string(&project_path).expect("escape path");
    for forbidden in [
        "/vibemux_test/",
        "owner_",
        project_path.as_str(),
        escaped_path.trim_matches('"'),
    ] {
        assert!(!encoded.contains(forbidden));
    }
    assert!(encoded.contains("Task 0"));
    server.shutdown().await.expect("stop first control server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn descriptor_failure_is_bounded_and_daemon_restart_changes_generation() {
    let temp = tempfile::tempdir().expect("temporary project");
    let paths = seed_daemon(temp.path(), 1).await;
    let server = DaemonControlServer::start_for_paths(&paths)
        .await
        .expect("control server");
    let mut poller = BridgePoller::new(paths.clone(), Duration::from_secs(5));
    let first = tokio::time::timeout(Duration::from_secs(20), poller.refresh())
        .await
        .expect("refresh deadline")
        .expect("first refresh");
    server.shutdown().await.expect("stop before restart");
    let failed = tokio::time::timeout(Duration::from_secs(5), poller.refresh()).await;
    assert!(failed.expect("bounded missing-daemon probe").is_err());
    let restarted_server = DaemonControlServer::start_for_paths(&paths)
        .await
        .expect("restarted control server");
    let restarted = tokio::time::timeout(Duration::from_secs(20), poller.refresh())
        .await
        .expect("restart refresh deadline")
        .expect("restart refresh");
    assert_ne!(first.generation, restarted.generation);
    assert_eq!(restarted.tasks.len(), 1);
    restarted_server
        .shutdown()
        .await
        .expect("stop restarted server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lifecycle_marks_last_snapshot_stale_and_hides_titles_after_control_failure() {
    let temp = tempfile::tempdir().expect("temporary project");
    let paths = seed_daemon(temp.path(), 1).await;
    let server = DaemonControlServer::start_for_paths(&paths)
        .await
        .expect("control server");
    let poller = BridgePoller::new(paths, Duration::from_secs(5));
    let mut poller = poller;
    let initial = tokio::time::timeout(Duration::from_secs(20), poller.refresh())
        .await
        .expect("refresh deadline")
        .expect("first refresh");
    assert!(initial.tasks[0].title.is_some());
    server.shutdown().await.expect("stop control server");

    let state = std::sync::Arc::new(tokio::sync::RwLock::new(initial));
    let (stop, receiver) = tokio::sync::watch::channel(false);
    let worker = tokio::spawn(poller.run(state.clone(), receiver));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if state.read().await.stale {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("stale snapshot deadline");
    let stale = state.read().await;
    assert!(stale.tasks[0].title.is_none());
    assert!(stale.tasks[0].state == "in_progress" || stale.tasks[0].state == "open");
    drop(stale);
    stop.send_replace(true);
    worker.await.expect("join bridge lifecycle");
}
