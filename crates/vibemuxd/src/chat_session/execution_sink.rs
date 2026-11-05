//! Owner-backed execution submissions mapped to the chat writer's execution
//! methods. Every turn claims one durable `chat_execution_attempt`; the
//! chat's lifetime Run survives every turn, and only a
//! `change_execution_process` release ends the process generation. Legacy
//! per-turn chat rows are never synthesized by this sink.

use std::{
    fmt,
    sync::{Mutex, MutexGuard, TryLockError},
};

use async_trait::async_trait;
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_harness::chat_session::{
    AssistantText, SessionPhase, SessionSnapshot, ToolObservation, TurnOutcome, TurnPhase,
    TurnRequest, TurnSummary,
};
use vibemux_store::{ChatExecutionProcessChange, ChatMutation, ChatTurnFinish};
use vibemux_types::chat::{
    ChatFailureCode, ChatRecordKind, ChatSession, ChatState, ChatTurnOutcome, ChatTurnPhase,
};
use vibemux_types::chat_command_progress::CommandProgressSnapshot;
use vibemux_types::{ChatAttemptId, chat_attempt::ChatExecutionAttempt};

use super::writer_sink::{protocol_matches, vendor_turn_id};
use super::{
    ChatRuntimeSink, RuntimeBinding, RuntimeCommandProgress, RuntimeError, RuntimeEvent,
    RuntimeStopReason, sink_state::ActiveOutput,
};
use crate::chat_writer::{ChatRecordWrite, ChatWriter};

/// The binding comes only from the daemon's successful process claim. A sink
/// is never reused for a different chat, generation, or process fence.
pub(crate) struct ChatExecutionSink {
    writer: ChatWriter,
    binding: RuntimeBinding,
    state: Mutex<SinkState>,
    #[cfg(feature = "test_helpers")]
    approval_scope: Option<std::sync::Arc<super::fixture_runtime::FixtureApprovalScope>>,
}

#[derive(Default)]
struct SinkState {
    vendor_id: Option<String>,
    active: Option<ActiveOutput>,
    /// The durable attempt identity of `active`; at most one attempt per
    /// chat is active, so it moves with the shared output state.
    attempt_id: Option<ChatAttemptId>,
    claiming: bool,
    ended: bool,
}

impl fmt::Debug for ChatExecutionSink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatExecutionSink")
            .field("chat_id", &self.binding.chat_id)
            .field("generation", &self.binding.generation)
            .finish_non_exhaustive()
    }
}

impl ChatExecutionSink {
    pub(crate) fn new(writer: ChatWriter, binding: RuntimeBinding) -> Self {
        Self {
            writer,
            binding,
            state: Mutex::new(SinkState::default()),
            #[cfg(feature = "test_helpers")]
            approval_scope: None,
        }
    }
    #[cfg(feature = "test_helpers")]
    pub(super) fn new_fixture(
        writer: ChatWriter,
        binding: RuntimeBinding,
        scope: std::sync::Arc<super::fixture_runtime::FixtureApprovalScope>,
    ) -> Self {
        Self {
            writer,
            binding,
            state: Mutex::new(SinkState::default()),
            approval_scope: Some(scope),
        }
    }

    /// Explicit private streaming read. The writer owns terminal history;
    /// this bounded view exists only for the exact currently active request.
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

    /// Session reads go through the execution owner projection so the sink
    /// never consults legacy per-turn rows.
    async fn owner_session(&self) -> Result<ChatSession, RuntimeError> {
        let stored = self
            .writer
            .execution_owner(self.binding.chat_id)
            .await
            .map_err(|_| RuntimeError::WriterUnavailable)?
            .ok_or(RuntimeError::WriterUnavailable)?;
        if stored.session.generation != self.binding.generation {
            return Err(RuntimeError::ClaimRefused);
        }
        Ok(stored.session)
    }

