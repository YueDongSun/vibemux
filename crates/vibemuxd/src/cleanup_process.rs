//! Fixed contained Git cleanup I/O; admission and physical authority belong to its caller.

use std::{
    ffi::{OsStr, OsString},
    fmt,
    future::Future,
    path::{Component, Path, PathBuf, Prefix},
    time::Duration,
};

use tokio::{
    io::{AsyncRead, AsyncReadExt},
    sync::watch,
    time::{Instant, sleep, sleep_until, timeout},
};
use vibemux_harness::dispatch::{DispatchError, ProcessExit, launch_spec::LaunchSpec};

use crate::harness_dispatch::native_process::{
    LaunchPlan, NativeProcess, ReleaseFailureCause, SessionProcessExit,
};

const MAX_PATH_BYTES: usize = 4096;
const MAX_STDOUT_BYTES: usize = 64 * 1024;
const STDOUT_CHUNK_BYTES: usize = 8 * 1024;
const RUN_DEADLINE: Duration = Duration::from_secs(60);
const JOIN_DEADLINE: Duration = Duration::from_secs(2);
const ROOT_POLL_INTERVAL: Duration = Duration::from_millis(10);
const ENVIRONMENT_NAMES: [&str; 5] = ["SYSTEMROOT", "WINDIR", "TEMP", "TMP", "PATH"];

pub(crate) struct CleanupGitCommand {
    pub git: PathBuf,
    pub trampoline: PathBuf,
    pub repository: PathBuf,
    pub worktree: PathBuf,
}

impl fmt::Debug for CleanupGitCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CleanupGitCommand(<redacted>)")
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct CleanupProcessReceipt {
    pub exit: ProcessExit,
    pub release_attempted: bool,
    pub timed_out: bool,
    pub cancelled: bool,
    pub stdout_drained: bool,
    pub stderr_drained: bool,
    pub termination_succeeded: bool,
    pub root_reaped: bool,
    pub tree_empty: bool,
}

impl fmt::Debug for CleanupProcessReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CleanupProcessReceipt(<redacted>)")
    }
}

pub(crate) enum CleanupRunProblem<E> {
    Authorization(E),
    Release(DispatchError),
    TimedOut,
    Cancelled,
    OutputLimit,
    ReadFailed,
}

impl<E> fmt::Debug for CleanupRunProblem<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Authorization(_) => "Authorization(<redacted>)",
            Self::Release(_) => "Release(<redacted>)",
            Self::TimedOut => "TimedOut",
            Self::Cancelled => "Cancelled",
            Self::OutputLimit => "OutputLimit",
            Self::ReadFailed => "ReadFailed",
        })
    }
}

pub(crate) enum CleanupProcessOutcome<E> {
    /// This carries no quiescence evidence, including after partial spawn failure.
    SpawnFailed(DispatchError),
    Finished {
        receipt: CleanupProcessReceipt,
        problem: Option<CleanupRunProblem<E>>,
    },
}

impl<E> fmt::Debug for CleanupProcessOutcome<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::SpawnFailed(_) => "CleanupProcessOutcome::SpawnFailed(<redacted>)",
            Self::Finished { .. } => "CleanupProcessOutcome::Finished(<redacted>)",
        })
    }
}

#[derive(Clone, Copy)]
struct RunLimits {
    deadline: Duration,
    max_stdout_bytes: usize,
}

/// Always enters the held-spawn seam. The caller owns stop checks inside its
/// authorization callback; cancellation never abandons that callback's future.
pub(crate) async fn run<E, F, A>(
    command: CleanupGitCommand,
    authorize: F,
    cancel: watch::Receiver<bool>,
) -> CleanupProcessOutcome<E>
where
    F: FnOnce(u32) -> A,
    A: Future<Output = Result<(), E>>,
{
    let plan = match command_plan(&command) {
        Ok(plan) => plan,
        Err(error) => return CleanupProcessOutcome::SpawnFailed(error),
    };
    execute_plan(
        plan,
        authorize,
        cancel,
        RunLimits {
            deadline: RUN_DEADLINE,
            max_stdout_bytes: MAX_STDOUT_BYTES,
        },
    )
    .await
}

