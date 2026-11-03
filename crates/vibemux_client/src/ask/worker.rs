#![forbid(unsafe_code)]
//! I/O adapters for Ask: the daemon-backed dispatch port and the worker
//! threads the GUI starts for an Ask, the route catalog, and the one-click
//! setup. Each worker owns a small current-thread runtime, reports through a
//! bounded channel, and wakes the GUI when it has news.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    time::Duration,
};

use uuid::Uuid;
use vibemux_harness::{
    AgentKind, HarnessRow,
    dispatch::{
        DispatchRequest, NativeProtocol, request::DISPATCH_REQUEST_SCHEMA_VERSION,
        route_config::ExecutablePolicy,
    },
};
use vibemuxd::{
    control::{ControlClient, ControlError, ControlGeneration, HarnessDispatchOutputQuery},
    harness_dispatch::DispatchOutputPage,
    process::DaemonPaths,
};

use super::{
    driver::{self, AskOperation, DispatchPort, DriverControl, DriverTiming},
    observation::{
        AskAdmission, AskCanonicalStatus, AskObservation, AskObserverState, AskSubmissionState,
        SubmitError,
    },
    routes::{self, SkipReason},
    session::{self, AskEnd, AskPhase, AskUpdate},
};
use crate::{
    change_notifier::ChangeNotifier,
    daemon_control::{self, DaemonCommand, DaemonControlResult},
    daemon_session::DaemonSession,
};

/// How often a running Ask reads output and status.
pub const POLL_INTERVAL: Duration = Duration::from_millis(400);
/// Updates buffered for the GUI; intermediate ones are dropped when full.
const UPDATE_CAPACITY: usize = 64;
/// The code the daemon reports when the folder has no dispatch config.
pub const UNCONFIGURED: &str = "harness_dispatch_unconfigured";
/// Setup never replaces an operator's routes.
const EXISTING_CONFIG: &str = "This folder already has .vibemux\\harness_dispatch.json; VibeMux leaves it as it is. Restart the daemon after editing it.";

struct ControlPort {
    paths: DaemonPaths,
    session: DaemonSession,
    generation_changed: bool,
    last_generation: Option<ControlGeneration>,
}

impl ControlPort {
    fn client(&mut self) -> Result<ControlClient, String> {
        let (client, restarted) = self.session.connect(&self.paths)?;
        let generation = client.generation();
        self.generation_changed |= restarted
            || self
                .last_generation
                .as_ref()
                .is_some_and(|previous| *previous != generation);
        self.last_generation = Some(generation);
        Ok(client)
    }
}

impl DispatchPort for ControlPort {
    async fn submit(&mut self, request: &DispatchRequest) -> Result<AskAdmission, SubmitError> {
        let client = self.client().map_err(|code| SubmitError {
            code,
            uncertain: false,
        })?;
        client
            .harness_dispatch_submit(request)
            .await
            .map(AskAdmission::from)
            .map_err(|error| {
                self.session.invalidate();
                // Only a positively identified local/remote refusal proves no
                // admission. A lost response is reconciled by the same UUID.
                let uncertain = !matches!(
                    error,
                    ControlError::FrameTooLarge
                        | ControlError::InvalidRequest
                        | ControlError::Unauthorized
                        | ControlError::UnsupportedVersion
                ) && !matches!(&error, ControlError::Remote { code } if definite_refusal(code));
                SubmitError {
                    code: error.code().to_string(),
                    uncertain,
                }
            })
    }

    async fn status(&mut self, request_id: Uuid) -> Result<AskCanonicalStatus, String> {
        let client = self.client()?;
        client
            .harness_dispatch_status(request_id)
            .await
            .map(AskCanonicalStatus::from)
            .map_err(|error| {
                self.session.invalidate();
                error.code().to_string()
            })
    }

    async fn output(
        &mut self,
        query: HarnessDispatchOutputQuery,
    ) -> Result<DispatchOutputPage, String> {
        let client = self.client()?;
        client
            .harness_dispatch_output(query)
            .await
            .map_err(|error| {
                self.session.invalidate();
                error.code().to_string()
            })
    }

    async fn cancel(&mut self, request_id: Uuid) -> Result<(), String> {
        let client = self.client()?;
        if self.generation_changed {
            return Err("ask_daemon_restarted".to_string());
        }
        client
            .harness_dispatch_cancel(request_id)
            .await
            .map(|_| ())
            .map_err(|error| {
                self.session.invalidate();
                error.code().to_string()
            })
    }

