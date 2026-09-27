//! Per-protocol session state machines for one prompt and one turn.
//!
//! The session decides what to write to the vendor's stdin and which records
//! count as correlated terminal evidence. It never performs I/O: the daemon
//! writes [`ProtocolSession::initial_input`], feeds every classified record to
//! [`ProtocolSession::receive`], writes the returned replies, and sends
//! [`ProtocolSession::cancel_message`] on cancellation.
//!
//! Posture: approval and permission requests are always declined, client
//! filesystem and terminal capabilities are never offered, and a terminal
//! status is accepted only when it correlates with this session's own RPC,
//! thread, turn, or session identifiers.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::AgentKind;

use super::{
    DispatchError, DispatchRoute, NativeProtocol, ObservationKind, request::MAX_PROMPT_BYTES,
};

const INITIALIZE_ID: u64 = 1;
const SESSION_ID: u64 = 2;
const PROMPT_ID: u64 = 3;
const CANCEL_ID: u64 = 4;
const CLAUDE_INITIALIZE_ID: &str = "vibemux_initialize";
const CLAUDE_INTERRUPT_ID: &str = "vibemux_interrupt";
const MAX_IDENTIFIER_BYTES: usize = 512;
const CLIENT_NAME: &str = "vibemux";
const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Correlated terminal evidence reported by the vendor protocol.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionTerminal {
    Completed,
    Failed,
    Cancelled,
    /// The initialize-only handshake succeeded (probe mode only).
    Probed,
}

/// First bytes the daemon writes after spawning the vendor process.
#[derive(Clone, Eq, PartialEq)]
pub enum InitialInput {
    /// One JSON line; stdin stays open for the rest of the exchange.
    Message(Value),
    /// Raw prompt bytes, after which stdin is closed (`codex exec -`).
    PromptThenClose(String),
}

impl fmt::Debug for InitialInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(_) => formatter.write_str("InitialInput::Message(..)"),
            Self::PromptThenClose(prompt) => write!(
                formatter,
                "InitialInput::PromptThenClose({} bytes)",
                prompt.len()
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Initialize,
    Session,
    Prompt,
    Finished,
}

pub struct ProtocolSession {
    harness: AgentKind,
    protocol: NativeProtocol,
    probe: bool,
    model: Option<String>,
    prompt: String,
    working_directory: String,
    phase: Phase,
    session_id: Option<String>,
    turn_id: Option<String>,
    terminal: Option<SessionTerminal>,
}

impl fmt::Debug for ProtocolSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProtocolSession")
            .field("harness", &self.harness)
            .field("protocol", &self.protocol)
            .field("probe", &self.probe)
            .field("phase", &self.phase)
            .field("terminal", &self.terminal)
            .finish_non_exhaustive()
    }
}

impl ProtocolSession {
    /// Initialize-only session. Sends no prompt and opens no vendor session.
    pub fn for_probe(
        route: &DispatchRoute,
        working_directory: String,
    ) -> Result<Self, DispatchError> {
        if !route.protocol.supports_probe() {
            return Err(DispatchError::ProbeUnsupported);
        }
        Self::new(route, true, String::new(), working_directory)
    }

    /// One-prompt session. `working_directory` is the canonical project root
    /// as UTF-8, supplied by the daemon.
    pub fn for_execution(
        route: &DispatchRoute,
        prompt: String,
        working_directory: String,
    ) -> Result<Self, DispatchError> {
        if prompt.trim().is_empty() || prompt.len() > MAX_PROMPT_BYTES {
            return Err(DispatchError::InvalidPrompt);
        }
        Self::new(route, false, prompt, working_directory)
    }

    fn new(
        route: &DispatchRoute,
        probe: bool,
        prompt: String,
        working_directory: String,
    ) -> Result<Self, DispatchError> {
        if !route.protocol.accepts(route.harness) {
            return Err(DispatchError::ConfigRouteInvalid);
        }
        if working_directory.is_empty() || working_directory.contains('\0') {
            return Err(DispatchError::WorkingDirectoryInvalid);
        }
        let phase = if route.protocol == NativeProtocol::CodexExec {
            Phase::Prompt
        } else {
            Phase::Initialize
        };
        Ok(Self {
            harness: route.harness,
            protocol: route.protocol,
            probe,
            model: route.model.clone(),
            prompt,
            working_directory,
            phase,
            session_id: None,
            turn_id: None,
            terminal: None,
        })
    }

