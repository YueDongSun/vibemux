//! DeepSeek Harness (DSH) session substrate: pure contracts and state machines
//! for driving one long-lived SDK runtime over newline-delimited JSON-RPC 2.0
//! stdio (ADR 030).
//!
//! Everything here is deterministic logic over values supplied by the caller.
//! No process, pipe, filesystem, clock, or network access happens in this
//! module tree; `vibemuxd` boots the runtime, frames stdout, moves bytes,
//! enforces deadlines, and terminates the contained process tree, applying
//! these rules at each step:
//!
//! - [`wire`] names the observed method set and decodes frames strictly;
//! - [`session`] drives the client side of one session: handshake, prompt
//!   admission, notification correlation, and teardown;
//! - [`records`] keeps a bounded, monotonically sequenced buffer of byte-exact
//!   runtime records for cursor-based polling.
//!
//! Two rules from AGENTS.md shape the design. Idle is not completion
//! (invariant 5), so only a correlated `turn/end` event is terminal evidence.
//! Vendor payloads do not become internal state without explicit mapping
//! (invariant 16), so runtime records stay opaque and byte-exact while the
//! session state machine reads only the discriminants it has verified.

pub mod records;
pub mod session;
pub mod wire;

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use records::{DshRecord, DshRecordBatch, DshRecordBuffer, DshRecordKind};
pub use session::{
    DshAction, DshSessionMachine, DshSessionSummary, DshTurnOutcome, DshTurnSummary,
};
pub use wire::{DshFrameKind, DshServerInfo};

/// Frame ceiling of one runtime line. The SDK protocol is newline-delimited
/// with no documented ceiling, so this is a VibeMux bound rather than a vendor
/// one; exceeding it ends the session with a fixed code.
pub const DEFAULT_FRAME_BYTES: usize = 512 * 1024;
pub const MIN_FRAME_BYTES: usize = 4 * 1024;
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

/// Default byte ceiling of the in-memory record buffer.
pub const DEFAULT_RECORD_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_RECORD_BYTES: usize = 64 * 1024 * 1024;
/// Default record-count ceiling of the in-memory record buffer.
pub const DEFAULT_RECORD_COUNT: usize = 8_192;
pub const MIN_RECORD_COUNT: usize = 16;
pub const MAX_RECORD_COUNT: usize = 65_536;
/// Maximum records one poll response may return.
pub const MAX_POLL_RECORDS: usize = 512;
/// Maximum bytes one poll response may return.
pub const MAX_POLL_BYTES: usize = 384 * 1024;

/// A cold Node boot plus profile resolution dominates startup, so the
/// initialize deadline is much larger than a native CLI's handshake.
pub const DEFAULT_INITIALIZE_DEADLINE_MS: u64 = 60_000;
pub const MIN_INITIALIZE_DEADLINE_MS: u64 = 1_000;
pub const MAX_INITIALIZE_DEADLINE_MS: u64 = 300_000;
/// Whole-turn ceiling, from `session/prompt` to `turn/end`.
pub const DEFAULT_TURN_DEADLINE_MS: u64 = 900_000;
pub const MIN_TURN_DEADLINE_MS: u64 = 1_000;
pub const MAX_TURN_DEADLINE_MS: u64 = 3_600_000;
pub const DEFAULT_SHUTDOWN_GRACE_MS: u64 = 5_000;
pub const MIN_SHUTDOWN_GRACE_MS: u64 = 100;
pub const MAX_SHUTDOWN_GRACE_MS: u64 = 30_000;

/// Keeps one prompt inside the Control IPC frame, matching the ADR 029 bound.
pub const MAX_PROMPT_BYTES: usize = 32 * 1024;
/// Bound on the daemon-chosen SDK session id.
pub const MAX_SESSION_ID_BYTES: usize = 128;
/// Bound on the working directory recorded in every session header.
pub const MAX_WORKING_DIRECTORY_BYTES: usize = 4096;
/// Bound on an adapter-owned reasoning-effort identifier.
pub const MAX_REASONING_EFFORT_BYTES: usize = 64;
/// Upper bound on `InitializeParams.maxTokens`.
pub const MAX_OUTPUT_TOKENS: u64 = 200_000;

