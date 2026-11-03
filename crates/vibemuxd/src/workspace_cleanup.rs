//! Supervised owner cleanup. Canonical claims precede the effect's final checks.
use crate::{
    WriterHandle,
    cleanup_executable::CleanupExecutablePin,
    cleanup_paths::CleanupPaths,
    cleanup_process::{self, CleanupGitCommand, CleanupProcessOutcome, CleanupRunProblem},
    terminal_observer::{CleanupTerminalProof, TerminalObserver},
    workspace_cleanup_contract::*,
    workspace_jobs::WorkspaceJobs,
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use time::OffsetDateTime;
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore, oneshot},
    time::Instant,
};
use uuid::Uuid;
use vibemux_store::{
    ChatExecutionOwnerCommit, SqliteStore, StoreError, WorkspaceCleanupCommit,
    WorkspaceCleanupConfirmation, WorkspaceCleanupMutation, WorkspaceCleanupPlan,
    WorkspaceCleanupSettlement,
};
use vibemux_types::{
    WorkspaceCleanupId,
    chat::ChatState,
    chat_owner::{ChatOwnerPhase, ChatWorkspaceEffect},
    workspace_cleanup::*,
};
use vibemux_workspace::{CleanupObservation, WorkspaceManager};

const MAX_PREVIEWS: usize = 64;
const PREVIEW_LIFETIME: Duration = Duration::from_secs(300);
const PREPARE_LIFETIME: Duration = Duration::from_secs(60);
const MAX_PENDING_EVIDENCE: usize = 64;
const EVIDENCE_ATTEMPTS: usize = 3;
const EVIDENCE_RETRY_DELAY: Duration = Duration::from_millis(25);
// The writer has64slots and a5s SQLite busy timeout. Shutdown first joins all
// producers, then gives that finite queue a bounded opportunity to drain.
const EVIDENCE_SHUTDOWN_DEADLINE: Duration = Duration::from_secs(330);

struct Images {
    git: Arc<CleanupExecutablePin>,
    trampoline: Arc<CleanupExecutablePin>,
}
impl Images {
    fn acquire(root: &Path, trampoline: &Path) -> Result<Self, &'static str> {
        let git = crate::chat_workspace::native_git(root).map_err(|error| error.code())?;
        Ok(Self {
            git: Arc::new(CleanupExecutablePin::acquire(&git, root)?),
            trampoline: Arc::new(CleanupExecutablePin::acquire(trampoline, root)?),
        })
    }
    fn verify(&self) -> Result<(), &'static str> {
        self.git.verify()?;
        self.trampoline.verify()
    }
}

struct ServiceState {
    writer: WriterHandle,
    root: PathBuf,
    trampoline: PathBuf,
    terminal: Arc<TerminalObserver>,
    jobs: WorkspaceJobs,
    incarnation: Uuid,
    observer: Result<Option<String>, &'static str>,
    previews: Mutex<HashMap<WorkspaceCleanupId, Preview>>,
    preview_slots: Arc<Semaphore>,
    startup_error: Mutex<Option<&'static str>>,
    evidence: Mutex<HashMap<WorkspaceCleanupId, Arc<EvidenceCustodian>>>,
    evidence_slots: Arc<Semaphore>,
    #[cfg(test)]
    after_confirm_read: Mutex<Option<Arc<tokio::sync::Barrier>>>,
    #[cfg(test)]
    after_evidence_read: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
}
struct Preview {
    deadline: Instant,
    _slot: OwnedSemaphorePermit,
}
enum PendingProof {
    Unspawned {
        claim: Option<vibemux_store::CleanupNeverSpawnedClaim>,
    },
    Joined {
        claim: Option<vibemux_store::CleanupCompletionClaim>,
        executor: Box<ExecutorBinding>,
        result: CleanupProcessResult,
        proof: CleanupQuiescence,
    },
    Written,
}
struct EvidenceCustodian {
    record: WorkspaceCleanup,
    operation_id: Uuid,
    proof: Mutex<PendingProof>,
    _slot: OwnedSemaphorePermit,
}
struct Prepared {
    images: Arc<Images>,
    manager: WorkspaceManager,
    paths: Option<CleanupPaths>,
    proof: CleanupTerminalProof,
}
#[derive(Clone)]
pub(crate) struct WorkspaceCleanupService {
    inner: Arc<ServiceState>,
}

