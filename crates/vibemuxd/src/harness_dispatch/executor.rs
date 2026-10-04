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
    native_process::{LaunchPlan, NativeProcess, resolve_environment},
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
    pub session: ProtocolSession,
    pub budget: CaptureBudget,
    pub limits: DispatchLimits,
}

/// Content-free evidence of one vendor run.
#[derive(Clone, Copy, Debug)]
pub(super) struct ExecutionEvidence {
    pub decision: OutcomeDecision,
    pub process: ProcessSummary,
    pub capture: CaptureSummary,
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
) {
    run_attempt_with_final_text(writer, transcripts, request_id, plan, cancel, None).await;
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
) -> Option<String> {
    let claim_writer = writer.clone();
    let claim = call_writer(move || {
        claim_writer.claim_harness_dispatch(request_id, OffsetDateTime::now_utc())
    })
    .await;
    let Ok(claim) = claim else {
        return None;
    };
    transcripts.begin(request_id);
    let sink = RecordSink::Transcript {
        store: &transcripts,
        request_id,
    };
    let evidence = execute(plan, sink, &mut cancel).await;
    let final_text =
        final_text_protocol.and_then(|protocol| transcripts.final_text(request_id, protocol));
    transcripts.finish(request_id);
    let finish = HarnessDispatchFinish {
        request_id,
        fence: claim.fence,
        decision: evidence.decision,
        process: evidence.process,
        capture: evidence.capture,
        timestamp: OffsetDateTime::now_utc(),
    };
    let _ = call_writer(move || writer.finish_harness_dispatch(finish)).await;
    final_text
}

/// Runs an initialize-only probe. Records are counted, never kept.
pub(super) async fn run_probe(
    plan: ExecutionPlan,
    mut cancel: watch::Receiver<bool>,
) -> ExecutionEvidence {
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
) -> ExecutionEvidence {
    let ExecutionPlan {
        launch,
        mut session,
        mut budget,
        limits,
    } = plan;
    let deadline = Instant::now() + Duration::from_millis(limits.deadline_ms);
    let not_started = ProcessExit {
        exit_code: None,
        forced_termination: false,
    };
    let spawned = if *cancel.borrow_and_update() {
        Err(DispatchError::Cancelled)
    } else {
        match resolve_environment(&launch.spec.environment_names) {
            Ok(environment) => NativeProcess::spawn(&launch, environment).await,
            Err(error) => Err(error),
        }
    };
    let (exchange, exit, stderr_bytes) = match spawned {
        Err(error) => (Err(error), not_started, 0),
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
            let (exit, stderr_bytes) = process.finish(end.force, Instant::now() + grace).await;
            (end.result, exit, stderr_bytes)
        }
    };
    ExecutionEvidence {
        decision: decide_outcome(exchange, session.terminal(), exit),
        process: ProcessSummary::new(exit, stderr_bytes),
        capture: budget.summary(),
    }
}

/// How the exchange ended.
struct ExchangeEnd {
    result: Result<(), DispatchError>,
    /// Kill the tree instead of letting the process exit on its own.
    force: bool,
}

impl ExchangeEnd {
    const fn failed(error: DispatchError) -> Self {
        Self {
            result: Err(error),
            force: true,
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
                    continue;
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
                        return ExchangeEnd { result: Ok(()), force: true };
                    }
                    Err(_) if cancel_requested => {
                        return ExchangeEnd::failed(DispatchError::Cancelled);
                    }
                    Err(_) => return ExchangeEnd::failed(DispatchError::DeadlineExceeded),
                    Ok(Err(_)) => return ExchangeEnd::failed(DispatchError::PipeFailed),
                    Ok(Ok(count)) => count,
                };
                if count == 0 {
                    return end_of_output(&mut framer, session, budget, sink, &input, cancel_requested);
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

fn end_of_output(
    framer: &mut JsonLineFramer,
    session: &mut ProtocolSession,
    budget: &mut CaptureBudget,
    sink: &RecordSink<'_>,
    input: &InputWriter,
    cancel_requested: bool,
) -> ExchangeEnd {
    let last = framer.finish().and_then(|last| match last {
        Some(frame) => handle_frame(frame, session, budget, sink, input),
        None => Ok(()),
    });
    if let Err(error) = last {
        return ExchangeEnd::failed(error);
    }
    if cancel_requested && !session.is_finished() {
        // The vendor exited without confirming the cancel.
        return ExchangeEnd {
            result: Err(DispatchError::Cancelled),
            force: false,
        };
    }
    ExchangeEnd {
        result: Ok(()),
        force: false,
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
