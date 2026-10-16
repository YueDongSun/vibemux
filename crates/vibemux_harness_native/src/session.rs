use serde_json::{Value, json};

use crate::{
    Error,
    config::Route,
    contract::{Harness, Kind, Protocol, Request, Status},
};

const INITIALIZE_ID: u64 = 1;
const SESSION_ID: u64 = 2;
const PROMPT_ID: u64 = 3;
const CANCEL_ID: u64 = 4;
const CLAUDE_INITIALIZE_ID: &str = "vibemux_initialize";

#[derive(Clone, Copy, Eq, PartialEq)]
enum Phase {
    Initialize,
    Session,
    Prompt,
    Finished,
}

pub(crate) struct Session {
    route: Route,
    request: Request,
    probe: bool,
    phase: Phase,
    session_id: Option<String>,
    turn_id: Option<String>,
    pub status: Option<Status>,
}

impl Session {
    pub fn new(route: Route, request: Request, probe: bool) -> Self {
        let phase = if route.protocol == Protocol::Codex {
            Phase::Prompt
        } else {
            Phase::Initialize
        };
        Self {
            route,
            request,
            probe,
            phase,
            session_id: None,
            turn_id: None,
            status: None,
        }
    }

    pub fn arguments(&self) -> Result<Vec<String>, Error> {
        let mut arguments = self.route.arguments.clone();
        let fixed: &[&str] = match self.route.protocol {
            Protocol::Appserver => &["app-server", "--stdio"],
            Protocol::Acp => match self.route.harness {
                Harness::Opencode => &["acp"],
                Harness::Copilot => &["--acp", "--no-auto-update"],
                Harness::Grok => &["agent", "stdio"],
                _ => return Err(Error::new("harness_unsupported_protocol")),
            },
            Protocol::Claude => &[
                "--bare",
                "-p",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--verbose",
                "--include-partial-messages",
                "--permission-mode",
                "dontAsk",
                "--tools=",
                "--no-session-persistence",
            ],
            Protocol::Codex if !self.probe => &[
                "exec",
                "--json",
                "--sandbox",
                "read-only",
                "--ephemeral",
                "-",
            ],
            _ => return Err(Error::new("harness_native_execution_unsupported")),
        };
        arguments.extend(fixed.iter().map(|value| (*value).to_string()));
        if let Some(model) = &self.route.model {
            if matches!(self.route.protocol, Protocol::Claude | Protocol::Codex) {
                arguments.extend(["--model".to_string(), model.clone()]);
            } else if self.route.protocol == Protocol::Acp {
                return Err(Error::new("harness_acp_model_selection_unsupported"));
            }
        }
        Ok(arguments)
    }

    pub fn initial(&self) -> Option<Value> {
        match self.route.protocol {
            Protocol::Appserver => Some(json!({"id":INITIALIZE_ID, "method":"initialize",
                "params":{"clientInfo":{"name":"vibemux","title":"VibeMux","version":"0.2.0"}}})),
            Protocol::Acp => Some(
                json!({"jsonrpc":"2.0","id":INITIALIZE_ID,"method":"initialize",
                "params":{"protocolVersion":1,"clientCapabilities":{
                    "fs":{"readTextFile":false,"writeTextFile":false},"terminal":false},
                    "clientInfo":{"name":"vibemux","version":"0.2.0"}}}),
            ),
            Protocol::Claude => Some(
                json!({"type":"control_request","request_id":CLAUDE_INITIALIZE_ID,
                "request":{"subtype":"initialize","hooks":null,"skills":[]}}),
            ),
            _ => None,
        }
    }

