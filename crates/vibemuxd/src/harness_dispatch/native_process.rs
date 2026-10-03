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
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command},
    task::JoinHandle,
    time::{Instant, timeout},
};
use vibemux_harness::dispatch::{
    DispatchError, ProcessExit,
    launch_spec::{LaunchSpec, SYSTEM_ENVIRONMENT_NAMES},
};
use vibemux_platform::{LAUNCH_GO_BYTE, ProcessTree};

/// Bound on reaping once the tree is terminated, on releasing the go byte,
/// and on the stderr drain after the process ended.
const REAP_DEADLINE: Duration = Duration::from_secs(2);
/// POSIX: poll interval while the trampoline may still exit on its own.
#[cfg(unix)]
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(10);
const STDERR_CHUNK_BYTES: usize = 8 * 1024;
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
    child: Child,
    tree: ProcessTree,
    /// `None` once closed.
    pub stdin: Option<ChildStdin>,
    pub stdout: ChildStdout,
    stderr_bytes: Arc<AtomicU64>,
    stderr_drain: JoinHandle<()>,
}

impl NativeProcess {
    /// Spawns and contains the trampoline, then releases the vendor.
    pub(crate) async fn spawn(
        plan: &LaunchPlan,
        environment: Vec<(String, OsString)>,
    ) -> Result<Self, DispatchError> {
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
        let Some(tree) = child
            .id()
            .and_then(|process_id| ProcessTree::contain(process_id).ok())
        else {
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
        let stderr_drain = tokio::spawn(drain_stderr(stderr, Arc::clone(&stderr_bytes)));
        let mut process = Self {
            child,
            tree,
            stdin: Some(stdin),
            stdout,
            stderr_bytes,
            stderr_drain,
        };
        if process.release().await.is_err() {
            process.finish(true, Instant::now()).await;
            return Err(DispatchError::PipeFailed);
        }
        Ok(process)
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
        (exit, self.stderr_bytes.load(Ordering::Relaxed))
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

async fn drain_stderr(mut stderr: ChildStderr, counter: Arc<AtomicU64>) {
    let mut buffer = vec![0_u8; STDERR_CHUNK_BYTES];
    loop {
        match stderr.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                counter.fetch_add(count as u64, Ordering::Relaxed);
            }
        }
    }
}