    fn generation_changed(&self) -> bool {
        self.generation_changed
    }
}

fn definite_refusal(code: &str) -> bool {
    matches!(
        code,
        "harness_dispatch_unconfigured"
            | "harness_dispatch_route_unavailable"
            | "harness_dispatch_execution_disabled"
            | "harness_dispatch_not_detected"
            | "harness_dispatch_invalid_request"
            | "harness_dispatch_invalid_prompt"
            | "harness_dispatch_busy"
            | "harness_dispatch_conflict"
            | "harness_dispatch_config_invalid"
            | "harness_dispatch_config_too_large"
            | "harness_dispatch_config_route_invalid"
            | "harness_dispatch_config_executable_invalid"
            | "harness_dispatch_config_environment_invalid"
    )
}

/// Independently cloneable switches; neither switch is canonical completion.
#[derive(Clone)]
pub struct AskControl {
    cancel: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

impl AskControl {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
    /// Stops only observation. Call after the app's bounded cancellation and
    /// reconciliation period; the daemon retains authority over its process.
    pub fn stop_observing(&self) {
        self.stop.store(true, Ordering::Release);
    }
}

struct WorkerLifecycle {
    control: AskControl,
    worker: Option<std::thread::JoinHandle<()>>,
    stop_on_drop: bool,
}

impl WorkerLifecycle {
    fn is_finished(&self) -> bool {
        self.worker
            .as_ref()
            .is_none_or(std::thread::JoinHandle::is_finished)
    }
    fn join(mut self) -> std::thread::Result<()> {
        self.control.stop_observing();
        self.worker
            .take()
            .map_or(Ok(()), std::thread::JoinHandle::join)
    }
}

impl Drop for WorkerLifecycle {
    fn drop(&mut self) {
        if self.stop_on_drop {
            self.control.stop_observing();
        }
    }
}

/// Legacy presentation receiver retained for egui/TUI consumers.
pub struct AskHandle {
    pub request_id: Uuid,
    pub updates: Receiver<AskUpdate>,
    lifecycle: WorkerLifecycle,
}

impl AskHandle {
    fn from_worker(
        request_id: Uuid,
        updates: Receiver<AskUpdate>,
        mut lifecycle: WorkerLifecycle,
    ) -> Self {
        // egui's reset requests cancel and immediately drops the receiver.
        // Keep its existing bounded cancellation reconciliation alive; native
        // collectors instead own explicit observer stop/join through their handle.
        lifecycle.stop_on_drop = false;
        Self {
            request_id,
            updates,
            lifecycle,
        }
    }

    pub fn cancel(&self) {
        self.lifecycle.control.cancel();
    }
}

/// Native replacement snapshots. Its owner must continuously drain updates,
/// including while the webview is hidden, and supervise this worker.
pub struct ObservedAskHandle {
    pub request_id: Uuid,
    pub updates: Receiver<AskObservation>,
    lifecycle: WorkerLifecycle,
}

impl ObservedAskHandle {
    /// Test-only supervised worker seam; never included by default builds.
    #[cfg(any(test, feature = "test_support"))]
    pub fn from_test_worker(
        request_id: Uuid,
        updates: Receiver<AskObservation>,
        cancel: Arc<AtomicBool>,
        stop: Arc<AtomicBool>,
        worker: std::thread::JoinHandle<()>,
    ) -> Self {
        Self {
            request_id,
            updates,
            lifecycle: WorkerLifecycle {
                control: AskControl { cancel, stop },
                worker: Some(worker),
                stop_on_drop: true,
            },
        }
    }

    pub fn cancel(&self) {
        self.lifecycle.control.cancel();
    }
    pub fn control(&self) -> AskControl {
        self.lifecycle.control.clone()
    }
    pub fn is_finished(&self) -> bool {
        self.lifecycle.is_finished()
    }
    /// Close the receiver before joining, unblocking a terminal delivery.
    /// No caller may hold registry locks while invoking this method.
    pub fn join(self) -> std::thread::Result<()> {
        drop(self.updates);
        self.lifecycle.join()
    }
}

fn runtime() -> Option<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()
}

