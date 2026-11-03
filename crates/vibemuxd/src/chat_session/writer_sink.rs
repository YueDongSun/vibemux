//! Maps one owned runtime's protocol facts to the authoritative chat writer.
//! Assistant updates replace bounded private memory; only the final snapshot is
//! published, once, before the turn's canonical outcome commits.

use std::{
    fmt,
    sync::{Mutex, MutexGuard, TryLockError},
};

use async_trait::async_trait;
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_harness::chat_session::{
    AssistantText, SessionPhase, SessionProtocol, SessionSnapshot, ToolObservation, TurnOutcome,
    TurnPhase, TurnRequest, TurnSummary,
};
use vibemux_store::{ChatMutation, ChatTurnFinish};
#[cfg(test)]
use vibemux_types::chat::MAX_CHAT_RECORD_BYTES;
use vibemux_types::chat::{
    ChatFailureCode, ChatHarness, ChatRecordKind, ChatSession, ChatState, ChatTurn,
    ChatTurnOutcome, ChatTurnPhase,
};
use vibemux_types::chat_command_progress::CommandProgressSnapshot;

use super::{
    ChatRuntimeSink, RuntimeBinding, RuntimeCommandProgress, RuntimeError, RuntimeEvent,
    RuntimeStopReason, sink_state::ActiveOutput,
};
use crate::chat_writer::{ChatRecordWrite, ChatWriter};

/// The binding comes only from the daemon's successful owner claim. A sink is
/// never reused for a different chat, generation, or process owner.
pub(crate) struct ChatWriterSink {
    writer: ChatWriter,
    binding: RuntimeBinding,
    state: Mutex<SinkState>,
}

#[derive(Default)]
struct SinkState {
    vendor_id: Option<String>,
    active: Option<ActiveOutput>,
    claiming: bool,
    ended: bool,
}

impl fmt::Debug for ChatWriterSink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatWriterSink")
            .field("chat_id", &self.binding.chat_id)
            .field("generation", &self.binding.generation)
            .finish_non_exhaustive()
    }
}

impl ChatWriterSink {
    pub(crate) fn new(writer: ChatWriter, binding: RuntimeBinding) -> Self {
        Self {
            writer,
            binding,
            state: Mutex::new(SinkState::default()),
        }
    }

    /// Explicit private streaming read. The writer owns terminal history; this
    /// bounded view exists only for the exact currently active request.
    pub(crate) fn live_text(&self, request_id: Uuid) -> Option<AssistantText> {
        let state = self.state.lock().ok()?;
        if state.ended {
            return None;
        }
        state
            .active
            .as_ref()
            .filter(|active| active.request_id == request_id)?
            .latest
            .clone()
    }

