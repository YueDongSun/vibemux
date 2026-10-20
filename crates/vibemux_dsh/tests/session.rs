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

/// Short deadlines keep the bounded-failure tests fast.
fn limits() -> DshSessionLimits {
    DshSessionLimits {
        initialize_deadline_ms: 1_000,
        turn_deadline_ms: 2_000,
        shutdown_grace_ms: 1_000,
        ..DshSessionLimits::default()
    }
}

async fn open(mode: &str) -> DshDriver {
    let mut driver = DshDriver::spawn(&launch(mode), settings(), limits())
        .await
        .expect("spawn");
    driver.await_ready().await.expect("handshake");
    driver
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
    let mut driver = DshDriver::spawn(&launch("hang"), settings(), limits())
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
    let mut driver = DshDriver::spawn(&launch("exit-early"), settings(), limits())
        .await
        .expect("spawn");
    let error = driver.await_ready().await.expect_err("closed transport");
    assert_eq!(error, DshDriverError::TransportClosed);
    assert!(!driver.transport_open());
    assert!(!driver.poll(0).expect("poll").transport_open);
    let _ = driver.close().await;
}

/// Opens a session whose runtime exits right after the handshake, and waits
/// for its stdout to end, so `close` writes `shutdown` into a pipe whose
/// reader is gone on every platform.
async fn open_until_exited(mode: &str) -> DshDriver {
    let driver = open(mode).await;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while driver.transport_open() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(!driver.transport_open(), "the runtime should have exited");
    driver
}

#[tokio::test]
async fn closing_after_the_runtime_exited_on_its_own_is_still_a_clean_close() {
    let mut driver = open_until_exited("exit-after-ready").await;
    // `close` documents that a peer that already died cannot receive the
    // frame and that the teardown still runs: a successful exit closes the
    // session rather than failing it.
    driver.close().await.expect("close");
    assert_eq!(driver.phase(), DshPhase::Closed);
    assert_eq!(driver.poll(0).expect("poll").summary.error_code, None);
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

/// Sessions each race test opens. The late answer loses the race in most
/// rounds, so a regression fails within a few of them.
const RACE_ROUNDS: usize = 8;

#[tokio::test]
async fn cancelling_before_the_prompt_is_answered_still_closes_cleanly() {
    for round in 0..RACE_ROUNDS {
        let mut driver = open("no-turn-end").await;
        driver.prompt("cancel at once").await.expect("prompt");
        // `prompt` returns once the frame is written, so the runtime's
        // receipt and the turn's first events usually arrive after this.
        driver.cancel().await.expect("cancel");
        let _ = driver.close().await;
        let summary = driver.poll(0).expect("poll").summary;
        assert_eq!(
            (driver.phase(), summary.error_code),
            (DshPhase::Closed, None),
            "round {round}"
        );
        assert_eq!(summary.cancelled_turns, 1, "round {round}");
    }
}

#[tokio::test]
async fn closing_before_the_prompt_is_answered_still_closes_cleanly() {
    for round in 0..RACE_ROUNDS {
        let mut driver = open("serve").await;
        driver.prompt("close at once").await.expect("prompt");
        // `shutdown` supersedes the unanswered prompt, and the runtime answers
        // both: the receipt and the whole turn arrive before its `{}`.
        driver.close().await.expect("close");
        let summary = driver.poll(0).expect("poll").summary;
        assert_eq!(
            (driver.phase(), summary.error_code),
            (DshPhase::Closed, None),
            "round {round}"
        );
        // A turn that settled before `shutdown` was written is completed;
        // otherwise closing cancelled it. It is never counted twice.
        assert_eq!(
            summary.completed_turns + summary.cancelled_turns,
            1,
            "round {round}"
        );
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
