//! Driver behaviour against a stand-in SDK runtime.
//!
//! Every test drives `vibemux_dsh_fixture`, which speaks the wire protocol and
//! nothing else, so no test needs the vendor runtime, a credential, or model
//! inference.

use std::path::PathBuf;

use vibemux_dsh::{DshDriver, DshDriverError, DshLaunch, DshPoll};
use vibemux_harness::dsh::{
    DshPhase, DshRecordKind, DshSessionLimits, DshSessionSettings, DshTurnOutcome,
};

/// The fixture binary built for this crate's tests.
fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_vibemux_dsh_fixture"))
}

fn launch(mode: &str) -> DshLaunch {
    DshLaunch {
        executable: fixture(),
        arguments: vec![mode.to_string()],
        // Nothing is inherited: the fixture needs no environment at all.
        environment_names: Vec::new(),
        // The workspace root is always available to the test process; the
        // platform temporary directory is not writable in every sandbox.
        working_directory: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
    }
}

fn settings() -> DshSessionSettings {
    DshSessionSettings {
        session_id: "vibemux_test_session".to_string(),
        working_directory: "C:\\project".to_string(),
        provider: "deepseek-official".to_string(),
        model: "deepseek-v4-flash".to_string(),
        reasoning_effort: None,
        max_tokens: None,
    }
}

/// Keep turn and shutdown failures fast while allowing fixture startup time.
fn limits() -> DshSessionLimits {
    DshSessionLimits {
        // Startup is setup for these behavior tests, not a one-second latency
        // assertion. Concurrent process starts can delay the first fixture
        // reply even when the executable was already built and run.
        initialize_deadline_ms: 5_000,
        turn_deadline_ms: 2_000,
        shutdown_grace_ms: 1_000,
        ..DshSessionLimits::default()
    }
}

async fn open(mode: &str) -> DshDriver {
    open_with_limits(mode, limits()).await
}

async fn open_with_limits(mode: &str, limits: DshSessionLimits) -> DshDriver {
    let mut driver = DshDriver::spawn(&launch(mode), settings(), limits)
        .await
        .expect("spawn");
    driver.await_ready().await.expect("handshake");
    driver
}

/// Like the production UI/control consumers, observe only poll/status after
/// delivery. The separate test deadline bounds a broken driver, not its turn.
async fn poll_until_settled(driver: &DshDriver) -> DshPoll {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let page = driver.poll(0).expect("poll");
        if page.summary.phase != DshPhase::Prompting {
            return page;
        }
        assert!(tokio::time::Instant::now() < deadline, "turn never settled");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

async fn open_with_descendant(mode: &str) -> (DshDriver, tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().expect("owned fixture directory");
    let mut launch = launch(mode);
    launch.working_directory = directory
        .path()
        .canonicalize()
        .expect("canonical directory");
    let mut driver = DshDriver::spawn(&launch, settings(), limits())
        .await
        .expect("spawn");
    driver.await_ready().await.expect("handshake");
    let heartbeat = directory.path().join("dsh-descendant-heartbeat");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while !heartbeat.is_file() {
        assert!(tokio::time::Instant::now() < deadline, "descendant started");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    (driver, directory, heartbeat)
}

async fn assert_heartbeat_stopped(heartbeat: &std::path::Path) {
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let first = std::fs::read_to_string(heartbeat).expect("heartbeat");
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(
        std::fs::read_to_string(heartbeat).expect("heartbeat"),
        first,
        "a closed session must not leave an executing descendant"
    );
}

#[tokio::test]
async fn graceful_close_terminates_descendants_before_returning_and_is_idempotent() {
    let (mut driver, _directory, heartbeat) = open_with_descendant("descendant").await;
    driver.close().await.expect("close");
    assert_eq!(driver.phase(), DshPhase::Closed);
    // Keep the driver alive: Drop must not be what stops the descendant.
    assert_heartbeat_stopped(&heartbeat).await;
    driver.close().await.expect("second close");
}

#[tokio::test]
async fn shutdown_timeout_still_terminates_descendants_before_returning() {
    let (mut driver, _directory, heartbeat) = open_with_descendant("descendant-hang").await;
    assert_eq!(driver.close().await, Err(DshDriverError::ShutdownTimeout));
    assert_heartbeat_stopped(&heartbeat).await;
    driver.close().await.expect("second close");
}

#[tokio::test]
async fn cancellation_terminates_descendants_before_close_returns() {
    let (mut driver, _directory, heartbeat) = open_with_descendant("descendant").await;
    driver.cancel().await.expect("cancel");
    driver.close().await.expect("close");
    assert_heartbeat_stopped(&heartbeat).await;
}

#[tokio::test]
async fn a_runtime_that_stops_reading_cannot_block_prompt_or_cleanup_forever() {
    let mut driver = open("no-read").await;
    // JSON escaping expands this valid 32 KiB prompt beyond a pipe buffer.
    // This exercises the shared driver used by the terminal, not the smaller
    // daemon IPC frame limit.
    let prompt = "\u{0001}".repeat(32 * 1024);
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), driver.prompt(&prompt))
        .await
        .expect("bounded prompt write");
    assert_eq!(result, Err(DshDriverError::TurnTimeout));
    assert!(!driver.transport_open());
    assert_eq!(driver.phase(), DshPhase::Failed);
    driver.close().await.expect("cleanup after partial frame");
}

