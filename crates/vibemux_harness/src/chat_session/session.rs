//! Generation-local protocol state. Durable replay and ownership live in the daemon.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use serde_json::{Value, json};
use uuid::Uuid;

use crate::dispatch::Sha256Digest;

use super::{
    AssistantText, IgnoredReason, SessionConfig, SessionEffect, SessionError, SessionEvent,
    SessionLimits, SessionPhase, SessionProtocol, SessionSnapshot, SessionTransition, TurnOutcome,
    TurnPhase, TurnRequest, TurnSummary, WireFrame, claude::ClaudeState, codex::CodexState,
    text::TextBuffer, types::validate_identifier,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RpcKind {
    Initialize,
    Open,
    StartTurn(Uuid),
    Interrupt(Uuid),
}

pub(super) struct ActiveTurn {
    pub summary: TurnSummary,
    pub text: TextBuffer,
}

pub struct SessionMachine {
    pub(super) config: SessionConfig,
    pub(super) limits: SessionLimits,
    pub(super) phase: SessionPhase,
    pub(super) vendor_session_id: Option<String>,
    pub(super) active: Option<ActiveTurn>,
    pub(super) last_turn: Option<TurnSummary>,
    used_requests: BTreeSet<Uuid>,
    used_turns: BTreeSet<Uuid>,
    pub(super) retired_vendor_turns: BTreeSet<String>,
    pub(super) retired_messages: BTreeSet<String>,
    pub(super) pending_rpc: BTreeMap<String, RpcKind>,
    pub(super) retired_rpc: BTreeSet<String>,
    pub(super) server_requests: BTreeMap<String, Sha256Digest>,
    rpc_requests: usize,
    records_received: u64,
    record_bytes: usize,
    pub(super) close_requested: bool,
    error: Option<SessionError>,
    pub(super) codex: CodexState,
    pub(super) claude: ClaudeState,
    pub(super) fixture_approval: Option<super::approval::FixtureApprovalState>,
}

impl fmt::Debug for SessionMachine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionMachine")
            .field("snapshot", &self.snapshot())
            .finish()
    }
}

impl SessionMachine {
    pub fn new(config: SessionConfig, limits: SessionLimits) -> Result<Self, SessionError> {
        config.validate()?;
        limits.validate()?;
        Ok(Self {
            config,
            limits,
            phase: SessionPhase::New,
            vendor_session_id: None,
            active: None,
            last_turn: None,
            used_requests: BTreeSet::new(),
            used_turns: BTreeSet::new(),
            retired_vendor_turns: BTreeSet::new(),
            retired_messages: BTreeSet::new(),
            pending_rpc: BTreeMap::new(),
            retired_rpc: BTreeSet::new(),
            server_requests: BTreeMap::new(),
            rpc_requests: 0,
            records_received: 0,
            record_bytes: 0,
            close_requested: false,
            error: None,
            codex: CodexState::default(),
            claude: ClaudeState::default(),
            fixture_approval: None,
        })
    }

    pub fn start(&mut self) -> Result<SessionTransition, SessionError> {
        if self.phase != SessionPhase::New {
            return Err(SessionError::UnexpectedState);
        }
        let effect = match self.config.target.protocol() {
            SessionProtocol::CodexAppServer => self.rpc(RpcKind::Initialize, "initialize", json!({
                "clientInfo":{"name":"vibemux","title":"VibeMux","version":env!("CARGO_PKG_VERSION")},
                "capabilities":{"experimentalApi":false}
            }))?,
            SessionProtocol::ClaudeStreamJson => self.control_request(RpcKind::Initialize, json!({"subtype":"initialize","hooks":null,"skills":[]}))?,
        };
        self.phase = SessionPhase::Initializing;
        Ok(SessionTransition {
            effects: vec![effect],
            ..Default::default()
        })
    }

