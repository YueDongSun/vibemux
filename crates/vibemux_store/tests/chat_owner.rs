//! Lifetime owner contracts through the authoritative store API.
use std::collections::BTreeSet;

use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_store::{ChatExecutionOwnerOpen, ChatOwnerChange, ChatOwnerMutation, SqliteStore};
use vibemux_types::{
    RunStatus, TaskStatus, ToolName, a2a::RunWorkspace, chat::ChatHarness,
    chat_owner::ChatOwnerPhase,
};

fn request() -> ChatExecutionOwnerOpen {
    ChatExecutionOwnerOpen {
        operation_id: Uuid::new_v4(),
        harness: ChatHarness::Codex,
        profile_sha256: "a".repeat(64),
        base_commit: "b".repeat(40),
        admitted_tools: BTreeSet::from([ToolName::ReadFiles, ToolName::EditFiles]),
        title_ref: None,
        timestamp: OffsetDateTime::from_unix_timestamp(1_793_433_600).expect("time"),
    }
}

#[test]
fn owner_ids_and_planned_receipt_survive_reopen_and_duplicate_open() {
    let temp = tempfile::tempdir().expect("temp");
    let database = temp.path().join("state.sqlite3");
    let mut store = SqliteStore::open(&database).expect("store");
    let input = request();
    let opened = store
        .open_chat_execution_owner(&input)
        .expect("allocate owner");
    assert_eq!(opened.owner.phase, ChatOwnerPhase::Provisioning);
    assert!(opened.owner.workspace.is_none());
    assert_eq!(opened.run.status(), RunStatus::Preparing);
    assert_eq!(opened.task.status(), TaskStatus::InProgress);
    let plan = RunWorkspace {
        path: format!("/owned/project/.vibemux/worktrees/{}", opened.owner.run_id),
        branch: format!("codex/run_{}", opened.owner.run_id.as_uuid().simple()),
        base_commit: input.base_commit.clone(),
        ownership_token: Uuid::new_v4().simple().to_string(),
    };
    let planned = store
        .change_chat_execution_owner(
            &ChatOwnerMutation {
                operation_id: Uuid::new_v4(),
                chat_id: opened.owner.chat_id,
                expected_version: opened.owner.version,
                timestamp: input.timestamp,
            },
            &ChatOwnerChange::Plan {
                workspace: plan.clone(),
                resource_key: "c".repeat(64),
            },
        )
        .expect("persist plan before Git");
    assert_eq!(planned.owner.workspace.as_ref(), Some(&plan));
    assert_eq!(planned.owner.phase, ChatOwnerPhase::Provisioning);
    drop(store);

    let mut store = SqliteStore::open(&database).expect("reopen");
    let repeated = store.open_chat_execution_owner(&input).expect("same Open");
    assert!(repeated.duplicate);
    assert_eq!(repeated.owner.chat_id, opened.owner.chat_id);
    assert_eq!(repeated.owner.task_id, opened.owner.task_id);
    assert_eq!(repeated.owner.run_id, opened.owner.run_id);
    assert_eq!(repeated.owner.workspace.as_ref(), Some(&plan));
    let claimed = store
        .change_chat_execution_owner(
            &ChatOwnerMutation {
                operation_id: Uuid::new_v4(),
                chat_id: opened.owner.chat_id,
                expected_version: planned.owner.version,
                timestamp: input.timestamp,
            },
            &ChatOwnerChange::ClaimCreation,
        )
        .expect("claim before Git effect");
    let ready = store
        .change_chat_execution_owner(
            &ChatOwnerMutation {
                operation_id: Uuid::new_v4(),
                chat_id: opened.owner.chat_id,
                expected_version: claimed.owner.version,
                timestamp: input.timestamp,
            },
            &ChatOwnerChange::VerifyWorkspace,
        )
        .expect("verified provisioning");
    assert_eq!(ready.owner.phase, ChatOwnerPhase::Open);
    assert_eq!(ready.run.status(), RunStatus::Preparing);
    assert_eq!(ready.owner.workspace.as_ref(), Some(&plan));
    let before_projection = store.events().expect("events before metadata read");
    let detail = store
        .frontend_task(ready.owner.task_id)
        .expect("task read")
        .expect("task");
    let run = detail
        .runs
        .iter()
        .find(|run| run.run_id == ready.owner.run_id)
        .expect("owner Run metadata");
    assert_eq!(run.worktree_path.as_deref(), Some(plan.path.as_str()));
    assert_eq!(run.branch.as_deref(), Some(plan.branch.as_str()));
    assert!(!run.worktree_path_truncated);
    assert_eq!(
        store.events().expect("metadata read is inert"),
        before_projection
    );
}