#[tokio::test]
async fn a_full_turn_completes_and_exposes_its_records() {
    let mut driver = open("serve").await;
    assert_eq!(driver.phase(), DshPhase::Ready);
    assert!(driver.transport_open());

    let handshake = driver.poll(0).expect("poll");
    assert!(
        handshake
            .records
            .iter()
            .any(|record| record.kind == DshRecordKind::Handshake),
        "the handshake result must be captured: {:?}",
        handshake.records
    );

    driver.prompt("do the thing").await.expect("prompt");
    assert!(matches!(
        driver.phase(),
        DshPhase::Prompting | DshPhase::Ready
    ));
    driver.await_turn().await.expect("turn settles");
    assert_eq!(driver.phase(), DshPhase::Ready);

    let poll = driver.poll(handshake.next).expect("poll");
    let kinds: Vec<DshRecordKind> = poll.records.iter().map(|record| record.kind).collect();
    assert!(kinds.contains(&DshRecordKind::TurnStart), "{kinds:?}");
    assert!(kinds.contains(&DshRecordKind::Text), "{kinds:?}");
    assert!(kinds.contains(&DshRecordKind::Status), "{kinds:?}");
    assert!(kinds.contains(&DshRecordKind::TurnEnd), "{kinds:?}");
    // Byte-exact runtime content, available only through the poll.
    assert!(
        poll.records
            .iter()
            .any(|record| record.raw_json.contains("fixture reply"))
    );
    assert_eq!(poll.summary.completed_turns, 1);
    assert_eq!(poll.summary.last_outcome, Some(DshTurnOutcome::Completed));
    assert!(!poll.truncated);
    // The page and its summary are one snapshot: nothing more to read means
    // the summary counts exactly the records this page reached.
    assert!(!poll.more);
    assert_eq!(poll.next, poll.summary.records);
    assert!(poll.transport_open);

    driver.close().await.expect("close");
    assert_eq!(driver.phase(), DshPhase::Closed);
}

#[tokio::test]
async fn a_second_turn_reuses_the_same_session() {
    let mut driver = open("serve").await;
    for ordinal in 1..=2 {
        driver.prompt("again").await.expect("prompt");
        driver.await_turn().await.expect("turn settles");
        let summary = driver.poll(0).expect("poll").summary;
        assert_eq!(summary.turns_started, ordinal);
        assert_eq!(summary.completed_turns, ordinal);
        assert_eq!(summary.current_turn.expect("current turn").ordinal, ordinal);
    }
    driver.close().await.expect("close");
}

#[tokio::test]
async fn a_handshake_that_never_answers_hits_its_deadline() {
    // This test explicitly exercises the short deadline, unlike the startup
    // needed to reach the behavior tested by the other sessions.
    let timeout_limits = DshSessionLimits {
        initialize_deadline_ms: 1_000,
        ..limits()
    };
    let mut driver = DshDriver::spawn(&launch("hang"), settings(), timeout_limits)
        .await
        .expect("spawn");
    assert_eq!(
        driver.await_ready().await,
        Err(DshDriverError::InitializeTimeout)
    );
    assert_eq!(driver.phase(), DshPhase::Failed);
    let summary = driver.poll(0).expect("poll").summary;
    assert_eq!(summary.error_code, Some("dsh_unexpected_exit"));
    // Closing a failed session still reaps the child.
    driver.close().await.expect("close");
}

