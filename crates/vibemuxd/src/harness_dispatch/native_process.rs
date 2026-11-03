//! One contained vendor process behind the launch trampoline (ADR 029 §6).
//!
//! The daemon spawns the trampoline with the vendor's canonical path and the
//! profile argv, a cleared environment holding only the allowlisted names,
//! and the project root as working directory. It contains the trampoline,
//! then writes the go byte; only then does the trampoline start the vendor,
//! which inherits the pipes. The vendor therefore runs inside the tree from
//! its first instruction. stderr is drained and counted, never retained.

use std::{
    ffi::OsString,
    future::Future,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
    time::Duration,
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin, ChildStdout, Command},
    task::JoinHandle,
    time::{Instant, timeout},
};
use vibemux_harness::dispatch::{
    DispatchError, ProcessExit,
    launch_spec::{LaunchSpec, SYSTEM_ENVIRONMENT_NAMES},
};
use vibemux_platform::{LAUNCH_GO_BYTE, ProcessTree};

#[cfg(all(test, feature = "test_helpers", windows))]
mod held_tests;
#[cfg(test)]
mod stderr_tests;

/// Bound on reaping once the tree is terminated, on releasing the go byte,
/// and on the stderr drain after the process ended.
const REAP_DEADLINE: Duration = Duration::from_secs(2);
/// POSIX: poll interval while the trampoline may still exit on its own.
#[cfg(unix)]
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(10);
const STDERR_CHUNK_BYTES: usize = 8 * 1024;
const STDOUT_DRAIN_CHUNK_BYTES: usize = 8 * 1024;
/// Scan only this many stderr bytes for fixed diagnostic categories. Raw
/// stderr is neither retained nor logged; the byte count remains uncapped.
const STDERR_DIAGNOSTIC_SCAN_BYTES: usize = 16 * 1024;
const STDERR_ACCESS_DENIED: u32 = 1 << 0;
const STDERR_SANDBOX: u32 = 1 << 1;
const STDERR_RENDERER: u32 = 1 << 2;
const STDERR_CRASHPAD: u32 = 1 << 3;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// Everything needed to start one vendor process. `Debug` is redacted: the
/// paths and argv never reach a log.
pub(crate) struct LaunchPlan {
    pub trampoline: PathBuf,
    /// Canonical vendor executable verified by the config loader.
    pub executable: PathBuf,
    pub spec: LaunchSpec,
    pub working_directory: PathBuf,
}

impl std::fmt::Debug for LaunchPlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LaunchPlan")
            .field("spec", &self.spec)
            .finish_non_exhaustive()
    }
}

/// Reads the allowlisted variables from the daemon environment. A missing
/// system name is skipped; a missing route name fails the launch.
pub(crate) fn resolve_environment(
    names: &[String],
) -> Result<Vec<(String, OsString)>, DispatchError> {
    let mut environment = Vec::with_capacity(names.len());
    for name in names {
        match std::env::var_os(name) {
            Some(value) => environment.push((name.clone(), value)),
            None if SYSTEM_ENVIRONMENT_NAMES.contains(&name.as_str()) => {}
            None => return Err(DispatchError::EnvironmentUnavailable),
        }
    }
    Ok(environment)
}

pub(crate) struct NativeProcess {
    // Drop the exact tree before Tokio may release/reap the child identity.
    tree: ProcessTree,
    child: Child,
    /// `None` once closed.
    pub stdin: Option<ChildStdin>,
    pub stdout: ChildStdout,
    stderr_bytes: Arc<AtomicU64>,
    stderr_flags: Arc<AtomicU32>,
    stderr_drain: JoinHandle<bool>,
}

/// Allowlisted, content-free stderr observations. No vendor line, path,
/// prompt, or environment value is stored in this structure.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StderrDiagnostic {
    pub bytes: u64,
    pub access_denied: bool,
    pub sandbox: bool,
    pub renderer: bool,
    pub crashpad: bool,
}

/// Persistent owners release their durable fence only with positive cleanup
/// evidence. This is independent of turn completion and stdout delivery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SessionProcessExit {
    pub exit: ProcessExit,
    pub termination_succeeded: bool,
    pub root_reaped: bool,
    pub tree_empty: bool,
    pub stderr_drained: bool,
    /// `finish_session` leaves stdout with its caller; failed-release joined
    /// cleanup drains it internally and records its actual EOF observation.
    pub stdout_drained: Option<bool>,
    pub cleanup_verified: bool,
    pub stderr_bytes: u64,
}

