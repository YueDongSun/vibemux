//! The workflow service: the daemon owner of dual-track workflows
//! (ADR 031 §4).
//!
//! The service pins the workflow config at startup, prepares workflows
//! from operator requests, starts at most one coordinator per workflow,
//! and answers status, export, slot, share, session, and policy queries.
//! Every state change goes through the single writer; every worker and
//! reviewer turn goes through the dispatch service; nothing here reads a
//! vendor transcript or emits a prompt unless the operator asks for the
//! retained text of a session explicitly.

use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, Weak},
    time::Duration,
};

use serde::Serialize;
use time::OffsetDateTime;
use tokio::{sync::watch, task::JoinSet, time::timeout};
use uuid::Uuid;
use vibemux_store::{PolicyVersionRecord, WorkflowSnapshot};
use vibemux_workflow::{
    Sha256Digest, SpecIdentifier,
    gates::WorkflowPhase,
    leases::LeaseState,
    policy::OperatorPolicy,
    renderer::TurnPurpose,
    slots::{
        EligibilityReport, EligibilityRequest, EvidenceLevel, InterfaceMode, ParallelismPlan,
        SlotCapability, SlotFacts, evaluate_eligibility, plan_parallelism,
    },
};
use vibemux_workspace::WorkspaceManager;

use super::{
    coordinator::{self, RunEnd, RunReport},
    error::WorkflowError,
    evidence::{
        ExportPage, PlannedSession, SessionInspection, WorkflowStatus, export_items, export_page,
        inspect_session, resumable, status_view,
    },
    prepare::{PrepareContext, TemplateOverrides, WorkflowPlan, compile, start_binding},
    request::{MAX_POLICY_BYTES, WorkflowRequest},
    runtime::{Control, RunContext, Unit, WorkflowSetup, now_ms, revoke_lease, transition},
    share::{find_bundle, share_bundle},
    startup::{finish_cancel, load_setup, observe_slots, reconcile},
    state_files::StateFiles,
    turns::{TurnRequest, WRITABLE_REQUIREMENTS, render_turn, session_id, workflow_id_for},
};
use crate::{
    WriterHandle,
    harness_dispatch::{HarnessDispatchService, call_writer, git_head::read_head_commit},
};

/// Page size for contract, bundle, and session lookup.
const WORKFLOW_PAGE_SIZE: usize = 256;
/// How long shutdown waits for coordinators to reach a turn boundary.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
/// Heartbeat used for lease changes when the config is missing.
const FALLBACK_HEARTBEAT_MS: u64 = 30_000;
const RECOVERED_LEASE_CODE: &str = "workflow_recovered_attempt_settled";
pub const RECOVERY_PENDING_CODE: &str = "attempt_recovery_pending";
pub const SLOT_INELIGIBLE_CODE: &str = "slot_ineligible";
/// The only reviewer requirement: a structured turn the daemon can run.
const REVIEW_REQUIREMENTS: [(SlotCapability, EvidenceLevel); 1] = [(
    SlotCapability::StructuredTurn,
    EvidenceLevel::AuthenticatedTurnVerified,
)];

/// Where the service finds its inputs.
#[derive(Clone)]
pub struct WorkflowServiceSettings {
    pub project_root: PathBuf,
    /// The daemon state directory: the config and private state live here.
    pub state_dir: PathBuf,
}

impl WorkflowServiceSettings {
    #[must_use]
    pub fn for_project(project_root: &Path, state_dir: &Path) -> Self {
        Self {
            project_root: project_root.to_path_buf(),
            state_dir: state_dir.to_path_buf(),
        }
    }
}

impl std::fmt::Debug for WorkflowServiceSettings {
    /// Redacted: paths never reach a log.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkflowServiceSettings")
            .finish_non_exhaustive()
    }
}

