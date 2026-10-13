use std::{process::Stdio, time::Duration};

#[cfg(windows)]
use std::path::PathBuf;

use tokio::{
    process::{Child, Command},
    time::{Instant, sleep, timeout},
};
use vibemuxd::{
    control::{ControlClient, DaemonHealth},
    process::DaemonPaths,
};

#[cfg(windows)]
use vibemuxd::control::DaemonControlServer;

const PROCESS_TEST_DEADLINE: Duration = Duration::from_secs(10);
const HEALTH_ATTEMPT_DEADLINE: Duration = Duration::from_millis(500);
const POLL_INTERVAL: Duration = Duration::from_millis(20);

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn standalone_daemon_owns_rust_database_and_shuts_down_cleanly() {
    let temp = tempfile::tempdir().expect("temp project");
    let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
    #[cfg(windows)]
    let _control_cleanup = ControlRuntimeCleanup::new(&paths);
    paths.ensure_runtime_dir().expect("runtime directory");
    let python_sentinel = b"python-reference-database-sentinel";
    std::fs::write(paths.python_database_path(), python_sentinel).expect("python sentinel");

    let mut child = spawn_daemon(&paths);
    let expected_process_id = child.id().expect("child process id");

    let health = wait_for_health(&paths, &mut child).await;
    assert!(health.healthy);
    assert_eq!(health.process_id, expected_process_id);
    assert!(paths.database_path().is_file());
    assert!(paths.writer_lock_path().is_file());
    #[cfg(windows)]
    {
        vibemux_platform::verify_restricted_path_acl(paths.descriptor_path())
            .expect("descriptor ACL");
        vibemux_platform::verify_restricted_path_acl(paths.writer_lock_path())
            .expect("writer lock ACL");
    }
    if paths.has_distinct_legacy_runtime() {
        assert!(paths.legacy_writer_lock_path().is_file());
    }
    assert_eq!(
        std::fs::read(paths.python_database_path()).expect("read Python sentinel"),
        python_sentinel
    );

    ControlClient::from_descriptor(paths.descriptor_path())
        .expect("control client")
        .shutdown()
        .await
        .expect("shutdown response");
    let output = timeout(PROCESS_TEST_DEADLINE, child.wait_with_output())
        .await
        .expect("daemon exit deadline")
        .expect("daemon exit output");
    assert!(
        output.status.success(),
        "daemon failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!paths.descriptor_path().exists());
    assert!(!paths.writer_lock_path().exists());
    assert!(!paths.legacy_writer_lock_path().exists());
    // Windows: the emptied per-project leaf under the shared control root is
    // released by the clean shutdown. Unix: the "runtime dir" IS the
    // project state dir holding the database, so it must survive.
    #[cfg(windows)]
    assert!(!paths.runtime_dir().exists());
    #[cfg(unix)]
    assert!(paths.runtime_dir().is_dir());
    assert_eq!(
        std::fs::read(paths.python_database_path()).expect("read Python sentinel after shutdown"),
        python_sentinel
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abrupt_exit_leaves_owned_artifacts_for_fail_closed_diagnosis() {
    let temp = tempfile::tempdir().expect("temp project");
    let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
    #[cfg(windows)]
    let _control_cleanup = ControlRuntimeCleanup::new(&paths);
    let mut child = spawn_daemon(&paths);
    let health = wait_for_health(&paths, &mut child).await;
    assert_eq!(health.process_id, child.id().expect("child process id"));
    let descriptor_before =
        std::fs::read(paths.descriptor_path()).expect("read live descriptor bytes");
    let lock_before = std::fs::read(paths.writer_lock_path()).expect("read live writer lock");
    let legacy_lock_before = paths.has_distinct_legacy_runtime().then(|| {
        std::fs::read(paths.legacy_writer_lock_path()).expect("read live compatibility lock")
    });

    child.kill().await.expect("kill daemon abruptly");
    let status = child.wait().await.expect("reap killed daemon");
    assert!(!status.success());
    assert_eq!(
        std::fs::read(paths.descriptor_path()).expect("read stale descriptor"),
        descriptor_before
    );
    assert_eq!(
        std::fs::read(paths.writer_lock_path()).expect("read stale writer lock"),
        lock_before
    );
    if let Some(legacy_lock_before) = &legacy_lock_before {
        assert_eq!(
            std::fs::read(paths.legacy_writer_lock_path()).expect("read stale compatibility lock"),
            *legacy_lock_before
        );
    }
    let error = ControlClient::from_descriptor(paths.descriptor_path())
        .expect("stale descriptor remains structurally valid")
        .health_with_deadline(Duration::from_millis(100))
        .await
        .expect_err("stale endpoint must not be healthy");
    assert!(matches!(
        error,
        vibemuxd::control::ControlError::EndpointUnavailable
            | vibemuxd::control::ControlError::Deadline
            | vibemuxd::control::ControlError::InvalidFrame
    ));

    let output = Command::new(env!("CARGO_BIN_EXE_vibemuxd"))
        .arg("--project-root")
        .arg(paths.project_root())
        .output()
        .await
        .expect("run replacement daemon");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("control_descriptor_exists"));
    assert_eq!(
        std::fs::read(paths.descriptor_path()).expect("descriptor after refused restart"),
        descriptor_before
    );
    assert_eq!(
        std::fs::read(paths.writer_lock_path()).expect("lock after refused restart"),
        lock_before
    );
    if let Some(legacy_lock_before) = &legacy_lock_before {
        assert_eq!(
            std::fs::read(paths.legacy_writer_lock_path())
                .expect("compatibility lock after refused restart"),
            *legacy_lock_before
        );
    }
}

/// The trusted constructor re-verifies the whole control-runtime ACL
/// surface (ADR 020, issue #7): with a healthy secured runtime the start
/// succeeds and every artifact verifies while the daemon runs.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trusted_start_verifies_the_full_control_runtime_surface() {
    let temp = tempfile::tempdir().expect("temp project");
    let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
    let _control_cleanup = ControlRuntimeCleanup::new(&paths);
    paths.ensure_runtime_dir().expect("runtime directory");

    let server = DaemonControlServer::start_for_paths(&paths)
        .await
        .expect("trusted start must pass the ACL verification");
    paths
        .verify_control_security()
        .expect("full surface verifies while running");
    paths
        .verify_control_artifact_acls()
        .expect("artifact ACLs verify while running");
    assert!(paths.descriptor_path().is_file());
    assert!(paths.writer_lock_path().is_file());

    server.shutdown().await.expect("clean shutdown");
    assert!(!paths.descriptor_path().exists());
    assert!(!paths.writer_lock_path().exists());
    assert!(!paths.legacy_writer_lock_path().exists());
}

/// A clean shutdown releases the per-project runtime leaf once its owned
/// artifacts are gone, while the shared control root and its ACL marker stay;
/// the next start recreates the leaf. Before this was fixed, every clean
/// start/stop cycle leaked one empty leaf under the user's real
/// `%LOCALAPPDATA%\VibeMux\runtime`.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clean_shutdown_releases_the_empty_runtime_leaf() {
    let temp = tempfile::tempdir().expect("temp project");
    let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
    let _control_cleanup = ControlRuntimeCleanup::new(&paths);
    for cycle in 0..2 {
        paths.ensure_runtime_dir().expect("runtime directory");
        let server = DaemonControlServer::start_for_paths(&paths)
            .await
            .expect("trusted start");
        assert!(paths.runtime_dir().is_dir());
        server.shutdown().await.expect("clean shutdown");
        assert!(
            !paths.runtime_dir().exists(),
            "cycle {cycle}: the empty runtime leaf must be released"
        );
        assert!(paths.control_runtime_root().is_dir());
        assert!(paths.acl_marker_path().is_file());
        assert!(paths.database_path().is_file());
    }
}

