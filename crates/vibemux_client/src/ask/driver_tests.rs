use super::*;
use std::collections::VecDeque;
use vibemux_harness::dispatch::{
    AttemptOutcome, ObservationKind, request::DISPATCH_REQUEST_SCHEMA_VERSION,
};
use vibemux_types::{RunId, TaskId};
use vibemuxd::harness_dispatch::{DispatchOutputFragment, OutputCursor};

const REQUEST_ID: Uuid = Uuid::from_u128(123);

struct ScriptedPort {
    receipt: Result<AskAdmission, SubmitError>,
    statuses: VecDeque<Result<AskCanonicalStatus, String>>,
    pages: VecDeque<Result<DispatchOutputPage, String>>,
    submitted: Vec<Uuid>,
    queried: Vec<Uuid>,
    cancels: usize,
    cancel_error: Option<String>,
    hang_submit: bool,
    hang_status: bool,
    restarted: bool,
}

fn status(phase: DispatchPhase, version: u64) -> AskCanonicalStatus {
    AskCanonicalStatus {
        request_id: REQUEST_ID,
        task_id: TaskId::from_uuid(Uuid::from_u128(456)).expect("task id"),
        run_id: RunId::from_uuid(Uuid::from_u128(789)).expect("run id"),
        harness: AgentKind::Claude,
        protocol: NativeProtocol::ClaudeStreamJson,
        phase,
        version,
        outcome: match phase {
            DispatchPhase::Completed => Some(AttemptOutcome::Completed),
            DispatchPhase::Cancelled => Some(AttemptOutcome::Cancelled),
            _ => None,
        },
        error_code: None,
    }
}

fn fixture() -> ScriptedPort {
    let canonical = status(DispatchPhase::Admitted, 1);
    ScriptedPort {
        receipt: Ok(AskAdmission {
            request_id: canonical.request_id,
            task_id: canonical.task_id,
            run_id: canonical.run_id,
            harness: canonical.harness,
            protocol: canonical.protocol,
            phase: canonical.phase,
            duplicate: false,
            sequence: 17,
        }),
        statuses: VecDeque::from([Ok(status(DispatchPhase::Completed, 3))]),
        pages: VecDeque::new(),
        submitted: Vec::new(),
        queried: Vec::new(),
        cancels: 0,
        cancel_error: None,
        hang_submit: false,
        hang_status: false,
        restarted: false,
    }
}

impl DispatchPort for ScriptedPort {
    async fn submit(&mut self, request: &DispatchRequest) -> Result<AskAdmission, SubmitError> {
        self.submitted.push(request.request_id);
        if self.hang_submit {
            std::future::pending::<()>().await;
        }
        self.receipt.clone()
    }

    async fn status(&mut self, request_id: Uuid) -> Result<AskCanonicalStatus, String> {
        self.queried.push(request_id);
        if self.hang_status {
            std::future::pending::<()>().await;
        }
        if self.statuses.len() > 1 {
            self.statuses.pop_front().expect("scripted status")
        } else {
            self.statuses.front().expect("last status").clone()
        }
    }

    async fn output(
        &mut self,
        query: HarnessDispatchOutputQuery,
    ) -> Result<DispatchOutputPage, String> {
        self.pages.pop_front().unwrap_or_else(|| {
            Ok(DispatchOutputPage {
                fragments: Vec::new(),
                next: query.cursor,
                complete: true,
            })
        })
    }

    async fn cancel(&mut self, _: Uuid) -> Result<(), String> {
        self.cancels += 1;
        self.cancel_error.clone().map_or(Ok(()), Err)
    }

    fn generation_changed(&self) -> bool {
        self.restarted
    }
}

fn submit() -> AskOperation {
    AskOperation::Submit(DispatchRequest {
        schema_version: DISPATCH_REQUEST_SCHEMA_VERSION,
        request_id: REQUEST_ID,
        harness: AgentKind::Claude,
        prompt: "synthetic prompt".to_string(),
    })
}

fn observe() -> AskOperation {
    AskOperation::Observe {
        request_id: REQUEST_ID,
        harness: AgentKind::Claude,
    }
}

