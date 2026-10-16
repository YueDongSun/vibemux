//! Content-free admission and recovery records for effectful model calls.
//!
//! Request and response text remains transient. Persisted identity consists
//! only of digests and bounded route facts, so submitted calls can be fenced
//! after recovery without replaying a request whose upstream outcome is
//! unknown.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::{
    Sha256Digest, SpecIdentifier,
    canonical_json::{CanonicalError, canonical_digest},
    gateway::{CallOutcome, GatewayProtocol, ModelCallReceipt, RouteRole},
    supervisor::{ProposalRejection, SupervisorProposal, parse_proposal},
    task_spec::TaskSpec,
    workflow_record::MAX_WORKFLOW_TASKS,
};

pub const MODEL_CALL_FINGERPRINT_DOMAIN: &str = "vibemux.workflow.model_call_fingerprint.v1";
pub const MAX_MODEL_CALL_INPUT_BYTES: usize = 256 * 1024;
pub const MAX_MODEL_CALLS_PER_OWNER: u32 = 64;
pub const MAX_TASK_CANDIDATE_RESPONSE_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelCallState {
    Reserved,
    Submitted,
    Completed,
    /// Upstream completed, but its output was rejected before use.
    Rejected,
    Blocked,
    Unknown,
}

/// Identity for a single role-bound request; prompts are represented by a
/// digest and are never included in a persisted admission.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelCallIdentity {
    /// Hash of the exact serialized gateway request body.
    pub request_sha256: Sha256Digest,
    /// Hash of the full local request, config, route, and role identity.
    pub request_fingerprint: Sha256Digest,
    pub config_sha256: Sha256Digest,
    pub observation_digest: Option<Sha256Digest>,
    pub role: RouteRole,
    pub requested_alias: String,
    pub protocol: GatewayProtocol,
    pub max_output_tokens: u32,
    pub timeout_ms: u64,
}

/// Stable, content-free instructions to the single writer to reserve one
/// model-call attempt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelCallAdmission {
    /// Stable identity of this one call request. Reuse with different
    /// admission data is an identity conflict.
    pub call_id: Uuid,
    pub owner_key: SpecIdentifier,
    pub workflow_id: Option<Uuid>,
    pub identity: ModelCallIdentity,
    pub deadline_unix_ms: u64,
    pub max_calls: u32,
}

impl ModelCallAdmission {
    pub fn validate(&self) -> Result<(), ModelCallError> {
        if self.deadline_unix_ms == 0
            || !(1..=MAX_MODEL_CALLS_PER_OWNER).contains(&self.max_calls)
            || !self.identity.protocol.adapter_supported()
            || self.identity.requested_alias.is_empty()
            || self.identity.requested_alias.len() > crate::gateway::MAX_ALIAS_BYTES
            || self
                .identity
                .requested_alias
                .chars()
                .any(|character| character.is_whitespace() || character.is_control())
            || self.identity.max_output_tokens == 0
            || self.identity.max_output_tokens > crate::gateway::MAX_OUTPUT_TOKENS
            || !(crate::gateway::MIN_TIMEOUT_MS..=crate::gateway::MAX_TIMEOUT_MS)
                .contains(&self.identity.timeout_ms)
        {
            return Err(ModelCallError::InvalidAdmission);
        }
        Ok(())
    }
}

/// Persisted state for one reserved or attempted call. It contains no prompt
/// text or model response text.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelCallRecord {
    pub call_id: Uuid,
    pub admission: ModelCallAdmission,
    pub state: ModelCallState,
    pub calls_used: u32,
    pub receipt: Option<ModelCallReceipt>,
    pub error_code: Option<SpecIdentifier>,
}

impl ModelCallRecord {
    pub fn reserved(
        admission: ModelCallAdmission,
        calls_used: u32,
    ) -> Result<Self, ModelCallError> {
        admission.validate()?;
        if calls_used == 0 || calls_used > admission.max_calls {
            return Err(ModelCallError::CallLimitExceeded);
        }
        Ok(Self {
            call_id: admission.call_id,
            admission,
            state: ModelCallState::Reserved,
            calls_used,
            receipt: None,
            error_code: None,
        })
    }

    /// A duplicate reservation may continue only while the writer still
    /// reports `reserved`. Submitted calls are never replayed.
    pub fn matches_admission(&self, admission: &ModelCallAdmission) -> bool {
        self.admission == *admission
    }