#[test]
fn legacy_chat_controls_and_recovery_cannot_mutate_an_owner() {
    use vibemux_store::{ChatMutation, ChatTurnAdmission};
    use vibemux_types::chat::ContentRef;
    let temp = tempfile::tempdir().expect("temp");
    let mut store = SqliteStore::open(&temp.path().join("state.sqlite3")).expect("store");
    let input = request();
    let opened = store.open_chat_execution_owner(&input).expect("open");
    let mutation = ChatMutation {
        operation_id: Uuid::new_v4(),
        chat_id: opened.owner.chat_id,
        expected_generation: 1,
        timestamp: input.timestamp,
    };
    assert_eq!(
        store
            .claim_chat_owner(opened.owner.chat_id, 1, None, input.timestamp)
            .expect_err("legacy claim")
            .code(),
        "chat_owner_control_required"
    );
    assert_eq!(
        store
            .close_chat(&mutation)
            .expect_err("legacy close")
            .code(),
        "chat_owner_control_required"
    );
    assert_eq!(
        store
            .request_chat_close(&mutation)
            .expect_err("legacy close reservation")
            .code(),
        "chat_owner_control_required"
    );
    assert_eq!(
        store
            .resume_chat(&mutation)
            .expect_err("legacy resume")
            .code(),
        "chat_owner_control_required"
    );
    assert_eq!(
        store
            .archive_chat(&mutation)
            .expect_err("legacy archive")
            .code(),
        "chat_owner_control_required"
    );
    let content = ContentRef {
        blob_id: Uuid::new_v4(),
        sha256: "d".repeat(64),
        byte_count: 10,
    };
    assert_eq!(
        store
            .rename_chat(&mutation, content.clone())
            .expect_err("legacy rename")
            .code(),
        "chat_owner_control_required"
    );
    assert_eq!(
        store
            .purge_chat_content(&mutation)
            .expect_err("legacy purge")
            .code(),
        "chat_owner_control_required"
    );
    assert_eq!(
        store
            .admit_chat_turn(&ChatTurnAdmission {
                chat_id: opened.owner.chat_id,
                request_id: Uuid::new_v4(),
                expected_generation: 1,
                base_commit: input.base_commit,
                prompt_ref: content,
                timestamp: input.timestamp
            })
            .expect_err("legacy send")
            .code(),
        "chat_owner_control_required"
    );
    assert!(
        store
            .list_chat_sessions(None, 16, true)
            .expect("legacy list")
            .is_empty()
    );
    assert!(
        store
            .recover_chats(input.timestamp)
            .expect("legacy recovery")
            .sessions
            .is_empty()
    );
    assert_eq!(
        store
            .chat_execution_owner(opened.owner.chat_id)
            .expect("owner")
            .expect("known"),
        opened.owner
    );
}

