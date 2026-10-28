//! One dispatch attempt or probe, from spawn to the fenced finish (ADR 029
//! §§5–6).
//!
//! An attempt claims its admitted record, spawns the contained vendor
//! process, runs the read/write exchange under the protocol session, ends
//! the process, and reports the decided outcome with its claim fence. The
//! exchange reads stdout while a separate task owns stdin, so a vendor that
//! stops draining its input never stalls the capture, and cancellation and
//! the deadline stay responsive. Records go straight to their sink; nothing
//! captured reaches a log or an event.

use std::{num::NonZeroUsize, sync::Arc, time::Duration};

use serde_json::Value;
use time::OffsetDateTime;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::ChildStdin,
    sync::{mpsc, watch},
    task::JoinHandle,
    time::{Instant, sleep, timeout_at},
};
use uuid::Uuid;
use vibemux_harness::dispatch::{
    DispatchError, DispatchLimits, NativeProtocol, ObservedRecord, OutcomeDecision, ProcessExit,
    ProtocolSession,
    capture_budget::{CaptureBudget, CaptureSummary},
    events::ProcessSummary,
    json_line_framer::JsonLineFramer,
    outcome::decide_outcome,
    protocol_session::InitialInput,
};
use vibemux_store::HarnessDispatchFinish;

use crate::{WriterError, WriterHandle};

use super::{
    native_process::{LaunchPlan, NativeProcess, cancellation_requested, resolve_environment},
    transcript_store::TranscriptStore,
};

const READ_CHUNK_BYTES: usize = 64 * 1024;
/// Lines waiting for the stdin task. A vendor that leaves this many replies
/// unread fails the attempt rather than growing the queue.
const INPUT_QUEUE_LINES: usize = 16;
/// Backoff between writer calls that met a saturated queue or a late
/// response. After the last one the call's error stands.
const WRITER_RETRY_DELAYS: [Duration; 3] = [
    Duration::from_millis(50),
    Duration::from_millis(200),
    Duration::from_millis(800),
];

/// Everything one vendor run needs besides its cancel signal.
pub(super) struct ExecutionPlan {
    pub launch: LaunchPlan,
    pub broker: Option<super::broker_owner::BrokerOwner>,
    pub lease_loss: Option<watch::Receiver<bool>>,
    pub session: ProtocolSession,
    pub budget: CaptureBudget,
    pub limits: DispatchLimits,
    #[cfg(feature = "test_helpers")]
    pub forget_cleanup: bool,
}

/// Content-free evidence of one vendor run.
#[derive(Clone, Copy, Debug)]
pub(super) struct ExecutionEvidence {
    pub decision: OutcomeDecision,
    pub process: ProcessSummary,
    pub capture: CaptureSummary,
}

/// Failed cleanup retains its exact process owner and cannot finish a dispatch.
pub(super) struct AttemptCompletion {
    pub final_text: Option<String>,
    pub unsettled: Option<NativeProcess>,
}

pub(super) struct OwnedExecution {
    pub evidence: ExecutionEvidence,
    pub unsettled: Option<NativeProcess>,
    pub broker: Option<super::broker_owner::BrokerOwner>,
}

/// Where accepted records go.
enum RecordSink<'a> {
    Transcript {
        store: &'a TranscriptStore,
        request_id: Uuid,
    },
    /// Probes keep only the summary.
    Discard,
}

impl RecordSink<'_> {
    fn append(&self, record: ObservedRecord) {
        if let Self::Transcript { store, request_id } = self {
            store.append(*request_id, record);
        }
    }
}

/// Runs one admitted attempt to its fenced finish. A claim that fails
/// launches nothing; a finish the writer cannot take after its retries
/// leaves the attempt for startup recovery.
pub(super) async fn run_attempt(
    writer: WriterHandle,
    transcripts: Arc<TranscriptStore>,
    request_id: Uuid,
    plan: ExecutionPlan,
    cancel: watch::Receiver<bool>,
) -> Option<NativeProcess> {
    run_attempt_with_final_text(writer, transcripts, request_id, plan, cancel, None)
        .await
        .unsettled
}

