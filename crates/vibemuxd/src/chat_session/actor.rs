//! A single process-local actor. No registry lock crosses an await or join.
use tokio::{
    sync::{mpsc, watch},
    time::{Instant, sleep_until, timeout},
};
use vibemux_harness::chat_session::{
    AssistantText, MAX_INPUT_CHUNK_BYTES, SessionDecoder, SessionEffect, SessionEvent,
    SessionMachine, SessionPhase, SessionSnapshot, SessionTransition, TurnPhase, TurnSummary,
};

use super::{
    approval_runtime::{ActiveApproval, ApprovalOutcome, ApprovalRuntime},
    contracts::*,
    input_writer::{InputState, InputWriteOutcome, InputWriter},
    transport::{NativeTransport, RuntimeTransport},
};
use crate::harness_dispatch::native_process::{NativeProcess, resolve_environment};

#[path = "approval_actor.rs"]
mod approval_outcomes;

const PROCESS_POLL: std::time::Duration = std::time::Duration::from_millis(100);

pub(crate) fn spawn_runtime(plan: RuntimePlan, sink: Sink) -> Result<RuntimeOwner, RuntimeError> {
    let machine = SessionMachine::new(plan.protocol.clone(), plan.protocol_limits.clone())?;
    spawn_runtime_with_machine(plan, sink, machine, LaunchCheck::Standard)
}
pub(super) enum LaunchCheck {
    Standard,
    #[cfg(feature = "test_helpers")]
    Fixture(Box<super::fixture_runtime::FixtureSpawnCheck>),
}
pub(super) fn spawn_runtime_with_machine(
    plan: RuntimePlan,
    sink: Sink,
    machine: SessionMachine,
    check: LaunchCheck,
) -> Result<RuntimeOwner, RuntimeError> {
    let fixture_check = match &check {
        LaunchCheck::Standard => false,
        #[cfg(feature = "test_helpers")]
        LaunchCheck::Fixture(_) => true,
    };
    if machine.fixture_file_approval_enabled() != fixture_check {
        return Err(RuntimeError::InvalidConfiguration);
    }
    plan.limits.validate()?;
    if plan.binding.generation == 0 {
        return Err(RuntimeError::InvalidConfiguration);
    }
    let decoder = SessionDecoder::new(&plan.protocol_limits)?;
    let (sender, commands) = mpsc::channel(RUNTIME_INBOX_CAPACITY);
    let (close, closing) = watch::channel(None);
    let (snapshots, receiver) = watch::channel(machine.snapshot());
    let (approval_updates, approvals) = watch::channel(None);
    let handle = RuntimeHandle {
        sender,
        close,
        snapshots: receiver,
        approvals,
    };
    let shutdown_limit = plan.limits.shutdown;
    let task = tokio::spawn(async move {
        let launch_valid = match check {
            LaunchCheck::Standard => true,
            #[cfg(feature = "test_helpers")]
            LaunchCheck::Fixture(check) => {
                let launch = crate::harness_dispatch::native_process::LaunchPlan {
                    trampoline: plan.launch.trampoline.clone(),
                    executable: plan.launch.executable.clone(),
                    working_directory: plan.launch.working_directory.clone(),
                    spec: plan.launch.spec.clone(),
                };
                matches!(
                    tokio::task::spawn_blocking(move || check.verify(&launch)).await,
                    Ok(Ok(()))
                )
            }
        };
        if !launch_valid {
            return startup_failed(plan.binding, machine, plan.limits, sink, snapshots).await;
        }
        let environment = resolve_environment(&plan.launch.spec.environment_names);
        let process = match environment {
            Ok(environment) => timeout(
                plan.limits.initialize,
                NativeProcess::spawn(&plan.launch, environment),
            )
            .await
            .map_err(|_| RuntimeError::SpawnFailed)
            .and_then(|result| result.map_err(|_| RuntimeError::SpawnFailed)),
            Err(_) => Err(RuntimeError::SpawnFailed),
        };
        match process {
            Ok(process) => {
                run_transport(
                    plan.binding,
                    machine,
                    decoder,
                    plan.limits,
                    sink,
                    NativeTransport(process),
                    commands,
                    closing,
                    snapshots,
                    approval_updates,
                )
                .await
            }
            Err(_) => startup_failed(plan.binding, machine, plan.limits, sink, snapshots).await,
        }
    });
    Ok(RuntimeOwner {
        handle,
        task,
        shutdown_limit,
    })
}

