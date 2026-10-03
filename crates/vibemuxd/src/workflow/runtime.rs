//! Shared run context of one workflow coordinator (ADR 031 §4).
//!
//! Every effect goes through one of three owners: the single writer (every
//! canonical state change, re-checked inside its transaction), the dispatch
//! service (every worker and reviewer turn), and the workspace manager
//! (every owned worktree). This module holds the primitives the coordinator
//! composes: versioned workflow updates, lease acquisition and heartbeats,
//! owned worktrees recorded in the private ledger, and turn planning and
//! execution with cooperative cancellation.

use std::{
    collections::BTreeMap,
    future::Future,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use time::OffsetDateTime;
use tokio::{sync::watch, task::JoinHandle};
use uuid::Uuid;
use vibemux_harness::{
    AgentKind,
    dispatch::{DispatchPhase, NativeProtocol, writable_profile::WritableGrant},
};
use vibemux_store::{AttemptPurpose, LeaseRecord, LeaseRequest, WorkflowSnapshot};
use vibemux_types::{RunId, a2a::RunWorkspace};
use vibemux_workflow::{
    Sha256Digest, SpecIdentifier,
    gates::WorkflowPhase,
    leases::{LeaseEvent, LeaseState},
    messages::Participant,
    renderer::{ContextBlock, TurnPurpose},
    workflow_record::WorkflowRecord,
};
use vibemux_workspace::WorkspaceManager;

use super::{
    broker::Delivery,
    error::WorkflowError,
    prepare::WorkflowPlan,
    settings::{LoadedWorkflowConfig, SlotConfig},
    state_files::{StateFiles, WorkspaceLedger},
    turns::{TurnRequest, lease_id_for, render_turn, turn_request_id},
};
use crate::{
    WriterError, WriterHandle,
    harness_dispatch::{HarnessDispatchService, WorkflowTurn, WorkflowTurnBinding, call_writer},
};

pub const VERSION_CONFLICT_CODE: &str = "store_workflow_version_conflict";
const LEASE_ENDED_CODE: &str = "workflow_lease_ended";
/// Bound on retries of one versioned update under concurrent changes.
const MAX_VERSION_RETRIES: usize = 8;
/// Bound on lease rounds of one unit; a unit that needs more has looped.
const MAX_LEASE_ROUNDS: u32 = 64;
/// While a cancellation could not reach a turn that is still being
/// admitted, it is retried at this interval.
const CANCEL_RETRY_INTERVAL: Duration = Duration::from_millis(200);
const MIN_HEARTBEAT_INTERVAL: Duration = Duration::from_millis(250);

/// The coordinator's control signal, sent by the service.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Control {
    Run,
    /// Finish the current turn, settle leases, and stop at `paused`.
    Pause,
    /// Cancel the running turns, settle leases, and stop at `cancelled`.
    Cancel,
}

/// Inputs fixed at daemon startup.
pub(crate) struct WorkflowSetup {
    pub config: LoadedWorkflowConfig,
    pub canonical_root: PathBuf,
    pub state: StateFiles,
    /// The execution protocol of each slot whose harness route may execute.
    pub slot_protocols: BTreeMap<SpecIdentifier, NativeProtocol>,
    pub trampoline: Option<PathBuf>,
}

/// Unix milliseconds of a timestamp.
#[must_use]
pub(crate) fn millis(timestamp: OffsetDateTime) -> u64 {
    u64::try_from(timestamp.unix_timestamp_nanos() / 1_000_000).unwrap_or(0)
}

#[must_use]
pub(crate) fn now_ms() -> u64 {
    millis(OffsetDateTime::now_utc())
}

#[must_use]
pub(crate) const fn attempt_purpose(purpose: TurnPurpose) -> AttemptPurpose {
    match purpose {
        TurnPurpose::Implement => AttemptPurpose::Implement,
        TurnPurpose::Repair => AttemptPurpose::Repair,
        TurnPurpose::Answer => AttemptPurpose::Answer,
        TurnPurpose::Review => AttemptPurpose::Review,
    }
}

/// One worker unit: a task assigned to a slot. In cooperative mode each
/// task has one unit; in compare mode each competitor is one unit of the
/// same task.
#[derive(Clone, Debug)]
pub(crate) struct Unit {
    pub task_key: SpecIdentifier,
    pub slot: SlotConfig,
    pub contract_id: Sha256Digest,
    pub session_id: Uuid,
}