async fn run(
    port: &mut ScriptedPort,
    operation: AskOperation,
    cancel: bool,
) -> Vec<AskObservation> {
    let mut observations = Vec::new();
    drive_observed(
        port,
        operation,
        DriverControl {
            cancel: &AtomicBool::new(cancel),
            stop: &AtomicBool::new(false),
        },
        DriverTiming {
            poll_interval: Duration::from_millis(1),
            deadline: Duration::from_secs(1),
        },
        |update| observations.push(update),
    )
    .await;
    observations
}

#[tokio::test]
async fn uncertain_submission_reconciles_the_reserved_id_without_resubmitting() {
    let mut port = fixture();
    port.receipt = Err(SubmitError {
        code: "control_deadline_exceeded".into(),
        uncertain: true,
    });
    let observations = run(&mut port, submit(), false).await;
    let last = observations.last().expect("observation");
    assert_eq!(port.submitted, [REQUEST_ID]);
    assert_eq!(port.queried, [REQUEST_ID]);
    assert!(last.admission.is_none(), "a lost receipt is not invented");
    assert_eq!(last.submission, AskSubmissionState::Confirmed);
    assert_eq!(
        last.canonical.as_ref().expect("canonical").task_id,
        status(DispatchPhase::Completed, 3).task_id
    );
    assert!(last.terminal_observed());
    assert_eq!(last.update.end, Some(AskEnd::Completed));
}

#[tokio::test]
async fn unresolved_submission_keeps_uncertain_ownership() {
    let mut port = fixture();
    port.receipt = Err(SubmitError {
        code: "control_invalid_frame".into(),
        uncertain: true,
    });
    port.statuses = VecDeque::from([Err("harness_dispatch_not_found".into())]);
    let last = run(&mut port, submit(), false).await.pop().expect("last");
    assert_eq!(port.submitted, [REQUEST_ID]);
    assert_eq!(port.queried, [REQUEST_ID; MAX_STATUS_FAILURES]);
    assert_eq!(last.submission, AskSubmissionState::Uncertain);
    assert_eq!(last.observer, AskObserverState::Failed);
    assert!(!last.terminal_observed());
    assert_eq!(port.cancels, 0);
}

#[tokio::test]
async fn completed_attempt_with_evicted_output_keeps_its_canonical_outcome() {
    let mut port = fixture();
    port.pages
        .push_back(Err(DispatchError::OutputUnavailable.code().to_string()));
    let last = run(&mut port, submit(), false).await.pop().expect("last");
    assert!(last.terminal_observed());
    assert_eq!(last.update.end, Some(AskEnd::Completed));
    assert_eq!(last.output, AskOutputState::Unavailable);
    assert_eq!(
        last.canonical.expect("canonical").outcome,
        Some(AttemptOutcome::Completed)
    );
    assert_eq!(last.admission.expect("receipt").sequence, 17);
}

#[tokio::test]
async fn output_failure_does_not_overwrite_the_last_daemon_outcome() {
    let mut port = fixture();
    port.pages
        .push_back(Err("control_endpoint_unavailable".into()));
    let last = run(&mut port, submit(), false).await.pop().expect("last");
    assert!(last.terminal_observed());
    assert_eq!(last.update.end, Some(AskEnd::Completed));
    assert_eq!(last.observer, AskObserverState::Failed);
    assert_eq!(last.output, AskOutputState::Failed);
    assert_eq!(port.cancels, 0);
}

#[tokio::test]
async fn observation_only_never_submits_or_implicitly_cancels() {
    let mut port = fixture();
    let last = run(&mut port, observe(), false).await.pop().expect("last");
    assert!(port.submitted.is_empty());
    assert!(last.admission.is_none());
    assert!(last.terminal_observed());
    assert_eq!(port.cancels, 0);
}

#[tokio::test]
async fn recovery_pending_retains_reservation_without_acknowledgement() {
    let mut port = fixture();
    port.statuses = VecDeque::from([Ok(status(DispatchPhase::RecoveryPending, 4))]);
    port.restarted = true;
    let last = run(&mut port, observe(), true).await.pop().expect("last");
    assert_eq!(port.cancels, 0);
    assert!(!last.terminal_observed());
    assert!(last.daemon_restarted);
    assert_eq!(
        last.canonical.expect("canonical").phase,
        DispatchPhase::RecoveryPending
    );
}

#[tokio::test]
async fn cancel_before_admission_never_submits() {
    let mut port = fixture();
    let last = run(&mut port, submit(), true).await.pop().expect("last");
    assert_eq!(last.submission, AskSubmissionState::NotSubmitted);
    assert!(!last.terminal_observed());
    assert!(port.submitted.is_empty());
    assert!(port.queried.is_empty());
    assert_eq!(port.cancels, 0);
}

