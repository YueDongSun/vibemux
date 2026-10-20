//! The client side of one long-lived DSH session.
//!
//! The machine owns the handshake, prompt admission, notification
//! correlation, and teardown; it performs no I/O. The daemon writes every
//! [`DshAction::Write`] frame to the runtime's stdin and applies every
//! [`DshAction::Terminate`] by tearing the contained process tree down.
//!
//! Two rules make this machine different from a one-shot protocol session:
//!
//! - **Idle is not completion.** `session.status: idle` is recorded for
//!   diagnostics and never ends a turn, because AGENTS.md §2 invariant 5
//!   forbids treating idle state as proof of completion. Only a correlated
//!   `turn/end` event carries a structured reason, and only a recognized
//!   `reason.kind` becomes a terminal outcome.
//! - **The notification stream is not filtered by session.** The protocol
//!   delivers every session in the runtime to every client, so a notification
//!   whose `sessionId` is not this session's is counted and dropped rather
//!   than allowed to advance this session's state.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    DshError, DshPhase, DshSessionLimits, DshSessionSettings, MAX_PROMPT_BYTES,
    wire::{
        self, DshFrameKind, EVENT_TURN_END, EVENT_TURN_START, METHOD_INITIALIZE,
        METHOD_SESSION_PROMPT, METHOD_SHUTDOWN, NOTIFICATION_METHODS, NOTIFICATION_SESSION_EVENT,
        NOTIFICATION_SESSION_STATUS,
    },
};
use crate::dispatch::Sha256Digest;

/// Abandoned request ids the machine remembers. Each `begin_close` or
/// `cancel` abandons at most one request, so a real teardown leaves one or
/// two; the bound only stops a caller that loops from growing the list.
const MAX_ABANDONED_REQUESTS: usize = 4;

/// How one turn ended, from the runtime's own structured reason.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DshTurnOutcome {
    /// `turn/end` reported `reason.kind: completed`.
    Completed,
    /// `turn/end` reported a failure reason (`blocked`, `error`,
    /// `max-tokens`, or `interrupted`).
    Failed,
    /// `turn/end` reported `aborted`, or the process went away mid-turn.
    Cancelled,
    /// The turn ended without a reason this adapter recognizes, or the
    /// session failed before a terminal event. Never counts as success.
    Unverified,
}

impl DshTurnOutcome {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Unverified => "unverified",
        }
    }
}

/// One instruction for the daemon's session driver.
#[derive(Clone, PartialEq)]
pub enum DshAction {
    /// Write one JSON-RPC frame to the runtime's stdin, followed by LF.
    Write(Value),
    /// Tear down the contained process tree. The SDK protocol publishes no
    /// cancel or session-close method, so this is how a turn is abandoned.
    Terminate,
}

impl fmt::Debug for DshAction {
    /// Redacted: a `session/prompt` frame carries the prompt text.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Write(frame) => {
                let method = frame.get("method").and_then(Value::as_str).unwrap_or("-");
                write!(formatter, "DshAction::Write({method})")
            }
            Self::Terminate => formatter.write_str("DshAction::Terminate"),
        }
    }
}

/// Content-free view of one turn.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DshTurnSummary {
    /// 1-based ordinal of this prompt inside the session.
    pub ordinal: u64,
    /// Runtime turn number, once `turn/start` or `turn/end` reported it.
    pub turn: Option<u64>,
    pub prompt_bytes: u64,
    pub prompt_sha256: Sha256Digest,
    pub outcome: Option<DshTurnOutcome>,
}

/// Allowlisted, content-free projection of one session.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DshSessionSummary {
    pub session_id: String,
    pub phase: DshPhase,
    pub provider: String,
    pub model: String,
    /// `serverInfo.version` reported by the handshake, or `None` before it.
    pub server_version: Option<String>,
    pub records: u64,
    pub record_bytes: u64,
    /// Records the runtime produced for other sessions in the same process.
    pub foreign_notifications: u64,
    /// Records that arrived for work this session had already abandoned and
    /// were dropped: an answer to a request it stopped waiting for, or an
    /// event of a turn it gave up on while closing.
    pub late_frames: u64,
    pub turns_started: u64,
    pub completed_turns: u64,
    pub failed_turns: u64,
    pub cancelled_turns: u64,
    pub unverified_turns: u64,
    pub prompt_bytes_total: u64,
    pub current_turn: Option<DshTurnSummary>,
    pub last_outcome: Option<DshTurnOutcome>,
    /// Stable code of the failure that ended the session, if any.
    pub error_code: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingKind {
    Initialize,
    Prompt,
    Shutdown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Pending {
    id: String,
    kind: PendingKind,
}

/// How far the runtime had reported a turn the session gave up on while
/// closing. The runtime may still be running it, so its remaining events can
/// arrive after the turn already has its outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AbandonedTurn {
    /// No `turn/start` yet: it, and then `turn/end`, may still arrive.
    NotStarted,
    /// Only `turn/end` may still arrive.
    Started,
}

/// Client-side state machine for one DSH session.
pub struct DshSessionMachine {
    settings: DshSessionSettings,
    limits: DshSessionLimits,
    phase: DshPhase,
    sequence: u64,
    pending: Option<Pending>,
    /// Ids of requests the session stopped waiting for before the runtime
    /// answered them. Each may still be answered once.
    abandoned_requests: Vec<String>,
    abandoned_turn: Option<AbandonedTurn>,
    server_version: Option<String>,
    current_turn: Option<DshTurnSummary>,
    last_outcome: Option<DshTurnOutcome>,
    turns_started: u64,
    completed_turns: u64,
    failed_turns: u64,
    cancelled_turns: u64,
    unverified_turns: u64,
    prompt_bytes_total: u64,
    foreign_notifications: u64,
    late_frames: u64,
    error_code: Option<&'static str>,
}

impl fmt::Debug for DshSessionMachine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DshSessionMachine")
            .field("phase", &self.phase)
            .field("turns_started", &self.turns_started)
            .field("pending", &self.pending.as_ref().map(|p| p.kind))
            .field("error_code", &self.error_code)
            .finish_non_exhaustive()
    }
}

impl DshSessionMachine {
    pub fn new(settings: DshSessionSettings, limits: DshSessionLimits) -> Result<Self, DshError> {
        settings.validate()?;
        limits.validate()?;
        Ok(Self {
            settings,
            limits,
            phase: DshPhase::Starting,
            sequence: 0,
            pending: None,
            abandoned_requests: Vec::new(),
            abandoned_turn: None,
            server_version: None,
            current_turn: None,
            last_outcome: None,
            turns_started: 0,
            completed_turns: 0,
            failed_turns: 0,
            cancelled_turns: 0,
            unverified_turns: 0,
            prompt_bytes_total: 0,
            foreign_notifications: 0,
            late_frames: 0,
            error_code: None,
        })
    }