/// Contained but unreleased. No pipe is available to a caller in this state.
pub(crate) struct HeldNativeProcess {
    process_id: u32,
    process: NativeProcess,
}

pub(crate) enum ReleaseFailureCause<E> {
    Authorization(E),
    Transport(DispatchError),
}

/// A failed release keeps ownership, but exposes no path back to release/pipes.
pub(crate) struct ReleaseFailure<E> {
    cause: ReleaseFailureCause<E>,
    release_attempted: bool,
    process: NativeProcess,
}

impl<E> std::fmt::Debug for ReleaseFailure<E> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match &self.cause {
            ReleaseFailureCause::Authorization(_) => "authorization",
            ReleaseFailureCause::Transport(_) => "transport",
        };
        formatter
            .debug_struct("ReleaseFailure")
            .field("kind", &kind)
            .field("release_attempted", &self.release_attempted)
            .finish_non_exhaustive()
    }
}

impl HeldNativeProcess {
    /// The PID is held for this callback; it is not durable process identity.
    /// A cleanup caller must record stronger identity and release authority
    /// before returning Ok. Dropping this future supplies no joined receipt.
    pub(crate) async fn release_after<E, F, A>(
        mut self,
        authorize: F,
    ) -> Result<NativeProcess, ReleaseFailure<E>>
    where
        F: FnOnce(u32) -> A,
        A: Future<Output = Result<(), E>>,
    {
        if let Err(error) = authorize(self.process_id).await {
            return Err(ReleaseFailure {
                cause: ReleaseFailureCause::Authorization(error),
                release_attempted: false,
                process: self.process,
            });
        }
        // A failed write/flush can follow delivery of go. Never call that
        // outcome "not started" or offer this owner another release attempt.
        let release_attempted = true;
        if let Err(error) = self.process.release().await {
            return Err(ReleaseFailure {
                cause: ReleaseFailureCause::Transport(error),
                release_attempted,
                process: self.process,
            });
        }
        Ok(self.process)
    }
}

impl<E> ReleaseFailure<E> {
    pub(crate) const fn release_attempted(&self) -> bool {
        self.release_attempted
    }

    /// Explicitly joins the failed owner. Authorization denial closes stdin so
    /// the unreleased trampoline can exit on EOF; uncertain transport kills its
    /// tree. The receipt also requires stdout EOF, unlike finish_session alone.
    pub(crate) async fn finish(mut self) -> (ReleaseFailureCause<E>, SessionProcessExit) {
        let mut receipt = self
            .process
            .finish_session(self.release_attempted(), Instant::now() + REAP_DEADLINE)
            .await;
        let stdout_drained = timeout(REAP_DEADLINE, async {
            let mut buffer = [0_u8; STDOUT_DRAIN_CHUNK_BYTES];
            loop {
                match self.process.stdout.read(&mut buffer).await {
                    Ok(0) => break true,
                    Ok(_) => {}
                    Err(_) => break false,
                }
            }
        })
        .await
        .unwrap_or(false);
        receipt.cleanup_verified &= stdout_drained;
        receipt.stdout_drained = Some(stdout_drained);
        (self.cause, receipt)
    }
}

impl NativeProcess {
    /// Spawns and contains the trampoline, then releases the vendor.
    pub(crate) async fn spawn(
        plan: &LaunchPlan,
        environment: Vec<(String, OsString)>,
    ) -> Result<Self, DispatchError> {
        // Existing callers have already crossed their admission boundary.
        match Self::spawn_held(plan, environment)
            .await?
            .release_after(|_| async { Ok::<(), DispatchError>(()) })
            .await
        {
            Ok(process) => Ok(process),
            Err(failure) => {
                let (cause, _cleanup) = failure.finish().await;
                Err(match cause {
                    ReleaseFailureCause::Authorization(error)
                    | ReleaseFailureCause::Transport(error) => error,
                })
            }
        }
    }