impl WorkspaceCleanupService {
    pub(crate) async fn confirm(
        &self,
        scope: WorkspaceCleanupScope,
        operation_id: Uuid,
        id: WorkspaceCleanupId,
        plan_version: u64,
    ) -> Result<WorkspaceCleanupView, &'static str> {
        if operation_id.is_nil() || plan_version != 1 {
            return Err("workspace_cleanup_invalid_request");
        }
        let (reply, early) = oneshot::channel();
        let service = self.clone();
        let job = self.inner.jobs.execute(async move {
            service
                .confirm_owned(scope, operation_id, id, plan_version, reply)
                .await
        });
        tokio::pin!(job);
        tokio::select! {biased;
            result=&mut job=>result,
            result=early=>match result {Ok(view)=>Ok(view),Err(_)=>job.await},
        }
    }
    async fn confirm_owned(
        &self,
        scope: WorkspaceCleanupScope,
        operation_id: Uuid,
        id: WorkspaceCleanupId,
        plan_version: u64,
        reply: oneshot::Sender<WorkspaceCleanupView>,
    ) -> Result<WorkspaceCleanupView, &'static str> {
        let row = self.row(scope, id).await?;
        // A committed replay uses its original binding even after restart. It
        // cannot obtain authority from the new service incarnation.
        if row.cleanup.confirmation_operation_id == Some(operation_id) {
            let input = WorkspaceCleanupConfirmation {
                operation_id,
                cleanup_id: id,
                expected_version: plan_version,
                daemon_incarnation: row.cleanup.daemon_incarnation,
                timestamp: OffsetDateTime::now_utc(),
            };
            let record = store_call(&self.inner.writer, move |store| {
                store.confirm_workspace_cleanup(&input)
            })
            .await?
            .cleanup;
            return self.view(record);
        }
        self.ready()?;
        if !self.view(row.cleanup.clone())?.can_confirm {
            return Err("workspace_cleanup_stale_plan");
        }
        #[cfg(test)]
        {
            let barrier = self
                .inner
                .after_confirm_read
                .lock()
                .map_err(|_| "workspace_cleanup_unavailable")?
                .clone();
            if let Some(barrier) = barrier {
                barrier.wait().await;
            }
        }
        let _guard = self
            .inner
            .jobs
            .write(&row.cleanup.binding.resource_key)
            .await?;
        let row = self.row(scope, id).await?;
        if row.cleanup.confirmation_operation_id == Some(operation_id) {
            let input = WorkspaceCleanupConfirmation {
                operation_id,
                cleanup_id: id,
                expected_version: plan_version,
                daemon_incarnation: row.cleanup.daemon_incarnation,
                timestamp: OffsetDateTime::now_utc(),
            };
            return self.view(
                store_call(&self.inner.writer, move |store| {
                    store.confirm_workspace_cleanup(&input)
                })
                .await?
                .cleanup,
            );
        }
        if !self.view(row.cleanup.clone())?.can_confirm {
            return Err("workspace_cleanup_stale_plan");
        }
        let input = WorkspaceCleanupConfirmation {
            operation_id,
            cleanup_id: id,
            expected_version: plan_version,
            daemon_incarnation: self.inner.incarnation,
            timestamp: OffsetDateTime::now_utc().max(row.cleanup.updated_at),
        };
        let deadline = self
            .inner
            .previews
            .lock()
            .map_err(|_| "workspace_cleanup_unavailable")?
            .get(&id)
            .ok_or("workspace_cleanup_stale_plan")?
            .deadline;
        let evidence_slot = Arc::clone(&self.inner.evidence_slots)
            .try_acquire_owned()
            .map_err(|_| "workspace_cleanup_evidence_pending")?;
        let mut claimed = store_call(&self.inner.writer, move |store| {
            confirm_live(store, input, deadline)
        })
        .await?;
        let view = self.view(claimed.cleanup.clone())?;
        let _ = reply.send(view.clone());
        let Some(claim) = claimed.execution_claim.take() else {
            return Ok(view);
        };
        if let Ok(mut previews) = self.inner.previews.lock() {
            previews.remove(&id);
        }
        let result = self
            .execute_claim(claimed.cleanup, claim, evidence_slot)
            .await;
        match result {
            Ok(record) => self.view(record),
            Err(code) => {
                let _ = self.mark_unknown(scope, id, code).await;
                Err(code)
            }
        }
    }

    async fn prepare(&self, record: &WorkspaceCleanup) -> Result<Prepared, &'static str> {
        if self.observer()? != record.binding.terminal_plugin_id {
            return Err("workspace_cleanup_terminal_changed");
        }
        let owner = self.owner(scope(record)).await?;
        if owner.owner.version != record.binding.owner_version
            || owner.session.generation != record.binding.session_generation
            || owner.session.version != record.binding.session_version
            || owner.owner.workspace.as_ref() != Some(&record.binding.workspace)
            || owner.owner.resource_key.as_ref() != Some(&record.binding.resource_key)
        {
            return Err("workspace_cleanup_changed");
        }
        let root = self.inner.root.clone();
        let trampoline = self.inner.trampoline.clone();
        let images = Arc::new(
            tokio::task::spawn_blocking(move || Images::acquire(&root, &trampoline))
                .await
                .map_err(|_| "workspace_cleanup_unavailable")??,
        );
        path_text(images.git.canonical_path())?;
        path_text(images.trampoline.canonical_path())?;
        let manager = WorkspaceManager::open_existing(
            images.git.canonical_path().to_path_buf(),
            self.inner.root.clone(),
            record.binding.workspace.base_commit.clone(),
        )
        .await
        .map_err(|error| error.code())?;
        let observation = manager
            .observe_cleanup(&record.binding.workspace)
            .await
            .map_err(|error| error.code())?;
        compare_observation(record, &observation)?;
        if physical_key(&record.binding.workspace.path).await? != record.binding.resource_key {
            return Err("workspace_cleanup_changed");
        }
        let paths = capture_paths(&observation, &record.binding.workspace).await?;
        if !paths.matches(&record.binding) {
            return Err("workspace_cleanup_changed");
        }
        let proof = self
            .inner
            .terminal
            .cleanup_clear(
                &record.binding.terminal_plugin_id,
                record.binding.run_id,
                &record.binding.workspace.path,
            )
            .await?;
        if proof.session_id != record.binding.session_id
            || proof.instance_id != record.binding.instance_id
        {
            return Err("workspace_cleanup_terminal_changed");
        }
        Ok(Prepared {
            images,
            manager,
            paths: Some(paths),
            proof,
        })
    }

    async fn execute_claim(
        &self,
        mut record: WorkspaceCleanup,
        claim: vibemux_store::CleanupExecutionClaim,
        evidence_slot: OwnedSemaphorePermit,
    ) -> Result<WorkspaceCleanup, &'static str> {
        let started = Instant::now();
        let mut prepared = match self.prepare(&record).await {
            Ok(prepared) => prepared,
            Err(code) => {
                return self
                    .refuse_unspawned(record, claim.retire_without_spawn(), code, evidence_slot)
                    .await;
            }
        };
        let cancel = self.inner.jobs.cancellation();
        if *cancel.borrow() || started.elapsed() > PREPARE_LIFETIME {
            return self
                .refuse_unspawned(
                    record,
                    claim.retire_without_spawn(),
                    "workspace_cleanup_cancelled",
                    evidence_slot,
                )
                .await;
        }
        let nonce = Uuid::new_v4();
        let (registration, completion) = claim.begin_spawn(nonce).map_err(|error| error.code())?;
        let mut registration = Some(registration);
        let mut executor = None;
        let command = CleanupGitCommand {
            git: prepared.images.git.canonical_path().to_path_buf(),
            trampoline: prepared.images.trampoline.canonical_path().to_path_buf(),
            repository: PathBuf::from(&record.binding.repository_root),
            worktree: PathBuf::from(&record.binding.workspace.path),
        };
        let outcome = cleanup_process::run(
            command,
            |pid| {
                let prepared = &mut prepared;
                let record = &mut record;
                let executor = &mut executor;
                let registration = &mut registration;
                let cancel = &cancel;
                async move {
                    let images = Arc::clone(&prepared.images);
                    let root = self.inner.root.clone();
                    let incarnation = record.daemon_incarnation;
                    let observed = tokio::task::spawn_blocking(move || {
                        executor_binding(pid, nonce, incarnation, &images, &root)
                    })
                    .await
                    .map_err(|_| "workspace_cleanup_identity_unavailable")??;
                    *executor = Some(observed.clone());
                    if *cancel.borrow() || started.elapsed() > PREPARE_LIFETIME {
                        return Err("workspace_cleanup_cancelled");
                    }
                    let input = mutation(record);
                    let authority = registration
                        .take()
                        .ok_or("workspace_cleanup_claim_unavailable")?;
                    let registered = store_call(&self.inner.writer, move |store| {
                        store.record_workspace_cleanup_executor(&input, authority, &observed)
                    })
                    .await?;
                    *record = registered.commit.cleanup;
                    let authority = registered
                        .arm_claim
                        .ok_or("workspace_cleanup_claim_unavailable")?;
                    if *cancel.borrow() || started.elapsed() > PREPARE_LIFETIME {
                        return Err("workspace_cleanup_cancelled");
                    }
                    let input = mutation(record);
                    let armed = store_call(&self.inner.writer, move |store| {
                        store.arm_workspace_cleanup(&input, authority)
                    })
                    .await?;
                    *record = armed.commit.cleanup;
                    let permit = armed
                        .release_permit
                        .ok_or("workspace_cleanup_claim_unavailable")?;
                    let observation = prepared
                        .manager
                        .observe_cleanup(&record.binding.workspace)
                        .await
                        .map_err(|error| error.code())?;
                    compare_observation(record, &observation)?;
                    let paths = prepared
                        .paths
                        .take()
                        .ok_or("workspace_cleanup_directory_unavailable")?;
                    let images = Arc::clone(&prepared.images);
                    prepared.paths = Some(
                        tokio::task::spawn_blocking(move || {
                            images.verify()?;
                            paths.verify_before_release()?;
                            Ok::<_, &'static str>(paths)
                        })
                        .await
                        .map_err(|_| "workspace_cleanup_unavailable")??,
                    );
                    self.inner
                        .terminal
                        .recheck_cleanup_clear(
                            &prepared.proof,
                            record.binding.run_id,
                            &record.binding.workspace.path,
                        )
                        .await?;
                    if *cancel.borrow() || started.elapsed() > PREPARE_LIFETIME {
                        return Err("workspace_cleanup_cancelled");
                    }
                    prepared
                        .paths
                        .as_mut()
                        .ok_or("workspace_cleanup_directory_unavailable")?
                        .release_target();
                    permit
                        .consume(nonce, record.daemon_incarnation)
                        .map_err(|error| error.code())
                }
            },
            cancel.clone(),
        )
        .await;
        let (receipt, problem) = match outcome {
            CleanupProcessOutcome::Finished { receipt, problem } => (receipt, problem),
            CleanupProcessOutcome::SpawnFailed(error) => {
                return Err(match error {
                    vibemux_harness::dispatch::DispatchError::ContainmentFailed => {
                        "workspace_cleanup_containment_unconfirmed"
                    }
                    vibemux_harness::dispatch::DispatchError::PipeFailed => {
                        "workspace_cleanup_pipe_unconfirmed"
                    }
                    vibemux_harness::dispatch::DispatchError::InvalidRequest => {
                        "workspace_cleanup_command_invalid"
                    }
                    _ => "workspace_cleanup_spawn_unconfirmed",
                });
            }
        };
        let Some(executor) = executor else {
            return Err("workspace_cleanup_identity_unavailable");
        };
        let result = CleanupProcessResult {
            executor_nonce: nonce,
            exit_code: receipt.exit.exit_code,
            forced_termination: receipt.exit.forced_termination,
            timed_out: receipt.timed_out,
            release_attempted: receipt.release_attempted,
        };
        let proof = CleanupQuiescence::JoinedProcess {
            executor_nonce: nonce,
            termination_succeeded: receipt.termination_succeeded,
            root_reaped: receipt.root_reaped,
            tree_empty: receipt.tree_empty,
            stdout_drained: receipt.stdout_drained,
            stderr_drained: receipt.stderr_drained,
        };
        let record = self
            .persist_joined(
                record,
                completion,
                executor,
                result.clone(),
                proof.clone(),
                evidence_slot,
            )
            .await?;
        if !proof.is_positive() {
            return Ok(record);
        }
        let code = problem.as_ref().map(problem_code);
        if !receipt.release_attempted {
            return self
                .settle(
                    record,
                    WorkspaceCleanupSettlement::Refused {
                        error_code: code.unwrap_or("workspace_cleanup_not_released").into(),
                    },
                )
                .await;
        }
        if code.is_some() || !result.is_successful() {
            return self
                .settle(
                    record,
                    WorkspaceCleanupSettlement::Unknown {
                        error_code: code.unwrap_or("workspace_cleanup_process_failed").into(),
                    },
                )
                .await;
        }
        prepared.paths = Some(
            verify_paths(
                prepared
                    .paths
                    .take()
                    .ok_or("workspace_cleanup_directory_unavailable")?,
            )
            .await?,
        );
        if !prepared
            .manager
            .observe_removed(&record.binding.workspace, &record.binding.head_commit)
            .await
            .map_err(|error| error.code())?
        {
            return self
                .settle(
                    record,
                    WorkspaceCleanupSettlement::Unknown {
                        error_code: "workspace_cleanup_postcondition_mismatch".into(),
                    },
                )
                .await;
        }
        prepared.paths = Some(
            verify_paths(
                prepared
                    .paths
                    .take()
                    .ok_or("workspace_cleanup_directory_unavailable")?,
            )
            .await?,
        );
        self.settle(record, WorkspaceCleanupSettlement::RemovedExecuted)
            .await
    }

    async fn persist_joined(
        &self,
        record: WorkspaceCleanup,
        claim: vibemux_store::CleanupCompletionClaim,
        executor: ExecutorBinding,
        result: CleanupProcessResult,
        proof: CleanupQuiescence,
        slot: OwnedSemaphorePermit,
    ) -> Result<WorkspaceCleanup, &'static str> {
        let claim = claim.retire_for_persistence();
        let custodian = self.retain_evidence(
            record,
            PendingProof::Joined {
                claim: Some(claim),
                executor: Box::new(executor),
                result,
                proof,
            },
            slot,
        )?;
        self.persist_evidence(custodian).await
    }
    async fn refuse_unspawned(
        &self,
        record: WorkspaceCleanup,
        proof: vibemux_store::CleanupNeverSpawnedClaim,
        code: &'static str,
        slot: OwnedSemaphorePermit,
    ) -> Result<WorkspaceCleanup, &'static str> {
        let custodian =
            self.retain_evidence(record, PendingProof::Unspawned { claim: Some(proof) }, slot)?;
        let quiet = self.persist_evidence(custodian).await?;
        self.settle(
            quiet,
            WorkspaceCleanupSettlement::Refused {
                error_code: code.into(),
            },
        )
        .await
    }

    fn retain_evidence(
        &self,
        record: WorkspaceCleanup,
        proof: PendingProof,
        slot: OwnedSemaphorePermit,
    ) -> Result<Arc<EvidenceCustodian>, &'static str> {
        let id = record.id;
        let custodian = Arc::new(EvidenceCustodian {
            record,
            operation_id: Uuid::new_v4(),
            proof: Mutex::new(proof),
            _slot: slot,
        });
        self.inner
            .evidence
            .lock()
            .map_err(|_| "workspace_cleanup_evidence_unavailable")?
            .insert(id, Arc::clone(&custodian));
        Ok(custodian)
    }
    async fn persist_evidence(
        &self,
        custodian: Arc<EvidenceCustodian>,
    ) -> Result<WorkspaceCleanup, &'static str> {
        let mut code = "workspace_cleanup_evidence_pending";
        for attempt in 0..EVIDENCE_ATTEMPTS {
            let current = match self
                .row(scope(&custodian.record), custodian.record.id)
                .await
            {
                Ok(row) => row.cleanup,
                Err(error) => {
                    code = error;
                    if attempt + 1 < EVIDENCE_ATTEMPTS {
                        tokio::time::sleep(EVIDENCE_RETRY_DELAY).await;
                    }
                    continue;
                }
            };
            let already_written = {
                #[cfg(test)]
                {
                    let hook = {
                        self.inner
                            .after_evidence_read
                            .lock()
                            .map_err(|_| "workspace_cleanup_unavailable")?
                            .take()
                    };
                    if let Some((ready, resume)) = hook {
                        let _ = ready.send(());
                        let _ = resume.await;
                    }
                }
                let state = custodian
                    .proof
                    .try_lock()
                    .map_err(|_| "workspace_cleanup_evidence_pending")?;
                match &*state {
                    PendingProof::Written => true,
                    PendingProof::Joined {
                        executor,
                        result,
                        proof,
                        ..
                    } => {
                        current.executor.as_ref() == Some(executor.as_ref())
                            && current.process_result.as_ref() == Some(result)
                            && current.quiescence.as_ref() == Some(proof)
                    }
                    PendingProof::Unspawned { .. } => {
                        current.claim_id == custodian.record.claim_id
                            && matches!(current.quiescence,Some(CleanupQuiescence::NeverSpawned {retired_claim_id}) if Some(retired_claim_id)==current.claim_id)
                    }
                }
            };
            if already_written {
                self.inner
                    .evidence
                    .lock()
                    .map_err(|_| "workspace_cleanup_evidence_unavailable")?
                    .remove(&current.id);
                return Ok(current);
            }
            let mut input = mutation(&current);
            input.operation_id = custodian.operation_id;
            let held = Arc::clone(&custodian);
            // The queued closure owns only an Arc. A failed try_send cannot drop
            // the sole proof; transfer occurs on the authoritative writer thread.
            let written = store_call(&self.inner.writer, move |store| {
                let mut state = held.proof.lock().map_err(|_| {
                    StoreError::WorkspaceCleanup("workspace_cleanup_evidence_unavailable")
                })?;
                let result = match &mut *state {
                    PendingProof::Unspawned { claim } => {
                        let authority = claim.take().ok_or(StoreError::WorkspaceCleanup(
                            "workspace_cleanup_evidence_pending",
                        ))?;
                        match store.record_workspace_cleanup_never_spawned(&input, authority) {
                            Ok(commit) => Ok(commit.cleanup),
                            Err(failure) => {
                                let (error, proof) = failure.into_parts();
                                *claim = Some(proof);
                                Err(error)
                            }
                        }
                    }
                    PendingProof::Joined {
                        claim,
                        executor,
                        result,
                        proof,
                    } => {
                        let authority = claim.take().ok_or(StoreError::WorkspaceCleanup(
                            "workspace_cleanup_evidence_pending",
                        ))?;
                        match store.record_workspace_cleanup_joined(
                            &input,
                            authority,
                            executor,
                            Some(result),
                            proof,
                        ) {
                            Ok(commit) => Ok(commit.cleanup),
                            Err(failure) => {
                                let (error, proof) = failure.into_parts();
                                *claim = Some(proof);
                                Err(error)
                            }
                        }
                    }
                    PendingProof::Written => store
                        .workspace_cleanup(input.cleanup_id)?
                        .map(|commit| commit.cleanup)
                        .ok_or(StoreError::WorkspaceCleanup("workspace_cleanup_not_found")),
                };
                if result.is_ok() {
                    *state = PendingProof::Written;
                }
                result
            })
            .await;
            match written {
                Ok(record) => {
                    self.inner
                        .evidence
                        .lock()
                        .map_err(|_| "workspace_cleanup_evidence_unavailable")?
                        .remove(&record.id);
                    return Ok(record);
                }
                Err(error) => code = error,
            }
            if attempt + 1 < EVIDENCE_ATTEMPTS {
                tokio::time::sleep(EVIDENCE_RETRY_DELAY).await;
            }
        }
        Err(code)
    }
    pub(crate) async fn flush_pending_evidence(&self) -> Result<(), &'static str> {
        let deadline = Instant::now() + EVIDENCE_SHUTDOWN_DEADLINE;
        self.flush_evidence_until(deadline).await
    }
    async fn flush_evidence_until(&self, deadline: Instant) -> Result<(), &'static str> {
        loop {
            let pending = self
                .inner
                .evidence
                .lock()
                .map_err(|_| "workspace_cleanup_evidence_unavailable")?
                .values()
                .cloned()
                .collect::<Vec<_>>();
            if pending.is_empty() {
                return Ok(());
            }
            let mut busy = false;
            for custodian in pending {
                if Instant::now() >= deadline {
                    return Err("workspace_cleanup_evidence_pending");
                }
                let persisted = tokio::time::timeout_at(deadline, self.persist_evidence(custodian))
                    .await
                    .map_err(|_| "workspace_cleanup_evidence_pending")?;
                if let Err(code) = persisted {
                    if code != "workspace_cleanup_writer_busy" {
                        return Err(code);
                    }
                    busy = true;
                }
            }
            if !busy {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("workspace_cleanup_evidence_pending");
            }
            tokio::time::sleep_until((Instant::now() + EVIDENCE_RETRY_DELAY).min(deadline)).await;
        }
    }
    async fn settle(
        &self,
        record: WorkspaceCleanup,
        outcome: WorkspaceCleanupSettlement,
    ) -> Result<WorkspaceCleanup, &'static str> {
        let input = mutation(&record);
        store_call(&self.inner.writer, move |store| {
            store.settle_workspace_cleanup(&input, &outcome)
        })
        .await
        .map(|commit| commit.cleanup)
    }
    async fn mark_unknown(
        &self,
        scope: WorkspaceCleanupScope,
        id: WorkspaceCleanupId,
        code: &'static str,
    ) -> Result<(), &'static str> {
        let current = self.row(scope, id).await?.cleanup;
        if current.phase.is_terminal() {
            return Ok(());
        }
        self.settle(
            current,
            WorkspaceCleanupSettlement::Unknown {
                error_code: code.into(),
            },
        )
        .await
        .map(|_| ())
    }

    pub(crate) async fn reconcile(
        &self,
        scope: WorkspaceCleanupScope,
        operation_id: Uuid,
        id: WorkspaceCleanupId,
        expected_version: u64,
    ) -> Result<WorkspaceCleanupView, &'static str> {
        if operation_id.is_nil() || expected_version == 0 {
            return Err("workspace_cleanup_invalid_request");
        }
        let service = self.clone();
        self.inner
            .jobs
            .execute(async move {
                service
                    .reconcile_owned(scope, operation_id, id, expected_version)
                    .await
            })
            .await
    }
    async fn reconcile_owned(
        &self,
        scope: WorkspaceCleanupScope,
        operation_id: Uuid,
        id: WorkspaceCleanupId,
        expected_version: u64,
    ) -> Result<WorkspaceCleanupView, &'static str> {
        let row = self.row(scope, id).await?;
        let _guard = self
            .inner
            .jobs
            .write(&row.cleanup.binding.resource_key)
            .await?;
        let before_flush = self.row(scope, id).await?.cleanup;
        if !before_flush.phase.is_terminal() && before_flush.version != expected_version {
            return Err("workspace_cleanup_conflict");
        }
        let pending = self
            .inner
            .evidence
            .lock()
            .map_err(|_| "workspace_cleanup_evidence_unavailable")?
            .get(&id)
            .cloned();
        if let Some(pending) = pending {
            self.persist_evidence(pending).await?;
        }
        let row = self.row(scope, id).await?;
        let record = row.cleanup;
        // A terminal receipt is historical and immutable; this is a read.
        if record.phase.is_terminal() {
            return self.view(record);
        }
        let expected_version = record.version;
        if !matches!(
            record.phase,
            WorkspaceCleanupPhase::Unknown | WorkspaceCleanupPhase::Quiescent
        ) || !record
            .quiescence
            .as_ref()
            .is_some_and(CleanupQuiescence::is_positive)
        {
            return Err("workspace_cleanup_quiescence_required");
        }
        self.owner(scope).await?;
        if matches!(
            record.quiescence,
            Some(CleanupQuiescence::NeverSpawned { .. })
        ) || record
            .process_result
            .as_ref()
            .is_some_and(|result| !result.release_attempted)
        {
            let input = WorkspaceCleanupMutation {
                operation_id,
                cleanup_id: id,
                expected_version: record.version,
                timestamp: OffsetDateTime::now_utc().max(record.updated_at),
            };
            let commit = store_call(&self.inner.writer, move |store| {
                store.reconcile_workspace_cleanup(
                    &input,
                    &WorkspaceCleanupSettlement::Refused {
                        error_code: "workspace_cleanup_not_started".into(),
                    },
                )
            })
            .await?;
            return self.view(commit.cleanup);
        }
        let root = self.inner.root.clone();
        let git = tokio::task::spawn_blocking(move || {
            let path = crate::chat_workspace::native_git(&root).map_err(|error| error.code())?;
            CleanupExecutablePin::acquire(&path, &root)
        })
        .await
        .map_err(|_| "workspace_cleanup_unavailable")??;
        let manager = WorkspaceManager::open_existing(
            git.canonical_path().to_path_buf(),
            self.inner.root.clone(),
            record.binding.workspace.base_commit.clone(),
        )
        .await
        .map_err(|error| error.code())?;
        let (repository, common) = manager.cleanup_repository_paths();
        if path_text(repository)? != record.binding.repository_root
            || path_text(common)? != record.binding.common_git_dir
        {
            return Err("workspace_cleanup_changed");
        }
        let binding = record.binding.clone();
        let pins = tokio::task::spawn_blocking(move || CleanupPaths::remaining(&binding))
            .await
            .map_err(|_| "workspace_cleanup_unavailable")??;
        let observed = manager
            .observe_removed(&record.binding.workspace, &record.binding.head_commit)
            .await
            .map_err(|error| error.code())?;
        let outcome = if observed {
            WorkspaceCleanupSettlement::RemovedObserved
        } else {
            // A present clean original object/registration can establish a
            // no-removal refusal. A moved, replaced, dirty or partial scope
            // remains Unknown; neither PID absence nor elapsed time helps.
            let observation = manager
                .observe_cleanup(&record.binding.workspace)
                .await
                .map_err(|error| error.code())?;
            compare_observation(&record, &observation)?;
            let current = capture_paths(&observation, &record.binding.workspace).await?;
            if !current.matches(&record.binding) {
                return Err("workspace_cleanup_changed");
            }
            WorkspaceCleanupSettlement::Refused {
                error_code: "workspace_cleanup_not_removed".into(),
            }
        };
        let (_git, _pins) = tokio::task::spawn_blocking(move || {
            git.verify()?;
            pins.verify_roots()?;
            Ok::<_, &'static str>((git, pins))
        })
        .await
        .map_err(|_| "workspace_cleanup_unavailable")??;
        let input = WorkspaceCleanupMutation {
            operation_id,
            cleanup_id: id,
            expected_version,
            timestamp: OffsetDateTime::now_utc().max(record.updated_at),
        };
        let commit = store_call(&self.inner.writer, move |store| {
            store.reconcile_workspace_cleanup(&input, &outcome)
        })
        .await?;
        self.view(commit.cleanup)
    }
    pub(crate) fn new(
        writer: WriterHandle,
        root: PathBuf,
        terminal: Arc<TerminalObserver>,
        jobs: WorkspaceJobs,
        observers: Vec<String>,
    ) -> Self {
        let observer = match observers.len() {
            0 => Ok(None),
            1 => Ok(observers.into_iter().next()),
            _ => Err("workspace_cleanup_terminal_ambiguous"),
        };
        let trampoline = std::env::current_exe()
            .ok()
            .and_then(|path| {
                path.parent()
                    .map(|parent| parent.join("vibemux_launch_trampoline.exe"))
            })
            .unwrap_or_default();
        Self {
            inner: Arc::new(ServiceState {
                writer,
                root,
                trampoline,
                terminal,
                jobs,
                incarnation: Uuid::new_v4(),
                observer,
                previews: Mutex::new(HashMap::new()),
                preview_slots: Arc::new(Semaphore::new(MAX_PREVIEWS)),
                startup_error: Mutex::new(None),
                evidence: Mutex::new(HashMap::new()),
                evidence_slots: Arc::new(Semaphore::new(MAX_PENDING_EVIDENCE)),
                #[cfg(test)]
                after_confirm_read: Mutex::new(None),
                #[cfg(test)]
                after_evidence_read: Mutex::new(None),
            }),
        }
    }

    /// Startup records uncertainty; it never recreates an executor capability.
    pub(crate) async fn recover(&self) {
        let result = self.recover_rows().await;
        if let Err(code) = result
            && let Ok(mut error) = self.inner.startup_error.lock()
        {
            *error = Some(code);
        }
    }
    async fn recover_rows(&self) -> Result<(), &'static str> {
        let mut after = None;
        loop {
            let page = store_call(&self.inner.writer, move |store| {
                store.workspace_cleanup_pending(after, 64)
            })
            .await?;
            if page.is_empty() {
                return Ok(());
            }
            for row in page {
                after = Some(row.cleanup.id);
                if row.cleanup.daemon_incarnation != self.inner.incarnation
                    && matches!(
                        row.cleanup.phase,
                        WorkspaceCleanupPhase::Claimed | WorkspaceCleanupPhase::Armed
                    )
                {
                    let mutation = mutation(&row.cleanup);
                    store_call(&self.inner.writer, move |store| {
                        store.settle_workspace_cleanup(
                            &mutation,
                            &WorkspaceCleanupSettlement::Unknown {
                                error_code: "workspace_cleanup_daemon_restarted".into(),
                            },
                        )
                    })
                    .await?;
                }
            }
        }
    }
    fn ready(&self) -> Result<(), &'static str> {
        if let Some(error) = *self
            .inner
            .startup_error
            .lock()
            .map_err(|_| "workspace_cleanup_unavailable")?
        {
            return Err(error);
        }
        Ok(())
    }
    fn observer(&self) -> Result<String, &'static str> {
        self.inner
            .observer
            .clone()?
            .ok_or("workspace_cleanup_terminal_unavailable")
    }

    pub(crate) async fn status(
        &self,
        scope: WorkspaceCleanupScope,
        id: Option<WorkspaceCleanupId>,
    ) -> Result<Option<WorkspaceCleanupView>, &'static str> {
        if let Some(id) = id {
            return self.view(self.row(scope, id).await?.cleanup).map(Some);
        }
        scope.validate()?;
        let row = store_call(&self.inner.writer, move |store| {
            if store.project_id()? != Some(scope.project_id) {
                return Err(StoreError::WorkspaceCleanup(
                    "workspace_cleanup_scope_mismatch",
                ));
            }
            let owner = store
                .chat_execution_owner_for_run(scope.run_id)?
                .ok_or(StoreError::WorkspaceCleanup("workspace_cleanup_not_found"))?;
            if owner.owner.task_id != scope.task_id {
                return Err(StoreError::WorkspaceCleanup(
                    "workspace_cleanup_scope_mismatch",
                ));
            }
            store.workspace_cleanup_for_run(scope.run_id)
        })
        .await?;
        row.map(|row| {
            require_scope(&row.cleanup, scope)?;
            self.view(row.cleanup)
        })
        .transpose()
    }
    async fn row(
        &self,
        scope: WorkspaceCleanupScope,
        id: WorkspaceCleanupId,
    ) -> Result<WorkspaceCleanupCommit, &'static str> {
        scope.validate()?;
        let row = store_call(&self.inner.writer, move |store| {
            if store.project_id()? != Some(scope.project_id) {
                return Err(StoreError::WorkspaceCleanup(
                    "workspace_cleanup_scope_mismatch",
                ));
            }
            store
                .workspace_cleanup(id)?
                .ok_or(StoreError::WorkspaceCleanup("workspace_cleanup_not_found"))
        })
        .await?;
        require_scope(&row.cleanup, scope)?;
        Ok(row)
    }
    fn view(&self, record: WorkspaceCleanup) -> Result<WorkspaceCleanupView, &'static str> {
        let eligible = record.phase == WorkspaceCleanupPhase::Planned
            && record.version == 1
            && record.daemon_incarnation == self.inner.incarnation
            && OffsetDateTime::now_utc() < record.expires_at
            && self
                .inner
                .previews
                .lock()
                .map_err(|_| "workspace_cleanup_unavailable")?
                .get(&record.id)
                .is_some_and(|preview| Instant::now() < preview.deadline);
        let scope = scope(&record);
        let view = WorkspaceCleanupView {
            schema_version: WORKSPACE_CLEANUP_VIEW_VERSION,
            scope,
            cleanup_id: record.id,
            phase: record.phase,
            version: record.version,
            plan_version: 1,
            branch: record.binding.workspace.branch,
            can_confirm: eligible,
            expires_at: record.expires_at,
            updated_at: record.updated_at,
            outcome: record.outcome,
            error_code: record.error_code,
        };
        view.validate_for(scope)?;
        Ok(view)
    }
    pub(crate) async fn plan(
        &self,
        scope: WorkspaceCleanupScope,
        operation_id: Uuid,
    ) -> Result<WorkspaceCleanupView, &'static str> {
        scope.validate()?;
        if operation_id.is_nil() {
            return Err("workspace_cleanup_invalid_request");
        }
        let service = self.clone();
        self.inner
            .jobs
            .execute(async move { service.plan_owned(scope, operation_id).await })
            .await
    }
    async fn plan_owned(
        &self,
        scope: WorkspaceCleanupScope,
        operation_id: Uuid,
    ) -> Result<WorkspaceCleanupView, &'static str> {
        self.ready()?;
        if let Some(row) = store_call(&self.inner.writer, move |store| {
            store.workspace_cleanup_for_plan_operation(operation_id)
        })
        .await?
        {
            require_scope(&row.cleanup, scope)?;
            return self.view(row.cleanup);
        }
        let observer = self.observer()?;
        let owner = self.owner(scope).await?;
        let key = owner
            .owner
            .resource_key
            .clone()
            .ok_or("workspace_cleanup_owner_unsettled")?;
        let _guard = self.inner.jobs.write(&key).await?;
        if let Some(row) = store_call(&self.inner.writer, move |store| {
            store.workspace_cleanup_for_plan_operation(operation_id)
        })
        .await?
        {
            require_scope(&row.cleanup, scope)?;
            return self.view(row.cleanup);
        }
        let owner = self.owner(scope).await?;
        let workspace = owner
            .owner
            .workspace
            .clone()
            .ok_or("workspace_cleanup_owner_unsettled")?;
        let root = self.inner.root.clone();
        let trampoline = self.inner.trampoline.clone();
        let images = tokio::task::spawn_blocking(move || Images::acquire(&root, &trampoline))
            .await
            .map_err(|_| "workspace_cleanup_unavailable")??;
        let manager = WorkspaceManager::open_existing(
            images.git.canonical_path().to_path_buf(),
            self.inner.root.clone(),
            workspace.base_commit.clone(),
        )
        .await
        .map_err(|error| error.code())?;
        let observation = manager
            .observe_cleanup(&workspace)
            .await
            .map_err(|error| error.code())?;
        if physical_key(&workspace.path).await? != key {
            return Err("workspace_cleanup_changed");
        }
        let paths = capture_paths(&observation, &workspace).await?;
        let proof = self
            .inner
            .terminal
            .cleanup_clear(&observer, scope.run_id, &workspace.path)
            .await?;
        let (repository_identity, common_directory_identity, worktree_identity) =
            paths.identities()?;
        let binding = WorkspaceCleanupBinding {
            project_id: scope.project_id,
            chat_id: owner.owner.chat_id,
            task_id: scope.task_id,
            run_id: scope.run_id,
            owner_version: owner.owner.version,
            session_generation: owner.session.generation,
            session_version: owner.session.version,
            workspace,
            resource_key: key,
            repository_root: path_text(&observation.repository_root)?,
            common_git_dir: path_text(&observation.common_git_dir)?,
            repository_identity,
            common_directory_identity,
            worktree_identity,
            head_commit: observation.head_commit,
            terminal_plugin_id: proof.plugin_id,
            session_id: proof.session_id,
            instance_id: proof.instance_id,
        };
        {
            let mut previews = self
                .inner
                .previews
                .lock()
                .map_err(|_| "workspace_cleanup_unavailable")?;
            previews.retain(|_, preview| preview.deadline > Instant::now());
        }
        let slot = Arc::clone(&self.inner.preview_slots)
            .try_acquire_owned()
            .map_err(|_| "workspace_cleanup_capacity")?;
        let input = WorkspaceCleanupPlan {
            operation_id,
            daemon_incarnation: self.inner.incarnation,
            binding,
            timestamp: OffsetDateTime::now_utc()
                .max(owner.owner.updated_at)
                .max(owner.session.updated_at),
        };
        let record = store_call(&self.inner.writer, move |store| {
            store.plan_workspace_cleanup(&input)
        })
        .await?
        .cleanup;
        self.inner
            .previews
            .lock()
            .map_err(|_| "workspace_cleanup_unavailable")?
            .entry(record.id)
            .or_insert(Preview {
                deadline: Instant::now() + PREVIEW_LIFETIME,
                _slot: slot,
            });
        self.view(record)
    }
    async fn owner(
        &self,
        scope: WorkspaceCleanupScope,
    ) -> Result<ChatExecutionOwnerCommit, &'static str> {
        let owner = store_call(&self.inner.writer, move |store| {
            if store.project_id()? != Some(scope.project_id) {
                return Err(StoreError::WorkspaceCleanup(
                    "workspace_cleanup_scope_mismatch",
                ));
            }
            store
                .chat_execution_owner_for_run(scope.run_id)?
                .ok_or(StoreError::WorkspaceCleanup("workspace_cleanup_not_found"))
        })
        .await?;
        if owner.owner.project_id != scope.project_id
            || owner.owner.task_id != scope.task_id
            || owner.owner.run_id != scope.run_id
        {
            return Err("workspace_cleanup_scope_mismatch");
        }
        if owner.owner.phase != ChatOwnerPhase::Finalized
            || owner.owner.workspace_effect != Some(ChatWorkspaceEffect::Settled)
            || owner.owner.workspace_verified_at.is_none()
            || owner.owner.error_code.is_some()
            || owner.session.state != ChatState::Closed
            || owner.session.active_request_id.is_some()
            || owner.process_claimed
        {
            return Err("workspace_cleanup_owner_unsettled");
        }
        Ok(owner)
    }
}

