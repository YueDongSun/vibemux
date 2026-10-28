//! Daemon-side harness request dispatch (ADR 029).
//!
//! [`HarnessDispatchService`] owns every effect the pure
//! `vibemux_harness::dispatch` rules leave to the daemon: it pins the
//! operator route config at startup, admits requests through the single
//! writer, runs each attempt in its own task (spawn, contain, exchange,
//! fenced finish), keeps bounded in-memory transcripts, and cancels and
//! joins every attempt before the writer shuts down. Canonical state is
//! written only by the writer; this module holds no database handle.
//!
//! Admission reserves the canonical project root, so at most one attempt
//! runs at a time; probes are separately limited to one at a time and write
//! nothing.

mod broker_owner;
#[cfg(feature = "test_helpers")]
mod qualification_controller;
#[cfg(feature = "test_helpers")]
pub(crate) use qualification_controller::FixtureExecutionGuard;
mod config_loader;
pub(crate) use broker_owner::BrokerScope;
mod executor;
pub(crate) mod git_head;
mod lease_watch;
#[cfg(feature = "test_helpers")]
pub mod native_process;
#[cfg(not(feature = "test_helpers"))]
pub(crate) mod native_process;
mod output_page;
mod transcript_store;

use std::{
    collections::HashMap,
    future::Future,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError, Weak},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::OffsetDateTime;
use tokio::{
    sync::{oneshot, watch},
    task::JoinSet,
    time::timeout,
};
use uuid::Uuid;
use vibemux_harness::{
    AgentKind,
    dispatch::{
        AttemptOutcome, DispatchError, DispatchLimits, DispatchPhase, DispatchRequest,
        DispatchRoute, NativeProtocol, ProtocolSession, attempt,
        capture_budget::{CaptureBudget, CaptureSummary},
        events::ProcessSummary,
        launch_spec::{LaunchSpec, SessionMode, build_launch_spec},
        request::DISPATCH_REQUEST_SCHEMA_VERSION,
        route_config::DispatchCatalogEntry,
        writable_profile::{self, WritableGrant, build_writable_launch_spec},
    },
};
use vibemux_store::{
    AttemptPurpose, HarnessDispatchAdmission, HarnessDispatchRecord, WorkflowAttemptAdmission,
};
use vibemux_workflow::{Sha256Digest, SpecIdentifier};

use crate::{WriterError, WriterHandle};

pub use config_loader::{DISPATCH_CONFIG_FILE_NAME, LoadedDispatchConfig, load_dispatch_config};
pub use output_page::{
    DispatchOutputFragment, DispatchOutputPage, MAX_OUTPUT_PAGE_FRAGMENTS,
    MAX_OUTPUT_PAGE_RAW_BYTES, OutputCursor, OutputPageLimits, OutputReassembler,
    OutputReassemblyError, ReassembledRecord, build_output_page,
};
pub use transcript_store::{
    LiveCapture, MAX_RETAINED_TRANSCRIPT_BYTES, MAX_RETAINED_TRANSCRIPTS, TranscriptPage,
};

use executor::ExecutionPlan;
pub(crate) use executor::call_writer;
use native_process::{LaunchPlan, NativeProcess};
use transcript_store::TranscriptStore;

/// File name of the launch trampoline next to the daemon executable.
pub const LAUNCH_TRAMPOLINE_NAME: &str = "vibemux_launch_trampoline";
/// Extra time shutdown waits for attempts beyond the configured grace: the
/// forced kill, reaping, and the final writer call.
const SHUTDOWN_JOIN_MARGIN: Duration = Duration::from_secs(15);
/// Upper bound on a probe's deadline, whatever the configured attempt
/// deadline: an initialize handshake that takes longer has failed.
pub const PROBE_DEADLINE_MS: u64 = 60_000;

/// Where the service finds its inputs.
#[derive(Clone)]
pub struct DispatchServiceSettings {
    pub project_root: PathBuf,
    pub config_path: PathBuf,
    /// `None` resolves [`LAUNCH_TRAMPOLINE_NAME`] next to the running
    /// executable.
    pub trampoline: Option<PathBuf>,
}

impl DispatchServiceSettings {
    /// Production layout: the config lives in the daemon state directory.
    #[must_use]
    pub fn for_project(project_root: &Path, state_dir: &Path) -> Self {
        Self {
            project_root: project_root.to_path_buf(),
            config_path: state_dir.join(DISPATCH_CONFIG_FILE_NAME),
            trampoline: None,
        }
    }
}

impl std::fmt::Debug for DispatchServiceSettings {
    /// Redacted: paths never reach a log.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DispatchServiceSettings")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum DispatchServiceError {
    #[error("{0}")]
    Dispatch(DispatchError),
    #[error("{0}")]
    Writer(WriterError),
}

impl DispatchServiceError {
    #[must_use]
    pub fn code(&self) -> &str {
        match self {
            Self::Dispatch(error) => error.code(),
            Self::Writer(error) => error.code(),
        }
    }
}

impl From<DispatchError> for DispatchServiceError {
    fn from(error: DispatchError) -> Self {
        Self::Dispatch(error)
    }
}

