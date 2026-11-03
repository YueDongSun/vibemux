//! Explicit project-scoped literal search over authenticated retained history.
pub use crate::chat_history::ConversationKind;
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;
use vibemux_types::{ChatId, chat::ChatRecordKind};

mod match_text;
mod search;
pub use match_text::find_preview;
pub use search::search_conversations;

pub const SEARCH_SCHEMA_VERSION: u32 = 1;
pub const MAX_SEARCH_QUERY_BYTES: usize = 256;
pub const MAX_SEARCH_RESULTS: u32 = 25;
pub const MAX_SEARCH_PAGE_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_SEARCH_PAGE_RECORDS: u32 = 256;
pub const MAX_SEARCH_PREVIEW_CHARACTERS: usize = 80;
pub const MAX_SEARCH_RESPONSE_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchScope {
    All,
    Saved,
    Workspace,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SearchRequest {
    pub query: String,
    pub case_sensitive: bool,
    pub scope: SearchScope,
    pub include_archived: bool,
    pub limit: u32,
    pub cursor: Option<SearchCursor>,
}
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SearchCursor {
    pub schema_version: u32,
    pub query: String,
    pub case_sensitive: bool,
    pub scope: SearchScope,
    pub include_archived: bool,
    pub kind: ConversationKind,
    pub after_chat_id: Option<ChatId>,
    pub chat_id: Option<ChatId>,
    pub generation: u64,
    pub version: u64,
    pub record_sequence_cutoff: u64,
    pub record_count: u64,
    pub records_seen: u64,
    pub after_sequence: u64,
    pub title_done: bool,
}
impl SearchRequest {
    pub fn validate(&self) -> Result<(), SearchError> {
        if self.query.trim().is_empty()
            || self.query.len() > MAX_SEARCH_QUERY_BYTES
            || self.limit == 0
            || self.limit > MAX_SEARCH_RESULTS
        {
            return Err(SearchError::InvalidRequest);
        }
        if let Some(c) = &self.cursor
            && (c.schema_version != SEARCH_SCHEMA_VERSION
                || c.query != self.query
                || c.case_sensitive != self.case_sensitive
                || c.scope != self.scope
                || c.include_archived != self.include_archived
                || c.after_sequence > c.record_sequence_cutoff
                || c.record_count > vibemux_types::chat::MAX_CHAT_RECORDS
                || c.records_seen > c.record_count
                || c.record_sequence_cutoff > i64::MAX as u64
                || c.version > i64::MAX as u64
                || c.generation > i64::MAX as u64
                || (c.chat_id.is_some() && (c.version == 0 || c.generation == 0))
                || (c.chat_id.is_none()
                    && (c.version != 0
                        || c.generation != 0
                        || c.record_sequence_cutoff != 0
                        || c.record_count != 0
                        || c.records_seen != 0
                        || c.after_sequence != 0
                        || c.title_done))
                || matches!(
                    (self.scope, c.kind),
                    (SearchScope::Saved, ConversationKind::Workspace)
                        | (SearchScope::Workspace, ConversationKind::Chat)
                ))
        {
            return Err(SearchError::InvalidRequest);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchError {
    InvalidRequest,
    InvalidResponse,
    ReadFailed,
    Limit,
}
impl SearchError {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidRequest => "chat_search_invalid_request",
            Self::InvalidResponse => "chat_search_invalid_response",
            Self::ReadFailed => "chat_search_read_failed",
            Self::Limit => "chat_search_limit",
        }
    }
    pub fn message(self) -> &'static str {
        match self {
            Self::InvalidRequest => {
                "Enter a literal search within 256 UTF-8 bytes and start again if its options changed."
            }
            Self::InvalidResponse => {
                "The conversation search response could not be verified. Start a new search."
            }
            Self::ReadFailed => {
                "Conversation history could not be read. Check the project's daemon before searching again."
            }
            Self::Limit => {
                "The search exceeded its bounded result size. Use a more specific query."
            }
        }
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SearchPreview {
    pub before: String,
    pub matched: String,
    pub after: String,
    pub match_count: u32,
    pub byte_offset: u32,
    pub end_byte_offset: u32,
    pub truncated: bool,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "target", rename_all = "snake_case", deny_unknown_fields)]
pub enum SearchTarget {
    Title,
    Record {
        record_id: Uuid,
        sequence: u64,
        record_kind: ChatRecordKind,
        correlation_id: Uuid,
    },
}
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SearchHit {
    pub kind: ConversationKind,
    pub chat_id: ChatId,
    pub generation: u64,
    pub version: u64,
    pub title: Option<String>,
    pub location: SearchTarget,
    pub preview: SearchPreview,
}
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SearchStats {
    pub chats_scanned: u32,
    pub records_scanned: u32,
    pub bytes_scanned: u64,
    pub unavailable_records: u32,
    pub unavailable_titles: u32,
    pub purged_chats: u32,
    pub changed_chats: u32,
}
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SearchPage {
    pub schema_version: u32,
    pub hits: Vec<SearchHit>,
    pub stats: SearchStats,
    pub next_cursor: Option<SearchCursor>,
    pub complete: bool,
}
impl fmt::Debug for SearchRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SearchRequest")
            .field("scope", &self.scope)
            .field("query_bytes", &self.query.len())
            .finish_non_exhaustive()
    }
}
impl fmt::Debug for SearchCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SearchCursor")
            .field("kind", &self.kind)
            .field("chat_id", &self.chat_id)
            .field("after_sequence", &self.after_sequence)
            .finish_non_exhaustive()
    }
}
impl fmt::Debug for SearchPreview {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SearchPreview")
            .field("byte_offset", &self.byte_offset)
            .field("match_count", &self.match_count)
            .finish_non_exhaustive()
    }
}
impl fmt::Debug for SearchHit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SearchHit")
            .field("chat_id", &self.chat_id)
            .field("location", &self.location)
            .finish_non_exhaustive()
    }
}
impl fmt::Debug for SearchPage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SearchPage")
            .field("hits", &self.hits.len())
            .field("stats", &self.stats)
            .field("complete", &self.complete)
            .finish_non_exhaustive()
    }
}