/// The release is limited to an EMPTY leaf: content the daemon does not own
/// keeps the leaf, and that content, untouched.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clean_shutdown_keeps_a_runtime_leaf_with_foreign_content() {
    let temp = tempfile::tempdir().expect("temp project");
    let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
    let _control_cleanup = ControlRuntimeCleanup::new(&paths);
    paths.ensure_runtime_dir().expect("runtime directory");
    let foreign = paths.runtime_dir().join("foreign.txt");
    std::fs::write(&foreign, b"not owned by the daemon").expect("foreign file");

    let server = DaemonControlServer::start_for_paths(&paths)
        .await
        .expect("trusted start");
    server.shutdown().await.expect("clean shutdown");
    assert!(!paths.descriptor_path().exists());
    assert!(!paths.writer_lock_path().exists());
    assert!(paths.runtime_dir().is_dir());
    assert_eq!(
        std::fs::read(&foreign).expect("foreign file survives"),
        b"not owned by the daemon"
    );
}

/// The Codex P2 race on PR #17, made deterministic: a start that runs while
/// a stopping generation is between its descriptor removal and its leaf
/// release. The stopping generation still holds the compatibility lock
/// there, so the new start is refused with `writer_lock_held` and never
/// uses a leaf that is about to be removed. Once the release is done, the
/// refused start succeeds on retry. Before the fix the compatibility lock
/// was already gone in that window and the second start went ahead in the
/// leaf the first generation was about to validate and remove.
#[cfg(all(windows, feature = "test_helpers"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_start_inside_the_leaf_release_window_is_refused_while_the_leaf_is_owned() {
    use vibemuxd::control::test_hooks::pause_before_leaf_release;

    let temp = tempfile::tempdir().expect("temp project");
    let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
    let _control_cleanup = ControlRuntimeCleanup::new(&paths);
    paths.ensure_runtime_dir().expect("runtime directory");
    let first = DaemonControlServer::start_for_paths(&paths)
        .await
        .expect("first generation starts");

    let gate = pause_before_leaf_release(paths.runtime_dir());
    let stopping = tokio::spawn(first.shutdown());
    let (gate, reached) = tokio::task::spawn_blocking(move || {
        let reached = gate.wait_until_reached(PROCESS_TEST_DEADLINE);
        (gate, reached)
    })
    .await
    .expect("gate wait");
    assert!(reached, "the shutdown must reach the leaf release");
    assert!(!paths.descriptor_path().exists());
    assert!(!paths.writer_lock_path().exists());
    assert!(paths.runtime_dir().is_dir());

    // Exactly what the daemon binary does for a new start.
    paths
        .ensure_runtime_dir()
        .expect("runtime directory for the second start");
    let refused = DaemonControlServer::start_for_paths(&paths)
        .await
        .err()
        .expect("a start inside the release window must be refused");
    assert_eq!(refused.code(), "writer_lock_held");
    assert!(
        paths.legacy_writer_lock_path().is_file(),
        "the stopping generation owns the leaf until it is released"
    );
    assert!(!paths.descriptor_path().exists());
    assert!(!paths.writer_lock_path().exists());

    gate.proceed();
    stopping
        .await
        .expect("shutdown task")
        .expect("clean shutdown");
    assert!(!paths.runtime_dir().exists());
    assert!(!paths.legacy_writer_lock_path().exists());
    assert!(paths.acl_marker_path().is_file());

    paths
        .ensure_runtime_dir()
        .expect("runtime directory for retry");
    let second = DaemonControlServer::start_for_paths(&paths)
        .await
        .expect("the refused start succeeds on retry");
    assert!(paths.writer_lock_path().is_file());
    second.shutdown().await.expect("clean shutdown of retry");
    assert!(!paths.runtime_dir().exists());
    assert!(!paths.legacy_writer_lock_path().exists());
}