impl From<WriterError> for DispatchServiceError {
    /// A store rejection that carries a dispatch code surfaces as that code.
    fn from(error: WriterError) -> Self {
        if let WriterError::Store { code } = &error
            && let Some(dispatch) = DispatchError::from_code(code)
        {
            return Self::Dispatch(dispatch);
        }
        Self::Writer(error)
    }
}

/// Answer to a submission.
#[derive(Clone, Debug)]
pub struct DispatchReceipt {
    pub record: HarnessDispatchRecord,
    /// The request id was already admitted with the same content; nothing
    /// new was written.
    pub duplicate: bool,
    /// Sequence of the event that last changed the record.
    pub sequence: u64,
}

/// Content-free result of an initialize-only probe; also the Control v5
/// probe payload.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeReport {
    pub harness: AgentKind,
    pub protocol: NativeProtocol,
    pub outcome: AttemptOutcome,
    pub error_code: Option<DispatchError>,
    pub process: ProcessSummary,
    pub capture: CaptureSummary,
}

/// The workflow binding of one structured worker or reviewer turn
/// (ADR 031 §5). The store re-checks every field inside the admission
/// transaction.
#[derive(Clone, Debug)]
pub(crate) struct WorkflowTurnBinding {
    pub workflow_id: Uuid,
    pub expected_workflow_version: u64,
    pub lease_id: Uuid,
    pub lease_generation: u64,
    pub task_key: SpecIdentifier,
    pub contract_id: Sha256Digest,
    pub purpose: AttemptPurpose,
    pub session_id: Uuid,
}

/// One workflow turn: a writable (or read-only review) structured turn in
/// an owned working directory instead of the project root.
pub(crate) struct WorkflowTurn {
    pub request_id: Uuid,
    pub harness: AgentKind,
    pub prompt: String,
    pub grant: WritableGrant,
    pub task_spec_digest: Sha256Digest,
    pub evidence_class: vibemux_workflow::slots::EvidenceClass,
    pub route_label: String,
    /// Owned worktree or materialized candidate directory.
    pub working_directory: PathBuf,
    pub base_commit: String,
    pub binding: WorkflowTurnBinding,
    /// Sticky heartbeat failure, observed by the registered execution owner.
    pub lease_loss: watch::Receiver<bool>,
    pub broker_scope: BrokerScope,
}

/// What a finished workflow turn left behind.
#[derive(Clone, Debug)]
pub(crate) struct WorkflowTurnOutcome {
    /// The terminal dispatch record.
    pub record: HarnessDispatchRecord,
    /// The final assistant message, if the protocol reported one.
    pub final_text: Option<String>,
    /// The caller's atomic message delivery ran after admission.
    pub delivery_applied: bool,
}

/// Cloneable handle; clones share one service.
#[derive(Clone)]
pub struct HarnessDispatchService {
    inner: Arc<ServiceInner>,
}

impl std::fmt::Debug for HarnessDispatchService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HarnessDispatchService")
            .finish_non_exhaustive()
    }
}

struct ServiceInner {
    writer: WriterHandle,
    setup: Result<DispatchSetup, DispatchError>,
    transcripts: Arc<TranscriptStore>,
    registry: Mutex<AttemptRegistry>,
    #[cfg(feature = "test_helpers")]
    fixture_dispatch_cleanup_loss: std::sync::atomic::AtomicBool,
    #[cfg(feature = "test_helpers")]
    fixture_verifier_cleanup_loss: std::sync::atomic::AtomicBool,
}

/// Inputs fixed at startup.
struct DispatchSetup {
    config: LoadedDispatchConfig,
    /// Canonical project root in the form a child process and the resource
    /// key use: on Windows without the verbatim prefix.
    working_directory: String,
    trampoline: Option<PathBuf>,
    #[cfg(feature = "test_helpers")]
    fixture_guard: Option<FixtureExecutionGuard>,
}

/// What one workflow turn launches with.
struct WorkflowLaunch {
    launch: LaunchPlan,
    /// The canonical owned directory, in the form a child process uses.
    working_directory: String,
    resource_key: Sha256Digest,
    /// Recorded as both the harness profile and the effective route profile.
    profile: &'static str,
    broker: Option<broker_owner::BrokerOwner>,
}

struct AttemptRegistry {
    accepting: bool,
    /// Cancel signal of each attempt task still running.
    attempts: HashMap<Uuid, watch::Sender<bool>>,
    /// Cancel signal of the running probe; also its one-at-a-time flag.
    probe: Option<watch::Sender<bool>>,
    tasks: JoinSet<()>,
    /// Exact owners retained after unproven cleanup. Admission stops and
    /// durable attempts stay nonterminal for explicit reconciliation.
    cleanup_pending: HashMap<Uuid, NativeProcess>,
}

impl HarnessDispatchService {
    /// Loads and pins the route config. Never fails: a missing or invalid
    /// config leaves dispatch unavailable with that code, while status and
    /// cancel (for example, acknowledging a recovered attempt) still work.
    pub async fn start(writer: WriterHandle, settings: DispatchServiceSettings) -> Self {
        let setup = tokio::task::spawn_blocking(move || prepare(&settings))
            .await
            .unwrap_or(Err(DispatchError::Internal));
        Self::with_setup(writer, setup)
    }

