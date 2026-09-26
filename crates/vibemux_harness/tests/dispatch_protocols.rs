//! Classification and protocol-session replay for the four native adapters.

use std::path::PathBuf;

use serde_json::{Value, json};
use vibemux_harness::AgentKind;
use vibemux_harness::dispatch::{
    DispatchError, DispatchRoute, NativeProtocol, ObservationKind, ObservedRecord, ProtocolSession,
    SessionTerminal, observation::classify, protocol_session::InitialInput,
};

const CODEX_EXEC: &str = include_str!("fixtures/dispatch/codex_exec.jsonl");
const CODEX_APP_SERVER: &str = include_str!("fixtures/dispatch/codex_app_server.jsonl");
const CLAUDE_STREAM: &str = include_str!("fixtures/dispatch/claude_stream.jsonl");
const ACP_STREAM: &str = include_str!("fixtures/dispatch/acp_stream.jsonl");

const TOKEN_SENTINEL: &str = "SYNTHETIC_SECRET_TOKEN";
const SHELL_TEXT: &str = "literal $(synthetic_command); & | > < `token` %SYNTHETIC_ENV%";
const PROMPT: &str = "synthetic prompt \"quoted\"\nsecond line $(synthetic_command) & | > <";
const WORKING_DIRECTORY: &str = "SYNTHETIC_PRIVATE_PATH";

fn route(harness: AgentKind, protocol: NativeProtocol) -> DispatchRoute {
    DispatchRoute {
        harness,
        protocol,
        executable: PathBuf::from("synthetic_executable"),
        environment_names: vec![],
        enabled: true,
        allow_execution: true,
        model: None,
    }
}

fn execution(harness: AgentKind, protocol: NativeProtocol) -> ProtocolSession {
    ProtocolSession::for_execution(
        &route(harness, protocol),
        PROMPT.to_string(),
        WORKING_DIRECTORY.to_string(),
    )
    .expect("execution session")
}

fn probe(harness: AgentKind, protocol: NativeProtocol) -> ProtocolSession {
    ProtocolSession::for_probe(&route(harness, protocol), WORKING_DIRECTORY.to_string())
        .expect("probe session")
}

fn parse(protocol: NativeProtocol, sequence: u64, line: &str) -> (ObservedRecord, Value) {
    let (record, value) =
        ObservedRecord::parse(protocol, sequence, line.to_string()).expect("fixture record");
    assert_eq!(record.raw_json().as_bytes(), line.as_bytes());
    (record, value)
}

fn feed(session: &mut ProtocolSession, value: Value) -> Result<Vec<Value>, DispatchError> {
    let kind = classify(session.protocol(), &value);
    session.receive(&value, kind)
}

/// Replays a fixture, returning the replies written after each record.
fn replay(session: &mut ProtocolSession, fixture: &str) -> Vec<Vec<Value>> {
    fixture
        .lines()
        .enumerate()
        .map(|(index, line)| {
            let (record, value) = parse(session.protocol(), index as u64 + 1, line);
            session
                .receive(&value, record.kind())
                .unwrap_or_else(|error| panic!("record {} rejected: {error}", index + 1))
        })
        .collect()
}

fn kinds(protocol: NativeProtocol, fixture: &str) -> Vec<ObservationKind> {
    fixture
        .lines()
        .enumerate()
        .map(|(index, line)| parse(protocol, index as u64 + 1, line).0.kind())
        .collect()
}

fn message(input: InitialInput) -> Value {
    match input {
        InitialInput::Message(value) => value,
        InitialInput::PromptThenClose(_) => panic!("expected a JSON message"),
    }
}

// ---- classification ----

