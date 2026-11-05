#![forbid(unsafe_code)]
//! Submit at most once; reconcile and observe only that request thereafter.

use std::{
    future::Future,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use uuid::Uuid;
use vibemux_harness::{
    AgentKind,
    dispatch::{DispatchError, DispatchPhase, DispatchRequest, NativeProtocol},
};
use vibemuxd::{
    control::HarnessDispatchOutputQuery,
    harness_dispatch::{DispatchOutputPage, OutputReassembler},
};

use super::{
    observation::{
        AskAdmission, AskCanonicalStatus, AskObservation, AskObserverState, AskOutputState,
        AskSubmissionState, SubmitError,
    },
    reply_text::ReplyText,
    session::{AskEnd, AskPhase},
};

const MAX_PAGES_PER_TICK: usize = 32;
const STOP_CHECK_INTERVAL: Duration = Duration::from_millis(25);
const MAX_STATUS_FAILURES: usize = 3;

pub(crate) trait DispatchPort {
    async fn submit(&mut self, request: &DispatchRequest) -> Result<AskAdmission, SubmitError>;
    async fn status(&mut self, request_id: Uuid) -> Result<AskCanonicalStatus, String>;
    async fn output(
        &mut self,
        query: HarnessDispatchOutputQuery,
    ) -> Result<DispatchOutputPage, String>;
    async fn cancel(&mut self, request_id: Uuid) -> Result<(), String>;
    fn generation_changed(&self) -> bool {
        false
    }
}

pub(crate) enum AskOperation {
    Submit(DispatchRequest),
    Observe {
        request_id: Uuid,
        harness: AgentKind,
    },
}

pub(crate) struct DriverControl<'a> {
    pub cancel: &'a AtomicBool,
    pub stop: &'a AtomicBool,
}

pub(crate) struct DriverTiming {
    pub poll_interval: Duration,
    pub deadline: Duration,
}