    #[cfg(feature = "test_helpers")]
    pub(crate) async fn start_with_fixture_guard(
        writer: WriterHandle,
        settings: DispatchServiceSettings,
        guard: FixtureExecutionGuard,
    ) -> Self {
        let setup = tokio::task::spawn_blocking(move || {
            let mut setup = prepare(&settings)?;
            setup.fixture_guard = Some(guard);
            Ok(setup)
        })
        .await
        .unwrap_or(Err(DispatchError::Internal));
        Self::with_setup(writer, setup)
    }

    /// A service without a config, for daemons started without project
    /// paths.
    #[must_use]
    pub fn unconfigured(writer: WriterHandle) -> Self {
        Self::with_setup(writer, Err(DispatchError::Unconfigured))
    }

    fn with_setup(writer: WriterHandle, setup: Result<DispatchSetup, DispatchError>) -> Self {
        Self {
            inner: Arc::new(ServiceInner {
                writer,
                setup,
                transcripts: Arc::default(),
                #[cfg(feature = "test_helpers")]
                fixture_dispatch_cleanup_loss: std::sync::atomic::AtomicBool::new(false),
                #[cfg(feature = "test_helpers")]
                fixture_verifier_cleanup_loss: std::sync::atomic::AtomicBool::new(false),
                registry: Mutex::new(AttemptRegistry {
                    accepting: true,
                    attempts: HashMap::new(),
                    probe: None,
                    tasks: JoinSet::new(),
                    cleanup_pending: HashMap::new(),
                }),
            }),
        }
    }

    pub(crate) fn process_cleanup_pending(&self) -> bool {
        !self.inner.registry().cleanup_pending.is_empty()
    }

    pub(crate) fn retain_unsettled_process(&self, process: NativeProcess) {
        let mut registry = self.inner.registry();
        registry.accepting = false;
        registry.cleanup_pending.insert(Uuid::new_v4(), process);
    }