pub(crate) async fn store_call<T: Send + 'static, F>(
    writer: &WriterHandle,
    action: F,
) -> Result<T, &'static str>
where
    F: FnOnce(&mut SqliteStore) -> Result<T, StoreError> + Send + 'static,
{
    writer
        .enqueue_store(move |store| Ok(action(store).map_err(|error| error.code())))
        .map_err(|error| {
            if matches!(error, crate::WriterError::QueueFull) {
                "workspace_cleanup_writer_busy"
            } else {
                "workspace_cleanup_writer_unavailable"
            }
        })?
        .await
        .map_err(|_| "workspace_cleanup_writer_unavailable")?
        .map_err(|_| "workspace_cleanup_writer_unavailable")?
}
fn scope(record: &WorkspaceCleanup) -> WorkspaceCleanupScope {
    WorkspaceCleanupScope {
        project_id: record.binding.project_id,
        task_id: record.binding.task_id,
        run_id: record.binding.run_id,
    }
}
fn require_scope(
    record: &WorkspaceCleanup,
    expected: WorkspaceCleanupScope,
) -> Result<(), &'static str> {
    if scope(record) != expected {
        Err("workspace_cleanup_scope_mismatch")
    } else {
        Ok(())
    }
}
fn mutation(record: &WorkspaceCleanup) -> WorkspaceCleanupMutation {
    WorkspaceCleanupMutation {
        operation_id: Uuid::new_v4(),
        cleanup_id: record.id,
        expected_version: record.version,
        timestamp: OffsetDateTime::now_utc().max(record.updated_at),
    }
}