impl Unit {
    /// The ledger key of the unit's worktree.
    #[must_use]
    pub fn key(&self) -> String {
        format!("worker:{}:{}", self.task_key, self.slot.slot_id)
    }

    #[must_use]
    pub fn participant(&self) -> Participant {
        Participant::Worker {
            task_key: self.task_key.clone(),
            session_id: self.session_id,
        }
    }
}

/// What one turn is.
#[derive(Clone, Debug)]
pub(crate) struct TurnSpec {
    pub task_key: SpecIdentifier,
    pub contract_id: Sha256Digest,
    pub session_id: Uuid,
    pub harness: AgentKind,
    pub lease: LeaseRecord,
    pub working_directory: PathBuf,
    pub purpose: TurnPurpose,
    pub notes: Vec<String>,
    pub context_blocks: Vec<ContextBlock>,
    pub reviewed_candidate: Option<Sha256Digest>,
}

/// A turn with its fixed identity and prompt. Planned once, so a retried
/// admission resolves to the same dispatch instead of a second launch.
#[derive(Clone, Debug)]
pub(crate) struct TurnPlan {
    pub spec: TurnSpec,
    pub request_id: Uuid,
    pub prompt: String,
}

/// The fixed identity of one turn.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TurnIdentity {
    pub request_id: Uuid,
    pub turn_number: u32,
    policy_digest: Sha256Digest,
}

/// What a finished turn left behind.
#[derive(Clone, Debug)]
pub(crate) struct TurnResult {
    pub request_id: Uuid,
    pub run_id: Uuid,
    pub phase: DispatchPhase,
    pub final_text: Option<String>,
    pub delivery_applied: bool,
}

impl TurnResult {
    #[must_use]
    pub fn completed(&self) -> bool {
        self.phase == DispatchPhase::Completed
    }
}

/// Aborts the heartbeat task when the lease's holder is done.
pub(crate) struct HeartbeatGuard(JoinHandle<()>);

impl Drop for HeartbeatGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(crate) struct RunContext {
    pub writer: WriterHandle,
    pub dispatch: HarnessDispatchService,
    pub setup: Arc<WorkflowSetup>,
    pub plan: WorkflowPlan,
    pub workflow_id: Uuid,
    pub manager: WorkspaceManager,
    pub control: watch::Receiver<Control>,
    pub ledger: Mutex<WorkspaceLedger>,
    /// Serializes creation and durable ledger publication for all units.
    pub workspace_gate: tokio::sync::Semaphore,
    /// Review turns share the reviewer slot, so they run one at a time.
    pub reviewer_turns: tokio::sync::Mutex<()>,
}

impl RunContext {
    #[must_use]
    pub fn control(&self) -> Control {
        *self.control.borrow()
    }

    #[must_use]
    pub fn stopping(&self) -> bool {
        self.control() != Control::Run
    }

    pub async fn record(&self) -> Result<WorkflowRecord, WorkflowError> {
        let writer = self.writer.clone();
        let workflow_id = self.workflow_id;
        call_writer(move || writer.workflow_record(workflow_id))
            .await?
            .ok_or(WorkflowError::NotFound)
    }

    pub async fn snapshot(&self) -> Result<WorkflowSnapshot, WorkflowError> {
        let writer = self.writer.clone();
        let workflow_id = self.workflow_id;
        call_writer(move || writer.workflow_snapshot(workflow_id))
            .await?
            .ok_or(WorkflowError::NotFound)
    }

    /// Runs a writer operation that names the workflow version it read,
    /// re-reading on a concurrent version change.
    pub async fn versioned<T, F>(&self, operation: F) -> Result<T, WorkflowError>
    where
        F: Fn(&WriterHandle, u64) -> Result<T, WriterError> + Clone + Send + Sync + 'static,
        T: Send + 'static,
    {
        versioned(&self.writer, self.workflow_id, operation).await
    }

    pub async fn transition(
        &self,
        phase: WorkflowPhase,
        reason_code: Option<&str>,
    ) -> Result<WorkflowRecord, WorkflowError> {
        transition(&self.writer, self.workflow_id, phase, reason_code).await
    }

    /// The unit's slot config and route label.
    #[must_use]
    pub fn route_label(&self, slot: &SlotConfig) -> String {
        self.setup.config.route_label(slot)
    }

