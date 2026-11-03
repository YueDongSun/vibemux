//! Closed owner-execution API. Content, Live and CommandProgress return private text.
use super::{
    ChatService, ChatServiceError,
    api::{self, ChatContentCursor, ChatContentError, ChatSessionStatus},
    owner_api::ChatOwnerStatus,
};
use crate::chat_writer::PrivateExecutionRecord;
use serde::{Deserialize, Serialize};
use std::fmt;
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_store::{ChatExecutionOwnerCommit, ChatOwnerMutation};
use vibemux_types::chat_command_progress::{
    CommandProgressScope, CommandProgressSnapshot, MAX_COMMAND_PROGRESS_INTEGER,
};
use vibemux_types::{
    ChatAttemptId, ChatId, RunId, TaskId,
    chat::{
        ChatError, ChatFailureCode, ChatRecordKind, ChatState, ChatTurnOutcome, ChatTurnPhase,
        MAX_CHAT_PROMPT_BYTES, MAX_CHAT_RECORD_BYTES,
    },
    chat_attempt::{ChatExecutionAttempt, ChatExecutionRecord},
};

pub const MAX_EXECUTION_API_PAGE: u32 = 32;
fn default_page() -> u32 {
    16
}
fn valid_version(value: u64) -> bool {
    value > 0 && value <= i64::MAX as u64
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChatExecutionRequest {
    CommandProgress {
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
    },
    Status {
        chat_id: ChatId,
    },
    Start {
        operation_id: Uuid,
        chat_id: ChatId,
        generation: u64,
        owner_version: u64,
        consent_execution: bool,
    },
    LookupStart {
        operation_id: Uuid,
        chat_id: ChatId,
        generation: u64,
        owner_version: u64,
    },
    Send {
        chat_id: ChatId,
        generation: u64,
        owner_version: u64,
        request_id: Uuid,
        prompt: String,
    },
    Submission {
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
    },
    Cancel {
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
    },
    Close {
        chat_id: ChatId,
        generation: u64,
    },
    Attempts {
        chat_id: ChatId,
        #[serde(default)]
        after_sequence: u64,
        #[serde(default = "default_page")]
        limit: u32,
    },
    Content {
        chat_id: ChatId,
        #[serde(default)]
        cursor: ChatContentCursor,
    },
    Live {
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
        #[serde(default)]
        revision: Option<u64>,
        #[serde(default)]
        byte_offset: u32,
    },
}
impl ChatExecutionRequest {
    pub fn validate(&self) -> Result<(), ChatError> {
        let valid = match self {
            Self::CommandProgress {
                generation,
                request_id,
                ..
            } => {
                *generation > 0
                    && *generation <= MAX_COMMAND_PROGRESS_INTEGER
                    && !request_id.is_nil()
            }
            Self::Status { .. } => true,
            Self::Start {
                operation_id,
                generation,
                owner_version,
                consent_execution,
                ..
            } => {
                !operation_id.is_nil()
                    && valid_version(*generation)
                    && valid_version(*owner_version)
                    && *consent_execution
            }
            Self::LookupStart {
                operation_id,
                generation,
                owner_version,
                ..
            } => {
                !operation_id.is_nil()
                    && valid_version(*generation)
                    && valid_version(*owner_version)
            }
            Self::Send {
                generation,
                owner_version,
                request_id,
                prompt,
                ..
            } => {
                valid_version(*generation)
                    && valid_version(*owner_version)
                    && !request_id.is_nil()
                    && !prompt.trim().is_empty()
                    && prompt.len() as u64 <= MAX_CHAT_PROMPT_BYTES
            }
            Self::Submission {
                generation,
                request_id,
                ..
            }
            | Self::Cancel {
                generation,
                request_id,
                ..
            } => valid_version(*generation) && !request_id.is_nil(),
            Self::Close { generation, .. } => valid_version(*generation),
            Self::Attempts {
                after_sequence,
                limit,
                ..
            } => {
                *after_sequence <= i64::MAX as u64 && *limit > 0 && *limit <= MAX_EXECUTION_API_PAGE
            }
            Self::Content { cursor, .. } => {
                cursor.after_sequence <= i64::MAX as u64
                    && u64::from(cursor.byte_offset) <= MAX_CHAT_RECORD_BYTES
            }
            Self::Live {
                generation,
                request_id,
                revision,
                byte_offset,
                ..
            } => {
                valid_version(*generation)
                    && !request_id.is_nil()
                    && revision.is_none_or(|value| value > 0)
                    && (revision.is_some() || *byte_offset == 0)
                    && u64::from(*byte_offset) <= MAX_CHAT_RECORD_BYTES
            }
        };
        if valid {
            Ok(())
        } else {
            Err(ChatError::InvalidInput)
        }
    }
    pub fn requires_execution_consent(&self) -> bool {
        matches!(self, Self::Start { .. } | Self::Send { .. })
    }
    pub fn is_mutation(&self) -> bool {
        matches!(
            self,
            Self::Start { .. } | Self::Send { .. } | Self::Cancel { .. } | Self::Close { .. }
        )
    }
    fn name(&self) -> &'static str {
        match self {
            Self::Status { .. } => "status",
            Self::Start { .. } => "start",
            Self::LookupStart { .. } => "lookup_start",
            Self::Send { .. } => "send",
            Self::Submission { .. } => "submission",
            Self::Cancel { .. } => "cancel",
            Self::Close { .. } => "close",
            Self::Attempts { .. } => "attempts",
            Self::Content { .. } => "content",
            Self::Live { .. } => "live",
            Self::CommandProgress { .. } => "command_progress",
        }
    }
}
impl fmt::Debug for ChatExecutionRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChatExecutionRequest")
            .field("operation", &self.name())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatExecutionStatus {
    pub owner: ChatOwnerStatus,
    pub session: ChatSessionStatus,
    pub process_claimed: bool,
}
impl ChatExecutionStatus {
    fn from_commit(
        service: &ChatService,
        value: &ChatExecutionOwnerCommit,
    ) -> Result<Self, ChatServiceError> {
        Ok(Self {
            owner: ChatOwnerStatus::from_commit(service, value)?,
            session: ChatSessionStatus::from(&value.session),
            process_claimed: value.process_claimed,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatAttemptStatus {
    pub schema_version: u32,
    pub chat_id: ChatId,
    pub attempt_id: ChatAttemptId,
    pub request_id: Uuid,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub generation: u64,
    pub owner_version: u64,
    pub version: u64,
    pub phase: ChatTurnPhase,
    pub admitted_sequence: u64,
    pub outcome: Option<ChatTurnOutcome>,
    pub error_code: Option<ChatFailureCode>,
    pub correlated_terminal: bool,
    pub output_complete: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}
impl From<&ChatExecutionAttempt> for ChatAttemptStatus {
    fn from(value: &ChatExecutionAttempt) -> Self {
        Self {
            schema_version: value.schema_version,
            chat_id: value.chat_id,
            attempt_id: value.attempt_id,
            request_id: value.request_id,
            task_id: value.owner_task_id,
            run_id: value.owner_run_id,
            generation: value.generation,
            owner_version: value.owner_version,
            version: value.version,
            phase: value.phase,
            admitted_sequence: value.admitted_sequence,
            outcome: value.outcome,
            error_code: value.error_code,
            correlated_terminal: value.correlated_terminal,
            output_complete: value.output_complete,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatExecutionRecordStatus {
    pub record_id: Uuid,
    pub chat_id: ChatId,
    pub attempt_id: ChatAttemptId,
    pub sequence: u64,
    pub kind: ChatRecordKind,
    pub original_byte_count: u64,
    pub retained_byte_count: Option<u64>,
    pub content_available: bool,
    pub truncated: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}
impl ChatExecutionRecordStatus {
    fn from_record(record: &ChatExecutionRecord, available: bool) -> Self {
        let retained = record
            .content_ref
            .as_ref()
            .map(|reference| reference.byte_count);
        Self {
            record_id: record.record_id,
            chat_id: record.chat_id,
            attempt_id: record.attempt_id,
            sequence: record.sequence,
            kind: record.kind,
            original_byte_count: record.original_byte_count,
            retained_byte_count: retained,
            content_available: available,
            truncated: retained.is_some_and(|bytes| bytes < record.original_byte_count),
            created_at: record.created_at,
        }
    }
}
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatExecutionContentPage {
    pub chat_id: ChatId,
    pub record: Option<ChatExecutionRecordStatus>,
    pub text: Option<String>,
    pub next_cursor: ChatContentCursor,
    pub complete: bool,
    pub more: bool,
    pub last_record_sequence: u64,
    pub content_error: Option<ChatContentError>,
}
impl fmt::Debug for ChatExecutionContentPage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatExecutionContentPage")
            .field("chat_id", &self.chat_id)
            .field("record", &self.record)
            .field("next_cursor", &self.next_cursor)
            .field("content_error", &self.content_error)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatExecutionLiveChunk {
    pub attempt_id: ChatAttemptId,
    pub revision: u64,
    pub byte_offset: u32,
    pub next_byte_offset: u32,
    pub reset: bool,
    pub text: String,
    pub complete: bool,
    pub more: bool,
    pub retained_byte_count: u32,
    pub original_byte_count: u64,
    pub truncated: bool,
}
impl fmt::Debug for ChatExecutionLiveChunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatExecutionLiveChunk")
            .field("attempt_id", &self.attempt_id)
            .field("revision", &self.revision)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChatExecutionResponse {
    Status {
        runtime: Option<ChatExecutionStatus>,
    },
    Process {
        runtime: ChatExecutionStatus,
        sequence: u64,
        duplicate: bool,
    },
    StartReceipt {
        runtime: Option<ChatExecutionStatus>,
        sequence: Option<u64>,
    },
    Attempt {
        runtime: ChatExecutionStatus,
        attempt: ChatAttemptStatus,
        sequence: u64,
        duplicate: bool,
    },
    Submission {
        runtime: ChatExecutionStatus,
        attempt: Option<ChatAttemptStatus>,
    },
    Attempts {
        chat_id: ChatId,
        attempts: Vec<ChatAttemptStatus>,
        next_sequence: u64,
        more: bool,
    },
    Content {
        page: ChatExecutionContentPage,
    },
    Live {
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
        snapshot: Option<ChatExecutionLiveChunk>,
    },
    CommandProgress {
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
        snapshot: Option<CommandProgressSnapshot>,
    },
}
impl fmt::Debug for ChatExecutionResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Status { .. } => "ChatExecutionResponse::Status",
            Self::Process { .. } => "ChatExecutionResponse::Process",
            Self::StartReceipt { .. } => "ChatExecutionResponse::StartReceipt",
            Self::Attempt { .. } => "ChatExecutionResponse::Attempt",
            Self::Submission { .. } => "ChatExecutionResponse::Submission",
            Self::Attempts { .. } => "ChatExecutionResponse::Attempts",
            Self::Content { .. } => "ChatExecutionResponse::Content(<private>)",
            Self::Live { .. } => "ChatExecutionResponse::Live(<private>)",
            Self::CommandProgress { .. } => "ChatExecutionResponse::CommandProgress(<private>)",
        })
    }
}

async fn runtime(
    service: &ChatService,
    chat_id: ChatId,
) -> Result<ChatExecutionStatus, ChatServiceError> {
    let value = service
        .execution_owner(chat_id)
        .await?
        .ok_or(ChatError::NotFound)?;
    ChatExecutionStatus::from_commit(service, &value)
}
fn require_generation(status: &ChatExecutionStatus, generation: u64) -> Result<(), ChatError> {
    if status.session.generation == generation {
        Ok(())
    } else {
        Err(ChatError::StaleGeneration)
    }
}
fn require_attempt(
    attempt: &ChatExecutionAttempt,
    chat_id: ChatId,
    generation: u64,
) -> Result<(), ChatError> {
    if attempt.chat_id == chat_id && attempt.generation == generation {
        Ok(())
    } else {
        Err(ChatError::Conflict)
    }
}

pub async fn execute(
    service: &ChatService,
    request: ChatExecutionRequest,
) -> Result<ChatExecutionResponse, ChatServiceError> {
    request.validate()?;
    let response = match request {
        ChatExecutionRequest::Status { chat_id } => ChatExecutionResponse::Status {
            runtime: service
                .execution_owner(chat_id)
                .await?
                .as_ref()
                .map(|value| ChatExecutionStatus::from_commit(service, value))
                .transpose()?,
        },
        ChatExecutionRequest::Start {
            operation_id,
            chat_id,
            generation,
            owner_version,
            ..
        } => {
            let result = service
                .start_execution(
                    ChatOwnerMutation {
                        operation_id,
                        chat_id,
                        expected_version: owner_version,
                        timestamp: OffsetDateTime::now_utc(),
                    },
                    generation,
                )
                .await?;
            ChatExecutionResponse::Process {
                runtime: runtime(service, chat_id).await?,
                sequence: result.sequence,
                duplicate: result.duplicate,
            }
        }
        ChatExecutionRequest::LookupStart {
            operation_id,
            chat_id,
            generation,
            owner_version,
        } => {
            match service
                .execution_start_receipt(operation_id, chat_id, generation, owner_version)
                .await?
            {
                Some(receipt) => ChatExecutionResponse::StartReceipt {
                    runtime: Some(runtime(service, chat_id).await?),
                    sequence: Some(receipt.sequence),
                },
                None => ChatExecutionResponse::StartReceipt {
                    runtime: None,
                    sequence: None,
                },
            }
        }
        ChatExecutionRequest::Send {
            chat_id,
            generation,
            owner_version,
            request_id,
            prompt,
        } => {
            let result = service
                .send_execution(chat_id, generation, owner_version, request_id, prompt)
                .await?;
            ChatExecutionResponse::Attempt {
                runtime: runtime(service, chat_id).await?,
                attempt: ChatAttemptStatus::from(&result.attempt),
                sequence: result.sequence,
                duplicate: result.duplicate,
            }
        }
        ChatExecutionRequest::Submission {
            chat_id,
            generation,
            request_id,
        } => {
            let status = runtime(service, chat_id).await?;
            let attempt = service.execution_attempt(request_id).await?;
            if let Some(attempt) = &attempt {
                require_attempt(attempt, chat_id, generation)?;
            } else {
                require_generation(&status, generation)?;
            }
            ChatExecutionResponse::Submission {
                runtime: status,
                attempt: attempt.as_ref().map(ChatAttemptStatus::from),
            }
        }
        ChatExecutionRequest::Cancel {
            chat_id,
            generation,
            request_id,
        } => {
            let attempt = service
                .cancel_execution(chat_id, generation, request_id)
                .await?;
            ChatExecutionResponse::Submission {
                runtime: runtime(service, chat_id).await?,
                attempt: Some(ChatAttemptStatus::from(&attempt)),
            }
        }
        ChatExecutionRequest::Close {
            chat_id,
            generation,
        } => {
            service.close_execution_process(chat_id, generation).await?;
            ChatExecutionResponse::Status {
                runtime: Some(runtime(service, chat_id).await?),
            }
        }
        ChatExecutionRequest::Attempts {
            chat_id,
            after_sequence,
            limit,
        } => {
            let mut attempts = service
                .execution_attempts(chat_id, after_sequence, limit + 1)
                .await?;
            let more = attempts.len() > limit as usize;
            attempts.truncate(limit as usize);
            ChatExecutionResponse::Attempts {
                chat_id,
                next_sequence: attempts
                    .last()
                    .map_or(after_sequence, |attempt| attempt.admitted_sequence),
                more,
                attempts: attempts.iter().map(ChatAttemptStatus::from).collect(),
            }
        }
        ChatExecutionRequest::Content { chat_id, cursor } => {
            let status = runtime(service, chat_id).await?;
            api::validate_content_cursor(cursor, status.session.last_record_sequence)?;
            let record = service
                .execution_records(chat_id, cursor.after_sequence, 1)
                .await?
                .into_iter()
                .next();
            ChatExecutionResponse::Content {
                page: content_page(&status, cursor, record)?,
            }
        }
        ChatExecutionRequest::CommandProgress {
            chat_id,
            generation,
            request_id,
        } => {
            let status = runtime(service, chat_id).await?;
            if status.session.content_purged {
                return Err(ChatError::ContentPurged.into());
            }
            require_generation(&status, generation)?;
            let snapshot = if status.session.state == ChatState::Busy
                && status.session.active_request_id == Some(request_id)
            {
                let attempt = service
                    .execution_attempt(request_id)
                    .await?
                    .ok_or(ChatError::NotFound)?;
                require_attempt(&attempt, chat_id, generation)?;
                if attempt.request_id != request_id {
                    return Err(ChatError::Conflict.into());
                }
                let snapshot = service
                    .command_progress(
                        CommandProgressScope::Workspace,
                        chat_id,
                        generation,
                        request_id,
                    )
                    .await?;
                let current = runtime(service, chat_id).await?;
                if current.session.content_purged {
                    return Err(ChatError::ContentPurged.into());
                }
                require_generation(&current, generation)?;
                if current.session.state != ChatState::Busy
                    || current.session.active_request_id != Some(request_id)
                {
                    None
                } else {
                    if let Some(value) = &snapshot {
                        value
                            .validate()
                            .map_err(|_| ChatError::ProjectionMismatch)?;
                        if value.canonical_id != *attempt.attempt_id.as_uuid() {
                            return Err(ChatError::ProjectionMismatch.into());
                        }
                    }
                    snapshot
                }
            } else {
                None
            };
            ChatExecutionResponse::CommandProgress {
                chat_id,
                generation,
                request_id,
                snapshot,
            }
        }
        ChatExecutionRequest::Live {
            chat_id,
            generation,
            request_id,
            revision,
            byte_offset,
        } => {
            let status = runtime(service, chat_id).await?;
            require_generation(&status, generation)?;
            let snapshot = if status.session.state == ChatState::Busy
                && status.session.active_request_id == Some(request_id)
            {
                let attempt = service
                    .execution_attempt(request_id)
                    .await?
                    .ok_or(ChatError::NotFound)?;
                require_attempt(&attempt, chat_id, generation)?;
                let text = service.live_text(chat_id, generation, request_id).await?;
                let current = runtime(service, chat_id).await?;
                require_generation(&current, generation)?;
                if current.session.state != ChatState::Busy
                    || current.session.active_request_id != Some(request_id)
                {
                    None
                } else {
                    live_chunk(&attempt, request_id, revision, byte_offset, text)?
                }
            } else {
                None
            };
            ChatExecutionResponse::Live {
                chat_id,
                generation,
                request_id,
                snapshot,
            }
        }
    };
    if serde_json::to_vec(&response)
        .map_err(|_| ChatError::ProjectionMismatch)?
        .len()
        > api::MAX_CHAT_RESPONSE_BYTES
    {
        return Err(ChatError::Capacity.into());
    }
    Ok(response)
}

fn content_page(
    status: &ChatExecutionStatus,
    cursor: ChatContentCursor,
    record: Option<PrivateExecutionRecord>,
) -> Result<ChatExecutionContentPage, ChatError> {
    let session = &status.session;
    api::validate_content_cursor(cursor, session.last_record_sequence)?;
    let Some(private) = record else {
        if cursor.after_sequence < session.last_record_sequence {
            return Err(ChatError::ProjectionMismatch);
        }
        return Ok(ChatExecutionContentPage {
            chat_id: session.chat_id,
            record: None,
            text: None,
            next_cursor: cursor,
            complete: true,
            more: false,
            last_record_sequence: session.last_record_sequence,
            content_error: None,
        });
    };
    let record = &private.record;
    if record.chat_id != session.chat_id || record.sequence <= cursor.after_sequence {
        return Err(ChatError::ProjectionMismatch);
    }
    let metadata = ChatExecutionRecordStatus::from_record(
        record,
        private.text.is_some() && private.content_error.is_none(),
    );
    let size = record
        .content_ref
        .as_ref()
        .map_or(record.original_byte_count, |reference| reference.byte_count);
    if u64::from(cursor.byte_offset) > size {
        return Err(ChatError::InvalidInput);
    }
    let next_record = ChatContentCursor {
        after_sequence: record.sequence,
        byte_offset: 0,
    };
    let (text, next_cursor, complete, content_error) = match (private.text, private.content_error) {
        (Some(text), None) => {
            if text.len() as u64 > MAX_CHAT_RECORD_BYTES || text.len() as u64 != size {
                return Err(ChatError::ProjectionMismatch);
            }
            let (chunk, end) = api::text_chunk(&text, cursor.byte_offset)?;
            let complete = end as usize == text.len();
            (
                Some(chunk),
                if complete {
                    next_record
                } else {
                    ChatContentCursor {
                        byte_offset: end,
                        ..cursor
                    }
                },
                complete,
                None,
            )
        }
        (_, error) => {
            let code = if record.content_ref.is_none() {
                ChatContentError::Purged
            } else if error == Some("chat_content_invalid_utf8") {
                ChatContentError::InvalidUtf8
            } else {
                ChatContentError::Unavailable
            };
            (None, next_record, true, Some(code))
        }
    };
    Ok(ChatExecutionContentPage {
        chat_id: session.chat_id,
        record: Some(metadata),
        text,
        next_cursor,
        complete,
        more: !complete || record.sequence < session.last_record_sequence,
        last_record_sequence: session.last_record_sequence.max(record.sequence),
        content_error,
    })
}

fn live_chunk(
    attempt: &ChatExecutionAttempt,
    request_id: Uuid,
    revision: Option<u64>,
    offset: u32,
    text: Option<vibemux_harness::chat_session::AssistantText>,
) -> Result<Option<ChatExecutionLiveChunk>, ChatError> {
    let Some(text) = text else {
        return Ok(None);
    };
    if text.request_id != request_id
        || text.turn_id != *attempt.attempt_id.as_uuid()
        || text.revision == 0
        || text.text.len() as u64 > MAX_CHAT_RECORD_BYTES
    {
        return Err(ChatError::ProjectionMismatch);
    }
    let reset = revision.is_some_and(|value| value != text.revision);
    let start = if reset { 0 } else { offset };
    let (chunk, end) = api::text_chunk(&text.text, start)?;
    let complete = end as usize == text.text.len();
    Ok(Some(ChatExecutionLiveChunk {
        attempt_id: attempt.attempt_id,
        revision: text.revision,
        byte_offset: start,
        next_byte_offset: end,
        reset,
        text: chunk,
        complete,
        more: !complete,
        retained_byte_count: u32::try_from(text.text.len())
            .map_err(|_| ChatError::ProjectionMismatch)?,
        original_byte_count: u64::try_from(text.original_bytes)
            .map_err(|_| ChatError::ProjectionMismatch)?,
        truncated: text.truncated,
    }))
}

#[cfg(test)]
#[path = "execution_api_tests.rs"]
mod tests;