/// Status and display delivery are independent. Ending this function never
/// upgrades incomplete canonical evidence into an execution outcome.
pub(crate) async fn drive_observed<P: DispatchPort>(
    port: &mut P,
    operation: AskOperation,
    control: DriverControl<'_>,
    timing: DriverTiming,
    mut sink: impl FnMut(AskObservation),
) {
    let (request_id, harness) = match &operation {
        AskOperation::Submit(request) => (request.request_id, request.harness),
        AskOperation::Observe {
            request_id,
            harness,
        } => (*request_id, *harness),
    };
    let owns_submission = matches!(&operation, AskOperation::Submit(_));
    let mut observation = AskObservation::pending(request_id);
    let mut reply = ReplyText::default();
    let mut reassembler = OutputReassembler::starting_after(0);
    let started = Instant::now();
    sink(observation.clone());

    if let AskOperation::Submit(request) = operation {
        if control.cancel.load(Ordering::Acquire) || control.stop.load(Ordering::Acquire) {
            observation.submission = AskSubmissionState::NotSubmitted;
            observation.cancellation_requested = control.cancel.load(Ordering::Acquire);
            observation.observer = AskObserverState::Stopped;
            observation.update.phase = AskPhase::Finished;
            observation.update.end = Some(AskEnd::Cancelled);
            sink(observation);
            return;
        }
        // From this point until a receipt/status proves otherwise, an
        // interrupted call may already have been admitted by the daemon.
        observation.submission = AskSubmissionState::Uncertain;
        match until_stopped(control.stop, port.submit(&request)).await {
            Some(Ok(receipt))
                if valid_identity(
                    request_id,
                    harness,
                    receipt.request_id,
                    receipt.harness,
                    receipt.protocol,
                ) =>
            {
                observation.admission = Some(receipt);
                observation.submission = AskSubmissionState::Confirmed;
            }
            Some(Ok(_)) => {
                finish_error(&mut observation, "ask_correlation_mismatch");
                sink(observation);
                return;
            }
            Some(Err(error)) if !error.uncertain => {
                observation.submission = AskSubmissionState::Rejected;
                finish_error(&mut observation, &error.code);
                sink(observation);
                return;
            }
            Some(Err(error)) => observation.observer_error = Some(error.code),
            None => {
                finish_stopped(&mut observation);
                sink(observation);
                return;
            }
        }
    } else {
        observation.submission = AskSubmissionState::Uncertain;
    }

    observation.observer = AskObserverState::Observing;
    observation.update.phase = if observation.submission == AskSubmissionState::Confirmed {
        AskPhase::Running
    } else {
        AskPhase::Submitting
    };
    sink(observation.clone());
    let mut cancel_sent = false;
    let mut status_failures = 0;
    loop {
        if control.stop.load(Ordering::Acquire) {
            finish_stopped(&mut observation);
            sink(observation);
            return;
        }
        observation.cancellation_requested |= control.cancel.load(Ordering::Acquire);
        let status = until_stopped(control.stop, port.status(request_id)).await;
        observation.daemon_restarted |= port.generation_changed();
        match status {
            Some(Ok(status)) => {
                if let Err(code) = accept_status(&mut observation, harness, status) {
                    finish_error(&mut observation, code);
                    sink(observation);
                    return;
                }
                status_failures = 0;
                if !cancel_sent {
                    observation.observer_error = None;
                }
            }
            Some(Err(code)) => {
                status_failures += 1;
                observation.observer_error = Some(code.clone());
                if status_failures >= MAX_STATUS_FAILURES {
                    abandon_owned(
                        port,
                        &mut observation,
                        owns_submission,
                        cancel_sent,
                        control.stop,
                    )
                    .await;
                    finish_error(&mut observation, &code);
                    sink(observation);
                    return;
                }
            }
            None => {
                finish_stopped(&mut observation);
                sink(observation);
                return;
            }
        }

        if observation
            .canonical
            .as_ref()
            .is_some_and(|status| status.phase == DispatchPhase::RecoveryPending)
        {
            // Cancellation acknowledges recovery in the daemon. Observation
            // or automatic cleanup must never make that operator decision.
            finish_error(&mut observation, "dispatch_recovery_pending");
            sink(observation);
            return;
        }

        if observation.cancellation_requested
            && !cancel_sent
            && !observation.terminal_observed()
            && observation.submission == AskSubmissionState::Confirmed
        {
            cancel_sent = true;
            match until_stopped(control.stop, port.cancel(request_id)).await {
                Some(Err(code)) => observation.observer_error = Some(code),
                Some(Ok(())) => {}
                None => {
                    finish_stopped(&mut observation);
                    sink(observation);
                    return;
                }
            }
        }

        let protocol = observation
            .canonical
            .as_ref()
            .map(|status| status.protocol)
            .or_else(|| {
                observation
                    .admission
                    .as_ref()
                    .map(|receipt| receipt.protocol)
            });
        if let Some(protocol) = protocol {
            let output = until_stopped(
                control.stop,
                read_output(port, request_id, protocol, &mut reassembler, &mut reply),
            )
            .await;
            observation.update.reply = reply.text();
            observation.update.truncated = reply.truncated();
            match output {
                Some(Ok(output)) => observation.output = output,
                Some(Err(code)) => {
                    observation.output = AskOutputState::Failed;
                    abandon_owned(
                        port,
                        &mut observation,
                        owns_submission,
                        cancel_sent,
                        control.stop,
                    )
                    .await;
                    finish_error(&mut observation, &code);
                    sink(observation);
                    return;
                }
                None => {
                    finish_stopped(&mut observation);
                    sink(observation);
                    return;
                }
            }
        }

        if observation.terminal_observed()
            && matches!(
                observation.output,
                AskOutputState::Complete | AskOutputState::Unavailable
            )
        {
            observation.observer = AskObserverState::Finished;
            observation.update.phase = AskPhase::Finished;
            observation.update.end = canonical_end(&observation);
            observation.observer_error = None;
            if observation.output == AskOutputState::Unavailable {
                observation.observer_error =
                    Some(DispatchError::OutputUnavailable.code().to_string());
            }
            sink(observation);
            return;
        }

        if started.elapsed() >= timing.deadline {
            abandon_owned(
                port,
                &mut observation,
                owns_submission,
                cancel_sent,
                control.stop,
            )
            .await;
            finish_error(&mut observation, "ask_timeout");
            sink(observation);
            return;
        }
        observation.update.phase = if observation.cancellation_requested {
            AskPhase::Cancelling
        } else if observation.submission == AskSubmissionState::Confirmed {
            AskPhase::Running
        } else {
            AskPhase::Submitting
        };
        sink(observation.clone());
        if until_stopped(control.stop, tokio::time::sleep(timing.poll_interval))
            .await
            .is_none()
        {
            finish_stopped(&mut observation);
            sink(observation);
            return;
        }
    }
}

fn valid_identity(
    request_id: Uuid,
    harness: AgentKind,
    observed_id: Uuid,
    observed_harness: AgentKind,
    protocol: NativeProtocol,
) -> bool {
    request_id == observed_id
        && harness == observed_harness
        && matches!(
            (harness, protocol),
            (AgentKind::Claude, NativeProtocol::ClaudeStreamJson)
                | (
                    AgentKind::Codex,
                    NativeProtocol::CodexAppServer | NativeProtocol::CodexExec
                )
        )
}