#[test]
fn generic_projection_and_a2a_guards_cover_an_owner_before_attempt_zero() {
    use vibemux_events::{ActorName, EventDraft, EventPayload, EventType};
    use vibemux_types::{EventId, Run, RunSpec, a2a::A2aRunStart};
    let temp = tempfile::tempdir().expect("temp");
    let mut store = SqliteStore::open(&temp.path().join("state.sqlite3")).expect("store");
    let input = request();
    let opened = store.open_chat_execution_owner(&input).expect("open");
    let event = || EventDraft {
        event_id: EventId::new(),
        event_type: EventType::new("task_updated").expect("event"),
        project_id: opened.owner.project_id,
        task_id: None,
        run_id: None,
        causation_id: None,
        actor: ActorName::new("test").expect("actor"),
        timestamp: input.timestamp,
        idempotency_key: Some(Uuid::new_v4().to_string()),
        payload: EventPayload::new(serde_json::json!({"fixture":true})).expect("payload"),
    };
    assert_eq!(
        store
            .commit_task(&opened.task, event())
            .expect_err("task ownership")
            .code(),
        "store_chat_bound_projection"
    );
    assert_eq!(
        store
            .commit_run(&opened.run, event())
            .expect_err("run ownership")
            .code(),
        "store_chat_bound_projection"
    );
    let foreign = Run::new(RunSpec {
        project_id: opened.owner.project_id,
        task_id: opened.owner.task_id,
        harness: "codex".into(),
        role: "worker".into(),
        protocol: "a2a".into(),
        base_commit: input.base_commit.clone(),
    })
    .expect("foreign run");
    assert_eq!(
        store
            .commit_run(&foreign, event())
            .expect_err("new run under owner task")
            .code(),
        "store_chat_bound_projection"
    );
    let start = A2aRunStart {
        task: opened.task,
        run: foreign,
        peer_id: "fixture".into(),
        external_task_id: "foreign".into(),
        transport: "http_json".into(),
        protocol_version: "1.0".into(),
        workspace: RunWorkspace {
            path: "/synthetic/work".into(),
            branch: "synthetic".into(),
            base_commit: input.base_commit,
            ownership_token: "synthetic".into(),
        },
        timestamp: input.timestamp,
        idempotency_key: "owner_a2a_guard".into(),
    };
    start
        .validate()
        .expect("valid A2A request reaches ownership guard");
    assert_eq!(
        store
            .start_a2a_run(start)
            .expect_err("A2A ownership")
            .code(),
        "store_a2a_binding_conflict"
    );
}

#[test]
fn schema_ten_upgrade_and_failed_owner_migration_preserve_read_only_chats() {
    use vibemux_store::ChatOpen;
    let temp = tempfile::tempdir().expect("temp");
    let database = temp.path().join("state.sqlite3");
    let mut store = SqliteStore::open(&database).expect("store");
    let input = request();
    let session = store
        .open_chat(&ChatOpen {
            operation_id: input.operation_id,
            harness: input.harness,
            profile_sha256: input.profile_sha256,
            resource_key: "d".repeat(64),
            base_commit: input.base_commit,
            title_ref: None,
            timestamp: input.timestamp,
        })
        .expect("historical read-only chat")
        .session;
    let events = store.events().expect("history");
    drop(store);
    let raw = rusqlite::Connection::open(&database).expect("owned migration fixture");
    raw.execute_batch("DROP TABLE workspace_cleanup_commands; DROP TABLE workspace_cleanups; DROP TABLE chat_approvals; DROP TABLE chat_execution_records; DROP TABLE chat_execution_attempts; DROP TABLE chat_execution_owners; DELETE FROM schema_migrations WHERE version>=11; CREATE INDEX chat_owner_workspace_reservation ON chat_sessions(resource_key);").expect("schema10 and late migration fault");
    assert_eq!(
        raw.query_row("SELECT MAX(version) FROM schema_migrations", [], |row| row
            .get::<_, u32>(
            0
        ))
        .expect("version"),
        10
    );
    assert!(SqliteStore::open(&database).is_err());
    assert_eq!(
        raw.query_row("SELECT MAX(version) FROM schema_migrations", [], |row| row
            .get::<_, u32>(
            0
        ))
        .expect("rollback version"),
        10
    );
    assert!(!raw.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='chat_execution_owners')", [], |row|row.get::<_,bool>(0)).expect("rolled back table"));
    raw.execute_batch("DROP INDEX chat_owner_workspace_reservation;")
        .expect("remove owned injected fault");
    drop(raw);
    let store = SqliteStore::open(&database).expect("forward migration");
    assert_eq!(
        store
            .chat_session(session.chat_id)
            .expect("session")
            .expect("retained"),
        session
    );
    assert_eq!(store.events().expect("retained history"), events);
    assert!(
        store
            .list_chat_execution_owners(None, 16, true)
            .expect("new owner table")
            .is_empty()
    );
}

