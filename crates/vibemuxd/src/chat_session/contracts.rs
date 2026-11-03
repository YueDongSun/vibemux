//! The writer/process boundary. Private bodies deliberately do not serialize.

use std::{fmt, sync::Arc, time::Duration};

use crate::chat_writer::ChatApprovalDecisionResult;
use async_trait::async_trait;
use thiserror::Error;
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};
use uuid::Uuid;
use vibemux_harness::{
    chat_session::{
        ApprovalChallenge, AssistantText, CommandProgress, SessionConfig, SessionError,
        SessionLimits, SessionSnapshot, ToolObservation, TurnRequest, TurnSummary,
    },
    dispatch::ProcessExit,
};
use vibemux_store::{ChatApprovalDeliveryClaim, ChatOwnerFence};
use vibemux_types::{
    ChatApprovalId, ChatId,
    chat_approval::{ChatApproval, ChatApprovalDecision, ChatApprovalDelivery},
};

use crate::harness_dispatch::native_process::LaunchPlan;

pub const RUNTIME_INBOX_CAPACITY: usize = 8;
pub const RUNTIME_STDIN_CAPACITY: usize = 16;
pub const RUNTIME_IDLE_LIMIT: Duration = Duration::from_secs(10 * 60);
pub const RUNTIME_INITIALIZE_LIMIT: Duration = Duration::from_secs(30);
pub const RUNTIME_CANCEL_GRACE: Duration = Duration::from_secs(3);
pub const RUNTIME_CALLBACK_LIMIT: Duration = Duration::from_secs(5);
pub const RUNTIME_SHUTDOWN_LIMIT: Duration = Duration::from_secs(15);
pub const RUNTIME_COMMAND_ACK_LIMIT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
pub struct RuntimeBinding {
    pub chat_id: ChatId,
    pub generation: u64,
    pub owner_fence: ChatOwnerFence,
}

#[derive(Clone, Debug)]
pub struct RuntimeLimits {
    pub idle: Duration,
    pub initialize: Duration,
    pub cancel_grace: Duration,
    pub callback: Duration,
    pub shutdown: Duration,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            idle: RUNTIME_IDLE_LIMIT,
            initialize: RUNTIME_INITIALIZE_LIMIT,
            cancel_grace: RUNTIME_CANCEL_GRACE,
            callback: RUNTIME_CALLBACK_LIMIT,
            shutdown: RUNTIME_SHUTDOWN_LIMIT,
        }
    }
}

impl RuntimeLimits {
    pub(crate) fn validate(&self) -> Result<(), RuntimeError> {
        let maximum = Self::default();
        for (value, ceiling) in [
            (self.idle, maximum.idle),
            (self.initialize, maximum.initialize),
            (self.cancel_grace, maximum.cancel_grace),
            (self.callback, maximum.callback),
            (self.shutdown, maximum.shutdown),
        ] {
            if value.is_zero() || value > ceiling {
                return Err(RuntimeError::InvalidConfiguration);
            }
        }
        Ok(())
    }
}

