//! Test-only native GUI acceptance over an owned synthetic project.
//! This module is never selected by renderer input or the normal entry point.
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fmt,
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use vibemux_types::{ChatId, RunId};

mod probe_fixture;
mod seed;
pub(crate) use probe_fixture::app_state;
mod storage;
mod supervision;
use crate::{app_state::AppState, event_sink::RecordingEventSink};
use storage::{
    OUTCOME_FILE, OwnedRoot, PLAN_FILE, QaPlan, QaRun, RUN_FILE, STOP_FILE, StopRequest,
};
use vibemuxd::{
    control::{
        ChatExecutionRequest, ChatExecutionResponse, ChatRequest, ChatResponse, DaemonControlServer,
    },
    process::DaemonPaths,
};

pub const USAGE: &str = "vibemux_desktop_qa <prepare|prepare_cleanup|prepare_surfaces|check|run|stop> --root <fresh absolute local directory>";

pub const QA_SCHEMA_VERSION: u32 = 1;
const QA_PLAN_SCHEMA_VERSION: u32 = 2;
const QA_MAX_RUN_SECONDS: u64 = 20 * 60;
const QA_SEED_TIMEOUT_SECONDS: u64 = 120;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum QaScenario {
    History,
    CleanupReview,
    Surfaces,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QaError {
    InvalidArguments,
    InvalidRoot,
    RootExists,
    Io,
    RuntimeUnavailable,
    FixtureUnavailable,
    SeedFailed,
    CheckFailed,
    OwnershipChanged,
    CleanupUnconfirmed,
}
impl QaError {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidArguments => "native_qa_invalid_arguments",
            Self::InvalidRoot => "native_qa_invalid_root",
            Self::RootExists => "native_qa_root_exists",
            Self::Io => "native_qa_io_failed",
            Self::RuntimeUnavailable => "native_qa_runtime_unavailable",
            Self::FixtureUnavailable => "native_qa_fixture_unavailable",
            Self::SeedFailed => "native_qa_seed_failed",
            Self::CheckFailed => "native_qa_check_failed",
            Self::OwnershipChanged => "native_qa_ownership_changed",
            Self::CleanupUnconfirmed => "native_qa_cleanup_unconfirmed",
        }
    }
}
impl fmt::Display for QaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for QaError {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SeedIds {
    pub saved_chat: ChatId,
    pub archived_chat: ChatId,
    pub workspace_chat: ChatId,
    pub workspace_run: RunId,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SeedSummary {
    pub saved_records: u64,
    pub archived_records: u64,
    pub workspace_records: u64,
    pub process_claims_present: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum QaPhase {
    Prepared,
    Checked,
    NotRunning,
    StopRequested,
    AlreadyStopped,
    GuiExited,
    GuiStartupFailed,
    CleanupUnconfirmed,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QaReport {
    schema_version: u32,
    phase: QaPhase,
    root_id: uuid::Uuid,
    run_id: Option<uuid::Uuid>,
    fixture_only: bool,
    production_profile_qualified: bool,
    baseline: Option<SeedSummary>,
    gui_ready_observed: Option<bool>,
    event_loop_returned: Option<bool>,
    daemon_joined: Option<bool>,
    monitor_joined: Option<bool>,
    unresolved_asks: Option<u32>,
    error_code: Option<String>,
}
impl QaReport {
    fn new(root: &OwnedRoot, phase: QaPhase) -> Self {
        Self {
            schema_version: QA_SCHEMA_VERSION,
            phase,
            root_id: root.id(),
            run_id: None,
            fixture_only: true,
            production_profile_qualified: false,
            baseline: None,
            gui_ready_observed: None,
            event_loop_returned: None,
            daemon_joined: None,
            monitor_joined: None,
            unresolved_asks: None,
            error_code: None,
        }
    }
}

/// Trusted developer CLI only. No renderer command reaches this dispatch.
pub fn execute(arguments: Vec<OsString>) -> Result<QaReport, QaError> {
    if arguments.len() != 3 || arguments[1] != "--root" {
        return Err(QaError::InvalidArguments);
    }
    let root = PathBuf::from(&arguments[2]);
    match arguments[0].to_str() {
        Some("prepare") => prepare(root, QaScenario::History),
        Some("prepare_cleanup") => prepare(root, QaScenario::CleanupReview),
        Some("prepare_surfaces") => prepare(root, QaScenario::Surfaces),
        Some("check") => check(root),
        Some("run") => run(root, run_window),
        Some("stop") => stop(root),
        _ => Err(QaError::InvalidArguments),
    }
}
fn runtime() -> Result<tokio::runtime::Runtime, QaError> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|_| QaError::RuntimeUnavailable)
}
fn start_server(
    runtime: &tokio::runtime::Runtime,
    layout: &seed::QaLayout,
) -> Result<DaemonControlServer, QaError> {
    let paths = &layout.paths;
    layout.verify()?;
    paths
        .ensure_runtime_dir()
        .map_err(|_| QaError::RuntimeUnavailable)?;
    let server = runtime
        .block_on(DaemonControlServer::start_for_paths_with_plugins(
            paths,
            layout.plugin_startup()?,
        ))
        .map_err(|_| QaError::RuntimeUnavailable)?;
    finish_startup(runtime, server, layout.wait_for_plugins())
}

fn finish_startup(
    runtime: &tokio::runtime::Runtime,
    server: DaemonControlServer,
    ready: impl std::future::Future<Output = Result<(), QaError>>,
) -> Result<DaemonControlServer, QaError> {
    if let Err(error) = runtime.block_on(ready) {
        finish_server(runtime, server)?;
        return Err(error);
    }
    Ok(server)
}
fn finish_server(
    runtime: &tokio::runtime::Runtime,
    server: DaemonControlServer,
) -> Result<(), QaError> {
    runtime
        .block_on(async { tokio::time::timeout(Duration::from_secs(650), server.shutdown()).await })
        .map_err(|_| QaError::CleanupUnconfirmed)?
        .map_err(|_| QaError::CleanupUnconfirmed)
}
fn load_plan(root: &OwnedRoot) -> Result<(QaPlan, QaScenario), QaError> {
    let plan: QaPlan = root.read(PLAN_FILE)?;
    let scenario = match (plan.schema_version, plan.scenario) {
        (1, None) => QaScenario::History,
        (QA_PLAN_SCHEMA_VERSION, Some(scenario)) => scenario,
        _ => return Err(QaError::OwnershipChanged),
    };
    if plan.root_id != root.id() || plan.baseline.process_claims_present {
        return Err(QaError::OwnershipChanged);
    }
    Ok((plan, scenario))
}
fn validate_settings(root: &OwnedRoot, project: &Path) -> Result<(), QaError> {
    let settings: crate::desktop_settings::DesktopSettings = root.read_settings()?;
    if settings.schema_version != crate::desktop_settings::SETTINGS_SCHEMA_VERSION
        || settings.project_folders.len() != 1
        || !same_file::is_same_file(&settings.project_folders[0], project)
            .map_err(|_| QaError::OwnershipChanged)?
    {
        return Err(QaError::OwnershipChanged);
    }
    Ok(())
}
/// Uses the production AppState resolver, not a custom runtime-directory adapter.
fn appstate_observation(
    root: &OwnedRoot,
    paths: &DaemonPaths,
    ids: &SeedIds,
) -> Result<Vec<vibemuxd::chat_session::api::ChatSessionStatus>, QaError> {
    let state = app_state(
        root.settings_path(),
        Arc::new(RecordingEventSink::default()),
    );
    let key = state
        .register_project(paths.project_root())
        .map_err(|_| QaError::CheckFailed)?;
    if state.settings_path() != root.settings_path() {
        return Err(QaError::OwnershipChanged);
    }
    let mut observed = Vec::new();
    for chat_id in [ids.saved_chat, ids.archived_chat] {
        match state
            .chat_request(&key, ChatRequest::Status { chat_id })
            .map_err(|_| QaError::CheckFailed)?
        {
            ChatResponse::Status {
                session: Some(session),
            } if session.chat_id == chat_id => observed.push(session),
            _ => return Err(QaError::CheckFailed),
        }
    }
    match state
        .chat_execution_request(
            &key,
            ChatExecutionRequest::Status {
                chat_id: ids.workspace_chat,
            },
        )
        .map_err(|_| QaError::CheckFailed)?
    {
        ChatExecutionResponse::Status {
            runtime: Some(runtime),
        } if runtime.session.chat_id == ids.workspace_chat
            && runtime.owner.run_id == ids.workspace_run =>
        {
            observed.push(runtime.session)
        }
        _ => return Err(QaError::CheckFailed),
    }
    Ok(observed)
}
fn check_baseline(
    runtime: &tokio::runtime::Runtime,
    root: &OwnedRoot,
    paths: &DaemonPaths,
    ids: &SeedIds,
    scenario: QaScenario,
) -> Result<SeedSummary, QaError> {
    validate_settings(root, paths.project_root())?;
    let before = appstate_observation(root, paths, ids)?;
    let descriptor = std::fs::read(paths.descriptor_path()).map_err(|_| QaError::CheckFailed)?;
    let first = runtime.block_on(seed::check_history(paths, ids, scenario))?;
    let second = runtime.block_on(seed::check_history(paths, ids, scenario))?;
    let after = appstate_observation(root, paths, ids)?;
    let descriptor_after =
        std::fs::read(paths.descriptor_path()).map_err(|_| QaError::CheckFailed)?;
    if first != second || before != after || descriptor != descriptor_after {
        return Err(QaError::CheckFailed);
    }
    root.verify()?;
    Ok(first)
}
fn prepare(path: PathBuf, scenario: QaScenario) -> Result<QaReport, QaError> {
    let root = OwnedRoot::create(path)?;
    let runtime = runtime()?;
    let layout = runtime.block_on(seed::prepare_layout(root.path(), scenario))?;
    layout.verify()?;
    let server = start_server(&runtime, &layout)?;
    let outcome = (|| {
        let ids = runtime
            .block_on(async {
                tokio::time::timeout(
                    Duration::from_secs(QA_SEED_TIMEOUT_SECONDS),
                    seed::seed_history(&layout.paths, scenario),
                )
                .await
            })
            .map_err(|_| QaError::CleanupUnconfirmed)??;
        // Registration is a trusted Rust fixture action in the owned settings file.
        appstate_observation(&root, &layout.paths, &ids)?;
        let summary = check_baseline(&runtime, &root, &layout.paths, &ids, scenario)?;
        Ok::<_, QaError>((ids, summary))
    })();
    finish_server(&runtime, server)?;
    let (ids, summary) = outcome?;
    root.write_new(
        PLAN_FILE,
        &QaPlan {
            schema_version: QA_PLAN_SCHEMA_VERSION,
            scenario: Some(scenario),
            root_id: root.id(),
            ids,
            baseline: summary.clone(),
        },
    )?;
    let mut report = QaReport::new(&root, QaPhase::Prepared);
    report.baseline = Some(summary);
    report.daemon_joined = Some(true);
    Ok(report)
}
fn check(path: PathBuf) -> Result<QaReport, QaError> {
    let root = OwnedRoot::open(path)?;
    let (plan, scenario) = load_plan(&root)?;
    let layout = seed::open_layout(root.path(), scenario)?;
    layout.verify()?;
    let runtime = runtime()?;
    let server = start_server(&runtime, &layout)?;
    let outcome = check_baseline(&runtime, &root, &layout.paths, &plan.ids, scenario);
    finish_server(&runtime, server)?;
    let summary = outcome?;
    if summary != plan.baseline {
        return Err(QaError::CheckFailed);
    }
    let mut report = QaReport::new(&root, QaPhase::Checked);
    report.baseline = Some(summary);
    report.daemon_joined = Some(true);
    Ok(report)
}
fn stop(path: PathBuf) -> Result<QaReport, QaError> {
    let root = OwnedRoot::open(path)?;
    let Some(run) = root.optional::<QaRun>(RUN_FILE)? else {
        return Ok(QaReport::new(&root, QaPhase::NotRunning));
    };
    if run.schema_version != QA_SCHEMA_VERSION
        || run.root_id != root.id()
        || run.run_id.is_nil()
        || run.runner_pid == 0
    {
        return Err(QaError::OwnershipChanged);
    }
    if let Some(outcome) = root.optional::<QaReport>(OUTCOME_FILE)? {
        if outcome.schema_version != QA_SCHEMA_VERSION
            || !outcome.fixture_only
            || outcome.production_profile_qualified
            || outcome.root_id != root.id()
            || outcome.run_id != Some(run.run_id)
            || !matches!(
                outcome.phase,
                QaPhase::GuiExited | QaPhase::GuiStartupFailed | QaPhase::CleanupUnconfirmed
            )
        {
            return Err(QaError::OwnershipChanged);
        }
        if outcome.phase == QaPhase::CleanupUnconfirmed
            || outcome.daemon_joined != Some(true)
            || outcome.monitor_joined == Some(false)
            || outcome.unresolved_asks.is_some_and(|count| count > 0)
        {
            return Err(QaError::CleanupUnconfirmed);
        }
        let mut report = outcome;
        report.phase = QaPhase::AlreadyStopped;
        return Ok(report);
    }
    let request = StopRequest {
        schema_version: QA_SCHEMA_VERSION,
        root_id: root.id(),
        run_id: run.run_id,
    };
    if let Some(previous) = root.optional::<StopRequest>(STOP_FILE)? {
        if previous != request {
            return Err(QaError::OwnershipChanged);
        }
    } else if let Some(stage) = root.publish_stop(&request)? {
        eprintln!("native_qa_stop_stage_retained: {stage}");
    }
    let mut report = QaReport::new(&root, QaPhase::StopRequested);
    report.run_id = Some(run.run_id);
    Ok(report)
}
fn run(
    path: PathBuf,
    window: impl FnOnce(
        Arc<OwnedRoot>,
        Arc<seed::QaLayout>,
        &QaRun,
        &mut QaReport,
    ) -> Result<(), QaError>,
) -> Result<QaReport, QaError> {
    let root = Arc::new(OwnedRoot::open(path)?);
    let (plan, scenario) = load_plan(&root)?;
    let layout = Arc::new(seed::open_layout(root.path(), scenario)?);
    layout.verify()?;
    if root.optional::<QaRun>(RUN_FILE)?.is_some()
        || root.optional::<QaReport>(OUTCOME_FILE)?.is_some()
    {
        return Err(QaError::OwnershipChanged);
    }
    let runtime = runtime()?;
    let server = start_server(&runtime, &layout)?;
    let baseline = check_baseline(&runtime, &root, &layout.paths, &plan.ids, scenario);
    if let Err(error) = baseline {
        finish_server(&runtime, server)?;
        return Err(error);
    }
    let run = QaRun {
        schema_version: QA_SCHEMA_VERSION,
        root_id: root.id(),
        run_id: uuid::Uuid::new_v4(),
        runner_pid: std::process::id(),
    };
    if let Err(error) = root.write_new(RUN_FILE, &run) {
        finish_server(&runtime, server)?;
        return Err(error);
    }
    let mut report = QaReport::new(&root, QaPhase::GuiStartupFailed);
    report.run_id = Some(run.run_id);
    report.baseline = Some(plan.baseline);
    let gui_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        window(Arc::clone(&root), Arc::clone(&layout), &run, &mut report)
    }))
    .unwrap_or(Err(QaError::RuntimeUnavailable));
    let joined = finish_server(&runtime, server);
    report.daemon_joined = Some(joined.is_ok());
    if joined.is_err() {
        report.phase = QaPhase::CleanupUnconfirmed;
        report.error_code = Some(QaError::CleanupUnconfirmed.code().into());
    } else if let Err(error) = gui_result {
        if error == QaError::CleanupUnconfirmed {
            report.phase = QaPhase::CleanupUnconfirmed;
        }
        report.error_code = Some(error.code().into());
    }
    root.write_new(OUTCOME_FILE, &report)?;
    joined?;
    gui_result?;
    Ok(report)
}
fn run_window(
    root: Arc<OwnedRoot>,
    layout: Arc<seed::QaLayout>,
    run: &QaRun,
    report: &mut QaReport,
) -> Result<(), QaError> {
    use tauri::Manager;
    root.verify()?;
    root.child_directory("webview")?;
    let app_state = Arc::new(OnceLock::<Arc<AppState>>::new());
    let app = crate::build_app(crate::LaunchMode::NativeQa {
        settings_path: root.settings_path(),
        webview_data_dir: root.webview_path(),
        state_capture: Arc::clone(&app_state),
    })
    .map_err(|_| QaError::RuntimeUnavailable)?;
    let handle = app.handle().clone();
    let app_state_event = Arc::clone(&app_state);
    let ready = Arc::new(AtomicBool::new(false));
    let ready_event = Arc::clone(&ready);
    let unresolved = Arc::new(AtomicUsize::new(0));
    let unresolved_event = Arc::clone(&unresolved);
    let unresolved_probes = Arc::new(AtomicUsize::new(0));
    let unresolved_probes_event = Arc::clone(&unresolved_probes);
    let root_event = Arc::clone(&root);
    let mismatch = Arc::new(AtomicBool::new(false));
    let mismatch_event = Arc::clone(&mismatch);
    let monitor = supervision::Monitor::start(
        root,
        run.clone(),
        Duration::from_secs(QA_MAX_RUN_SECONDS),
        move || layout.verify(),
        move || handle.exit(0),
    )?;
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        app.run_return(move |app, event| {
            if matches!(event, tauri::RunEvent::Ready) {
                let Some(state) = app.try_state::<Arc<AppState>>() else {
                    mismatch_event.store(true, Ordering::Release);
                    app.exit(1);
                    return;
                };
                let _ = app_state_event.set(Arc::clone(&state));
                if state.settings_path() != root_event.settings_path() {
                    mismatch_event.store(true, Ordering::Release);
                    app.exit(1);
                } else {
                    ready_event.store(true, Ordering::Release);
                }
            }
            if matches!(event, tauri::RunEvent::ExitRequested { .. }) {
                unresolved_event.fetch_max(crate::reconcile_app_exit(app), Ordering::AcqRel);
                if let Some(state) = app_state_event.get() {
                    unresolved_probes_event.fetch_max(state.probes.unresolved(), Ordering::AcqRel);
                }
            }
        })
    }));
    if let Some(state) = app_state.get() {
        unresolved.fetch_max(state.asks.shutdown(), Ordering::AcqRel);
        unresolved_probes.fetch_max(state.probes.shutdown(), Ordering::AcqRel);
    }
    let monitored = monitor.finish();
    assess_window_completion(
        report,
        WindowCompletion {
            monitored: monitored.map(|_| ()),
            ready: ready.load(Ordering::Acquire),
            event_loop_returned: outcome.is_ok(),
            owner_captured: app_state.get().is_some(),
            ownership_changed: mismatch.load(Ordering::Acquire),
            unresolved_asks: app_state
                .get()
                .map(|_| u32::try_from(unresolved.load(Ordering::Acquire)).unwrap_or(u32::MAX)),
            unresolved_probes: unresolved_probes.load(Ordering::Acquire),
        },
    )
}

