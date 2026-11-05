//! Bounded deny-only diagnostic I/O. This does not enable an approval profile.

use std::{future::pending, time::Duration};

use serde_json::{Value, json};
use tokio::{
    sync::watch,
    time::{Instant, sleep_until},
};
use uuid::Uuid;
use vibemux_harness::chat_session::{
    MAX_INPUT_CHUNK_BYTES, MAX_PROMPT_BYTES, SessionConfig, SessionDecoder, SessionEffect,
    SessionLimits, SessionMachine, SessionPhase, TurnRequest,
};

use super::{
    DIAGNOSTIC_REPORT_VERSION, DIAGNOSTIC_SHUTDOWN_LIMIT, DiagnosticReport, DiagnosticStop,
    ProbeContext, ProbeReport,
};
use crate::chat_session::{
    RuntimeCleanup,
    approval_probe::ProbeObserver,
    input_writer::{InputReceipt, InputWriteOutcome, InputWriter},
    transport::RuntimeTransport,
};
use vibemux_harness::chat_session::SessionTarget;

const ROOT_POLL: Duration = Duration::from_millis(100);
const OBSERVATION_RECORDS: usize = 128;

pub(super) async fn drive_probe<T: RuntimeTransport>(
    mut transport: T,
    context: ProbeContext,
    mut cancellation: watch::Receiver<bool>,
) -> DiagnosticReport {
    // Even invalid setup retains and joins any acquired pipe owner before finish.
    let mut input = transport.take_stdin().map(InputWriter::start);
    let shutdown = if context.limits.valid() {
        context.limits.shutdown
    } else {
        DIAGNOSTIC_SHUTDOWN_LIMIT
    };
    let mut report = DiagnosticReport {
        report_version: DIAGNOSTIC_REPORT_VERSION,
        observations: ProbeReport::default(),
        initialize_written: false,
        initialize_accepted: false,
        input_queued: false,
        input_written: false,
        denial_queued: false,
        denial_written: false,
        stdout_bytes: 0,
        stop: DiagnosticStop::InvalidConfiguration,
        cleanup: RuntimeCleanup::not_started().into(),
    };
    if let (Some(input), Ok(mut state)) = (input.as_mut(), ProbeState::new(context)) {
        report.stop = state.run(&mut transport, input, &mut cancellation).await;
        report.observations = state.observer.report();
        report.initialize_written = state.initialize_written;
        report.initialize_accepted = state.handshake_seen;
        report.input_queued = state.input_queued;
        report.input_written = state.input_written;
        report.denial_queued = state.denial_queued;
        report.denial_written = state.denial_written;
        report.stdout_bytes = state.stdout_bytes;
    }
    if let Some(input) = &mut input {
        input.task_abort_and_join().await;
    }
    report.cleanup = transport
        .finish(true, Instant::now() + shutdown)
        .await
        .into();
    report
}

struct ProbeState {
    machine: SessionMachine,
    decoder: SessionDecoder,
    observer: ProbeObserver,
    user_id: Uuid,
    turn_id: Uuid,
    prompt: Option<String>,
    limits: super::DiagnosticLimits,
    initialize_deadline: Instant,
    turn_deadline: Option<Instant>,
    initialize_receipt: Option<InputReceipt>,
    input_receipt: Option<InputReceipt>,
    denial_receipt: Option<InputReceipt>,
    initialize_written: bool,
    input_written: bool,
    denial_written: bool,
    handshake_seen: bool,
    input_queued: bool,
    denial_queued: bool,
    stdout_eof: bool,
    stdout_bytes: u64,
}

#[derive(Clone, Copy)]
enum ReceiptKind {
    Initialize,
    Input,
    Denial,
}

enum Action {
    Cancellation(bool),
    Receipt(ReceiptKind, InputWriteOutcome),
    Read(std::io::Result<usize>),
    Deadline,
    PollRoot,
}

