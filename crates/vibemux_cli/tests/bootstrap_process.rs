#![cfg(feature = "test_helpers")]

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use sysinfo::{Pid, ProcessesToUpdate, System};
use vibemux_cli::{
    DEFAULT_STARTUP_TIMEOUT, DaemonBootstrapConfig, DaemonCliError, daemon_health,
    recovery::{RecoveryStatus, inspect_runtime, recover_runtime},
    start_daemon, stop_daemon,
};
use vibemuxd::process::DaemonPaths;

/// Real-time bound for the hang fixture's started marker. It covers the
/// production startup deadline plus fixture launch time. Only a failed launch
/// comes near it.
const FIXTURE_MARKER_TIMEOUT: Duration =
    DEFAULT_STARTUP_TIMEOUT.saturating_add(Duration::from_secs(30));
const FIXTURE_MARKER_POLL_INTERVAL: Duration = Duration::from_millis(5);

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

// The start uses the production timing on this runtime's paused clock. While
// a `spawn_blocking` task runs, a paused current_thread clock does not
// auto-advance (tokio test-util, tokio-rs/tokio#5115). So the startup deadline
// cannot expire before the fixture has written its started marker, however
// slowly the OS launches it; the clock then jumps to the deadline at no
// real-time cost. The former real-time 150 ms deadline raced the launch: on a
// loaded Windows host the marker appeared 50-590 ms after the fixture's PID
// was known. The default start timeout preserves the production writer-startup
// bound.
#[tokio::test(start_paused = true)]
async fn startup_timeout_terminates_only_the_spawned_fixture() {
    let temp = tempfile::tempdir().expect("temp project");
    let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
    #[cfg(windows)]
    let _control_cleanup = ControlRuntimeCleanup::new(&paths);
    let config = DaemonBootstrapConfig::new(
        paths.clone(),
        PathBuf::from(env!("CARGO_BIN_EXE_vibemux_hang_fixture")),
    );
    let started = paths.state_dir().join("hang_fixture_started");
    let completed = paths.state_dir().join("hang_fixture_completed");
    let started_wait = tokio::task::spawn_blocking({
        let started = started.clone();
        move || wait_for_started_marker(&started)
    });

    let start_error = start_daemon(&config)
        .await
        .expect_err("fixture must exceed startup deadline");
    // Back to real time, so the exit wait below is bounded on the wall clock.
    tokio::time::resume();
    assert!(
        started_wait.await.expect("started marker wait"),
        "fixture never wrote its started marker (start error: {start_error:?})"
    );
    let fixture_process_id = marker_process_id(&started).expect("fixture process id");
    let mut guard = FixtureProcessGuard::new(fixture_process_id);
    assert_eq!(start_error, DaemonCliError::StartupTimeout);
    // The fixture's `HANG_DURATION` far exceeds this bound, so its exit here
    // can only come from the timeout's termination; it can then never write
    // `completed`.
    wait_for_process_exit(fixture_process_id).await;
    guard.disarm();
    assert!(!completed.exists());
    assert!(!paths.descriptor_path().exists());
    assert!(!paths.writer_lock_path().exists());
}

#[tokio::test]
async fn early_daemon_exit_fails_fast_instead_of_timing_out() {
    let temp = tempfile::tempdir().expect("temp project");
    let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
    #[cfg(windows)]
    let _control_cleanup = ControlRuntimeCleanup::new(&paths);
    let config = DaemonBootstrapConfig::new(
        paths.clone(),
        PathBuf::from(env!("CARGO_BIN_EXE_vibemux_exit_fixture")),
    )
    .with_timing(
        Duration::from_secs(10),
        Duration::from_millis(10),
        Duration::from_millis(100),
    );

    // The fixture exits immediately without publishing artifacts, so the
    // start must fail fast with the classified cause. This also pins the
    // Windows-owned Child wait: the CLI must observe the exact process exit
    // before the startup deadline and terminate only a still-running child.
    assert_eq!(
        start_daemon(&config)
            .await
            .expect_err("fixture must fail the start"),
        DaemonCliError::StartFailed
    );
    assert!(paths.state_dir().join("exit_fixture_started").is_file());
    assert!(!paths.descriptor_path().exists());
    assert!(!paths.writer_lock_path().exists());
}

/// The CLI can exit only after authenticated health, and the daemon must keep
/// running after the launcher process that started it has returned.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_survives_cli_launcher_exit_until_outer_owner_stops_it() {
    let temp = tempfile::tempdir().expect("temp project");
    let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
    #[cfg(windows)]
    let _control_cleanup = ControlRuntimeCleanup::new(&paths);
    let launcher_executable = PathBuf::from(env!("CARGO_BIN_EXE_vibemuxctl"));
    let daemon_executable = PathBuf::from(env!("CARGO_BIN_EXE_vibemux_recovery_fixture"));
    assert!(launcher_executable.is_absolute());
    assert!(daemon_executable.is_absolute());
    let project_root = paths.project_root().to_path_buf();

    let launcher_status = tokio::task::spawn_blocking(move || {
        Command::new(launcher_executable)
            .args(["daemon", "start", "--project-root"])
            .arg(project_root)
            .arg("--daemon-executable")
            .arg(daemon_executable)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
    })
    .await
    .expect("join CLI launcher")
    .expect("run CLI launcher");

    // Capture the owned daemon identity before checking launcher status so
    // the guard still cleans it up if a later assertion fails.
    let health_result = daemon_health(&paths).await;
    let mut process_guard = health_result
        .as_ref()
        .ok()
        .map(|health| FixtureProcessGuard::new(health.process_id));
    assert!(
        launcher_status.success(),
        "CLI launcher exited unsuccessfully"
    );
    let health = health_result.expect("daemon health after CLI process exited");
    assert!(health.healthy);
    let process_id = health.process_id;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let health_after_delay = daemon_health(&paths)
        .await
        .expect("daemon remains healthy after launcher exit");
    assert_eq!(health_after_delay.process_id, process_id);

    stop_daemon(&paths).await.expect("outer owner stops daemon");
    wait_for_process_exit(process_id).await;
    if let Some(guard) = process_guard.as_mut() {
        guard.disarm();
    }
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
    // `stop` returns only once the daemon process has exited, so a `start`
    // that follows it cannot race the daemon's runtime-leaf release.
    assert!(
        !process_is_present(restarted_process_id),
        "stop returned before the daemon process exited"
    );
    guard.disarm();
    assert!(!paths.descriptor_path().exists());
    assert!(!paths.writer_lock_path().exists());
    // The clean `stop` also released the emptied per-project runtime leaf
    // before the daemon exited (on unix that directory is the state dir).
    #[cfg(windows)]
    assert!(!paths.runtime_dir().exists());
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

fn process_is_present(process_id: u32) -> bool {
    let pid = Pid::from_u32(process_id);
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    system.process(pid).is_some()
}

async fn wait_for_process_exit(process_id: u32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if !process_is_present(process_id) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "fixture process did not exit"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The process ID the hang fixture wrote into its started marker. The fixture
/// creates the file before writing, so an empty file reads as `None`.
fn marker_process_id(marker: &Path) -> Option<u32> {
    std::fs::read_to_string(marker).ok()?.trim().parse().ok()
}

/// Blocks on real time until the started marker holds a process ID. Returns
/// `false` after `FIXTURE_MARKER_TIMEOUT`.
fn wait_for_started_marker(marker: &Path) -> bool {
    let deadline = std::time::Instant::now() + FIXTURE_MARKER_TIMEOUT;
    while marker_process_id(marker).is_none() {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(FIXTURE_MARKER_POLL_INTERVAL);
    }
    true
}
