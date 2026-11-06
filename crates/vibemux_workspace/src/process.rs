//! Bounded Git execution using the existing ADR 029 process-tree primitive.
//!
//! Windows releases the bundled trampoline only after Job Object assignment.
//! POSIX keeps an unreleased trampoline alive as the group leader and starts
//! Git in that group. The anchor prevents fast Git exits from invalidating
//! group verification, and keeps the group signalable until cleanup (including
//! on macOS, where an all-zombie group cannot be signalled). This owns ordinary
//! descendants; it is not a hostile-code sandbox.

use std::{process::ExitStatus, process::Stdio, time::Duration};

use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::{Child, Command},
    time::timeout,
};
use vibemux_platform::ProcessTree;

use crate::WorkspaceError;

const REAP_DEADLINE: Duration = Duration::from_secs(2);

// Fields drop in declaration order: terminate the tree before either child's
// drop can reap the anchor and release the identity of its POSIX group.
struct OwnedProcess {
    tree: ProcessTree,
    child: Child,
    #[cfg(unix)]
    anchor: Child,
}

impl OwnedProcess {
    async fn spawn(command: Command) -> Result<Self, WorkspaceError> {
        let (command, mut wrapper) = crate::blocking(move || {
            let wrapper = trampoline_command(&command)?;
            Ok((command, wrapper))
        })
        .await?;
        wrapper.stdin(Stdio::piped()).kill_on_drop(true);
        #[cfg(unix)]
        wrapper.stdout(Stdio::null()).stderr(Stdio::null());
        #[cfg(windows)]
        wrapper
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(0x0800_0000);
        ProcessTree::prepare_command(wrapper.as_std_mut());
        let mut anchor = wrapper.spawn().map_err(|_| WorkspaceError::Git)?;
        let Some(tree) = anchor.id().and_then(|id| ProcessTree::contain(id).ok()) else {
            // No go byte has been sent, so the wrapper cannot have spawned Git.
            let _ = anchor.start_kill();
            reap(&mut anchor).await?;
            return Err(WorkspaceError::ProcessCleanup);
        };
        #[cfg(unix)]
        {
            let mut tree = tree;
            let Some(group) = anchor.id().and_then(|id| i32::try_from(id).ok()) else {
                tree.terminate()
                    .map_err(|_| WorkspaceError::ProcessCleanup)?;
                reap(&mut anchor).await?;
                return Err(WorkspaceError::ProcessCleanup);
            };
            let mut command = command;
            command
                .process_group(group)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            match command.spawn() {
                Ok(child) => Ok(Self {
                    tree,
                    child,
                    anchor,
                }),
                Err(_) => {
                    tree.terminate()
                        .map_err(|_| WorkspaceError::ProcessCleanup)?;
                    reap(&mut anchor).await?;
                    Err(WorkspaceError::Git)
                }
            }
        }
        #[cfg(windows)]
        {
            drop(command);
            let mut process = Self {
                tree,
                child: anchor,
            };
            if release(&mut process.child).await.is_err() {
                process.terminate_and_reap().await?;
                return Err(WorkspaceError::ProcessCleanup);
            }
            Ok(process)
        }
    }

    async fn finish(&mut self) -> Result<ExitStatus, WorkspaceError> {
        // On POSIX this reaps Git, never the live group anchor. On Windows the
        // Job Object holds its own stable identity even after the wrapper exits.
        self.child.wait().await.map_err(|_| WorkspaceError::Git)?;
        self.terminate_and_reap().await
    }

    async fn terminate_and_reap(&mut self) -> Result<ExitStatus, WorkspaceError> {
        self.tree
            .terminate()
            .map_err(|_| WorkspaceError::ProcessCleanup)?;
        timeout(REAP_DEADLINE, async {
            #[cfg(unix)]
            let status = {
                let (status, anchor) = tokio::join!(self.child.wait(), self.anchor.wait());
                anchor.map_err(|_| WorkspaceError::ProcessCleanup)?;
                status
            };
            #[cfg(windows)]
            let status = self.child.wait().await;
            let status = status.map_err(|_| WorkspaceError::ProcessCleanup)?;
            #[cfg(windows)]
            while self
                .tree
                .active_process_count()
                .map_err(|_| WorkspaceError::ProcessCleanup)?
                != 0
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            Ok(status)
        })
        .await
        .map_err(|_| WorkspaceError::ProcessCleanup)?
    }
}

