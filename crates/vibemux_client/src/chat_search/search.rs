//! Stateless read cursors bound search options and canonical history positions.
use super::*;
use crate::chat_history::{HistoryClient, HistoryError};
use std::{collections::VecDeque, time::Duration};
use vibemux_types::chat::{MAX_CHAT_BYTES, MAX_CHAT_RECORD_BYTES};
use vibemuxd::{
    chat_session::api::{
        ChatContentCursor, ChatContentPage, ChatRecordStatus, ChatSessionStatus,
        MAX_CHAT_CHUNK_BYTES,
    },
    control::{ChatOwnerRequest, ChatOwnerResponse, ChatRequest, ChatResponse, ControlClient},
};

const READ_PAGE: u32 = 32;
const MAX_PAGE_CHATS: u32 = 128;
const SEARCH_TIMEOUT: Duration = Duration::from_secs(30);

struct ChatList {
    ids: Vec<ChatId>,
    next: Option<ChatId>,
    more: bool,
}
trait SearchSource {
    async fn list(
        &self,
        kind: ConversationKind,
        after: Option<ChatId>,
        archived: bool,
    ) -> Result<ChatList, SearchError>;
    async fn status(
        &self,
        kind: ConversationKind,
        id: ChatId,
    ) -> Result<Option<ChatSessionStatus>, SearchError>;
    async fn title(
        &self,
        kind: ConversationKind,
        id: ChatId,
    ) -> Result<(Option<String>, bool), SearchError>;
    async fn content(
        &self,
        kind: ConversationKind,
        id: ChatId,
        cursor: ChatContentCursor,
    ) -> Result<ChatContentPage, SearchError>;
}
struct AuthenticatedSource<'a>(&'a ControlClient);
fn history_error(error: HistoryError) -> SearchError {
    match error {
        HistoryError::InvalidResponse => SearchError::InvalidResponse,
        _ => SearchError::ReadFailed,
    }
}
impl SearchSource for AuthenticatedSource<'_> {
    async fn list(
        &self,
        kind: ConversationKind,
        after: Option<ChatId>,
        archived: bool,
    ) -> Result<ChatList, SearchError> {
        match kind {
            ConversationKind::Chat => match self
                .0
                .chat(&ChatRequest::List {
                    after,
                    limit: READ_PAGE,
                    archived,
                })
                .await
                .map_err(|_| SearchError::ReadFailed)?
            {
                ChatResponse::List {
                    sessions,
                    next_after,
                    more,
                } => Ok(ChatList {
                    ids: sessions.into_iter().map(|s| s.chat_id).collect(),
                    next: next_after,
                    more,
                }),
                _ => Err(SearchError::InvalidResponse),
            },
            ConversationKind::Workspace => match self
                .0
                .chat_owner(&ChatOwnerRequest::List {
                    after,
                    limit: READ_PAGE,
                    include_finalized: archived,
                })
                .await
                .map_err(|_| SearchError::ReadFailed)?
            {
                ChatOwnerResponse::List {
                    owners,
                    next_after,
                    more,
                } => Ok(ChatList {
                    ids: owners.into_iter().map(|o| o.chat_id).collect(),
                    next: next_after,
                    more,
                }),
                _ => Err(SearchError::InvalidResponse),
            },
        }
    }
    async fn status(
        &self,
        kind: ConversationKind,
        id: ChatId,
    ) -> Result<Option<ChatSessionStatus>, SearchError> {
        match HistoryClient::new(self.0, kind, id).status().await {
            Ok(s) => Ok(Some(s)),
            Err(HistoryError::NotFound) => Ok(None),
            Err(e) => Err(history_error(e)),
        }
    }
    async fn title(
        &self,
        kind: ConversationKind,
        id: ChatId,
    ) -> Result<(Option<String>, bool), SearchError> {
        let (title, error) = HistoryClient::new(self.0, kind, id)
            .title()
            .await
            .map_err(history_error)?;
        Ok((title, error.is_some()))
    }
    async fn content(
        &self,
        kind: ConversationKind,
        id: ChatId,
        cursor: ChatContentCursor,
    ) -> Result<ChatContentPage, SearchError> {
        HistoryClient::new(self.0, kind, id)
            .content(cursor)
            .await
            .map_err(history_error)
    }
}
impl SearchCursor {
    fn start(request: &SearchRequest) -> Self {
        Self {
            schema_version: SEARCH_SCHEMA_VERSION,
            query: request.query.clone(),
            case_sensitive: request.case_sensitive,
            scope: request.scope,
            include_archived: request.include_archived,
            kind: if request.scope == SearchScope::Workspace {
                ConversationKind::Workspace
            } else {
                ConversationKind::Chat
            },
            after_chat_id: None,
            chat_id: None,
            generation: 0,
            version: 0,
            record_sequence_cutoff: 0,
            record_count: 0,
            records_seen: 0,
            after_sequence: 0,
            title_done: false,
        }
    }
    fn finish_chat(&mut self, id: ChatId) {
        self.after_chat_id = Some(id);
        self.chat_id = None;
        self.generation = 0;
        self.version = 0;
        self.record_sequence_cutoff = 0;
        self.record_count = 0;
        self.records_seen = 0;
        self.after_sequence = 0;
        self.title_done = false;
    }
}

