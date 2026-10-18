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
/// DSH SDK request ids. The reference client uses opaque `req_<uuid>` strings,
/// so string ids are the observed shape; the transport only requires `id` to
/// be present on a request or response frame.
const DSH_INITIALIZE_ID: &str = "vibemux_initialize";
const DSH_PROMPT_ID: &str = "vibemux_prompt";
/// Wire-stable server identity the handshake must report
/// (`@deepseek-ai/dsh-sdk-protocol`, `serverInfo.name`).
const DSH_SERVER_NAME: &str = "deepseek-harness-sdk-runtime";
/// Bound on a DSH-side session id echoed back into protocol params.
const MAX_DSH_SESSION_ID_BYTES: usize = 128;
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

/// The route facts a session reads. The daemon derives it from a
/// [`DispatchRoute`]; a caller with its own route format (the standalone
/// native dispatcher of ADR 040) builds it directly, so both drive the same
/// vendor handshake.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionRoute {
    pub harness: AgentKind,
    pub protocol: NativeProtocol,
    pub model: Option<String>,
    /// DSH provider route; ignored by every other protocol.
    pub provider: Option<String>,
}

impl From<&DispatchRoute> for SessionRoute {
    fn from(route: &DispatchRoute) -> Self {
        Self {
            harness: route.harness,
            protocol: route.protocol,
            model: route.model.clone(),
            provider: route.provider.clone(),
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
    /// DSH provider route; `InitializeParams.provider` is required by the SDK
    /// protocol, so a launcher-pair route must name one.
    provider: Option<String>,
    prompt: String,
    working_directory: String,
    /// DSH-side session id. The SDK protocol lets the client choose it, and an
    /// unknown id lazily creates the agent+session pair.
    dsh_session_id: Option<String>,
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
        Self::probe(SessionRoute::from(route), working_directory)
    }

    /// One-prompt session. `working_directory` is the canonical project root
    /// as UTF-8, supplied by the daemon.
    pub fn for_execution(
        route: &DispatchRoute,
        prompt: String,
        working_directory: String,
    ) -> Result<Self, DispatchError> {
        Self::execution(
            SessionRoute::from(route),
            prompt,
            working_directory,
            MAX_PROMPT_BYTES,
        )
    }

    /// [`Self::for_probe`] for a caller that owns its own route format.
    pub fn probe(route: SessionRoute, working_directory: String) -> Result<Self, DispatchError> {
        if !route.protocol.supports_probe() {
            return Err(DispatchError::ProbeUnsupported);
        }
        Self::new(route, true, String::new(), working_directory)
    }

    /// [`Self::for_execution`] for a caller that owns its own route format
    /// and prompt-size policy. The daemon's policy is [`MAX_PROMPT_BYTES`].
    pub fn execution(
        route: SessionRoute,
        prompt: String,
        working_directory: String,
        max_prompt_bytes: usize,
    ) -> Result<Self, DispatchError> {
        if prompt.trim().is_empty() || prompt.len() > max_prompt_bytes {
            return Err(DispatchError::InvalidPrompt);
        }
        Self::new(route, false, prompt, working_directory)
    }

    fn new(
        route: SessionRoute,
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
        // A deterministic, prompt-free session id: the SDK protocol lets the
        // client name the session, and a one-shot attempt never resumes, so a
        // digest of the inputs is enough to keep two attempts distinct.
        let dsh_session_id = (route.protocol == NativeProtocol::DshSdkJsonRpc).then(|| {
            let digest = super::digest::hash_fields(
                "vibemux.dsh.session",
                &[working_directory.as_bytes(), prompt.as_bytes()],
            );
            format!("vibemux_{}", &digest.to_hex()[..32])
        });
        Ok(Self {
            harness: route.harness,
            protocol: route.protocol,
            probe,
            model: route.model,
            provider: route.provider,
            prompt,
            working_directory,
            dsh_session_id,
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
            // `initialize` is the first frame of the SDK handshake; stdin stays
            // open because `session/prompt` follows it (ADR 032 Decision 3).
            NativeProtocol::DshSdkJsonRpc => InitialInput::Message(json!({
                "jsonrpc": "2.0",
                "id": DSH_INITIALIZE_ID,
                "method": "initialize",
                "params": {
                    "cwd": self.working_directory,
                    "provider": self.provider.clone().unwrap_or_default(),
                    "model": self.model.clone().unwrap_or_default(),
                },
            })),
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
            NativeProtocol::DshSdkJsonRpc => self.receive_dsh(value, kind),
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
            // The SDK protocol publishes no cancel or session-close method:
            // "a client abandons a turn by closing the runtime process"
            // (ADR 032 Decision 6).
            NativeProtocol::DshSdkJsonRpc => None,
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

    /// Drives the DSH SDK stdio session (ADR 032). Frames are JSON-RPC 2.0:
    /// `id` plus `method` is a server request, `id` alone is a response, and
    /// `method` alone is a notification. `session.event` is unfiltered, so
    /// every notification is checked against this session's own id first.
    fn receive_dsh(
        &mut self,
        value: &Value,
        kind: ObservationKind,
    ) -> Result<Vec<Value>, DispatchError> {
        if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Err(DispatchError::InvalidJsonRpc);
        }
        if let Some(method) = value.get("method").and_then(Value::as_str) {
            // The server never sends a request in this protocol version, but a
            // future one might: decline instead of desynchronizing.
            if let Some(id) = value.get("id") {
                validate_rpc_id(id)?;
                return Ok(vec![json!({"jsonrpc": "2.0", "id": id,
                    "error": {"code": -32601, "message": "Client capability not granted"}})]);
            }
            return self.receive_dsh_notification(value, method, kind);
        }
        let Some(id) = value.get("id").and_then(Value::as_str) else {
            return Err(DispatchError::UncorrelatedResponse);
        };
        if value.get("error").is_some() {
            return Err(DispatchError::RpcError);
        }
        let result = value
            .get("result")
            .filter(|result| result.is_object())
            .ok_or(DispatchError::InvalidRpcResult)?;
        match id {
            DSH_INITIALIZE_ID => {
                if self.phase != Phase::Initialize {
                    return Err(DispatchError::UnexpectedResponse);
                }
                self.receive_dsh_initialize_result(result)
            }
            DSH_PROMPT_ID => {
                if self.phase != Phase::Prompt {
                    return Err(DispatchError::UnexpectedResponse);
                }
                // The receipt names the queued user message, not a result.
                identifier(result.get("messageId"))?;
                Ok(Vec::new())
            }
            _ => Err(DispatchError::UncorrelatedResponse),
        }
    }

    fn receive_dsh_initialize_result(
        &mut self,
        result: &Value,
    ) -> Result<Vec<Value>, DispatchError> {
        if result.pointer("/serverInfo/name").and_then(Value::as_str) != Some(DSH_SERVER_NAME) {
            return Err(DispatchError::InvalidInitialize);
        }
        if self.probe {
            self.terminal = Some(SessionTerminal::Probed);
            self.phase = Phase::Finished;
            return Ok(Vec::new());
        }
        // The SDK protocol resolves the provider route during initialization
        // and rejects later prompts when it fails, so reaching here means the
        // route exists. The prompt creates the agent+session pair lazily.
        self.phase = Phase::Prompt;
        let session_id = self.dsh_session_id.clone().ok_or(DispatchError::Internal)?;
        Ok(vec![json!({
            "jsonrpc": "2.0",
            "id": DSH_PROMPT_ID,
            "method": "session/prompt",
            "params": {
                "sessionId": session_id,
                "contentBlocks": [{"type": "text", "text": self.prompt}],
            },
        })])
    }

    fn receive_dsh_notification(
        &mut self,
        value: &Value,
        method: &str,
        kind: ObservationKind,
    ) -> Result<Vec<Value>, DispatchError> {
        // Every notification carries a session id, and the stream is not
        // filtered by session, so a foreign record must never advance this
        // session's state. Unknown methods stay tolerated: the protocol is
        // merge-extensible.
        if !matches!(
            method,
            "session.event" | "session.status" | "subagent.started" | "subagent.finished"
        ) {
            return Ok(Vec::new());
        }
        let Some(expected) = self.dsh_session_id.clone() else {
            return Err(DispatchError::Internal);
        };
        let Some(actual) = value.pointer("/params/sessionId").and_then(Value::as_str) else {
            return Err(DispatchError::UncorrelatedSession);
        };
        if actual != expected {
            // Another session in the same runtime; observe and drop.
            return Ok(Vec::new());
        }
        if method != "session.event" {
            return Ok(Vec::new());
        }
        // `dsh_session_id` is client-chosen and bounded, so an echoed id is
        // validated before it is trusted for correlation.
        if actual.len() > MAX_DSH_SESSION_ID_BYTES {
            return Err(DispatchError::InvalidIdentifier);
        }
        if value.pointer("/params/event/type").and_then(Value::as_str) != Some("turn/end") {
            return Ok(Vec::new());
        }
        if self.phase != Phase::Prompt {
            return Err(DispatchError::UnexpectedTerminal);
        }
        let turn = value
            .pointer("/params/event/data/turn")
            .and_then(Value::as_u64)
            .ok_or(DispatchError::InvalidIdentifier)?;
        self.correlate_turn(turn.to_string())?;
        if !kind.is_terminal() {
            // An unrecognized `reason.kind` is not a failure: the vocabulary is
            // merge-extensible, so the attempt falls through to the process
            // exit and the deadline instead of ending here.
            return Ok(Vec::new());
        }
        self.finish_with(kind);
        if self.terminal.is_none() {
            return Err(DispatchError::InvalidTerminal);
        }
        Ok(Vec::new())
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;
    use crate::dispatch::Sha256Digest;
    use crate::dispatch::observation::classify;

    use DispatchError::{
        ConfigRouteInvalid, InvalidIdentifier, InvalidInitialize, InvalidJsonRpc, InvalidPrompt,
        InvalidRpcId, InvalidRpcResult, ProbeUnsupported, RpcError, UncorrelatedResponse,
        UncorrelatedSession, UncorrelatedTurn, UnexpectedResponse, UnexpectedTerminal,
        UnsupportedProtocolVersion, WorkingDirectoryInvalid,
    };
    use NativeProtocol::{Acp, ClaudeStreamJson, CodexAppServer, CodexExec, DshSdkJsonRpc};

    const PROMPT: &str = "synthetic prompt";
    const ROOT: &str = "SYNTHETIC_ROOT";
    const REFUSAL: &str = "Client capability not granted";
    const THREAD: &str = "synthetic_thread";
    const TURN: &str = "synthetic_turn";
    const ACP_SESSION: &str = "synthetic_session";

    fn route(protocol: NativeProtocol) -> SessionRoute {
        let harness = match protocol {
            CodexExec | CodexAppServer => AgentKind::Codex,
            ClaudeStreamJson => AgentKind::Claude,
            Acp => AgentKind::OpenCode,
            DshSdkJsonRpc => AgentKind::Dsh,
        };
        SessionRoute {
            harness,
            protocol,
            model: None,
            provider: None,
        }
    }

    fn dsh_route() -> SessionRoute {
        SessionRoute {
            model: Some("synthetic_model".to_owned()),
            provider: Some("synthetic_provider".to_owned()),
            ..route(DshSdkJsonRpc)
        }
    }

    fn start(route: SessionRoute, prompt: &str, root: &str) -> ProtocolSession {
        ProtocolSession::execution(route, prompt.to_owned(), root.to_owned(), MAX_PROMPT_BYTES)
            .expect("execution session")
    }

    fn execution(protocol: NativeProtocol) -> ProtocolSession {
        start(route(protocol), PROMPT, ROOT)
    }

    fn probe(route: SessionRoute) -> ProtocolSession {
        ProtocolSession::probe(route, ROOT.to_owned()).expect("probe session")
    }

    /// Feeds one record with the kind the daemon's classifier assigns it.
    fn feed(session: &mut ProtocolSession, value: &Value) -> Result<Vec<Value>, DispatchError> {
        session.receive(value, classify(session.protocol(), value))
    }

    fn message(input: InitialInput) -> Value {
        match input {
            InitialInput::Message(value) => value,
            InitialInput::PromptThenClose(_) => panic!("expected a JSON message"),
        }
    }

    fn app_server_initialized() -> Value {
        json!({"id": 1, "result": {"userAgent": "synthetic_agent"}})
    }

    /// A Codex app-server session that has its thread and awaits the turn.
    fn app_server_with_thread() -> ProtocolSession {
        let mut session = execution(CodexAppServer);
        feed(&mut session, &app_server_initialized()).expect("initialize");
        feed(
            &mut session,
            &json!({"id": 2, "result": {"thread": {"id": THREAD}}}),
        )
        .expect("thread");
        session
    }

    fn turn_notification(method: &str, turn: &str, status: &str) -> Value {
        json!({"method": method,
            "params": {"threadId": THREAD, "turn": {"id": turn, "status": status}}})
    }

    fn acp_rpc(id: u64, result: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "result": result})
    }

    /// An ACP session that has its session id and awaits the prompt result.
    fn acp_with_session() -> ProtocolSession {
        let mut session = execution(Acp);
        feed(&mut session, &acp_rpc(1, json!({"protocolVersion": 1}))).expect("initialize");
        feed(&mut session, &acp_rpc(2, json!({"sessionId": ACP_SESSION}))).expect("session");
        session
    }

    fn claude_initialized() -> Value {
        json!({"type": "control_response",
            "response": {"subtype": "success", "request_id": "vibemux_initialize"}})
    }

    fn dsh_session_id(root: &str, prompt: &str) -> String {
        let digest =
            Sha256Digest::of_fields("vibemux.dsh.session", &[root.as_bytes(), prompt.as_bytes()]);
        format!("vibemux_{}", &digest.to_hex()[..32])
    }

    fn dsh_rpc(id: &str, result: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "result": result})
    }

