//! Durable cleanup metadata through the single-writer store boundary. No Git I/O.
use std::collections::BTreeSet;

use time::{Duration, OffsetDateTime};
use uuid::Uuid;
use vibemux_store::{
    ChatExecutionOwnerOpen, ChatOwnerChange, ChatOwnerMutation, SqliteStore,
    WorkspaceCleanupCommit, WorkspaceCleanupConfirmation, WorkspaceCleanupMutation,
    WorkspaceCleanupPlan, WorkspaceCleanupSettlement,
};
use vibemux_types::{
    chat::ChatHarness,
    workspace_cleanup::{
        CleanupCommandProfile, CleanupContainmentKind, CleanupObjectIdentity, CleanupProcessResult,
        CleanupQuiescence, ExecutorBinding, WorkspaceCleanup, WorkspaceCleanupBinding,
        WorkspaceCleanupOutcome, WorkspaceCleanupPhase,
    },
};

fn plan(store: &mut SqliteStore) -> WorkspaceCleanupPlan {
    let now = OffsetDateTime::from_unix_timestamp(1_793_433_600).expect("time");
    let mut owner = store
        .open_chat_execution_owner(&ChatExecutionOwnerOpen {
            operation_id: Uuid::new_v4(),
            harness: ChatHarness::Codex,
            profile_sha256: "a".repeat(64),
            base_commit: "b".repeat(40),
            admitted_tools: BTreeSet::new(),
            title_ref: None,
            timestamp: now,
        })
        .expect("owner");
    let workspace = vibemux_types::a2a::RunWorkspace {
        path: format!("/owned/project/.vibemux/worktrees/{}", owner.owner.run_id),
        branch: format!("codex/run_{}", owner.owner.run_id.as_uuid().simple()),
        base_commit: "b".repeat(40),
        ownership_token: Uuid::new_v4().simple().to_string(),
    };
    for change in [
        ChatOwnerChange::Plan {
            workspace: workspace.clone(),
            resource_key: "c".repeat(64),
        },
        ChatOwnerChange::ClaimCreation,
        ChatOwnerChange::VerifyWorkspace,
        ChatOwnerChange::Stop,
    ] {
        owner = store
            .change_chat_execution_owner(
                &ChatOwnerMutation {
                    operation_id: Uuid::new_v4(),
                    chat_id: owner.owner.chat_id,
                    expected_version: owner.owner.version,
                    timestamp: now,
                },
                &change,
            )
            .expect("finalized verified owner");
    }
    let identity = CleanupObjectIdentity::WindowsFileId128V1 {
        volume_serial_number: u64::MAX,
        file_id: [13; 16],
    };
    WorkspaceCleanupPlan {
        operation_id: Uuid::new_v4(),
        daemon_incarnation: Uuid::new_v4(),
        binding: WorkspaceCleanupBinding {
            project_id: owner.owner.project_id,
            chat_id: owner.owner.chat_id,
            task_id: owner.owner.task_id,
            run_id: owner.owner.run_id,
            owner_version: owner.owner.version,
            session_generation: owner.session.generation,
            session_version: owner.session.version,
            workspace,
            resource_key: "c".repeat(64),
            repository_root: "/owned/project".into(),
            common_git_dir: "/owned/project/.git".into(),
            repository_identity: identity.clone(),
            common_directory_identity: identity.clone(),
            worktree_identity: identity,
            head_commit: "d".repeat(40),
            terminal_plugin_id: "observer".into(),
            session_id: "session".into(),
            instance_id: "instance".into(),
        },
        timestamp: now + Duration::seconds(1),
    }
}

#[test]
fn previews_are_inert_and_only_fresh_confirmation_returns_execution_authority() {
    let temp = tempfile::tempdir().expect("temp");
    let database = temp.path().join("state.sqlite3");
    let mut store = SqliteStore::open(&database).expect("store");
    let input = plan(&mut store);
    let historical = store
        .chat_execution_owner_state(input.binding.chat_id)
        .expect("read")
        .expect("owner");
    let historical = serde_json::to_value((
        &historical.owner,
        &historical.session,
        &historical.task,
        &historical.run,
    ))
    .expect("snapshot");
    let preview = store.plan_workspace_cleanup(&input).expect("preview");
    assert_eq!(preview.cleanup.phase, WorkspaceCleanupPhase::Planned);
    assert!(preview.execution_claim.is_none());
    let mut retry = input.clone();
    retry.timestamp += Duration::seconds(1);
    let duplicate = store
        .plan_workspace_cleanup(&retry)
        .expect("Plan replay ignores observation time");
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.cleanup, preview.cleanup);
    let confirmation = WorkspaceCleanupConfirmation {
        operation_id: Uuid::new_v4(),
        cleanup_id: preview.cleanup.id,
        expected_version: preview.cleanup.version,
        daemon_incarnation: input.daemon_incarnation,
        timestamp: input.timestamp + Duration::seconds(2),
    };
    let claimed = store
        .confirm_workspace_cleanup(&confirmation)
        .expect("confirm");
    assert_eq!(claimed.cleanup.phase, WorkspaceCleanupPhase::Claimed);
    assert!(claimed.execution_claim.is_some());
    let repeated = store
        .confirm_workspace_cleanup(&confirmation)
        .expect("Confirm replay");
    assert!(repeated.duplicate);
    assert!(repeated.execution_claim.is_none());
    let current = store
        .chat_execution_owner_state(input.binding.chat_id)
        .expect("read")
        .expect("owner");
    assert_eq!(
        serde_json::to_value((
            &current.owner,
            &current.session,
            &current.task,
            &current.run
        ))
        .expect("snapshot"),
        historical
    );
    let events = store.events().expect("events");
    drop(store);
    let store = SqliteStore::open(&database).expect("reopen");
    let read = store
        .workspace_cleanup(preview.cleanup.id)
        .expect("read")
        .expect("record");
    assert_eq!(read.cleanup, claimed.cleanup);
    assert!(read.execution_claim.is_none());
    assert_eq!(store.events().expect("read leaves audit unchanged"), events);
}

