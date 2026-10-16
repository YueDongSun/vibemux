#![forbid(unsafe_code)]
//! Unified local CLI: the daemon lifecycle client and controlled bootstrap,
//! the daemon-backed control verbs, and the daemon-free inspection and native
//! harness commands that share one command tree and output contract.

pub mod application;
pub mod command;
pub mod decision_commands;
pub mod dispatch_commands;
pub mod dsh_commands;
pub mod failure;
pub mod output;
pub use vibemux_probe::snapshot as probe;
pub mod recovery;
pub mod workflow_commands;
pub mod workspace;

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Child,
    time::Duration,
};

#[cfg(unix)]
use std::process::{Command, Stdio};
#[cfg(windows)]
mod windows_bootstrap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::time::{Instant, sleep};
use vibemux_harness::HarnessRow;
use vibemuxd::{
    control::{ControlClient, ControlError, DaemonHealth},
    process::{DaemonPathError, DaemonPaths},
};

/// Time a start allows beyond the daemon's writer startup bound for process
/// launch, control runtime ACL verification, and descriptor publication.
const DAEMON_START_MARGIN: Duration = Duration::from_secs(10);
/// `daemon start` health wait (30 s). It is derived from the daemon's writer
/// startup bound, so the CLI never terminates a daemon that is still inside
/// that bound; a daemon that exits early is still reported as soon as the
/// exit is observed.
pub const DEFAULT_STARTUP_TIMEOUT: Duration =
    vibemuxd::DEFAULT_WRITER_STARTUP_TIMEOUT.saturating_add(DAEMON_START_MARGIN);
const _: () = assert!(
    DEFAULT_STARTUP_TIMEOUT.as_nanos() > vibemuxd::DEFAULT_WRITER_STARTUP_TIMEOUT.as_nanos(),
    "the CLI start wait must outlast the daemon's writer startup bound"
);
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(25);
pub const DEFAULT_HEALTH_PROBE_TIMEOUT: Duration = Duration::from_millis(500);
pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
pub struct DaemonBootstrapConfig {
    pub paths: DaemonPaths,
    pub daemon_executable: PathBuf,
    pub startup_timeout: Duration,
    pub poll_interval: Duration,
    pub health_probe_timeout: Duration,
}

impl DaemonBootstrapConfig {
    #[must_use]
    pub fn new(paths: DaemonPaths, daemon_executable: PathBuf) -> Self {
        Self {
            paths,
            daemon_executable,
            startup_timeout: DEFAULT_STARTUP_TIMEOUT,
            poll_interval: DEFAULT_POLL_INTERVAL,
            health_probe_timeout: DEFAULT_HEALTH_PROBE_TIMEOUT,
        }
    }

    #[must_use]
    pub fn with_timing(
        mut self,
        startup_timeout: Duration,
        poll_interval: Duration,
        health_probe_timeout: Duration,
    ) -> Self {
        self.startup_timeout = startup_timeout;
        self.poll_interval = poll_interval;
        self.health_probe_timeout = health_probe_timeout;
        self
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "status", content = "health")]
pub enum DaemonStartOutcome {
    Started(DaemonHealth),
    AlreadyRunning(DaemonHealth),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RuntimeLocation {
    Protected,
    Legacy,
}

struct RuntimeSelection<'a> {
    descriptor_path: &'a Path,
    writer_lock_path: &'a Path,
    compatibility_lock_path: Option<&'a Path>,
    location: RuntimeLocation,
    has_artifacts: bool,
}

#[cfg(unix)]
struct SpawnedDaemon {
    child: Child,
}

#[cfg(windows)]
struct SpawnedDaemon {
    child: Option<Child>,
}

#[cfg(unix)]
impl SpawnedDaemon {
    fn process_id(&self) -> Option<u32> {
        Some(self.child.id())
    }

    fn has_exited(&mut self) -> Result<bool, DaemonCliError> {
        self.child
            .try_wait()
            .map(|status| status.is_some())
            .map_err(|_| DaemonCliError::StartFailed)
    }

    fn release(mut self) -> Result<(), DaemonCliError> {
        std::thread::Builder::new()
            .name("vibemux_daemon_reaper".to_string())
            .spawn(move || {
                let _ = self.child.wait();
            })
            .map(|_| ())
            .map_err(|_| DaemonCliError::StartFailed)
    }

    fn terminate(&mut self) {
        terminate_child(&mut self.child);
    }
}

#[cfg(windows)]
impl SpawnedDaemon {
    fn process_id(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    fn has_exited(&mut self) -> Result<bool, DaemonCliError> {
        self.child
            .as_mut()
            .ok_or(DaemonCliError::StartFailed)?
            .try_wait()
            .map(|status| status.is_some())
            .map_err(|_| DaemonCliError::StartFailed)
    }

    /// Drops the process handle only after authenticated health confirms the
    /// daemon is ready.
    fn release(mut self) -> Result<(), DaemonCliError> {
        let child = self.child.take().ok_or(DaemonCliError::StartFailed)?;
        drop(child);
        Ok(())
    }

    fn terminate(&mut self) {
        if let Some(mut child) = self.child.take() {
            terminate_child(&mut child);
        }
    }
}

#[cfg(windows)]
impl Drop for SpawnedDaemon {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            terminate_child(child);
        }
    }
}

