//! Exact child ownership: contain before input, terminate the tree before reap.

use crate::{
    config::{CHILD_EXIT_POLL_INTERVAL, HostConfig, MAX_INPUT_BYTES, SHUTDOWN_DEADLINE},
    control_reader::{ReaderReply, ReaderRequest},
    framing::{LineReader, write_json},
    tools::ReadOperation,
};
use serde_json::Value;
use std::{
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant as StdInstant},
};
use tokio::{
    io::BufReader,
    process::{Child, ChildStdin, ChildStdout, Command},
    runtime::Handle,
    time::{Instant, timeout_at},
};
use vibemux_platform::ProcessTree;

const DROP_POLL_INTERVAL: Duration = Duration::from_millis(5);
const LAUNCH_POLL_INTERVAL: Duration = Duration::from_millis(5);
#[cfg(windows)]
const WINDOWS_CREATION_FLAGS: u32 = 0x0800_0200;

#[derive(Clone)]
pub(crate) enum WorkerLaunch {
    Native,
    #[cfg(feature = "test_helpers")]
    Fixture {
        executable: std::path::PathBuf,
        mode: crate::fixture_support::FixtureMode,
        control: crate::fixture_support::LaunchControl,
    },
}

enum ShutdownState {
    Live,
    Waiting { deadline: Instant, terminated: bool },
    Complete(Result<(), &'static str>),
}

pub(crate) struct ReaderProcess {
    tree: Option<ProcessTree>,
    pub(crate) child: Child,
    input: Option<ChildStdin>,
    output: Option<LineReader<BufReader<ChildStdout>>>,
    generation: u64,
    sequence: u64,
    startup_error: Option<&'static str>,
    shutdown: ShutdownState,
    #[cfg(feature = "test_helpers")]
    fail_cleanup: bool,
    #[cfg(feature = "test_helpers")]
    before_reap: Option<crate::fixture_support::LaunchGate>,
    #[cfg(feature = "test_helpers")]
    pub(crate) terminal_observed: Arc<AtomicBool>,
}

impl ReaderProcess {
    fn spawn(generation: u64, launch: &WorkerLaunch) -> Result<Self, &'static str> {
        let (executable, argument) = match launch {
            WorkerLaunch::Native => (
                std::env::current_exe().map_err(|_| "reader_spawn_failed")?,
                "--ipc_reader",
            ),
            #[cfg(feature = "test_helpers")]
            WorkerLaunch::Fixture {
                executable, mode, ..
            } => (executable.clone(), mode.argument()),
        };
        let mut command = Command::new(executable);
        command
            .arg(argument)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(feature = "test_helpers")]
        if matches!(
            launch,
            WorkerLaunch::Fixture {
                mode: crate::fixture_support::FixtureMode::NoOutputPipe,
                ..
            }
        ) {
            command.stdout(Stdio::null());
        }
        for key in ["SystemRoot", "WINDIR", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        #[cfg(windows)]
        command.creation_flags(WINDOWS_CREATION_FLAGS);
        ProcessTree::prepare_command(command.as_std_mut());
        #[cfg(feature = "test_helpers")]
        if let WorkerLaunch::Fixture { control, .. } = launch
            && let Some(gate) = &control.before_spawn
        {
            gate.wait();
        }
        let mut child = command.spawn().map_err(|_| "reader_spawn_failed")?;
        let process_id = child.id();
        #[cfg(feature = "test_helpers")]
        let process_id = if matches!(
            launch,
            WorkerLaunch::Fixture {
                mode: crate::fixture_support::FixtureMode::RejectContainment,
                ..
            }
        ) {
            Some(std::process::id()) // The platform API must refuse containing its owner.
        } else {
            process_id
        };
        let tree = process_id.and_then(|id| ProcessTree::contain(id).ok());
        let input = child.stdin.take();
        let output = child
            .stdout
            .take()
            .map(|output| LineReader::new(BufReader::new(output), MAX_INPUT_BYTES));
        let startup_error = if tree.is_none() {
            Some("reader_containment_failed")
        } else if input.is_none() || output.is_none() {
            Some("reader_pipe_unavailable")
        } else {
            None
        };
        // Synchronous ownership transfer: cancellation cannot strand a child in
        // an awaited partial-spawn helper. Even failed containment is retained.
        let worker = Self {
            tree,
            child,
            input,
            output,
            generation,
            sequence: 0,
            startup_error,
            shutdown: ShutdownState::Live,
            #[cfg(feature = "test_helpers")]
            fail_cleanup: matches!(launch, WorkerLaunch::Fixture { control, .. } if control.fail_cleanup),
            #[cfg(feature = "test_helpers")]
            before_reap: match launch {
                WorkerLaunch::Fixture { control, .. } => control.before_reap.clone(),
                WorkerLaunch::Native => None,
            },
            #[cfg(feature = "test_helpers")]
            terminal_observed: match launch {
                WorkerLaunch::Fixture { control, .. } => {
                    std::sync::Arc::clone(&control.terminal_observed)
                }
                WorkerLaunch::Native => Default::default(),
            },
        };
        #[cfg(feature = "test_helpers")]
        if let WorkerLaunch::Fixture { control, .. } = launch
            && let Some(gate) = &control.after_containment
        {
            gate.wait();
        }
        Ok(worker)
    }

