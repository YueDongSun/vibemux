#![forbid(unsafe_code)]
//! The console's daemon feed against a real local daemon: it reads only, it
//! never writes, and it survives a daemon that is missing at first.

use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use vibemux_client::{
    daemon_feed::{DaemonFeed, FeedRequest},
    daemon_model::{DaemonView, Section},
};
use vibemux_types::{
    Run, RunSpec, Task, TaskSpec,
    a2a::{A2aRunStart, RunWorkspace},
    frontend::FrontendTasksQuery,
};
use vibemuxd::{
    WriterWorker,
    control::{ControlClient, DaemonControlServer},
    process::DaemonPaths,
};

const DEADLINE: Duration = Duration::from_secs(20);

fn daemon_paths(root: &Path) -> DaemonPaths {
    #[cfg(windows)]
    {
        let local_app_data = root.join("private_local_app_data");
        std::fs::create_dir_all(&local_app_data).expect("private local app data");
        DaemonPaths::from_project_root_with_local_app_data(root, &local_app_data)
            .expect("daemon paths")
    }
    #[cfg(not(windows))]
    DaemonPaths::from_project_root(root).expect("daemon paths")
}

/// One task with one run whose workspace path, branch, and ownership token
/// are markers the feed must never show.
fn seed_one_task(paths: &DaemonPaths) -> String {
    paths.ensure_runtime_dir().expect("runtime directory");
    let writer = WriterWorker::start(paths.database_path()).expect("writer");
    let handle = writer.handle().expect("writer handle");
    let project_id = handle
        .project_id()
        .expect("project query")
        .expect("project");
    let task = Task::new(TaskSpec {
        project_id,
        title: "alpha feed task".into(),
        description: String::new(),
    })
    .expect("task");
    let task_id = task.task_id().to_string();
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
                path: "/PRIVATE_PATH_MARKER/run".into(),
                branch: "PRIVATE_BRANCH_MARKER".into(),
                base_commit: "a".repeat(40),
                ownership_token: "PRIVATE_OWNER_MARKER".into(),
            },
            timestamp: time::OffsetDateTime::now_utc(),
            idempotency_key: format!("start_{run_id}"),
        })
        .expect("seed task and run");
    drop(handle);
    writer.shutdown().expect("stop setup writer");
    task_id
}