    pub fn submit(&mut self, request: TurnRequest) -> Result<SessionTransition, SessionError> {
        if self.phase != SessionPhase::Ready || self.close_requested {
            return Err(SessionError::UnexpectedState);
        }
        if request.request_id.is_nil()
            || request.turn_id.is_nil()
            || request.request_id == request.turn_id
        {
            return Err(SessionError::InvalidIdentifier);
        }
        if request.prompt.trim().is_empty() || request.prompt.len() > self.limits.max_prompt_bytes {
            return Err(SessionError::InvalidPrompt);
        }
        if self.used_requests.contains(&request.request_id)
            || self.used_turns.contains(&request.turn_id)
        {
            return Err(SessionError::DuplicateRequest);
        }
        if self.used_requests.len() >= self.limits.max_turns {
            return Err(SessionError::TurnBudgetExhausted);
        }
        let rpc_allowance = if self.config.target.protocol() == SessionProtocol::CodexAppServer {
            2
        } else {
            1
        };
        if self.rpc_requests.saturating_add(rpc_allowance) > self.limits.max_rpc_requests {
            return Err(SessionError::RpcBudgetExhausted);
        }
        let effect = match self.config.target.protocol() {
            SessionProtocol::CodexAppServer => {
                let thread_id = self.vendor_session_id.as_ref().ok_or(SessionError::UncorrelatedSession)?.clone();
                self.rpc(RpcKind::StartTurn(request.turn_id), "turn/start", json!({
                    "threadId":thread_id,"input":[{"type":"text","text":request.prompt}],
                    "approvalPolicy":"never","sandboxPolicy":{"type":"readOnly","networkAccess":false}
                }))?
            }
            SessionProtocol::ClaudeStreamJson => self.frame(json!({
                "type":"user","uuid":request.request_id,"session_id":self.config.target.expected_id(),
                "message":{"role":"user","content":request.prompt},"parent_tool_use_id":null,
                "origin":{"kind":"human"}
            }))?,
        };
        let summary = TurnSummary {
            request_id: request.request_id,
            turn_id: request.turn_id,
            vendor_turn_id: None,
            phase: TurnPhase::Prepared,
            outcome: None,
            acknowledged: false,
            cancel_requested: false,
            prompt_bytes: request.prompt.len(),
        };
        self.used_requests.insert(request.request_id);
        self.used_turns.insert(request.turn_id);
        self.active = Some(ActiveTurn {
            summary: summary.clone(),
            text: TextBuffer::default(),
        });
        self.codex.reset_turn();
        self.claude.reset_turn();
        self.phase = SessionPhase::Busy;
        let effect = if let Some(approval) = self.fixture_approval.as_mut() {
            approval.prepare(request.request_id, request.turn_id)?;
            match effect {
                SessionEffect::Write(frame) => SessionEffect::WriteTurn {
                    request_id: request.request_id,
                    turn_id: request.turn_id,
                    frame,
                },
                _ => return Err(SessionError::UnexpectedState),
            }
        } else {
            effect
        };
        Ok(SessionTransition {
            effects: vec![effect],
            events: vec![SessionEvent::TurnPrepared { turn: summary }],
            text: Vec::new(),
            approvals: Vec::new(),
            ..Default::default()
        })
    }

    /// `sequence` is the owning reader's process-local record ordinal, starting
    /// at1. It is not a canonical event sequence. Repeated delivery is rejected
    /// before text is applied; equal legitimate delta strings are not deduped.
    pub fn receive(
        &mut self,
        sequence: u64,
        frame: &[u8],
    ) -> Result<SessionTransition, SessionError> {
        if sequence <= self.records_received {
            return Err(SessionError::SequenceReplay);
        }
        let result = self.receive_next(sequence, frame);
        if let Err(error) = result {
            self.poison(error);
        }
        result
    }

    fn receive_next(
        &mut self,
        sequence: u64,
        frame: &[u8],
    ) -> Result<SessionTransition, SessionError> {
        if matches!(
            self.phase,
            SessionPhase::New
                | SessionPhase::Closed
                | SessionPhase::Suspended
                | SessionPhase::RecoveryRequired
                | SessionPhase::Failed
        ) {
            return Err(SessionError::UnexpectedState);
        }
        if self.records_received.checked_add(1) != Some(sequence) {
            return Err(SessionError::SequenceGap);
        }
        if frame.is_empty() {
            return Err(SessionError::InvalidFrame);
        }
        if frame.len() > self.limits.max_frame_bytes {
            return Err(SessionError::FrameTooLarge);
        }
        let bytes = self
            .record_bytes
            .checked_add(frame.len())
            .ok_or(SessionError::RecordBudgetExhausted)?;
        if sequence > self.limits.max_records as u64 || bytes > self.limits.max_record_bytes {
            return Err(SessionError::RecordBudgetExhausted);
        }
        let value: Value = serde_json::from_slice(frame).map_err(|_| SessionError::InvalidJson)?;
        if !value.is_object() {
            return Err(SessionError::InvalidFrame);
        }
        self.records_received = sequence;
        self.record_bytes = bytes;
        match self.config.target.protocol() {
            SessionProtocol::CodexAppServer => self.receive_codex(value, frame.len()),
            SessionProtocol::ClaudeStreamJson => {
                self.receive_claude(value, Sha256Digest::of(frame))
            }
        }
    }