    /// Suppresses one already-observed fixture completion receipt. No OS
    /// failure is fabricated, and only a registered synthetic owner can arm it.
    #[cfg(feature = "test_helpers")]
    pub(crate) fn lose_next_fixture_cleanup(&self, verifier: bool) -> Result<(), DispatchError> {
        if self.inner.setup()?.fixture_guard.is_none() {
            return Err(DispatchError::ExecutionDisabled);
        }
        let flag = if verifier {
            &self.inner.fixture_verifier_cleanup_loss
        } else {
            &self.inner.fixture_dispatch_cleanup_loss
        };
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    #[cfg(feature = "test_helpers")]
    pub(crate) fn take_fixture_verifier_cleanup_loss(&self) -> bool {
        self.inner
            .fixture_verifier_cleanup_loss
            .swap(false, std::sync::atomic::Ordering::SeqCst)
    }

    #[cfg(feature = "test_helpers")]
    pub(crate) fn fixture_retained_process_count(&self) -> usize {
        self.inner.registry().cleanup_pending.len()
    }

    /// Content-free projection of the pinned routes.
    pub fn catalog(&self) -> Result<Vec<DispatchCatalogEntry>, DispatchServiceError> {
        Ok(self.inner.setup()?.config.config().catalog())
    }

    /// Digest of the pinned route config; a decision records it so a
    /// decided dispatch is refused after a config change (ADR 034 §11).
    pub fn config_digest(
        &self,
    ) -> Result<vibemux_harness::dispatch::Sha256Digest, DispatchServiceError> {
        Ok(self.inner.setup()?.config.digest())
    }

    /// Admits a request and starts its attempt. Gates run before any state
    /// changes; the store re-checks detection and the working-directory
    /// reservation inside the admission transaction. Repeating a request id
    /// with the same content returns the stored record.
    pub async fn submit(
        &self,
        request: DispatchRequest,
    ) -> Result<DispatchReceipt, DispatchServiceError> {
        self.submit_bound(request, None).await
    }

    /// [`Self::submit`] for a request a recorded decision chose (ADR 034
    /// §11). Every gate still runs; the store also checks the decision
    /// binding and consumes the decision in the admission transaction. The
    /// request that consumed it may repeat, subject to the same gates and
    /// request fingerprint as any replay. The receipt's `sequence` is the
    /// dispatch admission event; the `decision_consumed` event follows it.
    pub async fn submit_decided(
        &self,
        decision_id: Uuid,
        request: DispatchRequest,
    ) -> Result<DispatchReceipt, DispatchServiceError> {
        self.submit_bound(request, Some(decision_id)).await
    }

    async fn submit_bound(
        &self,
        request: DispatchRequest,
        decision_id: Option<Uuid>,
    ) -> Result<DispatchReceipt, DispatchServiceError> {
        request.validate()?;
        let setup = self.inner.setup()?;
        self.inner.ensure_accepting()?;
        let detected = self.detected(request.harness).await?;
        let config = setup.config.config();
        let route = config.execution_route(&request, detected)?;
        let launch = setup.launch_plan(route, SessionMode::Execute)?;
        let session = ProtocolSession::for_execution(
            route,
            request.prompt.clone(),
            setup.working_directory.clone(),
        )?;
        let budget = CaptureBudget::new(route.protocol, &config.limits)?;
        let root = launch.working_directory.clone();
        let base_commit = tokio::task::spawn_blocking(move || git_head::read_head_commit(&root))
            .await
            .unwrap_or(Err(DispatchError::Internal))?;
        let admission = HarnessDispatchAdmission {
            request_id: request.request_id,
            harness: request.harness,
            protocol: route.protocol,
            prompt: request.prompt_digest(),
            config_sha256: setup.config.digest(),
            resource_key: attempt::resource_key(&setup.working_directory),
            base_commit,
            timestamp: OffsetDateTime::now_utc(),
        };
        let writer = self.inner.writer.clone();
        // Safe to repeat: a repeated admission is an idempotent duplicate,
        // and a decided admission replays for the request that consumed it.
        let commit = match decision_id {
            None => call_writer(move || writer.admit_harness_dispatch(admission.clone())).await?,
            Some(decision_id) => {
                call_writer(move || {
                    writer
                        .admit_decided_dispatch(decision_id, admission.clone())
                        .map(|commit| commit.dispatch)
                })
                .await?
            }
        };
        let receipt = DispatchReceipt {
            duplicate: commit.event.is_none(),
            record: commit.record,
            sequence: commit.sequence,
        };
        if receipt.record.phase == DispatchPhase::Admitted {
            let plan = ExecutionPlan {
                #[cfg(feature = "test_helpers")]
                forget_cleanup: false,
                broker: None,
                lease_loss: None,
                launch,
                session,
                budget,
                limits: config.limits,
            };
            self.launch(request.request_id, plan).await;
        }
        Ok(receipt)
    }

    /// Admits one workflow turn through the writer and runs it to its
    /// fenced finish in the owned working directory. Admission reserves
    /// that directory (not the project root), so turns in distinct
    /// worktrees may overlap. Cancel it with [`Self::cancel`].
    pub(crate) async fn run_workflow_turn<BeforeLaunch, BeforeLaunchFuture>(
        &self,
        turn: WorkflowTurn,
        before_launch: BeforeLaunch,
    ) -> Result<WorkflowTurnOutcome, DispatchServiceError>
    where
        BeforeLaunch: FnOnce(Uuid) -> BeforeLaunchFuture,
        BeforeLaunchFuture: Future<Output = Result<(), DispatchServiceError>>,
    {
        let setup = self.inner.setup()?;
        self.inner.ensure_accepting()?;
        let request = DispatchRequest {
            schema_version: DISPATCH_REQUEST_SCHEMA_VERSION,
            request_id: turn.request_id,
            harness: turn.harness,
            prompt: turn.prompt,
        };
        request.validate()?;
        let detected = self.detected(turn.harness).await?;
        let config = setup.config.config();
        let route = config.execution_route(&request, detected)?;
        // The coordinator polls every turn of a step in one task, so the
        // blocking launch preparation must not run on it.
        let inner = Arc::clone(&self.inner);
        let launch_route = route.clone();
        let (grant, evidence_class) = (turn.grant, turn.evidence_class);
        let directory = turn.working_directory;
        let broker_scope = turn.broker_scope;
        let broker_binding = turn.binding.clone();
        let request_id = turn.request_id;
        let prepared = tokio::task::spawn_blocking(move || {
            let mut prepared =
                inner
                    .setup()?
                    .workflow_launch(&launch_route, grant, evidence_class, &directory)?;
            if inner.setup()?.uses_file_broker(launch_route.harness) {
                let broker = broker_owner::BrokerOwner::prepare(
                    &directory,
                    broker_scope,
                    request_id,
                    grant,
                    &broker_binding,
                    prepared.profile,
                )?;
                inner.setup()?.configure_broker(
                    launch_route.harness,
                    &broker,
                    &mut prepared.launch,
                )?;
                prepared.working_directory = child_path_text(&prepared.launch.working_directory)
                    .ok_or(DispatchError::WorkingDirectoryInvalid)?;
                prepared.broker = Some(broker);
            }
            Ok(prepared)
        })
        .await
        .unwrap_or(Err(DispatchError::Internal))?;
        let session = ProtocolSession::for_execution(
            route,
            request.prompt.clone(),
            prepared.working_directory,
        )?;
        let budget = CaptureBudget::new(route.protocol, &config.limits)?;
        let binding = turn.binding;
        let admission = WorkflowAttemptAdmission {
            workflow_id: binding.workflow_id,
            expected_workflow_version: binding.expected_workflow_version,
            lease_id: binding.lease_id,
            lease_generation: binding.lease_generation,
            task_key: binding.task_key,
            contract_id: binding.contract_id,
            purpose: binding.purpose,
            session_id: binding.session_id,
            execution: vibemux_store::WorkflowExecutionBinding {
                task_spec_digest: turn.task_spec_digest,
                effective_tools: [
                    (
                        turn.grant.read_files,
                        vibemux_workflow::task_spec::ToolName::ReadFiles,
                    ),
                    (
                        turn.grant.search_files,
                        vibemux_workflow::task_spec::ToolName::SearchFiles,
                    ),
                    (
                        turn.grant.edit_files,
                        vibemux_workflow::task_spec::ToolName::EditFiles,
                    ),
                    (
                        turn.grant.write_files,
                        vibemux_workflow::task_spec::ToolName::WriteFiles,
                    ),
                    (
                        turn.grant.run_dev_tests,
                        vibemux_workflow::task_spec::ToolName::RunDevTests,
                    ),
                ]
                .into_iter()
                .filter_map(|(allowed, tool)| allowed.then_some(tool))
                .collect(),
                harness_profile: prepared.profile.to_string(),
                route_config_digest: setup.config.digest(),
                evidence_class: turn.evidence_class,
                route_label: turn.route_label,
                effective_route: vibemux_workflow::execution_authority::EffectiveRouteIdentity {
                    harness: route.harness,
                    protocol: route.protocol,
                    model_alias: route.model.clone(),
                    endpoint: None,
                    config_digest: setup.config.digest(),
                    profile: prepared.profile.to_string(),
                },
            },
            dispatch: HarnessDispatchAdmission {
                request_id: request.request_id,
                harness: request.harness,
                protocol: route.protocol,
                prompt: request.prompt_digest(),
                config_sha256: setup.config.digest(),
                resource_key: prepared.resource_key,
                base_commit: turn.base_commit,
                timestamp: OffsetDateTime::now_utc(),
            },
        };
        let writer = self.inner.writer.clone();
        let commit = call_writer(move || writer.admit_workflow_attempt(admission.clone())).await?;
        let protocol = route.protocol;
        let request_id = request.request_id;
        let mut final_text = None;
        let mut delivery_applied = false;
        if commit.dispatch.record.phase == DispatchPhase::Admitted {
            if let Err(error) = before_launch(request_id).await {
                let writer = self.inner.writer.clone();
                call_writer(move || {
                    writer.cancel_harness_dispatch(request_id, OffsetDateTime::now_utc())
                })
                .await?;
                return Err(error);
            }
            delivery_applied = true;
            let plan = ExecutionPlan {
                #[cfg(feature = "test_helpers")]
                forget_cleanup: self
                    .inner
                    .fixture_dispatch_cleanup_loss
                    .swap(false, std::sync::atomic::Ordering::SeqCst),
                broker: prepared.broker,
                lease_loss: Some(turn.lease_loss.clone()),
                launch: prepared.launch,
                session,
                budget,
                limits: config.limits,
            };
            let (done, finished) = oneshot::channel();
            if self.launch_workflow_turn(request_id, plan, protocol, done, turn.lease_loss) {
                final_text = finished.await.ok().flatten();
            } else {
                let writer = self.inner.writer.clone();
                let _ = call_writer(move || {
                    writer.cancel_harness_dispatch(request_id, OffsetDateTime::now_utc())
                })
                .await;
            }
        }
        let record = self.status(request_id).await?;
        Ok(WorkflowTurnOutcome {
            record,
            final_text,
            delivery_applied,
        })
    }

    /// Starts a workflow turn task; `false` if the service stopped
    /// accepting or the request already runs.
    fn launch_workflow_turn(
        &self,
        request_id: Uuid,
        plan: ExecutionPlan,
        protocol: NativeProtocol,
        done: oneshot::Sender<Option<String>>,
        lease_loss: watch::Receiver<bool>,
    ) -> bool {
        let mut registry = self.inner.registry();
        if !registry.accepting || registry.attempts.contains_key(&request_id) {
            return false;
        }
        let (signal, cancel) = watch::channel(false);
        registry.attempts.insert(request_id, signal.clone());
        let writer = self.inner.writer.clone();
        let transcripts = Arc::clone(&self.inner.transcripts);
        let inner = Arc::downgrade(&self.inner);
        registry.tasks.spawn(async move {
            let execution = executor::run_attempt_with_final_text(
                writer,
                transcripts,
                request_id,
                plan,
                cancel,
                Some(protocol),
            );
            let completion = lease_watch::join_with_lease_loss(lease_loss, signal, execution).await;
            deregister(&inner, request_id, completion.unsettled);
            let _ = done.send(completion.final_text);
        });
        while registry.tasks.try_join_next().is_some() {}
        true
    }

    /// Canonical working-directory text and the resource key a workflow
    /// turn in `directory` reserves. Blocking.
    pub(crate) fn workflow_resource_key(directory: &Path) -> Option<(String, Sha256Digest)> {
        let canonical = std::fs::canonicalize(directory).ok()?;
        if !canonical.is_dir() {
            return None;
        }
        let text = child_path_text(&canonical)?;
        let key = attempt::resource_key(&text);
        Some((text, key))
    }

    /// The protocol of `harness`'s route, if the pinned config lets it
    /// execute.
    pub(crate) fn execution_protocol(&self, harness: AgentKind) -> Option<NativeProtocol> {
        let setup = self.inner.setup().ok()?;
        let route = setup
            .config
            .config()
            .routes
            .iter()
            .find(|route| route.harness == harness)?;
        (route.enabled && route.allow_execution && route.protocol.supports_execution())
            .then_some(route.protocol)
    }

    pub(crate) fn execution_profile(
        &self,
        harness: AgentKind,
        evidence_class: vibemux_workflow::slots::EvidenceClass,
    ) -> Option<&'static str> {
        let setup = self.inner.setup().ok()?;
        if evidence_class == vibemux_workflow::slots::EvidenceClass::Fixture {
            #[cfg(feature = "test_helpers")]
            setup.fixture_guard.as_ref()?;
            #[cfg(not(feature = "test_helpers"))]
            return None;
        }
        let route = setup
            .config
            .config()
            .routes
            .iter()
            .find(|route| route.harness == harness)?;
        setup.workflow_profile(route).ok()
    }

