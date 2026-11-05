use super::*;
use serde_json::{Value, json};

#[test]
fn private_request_debug_never_contains_prompt_or_title() {
    let request = ChatRequest::Send {
        chat_id: ChatId::new(),
        generation: 1,
        request_id: Uuid::new_v4(),
        prompt: "private-prompt-marker".to_owned(),
    };
    assert!(!format!("{request:?}").contains("private-prompt-marker"));
    for request in [
        ChatRequest::Open {
            operation_id: Uuid::new_v4(),
            harness: ChatHarness::Claude,
            title: Some("private-title-marker".to_owned()),
            consent_private_storage: true,
        },
        ChatRequest::Rename {
            operation_id: Uuid::new_v4(),
            chat_id: ChatId::new(),
            generation: 1,
            title: "private-title-marker".to_owned(),
        },
    ] {
        assert!(!format!("{request:?}").contains("private-title-marker"));
    }
}

#[test]
fn request_shape_rejects_extra_authority_and_requires_storage_consent() {
    for field in [
        "path",
        "executable",
        "argv",
        "env",
        "vendor_session_id",
        "method",
    ] {
        let mut value = json!({"operation":"status", "chat_id":ChatId::new()});
        value[field] = json!("unexpected");
        assert!(
            serde_json::from_value::<ChatRequest>(value).is_err(),
            "{field}"
        );
    }
    assert!(
        serde_json::from_value::<ChatRequest>(json!({
            "operation":"open", "operation_id":Uuid::new_v4(), "harness":"claude"
        }))
        .is_err()
    );
    let request: ChatRequest = serde_json::from_value(json!({
        "operation":"open", "operation_id":Uuid::new_v4(), "harness":"claude",
        "consent_private_storage":false
    }))
    .expect("well-formed but unconsented");
    assert_eq!(request.validate(), Err(ChatError::InvalidInput));
}

fn session_fixture() -> ChatSession {
    ChatSession {
        schema_version: 1,
        chat_id: ChatId::new(),
        project_id: ProjectId::new(),
        harness: ChatHarness::Claude,
        profile_sha256: "private-profile-marker".to_owned(),
        resource_key: "C:\\private\\project-marker".to_owned(),
        base_commit: "private-base-marker".to_owned(),
        state: ChatState::Busy,
        generation: 3,
        version: 7,
        expected_vendor_id: Some("private-expected-vendor-marker".to_owned()),
        vendor_session_id: Some("private-vendor-marker".to_owned()),
        vendor_confirmed_generation: Some(3),
        title_ref: Some(vibemux_types::chat::ContentRef {
            blob_id: Uuid::new_v4(),
            sha256: "private-title-hash-marker".to_owned(),
            byte_count: 10,
        }),
        retained_bytes: 40,
        record_count: 2,
        last_record_sequence: 42,
        content_purged: false,
        active_request_id: Some(Uuid::new_v4()),
        created_at: OffsetDateTime::UNIX_EPOCH,
        updated_at: OffsetDateTime::UNIX_EPOCH,
    }
}

fn turn_fixture(session: &ChatSession) -> ChatTurn {
    ChatTurn {
        schema_version: 1,
        chat_id: session.chat_id,
        turn_id: ChatTurnId::new(),
        request_id: session.active_request_id.expect("fixture request"),
        project_id: session.project_id,
        task_id: TaskId::new(),
        run_id: RunId::new(),
        generation: session.generation,
        version: 5,
        phase: ChatTurnPhase::Running,
        admitted_sequence: 39,
        base_commit: "private-turn-base-marker".to_owned(),
        prompt_ref: vibemux_types::chat::ContentRef {
            blob_id: Uuid::new_v4(),
            sha256: "private-prompt-hash-marker".to_owned(),
            byte_count: 20,
        },
        vendor_turn_id: Some("private-vendor-turn-marker".to_owned()),
        outcome: Some(ChatTurnOutcome::Failed),
        error_code: Some(ChatFailureCode::ProtocolViolation),
        correlated_terminal: true,
        output_complete: false,
        created_at: OffsetDateTime::UNIX_EPOCH,
        updated_at: OffsetDateTime::UNIX_EPOCH,
    }
}

