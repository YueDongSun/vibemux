//! Persistent chat metadata contracts through the canonical store boundary.
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_harness::{
    AgentKind, HarnessDetection,
    dispatch::{NativeProtocol, PromptDigest, Sha256Digest},
};
use vibemux_store::{
    ChatMutation, ChatOpen, ChatOwnerFence, ChatRecordAppend, ChatTurnAdmission, ChatTurnFinish,
    SqliteStore, StoreError,
};
use vibemux_types::chat::*;

fn reference(bytes: u64) -> ContentRef {
    ContentRef {
        blob_id: Uuid::new_v4(),
        sha256: "a".repeat(64),
        byte_count: bytes,
    }
}
fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_793_433_600).expect("fixed timestamp")
}
fn open_request() -> ChatOpen {
    ChatOpen {
        operation_id: Uuid::new_v4(),
        harness: ChatHarness::Claude,
        profile_sha256: "b".repeat(64),
        resource_key: "c".repeat(64),
        base_commit: "d".repeat(40),
        title_ref: None,
        timestamp: now(),
    }
}
fn mutation(chat_id: ChatId, generation: u64) -> ChatMutation {
    ChatMutation {
        operation_id: Uuid::new_v4(),
        chat_id,
        expected_generation: generation,
        timestamp: now(),
    }
}
fn new_store() -> (tempfile::TempDir, SqliteStore) {
    let temp = tempfile::tempdir().expect("temp");
    let mut store = SqliteStore::open(&temp.path().join("state.sqlite3")).expect("store");
    let detection = ["claude", "codex"]
        .into_iter()
        .map(|name| {
            (
                name.to_owned(),
                HarnessDetection {
                    detected: true,
                    path: None,
                    launcher: None,
                    version: Some("synthetic".to_owned()),
                },
            )
        })
        .collect();
    store
        .commit_harness_snapshot(&detection, "2026-10-31T00:00:00Z", "chat_detection")
        .expect("detection");
    (temp, store)
}
fn ready(store: &mut SqliteStore) -> (ChatSession, ChatOwnerFence) {
    let opened = store.open_chat(&open_request()).expect("open");
    let id = opened.session.chat_id;
    let claim = store
        .claim_chat_owner(id, 1, Some("expected_session".to_owned()), now())
        .expect("owner");
    let session = store
        .bind_chat_vendor(id, 1, claim.fence, "expected_session", now())
        .expect("vendor root proof")
        .session;
    (session, claim.fence)
}
fn admit(store: &mut SqliteStore, session: &ChatSession) -> ChatTurnAdmission {
    let admission = ChatTurnAdmission {
        chat_id: session.chat_id,
        request_id: Uuid::new_v4(),
        expected_generation: session.generation,
        base_commit: session.base_commit.clone(),
        prompt_ref: reference(12),
        timestamp: now(),
    };
    store.admit_chat_turn(&admission).expect("admit");
    admission
}
fn finish(request_id: Uuid, generation: u64, fence: ChatOwnerFence) -> ChatTurnFinish {
    ChatTurnFinish {
        request_id,
        generation,
        fence,
        outcome: ChatTurnOutcome::Completed,
        correlated_terminal: true,
        vendor_session_id: Some("expected_session".to_owned()),
        vendor_turn_id: None,
        error_code: None,
        process_alive: true,
        output_complete: true,
        timestamp: now(),
    }
}
fn code<T: std::fmt::Debug>(result: Result<T, StoreError>) -> &'static str {
    result.expect_err("must refuse").code()
}

#[test]
fn two_completed_turns_keep_one_ready_chat_with_distinct_canonical_attempts() {
    let temp = tempfile::tempdir().expect("temp");
    let mut store = SqliteStore::open(&temp.path().join("state.sqlite3")).expect("store");
    let now = OffsetDateTime::now_utc();
    let detection = std::collections::BTreeMap::from([(
        "claude".to_owned(),
        HarnessDetection {
            detected: true,
            path: None,
            launcher: None,
            version: Some("synthetic".to_owned()),
        },
    )]);
    store
        .commit_harness_snapshot(&detection, "2026-10-31T00:00:00Z", "chat_detection")
        .expect("actual test detection snapshot");
    let opened = store
        .open_chat(&ChatOpen {
            operation_id: Uuid::new_v4(),
            harness: ChatHarness::Claude,
            profile_sha256: "b".repeat(64),
            resource_key: "c".repeat(64),
            base_commit: "d".repeat(40),
            title_ref: None,
            timestamp: now,
        })
        .expect("open");
    let chat_id = opened.session.chat_id;
    let claim = store
        .claim_chat_owner(chat_id, 1, Some("expected_session".to_owned()), now)
        .expect("claim owner");
    store
        .mark_chat_transport_ready(chat_id, 1, claim.fence, now)
        .expect("transport ready without fabricated vendor proof");
    let mut attempts = Vec::new();
    for _ in 0..2 {
        let admission = ChatTurnAdmission {
            chat_id,
            request_id: Uuid::new_v4(),
            expected_generation: 1,
            base_commit: "d".repeat(40),
            prompt_ref: reference(12),
            timestamp: now,
        };
        let admitted = store.admit_chat_turn(&admission).expect("admit");
        let claimed = store
            .claim_chat_turn(admission.request_id, 1, claim.fence, now)
            .expect("single-use send claim");
        store
            .bind_chat_vendor(chat_id, 1, claim.fence, "expected_session", now)
            .expect("actual root vendor proof");
        let finished = store
            .finish_chat_turn(&ChatTurnFinish {
                request_id: admission.request_id,
                generation: 1,
                fence: claim.fence,
                outcome: ChatTurnOutcome::Completed,
                correlated_terminal: true,
                vendor_session_id: Some("expected_session".to_owned()),
                vendor_turn_id: None,
                error_code: None,
                process_alive: true,
                output_complete: true,
                timestamp: now,
            })
            .expect("finish turn while process remains alive");
        assert_eq!(claimed.turn.request_id, admission.request_id);
        assert_eq!(finished.turn.phase, ChatTurnPhase::Completed);
        assert_eq!(finished.session.state, ChatState::Ready);
        attempts.push((admitted.turn.task_id, admitted.turn.run_id));
    }
    assert_ne!(attempts[0], attempts[1]);
    assert_eq!(
        store
            .chat_session(chat_id)
            .expect("query")
            .expect("chat")
            .state,
        ChatState::Ready
    );
}