    pub fn receive(&mut self, value: &Value, kind: Kind) -> Result<Vec<Value>, Error> {
        if self.route.protocol == Protocol::Claude {
            return self.claude(value, kind);
        }
        if self.route.protocol == Protocol::Codex {
            if self.phase != Phase::Prompt
                && matches!(kind, Kind::Completed | Kind::Failed | Kind::Cancelled)
            {
                return Err(Error::new("harness_duplicate_terminal"));
            }
            self.finish_kind(kind);
            return Ok(vec![]);
        }
        if self.route.protocol == Protocol::Acp
            && value.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        {
            return Err(Error::new("harness_invalid_jsonrpc"));
        }
        if let Some(method) = value.get("method").and_then(Value::as_str) {
            if let Some(id) = value.get("id") {
                validate_rpc_id(id)?;
                let response = if self.route.protocol == Protocol::Acp
                    && method == "session/request_permission"
                {
                    json!({"jsonrpc":"2.0","id":id,"result":{"outcome":{"outcome":"cancelled"}}})
                } else if self.route.protocol == Protocol::Appserver
                    && matches!(
                        method,
                        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval"
                    )
                {
                    json!({"id":id,"result":{"decision":"decline"}})
                } else {
                    let mut response = json!({"id":id,"error":{"code":-32601,"message":"Client capability not granted"}});
                    if self.route.protocol == Protocol::Acp {
                        response["jsonrpc"] = json!("2.0");
                    }
                    response
                };
                return Ok(vec![response]);
            }
            if self.route.protocol == Protocol::Appserver
                && matches!(method, "turn/started" | "turn/completed")
            {
                self.validate_session(value.pointer("/params/threadId"))?;
                let turn_id = identifier(value.pointer("/params/turn/id"))?;
                if self
                    .turn_id
                    .as_ref()
                    .is_some_and(|expected| expected != &turn_id)
                {
                    return Err(Error::new("harness_uncorrelated_turn"));
                }
                if method == "turn/started" {
                    self.turn_id = Some(turn_id);
                } else {
                    if self.phase != Phase::Prompt || self.turn_id.is_none() {
                        return Err(Error::new("harness_unexpected_terminal"));
                    }
                    self.finish_kind(kind);
                    if self.status.is_none() {
                        return Err(Error::new("harness_invalid_terminal"));
                    }
                }
            }
            if self.route.protocol == Protocol::Acp && method == "session/update" {
                self.validate_session(value.pointer("/params/sessionId"))?;
            }
            return Ok(vec![]);
        }
        let Some(id) = value.get("id").and_then(Value::as_u64) else {
            return Err(Error::new("harness_uncorrelated_response"));
        };
        if id == CANCEL_ID {
            return Ok(vec![]);
        }
        let expected = match self.phase {
            Phase::Initialize => INITIALIZE_ID,
            Phase::Session => SESSION_ID,
            Phase::Prompt => PROMPT_ID,
            Phase::Finished => return Err(Error::new("harness_unexpected_response")),
        };
        if id != expected {
            return Err(Error::new("harness_uncorrelated_response"));
        }
        if value.get("error").is_some() {
            return Err(Error::new("harness_rpc_error"));
        }
        let result = value
            .get("result")
            .filter(|result| result.is_object())
            .ok_or_else(|| Error::new("harness_invalid_rpc_result"))?;
        match self.phase {
            Phase::Initialize => {
                if self.route.protocol == Protocol::Acp
                    && result.get("protocolVersion").and_then(Value::as_u64) != Some(1)
                {
                    return Err(Error::new("harness_unsupported_protocol_version"));
                }
                if self.route.protocol == Protocol::Appserver
                    && result.get("userAgent").and_then(Value::as_str).is_none()
                {
                    return Err(Error::new("harness_invalid_initialize"));
                }
                let mut requests = vec![];
                if self.route.protocol == Protocol::Appserver {
                    requests.push(json!({"method":"initialized"}));
                }
                if self.probe {
                    self.status = Some(Status::Probed);
                    self.phase = Phase::Finished;
                    return Ok(requests);
                }
                self.phase = Phase::Session;
                let cwd = self
                    .route
                    .working_directory
                    .to_str()
                    .ok_or_else(|| Error::new("harness_invalid_cwd"))?;
                let next = if self.route.protocol == Protocol::Acp {
                    json!({"jsonrpc":"2.0","id":SESSION_ID,"method":"session/new","params":{"cwd":cwd,"mcpServers":[]}})
                } else {
                    let mut params = json!({"cwd":cwd,"approvalPolicy":"never","sandbox":"read-only","ephemeral":true});
                    if let Some(model) = &self.route.model {
                        params["model"] = json!(model);
                    }
                    json!({"id":SESSION_ID,"method":"thread/start","params":params})
                };
                requests.push(next);
                Ok(requests)
            }
            Phase::Session => {
                let session_id = if self.route.protocol == Protocol::Acp {
                    identifier(result.get("sessionId"))?
                } else {
                    identifier(result.pointer("/thread/id"))?
                };
                self.session_id = Some(session_id.clone());
                self.phase = Phase::Prompt;
                let next = if self.route.protocol == Protocol::Acp {
                    json!({"jsonrpc":"2.0","id":PROMPT_ID,"method":"session/prompt",
                        "params":{"sessionId":session_id,"prompt":[{"type":"text","text":self.request.prompt}]}})
                } else {
                    json!({"id":PROMPT_ID,"method":"turn/start","params":{"threadId":session_id,
                        "input":[{"type":"text","text":self.request.prompt}]}})
                };
                Ok(vec![next])
            }
            Phase::Prompt => {
                if self.route.protocol == Protocol::Acp {
                    self.finish_kind(kind);
                    if self.status.is_none() {
                        return Err(Error::new("harness_invalid_terminal"));
                    }
                } else {
                    let turn_id = identifier(result.pointer("/turn/id"))?;
                    if self
                        .turn_id
                        .as_ref()
                        .is_some_and(|expected| expected != &turn_id)
                    {
                        return Err(Error::new("harness_uncorrelated_turn"));
                    }
                    self.turn_id = Some(turn_id);
                }
                Ok(vec![])
            }
            Phase::Finished => Err(Error::new("harness_unexpected_response")),
        }
    }

