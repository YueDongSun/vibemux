#![forbid(unsafe_code)]
use std::time::{Duration, Instant};

use vibemux_web::{
    console_model::{ConsoleConnection, ConsoleSection, ConsoleSnapshot},
    console_poller::ConsolePoller,
};
use vibemuxd::{control::DaemonControlServer, process::DaemonPaths};

fn daemon_paths(root: &std::path::Path) -> DaemonPaths {
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
    paths
}

fn not_configured<T>(section: &ConsoleSection<T>) -> bool {
    matches!(section, ConsoleSection::Unavailable { hint, .. } if hint == "not configured")
}

fn every_section_unavailable(snapshot: &ConsoleSnapshot, code: &str) -> bool {
    let unavailable =
        |state: &serde_json::Value| state["state"] == "unavailable" && state["code"] == code;
    let encoded = serde_json::to_value(snapshot).expect("encode snapshot");
    [
        "connection",
        "health",
        "plugins",
        "routes",
        "dsh",
        "slots",
        "decisions",
    ]
    .iter()
    .all(|section| unavailable(&encoded[*section]))
}

async fn refresh(poller: &mut ConsolePoller) -> ConsoleSnapshot {
    tokio::time::timeout(Duration::from_secs(20), poller.refresh())
        .await
        .expect("refresh deadline")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_reads_a_real_daemon_and_reports_unconfigured_features() {
    let temp = tempfile::tempdir().expect("temporary project");
    let paths = daemon_paths(temp.path());
    let server = DaemonControlServer::start_for_paths(&paths)
        .await
        .expect("control server");
    let mut poller = ConsolePoller::new(paths, Duration::from_secs(5));
    let snapshot = refresh(&mut poller).await;
    assert_eq!(snapshot.connection, ConsoleConnection::Connected);
    assert!(snapshot.observed_at.is_some());
    let ConsoleSection::Loaded { data: health } = &snapshot.health else {
        panic!("health: {:?}", snapshot.health);
    };
    assert!(health.healthy);
    assert!(health.store_schema_version > 0);
    assert!(
        matches!(&snapshot.plugins, ConsoleSection::Loaded { data } if !data.truncated),
        "{:?}",
        snapshot.plugins
    );
    // A daemon started without dispatch, DSH, or workflow configuration
    // answers each of those reads with "not configured". Recorded decisions
    // are listed whether or not decision routing is configured.
    assert!(not_configured(&snapshot.routes), "{:?}", snapshot.routes);
    assert!(not_configured(&snapshot.dsh), "{:?}", snapshot.dsh);
    assert!(not_configured(&snapshot.slots), "{:?}", snapshot.slots);
    assert!(
        matches!(&snapshot.decisions, ConsoleSection::Loaded { data } if data.rows.is_empty() && !data.truncated),
        "{:?}",
        snapshot.decisions
    );
    let encoded = serde_json::to_string(&snapshot).expect("encode snapshot");
    let project_path = temp.path().to_string_lossy().into_owned();
    let escaped_path = serde_json::to_string(&project_path).expect("escape path");
    assert!(!encoded.contains(project_path.as_str()));
    assert!(!encoded.contains(escaped_path.trim_matches('"')));
    assert!(!encoded.contains("process_id"));
    server.shutdown().await.expect("stop control server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_follows_a_daemon_that_stops_crashes_and_starts_again() {
    let temp = tempfile::tempdir().expect("temporary project");
    let paths = daemon_paths(temp.path());
    let mut poller = ConsolePoller::new(paths.clone(), Duration::from_secs(5));
    let before_start = refresh(&mut poller).await;
    assert!(
        every_section_unavailable(&before_start, "daemon_not_running"),
        "{before_start:?}"
    );

    let server = DaemonControlServer::start_for_paths(&paths)
        .await
        .expect("control server");
    let descriptor_path = server.descriptor_path().to_path_buf();
    let descriptor = std::fs::read(&descriptor_path).expect("descriptor");
    assert_eq!(
        refresh(&mut poller).await.connection,
        ConsoleConnection::Connected
    );

    // A crashed daemon leaves its descriptor behind with nothing listening;
    // the health deadline keeps the poll short.
    server.shutdown().await.expect("stop control server");
    paths.ensure_runtime_dir().expect("runtime directory");
    std::fs::write(&descriptor_path, &descriptor).expect("stale descriptor");
    let crashed = Instant::now();
    let after_crash = refresh(&mut poller).await;
    assert!(
        crashed.elapsed() < Duration::from_secs(6),
        "took {:?}",
        crashed.elapsed()
    );
    let ConsoleConnection::Unavailable { hint, .. } = &after_crash.connection else {
        panic!("connection: {:?}", after_crash.connection);
    };
    assert!(hint.contains("vibemuxctl daemon inspect"), "{hint}");
    assert!(matches!(
        after_crash.health,
        ConsoleSection::Unavailable { .. }
    ));
    std::fs::remove_file(&descriptor_path).expect("remove stale descriptor");

    let restarted = DaemonControlServer::start_for_paths(&paths)
        .await
        .expect("restarted control server");
    assert_eq!(
        refresh(&mut poller).await.connection,
        ConsoleConnection::Connected
    );
    restarted.shutdown().await.expect("stop restarted server");
}