#[test]
fn open_and_submission_keys_replay_original_bindings_after_state_changes() {
    let (_temp, mut store) = new_store();
    let open = open_request();
    let first = store.open_chat(&open).expect("open");
    let id = first.session.chat_id;
    let owner = store
        .claim_chat_owner(id, 1, Some("expected_session".to_owned()), now())
        .expect("owner");
    let session = store
        .bind_chat_vendor(id, 1, owner.fence, "expected_session", now())
        .expect("proof")
        .session;
    let admitted = admit(&mut store, &session);
    let original = store
        .chat_turn(admitted.request_id)
        .expect("turn")
        .expect("known");
    store
        .claim_chat_turn(admitted.request_id, 1, owner.fence, now())
        .expect("claim");
    store
        .finish_chat_turn(&finish(admitted.request_id, 1, owner.fence))
        .expect("finish");
    store
        .release_chat_owner(&mutation(id, 1), owner.fence, true, false)
        .expect("verified release");
    let resume = mutation(id, 1);
    assert_eq!(
        store
            .resume_chat(&resume)
            .expect("resume")
            .session
            .generation,
        2
    );
    let before = store.events().expect("events").len();
    let duplicate_open = store.open_chat(&open).expect("duplicate open");
    assert!(duplicate_open.duplicate);
    assert_eq!(duplicate_open.session.chat_id, id);
    let duplicate = store
        .admit_chat_turn(&admitted)
        .expect("old generation duplicate");
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.turn.task_id, original.task_id);
    let duplicate_finish = store
        .finish_chat_turn(&finish(admitted.request_id, 1, owner.fence))
        .expect("original fenced terminal repeat");
    assert!(duplicate_finish.duplicate);
    assert_eq!(store.events().expect("events").len(), before);
    let mut conflict = admitted.clone();
    conflict.prompt_ref.sha256 = "f".repeat(64);
    assert_eq!(code(store.admit_chat_turn(&conflict)), "chat_conflict");
    let mut other = open;
    other.harness = ChatHarness::Codex;
    assert_eq!(code(store.open_chat(&other)), "chat_conflict");
}

#[test]
fn a_send_claim_is_single_use_and_survives_database_reopen() {
    let (temp, mut store) = new_store();
    let (session, fence) = ready(&mut store);
    let admission = admit(&mut store, &session);
    store
        .claim_chat_turn(admission.request_id, 1, fence, now())
        .expect("claim");
    assert_eq!(
        code(store.claim_chat_turn(admission.request_id, 1, fence, now())),
        "chat_turn_already_claimed"
    );
    drop(store);
    let mut store = SqliteStore::open(&temp.path().join("state.sqlite3")).expect("reopen");
    assert_eq!(
        code(store.claim_chat_turn(admission.request_id, 1, fence, now())),
        "chat_turn_already_claimed"
    );
}

#[test]
fn resumed_generation_needs_actual_vendor_confirmation_before_success() {
    let (_temp, mut store) = new_store();
    let (session, fence) = ready(&mut store);
    store
        .release_chat_owner(&mutation(session.chat_id, 1), fence, true, false)
        .expect("release");
    let resumed = store
        .resume_chat(&mutation(session.chat_id, 1))
        .expect("resume")
        .session;
    let claim = store
        .claim_chat_owner(
            session.chat_id,
            2,
            Some("expected_session".to_owned()),
            now(),
        )
        .expect("claim resumed process");
    let transport = store
        .mark_chat_transport_ready(session.chat_id, 2, claim.fence, now())
        .expect("transport");
    assert_eq!(
        transport.session.vendor_session_id.as_deref(),
        Some("expected_session")
    );
    assert_eq!(transport.session.vendor_confirmed_generation, None);
    let admission = admit(&mut store, &resumed);
    store
        .claim_chat_turn(admission.request_id, 2, claim.fence, now())
        .expect("turn claim");
    assert_eq!(
        code(store.finish_chat_turn(&finish(admission.request_id, 2, claim.fence))),
        "chat_unconfirmed_session"
    );
    assert_eq!(
        code(store.bind_chat_vendor(session.chat_id, 2, claim.fence, "another_session", now())),
        "chat_conflict"
    );
    store
        .bind_chat_vendor(session.chat_id, 2, claim.fence, "expected_session", now())
        .expect("actual resumed identity");
    store
        .finish_chat_turn(&finish(admission.request_id, 2, claim.fence))
        .expect("correlated terminal");
    assert_eq!(
        code(store.bind_chat_vendor(session.chat_id, 1, fence, "expected_session", now())),
        "chat_stale_generation"
    );
}

#[test]
fn completed_outcome_does_not_fabricate_available_private_output() {
    let (_temp, mut store) = new_store();
    let (session, fence) = ready(&mut store);
    let admission = admit(&mut store, &session);
    store
        .claim_chat_turn(admission.request_id, 1, fence, now())
        .expect("claim");
    let mut proof = finish(admission.request_id, 1, fence);
    proof.output_complete = false;
    let result = store
        .finish_chat_turn(&proof)
        .expect("finished with output gap");
    assert_eq!(result.turn.phase, ChatTurnPhase::Completed);
    assert!(!result.turn.output_complete);
    assert_eq!(result.session.state, ChatState::Ready);
}

#[test]
fn cancellation_before_claim_prevents_send_and_completion_race_keeps_cancellation() {
    let (_temp, mut store) = new_store();
    let (session, fence) = ready(&mut store);
    let unsent = admit(&mut store, &session);
    let cancelled = store
        .cancel_chat_turn(unsent.request_id, now())
        .expect("cancel before send");
    assert_eq!(cancelled.turn.phase, ChatTurnPhase::Cancelled);
    assert_eq!(
        code(store.claim_chat_turn(unsent.request_id, 1, fence, now())),
        "chat_invalid_transition"
    );
    let running = admit(&mut store, &session);
    store
        .claim_chat_turn(running.request_id, 1, fence, now())
        .expect("claim");
    assert_eq!(
        store
            .cancel_chat_turn(running.request_id, now())
            .expect("cancel")
            .turn
            .phase,
        ChatTurnPhase::CancelRequested
    );
    let finished = store
        .finish_chat_turn(&finish(running.request_id, 1, fence))
        .expect("completion race");
    assert_eq!(finished.turn.phase, ChatTurnPhase::Cancelled);
    let before = store.events().expect("events").len();
    assert!(
        store
            .cancel_chat_turn(running.request_id, now())
            .expect("repeat cancel")
            .duplicate
    );
    assert_eq!(store.events().expect("events").len(), before);
}

fn old_ask(request_id: Uuid) -> vibemux_store::HarnessDispatchAdmission {
    vibemux_store::HarnessDispatchAdmission {
        request_id,
        harness: AgentKind::Codex,
        protocol: NativeProtocol::CodexAppServer,
        prompt: PromptDigest {
            sha256: Sha256Digest::of(b"synthetic prompt"),
            byte_count: 16,
        },
        config_sha256: Sha256Digest::of(b"synthetic config"),
        resource_key: Sha256Digest::from_hex(&"c".repeat(64)).expect("resource"),
        base_commit: "d".repeat(40),
        timestamp: now(),
    }
}