fn failed(request_id: Uuid, code: &str, submitting: bool) -> AskObservation {
    let mut observation = AskObservation::pending(request_id);
    observation.update.phase = AskPhase::Finished;
    observation.update.end = Some(AskEnd::Failed(code.to_string()));
    observation.submission = if submitting {
        AskSubmissionState::NotSubmitted
    } else {
        AskSubmissionState::Uncertain
    };
    observation.observer = AskObserverState::Failed;
    observation.observer_error = Some(code.to_string());
    observation
}

/// Starts an independent one-turn Ask. Existing consumers retain their API.
pub fn spawn_ask(
    project_root: PathBuf,
    harness: AgentKind,
    prompt: String,
    notifier: Arc<dyn ChangeNotifier>,
) -> std::io::Result<AskHandle> {
    let request_id = Uuid::new_v4();
    let operation = submit_operation(harness, prompt, request_id)?;
    let (updates, lifecycle) = spawn_worker(
        WorkerTarget::ProjectRoot(project_root),
        operation,
        notifier,
        |observation| observation.update,
    )?;
    Ok(AskHandle::from_worker(request_id, updates, lifecycle))
}

/// Starts one Ask using a caller-reserved dispatch ID. This function does not
/// deduplicate calls; the desktop must atomically bind its submission key
/// before calling it. The worker submits exactly once and never replaces IDs.
pub fn spawn_ask_with_id(
    project_root: PathBuf,
    harness: AgentKind,
    prompt: String,
    request_id: Uuid,
    notifier: Arc<dyn ChangeNotifier>,
) -> std::io::Result<ObservedAskHandle> {
    let operation = submit_operation(harness, prompt, request_id)?;
    let (updates, lifecycle) = spawn_worker(
        WorkerTarget::ProjectRoot(project_root),
        operation,
        notifier,
        |observation| observation,
    )?;
    Ok(ObservedAskHandle {
        request_id,
        updates,
        lifecycle,
    })
}

/// Attaches only to a known request: never submits, configures, starts a
/// daemon, or implicitly acknowledges recovery. Cancellation stays explicit.
pub fn spawn_observer(
    project_root: PathBuf,
    harness: AgentKind,
    request_id: Uuid,
    notifier: Arc<dyn ChangeNotifier>,
) -> std::io::Result<ObservedAskHandle> {
    validate_target(harness, request_id)?;
    let operation = AskOperation::Observe {
        request_id,
        harness,
    };
    let (updates, lifecycle) = spawn_worker(
        WorkerTarget::ProjectRoot(project_root),
        operation,
        notifier,
        |observation| observation,
    )?;
    Ok(ObservedAskHandle {
        request_id,
        updates,
        lifecycle,
    })
}

/// Integration-test seam for a temporary project's private runtime root.
#[cfg(any(test, feature = "test_support"))]
pub fn spawn_ask_for_paths(
    paths: DaemonPaths,
    harness: AgentKind,
    prompt: String,
    request_id: Uuid,
    notifier: Arc<dyn ChangeNotifier>,
) -> std::io::Result<ObservedAskHandle> {
    let operation = submit_operation(harness, prompt, request_id)?;
    let (updates, lifecycle) = spawn_worker(
        WorkerTarget::Paths(Box::new(paths)),
        operation,
        notifier,
        |observation| observation,
    )?;
    Ok(ObservedAskHandle {
        request_id,
        updates,
        lifecycle,
    })
}

/// Observation-only integration-test seam; uses the production driver.
#[cfg(any(test, feature = "test_support"))]
pub fn spawn_observer_for_paths(
    paths: DaemonPaths,
    harness: AgentKind,
    request_id: Uuid,
    notifier: Arc<dyn ChangeNotifier>,
) -> std::io::Result<ObservedAskHandle> {
    validate_target(harness, request_id)?;
    let operation = AskOperation::Observe {
        request_id,
        harness,
    };
    let (updates, lifecycle) = spawn_worker(
        WorkerTarget::Paths(Box::new(paths)),
        operation,
        notifier,
        |observation| observation,
    )?;
    Ok(ObservedAskHandle {
        request_id,
        updates,
        lifecycle,
    })
}

fn validate_target(harness: AgentKind, request_id: Uuid) -> std::io::Result<()> {
    if request_id.is_nil() || !matches!(harness, AgentKind::Claude | AgentKind::Codex) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "harness_dispatch_invalid_request",
        ));
    }
    Ok(())
}

