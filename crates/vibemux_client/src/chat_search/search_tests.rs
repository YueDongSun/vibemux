use super::*;
use std::cell::{Cell, RefCell};
use vibemux_types::{
    ChatTurnId, ProjectId,
    chat::{ChatHarness, ChatState},
};
use vibemuxd::chat_session::api::ChatContentError;

struct Record {
    row: ChatRecordStatus,
    text: String,
    unavailable_later: bool,
}
struct Chat {
    kind: ConversationKind,
    session: ChatSessionStatus,
    title: Option<String>,
    title_unavailable: bool,
    records: Vec<Record>,
}
struct Fake {
    chats: Vec<Chat>,
    status_calls: RefCell<std::collections::HashMap<ChatId, u32>>,
    change_after_first: bool,
    content_reads: Cell<u32>,
    title_reads: Cell<u32>,
    chunk_bytes: usize,
    bad_cursor: bool,
}
fn chat(kind: ConversationKind, id: u128, texts: &[&str]) -> Chat {
    let chat_id = ChatId::from_uuid(Uuid::from_u128(id)).expect("chat ID");
    let now = time::OffsetDateTime::UNIX_EPOCH;
    let records = texts
        .iter()
        .enumerate()
        .map(|(i, text)| Record {
            row: ChatRecordStatus {
                record_id: Uuid::new_v4(),
                chat_id,
                turn_id: ChatTurnId::new(),
                sequence: 10 + (i as u64) * 17,
                kind: ChatRecordKind::User,
                original_byte_count: text.len() as u64,
                retained_byte_count: Some(text.len() as u64),
                content_available: true,
                truncated: false,
                created_at: now,
            },
            text: (*text).to_owned(),
            unavailable_later: false,
        })
        .collect::<Vec<_>>();
    Chat {
        kind,
        session: ChatSessionStatus {
            schema_version: 1,
            chat_id,
            project_id: ProjectId::new(),
            harness: ChatHarness::Codex,
            state: ChatState::Closed,
            generation: 1,
            version: 3,
            active_request_id: None,
            record_count: records.len() as u64,
            last_record_sequence: records.last().map_or(0, |r| r.row.sequence),
            content_purged: false,
            has_title: true,
            created_at: now,
            updated_at: now,
        },
        title: Some("Fixture title".into()),
        title_unavailable: false,
        records,
    }
}
fn fake(chats: Vec<Chat>) -> Fake {
    Fake {
        chats,
        status_calls: RefCell::new(std::collections::HashMap::new()),
        change_after_first: false,
        content_reads: Cell::new(0),
        title_reads: Cell::new(0),
        chunk_bytes: 8192,
        bad_cursor: false,
    }
}
fn request(scope: SearchScope) -> SearchRequest {
    SearchRequest {
        query: "needle".into(),
        case_sensitive: false,
        scope,
        include_archived: false,
        limit: 25,
        cursor: None,
    }
}
impl SearchSource for Fake {
    async fn list(
        &self,
        kind: ConversationKind,
        after: Option<ChatId>,
        archived: bool,
    ) -> Result<ChatList, SearchError> {
        let mut ids = self
            .chats
            .iter()
            .filter(|c| {
                c.kind == kind
                    && (archived || c.session.state != ChatState::Archived)
                    && after.is_none_or(|id| c.session.chat_id > id)
            })
            .map(|c| c.session.chat_id)
            .collect::<Vec<_>>();
        ids.sort();
        let more = ids.len() > READ_PAGE as usize;
        ids.truncate(READ_PAGE as usize);
        Ok(ChatList {
            next: ids.last().copied(),
            ids,
            more,
        })
    }
    async fn status(
        &self,
        kind: ConversationKind,
        id: ChatId,
    ) -> Result<Option<ChatSessionStatus>, SearchError> {
        let Some(chat) = self
            .chats
            .iter()
            .find(|c| c.kind == kind && c.session.chat_id == id)
        else {
            return Ok(None);
        };
        let mut calls = self.status_calls.borrow_mut();
        let seen = calls.entry(id).or_default();
        *seen += 1;
        let mut session = chat.session.clone();
        if self.change_after_first && *seen > 1 {
            session.version += 1;
        }
        Ok(Some(session))
    }
    async fn title(
        &self,
        kind: ConversationKind,
        id: ChatId,
    ) -> Result<(Option<String>, bool), SearchError> {
        self.title_reads.set(self.title_reads.get() + 1);
        let c = self
            .chats
            .iter()
            .find(|c| c.kind == kind && c.session.chat_id == id)
            .ok_or(SearchError::ReadFailed)?;
        Ok((c.title.clone(), c.title_unavailable))
    }
    async fn content(
        &self,
        kind: ConversationKind,
        id: ChatId,
        cursor: ChatContentCursor,
    ) -> Result<ChatContentPage, SearchError> {
        self.content_reads.set(self.content_reads.get() + 1);
        let c = self
            .chats
            .iter()
            .find(|c| c.kind == kind && c.session.chat_id == id)
            .ok_or(SearchError::ReadFailed)?;
        let record = c
            .records
            .iter()
            .find(|r| r.row.sequence > cursor.after_sequence)
            .ok_or(SearchError::InvalidResponse)?;
        let unavailable = record.unavailable_later && cursor.byte_offset > 0;
        let start = cursor.byte_offset as usize;
        let mut end = (start + self.chunk_bytes).min(record.text.len());
        while !record.text.is_char_boundary(end) {
            end -= 1;
        }
        let complete = unavailable || end == record.text.len();
        let mut row = record.row.clone();
        row.content_available = !unavailable;
        let mut next_cursor = if complete {
            ChatContentCursor {
                after_sequence: row.sequence,
                byte_offset: 0,
            }
        } else {
            ChatContentCursor {
                byte_offset: end as u32,
                ..cursor
            }
        };
        if self.bad_cursor {
            next_cursor.byte_offset = cursor.byte_offset;
        }
        Ok(ChatContentPage {
            chat_id: id,
            record: Some(row.clone()),
            text: if unavailable {
                None
            } else {
                Some(
                    record
                        .text
                        .get(start..end)
                        .ok_or(SearchError::InvalidResponse)?
                        .into(),
                )
            },
            next_cursor,
            complete,
            more: !complete || row.sequence < c.session.last_record_sequence,
            last_record_sequence: c.session.last_record_sequence,
            content_error: unavailable.then_some(ChatContentError::Unavailable),
        })
    }
}