fn record_fixture(session: &ChatSession, text: &str) -> PrivateChatRecord {
    PrivateChatRecord {
        record: ChatRecord {
            schema_version: 1,
            record_id: Uuid::new_v4(),
            chat_id: session.chat_id,
            turn_id: ChatTurnId::new(),
            sequence: session.last_record_sequence,
            kind: ChatRecordKind::Assistant,
            content_ref: Some(vibemux_types::chat::ContentRef {
                blob_id: Uuid::new_v4(),
                sha256: "private-record-hash-marker".to_owned(),
                byte_count: text.len() as u64,
            }),
            original_byte_count: text.len() as u64,
            created_at: OffsetDateTime::UNIX_EPOCH,
        },
        text: Some(text.to_owned()),
        content_error: None,
    }
}

fn live_fixture(session: &ChatSession, revision: u64, text: &str) -> AssistantText {
    AssistantText {
        request_id: session.active_request_id.expect("fixture request"),
        turn_id: Uuid::new_v4(),
        revision,
        text: text.to_owned(),
        original_bytes: text.len(),
        truncated: false,
    }
}

fn json_keys(value: &Value) -> Vec<&str> {
    value
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect()
}

#[test]
fn public_session_and_turn_dtos_are_explicit_allowlists() {
    let session = session_fixture();
    let turn = turn_fixture(&session);
    let status = ChatSessionStatus::from(&session);
    let turn_status = ChatTurnStatus::from(&turn);
    let value = serde_json::to_value(&status).expect("session JSON");
    assert_eq!(
        json_keys(&value),
        vec![
            "active_request_id",
            "chat_id",
            "content_purged",
            "created_at",
            "generation",
            "harness",
            "has_title",
            "last_record_sequence",
            "project_id",
            "record_count",
            "schema_version",
            "state",
            "updated_at",
            "version",
        ]
    );
    let value = serde_json::to_value(&turn_status).expect("turn JSON");
    assert_eq!(
        json_keys(&value),
        vec![
            "admitted_sequence",
            "chat_id",
            "correlated_terminal",
            "created_at",
            "error_code",
            "generation",
            "outcome",
            "output_complete",
            "phase",
            "request_id",
            "run_id",
            "schema_version",
            "task_id",
            "turn_id",
            "updated_at",
            "version",
        ]
    );
    for value in [
        serde_json::to_string(&status).expect("session JSON"),
        serde_json::to_string(&turn_status).expect("turn JSON"),
        format!("{status:?}"),
        format!("{turn_status:?}"),
    ] {
        assert!(!value.contains("private-"), "private marker leaked");
        assert!(!value.contains("sha256"));
        assert!(!value.contains("vendor_"));
        assert!(!value.contains("content_ref"));
    }
    assert_eq!(turn_status.task_id, turn.task_id);
    assert_eq!(turn_status.run_id, turn.run_id);
    assert_eq!(
        turn_status.error_code,
        Some(ChatFailureCode::ProtocolViolation)
    );
    assert_eq!(turn_status.outcome, Some(ChatTurnOutcome::Failed));
}