#[tokio::test]
async fn a_runtime_that_exits_early_is_reported_as_a_transport_failure() {
    let directory = tempfile::tempdir().expect("owned fixture directory");
    let mut launch = launch("exit-early");
    launch.working_directory = directory.path().to_path_buf();
    let mut driver = DshDriver::spawn(&launch, settings(), limits())
        .await
        .expect("spawn");
    let error = driver.await_ready().await.expect_err("closed transport");
    let page = driver.poll(0).expect("poll");
    let cleanup = driver.close().await;
    assert_eq!(error, DshDriverError::TransportClosed);
    assert_eq!(page.summary.phase, DshPhase::Failed);
    assert_eq!(page.summary.error_code, Some("dsh_unexpected_exit"));
    assert!(!page.transport_open);
    assert!(!driver.transport_open());
    cleanup.expect("verified cleanup after early EOF");
    driver.close().await.expect("idempotent close");
}

/// The valid handshake and an immediate exit may both be consumed before a
/// waiter runs. Observe through poll/status, not an await_ready race that
/// assumes the runtime is still alive after its handshake.
async fn open_until_exited(mode: &str) -> DshDriver {
    let driver = DshDriver::spawn(&launch(mode), settings(), limits())
        .await
        .expect("spawn");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while driver.transport_open() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(!driver.transport_open(), "the runtime should have exited");
    assert_eq!(driver.summary().server_version.as_deref(), Some("0.0.1"));
    assert_eq!(driver.phase(), DshPhase::Failed);
    driver
}

#[tokio::test]
async fn a_successful_exit_without_shutdown_stays_failed_after_close() {
    let mut driver = open_until_exited("exit-after-ready").await;
    // Exit status zero is not a cooperative shutdown. Once EOF failed the
    // ready session, cleanup must not rewrite that failure as a clean close.
    driver.close().await.expect("close");
    assert_eq!(driver.phase(), DshPhase::Failed);
    assert_eq!(
        driver.poll(0).expect("poll").summary.error_code,
        Some("dsh_unexpected_exit")
    );
}

#[tokio::test]
async fn a_runtime_that_crashed_before_close_fails_the_session() {
    let mut driver = open_until_exited("crash-after-ready").await;
    // The child is reaped, but a nonzero exit without a `shutdown` answer is
    // an unexpected exit, never a clean close.
    driver.close().await.expect("close");
    assert_eq!(driver.phase(), DshPhase::Failed);
    assert_eq!(
        driver.poll(0).expect("poll").summary.error_code,
        Some("dsh_unexpected_exit")
    );
}

#[tokio::test]
async fn a_closed_session_never_reports_an_open_transport() {
    let mut driver = open("serve").await;
    driver.close().await.expect("close");
    assert_eq!(driver.phase(), DshPhase::Closed);
    assert!(!driver.transport_open());
    assert!(!driver.poll(0).expect("poll").transport_open);
}

#[tokio::test]
async fn a_structured_turn_error_is_failed_not_completed() {
    let mut driver = open("error-turn").await;
    driver.prompt("fail please").await.expect("prompt");
    driver.await_turn().await.expect("turn settles");
    let summary = driver.poll(0).expect("poll").summary;
    assert_eq!(summary.last_outcome, Some(DshTurnOutcome::Failed));
    assert_eq!(summary.failed_turns, 1);
    assert_eq!(summary.completed_turns, 0);
    driver.close().await.expect("close");
}

#[tokio::test]
async fn a_turn_deadline_is_enforced_without_await_turn() {
    let mut driver = DshDriver::spawn(
        &launch("no-turn-end"),
        settings(),
        DshSessionLimits {
            turn_deadline_ms: 1_000,
            ..limits()
        },
    )
    .await
    .expect("spawn");
    driver.await_ready().await.expect("handshake");
    driver
        .prompt("stall without a waiter")
        .await
        .expect("prompt");
    // Production consumers only use status/poll. No await_turn or even a
    // status read drives the session while its whole-turn budget expires.
    tokio::time::sleep(std::time::Duration::from_millis(1_500)).await;
    let summary = driver.summary();
    let page = driver.poll(0).expect("poll");
    let error = driver.transport_error();
    driver.close().await.expect("cleanup");

    assert_eq!(summary.phase, DshPhase::Failed);
    assert_eq!(page.summary, summary);
    assert_eq!(summary.completed_turns, 0);
    assert_eq!(summary.unverified_turns, 1);
    assert_eq!(summary.last_outcome, Some(DshTurnOutcome::Unverified));
    assert_eq!(summary.error_code, Some("dsh_unexpected_exit"));
    assert_eq!(error, Some(DshDriverError::TurnTimeout));
    assert!(!page.transport_open);
}