#[test]
fn fixtures_classify_conservatively_and_keep_raw_bytes() {
    use ObservationKind::*;
    assert_eq!(
        kinds(NativeProtocol::CodexExec, CODEX_EXEC),
        [Started, Started, Text, Text, Tool, Opaque, Completed]
    );
    assert_eq!(
        kinds(NativeProtocol::CodexAppServer, CODEX_APP_SERVER),
        [
            Opaque, Opaque, Opaque, Started, Text, Text, Tool, Approval, Usage, Completed
        ]
    );
    assert_eq!(
        kinds(NativeProtocol::ClaudeStreamJson, CLAUDE_STREAM),
        [
            Opaque, Started, Tool, Text, Text, Usage, Approval, Tool, Completed
        ]
    );
    assert_eq!(
        kinds(NativeProtocol::Acp, ACP_STREAM),
        [
            Opaque, Opaque, Tool, Text, Text, Usage, Approval, Opaque, Completed
        ]
    );
    let (record, value) = parse(
        NativeProtocol::CodexExec,
        6,
        CODEX_EXEC.lines().nth(5).unwrap(),
    );
    assert_eq!(value["extension"]["nested"][2]["preserve"], TOKEN_SENTINEL);
    assert!(!format!("{record:?}").contains(TOKEN_SENTINEL));
    let (_, tool) = parse(
        NativeProtocol::CodexExec,
        5,
        CODEX_EXEC.lines().nth(4).unwrap(),
    );
    assert_eq!(tool["item"]["command"], SHELL_TEXT);
}

#[test]
fn unknown_discriminants_and_nested_claims_stay_opaque() {
    let payloads = [
        json!({"type": "turn.completed.extension", "status": "success"}),
        json!({"method": "turn/completed/extension", "params": {"turn": {"status": "completed"}}}),
        json!({"type": "vendor.extension", "extension": {"type": "turn.completed"}}),
        json!({"method": "vendor/extension", "result": {"stopReason": "end_turn"}}),
        Value::Null,
        json!(false),
        json!(7),
        json!(SHELL_TEXT),
        json!([]),
        json!({}),
    ];
    for protocol in NativeProtocol::ALL {
        for payload in &payloads {
            assert_eq!(classify(protocol, payload), ObservationKind::Opaque);
        }
    }
}

#[test]
fn terminal_discriminants_require_the_right_scope_and_status() {
    for (payload, expected) in [
        (
            json!({"type": "turn.completed"}),
            ObservationKind::Completed,
        ),
        (json!({"type": "turn.failed"}), ObservationKind::Failed),
        (json!({"type": "error"}), ObservationKind::Failed),
        (json!({"type": "turn.cancelled"}), ObservationKind::Opaque),
        (
            json!({"type": "item.completed", "item": {"type": "error"}}),
            ObservationKind::Opaque,
        ),
    ] {
        assert_eq!(classify(NativeProtocol::CodexExec, &payload), expected);
    }
    for (status, expected) in [
        ("completed", ObservationKind::Completed),
        ("failed", ObservationKind::Failed),
        ("interrupted", ObservationKind::Cancelled),
        ("inProgress", ObservationKind::Opaque),
        ("future_status", ObservationKind::Opaque),
    ] {
        let payload = json!({"method": "turn/completed", "params": {"turn": {"status": status}}});
        assert_eq!(classify(NativeProtocol::CodexAppServer, &payload), expected);
    }
    for (stop_reason, expected) in [
        ("end_turn", ObservationKind::Completed),
        ("cancelled", ObservationKind::Cancelled),
        ("max_tokens", ObservationKind::Failed),
        ("max_turn_requests", ObservationKind::Failed),
        ("refusal", ObservationKind::Failed),
        ("future_stop", ObservationKind::Opaque),
    ] {
        let payload = json!({"jsonrpc": "2.0", "id": 3, "result": {"stopReason": stop_reason}});
        assert_eq!(classify(NativeProtocol::Acp, &payload), expected);
    }
    for (payload, expected) in [
        (
            json!({"type": "result", "subtype": "success", "is_error": false}),
            ObservationKind::Completed,
        ),
        (
            json!({"type": "result", "subtype": "success", "is_error": true}),
            ObservationKind::Failed,
        ),
        (
            json!({"type": "result", "subtype": "success", "terminal_reason": "aborted_tools"}),
            ObservationKind::Cancelled,
        ),
        (
            json!({"type": "result", "subtype": "success", "terminal_reason": "max_turns"}),
            ObservationKind::Failed,
        ),
        (
            json!({"type": "result", "subtype": "success", "is_error": "true"}),
            ObservationKind::Opaque,
        ),
        (
            json!({"type": "result", "subtype": "success", "parent_tool_use_id": "nested"}),
            ObservationKind::Opaque,
        ),
        (
            json!({"type": "stream_event", "event": {"type": "message_stop"}}),
            ObservationKind::Opaque,
        ),
    ] {
        assert_eq!(
            classify(NativeProtocol::ClaudeStreamJson, &payload),
            expected
        );
    }
}

