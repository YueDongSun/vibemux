#![forbid(unsafe_code)]
//! Thin lifecycle client and controlled bootstrap for the Rust daemon.

pub mod recovery;

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};

#[cfg(windows)]
use std::io::{BufRead, BufReader, Write};
#[cfg(windows)]
use std::{sync::mpsc, thread, time::Instant as StdInstant};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::time::{Instant, sleep};
use vibemuxd::{
    control::{ControlClient, ControlError, DaemonHealth},
    process::{DaemonPathError, DaemonPaths},
};

pub const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(25);
pub const DEFAULT_HEALTH_PROBE_TIMEOUT: Duration = Duration::from_millis(500);
pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const WINDOWS_POWERSHELL_RELATIVE_PATH: &str = "System32\\WindowsPowerShell\\v1.0\\powershell.exe";
#[cfg(windows)]
const DAEMON_EXECUTABLE_ENV: &str = "VIBEMUX_DAEMON_EXECUTABLE";
#[cfg(windows)]
const DAEMON_PROJECT_ROOT_ENV: &str = "VIBEMUX_DAEMON_PROJECT_ROOT";
#[cfg(windows)]
const WINDOWS_HELPER_TIMEOUT: Duration = Duration::from_secs(3);
#[cfg(windows)]
const WINDOWS_LAUNCHER_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$daemon_executable = [Environment]::GetEnvironmentVariable('VIBEMUX_DAEMON_EXECUTABLE')
$project_root = [Environment]::GetEnvironmentVariable('VIBEMUX_DAEMON_PROJECT_ROOT')
if ([string]::IsNullOrWhiteSpace($daemon_executable) -or [string]::IsNullOrWhiteSpace($project_root)) { exit 4 }
[Environment]::SetEnvironmentVariable('VIBEMUX_DAEMON_EXECUTABLE', $null, 'Process')
$start_info = New-Object System.Diagnostics.ProcessStartInfo
$start_info.FileName = $daemon_executable
$start_info.WorkingDirectory = $project_root
$start_info.UseShellExecute = $true
$start_info.WindowStyle = [System.Diagnostics.ProcessWindowStyle]::Hidden
try {
    $process = [System.Diagnostics.Process]::Start($start_info)
    if ($null -eq $process) { exit 4 }
    [Console]::Out.WriteLine($process.Id)
    [Console]::Out.Flush()
    $control_task = [Console]::In.ReadLineAsync()
    while (-not $control_task.Wait(25)) {
        if ($process.HasExited) { exit 5 }
    }
    $action = $control_task.Result
    if ($action -eq 'release') { exit 0 }
    if (-not $process.HasExited) {
        $process.Kill()
        $process.WaitForExit()
    }
    exit 0
} catch {
    exit 4
}
"#;

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
    process_id: u32,
    helper: Child,
    control: Option<std::process::ChildStdin>,
    helper_timeout: Duration,
}

