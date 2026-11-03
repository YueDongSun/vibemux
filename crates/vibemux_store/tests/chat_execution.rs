//! Owner-backed submissions keep one canonical Task/Run across all attempts.
use std::collections::{BTreeMap, BTreeSet};
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_harness::HarnessDetection;
use vibemux_store::{
    ChatExecutionAttemptAdmission, ChatExecutionOwnerOpen, ChatExecutionProcessChange,
    ChatExecutionStart, ChatMutation, ChatOwnerChange, ChatOwnerMutation, ChatTurnFinish,
    SqliteStore,
};
use vibemux_types::{
    RunStatus,
    a2a::RunWorkspace,
    chat::{ChatHarness, ChatState, ChatTurnOutcome, ContentRef},
};

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_793_433_600).expect("time")
}
fn ready_owner(
    store: &mut SqliteStore,
    harness: ChatHarness,
) -> vibemux_types::chat_owner::ChatExecutionOwner {
    store
        .commit_harness_snapshot(
            &BTreeMap::from([(
                harness.as_str().into(),
                HarnessDetection {
                    detected: true,
                    path: None,
                    launcher: None,
                    version: Some("fixture".into()),
                },
            )]),
            "2026-11-01T00:00:00Z",
            "execution_detection",
        )
        .expect("detection");
    let mut owner = store
        .open_chat_execution_owner(&ChatExecutionOwnerOpen {
            operation_id: Uuid::new_v4(),
            harness,
            profile_sha256: "a".repeat(64),
            base_commit: "b".repeat(40),
            admitted_tools: BTreeSet::new(),
            title_ref: None,
            timestamp: now(),
        })
        .expect("owner")
        .owner;
    let workspace = RunWorkspace {
        path: format!("/owned/{}", owner.run_id),
        branch: format!("codex/run_{}", owner.run_id.as_uuid().simple()),
        base_commit: owner.base_commit.clone(),
        ownership_token: Uuid::new_v4().simple().to_string(),
    };
    for change in [
        ChatOwnerChange::Plan {
            workspace,
            resource_key: "c".repeat(64),
        },
        ChatOwnerChange::ClaimCreation,
        ChatOwnerChange::VerifyWorkspace,
    ] {
        owner = store
            .change_chat_execution_owner(
                &ChatOwnerMutation {
                    operation_id: Uuid::new_v4(),
                    chat_id: owner.chat_id,
                    expected_version: owner.version,
                    timestamp: now(),
                },
                &change,
            )
            .expect("workspace intent")
            .owner;
    }
    owner
}

fn process_input(
    owner: &vibemux_types::chat_owner::ChatExecutionOwner,
    generation: u64,
    vendor: Option<String>,
) -> ChatExecutionStart {
    ChatExecutionStart {
        operation_id: Uuid::new_v4(),
        chat_id: owner.chat_id,
        expected_generation: generation,
        expected_owner_version: owner.version,
        profile_sha256: owner.profile_sha256.clone(),
        resource_key: owner.resource_key.clone().expect("resource"),
        observed_commit: owner.base_commit.clone(),
        expected_vendor_id: vendor,
        timestamp: now(),
    }
}

