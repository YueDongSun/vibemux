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
        mpsc::{self, Receiver, SyncSender},
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
    control::{ControlClient, HarnessDispatchOutputQuery},
    harness_dispatch::DispatchOutputPage,
    process::DaemonPaths,
};

use super::{
    routes::{self, SkipReason},
    session::{self, AskEnd, AskPhase, AskUpdate, AttemptStatus, DispatchPort},
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
}

impl ControlPort {
    fn client(&mut self) -> Result<ControlClient, String> {
        self.session
            .connect(&self.paths)
            .map(|(client, _restarted)| client)
    }
}

impl DispatchPort for ControlPort {
    async fn submit(&mut self, request: &DispatchRequest) -> Result<NativeProtocol, String> {
        let client = self.client()?;
        client
            .harness_dispatch_submit(request)
            .await
            .map(|receipt| receipt.protocol)
            .map_err(|error| error.code().to_string())
    }

    async fn status(&mut self, request_id: Uuid) -> Result<AttemptStatus, String> {
        let client = self.client()?;
        client
            .harness_dispatch_status(request_id)
            .await
            .map(|status| AttemptStatus {
                phase: status.phase,
                error_code: status.error_code.map(|error| error.code().to_string()),
            })
            .map_err(|error| error.code().to_string())
    }

    async fn output(
        &mut self,
        query: HarnessDispatchOutputQuery,
    ) -> Result<DispatchOutputPage, String> {
        let client = self.client()?;
        client
            .harness_dispatch_output(query)
            .await
            .map_err(|error| error.code().to_string())
    }

    async fn cancel(&mut self, request_id: Uuid) -> Result<(), String> {
        let client = self.client()?;
        client
            .harness_dispatch_cancel(request_id)
            .await
            .map(|_| ())
            .map_err(|error| error.code().to_string())
    }
}

/// A running Ask: its updates and its cancel switch.
pub struct AskHandle {
    pub request_id: Uuid,
    pub updates: Receiver<AskUpdate>,
    cancel: Arc<AtomicBool>,
}

impl AskHandle {
    /// Asks the daemon to cancel; the Ask ends when the daemon says so.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
}

fn runtime() -> Option<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()
}

fn failed(request_id: Uuid, code: &str) -> AskUpdate {
    AskUpdate {
        request_id,
        phase: AskPhase::Finished,
        reply: String::new(),
        truncated: false,
        end: Some(AskEnd::Failed(code.to_string())),
    }
}

/// Starts one Ask of `harness` in `project_root`'s daemon.
pub fn spawn_ask(
    project_root: PathBuf,
    harness: AgentKind,
    prompt: String,
    notifier: Arc<dyn ChangeNotifier>,
) -> std::io::Result<AskHandle> {
    let request = DispatchRequest {
        schema_version: DISPATCH_REQUEST_SCHEMA_VERSION,
        request_id: Uuid::new_v4(),
        harness,
        prompt,
    };
    let request_id = request.request_id;
    let (sender, updates) = mpsc::sync_channel(UPDATE_CAPACITY);
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    std::thread::Builder::new()
        .name("gui_ask".into())
        .spawn(move || {
            let deliver = |update: AskUpdate, sender: &SyncSender<AskUpdate>| {
                if update.end.is_some() {
                    let _ = sender.send(update);
                } else {
                    let _ = sender.try_send(update);
                }
                notifier.notify();
            };
            let Some(runtime) = runtime() else {
                deliver(failed(request_id, "frontend_runtime_unavailable"), &sender);
                return;
            };
            let paths = match DaemonPaths::from_project_root(&project_root) {
                Ok(paths) => paths,
                Err(error) => {
                    deliver(failed(request_id, error.code()), &sender);
                    return;
                }
            };
            let mut port = ControlPort {
                paths,
                session: DaemonSession::default(),
            };
            runtime.block_on(session::drive(
                &mut port,
                request,
                &flag,
                POLL_INTERVAL,
                session::ASK_DEADLINE,
                |update| deliver(update, &sender),
            ));
        })?;
    Ok(AskHandle {
        request_id,
        updates,
        cancel,
    })
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
        .filter(|route| route.enabled && route.allow_execution)
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