#[test]
fn request_defaults_and_all_operations_round_trip_without_extra_authority() {
    let chat_id = ChatId::new();
    let list: ChatRequest = serde_json::from_value(json!({"operation":"list"})).expect("defaults");
    assert_eq!(
        list,
        ChatRequest::List {
            after: None,
            limit: 16,
            archived: false
        }
    );
    let turns: ChatRequest =
        serde_json::from_value(json!({"operation":"turns","chat_id":chat_id})).expect("defaults");
    assert_eq!(
        turns,
        ChatRequest::Turns {
            chat_id,
            after_sequence: 0,
            limit: 16
        }
    );
    let content: ChatRequest =
        serde_json::from_value(json!({"operation":"content","chat_id":chat_id})).expect("defaults");
    assert_eq!(
        content,
        ChatRequest::Content {
            chat_id,
            cursor: ChatContentCursor::default()
        }
    );
    for value in [
        json!({"operation":"list","after":chat_id,"limit":32,"archived":true}),
        json!({"operation":"open","operation_id":Uuid::new_v4(),"harness":"codex","consent_private_storage":true}),
        json!({"operation":"status","chat_id":chat_id}),
        json!({"operation":"send","chat_id":chat_id,"generation":1,"request_id":Uuid::new_v4(),"prompt":"hello"}),
        json!({"operation":"cancel","chat_id":chat_id,"generation":1,"request_id":Uuid::new_v4()}),
        json!({"operation":"turns","chat_id":chat_id,"after_sequence":5,"limit":1}),
        json!({"operation":"title","chat_id":chat_id}),
        json!({"operation":"content","chat_id":chat_id,"cursor":{"after_sequence":2,"byte_offset":4}}),
        json!({"operation":"live","chat_id":chat_id,"generation":1,"request_id":Uuid::new_v4(),"revision":2,"byte_offset":4}),
        json!({"operation":"rename","operation_id":Uuid::new_v4(),"chat_id":chat_id,"generation":1,"title":"title"}),
        json!({"operation":"close","operation_id":Uuid::new_v4(),"chat_id":chat_id,"generation":1}),
        json!({"operation":"archive","operation_id":Uuid::new_v4(),"chat_id":chat_id,"generation":1}),
        json!({"operation":"resume","operation_id":Uuid::new_v4(),"chat_id":chat_id,"generation":1}),
        json!({"operation":"purge","operation_id":Uuid::new_v4(),"chat_id":chat_id,"generation":1}),
    ] {
        let request: ChatRequest = serde_json::from_value(value.clone()).expect("operation");
        assert_eq!(request.validate(), Ok(()));
        let encoded = serde_json::to_value(&request).expect("encode");
        assert_eq!(
            serde_json::from_value::<ChatRequest>(encoded).expect("decode"),
            request
        );
        let mut unknown = value;
        unknown["private_runtime_override"] = json!(true);
        assert!(serde_json::from_value::<ChatRequest>(unknown).is_err());
    }
}