    fn dsh_initialized() -> Value {
        dsh_rpc(
            "vibemux_initialize",
            json!({"serverInfo": {"name": "deepseek-harness-sdk-runtime"}}),
        )
    }

    fn dsh_notification(method: &str, session_id: &str, event: Value) -> Value {
        json!({"jsonrpc": "2.0", "method": method,
            "params": {"sessionId": session_id, "event": event}})
    }

    fn dsh_turn_end(session_id: &str, turn: u64, reason: &str) -> Value {
        dsh_notification(
            "session.event",
            session_id,
            json!({"type": "turn/end", "data": {"turn": turn, "reason": {"kind": reason}}}),
        )
    }

    /// A DSH session that sent its prompt and awaits the turn.
    fn dsh_prompted() -> (ProtocolSession, String) {
        let mut session = start(dsh_route(), PROMPT, ROOT);
        feed(&mut session, &dsh_initialized()).expect("initialize");
        (session, dsh_session_id(ROOT, PROMPT))
    }

    #[test]
    fn terminal_names_and_debug_forms_are_pinned_and_content_free() {
        for (terminal, name) in [
            (SessionTerminal::Completed, "completed"),
            (SessionTerminal::Failed, "failed"),
            (SessionTerminal::Cancelled, "cancelled"),
            (SessionTerminal::Probed, "probed"),
        ] {
            assert_eq!(
                serde_json::to_value(terminal).expect("serialize"),
                json!(name)
            );
            let parsed: SessionTerminal = serde_json::from_value(json!(name)).expect("deserialize");
            assert_eq!(parsed, terminal);
        }
        assert_eq!(
            format!("{:?}", InitialInput::Message(json!({"text": PROMPT}))),
            "InitialInput::Message(..)"
        );
        assert_eq!(
            format!("{:?}", InitialInput::PromptThenClose(PROMPT.to_owned())),
            "InitialInput::PromptThenClose(16 bytes)"
        );
        assert_eq!(
            format!("{:?}", execution(ClaudeStreamJson)),
            "ProtocolSession { harness: Claude, protocol: ClaudeStreamJson, probe: false, \
             phase: Initialize, terminal: None, .. }"
        );
        assert_eq!(
            format!("{:?}", probe(route(Acp))),
            "ProtocolSession { harness: OpenCode, protocol: Acp, probe: true, \
             phase: Initialize, terminal: None, .. }"
        );
    }