impl ProbeState {
    fn new(context: ProbeContext) -> Result<Self, DiagnosticStop> {
        if !context.limits.valid()
            || context.prompt.trim().is_empty()
            || context.prompt.len() > MAX_PROMPT_BYTES
        {
            return Err(DiagnosticStop::InvalidConfiguration);
        }
        let turn_id = Uuid::new_v4();
        if turn_id == context.user_id {
            return Err(DiagnosticStop::InvalidConfiguration);
        }
        let observer = ProbeObserver::new(
            context.session_id,
            context.user_id,
            context.working_directory.clone(),
        )
        .map_err(|_| DiagnosticStop::InvalidConfiguration)?;
        let protocol_limits = SessionLimits {
            max_turns: 1,
            max_records: OBSERVATION_RECORDS,
            max_record_bytes: usize::try_from(context.limits.output_bytes)
                .map_err(|_| DiagnosticStop::InvalidConfiguration)?,
            ..SessionLimits::default()
        };
        let machine = SessionMachine::new(
            SessionConfig {
                target: SessionTarget::Claude {
                    session_id: context.session_id,
                    resume: false,
                },
                generation_id: Uuid::new_v4(),
                working_directory: context.working_directory,
                model: None,
            },
            protocol_limits.clone(),
        )
        .map_err(|_| DiagnosticStop::InvalidConfiguration)?;
        let decoder = SessionDecoder::new(&protocol_limits)
            .map_err(|_| DiagnosticStop::InvalidConfiguration)?;
        let initialize_deadline = Instant::now()
            .checked_add(context.limits.initialize)
            .ok_or(DiagnosticStop::InvalidConfiguration)?;
        Ok(Self {
            machine,
            decoder,
            observer,
            user_id: context.user_id,
            turn_id,
            prompt: Some(context.prompt),
            limits: context.limits,
            initialize_deadline,
            turn_deadline: None,
            initialize_receipt: None,
            input_receipt: None,
            denial_receipt: None,
            initialize_written: false,
            input_written: false,
            denial_written: false,
            handshake_seen: false,
            input_queued: false,
            denial_queued: false,
            stdout_eof: false,
            stdout_bytes: 0,
        })
    }