#[test]
fn invalid_ids_generations_sizes_and_nested_cursors_are_refused() {
    let chat_id = ChatId::new();
    for value in [
        json!({"operation":"status","chat_id":Uuid::nil()}),
        json!({"operation":"list","after":Uuid::nil()}),
        json!({"operation":"content","chat_id":chat_id,"cursor":{"after_sequence":0,"byte_offset":0,"path":"private"}}),
        json!({"operation":"content","chat_id":chat_id,"cursor":{"after_sequence":-1,"byte_offset":0}}),
        json!({"operation":"open","operation_id":Uuid::new_v4(),"harness":"unregistered","consent_private_storage":true}),
    ] {
        assert!(serde_json::from_value::<ChatRequest>(value).is_err());
    }
    for value in [
        json!({"operation":"list","limit":0}),
        json!({"operation":"list","limit":33}),
        json!({"operation":"turns","chat_id":chat_id,"after_sequence":u64::MAX}),
        json!({"operation":"open","operation_id":Uuid::nil(),"harness":"claude","consent_private_storage":true}),
        json!({"operation":"send","chat_id":chat_id,"generation":0,"request_id":Uuid::new_v4(),"prompt":"ok"}),
        json!({"operation":"send","chat_id":chat_id,"generation":u64::MAX,"request_id":Uuid::new_v4(),"prompt":"ok"}),
        json!({"operation":"send","chat_id":chat_id,"generation":1,"request_id":Uuid::nil(),"prompt":"ok"}),
        json!({"operation":"send","chat_id":chat_id,"generation":1,"request_id":Uuid::new_v4(),"prompt":" \t\n"}),
        json!({"operation":"rename","operation_id":Uuid::nil(),"chat_id":chat_id,"generation":1,"title":"ok"}),
        json!({"operation":"rename","operation_id":Uuid::new_v4(),"chat_id":chat_id,"generation":1,"title":""}),
        json!({"operation":"close","operation_id":Uuid::new_v4(),"chat_id":chat_id,"generation":0}),
        json!({"operation":"cancel","chat_id":chat_id,"generation":1,"request_id":Uuid::nil()}),
        json!({"operation":"content","chat_id":chat_id,"cursor":{"after_sequence":u64::MAX,"byte_offset":0}}),
        json!({"operation":"content","chat_id":chat_id,"cursor":{"after_sequence":0,"byte_offset":262145}}),
        json!({"operation":"live","chat_id":chat_id,"generation":1,"request_id":Uuid::new_v4(),"byte_offset":1}),
        json!({"operation":"live","chat_id":chat_id,"generation":1,"request_id":Uuid::new_v4(),"revision":0}),
    ] {
        let request: ChatRequest = serde_json::from_value(value).expect("shape");
        assert_eq!(
            request.validate(),
            Err(ChatError::InvalidInput),
            "{request:?}"
        );
    }
    for (field, maximum, operation) in [("prompt", 32768, "send"), ("title", 512, "rename")] {
        let mut value = json!({"operation":operation,"chat_id":chat_id,"generation":1,
            "operation_id":Uuid::new_v4(),"request_id":Uuid::new_v4()});
        if operation == "send" {
            value
                .as_object_mut()
                .expect("object")
                .remove("operation_id");
        } else {
            value.as_object_mut().expect("object").remove("request_id");
        }
        value[field] = json!("界".repeat(maximum / 3));
        assert_eq!(
            serde_json::from_value::<ChatRequest>(value.clone())
                .expect("shape")
                .validate(),
            Ok(())
        );
        value[field] = json!("a".repeat(maximum + 1));
        assert_eq!(
            serde_json::from_value::<ChatRequest>(value)
                .expect("shape")
                .validate(),
            Err(ChatError::InvalidInput)
        );
    }
}

#[test]
fn large_escaped_unicode_record_reassembles_without_truncation() {
    let session = session_fixture();
    let text = format!(
        "{}{}{}",
        "a".repeat(8191),
        "🙂界".repeat(20000),
        "\0\r\n\t\\\"".repeat(2000)
    );
    let record = record_fixture(&session, &text).record;
    let mut cursor = ChatContentCursor::default();
    let mut assembled = String::new();
    let mut chunks = 0;
    loop {
        let page = content_page(
            &session,
            cursor,
            Some(PrivateChatRecord {
                record: record.clone(),
                text: Some(text.clone()),
                content_error: None,
            }),
        )
        .expect("valid cursor");
        let chunk = page.text.as_deref().expect("text");
        assert!(!chunk.is_empty());
        assert!(chunk.len() <= 8192);
        assert_eq!(page.content_error, None);
        assert_eq!(
            page.record.as_ref().expect("metadata").retained_byte_count,
            Some(text.len() as u64)
        );
        assert!(!page.record.as_ref().expect("metadata").truncated);
        assembled.push_str(chunk);
        let response =
            bounded_response(ChatResponse::Content { page: page.clone() }).expect("bounded");
        assert!(serde_json::to_vec(&response).expect("encode").len() < 65536);
        assert_eq!(
            serde_json::from_value::<ChatResponse>(
                serde_json::to_value(&response).expect("encode")
            )
            .expect("decode"),
            response
        );
        chunks += 1;
        assert!(chunks < 40, "cursor made no progress");
        if page.complete {
            assert_eq!(
                page.next_cursor,
                ChatContentCursor {
                    after_sequence: 42,
                    byte_offset: 0
                }
            );
            assert!(!page.more);
            break;
        }
        assert!(page.more);
        assert_eq!(page.next_cursor.after_sequence, cursor.after_sequence);
        assert!(page.next_cursor.byte_offset > cursor.byte_offset);
        cursor = page.next_cursor;
    }
    assert_eq!(assembled, text);
    assert!(chunks > 1);
}

