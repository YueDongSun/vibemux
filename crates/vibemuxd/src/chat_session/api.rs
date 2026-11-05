//! Typed local chat control. Only explicit private reads return titles or text.
//!
//! Record cursors name the record after `after_sequence`, with `byte_offset`
//! inside that immutable record. A completed record advances the sequence and
//! resets the offset. Live cursors additionally bind a replacement revision.

use std::fmt;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_harness::chat_session::AssistantText;
use vibemux_store::{ChatMutation, ChatSessionCommit, ChatTurnCommit};
use vibemux_types::{
    ChatId, ChatTurnId, ProjectId, RunId, TaskId,
    chat::{
        ChatError, ChatFailureCode, ChatHarness, ChatRecord, ChatRecordKind, ChatSession,
        ChatState, ChatTurn, ChatTurnOutcome, ChatTurnPhase, MAX_CHAT_PROMPT_BYTES,
        MAX_CHAT_RECORD_BYTES, MAX_CHAT_TITLE_BYTES,
    },
};

use super::{ChatService, ChatServiceError, ChatSessionAction};
use crate::chat_writer::PrivateChatRecord;
use vibemux_types::chat_command_progress::{
    CommandProgressScope, CommandProgressSnapshot, MAX_COMMAND_PROGRESS_INTEGER,
};

pub const DEFAULT_CHAT_PAGE: u32 = 16;
pub const MAX_CHAT_API_PAGE: u32 = 32;
pub const MAX_CHAT_CHUNK_BYTES: usize = 8192;
/// Leave space for the authenticated Control response envelope.
pub const MAX_CHAT_RESPONSE_BYTES: usize = 60 * 1024;

const fn default_page_limit() -> u32 {
    DEFAULT_CHAT_PAGE
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChatRequest {
    List {
        #[serde(default)]
        after: Option<ChatId>,
        #[serde(default = "default_page_limit")]
        limit: u32,
        #[serde(default)]
        archived: bool,
    },
    Open {
        operation_id: Uuid,
        harness: ChatHarness,
        #[serde(default)]
        title: Option<String>,
        consent_private_storage: bool,
    },
    LookupOpen {
        operation_id: Uuid,
    },
    Status {
        chat_id: ChatId,
    },
    Submission {
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
    },
    Send {
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
        prompt: String,
    },
    Cancel {
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
    },
    Turns {
        chat_id: ChatId,
        #[serde(default)]
        after_sequence: u64,
        #[serde(default = "default_page_limit")]
        limit: u32,
    },
    /// Explicit private title read; list and status never include the title.
    Title {
        chat_id: ChatId,
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
    CommandProgress {
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
    },
    Rename {
        operation_id: Uuid,
        chat_id: ChatId,
        generation: u64,
        title: String,
    },
    Close {
        operation_id: Uuid,
        chat_id: ChatId,
        generation: u64,
    },
    Archive {
        operation_id: Uuid,
        chat_id: ChatId,
        generation: u64,
    },
    Resume {
        operation_id: Uuid,
        chat_id: ChatId,
        generation: u64,
    },
    Purge {
        operation_id: Uuid,
        chat_id: ChatId,
        generation: u64,
    },
}

impl ChatRequest {
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
            Self::List { limit, .. } => valid_limit(*limit),
            Self::LookupOpen { operation_id } => !operation_id.is_nil(),
            Self::Submission {
                generation,
                request_id,
                ..
            } => valid_generation(*generation) && !request_id.is_nil(),
            Self::Open {
                operation_id,
                title,
                consent_private_storage,
                ..
            } => {
                !operation_id.is_nil()
                    && *consent_private_storage
                    && title
                        .as_ref()
                        .is_none_or(|text| valid_text(text, MAX_CHAT_TITLE_BYTES))
            }
            Self::Send {
                generation,
                request_id,
                prompt,
                ..
            } => {
                valid_generation(*generation)
                    && !request_id.is_nil()
                    && valid_text(prompt, MAX_CHAT_PROMPT_BYTES)
            }
            Self::Cancel {
                generation,
                request_id,
                ..
            } => valid_generation(*generation) && !request_id.is_nil(),
            Self::Turns {
                after_sequence,
                limit,
                ..
            } => *after_sequence <= i64::MAX as u64 && valid_limit(*limit),
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
                valid_generation(*generation)
                    && !request_id.is_nil()
                    && revision.is_none_or(|value| value > 0)
                    && (revision.is_some() || *byte_offset == 0)
                    && u64::from(*byte_offset) <= MAX_CHAT_RECORD_BYTES
            }
            Self::Rename {
                operation_id,
                generation,
                title,
                ..
            } => {
                !operation_id.is_nil()
                    && valid_generation(*generation)
                    && valid_text(title, MAX_CHAT_TITLE_BYTES)
            }
            Self::Close {
                operation_id,
                generation,
                ..
            }
            | Self::Archive {
                operation_id,
                generation,
                ..
            }
            | Self::Resume {
                operation_id,
                generation,
                ..
            }
            | Self::Purge {
                operation_id,
                generation,
                ..
            } => !operation_id.is_nil() && valid_generation(*generation),
            Self::Status { .. } | Self::Title { .. } => true,
        };
        if valid {
            Ok(())
        } else {
            Err(ChatError::InvalidInput)
        }
    }

    fn operation_name(&self) -> &'static str {
        match self {
            Self::List { .. } => "list",
            Self::Open { .. } => "open",
            Self::LookupOpen { .. } => "lookup_open",
            Self::Status { .. } => "status",
            Self::Submission { .. } => "submission",
            Self::Send { .. } => "send",
            Self::Cancel { .. } => "cancel",
            Self::Turns { .. } => "turns",
            Self::Title { .. } => "title",
            Self::Content { .. } => "content",
            Self::Live { .. } => "live",
            Self::CommandProgress { .. } => "command_progress",
            Self::Rename { .. } => "rename",
            Self::Close { .. } => "close",
            Self::Archive { .. } => "archive",
            Self::Resume { .. } => "resume",
            Self::Purge { .. } => "purge",
        }
    }
}