#[test]
fn an_unknown_creation_claim_cannot_replay_or_release_the_owner() {
    use vibemux_types::chat_owner::{ChatOwnerFailure, ChatWorkspaceEffect};
    let temp = tempfile::tempdir().expect("temp");
    let database = temp.path().join("state.sqlite3");
    let mut store = SqliteStore::open(&database).expect("store");
    let input = request();
    let opened = store.open_chat_execution_owner(&input).expect("open");
    let mutation = |version| ChatOwnerMutation {
        operation_id: Uuid::new_v4(),
        chat_id: opened.owner.chat_id,
        expected_version: version,
        timestamp: input.timestamp,
    };
    let plan = RunWorkspace {
        path: format!("/owned/{}", opened.owner.run_id),
        branch: format!("codex/run_{}", opened.owner.run_id.as_uuid().simple()),
        base_commit: input.base_commit.clone(),
        ownership_token: Uuid::new_v4().simple().to_string(),
    };
    let planned = store
        .change_chat_execution_owner(
            &mutation(1),
            &ChatOwnerChange::Plan {
                workspace: plan.clone(),
                resource_key: "c".repeat(64),
            },
        )
        .expect("plan");
    let claim = mutation(planned.owner.version);
    let claimed = store
        .change_chat_execution_owner(&claim, &ChatOwnerChange::ClaimCreation)
        .expect("claim once");
    let duplicate = store
        .change_chat_execution_owner(&claim, &ChatOwnerChange::ClaimCreation)
        .expect("same claim receipt");
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.owner, claimed.owner);
    let events_before = store.events().expect("events");
    assert!(
        store
            .change_chat_execution_owner(
                &mutation(claimed.owner.version),
                &ChatOwnerChange::ClaimCreation
            )
            .is_err()
    );
    assert!(
        store
            .change_chat_execution_owner(&mutation(claimed.owner.version), &ChatOwnerChange::Stop)
            .is_err()
    );
    assert_eq!(store.events().expect("no rejected effects"), events_before);
    drop(store);
    let mut store = SqliteStore::open(&database).expect("restart");
    let unknown = store
        .change_chat_execution_owner(
            &mutation(claimed.owner.version),
            &ChatOwnerChange::WorkspaceFailed {
                code: ChatOwnerFailure::WorkspaceUnverified,
                cleanup_verified: false,
            },
        )
        .expect("record unknown outcome");
    assert_eq!(
        unknown.owner.workspace_effect,
        Some(ChatWorkspaceEffect::Claimed)
    );
    assert_eq!(unknown.run.status(), RunStatus::Preparing);
    assert_eq!(unknown.task.status(), TaskStatus::Blocked);
    assert!(
        store
            .change_chat_execution_owner(&mutation(unknown.owner.version), &ChatOwnerChange::Stop)
            .is_err()
    );
    let reconciled = store
        .change_chat_execution_owner(
            &mutation(unknown.owner.version),
            &ChatOwnerChange::VerifyWorkspace,
        )
        .expect("daemon verified exact persisted receipt");
    assert_eq!(reconciled.owner.workspace.as_ref(), Some(&plan));
    let stop = mutation(reconciled.owner.version);
    let stopped = store
        .change_chat_execution_owner(&stop, &ChatOwnerChange::Stop)
        .expect("explicit stop after reconciliation");
    assert_eq!(stopped.owner.phase, ChatOwnerPhase::Finalized);
    assert_eq!(stopped.run.status(), RunStatus::Failed);
    assert_eq!(stopped.task.status(), TaskStatus::Cancelled);
    assert_eq!(stopped.owner.workspace.as_ref(), Some(&plan));
    assert!(
        store
            .change_chat_execution_owner(&stop, &ChatOwnerChange::Stop)
            .expect("same finalization")
            .duplicate
    );
    assert_eq!(
        store
            .open_chat_execution_owner(&input)
            .expect("Open replay cannot reopen owner")
            .owner
            .phase,
        ChatOwnerPhase::Finalized
    );
}