    /// Spawns/contains the trampoline and pipes without releasing its program.
    pub(crate) async fn spawn_held(
        plan: &LaunchPlan,
        environment: Vec<(String, OsString)>,
    ) -> Result<HeldNativeProcess, DispatchError> {
        let mut command = Command::new(&plan.trampoline);
        command
            .arg(&plan.executable)
            .args(&plan.spec.arguments)
            .env_clear()
            .envs(environment)
            .current_dir(&plan.working_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
        ProcessTree::prepare_command(command.as_std_mut());
        let mut child = command.spawn().map_err(|_| DispatchError::SpawnFailed)?;
        let Some((process_id, tree)) = child.id().and_then(|process_id| {
            ProcessTree::contain(process_id)
                .ok()
                .map(|tree| (process_id, tree))
        }) else {
            // The trampoline has not been released, so it started nothing.
            let _ = child.start_kill();
            let _ = timeout(REAP_DEADLINE, child.wait()).await;
            return Err(DispatchError::ContainmentFailed);
        };
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let mut tree = tree;
            let _ = tree.terminate();
            let _ = timeout(REAP_DEADLINE, child.wait()).await;
            return Err(DispatchError::PipeFailed);
        };
        let stderr_bytes = Arc::new(AtomicU64::new(0));
        let stderr_flags = Arc::new(AtomicU32::new(0));
        let stderr_drain = tokio::spawn(drain_stderr(
            stderr,
            Arc::clone(&stderr_bytes),
            Arc::clone(&stderr_flags),
        ));
        let process = Self {
            child,
            tree,
            stdin: Some(stdin),
            stdout,
            stderr_bytes,
            stderr_flags,
            stderr_drain,
        };
        Ok(HeldNativeProcess {
            process_id,
            process,
        })
    }

    async fn release(&mut self) -> Result<(), DispatchError> {
        let stdin = self.stdin.as_mut().ok_or(DispatchError::PipeFailed)?;
        let written = timeout(REAP_DEADLINE, async {
            stdin.write_all(&[LAUNCH_GO_BYTE]).await?;
            stdin.flush().await
        })
        .await;
        match written {
            Ok(Ok(())) => Ok(()),
            _ => Err(DispatchError::PipeFailed),
        }
    }

    pub(crate) fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// A persistent owner observes root death even when a descendant still
    /// holds stdout open. Windows Job handles preserve tree identity after root
    /// reaping; POSIX observes without reaping until group termination.
    pub(crate) fn root_exited(&mut self) -> Result<bool, DispatchError> {
        #[cfg(windows)]
        {
            self.child
                .try_wait()
                .map(|status| status.is_some())
                .map_err(|_| DispatchError::ProcessFailed)
        }
        #[cfg(unix)]
        {
            self.tree
                .leader_exited()
                .map_err(|_| DispatchError::ProcessFailed)
        }
    }

    pub(crate) fn stderr_diagnostic(&self) -> StderrDiagnostic {
        let flags = self.stderr_flags.load(Ordering::Relaxed);
        StderrDiagnostic {
            bytes: self.stderr_bytes.load(Ordering::Relaxed),
            access_denied: flags & STDERR_ACCESS_DENIED != 0,
            sandbox: flags & STDERR_SANDBOX != 0,
            renderer: flags & STDERR_RENDERER != 0,
            crashpad: flags & STDERR_CRASHPAD != 0,
        }
    }

    /// Ends the process and reports how. Unless `force` is set, the
    /// trampoline may exit on its own until `exit_deadline`; after that, or
    /// with `force`, the tree is killed. Either way the tree is terminated
    /// before the trampoline is reaped, which removes any member it left
    /// behind. Returns the exit and the stderr byte count.
    pub(crate) async fn finish(
        &mut self,
        force: bool,
        exit_deadline: Instant,
    ) -> (ProcessExit, u64) {
        self.close_stdin();
        let exited_naturally = !force && self.await_natural_exit(exit_deadline).await;
        let _ = self.tree.terminate();
        let exit_code = self.reap().await;
        if timeout(REAP_DEADLINE, &mut self.stderr_drain)
            .await
            .is_err()
        {
            self.stderr_drain.abort();
        }
        let exit = ProcessExit {
            exit_code,
            forced_termination: !exited_naturally,
        };
        (exit, self.stderr_diagnostic().bytes)
    }