    pub fn validate(&self) -> Result<(), ModelCallError> {
        self.admission.validate()?;
        if self.call_id != self.admission.call_id || self.call_id.is_nil() {
            return Err(ModelCallError::IdentityMismatch);
        }
        if self.calls_used == 0 || self.calls_used > self.admission.max_calls {
            return Err(ModelCallError::CallLimitExceeded);
        }
        let valid_terminal = match self.state {
            ModelCallState::Reserved | ModelCallState::Submitted => {
                self.receipt.is_none() && self.error_code.is_none()
            }
            ModelCallState::Completed => {
                self.error_code.is_none()
                    && self.receipt.as_ref().is_some_and(|receipt| {
                        receipt.outcome == CallOutcome::Completed
                            && receipt.response_sha256.is_some()
                    })
            }
            ModelCallState::Rejected => {
                self.error_code.as_ref().is_some_and(|code| {
                    matches!(
                        code.as_str(),
                        "model_call_response_too_large" | "gateway_result_invalid"
                    )
                }) && self.receipt.as_ref().is_some_and(|receipt| {
                    receipt.outcome == CallOutcome::Completed && receipt.response_sha256.is_some()
                })
            }
            ModelCallState::Blocked => {
                self.error_code.is_some()
                    && self.receipt.as_ref().is_none_or(|receipt| {
                        matches!(
                            receipt.outcome,
                            CallOutcome::Blocked | CallOutcome::FailedBeforeOutput
                        )
                    })
            }
            ModelCallState::Unknown => {
                self.error_code.is_some()
                    && self
                        .receipt
                        .as_ref()
                        .is_none_or(|receipt| receipt.outcome == CallOutcome::FailedAfterOutput)
            }
        };
        if !valid_terminal {
            return Err(ModelCallError::InvalidFinish);
        }
        if let Some(receipt) = &self.receipt {
            validate_receipt(self, receipt)?;
        }
        Ok(())
    }

    pub fn mark_submitted(&mut self) -> Result<(), ModelCallError> {
        if self.state != ModelCallState::Reserved {
            return Err(ModelCallError::NotReservable);
        }
        self.state = ModelCallState::Submitted;
        Ok(())
    }

    /// Finishes a submitted attempt from content-free receipt evidence. A
    /// missing receipt or a post-output failure leaves the outcome unknown.
    pub fn finish(
        &mut self,
        receipt: Option<ModelCallReceipt>,
        error_code: Option<SpecIdentifier>,
    ) -> Result<(), ModelCallError> {
        if self.state == ModelCallState::Reserved {
            if receipt.is_some() || error_code.is_none() {
                return Err(ModelCallError::InvalidFinish);
            }
            self.error_code = error_code;
            self.state = ModelCallState::Blocked;
            return Ok(());
        }
        if self.state != ModelCallState::Submitted {
            return Err(ModelCallError::NotSubmitted);
        }
        if let Some(receipt) = &receipt {
            validate_receipt(self, receipt)?;
        }
        let state = match receipt.as_ref().map(|receipt| receipt.outcome) {
            Some(CallOutcome::Completed)
                if error_code.as_ref().is_some_and(|code| {
                    matches!(
                        code.as_str(),
                        "model_call_response_too_large" | "gateway_result_invalid"
                    )
                }) && receipt
                    .as_ref()
                    .is_some_and(|receipt| receipt.response_sha256.is_some()) =>
            {
                ModelCallState::Rejected
            }
            Some(CallOutcome::Completed)
                if error_code.is_none()
                    && receipt
                        .as_ref()
                        .is_some_and(|receipt| receipt.response_sha256.is_some()) =>
            {
                ModelCallState::Completed
            }
            Some(CallOutcome::Blocked | CallOutcome::FailedBeforeOutput)
                if error_code.is_some() =>
            {
                ModelCallState::Blocked
            }
            Some(CallOutcome::FailedAfterOutput) if error_code.is_some() => ModelCallState::Unknown,
            None if error_code.is_some() => ModelCallState::Unknown,
            _ => return Err(ModelCallError::InvalidFinish),
        };
        self.receipt = receipt;
        self.error_code = error_code;
        self.state = state;
        Ok(())
    }

    /// Recovery converts submitted-without-terminal-evidence to `unknown`.
    /// The record cannot return to `reserved` or be submitted again.
    pub fn recover_after_restart(&mut self) -> Result<bool, ModelCallError> {
        if self.state != ModelCallState::Submitted {
            return Ok(false);
        }
        self.state = ModelCallState::Unknown;
        self.error_code = Some(
            SpecIdentifier::new("model_call_outcome_unknown")
                .map_err(|_| ModelCallError::InvalidAdmission)?,
        );
        Ok(true)
    }
}