    #[must_use]
    pub const fn phase(&self) -> DshPhase {
        self.phase
    }

    #[must_use]
    pub const fn limits(&self) -> &DshSessionLimits {
        &self.limits
    }

    #[must_use]
    pub const fn settings(&self) -> &DshSessionSettings {
        &self.settings
    }

    /// The `initialize` frame. The handshake resolves the provider route; the
    /// server rejects later prompts when it fails, so it always runs first.
    pub fn start(&mut self) -> Result<Vec<DshAction>, DshError> {
        if self.phase != DshPhase::Starting {
            return Err(DshError::Closed);
        }
        let id = self.allocate_id();
        let mut params = json!({
            "cwd": self.settings.working_directory,
            "provider": self.settings.provider,
            "model": self.settings.model,
        });
        if let Some(effort) = &self.settings.reasoning_effort {
            params["reasoningEffort"] = json!(effort);
        }
        if let Some(max_tokens) = self.settings.max_tokens {
            params["maxTokens"] = json!(max_tokens);
        }
        self.pending = Some(Pending {
            id: id.clone(),
            kind: PendingKind::Initialize,
        });
        self.phase = DshPhase::Initializing;
        Ok(vec![DshAction::Write(json!({
            "jsonrpc": wire::JSONRPC_VERSION,
            "id": id,
            "method": METHOD_INITIALIZE,
            "params": params,
        }))])
    }

    /// Admits one prompt. Refused while another turn runs: the session admits
    /// one turn at a time rather than queueing model work (ADR 032 §4).
    pub fn prompt(&mut self, prompt: &str) -> Result<Vec<DshAction>, DshError> {
        if self.phase.is_terminal() {
            return Err(DshError::Closed);
        }
        if self.phase == DshPhase::Prompting {
            return Err(DshError::TurnInFlight);
        }
        if !self.phase.accepts_prompt() {
            return Err(DshError::NotReady);
        }
        if prompt.trim().is_empty() || prompt.len() > MAX_PROMPT_BYTES {
            return Err(DshError::InvalidPrompt);
        }
        let ordinal = self.turns_started.saturating_add(1);
        let id = self.allocate_id();
        self.pending = Some(Pending {
            id: id.clone(),
            kind: PendingKind::Prompt,
        });
        self.phase = DshPhase::Prompting;
        self.turns_started = ordinal;
        self.prompt_bytes_total = self.prompt_bytes_total.saturating_add(prompt.len() as u64);
        self.current_turn = Some(DshTurnSummary {
            ordinal,
            turn: None,
            prompt_bytes: prompt.len() as u64,
            prompt_sha256: Sha256Digest::of(prompt.as_bytes()),
            outcome: None,
        });
        Ok(vec![DshAction::Write(json!({
            "jsonrpc": wire::JSONRPC_VERSION,
            "id": id,
            "method": METHOD_SESSION_PROMPT,
            "params": {
                "sessionId": self.settings.session_id,
                "contentBlocks": [{"type": "text", "text": prompt}],
            },
        }))])
    }

    /// Advances the session with one decoded frame.
    pub fn receive(&mut self, frame: &Value) -> Result<Vec<DshAction>, DshError> {
        if self.phase.is_terminal() {
            return Err(DshError::Closed);
        }
        match wire::frame_kind(frame).map_err(|error| self.violate(error))? {
            DshFrameKind::Request => {
                // Server-to-client requests are a dead capability in this
                // protocol version, but a future one must not desynchronize
                // the session: decline instead of ignoring.
                let id = frame.get("id").cloned().unwrap_or(Value::Null);
                Ok(vec![DshAction::Write(json!({
                    "jsonrpc": wire::JSONRPC_VERSION,
                    "id": id,
                    "error": {"code": -32601, "message": "Client capability not granted"},
                }))])
            }
            DshFrameKind::Notification => self.receive_notification(frame),
            DshFrameKind::Response => self.receive_response(frame),
        }
    }

    /// Writes `shutdown`. The runtime answers `{}` and exits; a runtime that
    /// does not is terminated after the configured grace period.
    pub fn begin_close(&mut self) -> Result<Vec<DshAction>, DshError> {
        if self.phase.is_terminal() {
            return Err(DshError::Closed);
        }
        let id = self.allocate_id();
        self.abandon_pending();
        self.pending = Some(Pending {
            id: id.clone(),
            kind: PendingKind::Shutdown,
        });
        self.abandon_turn(DshTurnOutcome::Cancelled);
        self.phase = DshPhase::Closing;
        Ok(vec![DshAction::Write(json!({
            "jsonrpc": wire::JSONRPC_VERSION,
            "id": id,
            "method": METHOD_SHUTDOWN,
        }))])
    }

    /// Cancels the session. The SDK protocol has no cancel method, so the
    /// answer is always to terminate the contained process tree. Frames the
    /// runtime wrote for the abandoned request and turn before it died can
    /// still arrive; they are dropped, not treated as violations.
    pub fn cancel(&mut self) -> Vec<DshAction> {
        if self.phase.is_terminal() {
            return Vec::new();
        }
        self.abandon_turn(DshTurnOutcome::Cancelled);
        self.abandon_pending();
        self.phase = DshPhase::Closing;
        vec![DshAction::Terminate]
    }

    /// Records an externally observed failure (deadline, frame ceiling,
    /// record-buffer ceiling, spawn failure) without inventing a frame.
    pub fn fail(&mut self, error: DshError) {
        if self.phase.is_terminal() {
            return;
        }
        self.violate(error);
    }

    /// Folds in the runtime process exit. A clean exit after `shutdown` closes
    /// the session; any other exit fails it, and a turn still running when the
    /// process went away is `Unverified` unless cancellation already resolved
    /// it.
    pub fn observe_exit(&mut self, clean_exit: bool) {
        if self.phase.is_terminal() {
            return;
        }
        let clean_close = self.phase == DshPhase::Closing && clean_exit;
        if clean_close {
            self.abandon_turn(DshTurnOutcome::Unverified);
            self.pending = None;
            self.phase = DshPhase::Closed;
        } else {
            self.violate(DshError::UnexpectedExit);
        }
    }