    /// The launch trampoline, for other contained daemon processes.
    pub(crate) fn trampoline(&self) -> Option<PathBuf> {
        self.inner.setup().ok()?.trampoline.clone()
    }

    pub async fn status(
        &self,
        request_id: Uuid,
    ) -> Result<HarnessDispatchRecord, DispatchServiceError> {
        let writer = self.inner.writer.clone();
        call_writer(move || writer.harness_dispatch(request_id))
            .await?
            .ok_or(DispatchServiceError::Dispatch(DispatchError::NotFound))
    }

    /// Commits the cancellation, then signals a running attempt. The
    /// executor sends the protocol cancel, waits the grace period, and kills
    /// the tree if the vendor does not end the turn.
    pub async fn cancel(
        &self,
        request_id: Uuid,
    ) -> Result<HarnessDispatchRecord, DispatchServiceError> {
        let writer = self.inner.writer.clone();
        let commit = call_writer(move || {
            writer.cancel_harness_dispatch(request_id, OffsetDateTime::now_utc())
        })
        .await?;
        if commit.record.phase == DispatchPhase::CancelRequested
            && let Some(signal) = self.inner.registry().attempts.get(&request_id)
        {
            let _ = signal.send(true);
        }
        Ok(commit.record)
    }

    /// Initialize-only handshake with a detected, enabled route. Sends no
    /// prompt and writes no canonical state.
    pub async fn probe(&self, harness: AgentKind) -> Result<ProbeReport, DispatchServiceError> {
        let setup = self.inner.setup()?;
        self.inner.ensure_accepting()?;
        let detected = self.detected(harness).await?;
        let config = setup.config.config();
        let route = config.probe_route(harness, detected)?;
        let launch = setup.launch_plan(route, SessionMode::Probe)?;
        let session = ProtocolSession::for_probe(route, setup.working_directory.clone())?;
        let budget = CaptureBudget::new(route.protocol, &config.limits)?;
        let plan = ExecutionPlan {
            #[cfg(feature = "test_helpers")]
            forget_cleanup: false,
            broker: None,
            lease_loss: None,
            launch,
            session,
            budget,
            limits: DispatchLimits {
                deadline_ms: config.limits.deadline_ms.min(PROBE_DEADLINE_MS),
                ..config.limits
            },
        };
        let (report_sender, report) = oneshot::channel();
        {
            let mut registry = self.inner.registry();
            if !registry.accepting {
                return Err(DispatchError::Internal.into());
            }
            if registry.probe.is_some() {
                return Err(DispatchError::Busy.into());
            }
            let (signal, cancel) = watch::channel(false);
            registry.probe = Some(signal);
            let inner = Arc::downgrade(&self.inner);
            registry.tasks.spawn(async move {
                let execution = executor::run_probe(plan, cancel).await;
                deregister(&inner, Uuid::new_v4(), execution.unsettled);
                if let Some(inner) = inner.upgrade() {
                    inner.registry().probe = None;
                }
                let _ = report_sender.send(execution.evidence);
            });
        }
        let evidence = report
            .await
            .map_err(|_| DispatchServiceError::Dispatch(DispatchError::Internal))?;
        Ok(ProbeReport {
            harness,
            protocol: route.protocol,
            outcome: evidence.decision.outcome,
            error_code: evidence.decision.error_code,
            process: evidence.process,
            capture: evidence.capture,
        })
    }

