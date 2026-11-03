use super::*;
use serde_json::{Value, json};
use uuid::Uuid;

mod approval_tests;
mod claude_tests;
mod command_tool_tests;
mod file_tool_tests;
mod invariant_tests;
mod policy_tests;
mod property_tests;

const CLAUDE_SESSION: Uuid = Uuid::from_u128(500);

fn request(ordinal: u128) -> TurnRequest {
    TurnRequest {
        request_id: Uuid::from_u128(100 + ordinal),
        turn_id: Uuid::from_u128(200 + ordinal),
        prompt: format!("synthetic turn {ordinal}"),
    }
}

fn claude_ready(resume: bool) -> SessionMachine {
    let mut machine = SessionMachine::new(
        config(SessionTarget::Claude {
            session_id: CLAUDE_SESSION,
            resume,
        }),
        SessionLimits::default(),
    )
    .expect("Claude machine");
    let initialize = write(&machine.start().expect("initialize"));
    let response = feed(&mut machine, json!({"type":"control_response","response":{"subtype":"success","request_id":initialize["request_id"],"response":{"claude_code_version":CLAUDE_HELP_VERSION}}})).expect("control initialized");
    assert!(
        response
            .events
            .iter()
            .all(|event| !matches!(event, SessionEvent::SessionBound { .. }))
    );
    assert_eq!(machine.snapshot().phase, SessionPhase::Ready);
    assert!(!machine.snapshot().vendor_session_confirmed);
    machine
}

fn claude_init() -> Value {
    json!({"type":"system","subtype":"init","uuid":Uuid::from_u128(999),"session_id":CLAUDE_SESSION,
        "cwd":config(SessionTarget::Claude { session_id:CLAUDE_SESSION,resume:false }).working_directory,
        "tools":[],"mcp_servers":[],"permissionMode":"dontAsk","plugins":[]})
}

fn claude_result(ordinal: u128, outcome: &str, text: &str) -> Value {
    json!({"type":"result","uuid":Uuid::from_u128(1000+ordinal),"session_id":CLAUDE_SESSION,
        "user_message_uuid":request(ordinal).request_id,"user_message_uuids":[request(ordinal).request_id],
        "subtype":"success","is_error":false,"terminal_reason":outcome,"result":text,"result_index":ordinal-1})
}

fn codex_turn(machine: &mut SessionMachine, ordinal: u128, vendor_turn: &str) -> Value {
    let input = write(&machine.submit(request(ordinal)).expect("submit"));
    feed(machine, json!({"id":input["id"],"result":{"turn":{"id":vendor_turn,"status":"inProgress","items":[]}}})).expect("acknowledge turn");
    input
}

fn codex_completed(vendor_turn: &str, status: &str) -> Value {
    json!({"method":"turn/completed","params":{"threadId":"root_thread","turn":{"id":vendor_turn,"status":status,"items":[]}}})
}

fn codex_opening(target: SessionTarget, limits: SessionLimits) -> (SessionMachine, Value) {
    let mut machine = SessionMachine::new(config(target), limits).expect("machine");
    let initialize = write(&machine.start().expect("initialize"));
    let opening = feed(
        &mut machine,
        json!({"id":initialize["id"],"result":{"userAgent":"codex/0.166.0"}}),
    )
    .expect("initialized");
    (machine, write(&opening))
}

fn codex_open_response(rpc_id: &Value, thread_id: &str) -> Value {
    json!({"id":rpc_id,"result":{
        "thread":{"id":thread_id,"sessionId":thread_id,"ephemeral":false,"status":{"type":"idle"},"turns":[]},
        "approvalPolicy":"never","sandbox":{"type":"readOnly","networkAccess":false},
        "cwd":config(SessionTarget::Codex{resume_thread_id:None}).working_directory,"model":"fixture_model"
    }})
}

#[test]
fn codex_resume_status_before_response_preserves_handshake_authority() {
    let (mut machine, open) = codex_opening(
        SessionTarget::Codex {
            resume_thread_id: Some("root_thread".into()),
        },
        SessionLimits::default(),
    );
    let status = json!({"method":"thread/status/changed","params":{"threadId":"root_thread","status":{"type":"idle"}}});
    let observed = feed(&mut machine, status).expect("matching early status");
    assert!(observed.effects.is_empty());
    assert!(
        observed
            .events
            .iter()
            .all(|event| !matches!(event, SessionEvent::SessionBound { .. }))
    );
    assert_eq!(machine.snapshot().phase, SessionPhase::Opening);
    assert!(!machine.snapshot().vendor_session_confirmed);
    feed(
        &mut machine,
        codex_open_response(&open["id"], "root_thread"),
    )
    .expect("qualified response");
    assert_eq!(machine.snapshot().phase, SessionPhase::Ready);
    assert!(machine.snapshot().vendor_session_confirmed);

    let (mut wrong, _) = codex_opening(
        SessionTarget::Codex {
            resume_thread_id: Some("root_thread".into()),
        },
        SessionLimits::default(),
    );
    assert_eq!(
        feed(
            &mut wrong,
            json!({"method":"thread/status/changed","params":{"threadId":"other_thread","status":{"type":"idle"}}})
        ),
        Err(SessionError::UncorrelatedSession)
    );
}

