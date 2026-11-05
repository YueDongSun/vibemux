//! Exact root-session and input-UUID correlation for Claude streaming input.
//! A serialized stream alone is never sufficient terminal evidence.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use uuid::Uuid;

use crate::dispatch::Sha256Digest;

use super::{
    CLAUDE_HELP_VERSION, DeniedCapability, IgnoredReason, SessionError, SessionEvent,
    SessionMachine, SessionPhase, SessionTransition, TurnOutcome,
    session::{RpcKind, identifier},
};

const CLAUDE_BUILTIN_PLUGINS: [&str; 3] = [
    "cc-plugin-agents-md",
    "cc-plugin-telemetry",
    "cc-plugin-plugin-authoring",
];

fn supported_plugins(initialization: &Value) -> bool {
    let Some(value) = initialization.get("plugins") else {
        return true;
    };
    let Some(plugins) = value.as_array() else {
        return false;
    };
    if plugins.is_empty() {
        return true;
    }
    if plugins.len() > CLAUDE_BUILTIN_PLUGINS.len()
        || initialization
            .get("claude_code_version")
            .and_then(Value::as_str)
            != Some(CLAUDE_HELP_VERSION)
    {
        return false;
    }
    let mut seen = BTreeSet::new();
    plugins.iter().all(|plugin| {
        let Some(name) = plugin.get("name").and_then(Value::as_str) else {
            return false;
        };
        CLAUDE_BUILTIN_PLUGINS.contains(&name)
            && plugin.get("source").and_then(Value::as_str)
                == Some(format!("{name}@builtin").as_str())
            && seen.insert(name)
    })
}

#[derive(Clone, Copy)]
enum BlockKind {
    Text,
    Thinking,
    Unknown,
}

#[derive(Default)]
pub(super) struct ClaudeState {
    records: BTreeMap<Uuid, Sha256Digest>,
    /// Actual root system/init evidence after every effective-profile check.
    /// Transport initialization and echoed input IDs never populate this.
    root_init_session_id: Option<String>,
    pending_ack: Option<Uuid>,
    pub(super) bound_messages: BTreeSet<String>,
    message: Option<String>,
    block: Option<(u64, BlockKind)>,
    next_block: u64,
    message_has_text: bool,
    result_index: Option<u64>,
}

impl ClaudeState {
    pub(super) fn reset_turn(&mut self) {
        // Root initialization qualifies this process, not just its first turn.
        self.pending_ack = None;
        self.bound_messages.clear();
        self.message = None;
        self.block = None;
        self.next_block = 0;
        self.message_has_text = false;
    }
}

