#![cfg(feature = "test_helpers")]

use std::{path::PathBuf, time::Duration};

use vibemux_cli::{DaemonBootstrapConfig, DaemonCliError, start_daemon};
use vibemuxd::process::DaemonPaths;

#[tokio::test]
async fn startup_timeout_terminates_only_the_spawned_fixture() {
    let temp = tempfile::tempdir().expect("temp project");
    let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
    let config = DaemonBootstrapConfig::new(
        paths.clone(),
        PathBuf::from(env!("CARGO_BIN_EXE_vibemux_hang_fixture")),
    )
    .with_timing(
        Duration::from_millis(150),
        Duration::from_millis(5),
        Duration::from_millis(20),
    );

    assert_eq!(
        start_daemon(&config)
            .await
            .expect_err("fixture must exceed startup deadline"),
        DaemonCliError::StartupTimeout
    );
    let started = paths.runtime_dir().join("hang_fixture_started");
    let completed = paths.runtime_dir().join("hang_fixture_completed");
    assert!(started.is_file());
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert!(!completed.exists());
    assert!(!paths.descriptor_path().exists());
    assert!(!paths.writer_lock_path().exists());
}