#[test]
fn record_parsing_rejects_non_objects_and_invalid_json_without_echoing_content() {
    let invalid = format!("{{\"{TOKEN_SENTINEL}\":");
    let error = ObservedRecord::parse(NativeProtocol::Acp, 1, invalid).unwrap_err();
    assert_eq!(error, DispatchError::InvalidJson);
    assert!(!error.to_string().contains(TOKEN_SENTINEL));
    assert_eq!(
        ObservedRecord::parse(NativeProtocol::Acp, 1, "[1]".to_string()).unwrap_err(),
        DispatchError::InvalidRecord
    );
    assert_eq!(
        ObservedRecord::parse(NativeProtocol::Acp, 0, "{}".to_string()).unwrap_err(),
        DispatchError::Internal
    );
    let spaced = r#"  { "type" : "vendor.extension", "text": "你好" }  "#;
    let (record, value) = ObservedRecord::parse(NativeProtocol::CodexExec, 1, spaced.to_string())
        .expect("whitespace and escapes are preserved");
    assert_eq!(record.raw_json(), spaced);
    assert_eq!(value["text"], "你好");
}

// ---- protocol sessions ----

#[test]
fn codex_app_server_session_correlates_thread_and_turn_and_declines_approval() {
    let mut session = execution(AgentKind::Codex, NativeProtocol::CodexAppServer);
    let initialize = message(session.initial_input());
    assert_eq!(initialize["method"], "initialize");
    assert_eq!(initialize["id"], 1);
    assert!(session.cancel_message().is_none());

    let replies = replay(&mut session, CODEX_APP_SERVER);
    assert_eq!(replies[0][0], json!({"method": "initialized"}));
    let thread_start = &replies[0][1];
    assert_eq!(thread_start["method"], "thread/start");
    assert_eq!(
        thread_start["params"],
        json!({"cwd": WORKING_DIRECTORY, "approvalPolicy": "never",
            "sandbox": "read-only", "ephemeral": true})
    );
    let turn_start = &replies[1][0];
    assert_eq!(turn_start["method"], "turn/start");
    assert_eq!(turn_start["params"]["threadId"], "synthetic_thread");
    assert_eq!(turn_start["params"]["input"][0]["text"], PROMPT);
    assert!(!serde_json::to_string(turn_start).unwrap().contains('\n'));
    assert_eq!(
        replies[7],
        vec![json!({"id": 9, "result": {"decision": "decline"}})]
    );
    for (index, reply) in replies.iter().enumerate() {
        if ![0, 1, 7].contains(&index) {
            assert!(reply.is_empty(), "unexpected reply after record {index}");
        }
    }
    assert_eq!(session.terminal(), Some(SessionTerminal::Completed));
    assert!(session.is_finished());
}