    /// `turn_id` is the durable attempt identity: the runtime's
    /// `TurnRequest.turn_id` equals `ChatExecutionAttempt.attempt_id`, so
    /// every callback re-checks that binding against the stored attempt.
    async fn active_attempt(
        &self,
        request_id: Uuid,
        turn_id: Uuid,
        prompt_bytes: Option<usize>,
    ) -> Result<(ChatSession, ChatExecutionAttempt), RuntimeError> {
        let attempt = self
            .writer
            .execution_attempt(request_id)
            .await
            .map_err(|_| RuntimeError::WriterUnavailable)?
            .ok_or(RuntimeError::WrongTurn)?;
        if attempt.chat_id != self.binding.chat_id
            || attempt.generation != self.binding.generation
            || *attempt.attempt_id.as_uuid() != turn_id
            || prompt_bytes.is_some_and(|bytes| bytes as u64 != attempt.prompt_ref.byte_count)
            || !matches!(
                attempt.phase,
                ChatTurnPhase::Running | ChatTurnPhase::CancelRequested
            )
        {
            return Err(RuntimeError::WrongTurn);
        }
        let session = self.owner_session().await?;
        if session.active_request_id != Some(request_id) || session.state != ChatState::Busy {
            return Err(RuntimeError::WrongTurn);
        }
        Ok((session, attempt))
    }