fn confirm(store: &mut SqliteStore, input: &WorkspaceCleanupPlan) -> WorkspaceCleanupCommit {
    let preview = store.plan_workspace_cleanup(input).expect("plan");
    store
        .confirm_workspace_cleanup(&confirmation(&preview.cleanup, input.daemon_incarnation))
        .expect("claim")
}
fn confirmation(record: &WorkspaceCleanup, incarnation: Uuid) -> WorkspaceCleanupConfirmation {
    WorkspaceCleanupConfirmation {
        operation_id: Uuid::new_v4(),
        cleanup_id: record.id,
        expected_version: record.version,
        daemon_incarnation: incarnation,
        timestamp: record.updated_at + Duration::seconds(1),
    }
}
fn mutation(record: &WorkspaceCleanup) -> WorkspaceCleanupMutation {
    WorkspaceCleanupMutation {
        operation_id: Uuid::new_v4(),
        cleanup_id: record.id,
        expected_version: record.version,
        timestamp: record.updated_at + Duration::seconds(1),
    }
}
fn executor(incarnation: Uuid, nonce: Uuid) -> ExecutorBinding {
    let identity = CleanupObjectIdentity::WindowsFileId128V1 {
        volume_serial_number: 3,
        file_id: [7; 16],
    };
    ExecutorBinding {
        executor_nonce: nonce,
        daemon_incarnation: incarnation,
        process_id: 42,
        observed_start_seconds: 1_793_433_600,
        expected_executable: "/owned/bin/trampoline.exe".into(),
        observed_executable: "/owned/bin/trampoline.exe".into(),
        consumed_executable_identity: identity.clone(),
        observed_path_identity: identity.clone(),
        program_executable: "/owned/bin/git.exe".into(),
        program_identity: identity,
        command_profile: CleanupCommandProfile::GitWorktreeRemoveV1,
        containment_kind: CleanupContainmentKind::WindowsJobV1,
    }
}
fn joined(nonce: Uuid) -> CleanupQuiescence {
    CleanupQuiescence::JoinedProcess {
        executor_nonce: nonce,
        termination_succeeded: true,
        root_reaped: true,
        tree_empty: true,
        stdout_drained: true,
        stderr_drained: true,
    }
}
fn result(nonce: Uuid, attempted: bool) -> CleanupProcessResult {
    CleanupProcessResult {
        executor_nonce: nonce,
        exit_code: Some(0),
        forced_termination: false,
        timed_out: false,
        release_attempted: attempted,
    }
}

#[test]
fn expiry_incarnation_and_competing_previews_are_checked_atomically() {
    let temp = tempfile::tempdir().expect("temp");
    let mut store = SqliteStore::open(&temp.path().join("state.sqlite3")).expect("store");
    let input = plan(&mut store);
    let first = store.plan_workspace_cleanup(&input).expect("first preview");
    let mut second_input = input.clone();
    second_input.operation_id = Uuid::new_v4();
    let second = store
        .plan_workspace_cleanup(&second_input)
        .expect("preview does not reserve");
    let events = store.events().expect("events");
    let mut expired = confirmation(&first.cleanup, input.daemon_incarnation);
    expired.timestamp = first.cleanup.expires_at;
    assert_eq!(
        store
            .confirm_workspace_cleanup(&expired)
            .expect_err("expiry")
            .code(),
        "workspace_cleanup_stale_plan"
    );
    let restarted = confirmation(&first.cleanup, Uuid::new_v4());
    assert_eq!(
        store
            .confirm_workspace_cleanup(&restarted)
            .expect_err("restart")
            .code(),
        "workspace_cleanup_stale_plan"
    );
    assert_eq!(store.events().expect("no failed effects"), events);
    store
        .confirm_workspace_cleanup(&confirmation(&first.cleanup, input.daemon_incarnation))
        .expect("first wins");
    let events = store.events().expect("claimed events");
    assert_eq!(
        store
            .confirm_workspace_cleanup(&confirmation(&second.cleanup, input.daemon_incarnation))
            .expect_err("reserved")
            .code(),
        "workspace_cleanup_busy"
    );
    assert_eq!(store.events().expect("loser rollback"), events);
    let mut collision = input.clone();
    collision.binding.head_commit = "e".repeat(40);
    assert_eq!(
        store
            .plan_workspace_cleanup(&collision)
            .expect_err("same operation different binding")
            .code(),
        "chat_conflict"
    );
}

