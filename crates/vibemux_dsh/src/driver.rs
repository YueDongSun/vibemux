//! One DSH SDK session driven end to end.
//!
//! The driver is deliberately small and sequential: it owns the child, the
//! reader task, and one shared state cell. Every rule that needs no I/O lives
//! in `vibemux_harness::dsh`; this module supplies bytes, time, and
//! containment.
//!
//! Cancellation follows ADR 032 Decision 6. The SDK protocol publishes no
//! cancel or session-close method, so abandoning a turn means tearing the
//! contained process tree down. `close` first asks politely with `shutdown`
//! and only escalates when the grace period expires.

use std::{
    num::NonZeroUsize,
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use serde::Serialize;
use serde_json::Value;
use tokio::{
    io::{AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::watch,
    task::JoinHandle,
    time::{Instant, timeout, timeout_at},
};
use vibemux_harness::dispatch::DispatchError;
use vibemux_harness::dispatch::json_line_framer::JsonLineFramer;
use vibemux_harness::dsh::{
    DshAction, DshError, DshPhase, DshRecordBuffer, DshRecordKind, DshSessionLimits,
    DshSessionMachine, DshSessionSettings, DshSessionSummary, records::DshRecordBatch,
};
use vibemux_platform::{LAUNCH_GO_BYTE, PlatformError, ProcessTree};

use crate::DshDriverError;

/// How often the driver checks a deadline while waiting for a phase change.
/// The wait itself is signal-driven; this only bounds the post-signal race.
const PHASE_POLL: Duration = Duration::from_millis(20);
const REAP_DEADLINE: Duration = Duration::from_secs(5);

/// A resolved, shell-free command line. The configuration layer produces one;
/// tests construct one directly so no vendor runtime is required.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DshLaunch {
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    /// Names forwarded from the parent environment; everything else is
    /// cleared before the child starts (AGENTS.md §12.1).
    pub environment_names: Vec<String>,
    pub working_directory: PathBuf,
}

/// One record as it leaves the driver. Owned, because the poll releases the
/// shared lock before returning.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DshPollRecord {
    pub sequence: u64,
    pub kind: DshRecordKind,
    /// Byte-exact runtime line. The session poll operation is the only path
    /// that returns runtime content.
    pub raw_json: String,
}

/// One poll answer: records after the caller's cursor, plus the summary.
///
/// Every field is read under one acquisition of the state lock, and the
/// reader appends a record and advances the machine under that same lock. The
/// summary therefore describes exactly the records the buffer held: when
/// `more` is false, `next` equals `summary.records`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DshPoll {
    pub records: Vec<DshPollRecord>,
    /// Cursor to pass to the next poll.
    pub next: u64,
    pub more: bool,
    /// The caller's cursor predated the retained window, so records were
    /// evicted by the bounds before it could read them.
    pub truncated: bool,
    pub summary: DshSessionSummary,
    /// Whether the transport was still open when the page was taken: the
    /// runtime's stdout had not ended and no write had found its stdin closed.
    pub transport_open: bool,
}

struct DriverState {
    machine: DshSessionMachine,
    buffer: DshRecordBuffer,
    /// Cleared when stdout reaches EOF or fails; the protocol has no
    /// per-request cancellation frame, so this is how a peer death surfaces.
    transport_open: bool,
    transport_error: Option<DshDriverError>,
}

/// A live DSH session.
pub struct DshDriver {
    // The tree must be dropped before its leader can be reaped/reused.
    tree: ProcessTree,
    child: Child,
    stdin: Option<ChildStdin>,
    state: Arc<Mutex<DriverState>>,
    phase: watch::Receiver<DshPhase>,
    reader: Option<JoinHandle<()>>,
    limits: DshSessionLimits,
    /// Whether this driver terminated the process tree, so the exit it
    /// caused is not mistaken for a crash.
    terminated: bool,
    closed: bool,
}

impl std::fmt::Debug for DshDriver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DshDriver")
            .field("phase", &self.phase())
            .field("closed", &self.closed)
            .finish_non_exhaustive()
    }
}