    /// Captured records after `after_sequence`, at most `max_records`.
    /// Transcripts live in memory only: a finished one may be evicted, and
    /// none survives a restart.
    pub async fn transcript(
        &self,
        request_id: Uuid,
        after_sequence: u64,
        max_records: usize,
    ) -> Result<TranscriptPage, DispatchServiceError> {
        match self
            .inner
            .transcripts
            .records(request_id, after_sequence, max_records)
        {
            // Tell an unknown request apart from output that is gone.
            Err(DispatchError::OutputUnavailable) => {
                self.status(request_id).await?;
                Err(DispatchError::OutputUnavailable.into())
            }
            page => page.map_err(Into::into),
        }
    }

    /// The output page at `cursor`, fragmenting records larger than the
    /// page (ADR 029 §7). Like [`Self::transcript`], an unknown request is
    /// `harness_dispatch_not_found` and evicted output is
    /// `harness_dispatch_output_unavailable`.
    pub async fn output(
        &self,
        request_id: Uuid,
        cursor: OutputCursor,
        limits: OutputPageLimits,
    ) -> Result<DispatchOutputPage, DispatchServiceError> {
        match self
            .inner
            .transcripts
            .output_page(request_id, cursor, limits)
        {
            Err(DispatchError::OutputUnavailable) => {
                self.status(request_id).await?;
                Err(DispatchError::OutputUnavailable.into())
            }
            page => page.map_err(Into::into),
        }
    }