pub(crate) struct RuntimePlan {
    pub binding: RuntimeBinding,
    pub launch: LaunchPlan,
    pub protocol: SessionConfig,
    pub protocol_limits: SessionLimits,
    pub limits: RuntimeLimits,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeStopReason {
    Closed,
    Idle,
    Shutdown,
    Deadline,
    CancellationUnconfirmed,
    ProcessLost,
    ProtocolViolation,
    InputFailed,
    WriterUnavailable,
    StartupFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeCleanup {
    pub exit: ProcessExit,
    pub root_reaped: bool,
    pub tree_empty: bool,
    pub cleanup_verified: bool,
    pub stderr_bytes: u64,
}

impl RuntimeCleanup {
    pub(crate) fn not_started() -> Self {
        Self {
            exit: ProcessExit {
                exit_code: None,
                forced_termination: false,
            },
            root_reaped: true,
            tree_empty: true,
            cleanup_verified: true,
            stderr_bytes: 0,
        }
    }
}

pub enum RuntimeEvent {
    Ready(SessionSnapshot),
    VendorBound {
        vendor_session_id: String,
    },
    TurnAcknowledged(TurnSummary),
    AssistantSnapshot(AssistantText),
    ToolCompleted(ToolObservation),
    TurnFinished {
        turn: TurnSummary,
        snapshot: SessionSnapshot,
        output_complete: bool,
    },
    TransportEnded {
        snapshot: SessionSnapshot,
        pending_turn: Option<TurnSummary>,
        reason: RuntimeStopReason,
        cleanup: RuntimeCleanup,
    },
}

/// One coalesced, bounded first-party replacement batch. Private text is
/// intentionally excluded from Debug, events, and durable history.
pub struct RuntimeCommandProgress {
    pub request_id: Uuid,
    pub turn_id: Uuid,
    pub updates: Vec<CommandProgress>,
    pub limited: bool,
}

impl fmt::Debug for RuntimeCommandProgress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeCommandProgress")
            .field("updates", &self.updates.len())
            .field("limited", &self.limited)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for RuntimeEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Ready(_) => "RuntimeEvent::Ready",
            Self::VendorBound { .. } => "RuntimeEvent::VendorBound",
            Self::TurnAcknowledged(_) => "RuntimeEvent::TurnAcknowledged",
            Self::AssistantSnapshot(_) => "RuntimeEvent::AssistantSnapshot(<private>)",
            Self::ToolCompleted(_) => "RuntimeEvent::ToolCompleted(<private>)",
            Self::TurnFinished { .. } => "RuntimeEvent::TurnFinished",
            Self::TransportEnded { .. } => "RuntimeEvent::TransportEnded",
        })
    }
}

/// Each callback must validate the same generation/fence. A late/unknown claim
/// result must refuse sending; it must never be retried as another input effect.
#[async_trait]
pub trait ChatRuntimeSink: Send + Sync {
    /// Optional private memory delivery has no callback allowance. Actual
    /// sinks must try-lock once and skip contention without awaiting/retrying.
    fn try_command_progress(
        &self,
        _binding: &RuntimeBinding,
        _progress: RuntimeCommandProgress,
    ) -> Result<bool, RuntimeError> {
        Ok(false)
    }

    async fn claim_turn(
        &self,
        binding: &RuntimeBinding,
        turn: &TurnRequest,
    ) -> Result<(), RuntimeError>;
    async fn event(
        &self,
        binding: &RuntimeBinding,
        event: RuntimeEvent,
    ) -> Result<(), RuntimeError>;