#[test]
fn old_ask_and_chat_reservations_and_submission_keys_exclude_each_other() {
    let (_temp, mut store) = new_store();
    let (session, _fence) = ready(&mut store);
    let ask = old_ask(Uuid::new_v4());
    store
        .admit_harness_dispatch(&ask)
        .expect("old Ask admitted");
    let new_turn = ChatTurnAdmission {
        chat_id: session.chat_id,
        request_id: Uuid::new_v4(),
        expected_generation: 1,
        base_commit: "d".repeat(40),
        prompt_ref: reference(12),
        timestamp: now(),
    };
    assert_eq!(code(store.admit_chat_turn(&new_turn)), "chat_busy");
    store
        .cancel_harness_dispatch(ask.request_id, now())
        .expect("cancel unsent Ask");
    store.admit_chat_turn(&new_turn).expect("chat admitted");
    assert_eq!(
        code(store.admit_harness_dispatch(&old_ask(Uuid::new_v4()))),
        "harness_dispatch_busy"
    );
    store
        .cancel_chat_turn(new_turn.request_id, now())
        .expect("cancel unsent chat");
    assert_eq!(
        code(store.admit_harness_dispatch(&old_ask(new_turn.request_id))),
        "harness_dispatch_conflict"
    );
    let mut collision = new_turn;
    collision.request_id = ask.request_id;
    assert_eq!(code(store.admit_chat_turn(&collision)), "chat_conflict");
}

#[test]
fn startup_recovery_never_replays_claimed_turns_or_silently_releases_them() {
    let (temp, mut store) = new_store();
    let (session, fence) = ready(&mut store);
    let admission = admit(&mut store, &session);
    store
        .claim_chat_turn(admission.request_id, 1, fence, now())
        .expect("send claim");
    drop(store);
    let mut store = SqliteStore::open(&temp.path().join("state.sqlite3")).expect("reopen");
    let recovered = store.recover_chats(now()).expect("recover");
    assert_eq!(recovered.turns.len(), 1);
    assert!(recovered.quarantined.is_empty());
    assert_eq!(
        recovered.turns[0].turn.phase,
        ChatTurnPhase::RecoveryRequired
    );
    assert_eq!(
        code(store.finish_chat_turn(&finish(admission.request_id, 1, fence))),
        "chat_stale_fence"
    );
    assert_eq!(
        code(store.cancel_chat_turn(admission.request_id, now())),
        "chat_invalid_transition"
    );
    assert_eq!(
        code(store.resume_chat(&mutation(session.chat_id, 1))),
        "chat_busy"
    );
    assert_eq!(
        code(store.admit_harness_dispatch(&old_ask(Uuid::new_v4()))),
        "harness_dispatch_busy"
    );
    assert_eq!(
        code(store.acknowledge_chat_recovery(&mutation(session.chat_id, 1), false)),
        "chat_invalid_input"
    );
    store
        .acknowledge_chat_recovery(&mutation(session.chat_id, 1), true)
        .expect("explicit verified cleanup");
    assert_eq!(
        store
            .chat_turn(admission.request_id)
            .expect("turn")
            .expect("known")
            .phase,
        ChatTurnPhase::Interrupted
    );
    store
        .admit_harness_dispatch(&old_ask(Uuid::new_v4()))
        .expect("reservation released only after acknowledgement");
    assert!(
        store
            .admit_chat_turn(&admission)
            .expect("old key remains a receipt")
            .duplicate
    );
}

#[test]
fn unsent_admission_is_interrupted_without_becoming_a_sendable_retry() {
    let (_temp, mut store) = new_store();
    let (session, fence) = ready(&mut store);
    let admission = admit(&mut store, &session);
    let recovery = store.recover_chats(now()).expect("recover");
    assert_eq!(recovery.turns[0].turn.phase, ChatTurnPhase::Interrupted);
    assert_eq!(
        code(store.claim_chat_turn(admission.request_id, 1, fence, now())),
        "chat_invalid_transition"
    );
    assert!(
        store
            .admit_chat_turn(&admission)
            .expect("replay receipt")
            .duplicate
    );
    assert!(
        store
            .recover_chats(now())
            .expect("idempotent startup recovery")
            .sessions
            .is_empty()
    );
}

#[test]
fn live_uncertainty_retains_resource_until_explicit_cleanup_acknowledgement() {
    let (_temp, mut store) = new_store();
    let (session, fence) = ready(&mut store);
    let admission = admit(&mut store, &session);
    store
        .claim_chat_turn(admission.request_id, 1, fence, now())
        .expect("claim");
    let mut report = finish(admission.request_id, 1, fence);
    report.outcome = ChatTurnOutcome::Interrupted;
    report.correlated_terminal = false;
    report.error_code = Some(ChatFailureCode::ProcessLost);
    report.output_complete = false;
    let unresolved = store
        .finish_chat_turn(&report)
        .expect("unknown owned process");
    assert_eq!(unresolved.turn.phase, ChatTurnPhase::RecoveryRequired);
    assert_eq!(
        unresolved.session.active_request_id,
        Some(admission.request_id)
    );
    assert_eq!(
        code(store.admit_harness_dispatch(&old_ask(Uuid::new_v4()))),
        "harness_dispatch_busy"
    );
    store
        .acknowledge_chat_recovery(&mutation(session.chat_id, 1), true)
        .expect("cleanup proven");
    assert_eq!(
        store
            .chat_session(session.chat_id)
            .expect("session")
            .expect("known")
            .state,
        ChatState::Closed
    );
}

#[test]
fn record_append_is_idempotent_and_content_is_not_shared_between_owners() {
    let (_temp, mut store) = new_store();
    let (session, fence) = ready(&mut store);
    let admission = admit(&mut store, &session);
    store
        .claim_chat_turn(admission.request_id, 1, fence, now())
        .expect("claim");
    let append = ChatRecordAppend {
        request_id: admission.request_id,
        generation: 1,
        fence,
        record_id: Uuid::new_v4(),
        kind: ChatRecordKind::Assistant,
        content_ref: reference(24),
        original_byte_count: 24,
        timestamp: now(),
    };
    let first = store.append_chat_record(&append).expect("append");
    let before = store.events().expect("events").len();
    let repeated = store.append_chat_record(&append).expect("repeat");
    assert!(repeated.duplicate);
    assert_eq!(first.record, repeated.record);
    assert_eq!(store.events().expect("events").len(), before);
    let mut changed = append.clone();
    changed.content_ref = reference(25);
    changed.original_byte_count = 25;
    assert_eq!(code(store.append_chat_record(&changed)), "chat_conflict");
    let mut alias = append.clone();
    alias.record_id = Uuid::new_v4();
    assert_eq!(code(store.append_chat_record(&alias)), "chat_conflict");
    assert!(
        store
            .chat_content_in_use(append.content_ref.blob_id)
            .expect("content use")
    );
    assert_eq!(
        store
            .chat_records(session.chat_id, 0, 128)
            .expect("records")
            .len(),
        2
    );
}