#[test]
fn completion_survives_lost_registration_and_revokes_late_registration_authority() {
    let temp = tempfile::tempdir().expect("temp");
    let mut store = SqliteStore::open(&temp.path().join("state.sqlite3")).expect("store");
    let input = plan(&mut store);
    let mut claimed = confirm(&mut store, &input);
    let nonce = Uuid::new_v4();
    let executor = executor(input.daemon_incarnation, nonce);
    let (registration, completion) = claimed
        .execution_claim
        .take()
        .expect("claim")
        .begin_spawn(nonce)
        .expect("split before spawn");
    let quiet = store
        .record_workspace_cleanup_joined(
            &mutation(&claimed.cleanup),
            completion,
            &executor,
            Some(&result(nonce, false)),
            &joined(nonce),
        )
        .expect("atomic registration and joined proof");
    assert_eq!(quiet.cleanup.executor, Some(executor.clone()));
    assert!(
        store
            .record_workspace_cleanup_executor(&mutation(&quiet.cleanup), registration, &executor)
            .is_err()
    );
    let refused = store
        .settle_workspace_cleanup(
            &mutation(&quiet.cleanup),
            &WorkspaceCleanupSettlement::Refused {
                error_code: "registration_ack_lost".into(),
            },
        )
        .expect("safe no-Go refusal");
    assert_eq!(refused.cleanup.phase, WorkspaceCleanupPhase::Refused);
}

#[test]
fn a_joined_completion_retires_both_pending_arm_and_pending_release_deliveries() {
    for arm_first in [false, true] {
        let temp = tempfile::tempdir().expect("temp");
        let mut store = SqliteStore::open(&temp.path().join("state.sqlite3")).expect("store");
        let input = plan(&mut store);
        let mut claimed = confirm(&mut store, &input);
        let nonce = Uuid::new_v4();
        let executor = executor(input.daemon_incarnation, nonce);
        let (registration, completion) = claimed
            .execution_claim
            .take()
            .expect("claim")
            .begin_spawn(nonce)
            .expect("split");
        let registered = store
            .record_workspace_cleanup_executor(&mutation(&claimed.cleanup), registration, &executor)
            .expect("held registration");
        let mut arm_claim = registered.arm_claim;
        let mut permit = None;
        let mut current = registered.commit.cleanup;
        if arm_first {
            let armed = store
                .arm_workspace_cleanup(&mutation(&current), arm_claim.take().expect("arm claim"))
                .expect("arm");
            permit = armed.release_permit;
            current = armed.commit.cleanup;
        }
        let quiet = store
            .record_workspace_cleanup_joined(
                &mutation(&current),
                completion,
                &executor,
                Some(&result(nonce, false)),
                &joined(nonce),
            )
            .expect("joined without Go");
        if let Some(arm_claim) = arm_claim {
            assert_eq!(
                store
                    .arm_workspace_cleanup(&mutation(&quiet.cleanup), arm_claim)
                    .expect_err("late arm")
                    .code(),
                "workspace_cleanup_retired_claim"
            );
        }
        if let Some(permit) = permit {
            assert_eq!(
                permit
                    .consume(nonce, input.daemon_incarnation)
                    .expect_err("late release")
                    .code(),
                "workspace_cleanup_retired_claim"
            );
        }
        assert!(
            store
                .settle_workspace_cleanup(
                    &mutation(&quiet.cleanup),
                    &WorkspaceCleanupSettlement::RemovedExecuted
                )
                .is_err(),
            "no successful released Git result"
        );
        store
            .settle_workspace_cleanup(
                &mutation(&quiet.cleanup),
                &WorkspaceCleanupSettlement::Refused {
                    error_code: "callback_denied".into(),
                },
            )
            .expect("no-Go settlement");
    }
}

#[test]
fn durable_positive_unknown_can_be_reconciled_after_reopen_without_execution_authority() {
    let temp = tempfile::tempdir().expect("temp");
    let database = temp.path().join("state.sqlite3");
    let mut store = SqliteStore::open(&database).expect("store");
    let input = plan(&mut store);
    let mut claimed = confirm(&mut store, &input);
    let nonce = Uuid::new_v4();
    let (registration, completion) = claimed
        .execution_claim
        .take()
        .expect("claim")
        .begin_spawn(nonce)
        .expect("split");
    drop(registration);
    let quiet = store
        .record_workspace_cleanup_joined(
            &mutation(&claimed.cleanup),
            completion,
            &executor(input.daemon_incarnation, nonce),
            None,
            &joined(nonce),
        )
        .expect("positive quiescence");
    let unknown = store
        .settle_workspace_cleanup(
            &mutation(&quiet.cleanup),
            &WorkspaceCleanupSettlement::Unknown {
                error_code: "postcondition_unavailable".into(),
            },
        )
        .expect("unknown with retained receipt");
    drop(store);
    let mut store = SqliteStore::open(&database).expect("reopen");
    assert!(
        store
            .workspace_cleanup(unknown.cleanup.id)
            .expect("status")
            .expect("row")
            .execution_claim
            .is_none()
    );
    let request = mutation(&unknown.cleanup);
    let reconciled = store
        .reconcile_workspace_cleanup(&request, &WorkspaceCleanupSettlement::RemovedObserved)
        .expect("fresh trusted postcondition verdict");
    assert_eq!(
        reconciled.cleanup.outcome,
        Some(WorkspaceCleanupOutcome::RemovedObserved)
    );
    assert!(reconciled.execution_claim.is_none());
    assert!(
        store
            .reconcile_workspace_cleanup(&request, &WorkspaceCleanupSettlement::RemovedObserved)
            .expect("exact replay")
            .duplicate
    );
}