#[test]
fn process_start_lookup_replays_the_original_binding_without_new_launch_authority() {
    let temp = tempfile::tempdir().expect("store fixture");
    let mut store = SqliteStore::open(&temp.path().join("state.sqlite3")).expect("store");
    let owner = ready_owner(&mut store, ChatHarness::Claude);
    let input = process_input(&owner, 1, Some(Uuid::new_v4().to_string()));
    assert!(
        store
            .replay_chat_execution_start(input.operation_id, owner.chat_id, 1, owner.version)
            .expect("not admitted")
            .is_none()
    );
    let started = store.start_chat_execution_process(&input).expect("start");
    let fence = started.fence.expect("fresh authority");
    store
        .change_chat_execution_process(
            &mutation(owner.chat_id, 1),
            fence,
            &ChatExecutionProcessChange::Released {
                cleanup_verified: true,
                closed: false,
            },
        )
        .expect("verified suspension");
    let replay = store
        .replay_chat_execution_start(input.operation_id, owner.chat_id, 1, owner.version)
        .expect("lookup")
        .expect("retained receipt");
    assert!(replay.duplicate);
    assert!(replay.fence.is_none());
    assert_eq!(replay.sequence, started.sequence);
    assert_eq!(replay.session.state, ChatState::Suspended);
    for (chat_id, generation, version) in [
        (vibemux_types::ChatId::new(), 1, owner.version),
        (owner.chat_id, 2, owner.version),
        (owner.chat_id, 1, owner.version + 1),
    ] {
        assert!(
            store
                .replay_chat_execution_start(input.operation_id, chat_id, generation, version)
                .is_err()
        );
    }
}
fn mutation(chat_id: vibemux_types::ChatId, generation: u64) -> ChatMutation {
    ChatMutation {
        operation_id: Uuid::new_v4(),
        chat_id,
        expected_generation: generation,
        timestamp: now(),
    }
}
fn admission(
    owner: &vibemux_types::chat_owner::ChatExecutionOwner,
    generation: u64,
) -> ChatExecutionAttemptAdmission {
    let request_id = Uuid::new_v4();
    ChatExecutionAttemptAdmission {
        chat_id: owner.chat_id,
        request_id,
        generation,
        owner_version: owner.version,
        observed_commit: owner.base_commit.clone(),
        effective_tools: BTreeSet::new(),
        prompt_ref: ContentRef {
            blob_id: request_id,
            sha256: "d".repeat(64),
            byte_count: 16,
        },
        timestamp: now(),
    }
}
fn codex_process(
    store: &mut SqliteStore,
    owner: &vibemux_types::chat_owner::ChatExecutionOwner,
) -> vibemux_store::ChatOwnerFence {
    let process = store
        .start_chat_execution_process(&process_input(owner, 1, None))
        .expect("process");
    let fence = process.fence.expect("fresh fence");
    store
        .change_chat_execution_process(
            &mutation(owner.chat_id, 1),
            fence,
            &ChatExecutionProcessChange::VendorBound {
                vendor_id: "thread_owned".into(),
            },
        )
        .expect("identity");
    store
        .change_chat_execution_process(
            &mutation(owner.chat_id, 1),
            fence,
            &ChatExecutionProcessChange::Ready,
        )
        .expect("ready");
    fence
}