    #[must_use]
    pub const fn protocol(&self) -> NativeProtocol {
        self.protocol
    }

    #[must_use]
    pub const fn terminal(&self) -> Option<SessionTerminal> {
        self.terminal
    }

    #[must_use]
    pub const fn is_finished(&self) -> bool {
        matches!(self.phase, Phase::Finished)
    }

    #[must_use]
    pub fn initial_input(&self) -> InitialInput {
        match self.protocol {
            NativeProtocol::CodexExec => InitialInput::PromptThenClose(self.prompt.clone()),
            NativeProtocol::CodexAppServer => InitialInput::Message(json!({
                "id": INITIALIZE_ID,
                "method": "initialize",
                "params": {"clientInfo": {
                    "name": CLIENT_NAME, "title": "VibeMux", "version": CLIENT_VERSION,
                }},
            })),
            NativeProtocol::Acp => InitialInput::Message(json!({
                "jsonrpc": "2.0",
                "id": INITIALIZE_ID,
                "method": "initialize",
                "params": {
                    "protocolVersion": 1,
                    "clientCapabilities": {
                        "fs": {"readTextFile": false, "writeTextFile": false},
                        "terminal": false,
                    },
                    "clientInfo": {"name": CLIENT_NAME, "version": CLIENT_VERSION},
                },
            })),
            NativeProtocol::ClaudeStreamJson => InitialInput::Message(json!({
                "type": "control_request",
                "request_id": CLAUDE_INITIALIZE_ID,
                "request": {"subtype": "initialize", "hooks": null, "skills": []},
            })),
        }
    }

    /// Advances the session with one classified record and returns the JSON
    /// lines to write back, in order. Any error is a protocol violation that
    /// ends the attempt.
    pub fn receive(
        &mut self,
        value: &Value,
        kind: ObservationKind,
    ) -> Result<Vec<Value>, DispatchError> {
        match self.protocol {
            NativeProtocol::ClaudeStreamJson => self.receive_claude(value, kind),
            NativeProtocol::CodexExec => {
                if kind.is_terminal() {
                    if self.phase != Phase::Prompt {
                        return Err(DispatchError::UnexpectedTerminal);
                    }
                    self.finish_with(kind);
                }
                Ok(Vec::new())
            }
            NativeProtocol::CodexAppServer | NativeProtocol::Acp => {
                self.receive_json_rpc(value, kind)
            }
        }
    }

    /// The protocol's cancel message, when the session has enough identity to
    /// address one. `None` means the daemon must terminate the process.
    #[must_use]
    pub fn cancel_message(&self) -> Option<Value> {
        match self.protocol {
            NativeProtocol::Acp => self.session_id.as_ref().map(|session_id| {
                json!({"jsonrpc": "2.0", "method": "session/cancel",
                    "params": {"sessionId": session_id}})
            }),
            NativeProtocol::CodexAppServer => self
                .session_id
                .as_ref()
                .zip(self.turn_id.as_ref())
                .map(|(session_id, turn_id)| {
                    json!({"id": CANCEL_ID, "method": "turn/interrupt",
                        "params": {"threadId": session_id, "turnId": turn_id}})
                }),
            NativeProtocol::ClaudeStreamJson => Some(json!({
                "type": "control_request",
                "request_id": CLAUDE_INTERRUPT_ID,
                "request": {"subtype": "interrupt"},
            })),
            NativeProtocol::CodexExec => None,
        }
    }