    pub(crate) async fn read(
        &mut self,
        config: &HostConfig,
        operation: ReadOperation,
    ) -> Result<Value, &'static str> {
        if let Some(code) = self.startup_error {
            return Err(code);
        }
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or("reader_sequence_exhausted")?;
        let request = ReaderRequest {
            generation: self.generation,
            sequence: self.sequence,
            descriptor: config.descriptor.clone(),
            operation,
        };
        let request = serde_json::to_value(request).map_err(|_| "reader_request_invalid")?;
        if request.to_string().len() > MAX_INPUT_BYTES {
            return Err("reader_request_invalid");
        }
        write_json(self.input.as_mut().ok_or("reader_unavailable")?, &request).await?;
        let bytes = self
            .output
            .as_mut()
            .ok_or("reader_unavailable")?
            .next()
            .await?
            .ok_or("reader_exited")?;
        let reply: ReaderReply =
            serde_json::from_slice(&bytes).map_err(|_| "reader_frame_invalid")?;
        if reply.generation != self.generation || reply.sequence != self.sequence {
            return Err("reader_generation_mismatch");
        }
        let value = reply.result.map_err(|error| error.code())?;
        if !value.is_object() {
            return Err("reader_frame_invalid");
        }
        Ok(value)
    }

    pub(crate) fn begin_shutdown(&mut self) {
        if !matches!(self.shutdown, ShutdownState::Live) {
            return;
        }
        self.input.take();
        self.output.take();
        let deadline = Instant::now() + SHUTDOWN_DEADLINE;
        let terminated = if let Some(mut tree) = self.tree.take() {
            let result = tree.terminate();
            drop(tree); // Explicit termination and drop precede every reap.
            result.is_ok()
        } else {
            // Same-executable children receive no input on failed containment;
            // they cannot create descendants before this cleanup.
            self.child.start_kill().is_ok()
        };
        if !terminated {
            let _ = self.child.start_kill();
        }
        #[cfg(feature = "test_helpers")]
        let terminated = terminated && !self.fail_cleanup;
        self.shutdown = ShutdownState::Waiting {
            deadline,
            terminated,
        };
    }

    pub(crate) async fn shutdown(&mut self) -> Result<(), &'static str> {
        self.begin_shutdown();
        let (deadline, terminated) = match self.shutdown {
            ShutdownState::Waiting {
                deadline,
                terminated,
            } => (deadline, terminated),
            ShutdownState::Complete(result) => return result,
            ShutdownState::Live => return Err("reader_cleanup_failed"),
        };
        // The future borrows this retained child. Losing a select branch neither
        // drops ownership nor resets the absolute cleanup deadline.
        timeout_at(deadline, async {
            #[cfg(feature = "test_helpers")]
            if let Some(gate) = &self.before_reap {
                gate.wait_before_reap().await;
            }
            self.child.wait().await
        })
        .await
        .map_err(|_| "reader_cleanup_failed")?
        .map_err(|_| "reader_cleanup_failed")?;
        #[cfg(feature = "test_helpers")]
        self.terminal_observed
            .store(true, std::sync::atomic::Ordering::Release);
        let result = if terminated {
            Ok(())
        } else {
            Err("reader_cleanup_failed")
        };
        self.shutdown = ShutdownState::Complete(result);
        result
    }

    pub(crate) async fn idle_fault(&mut self) -> &'static str {
        let Some(output) = self.output.as_mut() else {
            return "reader_pipe_unavailable";
        };
        loop {
            tokio::select! {
                frame = output.next() => return match frame {
                    Ok(None) => "reader_exited", Ok(Some(_)) => "reader_unsolicited_output", Err(code) => code,
                },
                _ = tokio::time::sleep(CHILD_EXIT_POLL_INTERVAL) => {
                    let Some(tree) = self.tree.as_ref() else { return "reader_containment_failed"; };
                    #[cfg(unix)]
                    let exited = tree.leader_exited();
                    #[cfg(windows)]
                    let exited = tree.active_process_count().map(|count| count == 0);
                    match exited { Ok(true) => return "reader_exited", Err(_) => return "reader_state_unavailable", Ok(false) => {} }
                }
            }
        }
    }
}