/// Stable, machine-readable DSH substrate failures. Every variant maps to one
/// `dsh_*` code; none of them carries prompt text, a path, or a credential.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum DshError {
    #[error("the DSH session settings are invalid")]
    ConfigInvalid,
    #[error("the DSH session id is invalid")]
    InvalidSessionId,
    #[error("the DSH prompt is empty or exceeds its bound")]
    InvalidPrompt,
    #[error("the DSH session is not ready for a prompt")]
    NotReady,
    #[error("a DSH turn is already running")]
    TurnInFlight,
    #[error("the DSH session is already closed or failed")]
    Closed,
    #[error("the DSH frame is not a JSON-RPC 2.0 object")]
    InvalidJsonRpc,
    #[error("the DSH response does not correlate with a pending request")]
    UncorrelatedResponse,
    #[error("the DSH notification does not correlate with this session")]
    UncorrelatedSession,
    #[error("the DSH handshake reported an unexpected server identity")]
    InvalidServerIdentity,
    #[error("the DSH handshake result is missing its server information")]
    InvalidServerInfo,
    #[error("the DSH prompt receipt is missing its message identity")]
    InvalidPromptReceipt,
    #[error("the DSH peer answered with an error")]
    RpcError,
    #[error("the DSH turn-end event is missing its identity")]
    InvalidTurnEnd,
    #[error("the DSH turn-end event arrived outside a running turn")]
    UnexpectedTurnEnd,
    #[error("the DSH runtime exited before the session finished")]
    UnexpectedExit,
    #[error("the DSH record buffer reached its bound")]
    RecordBufferFull,
    #[error("the DSH record page cursor is not valid for this buffer")]
    InvalidCursor,
}

impl DshError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ConfigInvalid => "dsh_config_invalid",
            Self::InvalidSessionId => "dsh_invalid_session_id",
            Self::InvalidPrompt => "dsh_invalid_prompt",
            Self::NotReady => "dsh_not_ready",
            Self::TurnInFlight => "dsh_turn_in_flight",
            Self::Closed => "dsh_session_closed",
            Self::InvalidJsonRpc => "dsh_invalid_json_rpc",
            Self::UncorrelatedResponse => "dsh_uncorrelated_response",
            Self::UncorrelatedSession => "dsh_uncorrelated_session",
            Self::InvalidServerIdentity => "dsh_invalid_server_identity",
            Self::InvalidServerInfo => "dsh_invalid_server_info",
            Self::InvalidPromptReceipt => "dsh_invalid_prompt_receipt",
            Self::RpcError => "dsh_rpc_error",
            Self::InvalidTurnEnd => "dsh_invalid_turn_end",
            Self::UnexpectedTurnEnd => "dsh_unexpected_turn_end",
            Self::UnexpectedExit => "dsh_unexpected_exit",
            Self::RecordBufferFull => "dsh_record_buffer_full",
            Self::InvalidCursor => "dsh_invalid_cursor",
        }
    }
}

/// Client-side phase of one DSH session. `Closed` and `Failed` are terminal.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DshPhase {
    /// Constructed; `initialize` has not been written.
    Starting,
    /// `initialize` is in flight.
    Initializing,
    /// The handshake succeeded and no turn is running.
    Ready,
    /// A `session/prompt` is in flight.
    Prompting,
    /// `shutdown` was written, or cancellation started; the process is exiting.
    Closing,
    /// The runtime acknowledged `shutdown` or exited cleanly while closing.
    Closed,
    /// A protocol violation, a deadline, or an unexpected exit ended the
    /// session.
    Failed,
}

impl DshPhase {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Initializing => "initializing",
            Self::Ready => "ready",
            Self::Prompting => "prompting",
            Self::Closing => "closing",
            Self::Closed => "closed",
            Self::Failed => "failed",
        }
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Closed | Self::Failed)
    }

    /// Whether a prompt may be admitted right now.
    #[must_use]
    pub const fn accepts_prompt(self) -> bool {
        matches!(self, Self::Ready)
    }
}