#[test]
fn record_cursor_rejects_invalid_utf8_offsets_past_tail_and_missing_records() {
    let session = session_fixture();
    for cursor in [
        ChatContentCursor {
            after_sequence: 0,
            byte_offset: 2,
        },
        ChatContentCursor {
            after_sequence: 0,
            byte_offset: 7,
        },
        ChatContentCursor {
            after_sequence: 43,
            byte_offset: 0,
        },
        ChatContentCursor {
            after_sequence: 42,
            byte_offset: 1,
        },
    ] {
        assert_eq!(
            content_page(&session, cursor, Some(record_fixture(&session, "a🙂b"))),
            Err(ChatError::InvalidInput)
        );
    }
    let page = content_page(
        &session,
        ChatContentCursor {
            after_sequence: 0,
            byte_offset: 6,
        },
        Some(record_fixture(&session, "a🙂b")),
    )
    .expect("end boundary");
    assert_eq!(page.text, Some(String::new()));
    assert!(page.complete);
    assert_eq!(
        content_page(&session, ChatContentCursor::default(), None),
        Err(ChatError::ProjectionMismatch)
    );
    let page = content_page(
        &session,
        ChatContentCursor {
            after_sequence: 42,
            byte_offset: 0,
        },
        None,
    )
    .expect("tail");
    assert!(page.complete && !page.more);
    assert_eq!(page.record, None);
    let mut wrong = record_fixture(&session, "reply");
    wrong.record.chat_id = ChatId::new();
    assert_eq!(
        content_page(&session, ChatContentCursor::default(), Some(wrong)),
        Err(ChatError::ProjectionMismatch)
    );
}

#[test]
fn record_completeness_more_and_content_failures_are_explicit() {
    let mut session = session_fixture();
    let mut record = record_fixture(&session, "retained");
    record.record.sequence = 40;
    record.record.original_byte_count = 100;
    let page = content_page(&session, ChatContentCursor::default(), Some(record)).expect("page");
    assert!(page.complete && page.more);
    assert!(page.record.expect("metadata").truncated);
    for (error, expected) in [
        (
            Some("chat_content_invalid_utf8"),
            ChatContentError::InvalidUtf8,
        ),
        (Some("private-path-marker"), ChatContentError::Unavailable),
        (None, ChatContentError::Unavailable),
    ] {
        let mut record = record_fixture(&session, "private-text-marker");
        record.text = None;
        record.content_error = error;
        let page = content_page(&session, ChatContentCursor::default(), Some(record))
            .expect("unavailable");
        assert_eq!(page.content_error, Some(expected));
        assert!(page.complete);
        assert_eq!(page.text, None);
        assert!(!page.record.as_ref().expect("metadata").content_available);
        let encoded = serde_json::to_string(&page).expect("encode");
        assert!(!encoded.contains("private-path-marker"));
        assert!(!encoded.contains("private-text-marker"));
    }
    session.content_purged = true;
    session.title_ref = None;
    let mut record = record_fixture(&session, "purged");
    record.record.content_ref = None;
    record.text = None;
    let page = content_page(&session, ChatContentCursor::default(), Some(record)).expect("purged");
    assert_eq!(page.content_error, Some(ChatContentError::Purged));
    assert!(page.complete);
    assert!(!ChatSessionStatus::from(&session).has_title);
}

#[test]
fn live_revision_changes_restart_at_zero_even_if_old_offset_is_past_new_tail() {
    let session = session_fixture();
    let request_id = session.active_request_id.expect("request");
    let text = "🙂".repeat(5000);
    let first = live_chunk(
        &session,
        request_id,
        None,
        0,
        Some(live_fixture(&session, 1, &text)),
    )
    .expect("first")
    .expect("active");
    assert_eq!(first.byte_offset, 0);
    assert_eq!(first.next_byte_offset, 8192);
    assert!(!first.reset && first.more && !first.complete);
    let second = live_chunk(
        &session,
        request_id,
        Some(1),
        first.next_byte_offset,
        Some(live_fixture(&session, 1, &text)),
    )
    .expect("second")
    .expect("active");
    assert_eq!(second.byte_offset, 8192);
    assert!(!second.reset);
    let replacement = live_chunk(
        &session,
        request_id,
        Some(1),
        second.next_byte_offset,
        Some(live_fixture(&session, 2, "new")),
    )
    .expect("reset")
    .expect("active");
    assert!(replacement.reset && replacement.complete);
    assert_eq!(replacement.revision, 2);
    assert_eq!(replacement.byte_offset, 0);
    assert_eq!(replacement.next_byte_offset, 3);
    assert_eq!(replacement.text, "new");
}

