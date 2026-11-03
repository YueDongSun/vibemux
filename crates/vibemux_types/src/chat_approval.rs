//! Sanitized approval metadata. Vendor identity and tool arguments stay private.
use crate::{
    ChatApprovalId, ChatAttemptId, ChatId, ProjectId, RunId, TaskId, ToolName,
    chat::{ChatError, ContentRef, MAX_CHAT_RECORD_BYTES, valid_digest},
};
use serde::{Deserialize, Serialize};
use std::fmt;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

pub const CHAT_APPROVAL_SCHEMA_VERSION: u32 = 1;
pub const MAX_CHAT_APPROVAL_SECONDS: i64 = 120;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatApprovalState {
    Pending,
    Decided,
    Expired,
    Invalidated,
}
impl ChatApprovalState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Decided => "decided",
            Self::Expired => "expired",
            Self::Invalidated => "invalidated",
        }
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatApprovalDecision {
    AllowOnce,
    Deny,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatApprovalDelivery {
    Claimed,
    Written,
    Unknown,
}
impl ChatApprovalDelivery {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claimed => "claimed",
            Self::Written => "written",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatApproval {
    pub schema_version: u32,
    pub approval_id: ChatApprovalId,
    pub chat_id: ChatId,
    pub attempt_id: ChatAttemptId,
    pub request_id: Uuid,
    pub project_id: ProjectId,
    pub owner_task_id: TaskId,
    pub owner_run_id: RunId,
    pub generation: u64,
    pub version: u64,
    pub admitted_sequence: u64,
    pub tool: ToolName,
    pub profile_sha256: String,
    pub evidence_ref: ContentRef,
    pub state: ChatApprovalState,
    pub decision: Option<ChatApprovalDecision>,
    pub decision_operation_id: Option<Uuid>,
    pub delivery: Option<ChatApprovalDelivery>,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}
impl ChatApproval {
    pub fn validate(&self) -> Result<(), ChatError> {
        self.evidence_ref.validate(MAX_CHAT_RECORD_BYTES)?;
        if self.schema_version != CHAT_APPROVAL_SCHEMA_VERSION
            || self.request_id.is_nil()
            || [self.generation, self.version, self.admitted_sequence]
                .iter()
                .any(|v| *v == 0 || *v > i64::MAX as u64)
            || !valid_digest(&self.profile_sha256)
            || self.updated_at < self.created_at
            || self.expires_at <= self.created_at
            || self.expires_at - self.created_at > Duration::seconds(MAX_CHAT_APPROVAL_SECONDS)
            || match self.state {
                ChatApprovalState::Decided => {
                    self.decision.is_none()
                        || self.delivery.is_none()
                        || self.decision_operation_id.is_none_or(|id| id.is_nil())
                }
                _ => {
                    self.decision.is_some()
                        || self.delivery.is_some()
                        || self.decision_operation_id.is_some()
                }
            }
        {
            return Err(ChatError::InvalidInput);
        }
        Ok(())
    }
}
impl fmt::Debug for ChatApproval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatApproval")
            .field("approval_id", &self.approval_id)
            .field("chat_id", &self.chat_id)
            .field("attempt_id", &self.attempt_id)
            .field("generation", &self.generation)
            .field("version", &self.version)
            .field("tool", &self.tool)
            .field("state", &self.state)
            .field("decision", &self.decision)
            .field("delivery", &self.delivery)
            .finish_non_exhaustive()
    }
}