#[test]
fn idle_owner_reservations_and_failed_transactions_keep_the_original_binding() {
    use vibemux_harness::HarnessDetection;
    use vibemux_store::ChatOpen;
    let temp = tempfile::tempdir().expect("temp");
    let database = temp.path().join("state.sqlite3");
    let mut store = SqliteStore::open(&database).expect("store");
    let input = request();
    let opened = store.open_chat_execution_owner(&input).expect("open");
    let mutation = |version| ChatOwnerMutation {
        operation_id: Uuid::new_v4(),
        chat_id: opened.owner.chat_id,
        expected_version: version,
        timestamp: input.timestamp,
    };
    let plan = RunWorkspace {
        path: format!("/owned/{}", opened.owner.run_id),
        branch: format!("codex/run_{}", opened.owner.run_id.as_uuid().simple()),
        base_commit: input.base_commit.clone(),
        ownership_token: Uuid::new_v4().simple().to_string(),
    };
    let action = ChatOwnerChange::Plan {
        workspace: plan.clone(),
        resource_key: "c".repeat(64),
    };
    let raw = rusqlite::Connection::open(&database).expect("fault fixture");
    raw.execute_batch("CREATE TRIGGER reject_owner_event BEFORE INSERT ON events BEGIN SELECT RAISE(ABORT,'synthetic_owner_fault'); END;").expect("inject fault");
    let before = store.events().expect("before");
    assert!(
        store
            .change_chat_execution_owner(&mutation(1), &action)
            .is_err()
    );
    assert_eq!(
        store
            .chat_execution_owner(opened.owner.chat_id)
            .expect("owner")
            .expect("retained"),
        opened.owner
    );
    assert_eq!(store.events().expect("rolled back event"), before);
    raw.execute_batch("DROP TRIGGER reject_owner_event;")
        .expect("remove fault");
    drop(raw);
    let planned = store
        .change_chat_execution_owner(&mutation(1), &action)
        .expect("plan");
    let claimed = store
        .change_chat_execution_owner(
            &mutation(planned.owner.version),
            &ChatOwnerChange::ClaimCreation,
        )
        .expect("claim");
    let ready = store
        .change_chat_execution_owner(
            &mutation(claimed.owner.version),
            &ChatOwnerChange::VerifyWorkspace,
        )
        .expect("ready");
    store
        .commit_harness_snapshot(
            &std::collections::BTreeMap::from([(
                "codex".into(),
                HarnessDetection {
                    detected: true,
                    path: None,
                    launcher: None,
                    version: Some("fixture".into()),
                },
            )]),
            "2026-11-01T00:00:00Z",
            "owner_test_detection",
        )
        .expect("detection");
    let legacy = store
        .open_chat(&ChatOpen {
            operation_id: Uuid::new_v4(),
            harness: ChatHarness::Codex,
            profile_sha256: "a".repeat(64),
            resource_key: "c".repeat(64),
            base_commit: input.base_commit.clone(),
            title_ref: None,
            timestamp: input.timestamp,
        })
        .expect("legacy metadata")
        .session;
    assert_eq!(
        store
            .claim_chat_owner(legacy.chat_id, 1, None, input.timestamp)
            .expect_err("idle owner holds resource")
            .code(),
        "chat_busy"
    );
    let mut changed = input.clone();
    changed.admitted_tools.insert(ToolName::RunDevTests);
    assert_eq!(
        store
            .open_chat_execution_owner(&changed)
            .expect_err("same ID cannot widen grants")
            .code(),
        "chat_conflict"
    );
    assert_eq!(
        store
            .chat_execution_owner(opened.owner.chat_id)
            .expect("retained owner")
            .expect("known"),
        ready.owner
    );
}