/// [`run_attempt`] that also returns the final assistant text of
/// `protocol`, read from the live transcript before it can be evicted
/// (ADR 031 §5).
pub(super) async fn run_attempt_with_final_text(
    writer: WriterHandle,
    transcripts: Arc<TranscriptStore>,
    request_id: Uuid,
    plan: ExecutionPlan,
    mut cancel: watch::Receiver<bool>,
    final_text_protocol: Option<NativeProtocol>,
) -> AttemptCompletion {
    let lease_loss = plan.lease_loss.clone();
    let claim_writer = writer.clone();
    let claim = call_writer(move || {
        claim_writer.claim_harness_dispatch(request_id, OffsetDateTime::now_utc())
    })
    .await;
    let Ok(claim) = claim else {
        return AttemptCompletion {
            final_text: None,
            unsettled: None,
        };
    };
    transcripts.begin(request_id);
    let sink = RecordSink::Transcript {
        store: &transcripts,
        request_id,
    };
    let OwnedExecution {
        mut evidence,
        unsettled,
        mut broker,
    } = execute(plan, sink, &mut cancel).await;
    let final_text =
        final_text_protocol.and_then(|protocol| transcripts.final_text(request_id, protocol));
    transcripts.finish(request_id);
    if unsettled.is_some() {
        return AttemptCompletion {
            final_text: None,
            unsettled,
        };
    }
    if super::broker_owner::interrupted(&cancel, lease_loss.as_ref()) {
        evidence.decision = OutcomeDecision {
            outcome: vibemux_harness::dispatch::AttemptOutcome::Cancelled,
            error_code: Some(DispatchError::Cancelled),
        };
    }
    let finish = HarnessDispatchFinish {
        request_id,
        fence: claim.fence,
        decision: evidence.decision,
        process: evidence.process,
        capture: evidence.capture,
        timestamp: OffsetDateTime::now_utc(),
    };
    let finished = call_writer(move || writer.finish_harness_dispatch(finish)).await;
    if finished.is_ok_and(|commit| {
        commit.record.phase == vibemux_harness::dispatch::DispatchPhase::Completed
    }) && !super::broker_owner::interrupted(&cancel, lease_loss.as_ref())
        && let Some(owner) = broker.as_mut()
    {
        owner.confirm_completion();
    }
    AttemptCompletion {
        final_text,
        unsettled: None,
    }
}

/// Runs an initialize-only probe. Records are counted, never kept.
pub(super) async fn run_probe(
    plan: ExecutionPlan,
    mut cancel: watch::Receiver<bool>,
) -> OwnedExecution {
    execute(plan, RecordSink::Discard, &mut cancel).await
}

/// Runs a blocking writer call off the async runtime. Only a saturated
/// queue or a late response is retried, so every caller must be safe to
/// repeat: each writer operation is idempotent or fenced.
pub(crate) async fn call_writer<T, F>(call: F) -> Result<T, WriterError>
where
    F: Fn() -> Result<T, WriterError> + Clone + Send + 'static,
    T: Send + 'static,
{
    let mut delays = WRITER_RETRY_DELAYS.iter();
    loop {
        let result = tokio::task::spawn_blocking(call.clone())
            .await
            .unwrap_or(Err(WriterError::ThreadTerminated));
        match (&result, delays.next()) {
            (Err(WriterError::QueueFull | WriterError::ResponseTimeout), Some(delay)) => {
                sleep(*delay).await;
            }
            _ => return result,
        }
    }
}