    /// An owned worktree for `key`: the recorded one, re-inspected, or a
    /// new one recorded in the ledger before any turn uses it.
    pub async fn workspace(&self, key: &str) -> Result<RunWorkspace, WorkflowError> {
        let _workspace_permit = self
            .workspace_gate
            .acquire()
            .await
            .map_err(|_| WorkflowError::Internal)?;
        let existing = self
            .ledger
            .lock()
            .map_err(|_| WorkflowError::Internal)?
            .workspaces
            .get(key)
            .cloned();
        if let Some(workspace) = existing {
            self.manager
                .inspect(&workspace)
                .await
                .map_err(|_| WorkflowError::Workspace)?;
            return Ok(workspace);
        }
        let workspace = self
            .manager
            .create(RunId::new())
            .await
            .map_err(|_| WorkflowError::Workspace)?;
        let ledger = {
            let mut ledger = self.ledger.lock().map_err(|_| WorkflowError::Internal)?;
            ledger.workspaces.insert(key.to_string(), workspace.clone());
            ledger.clone()
        };
        let state = self.setup.state.clone();
        let workflow_id = self.workflow_id;
        let published =
            tokio::task::spawn_blocking(move || state.write_ledger(workflow_id, &ledger))
                .await
                .map_err(|_| WorkflowError::Internal)
                .and_then(|result| result);
        if let Err(error) = published {
            // Nothing has used the new worktree yet. Remove it through the
            // manager's ownership checks, so a failed ledger write cannot
            // leave an unrecorded worktree behind.
            if self.manager.cleanup(&workspace).await.is_ok() {
                self.ledger
                    .lock()
                    .map_err(|_| WorkflowError::Internal)?
                    .workspaces
                    .remove(key);
            }
            return Err(error);
        }
        Ok(workspace)
    }

    /// Acquires the unit's next lease, or reuses an active one it already
    /// holds. Lease IDs are derived from the unit and a round number, so a
    /// retried acquisition resolves to the same lease.
    pub async fn acquire_lease(
        &self,
        task_key: &SpecIdentifier,
        slot_id: &SpecIdentifier,
        role: &str,
        working_directory: &Path,
    ) -> Result<LeaseRecord, WorkflowError> {
        let directory = working_directory.to_path_buf();
        let (_, worktree_key) = tokio::task::spawn_blocking(move || {
            HarnessDispatchService::workflow_resource_key(&directory)
        })
        .await
        .ok()
        .flatten()
        .ok_or(WorkflowError::Workspace)?;
        for round in 0..MAX_LEASE_ROUNDS {
            let lease_id = lease_id_for(self.workflow_id, task_key, slot_id, role, round);
            let writer = self.writer.clone();
            let existing = call_writer(move || writer.workflow_lease(lease_id)).await?;
            match existing {
                Some(lease)
                    if lease.state == LeaseState::Active && lease.worktree_key == worktree_key =>
                {
                    return Ok(lease);
                }
                Some(_) => continue,
                None => {}
            }
            let request = LeaseRequest {
                lease_id,
                workflow_id: self.workflow_id,
                task_key: task_key.clone(),
                slot_id: slot_id.clone(),
                worktree_key,
                heartbeat_deadline_ms: now_ms()
                    .saturating_add(self.setup.config.config.heartbeat_ms),
                timestamp: OffsetDateTime::now_utc(),
            };
            let writer = self.writer.clone();
            let commit =
                call_writer(move || writer.acquire_workflow_lease(request.clone())).await?;
            return Ok(commit.lease);
        }
        Err(WorkflowError::Internal)
    }

