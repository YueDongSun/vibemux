//! Closed-fixture approval protocol. No native vendor qualification is implied.

use std::collections::{BTreeMap, BTreeSet};
use std::io;

use serde_json::{Value, json};
use uuid::Uuid;
use vibemux_types::{ToolName, chat_approval::ChatApprovalDecision};

use crate::dispatch::Sha256Digest;

use super::{
    ApprovalChallenge, IgnoredReason, SessionEffect, SessionError, SessionLimits, SessionMachine,
    SessionPhase, SessionTransition, WireFrame, session::identifier,
};

#[cfg(any(test, feature = "test_helpers"))]
use super::{SessionConfig, SessionTarget};

const MAX_APPROVAL_INPUT_BYTES: usize = 256 * 1024;

struct ToolEvidence {
    id: String,
    input: Value,
}

struct PermissionRequest {
    id: String,
    tool_id: String,
    input: Value,
}

struct PendingApproval {
    challenge: ApprovalChallenge,
    decision: Option<ChatApprovalDecision>,
}

struct StreamTool {
    id: String,
    index: u64,
    input_json: String,
    initial_nonempty: bool,
    has_delta: bool,
}

struct StreamMessage {
    id: String,
    next_index: u64,
    block: Option<u64>,
    tool: Option<StreamTool>,
}

#[derive(Default)]
pub(super) struct FixtureApprovalState {
    prepared: Option<(Uuid, Uuid)>,
    input_written: bool,
    root_init: bool,
    records: BTreeMap<Uuid, Sha256Digest>,
    requests: BTreeMap<String, Sha256Digest>,
    messages: BTreeSet<String>,
    retired_tools: BTreeSet<String>,
    evidence: BTreeMap<String, ToolEvidence>,
    request: Option<PermissionRequest>,
    pending: Option<PendingApproval>,
    stream: Option<StreamMessage>,
    terminal: Option<Value>,
    retained_frames: usize,
    retained_bytes: usize,
}

impl FixtureApprovalState {
    pub(super) fn prepare(&mut self, request: Uuid, turn: Uuid) -> Result<(), SessionError> {
        if self.prepared.is_some() {
            return Err(SessionError::TurnBudgetExhausted);
        }
        self.prepared = Some((request, turn));
        Ok(())
    }

    fn reserve(&mut self, value: &Value, limits: &SessionLimits) -> Result<(), SessionError> {
        if self.retained_frames >= limits.max_pending_frames {
            return Err(SessionError::PendingOverflow);
        }
        let remaining = limits.max_pending_bytes.saturating_sub(self.retained_bytes);
        let bytes = encoded_len(value, remaining)?;
        self.retained_bytes += bytes;
        self.retained_frames += 1;
        Ok(())
    }

    fn insert_tool(&mut self, id: String, input: Value) -> Result<(), SessionError> {
        if self.pending.is_some() || self.retired_tools.contains(&id) || !self.evidence.is_empty() {
            return Err(SessionError::ConflictingRecord);
        }
        input_size(&input)?;
        self.evidence.insert(id.clone(), ToolEvidence { id, input });
        Ok(())
    }
}

impl SessionMachine {
    /// This constructor is available only to tests and the guarded fixture runner.
    /// The caller must separately establish the exact physical fixture sender.
    #[cfg(any(test, feature = "test_helpers"))]
    pub fn new_fixture_file_approval(
        config: SessionConfig,
        mut limits: SessionLimits,
    ) -> Result<Self, SessionError> {
        limits.validate()?;
        if !matches!(config.target, SessionTarget::Claude { resume: false, .. }) {
            return Err(SessionError::UnsafeProfile);
        }
        limits.max_turns = 1;
        let mut machine = Self::new(config, limits)?;
        machine.fixture_approval = Some(FixtureApprovalState::default());
        Ok(machine)
    }

    #[must_use]
    pub fn fixture_file_approval_enabled(&self) -> bool {
        self.fixture_approval.is_some()
    }

    pub fn mark_input_written(
        &mut self,
        request_id: Uuid,
        turn_id: Uuid,
    ) -> Result<SessionTransition, SessionError> {
        self.with_fixture_approval(|machine, state| {
            machine.require_fixture_turn(state, request_id, turn_id)?;
            if state.input_written {
                return Err(SessionError::DuplicateRequest);
            }
            state.input_written = true;
            machine.fixture_progress(state)
        })
    }