fn submit_operation(
    harness: AgentKind,
    prompt: String,
    request_id: Uuid,
) -> std::io::Result<AskOperation> {
    validate_target(harness, request_id)?;
    let request = DispatchRequest {
        schema_version: DISPATCH_REQUEST_SCHEMA_VERSION,
        request_id,
        harness,
        prompt,
    };
    request
        .validate()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error.code()))?;
    Ok(AskOperation::Submit(request))
}

enum WorkerTarget {
    ProjectRoot(PathBuf),
    #[cfg(any(test, feature = "test_support"))]
    Paths(Box<DaemonPaths>),
}

fn spawn_worker<T: Send + 'static>(
    target: WorkerTarget,
    operation: AskOperation,
    notifier: Arc<dyn ChangeNotifier>,
    project: fn(AskObservation) -> T,
) -> std::io::Result<(Receiver<T>, WorkerLifecycle)> {
    let request_id = match &operation {
        AskOperation::Submit(request) => request.request_id,
        AskOperation::Observe { request_id, .. } => *request_id,
    };
    let submitting = matches!(&operation, AskOperation::Submit(_));
    let (sender, updates) = mpsc::sync_channel(UPDATE_CAPACITY);
    let control = AskControl {
        cancel: Arc::new(AtomicBool::new(false)),
        stop: Arc::new(AtomicBool::new(false)),
    };
    let worker_control = control.clone();
    let worker = std::thread::Builder::new()
        .name("gui_ask".into())
        .spawn(move || {
            let deliver = |observation: AskObservation| {
                let terminal = observation.observer_finished();
                deliver_update(
                    project(observation),
                    terminal,
                    &sender,
                    &worker_control.stop,
                );
                notifier.notify();
            };
            let Some(runtime) = runtime() else {
                deliver(failed(
                    request_id,
                    "frontend_runtime_unavailable",
                    submitting,
                ));
                return;
            };
            let resolved = match target {
                WorkerTarget::ProjectRoot(project_root) => {
                    DaemonPaths::from_project_root(&project_root)
                }
                #[cfg(any(test, feature = "test_support"))]
                WorkerTarget::Paths(paths) => Ok(*paths),
            };
            let paths = match resolved {
                Ok(paths) => paths,
                Err(error) => {
                    deliver(failed(request_id, error.code(), submitting));
                    return;
                }
            };
            let mut port = ControlPort {
                paths,
                session: DaemonSession::default(),
                generation_changed: false,
                last_generation: None,
            };
            runtime.block_on(driver::drive_observed(
                &mut port,
                operation,
                DriverControl {
                    cancel: &worker_control.cancel,
                    stop: &worker_control.stop,
                },
                DriverTiming {
                    poll_interval: POLL_INTERVAL,
                    deadline: session::ASK_DEADLINE,
                },
                deliver,
            ));
        })?;
    Ok((
        updates,
        WorkerLifecycle {
            control,
            worker: Some(worker),
            stop_on_drop: true,
        },
    ))
}

fn deliver_update<T>(mut update: T, terminal: bool, sender: &SyncSender<T>, stop: &AtomicBool) {
    loop {
        match sender.try_send(update) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => return,
            Err(TrySendError::Full(pending)) => {
                if !terminal || stop.load(Ordering::Acquire) {
                    return;
                }
                update = pending;
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

/// The harnesses a folder's daemon can execute, or the code that explains
/// why none can ([`UNCONFIGURED`] when the folder has no dispatch config).
pub fn spawn_catalog(
    project_root: PathBuf,
    notifier: Arc<dyn ChangeNotifier>,
) -> Receiver<Result<Vec<AgentKind>, String>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let spawned = std::thread::Builder::new()
        .name("gui_ask_catalog".into())
        .spawn(move || {
            let result = runtime()
                .ok_or_else(|| "frontend_runtime_unavailable".to_string())
                .and_then(|runtime| runtime.block_on(catalog(&project_root)));
            let _ = sender.send(result);
            notifier.notify();
        });
    if spawned.is_err() {
        let (failed_sender, failed) = mpsc::sync_channel(1);
        let _ = failed_sender.send(Err("frontend_worker_unavailable".to_string()));
        return failed;
    }
    receiver
}

async fn catalog(project_root: &Path) -> Result<Vec<AgentKind>, String> {
    let paths =
        DaemonPaths::from_project_root(project_root).map_err(|error| error.code().to_string())?;
    let mut session = DaemonSession::default();
    let (client, _) = session.connect(&paths)?;
    let routes = client
        .harness_dispatch_catalog()
        .await
        .map_err(|error| error.code().to_string())?;
    Ok(routes
        .into_iter()
        .filter(|route| {
            route.enabled
                && route.allow_execution
                && matches!(
                    (route.harness, route.protocol),
                    (AgentKind::Claude, NativeProtocol::ClaudeStreamJson)
                        | (
                            AgentKind::Codex,
                            NativeProtocol::CodexAppServer | NativeProtocol::CodexExec
                        )
                )
        })
        .map(|route| route.harness)
        .collect())
}

/// How the one-click setup ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SetupOutcome {
    Done {
        config_path: PathBuf,
        enabled: Vec<AgentKind>,
        skipped: Vec<(AgentKind, SkipReason)>,
    },
    Failed(String),
}

/// Writes default read-only routes for the detected Claude Code and Codex
/// executables, refreshes the probe cache, and restarts the folder's daemon
/// so it loads them.
pub fn spawn_setup(
    project_root: PathBuf,
    notifier: Arc<dyn ChangeNotifier>,
) -> Receiver<SetupOutcome> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let spawned = std::thread::Builder::new()
        .name("gui_ask_setup".into())
        .spawn(move || {
            let _ = sender.send(setup(&project_root));
            notifier.notify();
        });
    if spawned.is_err() {
        let (failed_sender, failed) = mpsc::sync_channel(1);
        let _ = failed_sender.send(SetupOutcome::Failed(
            "Could not start the setup worker.".to_string(),
        ));
        return failed;
    }
    receiver
}