impl DaemonStartOutcome {
    #[must_use]
    pub const fn health(&self) -> &DaemonHealth {
        match self {
            Self::Started(health) | Self::AlreadyRunning(health) => health,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CliCommand {
    DaemonStart {
        project_root: Option<PathBuf>,
        daemon_executable: Option<PathBuf>,
    },
    DaemonHealth {
        project_root: Option<PathBuf>,
    },
    DaemonStop {
        project_root: Option<PathBuf>,
    },
    DaemonInspect {
        project_root: Option<PathBuf>,
    },
    DaemonRecover {
        project_root: Option<PathBuf>,
        confirmation: String,
    },
    Harnesses {
        project_root: Option<PathBuf>,
        cached: bool,
    },
    Switch {
        project_root: Option<PathBuf>,
        harness: String,
    },
    Dispatch {
        project_root: Option<PathBuf>,
        command: dispatch_commands::DispatchCommand,
    },
    /// The `decide` and `decisions` verbs (ADR 034).
    Decision {
        project_root: Option<PathBuf>,
        command: decision_commands::DecisionCommand,
    },
    Dsh {
        project_root: Option<PathBuf>,
        command: dsh_commands::DshCommand,
    },
    /// The workflow, slots, context, session, and prompt verbs.
    Workflow {
        project_root: Option<PathBuf>,
        command: workflow_commands::WorkflowCommand,
    },
    Help,
    Version,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum DaemonCliError {
    #[error("daemon bootstrap configuration is invalid")]
    InvalidConfiguration,
    #[error("daemon project root is invalid")]
    InvalidProjectRoot,
    #[error("daemon executable is unavailable")]
    ExecutableUnavailable,
    #[error("daemon is not running")]
    NotRunning,
    #[error("daemon runtime state is stale: {reason_code}")]
    StaleRuntime { reason_code: String },
    #[error("daemon process could not be spawned")]
    SpawnFailed,
    #[error("daemon process exited before becoming healthy")]
    StartFailed,
    #[error("daemon startup exceeded its deadline")]
    StartupTimeout,
    #[error("daemon shutdown exceeded its deadline")]
    ShutdownTimeout,
    #[error("daemon control operation failed: {code}")]
    Control { code: String },
    #[error("daemon recovery confirmation does not match current artifacts")]
    RecoveryConfirmationMismatch,
    #[error("daemon recovery is blocked: {reason_code}")]
    RecoveryBlocked { reason_code: String },
    #[error("daemon recovery artifact operation failed: {code}")]
    RecoveryArtifact { code: String },
    #[error("CLI arguments are invalid")]
    InvalidArguments,
    #[error(
        "harness probe cache is missing: run 'vibemux_probe --write-cache --project-root <root>' (writes {0})"
    )]
    HarnessProbeCacheMissing(String),
    #[error("harness is not detected on this machine: {0} (run 'vibemuxctl harnesses' to refresh)")]
    HarnessNotDetected(String),
    #[error("unknown harness: {0}")]
    UnknownHarness(String),
    #[error("dispatch prompt could not be read")]
    PromptUnavailable,
    #[error("dispatch prompt must be non-blank UTF-8 of at most 32 KiB")]
    InvalidPrompt,
    #[error("compiled workflow contract was rejected: {}", .0.join(", "))]
    ContractRejected(Vec<String>),
}

impl DaemonCliError {
    #[must_use]
    pub fn code(&self) -> &str {
        match self {
            Self::InvalidConfiguration => "daemon_invalid_configuration",
            Self::InvalidProjectRoot => "daemon_invalid_project_root",
            Self::ExecutableUnavailable => "daemon_executable_unavailable",
            Self::NotRunning => "daemon_not_running",
            Self::StaleRuntime { .. } => "daemon_stale_runtime",
            Self::SpawnFailed => "daemon_spawn_failed",
            Self::StartFailed => "daemon_start_failed",
            Self::StartupTimeout => "daemon_startup_timeout",
            Self::ShutdownTimeout => "daemon_shutdown_timeout",
            Self::Control { code } => code,
            Self::RecoveryConfirmationMismatch => "daemon_recovery_confirmation_mismatch",
            Self::RecoveryBlocked { .. } => "daemon_recovery_blocked",
            Self::RecoveryArtifact { code } => code,
            Self::InvalidArguments => "cli_invalid_arguments",
            Self::HarnessProbeCacheMissing(_) => "harness_probe_cache_missing",
            Self::HarnessNotDetected(_) => "harness_not_detected",
            Self::UnknownHarness(_) => "store_unknown_harness",
            Self::PromptUnavailable => "cli_prompt_unavailable",
            Self::InvalidPrompt => "harness_dispatch_invalid_prompt",
            Self::ContractRejected(_) => "workflow_contract_rejected",
        }
    }
}

impl From<DaemonPathError> for DaemonCliError {
    fn from(error: DaemonPathError) -> Self {
        let code = error.code();
        match error {
            DaemonPathError::InvalidProjectRoot => Self::InvalidProjectRoot,
            DaemonPathError::RuntimeDirectoryUnavailable
            | DaemonPathError::ControlRuntimeUnavailable
            | DaemonPathError::UnsafeRuntimeDirectory
            | DaemonPathError::UnsafeControlRuntime
            | DaemonPathError::ControlRuntimeSecurityInvalid
            | DaemonPathError::UnsafeDatabasePath
            | DaemonPathError::DatabaseCollision
            | DaemonPathError::LegacyRuntimeConflict => Self::Control {
                code: code.to_string(),
            },
        }
    }
}

pub async fn start_daemon(
    config: &DaemonBootstrapConfig,
) -> Result<DaemonStartOutcome, DaemonCliError> {
    validate_bootstrap_config(config)?;
    config.paths.ensure_runtime_dir()?;

    if select_runtime(&config.paths)?.has_artifacts {
        return wait_for_health(config, None).await;
    }
    if !config.daemon_executable.is_file() {
        return Err(DaemonCliError::ExecutableUnavailable);
    }
    let child = spawn_daemon_process(config)?;
    wait_for_health(config, Some(child)).await
}

pub async fn daemon_health(paths: &DaemonPaths) -> Result<DaemonHealth, DaemonCliError> {
    let selection = select_runtime(paths)?;
    validate_selected_runtime(paths, &selection)?;
    let client = control_client(&selection)?;
    client.health().await.map_err(map_control_error)
}

pub async fn stop_daemon(paths: &DaemonPaths) -> Result<DaemonHealth, DaemonCliError> {
    let selection = select_runtime(paths)?;
    validate_selected_runtime(paths, &selection)?;
    let client = control_client(&selection)?;
    let health = client.health().await.map_err(map_control_error)?;
    client.shutdown().await.map_err(map_control_error)?;
    let deadline = Instant::now() + DEFAULT_SHUTDOWN_TIMEOUT;
    while select_runtime(paths)?.has_artifacts {
        if Instant::now() >= deadline {
            return Err(DaemonCliError::ShutdownTimeout);
        }
        sleep(DEFAULT_POLL_INTERVAL).await;
    }
    // The compatibility lock is the last artifact to go: a clean trusted
    // Windows daemon removes it only after releasing its empty runtime leaf,
    // so the loop above already outlasts the release. The daemon can still
    // be exiting then, so `stop` returns only once the process itself is
    // gone, under the same deadline. An in-process server (tests) shares
    // this PID and never exits, so only a separate daemon process is
    // awaited.
    if health.process_id != std::process::id() {
        let process_id = health.process_id;
        while tokio::task::spawn_blocking(move || recovery::process_is_present(process_id))
            .await
            .unwrap_or(true)
        {
            if Instant::now() >= deadline {
                return Err(DaemonCliError::ShutdownTimeout);
            }
            sleep(DEFAULT_POLL_INTERVAL).await;
        }
    }
    Ok(health)
}

/// Return a connected control client for the harness surface.
///
/// Requires a running daemon: this helper does NOT auto-spawn the daemon
/// because spawning needs the daemon executable (and a per-process
/// configuration the lifecycle commands have but the harness CLI does not).
/// When the daemon is absent, callers see `NotRunning` and can decide
/// whether to start it (via the lifecycle verbs) before retrying.
async fn harness_client(paths: &DaemonPaths) -> Result<ControlClient, DaemonCliError> {
    let selection = select_runtime(paths)?;
    validate_selected_runtime(paths, &selection)?;
    control_client(&selection)
}

pub async fn harness_list(
    paths: &DaemonPaths,
    cached: bool,
) -> Result<Vec<HarnessRow>, DaemonCliError> {
    let client = harness_client(paths).await?;
    let result = if cached {
        client.harness_snapshot().await
    } else {
        client.harness_refresh().await
    };
    result.map_err(|error| map_harness_control_error(&paths.probe_cache_path(), error, None))
}

pub async fn harness_switch(
    paths: &DaemonPaths,
    harness: &str,
) -> Result<(String, String), DaemonCliError> {
    let client = harness_client(paths).await?;
    client
        .harness_switch(harness)
        .await
        .map_err(|error| map_harness_control_error(&paths.probe_cache_path(), error, Some(harness)))
}

/// Map a control error from a harness command into a CLI error with the
/// minimum context needed for an actionable user message. The probe-cache
/// path is fetched from the daemon paths so the operator gets the exact
/// location the probe must write to, with the `--write-cache
/// --project-root <root>` remediation text. The harness name (in scope at
/// the switch call site; `None` for listing reads where no single harness
/// applies) fills in the `harness_not_detected` / `store_unknown_harness`
/// messages so the operator sees the offending harness's name, not the
/// opaque wire code.
fn map_harness_control_error(
    probe_cache_path: &std::path::Path,
    error: ControlError,
    harness_name: Option<&str>,
) -> DaemonCliError {
    match error.code() {
        "harness_not_detected" => {
            DaemonCliError::HarnessNotDetected(harness_name.unwrap_or("harness").to_string())
        }
        "store_unknown_harness" => {
            DaemonCliError::UnknownHarness(harness_name.unwrap_or("harness").to_string())
        }
        "harness_probe_cache_missing" => DaemonCliError::HarnessProbeCacheMissing(
            probe_cache_path.to_string_lossy().into_owned(),
        ),
        _ => map_control_error(error),
    }
}

pub fn parse_cli_arguments(
    arguments: impl Iterator<Item = OsString>,
) -> Result<CliCommand, DaemonCliError> {
    let arguments = arguments.collect::<Vec<_>>();
    if arguments.len() == 1 && arguments[0] == "--help" {
        return Ok(CliCommand::Help);
    }
    if arguments.len() == 1 && arguments[0] == "--version" {
        return Ok(CliCommand::Version);
    }
    if arguments.is_empty() {
        return Err(DaemonCliError::InvalidArguments);
    }
    let verb = arguments[0]
        .to_str()
        .ok_or(DaemonCliError::InvalidArguments)?;
    if verb == "harnesses" || verb == "switch" {
        return parse_harness_arguments(verb, &arguments[1..]);
    }
    if verb == "dispatch" {
        return dispatch_commands::parse_dispatch_arguments(&arguments[1..]);
    }
    if verb == "decide" || verb == "decisions" {
        return decision_commands::parse_decision_arguments(verb, &arguments[1..]);
    }
    if verb == "dsh" {
        return match dsh_commands::parse_dsh_arguments(&arguments[1..])? {
            Some(command) => Ok(command),
            None => Err(DaemonCliError::InvalidArguments),
        };
    }
    if matches!(
        verb,
        "workflow" | "slots" | "context" | "session" | "prompt"
    ) {
        return workflow_commands::parse_workflow_arguments(verb, &arguments[1..]);
    }
    if verb != "daemon" {
        return Err(DaemonCliError::InvalidArguments);
    }
    if arguments.len() < 2 {
        return Err(DaemonCliError::InvalidArguments);
    }
    let action = arguments[1]
        .to_str()
        .ok_or(DaemonCliError::InvalidArguments)?;
    let mut project_root = None;
    let mut daemon_executable = None;
    let mut confirmation = None;
    let mut index = 2;
    while index < arguments.len() {
        let flag = arguments[index]
            .to_str()
            .ok_or(DaemonCliError::InvalidArguments)?;
        let value = arguments
            .get(index + 1)
            .ok_or(DaemonCliError::InvalidArguments)?;
        match flag {
            "--project-root" if project_root.is_none() => {
                project_root = Some(PathBuf::from(value));
            }
            "--daemon-executable" if action == "start" && daemon_executable.is_none() => {
                daemon_executable = Some(PathBuf::from(value));
            }
            "--confirmation" if action == "recover" && confirmation.is_none() => {
                confirmation = Some(
                    value
                        .to_str()
                        .ok_or(DaemonCliError::InvalidArguments)?
                        .to_string(),
                );
            }
            _ => return Err(DaemonCliError::InvalidArguments),
        }
        index += 2;
    }
    match action {
        "start" => Ok(CliCommand::DaemonStart {
            project_root,
            daemon_executable,
        }),
        "health" if daemon_executable.is_none() => Ok(CliCommand::DaemonHealth { project_root }),
        "stop" if daemon_executable.is_none() => Ok(CliCommand::DaemonStop { project_root }),
        "inspect" if daemon_executable.is_none() && confirmation.is_none() => {
            Ok(CliCommand::DaemonInspect { project_root })
        }
        "recover" if daemon_executable.is_none() => Ok(CliCommand::DaemonRecover {
            project_root,
            confirmation: confirmation.ok_or(DaemonCliError::InvalidArguments)?,
        }),
        _ => Err(DaemonCliError::InvalidArguments),
    }
}

pub fn daemon_paths(project_root: Option<&Path>) -> Result<DaemonPaths, DaemonCliError> {
    let project_root = match project_root {
        Some(project_root) => project_root.to_path_buf(),
        None => std::env::current_dir().map_err(|_| DaemonCliError::InvalidProjectRoot)?,
    };
    DaemonPaths::from_project_root(&project_root).map_err(DaemonCliError::from)
}

fn parse_harness_arguments(
    verb: &str,
    arguments: &[OsString],
) -> Result<CliCommand, DaemonCliError> {
    let mut project_root = None;
    let mut cached = false;
    let mut positional: Option<String> = None;
    let mut index = 0;
    while index < arguments.len() {
        let token = arguments[index]
            .to_str()
            .ok_or(DaemonCliError::InvalidArguments)?;
        match token {
            "--project-root" if project_root.is_none() => {
                let value = arguments
                    .get(index + 1)
                    .ok_or(DaemonCliError::InvalidArguments)?;
                project_root = Some(PathBuf::from(value));
                index += 2;
            }
            "--cached" if verb == "harnesses" && !cached => {
                cached = true;
                index += 1;
            }
            _ if verb == "switch" && !token.starts_with("--") && positional.is_none() => {
                positional = Some(token.to_string());
                index += 1;
            }
            _ => return Err(DaemonCliError::InvalidArguments),
        }
    }
    match verb {
        "harnesses" => Ok(CliCommand::Harnesses {
            project_root,
            cached,
        }),
        "switch" => Ok(CliCommand::Switch {
            project_root,
            harness: positional.ok_or(DaemonCliError::InvalidArguments)?,
        }),
        _ => Err(DaemonCliError::InvalidArguments),
    }
}

pub fn daemon_executable(explicit: Option<&Path>) -> Result<PathBuf, DaemonCliError> {
    if let Some(explicit) = explicit {
        return std::fs::canonicalize(explicit)
            .map_err(|_| DaemonCliError::ExecutableUnavailable)
            .and_then(|path| {
                if path.is_file() {
                    Ok(path)
                } else {
                    Err(DaemonCliError::ExecutableUnavailable)
                }
            });
    }
    let current = std::env::current_exe().map_err(|_| DaemonCliError::ExecutableUnavailable)?;
    let file_name = if cfg!(windows) {
        "vibemuxd.exe"
    } else {
        "vibemuxd"
    };
    let sibling = current
        .parent()
        .ok_or(DaemonCliError::ExecutableUnavailable)?
        .join(file_name);
    if sibling.is_file() {
        Ok(sibling)
    } else {
        Err(DaemonCliError::ExecutableUnavailable)
    }
}

/// Classify why a spawned daemon exited without a healthy start. On
/// Windows the launcher detaches the daemon process, so its stderr (which
/// would carry `daemon_control_runtime_security_invalid` from the
/// fail-closed start verification, issue #7) never reaches the CLI. The
/// control-runtime ACLs are the one such cause the CLI can observe for
/// itself: on the failure path only, re-check the SURVIVING phase-1
/// surface - protected root, runtime leaf, and the `.acl_v1` marker -
/// with ONE read-only native verification pass (~ms, no helper process -
/// ADR 025). The descriptor and writer lock are absent by definition on
/// this path (every daemon-side failure removes them), so they are not
/// part of the re-check. A missing runtime leaf is not a loosened ACL
/// either: a clean shutdown releases the empty leaf, and a start racing
/// that release fails on its writer lock. Then only the root and marker
/// are re-checked, and each path that exists still fails closed. The
/// happy path performs no security work at all.
#[cfg(windows)]
async fn classify_daemon_exit(paths: &DaemonPaths) -> DaemonCliError {
    // Clone for the `'static` closure; the re-check runs through
    // spawn_blocking because it performs blocking LSA/filesystem reads.
    let paths = paths.clone();
    let security_invalid =
        tokio::task::spawn_blocking(move || surviving_control_acls_invalid(&paths))
            .await
            // A verification pass that could not even run leaves the cause
            // undetermined; report the honest generic failure instead of
            // blaming the ACLs.
            .unwrap_or(false);
    if security_invalid {
        DaemonCliError::from(DaemonPathError::ControlRuntimeSecurityInvalid)
    } else {
        DaemonCliError::StartFailed
    }
}

#[cfg(windows)]
fn surviving_control_acls_invalid(paths: &DaemonPaths) -> bool {
    let verified = match std::fs::symlink_metadata(paths.runtime_dir()) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            paths.verify_control_root_acls()
        }
        _ => paths.verify_control_surviving_acls(),
    };
    verified.is_err()
}