    /// Closes exactly this contained owner. The old one-shot `finish` API keeps
    /// its existing semantics; persistent generation release needs this stronger
    /// receipt. Unsupported tree-emptiness evidence fails closed.
    pub(crate) async fn finish_session(
        &mut self,
        force: bool,
        exit_deadline: Instant,
    ) -> SessionProcessExit {
        self.close_stdin();
        let natural = !force && self.await_natural_exit(exit_deadline).await;
        let terminated = self.tree.terminate().is_ok();
        // POSIX must not reap a leader after a failed group signal: keeping its
        // PID reserved avoids signalling a recycled group during owner Drop.
        #[cfg(unix)]
        let may_reap = terminated;
        #[cfg(windows)]
        let may_reap = true;
        let status = if may_reap {
            match timeout(REAP_DEADLINE, self.child.wait()).await {
                Ok(Ok(status)) => Some(status),
                _ => {
                    let _ = self.child.start_kill();
                    timeout(REAP_DEADLINE, self.child.wait())
                        .await
                        .ok()
                        .and_then(Result::ok)
                }
            }
        } else {
            None
        };
        let root_reaped = status.is_some();
        #[cfg(windows)]
        let tree_empty = {
            let deadline = Instant::now() + REAP_DEADLINE;
            loop {
                match self.tree.active_process_count() {
                    Ok(0) => break true,
                    Ok(_) if Instant::now() < deadline => {
                        tokio::time::sleep(Duration::from_millis(10)).await
                    }
                    _ => break false,
                }
            }
        };
        // The current POSIX owner exposes leader exit, but no bounded proof of
        // group emptiness. Do not upgrade a successful signal into that proof.
        #[cfg(unix)]
        let tree_empty = false;
        let stderr_drained = join_stderr_drain(&mut self.stderr_drain).await;
        SessionProcessExit {
            exit: ProcessExit {
                exit_code: status.and_then(|status| status.code()),
                forced_termination: !natural,
            },
            root_reaped,
            tree_empty,
            termination_succeeded: terminated,
            stderr_drained,
            stdout_drained: None,
            cleanup_verified: terminated && root_reaped && tree_empty && stderr_drained,
            stderr_bytes: self.stderr_diagnostic().bytes,
        }
    }

    /// Windows: the job holds its members by handle, so the trampoline may
    /// be reaped before the job is terminated; the child keeps its status
    /// for [`NativeProcess::reap`].
    #[cfg(windows)]
    async fn await_natural_exit(&mut self, exit_deadline: Instant) -> bool {
        matches!(
            tokio::time::timeout_at(exit_deadline, self.child.wait()).await,
            Ok(Ok(_))
        )
    }

    /// POSIX: observes the exit without reaping, so the group id stays
    /// reserved until the tree has been terminated.
    #[cfg(unix)]
    async fn await_natural_exit(&mut self, exit_deadline: Instant) -> bool {
        loop {
            match self.tree.leader_exited() {
                Ok(true) => return true,
                Ok(false) if Instant::now() < exit_deadline => {
                    tokio::time::sleep(EXIT_POLL_INTERVAL).await;
                }
                _ => return false,
            }
        }
    }

    async fn reap(&mut self) -> Option<i32> {
        if let Ok(Ok(status)) = timeout(REAP_DEADLINE, self.child.wait()).await {
            return status.code();
        }
        let _ = self.child.start_kill();
        match timeout(REAP_DEADLINE, self.child.wait()).await {
            Ok(Ok(status)) => status.code(),
            _ => None,
        }
    }
}

impl Drop for NativeProcess {
    /// Abandoned attempt (task aborted at shutdown): the tree's own drop
    /// kills every member and `kill_on_drop` covers the trampoline.
    fn drop(&mut self) {
        self.stderr_drain.abort();
    }
}

async fn join_stderr_drain(drain: &mut JoinHandle<bool>) -> bool {
    match timeout(REAP_DEADLINE, &mut *drain).await {
        Ok(Ok(eof)) => eof,
        Ok(Err(_)) => false,
        Err(_) => {
            drain.abort();
            let _ = drain.await;
            false
        }
    }
}