#[tokio::test]
async fn abandoning_a_blocked_prompt_does_not_disarm_its_turn_deadline() {
    let mut driver = open_with_limits(
        "no-read",
        DshSessionLimits {
            turn_deadline_ms: 1_000,
            ..limits()
        },
    )
    .await;
    let prompt = "\u{0001}".repeat(32 * 1024);
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            driver.prompt(&prompt)
        )
        .await
        .is_err(),
        "the fixture must block the prompt write"
    );
    assert_eq!(driver.phase(), DshPhase::Prompting);
    // No prompt future or await_turn remains to enforce the budget. Leave
    // status and poll idle as well, exactly as a disconnected consumer does.
    tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;
    let page = driver.poll(0).expect("poll");
    let error = driver.transport_error();
    driver.close().await.expect("cleanup");
    assert_eq!(page.summary.phase, DshPhase::Failed);
    assert_eq!(page.summary.unverified_turns, 1);
    assert_eq!(page.summary.completed_turns, 0);
    assert_eq!(page.summary.last_outcome, Some(DshTurnOutcome::Unverified));
    assert!(!page.transport_open);
    assert_eq!(error, Some(DshDriverError::TurnTimeout));
}

#[tokio::test]
async fn a_crash_after_ready_fails_status_without_await_turn() {
    let mut driver = DshDriver::spawn(&launch("crash-after-ready"), settings(), limits())
        .await
        .expect("spawn");
    // The fixture writes its valid handshake before exiting. Do not wait on
    // await_ready either: the reader must report the exit to the machine.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while driver.transport_open() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let page = driver.poll(0).expect("poll");
    let summary = driver.summary();
    let error = driver.transport_error();
    driver.close().await.expect("cleanup");

    assert!(!page.transport_open, "the fixture must have exited");
    assert!(
        page.records
            .iter()
            .any(|record| record.kind == DshRecordKind::Handshake)
    );
    assert_eq!(summary.phase, DshPhase::Failed);
    assert_eq!(page.summary, summary);
    assert_eq!(summary.error_code, Some("dsh_unexpected_exit"));
    assert_eq!(summary.completed_turns, 0);
    assert_eq!(error, Some(DshDriverError::TransportClosed));
}

#[tokio::test]
async fn successful_poll_only_turns_do_not_inherit_an_earlier_turns_deadline() {
    let mut driver = open("delayed-turn-end").await;
    for ordinal in 1..=2 {
        driver.prompt("another bounded turn").await.expect("prompt");
        let page = poll_until_settled(&driver).await;
        assert_eq!(page.summary.phase, DshPhase::Ready);
        assert_eq!(page.summary.turns_started, ordinal);
        assert_eq!(page.summary.completed_turns, ordinal);
        assert_eq!(page.summary.unverified_turns, 0);
        assert_eq!(page.summary.last_outcome, Some(DshTurnOutcome::Completed));
    }
    // Each turn lasts 1.2s against a 2s budget, so the first deadline elapses
    // during the second turn. Let the second elapse while idle as well.
    tokio::time::sleep(std::time::Duration::from_millis(2_100)).await;
    assert_eq!(driver.phase(), DshPhase::Ready);
    assert!(driver.transport_open());
    assert_eq!(driver.transport_error(), None);
    driver.close().await.expect("close");
}

#[tokio::test]
async fn awaiting_late_or_again_does_not_restart_the_turn_budget() {
    let mut driver = open_with_limits(
        "no-turn-end",
        DshSessionLimits {
            turn_deadline_ms: 1_000,
            ..limits()
        },
    )
    .await;
    driver.prompt("one budget").await.expect("prompt");
    // Dropping a wait future is not cancellation of the driver's timer.
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), driver.await_turn())
            .await
            .is_err()
    );
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    let result = tokio::time::timeout(std::time::Duration::from_millis(600), driver.await_turn())
        .await
        .expect("the original deadline must end this wait");
    assert_eq!(result, Err(DshDriverError::TurnTimeout));
    assert_eq!(driver.await_turn().await, Err(DshDriverError::TurnTimeout));
    assert_eq!(driver.summary().unverified_turns, 1);
    driver.close().await.expect("close");
}