#[test]
fn concurrent_replays_admit_once_and_only_one_claim_grants_input() {
    let temp = tempfile::tempdir().expect("fixture");
    let database = temp.path().join("state.sqlite3");
    let mut store = SqliteStore::open(&database).expect("store");
    let owner = ready_owner(&mut store, ChatHarness::Codex);
    let fence = codex_process(&mut store, &owner);
    let input = admission(&owner, 1);
    drop(store);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
    let threads = (0..4)
        .map(|_| {
            let database = database.clone();
            let input = input.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut store = SqliteStore::open(&database).expect("connection");
                barrier.wait();
                store
                    .admit_chat_execution_attempt(&input)
                    .expect("same request")
            })
        })
        .collect::<Vec<_>>();
    let receipts = threads
        .into_iter()
        .map(|thread| thread.join().expect("join admission"))
        .collect::<Vec<_>>();
    assert_eq!(
        receipts.iter().filter(|receipt| !receipt.duplicate).count(),
        1
    );
    assert!(receipts.iter().all(|receipt| receipt.attempt.attempt_id
        == receipts[0].attempt.attempt_id
        && receipt.owner.run_id == owner.run_id));
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads = (0..2)
        .map(|_| {
            let database = database.clone();
            let barrier = barrier.clone();
            let request_id = input.request_id;
            std::thread::spawn(move || {
                let mut store = SqliteStore::open(&database).expect("connection");
                barrier.wait();
                store
                    .claim_chat_execution_attempt(request_id, 1, fence, now())
                    .is_ok()
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        threads
            .into_iter()
            .map(|thread| thread.join().expect("join claim"))
            .filter(|granted| *granted)
            .count(),
        1
    );
}

#[test]
fn two_submissions_settle_attempts_while_the_lifetime_run_keeps_running() {
    let temp = tempfile::tempdir().expect("temporary store");
    let database = temp.path().join("state.sqlite3");
    let mut store = SqliteStore::open(&database).expect("store");
    let owner = ready_owner(&mut store, ChatHarness::Codex);
    let start = ChatExecutionStart {
        operation_id: Uuid::new_v4(),
        chat_id: owner.chat_id,
        expected_generation: 1,
        expected_owner_version: owner.version,
        profile_sha256: owner.profile_sha256.clone(),
        resource_key: owner.resource_key.clone().expect("resource"),
        observed_commit: owner.base_commit.clone(),
        expected_vendor_id: None,
        timestamp: now(),
    };
    let process = store
        .start_chat_execution_process(&start)
        .expect("process authority");
    let fence = process.fence.expect("fresh process fence");
    for change in [
        ChatExecutionProcessChange::VendorBound {
            vendor_id: "thread_owned".into(),
        },
        ChatExecutionProcessChange::Ready,
    ] {
        store
            .change_chat_execution_process(
                &ChatMutation {
                    operation_id: Uuid::new_v4(),
                    chat_id: owner.chat_id,
                    expected_generation: 1,
                    timestamp: now(),
                },
                fence,
                &change,
            )
            .expect("ready generation");
    }
    let mut previous_attempt = None;
    for index in 0..2 {
        let current = store
            .chat_execution_owner(owner.chat_id)
            .expect("owner")
            .expect("known");
        let request_id = Uuid::new_v4();
        let input = ChatExecutionAttemptAdmission {
            chat_id: owner.chat_id,
            request_id,
            generation: 1,
            owner_version: current.version,
            observed_commit: owner.base_commit.clone(),
            effective_tools: BTreeSet::new(),
            prompt_ref: ContentRef {
                blob_id: request_id,
                sha256: "d".repeat(64),
                byte_count: 16,
            },
            timestamp: now(),
        };
        let admitted = store
            .admit_chat_execution_attempt(&input)
            .expect("admit attempt");
        assert_eq!(admitted.owner.run_id, owner.run_id);
        assert_eq!(admitted.owner.task_id, owner.task_id);
        assert_eq!(admitted.run.status(), RunStatus::Running);
        assert_ne!(previous_attempt, Some(admitted.attempt.attempt_id));
        previous_attempt = Some(admitted.attempt.attempt_id);
        let claim_time = now() + time::Duration::seconds(index * 10 + 1);
        let claimed = store
            .claim_chat_execution_attempt(request_id, 1, fence, claim_time)
            .expect("one-use input claim");
        assert_eq!(
            claimed.event.as_ref().expect("claim event").timestamp(),
            claim_time
        );
        let vendor_turn = format!("turn_{index}");
        let bound_time = claim_time + time::Duration::seconds(1);
        let bound = store
            .bind_chat_execution_attempt(request_id, 1, fence, &vendor_turn, bound_time)
            .expect("vendor acknowledgement");
        assert_eq!(
            bound.event.as_ref().expect("bind event").timestamp(),
            bound_time
        );
        let finished = store
            .finish_chat_execution_attempt(&ChatTurnFinish {
                request_id,
                generation: 1,
                fence,
                outcome: ChatTurnOutcome::Completed,
                correlated_terminal: true,
                vendor_session_id: Some("thread_owned".into()),
                vendor_turn_id: Some(vendor_turn),
                error_code: None,
                process_alive: true,
                timestamp: now(),
                output_complete: true,
            })
            .expect("attempt result");
        assert_eq!(finished.run.status(), RunStatus::Running);
        assert_eq!(finished.session.state, ChatState::Ready);
        assert!(
            store
                .admit_chat_execution_attempt(&input)
                .expect("exact replay")
                .duplicate
        );
    }
    drop(store);
    let store = SqliteStore::open(&database).expect("reopen");
    let attempts = store
        .chat_execution_attempts(owner.chat_id, 0, 16)
        .expect("history");
    assert_eq!(attempts.len(), 2);
    assert!(
        attempts
            .iter()
            .all(|attempt| attempt.owner_run_id == owner.run_id
                && attempt.outcome == Some(ChatTurnOutcome::Completed))
    );
}

#[test]
fn publication_rollback_and_namespaces_preserve_the_single_owner_pair() {
    use vibemux_store::ChatOpen;
    let temp = tempfile::tempdir().expect("fixture");
    let database = temp.path().join("state.sqlite3");
    let mut store = SqliteStore::open(&database).expect("store");
    let owner = ready_owner(&mut store, ChatHarness::Codex);
    let start = process_input(&owner, 1, None);
    let process = store.start_chat_execution_process(&start).expect("start");
    let fence = process.fence.expect("fresh authority");
    let duplicate = store
        .start_chat_execution_process(&start)
        .expect("start replay");
    assert!(duplicate.duplicate && duplicate.fence.is_none());
    store
        .change_chat_execution_process(
            &mutation(owner.chat_id, 1),
            fence,
            &ChatExecutionProcessChange::VendorBound {
                vendor_id: "thread_owned".into(),
            },
        )
        .expect("identity");
    store
        .change_chat_execution_process(
            &mutation(owner.chat_id, 1),
            fence,
            &ChatExecutionProcessChange::Ready,
        )
        .expect("ready");
    let mut input = admission(&owner, 1);
    input.request_id = start.operation_id;
    assert_eq!(
        store
            .admit_chat_execution_attempt(&input)
            .expect_err("operation collision")
            .code(),
        "chat_conflict"
    );
    input = admission(&owner, 1);
    let raw = rusqlite::Connection::open(&database).expect("fault fixture");
    raw.execute_batch("CREATE TRIGGER reject_attempt_event BEFORE INSERT ON events BEGIN SELECT RAISE(ABORT,'synthetic_attempt_fault'); END;").expect("inject rollback");
    let events = store.events().expect("events");
    assert!(store.admit_chat_execution_attempt(&input).is_err());
    let retained = store
        .chat_execution_owner_state(owner.chat_id)
        .expect("state")
        .expect("known");
    assert_eq!(retained.owner, owner);
    assert_eq!(retained.run.status(), RunStatus::Preparing);
    assert_eq!(retained.session.state, ChatState::Ready);
    assert_eq!(retained.session.retained_bytes, 0);
    assert!(
        store
            .chat_execution_attempts(owner.chat_id, 0, 16)
            .expect("attempts")
            .is_empty()
    );
    assert_eq!(store.events().expect("rollback event"), events);
    raw.execute_batch("DROP TRIGGER reject_attempt_event;")
        .expect("remove fault");
    let admitted = store
        .admit_chat_execution_attempt(&input)
        .expect("same request after rollback");
    assert_eq!(admitted.owner.run_id, owner.run_id);
    assert_eq!(
        store
            .open_chat(&ChatOpen {
                operation_id: input.request_id,
                harness: ChatHarness::Codex,
                profile_sha256: "a".repeat(64),
                resource_key: "e".repeat(64),
                base_commit: owner.base_commit,
                title_ref: None,
                timestamp: now()
            })
            .expect_err("attempt id cannot become legacy operation")
            .code(),
        "chat_conflict"
    );
    let pairs: u32 = raw
        .query_row(
            "SELECT COUNT(*) FROM projections WHERE entity_kind IN('task','run')",
            [],
            |row| row.get(0),
        )
        .expect("canonical pair count");
    assert_eq!(pairs, 2);
}

#[test]
fn owner_records_are_idempotent_bounded_and_validate_their_immutable_binding() {
    use vibemux_store::ChatRecordAppend;
    use vibemux_types::chat::ChatRecordKind;
    let temp = tempfile::tempdir().expect("fixture");
    let database = temp.path().join("state.sqlite3");
    let mut store = SqliteStore::open(&database).expect("store");
    let owner = ready_owner(&mut store, ChatHarness::Codex);
    let fence = codex_process(&mut store, &owner);
    let input = admission(&owner, 1);
    store.admit_chat_execution_attempt(&input).expect("admit");
    store
        .claim_chat_execution_attempt(input.request_id, 1, fence, now())
        .expect("claim");
    let record_id = Uuid::new_v4();
    let append = ChatRecordAppend {
        request_id: input.request_id,
        generation: 1,
        fence,
        record_id,
        kind: ChatRecordKind::Assistant,
        content_ref: ContentRef {
            blob_id: record_id,
            sha256: "e".repeat(64),
            byte_count: 5,
        },
        original_byte_count: 5,
        timestamp: now(),
    };
    let saved = store
        .append_chat_execution_record(&append)
        .expect("assistant reference");
    let events = store.events().expect("events");
    assert!(
        store
            .append_chat_execution_record(&append)
            .expect("record replay")
            .duplicate
    );
    assert_eq!(store.events().expect("no extra event"), events);
    let records = store
        .chat_execution_records(owner.chat_id, 0, 16)
        .expect("history");
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].kind, ChatRecordKind::User);
    assert_eq!(records[1], saved.record);
    let state = store
        .chat_execution_owner_state(owner.chat_id)
        .expect("state")
        .expect("known");
    assert_eq!(state.session.record_count, 2);
    assert_eq!(state.session.retained_bytes, 21);
    assert!(store.chat_execution_records(owner.chat_id, 0, 0).is_err());
    let raw = rusqlite::Connection::open(&database).expect("corruption fixture");
    raw.execute("UPDATE chat_execution_records SET record_json=json_set(record_json,'$.original_byte_count',999) WHERE record_id=?",[record_id.to_string()]).expect("change bounded metadata");
    assert_eq!(
        store
            .chat_execution_record(record_id)
            .expect_err("binding mismatch")
            .code(),
        "chat_projection_mismatch"
    );
    raw.execute("UPDATE chat_execution_records SET record_json=json_set(record_json,'$.original_byte_count',5) WHERE record_id=?",[record_id.to_string()]).expect("restore owned record fixture");
    raw.execute(
        "UPDATE chat_execution_attempts SET send_claimed=0,send_fence=NULL WHERE request_id=?",
        [input.request_id.to_string()],
    )
    .expect("inject contradictory Running claim");
    assert_eq!(
        store
            .chat_execution_attempt(input.request_id)
            .expect_err("Running requires the retained one-use claim")
            .code(),
        "chat_projection_mismatch"
    );
}