async fn drain_stderr(
    mut stderr: impl tokio::io::AsyncRead + Unpin,
    counter: Arc<AtomicU64>,
    flags: Arc<AtomicU32>,
) -> bool {
    let mut buffer = vec![0_u8; STDERR_CHUNK_BYTES];
    let mut scanned = 0_usize;
    let mut tail = String::new();
    loop {
        match stderr.read(&mut buffer).await {
            Ok(0) => return true,
            Err(_) => return false,
            Ok(count) => {
                counter.fetch_add(count as u64, Ordering::Relaxed);
                if scanned < STDERR_DIAGNOSTIC_SCAN_BYTES {
                    let bounded = count.min(STDERR_DIAGNOSTIC_SCAN_BYTES - scanned);
                    scanned += bounded;
                    let mut text = tail.clone();
                    text.push_str(
                        &String::from_utf8_lossy(&buffer[..bounded]).to_ascii_lowercase(),
                    );
                    let mut found = 0_u32;
                    if text.contains("access denied") || text.contains("access is denied") {
                        found |= STDERR_ACCESS_DENIED;
                    }
                    if text.contains("sandbox") {
                        found |= STDERR_SANDBOX;
                    }
                    if text.contains("renderer") {
                        found |= STDERR_RENDERER;
                    }
                    if text.contains("crashpad") {
                        found |= STDERR_CRASHPAD;
                    }
                    flags.fetch_or(found, Ordering::Relaxed);
                    tail = text
                        .chars()
                        .rev()
                        .take(32)
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect();
                }
            }
        }
    }
}

/// Test-only lifecycle probe driven through the same trampoline, cleared
/// environment, and ProcessTree as a trusted workflow verifier.
#[cfg(feature = "test_helpers")]
pub struct BrowserProbePlan {
    pub trampoline: PathBuf,
    pub node: PathBuf,
    pub script: PathBuf,
    pub browser: PathBuf,
    pub scratch: PathBuf,
    pub environment_names: Vec<String>,
    pub hold_after_attach: bool,
}

#[cfg(feature = "test_helpers")]
impl std::fmt::Debug for BrowserProbePlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BrowserProbePlan")
            .field("hold_after_attach", &self.hold_after_attach)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "test_helpers")]
#[derive(Debug)]
pub struct BrowserProbeResult {
    pub phases: Vec<String>,
    pub failed_at: Option<String>,
    pub error_class: Option<String>,
    pub browser_version: Option<String>,
    pub cdp_error_code: Option<i32>,
    pub browser_cleanup_observed: bool,
    pub browser_pid: Option<u32>,
    pub renderer_pids: Vec<u32>,
    pub browser_stderr_bytes: Option<u64>,
    pub browser_stderr_sandbox: bool,
    pub browser_stderr_access_denied: bool,
    pub browser_stderr_renderer: bool,
    pub browser_stderr_win32_access_denied: bool,
    pub browser_stderr_process_create: bool,
    pub browser_stderr_job_object: bool,
    pub native_stderr: StderrDiagnostic,
    pub exit: ProcessExit,
    pub active_members_before_cancel: Option<u32>,
    pub active_members_after_finish: Option<u32>,
    pub timed_out: bool,
    pub output_truncated: bool,
}