fn codex_with_limits(limits: SessionLimits) -> SessionMachine {
    let (mut machine, open) = codex_opening(
        SessionTarget::Codex {
            resume_thread_id: None,
        },
        limits,
    );
    feed(
        &mut machine,
        codex_open_response(&open["id"], "root_thread"),
    )
    .expect("opened");
    machine
}

fn config(target: SessionTarget) -> SessionConfig {
    SessionConfig {
        target,
        generation_id: Uuid::from_u128(1),
        working_directory: if cfg!(windows) {
            "C:\\synthetic_project".into()
        } else {
            "/synthetic_project".into()
        },
        model: None,
    }
}

fn write(transition: &SessionTransition) -> Value {
    let SessionEffect::Write(frame) = transition.effects.last().expect("write") else {
        panic!("expected protocol write")
    };
    serde_json::from_slice(frame.as_bytes()).expect("bounded JSON")
}

fn feed(machine: &mut SessionMachine, value: Value) -> Result<SessionTransition, SessionError> {
    machine.receive(
        machine.snapshot().records_received + 1,
        value.to_string().as_bytes(),
    )
}

fn codex_ready() -> SessionMachine {
    let config = config(SessionTarget::Codex {
        resume_thread_id: None,
    });
    let cwd = config.working_directory.clone();
    let mut machine = SessionMachine::new(config, SessionLimits::default()).expect("machine");
    let initialize = write(&machine.start().expect("initialize"));
    let opening = feed(
        &mut machine,
        json!({"id":initialize["id"],"result":{"userAgent":"codex/0.166.0"}}),
    )
    .expect("initialized");
    let open = write(&opening);
    assert_eq!(open["method"], "thread/start");
    assert_eq!(open["params"]["sandbox"], "read-only");
    assert_eq!(open["params"]["approvalPolicy"], "never");
    assert_eq!(open["params"]["ephemeral"], false);
    feed(&mut machine, json!({"id":open["id"],"result":{
        "thread":{"id":"root_thread","sessionId":"root_thread","ephemeral":false,"status":{"type":"idle"},"turns":[]},
        "approvalPolicy":"never","sandbox":{"type":"readOnly","networkAccess":false},"cwd":cwd,"model":"fixture_model"
    }})).expect("opened");
    assert_eq!(machine.snapshot().phase, SessionPhase::Ready);
    assert!(machine.snapshot().vendor_session_confirmed);
    machine
}

#[test]
fn two_codex_turns_share_the_exact_thread_and_finish_without_process_exit() {
    let mut machine = codex_ready();
    let mut rpc_ids = Vec::new();
    for (ordinal, vendor_turn) in [(1, "vendor_turn_1"), (2, "vendor_turn_2")] {
        let request = TurnRequest {
            request_id: Uuid::from_u128(100 + ordinal),
            turn_id: Uuid::from_u128(200 + ordinal),
            prompt: format!("synthetic turn {ordinal}"),
        };
        let submitted = machine.submit(request).expect("serial turn");
        let input = write(&submitted);
        rpc_ids.push(input["id"].clone());
        assert_eq!(input["method"], "turn/start");
        assert_eq!(input["params"]["threadId"], "root_thread");
        feed(&mut machine, json!({"id":input["id"],"result":{"turn":{"id":vendor_turn,"status":"inProgress","items":[]}}})).expect("turn acknowledged");
        feed(&mut machine, json!({"method":"item/agentMessage/delta","params":{"threadId":"root_thread","turnId":vendor_turn,"itemId":"item","delta":"hello"}})).expect("text");
        let terminal = feed(&mut machine, json!({"method":"turn/completed","params":{"threadId":"root_thread","turn":{"id":vendor_turn,"status":"completed","items":[]}}})).expect("terminal");
        assert!(terminal.events.iter().any(|event| matches!(event, SessionEvent::TurnFinished { turn } if turn.outcome == Some(TurnOutcome::Completed))));
        assert!(
            terminal.effects.is_empty(),
            "turn completion must leave stdin/process alive"
        );
        assert_eq!(machine.snapshot().phase, SessionPhase::Ready);
        assert_eq!(
            machine.snapshot().vendor_session_id.as_deref(),
            Some("root_thread")
        );
    }
    assert_ne!(rpc_ids[0], rpc_ids[1]);
    assert_eq!(machine.snapshot().turns_started, 2);
}
