//! Snapshot reads stay on the authenticated API; private storage is never opened.
use super::*;
use crate::chat_history::{HistoryClient, HistoryError};
use std::{collections::HashSet, time::Duration};
use vibemux_types::chat::{ChatState, MAX_CHAT_RECORD_BYTES, MAX_CHAT_TITLE_BYTES};
#[cfg(test)]
use vibemux_types::{
    ChatTurnId, RunId, TaskId,
    chat::{ChatTurnOutcome, ChatTurnPhase},
};
#[cfg(test)]
use vibemuxd::chat_session::api::ChatRecordStatus;
use vibemuxd::chat_session::api::{ChatContentCursor, ChatContentPage, MAX_CHAT_CHUNK_BYTES};
use vibemuxd::control::ControlClient;

const EXPORT_READ_TIMEOUT: Duration = Duration::from_secs(120);

enum HistoryRead {
    Status,
    Title,
    Turns(u64),
    Content(ChatContentCursor),
}
enum HistoryPage {
    Status(ChatSessionStatus),
    Title(Option<String>, Option<ChatContentError>),
    Turns(Vec<(u64, ExportTurn)>, u64, bool),
    Content(ChatContentPage),
}
trait HistorySource {
    async fn read(&self, read: HistoryRead) -> Result<HistoryPage, ExportError>;
}
struct ControlSource<'a> {
    client: &'a ControlClient,
    request: &'a ExportRequest,
}

fn export_error(error: HistoryError) -> ExportError {
    match error {
        HistoryError::ReadFailed => ExportError::ReadFailed,
        HistoryError::InvalidResponse | HistoryError::NotFound => ExportError::InvalidResponse,
    }
}
impl HistorySource for ControlSource<'_> {
    async fn read(&self, read: HistoryRead) -> Result<HistoryPage, ExportError> {
        let reader = HistoryClient::new(self.client, self.request.kind, self.request.chat_id);
        match read {
            HistoryRead::Status => reader
                .status()
                .await
                .map(HistoryPage::Status)
                .map_err(export_error),
            HistoryRead::Title => reader
                .title()
                .await
                .map(|(title, error)| HistoryPage::Title(title, error))
                .map_err(export_error),
            HistoryRead::Turns(after_sequence) => reader
                .turns(after_sequence, EXPORT_PAGE_LIMIT)
                .await
                .map(|page| HistoryPage::Turns(page.turns, page.next_sequence, page.more))
                .map_err(export_error),
            HistoryRead::Content(cursor) => reader
                .content(cursor)
                .await
                .map(HistoryPage::Content)
                .map_err(export_error),
        }
    }
}

fn require_session(
    session: &ChatSessionStatus,
    request: &ExportRequest,
) -> Result<(), ExportError> {
    if session.chat_id != request.chat_id
        || session.generation != request.generation
        || session.version != request.expected_version
    {
        return Err(ExportError::ConversationChanged);
    }
    if session.active_request_id.is_some()
        || !matches!(
            session.state,
            ChatState::Ready | ChatState::Suspended | ChatState::Closed | ChatState::Archived
        )
    {
        return Err(ExportError::NotQuiescent);
    }
    if session.record_count > MAX_EXPORT_RECORDS as u64 {
        return Err(ExportError::Limit);
    }
    if (session.record_count == 0) != (session.last_record_sequence == 0) {
        return Err(ExportError::InvalidResponse);
    }
    Ok(())
}

/// A read-only bounded snapshot. A changed session aborts without a destination write.
pub async fn collect_conversation(
    client: &ControlClient,
    request: &ExportRequest,
) -> Result<ConversationSnapshot, ExportError> {
    request.validate()?;
    tokio::time::timeout(
        EXPORT_READ_TIMEOUT,
        collect_from(&ControlSource { client, request }, request),
    )
    .await
    .map_err(|_| ExportError::ReadFailed)?
}