async fn execute_plan<E, F, A>(
    plan: LaunchPlan,
    authorize: F,
    mut cancel: watch::Receiver<bool>,
    limits: RunLimits,
) -> CleanupProcessOutcome<E>
where
    F: FnOnce(u32) -> A,
    A: Future<Output = Result<(), E>>,
{
    let held = match NativeProcess::spawn_held(&plan, cleanup_environment()).await {
        Ok(held) => held,
        Err(error) => return CleanupProcessOutcome::SpawnFailed(error),
    };
    // Do not select this future against cancellation or a timer: the callback
    // may be committing the durable arm and must reconcile its own outcome.
    let mut process = match held.release_after(authorize).await {
        Ok(process) => process,
        Err(failure) => {
            let release_attempted = failure.release_attempted();
            let (cause, joined) = failure.finish().await;
            let problem = match cause {
                ReleaseFailureCause::Authorization(error) => {
                    CleanupRunProblem::Authorization(error)
                }
                ReleaseFailureCause::Transport(error) => CleanupRunProblem::Release(error),
            };
            return CleanupProcessOutcome::Finished {
                receipt: receipt(
                    joined,
                    release_attempted,
                    joined.stdout_drained.unwrap_or(false),
                    false,
                    stop_requested(&cancel),
                ),
                problem: Some(problem),
            };
        }
    };
    process.close_stdin();
    let deadline = Instant::now() + limits.deadline;
    let (mut stdout_drained, mut problem) = drain_stdout::<_, E>(
        &mut process.stdout,
        &mut cancel,
        deadline,
        limits.max_stdout_bytes,
    )
    .await;
    // A root can close stdout and continue running. Keep cancellation/deadline
    // observation active until root exit rather than entering an uncancellable
    // natural-exit wait after stdout EOF.
    if problem.is_none() {
        problem = wait_for_root(&mut process, &mut cancel, deadline).await;
    }
    let timed_out = matches!(&problem, Some(CleanupRunProblem::TimedOut));
    let joined = process
        .finish_session(problem.is_some(), Instant::now() + JOIN_DEADLINE)
        .await;
    if !stdout_drained {
        stdout_drained = drain_after_termination(&mut process.stdout).await;
    }
    CleanupProcessOutcome::Finished {
        receipt: receipt(
            joined,
            true,
            stdout_drained,
            timed_out,
            stop_requested(&cancel),
        ),
        problem,
    }
}

fn receipt(
    joined: SessionProcessExit,
    release_attempted: bool,
    stdout_drained: bool,
    timed_out: bool,
    cancelled: bool,
) -> CleanupProcessReceipt {
    CleanupProcessReceipt {
        exit: joined.exit,
        release_attempted,
        timed_out,
        cancelled,
        stdout_drained,
        stderr_drained: joined.stderr_drained,
        termination_succeeded: joined.termination_succeeded,
        root_reaped: joined.root_reaped,
        tree_empty: joined.tree_empty,
    }
}

fn stop_requested(cancel: &watch::Receiver<bool>) -> bool {
    *cancel.borrow() || cancel.has_changed().is_err()
}

async fn drain_stdout<R: AsyncRead + Unpin, E>(
    stdout: &mut R,
    cancel: &mut watch::Receiver<bool>,
    deadline: Instant,
    limit: usize,
) -> (bool, Option<CleanupRunProblem<E>>) {
    let mut buffer = [0_u8; STDOUT_CHUNK_BYTES];
    let mut observed = 0_usize;
    loop {
        if stop_requested(cancel) {
            return (false, Some(CleanupRunProblem::Cancelled));
        }
        let remaining = limit.saturating_sub(observed);
        let capacity = buffer.len().min(remaining.saturating_add(1));
        tokio::select! {
            biased;
            _ = sleep_until(deadline) => return (false, Some(CleanupRunProblem::TimedOut)),
            _ = cancel.changed() => {},
            read = stdout.read(&mut buffer[..capacity]) => match read {
                Ok(0) => return (true, None),
                Ok(count) if count <= remaining => observed += count,
                Ok(_) => return (false, Some(CleanupRunProblem::OutputLimit)),
                Err(_) => return (false, Some(CleanupRunProblem::ReadFailed)),
            },
        }
    }
}