/// A start that finds the compatibility lock held fails with the existing
/// lock-held error before it touches the leaf: it neither creates a missing
/// leaf nor removes or writes into a present one, never opens the
/// database, and leaves the holder's lock file as it was.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_start_refused_by_a_held_compatibility_lock_never_touches_the_leaf() {
    let temp = tempfile::tempdir().expect("temp project");
    let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
    let _control_cleanup = ControlRuntimeCleanup::new(&paths);
    paths.ensure_runtime_dir().expect("runtime directory");
    std::fs::remove_dir(paths.runtime_dir()).expect("start without a leaf");
    let held = b"held by another generation\n";
    std::fs::write(paths.legacy_writer_lock_path(), held).expect("held compatibility lock");

    let error = DaemonControlServer::start_for_paths(&paths)
        .await
        .err()
        .expect("a held compatibility lock must refuse the start");
    assert_eq!(error.code(), "writer_lock_held");
    assert!(
        !paths.runtime_dir().exists(),
        "a start that does not own the compatibility lock must not create the leaf"
    );
    assert!(paths.control_runtime_root().is_dir());
    assert!(!paths.database_path().exists());
    assert_eq!(
        std::fs::read(paths.legacy_writer_lock_path()).expect("held lock survives"),
        held
    );

    // The daemon binary's `ensure_runtime_dir` may already have created the
    // leaf: the refused start keeps it, empty and untouched.
    paths.ensure_runtime_dir().expect("runtime directory");
    let error = DaemonControlServer::start_for_paths(&paths)
        .await
        .err()
        .expect("a held compatibility lock must refuse the start");
    assert_eq!(error.code(), "writer_lock_held");
    assert!(paths.runtime_dir().is_dir());
    assert!(
        std::fs::read_dir(paths.runtime_dir())
            .expect("read leaf")
            .next()
            .is_none()
    );
    assert!(!paths.database_path().exists());
    assert_eq!(
        std::fs::read(paths.legacy_writer_lock_path()).expect("held lock survives"),
        held
    );
}