    /// Size of the transcript a running attempt is capturing; `None` once
    /// it finished or if it never started.
    #[must_use]
    pub fn live_capture(&self, request_id: Uuid) -> Option<LiveCapture> {
        self.inner.transcripts.live_capture(request_id)
    }

    /// Stops admission and signals every attempt and probe to cancel,
    /// without waiting. The Control shutdown operation calls it before the
    /// server drains connections, so a connection waiting on a probe ends
    /// promptly.
    pub fn begin_shutdown(&self) {
        let mut registry = self.inner.registry();
        registry.accepting = false;
        for signal in registry.attempts.values().chain(registry.probe.iter()) {
            let _ = signal.send(true);
        }
    }

    /// Stops admission, cancels every attempt and probe, and waits for their
    /// tasks up to the grace period plus `SHUTDOWN_JOIN_MARGIN`; tasks
    /// still running then are aborted, which kills their process trees. An
    /// attempt that could not report its finish is recovered at the next
    /// start. Runs before the writer shuts down.
    pub async fn shutdown(&self) {
        self.begin_shutdown();
        let mut tasks = std::mem::take(&mut self.inner.registry().tasks);
        let grace = self.inner.setup.as_ref().map_or(Duration::ZERO, |setup| {
            Duration::from_millis(setup.config.config().limits.shutdown_grace_ms)
        });
        let joined = timeout(grace + SHUTDOWN_JOIN_MARGIN, async {
            while tasks.join_next().await.is_some() {}
        })
        .await;
        if joined.is_err() {
            tasks.shutdown().await;
        }
    }

    async fn detected(&self, harness: AgentKind) -> Result<bool, DispatchServiceError> {
        let writer = self.inner.writer.clone();
        let rows = call_writer(move || writer.harness_rows()).await?;
        Ok(rows
            .iter()
            .any(|row| row.name == harness.command_name() && row.available))
    }

    /// Starts the attempt task unless one already runs for this request (a
    /// concurrent repeat) or the service is shutting down, in which case the
    /// admitted attempt is cancelled before anything launches.
    async fn launch(&self, request_id: Uuid, plan: ExecutionPlan) {
        let accepting = {
            let mut registry = self.inner.registry();
            if registry.accepting && !registry.attempts.contains_key(&request_id) {
                let (signal, cancel) = watch::channel(false);
                registry.attempts.insert(request_id, signal);
                let writer = self.inner.writer.clone();
                let transcripts = Arc::clone(&self.inner.transcripts);
                let inner = Arc::downgrade(&self.inner);
                registry.tasks.spawn(async move {
                    let unsettled =
                        executor::run_attempt(writer, transcripts, request_id, plan, cancel).await;
                    deregister(&inner, request_id, unsettled);
                });
                // Finished tasks need no join; drop their results.
                while registry.tasks.try_join_next().is_some() {}
            }
            registry.accepting
        };
        if !accepting {
            let writer = self.inner.writer.clone();
            let _ = call_writer(move || {
                writer.cancel_harness_dispatch(request_id, OffsetDateTime::now_utc())
            })
            .await;
        }
    }
}

impl ServiceInner {
    fn setup(&self) -> Result<&DispatchSetup, DispatchError> {
        self.setup.as_ref().map_err(|error| *error)
    }

    fn ensure_accepting(&self) -> Result<(), DispatchError> {
        if self.registry().accepting {
            Ok(())
        } else {
            Err(DispatchError::Internal)
        }
    }

    fn registry(&self) -> MutexGuard<'_, AttemptRegistry> {
        // Every update leaves the registry consistent before it can panic.
        self.registry.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl DispatchSetup {
    fn uses_file_broker(&self, harness: AgentKind) -> bool {
        #[cfg(feature = "test_helpers")]
        {
            self.fixture_guard
                .as_ref()
                .is_some_and(|guard| guard.uses_file_broker(harness))
        }
        #[cfg(not(feature = "test_helpers"))]
        {
            let _ = harness;
            false
        }
    }

    fn configure_broker(
        &self,
        harness: AgentKind,
        owner: &broker_owner::BrokerOwner,
        launch: &mut LaunchPlan,
    ) -> Result<(), DispatchError> {
        #[cfg(feature = "test_helpers")]
        {
            self.fixture_guard
                .as_ref()
                .ok_or(DispatchError::ExecutionDisabled)?
                .configure_broker(harness, owner, launch)
        }
        #[cfg(not(feature = "test_helpers"))]
        {
            let _ = (harness, owner, launch);
            Err(DispatchError::ExecutionDisabled)
        }
    }

    fn workflow_profile(&self, route: &DispatchRoute) -> Result<&'static str, DispatchError> {
        #[cfg(feature = "test_helpers")]
        if let Some(guard) = &self.fixture_guard {
            // Revalidates the registered image before admitting a fixture slot.
            guard.launch_spec(route, WritableGrant::NONE)?;
            return guard.profile_id(route.harness);
        }
        writable_profile::writable_profile_id(route.protocol)
    }