impl SessionMachine {
    pub(super) fn receive_claude(
        &mut self,
        value: Value,
        digest: Sha256Digest,
    ) -> Result<SessionTransition, SessionError> {
        if self.fixture_file_approval_enabled()
            && value.get("type").and_then(Value::as_str) != Some("control_response")
        {
            return self.receive_fixture_approval(value, digest);
        }
        let kind = identifier(value.get("type"))?;
        match kind.as_str() {
            "control_request" => return self.deny_claude_request(&value),
            "control_response" => return self.claude_control_response(&value),
            _ => {}
        }
        if !matches!(
            kind.as_str(),
            "system" | "user" | "assistant" | "stream_event" | "result"
        ) || (kind == "system" && value.get("subtype").and_then(Value::as_str) != Some("init"))
        {
            // Keepalive, cancelled server requests and future informational
            // records have no authority to bind a session, publish text or
            // complete/cancel a turn. Frame/sequence budgets still apply.
            return Ok(Self::ignored(IgnoredReason::UnknownNotification));
        }
        if !root_record(&value) {
            return Ok(Self::ignored(IgnoredReason::Nested));
        }
        if kind != "user" && value.get("isReplay") == Some(&Value::Bool(true)) {
            return Ok(Self::ignored(IgnoredReason::Historical));
        }
        if value
            .get("resume_reason")
            .is_some_and(|reason| !reason.is_null())
        {
            return Err(SessionError::UnsolicitedResume);
        }
        if value.get("origin").is_some_and(|origin| {
            !origin.is_null() && origin.get("kind").and_then(Value::as_str) != Some("human")
        }) {
            return Ok(Self::ignored(IgnoredReason::Historical));
        }
        let record_id = uuid(value.get("uuid"))?;
        if let Some(previous) = self.claude.records.get(&record_id) {
            return if *previous == digest {
                Ok(Self::ignored(IgnoredReason::Duplicate))
            } else {
                Err(SessionError::ConflictingRecord)
            };
        }
        self.claude.records.insert(record_id, digest);
        let session_id = identifier(value.get("session_id"))?;
        if self.config.target.expected_id().as_deref() != Some(session_id.as_str()) {
            return Err(SessionError::UncorrelatedSession);
        }

        match kind.as_str() {
            "system" if value.get("subtype").and_then(Value::as_str) == Some("init") => {
                self.claude_initialized(&value, &session_id)
            }
            "user" => {
                if self.is_retired_request(record_id) {
                    return Ok(Self::ignored(IgnoredReason::RetiredTurn));
                }
                if value.get("isReplay") == Some(&Value::Bool(true))
                    && self
                        .active
                        .as_ref()
                        .is_none_or(|active| active.summary.request_id != record_id)
                {
                    return Ok(Self::ignored(IgnoredReason::Historical));
                }
                let active = self.active.as_ref().ok_or(SessionError::UncorrelatedTurn)?;
                if active.summary.request_id != record_id
                    || value.pointer("/message/role").and_then(Value::as_str) != Some("user")
                {
                    return Err(SessionError::UncorrelatedTurn);
                }
                if self.claude.root_init_session_id.is_none() || self.vendor_session_id.is_none() {
                    self.claude.pending_ack = Some(record_id);
                    return Ok(SessionTransition::default());
                }
                self.require_claude_init(&session_id)?;
                let mut transition = self.bind_session(&session_id)?;
                transition.extend(self.acknowledge()?);
                Ok(transition)
            }
            "stream_event" => self.claude_stream(&value, &session_id),
            "assistant" => self.claude_assistant(&value, &session_id),
            "result" => self.claude_result(&value, &session_id),
            _ => Ok(Self::ignored(IgnoredReason::UnknownNotification)),
        }
    }

    fn claude_control_response(
        &mut self,
        value: &Value,
    ) -> Result<SessionTransition, SessionError> {
        let id = identifier(value.pointer("/response/request_id"))?;
        let Some(kind) = self.take_rpc(&id)? else {
            return Ok(Self::ignored(IgnoredReason::Duplicate));
        };
        let success = match value.pointer("/response/subtype").and_then(Value::as_str) {
            Some("success") => true,
            Some("error") => false,
            _ => return Err(SessionError::InvalidFrame),
        };
        match kind {
            RpcKind::Initialize => {
                if !success {
                    return Err(SessionError::VendorRequestFailed);
                }
                if self.phase != SessionPhase::Initializing {
                    return Err(SessionError::UnexpectedState);
                }
                // The documented initialize response has no session_id. Ready
                // means transport-ready until validated root system/init proves
                // the pinned ID and effective profile.
                self.phase = SessionPhase::Ready;
                let mut transition = SessionTransition {
                    events: vec![SessionEvent::Initialized],
                    ..Default::default()
                };
                if self.fixture_file_approval_enabled() {
                    transition.extend(self.fixture_approval_progress()?);
                } else if self.claude.root_init_session_id.is_some() {
                    transition.extend(self.publish_claude_binding()?);
                }
                Ok(transition)
            }
            RpcKind::Interrupt(turn_id) => {
                if self
                    .active
                    .as_ref()
                    .is_none_or(|active| active.summary.turn_id != turn_id)
                {
                    return Ok(Self::ignored(IgnoredReason::RetiredTurn));
                }
                Ok(SessionTransition {
                    events: vec![if success {
                        SessionEvent::CancellationAcknowledged { turn_id }
                    } else {
                        SessionEvent::CancellationUnconfirmed { turn_id }
                    }],
                    ..Default::default()
                })
            }
            _ => Err(SessionError::UncorrelatedResponse),
        }
    }