/// Explicit private search only. No index, model, database or native mutation.
pub async fn search_conversations(
    client: &ControlClient,
    request: &SearchRequest,
) -> Result<SearchPage, SearchError> {
    request.validate()?;
    tokio::time::timeout(
        SEARCH_TIMEOUT,
        search_from(&AuthenticatedSource(client), request),
    )
    .await
    .map_err(|_| SearchError::ReadFailed)?
}
fn page(
    hits: Vec<SearchHit>,
    stats: SearchStats,
    next_cursor: Option<SearchCursor>,
) -> Result<SearchPage, SearchError> {
    let result = SearchPage {
        schema_version: SEARCH_SCHEMA_VERSION,
        hits,
        stats,
        complete: next_cursor.is_none(),
        next_cursor,
    };
    // Every field is independently bounded before this bounded final encoding.
    let encoded = serde_json::to_vec(&result).map_err(|_| SearchError::InvalidResponse)?;
    if encoded.len() > MAX_SEARCH_RESPONSE_BYTES {
        return Err(SearchError::Limit);
    }
    Ok(result)
}
async fn search_from(
    source: &impl SearchSource,
    request: &SearchRequest,
) -> Result<SearchPage, SearchError> {
    request.validate()?;
    let mut cursor = request
        .cursor
        .clone()
        .unwrap_or_else(|| SearchCursor::start(request));
    let mut hits = Vec::new();
    let mut stats = SearchStats::default();
    let mut queue = VecDeque::new();
    let mut chat_visits = 0;
    loop {
        if hits.len() >= request.limit as usize
            || stats.records_scanned >= MAX_SEARCH_PAGE_RECORDS
            || chat_visits >= MAX_PAGE_CHATS
            || stats.bytes_scanned + MAX_CHAT_RECORD_BYTES > MAX_SEARCH_PAGE_BYTES
        {
            return page(hits, stats, Some(cursor));
        }
        let id = if let Some(id) = cursor.chat_id {
            id
        } else {
            if queue.is_empty() {
                let listed = source
                    .list(cursor.kind, cursor.after_chat_id, request.include_archived)
                    .await?;
                let mut previous = cursor.after_chat_id;
                if listed.ids.len() > READ_PAGE as usize
                    || listed.next != listed.ids.last().copied()
                    || (listed.more && listed.ids.is_empty())
                {
                    return Err(SearchError::InvalidResponse);
                }
                for id in &listed.ids {
                    if previous.is_some_and(|p| *id <= p) {
                        return Err(SearchError::InvalidResponse);
                    }
                    previous = Some(*id);
                }
                queue = listed.ids.into();
                if queue.is_empty() {
                    if cursor.kind == ConversationKind::Chat && request.scope == SearchScope::All {
                        cursor.kind = ConversationKind::Workspace;
                        cursor.after_chat_id = None;
                        continue;
                    }
                    return page(hits, stats, None);
                }
            }
            queue.pop_front().ok_or(SearchError::InvalidResponse)?
        };
        chat_visits += 1;
        if cursor.chat_id.is_none() {
            stats.chats_scanned += 1;
        }
        let Some(session) = source.status(cursor.kind, id).await? else {
            stats.changed_chats += 1;
            cursor.finish_chat(id);
            continue;
        };
        if session.chat_id != id
            || session.generation == 0
            || session.version == 0
            || session.record_count > vibemux_types::chat::MAX_CHAT_RECORDS
            || (session.record_count == 0) != (session.last_record_sequence == 0)
            || session.last_record_sequence > i64::MAX as u64
        {
            return Err(SearchError::InvalidResponse);
        }
        if cursor.chat_id.is_some() {
            if cursor.generation != session.generation
                || cursor.version != session.version
                || cursor.record_sequence_cutoff != session.last_record_sequence
                || cursor.record_count != session.record_count
            {
                stats.changed_chats += 1;
                cursor.finish_chat(id);
                continue;
            }
        } else {
            cursor.chat_id = Some(id);
            cursor.generation = session.generation;
            cursor.version = session.version;
            cursor.record_sequence_cutoff = session.last_record_sequence;
            cursor.record_count = session.record_count;
        }
        if session.content_purged {
            stats.purged_chats += 1;
            cursor.finish_chat(id);
            continue;
        }
        let hit_start = hits.len();
        let mut title = None;
        if !cursor.title_done {
            cursor.title_done = true;
            if session.has_title {
                let (read, unavailable) = source.title(cursor.kind, id).await?;
                if unavailable || read.is_none() {
                    stats.unavailable_titles += 1;
                }
                if let Some(value) = read {
                    if unavailable
                        || value.len() > vibemux_types::chat::MAX_CHAT_TITLE_BYTES as usize
                    {
                        return Err(SearchError::InvalidResponse);
                    }
                    stats.bytes_scanned += value.len() as u64;
                    if let Some(preview) =
                        find_preview(&value, &request.query, request.case_sensitive)?
                    {
                        hits.push(SearchHit {
                            kind: cursor.kind,
                            chat_id: id,
                            generation: session.generation,
                            version: session.version,
                            title: Some(value.clone()),
                            location: SearchTarget::Title,
                            preview,
                        });
                    }
                    title = Some(value);
                }
            }
        }
        let mut changed = false;
        while cursor.after_sequence < cursor.record_sequence_cutoff
            && hits.len() < request.limit as usize
            && stats.records_scanned < MAX_SEARCH_PAGE_RECORDS
            && stats.bytes_scanned + MAX_CHAT_RECORD_BYTES <= MAX_SEARCH_PAGE_BYTES
        {
            match read_record(
                source,
                cursor.kind,
                id,
                cursor.after_sequence,
                cursor.record_sequence_cutoff,
            )
            .await?
            {
                RecordRead::Changed => {
                    changed = true;
                    break;
                }
                RecordRead::Record { row, text, bytes } => {
                    stats.records_scanned += 1;
                    stats.bytes_scanned += bytes;
                    cursor.records_seen += 1;
                    if cursor.records_seen > cursor.record_count {
                        return Err(SearchError::InvalidResponse);
                    }
                    if let Some(text) = text {
                        if let Some(preview) =
                            find_preview(&text, &request.query, request.case_sensitive)?
                        {
                            hits.push(SearchHit {
                                kind: cursor.kind,
                                chat_id: id,
                                generation: session.generation,
                                version: session.version,
                                title: title.clone(),
                                location: SearchTarget::Record {
                                    record_id: row.record_id,
                                    sequence: row.sequence,
                                    record_kind: row.kind,
                                    correlation_id: *row.turn_id.as_uuid(),
                                },
                                preview,
                            });
                        }
                    } else {
                        stats.unavailable_records += 1;
                    }
                    cursor.after_sequence = row.sequence;
                }
            }
        }
        if !changed {
            changed = source.status(cursor.kind, id).await?.as_ref() != Some(&session);
        }
        if changed {
            hits.truncate(hit_start);
            stats.changed_chats += 1;
            cursor.finish_chat(id);
        } else if cursor.after_sequence == cursor.record_sequence_cutoff {
            if cursor.records_seen != cursor.record_count {
                return Err(SearchError::InvalidResponse);
            }
            cursor.finish_chat(id);
        }
        // A page that suspends mid-conversation retains its exact read cursor.
        if cursor.chat_id.is_some() {
            return page(hits, stats, Some(cursor));
        }
    }
}