    #[test]
    fn construction_checks_the_route_prompt_and_working_directory() {
        let daemon_route = DispatchRoute {
            harness: AgentKind::Dsh,
            protocol: DshSdkJsonRpc,
            executable: PathBuf::from("synthetic_node.exe"),
            entry_script: Some(PathBuf::from("synthetic_entry.js")),
            profile: None,
            provider: Some("synthetic_provider".to_owned()),
            environment_names: Vec::new(),
            enabled: true,
            allow_execution: true,
            model: Some("synthetic_model".to_owned()),
        };
        assert_eq!(SessionRoute::from(&daemon_route), dsh_route());
        let at_limit = "a".repeat(MAX_PROMPT_BYTES);
        let over_limit = "a".repeat(MAX_PROMPT_BYTES + 1);
        assert!(ProtocolSession::for_execution(&daemon_route, at_limit, ROOT.to_owned()).is_ok());
        assert_eq!(
            ProtocolSession::for_execution(&daemon_route, over_limit, ROOT.to_owned()).err(),
            Some(InvalidPrompt)
        );
        assert!(ProtocolSession::for_probe(&daemon_route, ROOT.to_owned()).is_ok());

        assert_eq!(
            ProtocolSession::probe(route(CodexExec), ROOT.to_owned()).err(),
            Some(ProbeUnsupported)
        );
        let refused_prompt = |prompt: &str| {
            ProtocolSession::execution(route(CodexExec), prompt.to_owned(), ROOT.to_owned(), 8)
                .err()
        };
        for prompt in ["", " \n\t", "123456789"] {
            assert_eq!(refused_prompt(prompt), Some(InvalidPrompt), "{prompt:?}");
        }
        assert_eq!(refused_prompt("12345678"), None);
        let mismatched = SessionRoute {
            harness: AgentKind::Codex,
            ..dsh_route()
        };
        assert_eq!(
            ProtocolSession::probe(mismatched, ROOT.to_owned()).err(),
            Some(ConfigRouteInvalid)
        );
        for root in ["", "synthetic\0root"] {
            assert_eq!(
                ProtocolSession::execution(
                    route(CodexExec),
                    PROMPT.to_owned(),
                    root.to_owned(),
                    MAX_PROMPT_BYTES
                )
                .err(),
                Some(WorkingDirectoryInvalid),
                "{root:?}"
            );
        }
    }