    fn claude_initialized(
        &mut self,
        value: &Value,
        session_id: &str,
    ) -> Result<SessionTransition, SessionError> {
        if !matches!(
            self.phase,
            SessionPhase::Initializing | SessionPhase::Ready | SessionPhase::Busy
        ) {
            return Err(SessionError::UnexpectedState);
        }
        if value.get("cwd").and_then(Value::as_str) != Some(self.config.working_directory.as_str())
            || value.get("permissionMode").and_then(Value::as_str) != Some("dontAsk")
            || !value
                .get("tools")
                .and_then(Value::as_array)
                .is_some_and(Vec::is_empty)
            || !value
                .get("mcp_servers")
                .and_then(Value::as_array)
                .is_some_and(Vec::is_empty)
            || !supported_plugins(value)
        {
            return Err(SessionError::UnsafeProfile);
        }
        if self
            .config
            .model
            .as_ref()
            .is_some_and(|model| value.get("model").and_then(Value::as_str) != Some(model.as_str()))
        {
            return Err(SessionError::UnsafeProfile);
        }
        self.claude.root_init_session_id = Some(session_id.to_owned());
        if self.phase == SessionPhase::Initializing {
            // An early init is retained as proof, but the correlated control
            // handshake must still succeed before the session becomes usable.
            return Ok(SessionTransition::default());
        }
        self.publish_claude_binding()
    }

    fn publish_claude_binding(&mut self) -> Result<SessionTransition, SessionError> {
        let session_id = self
            .claude
            .root_init_session_id
            .clone()
            .ok_or(SessionError::UnsafeProfile)?;
        let mut transition = self.bind_session(&session_id)?;
        if let Some(request_id) = self.claude.pending_ack.take() {
            if self
                .active
                .as_ref()
                .is_none_or(|active| active.summary.request_id != request_id)
            {
                return Err(SessionError::UncorrelatedTurn);
            }
            transition.extend(self.acknowledge()?);
        }
        Ok(transition)
    }

    fn require_claude_init(&self, session_id: &str) -> Result<(), SessionError> {
        if self.claude.root_init_session_id.as_deref() != Some(session_id)
            || self.vendor_session_id.as_deref() != Some(session_id)
        {
            Err(SessionError::UnsafeProfile)
        } else {
            Ok(())
        }
    }

    /// A literal echo is mandatory for first reply/root terminal. Chunks that
    /// omit it can only use a message_start already bound by that echo.
    fn claude_request(
        &self,
        value: &Value,
        required: bool,
    ) -> Result<Option<IgnoredReason>, SessionError> {
        let active = self.active.as_ref().ok_or(SessionError::UncorrelatedTurn)?;
        if let Some(echo) = value.get("user_message_uuid") {
            let echoed = uuid(Some(echo))?;
            if self.is_retired_request(echoed) {
                return Ok(Some(IgnoredReason::RetiredTurn));
            }
            if echoed != active.summary.request_id {
                return Err(SessionError::UncorrelatedTurn);
            }
            if let Some(echoes) = value.get("user_message_uuids") {
                let echoes = echoes.as_array().ok_or(SessionError::InvalidFrame)?;
                if echoes.len() != 1 || uuid(echoes.first())? != active.summary.request_id {
                    return Err(SessionError::UncorrelatedTurn);
                }
            }
        } else if required || value.get("user_message_uuids").is_some() {
            return Err(SessionError::MissingCorrelation);
        }
        Ok(None)
    }