fn setup(project_root: &Path) -> SetupOutcome {
    if let Some(missing) = missing_dispatch_binary() {
        return SetupOutcome::Failed(format!(
            "{missing} was not found next to the GUI. Build it with: cargo build -p vibemuxd --bin vibemuxd --bin vibemux_launch_trampoline -p vibemux_cli --bin vibemuxctl"
        ));
    }
    let Some(runtime) = runtime() else {
        return SetupOutcome::Failed("Could not start the setup runtime.".to_string());
    };
    let report = runtime.block_on(vibemux_probe::run_probe(
        &vibemux_probe::ProbeConfig::from_environment(),
    ));
    if vibemux_probe::cache::write_cache(
        &report,
        &vibemux_probe::cache::default_cache_path(project_root),
    )
    .is_err()
    {
        return SetupOutcome::Failed("Could not save the harness detection cache.".to_string());
    }
    let policy = ExecutablePolicy::for_host();
    let plan = routes::plan_routes(&report, project_root, policy);
    if plan.routes.is_empty() {
        let reasons = plan
            .skipped
            .iter()
            .map(|(harness, reason)| format!("{}: {}", harness.display_name(), reason.message()))
            .collect::<Vec<_>>()
            .join("; ");
        return SetupOutcome::Failed(format!("No harness can be enabled ({reasons})."));
    }
    if routes::config_path(project_root).exists() {
        return SetupOutcome::Failed(EXISTING_CONFIG.to_string());
    }
    // Dispatch admits only a harness the daemon has recorded as detected,
    // and the daemon records detection only on request. Record it from the
    // cache saved above first, as `vibemuxctl harnesses` does, so the config
    // names only routes the daemon will admit.
    if let DaemonControlResult::Failed { message, .. } =
        daemon_control::run(DaemonCommand::Start, project_root)
    {
        return SetupOutcome::Failed(format!(
            "Starting VibeMux to record the detected harnesses failed: {message}"
        ));
    }
    let rows = match runtime.block_on(record_detection(project_root)) {
        Ok(rows) => rows,
        Err(code) => {
            return SetupOutcome::Failed(format!(
                "Recording the detected harnesses failed ({code}); nothing was written."
            ));
        }
    };
    let (detected, undetected) = routes::detected_routes(&plan.routes, &rows);
    if detected.is_empty() {
        return SetupOutcome::Failed(
            "VibeMux detected none of the harnesses to enable; nothing was written.".to_string(),
        );
    }
    let bytes = match routes::render_config(&detected) {
        Ok(bytes) => bytes,
        Err(_) => return SetupOutcome::Failed("Could not build the route config.".to_string()),
    };
    if let Err(error) = routes::validate_config(&bytes, policy) {
        return SetupOutcome::Failed(format!(
            "The daemon would reject the generated routes ({}).",
            error.code()
        ));
    }
    let config_path = match routes::write_config(project_root, &bytes) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return SetupOutcome::Failed(EXISTING_CONFIG.to_string());
        }
        Err(_) => return SetupOutcome::Failed("Could not write the route config.".to_string()),
    };
    // The daemon reads routes only at startup. Detection is already recorded
    // and survives the restart.
    if let DaemonControlResult::Failed { code, message } =
        daemon_control::run(DaemonCommand::Stop, project_root)
        && code != "daemon_not_running"
    {
        return SetupOutcome::Failed(format!(
            "Routes were written, but stopping the daemon failed: {message}"
        ));
    }
    if let DaemonControlResult::Failed { message, .. } =
        daemon_control::run(DaemonCommand::Start, project_root)
    {
        return SetupOutcome::Failed(format!(
            "Routes were written, but starting the daemon failed: {message}"
        ));
    }
    let mut skipped = plan.skipped;
    skipped.extend(
        undetected
            .into_iter()
            .map(|harness| (harness, SkipReason::NotDetected)),
    );
    SetupOutcome::Done {
        config_path,
        enabled: detected.iter().map(|route| route.harness).collect(),
        skipped,
    }
}

