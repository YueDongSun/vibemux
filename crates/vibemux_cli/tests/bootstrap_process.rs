#![cfg(feature = "test_helpers")]

use std::{path::PathBuf, time::Duration};

use sysinfo::{Pid, ProcessesToUpdate, System};
use vibemux_cli::{
    DaemonBootstrapConfig, DaemonCliError,
    recovery::{RecoveryStatus, inspect_runtime, recover_runtime},
    start_daemon, stop_daemon,
};
use vibemuxd::process::DaemonPaths;

#[cfg(windows)]
struct ControlRuntimeCleanup(PathBuf);

#[cfg(windows)]
impl ControlRuntimeCleanup {
    fn new(paths: &DaemonPaths) -> Self {
        Self(paths.runtime_dir().to_path_buf())
    }
}

#[cfg(windows)]
impl Drop for ControlRuntimeCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn startup_timeout_terminates_only_the_spawned_fixture() {
    let temp = tempfile::tempdir().expect("temp project");
    let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
    #[cfg(windows)]
    let _control_cleanup = ControlRuntimeCleanup::new(&paths);
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
    let started = paths.state_dir().join("hang_fixture_started");
    let completed = paths.state_dir().join("hang_fixture_completed");
    assert!(started.is_file());
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert!(!completed.exists());
    assert!(!paths.descriptor_path().exists());
    assert!(!paths.writer_lock_path().exists());
}

#[tokio::test]
async fn abrupt_process_recovery_preserves_database_and_allows_restart() {
    let temp = tempfile::tempdir().expect("temp project");
    let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
    #[cfg(windows)]
    let _control_cleanup = ControlRuntimeCleanup::new(&paths);
    let config = DaemonBootstrapConfig::new(
        paths.clone(),
        PathBuf::from(env!("CARGO_BIN_EXE_vibemux_recovery_fixture")),
    );
    let started = start_daemon(&config).await.expect("start recovery fixture");
    let process_id = started.health().process_id;
    let mut guard = FixtureProcessGuard::new(process_id);
    assert_eq!(
        inspect_runtime(&paths)
            .await
            .expect("inspect live fixture")
            .status,
        RecoveryStatus::Running
    );

    assert!(kill_process(process_id));
    wait_for_process_exit(process_id).await;
    guard.disarm();
    let database_before = std::fs::read(paths.database_path()).expect("database before recovery");
    let inspection = inspect_runtime(&paths)
        .await
        .expect("inspect stale fixture");
    assert_eq!(inspection.status, RecoveryStatus::Recoverable);
    let confirmation = inspection.confirmation.expect("recovery confirmation");
    assert_eq!(
        recover_runtime(&paths, &"0".repeat(64))
            .await
            .expect_err("wrong confirmation must fail"),
        DaemonCliError::RecoveryConfirmationMismatch
    );
    assert!(paths.descriptor_path().exists());
    assert!(paths.writer_lock_path().exists());

    let outcome = recover_runtime(&paths, &confirmation)
        .await
        .expect("recover stale fixture");
    assert!(outcome.descriptor_removed);
    assert!(outcome.writer_lock_removed);
    assert_eq!(
        outcome.compatibility_lock_removed,
        paths.has_distinct_legacy_runtime()
    );
    assert!(!paths.legacy_writer_lock_path().exists());
    assert_eq!(
        std::fs::read(paths.database_path()).expect("database after recovery"),
        database_before
    );

    let restarted = start_daemon(&config)
        .await
        .expect("restart recovered fixture");
    let restarted_process_id = restarted.health().process_id;
    guard = FixtureProcessGuard::new(restarted_process_id);
    stop_daemon(&paths).await.expect("stop recovered fixture");
    wait_for_process_exit(restarted_process_id).await;
    guard.disarm();
    assert!(!paths.descriptor_path().exists());
    assert!(!paths.writer_lock_path().exists());
}

struct FixtureProcessGuard {
    process_id: Option<u32>,
}

impl FixtureProcessGuard {
    fn new(process_id: u32) -> Self {
        Self {
            process_id: Some(process_id),
        }
    }

    fn disarm(&mut self) {
        self.process_id = None;
    }
}

impl Drop for FixtureProcessGuard {
    fn drop(&mut self) {
        if let Some(process_id) = self.process_id {
            let _ = kill_process(process_id);
        }
    }
}

fn kill_process(process_id: u32) -> bool {
    let pid = Pid::from_u32(process_id);
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    system.process(pid).is_some_and(sysinfo::Process::kill)
}

async fn wait_for_process_exit(process_id: u32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let pid = Pid::from_u32(process_id);
        let mut system = System::new();
        system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        if system.process(pid).is_none() {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "fixture process did not exit"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