    /// Constructs fixture wire bytes only. The daemon independently requires its
    /// durable delivery capability and exact physical sender before writing them.
    pub fn respond_to_approval(
        &mut self,
        challenge: &ApprovalChallenge,
        decision: ChatApprovalDecision,
    ) -> Result<WireFrame, SessionError> {
        self.with_fixture_approval(|machine, state| {
            machine.require_fixture_turn(state, challenge.request_id, challenge.turn_id)?;
            if !state.input_written || !state.root_init {
                return Err(SessionError::MissingCorrelation);
            }
            let pending = state
                .pending
                .as_mut()
                .ok_or(SessionError::UncorrelatedResponse)?;
            if pending.challenge != *challenge {
                return Err(SessionError::ConflictingRecord);
            }
            if pending.decision.is_some() {
                return Err(SessionError::DuplicateRequest);
            }
            let response = match decision {
                ChatApprovalDecision::AllowOnce => json!({
                    "behavior":"allow","updatedInput":pending.challenge.input
                }),
                ChatApprovalDecision::Deny => json!({
                    "behavior":"deny","message":"Denied by VibeMux"
                }),
            };
            let effect = machine.frame(json!({"type":"control_response","response":{
                "subtype":"success","request_id":pending.challenge.vendor_request_id,
                "response":response
            }}))?;
            let SessionEffect::Write(frame) = effect else {
                return Err(SessionError::UnexpectedState);
            };
            // Consume the one-use response authority before exposing any wire bytes.
            pending.decision = Some(decision);
            Ok(frame)
        })
    }

    pub fn mark_approval_written(
        &mut self,
        challenge_id: Uuid,
    ) -> Result<SessionTransition, SessionError> {
        self.with_fixture_approval(|machine, state| {
            let pending = state
                .pending
                .as_ref()
                .ok_or(SessionError::UncorrelatedResponse)?;
            if pending.challenge.challenge_id != challenge_id || pending.decision.is_none() {
                return Err(SessionError::UncorrelatedResponse);
            }
            if !matches!(machine.phase, SessionPhase::Busy) || machine.active.is_none() {
                return Err(SessionError::UnexpectedState);
            }
            let pending = state
                .pending
                .take()
                .ok_or(SessionError::UncorrelatedResponse)?;
            state.retired_tools.insert(pending.challenge.tool_use_id);
            if let Some(terminal) = state.terminal.take() {
                return machine.claude_terminal(&terminal);
            }
            Ok(SessionTransition::default())
        })
    }

    fn with_fixture_approval<T>(
        &mut self,
        action: impl FnOnce(&mut Self, &mut FixtureApprovalState) -> Result<T, SessionError>,
    ) -> Result<T, SessionError> {
        let mut state = self
            .fixture_approval
            .take()
            .ok_or(SessionError::UnsafeProfile)?;
        let result = action(self, &mut state);
        self.fixture_approval = Some(state);
        result
    }

    fn require_fixture_turn(
        &self,
        state: &FixtureApprovalState,
        request_id: Uuid,
        turn_id: Uuid,
    ) -> Result<(), SessionError> {
        let active = self.active.as_ref().ok_or(SessionError::UncorrelatedTurn)?;
        if self.phase != SessionPhase::Busy
            || self.close_requested
            || active.summary.cancel_requested
        {
            return Err(SessionError::UnexpectedState);
        }
        if state.prepared != Some((request_id, turn_id))
            || active.summary.request_id != request_id
            || active.summary.turn_id != turn_id
        {
            return Err(SessionError::UncorrelatedTurn);
        }
        Ok(())
    }

    pub(super) fn fixture_approval_progress(&mut self) -> Result<SessionTransition, SessionError> {
        self.with_fixture_approval(Self::fixture_progress)
    }

