//! Closed interactive approval API. Only Content returns private tool input.
use super::{ChatService, ChatServiceError, api::ChatContentError};
use serde::{Deserialize, Serialize};
use std::fmt;
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_store::{ChatApprovalCommit, ChatApprovalDecisionLookup};
use vibemux_types::{
    ChatApprovalId, ChatAttemptId, ChatId, RunId, TaskId, ToolName,
    chat::{ChatError, MAX_CHAT_RECORD_BYTES},
    chat_approval::{ChatApproval, ChatApprovalDecision, ChatApprovalDelivery, ChatApprovalState},
};

pub const MAX_APPROVAL_CONTENT_CHUNK_BYTES: usize = 8192;

fn valid_version(value: u64) -> bool {
    value > 0 && value <= i64::MAX as u64
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChatApprovalRequest {
    Pending {
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
    },
    Metadata {
        chat_id: ChatId,
        approval_id: ChatApprovalId,
    },
    Content {
        chat_id: ChatId,
        approval_id: ChatApprovalId,
        generation: u64,
        request_id: Uuid,
        byte_offset: u32,
    },
    Decide {
        chat_id: ChatId,
        approval_id: ChatApprovalId,
        generation: u64,
        request_id: Uuid,
        expected_version: u64,
        operation_id: Uuid,
        decision: ChatApprovalDecision,
    },
    Receipt {
        chat_id: ChatId,
        approval_id: ChatApprovalId,
        generation: u64,
        request_id: Uuid,
        expected_version: u64,
        operation_id: Uuid,
        decision: ChatApprovalDecision,
    },
}
impl ChatApprovalRequest {
    pub fn validate(&self) -> Result<(), ChatError> {
        let valid = match self {
            Self::Pending {
                generation,
                request_id,
                ..
            } => valid_version(*generation) && !request_id.is_nil(),
            Self::Metadata { .. } => true,
            Self::Content {
                generation,
                request_id,
                byte_offset,
                ..
            } => {
                valid_version(*generation)
                    && !request_id.is_nil()
                    && u64::from(*byte_offset) <= MAX_CHAT_RECORD_BYTES
            }
            Self::Decide {
                generation,
                request_id,
                expected_version,
                operation_id,
                ..
            }
            | Self::Receipt {
                generation,
                request_id,
                expected_version,
                operation_id,
                ..
            } => {
                valid_version(*generation)
                    && !request_id.is_nil()
                    && valid_version(*expected_version)
                    && !operation_id.is_nil()
            }
        };
        if valid {
            Ok(())
        } else {
            Err(ChatError::InvalidInput)
        }
    }
}

/// Explicit mapping: private blob references and profile fingerprints stay core-owned.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatApprovalStatus {
    pub approval_id: ChatApprovalId,
    pub chat_id: ChatId,
    pub attempt_id: ChatAttemptId,
    pub request_id: Uuid,
    pub owner_task_id: TaskId,
    pub owner_run_id: RunId,
    pub generation: u64,
    pub version: u64,
    pub evidence_byte_count: u64,
    pub tool: ToolName,
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
    pub decidable: bool,
}
impl ChatApprovalStatus {
    fn from_record(value: ChatApproval, live: bool, now: OffsetDateTime) -> Self {
        Self {
            approval_id: value.approval_id,
            chat_id: value.chat_id,
            attempt_id: value.attempt_id,
            request_id: value.request_id,
            owner_task_id: value.owner_task_id,
            owner_run_id: value.owner_run_id,
            generation: value.generation,
            version: value.version,
            evidence_byte_count: value.evidence_ref.byte_count,
            tool: value.tool,
            state: value.state,
            decision: value.decision,
            decision_operation_id: value.decision_operation_id,
            delivery: value.delivery,
            expires_at: value.expires_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
            decidable: live && value.state == ChatApprovalState::Pending && value.expires_at > now,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatApprovalReceipt {
    pub approval: ChatApprovalStatus,
    pub sequence: u64,
    pub duplicate: bool,
}
impl ChatApprovalReceipt {
    fn from_commit(value: ChatApprovalCommit, duplicate: bool, now: OffsetDateTime) -> Self {
        Self {
            approval: ChatApprovalStatus::from_record(value.approval, false, now),
            sequence: value.sequence,
            duplicate,
        }
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatApprovalContent {
    pub chat_id: ChatId,
    pub approval_id: ChatApprovalId,
    pub request_id: Uuid,
    pub generation: u64,
    pub byte_offset: u32,
    pub next_byte_offset: u32,
    /// Total bytes of the immutable JSON input, excluding the private envelope.
    pub byte_count: u64,
    pub text: Option<String>,
    pub complete: bool,
    pub content_error: Option<ChatContentError>,
}
impl fmt::Debug for ChatApprovalContent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatApprovalContent")
            .field("approval_id", &self.approval_id)
            .field("byte_offset", &self.byte_offset)
            .field("complete", &self.complete)
            .field("content_error", &self.content_error)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChatApprovalResponse {
    Pending {
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
        approval: Option<ChatApprovalStatus>,
        #[serde(with = "time::serde::rfc3339")]
        server_time: OffsetDateTime,
    },
    Metadata {
        approval: ChatApprovalStatus,
        #[serde(with = "time::serde::rfc3339")]
        server_time: OffsetDateTime,
    },
    Content {
        page: ChatApprovalContent,
        #[serde(with = "time::serde::rfc3339")]
        server_time: OffsetDateTime,
    },
    Decision {
        receipt: ChatApprovalReceipt,
        #[serde(with = "time::serde::rfc3339")]
        server_time: OffsetDateTime,
    },
    Receipt {
        receipt: Option<ChatApprovalReceipt>,
        #[serde(with = "time::serde::rfc3339")]
        server_time: OffsetDateTime,
    },
}

pub async fn execute(
    service: &ChatService,
    request: ChatApprovalRequest,
) -> Result<ChatApprovalResponse, ChatServiceError> {
    request.validate()?;
    Ok(match request {
        ChatApprovalRequest::Pending {
            chat_id,
            generation,
            request_id,
        } => {
            let value = service
                .approval_pending(chat_id, generation, request_id)
                .await?;
            let server_time = OffsetDateTime::now_utc();
            ChatApprovalResponse::Pending {
                chat_id,
                generation,
                request_id,
                approval: value.map(|v| ChatApprovalStatus::from_record(v, true, server_time)),
                server_time,
            }
        }
        ChatApprovalRequest::Metadata {
            chat_id,
            approval_id,
        } => {
            let (value, live) = service.approval_metadata(chat_id, approval_id).await?;
            let server_time = OffsetDateTime::now_utc();
            ChatApprovalResponse::Metadata {
                approval: ChatApprovalStatus::from_record(value, live, server_time),
                server_time,
            }
        }
        ChatApprovalRequest::Content {
            chat_id,
            approval_id,
            generation,
            request_id,
            byte_offset,
        } => {
            let (record, text, content_error) = service
                .approval_input(chat_id, approval_id, generation, request_id)
                .await?;
            let (text, next_byte_offset, byte_count, complete) = if let Some(text) = text {
                let (chunk, next, complete) = content_chunk(&text, byte_offset)?;
                (Some(chunk), next, text.len() as u64, complete)
            } else {
                (None, byte_offset, 0, false)
            };
            ChatApprovalResponse::Content {
                page: ChatApprovalContent {
                    chat_id: record.chat_id,
                    approval_id: record.approval_id,
                    request_id: record.request_id,
                    generation: record.generation,
                    byte_offset,
                    next_byte_offset,
                    byte_count,
                    text,
                    complete,
                    content_error,
                },
                server_time: OffsetDateTime::now_utc(),
            }
        }
        ChatApprovalRequest::Decide {
            chat_id,
            approval_id,
            generation,
            request_id,
            expected_version,
            operation_id,
            decision,
        } => {
            let (receipt, duplicate) = service
                .approval_decide(ChatApprovalDecisionLookup {
                    chat_id,
                    approval_id,
                    generation,
                    request_id,
                    expected_version,
                    operation_id,
                    decision,
                })
                .await?;
            let server_time = OffsetDateTime::now_utc();
            ChatApprovalResponse::Decision {
                receipt: ChatApprovalReceipt::from_commit(receipt, duplicate, server_time),
                server_time,
            }
        }
        ChatApprovalRequest::Receipt {
            chat_id,
            approval_id,
            generation,
            request_id,
            expected_version,
            operation_id,
            decision,
        } => {
            let receipt = service
                .approval_receipt(ChatApprovalDecisionLookup {
                    chat_id,
                    approval_id,
                    generation,
                    request_id,
                    expected_version,
                    operation_id,
                    decision,
                })
                .await?;
            let server_time = OffsetDateTime::now_utc();
            ChatApprovalResponse::Receipt {
                receipt: receipt.map(|v| ChatApprovalReceipt::from_commit(v, true, server_time)),
                server_time,
            }
        }
    })
}

fn content_chunk(text: &str, byte_offset: u32) -> Result<(String, u32, bool), ChatError> {
    let start = byte_offset as usize;
    if text.len() as u64 > MAX_CHAT_RECORD_BYTES
        || start > text.len()
        || !text.is_char_boundary(start)
    {
        return Err(ChatError::InvalidInput);
    }
    let mut end = start
        .saturating_add(MAX_APPROVAL_CONTENT_CHUNK_BYTES)
        .min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Ok((
        text[start..end].to_owned(),
        u32::try_from(end).map_err(|_| ChatError::Capacity)?,
        end == text.len(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn decision_is_closed_and_cannot_supply_private_or_authority_fields() {
        let base = json!({"operation":"decide","chat_id":ChatId::new(),"approval_id":ChatApprovalId::new(),
            "generation":1,"request_id":Uuid::new_v4(),"expected_version":1,"operation_id":Uuid::new_v4(),"decision":"allow_once"});
        assert!(
            serde_json::from_value::<ChatApprovalRequest>(base.clone())
                .expect("typed request")
                .validate()
                .is_ok()
        );
        for key in [
            "path",
            "vendor_request_id",
            "input",
            "tool_grants",
            "profile_sha256",
            "fence",
            "wire_bytes",
        ] {
            let mut changed = base.clone();
            changed[key] = json!("untrusted private value");
            assert!(serde_json::from_value::<ChatApprovalRequest>(changed).is_err());
        }
        for key in ["generation", "expected_version"] {
            let mut changed = base.clone();
            changed[key] = json!(0);
            assert!(
                serde_json::from_value::<ChatApprovalRequest>(changed)
                    .expect("shape")
                    .validate()
                    .is_err()
            );
        }
    }
    #[test]
    fn private_chunks_preserve_utf8_boundaries_and_debug_redacts_content() {
        let text = format!(
            "{}é🦀private_tool_input",
            "x".repeat(MAX_APPROVAL_CONTENT_CHUNK_BYTES - 1)
        );
        let (first, next, complete) = content_chunk(&text, 0).expect("first page");
        assert_eq!(next as usize, MAX_APPROVAL_CONTENT_CHUNK_BYTES - 1);
        assert!(!complete);
        let (last, end, complete) = content_chunk(&text, next).expect("last page");
        assert_eq!(first + &last, text);
        assert_eq!(end as usize, text.len());
        assert!(complete);
        assert!(content_chunk(&text, next + 1).is_err());
        let page = ChatApprovalContent {
            chat_id: ChatId::new(),
            approval_id: ChatApprovalId::new(),
            request_id: Uuid::new_v4(),
            generation: 1,
            byte_offset: 0,
            next_byte_offset: 18,
            byte_count: 18,
            text: Some("private_tool_input".into()),
            complete: true,
            content_error: None,
        };
        assert!(!format!("{page:?}").contains("private_tool_input"));
    }
}