#[cfg(unix)]
async fn classify_daemon_exit(_paths: &DaemonPaths) -> DaemonCliError {
    DaemonCliError::StartFailed
}

async fn wait_for_health(
    config: &DaemonBootstrapConfig,
    mut spawned: Option<SpawnedDaemon>,
) -> Result<DaemonStartOutcome, DaemonCliError> {
    let deadline = Instant::now() + config.startup_timeout;
    let mut child_exited = false;
    let initial_selection = select_runtime(&config.paths)?;
    let mut last_reason_code = if runtime_artifact_exists(initial_selection.writer_lock_path)?
        || initial_selection.compatibility_lock_path.is_some()
    {
        "writer_lock_held".to_string()
    } else {
        "control_descriptor_unavailable".to_string()
    };
    loop {
        let selection = select_runtime(&config.paths)?;
        if runtime_artifact_exists(selection.descriptor_path)? {
            match probe_health(config, selection.descriptor_path, deadline).await {
                Ok(health) => {
                    let spawned_process = spawned
                        .as_ref()
                        .and_then(SpawnedDaemon::process_id)
                        .is_some_and(|process_id| process_id == health.process_id);
                    let outcome = if spawned_process {
                        DaemonStartOutcome::Started(health)
                    } else {
                        DaemonStartOutcome::AlreadyRunning(health)
                    };
                    if let Some(mut spawned) = spawned.take() {
                        if spawned_process {
                            spawned.release()?;
                        } else {
                            spawned.terminate();
                        }
                    }
                    return Ok(outcome);
                }
                Err(error) => last_reason_code = error.code().to_string(),
            }
        }

        if let Some(spawned) = spawned.as_mut()
            && spawned.has_exited()?
        {
            child_exited = true;
        }
        if child_exited && !select_runtime(&config.paths)?.has_artifacts {
            return Err(classify_daemon_exit(&config.paths).await);
        }
        if Instant::now() >= deadline {
            if let Some(spawned) = spawned.as_mut() {
                spawned.terminate();
                return Err(if child_exited {
                    DaemonCliError::StartFailed
                } else {
                    DaemonCliError::StartupTimeout
                });
            }
            return Err(DaemonCliError::StaleRuntime {
                reason_code: last_reason_code,
            });
        }
        sleep(config.poll_interval).await;
    }
}