#[test]
fn live_chunks_reassemble_exact_revision_and_refuse_bad_offsets() {
    let session = session_fixture();
    let request_id = session.active_request_id.expect("request");
    let text = format!("{}{}", "界".repeat(10000), "\0".repeat(10000));
    let mut offset = 0;
    let mut assembled = String::new();
    loop {
        let chunk = live_chunk(
            &session,
            request_id,
            Some(4),
            offset,
            Some(live_fixture(&session, 4, &text)),
        )
        .expect("chunk")
        .expect("active");
        assert!(chunk.text.len() <= 8192);
        assert!(!chunk.reset);
        assembled.push_str(&chunk.text);
        let done = chunk.complete;
        offset = chunk.next_byte_offset;
        let response = bounded_response(ChatResponse::Live {
            chat_id: session.chat_id,
            generation: 3,
            request_id,
            snapshot: Some(chunk),
        })
        .expect("bounded");
        assert!(serde_json::to_vec(&response).expect("encode").len() < 65536);
        if done {
            break;
        }
    }
    assert_eq!(assembled, text);
    for offset in [1, 5] {
        assert_eq!(
            live_chunk(
                &session,
                request_id,
                Some(4),
                offset,
                Some(live_fixture(&session, 4, "🙂"))
            ),
            Err(ChatError::InvalidInput)
        );
    }
    assert_eq!(
        require_generation(&session, 2),
        Err(ChatError::StaleGeneration)
    );
}

#[test]
fn terminal_and_mismatched_active_snapshots_return_none() {
    let mut session = session_fixture();
    let request_id = session.active_request_id.expect("request");
    assert_eq!(
        live_chunk(&session, request_id, None, 0, None).expect("no text"),
        None
    );
    assert_eq!(
        live_chunk(
            &session,
            Uuid::new_v4(),
            None,
            0,
            Some(live_fixture(&session, 1, "private"))
        )
        .expect("wrong request"),
        None
    );
    let mut wrong = live_fixture(&session, 1, "private");
    wrong.request_id = Uuid::new_v4();
    assert_eq!(
        live_chunk(&session, request_id, None, 0, Some(wrong)).expect("wrong snapshot"),
        None
    );
    for state in [
        ChatState::Ready,
        ChatState::Closed,
        ChatState::Suspended,
        ChatState::RecoveryRequired,
        ChatState::Archived,
    ] {
        session.state = state;
        assert_eq!(
            live_chunk(
                &session,
                request_id,
                None,
                0,
                Some(live_fixture(&session, 1, "private"))
            )
            .expect("terminal"),
            None
        );
    }
    session.state = ChatState::Busy;
    session.active_request_id = None;
    assert_eq!(
        live_chunk(
            &session,
            request_id,
            None,
            0,
            Some(AssistantText {
                request_id,
                turn_id: Uuid::new_v4(),
                revision: 1,
                text: "private".to_owned(),
                original_bytes: 7,
                truncated: false,
            })
        )
        .expect("no active request"),
        None
    );
}