    #[test]
    fn sessions_start_in_their_protocol_phase_and_dsh_ids_are_input_digests() {
        for protocol in NativeProtocol::ALL {
            let session = execution(protocol);
            let expected = if protocol == CodexExec {
                Phase::Prompt
            } else {
                Phase::Initialize
            };
            assert_eq!(session.phase, expected, "{protocol:?}");
            assert_eq!(session.protocol(), protocol);
            assert_eq!(session.terminal(), None);
            assert!(!session.is_finished(), "{protocol:?}");
            assert!(!session.probe);
            if protocol != CodexExec {
                assert!(probe(route(protocol)).probe, "{protocol:?}");
            }
        }
        let identity = dsh_session_id(ROOT, PROMPT);
        assert_eq!(identity.len(), 40);
        let session_id = |session: ProtocolSession| session.dsh_session_id;
        assert_eq!(
            session_id(start(dsh_route(), PROMPT, ROOT)),
            Some(identity.clone())
        );
        let other_prompt = session_id(start(dsh_route(), "other prompt", ROOT));
        assert_eq!(other_prompt, Some(dsh_session_id(ROOT, "other prompt")));
        assert_ne!(other_prompt, Some(identity.clone()));
        let other_root = session_id(start(dsh_route(), PROMPT, "OTHER_ROOT"));
        assert_eq!(other_root, Some(dsh_session_id("OTHER_ROOT", PROMPT)));
        assert_ne!(other_root, Some(identity));
        assert_eq!(
            session_id(probe(dsh_route())),
            Some(dsh_session_id(ROOT, ""))
        );
    }