#[test]
fn concurrent_duplicate_owner_opens_commit_one_canonical_pair() {
    let temp = tempfile::tempdir().expect("temp");
    let database = temp.path().join("state.sqlite3");
    drop(SqliteStore::open(&database).expect("schema"));
    let input = request();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
    let threads = (0..4)
        .map(|_| {
            let database = database.clone();
            let input = input.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut store = SqliteStore::open(&database).expect("connection");
                barrier.wait();
                store.open_chat_execution_owner(&input).expect("same Open")
            })
        })
        .collect::<Vec<_>>();
    let results = threads
        .into_iter()
        .map(|thread| thread.join().expect("thread"))
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| !result.duplicate).count(), 1);
    assert!(
        results
            .iter()
            .all(|result| result.owner.task_id == results[0].owner.task_id
                && result.owner.run_id == results[0].owner.run_id
                && result.owner.chat_id == results[0].owner.chat_id)
    );
    let store = SqliteStore::open(&database).expect("inspect");
    assert_eq!(
        store
            .list_chat_execution_owners(None, 16, true)
            .expect("owners")
            .len(),
        1
    );
}

#[test]
fn known_unverified_process_cleanup_survives_restart_and_fences_effects() {
    use vibemux_types::chat_owner::ChatOwnerFailure;

    let temp = tempfile::tempdir().expect("temporary store");
    let database = temp.path().join("state.sqlite3");
    let input = request();
    let mut store = SqliteStore::open(&database).expect("store");
    let opened = store.open_chat_execution_owner(&input).expect("open");
    let mutation = |version| ChatOwnerMutation {
        operation_id: Uuid::new_v4(),
        chat_id: opened.owner.chat_id,
        expected_version: version,
        timestamp: input.timestamp,
    };
    let failed = store
        .change_chat_execution_owner(
            &mutation(opened.owner.version),
            &ChatOwnerChange::WorkspaceFailed {
                code: ChatOwnerFailure::WorkspaceCleanupUnverified,
                cleanup_verified: false,
            },
        )
        .expect("persist known cleanup failure");
    drop(store);
    let mut store = SqliteStore::open(&database).expect("restart");
    let reconciled = store
        .change_chat_execution_owner(&mutation(failed.owner.version), &ChatOwnerChange::Reconcile)
        .expect("metadata reconciliation retains the cleanup error");
    assert_eq!(
        reconciled.owner.error_code,
        Some(ChatOwnerFailure::WorkspaceCleanupUnverified)
    );
    let plan = RunWorkspace {
        path: format!("/owned/{}", opened.owner.run_id),
        branch: format!("codex/run_{}", opened.owner.run_id.as_uuid().simple()),
        base_commit: input.base_commit,
        ownership_token: Uuid::new_v4().simple().to_string(),
    };
    let events = store.events().expect("events before refusal");
    for change in [
        ChatOwnerChange::Plan {
            workspace: plan,
            resource_key: "c".repeat(64),
        },
        ChatOwnerChange::ClaimCreation,
        ChatOwnerChange::VerifyWorkspace,
        ChatOwnerChange::Stop,
        ChatOwnerChange::WorkspaceFailed {
            code: ChatOwnerFailure::WorkspaceUnverified,
            cleanup_verified: true,
        },
    ] {
        assert_eq!(
            store
                .change_chat_execution_owner(&mutation(reconciled.owner.version), &change)
                .expect_err("known live-process uncertainty fences effects")
                .code(),
            "chat_busy"
        );
    }
    assert_eq!(store.events().expect("no rejected effects"), events);
    assert_eq!(
        store
            .chat_execution_owner(opened.owner.chat_id)
            .expect("owner")
            .expect("retained"),
        reconciled.owner
    );
}