    fn fixture_progress(
        &mut self,
        state: &mut FixtureApprovalState,
    ) -> Result<SessionTransition, SessionError> {
        let mut transition = SessionTransition::default();
        if !state.root_init || self.phase == SessionPhase::Initializing {
            return Ok(transition);
        }
        let session_id = self
            .config
            .target
            .expected_id()
            .ok_or(SessionError::UnsafeProfile)?;
        transition.extend(self.bind_session(&session_id)?);
        if !state.input_written || self.active.is_none() {
            return Ok(transition);
        }
        let (request_id, turn_id) = state.prepared.ok_or(SessionError::MissingCorrelation)?;
        self.require_fixture_turn(state, request_id, turn_id)?;
        transition.extend(self.acknowledge()?);
        if state.pending.is_some() {
            return Ok(transition);
        }
        let Some(request) = state.request.as_ref() else {
            return Ok(transition);
        };
        let Some(tool) = state.evidence.get(&request.tool_id) else {
            if !state.evidence.is_empty() {
                return Err(SessionError::ConflictingRecord);
            }
            return Ok(transition);
        };
        if tool.id != request.tool_id || tool.input != request.input {
            return Err(SessionError::ConflictingRecord);
        }
        let extra = input_size(&request.input)?
            .checked_add(request.id.len() + request.tool_id.len())
            .ok_or(SessionError::PendingOverflow)?;
        if extra
            > self
                .limits
                .max_pending_bytes
                .saturating_sub(state.retained_bytes)
        {
            return Err(SessionError::PendingOverflow);
        }
        state.retained_bytes += extra;
        let request = state
            .request
            .take()
            .ok_or(SessionError::MissingCorrelation)?;
        state.evidence.remove(&request.tool_id);
        let challenge = ApprovalChallenge {
            challenge_id: Uuid::new_v4(),
            request_id,
            turn_id,
            vendor_session_id: session_id,
            vendor_request_id: request.id,
            tool_use_id: request.tool_id,
            tool: ToolName::WriteFiles,
            vendor_tool_name: "Write".to_owned(),
            input: request.input,
        };
        state.pending = Some(PendingApproval {
            challenge: challenge.clone(),
            decision: None,
        });
        transition.approvals.push(challenge);
        Ok(transition)
    }

    pub(super) fn receive_fixture_approval(
        &mut self,
        value: Value,
        digest: Sha256Digest,
    ) -> Result<SessionTransition, SessionError> {
        self.with_fixture_approval(|machine, state| {
            machine.apply_fixture_record(state, value, digest)
        })
    }