#[test]
fn codex_app_server_cancel_addresses_the_correlated_turn() {
    let mut session = execution(AgentKind::Codex, NativeProtocol::CodexAppServer);
    for (index, line) in CODEX_APP_SERVER.lines().take(4).enumerate() {
        let (record, value) = parse(NativeProtocol::CodexAppServer, index as u64 + 1, line);
        session.receive(&value, record.kind()).expect("prefix");
    }
    assert_eq!(
        session.cancel_message(),
        Some(json!({"id": 4, "method": "turn/interrupt",
            "params": {"threadId": "synthetic_thread", "turnId": "synthetic_turn"}}))
    );
    let interrupt_ack = json!({"id": 4, "result": {}});
    assert_eq!(feed(&mut session, interrupt_ack), Ok(vec![]));
    let interrupted = json!({"method": "turn/completed", "params": {"threadId": "synthetic_thread",
        "turn": {"id": "synthetic_turn", "status": "interrupted"}}});
    assert_eq!(feed(&mut session, interrupted), Ok(vec![]));
    assert_eq!(session.terminal(), Some(SessionTerminal::Cancelled));
}

#[test]
fn codex_app_server_model_travels_in_thread_params() {
    let mut routed = route(AgentKind::Codex, NativeProtocol::CodexAppServer);
    routed.model = Some("synthetic-model".to_string());
    let mut session =
        ProtocolSession::for_execution(&routed, PROMPT.to_string(), WORKING_DIRECTORY.into())
            .unwrap();
    let replies = feed(
        &mut session,
        json!({"id": 1, "result": {"userAgent": "agent"}}),
    )
    .unwrap();
    assert_eq!(replies[1]["params"]["model"], "synthetic-model");
}

#[test]
fn acp_session_offers_no_client_capabilities_and_cancels_permission_requests() {
    let mut session = execution(AgentKind::OpenCode, NativeProtocol::Acp);
    let initialize = message(session.initial_input());
    assert_eq!(initialize["params"]["protocolVersion"], 1);
    assert_eq!(
        initialize["params"]["clientCapabilities"],
        json!({"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false})
    );
    let replies = replay(&mut session, ACP_STREAM);
    assert_eq!(
        replies[0],
        vec![json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
            "params": {"cwd": WORKING_DIRECTORY, "mcpServers": []}})]
    );
    assert_eq!(replies[1][0]["method"], "session/prompt");
    assert_eq!(replies[1][0]["params"]["sessionId"], "synthetic_session");
    assert_eq!(replies[1][0]["params"]["prompt"][0]["text"], PROMPT);
    assert_eq!(
        replies[6],
        vec![json!({"jsonrpc": "2.0", "id": 8,
            "result": {"outcome": {"outcome": "cancelled"}}})]
    );
    assert_eq!(session.terminal(), Some(SessionTerminal::Completed));
}

#[test]
fn claude_session_initializes_then_sends_one_user_message() {
    let mut session = execution(AgentKind::Claude, NativeProtocol::ClaudeStreamJson);
    let initialize = message(session.initial_input());
    assert_eq!(initialize["request"]["subtype"], "initialize");
    assert_eq!(initialize["request_id"], "vibemux_initialize");
    let replies = replay(&mut session, CLAUDE_STREAM);
    assert_eq!(replies[0][0]["type"], "user");
    assert_eq!(replies[0][0]["message"]["content"], PROMPT);
    assert_eq!(replies[6][0]["type"], "control_response");
    assert_eq!(replies[6][0]["response"]["subtype"], "error");
    assert_eq!(replies[6][0]["response"]["request_id"], "permission_one");
    assert_eq!(session.terminal(), Some(SessionTerminal::Completed));
    assert_eq!(
        session.cancel_message().unwrap()["request"]["subtype"],
        "interrupt"
    );
}

#[test]
fn codex_exec_session_writes_the_prompt_to_stdin_and_rejects_a_second_terminal() {
    let mut session = execution(AgentKind::Codex, NativeProtocol::CodexExec);
    assert_eq!(
        session.initial_input(),
        InitialInput::PromptThenClose(PROMPT.to_string())
    );
    assert!(!format!("{:?}", session.initial_input()).contains("synthetic prompt"));
    assert!(session.cancel_message().is_none());
    let replies = replay(&mut session, CODEX_EXEC);
    assert!(replies.iter().all(Vec::is_empty));
    assert_eq!(session.terminal(), Some(SessionTerminal::Completed));
    assert_eq!(
        feed(&mut session, json!({"type": "turn.completed"})),
        Err(DispatchError::UnexpectedTerminal)
    );
}