/// Preserve the legacy submitter's best-effort cleanup while a read-only
/// attachment never acquires that authority. A restart fences cancellation.
async fn abandon_owned<P: DispatchPort>(
    port: &mut P,
    observation: &mut AskObservation,
    owns_submission: bool,
    cancel_sent: bool,
    stop: &AtomicBool,
) {
    if owns_submission
        && !cancel_sent
        && observation.submission == AskSubmissionState::Confirmed
        && !observation.daemon_restarted
        && !observation.terminal_observed()
    {
        let _ = until_stopped(stop, port.cancel(observation.update.request_id)).await;
        observation.cancellation_requested = true;
        observation.daemon_restarted |= port.generation_changed();
    }
}

fn accept_status(
    observation: &mut AskObservation,
    harness: AgentKind,
    status: AskCanonicalStatus,
) -> Result<(), &'static str> {
    if !valid_identity(
        observation.update.request_id,
        harness,
        status.request_id,
        status.harness,
        status.protocol,
    ) || observation.admission.as_ref().is_some_and(|receipt| {
        receipt.task_id != status.task_id
            || receipt.run_id != status.run_id
            || receipt.protocol != status.protocol
    }) || observation.canonical.as_ref().is_some_and(|previous| {
        previous.task_id != status.task_id
            || previous.run_id != status.run_id
            || previous.protocol != status.protocol
    }) {
        return Err("ask_correlation_mismatch");
    }
    if observation.canonical.as_ref().is_some_and(|previous| {
        status.version < previous.version
            || (previous.phase.is_terminal() && previous.phase != status.phase)
            || (status.version == previous.version && *previous != status)
    }) {
        return Err("ask_status_regressed");
    }
    observation.submission = AskSubmissionState::Confirmed;
    observation.canonical = Some(status);
    Ok(())
}

#[cfg(test)]
#[path = "driver_tests.rs"]
mod tests;

fn canonical_end(observation: &AskObservation) -> Option<AskEnd> {
    let status = observation.canonical.as_ref()?;
    super::session::attempt_end(&super::session::AttemptStatus {
        phase: status.phase,
        error_code: status.error_code.clone(),
    })
}

fn finish_error(observation: &mut AskObservation, code: &str) {
    observation.observer = AskObserverState::Failed;
    observation.observer_error = Some(code.to_string());
    observation.update.phase = AskPhase::Finished;
    observation.update.end =
        canonical_end(observation).or_else(|| Some(AskEnd::Failed(code.to_string())));
}

fn finish_stopped(observation: &mut AskObservation) {
    observation.observer = AskObserverState::Stopped;
    observation.observer_error = Some("ask_observer_stopped".to_string());
    observation.update.phase = AskPhase::Finished;
    observation.update.end = canonical_end(observation)
        .or_else(|| Some(AskEnd::Failed("ask_observer_stopped".to_string())));
}

async fn until_stopped<T>(stop: &AtomicBool, future: impl Future<Output = T>) -> Option<T> {
    tokio::pin!(future);
    loop {
        if stop.load(Ordering::Acquire) {
            return None;
        }
        tokio::select! {
            output = &mut future => return Some(output),
            () = tokio::time::sleep(STOP_CHECK_INTERVAL) => {},
        }
    }
}

async fn read_output<P: DispatchPort>(
    port: &mut P,
    request_id: Uuid,
    protocol: NativeProtocol,
    reassembler: &mut OutputReassembler,
    reply: &mut ReplyText,
) -> Result<AskOutputState, String> {
    for _ in 0..MAX_PAGES_PER_TICK {
        let query = HarnessDispatchOutputQuery {
            request_id,
            cursor: reassembler.cursor(),
        };
        let page = match port.output(query).await {
            Ok(page) => page,
            Err(code) if code == DispatchError::OutputUnavailable.code() => {
                return Ok(AskOutputState::Unavailable);
            }
            Err(code) => return Err(code),
        };
        let more = !page.fragments.is_empty();
        let complete = page.complete;
        let records = reassembler
            .accept(page)
            .map_err(|_| "dispatch_output_inconsistent".to_string())?;
        for record in records {
            if let Ok(value) = serde_json::from_str(&record.raw_json) {
                reply.absorb(protocol, &value);
            }
        }
        if complete {
            return Ok(AskOutputState::Complete);
        }
        if !more {
            break;
        }
    }
    Ok(AskOutputState::Partial)
}