/// Asks the folder's daemon to record harness detection from its probe
/// cache; returns the recorded rows.
async fn record_detection(project_root: &Path) -> Result<Vec<HarnessRow>, String> {
    let paths =
        DaemonPaths::from_project_root(project_root).map_err(|error| error.code().to_string())?;
    let mut session = DaemonSession::default();
    let (client, _) = session.connect(&paths)?;
    client
        .harness_refresh()
        .await
        .map_err(|error| error.code().to_string())
}

/// The first daemon-side binary dispatch needs that is missing next to the
/// GUI: `vibemuxd` and its launch trampoline must sit together.
fn missing_dispatch_binary() -> Option<&'static str> {
    let gui = std::env::current_exe().ok()?;
    for name in ["vibemuxd", "vibemux_launch_trampoline"] {
        let candidate = gui.with_file_name(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        if !candidate.is_file() {
            return Some(name);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change_notifier::CountingNotifier;

    #[test]
    fn legacy_cancel_then_drop_still_cancels_the_admitted_attempt_once() {
        use std::sync::{Barrier, atomic::AtomicUsize};
        use vibemux_harness::dispatch::DispatchPhase;
        use vibemux_types::{RunId, TaskId};

        struct Port {
            request_id: Uuid,
            submissions: Arc<AtomicUsize>,
            cancels: Arc<AtomicUsize>,
            status_entered: Option<mpsc::Sender<()>>,
            status_gate: Arc<Barrier>,
        }

        impl Port {
            fn status_value(&self) -> AskCanonicalStatus {
                AskCanonicalStatus {
                    request_id: self.request_id,
                    task_id: TaskId::from_uuid(Uuid::from_u128(10)).expect("task"),
                    run_id: RunId::from_uuid(Uuid::from_u128(20)).expect("run"),
                    harness: AgentKind::Claude,
                    protocol: NativeProtocol::ClaudeStreamJson,
                    phase: if self.cancels.load(Ordering::Acquire) > 0 {
                        DispatchPhase::Cancelled
                    } else {
                        DispatchPhase::Running
                    },
                    version: 2 + self.cancels.load(Ordering::Acquire) as u64,
                    outcome: None,
                    error_code: None,
                }
            }
        }

        impl DispatchPort for Port {
            async fn submit(
                &mut self,
                request: &DispatchRequest,
            ) -> Result<AskAdmission, SubmitError> {
                assert_eq!(request.request_id, self.request_id);
                self.submissions.fetch_add(1, Ordering::AcqRel);
                let status = self.status_value();
                Ok(AskAdmission {
                    request_id: self.request_id,
                    task_id: status.task_id,
                    run_id: status.run_id,
                    harness: status.harness,
                    protocol: status.protocol,
                    phase: DispatchPhase::Admitted,
                    duplicate: false,
                    sequence: 1,
                })
            }
            async fn status(&mut self, request_id: Uuid) -> Result<AskCanonicalStatus, String> {
                assert_eq!(request_id, self.request_id);
                if let Some(entered) = self.status_entered.take() {
                    entered.send(()).expect("test observes admission");
                    self.status_gate.wait();
                }
                Ok(self.status_value())
            }
            async fn output(
                &mut self,
                query: HarnessDispatchOutputQuery,
            ) -> Result<DispatchOutputPage, String> {
                assert_eq!(query.request_id, self.request_id);
                Ok(DispatchOutputPage {
                    fragments: vec![],
                    next: query.cursor,
                    complete: true,
                })
            }
            async fn cancel(&mut self, request_id: Uuid) -> Result<(), String> {
                assert_eq!(request_id, self.request_id);
                self.cancels.fetch_add(1, Ordering::AcqRel);
                Ok(())
            }
        }

        let request_id = Uuid::new_v4();
        let submissions = Arc::new(AtomicUsize::new(0));
        let cancels = Arc::new(AtomicUsize::new(0));
        let status_gate = Arc::new(Barrier::new(2));
        let (status_entered, admission_ready) = mpsc::channel();
        let (sender, updates) = mpsc::sync_channel(1);
        let (finished, completion) = mpsc::channel();
        let control = AskControl {
            cancel: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
        };
        let worker_control = control.clone();
        let mut port = Port {
            request_id,
            submissions: submissions.clone(),
            cancels: cancels.clone(),
            status_entered: Some(status_entered),
            status_gate: status_gate.clone(),
        };
        let worker = std::thread::spawn(move || {
            let mut terminal_observed = false;
            runtime().expect("runtime").block_on(driver::drive_observed(
                &mut port,
                submit_operation(AgentKind::Claude, "synthetic".into(), request_id)
                    .expect("request"),
                DriverControl {
                    cancel: &worker_control.cancel,
                    stop: &worker_control.stop,
                },
                DriverTiming {
                    poll_interval: Duration::from_millis(1),
                    deadline: Duration::from_secs(1),
                },
                |observation| {
                    terminal_observed |= observation.terminal_observed();
                    let terminal = observation.observer_finished();
                    deliver_update(observation.update, terminal, &sender, &worker_control.stop);
                },
            ));
            finished
                .send(terminal_observed)
                .expect("test waits for worker");
        });
        let handle = AskHandle::from_worker(
            request_id,
            updates,
            WorkerLifecycle {
                control,
                worker: Some(worker),
                stop_on_drop: true,
            },
        );
        admission_ready
            .recv_timeout(Duration::from_secs(2))
            .expect("admitted before reset");
        // This is the legacy AskBridge::reset sequence: no native collector
        // remains after the handle and its receiver leave the window.
        handle.cancel();
        drop(handle);
        status_gate.wait();
        let terminal_observed = completion
            .recv_timeout(Duration::from_secs(2))
            .expect("worker settled");
        assert_eq!(
            submissions.load(Ordering::Acquire),
            1,
            "no replacement submit"
        );
        assert_eq!(
            cancels.load(Ordering::Acquire),
            1,
            "reset sends one cancellation"
        );
        assert!(
            terminal_observed,
            "legacy cancellation is reconciled after the receiver closes"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn control_adapter_observes_a_real_authenticated_daemon_and_restart() {
        use vibemuxd::control::DaemonControlServer;

        let project = tempfile::tempdir().expect("temporary project");
        #[cfg(windows)]
        let paths = {
            let local_app_data = project.path().join("private_local_app_data");
            std::fs::create_dir_all(&local_app_data).expect("private local app data");
            DaemonPaths::from_project_root_with_local_app_data(project.path(), &local_app_data)
                .expect("paths")
        };
        #[cfg(not(windows))]
        let paths = DaemonPaths::from_project_root(project.path()).expect("paths");
        paths.ensure_runtime_dir().expect("runtime directory");
        vibemuxd::WriterWorker::start(paths.database_path())
            .expect("initialize test store")
            .shutdown()
            .expect("stop initialization writer");
        let server = DaemonControlServer::start_for_paths(&paths)
            .await
            .expect("daemon");
        let mut port = ControlPort {
            paths: paths.clone(),
            session: DaemonSession::default(),
            generation_changed: false,
            last_generation: None,
        };
        let request = DispatchRequest {
            schema_version: DISPATCH_REQUEST_SCHEMA_VERSION,
            request_id: Uuid::new_v4(),
            harness: AgentKind::Claude,
            prompt: "synthetic prompt".into(),
        };
        let rejected = port.submit(&request).await.expect_err("unconfigured route");
        assert_eq!(rejected.code, UNCONFIGURED);
        assert!(!rejected.uncertain);
        assert!(!port.generation_changed());
        server.shutdown().await.expect("stop first daemon");
        let missing = port
            .status(request.request_id)
            .await
            .expect_err("daemon stopped");
        assert_eq!(missing, "daemon_not_running");
        let server = DaemonControlServer::start_for_paths(&paths)
            .await
            .expect("second daemon");
        let rejected = port.submit(&request).await.expect_err("still unconfigured");
        assert_eq!(rejected.code, UNCONFIGURED);
        assert!(
            port.generation_changed(),
            "restart remains detectable after a failed call invalidated the ACL cache"
        );
        server.shutdown().await.expect("stop second daemon");
    }

    #[test]
    fn reserved_ids_and_prompt_bounds_are_validated_before_starting_a_worker() {
        let project = tempfile::tempdir().expect("project");
        let notifier = Arc::new(CountingNotifier::default());
        for (harness, prompt, request_id) in [
            (AgentKind::Claude, " ".to_string(), Uuid::new_v4()),
            (AgentKind::Claude, "ok".to_string(), Uuid::nil()),
            (AgentKind::OpenCode, "ok".to_string(), Uuid::new_v4()),
            (AgentKind::Codex, "é".repeat(16_385), Uuid::new_v4()),
        ] {
            let result = spawn_ask_with_id(
                project.path().to_path_buf(),
                harness,
                prompt,
                request_id,
                notifier.clone(),
            );
            assert!(result.is_err());
        }
        assert!(submit_operation(AgentKind::Claude, "é".repeat(16_384), Uuid::new_v4()).is_ok());
    }

    #[test]
    fn terminal_delivery_unblocks_when_observation_is_stopped() {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(1).expect("fill queue");
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = std::thread::spawn(move || deliver_update(2, true, &sender, &worker_stop));
        stop.store(true, Ordering::Release);
        worker.join().expect("terminal sender returned");
        assert_eq!(receiver.recv().expect("first update"), 1);
        assert!(receiver.recv().is_err());
    }

    #[test]
    fn terminal_delivery_unblocks_when_the_owned_receiver_closes() {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(1).expect("fill queue");
        let worker =
            std::thread::spawn(move || deliver_update(2, true, &sender, &AtomicBool::new(false)));
        drop(receiver);
        worker.join().expect("terminal sender returned");
    }

    #[test]
    fn dropping_a_native_handle_still_stops_its_observer() {
        let (_sender, updates) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let (finished, completion) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while !worker_stop.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
            finished
                .send(worker_stop.load(Ordering::Acquire))
                .expect("test waits");
        });
        let handle =
            ObservedAskHandle::from_test_worker(Uuid::new_v4(), updates, cancel, stop, worker);
        drop(handle);
        assert!(
            completion
                .recv_timeout(Duration::from_secs(3))
                .expect("worker returned")
        );
    }

    #[test]
    fn observation_start_failure_does_not_claim_work_was_never_submitted() {
        let update = failed(Uuid::new_v4(), "daemon_not_running", false);
        assert_eq!(update.submission, AskSubmissionState::Uncertain);
        assert!(!update.terminal_observed());
    }

    #[test]
    fn a_reserved_id_survives_local_connection_failure_and_the_worker_is_joinable() {
        let project = tempfile::tempdir().expect("project");
        let request_id = Uuid::new_v4();
        let handle = spawn_ask_with_id(
            project.path().to_path_buf(),
            AgentKind::Claude,
            "synthetic prompt".into(),
            request_id,
            Arc::new(CountingNotifier::default()),
        )
        .expect("worker");
        assert_eq!(handle.request_id, request_id);
        let mut last = handle
            .updates
            .recv_timeout(Duration::from_secs(10))
            .expect("initial");
        while !last.observer_finished() {
            last = handle
                .updates
                .recv_timeout(Duration::from_secs(10))
                .expect("final");
        }
        assert_eq!(last.update.request_id, request_id);
        assert_eq!(last.submission, AskSubmissionState::Rejected);
        assert!(!last.terminal_observed());
        handle.join().expect("worker joined");
    }

    #[test]
    fn spawn_catalog_notifies_when_its_result_is_ready() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let notifier = Arc::new(CountingNotifier::default());
        let receiver = spawn_catalog(temp.path().to_path_buf(), notifier.clone());
        let result = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("spawn_catalog must deliver a result within the deadline");
        assert!(
            result.is_err(),
            "no daemon is running for this fresh folder"
        );
        // The worker sends the result before it calls `notify()`.
        assert!(
            notifier.wait_for_count(1, Duration::from_secs(10)),
            "spawn_catalog must notify when its result is ready"
        );
    }
}