    pub fn cancel(&mut self, turn_id: Uuid) -> Result<SessionTransition, SessionError> {
        let active = self.active.as_mut().ok_or(SessionError::UnexpectedState)?;
        if active.summary.turn_id != turn_id {
            return Err(SessionError::UncorrelatedTurn);
        }
        if matches!(
            active.summary.phase,
            TurnPhase::Uncertain | TurnPhase::Finished
        ) {
            return Err(SessionError::UnexpectedState);
        }
        if active.summary.cancel_requested {
            return Ok(SessionTransition::default());
        }
        active.summary.cancel_requested = true;
        active.summary.phase = TurnPhase::Cancelling;
        let summary = active.summary.clone();
        let mut transition = SessionTransition {
            events: vec![SessionEvent::CancellationRequested { turn: summary }],
            ..Default::default()
        };
        match self.cancel_effect() {
            Ok(Some(effect)) => transition.effects.push(effect),
            Ok(None) => {}
            Err(error) => {
                self.poison(error);
                return Err(error);
            }
        }
        Ok(transition)
    }

    pub fn close(&mut self) -> Result<SessionTransition, SessionError> {
        if self.close_requested {
            return Ok(SessionTransition::default());
        }
        self.close_requested = true;
        let mut transition = SessionTransition {
            events: vec![SessionEvent::Closing],
            ..Default::default()
        };
        if let Some(active) = &self.active {
            if active.summary.phase != TurnPhase::Uncertain {
                transition.extend(self.cancel(active.summary.turn_id)?);
            } else {
                transition.effects.push(SessionEffect::CloseInput);
            }
        } else {
            self.retired_rpc
                .extend(std::mem::take(&mut self.pending_rpc).into_keys());
            self.phase = SessionPhase::Closing;
            transition.effects.push(SessionEffect::CloseInput);
        }
        Ok(transition)
    }

    /// A lost pipe does not prove process exit or turn completion. There is no
    /// retry/resend effect and an uncertain active turn stays reserved.
    pub fn transport_lost(&mut self) -> SessionTransition {
        if self.close_requested && self.active.is_none() && self.phase == SessionPhase::Closing {
            // Expected EOF after CloseInput is not a failure and is still
            // insufficient evidence that the owned process tree has exited.
            return SessionTransition::default();
        }
        if matches!(
            self.phase,
            SessionPhase::Closed
                | SessionPhase::Suspended
                | SessionPhase::RecoveryRequired
                | SessionPhase::Failed
        ) {
            return SessionTransition::default();
        }
        self.error = Some(SessionError::TransportLost);
        if let Some(active) = &mut self.active {
            active.summary.phase = TurnPhase::Uncertain;
            self.phase = SessionPhase::RecoveryRequired;
            SessionTransition {
                events: vec![SessionEvent::TurnUncertain {
                    turn: active.summary.clone(),
                }],
                ..Default::default()
            }
        } else {
            self.phase = SessionPhase::Suspended;
            SessionTransition {
                events: vec![SessionEvent::Suspended],
                ..Default::default()
            }
        }
    }

