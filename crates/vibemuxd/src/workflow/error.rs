//! Stable, content-free failure codes of the workflow service (ADR 031).
//!
//! No variant carries a path, prompt, request text, or vendor output; the
//! Control surface returns only the code.

use thiserror::Error;

use crate::{WriterError, harness_dispatch::DispatchServiceError};

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum WorkflowError {
    #[error("workflow service is not configured")]
    Unconfigured,
    #[error("workflow configuration is invalid")]
    ConfigInvalid,
    #[error("workflow executable or verifier is not trusted")]
    Untrusted,
    #[error("trusted verifier files changed since the daemon started")]
    VerifierChanged,
    #[error("workflow request is invalid")]
    RequestInvalid,
    #[error("request key is already bound to a different workflow plan")]
    RequestConflict,
    #[error("operator policy is invalid")]
    PolicyInvalid,
    #[error("no compiler route is available for an uncompiled request")]
    CompilerUnavailable,
    /// Content-free violation codes of the compiled candidates.
    #[error("compiled contract was rejected")]
    ContractRejected { codes: Vec<String> },
    #[error("a planned slot is not configured or cannot execute")]
    SlotUnavailable,
    #[error("workflow is not known")]
    NotFound,
    #[error("contract digest does not match the prepared workflow")]
    ContractMismatch,
    #[error("workflow is not in a phase that allows this operation")]
    InvalidPhase,
    #[error("workflow already runs in this daemon")]
    AlreadyRunning,
    #[error("Git operation failed")]
    Git,
    #[error("candidate snapshot could not be collected")]
    Snapshot,
    #[error("owned workspace operation failed")]
    Workspace,
    #[error("verifier could not run")]
    Verifier,
    #[error("model gateway call failed")]
    Gateway,
    #[error("context share is invalid")]
    ShareInvalid,
    #[error("session is not known")]
    SessionNotFound,
    #[error("native terminal sessions are not available")]
    NativeTuiUnavailable,
    #[error("prompt evaluation input is invalid")]
    EvaluationInvalid,
    /// A bounded optimizer refusal, such as an exhausted budget or a
    /// holdout that was already consulted.
    #[error("{code}")]
    Optimizer { code: &'static str },
    #[error("workflow service is shutting down")]
    ShuttingDown,
    #[error("internal workflow error")]
    Internal,
    #[error("{code}")]
    Dispatch { code: String },
    #[error("{code}")]
    Writer { code: String },
}

impl WorkflowError {
    #[must_use]
    pub fn code(&self) -> &str {
        match self {
            Self::Unconfigured => "workflow_unconfigured",
            Self::ConfigInvalid => "workflow_config_invalid",
            Self::Untrusted => "workflow_untrusted_executable",
            Self::VerifierChanged => "workflow_verifier_changed",
            Self::RequestInvalid => "workflow_request_invalid",
            Self::RequestConflict => "workflow_request_conflict",
            Self::PolicyInvalid => "workflow_policy_invalid",
            Self::CompilerUnavailable => "workflow_compiler_unavailable",
            Self::ContractRejected { .. } => "workflow_contract_rejected",
            Self::SlotUnavailable => "workflow_slot_unavailable",
            Self::NotFound => "workflow_not_found",
            Self::ContractMismatch => "workflow_contract_mismatch",
            Self::InvalidPhase => "workflow_invalid_phase",
            Self::AlreadyRunning => "workflow_already_running",
            Self::Git => "workflow_git_failed",
            Self::Snapshot => "workflow_snapshot_failed",
            Self::Workspace => "workflow_workspace_failed",
            Self::Verifier => "workflow_verifier_failed",
            Self::Gateway => "workflow_gateway_failed",
            Self::ShareInvalid => "workflow_share_invalid",
            Self::SessionNotFound => "workflow_session_not_found",
            Self::NativeTuiUnavailable => "workflow_native_tui_unavailable",
            Self::EvaluationInvalid => "workflow_evaluation_invalid",
            Self::Optimizer { code } => code,
            Self::ShuttingDown => "workflow_shutting_down",
            Self::Internal => "workflow_internal",
            Self::Dispatch { code } | Self::Writer { code } => code,
        }
    }
}

impl From<WriterError> for WorkflowError {
    fn from(error: WriterError) -> Self {
        Self::Writer {
            code: error.code().to_string(),
        }
    }
}

impl From<DispatchServiceError> for WorkflowError {
    fn from(error: DispatchServiceError) -> Self {
        Self::Dispatch {
            code: error.code().to_string(),
        }
    }
}
