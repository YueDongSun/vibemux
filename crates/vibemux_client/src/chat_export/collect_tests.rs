use super::*;
use std::{cell::RefCell, collections::VecDeque};
use vibemux_types::{
    ProjectId,
    chat::{ChatHarness, ChatRecordKind},
};

struct Fake {
    pages: RefCell<VecDeque<HistoryPage>>,
    reads: RefCell<Vec<ChatContentCursor>>,
}
impl HistorySource for Fake {
    async fn read(&self, read: HistoryRead) -> Result<HistoryPage, ExportError> {
        if let HistoryRead::Content(cursor) = read {
            self.reads.borrow_mut().push(cursor);
        }
        self.pages
            .borrow_mut()
            .pop_front()
            .ok_or(ExportError::ReadFailed)
    }
}
fn fixture() -> (
    ExportRequest,
    ChatSessionStatus,
    ExportTurn,
    ChatRecordStatus,
) {
    let chat_id = ChatId::new();
    let correlation = ChatTurnId::new();
    let now = OffsetDateTime::UNIX_EPOCH;
    (
        ExportRequest {
            kind: ExportKind::Chat,
            format: ExportFormat::Json,
            chat_id,
            generation: 1,
            expected_version: 4,
        },
        ChatSessionStatus {
            schema_version: 1,
            chat_id,
            project_id: ProjectId::new(),
            harness: ChatHarness::Claude,
            state: ChatState::Closed,
            generation: 1,
            version: 4,
            active_request_id: None,
            record_count: 1,
            last_record_sequence: 17,
            content_purged: false,
            has_title: false,
            created_at: now,
            updated_at: now,
        },
        ExportTurn {
            correlation_id: *correlation.as_uuid(),
            request_id: Uuid::new_v4(),
            task_id: TaskId::new(),
            run_id: RunId::new(),
            generation: 1,
            phase: ChatTurnPhase::Completed,
            outcome: Some(ChatTurnOutcome::Completed),
            error_code: None,
            correlated_terminal: true,
            output_complete: true,
            created_at: now,
            updated_at: now,
        },
        ChatRecordStatus {
            record_id: Uuid::new_v4(),
            chat_id,
            turn_id: correlation,
            sequence: 17,
            kind: ChatRecordKind::Assistant,
            original_byte_count: 6,
            retained_byte_count: Some(6),
            content_available: true,
            truncated: false,
            created_at: now,
        },
    )
}
fn page(
    session: &ChatSessionStatus,
    row: Option<ChatRecordStatus>,
    text: Option<&str>,
    offset: u32,
    complete: bool,
    error: Option<ChatContentError>,
) -> HistoryPage {
    HistoryPage::Content(ChatContentPage {
        chat_id: session.chat_id,
        record: row,
        text: text.map(str::to_owned),
        next_cursor: ChatContentCursor {
            after_sequence: if complete {
                session.last_record_sequence
            } else {
                0
            },
            byte_offset: offset,
        },
        complete,
        more: !complete,
        last_record_sequence: session.last_record_sequence,
        content_error: error,
    })
}
fn fake(pages: Vec<HistoryPage>) -> Fake {
    Fake {
        pages: RefCell::new(pages.into()),
        reads: RefCell::new(Vec::new()),
    }
}