#[tokio::test]
async fn a_turn_wait_without_an_admitted_turn_is_refused_instead_of_unbounded() {
    let mut starting = DshDriver::spawn(&launch("hang"), settings(), limits())
        .await
        .expect("spawn");
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_millis(250), starting.await_turn())
            .await
            .expect("no admitted turn"),
        Err(DshDriverError::InvalidState)
    );
    let _ = starting.close().await;

    let mut closing = open("no-turn-end").await;
    closing.prompt("abandon this turn").await.expect("prompt");
    closing.cancel().await.expect("cancel");
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_millis(250), closing.await_turn())
            .await
            .expect("the admitted turn was abandoned"),
        Err(DshDriverError::InvalidState)
    );
    closing.close().await.expect("close");
    assert_eq!(closing.summary().cancelled_turns, 1);
    assert_eq!(closing.summary().unverified_turns, 0);
}

#[tokio::test]
async fn completion_after_the_budget_is_not_success_for_a_poll_only_consumer() {
    let mut driver = open_with_limits(
        "delayed-turn-end",
        DshSessionLimits {
            turn_deadline_ms: 1_000,
            ..limits()
        },
    )
    .await;
    driver.prompt("too late").await.expect("prompt");
    tokio::time::sleep(std::time::Duration::from_millis(1_500)).await;
    let page = driver.poll(0).expect("poll");
    assert_eq!(page.summary.phase, DshPhase::Failed);
    assert_eq!(page.summary.completed_turns, 0);
    assert_eq!(page.summary.unverified_turns, 1);
    assert_eq!(page.summary.last_outcome, Some(DshTurnOutcome::Unverified));
    assert_eq!(driver.transport_error(), Some(DshDriverError::TurnTimeout));
    driver.close().await.expect("close");
}

#[tokio::test]
async fn a_crash_during_a_turn_is_unverified_for_a_poll_only_consumer() {
    let mut driver = open("crash-during-turn").await;
    driver.prompt("observe the crash").await.expect("prompt");
    let page = poll_until_settled(&driver).await;
    assert_eq!(page.summary.phase, DshPhase::Failed);
    assert_eq!(page.summary.unverified_turns, 1);
    assert_eq!(page.summary.completed_turns, 0);
    assert_eq!(page.summary.last_outcome, Some(DshTurnOutcome::Unverified));
    assert_eq!(page.summary.error_code, Some("dsh_unexpected_exit"));
    assert_eq!(
        driver.transport_error(),
        Some(DshDriverError::TransportClosed)
    );
    assert!(!page.transport_open);
    driver.close().await.expect("close");
}

#[tokio::test]
async fn a_silent_turn_deadline_terminates_descendants_without_a_close_call() {
    let (mut driver, _directory, heartbeat) = open_with_descendant("descendant-no-turn-end").await;
    driver
        .prompt("expire the whole tree")
        .await
        .expect("prompt");
    let page = poll_until_settled(&driver).await;
    assert_eq!(page.summary.phase, DshPhase::Failed);
    assert_eq!(driver.transport_error(), Some(DshDriverError::TurnTimeout));
    assert_eq!(page.summary.unverified_turns, 1);
    // Neither close nor Drop has run: the lifecycle task must do the kill.
    assert_heartbeat_stopped(&heartbeat).await;
    driver.close().await.expect("close");
}

#[tokio::test]
async fn process_death_is_detected_even_when_a_descendant_holds_stdout_open() {
    let (mut driver, _directory, heartbeat) = open_with_descendant("crash-with-descendant").await;
    driver.prompt("observe the leader").await.expect("prompt");
    let page = poll_until_settled(&driver).await;
    assert_eq!(page.summary.phase, DshPhase::Failed);
    // A whole-turn timeout would eventually fail as well, but is not evidence
    // that process death itself was noticed.
    assert_eq!(
        driver.transport_error(),
        Some(DshDriverError::TransportClosed)
    );
    assert_eq!(page.summary.unverified_turns, 1);
    assert_heartbeat_stopped(&heartbeat).await;
    driver.close().await.expect("close");
}

