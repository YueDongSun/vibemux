use super::*;

use std::{
    io,
    os::windows::ffi::OsStringExt,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
};
use tokio::{io::ReadBuf, sync::oneshot, time::timeout};
use vibemux_platform::TRAMPOLINE_EXIT_NOT_RELEASED;

const TEST_DEADLINE: Duration = Duration::from_secs(10);

fn command() -> CleanupGitCommand {
    CleanupGitCommand {
        git: "C:/trusted/git.exe".into(),
        trampoline: "C:/trusted/trampoline.exe".into(),
        repository: "C:/owned/project".into(),
        worktree: r"\\?\C:\owned\project\worktree Ω".into(),
    }
}

#[test]
fn git_profile_has_only_fixed_flags_and_one_literal_normal_windows_path() {
    let command = command();
    let plan = command_plan(&command).expect("fixed plan");
    assert_eq!(
        plan.spec.arguments,
        vec![
            "-c",
            "core.hooksPath=NUL",
            "-c",
            "core.autocrlf=false",
            "-c",
            "core.fsmonitor=false",
            "worktree",
            "remove",
            "--",
            r"C:\owned\project\worktree Ω",
        ]
    );
    assert_eq!(plan.executable, command.git);
    assert_eq!(plan.trampoline, command.trampoline);
    let environment = cleanup_environment();
    assert!(
        environment
            .iter()
            .all(|(name, _)| ENVIRONMENT_NAMES.contains(&name.as_str())
                || matches!(name.as_str(), "GIT_CONFIG_NOSYSTEM" | "GIT_CONFIG_GLOBAL"))
    );
    assert_eq!(
        environment
            .iter()
            .find(|(name, _)| name == "GIT_CONFIG_NOSYSTEM")
            .map(|(_, value)| value.as_os_str()),
        Some(OsStr::new("1"))
    );
    assert_eq!(
        environment
            .iter()
            .find(|(name, _)| name == "GIT_CONFIG_GLOBAL")
            .map(|(_, value)| value.as_os_str()),
        Some(OsStr::new("NUL"))
    );
}

#[test]
fn command_paths_are_bounded_unicode_native_paths_and_debug_is_redacted() {
    for path in [
        "relative.exe",
        "C:drive_relative.exe",
        r"\\server\share\git.exe",
        "C:/bad\npath.exe",
        "C:/tools/git.cmd",
    ] {
        let mut invalid = command();
        invalid.git = path.into();
        assert!(command_plan(&invalid).is_err());
    }
    let mut invalid = command();
    invalid.worktree = format!("C:/{}", "x".repeat(MAX_PATH_BYTES)).into();
    assert!(command_plan(&invalid).is_err());
    let mut invalid = command();
    invalid.repository = "C:/owned/../foreign".into();
    assert!(command_plan(&invalid).is_err());
    invalid = command();
    invalid.worktree = PathBuf::from(OsString::from_wide(&[67, 58, 92, 0xd800]));
    assert!(
        command_plan(&invalid).is_err(),
        "unpaired UTF-16 cannot become argv text"
    );
    assert_eq!(format!("{:?}", command()), "CleanupGitCommand(<redacted>)");
    let problem = CleanupRunProblem::Authorization("PRIVATE_CALLBACK_ERROR");
    assert!(!format!("{problem:?}").contains("PRIVATE"));
}

struct ReadFailure;
impl AsyncRead for ReadFailure {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Err(io::Error::other("PRIVATE_PIPE_ERROR")))
    }
}

#[tokio::test]
async fn stdout_read_failure_is_not_eof_and_no_raw_error_is_retained() {
    let (_sender, mut cancel) = watch::channel(false);
    let (eof, problem) = drain_stdout::<_, ()>(
        &mut ReadFailure,
        &mut cancel,
        Instant::now() + TEST_DEADLINE,
        MAX_STDOUT_BYTES,
    )
    .await;
    assert!(!eof);
    assert!(matches!(problem, Some(CleanupRunProblem::ReadFailed)));
    assert!(!format!("{problem:?}").contains("PRIVATE"));
}

