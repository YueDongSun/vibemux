//! Fixed decision error codes. Every code starts with `decision_` so it
//! reaches Control IPC unchanged; none carries free text.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Failures of a decision request or of binding a decision to a dispatch.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Error, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionError {
    #[error("decision routing is not configured")]
    NotConfigured,
    #[error("no harness has an executable route and a verified probe")]
    NoEligibleHarness,
    #[error("a label is not in the decision config allowlist")]
    UnknownLabel,
    #[error("the decision request is invalid")]
    InvalidRequest,
    #[error("the decision does not exist")]
    NotFound,
    #[error("the decision escalated to the operator and names no harness")]
    Escalated,
    #[error("the decision has expired")]
    Expired,
    #[error("the decision was already used by another dispatch request")]
    AlreadyConsumed,
    #[error("the dispatch harness differs from the decided harness")]
    HarnessMismatch,
    #[error("the dispatch prompt differs from the decided prompt")]
    PromptMismatch,
    #[error("the dispatch configuration changed after the decision")]
    ConfigChanged,
    #[error("the decision record is inconsistent")]
    Internal,
}

impl DecisionError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::NotConfigured => "decision_not_configured",
            Self::NoEligibleHarness => "decision_no_eligible_harness",
            Self::UnknownLabel => "decision_unknown_label",
            Self::InvalidRequest => "decision_invalid_request",
            Self::NotFound => "decision_not_found",
            Self::Escalated => "decision_escalated",
            Self::Expired => "decision_expired",
            Self::AlreadyConsumed => "decision_already_consumed",
            Self::HarnessMismatch => "decision_harness_mismatch",
            Self::PromptMismatch => "decision_prompt_mismatch",
            Self::ConfigChanged => "decision_config_changed",
            Self::Internal => "decision_internal",
        }
    }
}

/// Why an external backend produced no usable answer. Recorded as evidence;
/// the decision then falls back to the local chain.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Error, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalError {
    // Daemon-side peer failures.
    #[error("the configured credential variable is missing or empty")]
    CredentialUnavailable,
    #[error("the decision peer binary is unavailable")]
    PeerUnavailable,
    #[error("another external decision is in progress")]
    PeerBusy,
    #[error("the decision peer exceeded its deadline")]
    PeerTimeout,
    #[error("the decision peer broke its line protocol")]
    PeerProtocol,
    // Backend transport failures reported by the peer.
    #[error("the backend request is invalid")]
    RequestInvalid,
    #[error("the backend could not be reached")]
    Transport,
    #[error("the backend timed out")]
    BackendTimeout,
    #[error("the backend kept rate limiting or reporting overload")]
    RateLimited,
    #[error("the backend rejected the credential")]
    Unauthorized,
    #[error("the backend returned an unexpected HTTP status")]
    HttpStatus,
    #[error("the backend tried to redirect")]
    RedirectRefused,
    #[error("the backend response exceeded its size limit")]
    ResponseTooLarge,
    #[error("the backend response is malformed")]
    ResponseMalformed,
    // Answer validation failures (ADR 034 §8).
    #[error("the answer came from a model other than the pinned one")]
    AnswerModelMismatch,
    #[error("the answer chose an option outside the question")]
    AnswerUnknownChoice,
    #[error("the answer probabilities do not cover exactly the options")]
    AnswerProbabilityCoverage,
    #[error("an answer probability is not finite or outside [0, 1]")]
    AnswerProbabilityRange,
    #[error("the answer probabilities do not sum to one")]
    AnswerProbabilitySum,
    #[error("the answer choice is not the unique most probable option")]
    AnswerNotArgmax,
    #[error("the answer confidence is not finite or outside [0, 1]")]
    AnswerConfidenceRange,
}