/// Revalidate immediately before native file creation, after formatting the copy.
pub async fn verify_snapshot(
    client: &ControlClient,
    snapshot: &ConversationSnapshot,
) -> Result<(), ExportError> {
    let request = ExportRequest {
        kind: snapshot.kind,
        format: ExportFormat::Json,
        chat_id: snapshot.session.chat_id,
        generation: snapshot.session.generation,
        expected_version: snapshot.session.version,
    };
    verify_from(
        &ControlSource {
            client,
            request: &request,
        },
        &snapshot.session,
    )
    .await
}
async fn verify_from(
    source: &impl HistorySource,
    expected: &ChatSessionStatus,
) -> Result<(), ExportError> {
    match source.read(HistoryRead::Status).await? {
        HistoryPage::Status(session) if session == *expected => Ok(()),
        HistoryPage::Status(_) => Err(ExportError::ConversationChanged),
        _ => Err(ExportError::InvalidResponse),
    }
}
async fn collect_from(
    source: &impl HistorySource,
    request: &ExportRequest,
) -> Result<ConversationSnapshot, ExportError> {
    request.validate()?;
    let HistoryPage::Status(session) = source.read(HistoryRead::Status).await? else {
        return Err(ExportError::InvalidResponse);
    };
    require_session(&session, request)?;
    let (title, title_error) = if session.content_purged {
        (None, Some(ChatContentError::Purged))
    } else if session.has_title {
        match source.read(HistoryRead::Title).await? {
            HistoryPage::Title(Some(title), None) if title.len() as u64 <= MAX_CHAT_TITLE_BYTES => {
                (Some(title), None)
            }
            HistoryPage::Title(None, error) => {
                (None, Some(error.unwrap_or(ChatContentError::Unavailable)))
            }
            _ => return Err(ExportError::InvalidResponse),
        }
    } else {
        (None, None)
    };
    let mut turns = Vec::new();
    let mut after = 0;
    let mut ids = HashSet::new();
    let mut requests = HashSet::new();
    loop {
        let HistoryPage::Turns(page, next, more) = source.read(HistoryRead::Turns(after)).await?
        else {
            return Err(ExportError::InvalidResponse);
        };
        if page.len() > EXPORT_PAGE_LIMIT as usize || (page.is_empty() && more) {
            return Err(ExportError::InvalidResponse);
        }
        for (sequence, turn) in page {
            if sequence <= after
                || turn.generation == 0
                || turn.generation > session.generation
                || turn.request_id.is_nil()
                || turn.correlation_id.is_nil()
                || !ids.insert(turn.correlation_id)
                || !requests.insert(turn.request_id)
            {
                return Err(ExportError::InvalidResponse);
            }
            if turns.len() == MAX_EXPORT_TURNS {
                return Err(ExportError::Limit);
            }
            after = sequence;
            turns.push(turn);
        }
        if next != after {
            return Err(ExportError::InvalidResponse);
        }
        if !more {
            break;
        }
    }
    let mut cursor = ChatContentCursor::default();
    let mut records = Vec::new();
    let mut total = 0u64;
    let mut record_ids = HashSet::new();
    loop {
        let HistoryPage::Content(mut page) = source.read(HistoryRead::Content(cursor)).await?
        else {
            return Err(ExportError::InvalidResponse);
        };
        check_page(&page, &session)?;
        let Some(metadata) = page.record.clone() else {
            if cursor.byte_offset != 0
                || cursor.after_sequence != session.last_record_sequence
                || page.text.is_some()
                || page.content_error.is_some()
                || !page.complete
                || page.more
                || page.next_cursor != cursor
            {
                return Err(ExportError::InvalidResponse);
            }
            break;
        };
        if records.len() == MAX_EXPORT_RECORDS
            || metadata.chat_id != session.chat_id
            || metadata.sequence <= cursor.after_sequence
            || metadata.sequence > session.last_record_sequence
            || metadata.record_id.is_nil()
            || !record_ids.insert(metadata.record_id)
            || !ids.contains(metadata.turn_id.as_uuid())
            || metadata.original_byte_count > MAX_CHAT_BYTES
            || metadata.retained_byte_count.is_some_and(|size| {
                size > metadata.original_byte_count || size > MAX_CHAT_RECORD_BYTES
            })
            || metadata.truncated
                != metadata
                    .retained_byte_count
                    .is_some_and(|size| size < metadata.original_byte_count)
        {
            return Err(ExportError::InvalidResponse);
        }
        let mut text = String::new();
        let mut error;
        loop {
            error = page.content_error;
            if session.content_purged
                && (page.text.is_some() || error != Some(ChatContentError::Purged))
            {
                return Err(ExportError::InvalidResponse);
            }
            if let Some(code) = error {
                if page.text.is_some()
                    || page.record.as_ref().is_none_or(|row| row.content_available)
                    || !page.complete
                {
                    return Err(ExportError::InvalidResponse);
                }
                text.clear();
                error = Some(code);
            } else {
                let chunk = page.text.as_ref().ok_or(ExportError::InvalidResponse)?;
                if !page.record.as_ref().is_some_and(|r| r.content_available)
                    || chunk.len() > MAX_CHAT_CHUNK_BYTES
                {
                    return Err(ExportError::InvalidResponse);
                }
                let end = text
                    .len()
                    .checked_add(chunk.len())
                    .ok_or(ExportError::Limit)?;
                if end as u64 > MAX_CHAT_RECORD_BYTES
                    || total
                        .checked_add(end as u64)
                        .is_none_or(|size| size > MAX_EXPORT_CONTENT_BYTES)
                {
                    return Err(ExportError::Limit);
                }
                text.push_str(chunk);
                if page.complete && metadata.retained_byte_count != Some(text.len() as u64) {
                    return Err(ExportError::InvalidResponse);
                }
            }
            if page.complete {
                if page.next_cursor
                    != (ChatContentCursor {
                        after_sequence: metadata.sequence,
                        byte_offset: 0,
                    })
                    || page.more != (metadata.sequence < session.last_record_sequence)
                {
                    return Err(ExportError::InvalidResponse);
                }
                cursor = page.next_cursor;
                break;
            }
            if !page.more
                || page.next_cursor.after_sequence != cursor.after_sequence
                || page.next_cursor.byte_offset as usize != text.len()
                || page.next_cursor.byte_offset <= cursor.byte_offset
                || text.is_empty()
            {
                return Err(ExportError::InvalidResponse);
            }
            cursor = page.next_cursor;
            let HistoryPage::Content(next) = source.read(HistoryRead::Content(cursor)).await?
            else {
                return Err(ExportError::InvalidResponse);
            };
            check_page(&next, &session)?;
            let mut incoming = next.record.clone().ok_or(ExportError::InvalidResponse)?;
            incoming.content_available = metadata.content_available;
            if incoming != metadata {
                return Err(ExportError::InvalidResponse);
            }
            page = next;
        }
        total = total
            .checked_add(text.len() as u64)
            .ok_or(ExportError::Limit)?;
        records.push(ExportRecord {
            record_id: metadata.record_id,
            correlation_id: *metadata.turn_id.as_uuid(),
            sequence: metadata.sequence,
            kind: metadata.kind,
            original_byte_count: metadata.original_byte_count,
            retained_byte_count: metadata.retained_byte_count,
            truncated: metadata.truncated,
            content_error: error,
            text: if error.is_some() { None } else { Some(text) },
            created_at: metadata.created_at,
        });
        if !page.more {
            break;
        }
    }
    if records.len() as u64 != session.record_count
        || cursor.after_sequence != session.last_record_sequence
    {
        return Err(ExportError::InvalidResponse);
    }
    verify_from(source, &session).await?;
    let partial = title_error.is_some()
        || session.content_purged
        || turns.iter().any(|t| !t.output_complete)
        || records
            .iter()
            .any(|r| r.truncated || r.content_error.is_some());
    Ok(ConversationSnapshot {
        schema_version: EXPORT_SCHEMA_VERSION,
        kind: request.kind,
        captured_at: OffsetDateTime::now_utc(),
        record_sequence_cutoff: session.last_record_sequence,
        session,
        title,
        title_error,
        turns,
        records,
        partial,
    })
}
fn check_page(page: &ChatContentPage, session: &ChatSessionStatus) -> Result<(), ExportError> {
    if page.chat_id != session.chat_id || page.last_record_sequence != session.last_record_sequence
    {
        Err(ExportError::ConversationChanged)
    } else {
        Ok(())
    }
}

#[cfg(test)]
#[path = "collect_tests.rs"]
mod tests;