enum RecordRead {
    Changed,
    Record {
        row: ChatRecordStatus,
        text: Option<String>,
        bytes: u64,
    },
}
async fn read_record(
    source: &impl SearchSource,
    kind: ConversationKind,
    id: ChatId,
    after_sequence: u64,
    cutoff: u64,
) -> Result<RecordRead, SearchError> {
    let mut cursor = ChatContentCursor {
        after_sequence,
        byte_offset: 0,
    };
    let mut text = String::new();
    let mut metadata: Option<ChatRecordStatus> = None;
    let mut bytes = 0;
    loop {
        let page = source.content(kind, id, cursor).await?;
        if page.chat_id != id || page.last_record_sequence != cutoff {
            return Ok(RecordRead::Changed);
        }
        let row = page.record.as_ref().ok_or(SearchError::InvalidResponse)?;
        if row.chat_id != id
            || row.sequence <= after_sequence
            || row.sequence > cutoff
            || row.record_id.is_nil()
            || row.original_byte_count > MAX_CHAT_BYTES
            || row
                .retained_byte_count
                .is_some_and(|size| size > MAX_CHAT_RECORD_BYTES || size > row.original_byte_count)
            || row.truncated
                != row
                    .retained_byte_count
                    .is_some_and(|size| size < row.original_byte_count)
        {
            return Err(SearchError::InvalidResponse);
        }
        if let Some(first) = &metadata {
            let mut incoming = row.clone();
            incoming.content_available = first.content_available;
            if incoming != *first {
                return Err(SearchError::InvalidResponse);
            }
        } else {
            metadata = Some(row.clone());
        }
        if page.content_error.is_some() {
            if page.text.is_some()
                || row.content_available
                || !page.complete
                || page.next_cursor
                    != (ChatContentCursor {
                        after_sequence: row.sequence,
                        byte_offset: 0,
                    })
                || page.more != (row.sequence < cutoff)
            {
                return Err(SearchError::InvalidResponse);
            }
            return Ok(RecordRead::Record {
                row: row.clone(),
                text: None,
                bytes,
            });
        }
        let chunk = page.text.as_ref().ok_or(SearchError::InvalidResponse)?;
        if !row.content_available
            || chunk.len() > MAX_CHAT_CHUNK_BYTES
            || text.len() + chunk.len() > MAX_CHAT_RECORD_BYTES as usize
        {
            return Err(SearchError::InvalidResponse);
        }
        bytes += chunk.len() as u64;
        text.push_str(chunk);
        if page.complete {
            if row.retained_byte_count != Some(text.len() as u64)
                || page.next_cursor
                    != (ChatContentCursor {
                        after_sequence: row.sequence,
                        byte_offset: 0,
                    })
                || page.more != (row.sequence < cutoff)
            {
                return Err(SearchError::InvalidResponse);
            }
            return Ok(RecordRead::Record {
                row: row.clone(),
                text: Some(text),
                bytes,
            });
        }
        if !page.more
            || chunk.is_empty()
            || page.next_cursor.after_sequence != after_sequence
            || page.next_cursor.byte_offset as usize != text.len()
            || page.next_cursor.byte_offset <= cursor.byte_offset
        {
            return Err(SearchError::InvalidResponse);
        }
        cursor = page.next_cursor;
    }
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