    async fn run<T: RuntimeTransport>(
        &mut self,
        transport: &mut T,
        input: &InputWriter,
        cancellation: &mut watch::Receiver<bool>,
    ) -> DiagnosticStop {
        if *cancellation.borrow() || cancellation.has_changed().is_err() {
            return DiagnosticStop::Cancelled;
        }
        match transport.root_exited() {
            Ok(false) => {}
            Ok(true) => return DiagnosticStop::RootExited,
            Err(_) => return DiagnosticStop::ReadFailed,
        }
        let initialize = match self.machine.start().and_then(single_write) {
            Ok(frame) => frame,
            Err(_) => return DiagnosticStop::ProtocolRejected,
        };
        self.initialize_receipt = match input.send_tracked(initialize.as_bytes()) {
            Ok(receipt) => Some(receipt),
            Err(_) => return DiagnosticStop::InputUnknown,
        };
        let mut next_root_poll = Instant::now() + ROOT_POLL;
        let mut buffer = vec![0; MAX_INPUT_CHUNK_BYTES];
        loop {
            if *cancellation.borrow() {
                return DiagnosticStop::Cancelled;
            }
            let deadline = self.turn_deadline.unwrap_or(self.initialize_deadline);
            if Instant::now() >= deadline {
                return if self.input_queued {
                    DiagnosticStop::TurnDeadline
                } else {
                    DiagnosticStop::InitializeDeadline
                };
            }
            if self.observer.report().terminal_seen
                && self.input_written
                && (!self.denial_queued || self.denial_written)
            {
                return DiagnosticStop::ObservedTerminal;
            }
            if self.stdout_bytes >= self.limits.output_bytes
                && !self.observer.report().terminal_seen
            {
                return DiagnosticStop::OutputLimit;
            }
            // Drain the entire preceding output batch before scheduling this input.
            // The peer may receive these queued bytes before our flush receipt.
            if self.handshake_seen
                && self.initialize_written
                && !self.input_queued
                && let Err(stop) = self.queue_input(input)
            {
                return stop;
            }
            let deadline = self.turn_deadline.unwrap_or(self.initialize_deadline);
            let read_limit = usize::try_from(
                self.limits
                    .output_bytes
                    .saturating_sub(self.stdout_bytes)
                    .min(buffer.len() as u64),
            )
            .unwrap_or(buffer.len());
            let action = tokio::select! {
                changed = cancellation.changed() => {
                    Action::Cancellation(changed.is_err() || *cancellation.borrow())
                }
                outcome = receipt_outcome(&mut self.initialize_receipt) => {
                    Action::Receipt(ReceiptKind::Initialize, outcome)
                }
                outcome = receipt_outcome(&mut self.input_receipt) => {
                    Action::Receipt(ReceiptKind::Input, outcome)
                }
                outcome = receipt_outcome(&mut self.denial_receipt) => {
                    Action::Receipt(ReceiptKind::Denial, outcome)
                }
                read = read_output(transport, &mut buffer[..read_limit], self.stdout_eof || read_limit == 0) => Action::Read(read),
                () = sleep_until(deadline) => Action::Deadline,
                () = sleep_until(next_root_poll) => Action::PollRoot,
            };
            match action {
                Action::Cancellation(true) => return DiagnosticStop::Cancelled,
                Action::Cancellation(false) => {}
                Action::Receipt(kind, outcome) => {
                    match kind {
                        ReceiptKind::Initialize => self.initialize_receipt = None,
                        ReceiptKind::Input => self.input_receipt = None,
                        ReceiptKind::Denial => self.denial_receipt = None,
                    }
                    if outcome == InputWriteOutcome::Unknown {
                        return DiagnosticStop::InputUnknown;
                    }
                    match kind {
                        ReceiptKind::Initialize => self.initialize_written = true,
                        ReceiptKind::Input => self.input_written = true,
                        ReceiptKind::Denial => self.denial_written = true,
                    }
                }
                Action::Read(Err(_)) => return DiagnosticStop::ReadFailed,
                Action::Read(Ok(0)) => {
                    let final_frame = match self.decoder.finish() {
                        Ok(frame) => frame,
                        Err(_) => return DiagnosticStop::ProtocolRejected,
                    };
                    if let Some(frame) = final_frame
                        && let Err(stop) = self.record(&frame, input)
                    {
                        return stop;
                    }
                    self.stdout_eof = true;
                    if !self.observer.report().terminal_seen {
                        return DiagnosticStop::EofWithoutTerminal;
                    }
                    // Receipt completion remains independently observed after EOF.
                }
                Action::Read(Ok(bytes)) => {
                    let Some(total) = self.stdout_bytes.checked_add(bytes as u64) else {
                        return DiagnosticStop::OutputLimit;
                    };
                    self.stdout_bytes = total;
                    if total > self.limits.output_bytes {
                        return DiagnosticStop::OutputLimit;
                    }
                    let frames = match self.decoder.push(&buffer[..bytes]) {
                        Ok(frames) => frames,
                        Err(_) => return DiagnosticStop::ProtocolRejected,
                    };
                    for frame in frames {
                        if let Err(stop) = self.record(&frame, input) {
                            return stop;
                        }
                    }
                }
                Action::Deadline => {
                    return if self.input_queued {
                        DiagnosticStop::TurnDeadline
                    } else {
                        DiagnosticStop::InitializeDeadline
                    };
                }
                Action::PollRoot => {
                    next_root_poll = Instant::now() + ROOT_POLL;
                    match transport.root_exited() {
                        Ok(false) => {}
                        Ok(true) => return DiagnosticStop::RootExited,
                        Err(_) => return DiagnosticStop::ReadFailed,
                    }
                }
            }
        }
    }