/// Immutable inputs of one session. The daemon supplies them; nothing here is
/// read from the environment at this layer.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DshSessionSettings {
    /// SDK-side session id. The protocol lets the client choose it, and an
    /// unknown id lazily creates the agent+session pair.
    pub session_id: String,
    /// Working directory recorded on the session header.
    pub working_directory: String,
    /// Provider route every agent of this session runs on.
    pub provider: String,
    /// Model every agent of this session runs on.
    pub model: String,
    /// Optional adapter-owned reasoning effort.
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    /// Optional positive output-token cap.
    #[serde(default)]
    pub max_tokens: Option<u64>,
}

impl DshSessionSettings {
    pub fn validate(&self) -> Result<(), DshError> {
        if !session_id_is_valid(&self.session_id) {
            return Err(DshError::InvalidSessionId);
        }
        if self.working_directory.is_empty()
            || self.working_directory.len() > MAX_WORKING_DIRECTORY_BYTES
            || self.working_directory.contains('\0')
        {
            return Err(DshError::ConfigInvalid);
        }
        if !wire::route_label_is_valid(&self.provider) || !wire::route_label_is_valid(&self.model) {
            return Err(DshError::ConfigInvalid);
        }
        if let Some(effort) = &self.reasoning_effort {
            if !reasoning_effort_is_valid(effort) {
                return Err(DshError::ConfigInvalid);
            }
        }
        if let Some(max_tokens) = self.max_tokens {
            if max_tokens == 0 || max_tokens > MAX_OUTPUT_TOKENS {
                return Err(DshError::ConfigInvalid);
            }
        }
        Ok(())
    }
}

/// Bounded capture and timing limits of one session. Omitted fields take the
/// defaults.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct DshSessionLimits {
    pub frame_bytes: usize,
    pub record_bytes: usize,
    pub record_count: usize,
    pub initialize_deadline_ms: u64,
    pub turn_deadline_ms: u64,
    pub shutdown_grace_ms: u64,
}

impl Default for DshSessionLimits {
    fn default() -> Self {
        Self {
            frame_bytes: DEFAULT_FRAME_BYTES,
            record_bytes: DEFAULT_RECORD_BYTES,
            record_count: DEFAULT_RECORD_COUNT,
            initialize_deadline_ms: DEFAULT_INITIALIZE_DEADLINE_MS,
            turn_deadline_ms: DEFAULT_TURN_DEADLINE_MS,
            shutdown_grace_ms: DEFAULT_SHUTDOWN_GRACE_MS,
        }
    }
}

impl DshSessionLimits {
    pub fn validate(&self) -> Result<(), DshError> {
        let valid = (MIN_FRAME_BYTES..=MAX_FRAME_BYTES).contains(&self.frame_bytes)
            && (MIN_FRAME_BYTES..=MAX_RECORD_BYTES).contains(&self.record_bytes)
            && (MIN_RECORD_COUNT..=MAX_RECORD_COUNT).contains(&self.record_count)
            && (MIN_INITIALIZE_DEADLINE_MS..=MAX_INITIALIZE_DEADLINE_MS)
                .contains(&self.initialize_deadline_ms)
            && (MIN_TURN_DEADLINE_MS..=MAX_TURN_DEADLINE_MS).contains(&self.turn_deadline_ms)
            && (MIN_SHUTDOWN_GRACE_MS..=MAX_SHUTDOWN_GRACE_MS).contains(&self.shutdown_grace_ms);
        if valid {
            Ok(())
        } else {
            Err(DshError::ConfigInvalid)
        }
    }
}