#[test]
fn run_owner_lookup_round_trips_the_full_commit_and_leaves_state_and_events_unchanged() {
    let temp = tempfile::tempdir().expect("temporary owner store");
    let database = temp.path().join("state.sqlite3");
    let input = request();
    let mut store = SqliteStore::open(&database).expect("store");
    let opened = store.open_chat_execution_owner(&input).expect("owner");
    let workspace = RunWorkspace {
        path: format!("/owned/project/.vibemux/worktrees/{}", opened.owner.run_id),
        branch: format!("codex/run_{}", opened.owner.run_id.as_uuid().simple()),
        base_commit: input.base_commit.clone(),
        ownership_token: Uuid::new_v4().simple().to_string(),
    };
    store
        .change_chat_execution_owner(
            &ChatOwnerMutation {
                operation_id: Uuid::new_v4(),
                chat_id: opened.owner.chat_id,
                expected_version: opened.owner.version,
                timestamp: input.timestamp,
            },
            &ChatOwnerChange::Plan {
                workspace: workspace.clone(),
                resource_key: "c".repeat(64),
            },
        )
        .expect("durable workspace plan");
    drop(store);
    let store = SqliteStore::open(&database).expect("reopened durable store");
    let before = store
        .chat_execution_owner_state(opened.owner.chat_id)
        .expect("validated owner")
        .expect("known owner");
    let events = store.events().expect("before read");
    let found = store
        .chat_execution_owner_for_run(opened.owner.run_id)
        .expect("indexed lookup")
        .expect("exact owner");
    assert_eq!(found.owner, before.owner);
    assert_eq!(found.session, before.session);
    assert_eq!(found.owner.workspace.as_ref(), Some(&workspace));
    assert_eq!(
        serde_json::to_value(&found.task).expect("task"),
        serde_json::to_value(&before.task).expect("original task")
    );
    assert_eq!(
        serde_json::to_value(&found.run).expect("run"),
        serde_json::to_value(&before.run).expect("original run")
    );
    assert_eq!(found.sequence, before.sequence);
    assert_eq!(found.process_claimed, before.process_claimed);
    assert!(!found.duplicate);
    assert!(found.event.is_none());
    assert!(
        store
            .chat_execution_owner_for_run(vibemux_types::RunId::new())
            .expect("unknown run read")
            .is_none()
    );
    let after = store
        .chat_execution_owner_state(opened.owner.chat_id)
        .expect("after read")
        .expect("retained");
    assert_eq!(after.owner, before.owner);
    assert_eq!(after.session, before.session);
    assert_eq!(
        serde_json::to_value(&after.task).expect("task after"),
        serde_json::to_value(&before.task).expect("task before")
    );
    assert_eq!(
        serde_json::to_value(&after.run).expect("run after"),
        serde_json::to_value(&before.run).expect("run before")
    );
    assert_eq!(after.sequence, before.sequence);
    assert_eq!(after.process_claimed, before.process_claimed);
    assert_eq!(store.events().expect("read creates no events"), events);
}

#[test]
fn two_distinct_run_lookups_never_cross_map_their_owner_bindings() {
    let temp = tempfile::tempdir().expect("temporary owner store");
    let mut store = SqliteStore::open(&temp.path().join("state.sqlite3")).expect("store");
    let first = store
        .open_chat_execution_owner(&request())
        .expect("first owner");
    let second = store
        .open_chat_execution_owner(&request())
        .expect("second owner");
    assert_ne!(first.owner.run_id, second.owner.run_id);
    assert_ne!(first.owner.chat_id, second.owner.chat_id);
    let events = store.events().expect("events");
    for expected in [&first, &second] {
        let found = store
            .chat_execution_owner_for_run(expected.owner.run_id)
            .expect("lookup")
            .expect("owner");
        assert_eq!(found.owner, expected.owner);
        assert_eq!(found.session, expected.session);
        assert_eq!(found.task.task_id(), expected.owner.task_id);
        assert_eq!(found.run.run_id(), expected.owner.run_id);
        assert_eq!(found.run.task_id(), expected.owner.task_id);
        assert_eq!(found.sequence, expected.sequence);
    }
    assert_eq!(store.events().expect("no read effects"), events);
}