impl DshDriver {
    /// Spawns the runtime and attempts `initialize`. Once contained, ownership
    /// is returned even if a write fails; [`DshDriver::await_ready`] reports
    /// that failure and the caller must close the owned driver.
    pub async fn spawn(
        launch: &DshLaunch,
        settings: DshSessionSettings,
        limits: DshSessionLimits,
    ) -> Result<Self, DshDriverError> {
        Self::spawn_contained(launch, settings, limits, ProcessTree::contain).await
    }

    async fn spawn_contained(
        launch: &DshLaunch,
        settings: DshSessionSettings,
        limits: DshSessionLimits,
        contain: impl FnOnce(u32) -> Result<ProcessTree, PlatformError>,
    ) -> Result<Self, DshDriverError> {
        let machine =
            DshSessionMachine::new(settings, limits).map_err(|_| DshDriverError::ConfigInvalid)?;
        let buffer = DshRecordBuffer::new(&limits);

        if !launch.executable.is_absolute() {
            return Err(DshDriverError::SpawnFailed);
        }
        let trampoline = tokio::task::spawn_blocking(launch_trampoline)
            .await
            .map_err(|_| DshDriverError::SpawnFailed)??;
        let mut command = Command::new(trampoline);
        command
            .arg(&launch.executable)
            .args(&launch.arguments)
            .current_dir(&launch.working_directory)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // stderr is diagnostics only; the runtime never logs there in this
            // slice, so it is discarded rather than retained.
            .stderr(Stdio::null())
            .kill_on_drop(true);
        for name in &launch.environment_names {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
        }
        ProcessTree::prepare_command(command.as_std_mut());

        let mut child = command.spawn().map_err(|_| DshDriverError::SpawnFailed)?;
        // The helper cannot start the runtime until it receives the go byte.
        // Failed containment therefore leaves only this owned helper to reap.
        let tree = match child.id().map(contain) {
            Some(Ok(tree)) => tree,
            _ => {
                child
                    .start_kill()
                    .map_err(|_| DshDriverError::ProcessCleanup)?;
                reap(&mut child).await?;
                return Err(DshDriverError::SpawnFailed);
            }
        };
        let stdin = child.stdin.take().ok_or(DshDriverError::StdinUnavailable)?;
        let stdout = child
            .stdout
            .take()
            .ok_or(DshDriverError::StdinUnavailable)?;

        let (phase_sender, phase) = watch::channel(DshPhase::Starting);
        let state = Arc::new(Mutex::new(DriverState {
            machine,
            buffer,
            transport_open: true,
            transport_error: None,
        }));
        let reader = tokio::spawn(read_frames(
            stdout,
            Arc::clone(&state),
            phase_sender,
            limits.frame_bytes,
        ));

        let mut driver = Self {
            tree,
            child,
            stdin: Some(stdin),
            state,
            phase,
            reader: Some(reader),
            limits,
            terminated: false,
            closed: false,
        };
        if let Err(error) = driver
            .write_until(
                &[LAUNCH_GO_BYTE],
                Instant::now() + Duration::from_millis(limits.initialize_deadline_ms),
                DshDriverError::InitializeTimeout,
            )
            .await
        {
            driver.record_start_failure(error);
            return Ok(driver);
        }
        match driver.with_machine(|machine| machine.start()) {
            Ok(actions) => {
                if let Err(error) = driver.apply(actions).await {
                    driver.record_start_failure(error);
                }
            }
            Err(error) => driver.record_start_failure(map_session_error(error)),
        }
        Ok(driver)
    }

    fn record_start_failure(&mut self, error: DshDriverError) {
        let mut state = lock(&self.state);
        state.machine.fail(map_driver_to_session(&error));
        state.transport_open = false;
        state.transport_error = Some(error);
    }

    #[must_use]
    pub fn phase(&self) -> DshPhase {
        lock(&self.state).machine.phase()
    }

    /// Whether the transport is still open: the runtime's stdout has not ended
    /// and no write has found its stdin closed.
    #[must_use]
    pub fn transport_open(&self) -> bool {
        lock(&self.state).transport_open
    }

    /// Waits for the handshake to settle, bounded by the configured
    /// initialize deadline.
    pub async fn await_ready(&mut self) -> Result<(), DshDriverError> {
        let deadline = Duration::from_millis(self.limits.initialize_deadline_ms);
        self.await_phase(DshPhase::Ready, deadline).await
    }