async fn probe_health(
    config: &DaemonBootstrapConfig,
    descriptor_path: &Path,
    startup_deadline: Instant,
) -> Result<DaemonHealth, DaemonCliError> {
    let remaining = startup_deadline.saturating_duration_since(Instant::now());
    let probe_deadline = config.health_probe_timeout.min(remaining);
    if probe_deadline.is_zero() {
        return Err(DaemonCliError::StartupTimeout);
    }
    let client = ControlClient::from_descriptor(descriptor_path).map_err(map_control_error)?;
    client
        .health_with_deadline(probe_deadline)
        .await
        .map_err(map_control_error)
}

#[cfg(unix)]
fn spawn_daemon_process(config: &DaemonBootstrapConfig) -> Result<SpawnedDaemon, DaemonCliError> {
    let mut command = Command::new(&config.daemon_executable);
    command
        .arg("--project-root")
        .arg(config.paths.project_root())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    use std::os::unix::process::CommandExt;
    command.process_group(0);
    command
        .spawn()
        .map(|child| SpawnedDaemon { child })
        .map_err(|_| DaemonCliError::SpawnFailed)
}

#[cfg(windows)]
fn spawn_daemon_process(config: &DaemonBootstrapConfig) -> Result<SpawnedDaemon, DaemonCliError> {
    windows_bootstrap::spawn_daemon(&config.daemon_executable, config.paths.project_root())
        .map(|child| SpawnedDaemon { child: Some(child) })
        .map_err(|_| DaemonCliError::SpawnFailed)
}
fn terminate_child(child: &mut Child) {
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
}