    /// Called only after the daemon verifies the owned process tree exited.
    /// Even a clean exit cannot complete a turn lacking terminal evidence.
    pub fn process_exited(&mut self) -> SessionTransition {
        if self.close_requested && self.active.is_none() {
            if self.phase == SessionPhase::Closed {
                return SessionTransition::default();
            }
            self.phase = SessionPhase::Closed;
            SessionTransition {
                events: vec![SessionEvent::Closed],
                ..Default::default()
            }
        } else {
            self.transport_lost()
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            protocol: self.config.target.protocol(),
            generation_id: self.config.generation_id,
            phase: self.phase,
            expected_vendor_session_id: self.config.target.expected_id(),
            vendor_session_confirmed: self.vendor_session_id.is_some(),
            vendor_session_id: self.vendor_session_id.clone(),
            active_turn: self.active.as_ref().map(|active| active.summary.clone()),
            last_turn: self.last_turn.clone(),
            turns_started: self.used_requests.len(),
            records_received: self.records_received,
            record_bytes: self.record_bytes,
            rpc_requests: self.rpc_requests,
            close_requested: self.close_requested,
            error: self.error,
        }
    }

    pub(super) fn frame(&self, value: Value) -> Result<SessionEffect, SessionError> {
        let mut bytes = serde_json::to_vec(&value).map_err(|_| SessionError::InvalidFrame)?;
        bytes.push(b'\n');
        if bytes.len() > self.limits.max_frame_bytes {
            return Err(SessionError::FrameTooLarge);
        }
        Ok(SessionEffect::Write(WireFrame(bytes)))
    }

    pub(super) fn rpc(
        &mut self,
        kind: RpcKind,
        method: &str,
        params: Value,
    ) -> Result<SessionEffect, SessionError> {
        let id = self.next_rpc_id()?;
        let effect = self.frame(json!({"id":id,"method":method,"params":params}))?;
        self.rpc_requests += 1;
        self.pending_rpc.insert(id, kind);
        Ok(effect)
    }

    pub(super) fn control_request(
        &mut self,
        kind: RpcKind,
        request: Value,
    ) -> Result<SessionEffect, SessionError> {
        let id = self.next_rpc_id()?;
        let effect =
            self.frame(json!({"type":"control_request","request_id":id,"request":request}))?;
        self.rpc_requests += 1;
        self.pending_rpc.insert(id, kind);
        Ok(effect)
    }

    fn next_rpc_id(&self) -> Result<String, SessionError> {
        if self.rpc_requests >= self.limits.max_rpc_requests {
            return Err(SessionError::RpcBudgetExhausted);
        }
        Ok(format!(
            "vibemux_chat_{}_{}",
            self.config.generation_id,
            self.rpc_requests + 1
        ))
    }

    pub(super) fn take_rpc(&mut self, id: &str) -> Result<Option<RpcKind>, SessionError> {
        validate_identifier(id)?;
        if let Some(kind) = self.pending_rpc.remove(id) {
            self.retired_rpc.insert(id.to_owned());
            return Ok(Some(kind));
        }
        if self.retired_rpc.contains(id) {
            Ok(None)
        } else {
            Err(SessionError::UncorrelatedResponse)
        }
    }

    pub(super) fn bind_session(&mut self, id: &str) -> Result<SessionTransition, SessionError> {
        validate_identifier(id)?;
        if self
            .config
            .target
            .expected_id()
            .as_ref()
            .is_some_and(|expected| expected != id)
            || self
                .vendor_session_id
                .as_ref()
                .is_some_and(|expected| expected != id)
        {
            return Err(SessionError::UncorrelatedSession);
        }
        if self.vendor_session_id.is_some() {
            return Ok(SessionTransition::default());
        }
        self.vendor_session_id = Some(id.to_owned());
        Ok(SessionTransition {
            events: vec![SessionEvent::SessionBound {
                vendor_session_id: id.to_owned(),
            }],
            ..Default::default()
        })
    }

    pub(super) fn acknowledge(&mut self) -> Result<SessionTransition, SessionError> {
        let active = self.active.as_mut().ok_or(SessionError::UncorrelatedTurn)?;
        if active.summary.acknowledged {
            return Ok(SessionTransition::default());
        }
        active.summary.acknowledged = true;
        if !active.summary.cancel_requested {
            active.summary.phase = TurnPhase::Running;
        }
        Ok(SessionTransition {
            events: vec![SessionEvent::TurnAcknowledged {
                turn: active.summary.clone(),
            }],
            ..Default::default()
        })
    }