#[test]
fn purge_erases_content_references_but_keeps_canonical_and_replay_tombstones() {
    let (_temp, mut store) = new_store();
    let (session, fence) = ready(&mut store);
    let title = reference(5);
    store
        .rename_chat(&mutation(session.chat_id, 1), title.clone())
        .expect("private title");
    let admission = admit(&mut store, &session);
    store
        .claim_chat_turn(admission.request_id, 1, fence, now())
        .expect("claim");
    let body = reference(24);
    store
        .append_chat_record(&ChatRecordAppend {
            request_id: admission.request_id,
            generation: 1,
            fence,
            record_id: Uuid::new_v4(),
            kind: ChatRecordKind::Assistant,
            content_ref: body.clone(),
            original_byte_count: body.byte_count,
            timestamp: now(),
        })
        .expect("body");
    store
        .finish_chat_turn(&finish(admission.request_id, 1, fence))
        .expect("finish");
    assert_eq!(
        code(store.purge_chat_content(&mutation(session.chat_id, 1))),
        "chat_invalid_transition"
    );
    store
        .release_chat_owner(&mutation(session.chat_id, 1), fence, true, true)
        .expect("close after cleanup");
    store
        .archive_chat(&mutation(session.chat_id, 1))
        .expect("archive");
    let purge = mutation(session.chat_id, 1);
    let deleted = store.purge_chat_content(&purge).expect("purge");
    assert_eq!(deleted.released_content.len(), 3);
    assert!(!store.chat_content_in_use(body.blob_id).expect("released"));
    assert!(
        !store
            .chat_content_in_use(title.blob_id)
            .expect("released title")
    );
    let records = store
        .chat_records(session.chat_id, 0, 128)
        .expect("tombstones");
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|record| record.content_ref.is_none()));
    assert!(
        store
            .admit_chat_turn(&admission)
            .expect("original send stays duplicate")
            .duplicate
    );
    let repeat = store.purge_chat_content(&purge).expect("repeat purge");
    assert!(repeat.duplicate);
    assert_eq!(repeat.released_content, deleted.released_content);
    assert_eq!(
        code(store.resume_chat(&mutation(session.chat_id, 1))),
        "chat_invalid_transition"
    );
}

#[test]
fn raw_vendor_identity_is_not_in_default_debug_or_audit_events() {
    let (_temp, mut store) = new_store();
    let opened = store.open_chat(&open_request()).expect("open");
    let id = opened.session.chat_id;
    let secret = "PRIVATE_VENDOR_SESSION_MARKER";
    let owner = store
        .claim_chat_owner(id, 1, Some(secret.to_owned()), now())
        .expect("owner");
    let session = store
        .bind_chat_vendor(id, 1, owner.fence, secret, now())
        .expect("proof")
        .session;
    assert!(!format!("{session:?}").contains(secret));
    assert!(
        !serde_json::to_string(&store.events().expect("events"))
            .expect("JSON")
            .contains(secret)
    );
    let mut report = finish(Uuid::new_v4(), 1, owner.fence);
    report.vendor_session_id = Some(secret.to_owned());
    assert!(
        !format!("{report:?}").contains(secret),
        "finish debug must be content-free"
    );
}

#[test]
fn canonical_projection_and_a2a_writes_cannot_bypass_chat_ownership() {
    use vibemux_events::{ActorName, EventDraft, EventPayload, EventType};
    use vibemux_types::{
        EventId, Run, RunSpec, Task,
        a2a::{A2aRunStart, RunWorkspace},
    };
    let (_temp, mut store) = new_store();
    let (session, _) = ready(&mut store);
    let admission = admit(&mut store, &session);
    let turn = store
        .chat_turn(admission.request_id)
        .expect("turn")
        .expect("known");
    let task: Task = serde_json::from_value(
        store
            .projection("task", &turn.task_id.to_string())
            .expect("projection")
            .expect("task"),
    )
    .expect("task type");
    let run: Run = serde_json::from_value(
        store
            .projection("run", &turn.run_id.to_string())
            .expect("projection")
            .expect("run"),
    )
    .expect("run type");
    let draft = || EventDraft {
        event_id: EventId::new(),
        event_type: EventType::new("task_updated").expect("eventtype"),
        project_id: session.project_id,
        task_id: None,
        run_id: None,
        causation_id: None,
        actor: ActorName::new("test").expect("actor"),
        timestamp: now(),
        idempotency_key: Some(Uuid::new_v4().to_string()),
        payload: EventPayload::new(serde_json::json!({"fixture":true})).expect("payload"),
    };
    assert_eq!(
        code(store.commit_task(&task, draft())),
        "store_chat_bound_projection"
    );
    assert_eq!(
        code(store.commit_run(&run, draft())),
        "store_chat_bound_projection"
    );
    let foreign = Run::new(RunSpec {
        project_id: session.project_id,
        task_id: turn.task_id,
        harness: "codex".to_owned(),
        role: "worker".to_owned(),
        protocol: "a2a".to_owned(),
        base_commit: "d".repeat(40),
    })
    .expect("foreign run");
    assert_eq!(
        code(store.commit_run(&foreign, draft())),
        "store_chat_bound_projection"
    );
    let start = A2aRunStart {
        task,
        run: foreign,
        peer_id: "fixture".to_owned(),
        external_task_id: "foreign".to_owned(),
        transport: "http_json".to_owned(),
        protocol_version: "1.0".to_owned(),
        workspace: RunWorkspace {
            path: "/synthetic/work".to_owned(),
            branch: "synthetic".to_owned(),
            base_commit: "d".repeat(40),
            ownership_token: "synthetic".to_owned(),
        },
        timestamp: now(),
        idempotency_key: "foreign_a2a".to_owned(),
    };
    assert_eq!(
        code(store.start_a2a_run(start)),
        "store_a2a_binding_conflict"
    );
}