    fn apply_fixture_record(
        &mut self,
        state: &mut FixtureApprovalState,
        value: Value,
        digest: Sha256Digest,
    ) -> Result<SessionTransition, SessionError> {
        let kind = identifier(value.get("type"))?;
        if !matches!(
            kind.as_str(),
            "system" | "user" | "assistant" | "stream_event" | "result" | "control_request"
        ) {
            return Ok(Self::ignored(IgnoredReason::UnknownNotification));
        }
        check_root(&value)?;
        if value.get("isReplay") == Some(&Value::Bool(true))
            || value.get("resume_reason").is_some_and(|v| !v.is_null())
            || value.get("origin").is_some_and(|origin| {
                !origin.is_null() && origin.get("kind").and_then(Value::as_str) != Some("human")
            })
        {
            return Err(SessionError::UnsolicitedResume);
        }
        let session_id = self
            .config
            .target
            .expected_id()
            .ok_or(SessionError::UnsafeProfile)?;
        if (kind != "control_request" || value.get("session_id").is_some())
            && identifier(value.get("session_id"))? != session_id
        {
            return Err(SessionError::UncorrelatedSession);
        }
        check_echo(&value, state.prepared.map(|prepared| prepared.0))?;
        if kind == "control_request" {
            let id = identifier(value.get("request_id"))?;
            let request_digest = Sha256Digest::of(value.to_string().as_bytes());
            if let Some(previous) = state.requests.get(&id) {
                return if *previous == request_digest {
                    Ok(Self::ignored(IgnoredReason::Duplicate))
                } else {
                    Err(SessionError::ConflictingRecord)
                };
            }
            if state.request.is_some() || state.pending.is_some() {
                return Err(SessionError::ConflictingRecord);
            }
            if state.requests.len() >= self.limits.max_rpc_requests {
                return Err(SessionError::RpcBudgetExhausted);
            }
            let request = &value["request"];
            check_root(request)?;
            check_echo(request, state.prepared.map(|prepared| prepared.0))?;
            if request["subtype"] != "can_use_tool" || request["tool_name"] != "Write" {
                return Err(SessionError::UnsafeProfile);
            }
            if request.get("session_id").is_some()
                && identifier(request.get("session_id"))? != session_id
            {
                return Err(SessionError::UncorrelatedSession);
            }
            let tool_id = identifier(request.get("tool_use_id"))?;
            if state.retired_tools.contains(&tool_id) || state.prepared.is_none() {
                return Err(SessionError::UncorrelatedTurn);
            }
            input_size(&request["input"])?;
            state.reserve(&value, &self.limits)?;
            state.requests.insert(id.clone(), request_digest);
            state.request = Some(PermissionRequest {
                id,
                tool_id,
                input: request["input"].clone(),
            });
            return self.fixture_progress(state);
        }
        let record_id = record_uuid(value.get("uuid"))?;
        if let Some(previous) = state.records.get(&record_id) {
            return if *previous == digest {
                Ok(Self::ignored(IgnoredReason::Duplicate))
            } else {
                Err(SessionError::ConflictingRecord)
            };
        }
        state.reserve(&value, &self.limits)?;
        state.records.insert(record_id, digest);
        match kind.as_str() {
            "system" => {
                if value["subtype"] != "init"
                    || state.root_init
                    || value["cwd"].as_str() != Some(self.config.working_directory.as_str())
                    || value["permissionMode"] != "manual"
                    || value["tools"] != json!(["Write"])
                    || value["mcp_servers"] != json!([])
                    || value["plugins"] != json!([])
                    || self
                        .config
                        .model
                        .as_ref()
                        .is_some_and(|model| value["model"].as_str() != Some(model.as_str()))
                {
                    return Err(SessionError::UnsafeProfile);
                }
                state.root_init = true;
            }
            "user" => {
                if state
                    .prepared
                    .is_none_or(|prepared| prepared.0 != record_id)
                    || value.pointer("/message/role").and_then(Value::as_str) != Some("user")
                {
                    return Err(SessionError::UncorrelatedTurn);
                }
                // A replayed input does not prove physical write/flush completion.
            }
            "assistant" => {
                let text = self.fixture_assistant(state, &value)?;
                let mut transition = self.fixture_progress(state)?;
                transition.text = text;
                return Ok(transition);
            }
            "stream_event" => self.fixture_stream(state, &value)?,
            "result" => {
                if !state.root_init || !state.input_written || self.active.is_none() {
                    return Err(SessionError::MissingCorrelation);
                }
                let aborted = matches!(
                    value.get("terminal_reason").and_then(Value::as_str),
                    Some("aborted_streaming" | "aborted_tools")
                );
                if aborted
                    && self
                        .active
                        .as_ref()
                        .is_some_and(|turn| turn.summary.cancel_requested)
                    && state
                        .pending
                        .as_ref()
                        .is_none_or(|pending| pending.decision.is_none())
                {
                    if let Some(pending) = state.pending.take() {
                        state.retired_tools.insert(pending.challenge.tool_use_id);
                    }
                    state.request = None;
                    state.evidence.clear();
                    state.stream = None;
                    return self.claude_terminal(&value);
                }
                if let Some(pending) = &state.pending {
                    if pending.decision.is_none() {
                        return Err(SessionError::InvalidTerminal);
                    }
                    if state.terminal.is_some() {
                        return Err(SessionError::ConflictingRecord);
                    }
                    state.terminal = Some(value);
                    return Ok(SessionTransition::default());
                }
                if state.retired_tools.is_empty()
                    || state.request.is_some()
                    || !state.evidence.is_empty()
                {
                    return Err(SessionError::InvalidTerminal);
                }
                return self.claude_terminal(&value);
            }
            _ => {}
        }
        self.fixture_progress(state)
    }