    fn claude_stream(
        &mut self,
        value: &Value,
        session_id: &str,
    ) -> Result<SessionTransition, SessionError> {
        let event = value
            .get("event")
            .filter(|event| event.is_object())
            .ok_or(SessionError::InvalidFrame)?;
        let event_type = identifier(event.get("type"))?;
        if event_type == "ping" {
            return Ok(Self::ignored(IgnoredReason::UnknownNotification));
        }
        self.require_claude_init(session_id)?;
        let first = event_type == "message_start";
        if let Some(reason) = self.claude_request(value, first)? {
            return Ok(Self::ignored(reason));
        }
        let mut transition = self.bind_session(session_id)?;
        if first {
            if self.claude.message.is_some() || self.claude.block.is_some() {
                return Err(SessionError::InvalidFrame);
            }
            let message = identifier(event.pointer("/message/id"))?;
            if event.pointer("/message/role").and_then(Value::as_str) != Some("assistant") {
                return Err(SessionError::InvalidFrame);
            }
            if self.retired_messages.contains(&message) {
                return Ok(Self::ignored(IgnoredReason::RetiredTurn));
            }
            if self.claude.bound_messages.contains(&message) {
                return Err(SessionError::ConflictingRecord);
            }
            if self.claude.bound_messages.len() >= self.limits.max_text_items {
                return Err(SessionError::TextItemLimit);
            }
            self.claude.bound_messages.insert(message.clone());
            self.claude.message = Some(message);
            self.claude.next_block = 0;
            self.claude.message_has_text = false;
            transition.extend(self.acknowledge()?);
            return Ok(transition);
        }
        let message = self
            .claude
            .message
            .clone()
            .ok_or(SessionError::MissingCorrelation)?;
        match event_type.as_str() {
            "content_block_start" => {
                let index = event
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or(SessionError::InvalidFrame)?;
                if self.claude.block.is_some()
                    || index != self.claude.next_block
                    || index >= self.limits.max_text_items as u64
                {
                    return Err(SessionError::InvalidFrame);
                }
                self.claude.next_block += 1;
                let block = event
                    .get("content_block")
                    .filter(|block| block.is_object())
                    .ok_or(SessionError::InvalidFrame)?;
                let kind = match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        let text = block
                            .get("text")
                            .and_then(Value::as_str)
                            .ok_or(SessionError::InvalidFrame)?;
                        if self.claude.message_has_text {
                            transition
                                .text
                                .push(self.text_update(&message, "\n\n", true)?);
                        }
                        transition
                            .text
                            .push(self.text_update(&message, text, true)?);
                        self.claude.message_has_text = true;
                        BlockKind::Text
                    }
                    Some("thinking" | "redacted_thinking") => BlockKind::Thinking,
                    Some("tool_use" | "server_tool_use") => {
                        return Err(SessionError::UnsafeProfile);
                    }
                    _ => BlockKind::Unknown,
                };
                self.claude.block = Some((index, kind));
            }
            "content_block_delta" => {
                let index = event
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or(SessionError::InvalidFrame)?;
                let (current, kind) = self.claude.block.ok_or(SessionError::InvalidFrame)?;
                if index != current {
                    return Err(SessionError::InvalidFrame);
                }
                match (kind, event.pointer("/delta/type").and_then(Value::as_str)) {
                    (BlockKind::Text, Some("text_delta")) => {
                        let delta = event
                            .pointer("/delta/text")
                            .and_then(Value::as_str)
                            .ok_or(SessionError::InvalidFrame)?;
                        transition
                            .text
                            .push(self.text_update(&message, delta, true)?);
                    }
                    (_, Some("input_json_delta")) => return Err(SessionError::UnsafeProfile),
                    (BlockKind::Thinking | BlockKind::Unknown, _) => {}
                    _ => return Err(SessionError::InvalidFrame),
                }
            }
            "content_block_stop" => {
                let index = event
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or(SessionError::InvalidFrame)?;
                if self
                    .claude
                    .block
                    .is_none_or(|(current, _)| current != index)
                {
                    return Err(SessionError::InvalidFrame);
                }
                self.claude.block = None;
            }
            "message_stop" => {
                if self.claude.block.is_some() {
                    return Err(SessionError::InvalidFrame);
                }
                self.claude.message = None;
            }
            "message_delta" => {}
            _ => transition.extend(Self::ignored(IgnoredReason::UnknownNotification)),
        }
        Ok(transition)
    }

    fn claude_assistant(
        &mut self,
        value: &Value,
        session_id: &str,
    ) -> Result<SessionTransition, SessionError> {
        self.require_claude_init(session_id)?;
        let message_id = identifier(value.pointer("/message/id"))?;
        if self.retired_messages.contains(&message_id) {
            return Ok(Self::ignored(IgnoredReason::RetiredTurn));
        }
        let known = self.claude.bound_messages.contains(&message_id);
        if let Some(reason) = self.claude_request(value, !known)? {
            return Ok(Self::ignored(reason));
        }
        if value.pointer("/message/role").and_then(Value::as_str) != Some("assistant") {
            return Err(SessionError::InvalidFrame);
        }
        if !known {
            if self.claude.bound_messages.len() >= self.limits.max_text_items {
                return Err(SessionError::TextItemLimit);
            }
            self.claude.bound_messages.insert(message_id.clone());
        }
        let content = value
            .pointer("/message/content")
            .and_then(Value::as_array)
            .ok_or(SessionError::InvalidFrame)?;
        if content.len() > self.limits.max_text_items {
            return Err(SessionError::TextItemLimit);
        }
        let mut pieces = Vec::new();
        for block in content {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => pieces.push(
                    block
                        .get("text")
                        .and_then(Value::as_str)
                        .ok_or(SessionError::InvalidFrame)?,
                ),
                Some("tool_use" | "server_tool_use") => return Err(SessionError::UnsafeProfile),
                _ => {}
            }
        }
        let mut transition = self.bind_session(session_id)?;
        transition.extend(self.acknowledge()?);
        transition
            .text
            .push(self.text_update(&message_id, &pieces.join("\n\n"), false)?);
        Ok(transition)
    }

    fn claude_result(
        &mut self,
        value: &Value,
        session_id: &str,
    ) -> Result<SessionTransition, SessionError> {
        self.require_claude_init(session_id)?;
        if let Some(echo) = value.get("user_message_uuid")
            && self.is_retired_request(uuid(Some(echo))?)
        {
            return Ok(Self::ignored(IgnoredReason::RetiredTurn));
        }
        if let Some(reason) = self.claude_request(value, true)? {
            return Ok(Self::ignored(reason));
        }
        self.claude_terminal(value)
    }

    pub(super) fn claude_terminal(
        &mut self,
        value: &Value,
    ) -> Result<SessionTransition, SessionError> {
        let session_id = identifier(value.get("session_id"))?;
        if let Some(index) = value.get("result_index") {
            let index = index.as_u64().ok_or(SessionError::InvalidFrame)?;
            let expected = self
                .claude
                .result_index
                .map_or(Some(0), |previous| previous.checked_add(1))
                .ok_or(SessionError::RecordBudgetExhausted)?;
            if index != expected {
                return Err(SessionError::SequenceGap);
            }
            self.claude.result_index = Some(index);
        }
        let is_error = value
            .get("is_error")
            .and_then(Value::as_bool)
            .ok_or(SessionError::InvalidTerminal)?;
        let outcome = match value.get("terminal_reason").and_then(Value::as_str) {
            Some("aborted_streaming" | "aborted_tools") => TurnOutcome::Cancelled,
            _ if is_error => TurnOutcome::Failed,
            None | Some("completed")
                if value.get("subtype").and_then(Value::as_str) == Some("success") =>
            {
                TurnOutcome::Completed
            }
            Some(
                "max_turns"
                | "hook_stopped"
                | "stop_hook_prevented"
                | "blocking_limit"
                | "rapid_refill_breaker"
                | "prompt_too_long"
                | "image_error"
                | "model_error"
                | "api_error"
                | "budget_exhausted"
                | "turn_setup_failed"
                | "structured_output_retry_exhausted",
            ) => TurnOutcome::Failed,
            _ => return Err(SessionError::InvalidTerminal),
        };
        let mut transition = self.bind_session(&session_id)?;
        transition.extend(self.acknowledge()?);
        if let Some(text) = value.get("result").and_then(Value::as_str) {
            transition.text.push(self.final_text(text)?);
        }
        transition.extend(self.finish_turn(outcome)?);
        Ok(transition)
    }

    fn deny_claude_request(&mut self, value: &Value) -> Result<SessionTransition, SessionError> {
        let id = identifier(value.get("request_id"))?;
        let digest = Sha256Digest::of(value.to_string().as_bytes());
        if let Some(previous) = self.server_requests.get(&id) {
            return if *previous == digest {
                Ok(Self::ignored(IgnoredReason::Duplicate))
            } else {
                Err(SessionError::ConflictingRecord)
            };
        }
        if self.server_requests.len() >= self.limits.max_rpc_requests {
            return Err(SessionError::RpcBudgetExhausted);
        }
        self.server_requests.insert(id.clone(), digest);
        let capability =
            if value.pointer("/request/subtype").and_then(Value::as_str) == Some("can_use_tool") {
                DeniedCapability::Tool
            } else {
                DeniedCapability::Unknown
            };
        Ok(SessionTransition {
            effects: vec![self.frame(json!({"type":"control_response","response":{
                "subtype":"error","request_id":id,"error":"Client capability not granted"
            }}))?],
            events: vec![SessionEvent::CapabilityDenied { capability }],
            text: Vec::new(),
            approvals: Vec::new(),
            ..Default::default()
        })
    }
}

fn root_record(value: &Value) -> bool {
    ["parent_tool_use_id", "agent_id", "parent_agent_id"]
        .iter()
        .all(|key| value.get(key).is_none_or(Value::is_null))
}

fn uuid(value: Option<&Value>) -> Result<Uuid, SessionError> {
    let id = identifier(value)?;
    Uuid::parse_str(&id)
        .ok()
        .filter(|id| !id.is_nil())
        .ok_or(SessionError::InvalidIdentifier)
}