    /// Waits for the current turn to settle, bounded by the configured turn
    /// deadline.
    pub async fn await_turn(&mut self) -> Result<(), DshDriverError> {
        let deadline = Duration::from_millis(self.limits.turn_deadline_ms);
        self.await_phase(DshPhase::Ready, deadline).await
    }

    /// Sends one prompt. The turn is admitted only when the session is `Ready`
    /// and no other turn is running (ADR 032 Decision 4).
    pub async fn prompt(&mut self, prompt: &str) -> Result<(), DshDriverError> {
        let actions = self
            .with_machine(|machine| machine.prompt(prompt))
            .map_err(map_session_error)?;
        self.apply(actions).await
    }

    /// Reads the records after `cursor` without blocking the runtime.
    pub fn poll(&self, cursor: u64) -> Result<DshPoll, DshDriverError> {
        let state = lock(&self.state);
        Self::poll_locked(&state, cursor)
    }

    fn poll_locked(state: &DriverState, cursor: u64) -> Result<DshPoll, DshDriverError> {
        let batch: DshRecordBatch<'_> = state
            .buffer
            .after(cursor)
            .map_err(|_| DshDriverError::InvalidState)?;
        let records = batch
            .records
            .iter()
            .map(|record| DshPollRecord {
                sequence: record.sequence(),
                kind: record.kind(),
                raw_json: record.raw_json().to_string(),
            })
            .collect();
        Ok(DshPoll {
            records,
            next: batch.next,
            more: batch.more,
            truncated: batch.truncated,
            summary: Self::summary_locked(state),
            transport_open: state.transport_open,
        })
    }

    /// The content-free session summary, including the buffer counters. Cheap
    /// enough for a per-request status read; it copies no records.
    #[must_use]
    pub fn summary(&self) -> DshSessionSummary {
        let state = lock(&self.state);
        Self::summary_locked(&state)
    }

    fn summary_locked(state: &DriverState) -> DshSessionSummary {
        let mut summary = state.machine.summary();
        summary.records = state.buffer.head();
        summary.record_bytes = state.buffer.bytes() as u64;
        summary
    }

    /// The most recent failure the transport observed, if any.
    #[must_use]
    pub fn transport_error(&self) -> Option<DshDriverError> {
        lock(&self.state).transport_error.clone()
    }

    /// Cancels the session by terminating the contained process tree. There is
    /// no cancel frame in this protocol.
    pub async fn cancel(&mut self) -> Result<(), DshDriverError> {
        let actions = self
            .with_machine(|machine| Ok(machine.cancel()))
            .map_err(map_session_error)?;
        self.apply(actions).await?;
        self.terminate()
    }

    /// Asks the runtime to `shutdown`, then escalates to tree termination when
    /// the grace period expires. Remaining descendants are always terminated
    /// before the leader is reaped. A completed close is idempotent.
    ///
    /// `ShutdownTimeout` is reported only when the runtime was asked to close
    /// and ignored the request. A session that already failed or was cancelled
    /// is torn down without a spurious error, because termination is its
    /// documented teardown (ADR 032 Decision 6).
    pub async fn close(&mut self) -> Result<(), DshDriverError> {
        self.close_with(ProcessTree::terminate).await
    }