/// Whether `value` is usable as an SDK session id. The value reaches protocol
/// params and is echoed back into correlation checks, so the alphabet is
/// deliberately narrow.
#[must_use]
pub fn session_id_is_valid(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_SESSION_ID_BYTES
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn reasoning_effort_is_valid(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_REASONING_EFFORT_BYTES
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

impl fmt::Debug for DshSessionSettings {
    /// Redacted: the working directory is a local path and stays out of logs
    /// by default (AGENTS.md §7.7).
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DshSessionSettings")
            .field("session_id_bytes", &self.session_id.len())
            .field("working_directory_bytes", &self.working_directory.len())
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("has_reasoning_effort", &self.reasoning_effort.is_some())
            .field("max_tokens", &self.max_tokens)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> DshSessionSettings {
        DshSessionSettings {
            session_id: "vibemux_session_1".to_string(),
            working_directory: "C:\\project".to_string(),
            provider: "deepseek-official".to_string(),
            model: "deepseek-v4-flash".to_string(),
            reasoning_effort: None,
            max_tokens: None,
        }
    }

    #[test]
    fn session_settings_validation_is_strict() {
        settings().validate().expect("valid settings");
        let mut invalid = settings();
        invalid.session_id = "has space".to_string();
        assert_eq!(invalid.validate(), Err(DshError::InvalidSessionId));
        let mut invalid = settings();
        invalid.session_id = String::new();
        assert_eq!(invalid.validate(), Err(DshError::InvalidSessionId));
        let mut invalid = settings();
        invalid.working_directory = String::new();
        assert_eq!(invalid.validate(), Err(DshError::ConfigInvalid));
        let mut invalid = settings();
        invalid.provider = "-flag".to_string();
        assert_eq!(invalid.validate(), Err(DshError::ConfigInvalid));
        let mut invalid = settings();
        invalid.max_tokens = Some(0);
        assert_eq!(invalid.validate(), Err(DshError::ConfigInvalid));
        let mut valid = settings();
        valid.max_tokens = Some(MAX_OUTPUT_TOKENS);
        valid.reasoning_effort = Some("high".to_string());
        valid.validate().expect("bounded values are valid");
    }

    #[test]
    fn limits_validation_rejects_every_out_of_range_field() {
        DshSessionLimits::default().validate().expect("defaults");
        let cases = [
            DshSessionLimits {
                frame_bytes: MIN_FRAME_BYTES - 1,
                ..DshSessionLimits::default()
            },
            DshSessionLimits {
                record_bytes: MAX_RECORD_BYTES + 1,
                ..DshSessionLimits::default()
            },
            DshSessionLimits {
                record_count: 0,
                ..DshSessionLimits::default()
            },
            DshSessionLimits {
                initialize_deadline_ms: 0,
                ..DshSessionLimits::default()
            },
            DshSessionLimits {
                turn_deadline_ms: MAX_TURN_DEADLINE_MS + 1,
                ..DshSessionLimits::default()
            },
            DshSessionLimits {
                shutdown_grace_ms: MIN_SHUTDOWN_GRACE_MS - 1,
                ..DshSessionLimits::default()
            },
        ];
        for limits in cases {
            assert_eq!(
                limits.validate(),
                Err(DshError::ConfigInvalid),
                "{limits:?}"
            );
        }
    }

    #[test]
    fn error_codes_are_unique_and_prefixed() {
        let all = [
            DshError::ConfigInvalid,
            DshError::InvalidSessionId,
            DshError::InvalidPrompt,
            DshError::NotReady,
            DshError::TurnInFlight,
            DshError::Closed,
            DshError::InvalidJsonRpc,
            DshError::UncorrelatedResponse,
            DshError::UncorrelatedSession,
            DshError::InvalidServerIdentity,
            DshError::InvalidServerInfo,
            DshError::InvalidPromptReceipt,
            DshError::RpcError,
            DshError::InvalidTurnEnd,
            DshError::UnexpectedTurnEnd,
            DshError::UnexpectedExit,
            DshError::RecordBufferFull,
            DshError::InvalidCursor,
        ];
        let mut codes = std::collections::BTreeSet::new();
        for error in all {
            let code = error.code();
            assert!(code.starts_with("dsh_"), "{code}");
            assert!(codes.insert(code), "duplicate code {code}");
        }
    }

    #[test]
    fn phase_predicates_are_exhaustive() {
        for phase in [
            DshPhase::Starting,
            DshPhase::Initializing,
            DshPhase::Ready,
            DshPhase::Prompting,
            DshPhase::Closing,
            DshPhase::Closed,
            DshPhase::Failed,
        ] {
            assert!(!phase.as_str().is_empty());
            assert_eq!(phase.accepts_prompt(), phase == DshPhase::Ready);
            assert_eq!(
                phase.is_terminal(),
                matches!(phase, DshPhase::Closed | DshPhase::Failed)
            );
        }
    }
}
