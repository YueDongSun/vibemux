//! Windows effect owner for the explicit fixed probe (ADR 043).

use std::{
    ffi::OsString,
    fs, io,
    net::Ipv4Addr,
    path::{Component, Path, PathBuf, Prefix},
    process::{Command as StdCommand, Stdio},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    process::Command,
    task::JoinHandle,
    time::{Instant, sleep, timeout},
};
use vibemux_platform::{
    LAUNCH_GO_BYTE, ProcessTree,
    sandbox_mxc::{MxcError, MxcOfflinePolicy, offline_command},
};

use super::{
    ProbeOutcome, SandboxProbeReport,
    evidence::{FixtureReport, validate_evidence},
};

const EXECUTION_DEADLINE: Duration = Duration::from_secs(30);
const REAP_DEADLINE: Duration = Duration::from_secs(3);
const SOCKET_DEADLINE: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const OUTPUT_LIMIT: usize = 16 * 1024;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const JUNCTION_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$link = [Environment]::GetEnvironmentVariable('VIBEMUX_PROBE_LINK')
$target = [Environment]::GetEnvironmentVariable('VIBEMUX_PROBE_TARGET')
if ([string]::IsNullOrEmpty($link) -or [string]::IsNullOrEmpty($target)) { exit 4 }
New-Item -ItemType Junction -Path $link -Target $target -ErrorAction Stop | Out-Null
"#;

struct Fixture {
    root: PathBuf,
    workspace: PathBuf,
    denied: PathBuf,
    nonce: String,
}