    async fn close_with(
        &mut self,
        mut terminate: impl FnMut(&mut ProcessTree) -> Result<(), PlatformError>,
    ) -> Result<(), DshDriverError> {
        if self.closed {
            return Ok(());
        }
        let cooperative = !self.phase().is_terminal();
        let expiry = Instant::now() + Duration::from_millis(self.limits.shutdown_grace_ms);
        let mut write_timed_out = false;
        if cooperative {
            // A session that already failed cannot be asked to close, and a
            // peer that already died cannot receive the frame; the teardown
            // below still runs in both cases.
            if let Ok(actions) = self.with_machine(|machine| machine.begin_close()) {
                match self
                    .apply_until(actions, expiry, DshDriverError::ShutdownTimeout)
                    .await
                {
                    Err(DshDriverError::ProcessCleanup) => {
                        return Err(DshDriverError::ProcessCleanup);
                    }
                    Err(DshDriverError::ShutdownTimeout) => write_timed_out = true,
                    // A closed pipe still needs the same teardown below.
                    _ => {}
                }
            }
        }
        let exited = if cooperative && !write_timed_out {
            self.wait_for_exit(expiry).await?
        } else {
            false
        };
        // Even a successful shutdown can leave descendants running. On POSIX
        // the unreaped leader reserves the group id until this kill succeeds.
        terminate(&mut self.tree).map_err(|_| DshDriverError::ProcessCleanup)?;
        if !exited {
            self.terminated = true;
        }
        self.stdin = None;
        let exit = reap(&mut self.child).await?;
        #[cfg(windows)]
        timeout(REAP_DEADLINE, async {
            while self
                .tree
                .active_process_count()
                .map_err(|_| DshDriverError::ProcessCleanup)?
                != 0
            {
                tokio::time::sleep(PHASE_POLL).await;
            }
            Ok::<_, DshDriverError>(())
        })
        .await
        .map_err(|_| DshDriverError::ProcessCleanup)??;
        self.record_exit(exit.success() || self.terminated);
        if let Some(reader) = self.reader.take() {
            reader.abort();
            let _ = reader.await;
        }
        // The process is gone and its reader stopped, whether or not the
        // reader saw stdout end first.
        lock(&self.state).transport_open = false;
        self.closed = true;
        if (!exited || write_timed_out) && cooperative {
            return Err(DshDriverError::ShutdownTimeout);
        }
        Ok(())
    }

    fn terminate(&mut self) -> Result<(), DshDriverError> {
        self.tree
            .terminate()
            .map_err(|_| DshDriverError::ProcessCleanup)?;
        self.terminated = true;
        Ok(())
    }

    async fn wait_for_exit(&mut self, expiry: Instant) -> Result<bool, DshDriverError> {
        #[cfg(unix)]
        {
            loop {
                if self
                    .tree
                    .leader_exited()
                    .map_err(|_| DshDriverError::ProcessCleanup)?
                {
                    return Ok(true);
                }
                if Instant::now() >= expiry {
                    return Ok(false);
                }
                tokio::time::sleep_until(expiry.min(Instant::now() + PHASE_POLL)).await;
            }
        }
        #[cfg(windows)]
        {
            // Job identity is independent of the leader's PID after reaping.
            match timeout_at(expiry, self.child.wait()).await {
                Ok(Ok(_)) => Ok(true),
                Ok(Err(_)) => Err(DshDriverError::ProcessCleanup),
                Err(_) => Ok(false),
            }
        }
    }

    /// Folds a process exit into the machine. `clean_exit` is false when the
    /// child was not reaped or exited unsuccessfully on its own.
    fn record_exit(&mut self, clean_exit: bool) {
        let mut state = lock(&self.state);
        state.machine.observe_exit(clean_exit);
    }

