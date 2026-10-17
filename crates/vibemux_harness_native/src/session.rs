//! The ADR 040 adapter over the shared ADR 029 protocol engine.
//!
//! `vibemux_harness::dispatch` owns the vendor handshake, the approval and
//! permission declines, identifier correlation, and the hardened argv. This
//! module only maps a native route and request onto that engine and keeps this
//! crate's own error codes, so the CLI-process dispatcher and the daemon drive
//! one vendor posture.

use serde_json::Value;
use vibemux_harness::dispatch::{
    DispatchError, NativeProtocol, ProtocolSession, SessionRoute, SessionTerminal,
    launch_spec::{SessionMode, structured_arguments},
    protocol_session::InitialInput,
};

use crate::{
    Error,
    config::Route,
    contract::{Kind, MAX_PROMPT_BYTES, Request, Status},
};

/// Protocol violations the engine reports while a session runs, and the
/// native code each keeps. Every code is fixed and content-free, so the
/// dispatcher also accepts them in a plugin's completion.
const PROTOCOL_ERROR_CODES: [(DispatchError, &str); 13] = [
    (DispatchError::InvalidJsonRpc, "harness_invalid_jsonrpc"),
    (DispatchError::InvalidRpcId, "harness_invalid_rpc_id"),
    (
        DispatchError::InvalidRpcResult,
        "harness_invalid_rpc_result",
    ),
    (DispatchError::RpcError, "harness_rpc_error"),
    (
        DispatchError::InvalidIdentifier,
        "harness_invalid_identifier",
    ),
    (
        DispatchError::InvalidInitialize,
        "harness_invalid_initialize",
    ),
    (
        DispatchError::UnsupportedProtocolVersion,
        "harness_unsupported_protocol_version",
    ),
    (
        DispatchError::UncorrelatedResponse,
        "harness_uncorrelated_response",
    ),
    (
        DispatchError::UnexpectedResponse,
        "harness_unexpected_response",
    ),
    (
        DispatchError::UncorrelatedSession,
        "harness_uncorrelated_session",
    ),
    (DispatchError::UncorrelatedTurn, "harness_uncorrelated_turn"),
    (
        DispatchError::UnexpectedTerminal,
        "harness_unexpected_terminal",
    ),
    (DispatchError::InvalidTerminal, "harness_invalid_terminal"),
];

pub(crate) struct Session {
    engine: ProtocolSession,
    arguments: Vec<String>,
}

impl Session {
    /// Validates the route against the engine before any process exists:
    /// a route the engine cannot drive fails here, not mid-handshake.
    pub fn new(route: &Route, request: &Request, probe: bool) -> Result<Self, Error> {
        let protocol = route
            .protocol
            .native_protocol()
            .ok_or_else(|| Error::new("harness_native_execution_unsupported"))?;
        // ACP has no verified model selector; refuse rather than ignore it.
        if protocol == NativeProtocol::Acp && route.model.is_some() {
            return Err(Error::new("harness_acp_model_selection_unsupported"));
        }
        let harness = route.harness.agent_kind();
        let working_directory = route
            .working_directory
            .to_str()
            .ok_or_else(|| Error::new("harness_invalid_cwd"))?
            .to_string();
        let shared_route = SessionRoute {
            harness,
            protocol,
            model: route.model.clone(),
            provider: None,
        };
        let (engine, mode) = if probe {
            (
                ProtocolSession::probe(shared_route, working_directory),
                SessionMode::Probe,
            )
        } else {
            (
                ProtocolSession::execution(
                    shared_route,
                    request.prompt.clone(),
                    working_directory,
                    MAX_PROMPT_BYTES,
                ),
                SessionMode::Execute,
            )
        };
        let engine = engine.map_err(native_error)?;
        // The operator's prefix comes first; the fixed posture flags follow,
        // so they are the ones a vendor parser sees last.
        let mut arguments = route.arguments.clone();
        arguments.extend(
            structured_arguments(harness, protocol, mode, route.model.as_deref())
                .map_err(native_error)?,
        );
        Ok(Self { engine, arguments })
    }

    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }

    /// The first JSON line to write, or `None` when the prompt itself is the
    /// whole input and stdin closes after it (`codex exec -`).
    pub fn initial(&self) -> Option<Value> {
        match self.engine.initial_input() {
            InitialInput::Message(message) => Some(message),
            InitialInput::PromptThenClose(_) => None,
        }
    }

    pub fn receive(&mut self, value: &Value, kind: Kind) -> Result<Vec<Value>, Error> {
        self.engine.receive(value, kind).map_err(native_error)
    }

    /// Correlated terminal evidence, once the protocol has reported it.
    pub fn status(&self) -> Option<Status> {
        self.engine.terminal().map(|terminal| match terminal {
            SessionTerminal::Completed => Status::Completed,
            SessionTerminal::Failed => Status::Failed,
            SessionTerminal::Cancelled => Status::Cancelled,
            SessionTerminal::Probed => Status::Probed,
        })
    }

    pub fn cancel(&self) -> Option<Value> {
        self.engine.cancel_message()
    }
}