struct WindowCompletion {
    monitored: Result<(), QaError>,
    ready: bool,
    event_loop_returned: bool,
    owner_captured: bool,
    ownership_changed: bool,
    unresolved_asks: Option<u32>,
    unresolved_probes: usize,
}
fn assess_window_completion(
    report: &mut QaReport,
    observed: WindowCompletion,
) -> Result<(), QaError> {
    report.monitor_joined = Some(observed.monitored.is_ok());
    report.gui_ready_observed = Some(observed.ready);
    report.event_loop_returned = Some(observed.event_loop_returned);
    report.unresolved_asks = observed.unresolved_asks;
    if observed.unresolved_asks.is_some_and(|count| count > 0) || observed.unresolved_probes > 0 {
        report.phase = QaPhase::CleanupUnconfirmed;
        return Err(QaError::CleanupUnconfirmed);
    }
    if observed.ownership_changed {
        return Err(QaError::OwnershipChanged);
    }
    if let Err(error) = observed.monitored {
        // Keep the inner rejection distinct from the outer cleanup status.
        eprintln!("native_qa_monitor_failed: {}", error.code());
        return Err(QaError::CleanupUnconfirmed);
    }
    if !observed.event_loop_returned || !observed.owner_captured || !observed.ready {
        return Err(QaError::RuntimeUnavailable);
    }
    report.phase = QaPhase::GuiExited;
    Ok(())
}

#[cfg(test)]
mod tests;