#[tokio::test]
async fn cancel_racing_completion_preserves_completed_outcome() {
    let mut port = fixture();
    port.statuses = VecDeque::from([
        Ok(status(DispatchPhase::Running, 2)),
        Ok(status(DispatchPhase::Completed, 3)),
    ]);
    port.cancel_error = Some("harness_dispatch_terminal".into());
    let last = run(&mut port, observe(), true).await.pop().expect("last");
    assert_eq!(port.cancels, 1);
    assert_eq!(last.update.end, Some(AskEnd::Completed));
    assert!(last.observer_error.is_none());
}

#[tokio::test]
async fn stale_status_keeps_previous_canonical_identity_and_phase() {
    let mut port = fixture();
    port.statuses = VecDeque::from([
        Ok(status(DispatchPhase::Running, 2)),
        Ok(status(DispatchPhase::Admitted, 1)),
    ]);
    let last = run(&mut port, submit(), false).await.pop().expect("last");
    assert_eq!(last.observer_error.as_deref(), Some("ask_status_regressed"));
    assert_eq!(
        last.canonical.expect("last canonical").phase,
        DispatchPhase::Running
    );
    assert!(!last.update.end.is_some_and(|end| end == AskEnd::Completed));
}

#[tokio::test]
async fn wrong_receipt_correlation_is_never_accepted() {
    let mut port = fixture();
    port.receipt.as_mut().expect("receipt").request_id = Uuid::from_u128(999);
    let last = run(&mut port, submit(), false).await.pop().expect("last");
    assert!(last.admission.is_none());
    assert!(last.canonical.is_none());
    assert_eq!(last.submission, AskSubmissionState::Uncertain);
    assert_eq!(
        last.observer_error.as_deref(),
        Some("ask_correlation_mismatch")
    );
}

#[tokio::test]
async fn observer_stop_interrupts_a_pending_submit_without_claiming_rejection() {
    let mut port = fixture();
    port.hang_submit = true;
    let stop = AtomicBool::new(false);
    let cancel = AtomicBool::new(false);
    let mut observations = Vec::new();
    let driving = drive_observed(
        &mut port,
        submit(),
        DriverControl {
            cancel: &cancel,
            stop: &stop,
        },
        DriverTiming {
            poll_interval: Duration::from_millis(1),
            deadline: Duration::from_secs(1),
        },
        |update| observations.push(update),
    );
    let stopping = async {
        tokio::time::sleep(Duration::from_millis(30)).await;
        stop.store(true, Ordering::Release);
    };
    tokio::time::timeout(Duration::from_secs(1), async {
        tokio::join!(driving, stopping);
    })
    .await
    .expect("bounded stop");
    let last = observations.last().expect("last");
    assert_eq!(last.submission, AskSubmissionState::Uncertain);
    assert_eq!(last.observer, AskObserverState::Stopped);
    assert!(!last.terminal_observed());
    assert_eq!(port.submitted, [REQUEST_ID]);
}

#[tokio::test]
async fn fragmented_unicode_json_becomes_one_replacement_reply() {
    let mut port = fixture();
    let record = "{\"type\":\"result\",\"result\":\"你好 é\"}\r\n";
    let split = record.find('好').expect("split boundary");
    let fragment = |offset: usize, text: &str| DispatchOutputFragment {
        sequence: 1,
        kind: ObservationKind::Text,
        total_bytes: record.len() as u64,
        offset: offset as u64,
        fragment: text.into(),
    };
    port.pages = VecDeque::from([
        Ok(DispatchOutputPage {
            fragments: vec![fragment(0, &record[..split])],
            next: OutputCursor {
                after_sequence: 0,
                cursor_offset: split as u64,
            },
            complete: false,
        }),
        Ok(DispatchOutputPage {
            fragments: vec![fragment(split, &record[split..])],
            next: OutputCursor {
                after_sequence: 1,
                cursor_offset: 0,
            },
            complete: true,
        }),
    ]);
    let last = run(&mut port, submit(), false).await.pop().expect("last");
    assert_eq!(last.update.reply, "你好 é");
    assert_eq!(last.output, AskOutputState::Complete);
    assert_eq!(last.update.end, Some(AskEnd::Completed));
}