/// Whether `code` is one of the fixed protocol-violation codes above.
pub(crate) fn is_protocol_error_code(code: &str) -> bool {
    PROTOCOL_ERROR_CODES.iter().any(|(_, known)| *known == code)
}

fn native_error(error: DispatchError) -> Error {
    Error::with_source(native_code(error), error)
}

fn native_code(error: DispatchError) -> &'static str {
    if let Some((_, code)) = PROTOCOL_ERROR_CODES
        .iter()
        .find(|(shared, _)| *shared == error)
    {
        return code;
    }
    match error {
        DispatchError::InvalidPrompt => "harness_invalid_prompt",
        DispatchError::WorkingDirectoryInvalid => "harness_invalid_cwd",
        DispatchError::ProbeUnsupported => "harness_native_execution_unsupported",
        DispatchError::ConfigRouteInvalid => "harness_unsupported_protocol",
        _ => "harness_session_internal",
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;
    use uuid::Uuid;

    use super::*;
    use crate::contract::{Harness, Protocol, SCHEMA_VERSION};

    fn route(harness: Harness, protocol: Protocol, model: Option<&str>) -> Route {
        Route {
            harness,
            protocol,
            executable: PathBuf::from("vendor"),
            arguments: vec!["--operator_prefix".to_string()],
            working_directory: PathBuf::from("project"),
            environment_names: vec![],
            enabled: true,
            allow_execution: true,
            model: model.map(str::to_string),
        }
    }

    fn request(prompt: &str) -> Request {
        Request {
            schema_version: SCHEMA_VERSION,
            request_id: Uuid::new_v4(),
            harness: Harness::Claude,
            prompt: prompt.to_string(),
        }
    }

    fn code(result: Result<Session, Error>) -> &'static str {
        match result {
            Ok(_) => "ok",
            Err(error) => error.code(),
        }
    }

    #[test]
    fn claude_routes_get_the_shared_hardened_argv_after_the_operator_prefix() {
        let session = Session::new(
            &route(Harness::Claude, Protocol::Claude, Some("sonnet")),
            &request("hello"),
            false,
        )
        .expect("claude session");
        let arguments = session.arguments();
        assert_eq!(arguments[0], "--operator_prefix");
        for flag in ["--strict-mcp-config", "--restricted", "--tools="] {
            assert!(arguments.iter().any(|argument| argument == flag), "{flag}");
        }
        assert_eq!(&arguments[arguments.len() - 2..], ["--model", "sonnet"]);
    }

    #[test]
    fn codex_exec_writes_the_prompt_and_keeps_the_stdin_marker_last() {
        let session = Session::new(
            &route(Harness::Codex, Protocol::Codex, Some("gpt")),
            &request("hello"),
            false,
        )
        .expect("codex exec session");
        assert_eq!(session.arguments().last().map(String::as_str), Some("-"));
        assert!(session.initial().is_none());
    }

    #[test]
    fn unsupported_routes_keep_their_native_codes() {
        let valid_request = request("hello");
        assert_eq!(
            code(Session::new(
                &route(Harness::Codex, Protocol::Codex, None),
                &valid_request,
                true
            )),
            "harness_native_execution_unsupported"
        );
        assert_eq!(
            code(Session::new(
                &route(Harness::Opencode, Protocol::Opencode, None),
                &valid_request,
                false
            )),
            "harness_native_execution_unsupported"
        );
        assert_eq!(
            code(Session::new(
                &route(Harness::Opencode, Protocol::Acp, Some("model")),
                &valid_request,
                true
            )),
            "harness_acp_model_selection_unsupported"
        );
        assert_eq!(
            code(Session::new(
                &route(Harness::Claude, Protocol::Claude, None),
                &request(" "),
                false
            )),
            "harness_invalid_prompt"
        );
    }

    #[test]
    fn engine_protocol_violations_map_to_allowlisted_native_codes() {
        let mut session = Session::new(
            &route(Harness::Grok, Protocol::Acp, None),
            &request("hello"),
            false,
        )
        .expect("acp session");
        let error = session
            .receive(&json!({"id": 1, "result": {}}), Kind::Opaque)
            .expect_err("missing jsonrpc");
        assert_eq!(error.code(), "harness_invalid_jsonrpc");
        assert!(is_protocol_error_code(error.code()));
        for (shared, native) in PROTOCOL_ERROR_CODES {
            assert_eq!(native_code(shared), native);
            assert!(native.starts_with("harness_"));
        }
        assert_eq!(native_code(DispatchError::Busy), "harness_session_internal");
    }

    #[test]
    fn a_duplicate_codex_exec_terminal_is_an_unexpected_terminal() {
        let mut session = Session::new(
            &route(Harness::Codex, Protocol::Codex, None),
            &request("hello"),
            false,
        )
        .expect("codex exec session");
        session
            .receive(&json!({"type": "turn.completed"}), Kind::Completed)
            .expect("first terminal");
        assert_eq!(session.status(), Some(Status::Completed));
        let error = session
            .receive(&json!({"type": "turn.completed"}), Kind::Completed)
            .expect_err("second terminal");
        assert_eq!(error.code(), "harness_unexpected_terminal");
    }
}