/// An OS launch has one retained join owner even after its request is cancelled.
pub(crate) struct PendingLaunch {
    thread: Option<JoinHandle<Result<ReaderProcess, &'static str>>>,
    cancelled: Arc<AtomicBool>,
}

impl PendingLaunch {
    pub(crate) fn start(generation: u64, launch: WorkerLaunch) -> Result<Self, &'static str> {
        let runtime = Handle::try_current().map_err(|_| "reader_spawn_failed")?;
        #[cfg(feature = "test_helpers")]
        if let WorkerLaunch::Fixture { control, .. } = &launch {
            control.terminal_observed.store(false, Ordering::Release);
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let launch_cancelled = Arc::clone(&cancelled);
        let thread = std::thread::Builder::new()
            .name("mcp_reader_launch".to_owned())
            .spawn(move || {
                // Enter only to register Tokio process pipes. No future is run
                // here and no foreground runtime work is required to return.
                let _runtime = runtime.enter();
                let mut worker = ReaderProcess::spawn(generation, &launch)?;
                if launch_cancelled.load(Ordering::Acquire) {
                    worker.input.take();
                }
                Ok(worker)
            })
            .map_err(|_| "reader_spawn_failed")?;
        Ok(Self {
            thread: Some(thread),
            cancelled,
        })
    }

    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub(crate) async fn complete(&mut self) -> Result<ReaderProcess, &'static str> {
        while self
            .thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
        {
            tokio::time::sleep(LAUNCH_POLL_INTERVAL).await;
        }
        self.thread
            .take()
            .ok_or("reader_spawn_failed")?
            .join()
            .map_err(|_| "reader_spawn_failed")?
    }
}

impl Drop for PendingLaunch {
    fn drop(&mut self) {
        self.cancel();
        if let Some(thread) = self.thread.take() {
            // OS process launch is not safely interruptible. Final destruction
            // waits rather than detaching ownership, then drops/reaps any child.
            if let Ok(result) = thread.join() {
                drop(result);
            }
        }
    }
}

impl Drop for ReaderProcess {
    fn drop(&mut self) {
        // A panic or externally dropped owner has no async caller left. Keep a
        // bounded synchronous terminal observation, rather than claiming that
        // kill_on_drop alone proves reap. Normal paths await shutdown above.
        if let Some(mut tree) = self.tree.take() {
            let _ = tree.terminate();
            drop(tree);
        }
        if self.child.id().is_none() {
            return;
        }
        let _ = self.child.start_kill();
        let deadline = StdInstant::now() + SHUTDOWN_DEADLINE;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => {
                    #[cfg(feature = "test_helpers")]
                    self.terminal_observed
                        .store(true, std::sync::atomic::Ordering::Release);
                    return;
                }
                Err(_) => break,
                Ok(None) if StdInstant::now() < deadline => std::thread::sleep(DROP_POLL_INTERVAL),
                Ok(None) => break,
            }
        }
        eprintln!("reader_cleanup_unconfirmed");
    }
}