    fn state(&self) -> Result<MutexGuard<'_, SinkState>, RuntimeError> {
        self.state
            .lock()
            .map_err(|_| RuntimeError::WriterUnavailable)
    }

    pub(crate) fn command_progress(
        &self,
        request_id: Uuid,
    ) -> Result<Option<CommandProgressSnapshot>, RuntimeError> {
        let copied = {
            let state = self.state()?;
            if state.ended {
                return Ok(None);
            }
            state
                .active
                .as_ref()
                .filter(|active| active.request_id == request_id)
                .and_then(ActiveOutput::command_progress)
        };
        copied.map(|parts| parts.build()).transpose()
    }

    fn check_binding(&self, binding: &RuntimeBinding) -> Result<(), RuntimeError> {
        if binding.chat_id != self.binding.chat_id
            || binding.generation != self.binding.generation
            || binding.owner_fence != self.binding.owner_fence
        {
            return Err(RuntimeError::ClaimRefused);
        }
        Ok(())
    }

    async fn session(&self) -> Result<ChatSession, RuntimeError> {
        let session = self
            .writer
            .session(self.binding.chat_id)
            .await
            .map_err(|_| RuntimeError::WriterUnavailable)?
            .ok_or(RuntimeError::WriterUnavailable)?;
        if session.generation != self.binding.generation {
            return Err(RuntimeError::ClaimRefused);
        }
        Ok(session)
    }

    async fn active_turn(
        &self,
        request_id: Uuid,
        turn_id: Uuid,
        prompt_bytes: Option<usize>,
    ) -> Result<(ChatSession, ChatTurn), RuntimeError> {
        let turn = self
            .writer
            .turn(request_id)
            .await
            .map_err(|_| RuntimeError::WriterUnavailable)?
            .ok_or(RuntimeError::WrongTurn)?;
        if turn.chat_id != self.binding.chat_id
            || turn.generation != self.binding.generation
            || *turn.turn_id.as_uuid() != turn_id
            || prompt_bytes.is_some_and(|bytes| bytes as u64 != turn.prompt_ref.byte_count)
            || !matches!(
                turn.phase,
                ChatTurnPhase::Running | ChatTurnPhase::CancelRequested
            )
        {
            return Err(RuntimeError::WrongTurn);
        }
        let session = self.session().await?;
        if session.active_request_id != Some(request_id) || session.state != ChatState::Busy {
            return Err(RuntimeError::WrongTurn);
        }
        Ok((session, turn))
    }

    async fn acknowledge(&self, turn: TurnSummary) -> Result<(), RuntimeError> {
        if !turn.acknowledged
            || turn.outcome.is_some()
            || !matches!(turn.phase, TurnPhase::Running | TurnPhase::Cancelling)
        {
            return Err(RuntimeError::WrongTurn);
        }
        let (session, _) = self
            .active_turn(turn.request_id, turn.turn_id, Some(turn.prompt_bytes))
            .await?;
        let vendor_turn = vendor_turn_id(session.harness, &turn).ok_or(RuntimeError::WrongTurn)?;
        {
            let state = self.state()?;
            let active = state.active.as_ref().ok_or(RuntimeError::WrongTurn)?;
            if active.request_id != turn.request_id
                || active.turn_id != turn.turn_id
                || active.prompt_bytes != turn.prompt_bytes
                || state.vendor_id.as_ref() != session.vendor_session_id.as_ref()
                || state.vendor_id.is_none()
                || session.vendor_confirmed_generation != Some(self.binding.generation)
                || active
                    .acknowledged_vendor_turn
                    .as_ref()
                    .is_some_and(|known| known != &vendor_turn)
            {
                return Err(RuntimeError::WrongTurn);
            }
        }
        self.writer
            .bind_turn(
                turn.request_id,
                self.binding.generation,
                self.binding.owner_fence,
                vendor_turn.clone(),
            )
            .await
            .map_err(|_| RuntimeError::WriterUnavailable)?;
        let mut state = self.state()?;
        let active = state.active.as_mut().ok_or(RuntimeError::WrongTurn)?;
        if active.request_id != turn.request_id || active.turn_id != turn.turn_id {
            return Err(RuntimeError::WrongTurn);
        }
        active.acknowledged_vendor_turn = Some(vendor_turn);
        Ok(())
    }

    async fn assistant_snapshot(&self, text: AssistantText) -> Result<(), RuntimeError> {
        self.active_turn(text.request_id, text.turn_id, None)
            .await?;
        let mut state = self.state()?;
        let active = state.active.as_mut().ok_or(RuntimeError::WrongTurn)?;
        active.merge_snapshot(text)
    }

    async fn tool_record(&self, observation: ToolObservation) -> Result<(), RuntimeError> {
        let (session, turn) = self
            .active_turn(observation.request_id, observation.turn_id, None)
            .await?;
        if session.harness != ChatHarness::Codex {
            return Err(RuntimeError::WrongTurn);
        }
        let reserved = {
            let mut state = self.state()?;
            let active = state.active.as_mut().ok_or(RuntimeError::WrongTurn)?;
            if active.acknowledged_vendor_turn != turn.vendor_turn_id {
                return Err(RuntimeError::WrongTurn);
            }
            active.reserve_tool(&observation)?
        };
        let Some((record_id, body)) = reserved else {
            return Ok(());
        };
        let persisted = self
            .writer
            .append(ChatRecordWrite {
                request_id: observation.request_id,
                generation: self.binding.generation,
                fence: self.binding.owner_fence,
                record_id,
                kind: ChatRecordKind::Tool,
                original_byte_count: body.len() as u64,
                text: body,
            })
            .await
            .is_ok();
        let mut state = self.state()?;
        let active = state
            .active
            .as_mut()
            .filter(|active| {
                active.request_id == observation.request_id && active.turn_id == observation.turn_id
            })
            .ok_or(RuntimeError::WrongTurn)?;
        active.complete_tool(observation.record.call_id(), record_id, persisted)?;
        if persisted {
            Ok(())
        } else {
            Err(RuntimeError::ContentUnavailable)
        }
    }

    /// No mutex guard survives the append await. A fixed record ID also makes
    /// an unknown append response safe to reconcile without another record.
    async fn flush_output(&self, request_id: Uuid) -> Result<bool, RuntimeError> {
        let pending = {
            let state = self.state()?;
            match state.active.as_ref() {
                Some(active) if active.request_id == request_id => {
                    if active.flushed {
                        return Ok(active.output_available());
                    }
                    Some((
                        active.latest.clone(),
                        active.record_id,
                        active.output_available(),
                    ))
                }
                _ => None,
            }
        };
        let Some((latest, record_id, available)) = pending else {
            return Ok(false);
        };
        let Some(text) = latest else {
            return Ok(available);
        };
        let complete = available && !text.truncated;
        let persisted = if text.text.is_empty() {
            true
        } else {
            self.writer
                .append(ChatRecordWrite {
                    request_id,
                    generation: self.binding.generation,
                    fence: self.binding.owner_fence,
                    record_id,
                    kind: ChatRecordKind::Assistant,
                    original_byte_count: text.original_bytes as u64,
                    text: text.text,
                })
                .await
                .is_ok()
        };
        let mut state = self.state()?;
        if let Some(active) = state
            .active
            .as_mut()
            .filter(|active| active.request_id == request_id)
        {
            active.record_flush(persisted, complete);
        }
        Ok(complete && persisted)
    }

    async fn finish(
        &self,
        turn: TurnSummary,
        snapshot: SessionSnapshot,
        output_complete: bool,
    ) -> Result<(), RuntimeError> {
        let (session, stored) = self
            .active_turn(turn.request_id, turn.turn_id, Some(turn.prompt_bytes))
            .await?;
        let vendor_turn = vendor_turn_id(session.harness, &turn);
        let correlated = {
            let state = self.state()?;
            state.active.as_ref().is_some_and(|active| {
                active.request_id == turn.request_id
                    && active.turn_id == turn.turn_id
                    && active.acknowledged_vendor_turn.is_some()
                    && active.acknowledged_vendor_turn == vendor_turn
                    && stored.vendor_turn_id == vendor_turn
            }) && turn.acknowledged
                && turn.phase == TurnPhase::Finished
                && turn.outcome.is_some()
                && state.vendor_id.is_some()
                && state.vendor_id == session.vendor_session_id
                && snapshot.vendor_session_confirmed
                && state.vendor_id == snapshot.vendor_session_id
                && session.vendor_confirmed_generation == Some(self.binding.generation)
                && protocol_matches(session.harness, snapshot.protocol)
                && snapshot.last_turn.as_ref() == Some(&turn)
                && snapshot.active_turn.is_none()
                && matches!(snapshot.phase, SessionPhase::Ready | SessionPhase::Closing)
        };
        if !correlated {
            return Err(RuntimeError::WriterUnavailable);
        }
        let complete = self.flush_output(turn.request_id).await? && output_complete;
        let (outcome, error_code) = match turn.outcome {
            Some(TurnOutcome::Completed) => (ChatTurnOutcome::Completed, None),
            Some(TurnOutcome::Failed) => {
                (ChatTurnOutcome::Failed, Some(ChatFailureCode::VendorFailed))
            }
            Some(TurnOutcome::Cancelled) => {
                (ChatTurnOutcome::Cancelled, Some(ChatFailureCode::Cancelled))
            }
            None => return Err(RuntimeError::WrongTurn),
        };
        self.writer
            .finish_turn(ChatTurnFinish {
                request_id: turn.request_id,
                generation: self.binding.generation,
                fence: self.binding.owner_fence,
                outcome,
                error_code,
                correlated_terminal: true,
                vendor_session_id: session.vendor_session_id,
                vendor_turn_id: vendor_turn,
                process_alive: true,
                output_complete: complete,
                timestamp: OffsetDateTime::now_utc(),
            })
            .await
            .map_err(|_| RuntimeError::WriterUnavailable)?;
        self.state()?.active = None;
        Ok(())
    }

    async fn transport_ended(
        &self,
        snapshot: SessionSnapshot,
        pending: Option<TurnSummary>,
        reason: RuntimeStopReason,
        verified: bool,
    ) -> Result<(), RuntimeError> {
        self.state()?.ended = true;
        let session = self.session().await?;
        if let Some(request_id) = session.active_request_id {
            let turn = self
                .writer
                .turn(request_id)
                .await
                .map_err(|_| RuntimeError::WriterUnavailable)?
                .ok_or(RuntimeError::WrongTurn)?;
            if turn.chat_id != self.binding.chat_id || turn.generation != self.binding.generation {
                return Err(RuntimeError::WrongTurn);
            }
            // These are identity cross-checks only. Neither a snapshot nor
            // teardown manufactures an acknowledgement or terminal success.
            for observed in [pending.as_ref(), snapshot.active_turn.as_ref()]
                .into_iter()
                .flatten()
            {
                if observed.request_id != request_id || observed.turn_id != *turn.turn_id.as_uuid()
                {
                    return Err(RuntimeError::WrongTurn);
                }
            }
            if turn.phase == ChatTurnPhase::RecoveryRequired {
                self.state()?.active = None;
                return Ok(());
            }
            if turn.phase == ChatTurnPhase::Admitted {
                if !verified {
                    self.writer
                        .owner_lost(
                            self.binding.chat_id,
                            self.binding.generation,
                            self.binding.owner_fence,
                        )
                        .await
                        .map_err(|_| RuntimeError::WriterUnavailable)?;
                    self.state()?.active = None;
                    return Ok(());
                }
                self.writer
                    .cancel_unsent(
                        request_id,
                        self.binding.generation,
                        self.binding.owner_fence,
                    )
                    .await
                    .map_err(|_| RuntimeError::WriterUnavailable)?;
            } else if matches!(
                turn.phase,
                ChatTurnPhase::Running | ChatTurnPhase::CancelRequested
            ) {
                self.flush_output(request_id).await?;
                self.writer
                    .finish_turn(ChatTurnFinish {
                        request_id,
                        generation: self.binding.generation,
                        fence: self.binding.owner_fence,
                        outcome: ChatTurnOutcome::Interrupted,
                        correlated_terminal: false,
                        vendor_session_id: None,
                        vendor_turn_id: None,
                        error_code: Some(stop_failure(reason)),
                        process_alive: !verified,
                        output_complete: false,
                        timestamp: OffsetDateTime::now_utc(),
                    })
                    .await
                    .map_err(|_| RuntimeError::WriterUnavailable)?;
                self.state()?.active = None;
                if verified && reason == RuntimeStopReason::Closed {
                    self.writer
                        .close(self.mutation())
                        .await
                        .map_err(|_| RuntimeError::WriterUnavailable)?;
                }
                // finish_turn already clears the fence on dead or uncertain
                // owners; release_owner cannot follow that transition.
                return Ok(());
            } else {
                return Err(RuntimeError::WrongTurn);
            }
        }
        if !verified {
            if session.state != ChatState::RecoveryRequired {
                self.writer
                    .owner_lost(
                        self.binding.chat_id,
                        self.binding.generation,
                        self.binding.owner_fence,
                    )
                    .await
                    .map_err(|_| RuntimeError::WriterUnavailable)?;
            }
        } else {
            self.writer
                .release_owner(
                    self.mutation(),
                    self.binding.owner_fence,
                    true,
                    reason == RuntimeStopReason::Closed,
                )
                .await
                .map_err(|_| RuntimeError::WriterUnavailable)?;
        }
        self.state()?.active = None;
        Ok(())
    }

    fn mutation(&self) -> ChatMutation {
        ChatMutation {
            operation_id: Uuid::new_v4(),
            chat_id: self.binding.chat_id,
            expected_generation: self.binding.generation,
            timestamp: OffsetDateTime::now_utc(),
        }
    }
}

