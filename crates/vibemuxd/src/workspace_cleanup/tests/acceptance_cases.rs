//! Explicit service acceptance over owned temporary Git and a synthetic terminal peer.
use super::*;

use crate::chat_workspace::tests::fixture_git;
use std::fs;
use vibemux_events::EventEnvelope;

const ACCEPTANCE_DEADLINE: Duration = Duration::from_secs(25);
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const DIRTY_BYTES: &[u8] = b"owned data introduced after review\n\x00preserve exactly\n";

struct MainSnapshot {
    head: String,
    index: Vec<u8>,
    config: Vec<u8>,
    base_file: Vec<u8>,
}

impl MainSnapshot {
    async fn capture(fixture: &OwnedFixture) -> Self {
        assert!(
            fixture_git(&fixture.git, &fixture.root, &["status", "--porcelain"])
                .await
                .is_empty(),
            "main repository stays clean"
        );
        Self {
            head: fixture_git(&fixture.git, &fixture.root, &["rev-parse", "HEAD"]).await,
            index: fs::read(fixture.root.join(".git/index")).expect("main index"),
            config: fs::read(fixture.root.join(".git/config")).expect("main config"),
            base_file: fs::read(fixture.root.join("base.txt")).expect("main base file"),
        }
    }

    async fn assert_unchanged(&self, fixture: &OwnedFixture) {
        let current = Self::capture(fixture).await;
        assert_eq!(current.head, self.head, "main HEAD");
        assert_eq!(current.index, self.index, "main index bytes");
        assert_eq!(current.config, self.config, "main configuration bytes");
        assert_eq!(current.base_file, self.base_file, "main tracked data");
    }
}

