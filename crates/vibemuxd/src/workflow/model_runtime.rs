//! One-shot model-call execution through the single writer and a supplied
//! gateway adapter. This module does not select a provider, inspect AAG
//! permissions, or claim that an upstream call is deterministic or live.

use std::{
    future::pending,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::watch;
use uuid::Uuid;
use vibemux_workflow::{
    SpecIdentifier,
    gateway::{CallCorrelation, CallOutcome, MAX_COMPLETION_BYTES, ModelCallReceipt},
    model_call::{
        ModelCallAdmission, ModelCallError, ModelCallRecord, ModelCallRequest, ModelCallState,
    },
};

use crate::WriterHandle;

use super::aag_gateway::{Aag_Call_Class, Aag_Call_Request, Aag_Model_Gateway};

const CANCELLED_BEFORE_SUBMIT: &str = "model_call_cancelled_before_submit";
const OUTCOME_UNKNOWN: &str = "model_call_outcome_unknown";
const DEADLINE_EXCEEDED: &str = "model_call_deadline_exceeded";

/// A gateway completion carries response text only in process; the receipt
/// itself is content-free and persisted before the text is returned.
pub struct ModelGatewayCompletion {
    pub text: String,
    pub receipt: ModelCallReceipt,
}

/// Content-free failure description. The gateway adapter decides whether a
/// receipt proves a blocked/failed-before-output result or leaves execution
/// uncertain.
pub struct ModelGatewayFailure {
    pub error_code: &'static str,
    pub receipt: Option<ModelCallReceipt>,
}

#[async_trait]
pub trait ModelGateway: Send + Sync {
    /// A content-free eligibility check. Implementations should keep this
    /// local and side-effect free; it runs after reservation but before the
    /// writer records submission.
    async fn preflight(&self, _request: &ModelCallRequest) -> Result<(), ModelGatewayFailure> {
        Ok(())
    }

    async fn complete(
        &self,
        call_id: Uuid,
        attempt: u32,
        request: &ModelCallRequest,
        cancellation: watch::Receiver<bool>,
    ) -> Result<ModelGatewayCompletion, ModelGatewayFailure>;
}

/// Adapter from the generic one-shot runtime to the typed AAG transport.
/// Live model calls require explicit startup policy and matched v2 admission receipts.
/// Fixture execution retains its owned-listener permit. Neither path qualifies native workers.
pub struct AagModelGateway {
    gateway: Aag_Model_Gateway,
    workflow_id: Uuid,
    call_class: Aag_Call_Class,
}

impl std::fmt::Debug for AagModelGateway {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AagModelGateway")
            .field("workflow_id", &self.workflow_id)
            .field("call_class", &self.call_class)
            .finish_non_exhaustive()
    }
}

impl AagModelGateway {
    #[must_use]
    pub const fn new(
        gateway: Aag_Model_Gateway,
        workflow_id: Uuid,
        call_class: Aag_Call_Class,
    ) -> Self {
        Self {
            gateway,
            workflow_id,
            call_class,
        }
    }
}

#[async_trait]
impl ModelGateway for AagModelGateway {
    async fn preflight(&self, request: &ModelCallRequest) -> Result<(), ModelGatewayFailure> {
        let binding = self
            .gateway
            .validate_request(self.call_class, request.config_sha256, request.role)
            .map_err(aag_failure)?;
        if binding.model_alias != request.requested_alias
            || binding.protocol != request.protocol
            || binding.max_output_tokens != request.max_output_tokens
            || binding.timeout_ms != request.timeout_ms
        {
            return Err(ModelGatewayFailure {
                error_code: "gateway_route_changed",
                receipt: None,
            });
        }
        Ok(())
    }