fn validate_receipt(
    record: &ModelCallRecord,
    receipt: &ModelCallReceipt,
) -> Result<(), ModelCallError> {
    let identity = &record.admission.identity;
    if receipt.call_id != record.call_id
        || receipt.role != identity.role
        || receipt.requested_alias != identity.requested_alias
        || receipt.protocol != identity.protocol
        || receipt.request_sha256 != identity.request_sha256
        || identity
            .observation_digest
            .is_some_and(|observation| receipt.route_generation != observation)
        || receipt.attempt != record.calls_used
    {
        return Err(ModelCallError::ReceiptMismatch);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Error, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelCallError {
    #[error("model call admission is invalid")]
    InvalidAdmission,
    #[error("model call request identity does not match its admission")]
    IdentityMismatch,
    #[error("model call attempt limit is exhausted")]
    CallLimitExceeded,
    #[error("model call is not in a reservable state")]
    NotReservable,
    #[error("model call has not been submitted")]
    NotSubmitted,
    #[error("model call finish evidence is inconsistent")]
    InvalidFinish,
    #[error("model call receipt does not match the reservation")]
    ReceiptMismatch,
    #[error("model call content exceeds its configured bound")]
    ContentTooLarge,
    #[error("model output is not a bounded task-candidate response")]
    CandidateResponseInvalid,
    #[error("model output is not a typed supervisor proposal")]
    SupervisorProposalInvalid,
    #[error("canonical request identity could not be calculated")]
    IdentityUnavailable,
}

impl ModelCallError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidAdmission => "model_call_admission_invalid",
            Self::IdentityMismatch => "model_call_identity_mismatch",
            Self::CallLimitExceeded => "model_call_limit_exceeded",
            Self::NotReservable => "model_call_not_reservable",
            Self::NotSubmitted => "model_call_not_submitted",
            Self::InvalidFinish => "model_call_finish_invalid",
            Self::ReceiptMismatch => "model_call_receipt_mismatch",
            Self::ContentTooLarge => "model_call_content_too_large",
            Self::CandidateResponseInvalid => "model_candidate_response_invalid",
            Self::SupervisorProposalInvalid => "supervisor_proposal_invalid",
            Self::IdentityUnavailable => "model_call_identity_unavailable",
        }
    }
}

/// Strict result shape for the non-deterministic task-compiler role. Passing
/// this parser establishes typed input only; prepare remains the final scope
/// and source-coverage authority.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskCandidateResponse {
    task_specs: Vec<Value>,
}

pub fn compile_task_candidates(result_text: &str) -> Result<Vec<TaskSpec>, ModelCallError> {
    if result_text.len() > MAX_TASK_CANDIDATE_RESPONSE_BYTES {
        return Err(ModelCallError::ContentTooLarge);
    }
    let response: TaskCandidateResponse =
        serde_json::from_str(result_text).map_err(|_| ModelCallError::CandidateResponseInvalid)?;
    if response.task_specs.is_empty() || response.task_specs.len() > MAX_WORKFLOW_TASKS {
        return Err(ModelCallError::CandidateResponseInvalid);
    }
    response
        .task_specs
        .into_iter()
        .map(|candidate| {
            let bytes = serde_json::to_vec(&candidate)
                .map_err(|_| ModelCallError::CandidateResponseInvalid)?;
            TaskSpec::parse_candidate(&bytes).map_err(|_| ModelCallError::CandidateResponseInvalid)
        })
        .collect()
}

/// Parses only the existing typed supervisor proposal. This function grants
/// no authority to execute actions; the caller must validate it against
/// committed facts before applying any decision.
pub fn parse_supervisor_proposal(
    result_text: &str,
) -> Result<SupervisorProposal, ProposalRejection> {
    parse_proposal(result_text.as_bytes())
}

#[derive(Debug, Error)]
pub enum ModelCallIdentityError {
    #[error("model call request content exceeds its size limit")]
    ContentTooLarge,
    #[error("model call request identity could not be calculated")]
    Canonical(#[from] CanonicalError),
}

#[derive(Clone)]
pub struct ModelCallRequest {
    pub role: RouteRole,
    pub requested_alias: String,
    pub protocol: GatewayProtocol,
    pub max_output_tokens: u32,
    pub timeout_ms: u64,
    pub config_sha256: Sha256Digest,
    pub observation_digest: Option<Sha256Digest>,
    pub system_prompt: String,
    pub user_prompt: String,
}

impl std::fmt::Debug for ModelCallRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ModelCallRequest")
            .field("role", &self.role)
            .field("requested_alias", &self.requested_alias)
            .field("protocol", &self.protocol)
            .field("system_prompt", &"[redacted]")
            .field("user_prompt", &"[redacted]")
            .finish_non_exhaustive()
    }
}