    async fn acknowledge(&self, turn: TurnSummary) -> Result<(), RuntimeError> {
        if !turn.acknowledged
            || turn.outcome.is_some()
            || !matches!(turn.phase, TurnPhase::Running | TurnPhase::Cancelling)
        {
            return Err(RuntimeError::WrongTurn);
        }
        let (session, attempt) = self
            .active_attempt(turn.request_id, turn.turn_id, Some(turn.prompt_bytes))
            .await?;
        let vendor_turn = vendor_turn_id(session.harness, &turn).ok_or(RuntimeError::WrongTurn)?;
        {
            let state = self.state()?;
            let active = state.active.as_ref().ok_or(RuntimeError::WrongTurn)?;
            if active.request_id != turn.request_id
                || active.turn_id != turn.turn_id
                || active.prompt_bytes != turn.prompt_bytes
                || state.attempt_id != Some(attempt.attempt_id)
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
            .bind_execution_attempt(
                turn.request_id,
                self.binding.generation,
                self.binding.owner_fence,
                vendor_turn.clone(),
            )
            .await
            .map_err(|_| RuntimeError::WriterUnavailable)?;
        let mut state = self.state()?;
        if state.attempt_id != Some(attempt.attempt_id) {
            return Err(RuntimeError::WrongTurn);
        }
        let active = state.active.as_mut().ok_or(RuntimeError::WrongTurn)?;
        if active.request_id != turn.request_id {
            return Err(RuntimeError::WrongTurn);
        }
        active.acknowledged_vendor_turn = Some(vendor_turn);
        Ok(())
    }

    async fn assistant_snapshot(&self, text: AssistantText) -> Result<(), RuntimeError> {
        self.active_attempt(text.request_id, text.turn_id, None)
            .await?;
        let mut state = self.state()?;
        let active = state.active.as_mut().ok_or(RuntimeError::WrongTurn)?;
        active.merge_snapshot(text)
    }

    async fn tool_record(&self, observation: ToolObservation) -> Result<(), RuntimeError> {
        let (session, attempt) = self
            .active_attempt(observation.request_id, observation.turn_id, None)
            .await?;
        if session.harness != vibemux_types::chat::ChatHarness::Codex {
            return Err(RuntimeError::WrongTurn);
        }
        let reserved = {
            let mut state = self.state()?;
            let active = state.active.as_mut().ok_or(RuntimeError::WrongTurn)?;
            if active.acknowledged_vendor_turn != attempt.vendor_turn_id {
                return Err(RuntimeError::WrongTurn);
            }
            active.reserve_tool(&observation)?
        };
        let Some((record_id, body)) = reserved else {
            return Ok(());
        };
        let persisted = self
            .writer
            .append_execution_record(ChatRecordWrite {
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

    /// No mutex guard survives the append await. The fixed record ID keeps an
    /// unknown append response safe to reconcile without another record.
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
                .append_execution_record(ChatRecordWrite {
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
        let (session, attempt) = self
            .active_attempt(turn.request_id, turn.turn_id, Some(turn.prompt_bytes))
            .await?;
        let vendor_turn = vendor_turn_id(session.harness, &turn);
        let correlated = {
            let state = self.state()?;
            state.active.as_ref().is_some_and(|active| {
                active.request_id == turn.request_id
                    && active.turn_id == turn.turn_id
                    && active.prompt_bytes == turn.prompt_bytes
                    && active.acknowledged_vendor_turn.is_some()
                    && active.acknowledged_vendor_turn == vendor_turn
            }) && state.attempt_id == Some(attempt.attempt_id)
                && attempt.vendor_turn_id == vendor_turn
                && turn.acknowledged
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
            .finish_execution_attempt(ChatTurnFinish {
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
        let mut state = self.state()?;
        state.active = None;
        state.attempt_id = None;
        Ok(())
    }

    /// The process generation ends here and nowhere else: the store's
    /// release settles or retains the active attempt, and no legacy
    /// release/owner-lost/close call follows.
    async fn transport_ended(
        &self,
        snapshot: SessionSnapshot,
        pending: Option<TurnSummary>,
        reason: RuntimeStopReason,
        verified: bool,
    ) -> Result<(), RuntimeError> {
        self.state()?.ended = true;
        let session = self.owner_session().await?;
        if let Some(request_id) = session.active_request_id {
            let attempt = self
                .writer
                .execution_attempt(request_id)
                .await
                .map_err(|_| RuntimeError::WriterUnavailable)?
                .ok_or(RuntimeError::WrongTurn)?;
            if attempt.chat_id != self.binding.chat_id
                || attempt.generation != self.binding.generation
            {
                return Err(RuntimeError::WrongTurn);
            }
            // Cross-check the captured attempt/session/pending identities;
            // the runtime turn ID is the durable attempt ID. Teardown never
            // manufactures an acknowledgement or completion. A mismatched
            // observed identity always rejects, whatever else is captured.
            let attempt_turn = *attempt.attempt_id.as_uuid();
            let mut pending_evidence = false;
            for observed in [pending.as_ref(), snapshot.active_turn.as_ref()]
                .into_iter()
                .flatten()
            {
                if observed.request_id != request_id || observed.turn_id != attempt_turn {
                    return Err(RuntimeError::WrongTurn);
                }
                pending_evidence = true;
            }
            match (self.state()?.attempt_id, attempt.phase) {
                (None, ChatTurnPhase::Admitted) => {}
                (Some(captured), _) if captured == attempt.attempt_id => {}
                // Lost claim result: the writer committed the durable claim
                // after the caller stopped waiting, so no local capture
                // exists. Exact pending evidence against the durable
                // identity correlates the attempt; the store still settles
                // it as Interrupted and infers no success.
                (None, ChatTurnPhase::Running | ChatTurnPhase::CancelRequested)
                    if pending_evidence => {}
                _ => return Err(RuntimeError::WrongTurn),
            }
            self.flush_output(request_id).await?;
        }
        self.writer
            .change_execution_process(
                self.mutation(),
                self.binding.owner_fence,
                ChatExecutionProcessChange::Released {
                    cleanup_verified: verified,
                    closed: reason == RuntimeStopReason::Closed,
                },
            )
            .await
            .map_err(|_| RuntimeError::WriterUnavailable)?;
        let mut state = self.state()?;
        state.active = None;
        state.attempt_id = None;
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
impl ChatRuntimeSink for ChatExecutionSink {
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

    #[cfg(feature = "test_helpers")]
    async fn validate_approval_dispatch(
        &self,
        binding: &RuntimeBinding,
        evidence: &crate::chat_writer::ChatApprovalEvidence,
    ) -> Result<(), RuntimeError> {
        self.check_binding(binding)?;
        let (_, attempt) = self
            .active_attempt(
                evidence.request_id,
                *self
                    .writer
                    .execution_attempt(evidence.request_id)
                    .await
                    .map_err(|_| RuntimeError::WrongTurn)?
                    .ok_or(RuntimeError::WrongTurn)?
                    .attempt_id
                    .as_uuid(),
                None,
            )
            .await?;
        if attempt.phase != ChatTurnPhase::Running {
            return Err(RuntimeError::ClaimRefused);
        }
        self.approval_scope
            .as_ref()
            .ok_or(RuntimeError::ClaimRefused)?
            .verify(evidence)
            .await
            .map(|_| ())
    }
    async fn cancel_turn(
        &self,
        binding: &RuntimeBinding,
        turn: TurnSummary,
    ) -> Result<(), RuntimeError> {
        self.check_binding(binding)?;
        self.active_attempt(turn.request_id, turn.turn_id, None)
            .await?;
        self.writer
            .cancel_execution_attempt(turn.request_id, binding.generation, binding.owner_fence)
            .await
            .map_err(|_| RuntimeError::WriterUnavailable)?;
        Ok(())
    }
    #[cfg(feature = "test_helpers")]
    async fn admit_approval(
        &self,
        binding: &RuntimeBinding,
        challenge: vibemux_harness::chat_session::ApprovalChallenge,
        expires_at: OffsetDateTime,
    ) -> Result<vibemux_types::chat_approval::ChatApproval, RuntimeError> {
        self.check_binding(binding)?;
        let scope = self
            .approval_scope
            .as_ref()
            .ok_or(RuntimeError::ClaimRefused)?;
        let (session, _) = self
            .active_attempt(challenge.request_id, challenge.turn_id, None)
            .await?;
        if session.vendor_session_id.as_deref() != Some(challenge.vendor_session_id.as_str()) {
            return Err(RuntimeError::WrongTurn);
        }
        let body = crate::chat_writer::ChatApprovalEvidence {
            schema_version: 1,
            chat_id: binding.chat_id,
            request_id: challenge.request_id,
            generation: binding.generation,
            vendor_session_id: challenge.vendor_session_id,
            vendor_request_id: challenge.vendor_request_id,
            tool_use_id: challenge.tool_use_id,
            tool: challenge.tool,
            vendor_tool_name: challenge.vendor_tool_name,
            input: challenge.input,
        };
        scope.verify(&body).await?;
        self.writer
            .admit_approval(crate::chat_writer::ChatApprovalWrite {
                evidence_id: challenge.challenge_id,
                fence: binding.owner_fence,
                evidence: body,
                expires_at,
            })
            .await
            .map(|v| v.approval)
            .map_err(|_| RuntimeError::ClaimRefused)
    }
    #[cfg(feature = "test_helpers")]
    async fn decide_approval(
        &self,
        binding: &RuntimeBinding,
        request: super::RuntimeApprovalDecision,
    ) -> Result<crate::chat_writer::ChatApprovalDecisionResult, RuntimeError> {
        use vibemux_types::chat_approval::{ChatApprovalDelivery, ChatApprovalState};
        self.check_binding(binding)?;
        let scope = self
            .approval_scope
            .as_ref()
            .ok_or(RuntimeError::ClaimRefused)?;
        let record = self
            .writer
            .approval_metadata(binding.chat_id, request.approval_id)
            .await
            .map_err(|_| RuntimeError::ClaimRefused)?;
        if record.generation != binding.generation {
            return Err(RuntimeError::WrongTurn);
        }
        let owner = self
            .writer
            .execution_owner(binding.chat_id)
            .await
            .map_err(|_| RuntimeError::WriterUnavailable)?
            .ok_or(RuntimeError::WrongTurn)?;
        let mut input = vibemux_store::ChatApprovalDecisionInput {
            operation_id: request.operation_id,
            approval_id: request.approval_id,
            expected_version: request.expected_version,
            generation: binding.generation,
            fence: binding.owner_fence,
            decision: request.decision,
            observed_commit: owner.owner.base_commit,
            profile_sha256: owner.owner.profile_sha256,
            timestamp: OffsetDateTime::now_utc(),
        };
        if record.state == ChatApprovalState::Pending {
            self.active_attempt(record.request_id, *record.attempt_id.as_uuid(), None)
                .await?;
            let content = self
                .writer
                .approval_content(binding.chat_id, request.approval_id)
                .await
                .map_err(|_| RuntimeError::ContentUnavailable)?;
            let body = content.evidence.ok_or(RuntimeError::ContentUnavailable)?;
            input.observed_commit = scope.verify(&body).await?;
        }
        let mut result = self
            .writer
            .decide_approval(input)
            .await
            .map_err(|_| RuntimeError::ClaimRefused)?;
        if let Some(dispatch) = result.dispatch.as_ref()
            && scope.verify(dispatch.evidence()).await.is_err()
        {
            let Some(dispatch) = result.dispatch.take() else {
                return Err(RuntimeError::ClaimRefused);
            };
            let (claim, _) = dispatch.into_parts();
            result.receipt = self
                .writer
                .complete_approval_delivery(claim, ChatApprovalDelivery::Unknown)
                .await
                .map_err(|_| RuntimeError::WriterUnavailable)?;
        }
        Ok(result)
    }
    #[cfg(feature = "test_helpers")]
    async fn complete_approval(
        &self,
        binding: &RuntimeBinding,
        claim: vibemux_store::ChatApprovalDeliveryClaim,
        outcome: vibemux_types::chat_approval::ChatApprovalDelivery,
    ) -> Result<vibemux_types::chat_approval::ChatApproval, RuntimeError> {
        self.check_binding(binding)?;
        if self.approval_scope.is_none() {
            return Err(RuntimeError::ClaimRefused);
        }
        let record = self
            .writer
            .approval_metadata(binding.chat_id, claim.approval_id())
            .await
            .map_err(|_| RuntimeError::ClaimRefused)?;
        if record.generation != binding.generation {
            return Err(RuntimeError::WrongTurn);
        }
        self.writer
            .complete_approval_delivery(claim, outcome)
            .await
            .map(|v| v.approval)
            .map_err(|_| RuntimeError::WriterUnavailable)
    }
    #[cfg(feature = "test_helpers")]
    async fn expire_approval(
        &self,
        binding: &RuntimeBinding,
        id: vibemux_types::ChatApprovalId,
    ) -> Result<vibemux_types::chat_approval::ChatApproval, RuntimeError> {
        self.check_binding(binding)?;
        if self.approval_scope.is_none() {
            return Err(RuntimeError::ClaimRefused);
        }
        self.writer
            .expire_approval(binding.chat_id, id, binding.generation, binding.owner_fence)
            .await
            .map_err(|_| RuntimeError::WriterUnavailable)
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
            let attempt = self
                .writer
                .execution_attempt(turn.request_id)
                .await
                .map_err(|_| RuntimeError::ClaimRefused)?
                .ok_or(RuntimeError::ClaimRefused)?;
            // The runtime turn ID must equal the durable attempt ID; a
            // mismatched identity never reaches the one-use claim.
            if attempt.chat_id != binding.chat_id
                || attempt.generation != binding.generation
                || *attempt.attempt_id.as_uuid() != turn.turn_id
                || attempt.phase != ChatTurnPhase::Admitted
                || attempt.prompt_ref.byte_count != turn.prompt.len() as u64
            {
                return Err(RuntimeError::ClaimRefused);
            }
            // execution_prompt verifies the immutable blob's recorded byte
            // count and fingerprint before returning private bytes. Compare
            // exact text before the durable one-use claim, never afterward.
            let prompt = self
                .writer
                .execution_prompt(turn.request_id)
                .await
                .map_err(|_| RuntimeError::ClaimRefused)?;
            if prompt != turn.prompt {
                return Err(RuntimeError::ClaimRefused);
            }
            self.writer
                .claim_execution_attempt(turn.request_id, binding.generation, binding.owner_fence)
                .await
                .map_err(|_| RuntimeError::ClaimRefused)
        }
        .await;
        let mut state = self.state()?;
        state.claiming = false;
        if let Ok(commit) = &claimed {
            state.active = Some(ActiveOutput::new(
                turn.request_id,
                turn.turn_id,
                turn.prompt.len(),
            ));
            state.attempt_id = Some(commit.attempt.attempt_id);
        }
        claimed.map(|_| ())
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
                    .change_execution_process(
                        self.mutation(),
                        self.binding.owner_fence,
                        ChatExecutionProcessChange::Ready,
                    )
                    .await
                    .map_err(|_| RuntimeError::WriterUnavailable)?;
                Ok(())
            }
            RuntimeEvent::VendorBound { vendor_session_id } => {
                self.writer
                    .change_execution_process(
                        self.mutation(),
                        self.binding.owner_fence,
                        ChatExecutionProcessChange::VendorBound {
                            vendor_id: vendor_session_id.clone(),
                        },
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

#[cfg(test)]
#[path = "execution_sink_tests.rs"]
mod tests;
