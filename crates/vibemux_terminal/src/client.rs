//! Nonblocking GUI client for one owned helper process.
use crate::{
    protocol::{self, QUEUE_CAPACITY, Result, TerminalError, VERSION},
    wire::{Command, Notice, Open, Screen, Size, Status, command, notice},
};
use std::{
    path::{Path, PathBuf},
    process::{Command as OsCommand, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use vibemux_platform::ProcessTree;

pub struct TerminalClient {
    commands: Option<SyncSender<Command>>,
    notices: Receiver<Notice>,
    tree: Option<ProcessTree>,
    child: std::process::Child,
    reader: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
    overflow: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    started: Instant,
    stopping: Option<Instant>,
    pub screen: Option<Screen>,
    pub status: Status,
}
impl TerminalClient {
    pub fn start(host: &Path, open: Open, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self> {
        let first = Command {
            version: VERSION,
            operation: Some(command::Operation::Open(open)),
        };
        protocol::validate_command(&first)?;
        if !host.is_absolute() || !host.is_file() {
            return Err(TerminalError("terminal_host_missing"));
        }
        let mut launch = OsCommand::new(host);
        launch
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .current_dir(
                host.parent()
                    .ok_or(TerminalError("terminal_host_missing"))?,
            );
        ProcessTree::prepare_command(&mut launch);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            launch.creation_flags(0x08000000);
        }
        let mut child = launch
            .spawn()
            .map_err(|_| TerminalError("terminal_host_spawn_failed"))?;
        let mut tree = match ProcessTree::contain(child.id()) {
            Ok(tree) => tree,
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(TerminalError("terminal_host_containment_failed"));
            }
        };
        let pipes = (|| {
            let mut input = child
                .stdin
                .take()
                .ok_or(TerminalError("terminal_pipe_missing"))?;
            let output = child
                .stdout
                .take()
                .ok_or(TerminalError("terminal_pipe_missing"))?;
            protocol::write_message(&mut input, &first)?;
            Ok((input, output))
        })();
        let (mut input, mut output) = match pipes {
            Ok(pipes) => pipes,
            Err(error) => {
                let _ = tree.terminate();
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        let (command_tx, command_rx) = mpsc::sync_channel::<Command>(QUEUE_CAPACITY);
        let (notice_tx, notices) = mpsc::sync_channel(QUEUE_CAPACITY);
        let overflow = Arc::new(AtomicBool::new(false));
        let reader_overflow = Arc::clone(&overflow);
        let reader_wake = Arc::clone(&wake);
        let reader = std::thread::spawn(move || {
            loop {
                match protocol::read_message::<Notice>(&mut output) {
                    Ok(Some(message)) if protocol::validate_notice(&message).is_ok() => {
                        if notice_tx.try_send(message).is_err() {
                            reader_overflow.store(true, Ordering::Release);
                            break;
                        }
                        reader_wake();
                    }
                    Ok(None) => break,
                    _ => {
                        reader_overflow.store(true, Ordering::Release);
                        break;
                    }
                }
            }
            reader_wake();
        });
        let writer_overflow = Arc::clone(&overflow);
        let cancelled = Arc::new(AtomicBool::new(false));
        let writer_cancelled = Arc::clone(&cancelled);
        let writer = std::thread::spawn(move || {
            while let Ok(command) = command_rx.recv() {
                if writer_cancelled.load(Ordering::Acquire) {
                    break;
                }
                let close = matches!(command.operation, Some(command::Operation::Close(true)));
                if protocol::write_message(&mut input, &command).is_err() {
                    writer_overflow.store(true, Ordering::Release);
                    break;
                }
                if close {
                    break;
                }
            }
            // Dropping the pipe also releases the host's stdin reader.
        });
        Ok(Self {
            commands: Some(command_tx),
            notices,
            tree: Some(tree),
            child,
            reader: Some(reader),
            writer: Some(writer),
            overflow,
            cancelled,
            started: Instant::now(),
            stopping: None,
            screen: None,
            status: Status {
                phase: "starting".into(),
                ..Default::default()
            },
        })
    }
    pub fn poll(&mut self) {
        for notice in self.notices.try_iter() {
            match notice.payload {
                Some(notice::Payload::Screen(screen)) => self.screen = Some(screen),
                Some(notice::Payload::Status(status)) => self.status = status,
                _ => {}
            }
        }
        if self.overflow.load(Ordering::Acquire) {
            self.status.phase = "failed".into();
            self.status.code = "terminal_transport_failed".into();
            self.terminate();
        }
        if self.status.phase == "starting" && self.started.elapsed() > Duration::from_secs(45) {
            self.status.phase = "failed".into();
            self.status.code = "terminal_startup_timeout".into();
            self.terminate();
        }
        if self
            .stopping
            .is_some_and(|started| started.elapsed() > Duration::from_secs(3))
        {
            self.terminate();
            self.status.phase = "exited".into();
            self.stopping = None;
        }
        #[cfg(unix)]
        if self
            .tree
            .as_ref()
            .is_some_and(|tree| tree.leader_exited().unwrap_or(false))
        {
            self.terminate();
        }
        #[cfg(unix)]
        let may_reap = self.tree.is_none();
        #[cfg(windows)]
        let may_reap = true;
        if may_reap
            && self.child.try_wait().is_ok_and(|status| status.is_some())
            && !matches!(self.status.phase.as_str(), "failed" | "exited")
        {
            self.status.phase = "exited".into();
        }
    }
    pub fn send(&self, operation: command::Operation) -> Result<()> {
        let message = Command {
            version: VERSION,
            operation: Some(operation),
        };
        protocol::validate_command(&message)?;
        self.commands
            .as_ref()
            .ok_or(TerminalError("terminal_session_closed"))?
            .try_send(message)
            .map_err(|_| TerminalError("terminal_input_queue_full"))
    }
    pub fn stop(&mut self) {
        if !self.is_active() {
            return;
        }
        self.cancelled.store(true, Ordering::Release);
        // Close stdin instead of draining queued keystrokes after cancellation.
        self.commands.take();
        self.status.phase = "stopping".into();
        self.stopping = Some(Instant::now());
    }
    fn terminate(&mut self) {
        if let Some(mut tree) = self.tree.take() {
            let _ = tree.terminate();
        }
    }
    pub fn is_active(&self) -> bool {
        matches!(
            self.status.phase.as_str(),
            "starting" | "running" | "fixture" | "stopping"
        )
    }
}
impl Drop for TerminalClient {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.commands.take();
        self.terminate();
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(worker) = self.writer.take() {
            let _ = worker.join();
        }
        if let Some(worker) = self.reader.take() {
            let _ = worker.join();
        }
    }
}

/// Resolve only known executable names from the current operator's PATH.
pub fn find_executable(name: &str) -> Option<PathBuf> {
    if !matches!(name, "claude" | "git") {
        return None;
    }
    let file = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    if let Some(paths) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&paths) {
            let path = directory.join(&file);
            if path.is_absolute() && path.is_file() {
                return Some(path);
            }
        }
    }
    if name == "claude" {
        if let Some(directory) = home::home_dir() {
            let path = directory.join(".local").join("bin").join(file);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

pub fn open_request(root: &Path, fixture: bool, size: Size) -> Result<(PathBuf, Open)> {
    let own = std::env::current_exe().map_err(|_| TerminalError("terminal_host_missing"))?;
    let directory = own.parent().ok_or(TerminalError("terminal_host_missing"))?;
    let host = directory.join(if cfg!(windows) {
        "vibemux_terminal_host.exe"
    } else {
        "vibemux_terminal_host"
    });
    let executable = if fixture {
        directory.join(if cfg!(windows) {
            "vibemux_terminal_fixture.exe"
        } else {
            "vibemux_terminal_fixture"
        })
    } else {
        find_executable("claude").ok_or(TerminalError("claude_executable_missing"))?
    };
    let git_executable =
        find_executable("git").ok_or(TerminalError("terminal_git_executable_missing"))?;
    Ok((
        host,
        Open {
            project_root: root.to_string_lossy().into_owned(),
            executable: executable.to_string_lossy().into_owned(),
            git_executable: git_executable.to_string_lossy().into_owned(),
            size: Some(size),
            fixture,
        },
    ))
}