    #[must_use]
    pub fn summary(&self) -> DshSessionSummary {
        DshSessionSummary {
            session_id: self.settings.session_id.clone(),
            phase: self.phase,
            provider: self.settings.provider.clone(),
            model: self.settings.model.clone(),
            server_version: self.server_version.clone(),
            records: 0,
            record_bytes: 0,
            foreign_notifications: self.foreign_notifications,
            late_frames: self.late_frames,
            turns_started: self.turns_started,
            completed_turns: self.completed_turns,
            failed_turns: self.failed_turns,
            cancelled_turns: self.cancelled_turns,
            unverified_turns: self.unverified_turns,
            prompt_bytes_total: self.prompt_bytes_total,
            current_turn: self.current_turn,
            last_outcome: self.last_outcome,
            error_code: self.error_code,
        }
    }

    fn receive_response(&mut self, frame: &Value) -> Result<Vec<DshAction>, DshError> {
        let id = frame.get("id").and_then(Value::as_str);
        let pending = match self.pending.take() {
            Some(pending) if id == Some(pending.id.as_str()) => pending,
            other => {
                self.pending = other;
                return self.drop_abandoned_response(id);
            }
        };
        if frame.get("error").is_some() {
            return Err(self.violate(DshError::RpcError));
        }
        let Some(result) = frame.get("result").filter(|result| result.is_object()) else {
            return Err(self.violate(DshError::InvalidJsonRpc));
        };
        match pending.kind {
            PendingKind::Initialize => {
                let info = match wire::decode_server_info(result) {
                    Ok(info) => info,
                    Err(error) => return Err(self.violate(error)),
                };
                self.server_version = Some(info.version);
                self.phase = DshPhase::Ready;
                Ok(Vec::new())
            }
            PendingKind::Prompt => {
                if let Err(error) = wire::decode_prompt_receipt(result) {
                    return Err(self.violate(error));
                }
                // The receipt names the queued user message, not a turn
                // result, so the turn is still running.
                Ok(Vec::new())
            }
            PendingKind::Shutdown => {
                self.abandon_turn(DshTurnOutcome::Unverified);
                self.phase = DshPhase::Closed;
                Ok(Vec::new())
            }
        }
    }

    fn receive_notification(&mut self, frame: &Value) -> Result<Vec<DshAction>, DshError> {
        let Some(method) = frame.get("method").and_then(Value::as_str) else {
            return Err(self.violate(DshError::InvalidJsonRpc));
        };
        if !NOTIFICATION_METHODS.contains(&method) {
            // A tolerated extension: record it, do not interpret it.
            return Ok(Vec::new());
        }
        match method {
            NOTIFICATION_SESSION_EVENT | NOTIFICATION_SESSION_STATUS => {
                self.receive_session_notification(frame, method)
            }
            // `subagent.*` payloads are keyed by `parentSessionId` and
            // `childSessionId` and carry no `sessionId`, so they are never
            // correlated through the session-id path. They also never end the
            // parent turn; they are counted for diagnostics only.
            _ => {
                if wire::parent_session_id(frame) != Some(self.settings.session_id.as_str()) {
                    self.foreign_notifications = self.foreign_notifications.saturating_add(1);
                }
                Ok(Vec::new())
            }
        }
    }

    fn receive_session_notification(
        &mut self,
        frame: &Value,
        method: &str,
    ) -> Result<Vec<DshAction>, DshError> {
        let Some(session_id) = wire::notification_session_id(frame) else {
            return Err(self.violate(DshError::UncorrelatedSession));
        };
        if session_id != self.settings.session_id {
            self.foreign_notifications = self.foreign_notifications.saturating_add(1);
            return Ok(Vec::new());
        }
        if method == NOTIFICATION_SESSION_STATUS {
            // Diagnostics only. An idle whole-agent state is not proof of
            // task completion (AGENTS.md §2 invariant 5).
            return Ok(Vec::new());
        }
        self.receive_session_event(frame)
    }

    fn receive_session_event(&mut self, frame: &Value) -> Result<Vec<DshAction>, DshError> {
        match wire::session_event_type(frame) {
            Some(EVENT_TURN_START) => {
                if self.phase != DshPhase::Prompting {
                    return self.drop_abandoned_turn_event(EVENT_TURN_START);
                }
                if let Ok(turn) = wire::event_turn_number(frame)
                    && let Some(current) = self.current_turn.as_mut()
                {
                    current.turn = Some(turn);
                }
                Ok(Vec::new())
            }
            Some(EVENT_TURN_END) => self.receive_turn_end(frame),
            _ => Ok(Vec::new()),
        }
    }

    fn receive_turn_end(&mut self, frame: &Value) -> Result<Vec<DshAction>, DshError> {
        if self.phase != DshPhase::Prompting {
            return self.drop_abandoned_turn_event(EVENT_TURN_END);
        }
        let turn = match wire::event_turn_number(frame) {
            Ok(turn) => turn,
            Err(error) => return Err(self.violate(error)),
        };
        let outcome = match wire::turn_end_reason_kind(frame) {
            Some("completed") => DshTurnOutcome::Completed,
            Some("aborted") => DshTurnOutcome::Cancelled,
            Some("blocked" | "error" | "max-tokens" | "interrupted") => DshTurnOutcome::Failed,
            // A newer, still merge-extensible vocabulary. The turn is over, so
            // the session returns to `Ready`, but this adapter cannot claim it
            // succeeded.
            _ => DshTurnOutcome::Unverified,
        };
        self.resolve_turn(Some(turn), outcome);
        self.phase = DshPhase::Ready;
        Ok(Vec::new())
    }

    /// Ends the in-flight turn with `outcome`, leaving the session phase to
    /// the caller.
    fn resolve_turn(&mut self, turn: Option<u64>, outcome: DshTurnOutcome) {
        self.last_outcome = Some(outcome);
        match outcome {
            DshTurnOutcome::Completed => {
                self.completed_turns = self.completed_turns.saturating_add(1);
            }
            DshTurnOutcome::Failed => {
                self.failed_turns = self.failed_turns.saturating_add(1);
            }
            DshTurnOutcome::Cancelled => {
                self.cancelled_turns = self.cancelled_turns.saturating_add(1);
            }
            DshTurnOutcome::Unverified => {
                self.unverified_turns = self.unverified_turns.saturating_add(1);
            }
        }
        if let Some(current) = self.current_turn.as_mut()
            && current.outcome.is_none()
        {
            current.outcome = Some(outcome);
            if let Some(turn) = turn {
                current.turn = Some(turn);
            }
        }
    }