#[test]
fn startup_recovery_never_replays_input_or_silently_releases_process_authority() {
    use vibemux_types::chat::ChatTurnPhase;
    let temp = tempfile::tempdir().expect("fixture");
    let database = temp.path().join("state.sqlite3");
    let mut store = SqliteStore::open(&database).expect("store");
    let owner = ready_owner(&mut store, ChatHarness::Codex);
    let fence = codex_process(&mut store, &owner);
    let input = admission(&owner, 1);
    let admitted = store.admit_chat_execution_attempt(&input).expect("admit");
    store
        .claim_chat_execution_attempt(input.request_id, 1, fence, now())
        .expect("one input claim");
    drop(store);
    let mut store = SqliteStore::open(&database).expect("restart");
    let recovered = store
        .recover_chat_execution_processes(now())
        .expect("startup fencing");
    assert_eq!(recovered.sessions.len(), 1);
    assert!(recovered.quarantined.is_empty());
    assert_eq!(
        recovered.sessions[0].session.state,
        ChatState::RecoveryRequired
    );
    assert_eq!(
        store
            .chat_execution_attempt(input.request_id)
            .expect("attempt")
            .expect("known")
            .phase,
        ChatTurnPhase::RecoveryRequired
    );
    assert_eq!(
        store
            .start_chat_execution_process(&process_input(
                &admitted.owner,
                1,
                Some("thread_owned".into())
            ))
            .expect_err("old process authority retained")
            .code(),
        "chat_owner_already_claimed"
    );
    let events = store.events().expect("recovery events");
    assert!(
        store
            .recover_chat_execution_processes(now())
            .expect("repeat startup")
            .sessions
            .is_empty()
    );
    assert_eq!(store.events().expect("no repeated recovery"), events);
}