    fn fixture_assistant(
        &mut self,
        state: &mut FixtureApprovalState,
        value: &Value,
    ) -> Result<Vec<super::AssistantText>, SessionError> {
        if state.prepared.is_none() || state.stream.is_some() {
            return Err(SessionError::MissingCorrelation);
        }
        let message = &value["message"];
        self.fixture_metadata(state, message)?;
        let id = identifier(message.get("id"))?;
        if state.messages.len() >= self.limits.max_text_items {
            return Err(SessionError::TextItemLimit);
        }
        if message["role"] != "assistant" || !state.messages.insert(id.clone()) {
            return Err(SessionError::ConflictingRecord);
        }
        let content = message["content"]
            .as_array()
            .ok_or(SessionError::InvalidFrame)?;
        if content.len() > self.limits.max_text_items {
            return Err(SessionError::TextItemLimit);
        }
        let mut pieces = Vec::new();
        for block in content {
            self.fixture_metadata(state, block)?;
            match block["type"].as_str() {
                Some("tool_use") => {
                    if block["name"] != "Write" {
                        return Err(SessionError::UnsafeProfile);
                    }
                    input_size(&block["input"])?;
                    state.insert_tool(identifier(block.get("id"))?, block["input"].clone())?;
                }
                Some("server_tool_use") => return Err(SessionError::UnsafeProfile),
                Some("text") => {
                    pieces.push(block["text"].as_str().ok_or(SessionError::InvalidFrame)?)
                }
                _ => {}
            }
        }
        if pieces.is_empty() {
            Ok(Vec::new())
        } else {
            Ok(vec![self.text_update(&id, &pieces.join("\n\n"), false)?])
        }
    }

    fn fixture_stream(
        &self,
        state: &mut FixtureApprovalState,
        value: &Value,
    ) -> Result<(), SessionError> {
        if state.prepared.is_none() {
            return Err(SessionError::MissingCorrelation);
        }
        let event = &value["event"];
        self.fixture_metadata(state, event)?;
        if event["type"] == "message_start" {
            let message = &event["message"];
            self.fixture_metadata(state, message)?;
            let id = identifier(message.get("id"))?;
            if state.messages.len() >= self.limits.max_text_items {
                return Err(SessionError::TextItemLimit);
            }
            if state.stream.is_some()
                || message["role"] != "assistant"
                || !state.messages.insert(id.clone())
            {
                return Err(SessionError::ConflictingRecord);
            }
            state.stream = Some(StreamMessage {
                id,
                next_index: 0,
                block: None,
                tool: None,
            });
            return Ok(());
        }
        let mut stream = state
            .stream
            .take()
            .ok_or(SessionError::MissingCorrelation)?;
        for source in [value, event] {
            if source.get("message_id").is_some()
                && identifier(source.get("message_id"))? != stream.id
            {
                return Err(SessionError::ConflictingRecord);
            }
        }
        match event["type"].as_str() {
            Some("content_block_start") => {
                let index = event["index"].as_u64().ok_or(SessionError::InvalidFrame)?;
                if stream.block.is_some()
                    || index != stream.next_index
                    || index >= self.limits.max_text_items as u64
                {
                    return Err(SessionError::InvalidFrame);
                }
                stream.next_index += 1;
                stream.block = Some(index);
                let block = &event["content_block"];
                self.fixture_metadata(state, block)?;
                if block["type"] == "tool_use" {
                    if block["name"] != "Write"
                        || state.pending.is_some()
                        || !state.evidence.is_empty()
                    {
                        return Err(SessionError::UnsafeProfile);
                    }
                    let id = identifier(block.get("id"))?;
                    if state.retired_tools.contains(&id) {
                        return Err(SessionError::ConflictingRecord);
                    }
                    input_size(&block["input"])?;
                    stream.tool = Some(StreamTool {
                        id,
                        index,
                        input_json: block["input"].to_string(),
                        initial_nonempty: block["input"]
                            .as_object()
                            .is_some_and(|input| !input.is_empty()),
                        has_delta: false,
                    });
                } else if block["type"] == "server_tool_use" {
                    return Err(SessionError::UnsafeProfile);
                }
            }
            Some("content_block_delta") => {
                let index = event["index"].as_u64().ok_or(SessionError::InvalidFrame)?;
                if stream.block != Some(index) {
                    return Err(SessionError::InvalidFrame);
                }
                if event["delta"]["type"] == "input_json_delta" {
                    let tool = stream.tool.as_mut().ok_or(SessionError::UnsafeProfile)?;
                    if tool.index != index || tool.initial_nonempty {
                        return Err(SessionError::ConflictingRecord);
                    }
                    let partial = event["delta"]["partial_json"]
                        .as_str()
                        .ok_or(SessionError::InvalidFrame)?;
                    let retained = if tool.has_delta {
                        tool.input_json.len()
                    } else {
                        0
                    };
                    if partial.len() > MAX_APPROVAL_INPUT_BYTES.saturating_sub(retained) {
                        return Err(SessionError::PendingOverflow);
                    }
                    if !tool.has_delta {
                        tool.input_json.clear();
                        tool.has_delta = true;
                    }
                    tool.input_json.push_str(partial);
                } else if stream.tool.is_some() {
                    return Err(SessionError::InvalidFrame);
                }
            }
            Some("content_block_stop") => {
                let index = event["index"].as_u64().ok_or(SessionError::InvalidFrame)?;
                if stream.block != Some(index) {
                    return Err(SessionError::InvalidFrame);
                }
                stream.block = None;
                if let Some(tool) = stream.tool.take() {
                    let input = serde_json::from_str(&tool.input_json)
                        .map_err(|_| SessionError::InvalidJson)?;
                    state.insert_tool(tool.id, input)?;
                }
            }
            Some("message_stop") => {
                if stream.block.is_some() {
                    return Err(SessionError::InvalidFrame);
                }
                return Ok(());
            }
            Some("message_delta") => {}
            _ => return Err(SessionError::InvalidFrame),
        }
        state.stream = Some(stream);
        Ok(())
    }

