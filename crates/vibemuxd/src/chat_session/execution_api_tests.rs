use super::*;
use serde_json::json;
use vibemux_types::{
    ProjectId, RunStatus,
    chat::{ChatHarness, ContentRef},
    chat_owner::{ChatOwnerPhase, ChatWorkspaceEffect},
};

fn status(tail: u64) -> ChatExecutionStatus {
    let chat_id = ChatId::new();
    let project_id = ProjectId::new();
    ChatExecutionStatus {
        owner: ChatOwnerStatus {
            schema_version: 1,
            chat_id,
            project_id,
            task_id: TaskId::new(),
            run_id: RunId::new(),
            harness: ChatHarness::Codex,
            phase: ChatOwnerPhase::Open,
            version: 4,
            run_status: RunStatus::Running,
            admitted_tools: vec![],
            has_title: true,
            workspace_planned: true,
            workspace_branch: Some("codex/fixture".into()),
            workspace_effect: Some(ChatWorkspaceEffect::Settled),
            workspace_verified_at: Some(OffsetDateTime::UNIX_EPOCH),
            provisioning_active: false,
            error_code: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        },
        session: ChatSessionStatus {
            schema_version: 1,
            chat_id,
            project_id,
            harness: ChatHarness::Codex,
            state: ChatState::Ready,
            generation: 1,
            version: 2,
            active_request_id: None,
            record_count: tail,
            last_record_sequence: tail,
            content_purged: false,
            has_title: true,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        },
        process_claimed: true,
    }
}
fn private_record(status: &ChatExecutionStatus, text: &str) -> PrivateExecutionRecord {
    PrivateExecutionRecord {
        record: ChatExecutionRecord {
            schema_version: 1,
            record_id: Uuid::new_v4(),
            chat_id: status.session.chat_id,
            attempt_id: ChatAttemptId::new(),
            sequence: 1,
            kind: ChatRecordKind::Assistant,
            content_ref: Some(ContentRef {
                blob_id: Uuid::new_v4(),
                sha256: "a".repeat(64),
                byte_count: text.len() as u64,
            }),
            original_byte_count: text.len() as u64,
            created_at: OffsetDateTime::UNIX_EPOCH,
        },
        text: Some(text.into()),
        content_error: None,
    }
}

#[test]
fn execution_request_is_closed_and_start_requires_explicit_consent() {
    let mut value = json!({"operation":"start","operation_id":Uuid::new_v4(),"chat_id":ChatId::new(),"generation":1,"owner_version":1,"consent_execution":false});
    let request: ChatExecutionRequest = serde_json::from_value(value.clone()).expect("shape");
    assert_eq!(request.validate(), Err(ChatError::InvalidInput));
    value["consent_execution"] = json!(true);
    let request: ChatExecutionRequest = serde_json::from_value(value.clone()).expect("shape");
    request.validate().expect("explicit consent");
    assert!(request.requires_execution_consent());
    value["profile_sha256"] = json!("forged");
    assert!(serde_json::from_value::<ChatExecutionRequest>(value).is_err());
    let request = ChatExecutionRequest::Send {
        chat_id: ChatId::new(),
        generation: 1,
        owner_version: 1,
        request_id: Uuid::new_v4(),
        prompt: "PRIVATE_EXECUTION_API_中文".into(),
    };
    assert!(!format!("{request:?}").contains("PRIVATE_"));
    assert!(request.is_mutation());
    let lookup = ChatExecutionRequest::LookupStart {
        operation_id: Uuid::new_v4(),
        chat_id: ChatId::new(),
        generation: 1,
        owner_version: 1,
    };
    assert!(!lookup.is_mutation());
    assert!(!lookup.requires_execution_consent());
}