    /// Marks a still-running turn ended, for paths where the session is going
    /// away rather than the turn finishing.
    fn abandon_turn(&mut self, outcome: DshTurnOutcome) {
        if self.phase == DshPhase::Prompting {
            self.resolve_turn(None, outcome);
            let started = self.current_turn.is_some_and(|turn| turn.turn.is_some());
            self.abandoned_turn = Some(if started {
                AbandonedTurn::Started
            } else {
                AbandonedTurn::NotStarted
            });
        }
    }

    /// Drops a late event of the abandoned turn: its `turn/start`, if the
    /// runtime had not reported one, and then its `turn/end`, once each. The
    /// turn already has its outcome, and neither event changes it. Any other
    /// turn event outside a running turn is still a protocol violation.
    fn drop_abandoned_turn_event(&mut self, event_type: &str) -> Result<Vec<DshAction>, DshError> {
        self.abandoned_turn = match (self.abandoned_turn, event_type) {
            (Some(AbandonedTurn::NotStarted), EVENT_TURN_START) => Some(AbandonedTurn::Started),
            (Some(_), EVENT_TURN_END) => None,
            _ => return Err(self.violate(DshError::UnexpectedTurnEnd)),
        };
        self.late_frames = self.late_frames.saturating_add(1);
        Ok(Vec::new())
    }

    /// Stops waiting for the pending request. The runtime may already have
    /// answered it, so its id is kept and one late answer is dropped.
    fn abandon_pending(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        if self.abandoned_requests.len() == MAX_ABANDONED_REQUESTS {
            // Forget the oldest. A late answer to it then fails the session,
            // which is the conservative side.
            self.abandoned_requests.remove(0);
        }
        self.abandoned_requests.push(pending.id);
    }

    /// Drops the late answer to an abandoned request, whatever it carries:
    /// nothing waits for its result. Each abandoned id is consumed by its
    /// first answer, so a repeated answer or an id this session never
    /// abandoned is still a protocol violation, even while closing.
    fn drop_abandoned_response(&mut self, id: Option<&str>) -> Result<Vec<DshAction>, DshError> {
        let position = id.and_then(|id| {
            self.abandoned_requests
                .iter()
                .position(|abandoned| abandoned == id)
        });
        let Some(position) = position else {
            return Err(self.violate(DshError::UncorrelatedResponse));
        };
        self.abandoned_requests.remove(position);
        self.late_frames = self.late_frames.saturating_add(1);
        Ok(Vec::new())
    }

    fn violate(&mut self, error: DshError) -> DshError {
        self.error_code = Some(error.code());
        // Resolve the in-flight turn before the phase moves to `Failed`, so
        // `abandon_turn` still sees a prompting session.
        self.abandon_turn(DshTurnOutcome::Unverified);
        self.phase = DshPhase::Failed;
        self.pending = None;
        error
    }