async fn startup_failed(
    binding: RuntimeBinding,
    mut machine: SessionMachine,
    limits: RuntimeLimits,
    sink: Sink,
    snapshots: watch::Sender<SessionSnapshot>,
) -> RuntimeExit {
    machine.transport_lost();
    let snapshot = machine.snapshot();
    snapshots.send_replace(snapshot.clone());
    let mut cleanup = RuntimeCleanup::not_started();
    // NativeProcess startup can fail after creating a child. Its existing error
    // API has no positive cleanup receipt, so a restart needs reconciliation.
    cleanup.root_reaped = false;
    cleanup.tree_empty = false;
    cleanup.cleanup_verified = false;
    let confirmed = matches!(
        timeout(
            limits.callback,
            sink.event(
                &binding,
                RuntimeEvent::TransportEnded {
                    snapshot: snapshot.clone(),
                    pending_turn: None,
                    reason: RuntimeStopReason::StartupFailed,
                    cleanup,
                }
            )
        )
        .await,
        Ok(Ok(()))
    );
    RuntimeExit {
        reason: RuntimeStopReason::StartupFailed,
        snapshot,
        pending_turn: None,
        cleanup,
        writer_confirmed: confirmed,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_transport<T: RuntimeTransport>(
    binding: RuntimeBinding,
    machine: SessionMachine,
    decoder: SessionDecoder,
    limits: RuntimeLimits,
    sink: Sink,
    mut transport: T,
    commands: mpsc::Receiver<RuntimeCommand>,
    closing: watch::Receiver<Option<RuntimeStopReason>>,
    snapshots: watch::Sender<SessionSnapshot>,
    approval_updates: watch::Sender<Option<vibemux_types::chat_approval::ChatApproval>>,
) -> RuntimeExit {
    let Some(stdin) = transport.take_stdin() else {
        return startup_failed(binding, machine, limits, sink, snapshots).await;
    };
    let now = Instant::now();
    let mut actor = Actor {
        binding,
        machine,
        decoder,
        limits,
        sink,
        transport,
        input: InputWriter::start(stdin),
        commands,
        closing,
        snapshots,
        sequence: 0,
        initialized: false,
        last_activity: now,
        initialize_deadline: now,
        turn_deadline: None,
        cancel_deadline: None,
        close_deadline: None,
        close_reason: None,
        pending_turn: None,
        output_complete: true,
        tool_capture_retired: false,
        approvals: ApprovalRuntime::default(),
        approval_updates,
        deferred_text: None,
        deferred_finish: None,
    };
    actor.initialize_deadline = now + actor.limits.initialize;
    actor.run().await
}

struct Actor<T> {
    binding: RuntimeBinding,
    machine: SessionMachine,
    decoder: SessionDecoder,
    limits: RuntimeLimits,
    sink: Sink,
    transport: T,
    input: InputWriter,
    commands: mpsc::Receiver<RuntimeCommand>,
    closing: watch::Receiver<Option<RuntimeStopReason>>,
    snapshots: watch::Sender<SessionSnapshot>,
    sequence: u64,
    initialized: bool,
    last_activity: Instant,
    initialize_deadline: Instant,
    turn_deadline: Option<Instant>,
    cancel_deadline: Option<Instant>,
    close_deadline: Option<Instant>,
    close_reason: Option<RuntimeStopReason>,
    pending_turn: Option<TurnSummary>,
    output_complete: bool,
    tool_capture_retired: bool,
    approvals: ApprovalRuntime,
    approval_updates: watch::Sender<Option<vibemux_types::chat_approval::ChatApproval>>,
    deferred_text: Option<AssistantText>,
    deferred_finish: Option<(TurnSummary, SessionSnapshot, bool)>,
}

enum Action {
    Close,
    Command(Option<RuntimeCommand>),
    Read(std::io::Result<usize>),
    Input,
    Timer,
    Approval(ApprovalOutcome),
}

fn failure_reason(error: RuntimeError) -> RuntimeStopReason {
    match error {
        RuntimeError::InputFailed => RuntimeStopReason::InputFailed,
        RuntimeError::Protocol(_) => RuntimeStopReason::ProtocolViolation,
        _ => RuntimeStopReason::WriterUnavailable,
    }
}

impl<T: RuntimeTransport> Actor<T> {
    async fn run(&mut self) -> RuntimeExit {
        let requested = *self.closing.borrow();
        let reason = if let Some(reason) = requested {
            let _ = self.machine.close();
            reason
        } else {
            let started = self.machine.start().map_err(RuntimeError::from);
            if let Ok(transition) = started {
                if self.apply(transition).await.is_err() {
                    RuntimeStopReason::WriterUnavailable
                } else {
                    self.exchange().await
                }
            } else {
                RuntimeStopReason::ProtocolViolation
            }
        };
        // Close stdin before ending the exact contained process. No final send
        // can block on a full queue, and no caller joins while owning a receiver.
        self.approvals.abort();
        self.approval_updates.send_replace(None);
        self.input.close();
        self.input.task_abort_and_join().await;
        let force = !matches!(
            reason,
            RuntimeStopReason::Closed | RuntimeStopReason::Idle | RuntimeStopReason::Shutdown
        );
        let cleanup = self
            .transport
            .finish(force, Instant::now() + self.limits.cancel_grace)
            .await;
        if cleanup.cleanup_verified {
            self.machine.process_exited();
        } else {
            self.machine.transport_lost();
        }
        let snapshot = self.machine.snapshot();
        self.snapshots.send_replace(snapshot.clone());
        let writer_confirmed = self
            .emit(RuntimeEvent::TransportEnded {
                snapshot: snapshot.clone(),
                pending_turn: self.pending_turn.clone(),
                reason,
                cleanup,
            })
            .await
            .is_ok();
        // The protocol may already have marked an active turn uncertain; a
        // verified process exit still does not synthesize a successful turn.
        RuntimeExit {
            reason,
            snapshot,
            pending_turn: self.pending_turn.clone(),
            cleanup,
            writer_confirmed,
        }
    }

    async fn exchange(&mut self) -> RuntimeStopReason {
        let mut buffer = vec![0_u8; MAX_INPUT_CHUNK_BYTES];
        let mut input_closed = false;
        let mut commands_closed = false;
        loop {
            match self.transport.root_exited() {
                Ok(false) => {}
                Ok(true) | Err(_) => {
                    return self.close_reason.unwrap_or(RuntimeStopReason::ProcessLost);
                }
            }
            self.snapshots.send_replace(self.machine.snapshot());
            let deadline = self.next_deadline().min(Instant::now() + PROCESS_POLL);
            let requested = *self.closing.borrow();
            let action = if self.close_reason.is_none() && requested.is_some() {
                Action::Close
            } else if Instant::now() >= deadline {
                Action::Timer
            } else {
                tokio::select! {
                    _ = self.closing.changed(), if self.close_reason.is_none() => Action::Close,
                    command = self.commands.recv(), if !commands_closed => Action::Command(command),
                    outcome = self.approvals.next(), if self.approvals.pending() => Action::Approval(outcome),
                    _ = self.input.status.changed(), if !input_closed => Action::Input,
                    read = self.transport.read(&mut buffer) => Action::Read(read),
                    () = sleep_until(deadline) => Action::Timer,
                }
            };
            match action {
                Action::Close => {
                    let reason = self.closing.borrow().unwrap_or(RuntimeStopReason::Shutdown);
                    if self.begin_close(reason).await.is_err() {
                        return RuntimeStopReason::WriterUnavailable;
                    }
                }
                Action::Command(None) => {
                    if self.begin_close(RuntimeStopReason::Shutdown).await.is_err() {
                        return RuntimeStopReason::WriterUnavailable;
                    }
                    self.commands.close();
                    commands_closed = true;
                }
                Action::Command(Some(command)) => {
                    if let Err(reason) = self.command(command).await {
                        return reason;
                    }
                }
                Action::Approval(outcome) => {
                    if let Err(reason) = self.approval_outcome(outcome).await {
                        return reason;
                    }
                }
                Action::Input => match *self.input.status.borrow() {
                    InputState::Failed => return RuntimeStopReason::InputFailed,
                    InputState::Closed => input_closed = true,
                    InputState::Open => {}
                },
                Action::Read(Err(_)) => return RuntimeStopReason::ProcessLost,
                Action::Read(Ok(0)) => {
                    match self.decoder.finish() {
                        Ok(Some(frame)) => {
                            if self.frame(frame.as_bytes()).await.is_err() {
                                return RuntimeStopReason::ProtocolViolation;
                            }
                        }
                        Ok(None) => {}
                        Err(_) => return RuntimeStopReason::ProtocolViolation,
                    }
                    return self.close_reason.unwrap_or(RuntimeStopReason::ProcessLost);
                }
                Action::Read(Ok(count)) => {
                    let frames = match self.decoder.push(&buffer[..count]) {
                        Ok(frames) => frames,
                        Err(_) => return RuntimeStopReason::ProtocolViolation,
                    };
                    for frame in frames {
                        if let Err(error) = self.frame(frame.as_bytes()).await {
                            return failure_reason(error);
                        }
                        self.last_activity = Instant::now();
                        let requested = *self.closing.borrow();
                        if self.close_reason.is_none()
                            && let Some(reason) = requested
                            && self.begin_close(reason).await.is_err()
                        {
                            return RuntimeStopReason::WriterUnavailable;
                        }
                        let now = Instant::now();
                        if self.close_deadline.is_some_and(|deadline| now >= deadline)
                            || self.cancel_deadline.is_some_and(|deadline| now >= deadline)
                        {
                            return self
                                .close_reason
                                .unwrap_or(RuntimeStopReason::CancellationUnconfirmed);
                        }
                        if self.turn_deadline.is_some_and(|deadline| now >= deadline)
                            && self.begin_close(RuntimeStopReason::Deadline).await.is_err()
                        {
                            return RuntimeStopReason::WriterUnavailable;
                        }
                    }
                }
                Action::Timer => {
                    let now = Instant::now();
                    if let Some(deadline) = self.close_deadline {
                        if now >= deadline {
                            return self.close_reason.unwrap_or(RuntimeStopReason::Closed);
                        }
                        // Initialization, turn and idle clocks are retired once
                        // closing starts; a poll must yield back to I/O here.
                        continue;
                    }
                    if self.cancel_deadline.is_some_and(|deadline| now >= deadline) {
                        return self
                            .close_reason
                            .unwrap_or(RuntimeStopReason::CancellationUnconfirmed);
                    }
                    if !self.initialized && now >= self.initialize_deadline {
                        return RuntimeStopReason::StartupFailed;
                    }
                    if self
                        .approvals
                        .deadline()
                        .is_some_and(|deadline| now >= deadline)
                    {
                        if let Some(active) = self.approvals.active.as_ref() {
                            let id = active.record.approval_id;
                            self.approvals
                                .expire(self.sink.clone(), self.binding.clone(), id);
                            self.approval_updates.send_replace(None);
                        }
                        if self.begin_close(RuntimeStopReason::Deadline).await.is_err() {
                            return RuntimeStopReason::WriterUnavailable;
                        }
                        continue;
                    }
                    if self.turn_deadline.is_some_and(|deadline| now >= deadline) {
                        if self.begin_close(RuntimeStopReason::Deadline).await.is_err() {
                            return RuntimeStopReason::WriterUnavailable;
                        }
                    } else if (self.machine.snapshot().active_turn.is_none()
                        || self.machine.fixture_file_approval_enabled())
                        && !self.approvals.human_waiting()
                        && now >= self.last_activity + self.limits.idle
                        && self.begin_close(RuntimeStopReason::Idle).await.is_err()
                    {
                        return RuntimeStopReason::WriterUnavailable;
                    }
                }
            }
            if !self.approvals.pending() && self.drain_deferred().await.is_err() {
                return RuntimeStopReason::WriterUnavailable;
            }
        }
    }

    fn next_deadline(&self) -> Instant {
        if let Some(deadline) = self.close_deadline {
            return deadline;
        }
        let initial = if !self.initialized {
            self.initialize_deadline
        } else if let Some(deadline) = self.turn_deadline {
            deadline
        } else {
            self.last_activity + self.limits.idle
        };
        let initial = if self.machine.fixture_file_approval_enabled()
            && self.initialized
            && !self.approvals.human_waiting()
        {
            initial.min(self.last_activity + self.limits.idle)
        } else {
            initial
        };
        let initial = self
            .approvals
            .deadline()
            .map_or(initial, |deadline| initial.min(deadline));
        self.cancel_deadline
            .map_or(initial, |deadline| initial.min(deadline))
    }

    async fn begin_close(&mut self, reason: RuntimeStopReason) -> Result<(), RuntimeError> {
        if self.close_reason.is_some() {
            return Ok(());
        }
        if reason == RuntimeStopReason::Deadline
            && self
                .approvals
                .deadline()
                .is_some_and(|d| Instant::now() >= d)
            && let Some(active) = self.approvals.active.as_ref()
        {
            let id = active.record.approval_id;
            self.approvals
                .expire(self.sink.clone(), self.binding.clone(), id);
            self.approval_updates.send_replace(None);
        }
        self.close_reason = Some(reason);
        let deadline = Instant::now() + self.limits.cancel_grace;
        self.close_deadline = Some(
            self.cancel_deadline
                .map_or(deadline, |cancel| cancel.min(deadline)),
        );
        self.turn_deadline = None;
        let transition = self.machine.close()?;
        self.apply(transition).await
    }

    async fn command(&mut self, command: RuntimeCommand) -> Result<(), RuntimeStopReason> {
        match command {
            RuntimeCommand::Send {
                turn,
                deadline,
                response,
            } => {
                if self.close_reason.is_some()
                    || self.machine.snapshot().phase != SessionPhase::Ready
                {
                    let _ = response.send(Err(RuntimeError::NotReady));
                    return Ok(());
                }
                if let Err(error) = validate_turn(&turn, deadline) {
                    let _ = response.send(Err(error));
                    return Ok(());
                }
                self.pending_turn = Some(TurnSummary {
                    request_id: turn.request_id,
                    turn_id: turn.turn_id,
                    vendor_turn_id: None,
                    phase: TurnPhase::Uncertain,
                    outcome: None,
                    acknowledged: false,
                    cancel_requested: false,
                    prompt_bytes: turn.prompt.len(),
                });
                let claim = timeout(
                    self.limits.callback,
                    self.sink.claim_turn(&self.binding, &turn),
                )
                .await;
                if !matches!(claim, Ok(Ok(()))) {
                    let _ = response.send(Err(RuntimeError::ClaimRefused));
                    return Err(RuntimeStopReason::WriterUnavailable);
                }
                self.turn_deadline = Some(Instant::now() + deadline);
                self.cancel_deadline = None;
                self.output_complete = true;
                self.tool_capture_retired = false;
                let transition = match self.machine.submit(turn) {
                    Ok(transition) => transition,
                    Err(error) => {
                        let _ = response.send(Err(error.into()));
                        return Err(RuntimeStopReason::ProtocolViolation);
                    }
                };
                self.pending_turn = None;
                let result = self.apply(transition).await;
                let failure = result.as_ref().err().copied().map(failure_reason);
                let _ = response.send(result);
                if let Some(reason) = failure {
                    return Err(reason);
                }
            }
            RuntimeCommand::Cancel { turn_id, response } => {
                let active = self.machine.snapshot().active_turn;
                if active
                    .as_ref()
                    .is_none_or(|active| active.turn_id != turn_id)
                {
                    let _ = response.send(Err(RuntimeError::WrongTurn));
                    return Ok(());
                }
                let active = match active {
                    Some(active) => active,
                    None => {
                        let _ = response.send(Err(RuntimeError::WrongTurn));
                        return Ok(());
                    }
                };
                let result = match self.machine.cancel(turn_id) {
                    Ok(transition) => self.apply(transition).await,
                    Err(error) => Err(error.into()),
                };
                // Duplicate cancel requests do not renew the original grace.
                self.cancel_deadline
                    .get_or_insert(Instant::now() + self.limits.cancel_grace);
                let failed = result.is_err();
                if failed {
                    let _ = response.send(result);
                    return Err(RuntimeStopReason::ProtocolViolation);
                }
                self.approval_updates.send_replace(None);
                self.approvals
                    .cancel(self.sink.clone(), self.binding.clone(), active, response);
            }
            RuntimeCommand::DecideApproval { request, response } => {
                if self.close_reason.is_some()
                    || self
                        .machine
                        .snapshot()
                        .active_turn
                        .as_ref()
                        .is_none_or(|t| t.cancel_requested)
                {
                    let _ = response.send(Err(RuntimeError::NotReady));
                    return Ok(());
                }
                if self
                    .approvals
                    .deadline()
                    .is_none_or(|deadline| Instant::now() >= deadline)
                    || self
                        .turn_deadline
                        .is_none_or(|deadline| Instant::now() >= deadline)
                {
                    let _ = response.send(Err(RuntimeError::NotReady));
                    return Ok(());
                }
                if self.approvals.operation_pending() {
                    let _ = response.send(Err(RuntimeError::NotReady));
                    return Ok(());
                }
                let Some(active) = self.approvals.active.as_ref() else {
                    let _ = response.send(Err(RuntimeError::WrongTurn));
                    return Ok(());
                };
                if active.record.approval_id != request.approval_id
                    || active.record.version != request.expected_version
                {
                    let _ = response.send(Err(RuntimeError::NotReady));
                    return Ok(());
                }
                let frame = match self
                    .machine
                    .respond_to_approval(&active.challenge, request.decision)
                {
                    Ok(frame) => frame,
                    Err(error) => {
                        let _ = response.send(Err(error.into()));
                        return Ok(());
                    }
                };
                if let Some(active) = self.approvals.active.as_mut() {
                    active.prepared_frame = Some(frame);
                }
                self.approvals
                    .decide(self.sink.clone(), self.binding.clone(), request, response)
                    .map_err(failure_reason)?;
            }
        }
        self.last_activity = Instant::now();
        self.snapshots.send_replace(self.machine.snapshot());
        Ok(())
    }

    async fn frame(&mut self, frame: &[u8]) -> Result<(), RuntimeError> {
        self.sequence = self.sequence.checked_add(1).ok_or(RuntimeError::Protocol(
            vibemux_harness::chat_session::SessionError::RecordBudgetExhausted,
        ))?;
        let transition = self.machine.receive(self.sequence, frame)?;
        self.apply(transition).await
    }

    async fn apply(&mut self, transition: SessionTransition) -> Result<(), RuntimeError> {
        self.output_complete &= !transition.tool_capture_incomplete;
        let snapshot = self.machine.snapshot();
        for event in &transition.events {
            match event {
                SessionEvent::SessionBound { vendor_session_id } => {
                    self.emit(RuntimeEvent::VendorBound {
                        vendor_session_id: vendor_session_id.clone(),
                    })
                    .await?
                }
                SessionEvent::TurnAcknowledged { turn } => {
                    self.emit(RuntimeEvent::TurnAcknowledged(turn.clone()))
                        .await?
                }
                _ => {}
            }
        }
        // Correlate optional progress even on a transition that already moved
        // the pure machine to Ready. Never discard its terminal tools/finish.
        if !transition.command_progress.is_empty() || transition.command_progress_limited {
            let identity = transition
                .command_progress
                .first()
                .map(|update| (update.request_id, update.turn_id))
                .or_else(|| {
                    snapshot
                        .active_turn
                        .as_ref()
                        .map(|turn| (turn.request_id, turn.turn_id))
                })
                .or_else(|| {
                    transition.events.iter().find_map(|event| match event {
                        SessionEvent::TurnFinished { turn } => {
                            Some((turn.request_id, turn.turn_id))
                        }
                        _ => None,
                    })
                });
            if let Some((request_id, turn_id)) = identity {
                // No await, callback timeout, retry queue, or completeness
                // mutation: this path is replaceable private display only.
                let _ = self.sink.try_command_progress(
                    &self.binding,
                    RuntimeCommandProgress {
                        request_id,
                        turn_id,
                        updates: transition.command_progress,
                        limited: transition.command_progress_limited,
                    },
                );
            }
        }
        let tool_deadline = Instant::now() + self.limits.callback;
        for tool in transition.tools {
            if self.tool_capture_retired {
                self.output_complete = false;
                break;
            }
            // Optional display capture shares one absolute callback budget for
            // this batch. Retire it after uncertainty; a later item must not
            // renew that delay or discard an already correlated terminal.
            match tokio::time::timeout_at(
                tool_deadline,
                self.sink
                    .event(&self.binding, RuntimeEvent::ToolCompleted(tool)),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(RuntimeError::ContentUnavailable)) | Err(_) => {
                    self.output_complete = false;
                    self.tool_capture_retired = true;
                    break;
                }
                Ok(Err(error)) => return Err(error),
            }
        }
        for text in transition.text {
            if self.approvals.pending() {
                self.deferred_text = Some(text);
                continue;
            }
            match self.emit(RuntimeEvent::AssistantSnapshot(text)).await {
                Ok(()) => {}
                Err(RuntimeError::ContentUnavailable) => self.output_complete = false,
                Err(error) => return Err(error),
            }
        }
        for event in transition.events {
            if let SessionEvent::TurnFinished { turn } = event {
                if self.approvals.pending() || self.approvals.active.is_some() {
                    self.deferred_finish = Some((turn, snapshot.clone(), self.output_complete));
                    continue;
                }
                self.emit(RuntimeEvent::TurnFinished {
                    turn,
                    snapshot: snapshot.clone(),
                    output_complete: self.output_complete,
                })
                .await?;
                self.turn_deadline = None;
                self.cancel_deadline = None;
                self.last_activity = Instant::now();
                self.approval_updates.send_replace(None);
                self.finish_fresh_process()?;
            }
        }
        for effect in transition.effects {
            match effect {
                SessionEffect::Write(frame) => self.input.send(frame.as_bytes())?,
                SessionEffect::WriteTurn {
                    request_id,
                    turn_id,
                    frame,
                } => {
                    let receipt = self.input.send_tracked(frame.as_bytes())?;
                    self.approvals.input(request_id, turn_id, receipt)?;
                }
                SessionEffect::CloseInput => self.input.close(),
            }
        }
        for challenge in transition.approvals {
            let now = Instant::now();
            let Some(turn_deadline) = self.turn_deadline else {
                return Err(RuntimeError::WrongTurn);
            };
            let deadline = turn_deadline.min(
                now + std::time::Duration::from_secs(
                    vibemux_types::chat_approval::MAX_CHAT_APPROVAL_SECONDS as u64,
                ),
            );
            if deadline <= now || self.close_reason.is_some() {
                return Err(RuntimeError::NotReady);
            }
            let remaining = time::Duration::try_from(deadline - now)
                .map_err(|_| RuntimeError::InvalidConfiguration)?;
            let expires_at = time::OffsetDateTime::now_utc()
                .checked_add(remaining)
                .ok_or(RuntimeError::InvalidConfiguration)?;
            self.approvals.admit(
                self.sink.clone(),
                self.binding.clone(),
                challenge,
                expires_at,
                deadline,
            )?;
        }
        if !self.initialized && snapshot.phase == SessionPhase::Ready {
            self.emit(RuntimeEvent::Ready(snapshot)).await?;
            self.initialized = true;
            self.last_activity = Instant::now();
        }
        self.snapshots.send_replace(self.machine.snapshot());
        Ok(())
    }

    async fn drain_deferred(&mut self) -> Result<(), RuntimeError> {
        if let Some(text) = self.deferred_text.take() {
            match self.emit(RuntimeEvent::AssistantSnapshot(text)).await {
                Ok(()) => {}
                Err(RuntimeError::ContentUnavailable) => self.output_complete = false,
                Err(error) => return Err(error),
            }
        }
        if self.approvals.active.is_none()
            && let Some((turn, snapshot, output_complete)) = self.deferred_finish.take()
        {
            self.emit(RuntimeEvent::TurnFinished {
                turn,
                snapshot,
                output_complete: output_complete && self.output_complete,
            })
            .await?;
            self.turn_deadline = None;
            self.cancel_deadline = None;
            self.last_activity = Instant::now();
            self.approval_updates.send_replace(None);
            self.finish_fresh_process()?;
        }
        Ok(())
    }

    fn finish_fresh_process(&mut self) -> Result<(), RuntimeError> {
        if self.machine.fixture_file_approval_enabled() {
            self.approvals.active = None;
            let _ = self.machine.close()?;
            self.input.close();
            self.close_reason.get_or_insert(RuntimeStopReason::Closed);
            self.close_deadline
                .get_or_insert(Instant::now() + self.limits.cancel_grace);
        }
        Ok(())
    }

    async fn emit(&mut self, event: RuntimeEvent) -> Result<(), RuntimeError> {
        timeout(self.limits.callback, self.sink.event(&self.binding, event))
            .await
            .map_err(|_| RuntimeError::WriterUnavailable)?
    }
}