#[test]
fn run_lookup_uses_the_same_full_loader_for_corrupt_projection_binding_and_identity() {
    for variant in 0..3 {
        let temp = tempfile::tempdir().expect("owned corruption fixture");
        let database = temp.path().join("state.sqlite3");
        let mut store = SqliteStore::open(&database).expect("store");
        let opened = store.open_chat_execution_owner(&request()).expect("owner");
        let raw = rusqlite::Connection::open(&database).expect("owned fault seam");
        let column = if variant == 1 {
            "binding_json"
        } else {
            "record_json"
        };
        let encoded: String = raw
            .query_row(
                &format!("SELECT {column} FROM chat_execution_owners WHERE chat_id=?"),
                [opened.owner.chat_id.to_string()],
                |row| row.get(0),
            )
            .expect("existing fixture metadata");
        let mut changed: serde_json::Value = serde_json::from_str(&encoded).expect("fixture JSON");
        match variant {
            0 => changed["version"] = serde_json::json!(opened.owner.version + 1),
            1 => changed["base_commit"] = serde_json::json!("c".repeat(40)),
            _ => changed["run_id"] = serde_json::json!(vibemux_types::RunId::new()),
        }
        let changed = serde_json::to_string(&changed).expect("corrupt fixture");
        raw.execute(
            &format!("UPDATE chat_execution_owners SET {column}=? WHERE chat_id=?"),
            rusqlite::params![changed, opened.owner.chat_id.to_string()],
        )
        .expect("inject owned projection fault");
        let events = store.events().expect("events before refused read");
        let full_error = store
            .chat_execution_owner_state(opened.owner.chat_id)
            .expect_err("full validation refuses corruption");
        let indexed_error = store
            .chat_execution_owner_for_run(opened.owner.run_id)
            .expect_err("run lookup cannot bypass full validation");
        assert_eq!(indexed_error.code(), full_error.code());
        assert_eq!(store.events().expect("refusal creates no events"), events);
        let retained: String = raw
            .query_row(
                &format!("SELECT {column} FROM chat_execution_owners WHERE chat_id=?"),
                [opened.owner.chat_id.to_string()],
                |row| row.get(0),
            )
            .expect("corruption is not silently repaired");
        assert_eq!(retained, changed);
    }
}

#[test]
fn run_lookup_rejects_an_invalid_indexed_chat_identifier_without_repairing_it() {
    let temp = tempfile::tempdir().expect("owned identity fixture");
    let database = temp.path().join("state.sqlite3");
    let mut store = SqliteStore::open(&database).expect("store");
    let opened = store.open_chat_execution_owner(&request()).expect("owner");
    let raw = rusqlite::Connection::open(&database).expect("owned corruption seam");
    raw.execute_batch("PRAGMA foreign_keys=OFF;")
        .expect("fixture-only corruption");
    raw.execute(
        "UPDATE chat_execution_owners SET chat_id=? WHERE run_id=?",
        rusqlite::params!["not_a_chat_identifier", opened.owner.run_id.to_string()],
    )
    .expect("invalid scalar identity");
    let events = store.events().expect("events before read");
    assert_eq!(
        store
            .chat_execution_owner_for_run(opened.owner.run_id)
            .expect_err("typed identifier refusal")
            .code(),
        "chat_projection_mismatch"
    );
    assert_eq!(store.events().expect("no read effects"), events);
    let retained: String = raw
        .query_row(
            "SELECT chat_id FROM chat_execution_owners WHERE run_id=?",
            [opened.owner.run_id.to_string()],
            |row| row.get(0),
        )
        .expect("retained faulty fixture");
    assert_eq!(retained, "not_a_chat_identifier");
}