impl fmt::Debug for ChatRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChatRequest")
            .field("operation", &self.operation_name())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatContentCursor {
    pub after_sequence: u64,
    pub byte_offset: u32,
}

/// Public, content-free session allowlist. Store records must never be returned
/// directly: they also contain private title references and vendor bindings.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatSessionStatus {
    pub schema_version: u32,
    pub chat_id: ChatId,
    pub project_id: ProjectId,
    pub harness: ChatHarness,
    pub state: ChatState,
    pub generation: u64,
    pub version: u64,
    pub active_request_id: Option<Uuid>,
    pub record_count: u64,
    pub last_record_sequence: u64,
    pub content_purged: bool,
    pub has_title: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}
impl From<&ChatSession> for ChatSessionStatus {
    fn from(value: &ChatSession) -> Self {
        Self {
            schema_version: value.schema_version,
            chat_id: value.chat_id,
            project_id: value.project_id,
            harness: value.harness,
            state: value.state,
            generation: value.generation,
            version: value.version,
            active_request_id: value.active_request_id,
            record_count: value.record_count,
            last_record_sequence: value.last_record_sequence,
            content_purged: value.content_purged,
            has_title: value.title_ref.is_some() && !value.content_purged,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatTurnStatus {
    pub schema_version: u32,
    pub chat_id: ChatId,
    pub turn_id: ChatTurnId,
    pub request_id: Uuid,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub generation: u64,
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
impl From<&ChatTurn> for ChatTurnStatus {
    fn from(value: &ChatTurn) -> Self {
        Self {
            schema_version: value.schema_version,
            chat_id: value.chat_id,
            turn_id: value.turn_id,
            request_id: value.request_id,
            task_id: value.task_id,
            run_id: value.run_id,
            generation: value.generation,
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
pub struct ChatRecordStatus {
    pub record_id: Uuid,
    pub chat_id: ChatId,
    pub turn_id: ChatTurnId,
    pub sequence: u64,
    pub kind: ChatRecordKind,
    pub original_byte_count: u64,
    pub retained_byte_count: Option<u64>,
    pub content_available: bool,
    pub truncated: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}
impl ChatRecordStatus {
    fn from_record(record: &ChatRecord, available: bool) -> Self {
        let retained = record
            .content_ref
            .as_ref()
            .map(|reference| reference.byte_count);
        Self {
            record_id: record.record_id,
            chat_id: record.chat_id,
            turn_id: record.turn_id,
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ChatContentError {
    #[serde(rename = "chat_content_purged")]
    Purged,
    #[serde(rename = "chat_content_unavailable")]
    Unavailable,
    #[serde(rename = "chat_content_invalid_utf8")]
    InvalidUtf8,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatContentPage {
    pub chat_id: ChatId,
    pub record: Option<ChatRecordStatus>,
    pub text: Option<String>,
    pub next_cursor: ChatContentCursor,
    /// True when the cursor has finished this record. A content_error
    /// distinguishes an unavailable body from delivered content.
    pub complete: bool,
    /// More bytes in this record, or records after it at the observed tail.
    pub more: bool,
    pub last_record_sequence: u64,
    pub content_error: Option<ChatContentError>,
}
impl fmt::Debug for ChatContentPage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChatContentPage")
            .field("chat_id", &self.chat_id)
            .field("record", &self.record)
            .field("next_cursor", &self.next_cursor)
            .field("complete", &self.complete)
            .field("more", &self.more)
            .field("content_error", &self.content_error)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatLiveChunk {
    pub turn_id: ChatTurnId,
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
impl fmt::Debug for ChatLiveChunk {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChatLiveChunk")
            .field("turn_id", &self.turn_id)
            .field("revision", &self.revision)
            .field("byte_offset", &self.byte_offset)
            .field("next_byte_offset", &self.next_byte_offset)
            .field("reset", &self.reset)
            .field("complete", &self.complete)
            .field("truncated", &self.truncated)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChatResponse {
    List {
        sessions: Vec<ChatSessionStatus>,
        next_after: Option<ChatId>,
        more: bool,
    },
    Session {
        session: ChatSessionStatus,
        sequence: u64,
        duplicate: bool,
    },
    Status {
        session: Option<ChatSessionStatus>,
    },
    Submission {
        session: ChatSessionStatus,
        turn: Option<ChatTurnStatus>,
    },
    Turn {
        session: ChatSessionStatus,
        turn: ChatTurnStatus,
        sequence: u64,
        duplicate: bool,
    },
    Turns {
        chat_id: ChatId,
        turns: Vec<ChatTurnStatus>,
        next_sequence: u64,
        more: bool,
    },
    Title {
        chat_id: ChatId,
        title: Option<String>,
    },
    Content {
        page: ChatContentPage,
    },
    Live {
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
        snapshot: Option<ChatLiveChunk>,
    },
    CommandProgress {
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
        snapshot: Option<CommandProgressSnapshot>,
    },
}
impl fmt::Debug for ChatResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::List {
                sessions,
                next_after,
                more,
            } => formatter
                .debug_struct("ChatResponse::List")
                .field("sessions", sessions)
                .field("next_after", next_after)
                .field("more", more)
                .finish(),
            Self::Session {
                session,
                sequence,
                duplicate,
            } => formatter
                .debug_struct("ChatResponse::Session")
                .field("session", session)
                .field("sequence", sequence)
                .field("duplicate", duplicate)
                .finish(),
            Self::Status { session } => formatter
                .debug_struct("ChatResponse::Status")
                .field("session", session)
                .finish(),
            Self::Submission { session, turn } => formatter
                .debug_struct("ChatResponse::Submission")
                .field("session", session)
                .field("turn", turn)
                .finish(),
            Self::Turn {
                session,
                turn,
                sequence,
                duplicate,
            } => formatter
                .debug_struct("ChatResponse::Turn")
                .field("session", session)
                .field("turn", turn)
                .field("sequence", sequence)
                .field("duplicate", duplicate)
                .finish(),
            Self::Turns {
                chat_id,
                turns,
                next_sequence,
                more,
            } => formatter
                .debug_struct("ChatResponse::Turns")
                .field("chat_id", chat_id)
                .field("turns", turns)
                .field("next_sequence", next_sequence)
                .field("more", more)
                .finish(),
            Self::Title { chat_id, .. } => formatter
                .debug_struct("ChatResponse::Title")
                .field("chat_id", chat_id)
                .finish_non_exhaustive(),
            Self::Content { page } => formatter
                .debug_struct("ChatResponse::Content")
                .field("page", page)
                .finish(),
            Self::CommandProgress {
                chat_id,
                generation,
                request_id,
                snapshot,
            } => formatter
                .debug_struct("ChatResponse::CommandProgress")
                .field("chat_id", chat_id)
                .field("generation", generation)
                .field("request_id", request_id)
                .field("snapshot", snapshot)
                .finish(),
            Self::Live {
                chat_id,
                generation,
                request_id,
                snapshot,
            } => formatter
                .debug_struct("ChatResponse::Live")
                .field("chat_id", chat_id)
                .field("generation", generation)
                .field("request_id", request_id)
                .field("snapshot", snapshot)
                .finish(),
        }
    }
}

/// Called only after authenticated local Control admission. This adapter owns no
/// process, storage, replay, or transition semantics; those stay in ChatService.
pub async fn execute(
    service: &ChatService,
    request: ChatRequest,
) -> Result<ChatResponse, ChatServiceError> {
    request.validate()?;
    let response = match request {
        ChatRequest::List {
            after,
            limit,
            archived,
        } => {
            let mut sessions = service.sessions(after, limit + 1, archived).await?;
            let more = sessions.len() > limit as usize;
            sessions.truncate(limit as usize);
            ChatResponse::List {
                next_after: sessions.last().map(|session| session.chat_id),
                more,
                sessions: sessions.iter().map(ChatSessionStatus::from).collect(),
            }
        }
        ChatRequest::Open {
            operation_id,
            harness,
            title,
            ..
        } => session_response(service.open(operation_id, harness, title).await?),
        ChatRequest::LookupOpen { operation_id } => ChatResponse::Status {
            session: service
                .session_for_operation(operation_id)
                .await?
                .as_ref()
                .map(ChatSessionStatus::from),
        },
        ChatRequest::Status { chat_id } => ChatResponse::Status {
            session: Some(ChatSessionStatus::from(
                &existing_session(service, chat_id).await?,
            )),
        },
        ChatRequest::Submission {
            chat_id,
            generation,
            request_id,
        } => {
            let session = existing_session(service, chat_id).await?;
            let turn = service.turn(request_id).await?;
            submission_response(&session, generation, request_id, turn)?
        }
        ChatRequest::Send {
            chat_id,
            generation,
            request_id,
            prompt,
        } => turn_response(
            service
                .send(chat_id, generation, request_id, prompt)
                .await?,
        ),
        ChatRequest::Cancel {
            chat_id,
            generation,
            request_id,
        } => turn_response(service.cancel(chat_id, generation, request_id).await?),
        ChatRequest::Turns {
            chat_id,
            after_sequence,
            limit,
        } => {
            let mut turns = service.turns(chat_id, after_sequence, limit + 1).await?;
            let more = turns.len() > limit as usize;
            turns.truncate(limit as usize);
            ChatResponse::Turns {
                chat_id,
                next_sequence: turns
                    .last()
                    .map_or(after_sequence, |turn| turn.admitted_sequence),
                more,
                turns: turns.iter().map(ChatTurnStatus::from).collect(),
            }
        }
        ChatRequest::Title { chat_id } => ChatResponse::Title {
            chat_id,
            title: service.title(chat_id).await?,
        },
        ChatRequest::Content { chat_id, cursor } => {
            let session = existing_session(service, chat_id).await?;
            validate_content_cursor(cursor, session.last_record_sequence)?;
            let record = service
                .records(chat_id, cursor.after_sequence, 1)
                .await?
                .into_iter()
                .next();
            ChatResponse::Content {
                page: content_page(&session, cursor, record)?,
            }
        }
        ChatRequest::Live {
            chat_id,
            generation,
            request_id,
            revision,
            byte_offset,
        } => {
            let session = existing_session(service, chat_id).await?;
            require_generation(&session, generation)?;
            let snapshot = if session.state == ChatState::Busy
                && session.active_request_id == Some(request_id)
            {
                let text = service.live_text(chat_id, generation, request_id).await?;
                let current = existing_session(service, chat_id).await?;
                require_generation(&current, generation)?;
                live_chunk(&current, request_id, revision, byte_offset, text)?
            } else {
                None
            };
            ChatResponse::Live {
                chat_id,
                generation,
                request_id,
                snapshot,
            }
        }
        ChatRequest::CommandProgress {
            chat_id,
            generation,
            request_id,
        } => {
            let session = existing_session(service, chat_id).await?;
            if service.execution_owner(chat_id).await?.is_some() {
                return Err(ChatError::OwnerControlRequired.into());
            }
            if session.content_purged {
                return Err(ChatError::ContentPurged.into());
            }
            require_generation(&session, generation)?;
            let snapshot = if session.state == ChatState::Busy
                && session.active_request_id == Some(request_id)
            {
                let turn = service.turn(request_id).await?.ok_or(ChatError::NotFound)?;
                if turn.chat_id != chat_id
                    || turn.generation != generation
                    || turn.request_id != request_id
                {
                    return Err(ChatError::Conflict.into());
                }
                let snapshot = service
                    .command_progress(CommandProgressScope::Saved, chat_id, generation, request_id)
                    .await?;
                let current = existing_session(service, chat_id).await?;
                if current.content_purged {
                    return Err(ChatError::ContentPurged.into());
                }
                require_generation(&current, generation)?;
                if current.state != ChatState::Busy || current.active_request_id != Some(request_id)
                {
                    None
                } else {
                    if let Some(value) = &snapshot {
                        value
                            .validate()
                            .map_err(|_| ChatError::ProjectionMismatch)?;
                        if value.canonical_id != *turn.turn_id.as_uuid() {
                            return Err(ChatError::ProjectionMismatch.into());
                        }
                    }
                    snapshot
                }
            } else {
                None
            };
            ChatResponse::CommandProgress {
                chat_id,
                generation,
                request_id,
                snapshot,
            }
        }
        ChatRequest::Rename {
            operation_id,
            chat_id,
            generation,
            title,
        } => session_response(
            service
                .rename(mutation(operation_id, chat_id, generation), title)
                .await?,
        ),
        ChatRequest::Close {
            operation_id,
            chat_id,
            generation,
        } => session_response(
            service
                .change_session(
                    mutation(operation_id, chat_id, generation),
                    ChatSessionAction::Close,
                )
                .await?,
        ),
        ChatRequest::Archive {
            operation_id,
            chat_id,
            generation,
        } => session_response(
            service
                .change_session(
                    mutation(operation_id, chat_id, generation),
                    ChatSessionAction::Archive,
                )
                .await?,
        ),
        ChatRequest::Resume {
            operation_id,
            chat_id,
            generation,
        } => session_response(
            service
                .change_session(
                    mutation(operation_id, chat_id, generation),
                    ChatSessionAction::Resume,
                )
                .await?,
        ),
        ChatRequest::Purge {
            operation_id,
            chat_id,
            generation,
        } => session_response(
            service
                .change_session(
                    mutation(operation_id, chat_id, generation),
                    ChatSessionAction::Purge,
                )
                .await?,
        ),
    };
    bounded_response(response).map_err(Into::into)
}

async fn existing_session(
    service: &ChatService,
    chat_id: ChatId,
) -> Result<ChatSession, ChatServiceError> {
    service
        .session(chat_id)
        .await?
        .ok_or_else(|| ChatError::NotFound.into())
}

fn session_response(commit: ChatSessionCommit) -> ChatResponse {
    ChatResponse::Session {
        session: ChatSessionStatus::from(&commit.session),
        sequence: commit.sequence,
        duplicate: commit.duplicate,
    }
}
fn turn_response(commit: ChatTurnCommit) -> ChatResponse {
    ChatResponse::Turn {
        session: ChatSessionStatus::from(&commit.session),
        turn: ChatTurnStatus::from(&commit.turn),
        sequence: commit.sequence,
        duplicate: commit.duplicate,
    }
}
fn submission_response(
    session: &ChatSession,
    generation: u64,
    request_id: Uuid,
    turn: Option<ChatTurn>,
) -> Result<ChatResponse, ChatError> {
    match turn.as_ref() {
        Some(turn)
            if turn.chat_id != session.chat_id
                || turn.generation != generation
                || turn.request_id != request_id =>
        {
            return Err(ChatError::Conflict);
        }
        None => require_generation(session, generation)?,
        _ => {}
    }
    Ok(ChatResponse::Submission {
        session: ChatSessionStatus::from(session),
        turn: turn.as_ref().map(ChatTurnStatus::from),
    })
}

fn mutation(operation_id: Uuid, chat_id: ChatId, expected_generation: u64) -> ChatMutation {
    ChatMutation {
        operation_id,
        chat_id,
        expected_generation,
        timestamp: OffsetDateTime::now_utc(),
    }
}
fn valid_limit(limit: u32) -> bool {
    (1..=MAX_CHAT_API_PAGE).contains(&limit)
}
fn valid_generation(generation: u64) -> bool {
    generation > 0 && generation <= i64::MAX as u64
}
fn valid_text(text: &str, maximum: u64) -> bool {
    !text.trim().is_empty() && text.len() as u64 <= maximum
}
fn require_generation(session: &ChatSession, generation: u64) -> Result<(), ChatError> {
    if session.generation == generation {
        Ok(())
    } else {
        Err(ChatError::StaleGeneration)
    }
}
pub(super) fn validate_content_cursor(
    cursor: ChatContentCursor,
    tail: u64,
) -> Result<(), ChatError> {
    if cursor.after_sequence > tail || (cursor.after_sequence == tail && cursor.byte_offset != 0) {
        Err(ChatError::InvalidInput)
    } else {
        Ok(())
    }
}

fn content_page(
    session: &ChatSession,
    cursor: ChatContentCursor,
    record: Option<PrivateChatRecord>,
) -> Result<ChatContentPage, ChatError> {
    validate_content_cursor(cursor, session.last_record_sequence)?;
    let Some(private) = record else {
        if cursor.after_sequence < session.last_record_sequence {
            return Err(ChatError::ProjectionMismatch);
        }
        return Ok(ChatContentPage {
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
    let metadata = ChatRecordStatus::from_record(
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
            let (chunk, end) = text_chunk(&text, cursor.byte_offset)?;
            let complete = end as usize == text.len();
            let next = if complete {
                next_record
            } else {
                ChatContentCursor {
                    byte_offset: end,
                    ..cursor
                }
            };
            (Some(chunk), next, complete, None)
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
    Ok(ChatContentPage {
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
    session: &ChatSession,
    request_id: Uuid,
    revision: Option<u64>,
    byte_offset: u32,
    text: Option<AssistantText>,
) -> Result<Option<ChatLiveChunk>, ChatError> {
    if session.state != ChatState::Busy || session.active_request_id != Some(request_id) {
        return Ok(None);
    }
    let Some(text) = text.filter(|text| text.request_id == request_id) else {
        return Ok(None);
    };
    if text.revision == 0 || text.text.len() as u64 > MAX_CHAT_RECORD_BYTES {
        return Err(ChatError::ProjectionMismatch);
    }
    let reset = revision.is_some_and(|revision| revision != text.revision);
    let start = if reset { 0 } else { byte_offset };
    let (chunk, end) = text_chunk(&text.text, start)?;
    let complete = end as usize == text.text.len();
    Ok(Some(ChatLiveChunk {
        turn_id: ChatTurnId::from_uuid(text.turn_id).map_err(|_| ChatError::ProjectionMismatch)?,
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

pub(super) fn text_chunk(text: &str, byte_offset: u32) -> Result<(String, u32), ChatError> {
    let start = usize::try_from(byte_offset).map_err(|_| ChatError::InvalidInput)?;
    if start > text.len() || !text.is_char_boundary(start) {
        return Err(ChatError::InvalidInput);
    }
    let mut end = start + MAX_CHAT_CHUNK_BYTES.min(text.len() - start);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Ok((
        text[start..end].to_owned(),
        u32::try_from(end).map_err(|_| ChatError::InvalidInput)?,
    ))
}

fn bounded_response(response: ChatResponse) -> Result<ChatResponse, ChatError> {
    let bytes = serde_json::to_vec(&response).map_err(|_| ChatError::ProjectionMismatch)?;
    if bytes.len() > MAX_CHAT_RESPONSE_BYTES {
        return Err(ChatError::Capacity);
    }
    Ok(response)
}

#[cfg(test)]
#[path = "api_tests.rs"]
mod tests;