#[test]
fn failed_record_transaction_rolls_back_bytes_metadata_and_event_together() {
    let (temp, mut store) = new_store();
    let (session, fence) = ready(&mut store);
    let admission = admit(&mut store, &session);
    store
        .claim_chat_turn(admission.request_id, 1, fence, now())
        .expect("claim");
    let before = store
        .chat_session(session.chat_id)
        .expect("session")
        .expect("known");
    let event_count = store.events().expect("events").len();
    let raw =
        rusqlite::Connection::open(temp.path().join("state.sqlite3")).expect("fault injector");
    raw.execute_batch("CREATE TRIGGER reject_chat_event BEFORE INSERT ON events WHEN NEW.idempotency_key LIKE 'chat_event:%' BEGIN SELECT RAISE(ABORT,'synthetic_fault'); END;").expect("fault trigger");
    let reference = reference(32);
    let result = store.append_chat_record(&ChatRecordAppend {
        request_id: admission.request_id,
        generation: 1,
        fence,
        record_id: Uuid::new_v4(),
        kind: ChatRecordKind::Assistant,
        content_ref: reference.clone(),
        original_byte_count: reference.byte_count,
        timestamp: now(),
    });
    assert!(result.is_err());
    assert_eq!(store.events().expect("events").len(), event_count);
    assert_eq!(
        store
            .chat_session(session.chat_id)
            .expect("unchanged")
            .expect("known"),
        before
    );
    assert!(
        !store
            .chat_content_in_use(reference.blob_id)
            .expect("definitively unreferenced")
    );
}

#[test]
fn record_provenance_cannot_be_repointed_to_another_valid_private_blob() {
    let (temp, mut store) = new_store();
    let (session, fence) = ready(&mut store);
    let admission = admit(&mut store, &session);
    store
        .claim_chat_turn(admission.request_id, 1, fence, now())
        .expect("claim");
    let appended = store
        .append_chat_record(&ChatRecordAppend {
            request_id: admission.request_id,
            generation: 1,
            fence,
            record_id: Uuid::new_v4(),
            kind: ChatRecordKind::Assistant,
            content_ref: reference(24),
            original_byte_count: 24,
            timestamp: now(),
        })
        .expect("reply");
    let raw =
        rusqlite::Connection::open(temp.path().join("state.sqlite3")).expect("corruption fixture");
    let mut changed = appended.record;
    changed.content_ref = Some(admission.prompt_ref.clone());
    changed.original_byte_count = admission.prompt_ref.byte_count;
    raw.execute(
        "UPDATE chat_records SET record_json=? WHERE record_id=?",
        rusqlite::params![
            serde_json::to_string(&changed).expect("JSON"),
            changed.record_id.to_string()
        ],
    )
    .expect("corrupt pointer only");
    assert_eq!(
        code(store.chat_records(session.chat_id, 0, 128)),
        "chat_projection_mismatch"
    );
}

#[test]
fn completed_record_requires_its_original_terminal_proof() {
    let (temp, mut store) = new_store();
    let (session, fence) = ready(&mut store);
    let admission = admit(&mut store, &session);
    store
        .claim_chat_turn(admission.request_id, 1, fence, now())
        .expect("claim");
    store
        .finish_chat_turn(&finish(admission.request_id, 1, fence))
        .expect("complete");
    let raw =
        rusqlite::Connection::open(temp.path().join("state.sqlite3")).expect("corruption fixture");
    raw.execute(
        "UPDATE chat_turns SET finish_json=NULL WHERE request_id=?",
        [admission.request_id.to_string()],
    )
    .expect("drop proof only");
    assert_eq!(
        code(store.chat_turn(admission.request_id)),
        "chat_projection_mismatch"
    );
}

#[test]
fn vendor_confirmation_from_a_different_generation_is_rejected_on_read() {
    let (temp, mut store) = new_store();
    let (mut session, _) = ready(&mut store);
    session.vendor_confirmed_generation = Some(99);
    let raw =
        rusqlite::Connection::open(temp.path().join("state.sqlite3")).expect("corruption fixture");
    raw.execute(
        "UPDATE chat_sessions SET record_json=? WHERE chat_id=?",
        rusqlite::params![
            serde_json::to_string(&session).expect("JSON"),
            session.chat_id.to_string()
        ],
    )
    .expect("corrupt confirmation");
    assert_eq!(
        code(store.chat_session(session.chat_id)),
        "chat_projection_mismatch"
    );
}

#[test]
fn content_budget_refusal_keeps_existing_replay_protection_and_records() {
    let (_temp, mut store) = new_store();
    let (session, fence) = ready(&mut store);
    let admission = admit(&mut store, &session);
    store
        .claim_chat_turn(admission.request_id, 1, fence, now())
        .expect("claim");
    for _ in 0..127 {
        store
            .append_chat_record(&ChatRecordAppend {
                request_id: admission.request_id,
                generation: 1,
                fence,
                record_id: Uuid::new_v4(),
                kind: ChatRecordKind::Assistant,
                content_ref: reference(MAX_CHAT_RECORD_BYTES),
                original_byte_count: MAX_CHAT_RECORD_BYTES,
                timestamp: now(),
            })
            .expect("bounded record");
    }
    let too_much = reference(MAX_CHAT_RECORD_BYTES);
    assert_eq!(
        code(store.append_chat_record(&ChatRecordAppend {
            request_id: admission.request_id,
            generation: 1,
            fence,
            record_id: Uuid::new_v4(),
            kind: ChatRecordKind::Assistant,
            content_ref: too_much.clone(),
            original_byte_count: too_much.byte_count,
            timestamp: now()
        })),
        "chat_capacity"
    );
    assert!(
        !store
            .chat_content_in_use(too_much.blob_id)
            .expect("failed reference absent")
    );
    assert!(
        store
            .admit_chat_turn(&admission)
            .expect("original binding retained")
            .duplicate
    );
    assert_eq!(
        store
            .chat_session(session.chat_id)
            .expect("session")
            .expect("known")
            .record_count,
        128
    );
}

#[test]
fn owner_and_session_budgets_refuse_without_discarding_existing_state() {
    let (_temp, mut store) = new_store();
    let mut owners = Vec::new();
    for _ in 0..3 {
        let (session, fence) = ready(&mut store);
        owners.push((session, fence));
    }
    let opened = store
        .open_chat(&open_request())
        .expect("fourth metadata")
        .session;
    assert_eq!(
        code(store.claim_chat_owner(
            opened.chat_id,
            1,
            Some("expected_session".to_owned()),
            now()
        )),
        "chat_capacity"
    );
    store
        .release_chat_owner(&mutation(owners[0].0.chat_id, 1), owners[0].1, true, false)
        .expect("verified release frees process slot");
    store
        .claim_chat_owner(
            opened.chat_id,
            1,
            Some("expected_session".to_owned()),
            now(),
        )
        .expect("slot reused");
    for _ in 4..128 {
        store.open_chat(&open_request()).expect("bounded session");
    }
    assert_eq!(code(store.open_chat(&open_request())), "chat_capacity");
    assert_eq!(
        store
            .list_chat_sessions(None, 128, true)
            .expect("retained sessions")
            .len(),
        128
    );
}

