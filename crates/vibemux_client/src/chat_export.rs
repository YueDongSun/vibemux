//! Explicit bounded copies of private history through authenticated Control reads.
use serde::{Deserialize, Serialize};
use std::fmt;
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_types::{
    ChatId,
    chat::{ChatRecordKind, MAX_CHAT_BYTES, MAX_CHAT_RECORDS, MAX_CHAT_SUBMISSIONS},
};
use vibemuxd::chat_session::api::{ChatContentError, ChatSessionStatus};

mod collect;
mod format;
mod save;

pub use crate::chat_history::{ConversationKind as ExportKind, HistoryTurn as ExportTurn};
pub use collect::{collect_conversation, verify_snapshot};
pub use format::format_export;
pub use save::{pick_export_destination, write_export};

pub const EXPORT_SCHEMA_VERSION: u32 = 1;
pub const MAX_EXPORT_RECORDS: usize = MAX_CHAT_RECORDS as usize;
pub const MAX_EXPORT_CONTENT_BYTES: u64 = MAX_CHAT_BYTES;
pub const MAX_EXPORT_TURNS: usize = MAX_CHAT_SUBMISSIONS as usize;
/// JSON/control escaping plus bounded record/turn metadata, without clipping.
pub const MAX_EXPORT_BYTES: u64 =
    MAX_CHAT_BYTES * 6 + (MAX_CHAT_RECORDS + MAX_CHAT_SUBMISSIONS) * 2048 + 4096;
pub const EXPORT_PAGE_LIMIT: u32 = 32;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Json,
    Text,
}
impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Text => "txt",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExportRequest {
    pub kind: ExportKind,
    pub format: ExportFormat,
    pub chat_id: ChatId,
    pub generation: u64,
    pub expected_version: u64,
}
impl ExportRequest {
    pub fn validate(&self) -> Result<(), ExportError> {
        if [self.generation, self.expected_version]
            .iter()
            .any(|v| *v == 0 || *v > i64::MAX as u64)
        {
            Err(ExportError::InvalidRequest)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExportError {
    InvalidRequest,
    ConversationChanged,
    NotQuiescent,
    InvalidResponse,
    Limit,
    ReadFailed,
    InvalidDestination,
    DestinationExists,
    WriteFailed,
    PickerUnsupported,
}
impl ExportError {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidRequest => "chat_export_invalid_request",
            Self::ConversationChanged => "chat_export_changed",
            Self::NotQuiescent => "chat_export_busy",
            Self::InvalidResponse => "chat_export_invalid_response",
            Self::Limit => "chat_export_limit",
            Self::ReadFailed => "chat_export_read_failed",
            Self::InvalidDestination => "chat_export_invalid_destination",
            Self::DestinationExists => "chat_export_destination_exists",
            Self::WriteFailed => "chat_export_write_failed",
            Self::PickerUnsupported => "chat_export_picker_unsupported",
        }
    }
    pub fn message(self) -> &'static str {
        match self {
            Self::InvalidRequest => "Check the selected conversation and export format.",
            Self::ConversationChanged => {
                "The conversation changed. Refresh it before exporting again."
            }
            Self::NotQuiescent => {
                "Wait for the current conversation request to settle before exporting."
            }
            Self::InvalidResponse => {
                "The saved history could not be verified. No export was created."
            }
            Self::Limit => {
                "The retained history exceeds the bounded export limit. No export was created."
            }
            Self::ReadFailed => {
                "The saved history could not be read. Check the daemon before trying again."
            }
            Self::InvalidDestination => {
                "Choose a new ordinary file with the selected export extension."
            }
            Self::DestinationExists => {
                "That file already exists. Choose a new filename; exports never overwrite files."
            }
            Self::WriteFailed => {
                "The export file could not be completed. A partial file may remain at the selected destination."
            }
            Self::PickerUnsupported => {
                "The native export save dialog is unavailable on this platform."
            }
        }
    }
}

#[derive(Clone, Serialize)]
pub struct ExportRecord {
    pub record_id: Uuid,
    pub correlation_id: Uuid,
    pub sequence: u64,
    pub kind: ChatRecordKind,
    pub original_byte_count: u64,
    pub retained_byte_count: Option<u64>,
    pub truncated: bool,
    pub content_error: Option<ChatContentError>,
    pub text: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}
impl fmt::Debug for ExportRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExportRecord")
            .field("record_id", &self.record_id)
            .field("sequence", &self.sequence)
            .field("content_error", &self.content_error)
            .finish_non_exhaustive()
    }
}

/// A private copy; never log title/text or return the snapshot to the renderer.
#[derive(Clone, Serialize)]
pub struct ConversationSnapshot {
    pub schema_version: u32,
    pub kind: ExportKind,
    #[serde(with = "time::serde::rfc3339")]
    pub captured_at: OffsetDateTime,
    pub session: ChatSessionStatus,
    pub record_sequence_cutoff: u64,
    pub title: Option<String>,
    pub title_error: Option<ChatContentError>,
    pub turns: Vec<ExportTurn>,
    pub records: Vec<ExportRecord>,
    pub partial: bool,
}
impl fmt::Debug for ConversationSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConversationSnapshot")
            .field("chat_id", &self.session.chat_id)
            .field("records", &self.records.len())
            .field("partial", &self.partial)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportStatus {
    Saved,
    Cancelled,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExportReceipt {
    pub status: ExportStatus,
    pub kind: ExportKind,
    pub format: ExportFormat,
    pub chat_id: ChatId,
    pub generation: u64,
    pub version: u64,
    pub record_count: u64,
    pub byte_count: u64,
    pub partial: bool,
}