    async fn complete(
        &self,
        call_id: Uuid,
        attempt: u32,
        request: &ModelCallRequest,
        cancellation: watch::Receiver<bool>,
    ) -> Result<ModelGatewayCompletion, ModelGatewayFailure> {
        let identity = request.identity().map_err(|_| ModelGatewayFailure {
            error_code: "model_call_identity_unavailable",
            receipt: None,
        })?;
        let result = self
            .gateway
            .call(
                Aag_Call_Request {
                    class: self.call_class,
                    role: request.role,
                    correlation: CallCorrelation {
                        workflow_id: self.workflow_id,
                        call_id,
                        attempt,
                    },
                    expected_config_digest: request.config_sha256,
                    expected_route_generation: request.observation_digest,
                    system: request.system_prompt.clone(),
                    user: request.user_prompt.clone(),
                    stream: false,
                },
                cancellation,
            )
            .await;
        if result.outcome == CallOutcome::Completed {
            let receipt = result.receipt;
            match (result.completion, receipt) {
                (Some(completion), Some(receipt))
                    if receipt.call_id == call_id
                        && receipt.attempt == attempt
                        && receipt.role == identity.role
                        && receipt.requested_alias == identity.requested_alias
                        && receipt.protocol == identity.protocol
                        && receipt.request_sha256 == identity.request_sha256
                        && identity
                            .observation_digest
                            .is_none_or(|observed| receipt.route_generation == observed)
                        && receipt.response_sha256.is_some() =>
                {
                    return Ok(ModelGatewayCompletion {
                        text: completion.text,
                        receipt,
                    });
                }
                (_, receipt) => {
                    return Err(ModelGatewayFailure {
                        error_code: "gateway_result_invalid",
                        receipt,
                    });
                }
            }
        }
        let error_code = result
            .error
            .map(|error| error.code())
            .unwrap_or("gateway_result_incomplete");
        Err(ModelGatewayFailure {
            error_code,
            receipt: result.receipt,
        })
    }
}

fn aag_failure(error: super::aag_gateway::Aag_Gateway_Error) -> ModelGatewayFailure {
    ModelGatewayFailure {
        error_code: error.code(),
        receipt: None,
    }
}

/// Text is returned to the caller after its content-free receipt is stored.
pub struct ModelCallOutput {
    pub text: String,
    pub receipt: ModelCallReceipt,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ModelCallRuntimeError {
    #[error("model call input or reservation is invalid")]
    ModelCall(#[from] ModelCallError),
    #[error("single writer rejected the model call operation")]
    Writer,
    #[error("single writer task could not be joined")]
    WriterTaskFailed,
    #[error("model call is already submitted or terminal and cannot be replayed")]
    NotReplayable,
    #[error("model call was blocked before submission")]
    Blocked,
    #[error("submitted model call outcome is unknown and cannot be replayed")]
    OutcomeUnknown,
    #[error("model call deadline expired")]
    Deadline,
    #[error("model call was cancelled before submission")]
    Cancelled,
    #[error("model call gateway failed")]
    Gateway,
}

impl ModelCallRuntimeError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::ModelCall(error) => error.code(),
            Self::Writer => "model_call_writer_failed",
            Self::WriterTaskFailed => "model_call_writer_task_failed",
            Self::NotReplayable => "model_call_not_replayable",
            Self::Blocked => "model_call_blocked",
            Self::OutcomeUnknown => "model_call_outcome_unknown",
            Self::Deadline => "model_call_deadline_exceeded",
            Self::Cancelled => "model_call_cancelled",
            Self::Gateway => "model_call_gateway_failed",
        }
    }
}