fn control_client(selection: &RuntimeSelection<'_>) -> Result<ControlClient, DaemonCliError> {
    if !runtime_artifact_exists(selection.descriptor_path)? {
        return Err(if selection.has_artifacts {
            DaemonCliError::StaleRuntime {
                reason_code: "control_descriptor_unavailable".to_string(),
            }
        } else {
            DaemonCliError::NotRunning
        });
    }
    ControlClient::from_descriptor(selection.descriptor_path).map_err(map_control_error)
}

fn validate_selected_runtime(
    paths: &DaemonPaths,
    selection: &RuntimeSelection<'_>,
) -> Result<(), DaemonCliError> {
    if !selection.has_artifacts {
        return Err(DaemonCliError::NotRunning);
    }
    match selection.location {
        RuntimeLocation::Protected => paths.validate_runtime_dir(),
        RuntimeLocation::Legacy => paths.validate_state_dir(),
    }
    .map_err(DaemonCliError::from)
}

fn select_runtime(paths: &DaemonPaths) -> Result<RuntimeSelection<'_>, DaemonCliError> {
    let current_descriptor = runtime_artifact_exists(paths.descriptor_path())?;
    let current_lock = runtime_artifact_exists(paths.writer_lock_path())?;
    if !paths.has_distinct_legacy_runtime() {
        return Ok(RuntimeSelection {
            descriptor_path: paths.descriptor_path(),
            writer_lock_path: paths.writer_lock_path(),
            compatibility_lock_path: None,
            location: RuntimeLocation::Protected,
            has_artifacts: current_descriptor || current_lock,
        });
    }
    let legacy_descriptor = runtime_artifact_exists(paths.legacy_descriptor_path())?;
    let legacy_lock = runtime_artifact_exists(paths.legacy_writer_lock_path())?;
    if (current_descriptor || current_lock) && legacy_descriptor {
        return Err(DaemonCliError::StaleRuntime {
            reason_code: "daemon_runtime_generation_conflict".to_string(),
        });
    }
    if current_descriptor || current_lock {
        return Ok(RuntimeSelection {
            descriptor_path: paths.descriptor_path(),
            writer_lock_path: paths.writer_lock_path(),
            compatibility_lock_path: legacy_lock.then_some(paths.legacy_writer_lock_path()),
            location: RuntimeLocation::Protected,
            has_artifacts: true,
        });
    }
    if legacy_descriptor || legacy_lock {
        return Ok(RuntimeSelection {
            descriptor_path: paths.legacy_descriptor_path(),
            writer_lock_path: paths.legacy_writer_lock_path(),
            compatibility_lock_path: None,
            location: RuntimeLocation::Legacy,
            has_artifacts: true,
        });
    }
    Ok(RuntimeSelection {
        descriptor_path: paths.descriptor_path(),
        writer_lock_path: paths.writer_lock_path(),
        compatibility_lock_path: None,
        location: RuntimeLocation::Protected,
        has_artifacts: false,
    })
}