impl ModelCallRequest {
    pub fn identity(&self) -> Result<ModelCallIdentity, ModelCallIdentityError> {
        let content_bytes = self
            .system_prompt
            .len()
            .saturating_add(self.user_prompt.len());
        if content_bytes > MAX_MODEL_CALL_INPUT_BYTES {
            return Err(ModelCallIdentityError::ContentTooLarge);
        }
        #[derive(Serialize)]
        struct RequestFingerprint<'a> {
            role: RouteRole,
            requested_alias: &'a str,
            protocol: GatewayProtocol,
            max_output_tokens: u32,
            timeout_ms: u64,
            config_sha256: Sha256Digest,
            observation_digest: Option<Sha256Digest>,
            system_prompt: &'a str,
            user_prompt: &'a str,
        }
        let fingerprint = RequestFingerprint {
            role: self.role,
            requested_alias: &self.requested_alias,
            protocol: self.protocol,
            max_output_tokens: self.max_output_tokens,
            timeout_ms: self.timeout_ms,
            config_sha256: self.config_sha256,
            observation_digest: self.observation_digest,
            system_prompt: &self.system_prompt,
            user_prompt: &self.user_prompt,
        };
        let request_fingerprint = canonical_digest(MODEL_CALL_FINGERPRINT_DOMAIN, &fingerprint)
            .map_err(ModelCallIdentityError::Canonical)?;
        let binding = crate::gateway::RouteBinding {
            role: self.role,
            model_alias: self.requested_alias.clone(),
            protocol: self.protocol,
            max_output_tokens: self.max_output_tokens,
            timeout_ms: self.timeout_ms,
        };
        let body = crate::gateway::request_body(&binding, &self.system_prompt, &self.user_prompt);
        let body_bytes = serde_json::to_vec(&body)
            .map_err(|_| ModelCallIdentityError::Canonical(CanonicalError::Unserializable))?;
        let request_sha256 = Sha256Digest::of(&body_bytes);
        Ok(ModelCallIdentity {
            request_sha256,
            request_fingerprint,
            config_sha256: self.config_sha256,
            observation_digest: self.observation_digest,
            role: self.role,
            requested_alias: self.requested_alias.clone(),
            protocol: self.protocol,
            max_output_tokens: self.max_output_tokens,
            timeout_ms: self.timeout_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{gateway::ProviderAttribution, task_spec::fixtures::track_a_spec};

    fn request(system_prompt: &str, user_prompt: &str) -> ModelCallRequest {
        ModelCallRequest {
            role: RouteRole::Compiler,
            requested_alias: "compiler_model".to_string(),
            protocol: GatewayProtocol::OpenaiChatCompletions,
            max_output_tokens: 1024,
            timeout_ms: 10_000,
            config_sha256: Sha256Digest::of(b"config"),
            observation_digest: Some(Sha256Digest::of(b"route")),
            system_prompt: system_prompt.to_string(),
            user_prompt: user_prompt.to_string(),
        }
    }

    fn admission(request: &ModelCallRequest) -> ModelCallAdmission {
        ModelCallAdmission {
            call_id: Uuid::from_u128(10),
            owner_key: SpecIdentifier::new("compile_request_1").expect("owner key"),
            workflow_id: Some(Uuid::from_u128(1)),
            identity: request.identity().expect("identity"),
            deadline_unix_ms: 10_000,
            max_calls: 2,
        }
    }

    #[test]
    fn changed_payload_conflicts_with_the_stable_owner_admission() {
        let first = request("system", "task one");
        let changed = request("system", "task two");
        let reserved = ModelCallRecord::reserved(admission(&first), 1).expect("reservation");
        assert!(reserved.matches_admission(&admission(&first)));
        assert!(!reserved.matches_admission(&admission(&changed)));
        assert_ne!(
            first.identity().expect("first identity").request_sha256,
            changed.identity().expect("changed identity").request_sha256
        );
        assert_ne!(
            first
                .identity()
                .expect("first identity")
                .request_fingerprint,
            changed
                .identity()
                .expect("changed identity")
                .request_fingerprint
        );
        assert!(!format!("{first:?}").contains("task one"));
    }

    #[test]
    fn submitted_recovery_is_unknown_and_not_reservable() {
        let request = request("system", "task");
        let mut record = ModelCallRecord::reserved(admission(&request), 1).expect("reservation");
        record.mark_submitted().expect("submitted");
        assert!(
            record
                .recover_after_restart()
                .expect("recovery marked unknown")
        );
        assert_eq!(record.state, ModelCallState::Unknown);
        assert_eq!(
            record.error_code.as_ref().map(SpecIdentifier::as_str),
            Some("model_call_outcome_unknown")
        );
        assert_eq!(record.mark_submitted(), Err(ModelCallError::NotReservable));
        assert!(!record.recover_after_restart().expect("already recovered"));
    }

    #[test]
    fn completed_call_requires_a_matching_content_free_receipt() {
        let request = request("system", "task");
        let mut record = ModelCallRecord::reserved(admission(&request), 1).expect("reservation");
        record.mark_submitted().expect("submitted");
        record
            .finish(
                Some(ModelCallReceipt {
                    call_id: record.call_id,
                    role: request.role,
                    requested_alias: request.requested_alias.clone(),
                    protocol: request.protocol,
                    reported_model: None,
                    effective_provider: ProviderAttribution::Unknown,
                    usage: Default::default(),
                    http_status: Some(200),
                    outcome: CallOutcome::Completed,
                    failure: None,
                    elapsed_ms: 12,
                    request_sha256: record.admission.identity.request_sha256,
                    response_sha256: Some(Sha256Digest::of(b"response")),
                    route_generation: request
                        .observation_digest
                        .unwrap_or_else(|| Sha256Digest::of(b"observed route")),
                    attempt: 1,
                }),
                None,
            )
            .expect("completion");
        assert_eq!(record.state, ModelCallState::Completed);
        record.validate().expect("completed record is valid");
    }

    #[test]
    fn unknown_route_observation_stays_optional_but_receipt_keeps_actual_observation() {
        let mut request = request("system", "task");
        request.observation_digest = None;
        let mut record = ModelCallRecord::reserved(admission(&request), 1).expect("reservation");
        record.mark_submitted().expect("submitted");
        let actual_observation = Sha256Digest::of(b"observed at call time");
        record
            .finish(
                Some(ModelCallReceipt {
                    call_id: record.call_id,
                    role: request.role,
                    requested_alias: request.requested_alias.clone(),
                    protocol: request.protocol,
                    reported_model: None,
                    effective_provider: ProviderAttribution::Unknown,
                    usage: Default::default(),
                    http_status: Some(200),
                    outcome: CallOutcome::Completed,
                    failure: None,
                    elapsed_ms: 12,
                    request_sha256: record.admission.identity.request_sha256,
                    response_sha256: Some(Sha256Digest::of(b"response")),
                    route_generation: actual_observation,
                    attempt: 1,
                }),
                None,
            )
            .expect("completion keeps its observed route");
        assert_eq!(record.admission.identity.observation_digest, None);
        assert_eq!(
            record
                .receipt
                .as_ref()
                .map(|receipt| receipt.route_generation),
            Some(actual_observation)
        );
    }

    #[test]
    fn compiler_result_is_strict_json_and_parses_each_candidate_through_task_spec() {
        let task_spec = track_a_spec();
        let valid_result =
            serde_json::to_string(&json!({"task_specs": [task_spec]})).expect("candidate json");
        let parsed = compile_task_candidates(&valid_result).expect("typed task candidates");
        assert_eq!(parsed, vec![track_a_spec()]);

        let result_text = serde_json::to_string(&json!({"task_specs": []})).expect("json");
        assert_eq!(
            compile_task_candidates(&result_text),
            Err(ModelCallError::CandidateResponseInvalid)
        );
        assert_eq!(
            compile_task_candidates("Here is the plan: {\"task_specs\": []}"),
            Err(ModelCallError::CandidateResponseInvalid)
        );
        assert_eq!(
            compile_task_candidates("{\"task_specs\": [], \"extra\": true}"),
            Err(ModelCallError::CandidateResponseInvalid)
        );
    }

    #[test]
    fn supervisor_parser_returns_only_the_existing_typed_proposal() {
        let proposal = parse_supervisor_proposal(
            r#"{"decisions":[{"idempotency_key":"inspect_1","expected_workflow_version":1,"action":{"tool":"inspect_workflow","arguments":{}}}],"rationale":"inspect current state","evidence_refs":[]}"#,
        )
        .expect("typed proposal");
        assert_eq!(proposal.decisions.len(), 1);
        assert_eq!(
            parse_supervisor_proposal("run this shell command"),
            Err(ProposalRejection::Malformed)
        );
    }
}