#[test]
fn fresh_unspawned_retirement_allows_refusal_but_unknown_absence_never_does() {
    let temp = tempfile::tempdir().expect("temp");
    let mut store = SqliteStore::open(&temp.path().join("state.sqlite3")).expect("store");
    let input = plan(&mut store);
    let mut claimed = confirm(&mut store, &input);
    let events = store.events().expect("events");
    assert_eq!(
        store
            .settle_workspace_cleanup(
                &mutation(&claimed.cleanup),
                &WorkspaceCleanupSettlement::Refused {
                    error_code: "changed_head".into()
                }
            )
            .expect_err("no absence inference")
            .code(),
        "workspace_cleanup_quiescence_required"
    );
    assert_eq!(store.events().expect("no false settlement"), events);
    let proof = claimed
        .execution_claim
        .take()
        .expect("fresh")
        .retire_without_spawn();
    let quiet = store
        .record_workspace_cleanup_never_spawned(&mutation(&claimed.cleanup), proof)
        .expect("consumed unused authority");
    let refused = store
        .settle_workspace_cleanup(
            &mutation(&quiet.cleanup),
            &WorkspaceCleanupSettlement::Refused {
                error_code: "changed_head".into(),
            },
        )
        .expect("settled refusal");
    assert_eq!(refused.cleanup.phase, WorkspaceCleanupPhase::Refused);
    let mut next = input.clone();
    next.operation_id = Uuid::new_v4();
    next.timestamp = refused.cleanup.updated_at + Duration::seconds(1);
    let mut claimed = confirm(&mut store, &next);
    drop(claimed.execution_claim.take());
    let unknown = store
        .settle_workspace_cleanup(
            &mutation(&claimed.cleanup),
            &WorkspaceCleanupSettlement::Unknown {
                error_code: "reply_lost".into(),
            },
        )
        .expect("unknown retains reservation");
    let events = store.events().expect("events");
    assert!(unknown.cleanup.executor.is_none());
    assert_eq!(
        store
            .reconcile_workspace_cleanup(
                &mutation(&unknown.cleanup),
                &WorkspaceCleanupSettlement::RemovedObserved
            )
            .expect_err("empty fields are not proof")
            .code(),
        "workspace_cleanup_quiescence_required"
    );
    next.operation_id = Uuid::new_v4();
    next.timestamp = unknown.cleanup.updated_at + Duration::seconds(1);
    assert_eq!(
        store
            .plan_workspace_cleanup(&next)
            .expect_err("unknown fenced")
            .code(),
        "workspace_cleanup_busy"
    );
    assert_eq!(store.events().expect("failure rollback"), events);
}

#[test]
fn armed_joined_success_settles_once_and_the_removed_binding_is_a_tombstone() {
    let temp = tempfile::tempdir().expect("temp");
    let database = temp.path().join("state.sqlite3");
    let mut store = SqliteStore::open(&database).expect("store");
    let input = plan(&mut store);
    let mut claimed = confirm(&mut store, &input);
    let nonce = Uuid::new_v4();
    let executor = executor(input.daemon_incarnation, nonce);
    let (registration, completion) = claimed
        .execution_claim
        .take()
        .expect("claim")
        .begin_spawn(nonce)
        .expect("split");
    let registered = store
        .record_workspace_cleanup_executor(&mutation(&claimed.cleanup), registration, &executor)
        .expect("held");
    let armed = store
        .arm_workspace_cleanup(
            &mutation(&registered.commit.cleanup),
            registered.arm_claim.expect("arm claim"),
        )
        .expect("arm");
    armed
        .release_permit
        .expect("fresh permit")
        .consume(nonce, input.daemon_incarnation)
        .expect("one release permission");
    let quiet = store
        .record_workspace_cleanup_joined(
            &mutation(&armed.commit.cleanup),
            completion,
            &executor,
            Some(&result(nonce, true)),
            &joined(nonce),
        )
        .expect("joined released executor");
    let settlement = mutation(&quiet.cleanup);
    let removed = store
        .settle_workspace_cleanup(&settlement, &WorkspaceCleanupSettlement::RemovedExecuted)
        .expect("trusted postconditions");
    assert_eq!(
        removed.cleanup.outcome,
        Some(WorkspaceCleanupOutcome::RemovedExecuted)
    );
    let events = store.events().expect("events");
    let duplicate = store
        .settle_workspace_cleanup(&settlement, &WorkspaceCleanupSettlement::RemovedExecuted)
        .expect("same settlement");
    assert!(duplicate.duplicate);
    assert!(duplicate.execution_claim.is_none());
    assert_eq!(duplicate.cleanup, removed.cleanup);
    assert!(
        store
            .settle_workspace_cleanup(
                &mutation(&removed.cleanup),
                &WorkspaceCleanupSettlement::Unknown {
                    error_code: "later_path_appeared".into()
                }
            )
            .is_err()
    );
    let mut replanned = input.clone();
    replanned.operation_id = Uuid::new_v4();
    replanned.timestamp = removed.cleanup.updated_at + Duration::seconds(1);
    assert_eq!(
        store
            .plan_workspace_cleanup(&replanned)
            .expect_err("historical removed binding")
            .code(),
        "workspace_cleanup_busy"
    );
    assert_eq!(store.events().expect("immutable result"), events);
    drop(store);
    let store = SqliteStore::open(&database).expect("reopen");
    assert_eq!(
        store
            .workspace_cleanup(removed.cleanup.id)
            .expect("read")
            .expect("record")
            .cleanup,
        removed.cleanup
    );
}