async fn wait_for(
    feed: &mut DaemonFeed,
    what: &str,
    predicate: impl Fn(&DaemonView) -> bool,
) -> Arc<DaemonView> {
    let started = Instant::now();
    let mut last = None;
    loop {
        if let Some(view) = feed.latest() {
            if predicate(&view) {
                return view;
            }
            last = Some(view);
        }
        assert!(
            started.elapsed() < DEADLINE,
            "timed out waiting for {what}; last view: {last:#?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn not_configured<T>(section: &Section<T>) -> bool {
    matches!(section, Section::Unavailable(reason) if reason.hint == "not configured")
}

/// `(latest_sequence, detail_revision)` of every task, read directly.
async fn task_revisions(client: &ControlClient) -> Vec<(u64, u64)> {
    client
        .frontend_tasks(FrontendTasksQuery::default())
        .await
        .expect("tasks")
        .tasks
        .iter()
        .map(|task| (task.latest_sequence, task.detail_revision))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn feed_reads_a_real_daemon_without_writing() {
    let temp = tempfile::tempdir().expect("temporary project");
    let paths = daemon_paths(temp.path());
    let task_id = seed_one_task(&paths);
    let server = DaemonControlServer::start_for_paths(&paths)
        .await
        .expect("control server");
    let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
    let revisions_before = task_revisions(&client).await;
    let descriptor_before = std::fs::read(server.descriptor_path()).expect("descriptor");

    let mut feed = DaemonFeed::start_for_paths(paths.clone()).expect("feed");
    let view = wait_for(&mut feed, "loaded tasks", |view| {
        matches!(&view.tasks, Section::Loaded(list) if list.rows.len() == 1)
            && matches!(view.health, Section::Loaded(_))
    })
    .await;
    assert_eq!(view.connection, "connected");
    assert!(not_configured(&view.routes), "{:?}", view.routes);
    assert!(not_configured(&view.dsh), "{:?}", view.dsh);
    assert!(not_configured(&view.slots), "{:?}", view.slots);
    // Decisions are stored records, so the list answers without a config.
    assert!(
        matches!(&view.decisions, Section::Loaded(rows) if rows.is_empty()),
        "{:?}",
        view.decisions
    );

    assert!(feed.request(FeedRequest::OpenTask(task_id.clone())));
    let view = wait_for(
        &mut feed,
        "task detail",
        |view| matches!(&view.task_detail, Some(Section::Loaded(detail)) if detail.runs.len() == 1),
    )
    .await;
    let text = format!("{view:?}");
    for marker in [
        "PRIVATE_PATH_MARKER",
        "PRIVATE_BRANCH_MARKER",
        "PRIVATE_OWNER_MARKER",
    ] {
        assert!(!text.contains(marker), "{marker} leaked: {text}");
    }
    assert!(text.contains("alpha feed task"));
    drop(feed);

    assert_eq!(task_revisions(&client).await, revisions_before);
    assert_eq!(
        std::fs::read(server.descriptor_path()).expect("descriptor"),
        descriptor_before
    );
    server.shutdown().await.expect("stop control server");
}

fn tasks_loaded(view: &DaemonView) -> bool {
    matches!(&view.tasks, Section::Loaded(list) if list.rows.len() == 1)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn feed_follows_a_daemon_that_stops_and_starts_again() {
    let temp = tempfile::tempdir().expect("temporary project");
    let paths = daemon_paths(temp.path());
    seed_one_task(&paths);
    let server = DaemonControlServer::start_for_paths(&paths)
        .await
        .expect("control server");
    let mut feed = DaemonFeed::start_for_paths(paths.clone()).expect("feed");
    wait_for(&mut feed, "loaded tasks", tasks_loaded).await;

    server.shutdown().await.expect("stop control server");
    assert!(feed.request(FeedRequest::Refresh));
    wait_for(&mut feed, "daemon_not_running", |view| {
        matches!(&view.tasks, Section::Unavailable(reason) if reason.code == "daemon_not_running")
    })
    .await;

    let server = DaemonControlServer::start_for_paths(&paths)
        .await
        .expect("restarted control server");
    assert!(feed.request(FeedRequest::Refresh));
    let view = wait_for(&mut feed, "tasks after restart", tasks_loaded).await;
    assert_eq!(view.connection, "connected");
    drop(feed);
    server
        .shutdown()
        .await
        .expect("stop restarted control server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn feed_notices_a_crashed_daemon_within_one_short_poll() {
    let temp = tempfile::tempdir().expect("temporary project");
    let paths = daemon_paths(temp.path());
    seed_one_task(&paths);
    let server = DaemonControlServer::start_for_paths(&paths)
        .await
        .expect("control server");
    let descriptor_path = server.descriptor_path().to_path_buf();
    let descriptor = std::fs::read(&descriptor_path).expect("descriptor");
    let mut feed = DaemonFeed::start_for_paths(paths.clone()).expect("feed");
    wait_for(&mut feed, "loaded tasks", tasks_loaded).await;

    // A crashed daemon leaves its descriptor behind with nothing listening.
    server.shutdown().await.expect("stop control server");
    paths.ensure_runtime_dir().expect("runtime directory");
    std::fs::write(&descriptor_path, &descriptor).expect("stale descriptor");
    let crashed = Instant::now();
    assert!(feed.request(FeedRequest::Refresh));
    let view = wait_for(&mut feed, "an unavailable daemon", |view| {
        matches!(&view.tasks, Section::Unavailable(_))
    })
    .await;
    assert!(
        crashed.elapsed() < Duration::from_secs(6),
        "took {:?}: {}",
        crashed.elapsed(),
        view.connection
    );
    assert!(
        view.connection.contains("vibemuxctl daemon inspect"),
        "{}",
        view.connection
    );
    drop(feed);
    std::fs::remove_file(&descriptor_path).expect("remove stale descriptor");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn feed_reports_not_running_then_recovers_after_start() {
    let temp = tempfile::tempdir().expect("temporary project");
    let paths = daemon_paths(temp.path());
    seed_one_task(&paths);
    let mut feed = DaemonFeed::start_for_paths(paths.clone()).expect("feed");
    let view = wait_for(&mut feed, "daemon_not_running", |view| {
        matches!(&view.tasks, Section::Unavailable(reason) if reason.code == "daemon_not_running")
    })
    .await;
    assert!(view.connection.contains("daemon_not_running"));

    let server = DaemonControlServer::start_for_paths(&paths)
        .await
        .expect("control server");
    assert!(feed.request(FeedRequest::Refresh));
    wait_for(
        &mut feed,
        "tasks after start",
        |view| matches!(&view.tasks, Section::Loaded(list) if list.rows.len() == 1),
    )
    .await;
    drop(feed);
    server.shutdown().await.expect("stop control server");
}