    /// Keeps a lease's heartbeat deadline ahead while its holder works.
    pub fn heartbeat(&self, lease: &LeaseRecord) -> HeartbeatGuard {
        let writer = self.writer.clone();
        let lease_id = lease.lease_id;
        let generation = lease.generation;
        let heartbeat_ms = self.setup.config.config.heartbeat_ms;
        let interval = Duration::from_millis(heartbeat_ms / 2).max(MIN_HEARTBEAT_INTERVAL);
        HeartbeatGuard(tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let writer = writer.clone();
                let now = now_ms();
                let beat = call_writer(move || {
                    writer.change_workflow_lease(
                        lease_id,
                        LeaseEvent::Heartbeat {
                            generation,
                            now_ms: now,
                        },
                        heartbeat_ms,
                        "heartbeat".to_string(),
                        OffsetDateTime::now_utc(),
                    )
                })
                .await;
                if beat.is_err() {
                    break;
                }
            }
        }))
    }

    /// Releases a lease whose attempts settled. An already ended lease is
    /// fine: settlement is idempotent.
    pub async fn settle_lease(
        &self,
        lease: &LeaseRecord,
        reason_code: &str,
    ) -> Result<(), WorkflowError> {
        settle_lease(
            &self.writer,
            lease,
            reason_code,
            self.setup.config.config.heartbeat_ms,
        )
        .await
    }

    /// Fixes a turn's request ID and number from the attempts already
    /// admitted under its lease and session.
    pub async fn turn_identity(&self, spec: &TurnSpec) -> Result<TurnIdentity, WorkflowError> {
        let snapshot = self.snapshot().await?;
        let purpose = attempt_purpose(spec.purpose);
        let ordinal = snapshot
            .attempts
            .iter()
            .filter(|attempt| attempt.lease_id == spec.lease.lease_id && attempt.purpose == purpose)
            .count();
        let turn_number = if spec.purpose == TurnPurpose::Review {
            1
        } else {
            snapshot
                .attempts
                .iter()
                .filter(|attempt| {
                    attempt.session_id == spec.session_id
                        && attempt.purpose != AttemptPurpose::Review
                })
                .count()
                + 1
        };
        Ok(TurnIdentity {
            request_id: turn_request_id(
                self.workflow_id,
                spec.lease.lease_id,
                spec.purpose,
                u32::try_from(ordinal).map_err(|_| WorkflowError::Internal)?,
            ),
            turn_number: u32::try_from(turn_number).map_err(|_| WorkflowError::Internal)?,
            policy_digest: snapshot.record.policy_digest,
        })
    }

    /// Renders the prompt of a turn whose identity is fixed.
    pub fn render_plan(
        &self,
        spec: TurnSpec,
        identity: &TurnIdentity,
    ) -> Result<TurnPlan, WorkflowError> {
        let prompt = render_turn(
            &self.plan,
            identity.policy_digest,
            &TurnRequest {
                task_key: &spec.task_key,
                purpose: spec.purpose,
                turn_number: identity.turn_number,
                notes: &spec.notes,
                context_blocks: &spec.context_blocks,
                reviewed_candidate: spec.reviewed_candidate,
            },
        )?;
        Ok(TurnPlan {
            spec,
            request_id: identity.request_id,
            prompt: prompt.text,
        })
    }

    /// Fixes a turn's identity and prompt.
    pub async fn plan_turn(&self, spec: TurnSpec) -> Result<TurnPlan, WorkflowError> {
        let identity = self.turn_identity(&spec).await?;
        self.render_plan(spec, &identity)
    }

    /// Admits and runs one planned turn to its fenced finish. A cancel
    /// signal reaches the dispatch while the turn runs.
    pub async fn run_turn(
        &self,
        plan: &TurnPlan,
        deliveries: &[Delivery],
    ) -> Result<TurnResult, WorkflowError> {
        let spec = &plan.spec;
        let grant = if spec.purpose.is_read_only() {
            WritableGrant::READ_ONLY
        } else {
            WritableGrant {
                edit_files: true,
                run_dev_tests: true,
            }
        };
        let mut conflicts = 0;
        loop {
            let record = self.record().await?;
            let turn = WorkflowTurn {
                request_id: plan.request_id,
                harness: spec.harness,
                prompt: plan.prompt.clone(),
                grant,
                working_directory: spec.working_directory.clone(),
                base_commit: record.base_commit.clone(),
                binding: WorkflowTurnBinding {
                    workflow_id: self.workflow_id,
                    expected_workflow_version: record.version,
                    lease_id: spec.lease.lease_id,
                    lease_generation: spec.lease.generation,
                    task_key: spec.task_key.clone(),
                    contract_id: spec.contract_id,
                    purpose: attempt_purpose(spec.purpose),
                    session_id: spec.session_id,
                },
            };
            let writer = self.writer.clone();
            let workflow_id = self.workflow_id;
            let message_ids: Vec<Uuid> = deliveries.iter().map(Delivery::message_id).collect();
            let outcome = self
                .cancellable(
                    plan.request_id,
                    self.dispatch
                        .run_workflow_turn(turn, move |attempt| async move {
                            if !message_ids.is_empty() {
                                call_writer(move || {
                                    writer.deliver_workflow_messages(
                                        workflow_id,
                                        message_ids.clone(),
                                        attempt,
                                        OffsetDateTime::now_utc(),
                                    )
                                })
                                .await?;
                            }
                            Ok::<(), crate::harness_dispatch::DispatchServiceError>(())
                        }),
                )
                .await;
            match outcome {
                Err(error)
                    if error.code() == VERSION_CONFLICT_CODE && conflicts < MAX_VERSION_RETRIES =>
                {
                    conflicts += 1;
                }
                Err(error) => return Err(error.into()),
                Ok(outcome) => {
                    return Ok(TurnResult {
                        request_id: plan.request_id,
                        run_id: *outcome.record.run_id.as_uuid(),
                        phase: outcome.record.phase,
                        final_text: outcome.final_text,
                        delivery_applied: outcome.delivery_applied,
                    });
                }
            }
        }
    }

    /// Awaits `turn`; a cancel signal is forwarded to the dispatch, and
    /// retried while the turn is still being admitted.
    async fn cancellable<T>(&self, request_id: Uuid, turn: impl Future<Output = T>) -> T {
        let mut control = self.control.clone();
        let mut cancel_pending = *control.borrow() == Control::Cancel;
        let mut control_open = true;
        tokio::pin!(turn);
        loop {
            if cancel_pending {
                cancel_pending = self.dispatch.cancel(request_id).await.is_err();
            }
            tokio::select! {
                result = &mut turn => return result,
                changed = control.changed(), if control_open => {
                    match changed {
                        Ok(()) => {
                            if *control.borrow() == Control::Cancel {
                                cancel_pending = true;
                            }
                        }
                        Err(_) => control_open = false,
                    }
                }
                () = tokio::time::sleep(CANCEL_RETRY_INTERVAL), if cancel_pending => {}
            }
        }
    }
}