fn confirm_live(
    store: &mut SqliteStore,
    mut input: WorkspaceCleanupConfirmation,
    deadline: Instant,
) -> Result<WorkspaceCleanupCommit, StoreError> {
    if Instant::now() >= deadline {
        return Err(StoreError::WorkspaceCleanup("workspace_cleanup_stale_plan"));
    }
    input.timestamp = OffsetDateTime::now_utc().max(input.timestamp);
    store.confirm_workspace_cleanup(&input)
}
fn path_text(path: &Path) -> Result<String, &'static str> {
    crate::harness_dispatch::child_path_text(path).ok_or("workspace_cleanup_directory_unavailable")
}
async fn capture_paths(
    observation: &CleanupObservation,
    workspace: &vibemux_types::a2a::RunWorkspace,
) -> Result<CleanupPaths, &'static str> {
    let observation = CleanupObservation {
        repository_root: observation.repository_root.clone(),
        common_git_dir: observation.common_git_dir.clone(),
        head_commit: observation.head_commit.clone(),
    };
    let workspace = workspace.clone();
    tokio::task::spawn_blocking(move || CleanupPaths::capture(&observation, &workspace))
        .await
        .map_err(|_| "workspace_cleanup_unavailable")?
}

fn compare_observation(
    record: &WorkspaceCleanup,
    observation: &CleanupObservation,
) -> Result<(), &'static str> {
    if observation.head_commit != record.binding.head_commit
        || path_text(&observation.repository_root)? != record.binding.repository_root
        || path_text(&observation.common_git_dir)? != record.binding.common_git_dir
    {
        Err("workspace_cleanup_changed")
    } else {
        Ok(())
    }
}
async fn physical_key(path: &str) -> Result<String, &'static str> {
    let path = PathBuf::from(path);
    tokio::task::spawn_blocking(move || {
        crate::harness_dispatch::HarnessDispatchService::workflow_resource_key(&path)
    })
    .await
    .map_err(|_| "workspace_cleanup_directory_unavailable")?
    .map(|(_, key)| key.to_hex())
    .ok_or("workspace_cleanup_directory_unavailable")
}
async fn verify_paths(paths: CleanupPaths) -> Result<CleanupPaths, &'static str> {
    tokio::task::spawn_blocking(move || {
        paths.verify_roots()?;
        Ok(paths)
    })
    .await
    .map_err(|_| "workspace_cleanup_unavailable")?
}
fn executor_binding(
    pid: u32,
    nonce: Uuid,
    incarnation: Uuid,
    images: &Images,
    root: &Path,
) -> Result<ExecutorBinding, &'static str> {
    images.verify()?;
    let mut system = sysinfo::System::new();
    let process_id = sysinfo::Pid::from_u32(pid);
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[process_id]),
        true,
        sysinfo::ProcessRefreshKind::nothing().with_exe(sysinfo::UpdateKind::Always),
    );
    let process = system
        .process(process_id)
        .ok_or("workspace_cleanup_identity_unavailable")?;
    if process.status() == sysinfo::ProcessStatus::Zombie || process.start_time() == 0 {
        return Err("workspace_cleanup_identity_unavailable");
    }
    let observed = CleanupExecutablePin::acquire(
        process
            .exe()
            .ok_or("workspace_cleanup_identity_unavailable")?,
        root,
    )?;
    if observed.identity() != images.trampoline.identity() {
        return Err("workspace_cleanup_identity_mismatch");
    }
    let binding = ExecutorBinding {
        executor_nonce: nonce,
        daemon_incarnation: incarnation,
        process_id: pid,
        observed_start_seconds: process.start_time(),
        expected_executable: path_text(images.trampoline.canonical_path())?,
        observed_executable: path_text(observed.canonical_path())?,
        consumed_executable_identity: images.trampoline.identity(),
        observed_path_identity: observed.identity(),
        program_executable: path_text(images.git.canonical_path())?,
        program_identity: images.git.identity(),
        command_profile: CleanupCommandProfile::GitWorktreeRemoveV1,
        containment_kind: CleanupContainmentKind::WindowsJobV1,
    };
    binding
        .validate()
        .map_err(|_| "workspace_cleanup_identity_unavailable")?;
    images.verify()?;
    Ok(binding)
}
fn problem_code(problem: &CleanupRunProblem<&'static str>) -> &'static str {
    match problem {
        CleanupRunProblem::Authorization(code) => code,
        CleanupRunProblem::Release(error) => {
            if matches!(error, vibemux_harness::dispatch::DispatchError::PipeFailed) {
                "workspace_cleanup_release_unconfirmed"
            } else {
                "workspace_cleanup_transport_unconfirmed"
            }
        }
        CleanupRunProblem::TimedOut => "workspace_cleanup_process_timed_out",
        CleanupRunProblem::Cancelled => "workspace_cleanup_cancelled",
        CleanupRunProblem::OutputLimit => "workspace_cleanup_output_limit",
        CleanupRunProblem::ReadFailed => "workspace_cleanup_output_unavailable",
    }
}

#[cfg(test)]
mod tests;