async fn execute(
    plan: ExecutionPlan,
    sink: RecordSink<'_>,
    cancel: &mut watch::Receiver<bool>,
) -> OwnedExecution {
    let ExecutionPlan {
        launch,
        mut broker,
        lease_loss,
        mut session,
        mut budget,
        limits,
        #[cfg(feature = "test_helpers")]
        forget_cleanup,
    } = plan;
    let deadline = Instant::now() + Duration::from_millis(limits.deadline_ms);
    let not_started = ProcessExit {
        exit_code: None,
        forced_termination: false,
    };
    let spawned = if cancellation_requested(cancel) {
        Err(DispatchError::Cancelled)
    } else {
        match launch
            .environment
            .clone()
            .map(Ok)
            .unwrap_or_else(|| resolve_environment(&launch.spec.environment_names))
        {
            Ok(environment) => NativeProcess::spawn(&launch, environment).await,
            Err(error) => Err(error),
        }
    };
    let (exchange, exit, stderr_bytes, unsettled) = match spawned {
        Err(error) => (Err(error), not_started, 0, None),
        Ok(mut process) => {
            let end = run_exchange(
                &mut process,
                &mut session,
                &mut budget,
                &sink,
                &limits,
                cancel,
                deadline,
            )
            .await;
            let grace = Duration::from_millis(limits.shutdown_grace_ms);
            let shutdown_deadline = end
                .shutdown_deadline
                .unwrap_or(Instant::now() + grace)
                .min(deadline);
            // A correlated cancellation response has already honored the
            // interrupt. Preserve its remaining graceful shutdown interval;
            // an ordinary successful terminal must still react immediately.
            let confirmed_cancel =
                session.terminal() == Some(vibemux_harness::dispatch::SessionTerminal::Cancelled);
            let interrupt = if confirmed_cancel {
                None
            } else {
                Some(&mut *cancel)
            };
            let completion = process
                .finish(end.force, shutdown_deadline, interrupt)
                .await;
            let settled = completion.settled;
            #[cfg(feature = "test_helpers")]
            let settled = settled && !forget_cleanup;
            let result = if !settled {
                Err(DispatchError::ContainmentFailed)
            } else if completion.cancelled || cancellation_requested(cancel) {
                Err(DispatchError::Cancelled)
            } else {
                end.result
            };
            (
                result,
                completion.exit,
                completion.stderr_bytes,
                (!settled).then_some(process),
            )
        }
    };
    let mut decision = decide_outcome(exchange, session.terminal(), exit);
    if decision.outcome == vibemux_harness::dispatch::AttemptOutcome::Completed
        && unsettled.is_none()
        && let Some(mut owner) = broker.take()
    {
        // Keep filesystem import in this registered task. A detached blocking
        // task could continue writing after shutdown aborted its async waiter.
        let admission = run_inline_import(|| owner.admit_candidate(cancel, lease_loss.as_ref()));
        broker = Some(owner);
        if let Err(error) = admission {
            decision = OutcomeDecision {
                outcome: vibemux_harness::dispatch::AttemptOutcome::Failed,
                error_code: Some(error),
            };
        }
    }
    OwnedExecution {
        evidence: ExecutionEvidence {
            decision,
            process: ProcessSummary::new(exit, stderr_bytes),
            capture: budget.summary(),
        },
        unsettled,
        broker,
    }
}

/// Non-preemptible filesystem I/O remains part of the execution owner's poll.
/// Shutdown joins this owner even when an abort is requested during import.
/// Byte/file bounds do not imply a hard wall-clock bound on local disk I/O.
fn run_inline_import<T>(import: impl FnOnce() -> T) -> T {
    if tokio::runtime::Handle::current().runtime_flavor()
        == tokio::runtime::RuntimeFlavor::MultiThread
    {
        tokio::task::block_in_place(import)
    } else {
        import()
    }
}

/// How the exchange ended.
struct ExchangeEnd {
    result: Result<(), DispatchError>,
    /// Kill the tree instead of letting the process exit on its own.
    force: bool,
    shutdown_deadline: Option<Instant>,
}

impl ExchangeEnd {
    const fn failed(error: DispatchError) -> Self {
        Self {
            result: Err(error),
            force: true,
            shutdown_deadline: None,
        }
    }
}

