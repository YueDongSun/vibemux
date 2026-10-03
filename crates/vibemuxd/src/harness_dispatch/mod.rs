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

mod config_loader;
mod executor;
pub(crate) mod git_head;
pub(crate) mod native_process;
mod output_page;
mod transcript_store;

use std::{
    collections::HashMap,
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
        launch_spec::{SessionMode, build_launch_spec},
        request::DISPATCH_REQUEST_SCHEMA_VERSION,
        route_config::DispatchCatalogEntry,
        writable_profile::{WritableGrant, build_writable_launch_spec},
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
use native_process::LaunchPlan;
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
        if let WriterError::Store { code } = &error {
            if let Some(dispatch) = DispatchError::from_code(code) {
                return Self::Dispatch(dispatch);
            }
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
    /// Owned worktree or materialized candidate directory.
    pub working_directory: PathBuf,
    pub base_commit: String,
    pub binding: WorkflowTurnBinding,
}

/// What a finished workflow turn left behind.
#[derive(Clone, Debug)]
pub(crate) struct WorkflowTurnOutcome {
    /// The terminal dispatch record.
    pub record: HarnessDispatchRecord,
    /// The final assistant message, if the protocol reported one.
    pub final_text: Option<String>,
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
}

/// Inputs fixed at startup.
struct DispatchSetup {
    config: LoadedDispatchConfig,
    /// Canonical project root in the form a child process and the resource
    /// key use: on Windows without the verbatim prefix.
    working_directory: String,
    trampoline: Option<PathBuf>,
}

struct AttemptRegistry {
    accepting: bool,
    /// Cancel signal of each attempt task still running.
    attempts: HashMap<Uuid, watch::Sender<bool>>,
    /// Cancel signal of the running probe; also its one-at-a-time flag.
    probe: Option<watch::Sender<bool>>,
    tasks: JoinSet<()>,
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
                registry: Mutex::new(AttemptRegistry {
                    accepting: true,
                    attempts: HashMap::new(),
                    probe: None,
                    tasks: JoinSet::new(),
                }),
            }),
        }
    }

    /// Content-free projection of the pinned routes.
    pub fn catalog(&self) -> Result<Vec<DispatchCatalogEntry>, DispatchServiceError> {
        Ok(self.inner.setup()?.config.config().catalog())
    }

    /// Admits a request and starts its attempt. Gates run before any state
    /// changes; the store re-checks detection and the working-directory
    /// reservation inside the admission transaction. Repeating a request id
    /// with the same content returns the stored record.
    pub async fn submit(
        &self,
        request: DispatchRequest,
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
        // Safe to repeat: a repeated admission is an idempotent duplicate.
        let commit = call_writer(move || writer.admit_harness_dispatch(admission.clone())).await?;
        let receipt = DispatchReceipt {
            duplicate: commit.event.is_none(),
            record: commit.record,
            sequence: commit.sequence,
        };
        if receipt.record.phase == DispatchPhase::Admitted {
            let plan = ExecutionPlan {
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
    pub(crate) async fn run_workflow_turn(
        &self,
        turn: WorkflowTurn,
    ) -> Result<WorkflowTurnOutcome, DispatchServiceError> {
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
        let directory = turn.working_directory.clone();
        let (working_directory, resource_key) =
            tokio::task::spawn_blocking(move || Self::workflow_resource_key(&directory))
                .await
                .ok()
                .flatten()
                .ok_or(DispatchError::WorkingDirectoryInvalid)?;
        let launch = LaunchPlan {
            trampoline: setup
                .trampoline
                .clone()
                .ok_or(DispatchError::ExecutableUnavailable)?,
            executable: PathBuf::from(
                child_path_text(setup.config.executable(route.harness)?)
                    .ok_or(DispatchError::ExecutableUnavailable)?,
            ),
            spec: build_writable_launch_spec(route, turn.grant)?,
            working_directory: PathBuf::from(&working_directory),
        };
        let session = ProtocolSession::for_execution(
            route,
            request.prompt.clone(),
            working_directory.clone(),
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
            dispatch: HarnessDispatchAdmission {
                request_id: request.request_id,
                harness: request.harness,
                protocol: route.protocol,
                prompt: request.prompt_digest(),
                config_sha256: setup.config.digest(),
                resource_key,
                base_commit: turn.base_commit,
                timestamp: OffsetDateTime::now_utc(),
            },
        };
        let writer = self.inner.writer.clone();
        let commit = call_writer(move || writer.admit_workflow_attempt(admission.clone())).await?;
        let protocol = route.protocol;
        let request_id = request.request_id;
        let mut final_text = None;
        if commit.dispatch.record.phase == DispatchPhase::Admitted {
            let plan = ExecutionPlan {
                launch,
                session,
                budget,
                limits: config.limits,
            };
            let (done, finished) = oneshot::channel();
            if self.launch_workflow_turn(request_id, plan, protocol, done) {
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
        Ok(WorkflowTurnOutcome { record, final_text })
    }

    /// Starts a workflow turn task; `false` if the service stopped
    /// accepting or the request already runs.
    fn launch_workflow_turn(
        &self,
        request_id: Uuid,
        plan: ExecutionPlan,
        protocol: NativeProtocol,
        done: oneshot::Sender<Option<String>>,
    ) -> bool {
        let mut registry = self.inner.registry();
        if !registry.accepting || registry.attempts.contains_key(&request_id) {
            return false;
        }
        let (signal, cancel) = watch::channel(false);
        registry.attempts.insert(request_id, signal);
        let writer = self.inner.writer.clone();
        let transcripts = Arc::clone(&self.inner.transcripts);
        let inner = Arc::downgrade(&self.inner);
        registry.tasks.spawn(async move {
            let final_text = executor::run_attempt_with_final_text(
                writer,
                transcripts,
                request_id,
                plan,
                cancel,
                Some(protocol),
            )
            .await;
            deregister(&inner, request_id);
            let _ = done.send(final_text);
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
        if commit.record.phase == DispatchPhase::CancelRequested {
            if let Some(signal) = self.inner.registry().attempts.get(&request_id) {
                let _ = signal.send(true);
            }
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
                let evidence = executor::run_probe(plan, cancel).await;
                if let Some(inner) = inner.upgrade() {
                    inner.registry().probe = None;
                }
                let _ = report_sender.send(evidence);
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
    /// tasks up to the grace period plus [`SHUTDOWN_JOIN_MARGIN`]; tasks
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
                    executor::run_attempt(writer, transcripts, request_id, plan, cancel).await;
                    deregister(&inner, request_id);
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
            trampoline,
            executable: PathBuf::from(executable),
            spec: build_launch_spec(route, mode)?,
            working_directory: PathBuf::from(&self.working_directory),
        })
    }
}

fn deregister(inner: &Weak<ServiceInner>, request_id: Uuid) {
    if let Some(inner) = inner.upgrade() {
        inner.registry().attempts.remove(&request_id);
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