#[test]
fn private_response_debug_redacts_all_titles_and_text() {
    let session = session_fixture();
    let request_id = session.active_request_id.expect("request");
    for response in [
        ChatResponse::Title {
            chat_id: session.chat_id,
            title: Some("private-title-marker".to_owned()),
        },
        ChatResponse::Content {
            page: content_page(
                &session,
                ChatContentCursor::default(),
                Some(record_fixture(&session, "private-content-marker")),
            )
            .expect("page"),
        },
        ChatResponse::Live {
            chat_id: session.chat_id,
            generation: 3,
            request_id,
            snapshot: live_chunk(
                &session,
                request_id,
                None,
                0,
                Some(live_fixture(&session, 1, "private-live-marker")),
            )
            .expect("live"),
        },
    ] {
        let debug = format!("{response:?}");
        assert!(!debug.contains("private-"), "private Debug leak");
        assert!(
            serde_json::to_string(&response)
                .expect("explicit private JSON")
                .contains("private-")
        );
    }
}

#[test]
fn receipt_mapping_retains_canonical_ids_and_idempotency_only() {
    let session = session_fixture();
    let turn = turn_fixture(&session);
    let response = session_response(ChatSessionCommit {
        session: session.clone(),
        event: None,
        sequence: 50,
        duplicate: true,
        released_content: vec![],
    });
    let ChatResponse::Session {
        session: status,
        sequence,
        duplicate,
    } = response
    else {
        panic!("session response")
    };
    assert_eq!(status.chat_id, session.chat_id);
    assert_eq!(status.generation, 3);
    assert_eq!(sequence, 50);
    assert!(duplicate);
    let response = turn_response(ChatTurnCommit {
        session: session.clone(),
        turn: turn.clone(),
        event: None,
        sequence: 51,
        duplicate: false,
    });
    let ChatResponse::Turn {
        session: status,
        turn: metadata,
        sequence,
        duplicate,
    } = response
    else {
        panic!("turn response")
    };
    assert_eq!(status.chat_id, session.chat_id);
    assert_eq!(metadata.turn_id, turn.turn_id);
    assert_eq!(metadata.request_id, turn.request_id);
    assert_eq!(metadata.task_id, turn.task_id);
    assert_eq!(metadata.run_id, turn.run_id);
    assert_eq!(sequence, 51);
    assert!(!duplicate);
    let operation_id = Uuid::new_v4();
    let input = mutation(operation_id, session.chat_id, 3);
    assert_eq!(input.operation_id, operation_id);
    assert_eq!(input.chat_id, session.chat_id);
    assert_eq!(input.expected_generation, 3);
}

#[test]
fn every_response_shape_fits_control_frame_with_maximum_escaped_chunks() {
    let session = session_fixture();
    let turn = turn_fixture(&session);
    let request_id = turn.request_id;
    let controls = "\0".repeat(8192);
    let responses = [
        ChatResponse::List {
            sessions: vec![ChatSessionStatus::from(&session); 32],
            next_after: Some(session.chat_id),
            more: true,
        },
        ChatResponse::Status {
            session: Some(ChatSessionStatus::from(&session)),
        },
        ChatResponse::Submission {
            session: ChatSessionStatus::from(&session),
            turn: Some(ChatTurnStatus::from(&turn)),
        },
        ChatResponse::Session {
            session: ChatSessionStatus::from(&session),
            sequence: u64::MAX,
            duplicate: true,
        },
        ChatResponse::Turn {
            session: ChatSessionStatus::from(&session),
            turn: ChatTurnStatus::from(&turn),
            sequence: u64::MAX,
            duplicate: true,
        },
        ChatResponse::Turns {
            chat_id: session.chat_id,
            turns: vec![ChatTurnStatus::from(&turn); 32],
            next_sequence: u64::MAX,
            more: true,
        },
        ChatResponse::Title {
            chat_id: session.chat_id,
            title: Some("\0".repeat(512)),
        },
        ChatResponse::Content {
            page: content_page(
                &session,
                ChatContentCursor::default(),
                Some(record_fixture(&session, &controls)),
            )
            .expect("controls"),
        },
        ChatResponse::Live {
            chat_id: session.chat_id,
            generation: 3,
            request_id,
            snapshot: live_chunk(
                &session,
                request_id,
                None,
                0,
                Some(live_fixture(&session, 1, &controls)),
            )
            .expect("controls"),
        },
    ];
    for response in responses {
        let encoded = serde_json::to_vec(&response).expect("encode");
        assert!(
            encoded.len() <= 61440,
            "oversize response {}",
            encoded.len()
        );
        assert!(encoded.len() + 1024 < 65536, "no envelope room");
        assert!(bounded_response(response).is_ok());
    }
    assert_eq!(
        bounded_response(ChatResponse::Title {
            chat_id: session.chat_id,
            title: Some("\0".repeat(16384)),
        }),
        Err(ChatError::Capacity)
    );
}