#[test]
fn each_new_turn_records_observed_head_while_replay_keeps_original_provenance() {
    let (_temp, mut store) = new_store();
    let (session, fence) = ready(&mut store);
    let first = admit(&mut store, &session);
    store
        .claim_chat_turn(first.request_id, 1, fence, now())
        .expect("claim");
    let original = store
        .finish_chat_turn(&finish(first.request_id, 1, fence))
        .expect("finish")
        .turn;
    let mut replay = first.clone();
    replay.base_commit = "e".repeat(40);
    let duplicate = store
        .admit_chat_turn(&replay)
        .expect("duplicate before new HEAD observations");
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.turn.base_commit, "d".repeat(40));
    replay.request_id = Uuid::new_v4();
    replay.prompt_ref = reference(18);
    let second = store
        .admit_chat_turn(&replay)
        .expect("new turn at moved HEAD")
        .turn;
    assert_eq!(second.base_commit, "e".repeat(40));
    let run = store
        .projection("run", &second.run_id.to_string())
        .expect("run")
        .expect("known");
    assert_eq!(run["base_commit"], "e".repeat(40));
    assert_eq!(
        store
            .chat_session(session.chat_id)
            .expect("session")
            .expect("known")
            .base_commit,
        "d".repeat(40)
    );
    let first_page = store.chat_turns(session.chat_id, 0, 1).expect("page1");
    assert_eq!(first_page[0].turn_id, original.turn_id);
    let next = store
        .chat_turns(session.chat_id, first_page[0].admitted_sequence, 1)
        .expect("page2");
    assert_eq!(next[0].turn_id, second.turn_id);
    assert_eq!(
        store
            .chat_turn_by_id(session.chat_id, second.turn_id)
            .expect("lookup")
            .expect("known")
            .request_id,
        replay.request_id
    );
}

#[test]
fn concurrent_duplicates_share_one_canonical_admission_across_connections() {
    let (temp, mut store) = new_store();
    let (session, _) = ready(&mut store);
    let request = ChatTurnAdmission {
        chat_id: session.chat_id,
        request_id: Uuid::new_v4(),
        expected_generation: 1,
        base_commit: "d".repeat(40),
        prompt_ref: reference(12),
        timestamp: now(),
    };
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
    let threads = (0..4)
        .map(|_| {
            let path = temp.path().join("state.sqlite3");
            let request = request.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut store = SqliteStore::open(&path).expect("parallel connection");
                barrier.wait();
                store.admit_chat_turn(&request).expect("same binding")
            })
        })
        .collect::<Vec<_>>();
    let receipts = threads
        .into_iter()
        .map(|thread| thread.join().expect("thread"))
        .collect::<Vec<_>>();
    assert_eq!(
        receipts.iter().filter(|receipt| !receipt.duplicate).count(),
        1
    );
    assert!(
        receipts
            .iter()
            .all(|receipt| receipt.turn.task_id == receipts[0].turn.task_id
                && receipt.turn.run_id == receipts[0].turn.run_id)
    );
    assert_eq!(
        store
            .chat_records(session.chat_id, 0, 128)
            .expect("single user record")
            .len(),
        1
    );
}

#[test]
fn racing_old_ask_and_chat_admission_has_one_resource_owner() {
    let (temp, mut store) = new_store();
    let (session, _) = ready(&mut store);
    let request = ChatTurnAdmission {
        chat_id: session.chat_id,
        request_id: Uuid::new_v4(),
        expected_generation: 1,
        base_commit: "d".repeat(40),
        prompt_ref: reference(12),
        timestamp: now(),
    };
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let chat_path = temp.path().join("state.sqlite3");
    let chat_barrier = barrier.clone();
    let chat = std::thread::spawn(move || {
        let mut store = SqliteStore::open(&chat_path).expect("chat store");
        chat_barrier.wait();
        store.admit_chat_turn(&request).is_ok()
    });
    let ask_path = temp.path().join("state.sqlite3");
    let ask = std::thread::spawn(move || {
        let mut store = SqliteStore::open(&ask_path).expect("Ask store");
        barrier.wait();
        store
            .admit_harness_dispatch(&old_ask(Uuid::new_v4()))
            .is_ok()
    });
    assert_ne!(
        chat.join().expect("chat thread"),
        ask.join().expect("Ask thread"),
        "exactly one path may acquire the resource"
    );
}

fn remove_new_chat_schema_for_historical_fixture(path: &std::path::Path) {
    let raw = rusqlite::Connection::open(path).expect("owned historical fixture");
    raw.execute_batch("DROP TABLE workspace_cleanup_commands; DROP TABLE workspace_cleanups; DROP TABLE chat_approvals; DROP TABLE chat_execution_records; DROP TABLE chat_execution_attempts; DROP TABLE chat_execution_owners; DROP TABLE chat_operations; DROP TABLE chat_records; DROP TABLE chat_content; DROP TABLE chat_turns; DROP TABLE chat_sessions; DELETE FROM schema_migrations WHERE version>=10;").expect("retain unchanged schema9 tables");
    assert_eq!(
        raw.query_row("SELECT MAX(version) FROM schema_migrations", [], |row| row
            .get::<_, u32>(
            0
        ))
        .expect("historical version"),
        9
    );
}

#[test]
fn schema_nine_upgrade_preserves_existing_dispatch_records_and_events() {
    let (temp, mut store) = new_store();
    let request = old_ask(Uuid::new_v4());
    let old = store
        .admit_harness_dispatch(&request)
        .expect("historical Ask")
        .record;
    let events = serde_json::to_value(store.events().expect("events")).expect("JSON");
    drop(store);
    let path = temp.path().join("state.sqlite3");
    remove_new_chat_schema_for_historical_fixture(&path);
    let store = SqliteStore::open(&path).expect("migration10");
    assert_eq!(
        store
            .harness_dispatch(request.request_id)
            .expect("record")
            .expect("known"),
        old
    );
    assert_eq!(
        serde_json::to_value(store.events().expect("events")).expect("JSON"),
        events
    );
    assert!(
        store
            .list_chat_sessions(None, 128, true)
            .expect("new table")
            .is_empty()
    );
    let raw = rusqlite::Connection::open(path).expect("schema query");
    let version: u32 = raw
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .expect("version");
    assert_eq!(version, vibemux_store::STORE_SCHEMA_VERSION);
}