fn local_drive_path(path: &Path) -> bool {
    path.is_absolute()
        && matches!(path.components().next(), Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
        && !path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
}

impl Fixture {
    fn create(parent: &Path) -> io::Result<Self> {
        // Reject network/device syntax before any metadata access or creation.
        if !local_drive_path(parent) || !parent.is_dir() {
            return Err(io::Error::other("invalid parent"));
        }
        let parent = parent.canonicalize()?;
        if !local_drive_path(&parent) {
            return Err(io::Error::other("non-local canonical parent"));
        }
        let root = tempfile::Builder::new()
            .prefix("vibemux_sandbox_probe_")
            .tempdir_in(parent)?;
        let workspace = root.path().join("workspace");
        let denied = root.path().join("denied");
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        for directory in [
            &workspace,
            &denied,
            &workspace.join("scratch"),
            &workspace.join(".git"),
            &workspace.join("sealed"),
        ] {
            fs::create_dir(directory)?;
        }
        for file in [
            workspace.join("readable"),
            workspace.join(".git/sentinel"),
            workspace.join("sealed/sentinel"),
            denied.join("sentinel"),
        ] {
            fs::write(file, &nonce)?;
        }
        fs::write(root.path().join("owner"), &nonce)?;
        Ok(Self {
            // Cancellation retains this directory. Only terminal evidence
            // authorizes the explicit removal below.
            root: root.keep(),
            workspace,
            denied,
            nonce,
        })
    }

    fn remove(&self) -> bool {
        if self.root.canonicalize().ok().as_ref() != Some(&self.root)
            || !fs::symlink_metadata(&self.root)
                .is_ok_and(|value| value.is_dir() && !value.file_type().is_symlink())
            || fs::read(self.root.join("owner")).ok().as_deref() != Some(self.nonce.as_bytes())
            || self
                .root
                .file_name()
                .and_then(|name| name.to_str())
                .is_none_or(|name| !name.starts_with("vibemux_sandbox_probe_"))
        {
            return false;
        }
        fs::remove_dir_all(&self.root).is_ok()
    }

    fn preflight(&self) -> io::Result<()> {
        for file in [
            self.workspace.join("readable"),
            self.workspace.join(".git/sentinel"),
            self.workspace.join("sealed/sentinel"),
            self.denied.join("sentinel"),
            self.workspace.join("link/sentinel"),
        ] {
            if fs::read(file)? != self.nonce.as_bytes() {
                return Err(io::Error::other("fixture mismatch"));
            }
        }
        // Prove the same forbidden destinations are writable outside MXC, then
        // restore the fixture before launching the fixed isolated workload.
        for file in [
            self.denied.join("written"),
            self.denied.join("link_written"),
            self.denied.join("child_written"),
        ] {
            fs::write(&file, &self.nonce)?;
            fs::remove_file(file)?;
        }
        fs::write(self.workspace.join(".git/sentinel"), "preflight")?;
        fs::write(self.workspace.join(".git/sentinel"), &self.nonce)?;
        Ok(())
    }

    fn postconditions(&self, report: &mut SandboxProbeReport) {
        let equal =
            |path: PathBuf| fs::read(path).is_ok_and(|bytes| bytes == self.nonce.as_bytes());
        let independent = &mut report.independent;
        independent.workspace_output = equal(self.workspace.join("written"));
        independent.scratch_output = equal(self.workspace.join("scratch/written"));
        independent.child_started = equal(self.workspace.join("child_started"));
        independent.protected_unchanged = equal(self.workspace.join(".git/sentinel"));
        independent.sealed_unchanged = equal(self.workspace.join("sealed/sentinel"));
        independent.outside_unchanged = equal(self.denied.join("sentinel"));
        independent.forbidden_outputs_absent = ["written", "link_written", "child_written"]
            .into_iter()
            .all(|name| matches!(self.denied.join(name).try_exists(), Ok(false)));
    }
}

struct Listener {
    port: u16,
    accepts: Arc<AtomicU64>,
    task: Option<JoinHandle<()>>,
}

impl Listener {
    async fn start(nonce: &str) -> io::Result<Self> {
        let socket = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let port = socket.local_addr()?.port();
        let expected = nonce.as_bytes().to_vec();
        let accepts = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&accepts);
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = socket.accept().await {
                counter.fetch_add(1, Ordering::SeqCst);
                let mut challenge = [0; 32];
                let _ = timeout(SOCKET_DEADLINE, async {
                    stream.read_exact(&mut challenge).await?;
                    if challenge.as_slice() != expected {
                        return Err(io::Error::other("challenge mismatch"));
                    }
                    stream.write_all(&challenge).await
                })
                .await;
            }
        });
        Ok(Self {
            port,
            accepts,
            task: Some(task),
        })
    }

    async fn preflight(&self, nonce: &str) -> bool {
        timeout(SOCKET_DEADLINE, async {
            let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, self.port)).await?;
            socket.write_all(nonce.as_bytes()).await?;
            let mut response = [0; 32];
            socket.read_exact(&mut response).await?;
            Ok::<_, io::Error>(response.as_slice() == nonce.as_bytes())
        })
        .await
        .is_ok_and(|result| matches!(result, Ok(true)))
    }

    async fn stop(mut self) -> bool {
        let Some(task) = self.task.take() else {
            return false;
        };
        task.abort();
        matches!(timeout(REAP_DEADLINE, task).await, Ok(Ok(())) | Ok(Err(_)))
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

struct Captured {
    code: Option<i32>,
    execution_completed: bool,
    stdout: Vec<u8>,
    stderr_bytes: usize,
    output_complete: bool,
    tree_empty: bool,
    backend_reported_unavailable: bool,
}

async fn read_output(reader: impl AsyncRead + Unpin) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take((OUTPUT_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .await?;
    Ok(bytes)
}

async fn join_output(task: &mut JoinHandle<io::Result<Vec<u8>>>) -> Option<Vec<u8>> {
    match timeout(REAP_DEADLINE, &mut *task).await {
        Ok(Ok(Ok(bytes))) if bytes.len() <= OUTPUT_LIMIT => Some(bytes),
        _ => {
            task.abort();
            None
        }
    }
}

/// Reuse ADR 029's contained trampoline; never release the backend before
/// assignment. No arbitrary inherited environment survives this boundary.
async fn execute_contained(
    executable: &Path,
    desired: &StdCommand,
) -> Result<Captured, &'static str> {
    let mut command = Command::new(executable);
    command
        .arg("__launch_trampoline")
        .arg(desired.get_program())
        .args(desired.get_args())
        .env_clear()
        .envs(
            desired
                .get_envs()
                .filter_map(|(key, value)| value.map(|value| (key, value))),
        )
        .current_dir(desired.get_current_dir().ok_or("sandbox_cwd_missing")?)
        .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    ProcessTree::prepare_command(command.as_std_mut());
    let mut child = command
        .spawn()
        .map_err(|_| "sandbox_backend_spawn_failed")?;
    let tree = child.id().and_then(|pid| ProcessTree::contain(pid).ok());
    let Some(mut tree) = tree else {
        let _ = child.start_kill();
        let _ = timeout(REAP_DEADLINE, child.wait()).await;
        return Err("sandbox_containment_failed");
    };
    let (Some(mut input), Some(output), Some(error)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        let _ = tree.terminate();
        let _ = timeout(REAP_DEADLINE, child.wait()).await;
        return Err("sandbox_pipe_failed");
    };
    let mut output_task = tokio::spawn(read_output(output));
    let mut error_task = tokio::spawn(read_output(error));
    let released = timeout(REAP_DEADLINE, input.write_all(&[LAUNCH_GO_BYTE]))
        .await
        .is_ok_and(|result| result.is_ok());
    drop(input);
    let mut status = if released {
        timeout(EXECUTION_DEADLINE, child.wait())
            .await
            .ok()
            .and_then(Result::ok)
    } else {
        None
    };
    let execution_completed = status.is_some();
    let terminated = tree.terminate().is_ok();
    if status.is_none() {
        status = timeout(REAP_DEADLINE, child.wait())
            .await
            .ok()
            .and_then(Result::ok);
    }
    let deadline = Instant::now() + REAP_DEADLINE;
    let mut tree_empty = false;
    while Instant::now() < deadline {
        match tree.active_process_count() {
            Ok(0) => {
                tree_empty = terminated;
                break;
            }
            Ok(_) => sleep(POLL_INTERVAL).await,
            Err(_) => break,
        }
    }
    let output = join_output(&mut output_task).await;
    let error = join_output(&mut error_task).await;
    let complete = output.is_some() && error.is_some();
    let backend_reported_unavailable = error.as_ref().is_some_and(|bytes| {
        let marker = b"native MXC is unavailable";
        bytes.windows(marker.len()).any(|window| window == marker)
    });
    Ok(Captured {
        code: status.and_then(|s| s.code()),
        execution_completed,
        stdout: output.unwrap_or_default(),
        stderr_bytes: error.as_ref().map_or(OUTPUT_LIMIT + 1, Vec::len),
        output_complete: complete,
        tree_empty,
        backend_reported_unavailable,
    })
}

fn version_text(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?.trim();
    let version = text.strip_prefix("codex-cli ")?;
    if version.is_empty()
        || version.len() > 128
        || !version
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b".-_+".contains(&c))
    {
        return None;
    }
    Some(version.to_owned())
}