    #[test]
    fn initial_inputs_pin_every_handshake_and_offer_no_client_capability() {
        let version = env!("CARGO_PKG_VERSION");
        assert_eq!(
            execution(CodexExec).initial_input(),
            InitialInput::PromptThenClose(PROMPT.to_owned())
        );
        assert_eq!(
            message(start(dsh_route(), PROMPT, ROOT).initial_input()),
            json!({"jsonrpc": "2.0", "id": "vibemux_initialize", "method": "initialize",
                "params": {"cwd": ROOT, "provider": "synthetic_provider",
                    "model": "synthetic_model"}})
        );
        assert_eq!(
            message(execution(DshSdkJsonRpc).initial_input())["params"],
            json!({"cwd": ROOT, "provider": "", "model": ""})
        );
        assert_eq!(
            message(execution(CodexAppServer).initial_input()),
            json!({"id": 1, "method": "initialize", "params": {"clientInfo": {
                "name": "vibemux", "title": "VibeMux", "version": version}}})
        );
        assert_eq!(
            message(execution(Acp).initial_input()),
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": 1,
                "clientCapabilities": {
                    "fs": {"readTextFile": false, "writeTextFile": false},
                    "terminal": false,
                },
                "clientInfo": {"name": "vibemux", "version": version},
            }})
        );
        assert_eq!(
            message(execution(ClaudeStreamJson).initial_input()),
            json!({"type": "control_request", "request_id": "vibemux_initialize",
                "request": {"subtype": "initialize", "hooks": null, "skills": []}})
        );
    }

    #[test]
    fn cancel_messages_address_only_an_identified_turn_or_session() {
        assert_eq!(execution(CodexExec).cancel_message(), None);
        assert_eq!(dsh_prompted().0.cancel_message(), None);
        assert_eq!(
            execution(ClaudeStreamJson).cancel_message(),
            Some(
                json!({"type": "control_request", "request_id": "vibemux_interrupt",
                "request": {"subtype": "interrupt"}})
            )
        );
        assert_eq!(execution(Acp).cancel_message(), None);
        assert_eq!(
            acp_with_session().cancel_message(),
            Some(json!({"jsonrpc": "2.0", "method": "session/cancel",
                "params": {"sessionId": ACP_SESSION}}))
        );
        assert_eq!(execution(CodexAppServer).cancel_message(), None);
        let mut app_server = app_server_with_thread();
        assert_eq!(app_server.cancel_message(), None);
        assert_eq!(
            feed(
                &mut app_server,
                &json!({"id": 3, "result": {"turn": {"id": TURN}}})
            ),
            Ok(vec![])
        );
        assert_eq!(
            app_server.cancel_message(),
            Some(json!({"id": 4, "method": "turn/interrupt",
                "params": {"threadId": THREAD, "turnId": TURN}}))
        );
    }

    #[test]
    fn codex_exec_maps_each_terminal_kind_once_and_keeps_observing_progress() {
        for (kind, terminal) in [
            (ObservationKind::Completed, SessionTerminal::Completed),
            (ObservationKind::Failed, SessionTerminal::Failed),
            (ObservationKind::Cancelled, SessionTerminal::Cancelled),
        ] {
            let mut session = execution(CodexExec);
            assert_eq!(
                session.receive(&json!({}), ObservationKind::Text),
                Ok(vec![])
            );
            assert_eq!(session.terminal(), None);
            assert_eq!(session.receive(&json!({}), kind), Ok(vec![]));
            assert_eq!(session.terminal(), Some(terminal));
            assert!(session.is_finished());
            assert_eq!(
                session.receive(&json!({}), ObservationKind::Opaque),
                Ok(vec![])
            );
            assert_eq!(session.receive(&json!({}), kind), Err(UnexpectedTerminal));
            assert_eq!(session.terminal(), Some(terminal));
        }
        let mut session = execution(CodexExec);
        for kind in ObservationKind::ALL {
            if !kind.is_terminal() {
                session.finish_with(kind);
                assert_eq!(session.terminal(), None, "{kind:?}");
                assert_eq!(session.phase, Phase::Prompt, "{kind:?}");
            }
        }
    }

    #[test]
    fn codex_app_server_exchange_pins_every_reply() {
        let routed = SessionRoute {
            model: Some("synthetic_model".to_owned()),
            ..route(CodexAppServer)
        };
        let mut session = start(routed, PROMPT, ROOT);
        assert_eq!(
            feed(&mut session, &app_server_initialized()),
            Ok(vec![
                json!({"method": "initialized"}),
                json!({"id": 2, "method": "thread/start", "params": {"cwd": ROOT,
                    "approvalPolicy": "never", "sandbox": "read-only", "ephemeral": true,
                    "model": "synthetic_model"}}),
            ])
        );
        assert_eq!(session.phase, Phase::Session);
        assert_eq!(
            feed(
                &mut session,
                &json!({"id": 2, "result": {"thread": {"id": THREAD}}})
            ),
            Ok(vec![
                json!({"id": 3, "method": "turn/start", "params": {"threadId": THREAD,
                "input": [{"type": "text", "text": PROMPT}]}})
            ])
        );
        assert_eq!(session.phase, Phase::Prompt);
        assert_eq!(
            feed(
                &mut session,
                &json!({"id": 3, "result": {"turn": {"id": TURN}}})
            ),
            Ok(vec![])
        );
        assert_eq!(session.turn_id.as_deref(), Some(TURN));
        assert_eq!(
            feed(
                &mut session,
                &turn_notification("turn/started", TURN, "inProgress")
            ),
            Ok(vec![])
        );
        for (id, method) in [
            (
                json!("approval_one"),
                "item/commandExecution/requestApproval",
            ),
            (json!(-7), "item/fileChange/requestApproval"),
        ] {
            assert_eq!(
                feed(
                    &mut session,
                    &json!({"id": id, "method": method, "params": {}})
                ),
                Ok(vec![json!({"id": id, "result": {"decision": "decline"}})]),
                "{method}"
            );
        }
        assert_eq!(
            feed(&mut session, &json!({"id": 8, "method": "item/tool/call"})),
            Ok(vec![
                json!({"id": 8, "error": {"code": -32601, "message": REFUSAL}})
            ])
        );
        assert_eq!(
            feed(&mut session, &json!({"id": 4, "result": {}})),
            Ok(vec![])
        );
        assert_eq!(session.terminal(), None);
        assert_eq!(
            feed(
                &mut session,
                &turn_notification("turn/completed", TURN, "failed")
            ),
            Ok(vec![])
        );
        assert_eq!(session.terminal(), Some(SessionTerminal::Failed));
        assert!(session.is_finished());
    }

    #[test]
    fn codex_app_server_rejects_uncorrelated_or_malformed_evidence() {
        for response in [
            json!({"id": 1}),
            json!({"id": 1, "result": "synthetic_agent"}),
            json!({"id": 1, "result": ["synthetic_agent"]}),
        ] {
            let mut session = execution(CodexAppServer);
            assert_eq!(
                feed(&mut session, &response),
                Err(InvalidRpcResult),
                "{response}"
            );
        }
        let mut session = execution(CodexAppServer);
        feed(&mut session, &app_server_initialized()).expect("initialize");
        assert_eq!(
            feed(&mut session, &json!({"id": 2, "result": {"thread": {}}})),
            Err(InvalidIdentifier)
        );
        assert_eq!(
            feed(
                &mut app_server_with_thread(),
                &json!({"id": 3, "result": {}})
            ),
            Err(InvalidIdentifier)
        );
        let mut started_first = app_server_with_thread();
        feed(
            &mut started_first,
            &turn_notification("turn/started", TURN, "inProgress"),
        )
        .expect("turn started");
        assert_eq!(
            feed(
                &mut started_first,
                &json!({"id": 3, "result": {"turn": {"id": "other_turn"}}})
            ),
            Err(UncorrelatedTurn)
        );

        let mut session = app_server_with_thread();
        assert_eq!(
            feed(
                &mut session,
                &json!({"method": "turn/started",
                    "params": {"threadId": "other_thread", "turn": {"id": TURN}}})
            ),
            Err(UncorrelatedSession)
        );
        assert_eq!(
            feed(
                &mut session,
                &json!({"method": "turn/started", "params": {"threadId": THREAD, "turn": {}}})
            ),
            Err(InvalidIdentifier)
        );
        assert_eq!(
            feed(
                &mut session,
                &turn_notification("turn/completed", TURN, "completed")
            ),
            Err(UnexpectedTerminal)
        );
        assert_eq!(session.terminal(), None);

        let mut finished = app_server_with_thread();
        feed(
            &mut finished,
            &json!({"id": 3, "result": {"turn": {"id": TURN}}}),
        )
        .expect("turn");
        feed(
            &mut finished,
            &turn_notification("turn/completed", TURN, "completed"),
        )
        .expect("completed");
        assert_eq!(finished.terminal(), Some(SessionTerminal::Completed));
        for (record, expected) in [
            (
                turn_notification("turn/completed", TURN, "completed"),
                UnexpectedTerminal,
            ),
            (json!({"id": 1, "result": {}}), UnexpectedResponse),
            (json!({"id": 3, "error": {"code": 1}}), UnexpectedResponse),
        ] {
            assert_eq!(feed(&mut finished, &record), Err(expected), "{record}");
        }

        let mut fresh = execution(CodexAppServer);
        assert_eq!(
            feed(
                &mut fresh,
                &json!({"method": "session/update", "params": {}})
            ),
            Ok(vec![])
        );
    }

    #[test]
    fn acp_exchange_pins_every_reply_and_maps_each_stop_reason() {
        let mut session = execution(Acp);
        assert_eq!(
            feed(&mut session, &acp_rpc(1, json!({"protocolVersion": 1}))),
            Ok(vec![
                json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
                "params": {"cwd": ROOT, "mcpServers": []}})
            ])
        );
        assert_eq!(
            feed(&mut session, &acp_rpc(2, json!({"sessionId": ACP_SESSION}))),
            Ok(vec![
                json!({"jsonrpc": "2.0", "id": 3, "method": "session/prompt",
                "params": {"sessionId": ACP_SESSION,
                    "prompt": [{"type": "text", "text": PROMPT}]}})
            ])
        );
        let update = |session_id: &str| {
            json!({"jsonrpc": "2.0", "method": "session/update",
                "params": {"sessionId": session_id}})
        };
        assert_eq!(feed(&mut session, &update(ACP_SESSION)), Ok(vec![]));
        assert_eq!(
            feed(&mut session, &update("other_session")),
            Err(UncorrelatedSession)
        );
        assert_eq!(
            feed(
                &mut session,
                &json!({"jsonrpc": "2.0", "method": "turn/completed", "params": {}})
            ),
            Ok(vec![])
        );
        assert_eq!(
            feed(
                &mut session,
                &json!({"jsonrpc": "2.0", "id": "permission_one",
                    "method": "session/request_permission", "params": {}})
            ),
            Ok(vec![json!({"jsonrpc": "2.0", "id": "permission_one",
                "result": {"outcome": {"outcome": "cancelled"}}})])
        );
        assert_eq!(session.terminal(), None);
        for (stop_reason, terminal) in [
            ("end_turn", SessionTerminal::Completed),
            ("refusal", SessionTerminal::Failed),
            ("cancelled", SessionTerminal::Cancelled),
        ] {
            let mut session = acp_with_session();
            assert_eq!(
                feed(
                    &mut session,
                    &acp_rpc(3, json!({"stopReason": stop_reason}))
                ),
                Ok(vec![])
            );
            assert_eq!(session.terminal(), Some(terminal), "{stop_reason}");
            assert!(session.is_finished());
        }
        let mut session = execution(Acp);
        assert_eq!(
            feed(&mut session, &acp_rpc(1, json!({}))),
            Err(UnsupportedProtocolVersion)
        );
        let mut session = execution(Acp);
        feed(&mut session, &acp_rpc(1, json!({"protocolVersion": 1}))).expect("initialize");
        assert_eq!(
            feed(&mut session, &acp_rpc(2, json!({"sessionId": ""}))),
            Err(InvalidIdentifier)
        );
    }

    #[test]
    fn probes_finish_on_a_valid_handshake_and_send_nothing_more() {
        let mut app_server = probe(route(CodexAppServer));
        assert_eq!(
            feed(&mut app_server, &app_server_initialized()),
            Ok(vec![json!({"method": "initialized"})])
        );
        let mut acp = probe(route(Acp));
        assert_eq!(
            feed(&mut acp, &acp_rpc(1, json!({"protocolVersion": 1}))),
            Ok(vec![])
        );
        let mut claude = probe(route(ClaudeStreamJson));
        assert_eq!(feed(&mut claude, &claude_initialized()), Ok(vec![]));
        let mut dsh = probe(dsh_route());
        assert_eq!(feed(&mut dsh, &dsh_initialized()), Ok(vec![]));
        for session in [app_server, acp, claude, dsh] {
            assert_eq!(
                session.terminal(),
                Some(SessionTerminal::Probed),
                "{session:?}"
            );
            assert!(session.is_finished(), "{session:?}");
        }
    }

    #[test]
    fn claude_exchange_sends_one_user_message_and_refuses_every_request() {
        let mut session = execution(ClaudeStreamJson);
        assert_eq!(
            feed(&mut session, &json!({"type": "system", "subtype": "init"})),
            Ok(vec![])
        );
        let interrupted = json!({"type": "control_response",
            "response": {"subtype": "success", "request_id": "vibemux_interrupt"}});
        assert_eq!(feed(&mut session, &interrupted), Ok(vec![]));
        assert_eq!(session.phase, Phase::Initialize);
        assert_eq!(
            feed(&mut session, &claude_initialized()),
            Ok(vec![json!({"type": "user", "session_id": "",
                "message": {"role": "user", "content": PROMPT},
                "parent_tool_use_id": null})])
        );
        assert_eq!(session.phase, Phase::Prompt);
        assert_eq!(feed(&mut session, &interrupted), Ok(vec![]));
        assert_eq!(
            feed(&mut session, &claude_initialized()),
            Err(InvalidInitialize)
        );
        for request_id in [json!("permission_one"), json!(7)] {
            assert_eq!(
                feed(
                    &mut session,
                    &json!({"type": "control_request", "request_id": request_id,
                        "request": {"subtype": "can_use_tool"}})
                ),
                Ok(vec![json!({"type": "control_response", "response": {
                    "subtype": "error", "request_id": request_id, "error": REFUSAL}})]),
                "{request_id}"
            );
        }
        for request in [
            json!({"type": "control_request"}),
            json!({"type": "control_request", "request_id": ""}),
            json!({"type": "control_request", "request_id": {"nested": 1}}),
        ] {
            assert_eq!(feed(&mut session, &request), Err(InvalidRpcId), "{request}");
        }
        assert_eq!(
            session.receive(&json!({"type": "result"}), ObservationKind::Failed),
            Ok(vec![])
        );
        assert_eq!(session.terminal(), Some(SessionTerminal::Failed));
    }

    #[test]
    fn claude_handshake_needs_a_successful_correlated_response() {
        for response in [
            json!({"type": "control_response",
                "response": {"subtype": "error", "request_id": "vibemux_initialize"}}),
            json!({"type": "control_response",
                "response": {"subtype": "success", "request_id": "other"}}),
            json!({"type": "control_response", "response": {"subtype": "success"}}),
        ] {
            let mut session = execution(ClaudeStreamJson);
            assert_eq!(
                feed(&mut session, &response),
                Err(InvalidInitialize),
                "{response}"
            );
            assert_eq!(session.phase, Phase::Initialize);
        }
    }

    #[test]
    fn dsh_exchange_pins_the_prompt_and_ends_only_on_its_own_turn() {
        let mut session = start(dsh_route(), PROMPT, ROOT);
        let session_id = dsh_session_id(ROOT, PROMPT);
        assert_eq!(
            feed(&mut session, &dsh_initialized()),
            Ok(vec![json!({"jsonrpc": "2.0", "id": "vibemux_prompt",
                "method": "session/prompt", "params": {"sessionId": session_id,
                    "contentBlocks": [{"type": "text", "text": PROMPT}]}})])
        );
        assert_eq!(session.phase, Phase::Prompt);
        assert_eq!(
            feed(
                &mut session,
                &dsh_rpc("vibemux_prompt", json!({"messageId": "message_one"}))
            ),
            Ok(vec![])
        );
        assert_eq!(
            feed(
                &mut session,
                &dsh_notification(
                    "session.event",
                    &session_id,
                    json!({"type": "turn/start", "data": {"turn": 1}})
                )
            ),
            Ok(vec![])
        );
        assert_eq!(
            feed(&mut session, &dsh_turn_end("vibemux_other", 1, "completed")),
            Ok(vec![])
        );
        let turn_end = json!({"type": "turn/end",
            "data": {"turn": 1, "reason": {"kind": "completed"}}});
        for method in ["session.status", "subagent.started", "subagent.finished"] {
            let notification = dsh_notification(method, &session_id, turn_end.clone());
            assert_eq!(
                session.receive(&notification, ObservationKind::Completed),
                Ok(vec![]),
                "{method}"
            );
        }
        assert_eq!(
            feed(
                &mut session,
                &json!({"jsonrpc": "2.0", "method": "vendor.future", "params": {}})
            ),
            Ok(vec![])
        );
        assert_eq!(session.terminal(), None);
        assert_eq!(
            feed(&mut session, &dsh_turn_end(&session_id, 1, "completed")),
            Ok(vec![])
        );
        assert_eq!(session.terminal(), Some(SessionTerminal::Completed));
        assert!(session.is_finished());
        assert_eq!(
            feed(&mut session, &dsh_turn_end(&session_id, 1, "completed")),
            Err(UnexpectedTerminal)
        );
    }

    #[test]
    fn dsh_turn_end_correlates_the_turn_and_waits_out_unknown_reasons() {
        let (mut session, session_id) = dsh_prompted();
        assert_eq!(
            feed(&mut session, &dsh_turn_end(&session_id, 1, "future_reason")),
            Ok(vec![])
        );
        assert_eq!(session.terminal(), None);
        assert_eq!(session.turn_id.as_deref(), Some("1"));
        assert_eq!(
            feed(&mut session, &dsh_turn_end(&session_id, 2, "completed")),
            Err(UncorrelatedTurn)
        );
        assert_eq!(session.terminal(), None);
        for (reason, terminal) in [
            ("completed", SessionTerminal::Completed),
            ("error", SessionTerminal::Failed),
            ("aborted", SessionTerminal::Cancelled),
        ] {
            let (mut session, session_id) = dsh_prompted();
            assert_eq!(
                feed(&mut session, &dsh_turn_end(&session_id, 3, reason)),
                Ok(vec![])
            );
            assert_eq!(session.terminal(), Some(terminal), "{reason}");
        }
    }

    #[test]
    fn dsh_rejects_malformed_or_uncorrelated_frames() {
        let session_id = dsh_session_id(ROOT, PROMPT);
        let initialize = json!({"serverInfo": {"name": "deepseek-harness-sdk-runtime"}});
        for (frame, expected) in [
            (
                json!({"id": "vibemux_initialize", "result": initialize}),
                Err(InvalidJsonRpc),
            ),
            (
                json!({"jsonrpc": "1.0", "id": "vibemux_initialize", "result": initialize}),
                Err(InvalidJsonRpc),
            ),
            (
                json!({"jsonrpc": "2.0", "id": 1, "result": initialize}),
                Err(UncorrelatedResponse),
            ),
            (
                dsh_rpc("other", initialize.clone()),
                Err(UncorrelatedResponse),
            ),
            (
                json!({"jsonrpc": "2.0", "id": "vibemux_initialize", "error": {"code": 1}}),
                Err(RpcError),
            ),
            (
                json!({"jsonrpc": "2.0", "id": "vibemux_initialize"}),
                Err(InvalidRpcResult),
            ),
            (
                dsh_rpc("vibemux_initialize", json!("ready")),
                Err(InvalidRpcResult),
            ),
            (
                dsh_rpc(
                    "vibemux_initialize",
                    json!({"serverInfo": {"name": "other-runtime"}}),
                ),
                Err(InvalidInitialize),
            ),
            (
                dsh_rpc("vibemux_prompt", json!({"messageId": "message_one"})),
                Err(UnexpectedResponse),
            ),
            (
                json!({"jsonrpc": "2.0", "id": "", "method": "approval/ask"}),
                Err(InvalidRpcId),
            ),
            (
                json!({"jsonrpc": "2.0", "id": "request_one", "method": "approval/ask"}),
                Ok(vec![json!({"jsonrpc": "2.0", "id": "request_one",
                    "error": {"code": -32601, "message": REFUSAL}})]),
            ),
            (
                dsh_turn_end(&session_id, 1, "completed"),
                Err(UnexpectedTerminal),
            ),
            (
                dsh_notification(
                    "session.event",
                    &session_id,
                    json!({"type": "turn/start", "data": {"turn": 1}}),
                ),
                Ok(vec![]),
            ),
            (
                dsh_notification("session.status", &session_id, json!({"type": "turn/end"})),
                Ok(vec![]),
            ),
            (
                json!({"jsonrpc": "2.0", "method": "vendor.future", "params": {}}),
                Ok(vec![]),
            ),
        ] {
            let mut session = start(dsh_route(), PROMPT, ROOT);
            assert_eq!(feed(&mut session, &frame), expected, "{frame}");
            assert_eq!(session.phase, Phase::Initialize, "{frame}");
        }
        for method in [
            "session.event",
            "session.status",
            "subagent.started",
            "subagent.finished",
        ] {
            let (mut session, _) = dsh_prompted();
            assert_eq!(
                feed(
                    &mut session,
                    &json!({"jsonrpc": "2.0", "method": method, "params": {}})
                ),
                Err(UncorrelatedSession),
                "{method}"
            );
        }
        for (frame, expected) in [
            (dsh_initialized(), UnexpectedResponse),
            (dsh_rpc("vibemux_prompt", json!({})), InvalidIdentifier),
            (
                dsh_rpc("vibemux_prompt", json!({"messageId": ""})),
                InvalidIdentifier,
            ),
            (
                dsh_notification(
                    "session.event",
                    &session_id,
                    json!({"type": "turn/end", "data": {"reason": {"kind": "completed"}}}),
                ),
                InvalidIdentifier,
            ),
            (
                dsh_notification(
                    "session.event",
                    &session_id,
                    json!({"type": "turn/end",
                        "data": {"turn": "1", "reason": {"kind": "completed"}}}),
                ),
                InvalidIdentifier,
            ),
        ] {
            let (mut session, _) = dsh_prompted();
            assert_eq!(feed(&mut session, &frame), Err(expected), "{frame}");
            assert_eq!(session.terminal(), None, "{frame}");
        }
    }

    #[test]
    fn identifiers_and_rpc_ids_are_bounded() {
        let at_limit = "a".repeat(512);
        let over_limit = "a".repeat(513);
        assert_eq!(identifier(Some(&json!(at_limit))), Ok(at_limit.clone()));
        for refused in [json!(""), json!(over_limit), json!(7), Value::Null] {
            assert_eq!(
                identifier(Some(&refused)),
                Err(InvalidIdentifier),
                "{refused}"
            );
        }
        assert_eq!(identifier(None), Err(InvalidIdentifier));
        for accepted in [
            json!("a"),
            json!(at_limit),
            json!(0),
            json!(-1),
            json!(i64::MAX),
            json!(i64::MIN),
        ] {
            assert_eq!(validate_rpc_id(&accepted), Ok(()), "{accepted}");
        }
        for refused in [
            json!(""),
            json!(over_limit),
            json!(u64::MAX),
            json!(1.5),
            Value::Null,
            json!(true),
            json!([1]),
            json!({"id": 1}),
        ] {
            assert_eq!(validate_rpc_id(&refused), Err(InvalidRpcId), "{refused}");
        }
    }
}