    async fn await_phase(
        &mut self,
        target: DshPhase,
        deadline: Duration,
    ) -> Result<(), DshDriverError> {
        let expiry = Instant::now() + deadline;
        loop {
            // The machine is the source of truth; the watch channel is only a
            // wakeup signal, so a missed notification cannot strand the wait.
            let (phase, open, transport_error) = {
                let state = lock(&self.state);
                (
                    state.machine.phase(),
                    state.transport_open,
                    state.transport_error.clone(),
                )
            };
            if phase == target {
                return Ok(());
            }
            if phase.is_terminal() {
                return Err(transport_error.unwrap_or(DshDriverError::TransportClosed));
            }
            // stdout closed without a terminal phase: the runtime died.
            if !open {
                let error = transport_error.unwrap_or(DshDriverError::TransportClosed);
                self.fail_session(error.clone());
                return Err(error);
            }
            let remaining = expiry.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                let error = if self.phase() == DshPhase::Initializing {
                    DshDriverError::InitializeTimeout
                } else {
                    DshDriverError::TurnTimeout
                };
                self.fail_session(error.clone());
                return Err(error);
            }
            let step = remaining.min(PHASE_POLL);
            let _ = timeout(step, self.phase.changed()).await;
        }
    }

    fn fail_session(&mut self, error: DshDriverError) {
        let mut state = lock(&self.state);
        state.machine.fail(map_driver_to_session(&error));
    }

    fn with_machine<T>(
        &self,
        action: impl FnOnce(&mut DshSessionMachine) -> Result<T, DshError>,
    ) -> Result<T, DshError> {
        let mut state = lock(&self.state);
        action(&mut state.machine)
    }

    async fn apply(&mut self, actions: Vec<DshAction>) -> Result<(), DshDriverError> {
        let (budget, error) = if self.phase() == DshPhase::Initializing {
            (
                self.limits.initialize_deadline_ms,
                DshDriverError::InitializeTimeout,
            )
        } else {
            (self.limits.turn_deadline_ms, DshDriverError::TurnTimeout)
        };
        self.apply_until(
            actions,
            Instant::now() + Duration::from_millis(budget),
            error,
        )
        .await
    }

    async fn apply_until(
        &mut self,
        actions: Vec<DshAction>,
        expiry: Instant,
        deadline_error: DshDriverError,
    ) -> Result<(), DshDriverError> {
        for action in actions {
            match action {
                DshAction::Write(frame) => {
                    let mut encoded =
                        serde_json::to_vec(&frame).map_err(|_| DshDriverError::WriteFailed)?;
                    encoded.push(b'\n');
                    self.write_until(&encoded, expiry, deadline_error.clone())
                        .await?;
                }
                DshAction::Terminate => {
                    self.terminate()?;
                }
            }
        }
        Ok(())
    }

    async fn write_until(
        &mut self,
        encoded: &[u8],
        expiry: Instant,
        deadline_error: DshDriverError,
    ) -> Result<(), DshDriverError> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or(DshDriverError::StdinUnavailable)?;
        match timeout_at(expiry, async {
            stdin.write_all(encoded).await?;
            stdin.flush().await
        })
        .await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(self.write_failed(&error)),
            Err(_) => {
                // A cancelled write can leave a partial JSON frame. Never
                // reuse that transport or wait forever for a blocked reader.
                self.stdin = None;
                self.fail_session(deadline_error.clone());
                lock(&self.state).transport_open = false;
                self.terminate()?;
                Err(deadline_error)
            }
        }
    }

    /// Records a failed frame write. A closed stdin is a transport failure,
    /// so it fails the session exactly as stdout EOF does; any other error is
    /// a local write fault and leaves the session state to the caller.
    ///
    /// A session that is already closing is left to `close`: a runtime that
    /// exited first cannot read `shutdown`, and the reaped exit closes the
    /// session there.
    fn write_failed(&mut self, error: &std::io::Error) -> DshDriverError {
        let failure = write_failure(error);
        if failure == DshDriverError::TransportClosed {
            self.stdin = None;
            let mut state = lock(&self.state);
            state.transport_open = false;
            if state.machine.phase() == DshPhase::Closing {
                return failure;
            }
            if state.transport_error.is_none() {
                state.transport_error = Some(failure.clone());
            }
            state.machine.fail(map_driver_to_session(&failure));
        }
        failure
    }
}

/// Classifies a failed write to the runtime's stdin. Writing to a pipe whose
/// reader is gone reports `BrokenPipe` on every platform (`EPIPE`, or
/// `ERROR_BROKEN_PIPE` and `ERROR_NO_DATA` on Windows): the runtime closed
/// its end, normally by exiting.
fn write_failure(error: &std::io::Error) -> DshDriverError {
    if error.kind() == std::io::ErrorKind::BrokenPipe {
        DshDriverError::TransportClosed
    } else {
        DshDriverError::WriteFailed
    }
}

impl Drop for DshDriver {
    fn drop(&mut self) {
        // The tree and `kill_on_drop` are the backstop; the explicit teardown
        // path is `close`.
        let _ = self.tree.terminate();
        if let Some(reader) = self.reader.take() {
            reader.abort();
        }
    }
}

async fn reap(child: &mut Child) -> Result<std::process::ExitStatus, DshDriverError> {
    timeout(REAP_DEADLINE, child.wait())
        .await
        .map_err(|_| DshDriverError::ProcessCleanup)?
        .map_err(|_| DshDriverError::ProcessCleanup)
}