    pub(super) fn finish_turn(
        &mut self,
        outcome: TurnOutcome,
    ) -> Result<SessionTransition, SessionError> {
        let mut active = self.active.take().ok_or(SessionError::UncorrelatedTurn)?;
        if !active.summary.acknowledged || self.vendor_session_id.is_none() {
            self.active = Some(active);
            return Err(SessionError::MissingCorrelation);
        }
        active.summary.phase = TurnPhase::Finished;
        active.summary.outcome = Some(outcome);
        if let Some(id) = &active.summary.vendor_turn_id {
            self.retired_vendor_turns.insert(id.clone());
        }
        self.retired_messages
            .extend(self.claude.bound_messages.iter().cloned());
        self.last_turn = Some(active.summary.clone());
        self.phase = if self.close_requested {
            SessionPhase::Closing
        } else {
            SessionPhase::Ready
        };
        let effects = if self.close_requested {
            vec![SessionEffect::CloseInput]
        } else {
            Vec::new()
        };
        Ok(SessionTransition {
            effects,
            events: vec![SessionEvent::TurnFinished {
                turn: active.summary,
            }],
            text: Vec::new(),
            approvals: Vec::new(),
            ..Default::default()
        })
    }

    pub(super) fn text_update(
        &mut self,
        id: &str,
        text: &str,
        append: bool,
    ) -> Result<AssistantText, SessionError> {
        let active = self.active.as_mut().ok_or(SessionError::UncorrelatedTurn)?;
        active.text.update(id, text, append, &self.limits)?;
        Ok(active.text.snapshot(&active.summary, &self.limits))
    }

    pub(super) fn final_text(&mut self, text: &str) -> Result<AssistantText, SessionError> {
        let active = self.active.as_mut().ok_or(SessionError::UncorrelatedTurn)?;
        active.text.final_text(text, &self.limits)?;
        Ok(active.text.snapshot(&active.summary, &self.limits))
    }

    pub(super) fn is_retired_request(&self, request_id: Uuid) -> bool {
        self.used_requests.contains(&request_id)
            && self
                .active
                .as_ref()
                .is_none_or(|active| active.summary.request_id != request_id)
    }

    pub(super) fn cancel_effect(&mut self) -> Result<Option<SessionEffect>, SessionError> {
        let active = self.active.as_ref().ok_or(SessionError::UnexpectedState)?;
        let turn_id = active.summary.turn_id;
        if self
            .pending_rpc
            .values()
            .any(|kind| *kind == RpcKind::Interrupt(turn_id))
        {
            return Ok(None);
        }
        match self.config.target.protocol() {
            SessionProtocol::ClaudeStreamJson => self
                .control_request(RpcKind::Interrupt(turn_id), json!({"subtype":"interrupt"}))
                .map(Some),
            SessionProtocol::CodexAppServer => {
                let Some(vendor_turn_id) = active.summary.vendor_turn_id.clone() else {
                    return Ok(None);
                };
                let thread_id = self
                    .vendor_session_id
                    .clone()
                    .ok_or(SessionError::UncorrelatedSession)?;
                self.rpc(
                    RpcKind::Interrupt(turn_id),
                    "turn/interrupt",
                    json!({"threadId":thread_id,"turnId":vendor_turn_id}),
                )
                .map(Some)
            }
        }
    }

    fn poison(&mut self, error: SessionError) {
        self.error = Some(error);
        self.phase = if let Some(active) = &mut self.active {
            active.summary.phase = TurnPhase::Uncertain;
            SessionPhase::RecoveryRequired
        } else {
            SessionPhase::Failed
        };
    }

    pub(super) fn ignored(reason: IgnoredReason) -> SessionTransition {
        SessionTransition {
            events: vec![SessionEvent::RecordIgnored { reason }],
            ..Default::default()
        }
    }
}

pub(super) fn identifier(value: Option<&Value>) -> Result<String, SessionError> {
    let id = value
        .and_then(Value::as_str)
        .ok_or(SessionError::InvalidIdentifier)?;
    validate_identifier(id)?;
    Ok(id.to_owned())
}

pub(super) fn rpc_id(value: Option<&Value>) -> Result<String, SessionError> {
    let value = value.ok_or(SessionError::InvalidIdentifier)?;
    if let Some(number) = value.as_i64() {
        return Ok(number.to_string());
    }
    identifier(Some(value))
}