fn junction_command(fixture: &Fixture) -> Result<StdCommand, &'static str> {
    let system_root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or("sandbox_junction_helper_unavailable")?;
    let helper = system_root.join("System32/WindowsPowerShell/v1.0/powershell.exe");
    if !helper.is_file() {
        return Err("sandbox_junction_helper_unavailable");
    }
    let normal_path = |path: &Path| -> Result<OsString, &'static str> {
        let text = path.to_str().ok_or("sandbox_fixture_path_invalid")?;
        Ok(OsString::from(text.strip_prefix(r"\\?\").unwrap_or(text)))
    };
    let mut command = StdCommand::new(helper);
    command
        .args(["-NoProfile", "-NonInteractive", "-Command", JUNCTION_SCRIPT])
        .env_clear()
        .env("SystemRoot", &system_root)
        .env("WINDIR", &system_root)
        .env("TEMP", fixture.workspace.join("scratch"))
        .env("TMP", fixture.workspace.join("scratch"))
        .env(
            "VIBEMUX_PROBE_LINK",
            normal_path(&fixture.workspace.join("link"))?,
        )
        .env("VIBEMUX_PROBE_TARGET", normal_path(&fixture.denied)?)
        .current_dir(&fixture.workspace);
    Ok(command)
}

pub(super) async fn run(backend: &Path, parent: &Path) -> SandboxProbeReport {
    let mut report = SandboxProbeReport::new();
    let fixture = match Fixture::create(parent) {
        Ok(fixture) => fixture,
        Err(_) => {
            report.reason_code = Some("sandbox_fixture_unavailable");
            return report;
        }
    };
    let mut execution_started = false;
    let result = run_fixture(backend, &fixture, &mut report, &mut execution_started).await;
    fixture.postconditions(&mut report);
    let nonce = fixture.nonce.clone();
    // Explicit removal does not follow the directory symlink. Cancellation
    // skips this path and retains the generated directory for reconciliation.
    let fixture_name = fixture
        .root
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned);
    if !execution_started
        || (report.independent.process_exited && report.independent.process_tree_empty)
    {
        report.independent.fixture_removed = fixture.remove();
        if !report.independent.fixture_removed {
            report.retained_fixture_name = fixture_name;
        }
    } else {
        report.retained_fixture_name = fixture_name;
    }
    match result {
        Ok(observed) => {
            let validation = validate_evidence(&observed, &nonce, &report.independent);
            report.checks = observed.checks;
            report.outcome = if validation.is_ok() {
                ProbeOutcome::Passed
            } else {
                ProbeOutcome::Failed
            };
            report.reason_code = validation.err();
        }
        Err(code) => {
            report.outcome = if code == "sandbox_mxc_unavailable" {
                ProbeOutcome::Unavailable
            } else {
                ProbeOutcome::Failed
            };
            report.reason_code = Some(code);
        }
    }
    report
}

