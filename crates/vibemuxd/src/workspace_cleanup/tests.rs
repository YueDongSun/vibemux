//! Real owned Git removal with a bounded synthetic terminal protocol peer.
use super::*;
use crate::{
    chat_workspace::tests::OwnedFixture,
    plugin_registry::{PluginState, PluginStatus},
    plugin_requests::{PluginRequest, PluginRequestClient, RequestRoute},
};
use prost::Message;
use std::collections::BTreeMap;
use tokio::sync::{mpsc, watch};
use vibemux_plugin_protocol::{terminal::CLEANUP_INVENTORY_METHOD, wire::TerminalInventory};
use vibemux_store::ChatOwnerChange;
mod acceptance_cases;

struct Peer {
    terminal: Arc<TerminalObserver>,
    task: tokio::task::JoinHandle<()>,
    _routes: watch::Sender<BTreeMap<String, RequestRoute>>,
    _status: watch::Sender<PluginStatus>,
}
impl Peer {
    fn clear(root: PathBuf, jobs: WorkspaceJobs) -> Self {
        let (requests, mut receiver) = mpsc::channel::<PluginRequest>(8);
        let (status, status_rx) = watch::channel(PluginStatus {
            plugin_id: "fixture_observer".into(),
            state: PluginState::Active,
            restarts: 0,
            max_restarts: 0,
            session_id: Some("fixture_session".into()),
            process_id: None,
            last_error_code: None,
            graceful_shutdown: None,
        });
        let (routes, routes_rx) = watch::channel(BTreeMap::from([(
            "fixture_observer".into(),
            RequestRoute {
                requests,
                status: status_rx,
            },
        )]));
        let terminal = Arc::new(TerminalObserver::with_workspace_jobs(
            PluginRequestClient { routes: routes_rx },
            root,
            jobs,
        ));
        let task = tokio::spawn(async move {
            while let Some(request) = receiver.recv().await {
                assert_eq!(request.method, CLEANUP_INVENTORY_METHOD);
                let _ = request.response.send(Ok(TerminalInventory {
                    instance_id: "fixture_backend".into(),
                    panes: Vec::new(),
                }
                .encode_to_vec()));
            }
        });
        Self {
            terminal,
            task,
            _routes: routes,
            _status: status,
        }
    }
    async fn finish(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}

fn service(fixture: &OwnedFixture, peer: &Peer, jobs: WorkspaceJobs) -> WorkspaceCleanupService {
    let mut service = WorkspaceCleanupService::new(
        fixture.writer.clone(),
        fixture.root.clone(),
        Arc::clone(&peer.terminal),
        jobs,
        vec!["fixture_observer".into()],
    );
    let image = std::env::current_exe().expect("test image");
    let trampoline = image
        .parent()
        .expect("deps")
        .parent()
        .expect("profile")
        .join("vibemux_launch_trampoline.exe");
    assert!(
        trampoline.is_file(),
        "build the first-party trampoline before explicit service acceptance"
    );
    Arc::get_mut(&mut service.inner)
        .expect("fresh service")
        .trampoline = trampoline;
    service
}
fn fixture_scope(fixture: &OwnedFixture) -> WorkspaceCleanupScope {
    WorkspaceCleanupScope {
        project_id: fixture.owner.project_id,
        task_id: fixture.owner.task_id,
        run_id: fixture.owner.run_id,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit cleanup service acceptance: build the first-party trampoline"]
async fn a_reviewed_clean_owner_is_removed_once_with_its_branch_and_history_retained() {
    let mut fixture = OwnedFixture::new().await;
    fixture.change(ChatOwnerChange::Stop).await;
    let jobs = WorkspaceJobs::new();
    let peer = Peer::clear(fixture.root.clone(), jobs.clone());
    let service = service(&fixture, &peer, jobs.clone());
    service.recover().await;
    let scope = fixture_scope(&fixture);
    let operation = Uuid::new_v4();
    let preview = service
        .plan(scope, operation)
        .await
        .expect("reviewable plan");
    assert!(preview.can_confirm);
    assert!(fixture.worktree().exists(), "Plan never removes");
    let request = Uuid::new_v4();
    let admitted = service
        .confirm(scope, request, preview.cleanup_id, preview.plan_version)
        .await
        .expect("explicit confirmation");
    assert!(matches!(
        admitted.phase,
        WorkspaceCleanupPhase::Claimed
            | WorkspaceCleanupPhase::Armed
            | WorkspaceCleanupPhase::Quiescent
            | WorkspaceCleanupPhase::Removed
    ));
    let final_view = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let view = service
                .status(scope, Some(preview.cleanup_id))
                .await
                .expect("status")
                .expect("record");
            if view.phase.is_terminal() || view.phase == WorkspaceCleanupPhase::Unknown {
                break view;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("owned cleanup finishes");
    let repeated = service
        .confirm(scope, request, preview.cleanup_id, preview.plan_version)
        .await;
    let discovered = service.status(scope, None).await;
    let writer = fixture.writer.clone();
    let chat_id = fixture.owner.chat_id;
    let history = tokio::task::spawn_blocking(move || {
        writer.with_store(move |store| store.chat_execution_owner_state(chat_id))
    })
    .await
    .expect("read task")
    .expect("history")
    .expect("owner");
    jobs.shutdown().await.expect("all accepted work joined");
    peer.finish().await;
    let removed = final_view.phase == WorkspaceCleanupPhase::Removed;
    if removed {
        fixture.finish_removed().await;
    } else {
        drop(fixture);
    }
    assert_eq!(
        final_view.outcome,
        Some(WorkspaceCleanupOutcome::RemovedExecuted),
        "{final_view:?}"
    );
    assert_eq!(
        repeated.expect("same confirmation reads its receipt"),
        final_view
    );
    assert_eq!(
        discovered
            .expect("restart-discoverable status")
            .expect("record"),
        final_view
    );
    assert_eq!(history.owner.phase, ChatOwnerPhase::Finalized);
    assert!(history.owner.workspace.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit cleanup service acceptance: build the first-party trampoline"]
async fn concurrent_same_confirmation_returns_receipts_after_waiting_on_the_effect_gate() {
    let mut fixture = OwnedFixture::new().await;
    fixture.change(ChatOwnerChange::Stop).await;
    let jobs = WorkspaceJobs::new();
    let peer = Peer::clear(fixture.root.clone(), jobs.clone());
    let service = service(&fixture, &peer, jobs.clone());
    let scope = fixture_scope(&fixture);
    let preview = service.plan(scope, Uuid::new_v4()).await.expect("plan");
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    *service.inner.after_confirm_read.lock().expect("test hook") = Some(barrier.clone());
    let operation_id = Uuid::new_v4();
    let mut callers = Vec::new();
    for _ in 0..2 {
        let service = service.clone();
        let id = preview.cleanup_id;
        callers.push(tokio::spawn(async move {
            service.confirm(scope, operation_id, id, 1).await
        }));
    }
    tokio::time::timeout(Duration::from_secs(5), barrier.wait())
        .await
        .expect("both callers hold the same original Planned snapshot");
    let mut results = Vec::new();
    for caller in callers {
        results.push(caller.await.expect("caller"));
    }
    let final_view = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let view = service
                .status(scope, Some(preview.cleanup_id))
                .await
                .expect("status")
                .expect("row");
            if view.phase.is_terminal() || view.phase == WorkspaceCleanupPhase::Unknown {
                break view;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("effect completes");
    let writer = fixture.writer.clone();
    let events = tokio::task::spawn_blocking(move || writer.with_store(|store| store.events()))
        .await
        .expect("read task")
        .expect("events");
    jobs.shutdown().await.expect("joined");
    peer.finish().await;
    if final_view.phase == WorkspaceCleanupPhase::Removed {
        fixture.finish_removed().await;
    } else {
        drop(fixture);
    }
    assert!(
        results.iter().all(Result::is_ok),
        "each duplicate returns a receipt, got {results:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type().as_str() == "workspace_cleanup_armed")
            .count(),
        1
    );
    assert_eq!(
        final_view.outcome,
        Some(WorkspaceCleanupOutcome::RemovedExecuted)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit cleanup service acceptance: build the first-party trampoline"]
async fn writer_queue_saturation_keeps_retired_evidence_for_explicit_reconciliation() {
    let mut fixture = OwnedFixture::new().await;
    fixture.change(ChatOwnerChange::Stop).await;
    let jobs = WorkspaceJobs::new();
    let peer = Peer::clear(fixture.root.clone(), jobs.clone());
    let service = service(&fixture, &peer, jobs.clone());
    let scope = fixture_scope(&fixture);
    let preview = service.plan(scope, Uuid::new_v4()).await.expect("plan");
    let input = WorkspaceCleanupConfirmation {
        operation_id: Uuid::new_v4(),
        cleanup_id: preview.cleanup_id,
        expected_version: 1,
        daemon_incarnation: service.inner.incarnation,
        timestamp: OffsetDateTime::now_utc(),
    };
    let mut claimed = store_call(&fixture.writer, move |store| {
        store.confirm_workspace_cleanup(&input)
    })
    .await
    .expect("fresh unused claim");
    let proof = claimed
        .execution_claim
        .take()
        .expect("claim")
        .retire_without_spawn();
    let slot = Arc::clone(&service.inner.evidence_slots)
        .try_acquire_owned()
        .expect("reserved evidence slot");
    let custodian = service
        .retain_evidence(
            claimed.cleanup.clone(),
            PendingProof::Unspawned { claim: Some(proof) },
            slot,
        )
        .expect("owned proof");
    let (observed, read_done) = oneshot::channel();
    let (resume, continue_read) = oneshot::channel();
    *service.inner.after_evidence_read.lock().expect("hook") = Some((observed, continue_read));
    let producer = service.clone();
    let custody = Arc::clone(&custodian);
    let persistence = tokio::spawn(async move { producer.persist_evidence(custody).await });
    read_done
        .await
        .expect("actual row read completed before queue saturation");
    let (started, blocked) = oneshot::channel();
    let (release, gate) = std::sync::mpsc::channel();
    let occupied = fixture
        .writer
        .enqueue_store(move |_| {
            let _ = started.send(());
            let _ = gate.recv();
            Ok(())
        })
        .expect("occupied writer");
    blocked.await.expect("writer actually blocked");
    let mut queued = Vec::new();
    for _ in 0..crate::DEFAULT_WRITER_QUEUE_CAPACITY {
        queued.push(
            fixture
                .writer
                .enqueue_store(|_| Ok(()))
                .expect("fill exact bounded queue"),
        );
    }
    assert!(matches!(
        fixture.writer.enqueue_store(|_| Ok(())),
        Err(crate::WriterError::QueueFull)
    ));
    resume
        .send(())
        .expect("continue evidence handoff into full queue");
    let failed = persistence.await.expect("producer task");
    let retained = matches!(
        &*custodian.proof.lock().expect("proof"),
        PendingProof::Unspawned { claim: Some(_) }
    );
    release.send(()).expect("recover writer capacity");
    occupied
        .await
        .expect("writer response")
        .expect("writer done");
    for ack in queued {
        ack.await.expect("queued response").expect("queued read");
    }
    let reconciled = service
        .reconcile(
            scope,
            Uuid::new_v4(),
            preview.cleanup_id,
            claimed.cleanup.version,
        )
        .await;
    let outbox_empty = service.inner.evidence.lock().expect("outbox").is_empty();
    let present = fixture.worktree().exists();
    jobs.shutdown().await.expect("jobs joined");
    peer.finish().await;
    fixture.finish().await;
    assert_eq!(
        failed.expect_err("full writer observed"),
        "workspace_cleanup_writer_busy"
    );
    assert!(
        retained,
        "failed enqueue and later read failures cannot consume the sole proof"
    );
    assert_eq!(
        reconciled
            .expect("explicit recovery persists retired proof and settles")
            .phase,
        WorkspaceCleanupPhase::Refused
    );
    assert!(
        outbox_empty && present,
        "reconciliation performs no removal"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit cleanup service acceptance: build the first-party trampoline"]
async fn evidence_flush_deadline_stops_waiting_without_dropping_custody() {
    let mut fixture = OwnedFixture::new().await;
    fixture.change(ChatOwnerChange::Stop).await;
    let jobs = WorkspaceJobs::new();
    let peer = Peer::clear(fixture.root.clone(), jobs.clone());
    let service = service(&fixture, &peer, jobs.clone());
    let scope = fixture_scope(&fixture);
    let preview = service.plan(scope, Uuid::new_v4()).await.expect("plan");
    let input = WorkspaceCleanupConfirmation {
        operation_id: Uuid::new_v4(),
        cleanup_id: preview.cleanup_id,
        expected_version: 1,
        daemon_incarnation: service.inner.incarnation,
        timestamp: OffsetDateTime::now_utc(),
    };
    let mut claimed = store_call(&fixture.writer, move |store| {
        store.confirm_workspace_cleanup(&input)
    })
    .await
    .expect("unused claim");
    let proof = claimed
        .execution_claim
        .take()
        .expect("claim")
        .retire_without_spawn();
    let slot = Arc::clone(&service.inner.evidence_slots)
        .try_acquire_owned()
        .expect("evidence slot");
    let custodian = service
        .retain_evidence(
            claimed.cleanup.clone(),
            PendingProof::Unspawned { claim: Some(proof) },
            slot,
        )
        .expect("custody");
    let (started, blocked) = oneshot::channel();
    let (release, gate) = std::sync::mpsc::channel();
    let occupied = fixture
        .writer
        .enqueue_store(move |_| {
            let _ = started.send(());
            let _ = gate.recv();
            Ok(())
        })
        .expect("block writer");
    blocked.await.expect("callback actually occupied");
    // The evidence read is ACCEPTED behind that callback, so QueueFull handling
    // alone cannot enforce this deadline. The callback remains owned throughout.
    let outcome = tokio::time::timeout(
        Duration::from_secs(2),
        service.flush_evidence_until(Instant::now() + Duration::from_millis(30)),
    )
    .await;
    let retained = matches!(
        &*custodian.proof.lock().expect("proof"),
        PendingProof::Unspawned { claim: Some(_) }
    );
    release.send(()).expect("release occupied writer");
    occupied.await.expect("response").expect("done");
    service
        .flush_evidence_until(Instant::now() + Duration::from_secs(2))
        .await
        .expect("capacity recovered");
    let row = service
        .status(scope, Some(preview.cleanup_id))
        .await
        .expect("status")
        .expect("row");
    let settled = service
        .reconcile(scope, Uuid::new_v4(), row.cleanup_id, row.version)
        .await;
    jobs.shutdown().await.expect("joined");
    peer.finish().await;
    fixture.finish().await;
    assert_eq!(
        outcome.expect("absolute evidence deadline observed"),
        Err("workspace_cleanup_evidence_pending")
    );
    assert!(
        retained,
        "an observation timeout cannot discard the retired proof"
    );
    assert_eq!(
        settled.expect("explicit no-effect settlement").phase,
        WorkspaceCleanupPhase::Refused
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit cleanup service acceptance: build the first-party trampoline"]
async fn writer_commit_checks_monotonic_expiry_even_when_wall_clock_preview_is_live() {
    let mut fixture = OwnedFixture::new().await;
    fixture.change(ChatOwnerChange::Stop).await;
    let jobs = WorkspaceJobs::new();
    let peer = Peer::clear(fixture.root.clone(), jobs.clone());
    let service = service(&fixture, &peer, jobs.clone());
    let scope = fixture_scope(&fixture);
    let preview = service
        .plan(scope, Uuid::new_v4())
        .await
        .expect("live wall-clock preview");
    assert!(preview.can_confirm && preview.expires_at > OffsetDateTime::now_utc());
    let input = WorkspaceCleanupConfirmation {
        operation_id: Uuid::new_v4(),
        cleanup_id: preview.cleanup_id,
        expected_version: 1,
        daemon_incarnation: service.inner.incarnation,
        timestamp: OffsetDateTime::now_utc(),
    };
    let expired = Instant::now() - Duration::from_millis(1);
    let result = store_call(&fixture.writer, move |store| {
        confirm_live(store, input, expired)
    })
    .await;
    let record = service
        .status(scope, Some(preview.cleanup_id))
        .await
        .expect("status")
        .expect("record");
    jobs.shutdown().await.expect("joined");
    peer.finish().await;
    let present = fixture.worktree().exists();
    fixture.finish().await;
    assert_eq!(
        result.expect_err("writer rejects expired monotonic authority"),
        "workspace_cleanup_stale_plan"
    );
    assert_eq!(record.phase, WorkspaceCleanupPhase::Planned);
    assert!(present);
}