#[test]
fn failed_claim_transaction_rolls_back_reservation_operation_and_event() {
    let temp = tempfile::tempdir().expect("temp");
    let database = temp.path().join("state.sqlite3");
    let mut store = SqliteStore::open(&database).expect("store");
    let input = plan(&mut store);
    let preview = store.plan_workspace_cleanup(&input).expect("plan");
    let request = confirmation(&preview.cleanup, input.daemon_incarnation);
    let raw = rusqlite::Connection::open(&database).expect("owned fault fixture");
    raw.execute_batch("CREATE TRIGGER reject_cleanup_command BEFORE INSERT ON workspace_cleanup_commands BEGIN SELECT RAISE(ABORT,'synthetic_cleanup_fault'); END;").expect("inject late failure");
    let events = store.events().expect("before");
    assert!(store.confirm_workspace_cleanup(&request).is_err());
    assert_eq!(
        store
            .workspace_cleanup(preview.cleanup.id)
            .expect("read")
            .expect("record")
            .cleanup,
        preview.cleanup
    );
    assert_eq!(store.events().expect("event rollback"), events);
    raw.execute_batch("DROP TRIGGER reject_cleanup_command;")
        .expect("remove fault");
    assert!(
        store
            .confirm_workspace_cleanup(&request)
            .expect("retry after definite rollback")
            .execution_claim
            .is_some()
    );
}

#[test]
fn idle_read_only_process_claims_block_both_preview_and_late_confirmation() {
    for ready in [false, true] {
        let mut store = SqliteStore::open(std::path::Path::new(":memory:")).expect("store");
        let input = plan(&mut store);
        let preview = store
            .plan_workspace_cleanup(&input)
            .expect("preview before idle process");
        let detections = std::collections::BTreeMap::from([(
            "codex".to_owned(),
            vibemux_harness::HarnessDetection {
                detected: true,
                path: None,
                launcher: None,
                version: Some("synthetic".into()),
            },
        )]);
        store
            .commit_harness_snapshot(
                &detections,
                "2026-10-31T00:00:00Z",
                "cleanup_fixture_detection",
            )
            .expect("metadata detection");
        let opened = store
            .open_chat(&vibemux_store::ChatOpen {
                operation_id: Uuid::new_v4(),
                harness: ChatHarness::Codex,
                profile_sha256: "a".repeat(64),
                resource_key: input.binding.resource_key.clone(),
                base_commit: input.binding.workspace.base_commit.clone(),
                title_ref: None,
                timestamp: input.timestamp,
            })
            .expect("read-only session");
        let claimed = store
            .claim_chat_owner(
                opened.session.chat_id,
                1,
                Some("fixed_session".into()),
                input.timestamp,
            )
            .expect("idle process claim");
        if ready {
            store
                .bind_chat_vendor(
                    opened.session.chat_id,
                    1,
                    claimed.fence,
                    "fixed_session",
                    input.timestamp,
                )
                .expect("idle ready process");
        }
        let before = store.events().expect("before");
        let mut new_preview = input.clone();
        new_preview.operation_id = Uuid::new_v4();
        assert_eq!(
            store
                .plan_workspace_cleanup(&new_preview)
                .expect_err("idle live process blocks plan")
                .code(),
            "workspace_cleanup_busy"
        );
        assert_eq!(
            store
                .confirm_workspace_cleanup(&confirmation(
                    &preview.cleanup,
                    input.daemon_incarnation
                ))
                .expect_err("idle live process blocks confirm")
                .code(),
            "workspace_cleanup_busy"
        );
        assert_eq!(store.events().expect("unchanged"), before);
    }
}