#[cfg(feature = "test_helpers")]
pub async fn run_browser_lifecycle_probe(
    plan: BrowserProbePlan,
) -> Result<BrowserProbeResult, DispatchError> {
    use serde_json::Value;

    const MAX_PROBE_OUTPUT_BYTES: usize = 64 * 1024;
    const PROBE_DEADLINE: Duration = Duration::from_secs(70);
    // Node and Chromium must receive ordinary Windows paths. The trusted
    // workflow verifier uses this same conversion before crossing argv.
    let script = super::child_path_text(&plan.script).ok_or(DispatchError::InvalidRequest)?;
    let browser = super::child_path_text(&plan.browser).ok_or(DispatchError::InvalidRequest)?;
    let scratch = super::child_path_text(&plan.scratch).ok_or(DispatchError::InvalidRequest)?;
    let node = super::child_path_text(&plan.node).ok_or(DispatchError::InvalidRequest)?;
    let mut arguments = vec![
        script,
        "--browser".to_string(),
        browser,
        "--temp_root".to_string(),
        scratch.clone(),
    ];
    if plan.hold_after_attach {
        arguments.push("--hold_after_attach".to_string());
    }
    let mut environment = resolve_environment(&plan.environment_names)?;
    environment.retain(|(name, _)| name != "TEMP" && name != "TMP");
    environment.push(("TEMP".to_string(), plan.scratch.as_os_str().to_os_string()));
    environment.push(("TMP".to_string(), plan.scratch.as_os_str().to_os_string()));
    let launch = LaunchPlan {
        trampoline: plan.trampoline,
        executable: PathBuf::from(node),
        spec: LaunchSpec {
            executable: plan.node,
            arguments,
            environment_names: plan.environment_names,
        },
        working_directory: PathBuf::from(scratch),
    };
    let mut process = NativeProcess::spawn(&launch, environment).await?;
    process.close_stdin();
    let mut result = empty_browser_probe_result();
    let deadline = Instant::now() + PROBE_DEADLINE;
    let mut pending = Vec::<u8>::new();
    let mut received = 0_usize;
    let mut eof = false;
    let mut held = false;
    let mut buffer = [0_u8; 1024];
    while !eof && !held {
        let read = timeout(
            deadline.saturating_duration_since(Instant::now()),
            process.stdout.read(&mut buffer),
        )
        .await;
        match read {
            Ok(Ok(0)) => eof = true,
            Ok(Ok(count)) => {
                received = received.saturating_add(count);
                if received > MAX_PROBE_OUTPUT_BYTES {
                    result.output_truncated = true;
                    break;
                }
                pending.extend_from_slice(&buffer[..count]);
                while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                    let line = pending.drain(..=end).collect::<Vec<u8>>();
                    if let Ok(value) = serde_json::from_slice::<Value>(&line) {
                        observe_probe_line(&value, &mut result);
                        held = plan.hold_after_attach
                            && value.get("phase").and_then(Value::as_str)
                                == Some("held_for_cancel");
                        #[cfg(windows)]
                        if held {
                            result.active_members_before_cancel =
                                process.tree.active_process_count().ok();
                        }
                    }
                }
            }
            Ok(Err(_)) | Err(_) => {
                result.timed_out = true;
                break;
            }
        }
    }
    let force = held || !eof || result.output_truncated;
    let (exit, _) = process.finish(force, Instant::now() + REAP_DEADLINE).await;
    result.exit = exit;
    result.native_stderr = process.stderr_diagnostic();
    #[cfg(windows)]
    {
        let query_deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match process.tree.active_process_count() {
                Ok(count) if count == 0 || Instant::now() >= query_deadline => {
                    result.active_members_after_finish = Some(count);
                    break;
                }
                Ok(_) => tokio::time::sleep(Duration::from_millis(20)).await,
                Err(_) => break,
            }
        }
    }
    Ok(result)
}

#[cfg(feature = "test_helpers")]
pub fn observe_browser_probe_output(output: &[u8]) -> BrowserProbeResult {
    let mut result = empty_browser_probe_result();
    let mut pending = Vec::<u8>::new();
    for chunk in output.chunks(1024) {
        if result.output_truncated {
            break;
        }
        if pending.len().saturating_add(chunk.len()) > 64 * 1024 {
            result.output_truncated = true;
            break;
        }
        pending.extend_from_slice(chunk);
        while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
            let line = pending.drain(..=end).collect::<Vec<_>>();
            if let Ok(value) = serde_json::from_slice(&line) {
                observe_probe_line(&value, &mut result);
            }
        }
    }
    result
}

#[cfg(feature = "test_helpers")]
fn empty_browser_probe_result() -> BrowserProbeResult {
    BrowserProbeResult {
        phases: Vec::new(),
        failed_at: None,
        error_class: None,
        browser_version: None,
        cdp_error_code: None,
        browser_cleanup_observed: false,
        browser_pid: None,
        renderer_pids: Vec::new(),
        browser_stderr_bytes: None,
        browser_stderr_sandbox: false,
        browser_stderr_access_denied: false,
        browser_stderr_renderer: false,
        browser_stderr_win32_access_denied: false,
        browser_stderr_process_create: false,
        browser_stderr_job_object: false,
        native_stderr: StderrDiagnostic::default(),
        exit: ProcessExit {
            exit_code: None,
            forced_termination: true,
        },
        active_members_before_cancel: None,
        active_members_after_finish: None,
        timed_out: false,
        output_truncated: false,
    }
}