    fn allocate_id(&mut self) -> String {
        self.sequence = self.sequence.saturating_add(1);
        format!("vibemux_{}", self.sequence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> DshSessionSettings {
        DshSessionSettings {
            session_id: "vibemux_session_1".to_string(),
            working_directory: "C:\\project".to_string(),
            provider: "deepseek-official".to_string(),
            model: "deepseek-v4-flash".to_string(),
            reasoning_effort: None,
            max_tokens: None,
        }
    }

    fn machine() -> DshSessionMachine {
        DshSessionMachine::new(settings(), DshSessionLimits::default()).expect("machine")
    }

    /// Runs `start` and the successful handshake, leaving the machine `Ready`.
    fn ready() -> DshSessionMachine {
        let mut machine = machine();
        let actions = machine.start().expect("start");
        assert_eq!(actions.len(), 1);
        let id = written_id(&actions[0]);
        let reply = json!({"jsonrpc": "2.0", "id": id,
            "result": {"serverInfo": {"name": wire::SERVER_NAME, "version": "0.0.1"}}});
        assert!(machine.receive(&reply).expect("handshake").is_empty());
        assert_eq!(machine.phase(), DshPhase::Ready);
        machine
    }

    fn written_id(action: &DshAction) -> String {
        match action {
            DshAction::Write(frame) => frame
                .get("id")
                .and_then(Value::as_str)
                .expect("request id")
                .to_string(),
            other => panic!("expected a write, got {other:?}"),
        }
    }

    fn written_method(action: &DshAction) -> String {
        match action {
            DshAction::Write(frame) => frame
                .get("method")
                .and_then(Value::as_str)
                .expect("method")
                .to_string(),
            other => panic!("expected a write, got {other:?}"),
        }
    }

    fn turn_end(session: &str, turn: u64, reason: &str) -> Value {
        json!({"jsonrpc": "2.0", "method": "session.event",
            "params": {"sessionId": session, "event": {
                "type": "turn/end", "seq": 9, "time": 1,
                "data": {"turn": turn, "reason": {"kind": reason}}}}})
    }

    fn turn_start(session: &str, turn: u64) -> Value {
        json!({"jsonrpc": "2.0", "method": "session.event",
            "params": {"sessionId": session, "event": {
                "type": "turn/start", "seq": 1, "time": 1, "data": {"turn": turn}}}})
    }

    fn receipt(id: &str) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "result": {"messageId": "msg_1"}})
    }

    #[test]
    fn start_initializes_then_hands_off_to_a_prompt() {
        let mut machine = machine();
        assert_eq!(machine.phase(), DshPhase::Starting);
        let actions = machine.start().expect("start");
        assert_eq!(written_method(&actions[0]), METHOD_INITIALIZE);
        assert_eq!(machine.phase(), DshPhase::Initializing);
        match &actions[0] {
            DshAction::Write(frame) => {
                assert_eq!(frame["params"]["cwd"], "C:\\project");
                assert_eq!(frame["params"]["provider"], "deepseek-official");
                assert_eq!(frame["params"]["model"], "deepseek-v4-flash");
                assert!(frame["params"].get("reasoningEffort").is_none());
                assert!(frame["params"].get("maxTokens").is_none());
            }
            other => panic!("expected a write, got {other:?}"),
        }
        // A second start is refused rather than re-handshaken.
        assert_eq!(machine.start(), Err(DshError::Closed));
    }

    #[test]
    fn optional_initialize_params_are_forwarded_only_when_set() {
        let mut with_options = settings();
        with_options.reasoning_effort = Some("high".to_string());
        with_options.max_tokens = Some(4096);
        let mut machine =
            DshSessionMachine::new(with_options, DshSessionLimits::default()).expect("machine");
        let actions = machine.start().expect("start");
        match &actions[0] {
            DshAction::Write(frame) => {
                assert_eq!(frame["params"]["reasoningEffort"], "high");
                assert_eq!(frame["params"]["maxTokens"], 4096);
            }
            other => panic!("expected a write, got {other:?}"),
        }
    }

    #[test]
    fn a_foreign_server_identity_fails_the_session() {
        let mut machine = machine();
        let actions = machine.start().expect("start");
        let reply = json!({"jsonrpc": "2.0", "id": written_id(&actions[0]),
            "result": {"serverInfo": {"name": "not-the-sdk-runtime", "version": "1"}}});
        assert_eq!(
            machine.receive(&reply),
            Err(DshError::InvalidServerIdentity)
        );
        assert_eq!(machine.phase(), DshPhase::Failed);
        assert_eq!(
            machine.summary().error_code,
            Some("dsh_invalid_server_identity")
        );
    }

    #[test]
    fn a_prompt_is_admitted_once_and_carries_the_clients_session_id() {
        let mut machine = ready();
        let actions = machine.prompt("hello").expect("prompt");
        assert_eq!(written_method(&actions[0]), METHOD_SESSION_PROMPT);
        match &actions[0] {
            DshAction::Write(frame) => {
                assert_eq!(frame["params"]["sessionId"], "vibemux_session_1");
                assert_eq!(frame["params"]["contentBlocks"][0]["type"], "text");
                assert_eq!(frame["params"]["contentBlocks"][0]["text"], "hello");
            }
            other => panic!("expected a write, got {other:?}"),
        }
        assert_eq!(machine.phase(), DshPhase::Prompting);
        // One turn at a time: a second prompt is refused, not queued.
        assert_eq!(machine.prompt("again"), Err(DshError::TurnInFlight));
        // The prompt never reaches Debug output.
        assert!(!format!("{actions:?}").contains("hello"));
    }

    #[test]
    fn an_empty_or_oversized_prompt_is_refused_without_starting_a_turn() {
        let mut machine = ready();
        for prompt in [String::new(), "   \n".to_string()] {
            assert_eq!(machine.prompt(&prompt), Err(DshError::InvalidPrompt));
        }
        assert_eq!(
            machine.prompt(&"x".repeat(MAX_PROMPT_BYTES + 1)),
            Err(DshError::InvalidPrompt)
        );
        assert_eq!(machine.phase(), DshPhase::Ready);
        assert_eq!(machine.summary().turns_started, 0);
    }

    #[test]
    fn a_prompt_before_the_handshake_is_not_ready() {
        let mut machine = machine();
        assert_eq!(machine.prompt("early"), Err(DshError::NotReady));
        let _ = machine.start().expect("start");
        assert_eq!(machine.prompt("early"), Err(DshError::NotReady));
    }

    #[test]
    fn a_turn_completes_only_through_a_structured_turn_end() {
        let mut machine = ready();
        let actions = machine.prompt("do it").expect("prompt");
        let reply = json!({"jsonrpc": "2.0", "id": written_id(&actions[0]),
            "result": {"messageId": "msg_1"}});
        assert!(machine.receive(&reply).expect("receipt").is_empty());
        // The receipt is not a result; the turn is still running.
        assert_eq!(machine.phase(), DshPhase::Prompting);

        // Idle status arrives first and must not complete anything.
        let idle = json!({"jsonrpc": "2.0", "method": "session.status",
            "params": {"sessionId": "vibemux_session_1", "status": "idle"}});
        assert!(machine.receive(&idle).expect("idle").is_empty());
        assert_eq!(machine.phase(), DshPhase::Prompting);
        assert_eq!(machine.summary().completed_turns, 0);

        let start = json!({"jsonrpc": "2.0", "method": "session.event",
            "params": {"sessionId": "vibemux_session_1", "event": {
                "type": "turn/start", "seq": 1, "time": 1, "data": {"turn": 7}}}});
        assert!(machine.receive(&start).expect("turn start").is_empty());
        assert!(
            machine
                .receive(&turn_end("vibemux_session_1", 7, "completed"))
                .is_ok()
        );
        assert_eq!(machine.phase(), DshPhase::Ready);
        let summary = machine.summary();
        assert_eq!(summary.completed_turns, 1);
        assert_eq!(summary.last_outcome, Some(DshTurnOutcome::Completed));
        assert_eq!(summary.current_turn.expect("turn").turn, Some(7));
        assert_eq!(summary.prompt_bytes_total, 5);
    }

    #[test]
    fn every_documented_turn_end_reason_maps_to_an_outcome() {
        for (reason, expected) in [
            ("completed", DshTurnOutcome::Completed),
            ("aborted", DshTurnOutcome::Cancelled),
            ("blocked", DshTurnOutcome::Failed),
            ("error", DshTurnOutcome::Failed),
            ("max-tokens", DshTurnOutcome::Failed),
            ("interrupted", DshTurnOutcome::Failed),
        ] {
            let mut machine = ready();
            let actions = machine.prompt("turn").expect("prompt");
            let receipt = json!({"jsonrpc": "2.0", "id": written_id(&actions[0]),
                "result": {"messageId": "msg_1"}});
            machine.receive(&receipt).expect("receipt");
            machine
                .receive(&turn_end("vibemux_session_1", 1, reason))
                .expect("turn end");
            assert_eq!(machine.summary().last_outcome, Some(expected), "{reason}");
            assert_eq!(machine.phase(), DshPhase::Ready);
        }
    }

    #[test]
    fn an_unknown_turn_end_reason_is_unverified_but_ends_the_turn() {
        let mut machine = ready();
        let actions = machine.prompt("turn").expect("prompt");
        let receipt = json!({"jsonrpc": "2.0", "id": written_id(&actions[0]),
            "result": {"messageId": "msg_1"}});
        machine.receive(&receipt).expect("receipt");
        // A newer vocabulary is not a protocol violation.
        assert!(
            machine
                .receive(&turn_end("vibemux_session_1", 1, "future-reason"))
                .is_ok()
        );
        let summary = machine.summary();
        assert_eq!(summary.last_outcome, Some(DshTurnOutcome::Unverified));
        assert_eq!(summary.completed_turns, 0);
        assert_eq!(summary.unverified_turns, 1);
        assert_eq!(machine.phase(), DshPhase::Ready);
    }

    #[test]
    fn a_missing_turn_end_reason_is_unverified() {
        let mut machine = ready();
        let actions = machine.prompt("turn").expect("prompt");
        let receipt = json!({"jsonrpc": "2.0", "id": written_id(&actions[0]),
            "result": {"messageId": "msg_1"}});
        machine.receive(&receipt).expect("receipt");
        let truncated = json!({"jsonrpc": "2.0", "method": "session.event",
            "params": {"sessionId": "vibemux_session_1",
                "event": {"type": "turn/end", "seq": 2, "time": 1, "data": {"turn": 1}}}});
        assert!(machine.receive(&truncated).is_ok());
        assert_eq!(
            machine.summary().last_outcome,
            Some(DshTurnOutcome::Unverified)
        );
    }

    #[test]
    fn a_turn_end_without_a_turn_number_is_a_protocol_violation() {
        let mut machine = ready();
        let actions = machine.prompt("turn").expect("prompt");
        let receipt = json!({"jsonrpc": "2.0", "id": written_id(&actions[0]),
            "result": {"messageId": "msg_1"}});
        machine.receive(&receipt).expect("receipt");
        let malformed = json!({"jsonrpc": "2.0", "method": "session.event",
            "params": {"sessionId": "vibemux_session_1",
                "event": {"type": "turn/end", "seq": 2, "time": 1,
                    "data": {"reason": {"kind": "completed"}}}}});
        assert_eq!(machine.receive(&malformed), Err(DshError::InvalidTurnEnd));
        assert_eq!(machine.phase(), DshPhase::Failed);
    }

    #[test]
    fn notifications_for_other_sessions_are_counted_and_ignored() {
        let mut machine = ready();
        let actions = machine.prompt("turn").expect("prompt");
        let receipt = json!({"jsonrpc": "2.0", "id": written_id(&actions[0]),
            "result": {"messageId": "msg_1"}});
        machine.receive(&receipt).expect("receipt");
        // The stream is unfiltered, so a sibling session's completion arrives.
        assert!(
            machine
                .receive(&turn_end("someone_else", 1, "completed"))
                .expect("foreign turn end")
                .is_empty()
        );
        let summary = machine.summary();
        assert_eq!(summary.foreign_notifications, 1);
        assert_eq!(summary.completed_turns, 0);
        assert_eq!(machine.phase(), DshPhase::Prompting);
        // A notification with no session id at all is a violation.
        let anonymous = json!({"jsonrpc": "2.0", "method": "session.event",
            "params": {"event": {"type": "turn/end", "data": {"turn": 1}}}});
        assert_eq!(
            machine.receive(&anonymous),
            Err(DshError::UncorrelatedSession)
        );
    }

    #[test]
    fn unknown_notification_methods_and_event_types_are_tolerated() {
        let mut machine = ready();
        for frame in [
            json!({"jsonrpc": "2.0", "method": "future.notification",
                "params": {"anything": true}}),
            json!({"method": "session.event",
                "params": {"sessionId": "vibemux_session_1",
                    "event": {"type": "future/event", "data": {}}}}),
            json!({"method": "subagent.started",
                "params": {"parentSessionId": "vibemux_session_1",
                    "childSessionId": "child"}}),
        ] {
            assert!(
                machine.receive(&frame).expect("tolerated").is_empty(),
                "{frame}"
            );
        }
        assert_eq!(machine.phase(), DshPhase::Ready);
    }

    #[test]
    fn subagent_notifications_never_correlate_through_the_session_id() {
        // Regression: `subagent.*` payloads carry no `sessionId` at all, so a
        // client that demanded one would fail the session on the first
        // subagent event. A foreign parent is counted, never fatal.
        let mut machine = ready();
        let ours = json!({"jsonrpc": "2.0", "method": "subagent.started",
            "params": {"parentSessionId": "vibemux_session_1", "childSessionId": "kid"}});
        assert!(machine.receive(&ours).expect("own subagent").is_empty());
        assert_eq!(machine.summary().foreign_notifications, 0);
        let foreign = json!({"jsonrpc": "2.0", "method": "subagent.finished",
            "params": {"provider": "p", "agentId": "a", "parentSessionId": "other",
                "childSessionId": "kid", "status": "ok", "stopReason": {}}});
        assert!(
            machine
                .receive(&foreign)
                .expect("foreign subagent")
                .is_empty()
        );
        assert_eq!(machine.summary().foreign_notifications, 1);
        assert_eq!(machine.phase(), DshPhase::Ready);
    }

    #[test]
    fn responses_correlate_with_the_pending_request() {
        let mut machine = ready();
        let actions = machine.prompt("turn").expect("prompt");
        let id = written_id(&actions[0]);
        let wrong = json!({"jsonrpc": "2.0", "id": "vibemux_999", "result": {}});
        assert_eq!(machine.receive(&wrong), Err(DshError::UncorrelatedResponse));
        assert_eq!(machine.phase(), DshPhase::Failed);
        // The failed machine refuses everything afterwards.
        assert_eq!(
            machine.receive(&json!({"jsonrpc": "2.0", "id": id, "result": {}})),
            Err(DshError::Closed)
        );
    }

    #[test]
    fn an_error_response_fails_the_session_and_keeps_no_result() {
        let mut machine = ready();
        let actions = machine.prompt("turn").expect("prompt");
        let error = json!({"jsonrpc": "2.0", "id": written_id(&actions[0]),
            "error": {"code": -32603, "message": "handler failed"}});
        assert_eq!(machine.receive(&error), Err(DshError::RpcError));
        assert_eq!(machine.phase(), DshPhase::Failed);
        assert_eq!(machine.summary().error_code, Some("dsh_rpc_error"));
    }

    #[test]
    fn a_prompt_receipt_must_carry_a_message_identity() {
        let mut machine = ready();
        let actions = machine.prompt("turn").expect("prompt");
        let receipt = json!({"jsonrpc": "2.0", "id": written_id(&actions[0]), "result": {}});
        assert_eq!(
            machine.receive(&receipt),
            Err(DshError::InvalidPromptReceipt)
        );
        assert_eq!(machine.phase(), DshPhase::Failed);
    }

    #[test]
    fn a_server_request_is_declined_without_desynchronizing() {
        let mut machine = ready();
        let request = json!({"jsonrpc": "2.0", "id": "srv_1",
            "method": "session/request_permission", "params": {}});
        let actions = machine.receive(&request).expect("declined");
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            DshAction::Write(frame) => {
                assert_eq!(frame["id"], "srv_1");
                assert_eq!(frame["error"]["code"], -32601);
            }
            other => panic!("expected a write, got {other:?}"),
        }
        assert_eq!(machine.phase(), DshPhase::Ready);
    }

    #[test]
    fn malformed_frames_fail_the_session() {
        for frame in [
            json!({}),
            json!([]),
            json!("text"),
            json!({"jsonrpc": "1.0", "id": 1}),
        ] {
            let mut machine = ready();
            let result = machine.receive(&frame);
            assert!(result.is_err(), "{frame} -> {result:?}");
            assert_eq!(machine.phase(), DshPhase::Failed);
        }
    }

    #[test]
    fn closing_writes_shutdown_then_accepts_a_clean_exit() {
        let mut machine = ready();
        let actions = machine.begin_close().expect("close");
        assert_eq!(written_method(&actions[0]), METHOD_SHUTDOWN);
        assert_eq!(machine.phase(), DshPhase::Closing);
        let reply = json!({"jsonrpc": "2.0", "id": written_id(&actions[0]), "result": {}});
        assert!(machine.receive(&reply).expect("shutdown reply").is_empty());
        assert_eq!(machine.phase(), DshPhase::Closed);
        machine.observe_exit(true);
        assert_eq!(machine.phase(), DshPhase::Closed);
        // A closed session refuses further work.
        assert_eq!(machine.prompt("again"), Err(DshError::Closed));
        assert!(machine.cancel().is_empty());
    }

    #[test]
    fn closing_while_a_turn_runs_cancels_that_turn() {
        let mut machine = ready();
        machine.prompt("turn").expect("prompt");
        machine.begin_close().expect("close");
        let summary = machine.summary();
        assert_eq!(summary.cancelled_turns, 1);
        assert_eq!(
            summary.current_turn.expect("turn").outcome,
            Some(DshTurnOutcome::Cancelled)
        );
    }

    #[test]
    fn cancellation_terminates_because_the_protocol_has_no_cancel_method() {
        let mut machine = ready();
        machine.prompt("turn").expect("prompt");
        assert_eq!(machine.cancel(), vec![DshAction::Terminate]);
        assert_eq!(machine.phase(), DshPhase::Closing);
        assert_eq!(machine.summary().cancelled_turns, 1);
        machine.observe_exit(true);
        assert_eq!(machine.phase(), DshPhase::Closed);
    }

    #[test]
    fn a_prompt_answered_after_cancel_is_dropped_and_the_session_still_closes() {
        // Regression: the driver returns from `prompt` once the frame is
        // written, so cancelling at once can beat the runtime's receipt. The
        // late receipt and the turn's first events then arrive while closing.
        let mut machine = ready();
        let prompt = written_id(&machine.prompt("turn").expect("prompt")[0]);
        assert_eq!(machine.cancel(), vec![DshAction::Terminate]);
        for late in [
            receipt(&prompt),
            turn_start("vibemux_session_1", 1),
            turn_end("vibemux_session_1", 1, "aborted"),
        ] {
            assert!(machine.receive(&late).expect("late frame").is_empty());
            assert_eq!(machine.phase(), DshPhase::Closing);
        }
        machine.observe_exit(true);
        assert_eq!(machine.phase(), DshPhase::Closed);
        let summary = machine.summary();
        assert_eq!(summary.error_code, None);
        assert_eq!(summary.late_frames, 3);
        // The late events never re-resolve the cancelled turn.
        assert_eq!(summary.cancelled_turns, 1);
        assert_eq!(summary.failed_turns, 0);
        assert_eq!(summary.unverified_turns, 0);
        assert_eq!(
            summary.current_turn.expect("turn").outcome,
            Some(DshTurnOutcome::Cancelled)
        );
    }

    #[test]
    fn a_prompt_answered_after_shutdown_was_written_is_dropped() {
        // `begin_close` supersedes the unanswered prompt; the runtime answers
        // it, finishes the turn, and only then answers `shutdown`.
        let mut machine = ready();
        let prompt = written_id(&machine.prompt("turn").expect("prompt")[0]);
        let shutdown = written_id(&machine.begin_close().expect("close")[0]);
        for late in [
            receipt(&prompt),
            turn_start("vibemux_session_1", 1),
            turn_end("vibemux_session_1", 1, "completed"),
        ] {
            assert!(machine.receive(&late).expect("late frame").is_empty());
            assert_eq!(machine.phase(), DshPhase::Closing);
        }
        let reply = json!({"jsonrpc": "2.0", "id": shutdown, "result": {}});
        assert!(machine.receive(&reply).expect("shutdown reply").is_empty());
        assert_eq!(machine.phase(), DshPhase::Closed);
        let summary = machine.summary();
        assert_eq!(summary.error_code, None);
        assert_eq!(summary.late_frames, 3);
        // Closing already cancelled the turn; a late completion is not success.
        assert_eq!(summary.completed_turns, 0);
        assert_eq!(summary.cancelled_turns, 1);
    }

    #[test]
    fn a_late_error_answer_to_an_abandoned_request_is_dropped() {
        let mut machine = ready();
        let prompt = written_id(&machine.prompt("turn").expect("prompt")[0]);
        machine.cancel();
        let error = json!({"jsonrpc": "2.0", "id": prompt,
            "error": {"code": -32603, "message": "session closed"}});
        assert!(machine.receive(&error).expect("late error").is_empty());
        assert_eq!(machine.phase(), DshPhase::Closing);
        assert_eq!(machine.summary().late_frames, 1);
    }

    #[test]
    fn an_abandoned_request_is_answered_at_most_once() {
        let mut machine = ready();
        let prompt = written_id(&machine.prompt("turn").expect("prompt")[0]);
        machine.cancel();
        machine.receive(&receipt(&prompt)).expect("late receipt");
        // A second answer to the same id is a malformed runtime.
        assert_eq!(
            machine.receive(&receipt(&prompt)),
            Err(DshError::UncorrelatedResponse)
        );
        assert_eq!(machine.phase(), DshPhase::Failed);
        assert_eq!(
            machine.summary().error_code,
            Some("dsh_uncorrelated_response")
        );
    }

    #[test]
    fn an_unknown_response_id_while_closing_still_fails_the_session() {
        for close_by_cancel in [true, false] {
            let mut machine = ready();
            machine.prompt("turn").expect("prompt");
            if close_by_cancel {
                machine.cancel();
            } else {
                machine.begin_close().expect("close");
            }
            assert_eq!(machine.phase(), DshPhase::Closing);
            assert_eq!(
                machine.receive(&receipt("vibemux_999")),
                Err(DshError::UncorrelatedResponse),
                "cancel: {close_by_cancel}"
            );
            assert_eq!(machine.phase(), DshPhase::Failed);
        }
    }

    #[test]
    fn an_abandoned_turn_reports_each_remaining_event_once() {
        // Not started when abandoned: one `turn/start`, then one `turn/end`.
        let mut machine = ready();
        machine.prompt("turn").expect("prompt");
        machine.cancel();
        machine
            .receive(&turn_start("vibemux_session_1", 1))
            .expect("late start");
        assert_eq!(
            machine.receive(&turn_start("vibemux_session_1", 1)),
            Err(DshError::UnexpectedTurnEnd)
        );

        // Already started when abandoned: only its `turn/end` may follow.
        let mut machine = ready();
        let prompt = written_id(&machine.prompt("turn").expect("prompt")[0]);
        machine.receive(&receipt(&prompt)).expect("receipt");
        machine
            .receive(&turn_start("vibemux_session_1", 1))
            .expect("turn start");
        machine.cancel();
        assert_eq!(
            machine.receive(&turn_start("vibemux_session_1", 2)),
            Err(DshError::UnexpectedTurnEnd)
        );

        let mut machine = ready();
        machine.prompt("turn").expect("prompt");
        machine.cancel();
        machine
            .receive(&turn_end("vibemux_session_1", 1, "aborted"))
            .expect("late end");
        assert_eq!(
            machine.receive(&turn_end("vibemux_session_1", 1, "aborted")),
            Err(DshError::UnexpectedTurnEnd)
        );
    }

    #[test]
    fn a_turn_event_while_closing_an_idle_session_is_still_a_violation() {
        // No turn was abandoned, so nothing is expected to arrive late.
        let mut machine = ready();
        machine.begin_close().expect("close");
        assert_eq!(
            machine.receive(&turn_end("vibemux_session_1", 1, "completed")),
            Err(DshError::UnexpectedTurnEnd)
        );
        assert_eq!(machine.phase(), DshPhase::Failed);
    }

    #[test]
    fn only_the_most_recent_abandoned_requests_are_remembered() {
        let mut machine = ready();
        let prompt = written_id(&machine.prompt("turn").expect("prompt")[0]);
        // Each repeated close abandons the previous `shutdown`, so the prompt
        // is the oldest id and is forgotten once the bound is exceeded.
        let mut newest = String::new();
        for _ in 0..=MAX_ABANDONED_REQUESTS {
            newest = written_id(&machine.begin_close().expect("close")[0]);
        }
        machine.cancel();
        machine
            .receive(&json!({"jsonrpc": "2.0", "id": newest, "result": {}}))
            .expect("newest abandoned shutdown");
        assert_eq!(
            machine.receive(&receipt(&prompt)),
            Err(DshError::UncorrelatedResponse)
        );
    }

    #[test]
    fn an_unexpected_exit_fails_the_session_and_unverifies_the_turn() {
        let mut machine = ready();
        machine.prompt("turn").expect("prompt");
        machine.observe_exit(false);
        assert_eq!(machine.phase(), DshPhase::Failed);
        let summary = machine.summary();
        assert_eq!(summary.error_code, Some("dsh_unexpected_exit"));
        assert_eq!(summary.unverified_turns, 1);
        assert_eq!(summary.completed_turns, 0);
    }

    #[test]
    fn an_exit_during_initialize_fails_the_session() {
        let mut machine = machine();
        machine.start().expect("start");
        machine.observe_exit(true);
        assert_eq!(machine.phase(), DshPhase::Failed);
        assert_eq!(machine.summary().error_code, Some("dsh_unexpected_exit"));
    }

    #[test]
    fn an_externally_reported_deadline_records_a_stable_code() {
        let mut machine = ready();
        machine.prompt("turn").expect("prompt");
        machine.fail(DshError::RecordBufferFull);
        assert_eq!(machine.phase(), DshPhase::Failed);
        assert_eq!(machine.summary().error_code, Some("dsh_record_buffer_full"));
        // A failure is not overwritten by a second report.
        machine.fail(DshError::NotReady);
        assert_eq!(machine.summary().error_code, Some("dsh_record_buffer_full"));
    }

    #[test]
    fn consecutive_turns_advance_the_ordinal_and_keep_counters() {
        let mut machine = ready();
        for ordinal in 1..=3 {
            let actions = machine.prompt("turn").expect("prompt");
            let receipt = json!({"jsonrpc": "2.0", "id": written_id(&actions[0]),
                "result": {"messageId": "msg"}});
            machine.receive(&receipt).expect("receipt");
            assert!(
                machine
                    .receive(&turn_end("vibemux_session_1", ordinal, "completed"))
                    .is_ok()
            );
            assert_eq!(
                machine.summary().current_turn.expect("turn").ordinal,
                ordinal
            );
        }
        let summary = machine.summary();
        assert_eq!(summary.turns_started, 3);
        assert_eq!(summary.completed_turns, 3);
        assert_eq!(summary.prompt_bytes_total, 12);
        // Request ids never repeat inside one session. The handshake took one
        // id and each of the three turns took one, so the next is the fifth.
        let actions = machine.prompt("turn").expect("prompt");
        assert_eq!(written_id(&actions[0]), "vibemux_5");
    }

    #[test]
    fn request_ids_are_distinct_across_start_prompt_and_close() {
        let mut machine = machine();
        let start = machine.start().expect("start");
        let initialize = written_id(&start[0]);
        let reply = json!({"jsonrpc": "2.0", "id": initialize,
            "result": {"serverInfo": {"name": wire::SERVER_NAME, "version": "1"}}});
        machine.receive(&reply).expect("handshake");
        let prompt = machine.prompt("turn").expect("prompt");
        let close = machine.begin_close().expect("close");
        assert_ne!(initialize, written_id(&prompt[0]));
        assert_ne!(written_id(&prompt[0]), written_id(&close[0]));
    }

    #[test]
    fn the_summary_carries_no_prompt_text() {
        let mut machine = ready();
        machine.prompt("super secret prompt body").expect("prompt");
        let encoded = serde_json::to_string(&machine.summary()).expect("summary");
        assert!(!encoded.contains("super secret"));
        assert!(!encoded.contains("C:\\\\project"));
        assert!(encoded.contains("\"prompt_bytes\":24"));
    }

    #[test]
    fn settings_and_limits_are_validated_before_construction() {
        let mut invalid = settings();
        invalid.session_id = String::new();
        assert_eq!(
            DshSessionMachine::new(invalid, DshSessionLimits::default()).err(),
            Some(DshError::InvalidSessionId)
        );
        let invalid_limits = DshSessionLimits {
            frame_bytes: 0,
            ..DshSessionLimits::default()
        };
        assert_eq!(
            DshSessionMachine::new(settings(), invalid_limits).err(),
            Some(DshError::ConfigInvalid)
        );
    }
}