#[tokio::test]
async fn close_disarms_the_turn_deadline_while_late_frames_drain() {
    let mut driver = open_with_limits(
        "delayed-turn-end",
        DshSessionLimits {
            turn_deadline_ms: 1_000,
            shutdown_grace_ms: 3_000,
            ..limits()
        },
    )
    .await;
    driver
        .prompt("close before completion")
        .await
        .expect("prompt");
    // The fixture takes 1.2s to finish its turn before it reads shutdown.
    // Closing owns that interval, not the abandoned turn's one-second timer.
    driver.close().await.expect("cooperative close");
    let summary = driver.summary();
    assert_eq!(summary.phase, DshPhase::Closed);
    assert_eq!(summary.cancelled_turns, 1);
    assert_eq!(summary.completed_turns, 0);
    assert_eq!(summary.unverified_turns, 0);
    assert_eq!(summary.error_code, None);
    assert_eq!(driver.transport_error(), None);
}

#[tokio::test]
async fn cancellation_disarms_the_timer_before_a_later_close() {
    let mut driver = open_with_limits(
        "no-turn-end",
        DshSessionLimits {
            turn_deadline_ms: 1_000,
            ..limits()
        },
    )
    .await;
    driver
        .prompt("cancel before the deadline")
        .await
        .expect("prompt");
    driver.cancel().await.expect("cancel");
    tokio::time::sleep(std::time::Duration::from_millis(1_500)).await;
    let summary = driver.summary();
    assert_eq!(summary.phase, DshPhase::Closing);
    assert_eq!(summary.cancelled_turns, 1);
    assert_eq!(summary.unverified_turns, 0);
    assert_eq!(driver.transport_error(), None);
    driver.close().await.expect("close");
    assert_eq!(driver.phase(), DshPhase::Closed);
}

#[tokio::test]
async fn eof_during_shutdown_is_judged_by_the_owned_exit_status() {
    for (mode, expected_phase, expected_error) in [
        ("shutdown-eof", DshPhase::Closed, None),
        (
            "shutdown-crash",
            DshPhase::Failed,
            Some("dsh_unexpected_exit"),
        ),
    ] {
        let mut driver = open(mode).await;
        driver.close().await.expect("cleanup");
        let summary = driver.summary();
        assert_eq!(summary.phase, expected_phase, "{mode}");
        assert_eq!(summary.error_code, expected_error, "{mode}");
        assert!(!driver.transport_open());
        driver.close().await.expect("idempotent close");
    }
}

#[tokio::test]
async fn idle_status_alone_never_completes_a_turn() {
    let mut driver = open("no-turn-end").await;
    driver.prompt("stall").await.expect("prompt");
    // The fixture emits `session.status: idle` and then nothing. The turn is
    // still running, so the deadline is the only thing that ends it.
    assert_eq!(driver.await_turn().await, Err(DshDriverError::TurnTimeout));
    let summary = driver.poll(0).expect("poll").summary;
    assert_eq!(summary.completed_turns, 0);
    assert_eq!(summary.unverified_turns, 1);
    let _ = driver.close().await;
}

#[tokio::test]
async fn another_sessions_turn_end_is_counted_and_ignored() {
    let mut driver = open("foreign").await;
    driver.prompt("mine").await.expect("prompt");
    driver.await_turn().await.expect("turn settles");
    let summary = driver.poll(0).expect("poll").summary;
    assert_eq!(summary.foreign_notifications, 1);
    assert_eq!(summary.completed_turns, 1);
    driver.close().await.expect("close");
}

#[tokio::test]
async fn a_non_json_stdout_line_ends_the_session() {
    let mut driver = open("garbage").await;
    driver.prompt("break the channel").await.expect("prompt");
    let error = driver.await_turn().await.expect_err("violation");
    assert_eq!(error, DshDriverError::InvalidState);
    assert_eq!(driver.phase(), DshPhase::Failed);
    let _ = driver.close().await;
}

#[tokio::test]
async fn cancellation_terminates_the_runtime() {
    let mut driver = open("no-turn-end").await;
    driver.prompt("cancel me").await.expect("prompt");
    // Cancel a turn that is running, after the prompt's response arrived.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while driver
        .summary()
        .current_turn
        .and_then(|turn| turn.turn)
        .is_none()
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    driver.cancel().await.expect("cancel");
    let summary = driver.poll(0).expect("poll").summary;
    assert_eq!(summary.cancelled_turns, 1);
    // A cancelled session still closes cleanly enough to reap the child, and
    // the exit the driver caused by terminating the tree is not a crash.
    let _ = driver.close().await;
    assert_eq!(driver.phase(), DshPhase::Closed);
    assert_eq!(driver.poll(0).expect("poll").summary.error_code, None);
}