#[test]
fn schema_eleven_upgrade_and_failed_attempt_migration_preserve_owner_history() {
    let temp = tempfile::tempdir().expect("fixture");
    let database = temp.path().join("state.sqlite3");
    let mut store = SqliteStore::open(&database).expect("store");
    let owner = ready_owner(&mut store, ChatHarness::Codex);
    let events = store.events().expect("events");
    drop(store);
    let raw = rusqlite::Connection::open(&database).expect("closed migration fixture");
    raw.execute_batch("DROP TABLE workspace_cleanup_commands; DROP TABLE workspace_cleanups; DROP TABLE chat_approvals; DROP TABLE chat_execution_records; DROP TABLE chat_execution_attempts; DELETE FROM schema_migrations WHERE version>=12; CREATE INDEX chat_execution_active_attempt ON chat_sessions(resource_key);").expect("schema11 and late fault");
    assert!(SqliteStore::open(&database).is_err());
    let version: u32 = raw
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .expect("retained version");
    assert_eq!(version, 11);
    assert!(!raw.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='chat_execution_attempts' AND type='table')",[],|row|row.get::<_,bool>(0)).expect("rolled-back table"));
    raw.execute_batch("DROP INDEX chat_execution_active_attempt;")
        .expect("remove owned fault");
    drop(raw);
    let store = SqliteStore::open(&database).expect("forward migration");
    assert_eq!(
        store
            .chat_execution_owner(owner.chat_id)
            .expect("owner")
            .expect("known"),
        owner
    );
    assert_eq!(store.events().expect("events retained"), events);
    assert!(
        store
            .chat_execution_attempts(owner.chat_id, 0, 16)
            .expect("empty attempts")
            .is_empty()
    );
}