#[test]
fn failed_schema_ten_creation_rolls_back_all_new_tables_and_marker() {
    let (temp, store) = new_store();
    drop(store);
    let path = temp.path().join("state.sqlite3");
    remove_new_chat_schema_for_historical_fixture(&path);
    let raw = rusqlite::Connection::open(&path).expect("fault fixture");
    raw.execute_batch("CREATE TABLE chat_operations(synthetic INTEGER);")
        .expect("late migration conflict");
    assert!(SqliteStore::open(&path).is_err());
    let version: u32 = raw
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .expect("version");
    assert_eq!(version, 9);
    let exists:bool=raw.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='chat_sessions')",[],|row|row.get(0)).expect("table");
    assert!(!exists);
}

#[test]
fn truncated_records_keep_true_source_size_and_preflight_reads_survive_purge() {
    let (_temp, mut store) = new_store();
    let open = open_request();
    let opened = store.open_chat(&open).expect("open").session;
    assert!(
        store
            .chat_operation_known(open.operation_id)
            .expect("known open")
    );
    let claim = store
        .claim_chat_owner(
            opened.chat_id,
            1,
            Some("expected_session".to_owned()),
            now(),
        )
        .expect("owner");
    let session = store
        .bind_chat_vendor(opened.chat_id, 1, claim.fence, "expected_session", now())
        .expect("vendor")
        .session;
    let admission = admit(&mut store, &session);
    store
        .claim_chat_turn(admission.request_id, 1, claim.fence, now())
        .expect("claim turn");
    let mut append = ChatRecordAppend {
        request_id: admission.request_id,
        generation: 1,
        fence: claim.fence,
        record_id: Uuid::new_v4(),
        kind: ChatRecordKind::Assistant,
        content_ref: reference(12),
        original_byte_count: 200,
        timestamp: now(),
    };
    let record = store
        .append_chat_record(&append)
        .expect("truncated private display")
        .record;
    assert_eq!(record.original_byte_count, 200);
    assert_eq!(record.content_ref.as_ref().expect("stored").byte_count, 12);
    assert_eq!(
        store
            .chat_record_by_id(session.chat_id, append.record_id)
            .expect("known record")
            .expect("record"),
        record
    );
    append.original_byte_count = 201;
    assert_eq!(code(store.append_chat_record(&append)), "chat_conflict");
    append.record_id = Uuid::new_v4();
    append.original_byte_count = 11;
    assert_eq!(
        code(store.append_chat_record(&append)),
        "chat_invalid_input"
    );
    append.original_byte_count = MAX_CHAT_BYTES + 1;
    assert_eq!(
        code(store.append_chat_record(&append)),
        "chat_invalid_input"
    );
    store
        .finish_chat_turn(&finish(admission.request_id, 1, claim.fence))
        .expect("finish");
    store
        .release_chat_owner(&mutation(session.chat_id, 1), claim.fence, true, true)
        .expect("close");
    store
        .archive_chat(&mutation(session.chat_id, 1))
        .expect("archive");
    store
        .purge_chat_content(&mutation(session.chat_id, 1))
        .expect("purge");
    assert!(
        store
            .chat_operation_known(open.operation_id)
            .expect("open still known")
    );
    let tombstone = store
        .chat_record_by_id(session.chat_id, record.record_id)
        .expect("known tombstone")
        .expect("retained");
    assert!(tombstone.content_ref.is_none());
    assert_eq!(tombstone.original_byte_count, 200);
}

#[test]
fn unknown_idle_owners_keep_process_budget_until_acknowledged() {
    let (_temp, mut store) = new_store();
    let owners = (0..3).map(|_| ready(&mut store).0).collect::<Vec<_>>();
    let recovery = store.recover_chats(now()).expect("recover idle owners");
    assert_eq!(recovery.sessions.len(), 3);
    assert!(recovery.turns.is_empty());
    assert!(
        recovery
            .sessions
            .iter()
            .all(|entry| entry.session.state == ChatState::RecoveryRequired
                && entry.session.active_request_id.is_none())
    );
    let mut request = open_request();
    request.resource_key = "e".repeat(64);
    let fourth = store
        .open_chat(&request)
        .expect("metadata-only open")
        .session;
    assert_eq!(
        code(store.claim_chat_owner(fourth.chat_id, 1, Some("fourth_session".to_owned()), now())),
        "chat_capacity"
    );
    assert_eq!(
        code(store.acknowledge_chat_recovery(&mutation(owners[0].chat_id, 1), false)),
        "chat_invalid_input"
    );
    assert_eq!(
        code(store.claim_chat_owner(fourth.chat_id, 1, Some("fourth_session".to_owned()), now())),
        "chat_capacity"
    );
    store
        .acknowledge_chat_recovery(&mutation(owners[0].chat_id, 1), true)
        .expect("verified cleanup releases one slot");
    store
        .claim_chat_owner(fourth.chat_id, 1, Some("fourth_session".to_owned()), now())
        .expect("remaining slot may be claimed");
}

#[test]
fn losing_one_idle_owner_is_fenced_and_preserves_other_sessions() {
    let (_temp, mut store) = new_store();
    let (first, fence) = ready(&mut store);
    let mut another = open_request();
    another.resource_key = "e".repeat(64);
    let second = store.open_chat(&another).expect("second chat").session;
    let second_owner = store
        .claim_chat_owner(second.chat_id, 1, None, now())
        .expect("second owner");
    assert_eq!(
        code(store.lose_chat_owner(first.chat_id, 2, fence, now())),
        "chat_stale_generation"
    );
    assert_eq!(
        code(store.lose_chat_owner(first.chat_id, 1, second_owner.fence, now())),
        "chat_stale_fence"
    );
    let lost = store
        .lose_chat_owner(first.chat_id, 1, fence, now())
        .expect("record lost owner");
    assert_eq!(lost.session.state, ChatState::RecoveryRequired);
    assert_eq!(
        store
            .chat_session(second.chat_id)
            .expect("read second")
            .expect("second")
            .state,
        ChatState::Opening
    );
    assert_eq!(
        code(store.resume_chat(&mutation(first.chat_id, 1))),
        "chat_invalid_transition"
    );
    assert_eq!(
        code(store.lose_chat_owner(first.chat_id, 1, fence, now())),
        "chat_stale_fence"
    );
    let replacement = store
        .open_chat(&open_request())
        .expect("replacement metadata")
        .session;
    assert_eq!(
        code(store.claim_chat_owner(replacement.chat_id, 1, None, now())),
        "chat_busy"
    );
}