    fn receive_json_rpc(
        &mut self,
        value: &Value,
        kind: ObservationKind,
    ) -> Result<Vec<Value>, DispatchError> {
        let acp = self.protocol == NativeProtocol::Acp;
        if acp && value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Err(DispatchError::InvalidJsonRpc);
        }
        if let Some(method) = value.get("method").and_then(Value::as_str) {
            if let Some(id) = value.get("id") {
                validate_rpc_id(id)?;
                return Ok(vec![self.decline_server_request(method, id)]);
            }
            if !acp && matches!(method, "turn/started" | "turn/completed") {
                self.receive_turn_notification(value, method, kind)?;
            }
            if acp && method == "session/update" {
                self.validate_session(value.pointer("/params/sessionId"))?;
            }
            return Ok(Vec::new());
        }
        let Some(id) = value.get("id").and_then(Value::as_u64) else {
            return Err(DispatchError::UncorrelatedResponse);
        };
        if id == CANCEL_ID {
            return Ok(Vec::new());
        }
        let expected = match self.phase {
            Phase::Initialize => INITIALIZE_ID,
            Phase::Session => SESSION_ID,
            Phase::Prompt => PROMPT_ID,
            Phase::Finished => return Err(DispatchError::UnexpectedResponse),
        };
        if id != expected {
            return Err(DispatchError::UncorrelatedResponse);
        }
        if value.get("error").is_some() {
            return Err(DispatchError::RpcError);
        }
        let result = value
            .get("result")
            .filter(|result| result.is_object())
            .ok_or(DispatchError::InvalidRpcResult)?;
        match self.phase {
            Phase::Initialize => self.receive_initialize_result(result),
            Phase::Session => self.receive_session_result(result),
            Phase::Prompt => {
                if acp {
                    self.finish_with(kind);
                    if self.terminal.is_none() {
                        return Err(DispatchError::InvalidTerminal);
                    }
                } else {
                    self.correlate_turn(identifier(result.pointer("/turn/id"))?)?;
                }
                Ok(Vec::new())
            }
            Phase::Finished => Err(DispatchError::UnexpectedResponse),
        }
    }

    fn decline_server_request(&self, method: &str, id: &Value) -> Value {
        match self.protocol {
            NativeProtocol::Acp if method == "session/request_permission" => {
                json!({"jsonrpc": "2.0", "id": id,
                    "result": {"outcome": {"outcome": "cancelled"}}})
            }
            NativeProtocol::CodexAppServer
                if matches!(
                    method,
                    "item/commandExecution/requestApproval" | "item/fileChange/requestApproval"
                ) =>
            {
                json!({"id": id, "result": {"decision": "decline"}})
            }
            _ => {
                let mut response = json!({"id": id,
                    "error": {"code": -32601, "message": "Client capability not granted"}});
                if self.protocol == NativeProtocol::Acp {
                    response["jsonrpc"] = json!("2.0");
                }
                response
            }
        }
    }

    fn receive_turn_notification(
        &mut self,
        value: &Value,
        method: &str,
        kind: ObservationKind,
    ) -> Result<(), DispatchError> {
        self.validate_session(value.pointer("/params/threadId"))?;
        let turn_id = identifier(value.pointer("/params/turn/id"))?;
        if method == "turn/started" {
            return self.correlate_turn(turn_id);
        }
        if self.phase != Phase::Prompt || self.turn_id.is_none() {
            return Err(DispatchError::UnexpectedTerminal);
        }
        self.correlate_turn(turn_id)?;
        self.finish_with(kind);
        if self.terminal.is_none() {
            return Err(DispatchError::InvalidTerminal);
        }
        Ok(())
    }

    fn receive_initialize_result(&mut self, result: &Value) -> Result<Vec<Value>, DispatchError> {
        let acp = self.protocol == NativeProtocol::Acp;
        if acp && result.get("protocolVersion").and_then(Value::as_u64) != Some(1) {
            return Err(DispatchError::UnsupportedProtocolVersion);
        }
        if !acp && result.get("userAgent").and_then(Value::as_str).is_none() {
            return Err(DispatchError::InvalidInitialize);
        }
        let mut replies = Vec::new();
        if !acp {
            replies.push(json!({"method": "initialized"}));
        }
        if self.probe {
            self.terminal = Some(SessionTerminal::Probed);
            self.phase = Phase::Finished;
            return Ok(replies);
        }
        self.phase = Phase::Session;
        let cwd = self.working_directory.as_str();
        replies.push(if acp {
            json!({"jsonrpc": "2.0", "id": SESSION_ID, "method": "session/new",
                "params": {"cwd": cwd, "mcpServers": []}})
        } else {
            let mut params = json!({"cwd": cwd, "approvalPolicy": "never",
                "sandbox": "read-only", "ephemeral": true});
            if let Some(model) = &self.model {
                params["model"] = json!(model);
            }
            json!({"id": SESSION_ID, "method": "thread/start", "params": params})
        });
        Ok(replies)
    }

    fn receive_session_result(&mut self, result: &Value) -> Result<Vec<Value>, DispatchError> {
        let acp = self.protocol == NativeProtocol::Acp;
        let session_id = if acp {
            identifier(result.get("sessionId"))?
        } else {
            identifier(result.pointer("/thread/id"))?
        };
        self.session_id = Some(session_id.clone());
        self.phase = Phase::Prompt;
        let input = json!([{"type": "text", "text": self.prompt}]);
        Ok(vec![if acp {
            json!({"jsonrpc": "2.0", "id": PROMPT_ID, "method": "session/prompt",
                "params": {"sessionId": session_id, "prompt": input}})
        } else {
            json!({"id": PROMPT_ID, "method": "turn/start",
                "params": {"threadId": session_id, "input": input}})
        }])
    }

    fn receive_claude(
        &mut self,
        value: &Value,
        kind: ObservationKind,
    ) -> Result<Vec<Value>, DispatchError> {
        match value.get("type").and_then(Value::as_str) {
            Some("control_request") => {
                let request_id = value.get("request_id").ok_or(DispatchError::InvalidRpcId)?;
                validate_rpc_id(request_id)?;
                Ok(vec![json!({"type": "control_response", "response": {
                    "subtype": "error", "request_id": request_id,
                    "error": "Client capability not granted"}})])
            }
            Some("control_response") => {
                let response_id = value
                    .pointer("/response/request_id")
                    .and_then(Value::as_str);
                if response_id == Some(CLAUDE_INTERRUPT_ID) {
                    return Ok(Vec::new());
                }
                if self.phase != Phase::Initialize
                    || response_id != Some(CLAUDE_INITIALIZE_ID)
                    || value.pointer("/response/subtype").and_then(Value::as_str) != Some("success")
                {
                    return Err(DispatchError::InvalidInitialize);
                }
                if self.probe {
                    self.terminal = Some(SessionTerminal::Probed);
                    self.phase = Phase::Finished;
                    return Ok(Vec::new());
                }
                self.phase = Phase::Prompt;
                Ok(vec![json!({"type": "user", "session_id": "",
                    "message": {"role": "user", "content": self.prompt},
                    "parent_tool_use_id": null})])
            }
            _ => {
                if kind.is_terminal() {
                    if self.phase != Phase::Prompt {
                        return Err(DispatchError::UnexpectedTerminal);
                    }
                    self.finish_with(kind);
                }
                Ok(Vec::new())
            }
        }
    }

    fn validate_session(&self, value: Option<&Value>) -> Result<(), DispatchError> {
        match (&self.session_id, value.and_then(Value::as_str)) {
            (Some(expected), Some(actual)) if expected == actual => Ok(()),
            _ => Err(DispatchError::UncorrelatedSession),
        }
    }

    fn correlate_turn(&mut self, turn_id: String) -> Result<(), DispatchError> {
        match &self.turn_id {
            Some(expected) if expected != &turn_id => Err(DispatchError::UncorrelatedTurn),
            Some(_) => Ok(()),
            None => {
                self.turn_id = Some(turn_id);
                Ok(())
            }
        }
    }

    fn finish_with(&mut self, kind: ObservationKind) {
        let terminal = match kind {
            ObservationKind::Completed => SessionTerminal::Completed,
            ObservationKind::Failed => SessionTerminal::Failed,
            ObservationKind::Cancelled => SessionTerminal::Cancelled,
            _ => return,
        };
        self.terminal = Some(terminal);
        self.phase = Phase::Finished;
    }
}

fn identifier(value: Option<&Value>) -> Result<String, DispatchError> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= MAX_IDENTIFIER_BYTES)
        .map(str::to_string)
        .ok_or(DispatchError::InvalidIdentifier)
}

fn validate_rpc_id(value: &Value) -> Result<(), DispatchError> {
    let valid = value
        .as_str()
        .is_some_and(|text| !text.is_empty() && text.len() <= MAX_IDENTIFIER_BYTES)
        || value.as_i64().is_some();
    if valid {
        Ok(())
    } else {
        Err(DispatchError::InvalidRpcId)
    }
}