#[async_trait]
impl ChatRuntimeSink for ChatWriterSink {
    fn try_command_progress(
        &self,
        binding: &RuntimeBinding,
        progress: RuntimeCommandProgress,
    ) -> Result<bool, RuntimeError> {
        self.check_binding(binding)?;
        let mut state = match self.state.try_lock() {
            Ok(state) => state,
            Err(TryLockError::WouldBlock) => return Ok(false),
            Err(TryLockError::Poisoned(_)) => return Err(RuntimeError::WriterUnavailable),
        };
        if state.ended {
            return Err(RuntimeError::Stopped);
        }
        let active = state.active.as_mut().ok_or(RuntimeError::WrongTurn)?;
        active.merge_command_progress(
            progress.request_id,
            progress.turn_id,
            progress.updates,
            progress.limited,
        )?;
        Ok(true)
    }

    async fn claim_turn(
        &self,
        binding: &RuntimeBinding,
        turn: &TurnRequest,
    ) -> Result<(), RuntimeError> {
        self.check_binding(binding)?;
        {
            let mut state = self.state()?;
            if state.ended || state.claiming || state.active.is_some() {
                return Err(RuntimeError::ClaimRefused);
            }
            state.claiming = true;
        }
        let claimed = async {
            let stored = self
                .writer
                .turn(turn.request_id)
                .await
                .map_err(|_| RuntimeError::ClaimRefused)?
                .ok_or(RuntimeError::ClaimRefused)?;
            if stored.chat_id != binding.chat_id
                || stored.generation != binding.generation
                || *stored.turn_id.as_uuid() != turn.turn_id
                || stored.phase != ChatTurnPhase::Admitted
                || stored.prompt_ref.byte_count != turn.prompt.len() as u64
            {
                return Err(RuntimeError::ClaimRefused);
            }
            // prompt() verifies the immutable blob's recorded byte count and
            // fingerprint before returning private bytes. Compare exact text
            // before the durable one-use send claim, never afterward.
            let prompt = self
                .writer
                .prompt(turn.request_id)
                .await
                .map_err(|_| RuntimeError::ClaimRefused)?;
            if prompt != turn.prompt {
                return Err(RuntimeError::ClaimRefused);
            }
            self.writer
                .claim_turn(turn.request_id, binding.generation, binding.owner_fence)
                .await
                .map_err(|_| RuntimeError::ClaimRefused)?;
            Ok(())
        }
        .await;
        let mut state = self.state()?;
        state.claiming = false;
        if claimed.is_ok() {
            state.active = Some(ActiveOutput::new(
                turn.request_id,
                turn.turn_id,
                turn.prompt.len(),
            ));
        }
        claimed
    }

