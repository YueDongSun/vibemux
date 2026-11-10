//! One DSH SDK session driven end to end.
//!
//! The driver owns one lifecycle task and one shared state cell. Every rule
//! that needs no I/O lives in `vibemux_harness::dsh`; this module supplies
//! bytes, time, and containment. Deadlines and unexpected exits advance the
//! machine even when no consumer waits for a turn.
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

/// POSIX exit observation is non-reaping and must be polled. Turn deadlines
/// use their own absolute timer, not this interval or a consumer's reads.
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
    // The lifecycle task may terminate, but only close reaps on POSIX. Keep
    // the tree before the child so Drop preserves that ordering as well.
    tree: ProcessTree,
    child: Child,
    /// An exit caused by our termination is not an unexpected crash.
    terminated: bool,
    machine: DshSessionMachine,
    buffer: DshRecordBuffer,
    /// One absolute budget from prompt admission, shared by writes, the
    /// lifecycle timer, and frame handling. Never restarted by await_turn.
    turn_expiry: Option<Instant>,
    /// Cleared when stdout reaches EOF or fails; the protocol has no
    /// per-request cancellation frame, so this is how a peer death surfaces.
    transport_open: bool,
    transport_error: Option<DshDriverError>,
    /// EOF can precede a waitable leader exit. The lifecycle task gives that
    /// exit at most the shutdown grace, never beyond the remaining turn budget,
    /// before its first termination attempt, without reaping the leader.
    eof_cleanup_grace: Duration,
    /// Failure is already visible to poll/status, but error-returning callers
    /// must wait for cleanup so a cleanup error retains precedence.
    eof_cleanup_pending: bool,
}

impl DriverState {
    fn terminate_with(
        &mut self,
        terminate: impl FnOnce(&mut ProcessTree) -> Result<(), PlatformError>,
    ) -> Result<(), DshDriverError> {
        if terminate(&mut self.tree).is_err() {
            // Cleanup has priority over the original transport/deadline error.
            // The owner and unreaped leader remain available for close's retry.
            self.transport_error = Some(DshDriverError::ProcessCleanup);
            return Err(DshDriverError::ProcessCleanup);
        }
        self.terminated = true;
        Ok(())
    }

    fn leader_exited(&mut self) -> Result<bool, DshDriverError> {
        #[cfg(unix)]
        {
            self.tree
                .leader_exited()
                .map_err(|_| DshDriverError::ProcessCleanup)
        }
        #[cfg(windows)]
        {
            // Reaping cannot invalidate the owned Job Object's identity.
            self.child
                .try_wait()
                .map(|status| status.is_some())
                .map_err(|_| DshDriverError::ProcessCleanup)
        }
    }

    /// Atomically adjudicate a deadline against completion/cancel/close. The
    /// timer rechecks this cell after every wake, so an old turn's wake cannot
    /// terminate its successor. Readers also check before accepting a frame,
    /// so a late completion cannot win merely by beating the timer task.
    fn expire_turn_with(
        &mut self,
        now: Instant,
        terminate: impl FnOnce(&mut ProcessTree) -> Result<(), PlatformError>,
    ) -> bool {
        if self.machine.phase() != DshPhase::Prompting {
            self.turn_expiry = None;
            return false;
        }
        if self.turn_expiry.is_none_or(|expiry| now < expiry) {
            return false;
        }
        self.fail_transport_with(DshDriverError::TurnTimeout, terminate);
        true
    }

    /// Invalidate the transport and terminate without reaping. A concurrent
    /// writer must retain the first failure, and must not retry a failed
    /// cleanup behind the owner's back: only close can verify that retry.
    fn fail_transport_with(
        &mut self,
        error: DshDriverError,
        terminate: impl FnOnce(&mut ProcessTree) -> Result<(), PlatformError>,
    ) -> DshDriverError {
        self.machine.fail(map_driver_to_session(&error));
        self.turn_expiry = None;
        self.transport_open = false;
        let error = self.transport_error.get_or_insert(error).clone();
        if !self.eof_cleanup_pending
            && error != DshDriverError::ProcessCleanup
            && let Err(cleanup) = self.terminate_with(terminate)
        {
            return cleanup;
        }
        error
    }
}

