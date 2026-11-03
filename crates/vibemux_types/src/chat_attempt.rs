//! Correlated submissions inside a lifetime chat Run.
use crate::{
    ChatAttemptId, ChatId, ProjectId, RunId, TaskId, ToolName,
    chat::{
        ChatError, ChatFailureCode, ChatRecordKind, ChatTurnOutcome, ChatTurnPhase, ContentRef,
        MAX_CHAT_BYTES, MAX_CHAT_PROMPT_BYTES, MAX_CHAT_RECORD_BYTES, valid_vendor_id,
    },
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fmt};
use time::OffsetDateTime;
use uuid::Uuid;

pub const CHAT_ATTEMPT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatExecutionAttempt {
    pub schema_version: u32,
    pub chat_id: ChatId,
    pub attempt_id: ChatAttemptId,
    pub request_id: Uuid,
    pub project_id: ProjectId,
    pub owner_task_id: TaskId,
    pub owner_run_id: RunId,
    pub generation: u64,
    pub owner_version: u64,
    pub version: u64,
    pub phase: ChatTurnPhase,
    pub admitted_sequence: u64,
    pub base_commit: String,
    pub effective_tools: BTreeSet<ToolName>,
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

impl ChatExecutionAttempt {
    pub fn validate(&self) -> Result<(), ChatError> {
        self.prompt_ref.validate(MAX_CHAT_PROMPT_BYTES)?;
        let expected_outcome = match self.phase {
            ChatTurnPhase::Completed => Some(ChatTurnOutcome::Completed),
            ChatTurnPhase::Failed => Some(ChatTurnOutcome::Failed),
            ChatTurnPhase::Cancelled => Some(ChatTurnOutcome::Cancelled),
            ChatTurnPhase::Interrupted => Some(ChatTurnOutcome::Interrupted),
            _ => None,
        };
        if self.schema_version != CHAT_ATTEMPT_SCHEMA_VERSION
            || self.request_id.is_nil()
            || self.generation == 0
            || self.generation > i64::MAX as u64
            || self.owner_version == 0
            || self.owner_version > i64::MAX as u64
            || self.version == 0
            || self.version > i64::MAX as u64
            || self.admitted_sequence == 0
            || self.admitted_sequence > i64::MAX as u64
            || !matches!(self.base_commit.len(), 40 | 64)
            || !self
                .base_commit
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || self.updated_at < self.created_at
            || self.outcome != expected_outcome
            || self
                .vendor_turn_id
                .as_ref()
                .is_some_and(|id| !valid_vendor_id(id))
            || (self.correlated_terminal
                && (!self.phase.is_terminal() || self.vendor_turn_id.is_none()))
            || (self.output_complete && !self.correlated_terminal)
            || (self.phase == ChatTurnPhase::Completed && !self.correlated_terminal)
            || (matches!(self.phase, ChatTurnPhase::Failed) && !self.correlated_terminal)
            || match self.phase {
                ChatTurnPhase::Failed
                | ChatTurnPhase::Cancelled
                | ChatTurnPhase::Interrupted
                | ChatTurnPhase::RecoveryRequired => self.error_code.is_none(),
                _ => self.error_code.is_some(),
            }
        {
            return Err(ChatError::InvalidInput);
        }
        Ok(())
    }
}
impl fmt::Debug for ChatExecutionAttempt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChatExecutionAttempt")
            .field("chat_id", &self.chat_id)
            .field("attempt_id", &self.attempt_id)
            .field("request_id", &self.request_id)
            .field("generation", &self.generation)
            .field("phase", &self.phase)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatExecutionRecord {
    pub schema_version: u32,
    pub record_id: Uuid,
    pub chat_id: ChatId,
    pub attempt_id: ChatAttemptId,
    pub sequence: u64,
    pub kind: ChatRecordKind,
    pub content_ref: Option<ContentRef>,
    pub original_byte_count: u64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}
impl ChatExecutionRecord {
    pub fn validate(&self) -> Result<(), ChatError> {
        if self.schema_version != CHAT_ATTEMPT_SCHEMA_VERSION
            || self.record_id.is_nil()
            || self.sequence == 0
            || self.sequence > i64::MAX as u64
            || self.original_byte_count > MAX_CHAT_BYTES
        {
            return Err(ChatError::InvalidInput);
        }
        if let Some(reference) = &self.content_ref {
            reference.validate(MAX_CHAT_RECORD_BYTES)?;
            if reference.byte_count > self.original_byte_count {
                return Err(ChatError::InvalidInput);
            }
        }
        Ok(())
    }
}
impl fmt::Debug for ChatExecutionRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChatExecutionRecord")
            .field("record_id", &self.record_id)
            .field("attempt_id", &self.attempt_id)
            .field("sequence", &self.sequence)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}