/// Reads stdout until EOF, feeding each frame through the budget, the sink,
/// and the session, and queueing the session's replies. Cancellation sends
/// the protocol cancel when the session can address one and then allows
/// the shutdown grace; the deadline and a vendor that keeps its output open
/// after the terminal end the exchange with a forced kill.
async fn run_exchange(
    process: &mut NativeProcess,
    session: &mut ProtocolSession,
    budget: &mut CaptureBudget,
    sink: &RecordSink<'_>,
    limits: &DispatchLimits,
    cancel: &mut watch::Receiver<bool>,
    deadline: Instant,
) -> ExchangeEnd {
    let Some(stdin) = process.stdin.take() else {
        return ExchangeEnd::failed(DispatchError::PipeFailed);
    };
    let Some(frame_limit) = NonZeroUsize::new(limits.frame_bytes) else {
        return ExchangeEnd::failed(DispatchError::Internal);
    };
    let mut input = InputWriter::start(stdin);
    let mut framer = JsonLineFramer::new(frame_limit);
    let grace = Duration::from_millis(limits.shutdown_grace_ms);
    let mut read_deadline = deadline;
    let mut cancel_requested = false;
    let mut terminal_seen = false;

    let first = match session.initial_input() {
        InitialInput::Message(message) => input.send_line(&message),
        InitialInput::PromptThenClose(prompt) => {
            let sent = input.send(prompt.into_bytes());
            input.close();
            sent
        }
    };
    if let Err(error) = first {
        return ExchangeEnd::failed(error);
    }

    let mut buffer = vec![0_u8; READ_CHUNK_BYTES];
    let mut frames = Vec::new();
    loop {
        tokio::select! {
            biased;
            // A dropped sender (service shutdown) also cancels.
            _ = cancel.changed(), if !cancel_requested => {
                cancel_requested = true;
                if session.is_finished() {
                    return ExchangeEnd::failed(DispatchError::Cancelled);
                }
                let Some(message) = session.cancel_message() else {
                    return ExchangeEnd::failed(DispatchError::Cancelled);
                };
                if input.send_line(&message).is_err() {
                    return ExchangeEnd::failed(DispatchError::Cancelled);
                }
                read_deadline = read_deadline.min(Instant::now() + grace);
            }
            read = timeout_at(read_deadline, process.stdout.read(&mut buffer)) => {
                let count = match read {
                    Err(_) if session.is_finished() => {
                        return ExchangeEnd { result: Ok(()), force: true, shutdown_deadline: Some(read_deadline) };
                    }
                    Err(_) if cancel_requested => {
                        return ExchangeEnd::failed(DispatchError::Cancelled);
                    }
                    Err(_) => return ExchangeEnd::failed(DispatchError::DeadlineExceeded),
                    Ok(Err(_)) => return ExchangeEnd::failed(DispatchError::PipeFailed),
                    Ok(Ok(count)) => count,
                };
                if count == 0 {
                    return end_of_output(&mut framer, session, budget, sink, &input,
                        ShutdownState { cancel_requested, terminal_seen, deadline: read_deadline });
                }
                let handled = framer
                    .push(&buffer[..count], &mut frames)
                    .and_then(|()| {
                        frames
                            .drain(..)
                            .try_for_each(|frame| handle_frame(frame, session, budget, sink, &input))
                    });
                if let Err(error) = handled {
                    return ExchangeEnd::failed(error);
                }
                if session.is_finished() && !terminal_seen {
                    terminal_seen = true;
                    input.close();
                    read_deadline = read_deadline.min(Instant::now() + grace);
                }
            }
        }
    }
}

struct ShutdownState {
    cancel_requested: bool,
    terminal_seen: bool,
    deadline: Instant,
}

fn end_of_output(
    framer: &mut JsonLineFramer,
    session: &mut ProtocolSession,
    budget: &mut CaptureBudget,
    sink: &RecordSink<'_>,
    input: &InputWriter,
    shutdown: ShutdownState,
) -> ExchangeEnd {
    let last = framer.finish().and_then(|last| match last {
        Some(frame) => handle_frame(frame, session, budget, sink, input),
        None => Ok(()),
    });
    if let Err(error) = last {
        return ExchangeEnd::failed(error);
    }
    if shutdown.cancel_requested && !session.is_finished() {
        // The vendor exited without confirming the cancel.
        return ExchangeEnd {
            result: Err(DispatchError::Cancelled),
            force: false,
            shutdown_deadline: None,
        };
    }
    ExchangeEnd {
        result: Ok(()),
        force: false,
        // A last record without a newline may establish the terminal only
        // inside framer.finish(). Cancellation already owns a grace deadline.
        shutdown_deadline: (shutdown.terminal_seen || shutdown.cancel_requested)
            .then_some(shutdown.deadline),
    }
}

/// Accounts, keeps, and then interprets one frame, so the capture summary
/// and the transcript cover the same records even when the session rejects
/// the last one.
fn handle_frame(
    frame: String,
    session: &mut ProtocolSession,
    budget: &mut CaptureBudget,
    sink: &RecordSink<'_>,
    input: &InputWriter,
) -> Result<(), DispatchError> {
    let (record, value) = budget.accept(frame)?;
    let kind = record.kind();
    sink.append(record);
    for reply in session.receive(&value, kind)? {
        input.send_line(&reply)?;
    }
    Ok(())
}

/// Owns the vendor's stdin. Lines are written in order by a separate task;
/// closing drops the queue's sender, so stdin closes once the queued lines
/// are written.
struct InputWriter {
    sender: Option<mpsc::Sender<Vec<u8>>>,
    task: JoinHandle<()>,
}