#[tokio::test]
async fn utf8_chunks_pin_metadata_and_advance_exact_byte_cursors() {
    let (request, session, turn, row) = fixture();
    let source = fake(vec![
        HistoryPage::Status(session.clone()),
        HistoryPage::Turns(vec![(8, turn)], 8, false),
        page(&session, Some(row.clone()), Some("中"), 3, false, None),
        page(&session, Some(row), Some("文"), 0, true, None),
        HistoryPage::Status(session),
    ]);
    let snapshot = collect_from(&source, &request)
        .await
        .expect("consistent snapshot");
    assert_eq!(snapshot.records[0].text.as_deref(), Some("中文"));
    assert!(!snapshot.partial);
    assert_eq!(
        *source.reads.borrow(),
        vec![
            ChatContentCursor::default(),
            ChatContentCursor {
                after_sequence: 0,
                byte_offset: 3
            }
        ]
    );
    assert!(!format!("{snapshot:?}").contains("中文"));
    assert!(source.pages.borrow().is_empty());
}
#[tokio::test]
async fn retained_truncation_can_have_a_larger_original_record() {
    let (request, session, turn, mut row) = fixture();
    row.original_byte_count = MAX_CHAT_RECORD_BYTES + 1;
    row.truncated = true;
    let source = fake(vec![
        HistoryPage::Status(session.clone()),
        HistoryPage::Turns(vec![(8, turn)], 8, false),
        page(&session, Some(row), Some("中文"), 0, true, None),
        HistoryPage::Status(session),
    ]);
    let snapshot = collect_from(&source, &request)
        .await
        .expect("bounded retention, larger original");
    assert!(snapshot.partial);
    assert!(snapshot.records[0].truncated);
    assert_eq!(snapshot.records[0].text.as_deref(), Some("中文"));
}
#[tokio::test]
async fn an_unavailable_later_chunk_discards_the_entire_record_body() {
    let (request, session, turn, row) = fixture();
    let mut unavailable = row.clone();
    unavailable.content_available = false;
    let source = fake(vec![
        HistoryPage::Status(session.clone()),
        HistoryPage::Turns(vec![(8, turn)], 8, false),
        page(&session, Some(row), Some("中"), 3, false, None),
        page(
            &session,
            Some(unavailable),
            None,
            0,
            true,
            Some(ChatContentError::Unavailable),
        ),
        HistoryPage::Status(session),
    ]);
    let snapshot = collect_from(&source, &request)
        .await
        .expect("explicit unavailable placeholder");
    assert!(snapshot.partial);
    assert_eq!(snapshot.records.len(), 1);
    assert!(snapshot.records[0].text.is_none());
    assert_eq!(
        snapshot.records[0].content_error,
        Some(ChatContentError::Unavailable)
    );
}
#[tokio::test]
async fn malformed_cursor_identity_counts_and_mid_read_changes_refuse_the_snapshot() {
    for scenario in 0..6 {
        let (request, session, turn, mut row) = fixture();
        if scenario == 1 {
            row.chat_id = ChatId::new();
        }
        let mut chunk = match page(&session, Some(row), Some("中文"), 0, true, None) {
            HistoryPage::Content(p) => p,
            _ => unreachable!(),
        };
        if scenario == 0 {
            chunk.next_cursor.byte_offset = 1;
        }
        if scenario == 2 {
            chunk.last_record_sequence += 1;
        }
        if scenario == 3 {
            chunk.record.as_mut().unwrap().retained_byte_count = Some(5);
        }
        let mut end = session.clone();
        if scenario == 4 {
            end.version += 1;
        }
        let mut start = session.clone();
        if scenario == 5 {
            start.record_count = 2;
        }
        let source = fake(vec![
            HistoryPage::Status(start),
            HistoryPage::Turns(vec![(8, turn)], 8, false),
            HistoryPage::Content(chunk),
            HistoryPage::Status(end),
        ]);
        assert!(
            collect_from(&source, &request).await.is_err(),
            "scenario {scenario}"
        );
    }
}
#[tokio::test]
async fn busy_or_stale_selection_never_reads_a_private_body() {
    for scenario in 0..3 {
        let (request, mut session, _, _) = fixture();
        if scenario == 0 {
            session.active_request_id = Some(Uuid::new_v4());
        } else if scenario == 1 {
            session.state = ChatState::RecoveryRequired;
        } else {
            session.version += 1;
        }
        let source = fake(vec![HistoryPage::Status(session)]);
        assert!(collect_from(&source, &request).await.is_err());
        assert!(source.reads.borrow().is_empty());
    }
}
#[tokio::test]
async fn purge_keeps_all_metadata_and_no_private_copy() {
    let (request, mut session, turn, mut row) = fixture();
    session.content_purged = true;
    row.content_available = false;
    row.retained_byte_count = None;
    let source = fake(vec![
        HistoryPage::Status(session.clone()),
        HistoryPage::Turns(vec![(8, turn)], 8, false),
        page(
            &session,
            Some(row),
            None,
            0,
            true,
            Some(ChatContentError::Purged),
        ),
        HistoryPage::Status(session),
    ]);
    let snapshot = collect_from(&source, &request)
        .await
        .expect("purged metadata snapshot");
    assert_eq!(snapshot.title_error, Some(ChatContentError::Purged));
    assert!(snapshot.partial && snapshot.title.is_none());
    assert!(snapshot.records[0].text.is_none());
    assert_eq!(snapshot.records.len(), 1);
}
#[tokio::test]
async fn empty_conversation_and_paginated_global_sequences_are_not_gaps() {
    let (request, mut session, turn, _) = fixture();
    session.record_count = 0;
    session.last_record_sequence = 0;
    let mut next_turn = turn.clone();
    next_turn.correlation_id = Uuid::new_v4();
    next_turn.request_id = Uuid::new_v4();
    let source = fake(vec![
        HistoryPage::Status(session.clone()),
        HistoryPage::Turns(vec![(12, turn)], 12, true),
        HistoryPage::Turns(vec![(123, next_turn)], 123, false),
        page(&session, None, None, 0, true, None),
        HistoryPage::Status(session),
    ]);
    let snapshot = collect_from(&source, &request)
        .await
        .expect("global sequence pagination");
    assert_eq!(snapshot.turns.len(), 2);
    assert!(snapshot.records.is_empty());
    assert!(!snapshot.partial);
}
#[tokio::test]
async fn changed_or_stuck_turn_pagination_is_rejected() {
    for scenario in 0..2 {
        let (request, session, turn, _) = fixture();
        let bad = if scenario == 0 {
            HistoryPage::Turns(vec![], 0, true)
        } else {
            HistoryPage::Turns(vec![(8, turn)], 9, false)
        };
        let source = fake(vec![HistoryPage::Status(session), bad]);
        assert_eq!(
            collect_from(&source, &request)
                .await
                .expect_err("pagination refusal"),
            ExportError::InvalidResponse
        );
    }
}