#[test]
fn uncertain_cleanup_retains_claims_and_verified_cleanup_allows_exact_resume() {
    use vibemux_types::chat::{ChatFailureCode, ChatTurnPhase};
    let temp = tempfile::tempdir().expect("fixture");
    let database = temp.path().join("state.sqlite3");
    let mut store = SqliteStore::open(&database).expect("store");
    let owner = ready_owner(&mut store, ChatHarness::Codex);
    let fence = codex_process(&mut store, &owner);
    let input = admission(&owner, 1);
    let admitted = store.admit_chat_execution_attempt(&input).expect("attempt");
    let claim_time = now() + time::Duration::seconds(20);
    store
        .claim_chat_execution_attempt(input.request_id, 1, fence, claim_time)
        .expect("claim");
    assert_eq!(
        store
            .claim_chat_execution_attempt(input.request_id, 1, fence, now())
            .expect_err("one use")
            .code(),
        "chat_turn_already_claimed"
    );
    let unknown = mutation(owner.chat_id, 1);
    let released = store
        .change_chat_execution_process(
            &unknown,
            fence,
            &ChatExecutionProcessChange::Released {
                cleanup_verified: false,
                closed: false,
            },
        )
        .expect("unknown cleanup");
    assert_eq!(released.session.state, ChatState::RecoveryRequired);
    assert!(released.event.as_ref().expect("event").timestamp() >= claim_time);
    let pending = store
        .chat_execution_attempt(input.request_id)
        .expect("attempt")
        .expect("known");
    assert_eq!(pending.phase, ChatTurnPhase::RecoveryRequired);
    assert_eq!(
        pending.error_code,
        Some(ChatFailureCode::OwnershipUncertain)
    );
    assert_eq!(released.run.status(), RunStatus::Running);
    assert_eq!(
        store
            .start_chat_execution_process(&process_input(
                &admitted.owner,
                1,
                Some("thread_owned".into())
            ))
            .expect_err("unreaped owner")
            .code(),
        "chat_owner_already_claimed"
    );
    assert_eq!(
        store
            .change_chat_execution_owner(
                &ChatOwnerMutation {
                    operation_id: Uuid::new_v4(),
                    chat_id: owner.chat_id,
                    expected_version: admitted.owner.version,
                    timestamp: now()
                },
                &ChatOwnerChange::Stop
            )
            .expect_err("no finalization while uncertain")
            .code(),
        "chat_busy"
    );
    let verified = store
        .change_chat_execution_process(
            &mutation(owner.chat_id, 1),
            fence,
            &ChatExecutionProcessChange::Released {
                cleanup_verified: true,
                closed: false,
            },
        )
        .expect("exact cleanup proof");
    assert_eq!(verified.session.state, ChatState::Suspended);
    assert_eq!(
        store
            .chat_execution_attempt(input.request_id)
            .expect("attempt")
            .expect("known")
            .phase,
        ChatTurnPhase::Interrupted
    );
    drop(store);
    let mut store = SqliteStore::open(&database).expect("reopen");
    let start = process_input(&verified.owner, 1, Some("thread_owned".into()));
    let resumed = store
        .start_chat_execution_process(&start)
        .expect("resume exact vendor");
    assert_eq!(resumed.session.generation, 2);
    assert!(resumed.fence.is_some());
    assert!(
        store
            .start_chat_execution_process(&start)
            .expect("replay")
            .fence
            .is_none()
    );
    let old_replay = store
        .change_chat_execution_process(
            &unknown,
            fence,
            &ChatExecutionProcessChange::Released {
                cleanup_verified: false,
                closed: false,
            },
        )
        .expect("old acknowledged operation");
    assert!(old_replay.duplicate && old_replay.fence.is_none());
    assert_eq!(old_replay.session.generation, 2);
    assert_eq!(old_replay.session.state, ChatState::Opening);
    assert!(
        store
            .admit_chat_execution_attempt(&input)
            .expect("historical admission replay")
            .duplicate
    );
}