/// What `workflow prepare` returns: identities and digests, no prompt.
#[derive(Clone, Debug, Serialize)]
pub struct PrepareResponse {
    pub workflow_id: Uuid,
    pub phase: WorkflowPhase,
    /// The digest `workflow start --contract` must present.
    pub start_contract: Sha256Digest,
    pub contracts: Vec<PreparedContract>,
    pub narrowing_count: usize,
    /// The request key was already prepared with the same admission.
    pub duplicate: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct PreparedContract {
    pub task_key: String,
    pub contract_id: Sha256Digest,
    pub contract_version: u32,
    pub template_version: String,
    pub rendered_prompt_digest: Sha256Digest,
    pub rendered_prompt_bytes: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct StartResponse {
    pub workflow_id: Uuid,
    pub phase: WorkflowPhase,
    pub parallelism: ParallelismPlan,
    /// This request ID already started the running coordinator.
    pub duplicate: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct PhaseControl {
    pub workflow_id: Uuid,
    pub phase: WorkflowPhase,
    /// A running coordinator was signalled and stops at its next turn
    /// boundary.
    pub signalled: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct SlotRoute {
    pub slot_id: SpecIdentifier,
    pub route_label: String,
    pub executes: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct SlotsResponse {
    pub evidence_class: vibemux_workflow::slots::EvidenceClass,
    pub slots: Vec<SlotFacts>,
    pub routes: Vec<SlotRoute>,
    pub writable: EligibilityReport,
}

#[derive(Clone, Debug, Serialize)]
pub struct ShareResponse {
    pub workflow_id: Uuid,
    pub message_id: Uuid,
    pub bundle_id: Uuid,
    pub recipient: String,
    pub recipient_sequence: u64,
    pub duplicate: bool,
}

/// One prompt of a session, returned only on explicit request. Index
/// `0..total - 1` are the prompts already sent, in attempt order; the last
/// index is the next prompt.
#[derive(Clone, Debug, Serialize)]
pub struct SessionPrompt {
    pub session_id: Uuid,
    pub index: usize,
    pub total: usize,
    pub source: PromptSource,
    /// The attempt that sent it; `None` for the next prompt.
    pub request_id: Option<Uuid>,
    pub rendered_prompt_sha256: Option<Sha256Digest>,
    pub text: Option<String>,
    /// Why `text` is absent.
    pub unavailable_reason: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptSource {
    /// Sent; its text comes from the opt-in content store.
    Sent,
    /// The exact next prompt, when it is fixed before the run continues.
    Next,
}

/// Why a sent prompt's text is not shown.
pub const PROMPT_NOT_RETAINED_CODE: &str = "prompt_not_retained";

#[derive(Clone, Debug, Serialize)]
pub struct PurgeResponse {
    pub workflow_id: Uuid,
    pub deleted: u32,
    pub already_absent: u32,
}

/// Cloneable handle; clones share one service.
#[derive(Clone)]
pub struct WorkflowService {
    inner: Arc<ServiceInner>,
}

impl std::fmt::Debug for WorkflowService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkflowService")
            .finish_non_exhaustive()
    }
}

struct ServiceInner {
    writer: WriterHandle,
    dispatch: HarnessDispatchService,
    setup: Result<Arc<WorkflowSetup>, WorkflowError>,
    runs: Mutex<RunRegistry>,
    /// Serializes start with operator phase controls until the coordinator
    /// has an addressable control channel.
    lifecycle_gate: tokio::sync::Semaphore,
    /// At most one prompt-policy evaluation runs at a time.
    evaluation: tokio::sync::Mutex<()>,
}

struct RunRegistry {
    accepting: bool,
    active: HashMap<Uuid, ActiveRun>,
    reports: HashMap<Uuid, RunReport>,
    tasks: JoinSet<()>,
}

struct ActiveRun {
    control: watch::Sender<Control>,
    start_request_id: Uuid,
}

impl ServiceInner {
    fn runs(&self) -> MutexGuard<'_, RunRegistry> {
        self.runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn setup(&self) -> Result<Arc<WorkflowSetup>, WorkflowError> {
        self.setup.clone()
    }

    fn heartbeat_ms(&self) -> u64 {
        self.setup.as_ref().map_or(FALLBACK_HEARTBEAT_MS, |setup| {
            setup.config.config.heartbeat_ms
        })
    }
}

impl WorkflowService {
    /// Pins the config, records slot observations, and reconciles
    /// workflows a previous daemon left mid-run. Never fails: a missing or
    /// invalid config leaves the service unavailable with that code, while
    /// status, cancel, and export still work.
    pub async fn start(
        writer: WriterHandle,
        dispatch: HarnessDispatchService,
        settings: WorkflowServiceSettings,
    ) -> Self {
        let loader = dispatch.clone();
        let mut setup = tokio::task::spawn_blocking(move || {
            load_setup(&settings.project_root, &settings.state_dir, &loader)
        })
        .await
        .unwrap_or(Err(WorkflowError::Internal))
        .map(Arc::new);
        if let Ok(loaded) = &setup {
            if let Err(error) = observe_slots(&writer, loaded).await {
                setup = Err(error);
            }
        }
        let heartbeat_ms = setup.as_ref().map_or(FALLBACK_HEARTBEAT_MS, |setup| {
            setup.config.config.heartbeat_ms
        });
        if let Err(error) = reconcile(&writer, heartbeat_ms).await {
            setup = Err(error);
        }
        Self::with_setup(writer, dispatch, setup)
    }

    /// A service without a config, for daemons started without project
    /// paths.
    #[must_use]
    pub fn unconfigured(writer: WriterHandle, dispatch: HarnessDispatchService) -> Self {
        Self::with_setup(writer, dispatch, Err(WorkflowError::Unconfigured))
    }

    fn with_setup(
        writer: WriterHandle,
        dispatch: HarnessDispatchService,
        setup: Result<Arc<WorkflowSetup>, WorkflowError>,
    ) -> Self {
        Self {
            inner: Arc::new(ServiceInner {
                writer,
                dispatch,
                setup,
                runs: Mutex::new(RunRegistry {
                    accepting: true,
                    active: HashMap::new(),
                    reports: HashMap::new(),
                    tasks: JoinSet::new(),
                }),
                lifecycle_gate: tokio::sync::Semaphore::new(1),
                evaluation: tokio::sync::Mutex::new(()),
            }),
        }
    }

    pub(super) fn accepting(&self) -> Result<(), WorkflowError> {
        if self.inner.runs().accepting {
            Ok(())
        } else {
            Err(WorkflowError::ShuttingDown)
        }
    }

    /// Validates and compiles a request, then admits it through the writer.
    /// The same request key with the same admission returns the stored
    /// workflow; a different one is refused.
    pub async fn prepare(
        &self,
        request: serde_json::Value,
        policy: serde_json::Value,
    ) -> Result<PrepareResponse, WorkflowError> {
        let overrides = self.active_template_overrides().await?;
        self.prepare_with(request, policy, overrides).await
    }

    /// [`Self::prepare`] with explicit template wording: the active
    /// policy's for operator requests, an evaluated policy's for an
    /// evaluation case.
    pub(super) async fn prepare_with(
        &self,
        request: serde_json::Value,
        policy: serde_json::Value,
        overrides: TemplateOverrides,
    ) -> Result<PrepareResponse, WorkflowError> {
        self.accepting()?;
        let setup = self.inner.setup()?;
        let request = WorkflowRequest::parse(request)?;
        let policy_bytes = serde_json::to_vec(&policy).map_err(|_| WorkflowError::PolicyInvalid)?;
        if policy_bytes.len() > MAX_POLICY_BYTES {
            return Err(WorkflowError::PolicyInvalid);
        }
        let policy =
            OperatorPolicy::parse(&policy_bytes).map_err(|_| WorkflowError::PolicyInvalid)?;
        let compile_setup = Arc::clone(&setup);
        let workflow_id = workflow_id_for(&request.request_key);
        let prepared = tokio::task::spawn_blocking(move || {
            let head =
                read_head_commit(&compile_setup.canonical_root).map_err(|_| WorkflowError::Git)?;
            let prepared = compile(
                &request,
                &policy,
                workflow_id,
                &PrepareContext {
                    config: &compile_setup.config,
                    canonical_root: &compile_setup.canonical_root,
                    head_commit: &head,
                    slot_protocols: &compile_setup.slot_protocols,
                    template_overrides: &overrides,
                    now_ms: now_ms(),
                },
            )?;
            Ok::<_, WorkflowError>(prepared)
        })
        .await
        .map_err(|_| WorkflowError::Internal)??;
        let writer = self.inner.writer.clone();
        let record = prepared.record.clone();
        let contracts = prepared.contracts.clone();
        let commit = call_writer(move || {
            writer.prepare_workflow(record.clone(), contracts.clone(), OffsetDateTime::now_utc())
        })
        .await
        .map_err(|error| match error.code() {
            "store_workflow_request_conflict" => WorkflowError::RequestConflict,
            _ => WorkflowError::from(error),
        })?;
        let state = setup.state.clone();
        let plan = prepared.plan.clone();
        tokio::task::spawn_blocking(move || state.write_plan(workflow_id, &plan))
            .await
            .map_err(|_| WorkflowError::Internal)??;
        let start_contract = start_binding(&commit.record, &prepared.plan)?;
        Ok(PrepareResponse {
            workflow_id,
            phase: commit.record.phase,
            start_contract,
            contracts: prepared
                .contracts
                .iter()
                .map(|contract| PreparedContract {
                    task_key: contract.task_key.clone(),
                    contract_id: contract.contract_id,
                    contract_version: contract.contract_version,
                    template_version: contract.template_version.clone(),
                    rendered_prompt_digest: contract.rendered_prompt_digest,
                    rendered_prompt_bytes: contract.rendered_prompt_bytes,
                })
                .collect(),
            narrowing_count: prepared.narrowing_count,
            duplicate: commit.event.is_none(),
        })
    }

    /// The template wording of the active promoted prompt policy.
    async fn active_template_overrides(&self) -> Result<TemplateOverrides, WorkflowError> {
        let writer = self.inner.writer.clone();
        let versions = call_writer(move || writer.prompt_policy_versions()).await?;
        Ok(
            active_policy(&versions).map_or_else(TemplateOverrides::new, |record| {
                record.policy.template_overrides.clone()
            }),
        )
    }

    /// Starts (or resumes) the coordinator of the workflow whose start
    /// binding is `contract`. Repeating the request ID of the running
    /// coordinator returns it; another request ID is refused.
    pub async fn start_workflow(
        &self,
        contract: Sha256Digest,
        request_id: Uuid,
    ) -> Result<StartResponse, WorkflowError> {
        let _lifecycle_permit = self
            .inner
            .lifecycle_gate
            .acquire()
            .await
            .map_err(|_| WorkflowError::Internal)?;
        self.accepting()?;
        let setup = self.inner.setup()?;
        let (record, plan) = self.find_by_binding(&setup, contract).await?;
        let workflow_id = record.workflow_id;
        {
            let mut runs = self.inner.runs();
            while runs.tasks.try_join_next().is_some() {}
            if let Some(active) = runs.active.get(&workflow_id) {
                return if active.start_request_id == request_id {
                    Ok(StartResponse {
                        workflow_id,
                        phase: record.phase,
                        parallelism: ParallelismPlan::Sequential,
                        duplicate: true,
                    })
                } else {
                    Err(WorkflowError::AlreadyRunning)
                };
            }
        }
        if !resumable(record.phase) && record.phase != WorkflowPhase::Running {
            return Err(WorkflowError::InvalidPhase);
        }
        let verifier = Arc::clone(&setup);
        tokio::task::spawn_blocking(move || verifier.config.check_verifier_unchanged())
            .await
            .map_err(|_| WorkflowError::Internal)??;
        let units = units_of(&setup, &record, &plan, workflow_id)?;
        if let Some(code) = self.settle_recovered_leases(workflow_id).await? {
            return self.block(workflow_id, record.phase, code).await;
        }
        let parallelism = self.parallelism(&setup, &record, &plan, &units).await?;
        let width = match parallelism {
            ParallelismPlan::Blocked { reason } => {
                return self.block(workflow_id, record.phase, reason).await;
            }
            ParallelismPlan::Sequential => 1,
            ParallelismPlan::Parallel { width } => usize::try_from(width).unwrap_or(1),
        };
        if record.phase != WorkflowPhase::Running {
            transition(
                &self.inner.writer,
                workflow_id,
                WorkflowPhase::Running,
                None,
            )
            .await?;
        }
        let manager = WorkspaceManager::new(
            setup.config.git_executable.clone(),
            setup.canonical_root.clone(),
            record.base_commit.clone(),
        )
        .await
        .map_err(|_| WorkflowError::Workspace)?;
        let state = setup.state.clone();
        let ledger = tokio::task::spawn_blocking(move || state.read_ledger(workflow_id))
            .await
            .map_err(|_| WorkflowError::Internal)??;
        let (control, receiver) = watch::channel(Control::Run);
        let context = RunContext {
            writer: self.inner.writer.clone(),
            dispatch: self.inner.dispatch.clone(),
            setup,
            plan,
            workflow_id,
            manager,
            control: receiver,
            ledger: Mutex::new(ledger),
            workspace_gate: tokio::sync::Semaphore::new(1),
            reviewer_turns: tokio::sync::Mutex::new(()),
        };
        let weak = Arc::downgrade(&self.inner);
        let mut runs = self.inner.runs();
        if !runs.accepting {
            return Err(WorkflowError::ShuttingDown);
        }
        if runs.active.contains_key(&workflow_id) {
            return Err(WorkflowError::AlreadyRunning);
        }
        runs.active.insert(
            workflow_id,
            ActiveRun {
                control,
                start_request_id: request_id,
            },
        );
        runs.reports.remove(&workflow_id);
        runs.tasks.spawn(drive(weak, context, units, width));
        Ok(StartResponse {
            workflow_id,
            phase: WorkflowPhase::Running,
            parallelism,
            duplicate: false,
        })
    }

    /// The workflow whose start binding is `contract`, with its plan.
    async fn find_by_binding(
        &self,
        setup: &Arc<WorkflowSetup>,
        contract: Sha256Digest,
    ) -> Result<
        (
            vibemux_workflow::workflow_record::WorkflowRecord,
            WorkflowPlan,
        ),
        WorkflowError,
    > {
        let mut after = None;
        loop {
            let writer = self.inner.writer.clone();
            let records =
                call_writer(move || writer.workflows_after(after, WORKFLOW_PAGE_SIZE)).await?;
            let Some(last) = records.last() else {
                return Err(WorkflowError::ContractMismatch);
            };
            after = Some(last.workflow_id);
            let state = setup.state.clone();
            if let Some(found) = tokio::task::spawn_blocking(move || {
                records.into_iter().find_map(|record| {
                    let plan = state.read_plan(record.workflow_id).ok()?;
                    (start_binding(&record, &plan).ok()? == contract).then_some((record, plan))
                })
            })
            .await
            .map_err(|_| WorkflowError::Internal)?
            {
                return Ok(found);
            }
        }
    }

    /// Ends this workflow's quarantined leases whose attempts have settled
    /// since the restart. Returns a block reason while any attempt still
    /// waits for the operator (`vibemuxctl dispatch cancel <request-id>`).
    async fn settle_recovered_leases(
        &self,
        workflow_id: Uuid,
    ) -> Result<Option<&'static str>, WorkflowError> {
        let snapshot = self.snapshot(workflow_id).await?;
        for lease in snapshot
            .leases
            .iter()
            .filter(|lease| lease.state == LeaseState::Quarantined)
        {
            let mut settled = true;
            for attempt in snapshot.attempts.iter().filter(|attempt| {
                attempt.lease_id == lease.lease_id && attempt.lease_generation == lease.generation
            }) {
                let phase = self.inner.dispatch.status(attempt.request_id).await?.phase;
                settled &= phase.is_terminal();
            }
            if !settled {
                return Ok(Some(RECOVERY_PENDING_CODE));
            }
            revoke_lease(
                &self.inner.writer,
                lease,
                RECOVERED_LEASE_CODE,
                self.inner.heartbeat_ms(),
            )
            .await?;
        }
        Ok(None)
    }

    /// The parallelism the eligible assigned slots allow. A slot this
    /// workflow's own live lease holds counts as available to it.
    async fn parallelism(
        &self,
        setup: &WorkflowSetup,
        record: &vibemux_workflow::workflow_record::WorkflowRecord,
        plan: &WorkflowPlan,
        units: &[Unit],
    ) -> Result<ParallelismPlan, WorkflowError> {
        let writer = self.inner.writer.clone();
        let mut slots = call_writer(move || writer.workflow_slots()).await?;
        let snapshot = self.snapshot(record.workflow_id).await?;
        let own: BTreeSet<&SpecIdentifier> = snapshot
            .leases
            .iter()
            .filter(|lease| lease.state == LeaseState::Active)
            .map(|lease| &lease.slot_id)
            .collect();
        for slot in &mut slots {
            if own.contains(&slot.slot_id) {
                slot.leased_generation = None;
            }
        }
        let now = now_ms();
        let request = |required| EligibilityRequest {
            required,
            class: record.evidence_class,
            interface_mode: InterfaceMode::Structured,
            now_ms: now,
            excluded_slots: &[],
            allow_unknown_quota: true,
        };
        let writable = evaluate_eligibility(&slots, request(&WRITABLE_REQUIREMENTS));
        let review = evaluate_eligibility(&slots, request(&REVIEW_REQUIREMENTS));
        let assigned: BTreeSet<&SpecIdentifier> =
            units.iter().map(|unit| &unit.slot.slot_id).collect();
        let eligible: BTreeSet<&SpecIdentifier> = assigned
            .iter()
            .copied()
            .filter(|slot_id| writable.eligible.contains(slot_id))
            .collect();
        let reviewer_ready = review.eligible.contains(&plan.reviewer_slot);
        if eligible.len() != assigned.len()
            || !reviewer_ready
            || setup.config.slot(&plan.reviewer_slot).is_none()
        {
            return Ok(ParallelismPlan::Blocked {
                reason: SLOT_INELIGIBLE_CODE,
            });
        }
        Ok(plan_parallelism(
            eligible.len(),
            units.len(),
            plan.policy.max_parallel_slots,
        ))
    }

    /// Records a block reason before any work starts.
    async fn block(
        &self,
        workflow_id: Uuid,
        phase: WorkflowPhase,
        reason: &str,
    ) -> Result<StartResponse, WorkflowError> {
        let writer = &self.inner.writer;
        if phase == WorkflowPhase::Paused {
            transition(writer, workflow_id, WorkflowPhase::Running, None).await?;
        }
        let record = if phase == WorkflowPhase::Blocked {
            // Blocked again: refresh the reason through `running`.
            transition(writer, workflow_id, WorkflowPhase::Running, None).await?;
            transition(writer, workflow_id, WorkflowPhase::Blocked, Some(reason)).await?
        } else {
            transition(writer, workflow_id, WorkflowPhase::Blocked, Some(reason)).await?
        };
        Ok(StartResponse {
            workflow_id,
            phase: record.phase,
            parallelism: ParallelismPlan::Blocked {
                reason: block_reason(reason),
            },
            duplicate: false,
        })
    }

    /// Asks the running coordinator to stop at its next turn boundary, or
    /// pauses a workflow no coordinator drives.
    pub async fn pause(&self, workflow_id: Uuid) -> Result<PhaseControl, WorkflowError> {
        let _lifecycle_permit = self
            .inner
            .lifecycle_gate
            .acquire()
            .await
            .map_err(|_| WorkflowError::Internal)?;
        let signalled = self.signal(workflow_id, Control::Pause);
        let record = self.record(workflow_id).await?;
        if signalled {
            return Ok(PhaseControl {
                workflow_id,
                phase: record.phase,
                signalled,
            });
        }
        let phase = match record.phase {
            WorkflowPhase::Paused => WorkflowPhase::Paused,
            WorkflowPhase::Running => {
                transition(&self.inner.writer, workflow_id, WorkflowPhase::Paused, None)
                    .await?
                    .phase
            }
            _ => return Err(WorkflowError::InvalidPhase),
        };
        Ok(PhaseControl {
            workflow_id,
            phase,
            signalled,
        })
    }

    /// Commits the cancellation request, then signals the coordinator (or
    /// finishes the cancellation when none runs). A result that arrives
    /// after the request can no longer be accepted: the writer refuses
    /// acceptance outside `running` and `integrating`.
    pub async fn cancel(&self, workflow_id: Uuid) -> Result<PhaseControl, WorkflowError> {
        let _lifecycle_permit = self
            .inner
            .lifecycle_gate
            .acquire()
            .await
            .map_err(|_| WorkflowError::Internal)?;
        let record = self.record(workflow_id).await?;
        let writer = &self.inner.writer;
        match record.phase {
            WorkflowPhase::Cancelled => {
                return Ok(PhaseControl {
                    workflow_id,
                    phase: record.phase,
                    signalled: false,
                });
            }
            WorkflowPhase::Accepted | WorkflowPhase::Failed => {
                return Err(WorkflowError::InvalidPhase);
            }
            WorkflowPhase::Prepared => {
                let record =
                    transition(writer, workflow_id, WorkflowPhase::Cancelled, None).await?;
                return Ok(PhaseControl {
                    workflow_id,
                    phase: record.phase,
                    signalled: false,
                });
            }
            WorkflowPhase::CancelRequested => {}
            _ => {
                // The signal goes first, so a coordinator that observes the
                // new phase already knows it is stopping.
                self.signal(workflow_id, Control::Cancel);
                transition(
                    writer,
                    workflow_id,
                    WorkflowPhase::CancelRequested,
                    Some("operator_cancel"),
                )
                .await?;
            }
        }
        let signalled = self.signal(workflow_id, Control::Cancel);
        if !signalled {
            finish_cancel(writer, workflow_id, self.inner.heartbeat_ms()).await?;
        }
        let record = self.record(workflow_id).await?;
        Ok(PhaseControl {
            workflow_id,
            phase: record.phase,
            signalled,
        })
    }

    fn signal(&self, workflow_id: Uuid, control: Control) -> bool {
        let runs = self.inner.runs();
        runs.active
            .get(&workflow_id)
            .is_some_and(|active| active.control.send(control).is_ok())
    }

    /// Content-free status of one workflow.
    pub async fn status(&self, workflow_id: Uuid) -> Result<WorkflowStatus, WorkflowError> {
        let located = self.locate(workflow_id).await?;
        Ok(self.status_of(&located))
    }

    fn status_of(&self, located: &Located) -> WorkflowStatus {
        let snapshot = &located.snapshot;
        let workflow_id = snapshot.record.workflow_id;
        let start_contract = located
            .plan
            .as_ref()
            .and_then(|plan| start_binding(&snapshot.record, plan).ok());
        let (running_here, last_run) = {
            let runs = self.inner.runs();
            (
                runs.active.contains_key(&workflow_id),
                runs.reports.get(&workflow_id).cloned(),
            )
        };
        status_view(
            snapshot,
            &located.planned,
            start_contract,
            running_here,
            last_run,
        )
    }

    /// One page of the content-free export, starting at `cursor`.
    pub async fn export(
        &self,
        workflow_id: Uuid,
        cursor: usize,
    ) -> Result<ExportPage, WorkflowError> {
        let located = self.locate(workflow_id).await?;
        let status = self.status_of(&located);
        let items = export_items(&located.snapshot, &status)?;
        export_page(workflow_id, &items, cursor)
    }

    /// Slot observations, their routes, and writable eligibility now.
    pub async fn slots(&self) -> Result<SlotsResponse, WorkflowError> {
        let setup = self.inner.setup()?;
        let writer = self.inner.writer.clone();
        let slots = call_writer(move || writer.workflow_slots()).await?;
        let writable = evaluate_eligibility(
            &slots,
            EligibilityRequest {
                required: &WRITABLE_REQUIREMENTS,
                class: setup.config.config.evidence_class,
                interface_mode: InterfaceMode::Structured,
                now_ms: now_ms(),
                excluded_slots: &[],
                allow_unknown_quota: true,
            },
        );
        let routes = setup
            .config
            .config
            .slots
            .iter()
            .map(|slot| SlotRoute {
                slot_id: slot.slot_id.clone(),
                route_label: setup.config.route_label(slot),
                executes: setup.slot_protocols.contains_key(&slot.slot_id),
            })
            .collect();
        Ok(SlotsResponse {
            evidence_class: setup.config.config.evidence_class,
            slots,
            routes,
            writable,
        })
    }

    /// Grants a recorded bundle to a worker session of the same
    /// cooperative workflow.
    pub async fn share(
        &self,
        bundle_id: Uuid,
        to_session: Uuid,
    ) -> Result<ShareResponse, WorkflowError> {
        self.accepting()?;
        let setup = self.inner.setup()?;
        let located = self
            .search(|located| find_bundle(&located.snapshot, bundle_id).is_some())
            .await?
            .ok_or(WorkflowError::ShareInvalid)?;
        let snapshot = located.snapshot;
        let workflow_id = snapshot.record.workflow_id;
        let bundle = find_bundle(&snapshot, bundle_id)
            .cloned()
            .ok_or(WorkflowError::ShareInvalid)?;
        let plan = located.plan.ok_or(WorkflowError::NotFound)?;
        let (message, duplicate) = share_bundle(
            &self.inner.writer,
            &setup.state,
            &plan,
            &snapshot,
            &bundle,
            to_session,
        )
        .await?;
        Ok(ShareResponse {
            workflow_id,
            message_id: message.envelope.message_id,
            bundle_id,
            recipient: message.recipient_label,
            recipient_sequence: message.recipient_sequence,
            duplicate,
        })
    }

    /// The content-free view of one session.
    pub async fn inspect(&self, session: Uuid) -> Result<SessionInspection, WorkflowError> {
        let located = self.find_session(session).await?;
        inspect_session(&located.snapshot, &located.planned, session)
            .ok_or(WorkflowError::SessionNotFound)
    }

    async fn find_session(&self, session: Uuid) -> Result<Located, WorkflowError> {
        self.search(|located| {
            inspect_session(&located.snapshot, &located.planned, session).is_some()
        })
        .await?
        .ok_or(WorkflowError::SessionNotFound)
    }

    /// Prompt `index` of one session: a sent prompt from the opt-in
    /// content store or, at the last index, the exact next prompt when it
    /// is fixed. This is vendor-bound content; only the explicit
    /// `session inspect --prompts` request returns it.
    pub async fn session_prompt(
        &self,
        session: Uuid,
        index: usize,
    ) -> Result<SessionPrompt, WorkflowError> {
        let setup = self.inner.setup()?;
        let located = self.find_session(session).await?;
        let inspection = inspect_session(&located.snapshot, &located.planned, session)
            .ok_or(WorkflowError::SessionNotFound)?;
        let total = inspection.attempts.len() + 1;
        if index >= total {
            return Err(WorkflowError::RequestInvalid);
        }
        if let Some(attempt) = inspection.attempts.get(index) {
            let state = setup.state.clone();
            let digest = attempt.rendered_prompt_sha256;
            let text = tokio::task::spawn_blocking(move || state.read_content(digest))
                .await
                .map_err(|_| WorkflowError::Internal)?
                .ok()
                .flatten()
                .and_then(|bytes| String::from_utf8(bytes).ok());
            let unavailable_reason = text.is_none().then_some(PROMPT_NOT_RETAINED_CODE);
            return Ok(SessionPrompt {
                session_id: session,
                index,
                total,
                source: PromptSource::Sent,
                request_id: Some(attempt.request_id),
                rendered_prompt_sha256: Some(digest),
                text,
                unavailable_reason,
            });
        }
        let (text, unavailable_reason) = match &located.plan {
            Some(plan) => next_prompt(&located.snapshot, plan, &inspection, session),
            None => (None, Some("plan_unavailable")),
        };
        Ok(SessionPrompt {
            session_id: session,
            index,
            total,
            source: PromptSource::Next,
            request_id: None,
            rendered_prompt_sha256: text.as_ref().map(|text| Sha256Digest::of(text.as_bytes())),
            text,
            unavailable_reason,
        })
    }

    /// Native terminal attach is not implemented: every session runs as a
    /// structured turn. The refusal is explicit rather than a log panel.
    pub async fn attach(&self, session: Uuid) -> Result<(), WorkflowError> {
        self.inspect(session).await?;
        Err(WorkflowError::NativeTuiUnavailable)
    }

    /// The versions of the prompt policy.
    pub async fn policy_versions(&self) -> Result<Vec<PolicyVersionRecord>, WorkflowError> {
        let writer = self.inner.writer.clone();
        Ok(call_writer(move || writer.prompt_policy_versions()).await?)
    }

    /// Rolls the active prompt policy back to its parent. Prepared and
    /// running workflows keep the contracts they were admitted with.
    pub async fn rollback_policy(&self) -> Result<PolicyVersionRecord, WorkflowError> {
        let writer = self.inner.writer.clone();
        Ok(call_writer(move || writer.rollback_prompt_policy(OffsetDateTime::now_utc())).await?)
    }

    /// Deletes every retained text of a workflow from the opt-in content
    /// store and records the deletion. Candidate blobs are not content.
    pub async fn purge_content(&self, workflow_id: Uuid) -> Result<PurgeResponse, WorkflowError> {
        let setup = self.inner.setup()?;
        self.record(workflow_id).await?;
        let _content_permit = setup.state.content_permit().await?;
        let state = setup.state.clone();
        let entries = tokio::task::spawn_blocking(move || state.content_index())
            .await
            .map_err(|_| WorkflowError::Internal)??;
        let mut deleted = 0;
        let mut already_absent = 0;
        let mut digests: Vec<Sha256Digest> = entries
            .iter()
            .filter(|entry| entry.workflow_id == workflow_id)
            .map(|entry| entry.sha256)
            .collect();
        digests.sort();
        digests.dedup();
        for digest in digests {
            if self
                .delete_content(&setup.state, workflow_id, digest, None, "operator_purge")
                .await?
            {
                deleted += 1;
            } else {
                already_absent += 1;
            }
        }
        Ok(PurgeResponse {
            workflow_id,
            deleted,
            already_absent,
        })
    }

    /// Deletes retained content whose retention ended.
    pub async fn sweep_expired_content(&self) -> Result<u32, WorkflowError> {
        let setup = self.inner.setup()?;
        let _content_permit = setup.state.content_permit().await?;
        let state = setup.state.clone();
        let entries = tokio::task::spawn_blocking(move || state.content_index())
            .await
            .map_err(|_| WorkflowError::Internal)??;
        let now = now_ms();
        let mut deleted = 0;
        let expired: BTreeSet<(Uuid, Sha256Digest)> = entries
            .into_iter()
            .filter(|entry| {
                entry
                    .retain_until_ms
                    .is_some_and(|deadline| deadline <= now)
            })
            .map(|entry| (entry.workflow_id, entry.sha256))
            .collect();
        for (workflow_id, digest) in expired {
            if self
                .delete_content(
                    &setup.state,
                    workflow_id,
                    digest,
                    Some(now),
                    "retention_expired",
                )
                .await?
            {
                deleted += 1;
            }
        }
        Ok(deleted)
    }

    async fn delete_content(
        &self,
        state: &StateFiles,
        workflow_id: Uuid,
        digest: Sha256Digest,
        expired_before: Option<u64>,
        reason: &str,
    ) -> Result<bool, WorkflowError> {
        let files = state.clone();
        let (released, last_owner) = tokio::task::spawn_blocking(move || {
            files.release_content_owner(workflow_id, digest, expired_before)
        })
        .await
        .map_err(|_| WorkflowError::Internal)??;
        if last_owner {
            let writer = self.inner.writer.clone();
            let reason = reason.to_string();
            call_writer(move || {
                writer.delete_workflow_content(digest, reason.clone(), OffsetDateTime::now_utc())
            })
            .await?;
        }
        Ok(released)
    }

    /// Stops admission and asks every coordinator to stop at its next turn
    /// boundary, without waiting.
    pub fn begin_shutdown(&self) {
        let mut runs = self.inner.runs();
        runs.accepting = false;
        for active in runs.active.values() {
            let _ = active.control.send(Control::Pause);
        }
    }

    /// Stops every coordinator, waiting up to [`SHUTDOWN_GRACE`] for them
    /// to reach a turn boundary; the rest are aborted. A workflow left
    /// running is blocked at the next start. Runs before the dispatch
    /// service shuts down.
    pub async fn shutdown(&self) {
        self.begin_shutdown();
        let mut tasks = std::mem::take(&mut self.inner.runs().tasks);
        let joined = timeout(SHUTDOWN_GRACE, async {
            while tasks.join_next().await.is_some() {}
        })
        .await;
        if joined.is_err() {
            tasks.shutdown().await;
        }
    }

    /// Waits until no coordinator of `workflow_id` runs, up to `limit`.
    /// For tests and the offline acceptance runner.
    pub async fn wait_idle(&self, workflow_id: Uuid, limit: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            if !self.inner.runs().active.contains_key(&workflow_id) {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    async fn record(
        &self,
        workflow_id: Uuid,
    ) -> Result<vibemux_workflow::workflow_record::WorkflowRecord, WorkflowError> {
        let writer = self.inner.writer.clone();
        call_writer(move || writer.workflow_record(workflow_id))
            .await?
            .ok_or(WorkflowError::NotFound)
    }

    pub(super) fn setup_handle(&self) -> Result<Arc<WorkflowSetup>, WorkflowError> {
        self.inner.setup()
    }

    pub(super) fn writer(&self) -> &WriterHandle {
        &self.inner.writer
    }

    pub(super) fn evaluation_lock(&self) -> &tokio::sync::Mutex<()> {
        &self.inner.evaluation
    }

    pub(super) async fn snapshot(
        &self,
        workflow_id: Uuid,
    ) -> Result<WorkflowSnapshot, WorkflowError> {
        let writer = self.inner.writer.clone();
        call_writer(move || writer.workflow_snapshot(workflow_id))
            .await?
            .ok_or(WorkflowError::NotFound)
    }

    /// A workflow's snapshot with its private plan and planned sessions,
    /// when the config and plan are readable.
    async fn locate(&self, workflow_id: Uuid) -> Result<Located, WorkflowError> {
        let snapshot = self.snapshot(workflow_id).await?;
        let Ok(setup) = self.inner.setup() else {
            return Ok(Located {
                snapshot,
                plan: None,
                planned: Vec::new(),
            });
        };
        let state = setup.state.clone();
        let plan = tokio::task::spawn_blocking(move || state.read_plan(workflow_id))
            .await
            .map_err(|_| WorkflowError::Internal)?
            .ok();
        let planned = plan
            .as_ref()
            .map(|plan| planned_sessions(&setup, plan, workflow_id))
            .unwrap_or_default();
        Ok(Located {
            snapshot,
            plan,
            planned,
        })
    }

    /// The first recent workflow matching `predicate`.
    async fn search(
        &self,
        predicate: impl Fn(&Located) -> bool,
    ) -> Result<Option<Located>, WorkflowError> {
        let mut after = None;
        loop {
            let writer = self.inner.writer.clone();
            let records =
                call_writer(move || writer.workflows_after(after, WORKFLOW_PAGE_SIZE)).await?;
            let Some(last) = records.last() else {
                return Ok(None);
            };
            after = Some(last.workflow_id);
            for record in records {
                let located = self.locate(record.workflow_id).await?;
                if predicate(&located) {
                    return Ok(Some(located));
                }
            }
        }
    }
}

/// A workflow snapshot with what only this daemon's state knows about it.
struct Located {
    snapshot: WorkflowSnapshot,
    plan: Option<WorkflowPlan>,
    planned: Vec<PlannedSession>,
}

/// The worker sessions a plan assigns, with their routes.
fn planned_sessions(
    setup: &WorkflowSetup,
    plan: &WorkflowPlan,
    workflow_id: Uuid,
) -> Vec<PlannedSession> {
    plan.assignments
        .iter()
        .flat_map(|(task_key, slots)| {
            slots.iter().filter_map(move |slot_id| {
                let slot = setup.config.slot(slot_id)?;
                Some(PlannedSession {
                    session_id: session_id(workflow_id, task_key, slot_id),
                    task_key: task_key.clone(),
                    slot_id: slot_id.clone(),
                    harness: slot.harness,
                    route: setup.config.route_label(slot),
                })
            })
        })
        .collect()
}

/// Runs one coordinator and records how it ended. A cancellation committed
/// while the coordinator was already ending is finished here.
async fn drive(inner: Weak<ServiceInner>, context: RunContext, units: Vec<Unit>, width: usize) {
    let workflow_id = context.workflow_id;
    let report = coordinator::run(&context, units, width).await;
    if report.end != RunEnd::Cancelled {
        if let Ok(record) = context.record().await {
            if record.phase == WorkflowPhase::CancelRequested {
                let _ = finish_cancel(
                    &context.writer,
                    workflow_id,
                    context.setup.config.config.heartbeat_ms,
                )
                .await;
            }
        }
    }
    if let Some(inner) = inner.upgrade() {
        let mut runs = inner.runs();
        runs.active.remove(&workflow_id);
        runs.reports.insert(workflow_id, report);
    }
}

/// The worker units of a plan, in task then slot order.
fn units_of(
    setup: &WorkflowSetup,
    record: &vibemux_workflow::workflow_record::WorkflowRecord,
    plan: &WorkflowPlan,
    workflow_id: Uuid,
) -> Result<Vec<Unit>, WorkflowError> {
    let mut units = Vec::new();
    for (task_key, slots) in &plan.assignments {
        let task = record.task(task_key).ok_or(WorkflowError::Internal)?;
        for slot_id in slots {
            let slot = setup
                .config
                .slot(slot_id)
                .ok_or(WorkflowError::SlotUnavailable)?
                .clone();
            units.push(Unit {
                task_key: task_key.clone(),
                session_id: session_id(workflow_id, task_key, slot_id),
                slot,
                contract_id: task.contract_id,
            });
        }
    }
    Ok(units)
}

/// The active version of a policy history.
pub(super) fn active_policy(versions: &[PolicyVersionRecord]) -> Option<&PolicyVersionRecord> {
    versions
        .iter()
        .find(|record| record.version.state == vibemux_workflow::optimizer::PolicyState::Active)
}

/// The static form of a block reason.
fn block_reason(reason: &str) -> &'static str {
    match reason {
        RECOVERY_PENDING_CODE => RECOVERY_PENDING_CODE,
        "no_eligible_slot" => "no_eligible_slot",
        "zero_concurrency_limit" => "zero_concurrency_limit",
        "no_runnable_task" => "no_runnable_task",
        _ => SLOT_INELIGIBLE_CODE,
    }
}

/// The exact next prompt of a worker session, when it does not depend on
/// a gate outcome or a routed message: the first implement turn of a
/// session that has not run yet.
fn next_prompt(
    snapshot: &WorkflowSnapshot,
    plan: &WorkflowPlan,
    inspection: &SessionInspection,
    session: Uuid,
) -> (Option<String>, Option<&'static str>) {
    if inspection.session.role != "worker" {
        return (None, Some("reviewer_prompt_depends_on_candidate"));
    }
    if !resumable(snapshot.record.phase) {
        return (None, Some("workflow_not_stopped"));
    }
    if snapshot
        .attempts
        .iter()
        .any(|attempt| attempt.session_id == session)
    {
        return (None, Some("next_prompt_depends_on_gate_outcome"));
    }
    let rendered = render_turn(
        plan,
        snapshot.record.policy_digest,
        &TurnRequest {
            task_key: &inspection.session.task_key,
            purpose: TurnPurpose::Implement,
            turn_number: 1,
            notes: &[],
            context_blocks: &[],
            reviewed_candidate: None,
        },
    );
    match rendered {
        Ok(prompt) => (Some(prompt.text), None),
        Err(_) => (None, Some("render_failed")),
    }
}