    fn queue_input(&mut self, input: &InputWriter) -> Result<(), DiagnosticStop> {
        let prompt = self.prompt.take().ok_or(DiagnosticStop::ProtocolRejected)?;
        let submitted = self
            .machine
            .submit(TurnRequest {
                request_id: self.user_id,
                turn_id: self.turn_id,
                prompt,
            })
            .and_then(single_write)
            .map_err(|_| DiagnosticStop::ProtocolRejected)?;
        let receipt = input
            .send_tracked(submitted.as_bytes())
            .map_err(|_| DiagnosticStop::InputUnknown)?;
        self.input_receipt = Some(receipt);
        self.input_queued = true;
        self.turn_deadline = Some(
            Instant::now()
                .checked_add(self.limits.turn)
                .ok_or(DiagnosticStop::InvalidConfiguration)?,
        );
        Ok(())
    }

    fn record(&mut self, frame: &str, input: &InputWriter) -> Result<(), DiagnosticStop> {
        let value: Value =
            serde_json::from_str(frame).map_err(|_| DiagnosticStop::ProtocolRejected)?;
        if !value.is_object() {
            return Err(DiagnosticStop::ProtocolRejected);
        }
        let kind = value["type"]
            .as_str()
            .ok_or(DiagnosticStop::ProtocolRejected)?;
        if !self.input_queued && is_work(&value) {
            return Err(DiagnosticStop::PreInputWork);
        }
        if kind == "control_response" {
            if self.handshake_seen {
                return Err(DiagnosticStop::ProtocolRejected);
            }
            self.machine
                .receive(1, frame.as_bytes())
                .map_err(|_| DiagnosticStop::ProtocolRejected)?;
            if self.machine.snapshot().phase != SessionPhase::Ready {
                return Err(DiagnosticStop::ProtocolRejected);
            }
            self.handshake_seen = true;
        } else if kind == "control_request" && value["request"]["subtype"] != "can_use_tool" {
            return Err(DiagnosticStop::ProtocolRejected);
        }
        // No manual-profile, assistant, tool or result record reaches the
        // production read-only SessionMachine parser.
        self.observer
            .observe(&value)
            .map_err(|_| DiagnosticStop::ProtocolRejected)?;
        if let Some(request_id) = self.observer.pending_request_id().map(str::to_owned) {
            if self.denial_queued {
                return Err(DiagnosticStop::ProtocolRejected);
            }
            self.observer
                .mark_denied()
                .map_err(|_| DiagnosticStop::ProtocolRejected)?;
            let mut response = serde_json::to_vec(&json!({
                "type":"control_response","response":{
                    "subtype":"success","request_id":request_id,
                    "response":{"behavior":"deny","message":"Diagnostic denied"}
                }
            }))
            .map_err(|_| DiagnosticStop::ProtocolRejected)?;
            response.push(b'\n');
            self.denial_receipt = Some(
                input
                    .send_tracked(&response)
                    .map_err(|_| DiagnosticStop::InputUnknown)?,
            );
            self.denial_queued = true;
        }
        Ok(())
    }
}

fn single_write(
    transition: vibemux_harness::chat_session::SessionTransition,
) -> Result<vibemux_harness::chat_session::WireFrame, vibemux_harness::chat_session::SessionError> {
    let mut effects = transition.effects.into_iter();
    match (effects.next(), effects.next()) {
        (Some(SessionEffect::Write(frame)), None) => Ok(frame),
        _ => Err(vibemux_harness::chat_session::SessionError::UnexpectedState),
    }
}

fn is_work(value: &Value) -> bool {
    matches!(value["type"].as_str(), Some("assistant" | "result"))
        || (value["type"] == "stream_event" && value["event"]["type"] != "ping")
        || (value["type"] == "control_request" && value["request"]["subtype"] == "can_use_tool")
        || value["type"] == "tool"
}

async fn receipt_outcome(receipt: &mut Option<InputReceipt>) -> InputWriteOutcome {
    match receipt {
        Some(receipt) => receipt.await,
        None => pending().await,
    }
}

async fn read_output<T: RuntimeTransport>(
    transport: &mut T,
    buffer: &mut [u8],
    eof: bool,
) -> std::io::Result<usize> {
    if eof {
        pending().await
    } else {
        transport.read(buffer).await
    }
}

#[cfg(test)]
#[path = "driver_tests.rs"]
mod tests;