    fn fixture_metadata(
        &self,
        state: &FixtureApprovalState,
        value: &Value,
    ) -> Result<(), SessionError> {
        check_root(value)?;
        check_echo(value, state.prepared.map(|prepared| prepared.0))?;
        if value.get("session_id").is_some()
            && Some(identifier(value.get("session_id"))?) != self.config.target.expected_id()
        {
            return Err(SessionError::UncorrelatedSession);
        }
        Ok(())
    }
}

fn check_root(value: &Value) -> Result<(), SessionError> {
    if [
        "parent_tool_use_id",
        "agent_id",
        "parent_agent_id",
        "subagent_id",
    ]
    .iter()
    .any(|key| value.get(key).is_some_and(|field| !field.is_null()))
    {
        return Err(SessionError::UncorrelatedTurn);
    }
    Ok(())
}

fn check_echo(value: &Value, expected: Option<Uuid>) -> Result<(), SessionError> {
    if let Some(echo) = value.get("user_message_uuid") {
        if Some(record_uuid(Some(echo))?) != expected {
            return Err(SessionError::UncorrelatedTurn);
        }
    } else if value.get("user_message_uuids").is_some() {
        return Err(SessionError::MissingCorrelation);
    }
    if let Some(echoes) = value.get("user_message_uuids") {
        let echoes = echoes.as_array().ok_or(SessionError::InvalidFrame)?;
        if echoes.len() != 1 || Some(record_uuid(echoes.first())?) != expected {
            return Err(SessionError::UncorrelatedTurn);
        }
    }
    Ok(())
}

fn record_uuid(value: Option<&Value>) -> Result<Uuid, SessionError> {
    Uuid::parse_str(&identifier(value)?)
        .ok()
        .filter(|id| !id.is_nil())
        .ok_or(SessionError::InvalidIdentifier)
}

fn input_size(value: &Value) -> Result<usize, SessionError> {
    if !value.is_object() {
        return Err(SessionError::InvalidFrame);
    }
    encoded_len(value, MAX_APPROVAL_INPUT_BYTES)
}

fn encoded_len(value: &Value, limit: usize) -> Result<usize, SessionError> {
    struct Counter {
        bytes: usize,
        limit: usize,
    }
    impl io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.bytes) {
                return Err(io::Error::other("fixture_pending_limit"));
            }
            self.bytes += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { bytes: 0, limit };
    serde_json::to_writer(&mut counter, value).map_err(|_| SessionError::PendingOverflow)?;
    Ok(counter.bytes)
}