#[tokio::test]
async fn cancelling_immediately_after_prompt_still_closes_cleanly() {
    let mut driver = open("late_prompt").await;
    driver.prompt("cancel at once").await.expect("prompt");
    // This fixture cannot answer until shutdown, so cancellation always
    // precedes the receipt. The pure-machine test injects a buffered reply.
    driver.cancel().await.expect("cancel");
    driver.close().await.expect("close");
    let summary = driver.poll(0).expect("poll").summary;
    assert_eq!(
        (driver.phase(), summary.error_code),
        (DshPhase::Closed, None)
    );
    assert_eq!(summary.cancelled_turns, 1);
    assert_eq!(summary.completed_turns, 0);
    assert!(!driver.transport_open());
}

#[tokio::test]
async fn closing_before_the_prompt_is_answered_still_closes_cleanly() {
    let mut driver = open("late_prompt").await;
    driver.prompt("close at once").await.expect("prompt");
    // The fixture withholds every prompt frame until it reads shutdown.
    // Closing may stop the reader before all buffered frames are consumed;
    // the pure-machine regression verifies their acceptance and exact count.
    driver.close().await.expect("close");
    let summary = driver.poll(0).expect("poll").summary;
    assert_eq!(
        (driver.phase(), summary.error_code),
        (DshPhase::Closed, None)
    );
    assert_eq!(summary.cancelled_turns, 1);
    assert_eq!(summary.completed_turns, 0);
    assert!(!driver.transport_open());
}

/// Sessions the shutdown-first test opens. Whether the reader consumes the
/// late frames before `close` stops it varies by run, so several rounds make
/// a regression surface reliably.
const RACE_ROUNDS: usize = 8;

#[tokio::test]
async fn a_prompt_answered_after_the_shutdown_reply_still_closes_cleanly() {
    for round in 0..RACE_ROUNDS {
        // The fixture answers `shutdown` first, then the prompt it held and
        // that prompt's whole turn, so they reach a session already closed.
        let mut driver = open("shutdown-first").await;
        driver.prompt("answer me last").await.expect("prompt");
        driver.close().await.expect("close");
        let summary = driver.poll(0).expect("poll").summary;
        assert_eq!(
            (driver.phase(), summary.error_code),
            (DshPhase::Closed, None),
            "round {round}"
        );
        // A late frame the closed session refused would surface here, and
        // the terminal UI would report the transport as ended in error.
        assert_eq!(driver.transport_error(), None, "round {round}");
        assert_eq!(summary.cancelled_turns, 1, "round {round}");
    }
}

#[tokio::test]
async fn polling_advances_the_cursor_and_a_stale_cursor_reports_a_gap() {
    let mut driver = open("serve").await;
    driver.prompt("one").await.expect("prompt");
    driver.await_turn().await.expect("turn settles");

    let first = driver.poll(0).expect("first page");
    assert!(first.next > 0);
    assert!(!first.more);
    let second = driver.poll(first.next).expect("settled page");
    assert!(second.records.is_empty());
    assert_eq!(second.next, first.next);

    // A cursor ahead of the head is refused rather than silently clamped.
    assert_eq!(
        driver.poll(first.next + 1).err(),
        Some(DshDriverError::InvalidState)
    );
    driver.close().await.expect("close");
}

#[tokio::test]
async fn the_poll_summary_carries_no_prompt_text() {
    let mut driver = open("serve").await;
    driver
        .prompt("a prompt that must never appear in a summary")
        .await
        .expect("prompt");
    driver.await_turn().await.expect("turn settles");
    let poll: DshPoll = driver.poll(0).expect("poll");
    let summary = serde_json::to_string(&poll.summary).expect("summary json");
    assert!(!summary.contains("must never appear"));
    // The prompt is sized and hashed, never quoted.
    assert!(summary.contains("\"prompt_bytes\":44"));
    let _ = driver.close().await;
}

#[tokio::test]
async fn a_prompt_is_refused_before_the_handshake_settles() {
    // `hang` never answers, so the session is still initializing; a prompt
    // must be refused rather than written early.
    let mut driver = DshDriver::spawn(&launch("hang"), settings(), limits())
        .await
        .expect("spawn");
    assert_eq!(
        driver.prompt("too early").await.err(),
        Some(DshDriverError::InvalidState)
    );
    assert_eq!(driver.phase(), DshPhase::Initializing);
    let _ = driver.close().await;
}