#[test]
fn migration_fourteen_is_atomic_and_preserves_historical_json_and_events() {
    let temp = tempfile::tempdir().expect("temp");
    let database = temp.path().join("state.sqlite3");
    let mut store = SqliteStore::open(&database).expect("store");
    let input = plan(&mut store);
    let historical = store
        .chat_execution_owner_state(input.binding.chat_id)
        .expect("read")
        .expect("owner");
    let events = store.events().expect("events");
    drop(store);
    let raw = rusqlite::Connection::open(&database).expect("closed owned fixture");
    let original:Vec<String>=raw.prepare("SELECT record_json FROM chat_sessions UNION ALL SELECT record_json FROM chat_execution_owners").expect("query").query_map([],|row|row.get(0)).expect("rows").collect::<Result<_,_>>().expect("history");
    raw.execute_batch("DROP TABLE workspace_cleanup_commands; DROP TABLE workspace_cleanups; DELETE FROM schema_migrations WHERE version=14; CREATE INDEX workspace_cleanup_resource ON chat_sessions(resource_key);").expect("schema13 and late collision");
    assert!(SqliteStore::open(&database).is_err());
    assert_eq!(
        raw.query_row("SELECT MAX(version) FROM schema_migrations", [], |r| r
            .get::<_, u32>(0))
            .expect("version"),
        13
    );
    assert!(!raw.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='workspace_cleanups')",[],|r|r.get::<_,bool>(0)).expect("atomic table rollback"));
    raw.execute_batch("DROP INDEX workspace_cleanup_resource;")
        .expect("remove fault");
    drop(raw);
    let store = SqliteStore::open(&database).expect("upgrade");
    assert_eq!(
        store
            .chat_execution_owner_state(input.binding.chat_id)
            .expect("read")
            .expect("owner")
            .owner,
        historical.owner
    );
    assert_eq!(store.events().expect("events preserved"), events);
    let raw = rusqlite::Connection::open(&database).expect("read fixture");
    let restored:Vec<String>=raw.prepare("SELECT record_json FROM chat_sessions UNION ALL SELECT record_json FROM chat_execution_owners").expect("query").query_map([],|row|row.get(0)).expect("rows").collect::<Result<_,_>>().expect("history");
    assert_eq!(restored, original);
}

#[test]
fn concurrent_confirmations_issue_exactly_one_fresh_claim() {
    let temp = tempfile::tempdir().expect("temp");
    let database = temp.path().join("state.sqlite3");
    let mut store = SqliteStore::open(&database).expect("store");
    let input = plan(&mut store);
    let preview = store.plan_workspace_cleanup(&input).expect("plan");
    let request = confirmation(&preview.cleanup, input.daemon_incarnation);
    drop(store);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let database = database.clone();
            let request = request.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut store = SqliteStore::open(&database).expect("concurrent store");
                barrier.wait();
                let commit = store
                    .confirm_workspace_cleanup(&request)
                    .expect("serialized confirm/replay");
                (commit.execution_claim.is_some(), commit.duplicate)
            })
        })
        .collect();
    let results: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().expect("worker"))
        .collect();
    assert_eq!(results.iter().filter(|(claim, _)| *claim).count(), 1);
    assert_eq!(
        results.iter().filter(|(_, duplicate)| *duplicate).count(),
        1
    );
    let store = SqliteStore::open(&database).expect("reopen");
    assert_eq!(
        store
            .events()
            .expect("events")
            .iter()
            .filter(|event| event.event_type().as_str() == "workspace_cleanup_claimed")
            .count(),
        1
    );
}

#[test]
fn negative_join_proof_remains_unknown_and_never_releases_the_reservation() {
    let mut store = SqliteStore::open(std::path::Path::new(":memory:")).expect("store");
    let input = plan(&mut store);
    let mut claimed = confirm(&mut store, &input);
    let nonce = Uuid::new_v4();
    let (registration, completion) = claimed
        .execution_claim
        .take()
        .expect("claim")
        .begin_spawn(nonce)
        .expect("split");
    drop(registration);
    let mut proof = joined(nonce);
    if let CleanupQuiescence::JoinedProcess { tree_empty, .. } = &mut proof {
        *tree_empty = false;
    }
    let unknown = store
        .record_workspace_cleanup_joined(
            &mutation(&claimed.cleanup),
            completion,
            &executor(input.daemon_incarnation, nonce),
            None,
            &proof,
        )
        .expect("uncertain owned cleanup");
    assert_eq!(unknown.cleanup.phase, WorkspaceCleanupPhase::Unknown);
    let before = store.events().expect("before");
    assert_eq!(
        store
            .reconcile_workspace_cleanup(
                &mutation(&unknown.cleanup),
                &WorkspaceCleanupSettlement::Refused {
                    error_code: "unchanged_path".into()
                }
            )
            .expect_err("negative proof cannot release")
            .code(),
        "workspace_cleanup_quiescence_required"
    );
    assert_eq!(store.events().expect("no settlement"), before);
    let mut retry = input;
    retry.operation_id = Uuid::new_v4();
    retry.timestamp = unknown.cleanup.updated_at + Duration::seconds(1);
    assert_eq!(
        store
            .plan_workspace_cleanup(&retry)
            .expect_err("still reserved")
            .code(),
        "workspace_cleanup_busy"
    );
}