/// Resolve only the bundled regular executable, never a helper from PATH.
fn launch_trampoline() -> Result<PathBuf, DshDriverError> {
    let current = std::env::current_exe().map_err(|_| DshDriverError::SpawnFailed)?;
    let directory = current.parent().ok_or(DshDriverError::SpawnFailed)?;
    let directory = if directory.file_name().is_some_and(|name| name == "deps") {
        directory.parent().ok_or(DshDriverError::SpawnFailed)?
    } else {
        directory
    };
    let path = directory.join(format!(
        "vibemux_launch_trampoline{}",
        std::env::consts::EXE_SUFFIX
    ));
    let metadata = std::fs::symlink_metadata(&path).map_err(|_| DshDriverError::SpawnFailed)?;
    if !metadata.file_type().is_file() {
        return Err(DshDriverError::SpawnFailed);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(DshDriverError::SpawnFailed);
        }
    }
    Ok(path)
}

/// Reads runtime stdout into the state cell until EOF or a framing failure.
async fn read_frames(
    stdout: tokio::process::ChildStdout,
    state: Arc<Mutex<DriverState>>,
    phase: watch::Sender<DshPhase>,
    frame_bytes: usize,
) {
    use tokio::io::AsyncBufReadExt;

    let Some(limit) = NonZeroUsize::new(frame_bytes) else {
        return;
    };
    let mut framer = JsonLineFramer::new(limit);
    let mut reader = BufReader::new(stdout);
    let mut frames: Vec<String> = Vec::new();
    loop {
        let chunk = match reader.fill_buf().await {
            Ok(chunk) => chunk,
            Err(_) => {
                finish_transport(&state, &phase, DshDriverError::TransportClosed);
                return;
            }
        };
        if chunk.is_empty() {
            // EOF. A clean `shutdown` closes stdout too, so the reader must
            // not conclude the session: only the owner knows whether the close
            // was expected. It records the transport state and wakes the wait.
            let mut guard = lock(&state);
            guard.transport_open = false;
            drop(guard);
            let _ = phase.send(DshPhase::Failed);
            return;
        }
        let consumed = chunk.len();
        let outcome = framer.push(chunk, &mut frames);
        reader.consume(consumed);
        if let Err(error) = outcome {
            // The framer is the ADR 029 one, so its failures are dispatch
            // errors; map them onto the driver vocabulary.
            let mapped = match error {
                DispatchError::FrameTooLarge => DshDriverError::FrameTooLarge,
                DispatchError::InvalidUtf8 => DshDriverError::InvalidUtf8,
                _ => DshDriverError::InvalidState,
            };
            finish_transport(&state, &phase, mapped);
            return;
        }
        for frame in frames.drain(..) {
            let parsed: Value = match serde_json::from_str(&frame) {
                Ok(parsed) => parsed,
                // A line that is not JSON cannot be a protocol frame; stdout
                // purity is a documented deployment requirement, so this is a
                // session-ending violation rather than noise to skip.
                Err(_) => {
                    finish_transport(&state, &phase, DshDriverError::InvalidState);
                    return;
                }
            };
            let mut guard = lock(&state);
            // The record is captured before the machine judges it, so a
            // protocol violation still leaves the bytes available as evidence.
            if guard.buffer.push(frame, &parsed).is_err() {
                drop(guard);
                finish_transport(&state, &phase, DshDriverError::FrameTooLarge);
                return;
            }
            match guard.machine.receive(&parsed) {
                Ok(_) => {}
                Err(_) => {
                    guard.transport_error = Some(DshDriverError::InvalidState);
                }
            }
            let phase_now = guard.machine.phase();
            drop(guard);
            let _ = phase.send(phase_now);
        }
    }
}

fn finish_transport(
    state: &Arc<Mutex<DriverState>>,
    phase: &watch::Sender<DshPhase>,
    error: DshDriverError,
) {
    let mut guard = lock(state);
    guard.transport_open = false;
    if guard.transport_error.is_none() {
        guard.transport_error = Some(error.clone());
    }
    guard.machine.fail(map_driver_to_session(&error));
    let current = guard.machine.phase();
    drop(guard);
    let _ = phase.send(current);
}