#[cfg(unix)]
impl SpawnedDaemon {
    fn process_id(&self) -> u32 {
        self.child.id()
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
    fn process_id(&self) -> u32 {
        self.process_id
    }

    fn has_exited(&mut self) -> Result<bool, DaemonCliError> {
        self.helper
            .try_wait()
            .map(|status| status.is_some())
            .map_err(|_| DaemonCliError::StartFailed)
    }

    fn release(mut self) -> Result<(), DaemonCliError> {
        self.send_control("release")?;
        let status = wait_for_child_exit(&mut self.helper, self.helper_timeout)?
            .ok_or(DaemonCliError::StartFailed)?;
        if status.success() {
            Ok(())
        } else {
            Err(DaemonCliError::StartFailed)
        }
    }

    fn terminate(&mut self) {
        let _ = self.send_control("terminate");
        if wait_for_child_exit(&mut self.helper, self.helper_timeout)
            .ok()
            .flatten()
            .is_none()
        {
            terminate_child(&mut self.helper);
        }
    }

    fn send_control(&mut self, action: &str) -> Result<(), DaemonCliError> {
        send_windows_control(&mut self.control, action)
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
    Ok(health)
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
    if arguments.len() < 2 || arguments[0] != "daemon" {
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
                        .map(SpawnedDaemon::process_id)
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

        if let Some(spawned) = spawned.as_mut() {
            if spawned.has_exited()? {
                child_exited = true;
            }
        }
        if child_exited && !select_runtime(&config.paths)?.has_artifacts {
            return Err(DaemonCliError::StartFailed);
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
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use std::os::windows::process::CommandExt;

    let powershell = windows_powershell()?;
    let encoded_script = {
        let mut utf16 = Vec::with_capacity(WINDOWS_LAUNCHER_SCRIPT.len() * 2);
        for unit in WINDOWS_LAUNCHER_SCRIPT.encode_utf16() {
            utf16.extend_from_slice(&unit.to_le_bytes());
        }
        STANDARD.encode(utf16)
    };
    let mut command = Command::new(powershell);
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
        ])
        .arg(encoded_script)
        .env(DAEMON_EXECUTABLE_ENV, &config.daemon_executable)
        .env(DAEMON_PROJECT_ROOT_ENV, config.paths.project_root())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    let mut helper = command.spawn().map_err(|_| DaemonCliError::SpawnFailed)?;
    let control = helper.stdin.take().ok_or(DaemonCliError::SpawnFailed)?;
    let stdout = helper.stdout.take().ok_or(DaemonCliError::SpawnFailed)?;
    let (line_sender, line_receiver) = mpsc::sync_channel(1);
    let reader = thread::spawn(move || {
        let mut process_id_line = String::new();
        let result = BufReader::new(stdout)
            .read_line(&mut process_id_line)
            .map(|_| process_id_line);
        let _ = line_sender.send(result);
    });
    let helper_timeout = config.startup_timeout.max(WINDOWS_HELPER_TIMEOUT);
    let process_id_line = match line_receiver.recv_timeout(helper_timeout) {
        Ok(Ok(line)) => line,
        Ok(Err(_)) | Err(_) => {
            let mut control = Some(control);
            let _ = send_windows_control(&mut control, "terminate");
            if wait_for_child_exit(&mut helper, helper_timeout)
                .ok()
                .flatten()
                .is_none()
            {
                terminate_child(&mut helper);
            }
            return Err(DaemonCliError::SpawnFailed);
        }
    };
    let _ = reader.join();
    let process_id = match process_id_line.trim().parse::<u32>() {
        Ok(process_id) if process_id != 0 => process_id,
        _ => {
            let mut control = Some(control);
            let _ = send_windows_control(&mut control, "terminate");
            if wait_for_child_exit(&mut helper, helper_timeout)
                .ok()
                .flatten()
                .is_none()
            {
                terminate_child(&mut helper);
            }
            return Err(DaemonCliError::SpawnFailed);
        }
    };
    Ok(SpawnedDaemon {
        process_id,
        helper,
        control: Some(control),
        helper_timeout,
    })
}

#[cfg(windows)]
fn windows_powershell() -> Result<PathBuf, DaemonCliError> {
    let system_root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .ok_or(DaemonCliError::SpawnFailed)?;
    let powershell = system_root.join(WINDOWS_POWERSHELL_RELATIVE_PATH);
    if powershell.is_file() {
        Ok(powershell)
    } else {
        Err(DaemonCliError::SpawnFailed)
    }
}

#[cfg(windows)]
fn send_windows_control(
    control: &mut Option<std::process::ChildStdin>,
    action: &str,
) -> Result<(), DaemonCliError> {
    let mut control = control.take().ok_or(DaemonCliError::StartFailed)?;
    writeln!(control, "{action}").map_err(|_| DaemonCliError::StartFailed)?;
    control.flush().map_err(|_| DaemonCliError::StartFailed)
}

#[cfg(windows)]
fn wait_for_child_exit(
    child: &mut Child,
    deadline: Duration,
) -> Result<Option<std::process::ExitStatus>, DaemonCliError> {
    let expires = StdInstant::now() + deadline;
    loop {
        if let Some(status) = child.try_wait().map_err(|_| DaemonCliError::StartFailed)? {
            return Ok(Some(status));
        }
        if StdInstant::now() >= expires {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(10));
    }
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

    fn short_test_config(paths: DaemonPaths) -> DaemonBootstrapConfig {
        DaemonBootstrapConfig::new(paths, PathBuf::from("missing_vibemuxd")).with_timing(
            Duration::from_millis(75),
            Duration::from_millis(5),
            Duration::from_millis(10),
        )
    }
}