#[tokio::test]
async fn all_scope_searches_saved_and_workspace_titles_and_messages_without_duplicate_sources() {
    let mut a = chat(
        ConversationKind::Chat,
        1,
        &["prefix NEEDLE suffix", "unrelated"],
    );
    a.title = Some("needle title".into());
    let b = chat(ConversationKind::Workspace, 2, &["workspace needle"]);
    let source = fake(vec![a, b]);
    let result = search_from(&source, &request(SearchScope::All))
        .await
        .expect("all retained sources");
    assert!(result.complete && result.next_cursor.is_none());
    assert_eq!(result.hits.len(), 3);
    assert_eq!(result.stats.records_scanned, 3);
    assert_eq!(
        result
            .hits
            .iter()
            .filter(|h| h.kind == ConversationKind::Workspace)
            .count(),
        1
    );
    assert!(matches!(result.hits[0].location, SearchTarget::Title));
    assert!(!format!("{result:?}").contains("NEEDLE"));
}
#[tokio::test]
async fn continuation_preserves_whole_record_progress_and_exact_coverage() {
    let source = fake(vec![chat(
        ConversationKind::Chat,
        1,
        &["needle one", "needle two"],
    )]);
    let mut query = request(SearchScope::Saved);
    query.limit = 1;
    let first = search_from(&source, &query).await.expect("first page");
    assert_eq!(first.hits.len(), 1);
    let cursor = first.next_cursor.clone().expect("continuation");
    assert_eq!(cursor.records_seen, 1);
    assert_eq!(cursor.record_count, 2);
    assert_eq!(cursor.after_sequence, 10);
    query.cursor = Some(cursor);
    let second = search_from(&source, &query).await.expect("second page");
    assert_eq!(second.hits.len(), 1);
    assert_ne!(second.hits[0].location, first.hits[0].location);
    query.cursor = second.next_cursor;
    let final_page = search_from(&source, &query).await.expect("confirm end");
    assert!(final_page.complete && final_page.hits.is_empty());
}
#[tokio::test]
async fn changed_history_drops_its_new_previews_and_reports_a_gap() {
    let mut source = fake(vec![chat(ConversationKind::Chat, 1, &["needle"])]);
    source.change_after_first = true;
    let result = search_from(&source, &request(SearchScope::Saved))
        .await
        .expect("changed source gap");
    assert!(result.hits.is_empty() && result.complete);
    assert_eq!(result.stats.changed_chats, 1);
}
#[tokio::test]
async fn purged_history_is_not_read_and_archived_history_requires_the_option() {
    let mut a = chat(ConversationKind::Chat, 1, &["needle"]);
    a.session.content_purged = true;
    a.session.has_title = false;
    let mut b = chat(ConversationKind::Chat, 2, &["needle"]);
    b.session.state = ChatState::Archived;
    let source = fake(vec![a, b]);
    let first = search_from(&source, &request(SearchScope::Saved))
        .await
        .expect("active only");
    assert_eq!(first.stats.purged_chats, 1);
    assert_eq!(source.content_reads.get(), 0);
    assert_eq!(source.title_reads.get(), 0);
    let mut query = request(SearchScope::Saved);
    query.include_archived = true;
    let next = search_from(&source, &query)
        .await
        .expect("archived included");
    assert_eq!(next.hits.len(), 1);
}
#[tokio::test]
async fn utf8_matches_across_chunks_and_unavailable_suffix_never_exposes_a_prefix() {
    let mut a = chat(ConversationKind::Chat, 1, &["中文needle"]);
    a.records[0].unavailable_later = true;
    let mut source = fake(vec![a]);
    source.chunk_bytes = 3;
    let result = search_from(&source, &request(SearchScope::Saved))
        .await
        .expect("unavailable record");
    assert!(result.hits.is_empty());
    assert_eq!(result.stats.unavailable_records, 1);
    source.chats[0].records[0].unavailable_later = false;
    let mut query = request(SearchScope::Saved);
    query.query = "中文".into();
    let result = search_from(&source, &query).await.expect("UTF8 boundary");
    assert_eq!(result.hits[0].preview.matched, "中文");
}
#[tokio::test]
async fn stalled_chunks_and_missing_canonical_metadata_are_refused() {
    let mut source = fake(vec![chat(ConversationKind::Chat, 1, &["needle record"])]);
    source.chunk_bytes = 3;
    source.bad_cursor = true;
    assert_eq!(
        search_from(&source, &request(SearchScope::Saved))
            .await
            .expect_err("stalled cursor"),
        SearchError::InvalidResponse
    );
    source.bad_cursor = false;
    source.chats[0].session.record_count = 2;
    assert_eq!(
        search_from(&source, &request(SearchScope::Saved))
            .await
            .expect_err("missing metadata"),
        SearchError::InvalidResponse
    );
}
#[tokio::test]
async fn an_unavailable_title_is_a_visible_gap_and_does_not_hide_message_matches() {
    let mut a = chat(ConversationKind::Chat, 1, &["needle body"]);
    a.title = None;
    a.title_unavailable = true;
    let source = fake(vec![a]);
    let result = search_from(&source, &request(SearchScope::Saved))
        .await
        .expect("title unavailable");
    assert_eq!(result.stats.unavailable_titles, 1);
    assert_eq!(result.hits.len(), 1);
    assert!(result.hits[0].title.is_none());
}
#[test]
fn cursor_options_and_counts_are_bound_without_logging_query_text() {
    let mut query = request(SearchScope::All);
    query.query = "PRIVATE_QUERY".into();
    let mut cursor = SearchCursor::start(&query);
    query.cursor = Some(cursor.clone());
    assert!(query.validate().is_ok());
    cursor.query = "different".into();
    query.cursor = Some(cursor);
    assert_eq!(query.validate(), Err(SearchError::InvalidRequest));
    assert!(!format!("{query:?}").contains("PRIVATE_QUERY"));
    assert!(!format!("{:?}", query.cursor).contains("different"));
}