    /// Default read-only sinks grant no approval capability. Implementations
    /// must preserve this exact process binding on the authoritative writer.
    async fn admit_approval(
        &self,
        _binding: &RuntimeBinding,
        _challenge: ApprovalChallenge,
        _expires_at: time::OffsetDateTime,
    ) -> Result<ChatApproval, RuntimeError> {
        Err(RuntimeError::ClaimRefused)
    }
    async fn decide_approval(
        &self,
        _binding: &RuntimeBinding,
        _request: RuntimeApprovalDecision,
    ) -> Result<ChatApprovalDecisionResult, RuntimeError> {
        Err(RuntimeError::ClaimRefused)
    }
    async fn complete_approval(
        &self,
        _binding: &RuntimeBinding,
        _claim: ChatApprovalDeliveryClaim,
        _outcome: ChatApprovalDelivery,
    ) -> Result<ChatApproval, RuntimeError> {
        Err(RuntimeError::ClaimRefused)
    }
    async fn validate_approval_dispatch(
        &self,
        _binding: &RuntimeBinding,
        _evidence: &crate::chat_writer::ChatApprovalEvidence,
    ) -> Result<(), RuntimeError> {
        Err(RuntimeError::ClaimRefused)
    }
    async fn expire_approval(
        &self,
        _binding: &RuntimeBinding,
        _id: ChatApprovalId,
    ) -> Result<ChatApproval, RuntimeError> {
        Err(RuntimeError::ClaimRefused)
    }
    async fn cancel_turn(
        &self,
        _binding: &RuntimeBinding,
        _turn: TurnSummary,
    ) -> Result<(), RuntimeError> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RuntimeApprovalDecision {
    pub approval_id: ChatApprovalId,
    pub operation_id: Uuid,
    pub expected_version: u64,
    pub decision: ChatApprovalDecision,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum RuntimeError {
    #[error("chat_runtime_invalid_configuration")]
    InvalidConfiguration,
    #[error("chat_runtime_queue_full")]
    QueueFull,
    #[error("chat_runtime_not_ready")]
    NotReady,
    #[error("chat_runtime_wrong_turn")]
    WrongTurn,
    #[error("chat_runtime_stopped")]
    Stopped,
    #[error("chat_runtime_acknowledgement_uncertain")]
    AcknowledgementUncertain,
    #[error("chat_runtime_claim_refused")]
    ClaimRefused,
    #[error("chat_runtime_writer_unavailable")]
    WriterUnavailable,
    #[error("chat_runtime_content_unavailable")]
    ContentUnavailable,
    #[error("chat_runtime_protocol: {0}")]
    Protocol(SessionError),
    #[error("chat_runtime_spawn_failed")]
    SpawnFailed,
    #[error("chat_runtime_input_failed")]
    InputFailed,
}

impl From<SessionError> for RuntimeError {
    fn from(value: SessionError) -> Self {
        Self::Protocol(value)
    }
}

pub(crate) enum RuntimeCommand {
    Send {
        turn: TurnRequest,
        deadline: Duration,
        response: oneshot::Sender<Result<(), RuntimeError>>,
    },
    Cancel {
        turn_id: Uuid,
        response: oneshot::Sender<Result<(), RuntimeError>>,
    },
    DecideApproval {
        request: RuntimeApprovalDecision,
        response: oneshot::Sender<Result<ChatApproval, RuntimeError>>,
    },
}

#[derive(Clone)]
pub struct RuntimeHandle {
    pub(crate) sender: mpsc::Sender<RuntimeCommand>,
    pub(crate) close: watch::Sender<Option<RuntimeStopReason>>,
    pub(crate) snapshots: watch::Receiver<SessionSnapshot>,
    pub(crate) approvals: watch::Receiver<Option<ChatApproval>>,
}

impl RuntimeHandle {
    pub fn pending_approval(&self) -> Option<ChatApproval> {
        self.approvals.borrow().clone()
    }
    pub async fn decide_approval(
        &self,
        request: RuntimeApprovalDecision,
    ) -> Result<ChatApproval, RuntimeError> {
        if request.operation_id.is_nil() || request.expected_version == 0 {
            return Err(RuntimeError::InvalidConfiguration);
        }
        let (response, receiver) = oneshot::channel();
        self.sender
            .try_send(RuntimeCommand::DecideApproval { request, response })
            .map_err(queue_error)?;
        await_command(receiver).await
    }
    #[must_use]
    pub fn snapshot(&self) -> SessionSnapshot {
        self.snapshots.borrow().clone()
    }

    pub async fn send(&self, turn: TurnRequest, deadline: Duration) -> Result<(), RuntimeError> {
        validate_turn(&turn, deadline)?;
        let (response, receiver) = oneshot::channel();
        self.sender
            .try_send(RuntimeCommand::Send {
                turn,
                deadline,
                response,
            })
            .map_err(queue_error)?;
        await_command(receiver).await
    }

    /// Queue one already-admitted durable turn and return promptly. The actor
    /// owns the command independently of this receipt/response receiver.
    pub fn queue_turn(&self, turn: TurnRequest, deadline: Duration) -> Result<(), RuntimeError> {
        validate_turn(&turn, deadline)?;
        let (response, receiver) = oneshot::channel();
        drop(receiver);
        self.sender
            .try_send(RuntimeCommand::Send {
                turn,
                deadline,
                response,
            })
            .map_err(queue_error)
    }

    pub async fn cancel(&self, turn_id: Uuid) -> Result<(), RuntimeError> {
        let (response, receiver) = oneshot::channel();
        self.sender
            .try_send(RuntimeCommand::Cancel { turn_id, response })
            .map_err(queue_error)?;
        await_command(receiver).await
    }

    /// This independent switch remains usable when the bounded inbox is full.
    pub fn close(&self) {
        self.request_stop(RuntimeStopReason::Closed);
    }

    pub(crate) fn request_stop(&self, reason: RuntimeStopReason) {
        self.close.send_if_modified(|requested| {
            if requested.is_some() {
                false
            } else {
                *requested = Some(reason);
                true
            }
        });
    }

    pub async fn wait_ready(&self, bound: Duration) -> Result<SessionSnapshot, RuntimeError> {
        let mut snapshots = self.snapshots.clone();
        tokio::time::timeout(bound, async {
            loop {
                let snapshot = snapshots.borrow().clone();
                if snapshot.phase == vibemux_harness::chat_session::SessionPhase::Ready {
                    return Ok(snapshot);
                }
                if matches!(
                    snapshot.phase,
                    vibemux_harness::chat_session::SessionPhase::Failed
                        | vibemux_harness::chat_session::SessionPhase::Closed
                        | vibemux_harness::chat_session::SessionPhase::Suspended
                        | vibemux_harness::chat_session::SessionPhase::RecoveryRequired
                ) {
                    return Err(RuntimeError::Stopped);
                }
                snapshots
                    .changed()
                    .await
                    .map_err(|_| RuntimeError::Stopped)?;
            }
        })
        .await
        .map_err(|_| RuntimeError::AcknowledgementUncertain)?
    }
}

pub(crate) fn validate_turn(turn: &TurnRequest, deadline: Duration) -> Result<(), RuntimeError> {
    if deadline.is_zero() || tokio::time::Instant::now().checked_add(deadline).is_none() {
        return Err(RuntimeError::InvalidConfiguration);
    }
    if turn.request_id.is_nil() || turn.turn_id.is_nil() || turn.request_id == turn.turn_id {
        return Err(RuntimeError::Protocol(SessionError::InvalidIdentifier));
    }
    if turn.prompt.trim().is_empty()
        || turn.prompt.len() > vibemux_harness::chat_session::MAX_PROMPT_BYTES
    {
        return Err(RuntimeError::Protocol(SessionError::InvalidPrompt));
    }
    Ok(())
}

fn queue_error<T>(error: mpsc::error::TrySendError<T>) -> RuntimeError {
    match error {
        mpsc::error::TrySendError::Full(_) => RuntimeError::QueueFull,
        mpsc::error::TrySendError::Closed(_) => RuntimeError::Stopped,
    }
}

async fn await_command<T>(
    receiver: oneshot::Receiver<Result<T, RuntimeError>>,
) -> Result<T, RuntimeError> {
    tokio::time::timeout(RUNTIME_COMMAND_ACK_LIMIT, receiver)
        .await
        .map_err(|_| RuntimeError::AcknowledgementUncertain)?
        .map_err(|_| RuntimeError::AcknowledgementUncertain)?
}

pub struct RuntimeOwner {
    pub handle: RuntimeHandle,
    pub(crate) task: JoinHandle<RuntimeExit>,
    pub(crate) shutdown_limit: Duration,
}

impl RuntimeOwner {
    pub async fn shutdown(mut self) -> Result<RuntimeExit, RuntimeError> {
        self.handle.request_stop(RuntimeStopReason::Shutdown);
        match tokio::time::timeout(self.shutdown_limit, &mut self.task).await {
            Ok(result) => result.map_err(|_| RuntimeError::Stopped),
            Err(_) => {
                self.task.abort();
                let _ = (&mut self.task).await;
                Err(RuntimeError::AcknowledgementUncertain)
            }
        }
    }
}

impl Drop for RuntimeOwner {
    fn drop(&mut self) {
        self.handle.request_stop(RuntimeStopReason::Shutdown);
    }
}

#[derive(Clone, Debug)]
pub struct RuntimeExit {
    pub reason: RuntimeStopReason,
    pub snapshot: SessionSnapshot,
    pub pending_turn: Option<TurnSummary>,
    pub cleanup: RuntimeCleanup,
    pub writer_confirmed: bool,
}

pub(crate) type Sink = Arc<dyn ChatRuntimeSink>;