/// Reserves once, records submission before making one gateway request, and
/// stores terminal evidence before returning any model text. A submitted call
/// is never retried by this function; callers remain responsible for AAG
/// authorization, policy, route health, and evidence admission.
pub async fn run_model_call<G: ModelGateway + ?Sized>(
    writer: &WriterHandle,
    gateway: &G,
    request: ModelCallRequest,
    admission: ModelCallAdmission,
    mut cancellation: watch::Receiver<bool>,
) -> Result<ModelCallOutput, ModelCallRuntimeError> {
    admission.validate()?;
    if request.identity().map_err(|error| match error {
        vibemux_workflow::model_call::ModelCallIdentityError::ContentTooLarge => {
            ModelCallError::ContentTooLarge
        }
        vibemux_workflow::model_call::ModelCallIdentityError::Canonical(_) => {
            ModelCallError::IdentityUnavailable
        }
    })? != admission.identity
    {
        return Err(ModelCallError::IdentityMismatch.into());
    }
    if *cancellation.borrow() {
        return Err(ModelCallRuntimeError::Cancelled);
    }
    if unix_time_ms() >= admission.deadline_unix_ms {
        return Err(ModelCallRuntimeError::Deadline);
    }

    let reservation_writer = writer.clone();
    let reserve_admission = admission.clone();
    let reserved = tokio::task::spawn_blocking(move || {
        reservation_writer.reserve_model_call(reserve_admission)
    })
    .await
    .map_err(|_| ModelCallRuntimeError::WriterTaskFailed)?
    .map_err(|_| ModelCallRuntimeError::Writer)?;
    reserved.validate()?;
    if !reserved.matches_admission(&admission) {
        return Err(ModelCallError::IdentityMismatch.into());
    }
    if reserved.state != ModelCallState::Reserved {
        return Err(ModelCallRuntimeError::NotReplayable);
    }
    if unix_time_ms() >= admission.deadline_unix_ms {
        finish_record(
            writer,
            reserved.call_id,
            None,
            Some(DEADLINE_EXCEEDED.to_string()),
        )
        .await?;
        return Err(ModelCallRuntimeError::Deadline);
    }
    let preflight_remaining = admission.deadline_unix_ms.saturating_sub(unix_time_ms());
    if preflight_remaining == 0 {
        finish_record(
            writer,
            reserved.call_id,
            None,
            Some(DEADLINE_EXCEEDED.to_string()),
        )
        .await?;
        return Err(ModelCallRuntimeError::Deadline);
    }
    let preflight = tokio::time::timeout(
        Duration::from_millis(preflight_remaining),
        gateway.preflight(&request),
    )
    .await;
    let preflight_error = match preflight {
        Ok(Ok(())) => None,
        Ok(Err(failure)) => Some(failure),
        Err(_) => {
            finish_record(
                writer,
                reserved.call_id,
                None,
                Some(DEADLINE_EXCEEDED.to_string()),
            )
            .await?;
            return Err(ModelCallRuntimeError::Deadline);
        }
    };
    if let Some(failure) = preflight_error {
        let blocked = finish_record(
            writer,
            reserved.call_id,
            failure.receipt,
            Some(failure.error_code.to_string()),
        )
        .await?;
        blocked.validate()?;
        if blocked.state != ModelCallState::Blocked {
            return Err(ModelCallRuntimeError::NotReplayable);
        }
        return Err(ModelCallRuntimeError::Blocked);
    }
    if unix_time_ms() >= admission.deadline_unix_ms {
        finish_record(
            writer,
            reserved.call_id,
            None,
            Some(DEADLINE_EXCEEDED.to_string()),
        )
        .await?;
        return Err(ModelCallRuntimeError::Deadline);
    }
    if *cancellation.borrow() {
        finish_record(
            writer,
            reserved.call_id,
            None,
            Some(CANCELLED_BEFORE_SUBMIT.to_string()),
        )
        .await?;
        return Err(ModelCallRuntimeError::Cancelled);
    }

    let call_id = reserved.call_id;
    let submit_writer = writer.clone();
    let submitted = tokio::task::spawn_blocking(move || submit_writer.submit_model_call(call_id))
        .await
        .map_err(|_| ModelCallRuntimeError::WriterTaskFailed)?
        .map_err(|_| ModelCallRuntimeError::Writer)?;
    submitted.validate()?;
    if submitted.call_id != call_id
        || !submitted.matches_admission(&admission)
        || submitted.state != ModelCallState::Submitted
    {
        return Err(ModelCallRuntimeError::NotReplayable);
    }

    let remaining_ms = admission.deadline_unix_ms.saturating_sub(unix_time_ms());
    if remaining_ms == 0 {
        finish_record(writer, call_id, None, Some(OUTCOME_UNKNOWN.to_string())).await?;
        return Err(ModelCallRuntimeError::OutcomeUnknown);
    }
    let timeout = Duration::from_millis(admission.identity.timeout_ms.min(remaining_ms));
    let gateway_cancellation = cancellation.clone();
    let response = tokio::select! {
        biased;
        _ = wait_for_cancellation(&mut cancellation) => {
            finish_record(writer, call_id, None, Some(OUTCOME_UNKNOWN.to_string())).await?;
            return Err(ModelCallRuntimeError::OutcomeUnknown);
        }
        result = tokio::time::timeout(timeout, gateway.complete(
            call_id,
            submitted.calls_used,
            &request,
            gateway_cancellation,
        )) => match result {
            Ok(result) => result,
            Err(_) => {
                finish_record(writer, call_id, None, Some(OUTCOME_UNKNOWN.to_string())).await?;
                return Err(ModelCallRuntimeError::OutcomeUnknown);
            }
        }
    };

    match response {
        Ok(completion) => {
            let mut completed = submitted.clone();
            if completed
                .finish(Some(completion.receipt.clone()), None)
                .is_err()
            {
                finish_record(writer, call_id, None, Some(OUTCOME_UNKNOWN.to_string())).await?;
                return Err(ModelCallRuntimeError::ModelCall(
                    ModelCallError::ReceiptMismatch,
                ));
            }
            if completion.text.len() > MAX_COMPLETION_BYTES {
                finish_record(
                    writer,
                    call_id,
                    Some(completion.receipt),
                    Some("model_call_response_too_large".into()),
                )
                .await?;
                return Err(ModelCallError::ContentTooLarge.into());
            }
            let stored =
                finish_record(writer, call_id, Some(completion.receipt.clone()), None).await?;
            stored.validate()?;
            if stored.state != ModelCallState::Completed {
                return Err(ModelCallRuntimeError::NotReplayable);
            }
            Ok(ModelCallOutput {
                text: completion.text,
                receipt: completion.receipt,
            })
        }
        Err(failure) => {
            let failure_receipt = failure.receipt;
            let failure_code = failure.error_code;
            if let Some(receipt) = failure_receipt
                .as_ref()
                .filter(|receipt| receipt.outcome == CallOutcome::Completed)
            {
                let receipt = receipt.clone();
                let mut completed = submitted.clone();
                if completed.finish(Some(receipt.clone()), None).is_err() {
                    finish_record(writer, call_id, None, Some(OUTCOME_UNKNOWN.to_string())).await?;
                    return Err(ModelCallRuntimeError::ModelCall(
                        ModelCallError::ReceiptMismatch,
                    ));
                }
                finish_record(
                    writer,
                    call_id,
                    Some(receipt),
                    Some("gateway_result_invalid".into()),
                )
                .await?;
                return Err(ModelCallRuntimeError::Gateway);
            }
            let mut expected = submitted;
            let stable_error =
                SpecIdentifier::new(failure_code).map_err(|_| ModelCallRuntimeError::Gateway)?;
            if expected
                .finish(failure_receipt.clone(), Some(stable_error))
                .is_err()
            {
                finish_record(writer, call_id, None, Some(OUTCOME_UNKNOWN.to_string())).await?;
                return Err(ModelCallRuntimeError::OutcomeUnknown);
            }
            let record = finish_record(
                writer,
                call_id,
                failure_receipt,
                Some(failure_code.to_string()),
            )
            .await?;
            record.validate()?;
            if record.admission != expected.admission || record.state != expected.state {
                return Err(ModelCallRuntimeError::NotReplayable);
            }
            match record.state {
                ModelCallState::Blocked => Err(ModelCallRuntimeError::Blocked),
                ModelCallState::Unknown => Err(ModelCallRuntimeError::OutcomeUnknown),
                _ => Err(ModelCallRuntimeError::Gateway),
            }
        }
    }
}

async fn finish_record(
    writer: &WriterHandle,
    call_id: Uuid,
    receipt: Option<ModelCallReceipt>,
    error_code: Option<String>,
) -> Result<ModelCallRecord, ModelCallRuntimeError> {
    let finish_writer = writer.clone();
    tokio::task::spawn_blocking(move || {
        finish_writer.finish_model_call(call_id, receipt, error_code)
    })
    .await
    .map_err(|_| ModelCallRuntimeError::WriterTaskFailed)?
    .map_err(|_| ModelCallRuntimeError::Writer)
}

async fn wait_for_cancellation(cancellation: &mut watch::Receiver<bool>) {
    loop {
        if *cancellation.borrow() {
            return;
        }
        if cancellation.changed().await.is_err() {
            pending::<()>().await;
        }
    }
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(u64::MAX)
}