/// The release is bound to the generation that owns the leaf: when the
/// compatibility lock no longer carries this generation's nonce (another
/// generation re-acquired it after it was removed externally), a clean
/// shutdown keeps the leaf and leaves that lock alone.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clean_shutdown_keeps_the_leaf_once_the_compatibility_lock_is_not_its_own() {
    let temp = tempfile::tempdir().expect("temp project");
    let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
    let _control_cleanup = ControlRuntimeCleanup::new(&paths);
    paths.ensure_runtime_dir().expect("runtime directory");
    let server = DaemonControlServer::start_for_paths(&paths)
        .await
        .expect("trusted start");
    let foreign = b"another generation's nonce\n";
    std::fs::write(paths.legacy_writer_lock_path(), foreign).expect("foreign compatibility lock");

    server.shutdown().await.expect("clean shutdown");
    assert!(!paths.descriptor_path().exists());
    assert!(!paths.writer_lock_path().exists());
    assert!(
        paths.runtime_dir().is_dir(),
        "a generation that no longer owns the leaf must not remove it"
    );
    assert_eq!(
        std::fs::read(paths.legacy_writer_lock_path()).expect("foreign lock survives"),
        foreign
    );
}

/// With the runtime leaf's ACL loosened, the trusted constructor fails
/// closed with `daemon_control_runtime_security_invalid`, leaves NO
/// runtime artifacts behind (descriptor removed by its guard, both
/// lifecycle locks removed by the joined writer), keeps the database, and
/// the standalone daemon binary surfaces the same code on stderr.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trusted_start_fails_closed_when_the_runtime_acl_is_loosened() {
    let temp = tempfile::tempdir().expect("temp project");
    let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
    let _control_cleanup = ControlRuntimeCleanup::new(&paths);
    paths.ensure_runtime_dir().expect("runtime directory");
    // Loosen the PROJECT-PRIVATE runtime leaf only (never the shared
    // control root, so sibling tests are unaffected); the extra ACE breaks
    // the exactly-three-rules invariant.
    vibemux_platform::test_helpers::loosen_with_icacls(
        paths.runtime_dir(),
        "*S-1-5-32-545:(OI)(CI)F",
    );

    let error = DaemonControlServer::start_for_paths(&paths)
        .await
        .err()
        .expect("loosened runtime ACL must fail closed");
    assert_eq!(error.code(), "daemon_control_runtime_security_invalid");
    // Fail-closed post-state: no descriptor, no lifecycle locks, database
    // kept (the writer opened it before verification and shutdown joins it).
    assert!(!paths.descriptor_path().exists());
    assert!(!paths.writer_lock_path().exists());
    assert!(!paths.legacy_writer_lock_path().exists());
    assert!(paths.database_path().is_file());
    // A failed start keeps the leaf: the CLI's post-mortem classification
    // re-verifies its ACL (`verify_control_surviving_acls`).
    assert!(paths.runtime_dir().is_dir());

    // A retry must fail identically - the failure is deterministic, not a
    // one-shot race.
    let retry = DaemonControlServer::start_for_paths(&paths)
        .await
        .err()
        .expect("retry must fail closed identically");
    assert_eq!(retry.code(), "daemon_control_runtime_security_invalid");
    assert!(!paths.descriptor_path().exists());
    assert!(!paths.writer_lock_path().exists());

    // End-to-end: the standalone daemon binary reports the same code on
    // stderr (mirrors the control_descriptor_exists assertion above).
    let output = Command::new(env!("CARGO_BIN_EXE_vibemuxd"))
        .arg("--project-root")
        .arg(paths.project_root())
        .output()
        .await
        .expect("run daemon against loosened runtime");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("daemon_control_runtime_security_invalid"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!paths.descriptor_path().exists());
    assert!(!paths.writer_lock_path().exists());
}

fn spawn_daemon(paths: &DaemonPaths) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_vibemuxd"));
    command
        .arg("--project-root")
        .arg(paths.project_root())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command.spawn().expect("spawn daemon process")
}

async fn wait_for_health(paths: &DaemonPaths, child: &mut Child) -> DaemonHealth {
    let deadline = Instant::now() + PROCESS_TEST_DEADLINE;
    loop {
        if paths.descriptor_path().is_file() {
            if let Ok(client) = ControlClient::from_descriptor(paths.descriptor_path()) {
                if let Ok(health) = client.health_with_deadline(HEALTH_ATTEMPT_DEADLINE).await {
                    return health;
                }
            }
        }
        if let Some(status) = child.try_wait().expect("query daemon status") {
            panic!("daemon exited before health: {status}");
        }
        assert!(Instant::now() < deadline, "daemon health deadline exceeded");
        sleep(POLL_INTERVAL).await;
    }
}