#[test]
fn status_attempt_and_record_projections_do_not_expose_private_bindings() {
    let status = status(1);
    let value = serde_json::to_value(&status).expect("status");
    for part in [&value["owner"], &value["session"]] {
        for key in [
            "workspace",
            "ownership_token",
            "resource_key",
            "profile_sha256",
            "vendor_session_id",
            "expected_vendor_id",
            "title_ref",
            "title",
            "owner_fence",
        ] {
            assert!(part.get(key).is_none(), "private {key}");
        }
    }
    let private = private_record(&status, "PRIVATE_RECORD_TEXT");
    let metadata = ChatExecutionRecordStatus::from_record(&private.record, true);
    let value = serde_json::to_value(&metadata).expect("record metadata");
    assert!(value.get("attempt_id").is_some());
    for key in ["turn_id", "content_ref", "sha256", "blob_id", "text"] {
        assert!(value.get(key).is_none(), "private {key}");
    }
    let page =
        content_page(&status, ChatContentCursor::default(), Some(private)).expect("private page");
    assert!(!format!("{page:?}").contains("PRIVATE_"));
    assert!(!format!("{:?}", ChatExecutionResponse::Content { page }).contains("PRIVATE_"));
}

#[test]
fn owner_content_cursor_preserves_utf8_and_reports_missing_bodies() {
    let status = status(1);
    let text = "中".repeat(9000);
    let private = private_record(&status, &text);
    let mut cursor = ChatContentCursor::default();
    let mut assembled = String::new();
    for _ in 0..8 {
        let page = content_page(&status, cursor, Some(private.clone())).expect("chunk");
        assert!(page.text.as_ref().expect("text").len() <= api::MAX_CHAT_CHUNK_BYTES);
        assembled.push_str(page.text.as_ref().expect("text"));
        cursor = page.next_cursor;
        if page.complete {
            assert!(!page.more);
            break;
        }
        assert!(text.is_char_boundary(cursor.byte_offset as usize));
    }
    assert_eq!(assembled, text);
    assert_eq!(cursor.after_sequence, 1);
    assert_eq!(cursor.byte_offset, 0);
    assert_eq!(
        content_page(
            &status,
            ChatContentCursor {
                after_sequence: 0,
                byte_offset: 1
            },
            Some(private.clone())
        )
        .expect_err("half character"),
        ChatError::InvalidInput
    );
    let missing = PrivateExecutionRecord {
        text: None,
        content_error: Some("chat_content_unavailable"),
        ..private
    };
    let page =
        content_page(&status, ChatContentCursor::default(), Some(missing)).expect("unavailable");
    assert_eq!(page.content_error, Some(ChatContentError::Unavailable));
    assert!(page.complete);
    assert_eq!(page.text, None);
}

#[test]
fn live_replacement_resets_offset_and_keeps_the_durable_attempt_identity() {
    let status = status(0);
    let request_id = Uuid::new_v4();
    let attempt_id = ChatAttemptId::new();
    let attempt:ChatExecutionAttempt=serde_json::from_value(json!({"schema_version":1,"chat_id":status.session.chat_id,"attempt_id":attempt_id,"request_id":request_id,"project_id":status.owner.project_id,"owner_task_id":status.owner.task_id,"owner_run_id":status.owner.run_id,"generation":1,"owner_version":4,"version":2,"phase":"running","admitted_sequence":1,"base_commit":"a".repeat(40),"effective_tools":[],"prompt_ref":{"blob_id":Uuid::new_v4(),"sha256":"b".repeat(64),"byte_count":16},"vendor_turn_id":null,"outcome":null,"error_code":null,"correlated_terminal":false,"output_complete":false,"created_at":"1970-01-01T00:00:00Z","updated_at":"1970-01-01T00:00:00Z"})).expect("attempt");
    let text = vibemux_harness::chat_session::AssistantText {
        request_id,
        turn_id: *attempt_id.as_uuid(),
        revision: 2,
        text: "中".into(),
        original_bytes: 3,
        truncated: false,
    };
    let chunk = live_chunk(&attempt, request_id, Some(1), 9000, Some(text.clone()))
        .expect("reset")
        .expect("chunk");
    assert!(chunk.reset);
    assert_eq!(chunk.byte_offset, 0);
    assert_eq!(chunk.attempt_id, attempt_id);
    assert_eq!(chunk.text, "中");
    let wrong = vibemux_harness::chat_session::AssistantText {
        turn_id: Uuid::new_v4(),
        ..text
    };
    assert_eq!(
        live_chunk(&attempt, request_id, None, 0, Some(wrong)).expect_err("foreign turn"),
        ChatError::ProjectionMismatch
    );
}
