//! Persistent chat metadata. Private text is represented only by bounded blob references.
pub use crate::{ChatId, ChatTurnId};
use crate::{ProjectId, RunId, TaskId};
use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

pub const CHAT_SCHEMA_VERSION: u32 = 1;
pub const MAX_CHAT_SESSIONS: u64 = 128;
pub const MAX_CHAT_PROCESSES: u64 = 3;
pub const MAX_CHAT_SUBMISSIONS: u64 = 65_536;
pub const MAX_CHAT_RECORDS: u64 = 4096;
pub const MAX_CHAT_RECORD_BYTES: u64 = 256 * 1024;
pub const MAX_CHAT_PROMPT_BYTES: u64 = 32 * 1024;
pub const MAX_CHAT_TITLE_BYTES: u64 = 512;
pub const MAX_CHAT_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_PROJECT_CHAT_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_CHAT_PAGE: u32 = 128;
pub const MAX_VENDOR_ID_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatHarness {
    Claude,
    Codex,
}
impl ChatHarness {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatState {
    Opening,
    Ready,
    Busy,
    Suspended,
    RecoveryRequired,
    Closed,
    Archived,
}
impl ChatState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Opening => "opening",
            Self::Ready => "ready",
            Self::Busy => "busy",
            Self::Suspended => "suspended",
            Self::RecoveryRequired => "recovery_required",
            Self::Closed => "closed",
            Self::Archived => "archived",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatTurnPhase {
    Admitted,
    Running,
    CancelRequested,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
    RecoveryRequired,
}
impl ChatTurnPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::Running => "running",
            Self::CancelRequested => "cancel_requested",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
            Self::RecoveryRequired => "recovery_required",
        }
    }
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }
    pub const fn holds_reservation(self) -> bool {
        !self.is_terminal()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatTurnOutcome {
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatFailureCode {
    VendorFailed,
    ProtocolViolation,
    ProcessLost,
    DeadlineExceeded,
    Cancelled,
    Interrupted,
    OwnershipUncertain,
    OutputUnavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRecordKind {
    User,
    Assistant,
    Tool,
    Approval,
    Notice,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContentRef {
    pub blob_id: Uuid,
    pub sha256: String,
    pub byte_count: u64,
}
impl ContentRef {
    pub fn validate(&self, maximum: u64) -> Result<(), ChatError> {
        if self.blob_id.is_nil()
            || !valid_digest(&self.sha256)
            || self.byte_count == 0
            || self.byte_count > maximum
        {
            return Err(ChatError::InvalidInput);
        }
        Ok(())
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatSession {
    pub schema_version: u32,
    pub chat_id: ChatId,
    pub project_id: ProjectId,
    pub harness: ChatHarness,
    pub profile_sha256: String,
    pub resource_key: String,
    pub base_commit: String,
    pub state: ChatState,
    pub generation: u64,
    pub version: u64,
    pub expected_vendor_id: Option<String>,
    pub vendor_session_id: Option<String>,
    pub vendor_confirmed_generation: Option<u64>,
    pub title_ref: Option<ContentRef>,
    pub retained_bytes: u64,
    pub record_count: u64,
    pub last_record_sequence: u64,
    pub content_purged: bool,
    pub active_request_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}
impl fmt::Debug for ChatSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatSession")
            .field("chat_id", &self.chat_id)
            .field("state", &self.state)
            .field("generation", &self.generation)
            .field("version", &self.version)
            .field(
                "vendor_confirmed_generation",
                &self.vendor_confirmed_generation,
            )
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatTurn {
    pub schema_version: u32,
    pub chat_id: ChatId,
    pub turn_id: ChatTurnId,
    pub request_id: Uuid,
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub generation: u64,
    pub version: u64,
    pub phase: ChatTurnPhase,
    /// Immutable global event cursor for paging admitted turns.
    pub admitted_sequence: u64,
    /// Git HEAD observed by the daemon for this admission, not a renderer input.
    pub base_commit: String,
    pub prompt_ref: ContentRef,
    pub vendor_turn_id: Option<String>,
    pub outcome: Option<ChatTurnOutcome>,
    pub error_code: Option<ChatFailureCode>,
    pub correlated_terminal: bool,
    pub output_complete: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}
impl fmt::Debug for ChatTurn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatTurn")
            .field("chat_id", &self.chat_id)
            .field("turn_id", &self.turn_id)
            .field("request_id", &self.request_id)
            .field("phase", &self.phase)
            .field("generation", &self.generation)
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatRecord {
    pub schema_version: u32,
    pub record_id: Uuid,
    pub chat_id: ChatId,
    pub turn_id: ChatTurnId,
    pub sequence: u64,
    pub kind: ChatRecordKind,
    pub content_ref: Option<ContentRef>,
    pub original_byte_count: u64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ChatError {
    #[error("chat input is invalid")]
    InvalidInput,
    #[error("chat was not found")]
    NotFound,
    #[error("chat operation binding conflicts")]
    Conflict,
    #[error("chat resource is busy")]
    Busy,
    #[error("chat capacity is exhausted")]
    Capacity,
    #[error("chat generation is stale")]
    StaleGeneration,
    #[error("chat ownership fence is stale")]
    StaleFence,
    #[error("chat transition is invalid")]
    InvalidTransition,
    #[error("chat harness was not detected")]
    NotDetected,
    #[error("chat projection integrity failed")]
    ProjectionMismatch,
    #[error("chat content is unavailable")]
    ContentUnavailable,
    #[error("chat content was purged")]
    ContentPurged,
    #[error("chat vendor identity is not confirmed")]
    UnconfirmedSession,
    #[error("chat process ownership is already claimed")]
    OwnerAlreadyClaimed,
    #[error("chat turn send is already claimed")]
    TurnAlreadyClaimed,
    #[error("chat requires the lifetime-owner control interface")]
    OwnerControlRequired,
}
impl ChatError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidInput => "chat_invalid_input",
            Self::NotFound => "chat_not_found",
            Self::Conflict => "chat_conflict",
            Self::Busy => "chat_busy",
            Self::Capacity => "chat_capacity",
            Self::StaleGeneration => "chat_stale_generation",
            Self::StaleFence => "chat_stale_fence",
            Self::InvalidTransition => "chat_invalid_transition",
            Self::NotDetected => "chat_not_detected",
            Self::ProjectionMismatch => "chat_projection_mismatch",
            Self::ContentUnavailable => "chat_content_unavailable",
            Self::ContentPurged => "chat_content_purged",
            Self::UnconfirmedSession => "chat_unconfirmed_session",
            Self::OwnerAlreadyClaimed => "chat_owner_already_claimed",
            Self::TurnAlreadyClaimed => "chat_turn_already_claimed",
            Self::OwnerControlRequired => "chat_owner_control_required",
        }
    }
}

pub fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
pub fn valid_vendor_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_VENDOR_ID_BYTES && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn content_references_are_bounded_and_not_paths() {
        let mut reference = ContentRef {
            blob_id: Uuid::new_v4(),
            sha256: "a".repeat(64),
            byte_count: MAX_CHAT_RECORD_BYTES,
        };
        assert_eq!(reference.validate(MAX_CHAT_RECORD_BYTES), Ok(()));
        reference.byte_count += 1;
        assert_eq!(
            reference.validate(MAX_CHAT_RECORD_BYTES),
            Err(ChatError::InvalidInput)
        );
        reference.byte_count = 1;
        reference.sha256 = "../private".to_owned();
        assert!(reference.validate(MAX_CHAT_RECORD_BYTES).is_err());
    }
    #[test]
    fn unknown_turns_keep_reservations() {
        assert!(ChatTurnPhase::RecoveryRequired.holds_reservation());
        assert!(!ChatTurnPhase::Completed.holds_reservation());
        assert_eq!(ChatState::Ready.as_str(), "ready");
    }
}