#[test]
fn reconciliation_queries_are_strict_readonly_requests() {
    let chat_id = ChatId::new();
    for value in [
        json!({"operation":"lookup_open","operation_id":Uuid::new_v4()}),
        json!({"operation":"submission","chat_id":chat_id,"generation":3,"request_id":Uuid::new_v4()}),
    ] {
        let request: ChatRequest =
            serde_json::from_value(value.clone()).expect("reconciliation query");
        assert_eq!(request.validate(), Ok(()));
        let mut extra = value;
        extra["prompt"] = json!("must not submit");
        assert!(serde_json::from_value::<ChatRequest>(extra).is_err());
    }
    for value in [
        json!({"operation":"lookup_open","operation_id":Uuid::nil()}),
        json!({"operation":"submission","chat_id":chat_id,"generation":0,"request_id":Uuid::new_v4()}),
        json!({"operation":"submission","chat_id":chat_id,"generation":3,"request_id":Uuid::nil()}),
    ] {
        assert_eq!(
            serde_json::from_value::<ChatRequest>(value)
                .expect("shape")
                .validate(),
            Err(ChatError::InvalidInput)
        );
    }
    assert_eq!(
        ChatRequest::LookupOpen {
            operation_id: Uuid::new_v4()
        }
        .operation_name(),
        "lookup_open"
    );
    assert_eq!(
        ChatRequest::Submission {
            chat_id,
            generation: 3,
            request_id: Uuid::new_v4()
        }
        .operation_name(),
        "submission"
    );
}

#[test]
fn submission_reconciliation_retains_original_generation_and_hides_foreign_turns() {
    let mut session = session_fixture();
    let mut turn = turn_fixture(&session);
    let request_id = turn.request_id;
    session.generation = 4;
    let response =
        submission_response(&session, 3, request_id, Some(turn.clone())).expect("original binding");
    let ChatResponse::Submission {
        session: status,
        turn: metadata,
    } = response
    else {
        panic!("submission")
    };
    assert_eq!(status.generation, 4);
    assert_eq!(metadata.expect("known turn").generation, 3);
    assert_eq!(
        submission_response(&session, 3, request_id, None),
        Err(ChatError::StaleGeneration)
    );
    let response = submission_response(&session, 4, request_id, None).expect("current absence");
    assert!(matches!(
        response,
        ChatResponse::Submission { turn: None, .. }
    ));
    turn.chat_id = ChatId::new();
    assert_eq!(
        submission_response(&session, 3, request_id, Some(turn.clone())),
        Err(ChatError::Conflict)
    );
    turn.chat_id = session.chat_id;
    turn.generation = 2;
    assert_eq!(
        submission_response(&session, 3, request_id, Some(turn.clone())),
        Err(ChatError::Conflict)
    );
    turn.generation = 3;
    turn.request_id = Uuid::new_v4();
    assert_eq!(
        submission_response(&session, 3, request_id, Some(turn)),
        Err(ChatError::Conflict)
    );
}

#[test]
fn absent_open_lookup_is_distinct_from_a_private_status_record() {
    let absent = ChatResponse::Status { session: None };
    assert_eq!(
        serde_json::to_value(&absent).expect("encode"),
        json!({"operation":"status","session":null})
    );
    let present = ChatResponse::Status {
        session: Some(ChatSessionStatus::from(&session_fixture())),
    };
    assert!(
        !serde_json::to_string(&present)
            .expect("public JSON")
            .contains("private-")
    );
    assert!(!format!("{present:?}").contains("private-"));
}