#[test]
fn probes_stop_after_initialize_without_a_session_or_prompt() {
    let mut app_server = probe(AgentKind::Codex, NativeProtocol::CodexAppServer);
    assert_eq!(
        feed(
            &mut app_server,
            json!({"id": 1, "result": {"userAgent": "agent"}})
        ),
        Ok(vec![json!({"method": "initialized"})])
    );
    assert_eq!(app_server.terminal(), Some(SessionTerminal::Probed));

    let mut acp = probe(AgentKind::Grok, NativeProtocol::Acp);
    assert_eq!(
        feed(
            &mut acp,
            json!({"jsonrpc": "2.0", "id": 1, "result": {"protocolVersion": 1}})
        ),
        Ok(vec![])
    );
    assert_eq!(acp.terminal(), Some(SessionTerminal::Probed));

    let mut claude = probe(AgentKind::Claude, NativeProtocol::ClaudeStreamJson);
    let first = CLAUDE_STREAM.lines().next().unwrap();
    assert_eq!(
        feed(&mut claude, serde_json::from_str(first).unwrap()),
        Ok(vec![])
    );
    assert_eq!(claude.terminal(), Some(SessionTerminal::Probed));

    assert_eq!(
        ProtocolSession::for_probe(
            &route(AgentKind::Codex, NativeProtocol::CodexExec),
            WORKING_DIRECTORY.to_string()
        )
        .unwrap_err(),
        DispatchError::ProbeUnsupported
    );
}

#[test]
fn uncorrelated_or_malformed_protocol_evidence_is_rejected() {
    let started = |session: &mut ProtocolSession| {
        for (index, line) in CODEX_APP_SERVER.lines().take(4).enumerate() {
            let (record, value) = parse(NativeProtocol::CodexAppServer, index as u64 + 1, line);
            session.receive(&value, record.kind()).expect("prefix");
        }
    };
    let cases: Vec<(NativeProtocol, AgentKind, usize, Value, DispatchError)> = vec![
        (
            NativeProtocol::CodexAppServer,
            AgentKind::Codex,
            0,
            json!({"method": "turn/completed", "params": {"threadId": "t",
                "turn": {"id": "u", "status": "completed"}}}),
            DispatchError::UncorrelatedSession,
        ),
        (
            NativeProtocol::CodexAppServer,
            AgentKind::Codex,
            0,
            json!({"id": 2, "result": {}}),
            DispatchError::UncorrelatedResponse,
        ),
        (
            NativeProtocol::CodexAppServer,
            AgentKind::Codex,
            0,
            json!({"id": 1, "error": {"code": 1}}),
            DispatchError::RpcError,
        ),
        (
            NativeProtocol::CodexAppServer,
            AgentKind::Codex,
            0,
            json!({"id": 1, "result": {}}),
            DispatchError::InvalidInitialize,
        ),
        (
            NativeProtocol::CodexAppServer,
            AgentKind::Codex,
            0,
            json!({"id": {"nested": 1}, "method": "item/tool/call"}),
            DispatchError::InvalidRpcId,
        ),
        (
            NativeProtocol::CodexAppServer,
            AgentKind::Codex,
            0,
            json!({"result": {}}),
            DispatchError::UncorrelatedResponse,
        ),
        (
            NativeProtocol::CodexAppServer,
            AgentKind::Codex,
            4,
            json!({"method": "turn/completed", "params": {"threadId": "synthetic_thread",
                "turn": {"id": "other_turn", "status": "completed"}}}),
            DispatchError::UncorrelatedTurn,
        ),
        (
            NativeProtocol::CodexAppServer,
            AgentKind::Codex,
            4,
            json!({"method": "turn/completed", "params": {"threadId": "synthetic_thread",
                "turn": {"id": "synthetic_turn", "status": "inProgress"}}}),
            DispatchError::InvalidTerminal,
        ),
        (
            NativeProtocol::Acp,
            AgentKind::Copilot,
            0,
            json!({"id": 1, "result": {"protocolVersion": 1}}),
            DispatchError::InvalidJsonRpc,
        ),
        (
            NativeProtocol::Acp,
            AgentKind::Copilot,
            0,
            json!({"jsonrpc": "2.0", "id": 1, "result": {"protocolVersion": 2}}),
            DispatchError::UnsupportedProtocolVersion,
        ),
        (
            NativeProtocol::Acp,
            AgentKind::Copilot,
            0,
            json!({"jsonrpc": "2.0", "method": "session/update",
                "params": {"sessionId": "synthetic_session"}}),
            DispatchError::UncorrelatedSession,
        ),
        (
            NativeProtocol::ClaudeStreamJson,
            AgentKind::Claude,
            0,
            json!({"type": "result", "subtype": "success"}),
            DispatchError::UnexpectedTerminal,
        ),
        (
            NativeProtocol::ClaudeStreamJson,
            AgentKind::Claude,
            0,
            json!({"type": "control_response",
                "response": {"subtype": "success", "request_id": "other"}}),
            DispatchError::InvalidInitialize,
        ),
    ];
    for (protocol, harness, prefix, value, expected) in cases {
        let mut session = execution(harness, protocol);
        if prefix > 0 {
            started(&mut session);
        }
        assert_eq!(feed(&mut session, value), Err(expected), "{protocol:?}");
        assert_eq!(session.terminal(), None);
    }
}