#[test]
fn unsent_cancel_wins_the_claim_and_claimed_cancel_requires_a_result() {
    use vibemux_types::chat::ChatTurnPhase;
    let temp = tempfile::tempdir().expect("fixture");
    let mut store = SqliteStore::open(&temp.path().join("state.sqlite3")).expect("store");
    let owner = ready_owner(&mut store, ChatHarness::Codex);
    let fence = codex_process(&mut store, &owner);
    let input = admission(&owner, 1);
    let admitted = store.admit_chat_execution_attempt(&input).expect("first");
    let cancelled = store
        .cancel_chat_execution_attempt(input.request_id, 1, fence, now())
        .expect("cancel before input");
    assert_eq!(cancelled.attempt.phase, ChatTurnPhase::Cancelled);
    assert_eq!(cancelled.session.state, ChatState::Ready);
    assert!(
        store
            .claim_chat_execution_attempt(input.request_id, 1, fence, now())
            .is_err()
    );
    let second = admission(&admitted.owner, 1);
    store.admit_chat_execution_attempt(&second).expect("next");
    store
        .claim_chat_execution_attempt(second.request_id, 1, fence, now())
        .expect("send claimed");
    let requested = store
        .cancel_chat_execution_attempt(second.request_id, 1, fence, now())
        .expect("cancel requested");
    assert_eq!(requested.attempt.phase, ChatTurnPhase::CancelRequested);
    assert_eq!(requested.session.active_request_id, Some(second.request_id));
    assert!(
        store
            .cancel_chat_execution_attempt(second.request_id, 1, fence, now())
            .expect("cancel replay")
            .duplicate
    );
    assert_eq!(requested.run.status(), RunStatus::Running);
}