async fn idle(jobs: &WorkspaceJobs) {
    tokio::time::timeout(ACCEPTANCE_DEADLINE, async {
        while jobs.pending_for_test() != 0 {
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    })
    .await
    .expect("accepted producer and its observations joined");
}

async fn read_record(fixture: &OwnedFixture, id: WorkspaceCleanupId) -> WorkspaceCleanup {
    store_call(&fixture.writer, move |store| store.workspace_cleanup(id))
        .await
        .expect("cleanup read")
        .expect("durable cleanup")
        .cleanup
}

async fn events(fixture: &OwnedFixture) -> Vec<EventEnvelope> {
    store_call(&fixture.writer, |store| store.events())
        .await
        .expect("audit read")
}

fn count(events: &[EventEnvelope], kind: &str) -> usize {
    events
        .iter()
        .filter(|event| event.event_type().as_str() == kind)
        .count()
}

async fn retained_head(fixture: &OwnedFixture) -> String {
    let branch = &fixture
        .owner
        .workspace
        .as_ref()
        .expect("owned workspace")
        .branch;
    let reference = format!("refs/heads/{branch}");
    fixture_git(
        &fixture.git,
        &fixture.root,
        &["rev-parse", "--verify", &reference],
    )
    .await
    .trim()
    .to_owned()
}

fn assert_no_spawn(record: &WorkspaceCleanup, audit: &[EventEnvelope]) {
    assert_eq!(record.phase, WorkspaceCleanupPhase::Refused);
    assert_eq!(record.outcome, Some(WorkspaceCleanupOutcome::Refused));
    assert!(
        record.executor.is_none()
            && record.arm_operation_id.is_none()
            && record.process_result.is_none()
    );
    assert!(
        matches!(record.quiescence, Some(CleanupQuiescence::NeverSpawned { retired_claim_id })
        if Some(retired_claim_id) == record.claim_id)
    );
    assert_eq!(count(audit, "workspace_cleanup_claimed"), 1);
    assert_eq!(count(audit, "workspace_cleanup_executor_registered"), 0);
    assert_eq!(count(audit, "workspace_cleanup_armed"), 0);
    assert_eq!(count(audit, "workspace_cleanup_executor_joined"), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit cleanup service acceptance: build first-party trampoline"]
async fn dirty_and_ignored_data_after_review_refuse_before_spawn_and_preserve_exact_bytes() {
    for ignored in [false, true] {
        let mut fixture = OwnedFixture::new().await;
        fixture.change(ChatOwnerChange::Stop).await;
        let main = MainSnapshot::capture(&fixture).await;
        let jobs = WorkspaceJobs::new();
        let peer = Peer::clear(fixture.root.clone(), jobs.clone());
        let service = service(&fixture, &peer, jobs.clone());
        let scope = fixture_scope(&fixture);
        let preview = service
            .plan(scope, Uuid::new_v4())
            .await
            .expect("clean reviewed plan");
        assert!(preview.can_confirm);
        let path = if ignored {
            let directory = fixture.worktree().join(".vibemux");
            fs::create_dir(&directory).expect("owned ignored directory");
            directory.join("ignored_payload.bin")
        } else {
            fixture.worktree().join("base.txt")
        };
        let original = (!ignored).then(|| fs::read(&path).expect("original tracked bytes"));
        fs::write(&path, DIRTY_BYTES).expect("inject only owned after-review data");
        service
            .confirm(
                scope,
                Uuid::new_v4(),
                preview.cleanup_id,
                preview.plan_version,
            )
            .await
            .expect("fresh confirmation admitted");
        idle(&jobs).await;
        let record = read_record(&fixture, preview.cleanup_id).await;
        let audit = events(&fixture).await;
        jobs.shutdown().await.expect("owned jobs joined");
        peer.finish().await;
        assert_no_spawn(&record, &audit);
        assert_eq!(
            record.error_code.as_deref(),
            Some("workspace_unsafe_cleanup")
        );
        assert_eq!(fs::read(&path).expect("retained data"), DIRTY_BYTES);
        assert_eq!(retained_head(&fixture).await, fixture.base);
        assert!(fixture.worktree().exists());
        main.assert_unchanged(&fixture).await;
        // Restore only this test's fault after every preservation assertion.
        // A failed assertion reaches OwnedFixture::drop with the data retained.
        if let Some(original) = original {
            fs::write(&path, original).expect("restore exact original fixture bytes");
        } else {
            fs::remove_file(&path).expect("remove owned injected ignored file");
            fs::remove_dir(path.parent().expect("owned ignored parent"))
                .expect("remove empty owned directory");
        }
        fixture.finish().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit cleanup service acceptance: build first-party trampoline"]
async fn committed_head_before_plan_is_approved_and_retained_after_removal() {
    let mut fixture = OwnedFixture::new().await;
    let advanced = fixture.advance_head().await;
    assert_ne!(advanced, fixture.base);
    fixture.change(ChatOwnerChange::Stop).await;
    let main = MainSnapshot::capture(&fixture).await;
    let jobs = WorkspaceJobs::new();
    let peer = Peer::clear(fixture.root.clone(), jobs.clone());
    let service = service(&fixture, &peer, jobs.clone());
    let scope = fixture_scope(&fixture);
    let preview = service
        .plan(scope, Uuid::new_v4())
        .await
        .expect("review advanced clean HEAD");
    assert_eq!(
        read_record(&fixture, preview.cleanup_id)
            .await
            .binding
            .head_commit,
        advanced
    );
    service
        .confirm(
            scope,
            Uuid::new_v4(),
            preview.cleanup_id,
            preview.plan_version,
        )
        .await
        .expect("confirm approved HEAD");
    idle(&jobs).await;
    let record = read_record(&fixture, preview.cleanup_id).await;
    jobs.shutdown().await.expect("joined");
    peer.finish().await;
    assert_eq!(record.phase, WorkspaceCleanupPhase::Removed);
    assert_eq!(
        record.outcome,
        Some(WorkspaceCleanupOutcome::RemovedExecuted)
    );
    assert_eq!(record.binding.head_commit, advanced);
    assert!(!fixture.worktree().exists());
    assert_eq!(retained_head(&fixture).await, advanced);
    main.assert_unchanged(&fixture).await;
    // Parent-owned helper must check this approved branch HEAD while retaining
    // the original base for all main-repository integrity assertions.
    fixture.finish_removed_at(&advanced).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit cleanup service acceptance: build first-party trampoline"]
async fn clean_head_changed_after_plan_refuses_without_spawn_or_history_rewrite() {
    let mut fixture = OwnedFixture::new().await;
    fixture.change(ChatOwnerChange::Stop).await;
    let main = MainSnapshot::capture(&fixture).await;
    let jobs = WorkspaceJobs::new();
    let peer = Peer::clear(fixture.root.clone(), jobs.clone());
    let service = service(&fixture, &peer, jobs.clone());
    let scope = fixture_scope(&fixture);
    let preview = service
        .plan(scope, Uuid::new_v4())
        .await
        .expect("review original HEAD");
    let advanced = fixture.advance_head().await;
    assert_ne!(advanced, fixture.base);
    service
        .confirm(
            scope,
            Uuid::new_v4(),
            preview.cleanup_id,
            preview.plan_version,
        )
        .await
        .expect("fresh confirm");
    idle(&jobs).await;
    let record = read_record(&fixture, preview.cleanup_id).await;
    let audit = events(&fixture).await;
    jobs.shutdown().await.expect("joined");
    peer.finish().await;
    assert_no_spawn(&record, &audit);
    assert_eq!(
        record.error_code.as_deref(),
        Some("workspace_cleanup_changed")
    );
    assert_eq!(record.binding.head_commit, fixture.base);
    assert!(fixture.worktree().exists());
    assert_eq!(retained_head(&fixture).await, advanced);
    assert_eq!(
        fixture_git(&fixture.git, &fixture.worktree(), &["rev-parse", "HEAD"])
            .await
            .trim(),
        advanced
    );
    main.assert_unchanged(&fixture).await;
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit cleanup service acceptance: build first-party trampoline"]
async fn dropped_confirm_waiter_after_owned_admission_does_not_cancel_or_duplicate_removal() {
    let mut fixture = OwnedFixture::new().await;
    fixture.change(ChatOwnerChange::Stop).await;
    let main = MainSnapshot::capture(&fixture).await;
    let jobs = WorkspaceJobs::new();
    let peer = Peer::clear(fixture.root.clone(), jobs.clone());
    let service = service(&fixture, &peer, jobs.clone());
    let scope = fixture_scope(&fixture);
    let preview = service.plan(scope, Uuid::new_v4()).await.expect("plan");
    let key = fixture.owner.resource_key.as_ref().expect("owned resource");
    let gate = jobs.write(key).await.expect("hold effect gate");
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    *service.inner.after_confirm_read.lock().expect("hook") = Some(Arc::clone(&barrier));
    let caller_service = service.clone();
    let operation_id = Uuid::new_v4();
    let id = preview.cleanup_id;
    let version = preview.plan_version;
    let caller = tokio::spawn(async move {
        caller_service
            .confirm(scope, operation_id, id, version)
            .await
    });
    tokio::time::timeout(ACCEPTANCE_DEADLINE, barrier.wait())
        .await
        .expect("owned job passed original read");
    assert!(jobs.pending_for_test() > 0, "producer is actually owned");
    caller.abort();
    assert!(
        caller
            .await
            .expect_err("RPC waiter cancelled")
            .is_cancelled()
    );
    assert_eq!(
        read_record(&fixture, id).await.phase,
        WorkspaceCleanupPhase::Planned
    );
    *service.inner.after_confirm_read.lock().expect("hook") = None;
    drop(gate);
    idle(&jobs).await;
    let record = read_record(&fixture, id).await;
    let audit = events(&fixture).await;
    let replay = service
        .confirm(scope, operation_id, id, version)
        .await
        .expect("same confirmation receipt");
    assert_eq!(
        events(&fixture).await,
        audit,
        "duplicate performs no second job effect"
    );
    jobs.shutdown().await.expect("owned producer joined");
    peer.finish().await;
    assert_eq!(record.phase, WorkspaceCleanupPhase::Removed);
    assert_eq!(
        record.outcome,
        Some(WorkspaceCleanupOutcome::RemovedExecuted)
    );
    assert_eq!(replay.outcome, record.outcome);
    assert_eq!(count(&audit, "workspace_cleanup_claimed"), 1);
    assert_eq!(count(&audit, "workspace_cleanup_armed"), 1);
    assert_eq!(count(&audit, "workspace_cleanup_executor_joined"), 1);
    assert_eq!(count(&audit, "workspace_cleanup_settled"), 1);
    main.assert_unchanged(&fixture).await;
    fixture.finish_removed().await;
}

async fn settlement_fault(fixture: &OwnedFixture, install: bool) {
    let database = fixture.state_dir.join("state.sqlite3");
    assert!(database.is_file() && database.parent() == Some(fixture.state_dir.as_path()));
    tokio::task::spawn_blocking(move || {
        let connection = rusqlite::Connection::open_with_flags(
            database,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .expect("existing owned fault database");
        connection
            .busy_timeout(Duration::from_secs(5))
            .expect("bounded fixture lock wait");
        connection
            .execute_batch(if install {
                "CREATE TRIGGER fixture_reject_cleanup_settlement BEFORE INSERT ON chat_operations
             WHEN NEW.kind='workspace_cleanup_settled'
             BEGIN SELECT RAISE(ABORT,'fixture_settlement_rejected'); END;"
            } else {
                "DROP TRIGGER fixture_reject_cleanup_settlement;"
            })
            .expect("owned settlement write fault");
    })
    .await
    .expect("fault fixture task");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit cleanup service acceptance: build first-party trampoline"]
async fn failed_final_settlement_leaves_durable_quiescence_for_restart_reconciliation_only() {
    let mut fixture = OwnedFixture::new().await;
    fixture.change(ChatOwnerChange::Stop).await;
    let main = MainSnapshot::capture(&fixture).await;
    let jobs = WorkspaceJobs::new();
    let peer = Peer::clear(fixture.root.clone(), jobs.clone());
    let first_service = service(&fixture, &peer, jobs.clone());
    let scope = fixture_scope(&fixture);
    let preview = first_service
        .plan(scope, Uuid::new_v4())
        .await
        .expect("plan");
    let before = events(&fixture).await;
    settlement_fault(&fixture, true).await;
    first_service
        .confirm(
            scope,
            Uuid::new_v4(),
            preview.cleanup_id,
            preview.plan_version,
        )
        .await
        .expect("effect admission acknowledged before settlement");
    idle(&jobs).await;
    let quiet = read_record(&fixture, preview.cleanup_id).await;
    let fault_audit = events(&fixture).await;
    jobs.shutdown()
        .await
        .expect("original producer joined despite failed settlement reply");
    peer.finish().await;
    assert_eq!(quiet.phase, WorkspaceCleanupPhase::Quiescent);
    assert!(
        quiet
            .quiescence
            .as_ref()
            .is_some_and(CleanupQuiescence::is_positive)
    );
    assert!(
        quiet
            .process_result
            .as_ref()
            .is_some_and(CleanupProcessResult::is_successful)
    );
    assert!(quiet.outcome.is_none() && quiet.error_code.is_none());
    assert!(
        !fixture.worktree().exists(),
        "Git effect finished before final write fault"
    );
    assert!(
        fault_audit.starts_with(&before),
        "prior audit history stays intact"
    );
    assert_eq!(count(&fault_audit, "workspace_cleanup_armed"), 1);
    assert_eq!(count(&fault_audit, "workspace_cleanup_executor_joined"), 1);
    assert_eq!(
        count(&fault_audit, "workspace_cleanup_settled"),
        0,
        "failed settlement and Unknown writes roll back"
    );
    assert_eq!(retained_head(&fixture).await, fixture.base);
    main.assert_unchanged(&fixture).await;
    settlement_fault(&fixture, false).await;
    let old_incarnation = first_service.inner.incarnation;
    drop(first_service);

    let new_jobs = WorkspaceJobs::new();
    let new_peer = Peer::clear(fixture.root.clone(), new_jobs.clone());
    let restarted = service(&fixture, &new_peer, new_jobs.clone());
    assert_ne!(restarted.inner.incarnation, old_incarnation);
    restarted.recover().await;
    assert_eq!(
        read_record(&fixture, preview.cleanup_id).await,
        quiet,
        "startup neither re-executes nor invents effect authority"
    );
    assert_eq!(events(&fixture).await, fault_audit);
    let operation = Uuid::new_v4();
    let reconciled = restarted
        .reconcile(scope, operation, preview.cleanup_id, quiet.version)
        .await
        .expect("explicit reconciliation of durable positive quiescence");
    let after = events(&fixture).await;
    let repeated = restarted
        .reconcile(scope, operation, preview.cleanup_id, quiet.version)
        .await
        .expect("terminal reconciliation replay");
    assert_eq!(repeated, reconciled);
    assert_eq!(events(&fixture).await, after);
    new_jobs.shutdown().await.expect("reconciliation joined");
    new_peer.finish().await;
    assert_eq!(reconciled.phase, WorkspaceCleanupPhase::Removed);
    assert_eq!(
        reconciled.outcome,
        Some(WorkspaceCleanupOutcome::RemovedObserved)
    );
    let settled = read_record(&fixture, preview.cleanup_id).await;
    assert_eq!(settled.executor, quiet.executor, "same audited executor");
    assert_eq!(
        settled.arm_operation_id, quiet.arm_operation_id,
        "same Go arm"
    );
    assert_eq!(
        settled.process_result, quiet.process_result,
        "same joined result"
    );
    assert_eq!(
        settled.quiescence, quiet.quiescence,
        "same durable positive proof"
    );
    assert_eq!(count(&after, "workspace_cleanup_armed"), 1, "no second Go");
    assert_eq!(
        count(&after, "workspace_cleanup_executor_joined"),
        1,
        "no second executor"
    );
    assert_eq!(count(&after, "workspace_cleanup_reconciled"), 1);
    assert!(after.starts_with(&fault_audit));
    main.assert_unchanged(&fixture).await;
    fixture.finish_removed().await;
}