#[test]
fn metadata_above_legacy_chat_limit_round_trips_and_oversized_rows_fail_closed() {
    let temp = tempfile::tempdir().expect("temp");
    let database = temp.path().join("state.sqlite3");
    let mut store = SqliteStore::open(&database).expect("store");
    let mut input = plan(&mut store);
    input.binding.repository_root = format!("/{}", "r".repeat(4000));
    input.binding.common_git_dir = format!("/{}", "g".repeat(4000));
    let mut claimed = confirm(&mut store, &input);
    let nonce = Uuid::new_v4();
    let (registration, completion) = claimed
        .execution_claim
        .take()
        .expect("claim")
        .begin_spawn(nonce)
        .expect("split");
    let mut executor = executor(input.daemon_incarnation, nonce);
    executor.expected_executable = format!("/{}", "e".repeat(4000));
    executor.observed_executable = executor.expected_executable.clone();
    executor.program_executable = format!("/{}", "p".repeat(4000));
    let registered = store
        .record_workspace_cleanup_executor(&mutation(&claimed.cleanup), registration, &executor)
        .expect("large private metadata");
    let encoded = serde_json::to_string(&registered.commit.cleanup).expect("record");
    assert!(encoded.len() > 16 * 1024);
    assert_eq!(
        store
            .workspace_cleanup(claimed.cleanup.id)
            .expect("bounded 64KiB read")
            .expect("row")
            .cleanup,
        registered.commit.cleanup
    );
    for event in store.events().expect("events").iter().filter(|event| {
        event
            .event_type()
            .as_str()
            .starts_with("workspace_cleanup_")
    }) {
        let encoded = serde_json::to_string(event).expect("event");
        assert!(!encoded.contains(&input.binding.workspace.ownership_token));
        assert!(!encoded.contains(&input.binding.head_commit));
        assert!(!encoded.contains(&input.binding.repository_root));
    }
    drop(registered.arm_claim);
    drop(completion);
    let raw = rusqlite::Connection::open(&database).expect("owned corruption fixture");
    raw.execute(
        "UPDATE workspace_cleanups SET record_json=? WHERE cleanup_id=?",
        rusqlite::params!["x".repeat(65537), claimed.cleanup.id.to_string()],
    )
    .expect("oversized row");
    assert_eq!(
        store
            .workspace_cleanup(claimed.cleanup.id)
            .expect_err("SQL-bounded decode")
            .code(),
        "workspace_cleanup_projection_mismatch"
    );
}

#[test]
fn stale_and_failed_completion_writes_keep_a_retired_proof_for_safe_retry() {
    let temp = tempfile::tempdir().expect("temp");
    let database = temp.path().join("state.sqlite3");
    let mut store = SqliteStore::open(&database).expect("store");
    let input = plan(&mut store);
    let mut claimed = confirm(&mut store, &input);
    let nonce = Uuid::new_v4();
    let executor = executor(input.daemon_incarnation, nonce);
    let (registration, completion) = claimed
        .execution_claim
        .take()
        .expect("claim")
        .begin_spawn(nonce)
        .expect("split");
    let registered = store
        .record_workspace_cleanup_executor(&mutation(&claimed.cleanup), registration, &executor)
        .expect("register");
    let mut stale = mutation(&registered.commit.cleanup);
    let armed = store
        .arm_workspace_cleanup(
            &mutation(&registered.commit.cleanup),
            registered.arm_claim.expect("arm claim"),
        )
        .expect("arm");
    let failure = store
        .record_workspace_cleanup_joined(&stale, completion, &executor, None, &joined(nonce))
        .expect_err("stale version");
    assert_eq!(failure.code(), "workspace_cleanup_conflict");
    assert_eq!(
        armed
            .release_permit
            .expect("delayed permit")
            .consume(nonce, input.daemon_incarnation)
            .expect_err("even failed completion retires Go")
            .code(),
        "workspace_cleanup_retired_claim"
    );
    let (_, completion) = failure.into_parts();
    let current = store
        .workspace_cleanup(claimed.cleanup.id)
        .expect("read first")
        .expect("record")
        .cleanup;
    stale.expected_version = current.version;
    stale.timestamp = current.updated_at + Duration::seconds(1);
    let raw = rusqlite::Connection::open(&database).expect("fault fixture");
    raw.execute_batch("CREATE TRIGGER reject_cleanup_join BEFORE INSERT ON workspace_cleanup_commands BEGIN SELECT RAISE(ABORT,'synthetic_join_fault'); END;").expect("late fault");
    let before = store.events().expect("events");
    let failure = store
        .record_workspace_cleanup_joined(&stale, completion, &executor, None, &joined(nonce))
        .expect_err("late SQL rollback");
    assert_eq!(failure.code(), "store_database_error");
    assert_eq!(store.events().expect("rolled back"), before);
    let (_, completion) = failure.into_parts();
    raw.execute_batch("DROP TRIGGER reject_cleanup_join;")
        .expect("remove fault");
    assert_eq!(
        store
            .record_workspace_cleanup_joined(&stale, completion, &executor, None, &joined(nonce))
            .expect("same retired proof retry")
            .cleanup
            .phase,
        WorkspaceCleanupPhase::Quiescent
    );
}