#[test]
fn runtime_unsent_cancel_requires_the_exact_live_owner_and_an_unclaimed_turn() {
    let (_temp, mut store) = new_store();
    let (session, fence) = ready(&mut store);
    let other = store
        .open_chat(&open_request())
        .expect("other chat")
        .session;
    let other_fence = store
        .claim_chat_owner(other.chat_id, 1, None, now())
        .expect("other idle owner")
        .fence;
    let admission = admit(&mut store, &session);
    assert_eq!(
        code(store.cancel_unsent_chat_turn(admission.request_id, 1, other_fence, now())),
        "chat_stale_fence"
    );
    assert_eq!(
        code(store.cancel_unsent_chat_turn(admission.request_id, 2, fence, now())),
        "chat_stale_generation"
    );
    assert_eq!(
        store
            .chat_turn(admission.request_id)
            .expect("read")
            .expect("turn")
            .phase,
        ChatTurnPhase::Admitted
    );
    let cancelled = store
        .cancel_unsent_chat_turn(admission.request_id, 1, fence, now())
        .expect("cancel owned admission");
    assert_eq!(cancelled.turn.phase, ChatTurnPhase::Cancelled);
    let second = admit(&mut store, &session);
    store
        .claim_chat_turn(second.request_id, 1, fence, now())
        .expect("claim second");
    assert_eq!(
        code(store.cancel_unsent_chat_turn(second.request_id, 1, fence, now())),
        "chat_invalid_transition"
    );
    assert_eq!(
        store
            .chat_turn(second.request_id)
            .expect("read")
            .expect("turn")
            .phase,
        ChatTurnPhase::Running
    );
}

#[test]
fn close_receipt_reserves_its_id_before_effects_and_excludes_ask_ids() {
    let (_temp, mut store) = new_store();
    let (session, fence) = ready(&mut store);
    let request = mutation(session.chat_id, 1);
    let accepted = store.request_chat_close(&request).expect("reserve close");
    assert!(!accepted.duplicate);
    assert_eq!(accepted.session.state, ChatState::Ready);
    assert_eq!(
        code(store.admit_harness_dispatch(&old_ask(request.operation_id))),
        "harness_dispatch_conflict"
    );
    store
        .release_chat_owner(&mutation(session.chat_id, 1), fence, true, true)
        .expect("verified close");
    store
        .resume_chat(&mutation(session.chat_id, 1))
        .expect("explicit resume metadata");
    let replay = store.close_chat(&request).expect("old close receipt");
    assert!(replay.duplicate);
    assert_eq!(replay.session.generation, 2);
    assert_eq!(replay.session.state, ChatState::Opening);
    let ask = old_ask(Uuid::new_v4());
    store.admit_harness_dispatch(&ask).expect("Ask admission");
    store
        .cancel_harness_dispatch(ask.request_id, now())
        .expect("cancel unsent Ask");
    let conflict = ChatMutation {
        operation_id: ask.request_id,
        ..mutation(session.chat_id, 2)
    };
    assert_eq!(code(store.request_chat_close(&conflict)), "chat_conflict");
}

#[test]
fn unknown_idle_owners_hold_resource_for_new_asks_and_chat_claims() {
    let (_temp, mut store) = new_store();
    let (original, _) = ready(&mut store);
    store
        .recover_chats(now())
        .expect("recover without an active turn");
    let replacement = store
        .open_chat(&open_request())
        .expect("new metadata")
        .session;
    assert_eq!(
        code(store.claim_chat_owner(
            replacement.chat_id,
            1,
            Some("replacement".to_owned()),
            now()
        )),
        "chat_busy"
    );
    assert_eq!(
        code(store.admit_harness_dispatch(&old_ask(Uuid::new_v4()))),
        "harness_dispatch_busy"
    );
    store
        .acknowledge_chat_recovery(&mutation(original.chat_id, 1), true)
        .expect("verified original-owner cleanup");
    let ask = old_ask(Uuid::new_v4());
    store
        .admit_harness_dispatch(&ask)
        .expect("Ask may use released resource");
    store
        .cancel_harness_dispatch(ask.request_id, now())
        .expect("release unsent Ask");
    store
        .claim_chat_owner(
            replacement.chat_id,
            1,
            Some("replacement".to_owned()),
            now(),
        )
        .expect("new chat may use released resource");
}

#[test]
fn operation_session_lookup_preserves_opening_binding_after_observations_change() {
    let (_temp, mut store) = new_store();
    let open = open_request();
    let original = store.open_chat(&open).expect("open").session;
    let count = store.events().expect("events").len();
    assert!(
        store
            .chat_session_for_operation(Uuid::new_v4())
            .expect("unknown operation")
            .is_none()
    );
    assert_eq!(
        code(store.chat_session_for_operation(Uuid::nil())),
        "chat_invalid_input"
    );
    let known = store
        .chat_session_for_operation(open.operation_id)
        .expect("read original binding")
        .expect("session");
    assert_eq!(known, original);
    assert_eq!(store.events().expect("read does not mutate").len(), count);
    let mut replay = open.clone();
    replay.base_commit = "e".repeat(40);
    replay.profile_sha256 = "f".repeat(64);
    assert_eq!(code(store.open_chat(&replay)), "chat_conflict");
    replay.base_commit = known.base_commit;
    replay.profile_sha256 = known.profile_sha256;
    replay.resource_key = known.resource_key;
    assert!(
        store
            .open_chat(&replay)
            .expect("reconstructed original server observations")
            .duplicate
    );
    store
        .close_chat(&mutation(original.chat_id, 1))
        .expect("close unclaimed metadata");
    store
        .archive_chat(&mutation(original.chat_id, 1))
        .expect("archive");
    store
        .purge_chat_content(&mutation(original.chat_id, 1))
        .expect("purge");
    let current = store
        .chat_session_for_operation(open.operation_id)
        .expect("read after purge")
        .expect("retained binding");
    assert_eq!(current.chat_id, original.chat_id);
    assert_eq!(current.state, ChatState::Archived);
    assert!(current.content_purged);
    assert_eq!(current.base_commit, "d".repeat(40));
    assert_eq!(current.profile_sha256, "b".repeat(64));
}

#[test]
fn operation_session_lookup_applies_normal_session_integrity_validation() {
    let (temp, mut store) = new_store();
    let open = open_request();
    let mut session = store.open_chat(&open).expect("open").session;
    session.version += 1;
    let raw = rusqlite::Connection::open(temp.path().join("state.sqlite3"))
        .expect("owned corruption fixture");
    raw.execute(
        "UPDATE chat_sessions SET record_json=? WHERE chat_id=?",
        rusqlite::params![
            serde_json::to_string(&session).expect("JSON"),
            session.chat_id.to_string()
        ],
    )
    .expect("corrupt version mismatch");
    assert_eq!(
        code(store.chat_session_for_operation(open.operation_id)),
        "chat_projection_mismatch"
    );
}