pub(super) async fn run_command(
    command: Command,
    deadline: Duration,
    maximum_output: usize,
) -> Result<Vec<u8>, WorkspaceError> {
    let mut process = OwnedProcess::spawn(command).await?;
    let Some((stdout, stderr)) = process.child.stdout.take().zip(process.child.stderr.take())
    else {
        process.terminate_and_reap().await?;
        return Err(WorkspaceError::Git);
    };
    let execution = async {
        let (output, _diagnostics, status) = tokio::try_join!(
            read_bounded(stdout, maximum_output),
            read_bounded(stderr, maximum_output),
            process.finish(),
        )?;
        if !status.success() {
            return Err(WorkspaceError::Git);
        }
        Ok(output)
    };
    let failure = match timeout(deadline, execution).await {
        Ok(Ok(output)) => return Ok(output),
        Ok(Err(error)) => error,
        Err(_) => WorkspaceError::Deadline,
    };
    process.terminate_and_reap().await?;
    Err(failure)
}

async fn reap(child: &mut Child) -> Result<ExitStatus, WorkspaceError> {
    timeout(REAP_DEADLINE, child.wait())
        .await
        .map_err(|_| WorkspaceError::ProcessCleanup)?
        .map_err(|_| WorkspaceError::ProcessCleanup)
}

async fn read_bounded(
    mut stream: impl AsyncRead + Unpin,
    maximum: usize,
) -> Result<Vec<u8>, WorkspaceError> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let count = stream
            .read(&mut buffer)
            .await
            .map_err(|_| WorkspaceError::Git)?;
        if count == 0 {
            return Ok(output);
        }
        if count > maximum.saturating_sub(output.len()) {
            return Err(WorkspaceError::OutputLimit);
        }
        output.extend_from_slice(&buffer[..count]);
    }
}

fn trampoline_command(command: &Command) -> Result<Command, WorkspaceError> {
    let current = std::env::current_exe().map_err(|_| WorkspaceError::ProcessCleanup)?;
    let directory = current.parent().ok_or(WorkspaceError::ProcessCleanup)?;
    // Cargo test executables live under target/<profile>/deps; the real bundled
    // helper is built beside the normal workspace binaries, not taken from PATH.
    let directory = if directory.file_name().is_some_and(|name| name == "deps") {
        directory.parent().ok_or(WorkspaceError::ProcessCleanup)?
    } else {
        directory
    };
    let trampoline = directory.join(format!(
        "vibemux_launch_trampoline{}",
        std::env::consts::EXE_SUFFIX
    ));
    if !crate::plain_metadata(&trampoline)
        .map_err(|_| WorkspaceError::ProcessCleanup)?
        .is_file()
    {
        return Err(WorkspaceError::ProcessCleanup);
    }
    let original = command.as_std();
    let mut wrapped = Command::new(trampoline);
    wrapped
        .arg(original.get_program())
        .args(original.get_args())
        .env_clear();
    if let Some(cwd) = original.get_current_dir() {
        wrapped.current_dir(cwd);
    }
    for (key, value) in original.get_envs() {
        if let Some(value) = value {
            wrapped.env(key, value);
        }
    }
    Ok(wrapped)
}

#[cfg(windows)]
async fn release(child: &mut Child) -> Result<(), WorkspaceError> {
    use tokio::io::AsyncWriteExt;
    let mut stdin = child.stdin.take().ok_or(WorkspaceError::ProcessCleanup)?;
    timeout(REAP_DEADLINE, async {
        stdin.write_all(&[vibemux_platform::LAUNCH_GO_BYTE]).await?;
        stdin.shutdown().await
    })
    .await
    .map_err(|_| WorkspaceError::ProcessCleanup)?
    .map_err(|_| WorkspaceError::ProcessCleanup)
}