    /// Resolves the owned working directory and builds the launch plan and
    /// profile of one workflow turn. Blocking: besides the filesystem
    /// lookups, a fixture guard rehashes its pinned images here.
    fn workflow_launch(
        &self,
        route: &DispatchRoute,
        grant: WritableGrant,
        evidence_class: vibemux_workflow::slots::EvidenceClass,
        directory: &Path,
    ) -> Result<WorkflowLaunch, DispatchError> {
        let (working_directory, resource_key) =
            HarnessDispatchService::workflow_resource_key(directory)
                .ok_or(DispatchError::WorkingDirectoryInvalid)?;
        let trampoline = self
            .trampoline
            .clone()
            .ok_or(DispatchError::ExecutableUnavailable)?;
        let executable = child_path_text(self.config.executable(route.harness)?)
            .ok_or(DispatchError::ExecutableUnavailable)?;
        let (spec, profile) = self.workflow_spec(route, grant, evidence_class)?;
        Ok(WorkflowLaunch {
            launch: LaunchPlan {
                environment: None,
                trampoline,
                executable: PathBuf::from(executable),
                spec,
                working_directory: PathBuf::from(&working_directory),
            },
            working_directory,
            resource_key,
            profile,
            broker: None,
        })
    }

    /// The launch spec and profile of a workflow turn. A fixture guard
    /// revalidates the registered images once for both.
    fn workflow_spec(
        &self,
        route: &DispatchRoute,
        grant: WritableGrant,
        evidence_class: vibemux_workflow::slots::EvidenceClass,
    ) -> Result<(LaunchSpec, &'static str), DispatchError> {
        #[cfg(feature = "test_helpers")]
        if let Some(guard) = &self.fixture_guard {
            if evidence_class != vibemux_workflow::slots::EvidenceClass::Fixture {
                return Err(DispatchError::ExecutionDisabled);
            }
            // Revalidates the registered image before every fixture launch.
            let spec = guard.launch_spec(route, grant)?;
            let profile = guard.profile_id(route.harness)?;
            return Ok((spec, profile));
        }
        if evidence_class == vibemux_workflow::slots::EvidenceClass::Fixture {
            return Err(DispatchError::ExecutionDisabled);
        }
        let spec = build_writable_launch_spec(route, grant)?;
        Ok((spec, writable_profile::writable_profile_id(route.protocol)?))
    }

    fn launch_plan(
        &self,
        route: &DispatchRoute,
        mode: SessionMode,
    ) -> Result<LaunchPlan, DispatchError> {
        let trampoline = self
            .trampoline
            .clone()
            .ok_or(DispatchError::ExecutableUnavailable)?;
        let executable = child_path_text(self.config.executable(route.harness)?)
            .ok_or(DispatchError::ExecutableUnavailable)?;
        Ok(LaunchPlan {
            environment: None,
            trampoline,
            executable: PathBuf::from(executable),
            spec: build_launch_spec(route, mode)?,
            working_directory: PathBuf::from(&self.working_directory),
        })
    }
}

fn deregister(inner: &Weak<ServiceInner>, request_id: Uuid, unsettled: Option<NativeProcess>) {
    if let Some(inner) = inner.upgrade() {
        let mut registry = inner.registry();
        if let Some(process) = unsettled {
            registry.accepting = false;
            registry.cleanup_pending.insert(request_id, process);
        }
        registry.attempts.remove(&request_id);
    }
}

fn prepare(settings: &DispatchServiceSettings) -> Result<DispatchSetup, DispatchError> {
    let canonical_root = std::fs::canonicalize(&settings.project_root)
        .map_err(|_| DispatchError::WorkingDirectoryInvalid)?;
    let config = load_dispatch_config(&settings.config_path, &canonical_root)?;
    let working_directory =
        child_path_text(&canonical_root).ok_or(DispatchError::WorkingDirectoryInvalid)?;
    let trampoline = match &settings.trampoline {
        Some(path) => Some(path.clone()),
        None => bundled_trampoline(),
    };
    Ok(DispatchSetup {
        config,
        working_directory,
        trampoline: trampoline.filter(|path| path.is_file()),
        #[cfg(feature = "test_helpers")]
        fixture_guard: None,
    })
}

fn bundled_trampoline() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let name = format!("{LAUNCH_TRAMPOLINE_NAME}{}", std::env::consts::EXE_SUFFIX);
    Some(executable.parent()?.join(name))
}

/// A canonical path as UTF-8 without the Windows verbatim prefix, which a
/// vendor CLI does not expect in its program path, its working directory,
/// or a protocol field.
pub(crate) fn child_path_text(path: &Path) -> Option<String> {
    let text = path.to_str()?;
    #[cfg(windows)]
    {
        if let Some(unc) = text.strip_prefix("\\\\?\\UNC\\") {
            return Some(format!("\\\\{unc}"));
        }
        Some(text.strip_prefix("\\\\?\\").unwrap_or(text).to_string())
    }
    #[cfg(not(windows))]
    {
        Some(text.to_string())
    }
}