impl InputWriter {
    fn start(stdin: ChildStdin) -> Self {
        let (sender, receiver) = mpsc::channel(INPUT_QUEUE_LINES);
        Self {
            sender: Some(sender),
            task: tokio::spawn(write_input(stdin, receiver)),
        }
    }

    fn send_line(&self, message: &Value) -> Result<(), DispatchError> {
        let mut line = serde_json::to_vec(message).map_err(|_| DispatchError::Internal)?;
        line.push(b'\n');
        self.send(line)
    }

    /// Queues bytes without waiting. After [`InputWriter::close`] the
    /// session has finished and a late reply is moot, so it is dropped.
    fn send(&self, bytes: Vec<u8>) -> Result<(), DispatchError> {
        match &self.sender {
            Some(sender) => sender
                .try_send(bytes)
                .map_err(|_| DispatchError::PipeFailed),
            None => Ok(()),
        }
    }

    fn close(&mut self) {
        self.sender = None;
    }
}

impl Drop for InputWriter {
    /// The exchange is over; a write still blocked on the vendor ends with
    /// the process tree.
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn write_input(mut stdin: ChildStdin, mut receiver: mpsc::Receiver<Vec<u8>>) {
    while let Some(bytes) = receiver.recv().await {
        if stdin.write_all(&bytes).await.is_err() || stdin.flush().await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod completion_tests {
    use super::*;
    use vibemux_harness::dispatch::{DispatchRoute, SessionTerminal};

    #[tokio::test]
    async fn eof_cancellation_ack_does_not_renew_the_existing_grace() {
        let route: DispatchRoute = serde_json::from_value(serde_json::json!({
            "harness": "claude", "protocol": "claude_stream_json",
            "executable": "C:/fixture/claude.exe", "enabled": true, "allow_execution": true,
        }))
        .expect("route");
        let mut session =
            ProtocolSession::for_execution(&route, "fixture".into(), "C:/fixture".into())
                .expect("session");
        let limits = DispatchLimits::default();
        let mut budget = CaptureBudget::new(route.protocol, &limits).expect("budget");
        let input = InputWriter {
            sender: None,
            task: tokio::spawn(async {}),
        };
        let sink = RecordSink::Discard;
        let initialize = serde_json::json!({
            "type": "control_response",
            "response": {"subtype": "success", "request_id": "vibemux_initialize", "response": {}}
        });
        handle_frame(
            initialize.to_string(),
            &mut session,
            &mut budget,
            &sink,
            &input,
        )
        .expect("correlated initialize");
        assert!(session.cancel_message().is_some());
        let mut framer = JsonLineFramer::new(NonZeroUsize::new(limits.frame_bytes).unwrap());
        let mut frames = Vec::new();
        framer
            .push(
                serde_json::json!({
                    "type": "result", "subtype": "success", "is_error": false,
                    "terminal_reason": "aborted_streaming", "parent_tool_use_id": null,
                    "result": "fixture stopped",
                })
                .to_string()
                .as_bytes(),
                &mut frames,
            )
            .expect("unterminated final frame");
        assert!(frames.is_empty());
        assert!(!session.is_finished());
        let deadline = Instant::now() + Duration::from_millis(1);
        let end = end_of_output(
            &mut framer,
            &mut session,
            &mut budget,
            &sink,
            &input,
            ShutdownState {
                cancel_requested: true,
                terminal_seen: false,
                deadline,
            },
        );
        assert!(end.result.is_ok());
        assert_eq!(session.terminal(), Some(SessionTerminal::Cancelled));
        assert_eq!(end.shutdown_deadline, Some(deadline));
        assert!(!end.force);
    }
}

#[cfg(test)]
mod import_ownership_tests {
    use super::run_inline_import;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn abort_does_not_detach_a_started_import_from_its_join() {
        let (started, observed) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        let wrote = Arc::new(AtomicBool::new(false));
        let worker_wrote = Arc::clone(&wrote);
        let task = tokio::spawn(async move {
            run_inline_import(|| {
                started.send(()).unwrap();
                released.recv().unwrap();
                worker_wrote.store(true, Ordering::SeqCst);
            });
            tokio::task::yield_now().await;
        });
        observed.await.unwrap();
        task.abort();
        assert!(!task.is_finished(), "ownership ended while import was held");
        assert!(!wrote.load(Ordering::SeqCst));
        release.send(()).unwrap();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(
            wrote.load(Ordering::SeqCst),
            "join must observe completed import"
        );
    }
}