#[test]
fn first_claude_input_is_bounded_by_the_unused_uuid_and_completion_still_requires_proof() {
    let temp = tempfile::tempdir().expect("fixture");
    let mut store = SqliteStore::open(&temp.path().join("state.sqlite3")).expect("store");
    let owner = ready_owner(&mut store, ChatHarness::Claude);
    let vendor_id = Uuid::new_v4().to_string();
    let start = process_input(&owner, 1, Some(vendor_id.clone()));
    let process = store
        .start_chat_execution_process(&start)
        .expect("pinned Claude root");
    let fence = process.fence.expect("fresh fence");
    let ready = store
        .change_chat_execution_process(
            &mutation(owner.chat_id, 1),
            fence,
            &ChatExecutionProcessChange::Ready,
        )
        .expect("control initialize ready");
    assert!(ready.session.vendor_session_id.is_none());
    let input = admission(&owner, 1);
    store
        .admit_chat_execution_attempt(&input)
        .expect("first bootstrap input");
    store
        .claim_chat_execution_attempt(input.request_id, 1, fence, now())
        .expect("one input");
    assert_eq!(
        store
            .bind_chat_execution_attempt(input.request_id, 1, fence, "claude_turn", now())
            .expect_err("root proof missing")
            .code(),
        "chat_unconfirmed_session"
    );
    let finish = ChatTurnFinish {
        request_id: input.request_id,
        generation: 1,
        fence,
        outcome: ChatTurnOutcome::Completed,
        correlated_terminal: true,
        vendor_session_id: Some(vendor_id.clone()),
        vendor_turn_id: Some("claude_turn".into()),
        error_code: None,
        process_alive: true,
        timestamp: now(),
        output_complete: true,
    };
    assert!(store.finish_chat_execution_attempt(&finish).is_err());
    store
        .change_chat_execution_process(
            &mutation(owner.chat_id, 1),
            fence,
            &ChatExecutionProcessChange::VendorBound {
                vendor_id: vendor_id.clone(),
            },
        )
        .expect("validated root initialization");
    store
        .bind_chat_execution_attempt(input.request_id, 1, fence, "claude_turn", now())
        .expect("correlated acknowledgement");
    assert_eq!(
        store
            .finish_chat_execution_attempt(&finish)
            .expect("proved result")
            .run
            .status(),
        RunStatus::Running
    );
    store
        .change_chat_execution_process(
            &mutation(owner.chat_id, 1),
            fence,
            &ChatExecutionProcessChange::Released {
                cleanup_verified: true,
                closed: false,
            },
        )
        .expect("verified suspension");
    let current = store
        .chat_execution_owner(owner.chat_id)
        .expect("owner read")
        .expect("owner");
    let resumed = store
        .start_chat_execution_process(&process_input(&current, 1, Some(vendor_id.clone())))
        .expect("exact resume");
    assert_eq!(resumed.session.generation, 2);
    let resumed_fence = resumed.fence.expect("fresh generation fence");
    store
        .change_chat_execution_process(
            &mutation(owner.chat_id, 2),
            resumed_fence,
            &ChatExecutionProcessChange::Ready,
        )
        .expect("resume control ready before root confirmation");
    let followup = admission(&current, 2);
    store
        .admit_chat_execution_attempt(&followup)
        .expect("one exact-resume bootstrap");
    store
        .claim_chat_execution_attempt(followup.request_id, 2, resumed_fence, now())
        .expect("claim once");
    assert_eq!(
        store
            .bind_chat_execution_attempt(
                followup.request_id,
                2,
                resumed_fence,
                "resumed_turn",
                now()
            )
            .expect_err("resume still needs current root proof")
            .code(),
        "chat_unconfirmed_session"
    );
    store
        .change_chat_execution_process(
            &mutation(owner.chat_id, 2),
            resumed_fence,
            &ChatExecutionProcessChange::VendorBound { vendor_id },
        )
        .expect("matching resumed root proof");
    store
        .bind_chat_execution_attempt(followup.request_id, 2, resumed_fence, "resumed_turn", now())
        .expect("acknowledgement after proof");
}

#[test]
fn unconfirmed_codex_and_claimed_unconfirmed_claude_cannot_restart_as_new_roots() {
    for harness in [ChatHarness::Codex, ChatHarness::Claude] {
        let temp = tempfile::tempdir().expect("fixture");
        let mut store = SqliteStore::open(&temp.path().join("state.sqlite3")).expect("store");
        let owner = ready_owner(&mut store, harness);
        let vendor = (harness == ChatHarness::Claude).then(|| Uuid::new_v4().to_string());
        if harness == ChatHarness::Codex {
            assert_eq!(
                store
                    .start_chat_execution_process(&process_input(
                        &owner,
                        1,
                        Some("unowned_thread".into())
                    ))
                    .expect_err("a fresh Codex owner cannot adopt a caller's thread")
                    .code(),
                "chat_unconfirmed_session"
            );
        }
        let process = store
            .start_chat_execution_process(&process_input(&owner, 1, vendor.clone()))
            .expect("start");
        let fence = process.fence.expect("fresh fence");
        let ready = store.change_chat_execution_process(
            &mutation(owner.chat_id, 1),
            fence,
            &ChatExecutionProcessChange::Ready,
        );
        if harness == ChatHarness::Codex {
            assert_eq!(
                ready
                    .expect_err("Codex requires bound thread before Ready")
                    .code(),
                "chat_unconfirmed_session"
            );
        } else {
            ready.expect("one Claude bootstrap");
            let input = admission(&owner, 1);
            let admitted = store.admit_chat_execution_attempt(&input).expect("admit");
            store
                .claim_chat_execution_attempt(input.request_id, 1, fence, now())
                .expect("claim");
            store
                .change_chat_execution_process(
                    &mutation(owner.chat_id, 1),
                    fence,
                    &ChatExecutionProcessChange::Released {
                        cleanup_verified: true,
                        closed: false,
                    },
                )
                .expect("known cleanup without root proof");
            assert_eq!(
                store
                    .start_chat_execution_process(&process_input(&admitted.owner, 1, vendor))
                    .expect_err("previous input prevents a fresh unconfirmed root")
                    .code(),
                "chat_unconfirmed_session"
            );
        }
    }
}