async fn run_fixture(
    backend: &Path,
    fixture: &Fixture,
    report: &mut SandboxProbeReport,
    execution_started: &mut bool,
) -> Result<FixtureReport, &'static str> {
    let executable = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|_| "sandbox_runner_unavailable")?;
    let junction = junction_command(fixture)?;
    *execution_started = true;
    let setup = execute_contained(&executable, &junction).await?;
    report.independent.process_exited = setup.code.is_some();
    report.independent.process_tree_empty = setup.tree_empty;
    if setup.code != Some(0)
        || !setup.execution_completed
        || !setup.output_complete
        || !setup.tree_empty
    {
        return Err("sandbox_junction_setup_failed");
    }
    fixture
        .preflight()
        .map_err(|_| "sandbox_fixture_preflight_failed")?;
    report.independent.fixture_preflight = true;
    let listener = Listener::start(&fixture.nonce)
        .await
        .map_err(|_| "sandbox_listener_unavailable")?;
    if !listener.preflight(&fixture.nonce).await {
        return Err("sandbox_listener_preflight_failed");
    }
    report.independent.listener_challenge = true;
    let before = listener.accepts.load(Ordering::SeqCst);
    let policy = MxcOfflinePolicy {
        workspace: fixture.workspace.clone(),
        scratch: fixture.workspace.join("scratch"),
        read_only_paths: vec![fixture.workspace.join(".git")],
        denied_paths: vec![fixture.workspace.join("sealed")],
        profile_id: format!("rd_mxc_{}", uuid::Uuid::new_v4().simple()),
    };
    let arguments = vec![
        OsString::from("__fixture"),
        fixture.workspace.as_os_str().into(),
        fixture.denied.as_os_str().into(),
        OsString::from(&fixture.nonce),
        OsString::from(listener.port.to_string()),
    ];
    let mut alias_policy = policy.clone();
    alias_policy.scratch = fixture.workspace.join("link");
    let scratch_alias_rejected = matches!(
        offline_command(backend, &executable, &arguments, &alias_policy),
        Err(MxcError::PathNotCanonical)
    );
    let mut alias_policy = policy.clone();
    alias_policy.read_only_paths = vec![fixture.workspace.join("link")];
    let protected_alias_rejected = matches!(
        offline_command(backend, &executable, &arguments, &alias_policy),
        Err(MxcError::PathNotCanonical)
    );
    report.independent.path_aliases_rejected = scratch_alias_rejected && protected_alias_rejected;
    if !report.independent.path_aliases_rejected {
        return Err("sandbox_path_alias_control_failed");
    }
    let desired =
        offline_command(backend, &executable, &arguments, &policy).map_err(|error| error.code())?;
    // Version inspection shares the exact cleared bootstrap environment. It is
    // an identity observation, not an inference call or saved-policy mutation.
    let mut version = StdCommand::new(desired.get_program());
    version
        .arg("--version")
        .env_clear()
        .envs(
            desired
                .get_envs()
                .filter_map(|(key, value)| value.map(|value| (key, value))),
        )
        .current_dir(&fixture.workspace);
    report.independent.process_exited = false;
    report.independent.process_tree_empty = false;
    let captured_version = execute_contained(&executable, &version).await?;
    report.independent.process_exited = captured_version.code.is_some();
    report.independent.process_tree_empty = captured_version.tree_empty;
    if captured_version.code != Some(0)
        || !captured_version.execution_completed
        || !captured_version.output_complete
        || !captured_version.tree_empty
    {
        return Err("sandbox_backend_version_unavailable");
    }
    report.backend_cli_version = version_text(&captured_version.stdout);
    if report.backend_cli_version.is_none() {
        return Err("sandbox_backend_version_invalid");
    }
    report.independent.process_exited = false;
    report.independent.process_tree_empty = false;
    let captured = execute_contained(&executable, &desired).await?;
    report.stderr_bytes = captured.stderr_bytes;
    report.backend_exit_code = captured.code;
    report.independent.process_exited = captured.code.is_some();
    report.independent.execution_completed = captured.execution_completed;
    report.independent.process_tree_empty = captured.tree_empty;
    report.independent.output_complete = captured.output_complete;
    // The listener remains bound throughout execution. Completion of all fixed
    // child work precedes this snapshot; any successful connect was synchronous.
    report.independent.loopback_accepts = listener
        .accepts
        .load(Ordering::SeqCst)
        .saturating_sub(before);
    report.independent.listener_stopped = listener.stop().await;
    if captured.backend_reported_unavailable {
        return Err("sandbox_mxc_unavailable");
    }
    if !captured.execution_completed || captured.code != Some(0) {
        return Err("sandbox_backend_exit_failed");
    }
    if !captured.output_complete || !captured.tree_empty {
        return Err("sandbox_cleanup_unconfirmed");
    }
    serde_json::from_slice(&captured.stdout).map_err(|_| "sandbox_probe_frame_invalid")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolved_namespace_validation_never_accepts_a_share_or_device_path() {
        for path in [r"C:\scratch", r"\\?\C:\scratch"] {
            assert!(local_drive_path(Path::new(path)));
        }
        for path in [
            r"\\server\share\scratch",
            r"\\?\UNC\server\share\scratch",
            r"\\.\C:\scratch",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1\scratch",
            r"C:relative",
            r"C:\scratch\..\outside",
        ] {
            assert!(!local_drive_path(Path::new(path)));
        }
    }
    #[tokio::test]
    async fn cancellation_retains_the_owned_fixture_at_an_await_boundary() {
        let parent = tempfile::tempdir().unwrap();
        let parent_path = parent.path().to_path_buf();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let fixture = Fixture::create(&parent_path).unwrap();
            sender.send(fixture.root.clone()).unwrap();
            std::future::pending::<()>().await;
            drop(fixture);
        });
        let root = receiver.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(root.join("owner").is_file());
        assert!(root.join("denied/sentinel").is_file());
    }
    #[test]
    fn version_observation_accepts_only_bounded_content_free_text() {
        assert_eq!(
            version_text(b"codex-cli 0.164.0-alpha.11.1\r\n").as_deref(),
            Some("0.164.0-alpha.11.1")
        );
        assert!(version_text(b"codex-cli version\nextra").is_none());
        assert!(version_text(b"secret=example").is_none());
    }

    #[tokio::test]
    async fn output_capture_is_bounded_and_reports_truncation() {
        let input = vec![b'x'; OUTPUT_LIMIT * 2];
        let captured = read_output(input.as_slice()).await.unwrap();
        assert_eq!(captured.len(), OUTPUT_LIMIT + 1);
        let mut task = tokio::spawn(async move { Ok(captured) });
        assert!(join_output(&mut task).await.is_none());
    }
}