async fn wait_for_root<E>(
    process: &mut NativeProcess,
    cancel: &mut watch::Receiver<bool>,
    deadline: Instant,
) -> Option<CleanupRunProblem<E>> {
    loop {
        if stop_requested(cancel) {
            return Some(CleanupRunProblem::Cancelled);
        }
        match process.root_exited() {
            Ok(true) => return None,
            Err(_) => return Some(CleanupRunProblem::ReadFailed),
            Ok(false) => {}
        }
        tokio::select! {
            biased;
            _ = sleep_until(deadline) => return Some(CleanupRunProblem::TimedOut),
            _ = cancel.changed() => {},
            _ = sleep(ROOT_POLL_INTERVAL) => {},
        }
    }
}

async fn drain_after_termination(stdout: &mut (impl AsyncRead + Unpin)) -> bool {
    timeout(JOIN_DEADLINE, async {
        let mut buffer = [0_u8; STDOUT_CHUNK_BYTES];
        loop {
            match stdout.read(&mut buffer).await {
                Ok(0) => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
    })
    .await
    .unwrap_or(false)
}

fn command_plan(command: &CleanupGitCommand) -> Result<LaunchPlan, DispatchError> {
    native_path(&command.git, true)?;
    native_path(&command.trampoline, true)?;
    let repository = native_path(&command.repository, false)?;
    let worktree = native_path(&command.worktree, false)?;
    Ok(LaunchPlan {
        trampoline: command.trampoline.clone(),
        executable: command.git.clone(),
        working_directory: repository.into(),
        spec: LaunchSpec {
            executable: command.git.clone(),
            arguments: [
                "-c",
                "core.hooksPath=NUL",
                "-c",
                "core.autocrlf=false",
                "-c",
                "core.fsmonitor=false",
                "worktree",
                "remove",
                "--",
            ]
            .into_iter()
            .map(str::to_owned)
            .chain(std::iter::once(worktree))
            .collect(),
            environment_names: ENVIRONMENT_NAMES
                .into_iter()
                .chain(["GIT_CONFIG_NOSYSTEM", "GIT_CONFIG_GLOBAL"])
                .map(str::to_owned)
                .collect(),
        },
    })
}

fn native_path(path: &Path, executable: bool) -> Result<String, DispatchError> {
    let text = path.to_str().ok_or(DispatchError::InvalidRequest)?;
    let mut components = path.components();
    if text.len() > MAX_PATH_BYTES || text.chars().any(char::is_control)
        || !matches!(components.next(), Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
        || components.next() != Some(Component::RootDir)
        || components.any(|part| !matches!(part, Component::Normal(name)
            if name != OsStr::new(".") && name != OsStr::new("..") && !name.to_string_lossy().contains(':')))
        || (executable && !path.extension().and_then(OsStr::to_str)
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe")))
    {
        return Err(DispatchError::InvalidRequest);
    }
    Ok(text.strip_prefix(r"\\?\").unwrap_or(text).to_owned())
}

fn cleanup_environment() -> Vec<(String, OsString)> {
    let mut environment = ENVIRONMENT_NAMES
        .into_iter()
        .filter_map(|name| std::env::var_os(name).map(|value| (name.to_owned(), value)))
        .collect::<Vec<_>>();
    environment.extend([
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        ("GIT_CONFIG_GLOBAL".into(), "NUL".into()),
    ]);
    environment
}

#[cfg(all(test, feature = "test_helpers"))]
async fn run_plan<E, F, A>(
    plan: LaunchPlan,
    authorize: F,
    cancel: watch::Receiver<bool>,
    limits: RunLimits,
) -> CleanupProcessOutcome<E>
where
    F: FnOnce(u32) -> A,
    A: Future<Output = Result<(), E>>,
{
    execute_plan(plan, authorize, cancel, limits).await
}

#[cfg(all(test, feature = "test_helpers"))]
mod tests;