/// [`RunContext::versioned`] for callers without a run context.
pub(crate) async fn versioned<T, F>(
    writer: &WriterHandle,
    workflow_id: Uuid,
    operation: F,
) -> Result<T, WorkflowError>
where
    F: Fn(&WriterHandle, u64) -> Result<T, WriterError> + Clone + Send + Sync + 'static,
    T: Send + 'static,
{
    for _ in 0..MAX_VERSION_RETRIES {
        let reader = writer.clone();
        let version = call_writer(move || reader.workflow_record(workflow_id))
            .await?
            .ok_or(WorkflowError::NotFound)?
            .version;
        let handle = writer.clone();
        let call = operation.clone();
        match call_writer(move || call(&handle, version)).await {
            Err(WriterError::Store { code }) if code == VERSION_CONFLICT_CODE => {}
            other => return other.map_err(WorkflowError::from),
        }
    }
    Err(WorkflowError::Writer {
        code: VERSION_CONFLICT_CODE.to_string(),
    })
}

pub(crate) async fn transition(
    writer: &WriterHandle,
    workflow_id: Uuid,
    phase: WorkflowPhase,
    reason_code: Option<&str>,
) -> Result<WorkflowRecord, WorkflowError> {
    let reason = reason_code.map(str::to_string);
    versioned(writer, workflow_id, move |handle, version| {
        handle
            .transition_workflow(
                workflow_id,
                version,
                phase,
                reason.clone(),
                OffsetDateTime::now_utc(),
            )
            .map(|commit| commit.record)
    })
    .await
}

pub(crate) async fn settle_lease(
    writer: &WriterHandle,
    lease: &LeaseRecord,
    reason_code: &str,
    heartbeat_ms: u64,
) -> Result<(), WorkflowError> {
    let handle = writer.clone();
    let lease_id = lease.lease_id;
    let generation = lease.generation;
    let reason = reason_code.to_string();
    match call_writer(move || {
        handle.change_workflow_lease(
            lease_id,
            LeaseEvent::AttemptSettled { generation },
            heartbeat_ms,
            reason.clone(),
            OffsetDateTime::now_utc(),
        )
    })
    .await
    {
        Ok(_) => Ok(()),
        Err(WriterError::Store { code }) if code == LEASE_ENDED_CODE => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Ends a lease after its attempts settled, whatever its state.
pub(crate) async fn revoke_lease(
    writer: &WriterHandle,
    lease: &LeaseRecord,
    reason_code: &str,
    heartbeat_ms: u64,
) -> Result<(), WorkflowError> {
    let handle = writer.clone();
    let lease_id = lease.lease_id;
    let generation = lease.generation;
    let reason = reason_code.to_string();
    match call_writer(move || {
        handle.change_workflow_lease(
            lease_id,
            LeaseEvent::Revoke {
                generation,
                attempt_settled: true,
            },
            heartbeat_ms,
            reason.clone(),
            OffsetDateTime::now_utc(),
        )
    })
    .await
    {
        Ok(_) => Ok(()),
        Err(WriterError::Store { code }) if code == LEASE_ENDED_CODE => Ok(()),
        Err(error) => Err(error.into()),
    }
}