#[cfg(feature = "test_helpers")]
fn observe_probe_line(value: &serde_json::Value, result: &mut BrowserProbeResult) {
    let Some(phase) = value.get("phase").and_then(serde_json::Value::as_str) else {
        return;
    };
    let allowed = [
        "browser_spawned",
        "browser_ready",
        "target_created",
        "target_attached",
        "runtime_enabled",
        "runtime_evaluated",
        "process_observation",
        "held_for_cancel",
        "page_enabled",
        "failure",
        "cleanup",
    ];
    if !allowed.contains(&phase) {
        return;
    }
    result.phases.push(phase.to_string());
    if phase == "browser_spawned" {
        result.browser_pid = value
            .get("browser_pid")
            .and_then(serde_json::Value::as_u64)
            .and_then(|pid| u32::try_from(pid).ok());
    }
    if phase == "browser_ready" {
        result.browser_version = value
            .get("browser_version")
            .and_then(serde_json::Value::as_str)
            .filter(|version| {
                !version.is_empty()
                    && version.len() <= 32
                    && version
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || byte == b'.')
            })
            .map(str::to_string);
    }
    if phase == "process_observation"
        && let Some(processes) = value.get("processes").and_then(serde_json::Value::as_array)
    {
        result.renderer_pids = processes
            .iter()
            .filter(|entry| {
                entry.get("type").and_then(serde_json::Value::as_str) == Some("renderer")
            })
            .filter_map(|entry| entry.get("pid").and_then(serde_json::Value::as_u64))
            .filter_map(|pid| u32::try_from(pid).ok())
            .take(32)
            .collect();
    }
    if phase == "failure" {
        const FAILED_AT: [&str; 7] = [
            "arguments",
            "browser_ready",
            "target_create",
            "target_attach",
            "runtime_enable",
            "runtime_evaluate",
            "page_enable",
        ];
        const ERROR_CLASSES: [&str; 8] = [
            "invalid_arguments",
            "timeout",
            "websocket",
            "access_denied",
            "spawn_error",
            "cdp_error",
            "aborted",
            "other",
        ];
        result.failed_at = value
            .get("failed_at")
            .and_then(serde_json::Value::as_str)
            .filter(|stage| FAILED_AT.contains(stage))
            .map(str::to_string);
        result.error_class = value
            .get("error_class")
            .and_then(serde_json::Value::as_str)
            .filter(|class| ERROR_CLASSES.contains(class))
            .map(str::to_string);
        result.cdp_error_code = value
            .get("cdp_error_code")
            .and_then(serde_json::Value::as_i64)
            .and_then(|code| i32::try_from(code).ok());
    }
    if phase == "cleanup" {
        result.browser_cleanup_observed = value
            .get("browser_exit_observed")
            .and_then(serde_json::Value::as_bool)
            == Some(true);
    }
    if (phase == "failure" || phase == "cleanup")
        && let Some(stderr) = value.get("stderr")
    {
        result.browser_stderr_bytes = stderr.get("bytes").and_then(serde_json::Value::as_u64);
        result.browser_stderr_sandbox |=
            stderr.get("sandbox").and_then(serde_json::Value::as_bool) == Some(true);
        result.browser_stderr_access_denied |= stderr
            .get("access_denied")
            .and_then(serde_json::Value::as_bool)
            == Some(true);
        result.browser_stderr_renderer |=
            stderr.get("renderer").and_then(serde_json::Value::as_bool) == Some(true);
        result.browser_stderr_win32_access_denied |= stderr
            .get("win32_access_denied")
            .and_then(serde_json::Value::as_bool)
            == Some(true);
        result.browser_stderr_process_create |= stderr
            .get("process_create")
            .and_then(serde_json::Value::as_bool)
            == Some(true);
        result.browser_stderr_job_object |= stderr
            .get("job_object")
            .and_then(serde_json::Value::as_bool)
            == Some(true);
    }
}