/// Locks the state cell, recovering from a poisoned lock: every critical
/// section here is a short, panic-free update, so a poisoned lock still holds
/// a consistent machine and failing the whole session would be worse.
fn lock(state: &Arc<Mutex<DriverState>>) -> std::sync::MutexGuard<'_, DriverState> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

fn map_driver_to_session(error: &DshDriverError) -> DshError {
    match error {
        DshDriverError::TransportClosed => DshError::UnexpectedExit,
        DshDriverError::FrameTooLarge => DshError::RecordBufferFull,
        DshDriverError::InvalidUtf8 | DshDriverError::InvalidState => DshError::InvalidJsonRpc,
        DshDriverError::InitializeTimeout | DshDriverError::TurnTimeout => DshError::UnexpectedExit,
        _ => DshError::ConfigInvalid,
    }
}

fn map_session_error(error: DshError) -> DshDriverError {
    match error {
        DshError::Closed | DshError::NotReady | DshError::TurnInFlight => {
            DshDriverError::InvalidState
        }
        DshError::InvalidPrompt => DshDriverError::ConfigInvalid,
        _ => DshDriverError::InvalidState,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    fn test_launch() -> DshLaunch {
        DshLaunch {
            executable: std::env::current_exe().expect("test executable"),
            arguments: vec!["--list".into()],
            environment_names: Vec::new(),
            working_directory: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        }
    }

    fn test_settings() -> DshSessionSettings {
        DshSessionSettings {
            session_id: "cleanup_failure_test".into(),
            working_directory: "C:\\project".into(),
            provider: "deepseek-official".into(),
            model: "deepseek-v4-flash".into(),
            reasoning_effort: None,
            max_tokens: None,
        }
    }

    fn containment_failure() -> PlatformError {
        PlatformError::ProcessTree {
            operation: vibemux_platform::ProcessTreeOperation::Verify,
            os_code: None,
        }
    }

    #[tokio::test]
    async fn containment_failure_refuses_launch_before_releasing_the_helper() {
        let mut containment_attempted = false;
        let result = DshDriver::spawn_contained(
            &test_launch(),
            test_settings(),
            DshSessionLimits::default(),
            |_| {
                containment_attempted = true;
                Err(containment_failure())
            },
        )
        .await;
        assert!(containment_attempted, "the test must reach containment");
        assert!(matches!(result, Err(DshDriverError::SpawnFailed)));
    }

    #[tokio::test]
    async fn cleanup_failure_is_reported_and_keeps_the_leader_owned_for_retry() {
        let mut driver =
            DshDriver::spawn(&test_launch(), test_settings(), DshSessionLimits::default())
                .await
                .expect("spawn contained test executable");
        assert!(
            driver
                .wait_for_exit(Instant::now() + Duration::from_secs(5))
                .await
                .expect("exit")
        );
        assert_eq!(
            driver.close_with(|_| Err(containment_failure())).await,
            Err(DshDriverError::ProcessCleanup)
        );
        assert!(!driver.closed);
        #[cfg(unix)]
        assert!(
            driver
                .tree
                .leader_exited()
                .expect("leader must remain unreaped")
        );
        driver.close().await.expect("retry successful teardown");
        assert!(driver.closed);
        driver.close().await.expect("idempotent close");
    }

    #[test]
    fn a_closed_runtime_pipe_is_a_transport_failure_and_other_errors_are_write_faults() {
        assert_eq!(
            write_failure(&Error::from(ErrorKind::BrokenPipe)),
            DshDriverError::TransportClosed
        );
        for kind in [
            ErrorKind::WouldBlock,
            ErrorKind::Interrupted,
            ErrorKind::PermissionDenied,
            ErrorKind::Other,
        ] {
            assert_eq!(
                write_failure(&Error::from(kind)),
                DshDriverError::WriteFailed,
                "{kind:?}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_pipe_closure_codes_are_transport_failures() {
        const ERROR_BROKEN_PIPE: i32 = 109;
        const ERROR_NO_DATA: i32 = 232;
        for code in [ERROR_BROKEN_PIPE, ERROR_NO_DATA] {
            assert_eq!(
                write_failure(&Error::from_raw_os_error(code)),
                DshDriverError::TransportClosed,
                "{code}"
            );
        }
    }
}