fn map_control_error(error: ControlError) -> DaemonCliError {
    let code = error.code().to_string();
    match error {
        ControlError::DescriptorInvalid
        | ControlError::EndpointUnavailable
        | ControlError::Unauthorized
        | ControlError::UnsupportedVersion
        | ControlError::Deadline => DaemonCliError::StaleRuntime { reason_code: code },
        _ => DaemonCliError::Control { code },
    }
}

fn runtime_artifact_exists(path: &Path) -> Result<bool, DaemonCliError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(DaemonCliError::StaleRuntime {
            reason_code: "runtime_artifact_inaccessible".to_string(),
        }),
    }
}

fn validate_bootstrap_config(config: &DaemonBootstrapConfig) -> Result<(), DaemonCliError> {
    if config.startup_timeout.is_zero()
        || config.poll_interval.is_zero()
        || config.health_probe_timeout.is_zero()
    {
        return Err(DaemonCliError::InvalidConfiguration);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use vibemuxd::control::DaemonControlServer;

    use super::*;

    #[cfg(windows)]
    struct ControlRuntimeCleanup(PathBuf);

    #[cfg(windows)]
    impl ControlRuntimeCleanup {
        fn new(paths: &DaemonPaths) -> Self {
            Self(paths.runtime_dir().to_path_buf())
        }
    }

    #[cfg(windows)]
    impl Drop for ControlRuntimeCleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn parser_preserves_path_arguments_without_command_strings() {
        let command = parse_cli_arguments(
            [
                "daemon",
                "start",
                "--project-root",
                "C:\\project with spaces",
                "--daemon-executable",
                "C:\\bin with spaces\\vibemuxd.exe",
            ]
            .into_iter()
            .map(OsString::from),
        )
        .expect("parse lifecycle command");
        assert_eq!(
            command,
            CliCommand::DaemonStart {
                project_root: Some(PathBuf::from("C:\\project with spaces")),
                daemon_executable: Some(PathBuf::from("C:\\bin with spaces\\vibemuxd.exe")),
            }
        );
        assert!(parse_cli_arguments([OsString::from("daemon start")].into_iter()).is_err());

        let confirmation = "a".repeat(64);
        assert_eq!(
            parse_cli_arguments(
                [
                    OsString::from("daemon"),
                    OsString::from("recover"),
                    OsString::from("--confirmation"),
                    OsString::from(&confirmation),
                ]
                .into_iter()
            )
            .expect("parse recovery command"),
            CliCommand::DaemonRecover {
                project_root: None,
                confirmation,
            }
        );
        assert!(
            parse_cli_arguments([OsString::from("daemon"), OsString::from("recover")].into_iter())
                .is_err()
        );
    }

    #[tokio::test]
    async fn missing_descriptor_reports_not_running() {
        let temp = tempfile::tempdir().expect("temp project");
        let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
        assert_eq!(
            daemon_health(&paths)
                .await
                .expect_err("daemon must be absent"),
            DaemonCliError::NotRunning
        );
    }

    #[tokio::test]
    async fn stale_descriptor_is_reported_without_modification_or_spawn() {
        let temp = tempfile::tempdir().expect("temp project");
        let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
        #[cfg(windows)]
        let _control_cleanup = ControlRuntimeCleanup::new(&paths);
        paths.ensure_runtime_dir().expect("runtime directory");
        let stale = b"stale-control-descriptor";
        std::fs::write(paths.descriptor_path(), stale).expect("stale descriptor");
        let config = short_test_config(paths.clone());

        assert!(matches!(
            start_daemon(&config).await,
            Err(DaemonCliError::StaleRuntime { .. })
        ));
        assert_eq!(
            std::fs::read(paths.descriptor_path()).expect("read stale descriptor"),
            stale
        );
        assert!(!paths.database_path().exists());
    }

    #[tokio::test]
    async fn lock_only_state_is_reported_without_modification_or_spawn() {
        let temp = tempfile::tempdir().expect("temp project");
        let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
        #[cfg(windows)]
        let _control_cleanup = ControlRuntimeCleanup::new(&paths);
        paths.ensure_runtime_dir().expect("runtime directory");
        let stale = b"stale-writer-lock";
        std::fs::write(paths.writer_lock_path(), stale).expect("stale writer lock");
        let config = short_test_config(paths.clone());

        assert!(matches!(
            start_daemon(&config).await,
            Err(DaemonCliError::StaleRuntime { .. })
        ));
        assert_eq!(
            std::fs::read(paths.writer_lock_path()).expect("read stale lock"),
            stale
        );
        assert!(!paths.descriptor_path().exists());
        assert!(!paths.database_path().exists());
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn healthy_legacy_daemon_is_discoverable_and_stoppable() {
        let temp = tempfile::tempdir().expect("temp project");
        let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
        let _control_cleanup = ControlRuntimeCleanup::new(&paths);
        paths.ensure_runtime_dir().expect("runtime directory");
        let legacy_server = DaemonControlServer::start(paths.database_path(), paths.state_dir())
            .await
            .expect("legacy control server");
        let health = daemon_health(&paths).await.expect("legacy daemon health");
        let config = short_test_config(paths.clone());
        let outcome = start_daemon(&config)
            .await
            .expect("legacy daemon already running");
        assert!(matches!(outcome, DaemonStartOutcome::AlreadyRunning(_)));
        assert_eq!(outcome.health().process_id, health.process_id);
        stop_daemon(&paths).await.expect("stop legacy daemon");
        legacy_server.wait().await.expect("wait legacy daemon");
        assert!(!paths.legacy_descriptor_path().exists());
        assert!(!paths.legacy_writer_lock_path().exists());
        assert!(!paths.descriptor_path().exists());
        assert!(!paths.writer_lock_path().exists());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn classify_daemon_exit_distinguishes_security_failures() {
        // The marker below is deliberately loosened, so this test runs on a
        // PRIVATE control root. The real `%LOCALAPPDATA%` marker is shared
        // with every sibling test and live daemon, and each trusted start
        // verifies its ACL in phase 1: loosening the shared one failed the
        // concurrent `recovery::tests` daemon starts with
        // `ControlRuntimeSecurityInvalid` (stage 7 on index 3, the marker).
        let temp = tempfile::tempdir().expect("temp project");
        let control_base = tempfile::tempdir().expect("isolated control base");
        let paths =
            DaemonPaths::from_project_root_with_local_app_data(temp.path(), control_base.path())
                .expect("daemon paths");
        paths.ensure_runtime_dir().expect("runtime directory");
        // Healthy ACLs: the daemon's exit cause stays the generic failure.
        assert_eq!(
            classify_daemon_exit(&paths).await,
            DaemonCliError::StartFailed
        );
        // Loosen ONLY the `.acl_v1` marker: the daemon's phase-1
        // verification rejects it, and the CLI re-check must classify the
        // security failure too (it used to check root+leaf only and
        // reported the generic `daemon_start_failed` for marker drift -
        // the code-review V2 finding).
        let marker = paths.acl_marker_path();
        vibemux_platform::test_helpers::loosen_with_icacls(&marker, "*S-1-1-0:F");
        assert_eq!(
            classify_daemon_exit(&paths).await,
            DaemonCliError::Control {
                code: "daemon_control_runtime_security_invalid".to_string()
            }
        );
        // Regression pin for the shared-marker race: while this test holds
        // its marker loosened, a sibling project's surviving surface under
        // the REAL shared control root must still verify. With the old
        // shared-root setup this failed deterministically.
        let sibling_temp = tempfile::tempdir().expect("sibling project");
        let sibling = DaemonPaths::from_project_root(sibling_temp.path()).expect("sibling paths");
        let _sibling_cleanup = ControlRuntimeCleanup::new(&sibling);
        sibling
            .ensure_runtime_dir()
            .expect("sibling runtime directory");
        sibling
            .verify_control_surviving_acls()
            .expect("sibling control surface is unaffected by this test");
        vibemux_platform::test_helpers::restore_with_icacls(&marker, "*S-1-1-0");
        // Loosen the project-private runtime leaf only.
        vibemux_platform::test_helpers::loosen_with_icacls(
            paths.runtime_dir(),
            "*S-1-5-32-545:(OI)(CI)F",
        );
        assert_eq!(
            classify_daemon_exit(&paths).await,
            DaemonCliError::Control {
                code: "daemon_control_runtime_security_invalid".to_string()
            }
        );
        // An ABSENT leaf under a healthy root and marker is what a start
        // sees when it races the previous daemon's clean-shutdown release:
        // the plain failure, not a loosened ACL (it used to require the
        // leaf to exist and reported `security_invalid`).
        std::fs::remove_dir(paths.runtime_dir()).expect("release the empty leaf");
        assert_eq!(
            classify_daemon_exit(&paths).await,
            DaemonCliError::StartFailed
        );
        // Every path that still exists stays fail-closed.
        vibemux_platform::test_helpers::loosen_with_icacls(&marker, "*S-1-1-0:F");
        assert_eq!(
            classify_daemon_exit(&paths).await,
            DaemonCliError::Control {
                code: "daemon_control_runtime_security_invalid".to_string()
            }
        );
    }

    fn short_test_config(paths: DaemonPaths) -> DaemonBootstrapConfig {
        DaemonBootstrapConfig::new(paths, PathBuf::from("missing_vibemuxd")).with_timing(
            Duration::from_millis(75),
            Duration::from_millis(5),
            Duration::from_millis(10),
        )
    }

    #[test]
    fn parser_routes_harness_commands() {
        assert_eq!(
            parse_cli_arguments([OsString::from("harnesses")].into_iter()).expect("harnesses"),
            CliCommand::Harnesses {
                project_root: None,
                cached: false,
            }
        );
        assert_eq!(
            parse_cli_arguments(
                [
                    OsString::from("harnesses"),
                    OsString::from("--cached"),
                    OsString::from("--project-root"),
                    OsString::from("C:\\repo"),
                ]
                .into_iter()
            )
            .expect("harnesses cached"),
            CliCommand::Harnesses {
                project_root: Some(PathBuf::from("C:\\repo")),
                cached: true,
            }
        );
        assert_eq!(
            parse_cli_arguments(
                [
                    OsString::from("switch"),
                    OsString::from("claude"),
                    OsString::from("--project-root"),
                    OsString::from("."),
                ]
                .into_iter()
            )
            .expect("switch"),
            CliCommand::Switch {
                project_root: Some(PathBuf::from(".")),
                harness: "claude".to_string(),
            }
        );
        assert!(parse_cli_arguments([OsString::from("switch")].into_iter()).is_err());
        assert!(
            parse_cli_arguments([OsString::from("harnesses"), OsString::from("bogus")].into_iter())
                .is_err()
        );
    }

    #[test]
    fn harness_error_messages_carry_the_harness_name() {
        let probe_cache = PathBuf::from("C:\\tmp\\.vibemux\\probe_cache.json");
        for (code, name) in [
            ("harness_not_detected", "qwen"),
            ("store_unknown_harness", "bogus-harness"),
        ] {
            let error = map_harness_control_error(
                &probe_cache,
                ControlError::Remote {
                    code: code.to_string(),
                },
                Some(name),
            );
            assert_eq!(error.code(), code);
            let rendered = error.to_string();
            assert!(
                rendered.contains(name),
                "'{code}' message must name the harness ({name}): {rendered}"
            );
            assert!(
                !rendered.contains(code),
                "'{code}' message must not leak the wire code: {rendered}"
            );
        }
        let missing = map_harness_control_error(
            &probe_cache,
            ControlError::Remote {
                code: "harness_probe_cache_missing".to_string(),
            },
            None,
        );
        assert_eq!(missing.code(), "harness_probe_cache_missing");
        let rendered = missing.to_string();
        assert!(rendered.contains("--write-cache --project-root <root>"));
        assert!(rendered.contains("C:\\tmp\\.vibemux\\probe_cache.json"));
    }
}