impl ExternalError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::CredentialUnavailable => "decision_credential_unavailable",
            Self::PeerUnavailable => "decision_peer_unavailable",
            Self::PeerBusy => "decision_peer_busy",
            Self::PeerTimeout => "decision_peer_timeout",
            Self::PeerProtocol => "decision_peer_protocol",
            Self::RequestInvalid => "decision_backend_request_invalid",
            Self::Transport => "decision_backend_transport",
            Self::BackendTimeout => "decision_backend_timeout",
            Self::RateLimited => "decision_backend_rate_limited",
            Self::Unauthorized => "decision_backend_unauthorized",
            Self::HttpStatus => "decision_backend_http_status",
            Self::RedirectRefused => "decision_backend_redirect_refused",
            Self::ResponseTooLarge => "decision_backend_response_too_large",
            Self::ResponseMalformed => "decision_backend_response_malformed",
            Self::AnswerModelMismatch => "decision_answer_model_mismatch",
            Self::AnswerUnknownChoice => "decision_answer_unknown_choice",
            Self::AnswerProbabilityCoverage => "decision_answer_probability_coverage",
            Self::AnswerProbabilityRange => "decision_answer_probability_range",
            Self::AnswerProbabilitySum => "decision_answer_probability_sum",
            Self::AnswerNotArgmax => "decision_answer_not_argmax",
            Self::AnswerConfidenceRange => "decision_answer_confidence_range",
        }
    }

    /// True when a backend answered but the answer failed validation.
    #[must_use]
    pub const fn is_answer_rejection(self) -> bool {
        matches!(
            self,
            Self::AnswerModelMismatch
                | Self::AnswerUnknownChoice
                | Self::AnswerProbabilityCoverage
                | Self::AnswerProbabilityRange
                | Self::AnswerProbabilitySum
                | Self::AnswerNotArgmax
                | Self::AnswerConfidenceRange
        )
    }

    /// Every variant, for exhaustive code tests.
    #[must_use]
    pub const fn all() -> [Self; 21] {
        [
            Self::CredentialUnavailable,
            Self::PeerUnavailable,
            Self::PeerBusy,
            Self::PeerTimeout,
            Self::PeerProtocol,
            Self::RequestInvalid,
            Self::Transport,
            Self::BackendTimeout,
            Self::RateLimited,
            Self::Unauthorized,
            Self::HttpStatus,
            Self::RedirectRefused,
            Self::ResponseTooLarge,
            Self::ResponseMalformed,
            Self::AnswerModelMismatch,
            Self::AnswerUnknownChoice,
            Self::AnswerProbabilityCoverage,
            Self::AnswerProbabilityRange,
            Self::AnswerProbabilitySum,
            Self::AnswerNotArgmax,
            Self::AnswerConfidenceRange,
        ]
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn external_codes_are_unique_prefixed_and_match_serde() {
        let mut codes = BTreeSet::new();
        for error in ExternalError::all() {
            let code = error.code();
            assert!(code.starts_with("decision_"), "{code}");
            assert!(codes.insert(code), "duplicate {code}");
            let wire = serde_json::to_string(&error).unwrap();
            let back: ExternalError = serde_json::from_str(&wire).unwrap();
            assert_eq!(back, error);
        }
    }

    #[test]
    fn decision_codes_are_prefixed() {
        for error in [
            DecisionError::NotConfigured,
            DecisionError::NoEligibleHarness,
            DecisionError::UnknownLabel,
            DecisionError::InvalidRequest,
            DecisionError::NotFound,
            DecisionError::Escalated,
            DecisionError::Expired,
            DecisionError::AlreadyConsumed,
            DecisionError::HarnessMismatch,
            DecisionError::PromptMismatch,
            DecisionError::ConfigChanged,
            DecisionError::Internal,
        ] {
            assert!(error.code().starts_with("decision_"));
        }
    }

    #[test]
    fn only_answer_failures_are_rejections() {
        assert!(ExternalError::AnswerNotArgmax.is_answer_rejection());
        assert!(!ExternalError::Transport.is_answer_rejection());
        assert!(!ExternalError::CredentialUnavailable.is_answer_rejection());
    }
}
