#![forbid(unsafe_code)]
//! Content-free daemon evidence and local observation state for native Ask.
//! A stopped observer is not evidence that its canonical attempt stopped.

use uuid::Uuid;
use vibemux_harness::{
    AgentKind,
    dispatch::{AttemptOutcome, DispatchPhase, NativeProtocol},
};
use vibemux_types::{RunId, TaskId};
use vibemuxd::control::{HarnessDispatchReceipt, HarnessDispatchStatus};

use super::session::{AskPhase, AskUpdate};

/// The allowlisted admission receipt. Every identity comes from the daemon.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AskAdmission {
    pub request_id: Uuid,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub harness: AgentKind,
    pub protocol: NativeProtocol,
    pub phase: DispatchPhase,
    pub duplicate: bool,
    pub sequence: u64,
}

impl From<HarnessDispatchReceipt> for AskAdmission {
    fn from(receipt: HarnessDispatchReceipt) -> Self {
        Self {
            request_id: receipt.request_id,
            task_id: receipt.task_id,
            run_id: receipt.run_id,
            harness: receipt.harness,
            protocol: receipt.protocol,
            phase: receipt.phase,
            duplicate: receipt.duplicate,
            sequence: receipt.sequence,
        }
    }
}

/// Last authoritative status; no prompt, reply, path or process metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AskCanonicalStatus {
    pub request_id: Uuid,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub harness: AgentKind,
    pub protocol: NativeProtocol,
    pub phase: DispatchPhase,
    pub version: u64,
    pub outcome: Option<AttemptOutcome>,
    pub error_code: Option<String>,
}

impl From<HarnessDispatchStatus> for AskCanonicalStatus {
    fn from(status: HarnessDispatchStatus) -> Self {
        Self {
            request_id: status.request_id,
            task_id: status.task_id,
            run_id: status.run_id,
            harness: status.harness,
            protocol: status.protocol,
            phase: status.phase,
            version: status.version,
            outcome: status.outcome,
            error_code: status.error_code.map(|error| error.code().to_string()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AskSubmissionState {
    Pending,
    Confirmed,
    Rejected,
    /// The request may have been committed. Only read the same request ID.
    Uncertain,
    /// Cancellation or validation stopped the worker before submission.
    NotSubmitted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AskObserverState {
    Submitting,
    Observing,
    Finished,
    Failed,
    Stopped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AskOutputState {
    Pending,
    Partial,
    Complete,
    Unavailable,
    Failed,
}

/// A complete replacement snapshot. The legacy update is presentation only;
/// native clients use `canonical` for execution outcome and ownership.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AskObservation {
    pub update: AskUpdate,
    pub admission: Option<AskAdmission>,
    pub canonical: Option<AskCanonicalStatus>,
    pub submission: AskSubmissionState,
    pub observer: AskObserverState,
    pub observer_error: Option<String>,
    pub output: AskOutputState,
    pub cancellation_requested: bool,
    pub daemon_restarted: bool,
}

impl AskObservation {
    #[must_use]
    pub fn pending(request_id: Uuid) -> Self {
        Self {
            update: AskUpdate {
                request_id,
                phase: AskPhase::Submitting,
                reply: String::new(),
                truncated: false,
                end: None,
            },
            admission: None,
            canonical: None,
            submission: AskSubmissionState::Pending,
            observer: AskObserverState::Submitting,
            observer_error: None,
            output: AskOutputState::Pending,
            cancellation_requested: false,
            daemon_restarted: false,
        }
    }

    #[must_use]
    pub fn terminal_observed(&self) -> bool {
        self.canonical
            .as_ref()
            .is_some_and(|status| status.phase.is_terminal())
    }

    /// The observer has ended; this says nothing about canonical completion.
    #[must_use]
    pub fn observer_finished(&self) -> bool {
        matches!(
            self.observer,
            AskObserverState::Finished | AskObserverState::Failed | AskObserverState::Stopped
        )
    }
}

/// Whether a failed submit may have crossed the authenticated transport.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SubmitError {
    pub code: String,
    pub uncertain: bool,
}