    async fn event(
        &self,
        binding: &RuntimeBinding,
        event: RuntimeEvent,
    ) -> Result<(), RuntimeError> {
        self.check_binding(binding)?;
        if self.state()?.ended {
            return Err(RuntimeError::Stopped);
        }
        match event {
            RuntimeEvent::Ready(snapshot) => {
                if snapshot.phase != SessionPhase::Ready || snapshot.active_turn.is_some() {
                    return Err(RuntimeError::NotReady);
                }
                self.writer
                    .transport_ready(binding.chat_id, binding.generation, binding.owner_fence)
                    .await
                    .map_err(|_| RuntimeError::WriterUnavailable)?;
                Ok(())
            }
            RuntimeEvent::VendorBound { vendor_session_id } => {
                self.writer
                    .bind_vendor(
                        binding.chat_id,
                        binding.generation,
                        binding.owner_fence,
                        vendor_session_id.clone(),
                    )
                    .await
                    .map_err(|_| RuntimeError::WriterUnavailable)?;
                self.state()?.vendor_id = Some(vendor_session_id);
                Ok(())
            }
            RuntimeEvent::TurnAcknowledged(turn) => self.acknowledge(turn).await,
            RuntimeEvent::AssistantSnapshot(text) => self.assistant_snapshot(text).await,
            RuntimeEvent::ToolCompleted(observation) => self.tool_record(observation).await,
            RuntimeEvent::TurnFinished {
                turn,
                snapshot,
                output_complete,
            } => self.finish(turn, snapshot, output_complete).await,
            RuntimeEvent::TransportEnded {
                snapshot,
                pending_turn,
                reason,
                cleanup,
            } => {
                self.transport_ended(
                    snapshot,
                    pending_turn,
                    reason,
                    cleanup.root_reaped && cleanup.tree_empty && cleanup.cleanup_verified,
                )
                .await
            }
        }
    }
}