fn fixture_plan(root: &Path, mode: &str, ready: Option<&Path>) -> LaunchPlan {
    let image = std::env::current_exe().expect("test image");
    let profile = image.parent().expect("deps").parent().expect("profile");
    let executable = profile.join("vibemux_native_fixture.exe");
    let trampoline = profile.join("vibemux_launch_trampoline.exe");
    assert!(
        executable.is_file() && trampoline.is_file(),
        "build fixed native fixture and trampoline first"
    );
    let mut arguments = vec!["--fixture_mode".into(), mode.into()];
    if matches!(mode, "oversized" | "stderr") {
        arguments.extend(["exec".into(), "--json".into()]);
    }
    if let Some(ready) = ready {
        arguments.extend([
            "--fixture_ready_path".into(),
            ready.to_str().expect("owned fixture path").into(),
        ]);
    }
    LaunchPlan {
        trampoline,
        executable: executable.clone(),
        working_directory: root.into(),
        spec: LaunchSpec {
            executable,
            arguments,
            environment_names: vec![],
        },
    }
}

fn limits() -> RunLimits {
    RunLimits {
        deadline: TEST_DEADLINE,
        max_stdout_bytes: MAX_STDOUT_BYTES,
    }
}

fn joined(receipt: &CleanupProcessReceipt) {
    assert!(receipt.termination_succeeded && receipt.root_reaped && receipt.tree_empty);
    assert!(receipt.stdout_drained && receipt.stderr_drained);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit cleanup process acceptance: build fixed fixture and trampoline"]
async fn denied_authorization_joins_without_go_or_starting_the_program() {
    let root = tempfile::tempdir().expect("owned root");
    let ready = root.path().join("never_started");
    for initially_cancelled in [false, true] {
        let (_sender, cancel) = watch::channel(initially_cancelled);
        let outcome = run_plan(
            fixture_plan(root.path(), "early_descendant", Some(&ready)),
            |_| async { Err("PRIVATE_AUTHORIZATION") },
            cancel,
            limits(),
        )
        .await;
        assert!(!format!("{outcome:?}").contains("PRIVATE"));
        let CleanupProcessOutcome::Finished { receipt, problem } = outcome else {
            panic!("joined denial")
        };
        assert!(matches!(
            problem,
            Some(CleanupRunProblem::Authorization("PRIVATE_AUTHORIZATION"))
        ));
        assert!(!receipt.release_attempted);
        assert_eq!(receipt.cancelled, initially_cancelled);
        assert_eq!(receipt.exit.exit_code, Some(TRAMPOLINE_EXIT_NOT_RELEASED));
        joined(&receipt);
        assert!(!ready.exists());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit cleanup process acceptance: build fixed fixture and trampoline"]
async fn cancellation_does_not_abort_pending_authorization() {
    let root = tempfile::tempdir().expect("owned root");
    let ready = root.path().join("never_started");
    let (sender, cancel) = watch::channel(false);
    let (entered_tx, entered_rx) = oneshot::channel();
    let (continue_tx, continue_rx) = oneshot::channel();
    let request = tokio::spawn(run_plan(
        fixture_plan(root.path(), "early_descendant", Some(&ready)),
        move |pid| async move {
            entered_tx.send(pid).expect("callback entered");
            continue_rx.await.expect("callback explicitly completed");
            Err::<(), _>("stopped")
        },
        cancel,
        limits(),
    ));
    assert_ne!(
        timeout(TEST_DEADLINE, entered_rx)
            .await
            .expect("entered deadline")
            .expect("entered PID"),
        0
    );
    sender.send(true).expect("cancel request");
    tokio::task::yield_now().await;
    assert!(!request.is_finished(), "authorization must remain awaited");
    continue_tx.send(()).expect("finish callback");
    let CleanupProcessOutcome::Finished { receipt, problem } = timeout(TEST_DEADLINE, request)
        .await
        .expect("joined deadline")
        .expect("task")
    else {
        panic!("joined denial")
    };
    assert!(matches!(
        problem,
        Some(CleanupRunProblem::Authorization("stopped"))
    ));
    assert!(receipt.cancelled);
    assert!(!receipt.release_attempted);
    joined(&receipt);
    assert!(!ready.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit cleanup process acceptance: build fixed fixture and trampoline"]
async fn released_echo_stderr_and_nonzero_exit_keep_exact_joined_receipts() {
    let root = tempfile::tempdir().expect("owned root");
    for (mode, code) in [("echo_stdin", 0), ("stderr", 0), ("exit_code", 7)] {
        let (_sender, cancel) = watch::channel(false);
        let count = Arc::new(AtomicUsize::new(0));
        let approvals = Arc::clone(&count);
        let outcome = run_plan(
            fixture_plan(root.path(), mode, None),
            move |_| async move {
                approvals.fetch_add(1, Ordering::AcqRel);
                Ok::<(), ()>(())
            },
            cancel,
            limits(),
        )
        .await;
        let CleanupProcessOutcome::Finished { receipt, problem } = outcome else {
            panic!("joined process")
        };
        assert!(problem.is_none());
        assert_eq!(receipt.exit.exit_code, Some(code));
        assert!(!receipt.exit.forced_termination);
        assert!(receipt.release_attempted && !receipt.cancelled && !receipt.timed_out);
        assert_eq!(count.load(Ordering::Acquire), 1);
        joined(&receipt);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit cleanup process acceptance: build fixed fixture and trampoline"]
async fn output_limit_and_timeout_force_join_and_observe_stdout_eof() {
    let root = tempfile::tempdir().expect("owned root");
    for mode in ["oversized", "silent_input"] {
        let (_sender, cancel) = watch::channel(false);
        let mut budget = limits();
        if mode == "silent_input" {
            budget.deadline = Duration::from_millis(150);
        }
        let outcome = run_plan(
            fixture_plan(root.path(), mode, None),
            |_| async { Ok::<(), ()>(()) },
            cancel,
            budget,
        )
        .await;
        let CleanupProcessOutcome::Finished { receipt, problem } = outcome else {
            panic!("joined failure")
        };
        if mode == "oversized" {
            assert!(matches!(problem, Some(CleanupRunProblem::OutputLimit)));
            assert!(!receipt.timed_out);
        } else {
            assert!(matches!(problem, Some(CleanupRunProblem::TimedOut)));
            assert!(receipt.timed_out);
        }
        assert!(receipt.release_attempted && receipt.exit.forced_termination);
        joined(&receipt);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit cleanup process acceptance: build fixed fixture and trampoline"]
async fn cancellation_joins_the_descendant_that_holds_stdout_open() {
    let root = tempfile::tempdir().expect("owned root");
    let ready = root.path().join("descendant_ready");
    let (sender, cancel) = watch::channel(false);
    let request = tokio::spawn(run_plan(
        fixture_plan(root.path(), "early_descendant", Some(&ready)),
        |_| async { Ok::<(), ()>(()) },
        cancel,
        limits(),
    ));
    timeout(TEST_DEADLINE, async {
        loop {
            if std::fs::read_to_string(&ready)
                .ok()
                .is_some_and(|value| value.parse::<u32>().is_ok_and(|pid| pid != 0))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("owned descendant started");
    sender.send(true).expect("cancel request");
    let CleanupProcessOutcome::Finished { receipt, problem } = timeout(TEST_DEADLINE, request)
        .await
        .expect("joined deadline")
        .expect("task")
    else {
        panic!("joined cancellation")
    };
    assert!(matches!(problem, Some(CleanupRunProblem::Cancelled)));
    assert!(receipt.release_attempted && receipt.cancelled);
    joined(&receipt);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit cleanup process acceptance: build fixed fixture and trampoline"]
async fn spawn_failure_has_no_joined_receipt_and_never_calls_authorization() {
    let root = tempfile::tempdir().expect("owned root");
    let mut plan = fixture_plan(root.path(), "echo_stdin", None);
    plan.trampoline = root.path().join("absent_trampoline.exe");
    let (_sender, cancel) = watch::channel(false);
    let count = Arc::new(AtomicUsize::new(0));
    let approvals = Arc::clone(&count);
    let outcome = run_plan(
        plan,
        move |_| async move {
            approvals.fetch_add(1, Ordering::AcqRel);
            Ok::<(), ()>(())
        },
        cancel,
        limits(),
    )
    .await;
    assert!(matches!(
        outcome,
        CleanupProcessOutcome::<()>::SpawnFailed(DispatchError::SpawnFailed)
    ));
    assert_eq!(count.load(Ordering::Acquire), 0);
}