#[test]
fn a_failed_never_spawned_write_retains_only_the_retired_proof() {
    let temp = tempfile::tempdir().expect("temp");
    let database = temp.path().join("state.sqlite3");
    let mut store = SqliteStore::open(&database).expect("store");
    let input = plan(&mut store);
    let mut claimed = confirm(&mut store, &input);
    let proof = claimed
        .execution_claim
        .take()
        .expect("claim")
        .retire_without_spawn();
    let request = mutation(&claimed.cleanup);
    let raw = rusqlite::Connection::open(&database).expect("fault fixture");
    raw.execute_batch("CREATE TRIGGER reject_unspawned BEFORE INSERT ON workspace_cleanup_commands BEGIN SELECT RAISE(ABORT,'synthetic_no_spawn_fault'); END;").expect("late failure");
    let failure = store
        .record_workspace_cleanup_never_spawned(&request, proof)
        .expect_err("rollback");
    assert_eq!(failure.code(), "store_database_error");
    let (_, proof) = failure.into_parts();
    assert_eq!(
        store
            .workspace_cleanup(claimed.cleanup.id)
            .expect("read first")
            .expect("row")
            .cleanup
            .phase,
        WorkspaceCleanupPhase::Claimed
    );
    raw.execute_batch("DROP TRIGGER reject_unspawned;")
        .expect("remove fault");
    assert_eq!(
        store
            .record_workspace_cleanup_never_spawned(&request, proof)
            .expect("safe retired proof retry")
            .cleanup
            .phase,
        WorkspaceCleanupPhase::Quiescent
    );
}

#[test]
fn a_retained_unused_claim_can_prove_never_spawned_after_unknown() {
    let mut store = SqliteStore::open(std::path::Path::new(":memory:")).expect("store");
    let input = plan(&mut store);
    let mut claimed = confirm(&mut store, &input);
    let unused = claimed
        .execution_claim
        .take()
        .expect("retained live authority");
    let unknown = store
        .settle_workspace_cleanup(
            &mutation(&claimed.cleanup),
            &WorkspaceCleanupSettlement::Unknown {
                error_code: "delivery_delayed".into(),
            },
        )
        .expect("unknown before delivery");
    let proof = unused.retire_without_spawn();
    let quiet = store
        .record_workspace_cleanup_never_spawned(&mutation(&unknown.cleanup), proof)
        .expect("fresh sealed retirement, not absence inference");
    assert_eq!(quiet.cleanup.phase, WorkspaceCleanupPhase::Quiescent);
    assert_eq!(quiet.cleanup.error_code, None);
    let refused = store
        .reconcile_workspace_cleanup(
            &mutation(&quiet.cleanup),
            &WorkspaceCleanupSettlement::Refused {
                error_code: "cancelled_before_spawn".into(),
            },
        )
        .expect("explicit refusal");
    assert_eq!(refused.cleanup.phase, WorkspaceCleanupPhase::Refused);
    assert!(refused.execution_claim.is_none());
}

#[test]
fn plan_operation_lookup_and_pending_pages_are_inert_and_never_return_authority() {
    let mut store = SqliteStore::open(std::path::Path::new(":memory:")).expect("store");
    let input = plan(&mut store);
    assert!(
        store
            .workspace_cleanup_for_plan_operation(input.operation_id)
            .expect("missing receipt")
            .is_none()
    );
    let preview = store.plan_workspace_cleanup(&input).expect("plan");
    assert!(
        store
            .workspace_cleanup_pending(None, 16)
            .expect("previews do not hold effects")
            .is_empty()
    );
    let receipt = store
        .workspace_cleanup_for_plan_operation(input.operation_id)
        .expect("lost Plan reply recovery")
        .expect("receipt");
    assert_eq!(receipt.cleanup, preview.cleanup);
    assert!(receipt.execution_claim.is_none());
    let mut other_preview = input.clone();
    other_preview.operation_id = Uuid::new_v4();
    store
        .plan_workspace_cleanup(&other_preview)
        .expect("later inert preview");
    let confirmation = confirmation(&preview.cleanup, input.daemon_incarnation);
    let claimed = store
        .confirm_workspace_cleanup(&confirmation)
        .expect("claim");
    let before = store.events().expect("events");
    let page = store.workspace_cleanup_pending(None, 1).expect("pending");
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].cleanup, claimed.cleanup);
    assert!(page[0].execution_claim.is_none());
    let latest = store
        .workspace_cleanup_for_run(input.binding.run_id)
        .expect("discover after client restart")
        .expect("record");
    assert_eq!(latest.cleanup, claimed.cleanup);
    assert!(latest.execution_claim.is_none());
    assert!(
        store
            .workspace_cleanup_pending(Some(claimed.cleanup.id), 1)
            .expect("stable cursor")
            .is_empty()
    );
    assert!(store.workspace_cleanup_pending(None, 0).is_err());
    assert!(store.workspace_cleanup_pending(None, 129).is_err());
    assert!(
        store
            .workspace_cleanup_for_plan_operation(confirmation.operation_id)
            .is_err(),
        "wrong operation kind"
    );
    assert_eq!(store.events().expect("reads are inert"), before);
}