    fn claude(&mut self, value: &Value, kind: Kind) -> Result<Vec<Value>, Error> {
        if value.get("type").and_then(Value::as_str) == Some("control_request") {
            let request_id = value
                .get("request_id")
                .ok_or_else(|| Error::new("harness_invalid_rpc_id"))?;
            validate_rpc_id(request_id)?;
            return Ok(vec![
                json!({"type":"control_response","response":{"subtype":"error",
                "request_id":request_id,"error":"Client capability not granted"}}),
            ]);
        }
        if value.get("type").and_then(Value::as_str) == Some("control_response") {
            if value
                .pointer("/response/request_id")
                .and_then(Value::as_str)
                == Some("vibemux_interrupt")
            {
                return Ok(vec![]);
            }
            if self.phase != Phase::Initialize
                || value
                    .pointer("/response/request_id")
                    .and_then(Value::as_str)
                    != Some(CLAUDE_INITIALIZE_ID)
                || value.pointer("/response/subtype").and_then(Value::as_str) != Some("success")
            {
                return Err(Error::new("harness_invalid_initialize"));
            }
            if self.probe {
                self.status = Some(Status::Probed);
                self.phase = Phase::Finished;
                return Ok(vec![]);
            }
            self.phase = Phase::Prompt;
            return Ok(vec![json!({"type":"user","session_id":"","message":{
                "role":"user","content":self.request.prompt},"parent_tool_use_id":null})]);
        }
        if matches!(kind, Kind::Completed | Kind::Failed | Kind::Cancelled) {
            if self.phase != Phase::Prompt {
                return Err(Error::new("harness_unexpected_terminal"));
            }
            self.finish_kind(kind);
        }
        Ok(vec![])
    }

    fn validate_session(&self, value: Option<&Value>) -> Result<(), Error> {
        if self.session_id.as_deref() != value.and_then(Value::as_str) || self.session_id.is_none()
        {
            return Err(Error::new("harness_uncorrelated_session"));
        }
        Ok(())
    }

    fn finish_kind(&mut self, kind: Kind) {
        let status = match kind {
            Kind::Completed => Status::Completed,
            Kind::Failed => Status::Failed,
            Kind::Cancelled => Status::Cancelled,
            _ => return,
        };
        self.status = Some(status);
        self.phase = Phase::Finished;
    }

    pub fn cancel(&self) -> Option<Value> {
        match self.route.protocol {
            Protocol::Acp => self.session_id.as_ref().map(|session_id|
                json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session_id}})),
            Protocol::Appserver => self.session_id.as_ref().zip(self.turn_id.as_ref()).map(|(session_id, turn_id)|
                json!({"id":CANCEL_ID,"method":"turn/interrupt","params":{"threadId":session_id,"turnId":turn_id}})),
            Protocol::Claude => Some(json!({"type":"control_request","request_id":"vibemux_interrupt",
                "request":{"subtype":"interrupt"}})),
            _ => None,
        }
    }
}

fn identifier(value: Option<&Value>) -> Result<String, Error> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= 512)
        .map(str::to_string)
        .ok_or_else(|| Error::new("harness_invalid_identifier"))
}

fn validate_rpc_id(value: &Value) -> Result<(), Error> {
    if value
        .as_str()
        .is_some_and(|value| !value.is_empty() && value.len() <= 512)
        || value.as_i64().is_some()
    {
        Ok(())
    } else {
        Err(Error::new("harness_invalid_rpc_id"))
    }
}