#[test]
fn acp_prompt_result_needs_a_known_stop_reason_and_nothing_follows_a_finish() {
    let mut session = execution(AgentKind::Grok, NativeProtocol::Acp);
    for line in ACP_STREAM.lines().take(2) {
        feed(&mut session, serde_json::from_str(line).unwrap()).unwrap();
    }
    let mut unknown = execution(AgentKind::Grok, NativeProtocol::Acp);
    for line in ACP_STREAM.lines().take(2) {
        feed(&mut unknown, serde_json::from_str(line).unwrap()).unwrap();
    }
    assert_eq!(
        feed(
            &mut unknown,
            json!({"jsonrpc": "2.0", "id": 3, "result": {"stopReason": "future"}})
        ),
        Err(DispatchError::InvalidTerminal)
    );
    feed(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 3, "result": {"stopReason": "end_turn"}}),
    )
    .unwrap();
    assert_eq!(session.terminal(), Some(SessionTerminal::Completed));
    assert_eq!(
        feed(
            &mut session,
            json!({"jsonrpc": "2.0", "id": 3, "result": {"stopReason": "end_turn"}})
        ),
        Err(DispatchError::UnexpectedResponse)
    );
    assert_eq!(
        session.cancel_message(),
        Some(json!({"jsonrpc": "2.0", "method": "session/cancel",
            "params": {"sessionId": "synthetic_session"}}))
    );
}

#[test]
fn session_construction_validates_inputs_and_debug_is_content_free() {
    let routed = route(AgentKind::Claude, NativeProtocol::ClaudeStreamJson);
    assert_eq!(
        ProtocolSession::for_execution(&routed, "  ".into(), WORKING_DIRECTORY.into()).unwrap_err(),
        DispatchError::InvalidPrompt
    );
    assert_eq!(
        ProtocolSession::for_execution(&routed, PROMPT.into(), String::new()).unwrap_err(),
        DispatchError::WorkingDirectoryInvalid
    );
    assert_eq!(
        ProtocolSession::for_execution(
            &route(AgentKind::Qwen, NativeProtocol::Acp),
            PROMPT.into(),
            WORKING_DIRECTORY.into()
        )
        .unwrap_err(),
        DispatchError::ConfigRouteInvalid
    );
    let session = execution(AgentKind::Claude, NativeProtocol::ClaudeStreamJson);
    let debug = format!("{session:?}");
    assert!(!debug.contains("synthetic prompt"));
    assert!(!debug.contains(WORKING_DIRECTORY));
}
