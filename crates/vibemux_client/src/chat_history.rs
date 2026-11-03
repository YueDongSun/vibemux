//! GUI-neutral private history reads through the authenticated Control client.
//! Callers own snapshot consistency, activity policy and text presentation.
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_types::{
    ChatId, ChatTurnId, RunId, TaskId,
    chat::{
        ChatFailureCode, ChatTurnOutcome, ChatTurnPhase, MAX_CHAT_RECORD_BYTES,
        MAX_CHAT_TITLE_BYTES,
    },
};
use vibemuxd::chat_session::api::{
    ChatContentCursor, ChatContentError, ChatContentPage, ChatRecordStatus, ChatSessionStatus,
    MAX_CHAT_API_PAGE, MAX_CHAT_CHUNK_BYTES,
};
use vibemuxd::control::{
    ChatExecutionRequest, ChatExecutionResponse, ChatOwnerRequest, ChatOwnerResponse, ChatRequest,
    ChatResponse, ControlClient,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationKind {
    Chat,
    Workspace,
}
#[derive(Clone, Debug, Serialize)]
pub struct HistoryTurn {
    pub correlation_id: Uuid,
    pub request_id: Uuid,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub generation: u64,
    pub phase: ChatTurnPhase,
    pub outcome: Option<ChatTurnOutcome>,
    pub error_code: Option<ChatFailureCode>,
    pub correlated_terminal: bool,
    pub output_complete: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryError {
    ReadFailed,
    InvalidResponse,
    NotFound,
}

#[derive(Clone, Debug)]
pub struct HistoryTurnPage {
    pub turns: Vec<(u64, HistoryTurn)>,
    pub next_sequence: u64,
    pub more: bool,
}

/// A project-bound authenticated reader; it never opens private storage itself.
pub struct HistoryClient<'a> {
    client: &'a ControlClient,
    kind: ConversationKind,
    chat_id: ChatId,
}

impl<'a> HistoryClient<'a> {
    pub fn new(client: &'a ControlClient, kind: ConversationKind, chat_id: ChatId) -> Self {
        Self {
            client,
            kind,
            chat_id,
        }
    }

    pub async fn status(&self) -> Result<ChatSessionStatus, HistoryError> {
        let session = match self.kind {
            ConversationKind::Chat => match self
                .client
                .chat(&ChatRequest::Status {
                    chat_id: self.chat_id,
                })
                .await
                .map_err(|_| HistoryError::ReadFailed)?
            {
                ChatResponse::Status { session } => session.ok_or(HistoryError::NotFound)?,
                _ => return Err(HistoryError::InvalidResponse),
            },
            ConversationKind::Workspace => match self
                .client
                .chat_execution(&ChatExecutionRequest::Status {
                    chat_id: self.chat_id,
                })
                .await
                .map_err(|_| HistoryError::ReadFailed)?
            {
                ChatExecutionResponse::Status { runtime } => {
                    runtime.ok_or(HistoryError::NotFound)?.session
                }
                _ => return Err(HistoryError::InvalidResponse),
            },
        };
        if session.chat_id != self.chat_id
            || session.generation == 0
            || session.generation > i64::MAX as u64
            || session.version == 0
            || session.version > i64::MAX as u64
        {
            return Err(HistoryError::InvalidResponse);
        }
        Ok(session)
    }

    pub async fn title(&self) -> Result<(Option<String>, Option<ChatContentError>), HistoryError> {
        let title = match self.kind {
            ConversationKind::Chat => match self
                .client
                .chat(&ChatRequest::Title {
                    chat_id: self.chat_id,
                })
                .await
            {
                Ok(ChatResponse::Title { chat_id, title }) if chat_id == self.chat_id => title,
                Ok(_) => return Err(HistoryError::InvalidResponse),
                Err(error) => return title_error(error.code()),
            },
            ConversationKind::Workspace => match self
                .client
                .chat_owner(&ChatOwnerRequest::Title {
                    chat_id: self.chat_id,
                })
                .await
            {
                Ok(ChatOwnerResponse::Title { chat_id, title }) if chat_id == self.chat_id => title,
                Ok(_) => return Err(HistoryError::InvalidResponse),
                Err(error) => return title_error(error.code()),
            },
        };
        if title
            .as_ref()
            .is_some_and(|text| text.len() as u64 > MAX_CHAT_TITLE_BYTES)
        {
            return Err(HistoryError::InvalidResponse);
        }
        Ok((title, None))
    }

    pub async fn content(
        &self,
        cursor: ChatContentCursor,
    ) -> Result<ChatContentPage, HistoryError> {
        if cursor.after_sequence > i64::MAX as u64
            || u64::from(cursor.byte_offset) > MAX_CHAT_RECORD_BYTES
        {
            return Err(HistoryError::InvalidResponse);
        }
        let page = match self.kind {
            ConversationKind::Chat => match self
                .client
                .chat(&ChatRequest::Content {
                    chat_id: self.chat_id,
                    cursor,
                })
                .await
                .map_err(|_| HistoryError::ReadFailed)?
            {
                ChatResponse::Content { page } => page,
                _ => return Err(HistoryError::InvalidResponse),
            },
            ConversationKind::Workspace => match self
                .client
                .chat_execution(&ChatExecutionRequest::Content {
                    chat_id: self.chat_id,
                    cursor,
                })
                .await
                .map_err(|_| HistoryError::ReadFailed)?
            {
                ChatExecutionResponse::Content { page } => {
                    let record = page
                        .record
                        .map(|record| -> Result<ChatRecordStatus, HistoryError> {
                            Ok(ChatRecordStatus {
                                record_id: record.record_id,
                                chat_id: record.chat_id,
                                turn_id: ChatTurnId::from_uuid(*record.attempt_id.as_uuid())
                                    .map_err(|_| HistoryError::InvalidResponse)?,
                                sequence: record.sequence,
                                kind: record.kind,
                                original_byte_count: record.original_byte_count,
                                retained_byte_count: record.retained_byte_count,
                                content_available: record.content_available,
                                truncated: record.truncated,
                                created_at: record.created_at,
                            })
                        })
                        .transpose()?;
                    ChatContentPage {
                        chat_id: page.chat_id,
                        record,
                        text: page.text,
                        next_cursor: page.next_cursor,
                        complete: page.complete,
                        more: page.more,
                        last_record_sequence: page.last_record_sequence,
                        content_error: page.content_error,
                    }
                }
                _ => return Err(HistoryError::InvalidResponse),
            },
        };
        if page.chat_id != self.chat_id
            || page
                .record
                .as_ref()
                .is_some_and(|record| record.chat_id != self.chat_id)
            || page
                .text
                .as_ref()
                .is_some_and(|text| text.len() > MAX_CHAT_CHUNK_BYTES)
            || page.last_record_sequence > i64::MAX as u64
            || page.next_cursor.after_sequence > i64::MAX as u64
            || u64::from(page.next_cursor.byte_offset) > MAX_CHAT_RECORD_BYTES
        {
            return Err(HistoryError::InvalidResponse);
        }
        Ok(page)
    }

    pub async fn turns(
        &self,
        after_sequence: u64,
        limit: u32,
    ) -> Result<HistoryTurnPage, HistoryError> {
        if after_sequence > i64::MAX as u64 || limit == 0 || limit > MAX_CHAT_API_PAGE {
            return Err(HistoryError::InvalidResponse);
        }
        let page = match self.kind {
            ConversationKind::Chat => match self
                .client
                .chat(&ChatRequest::Turns {
                    chat_id: self.chat_id,
                    after_sequence,
                    limit,
                })
                .await
                .map_err(|_| HistoryError::ReadFailed)?
            {
                ChatResponse::Turns {
                    chat_id,
                    turns,
                    next_sequence,
                    more,
                } if chat_id == self.chat_id => {
                    if turns.len() > limit as usize {
                        return Err(HistoryError::InvalidResponse);
                    }
                    let turns = turns
                        .into_iter()
                        .map(|turn| {
                            if turn.chat_id != self.chat_id || turn.version == 0 {
                                return Err(HistoryError::InvalidResponse);
                            }
                            Ok((
                                turn.admitted_sequence,
                                HistoryTurn {
                                    correlation_id: *turn.turn_id.as_uuid(),
                                    request_id: turn.request_id,
                                    task_id: turn.task_id,
                                    run_id: turn.run_id,
                                    generation: turn.generation,
                                    phase: turn.phase,
                                    outcome: turn.outcome,
                                    error_code: turn.error_code,
                                    correlated_terminal: turn.correlated_terminal,
                                    output_complete: turn.output_complete,
                                    created_at: turn.created_at,
                                    updated_at: turn.updated_at,
                                },
                            ))
                        })
                        .collect::<Result<Vec<_>, HistoryError>>()?;
                    HistoryTurnPage {
                        turns,
                        next_sequence,
                        more,
                    }
                }
                _ => return Err(HistoryError::InvalidResponse),
            },
            ConversationKind::Workspace => match self
                .client
                .chat_execution(&ChatExecutionRequest::Attempts {
                    chat_id: self.chat_id,
                    after_sequence,
                    limit,
                })
                .await
                .map_err(|_| HistoryError::ReadFailed)?
            {
                ChatExecutionResponse::Attempts {
                    chat_id,
                    attempts,
                    next_sequence,
                    more,
                } if chat_id == self.chat_id => {
                    if attempts.len() > limit as usize {
                        return Err(HistoryError::InvalidResponse);
                    }
                    let turns = attempts
                        .into_iter()
                        .map(|turn| {
                            if turn.chat_id != self.chat_id || turn.version == 0 {
                                return Err(HistoryError::InvalidResponse);
                            }
                            Ok((
                                turn.admitted_sequence,
                                HistoryTurn {
                                    correlation_id: *turn.attempt_id.as_uuid(),
                                    request_id: turn.request_id,
                                    task_id: turn.task_id,
                                    run_id: turn.run_id,
                                    generation: turn.generation,
                                    phase: turn.phase,
                                    outcome: turn.outcome,
                                    error_code: turn.error_code,
                                    correlated_terminal: turn.correlated_terminal,
                                    output_complete: turn.output_complete,
                                    created_at: turn.created_at,
                                    updated_at: turn.updated_at,
                                },
                            ))
                        })
                        .collect::<Result<Vec<_>, HistoryError>>()?;
                    HistoryTurnPage {
                        turns,
                        next_sequence,
                        more,
                    }
                }
                _ => return Err(HistoryError::InvalidResponse),
            },
        };
        let mut previous = after_sequence;
        for (sequence, turn) in &page.turns {
            if *sequence <= previous
                || *sequence > i64::MAX as u64
                || turn.generation == 0
                || turn.generation > i64::MAX as u64
                || turn.request_id.is_nil()
                || turn.correlation_id.is_nil()
            {
                return Err(HistoryError::InvalidResponse);
            }
            previous = *sequence;
        }
        if page.next_sequence != previous || (page.turns.is_empty() && page.more) {
            return Err(HistoryError::InvalidResponse);
        }
        Ok(page)
    }
}

fn title_error(code: &str) -> Result<(Option<String>, Option<ChatContentError>), HistoryError> {
    let error = match code {
        "chat_content_purged" => ChatContentError::Purged,
        "chat_content_unavailable" => ChatContentError::Unavailable,
        "chat_content_invalid_utf8" => ChatContentError::InvalidUtf8,
        _ => return Err(HistoryError::ReadFailed),
    };
    Ok((None, Some(error)))
}