pub(super) fn vendor_turn_id(harness: ChatHarness, turn: &TurnSummary) -> Option<String> {
    match harness {
        ChatHarness::Codex => turn.vendor_turn_id.clone(),
        ChatHarness::Claude => {
            let request = turn.request_id.to_string();
            if turn
                .vendor_turn_id
                .as_ref()
                .is_some_and(|vendor| vendor != &request)
            {
                None
            } else {
                Some(request)
            }
        }
    }
}

pub(super) fn protocol_matches(harness: ChatHarness, protocol: SessionProtocol) -> bool {
    matches!(
        (harness, protocol),
        (ChatHarness::Claude, SessionProtocol::ClaudeStreamJson)
            | (ChatHarness::Codex, SessionProtocol::CodexAppServer)
    )
}

fn stop_failure(reason: RuntimeStopReason) -> ChatFailureCode {
    match reason {
        RuntimeStopReason::Deadline => ChatFailureCode::DeadlineExceeded,
        RuntimeStopReason::ProtocolViolation => ChatFailureCode::ProtocolViolation,
        RuntimeStopReason::ProcessLost
        | RuntimeStopReason::InputFailed
        | RuntimeStopReason::StartupFailed => ChatFailureCode::ProcessLost,
        RuntimeStopReason::CancellationUnconfirmed | RuntimeStopReason::WriterUnavailable => {
            ChatFailureCode::OwnershipUncertain
        }
        RuntimeStopReason::Closed | RuntimeStopReason::Idle | RuntimeStopReason::Shutdown => {
            ChatFailureCode::Interrupted
        }
    }
}

#[cfg(test)]
#[path = "writer_sink_tests.rs"]
mod tests;