/// A live DSH session.
pub struct DshDriver {
    stdin: Option<ChildStdin>,
    state: Arc<Mutex<DriverState>>,
    phase: watch::Receiver<DshPhase>,
    phase_sender: watch::Sender<DshPhase>,
    /// Owns stdout, the whole-turn timer, and non-reaping exit observation.
    /// close joins it and Drop aborts it; there is never a task per turn.
    reader: Option<JoinHandle<()>>,
    limits: DshSessionLimits,
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
            tree,
            child,
            terminated: false,
            machine,
            buffer,
            turn_expiry: None,
            transport_open: true,
            transport_error: None,
            eof_cleanup_grace: Duration::from_millis(limits.shutdown_grace_ms),
            eof_cleanup_pending: false,
        }));
        let reader = tokio::spawn(read_frames(
            stdout,
            Arc::clone(&state),
            phase_sender.clone(),
            limits.frame_bytes,
        ));

        let mut driver = Self {
            stdin: Some(stdin),
            state,
            phase,
            phase_sender,
            reader: Some(reader),
            limits,
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
        state.transport_error.get_or_insert(error);
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
        let expiry = Instant::now() + Duration::from_millis(self.limits.initialize_deadline_ms);
        self.await_phase(DshPhase::Ready, Some(expiry)).await
    }

    /// Waits for the current turn to settle. The lifecycle task owns the
    /// deadline from prompt admission; waiting (or waiting again) never
    /// extends it, and dropping this future does not disarm it.
    pub async fn await_turn(&mut self) -> Result<(), DshDriverError> {
        if matches!(
            self.phase(),
            DshPhase::Starting | DshPhase::Initializing | DshPhase::Closing
        ) {
            // No turn was admitted, or it has already been abandoned. There
            // is no armed turn timer to bound a wait; do not invent a budget.
            return Err(self
                .transport_error()
                .unwrap_or(DshDriverError::InvalidState));
        }
        self.await_phase(DshPhase::Ready, None).await
    }

    /// Sends one prompt. The turn is admitted only when the session is `Ready`
    /// and no other turn is running (ADR 032 Decision 4).
    pub async fn prompt(&mut self, prompt: &str) -> Result<(), DshDriverError> {
        let (actions, expiry) = {
            let mut state = lock(&self.state);
            let expiry = Instant::now() + Duration::from_millis(self.limits.turn_deadline_ms);
            let actions = state.machine.prompt(prompt).map_err(map_session_error)?;
            // Arm before writing: a timed-out or abandoned write must not
            // strand a prompting session either.
            state.turn_expiry = Some(expiry);
            (actions, expiry)
        };
        self.phase_sender.send_replace(DshPhase::Prompting);
        self.apply_until(actions, expiry, DshDriverError::TurnTimeout)
            .await
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
        if self.closed {
            return Ok(());
        }
        let actions = self
            .with_machine(|machine| Ok(machine.cancel()))
            .map_err(map_session_error)?;
        // Enter Closing (or observe Failed) before waiting: EOF cannot start
        // a new cleanup between this wait and cancellation's termination.
        self.await_transport_cleanup().await;
        if self.transport_error() == Some(DshDriverError::ProcessCleanup) {
            return Err(DshDriverError::ProcessCleanup);
        }
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
        let expiry = Instant::now() + Duration::from_millis(self.limits.shutdown_grace_ms);
        // Decide and enter Closing under the same lock as the lifecycle task.
        // A deadline that won first must not acquire a spurious shutdown wait;
        // a close that won first disarms the turn's timer atomically.
        let actions = self.with_machine(|machine| machine.begin_close()).ok();
        // Check pending cleanup after entering Closing/observing Failed. EOF
        // cannot start a new attempt past that state transition. Let its owner
        // finish before close retries termination or reaps the leader.
        self.await_transport_cleanup().await;
        let cooperative = actions.is_some();
        let mut write_timed_out = false;
        if let Some(actions) = actions {
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
        let exited = if cooperative && !write_timed_out {
            self.wait_for_exit(expiry).await?
        } else {
            false
        };
        // Even a successful shutdown can leave descendants running. On POSIX
        // the unreaped leader reserves the group id until this kill succeeds.
        {
            let mut state = lock(&self.state);
            if terminate(&mut state.tree).is_err() {
                state.transport_error = Some(DshDriverError::ProcessCleanup);
                return Err(DshDriverError::ProcessCleanup);
            }
            if !exited {
                state.terminated = true;
            }
        }
        self.stdin = None;
        let exit = self.reap_owned_child().await?;
        #[cfg(windows)]
        timeout(REAP_DEADLINE, async {
            loop {
                let active = lock(&self.state)
                    .tree
                    .active_process_count()
                    .map_err(|_| DshDriverError::ProcessCleanup)?;
                if active == 0 {
                    return Ok::<_, DshDriverError>(());
                }
                tokio::time::sleep(PHASE_POLL).await;
            }
        })
        .await
        .map_err(|_| DshDriverError::ProcessCleanup)??;
        let terminated = lock(&self.state).terminated;
        self.record_exit(exit.success() || terminated);
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
        lock(&self.state).terminate_with(ProcessTree::terminate)
    }

    async fn wait_for_exit(&mut self, expiry: Instant) -> Result<bool, DshDriverError> {
        loop {
            if lock(&self.state).leader_exited()? {
                return Ok(true);
            }
            if Instant::now() >= expiry {
                return Ok(false);
            }
            tokio::time::sleep_until(expiry.min(Instant::now() + PHASE_POLL)).await;
        }
    }

    /// Called only after tree termination succeeded. Never hold the shared
    /// lock over an await, or reap before terminating the POSIX group.
    async fn reap_owned_child(&mut self) -> Result<std::process::ExitStatus, DshDriverError> {
        timeout(REAP_DEADLINE, async {
            loop {
                let exit = lock(&self.state)
                    .child
                    .try_wait()
                    .map_err(|_| DshDriverError::ProcessCleanup)?;
                if let Some(exit) = exit {
                    return Ok(exit);
                }
                tokio::time::sleep(PHASE_POLL).await;
            }
        })
        .await
        .map_err(|_| DshDriverError::ProcessCleanup)?
    }

    /// Folds a process exit into the machine. `clean_exit` is false when the
    /// child was not reaped or exited unsuccessfully on its own.
    fn record_exit(&mut self, clean_exit: bool) {
        let mut state = lock(&self.state);
        state.machine.observe_exit(clean_exit);
    }

    async fn await_transport_cleanup(&mut self) {
        loop {
            self.phase.borrow_and_update();
            if !lock(&self.state).eof_cleanup_pending {
                return;
            }
            let _ = self.phase.changed().await;
        }
    }

    async fn await_phase(
        &mut self,
        target: DshPhase,
        expiry: Option<Instant>,
    ) -> Result<(), DshDriverError> {
        loop {
            // The machine is the source of truth; the watch channel is only a
            // wakeup signal. Mark it seen before the snapshot to avoid losing
            // a transition between checking the machine and waiting.
            self.phase.borrow_and_update();
            let (phase, open, transport_error, cleanup_pending) = {
                let state = lock(&self.state);
                (
                    state.machine.phase(),
                    state.transport_open,
                    state.transport_error.clone(),
                    state.eof_cleanup_pending,
                )
            };
            if cleanup_pending {
                let _ = self.phase.changed().await;
                continue;
            }
            if phase == target {
                return Ok(());
            }
            if phase.is_terminal() {
                return Err(transport_error.unwrap_or(DshDriverError::TransportClosed));
            }
            if !open {
                let error = transport_error.unwrap_or(DshDriverError::TransportClosed);
                self.fail_session(error.clone());
                return Err(error);
            }
            tokio::select! {
                _ = wait_for_deadline(expiry) => {
                    let error = {
                        let mut state = lock(&self.state);
                        let current = state.machine.phase();
                        if current == target || current.is_terminal() {
                            // EOF/completion may have won after our snapshot,
                            // before its watch wakeup was delivered. Recheck
                            // pending cleanup instead of returning a stale timer.
                            None
                        } else {
                            let error = if current == DshPhase::Initializing {
                                DshDriverError::InitializeTimeout
                            } else {
                                DshDriverError::TurnTimeout
                            };
                            state.machine.fail(map_driver_to_session(&error));
                            state.turn_expiry = None;
                            state.transport_error.get_or_insert(error.clone());
                            Some(error)
                        }
                    };
                    if let Some(error) = error {
                        self.phase_sender.send_replace(DshPhase::Failed);
                        return Err(error);
                    }
                }
                _ = self.phase.changed() => {}
            }
        }
    }

    fn fail_session(&mut self, error: DshDriverError) {
        let mut state = lock(&self.state);
        state.machine.fail(map_driver_to_session(&error));
        state.turn_expiry = None;
        state.transport_error.get_or_insert(error);
        let phase = state.machine.phase();
        drop(state);
        self.phase_sender.send_replace(phase);
    }

    fn with_machine<T>(
        &self,
        action: impl FnOnce(&mut DshSessionMachine) -> Result<T, DshError>,
    ) -> Result<T, DshError> {
        let mut state = lock(&self.state);
        // A caller can run before a woken timer. An already-expired budget
        // must be resolved before cancel/close can abandon and disarm it.
        state.expire_turn_with(Instant::now(), ProcessTree::terminate);
        let result = action(&mut state.machine);
        let phase = state.machine.phase();
        if phase != DshPhase::Prompting {
            state.turn_expiry = None;
        }
        drop(state);
        self.phase_sender.send_replace(phase);
        result
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
        if let Err(error) = self.check_write_state() {
            self.await_transport_cleanup().await;
            return Err(self.transport_error().unwrap_or(error));
        }
        let stdin = self
            .stdin
            .as_mut()
            .ok_or(DshDriverError::StdinUnavailable)?;
        let result = match timeout_at(expiry, async {
            stdin.write_all(encoded).await?;
            stdin.flush().await
        })
        .await
        {
            // The lifecycle task may have failed the turn while a write was
            // pending. Even a ready write must not hide that failure.
            Ok(Ok(())) => self.check_write_state(),
            Ok(Err(error)) => Err(self.write_failed(&error)),
            Err(_) => Err(self.write_timed_out(deadline_error)),
        };
        if let Err(error) = result {
            self.await_transport_cleanup().await;
            // Historical errors in Closing must not block close's cleanup
            // retry, but a newly failed transport must report cleanup first.
            return Err(if self.phase() == DshPhase::Failed {
                self.transport_error().unwrap_or(error)
            } else {
                error
            });
        }
        Ok(())
    }

    fn check_write_state(&mut self) -> Result<(), DshDriverError> {
        let mut state = lock(&self.state);
        let expired = state.expire_turn_with(Instant::now(), ProcessTree::terminate);
        let phase = state.machine.phase();
        let error = (phase == DshPhase::Failed).then(|| {
            state
                .transport_error
                .clone()
                .unwrap_or(DshDriverError::InvalidState)
        });
        drop(state);
        if expired {
            self.phase_sender.send_replace(phase);
        }
        if let Some(error) = error {
            self.stdin = None;
            return Err(error);
        }
        Ok(())
    }

    fn write_timed_out(&mut self, error: DshDriverError) -> DshDriverError {
        // A cancelled write can leave a partial JSON frame. Invalidate it
        // atomically, retaining a failure already recorded by the watchdog.
        self.stdin = None;
        let mut state = lock(&self.state);
        let error = state.fail_transport_with(error, ProcessTree::terminate);
        let phase = state.machine.phase();
        drop(state);
        self.phase_sender.send_replace(phase);
        error
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
        let mut state = lock(&self.state);
        // The pipe and turn timers can become ready together. Check the
        // absolute budget and its recorded cause under a single lock.
        state.expire_turn_with(Instant::now(), ProcessTree::terminate);
        let failure = if state.machine.phase() == DshPhase::Failed {
            state.transport_error.clone().unwrap_or(failure)
        } else if failure == DshDriverError::TransportClosed {
            state.transport_open = false;
            state.turn_expiry = None;
            if matches!(state.machine.phase(), DshPhase::Closing | DshPhase::Closed) {
                // Intentional shutdown, including a cleanup retry, still
                // needs close to judge the reaped exit and verify cleanup.
                failure
            } else {
                state.fail_transport_with(failure, ProcessTree::terminate)
            }
        } else {
            failure
        };
        let phase = state.machine.phase();
        drop(state);
        if failure != DshDriverError::WriteFailed {
            self.stdin = None;
        }
        self.phase_sender.send_replace(phase);
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
        // path is `close`. Abort the owned task before releasing process state.
        if let Some(reader) = self.reader.take() {
            reader.abort();
        }
        let _ = lock(&self.state).tree.terminate();
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

async fn wait_for_deadline(expiry: Option<Instant>) {
    match expiry {
        Some(expiry) => tokio::time::sleep_until(expiry).await,
        None => std::future::pending().await,
    }
}

/// The single owned lifecycle task: stdout, an absolute whole-turn timer, and
/// process-exit observation. It never reaps the POSIX leader; close retains
/// that owner until termination and bounded reaping both succeed.
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
    let mut changed = phase.subscribe();
    let mut exit_check = Instant::now() + PHASE_POLL;
    let mut exit_seen = false;
    loop {
        changed.borrow_and_update();
        let expiry = lock(&state).turn_expiry;
        let chunk = tokio::select! {
            biased;
            _ = wait_for_deadline(expiry) => {
                if expire_turn(&state, &phase) {
                    return;
                }
                continue;
            }
            // A descendant that keeps writing stdout must not starve exit
            // observation. Both timers take priority over another read.
            _ = tokio::time::sleep_until(exit_check) => {
                exit_check = Instant::now() + PHASE_POLL;
                let exited = {
                    let mut guard = lock(&state);
                    if guard.machine.phase().is_terminal()
                        || guard.machine.phase() == DshPhase::Closing
                    {
                        Ok(false)
                    } else {
                        guard.leader_exited()
                    }
                };
                match exited {
                    // Give buffered stdout a scheduling turn to drain before
                    // interpreting exit. A descendant holding the pipe open
                    // cannot keep the session alive indefinitely, though.
                    Ok(true) if !exit_seen => exit_seen = true,
                    Ok(true) => {
                        finish_transport(&state, &phase, DshDriverError::TransportClosed).await;
                        return;
                    }
                    Ok(false) => {}
                    Err(error) => {
                        finish_transport(&state, &phase, error).await;
                        return;
                    }
                }
                continue;
            }
            result = reader.fill_buf() => match result {
                Ok(chunk) => chunk,
                Err(_) => {
                    finish_transport(&state, &phase, DshDriverError::TransportClosed).await;
                    return;
                }
            },
            _ = changed.changed() => continue,
        };
        if chunk.is_empty() {
            // EOF outside an intentional close is an authoritative failure,
            // not merely a wakeup for callers that happen to await a turn.
            finish_transport(&state, &phase, DshDriverError::TransportClosed).await;
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
            finish_transport(&state, &phase, mapped).await;
            return;
        }
        for frame in frames.drain(..) {
            let parsed: Value = match serde_json::from_str(&frame) {
                Ok(parsed) => parsed,
                // A line that is not JSON cannot be a protocol frame; stdout
                // purity is a documented deployment requirement, so this is a
                // session-ending violation rather than noise to skip.
                Err(_) => {
                    finish_transport(&state, &phase, DshDriverError::InvalidState).await;
                    return;
                }
            };
            let frame_error = {
                let mut guard = lock(&state);
                // A ready stdout can win scheduling against the timer. Judge
                // the budget before accepting any late turn/end.
                if guard.expire_turn_with(Instant::now(), ProcessTree::terminate) {
                    drop(guard);
                    phase.send_replace(DshPhase::Failed);
                    return;
                }
                // Capture the record before the machine judges it, so a
                // protocol violation still leaves the bytes as evidence.
                if guard.buffer.push(frame, &parsed).is_err() {
                    Some(DshDriverError::FrameTooLarge)
                } else {
                    match guard.machine.receive(&parsed) {
                        Ok(_) => {}
                        Err(_) => {
                            guard
                                .transport_error
                                .get_or_insert(DshDriverError::InvalidState);
                        }
                    }
                    let phase_now = guard.machine.phase();
                    if phase_now != DshPhase::Prompting {
                        guard.turn_expiry = None;
                    }
                    drop(guard);
                    phase.send_replace(phase_now);
                    None
                }
            };
            if let Some(error) = frame_error {
                finish_transport(&state, &phase, error).await;
                return;
            }
        }
    }
}

fn expire_turn(state: &Arc<Mutex<DriverState>>, phase: &watch::Sender<DshPhase>) -> bool {
    let mut guard = lock(state);
    let expired = guard.expire_turn_with(Instant::now(), ProcessTree::terminate);
    let current = guard.machine.phase();
    drop(guard);
    if expired {
        phase.send_replace(current);
    }
    expired
}

async fn finish_transport(
    state: &Arc<Mutex<DriverState>>,
    phase: &watch::Sender<DshPhase>,
    error: DshDriverError,
) {
    finish_transport_with(
        state,
        phase,
        error,
        DriverState::leader_exited,
        ProcessTree::terminate,
    )
    .await;
}

async fn finish_transport_with(
    state: &Arc<Mutex<DriverState>>,
    phase: &watch::Sender<DshPhase>,
    error: DshDriverError,
    mut observe_exit: impl FnMut(&mut DriverState) -> Result<bool, DshDriverError>,
    mut terminate: impl FnMut(&mut ProcessTree) -> Result<(), PlatformError>,
) {
    let (cleanup_expiry, current) = {
        let mut guard = lock(state);
        let now = Instant::now();
        // EOF can be scheduled before an already-ready deadline timer, too.
        guard.expire_turn_with(now, ProcessTree::terminate);
        let turn_expiry = guard.turn_expiry;
        guard.transport_open = false;
        guard.turn_expiry = None;
        let current = guard.machine.phase();
        // Only the close owner can decide whether EOF during teardown is clean.
        let expected_eof = error == DshDriverError::TransportClosed
            && matches!(current, DshPhase::Closing | DshPhase::Closed);
        let mut cleanup_expiry = None;
        if !expected_eof {
            guard.transport_error.get_or_insert(error.clone());
            guard.machine.fail(map_driver_to_session(&error));
            if error == DshDriverError::TransportClosed && !current.is_terminal() {
                // Closing the last pipe can precede a waitable process exit.
                // On Darwin, killpg can return EPERM in that interval because
                // the exiting member is no longer signalable, but NOWAIT cannot
                // yet prove it dead. Observe the exit before the FIRST kill,
                // bounded by the existing shutdown grace and turn deadline.
                let expiry = now + guard.eof_cleanup_grace;
                cleanup_expiry = Some(turn_expiry.map_or(expiry, |turn| turn.min(expiry)));
                guard.eof_cleanup_pending = true;
            }
        }
        (cleanup_expiry, guard.machine.phase())
    };
    // Poll/status see the authoritative failure immediately; awaited errors
    // wait for cleanup, so a failed termination still takes precedence.
    phase.send_replace(current);
    let Some(expiry) = cleanup_expiry else {
        return;
    };
    loop {
        let (finished, current) = {
            let mut guard = lock(state);
            let finished = if guard.transport_error == Some(DshDriverError::ProcessCleanup) {
                // Only close may retry an already-failed cleanup operation.
                true
            } else {
                match observe_exit(&mut guard) {
                    Ok(false) if Instant::now() < expiry => false,
                    Ok(_) => {
                        // EOF need not mean exit: after the bounded observation
                        // window, terminate a peer that just closed its pipe.
                        // Every termination error stays an error; no retry here.
                        let _ = guard.terminate_with(&mut terminate);
                        true
                    }
                    Err(_) => {
                        guard.transport_error = Some(DshDriverError::ProcessCleanup);
                        true
                    }
                }
            };
            if finished {
                guard.eof_cleanup_pending = false;
            }
            (finished, guard.machine.phase())
        };
        if finished {
            phase.send_replace(current);
            return;
        }
        tokio::time::sleep_until(expiry.min(Instant::now() + PHASE_POLL)).await;
    }
}

/// Locks the state cell, recovering from a poisoned lock: every critical
/// section here is a short, panic-free update, so a poisoned lock still holds
/// a consistent machine and failing the whole session would be worse.
fn lock(state: &Arc<Mutex<DriverState>>) -> std::sync::MutexGuard<'_, DriverState> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

fn map_driver_to_session(error: &DshDriverError) -> DshError {
    match error {
        DshDriverError::TransportClosed | DshDriverError::ProcessCleanup => {
            DshError::UnexpectedExit
        }
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
            lock(&driver.state)
                .tree
                .leader_exited()
                .expect("leader must remain unreaped")
        );
        driver.close().await.expect("retry successful teardown");
        assert!(driver.closed);
        driver.close().await.expect("idempotent close");
    }

    /// The owned test executable exits without being reaped on POSIX. Stop
    /// its reader before injecting machine transitions and cleanup failures.
    async fn exited_driver_with_ready_machine() -> DshDriver {
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
        driver.await_transport_cleanup().await;
        if let Some(reader) = driver.reader.take() {
            reader.abort();
            let _ = reader.await;
        }
        let mut machine =
            DshSessionMachine::new(test_settings(), DshSessionLimits::default()).expect("machine");
        let initialize = machine.start().expect("initialize");
        let DshAction::Write(frame) = &initialize[0] else {
            panic!("initialize must write a frame");
        };
        machine
            .receive(&serde_json::json!({
                "jsonrpc": "2.0", "id": frame["id"],
                "result": {"serverInfo": {
                    "name": "deepseek-harness-sdk-runtime", "version": "0.0.1"
                }}
            }))
            .expect("ready");
        {
            let mut state = lock(&driver.state);
            state.machine = machine;
            state.transport_open = true;
            state.transport_error = None;
        }
        driver
    }

    async fn assert_pending_once(future: impl std::future::Future) {
        tokio::pin!(future);
        std::future::poll_fn(|context| {
            assert!(future.as_mut().poll(context).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
    }

    #[tokio::test]
    async fn eof_waits_for_a_waitable_exit_without_hiding_a_cleanup_failure() {
        use std::cell::Cell;

        let mut driver = exited_driver_with_ready_machine().await;
        lock(&driver.state)
            .machine
            .prompt("EOF before a waitable exit")
            .expect("prompt");
        let state = Arc::clone(&driver.state);
        let phase = driver.phase_sender.clone();
        let waitable = Cell::new(false);
        let attempts = Cell::new(0);
        let cleanup = finish_transport_with(
            &state,
            &phase,
            DshDriverError::TransportClosed,
            |_| Ok(waitable.get()),
            |_| {
                assert!(waitable.get(), "must observe exit before the first kill");
                attempts.set(attempts.get() + 1);
                Err(containment_failure())
            },
        );
        tokio::pin!(cleanup);
        // Explicitly poll each future once: no sleep guesses which operation
        // won. The exit remains unobservable until the test releases it.
        assert_pending_once(cleanup.as_mut()).await;
        let page = driver.poll(0).expect("poll");
        assert_eq!(page.summary.phase, DshPhase::Failed);
        assert_eq!(page.summary.unverified_turns, 1);
        assert_eq!(page.summary.completed_turns, 0);
        assert!(!page.transport_open);
        assert_eq!(attempts.get(), 0);
        assert_pending_once(driver.await_turn()).await;
        assert_pending_once(driver.await_ready()).await;
        assert_pending_once(driver.write_until(
            &[],
            Instant::now() + Duration::from_secs(5),
            DshDriverError::TurnTimeout,
        ))
        .await;
        assert_pending_once(driver.cancel()).await;
        assert_pending_once(driver.close()).await;
        // An abandoned writer must not race the lifecycle owner's first kill.
        {
            let mut guard = lock(&state);
            assert_eq!(
                guard.fail_transport_with(DshDriverError::TurnTimeout, |_| {
                    panic!("the pending EOF cleanup belongs to the lifecycle task")
                }),
                DshDriverError::TransportClosed
            );
        }
        waitable.set(true);
        cleanup.await;
        assert_eq!(attempts.get(), 1, "a failed kill is never retried here");
        assert_eq!(
            driver.await_turn().await,
            Err(DshDriverError::ProcessCleanup)
        );
        assert_eq!(
            driver.await_ready().await,
            Err(DshDriverError::ProcessCleanup)
        );
        assert_eq!(
            driver
                .write_until(&[], Instant::now(), DshDriverError::TurnTimeout)
                .await,
            Err(DshDriverError::ProcessCleanup)
        );
        assert_eq!(driver.cancel().await, Err(DshDriverError::ProcessCleanup));
        assert!(!driver.closed);
        #[cfg(unix)]
        assert!(lock(&state).tree.leader_exited().expect("unreaped owner"));
        driver.close().await.expect("explicit cleanup retry");
        assert!(driver.closed);
    }

    #[tokio::test]
    async fn close_waits_for_eof_cleanup_before_retrying_or_reaping() {
        use std::cell::Cell;

        let mut driver = exited_driver_with_ready_machine().await;
        let state = Arc::clone(&driver.state);
        let phase = driver.phase_sender.clone();
        let waitable = Cell::new(false);
        let cleanup_attempts = Cell::new(0);
        let close_attempts = Cell::new(0);
        {
            let cleanup = finish_transport_with(
                &state,
                &phase,
                DshDriverError::TransportClosed,
                |_| Ok(waitable.get()),
                |_| {
                    cleanup_attempts.set(cleanup_attempts.get() + 1);
                    Err(containment_failure())
                },
            );
            tokio::pin!(cleanup);
            assert_pending_once(cleanup.as_mut()).await;
            let close = driver.close_with(|tree| {
                assert_eq!(cleanup_attempts.get(), 1);
                close_attempts.set(close_attempts.get() + 1);
                tree.terminate()
            });
            tokio::pin!(close);
            assert_pending_once(close.as_mut()).await;
            assert_eq!(cleanup_attempts.get(), 0);
            assert_eq!(close_attempts.get(), 0);
            #[cfg(unix)]
            assert!(lock(&state).tree.leader_exited().expect("unreaped owner"));
            waitable.set(true);
            cleanup.await;
            close
                .await
                .expect("close retries only the finished attempt");
        }
        assert_eq!(cleanup_attempts.get(), 1);
        assert_eq!(close_attempts.get(), 1);
        assert!(driver.closed);
        // The daemon's replacement path awaits this same verified close before
        // dropping the old driver or admitting the replacement session.
        driver.close().await.expect("idempotent close");
    }

    #[tokio::test]
    async fn a_ready_wait_timeout_cannot_overtake_pending_eof_cleanup() {
        let mut driver = exited_driver_with_ready_machine().await;
        let mut machine =
            DshSessionMachine::new(test_settings(), DshSessionLimits::default()).expect("machine");
        machine.start().expect("initializing");
        lock(&driver.state).machine = machine;
        let state = Arc::clone(&driver.state);
        let phase = driver.phase_sender.clone();
        {
            let expiry = Instant::now() + Duration::from_secs(1);
            let waiter = driver.await_phase(DshPhase::Ready, Some(expiry));
            tokio::pin!(waiter);
            assert_pending_once(waiter.as_mut()).await;
            {
                let mut guard = lock(&state);
                guard.machine.fail(DshError::UnexpectedExit);
                guard.transport_open = false;
                guard.transport_error = Some(DshDriverError::TransportClosed);
                guard.eof_cleanup_pending = true;
            }
            // Pause between publishing the EOF state and sending its watch
            // wakeup. Only the stale initialize timer is ready on the next poll.
            tokio::time::sleep_until(expiry).await;
            assert_pending_once(waiter.as_mut()).await;
            {
                let mut guard = lock(&state);
                guard.transport_error = Some(DshDriverError::ProcessCleanup);
                guard.eof_cleanup_pending = false;
            }
            phase.send_replace(DshPhase::Failed);
            assert_eq!(waiter.await, Err(DshDriverError::ProcessCleanup));
        }
        driver.close().await.expect("explicit cleanup retry");
    }

    #[tokio::test]
    async fn eof_exit_observation_is_bounded_by_shutdown_grace_and_remaining_turn_budget() {
        use std::cell::Cell;

        for prompting in [false, true] {
            let mut driver = exited_driver_with_ready_machine().await;
            let budget = Duration::from_secs(1);
            let expiry = Instant::now() + budget;
            {
                let mut state = lock(&driver.state);
                if prompting {
                    state.machine.prompt("EOF during a turn").expect("prompt");
                    state.turn_expiry = Some(expiry);
                    state.eof_cleanup_grace = Duration::from_secs(5);
                } else {
                    state.eof_cleanup_grace = budget;
                }
            }
            let attempts = Cell::new(0);
            timeout(
                Duration::from_secs(3),
                finish_transport_with(
                    &driver.state,
                    &driver.phase_sender,
                    DshDriverError::TransportClosed,
                    |_| Ok(false),
                    |tree| {
                        assert!(Instant::now() >= expiry);
                        attempts.set(attempts.get() + 1);
                        tree.terminate()
                    },
                ),
            )
            .await
            .expect("EOF from a living peer must not stall cleanup");
            assert_eq!(attempts.get(), 1);
            assert_eq!(
                driver.transport_error(),
                Some(DshDriverError::TransportClosed)
            );
            assert!(!lock(&driver.state).eof_cleanup_pending);
            assert_eq!(driver.summary().unverified_turns, u64::from(prompting));
            driver.close().await.expect("cleanup");
        }
    }

    #[tokio::test]
    async fn eof_exit_query_failure_keeps_ownership_and_does_not_signal() {
        let mut driver = exited_driver_with_ready_machine().await;
        finish_transport_with(
            &driver.state,
            &driver.phase_sender,
            DshDriverError::TransportClosed,
            |_| Err(DshDriverError::ProcessCleanup),
            |_| panic!("failed ownership observation cannot authorize a signal"),
        )
        .await;
        assert_eq!(
            driver.await_ready().await,
            Err(DshDriverError::ProcessCleanup)
        );
        assert!(!driver.closed);
        #[cfg(unix)]
        assert!(
            lock(&driver.state)
                .tree
                .leader_exited()
                .expect("unreaped owner")
        );
        driver.close().await.expect("explicit cleanup retry");
        assert!(driver.closed);
    }

    #[tokio::test]
    async fn a_deadline_cleanup_failure_retains_ownership_and_the_unverified_turn() {
        let mut driver = exited_driver_with_ready_machine().await;
        {
            let mut state = lock(&driver.state);
            state.machine.prompt("deadline").expect("prompt");
            state.turn_expiry = Some(Instant::now());
            assert!(state.expire_turn_with(Instant::now(), |_| Err(containment_failure())));
            assert_eq!(state.machine.phase(), DshPhase::Failed);
            assert_eq!(state.transport_error, Some(DshDriverError::ProcessCleanup));
            assert_eq!(state.machine.summary().unverified_turns, 1);
            assert!(!state.transport_open);
            // Buffered completion must not rewrite even a timeout whose tree
            // termination failed. The process owner remains needed for retry.
            assert!(
                state
                    .machine
                    .receive(&serde_json::json!({
                        "jsonrpc": "2.0", "method": "session.event", "params": {
                            "sessionId": "cleanup_failure_test", "event": {
                                "type": "turn/end", "seq": 1, "time": 1,
                                "data": {"turn": 1, "reason": {"kind": "completed"}}
                            }
                        }
                    }))
                    .is_err()
            );
            assert_eq!(state.machine.summary().completed_turns, 0);
        }
        assert_eq!(
            driver.await_turn().await,
            Err(DshDriverError::ProcessCleanup)
        );
        assert_eq!(
            driver.close_with(|_| Err(containment_failure())).await,
            Err(DshDriverError::ProcessCleanup)
        );
        assert!(!driver.closed);
        #[cfg(unix)]
        assert!(
            lock(&driver.state)
                .tree
                .leader_exited()
                .expect("unreaped leader")
        );
        driver.close().await.expect("retry cleanup");
        assert!(driver.closed);
        assert_eq!(driver.summary().unverified_turns, 1);
        assert_eq!(driver.summary().completed_turns, 0);
    }

    #[tokio::test]
    async fn a_stale_timer_rechecks_the_current_turn_and_shutdown_phase() {
        let mut driver = exited_driver_with_ready_machine().await;
        {
            let mut state = lock(&driver.state);
            let now = Instant::now();
            let prompt = state.machine.prompt("first turn").expect("prompt");
            let DshAction::Write(frame) = &prompt[0] else {
                panic!("prompt must write a frame");
            };
            state
                .machine
                .receive(&serde_json::json!({
                    "jsonrpc": "2.0", "id": frame["id"],
                    "result": {"messageId": "first"}
                }))
                .expect("receipt");
            state
                .machine
                .receive(&serde_json::json!({
                    "jsonrpc": "2.0", "method": "session.event", "params": {
                        "sessionId": "cleanup_failure_test", "event": {
                            "type": "turn/end", "seq": 1, "time": 1,
                            "data": {"turn": 1, "reason": {"kind": "completed"}}
                        }
                    }
                }))
                .expect("completed turn");
            state.machine.prompt("second turn").expect("prompt");
            state.turn_expiry = Some(now + Duration::from_secs(2));
            assert!(!state.expire_turn_with(now + Duration::from_secs(1), |_| {
                panic!("an old timer must not terminate the next turn")
            }));
            assert_eq!(state.machine.phase(), DshPhase::Prompting);
            state.machine.begin_close().expect("close");
            assert!(!state.expire_turn_with(now + Duration::from_secs(3), |_| {
                panic!("a closing session is no longer subject to its turn deadline")
            }));
            assert_eq!(state.turn_expiry, None);
            assert_eq!(state.machine.phase(), DshPhase::Closing);
            assert_eq!(state.machine.summary().completed_turns, 1);
            assert_eq!(state.machine.summary().cancelled_turns, 1);
            assert_eq!(state.machine.summary().unverified_turns, 0);
        }
        driver.close().await.expect("cleanup");
    }

    #[tokio::test]
    async fn expired_io_cannot_replace_the_turn_timeout_with_pipe_closure() {
        for eof in [false, true] {
            let mut driver = exited_driver_with_ready_machine().await;
            {
                let mut state = lock(&driver.state);
                state.machine.prompt("already overdue").expect("prompt");
                state.turn_expiry = Some(Instant::now());
            }
            if eof {
                finish_transport(
                    &driver.state,
                    &driver.phase_sender,
                    DshDriverError::TransportClosed,
                )
                .await;
            } else {
                assert_eq!(
                    driver.write_failed(&Error::from(ErrorKind::BrokenPipe)),
                    DshDriverError::TurnTimeout
                );
            }
            let summary = driver.summary();
            let error = driver.transport_error();
            driver.close().await.expect("cleanup");
            assert_eq!(summary.phase, DshPhase::Failed, "eof={eof}");
            assert_eq!(summary.unverified_turns, 1, "eof={eof}");
            assert_eq!(summary.completed_turns, 0, "eof={eof}");
            assert_eq!(error, Some(DshDriverError::TurnTimeout), "eof={eof}");
        }
    }

    #[tokio::test]
    async fn a_late_cancel_or_close_cannot_beat_an_elapsed_turn_deadline() {
        for cancel in [false, true] {
            let mut driver = exited_driver_with_ready_machine().await;
            {
                let mut state = lock(&driver.state);
                state.machine.prompt("already overdue").expect("prompt");
                // No lifecycle task can win this race: it has been joined.
                // The operation must adjudicate the absolute deadline itself.
                state.turn_expiry = Some(Instant::now());
            }
            if cancel {
                driver.cancel().await.expect("cancel");
            } else {
                driver.close().await.expect("close");
            }
            let summary = driver.summary();
            let error = driver.transport_error();
            driver.close().await.expect("cleanup");
            assert_eq!(summary.phase, DshPhase::Failed, "cancel={cancel}");
            assert_eq!(summary.unverified_turns, 1, "cancel={cancel}");
            assert_eq!(summary.cancelled_turns, 0, "cancel={cancel}");
            assert_eq!(summary.completed_turns, 0, "cancel={cancel}");
            assert_eq!(error, Some(DshDriverError::TurnTimeout), "cancel={cancel}");
        }
    }

    #[tokio::test]
    async fn a_ready_writer_cannot_hide_the_watchdogs_cleanup_failure() {
        let mut driver = exited_driver_with_ready_machine().await;
        {
            let mut state = lock(&driver.state);
            state.machine.prompt("deadline").expect("prompt");
            state.turn_expiry = Some(Instant::now());
            assert!(state.expire_turn_with(Instant::now(), |_| Err(containment_failure())));
        }
        // Tokio polls the inner write before its timeout. An already-ready
        // write/flush must still report the watchdog's authoritative failure.
        let result = driver
            .write_until(&[], Instant::now(), DshDriverError::TurnTimeout)
            .await;
        let timeout = driver.write_timed_out(DshDriverError::TurnTimeout);
        let broken_pipe = driver.write_failed(&Error::from(ErrorKind::BrokenPipe));
        let cancelled = driver.cancel().await;
        {
            let mut state = lock(&driver.state);
            assert_eq!(
                state.fail_transport_with(DshDriverError::TurnTimeout, |_| {
                    panic!("only close may retry an earlier cleanup failure")
                }),
                DshDriverError::ProcessCleanup
            );
        }
        let error = driver.transport_error();
        driver.close().await.expect("cleanup retry");
        // A retained historical failure must not poison an already-verified
        // teardown or make a later cancellation signal anything again.
        driver.cancel().await.expect("already-cleaned cancellation");
        assert_eq!(result, Err(DshDriverError::ProcessCleanup));
        assert_eq!(timeout, DshDriverError::ProcessCleanup);
        assert_eq!(broken_pipe, DshDriverError::ProcessCleanup);
        assert_eq!(cancelled, Err(DshDriverError::ProcessCleanup));
        assert_eq!(error, Some(DshDriverError::ProcessCleanup));
        assert_eq!(driver.summary().unverified_turns, 1);
    }

    #[tokio::test]
    async fn a_close_retry_reaches_cleanup_after_a_previous_cleanup_error() {
        let mut driver = exited_driver_with_ready_machine().await;
        {
            let mut state = lock(&driver.state);
            // A shutdown was delivered and the peer exited without a reply,
            // but termination of the owned tree failed before close could
            // clear stdin/reap. Its next write will encounter BrokenPipe.
            state.machine.begin_close().expect("closing");
            state.transport_open = false;
            state.transport_error = Some(DshDriverError::ProcessCleanup);
        }
        // Exercise the closed-pipe path even if startup already consumed the
        // fixture's EOF and cleared stdin before the artificial Closing state.
        assert_eq!(
            driver.write_failed(&Error::from(ErrorKind::BrokenPipe)),
            DshDriverError::TransportClosed
        );
        let mut retried = false;
        driver
            .close_with(|tree| {
                retried = true;
                tree.terminate()
            })
            .await
            .expect("retry must reach termination and reap");
        assert!(retried);
        assert!(driver.closed);
        assert_eq!(driver.phase(), DshPhase::Closed);
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
