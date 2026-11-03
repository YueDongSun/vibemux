use super::*;
use vibemux_types::{ToolName, chat_approval::ChatApprovalDecision};

fn fixture(limits: SessionLimits) -> SessionMachine {
    let mut machine = SessionMachine::new_fixture_file_approval(
        config(SessionTarget::Claude {
            session_id: CLAUDE_SESSION,
            resume: false,
        }),
        limits,
    )
    .expect("fixture machine");
    let initialize = write(&machine.start().expect("start"));
    feed(&mut machine, json!({"type":"control_response","response":{"subtype":"success",
        "request_id":initialize["request_id"],"response":{"claude_code_version":CLAUDE_HELP_VERSION}}}))
        .expect("initialized");
    machine
}

fn fixture_init() -> Value {
    let mut value = claude_init();
    value["permissionMode"] = json!("manual");
    value["tools"] = json!(["Write"]);
    value
}

fn private_input() -> Value {
    json!({"file_path":"private_fixture_candidate","content":"private fixture content"})
}

fn tool(tool_id: &str, ordinal: u128) -> Value {
    json!({"type":"assistant","uuid":Uuid::from_u128(3000+ordinal),
        "session_id":CLAUDE_SESSION,"parent_tool_use_id":null,
        "message":{"id":format!("private_message_{ordinal}"),"role":"assistant","content":[{
            "type":"tool_use","id":tool_id,"name":"Write","input":private_input()}]}})
}

fn permission(request_id: &str, tool_id: &str) -> Value {
    json!({"type":"control_request","request_id":request_id,"request":{
        "subtype":"can_use_tool","tool_name":"Write","tool_use_id":tool_id,"input":private_input()}})
}

fn submit_fixture(machine: &mut SessionMachine) {
    let submitted = machine.submit(request(1)).expect("one input");
    assert!(
        matches!(&submitted.effects[0], SessionEffect::WriteTurn { request_id, turn_id, .. }
        if *request_id == request(1).request_id && *turn_id == request(1).turn_id)
    );
    assert!(submitted.approvals.is_empty());
}

fn challenge(machine: &mut SessionMachine) -> ApprovalChallenge {
    submit_fixture(machine);
    machine
        .mark_input_written(request(1).request_id, request(1).turn_id)
        .expect("input flush");
    feed(machine, fixture_init()).expect("manual init");
    feed(machine, tool("private_tool", 1)).expect("complete tool");
    let transition = feed(machine, permission("private_rpc", "private_tool")).expect("permission");
    assert_eq!(transition.approvals.len(), 1);
    transition.approvals[0].clone()
}

fn wire(frame: &WireFrame) -> Value {
    serde_json::from_slice(frame.as_bytes()).expect("response JSON")
}

#[test]
fn fixture_constructor_is_claude_fresh_only_and_default_stays_read_only() {
    assert!(
        SessionMachine::new_fixture_file_approval(
            config(SessionTarget::Claude {
                session_id: CLAUDE_SESSION,
                resume: true
            }),
            SessionLimits::default()
        )
        .is_err()
    );
    assert!(
        SessionMachine::new_fixture_file_approval(
            config(SessionTarget::Codex {
                resume_thread_id: None
            }),
            SessionLimits::default()
        )
        .is_err()
    );
    let mut normal = claude_ready(false);
    assert!(!normal.fixture_file_approval_enabled());
    assert!(matches!(
        &normal.submit(request(1)).expect("submit").effects[0],
        SessionEffect::Write(_)
    ));
    assert_eq!(
        feed(&mut normal, fixture_init()),
        Err(SessionError::UnsafeProfile)
    );
    let mut normal = claude_ready(false);
    normal.submit(request(1)).expect("submit");
    feed(&mut normal, claude_init()).expect("read-only init");
    assert_eq!(
        feed(&mut normal, tool("private_tool", 1)),
        Err(SessionError::MissingCorrelation)
    );
}

#[test]
fn root_init_and_exact_input_flush_are_both_required_before_an_offer() {
    let mut machine = fixture(SessionLimits::default());
    submit_fixture(&mut machine);
    assert!(
        feed(&mut machine, permission("private_rpc", "private_tool"))
            .expect("early request")
            .approvals
            .is_empty()
    );
    assert!(
        feed(&mut machine, tool("private_tool", 1))
            .expect("early evidence")
            .approvals
            .is_empty()
    );
    assert!(
        !machine
            .snapshot()
            .active_turn
            .expect("prepared")
            .acknowledged
    );
    assert_eq!(
        machine.mark_input_written(request(1).request_id, Uuid::from_u128(42)),
        Err(SessionError::UncorrelatedTurn)
    );
    assert!(
        machine
            .mark_input_written(request(1).request_id, request(1).turn_id)
            .expect("flush")
            .approvals
            .is_empty()
    );
    let qualified = feed(&mut machine, fixture_init()).expect("init");
    assert_eq!(qualified.approvals.len(), 1);
    assert!(
        machine
            .snapshot()
            .active_turn
            .expect("acknowledged")
            .acknowledged
    );
    assert_eq!(
        machine.mark_input_written(request(1).request_id, request(1).turn_id),
        Err(SessionError::DuplicateRequest)
    );
    assert!(
        feed(&mut machine, permission("private_rpc", "private_tool"))
            .expect("exact duplicate")
            .approvals
            .is_empty()
    );
}

#[test]
fn allow_and_deny_use_exact_original_fields_once() {
    for decision in [ChatApprovalDecision::AllowOnce, ChatApprovalDecision::Deny] {
        let mut machine = fixture(SessionLimits::default());
        let challenge = challenge(&mut machine);
        assert_eq!(challenge.tool, ToolName::WriteFiles);
        assert_eq!(challenge.vendor_tool_name, "Write");
        let response = wire(
            &machine
                .respond_to_approval(&challenge, decision)
                .expect("one response"),
        );
        assert_eq!(response["response"]["request_id"], "private_rpc");
        assert_eq!(response["response"]["subtype"], "success");
        let body = &response["response"]["response"];
        assert!(body.get("updatedPermissions").is_none());
        match decision {
            ChatApprovalDecision::AllowOnce => {
                assert_eq!(body["behavior"], "allow");
                assert_eq!(body["updatedInput"], private_input());
            }
            ChatApprovalDecision::Deny => {
                assert_eq!(body["behavior"], "deny");
                assert!(body.get("updatedInput").is_none());
            }
        }
        assert_eq!(
            machine.respond_to_approval(&challenge, decision),
            Err(SessionError::DuplicateRequest)
        );
        machine
            .mark_approval_written(challenge.challenge_id)
            .expect("written");
        assert_eq!(
            machine.respond_to_approval(&challenge, decision),
            Err(SessionError::UncorrelatedResponse)
        );
        assert_eq!(
            machine.mark_approval_written(challenge.challenge_id),
            Err(SessionError::UncorrelatedResponse)
        );
    }
}

#[test]
fn every_private_challenge_field_is_part_of_response_identity() {
    let mut machine = fixture(SessionLimits::default());
    let challenge = challenge(&mut machine);
    let mut changes = Vec::new();
    let mut changed = challenge.clone();
    changed.input["content"] = json!("changed");
    changes.push(changed);
    let mut changed = challenge.clone();
    changed.vendor_session_id = "foreign".into();
    changes.push(changed);
    let mut changed = challenge.clone();
    changed.vendor_request_id = "foreign".into();
    changes.push(changed);
    let mut changed = challenge.clone();
    changed.tool_use_id = "foreign".into();
    changes.push(changed);
    let mut changed = challenge.clone();
    changed.vendor_tool_name = "Edit".into();
    changes.push(changed);
    let mut changed = challenge.clone();
    changed.tool = ToolName::EditFiles;
    changes.push(changed);
    let mut changed = challenge.clone();
    changed.challenge_id = Uuid::from_u128(9000);
    changes.push(changed);
    for changed in changes {
        assert_eq!(
            machine.respond_to_approval(&changed, ChatApprovalDecision::AllowOnce),
            Err(SessionError::ConflictingRecord)
        );
    }
    let mut changed = challenge.clone();
    changed.request_id = Uuid::from_u128(9001);
    assert_eq!(
        machine.respond_to_approval(&changed, ChatApprovalDecision::AllowOnce),
        Err(SessionError::UncorrelatedTurn)
    );
    let mut changed = challenge.clone();
    changed.turn_id = Uuid::from_u128(9002);
    assert_eq!(
        machine.respond_to_approval(&changed, ChatApprovalDecision::AllowOnce),
        Err(SessionError::UncorrelatedTurn)
    );
    machine
        .respond_to_approval(&challenge, ChatApprovalDecision::Deny)
        .expect("original still available");
}

#[test]
fn cancellation_and_close_refuse_new_responses_without_rewriting_a_queued_decision() {
    for closing in [false, true] {
        let mut machine = fixture(SessionLimits::default());
        let challenge = challenge(&mut machine);
        if closing {
            machine.close().expect("close");
        } else {
            machine.cancel(request(1).turn_id).expect("cancel");
        }
        assert_eq!(
            machine.respond_to_approval(&challenge, ChatApprovalDecision::AllowOnce),
            Err(SessionError::UnexpectedState)
        );
    }
    let mut machine = fixture(SessionLimits::default());
    let challenge = challenge(&mut machine);
    machine
        .respond_to_approval(&challenge, ChatApprovalDecision::AllowOnce)
        .expect("queued allow");
    machine.cancel(request(1).turn_id).expect("later cancel");
    machine
        .mark_approval_written(challenge.challenge_id)
        .expect("settle original response");
}

#[test]
fn terminal_requires_a_decision_and_waits_for_approval_flush() {
    let mut premature = fixture(SessionLimits::default());
    challenge(&mut premature);
    assert_eq!(
        feed(&mut premature, claude_result(1, "completed", "answer")),
        Err(SessionError::InvalidTerminal)
    );
    let mut machine = fixture(SessionLimits::default());
    let challenge = challenge(&mut machine);
    machine
        .respond_to_approval(&challenge, ChatApprovalDecision::AllowOnce)
        .expect("queue");
    let buffered = feed(
        &mut machine,
        claude_result(1, "completed", "final fixture answer"),
    )
    .expect("buffer terminal");
    assert!(buffered.events.is_empty());
    assert!(machine.snapshot().active_turn.is_some());
    let finished = machine
        .mark_approval_written(challenge.challenge_id)
        .expect("flush then terminal");
    assert!(
        finished
            .events
            .iter()
            .any(|event| matches!(event, SessionEvent::TurnFinished { .. }))
    );
    assert!(machine.snapshot().active_turn.is_none());
    assert_eq!(
        machine.submit(request(2)),
        Err(SessionError::TurnBudgetExhausted)
    );
}

#[test]
fn an_aborted_cancelled_turn_needs_no_permission_response() {
    let mut machine = fixture(SessionLimits::default());
    let offered = challenge(&mut machine);
    machine.cancel(request(1).turn_id).expect("cancel");
    let finished = feed(
        &mut machine,
        claude_result(1, "aborted_streaming", "cancelled"),
    )
    .expect("aborted terminal");
    assert!(finished.events.iter().any(|event|matches!(event,SessionEvent::TurnFinished {turn} if turn.outcome==Some(TurnOutcome::Cancelled))));
    assert!(
        machine
            .respond_to_approval(&offered, ChatApprovalDecision::AllowOnce)
            .is_err()
    );
}

#[test]
fn terminal_without_an_echo_is_only_accepted_inside_the_closed_fixture() {
    let mut machine = fixture(SessionLimits::default());
    let challenge = challenge(&mut machine);
    machine
        .respond_to_approval(&challenge, ChatApprovalDecision::Deny)
        .expect("queue deny");
    machine
        .mark_approval_written(challenge.challenge_id)
        .expect("flush");
    let mut terminal = claude_result(1, "completed", "fixture answer");
    terminal
        .as_object_mut()
        .expect("object")
        .remove("user_message_uuid");
    terminal
        .as_object_mut()
        .expect("object")
        .remove("user_message_uuids");
    feed(&mut machine, terminal).expect("closed one-input source proof");
    assert_eq!(
        machine.snapshot().last_turn.expect("finished").outcome,
        Some(TurnOutcome::Completed)
    );
}

#[test]
fn streamed_tool_input_is_complete_and_matches_before_offer() {
    let mut machine = fixture(SessionLimits::default());
    submit_fixture(&mut machine);
    machine
        .mark_input_written(request(1).request_id, request(1).turn_id)
        .expect("flush");
    feed(&mut machine, fixture_init()).expect("init");
    let events = [
        json!({"type":"message_start","message":{"id":"stream_message","role":"assistant"}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"stream_tool","name":"Write","input":{}}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":private_input().to_string()}}),
    ];
    for (index, event) in events.into_iter().enumerate() {
        let frame = json!({"type":"stream_event","uuid":Uuid::from_u128(4000+index as u128),
            "session_id":CLAUDE_SESSION,"parent_tool_use_id":null,"event":event});
        assert!(
            feed(&mut machine, frame)
                .expect("stream")
                .approvals
                .is_empty()
        );
    }
    assert!(
        feed(&mut machine, permission("stream_rpc", "stream_tool"))
            .expect("early request")
            .approvals
            .is_empty()
    );
    let stopped = feed(&mut machine, json!({"type":"stream_event","uuid":Uuid::from_u128(4010),
        "session_id":CLAUDE_SESSION,"parent_tool_use_id":null,"event":{"type":"content_block_stop","index":0}})).expect("complete input");
    assert_eq!(stopped.approvals.len(), 1);
    assert_eq!(stopped.approvals[0].input, private_input());
}

#[test]
fn foreign_nested_historical_and_changed_inputs_never_offer() {
    let mut cases = Vec::new();
    let mut foreign = tool("private_tool", 1);
    foreign["session_id"] = json!(Uuid::from_u128(42));
    cases.push(foreign);
    let mut nested = tool("private_tool", 1);
    nested["parent_agent_id"] = json!("nested");
    cases.push(nested);
    let mut historical = tool("private_tool", 1);
    historical["isReplay"] = json!(true);
    cases.push(historical);
    let mut echo = tool("private_tool", 1);
    echo["user_message_uuid"] = json!(Uuid::from_u128(42));
    cases.push(echo);
    for frame in cases {
        let mut machine = fixture(SessionLimits::default());
        submit_fixture(&mut machine);
        assert!(feed(&mut machine, frame).is_err());
    }
    let mut changed = fixture(SessionLimits::default());
    let challenge = challenge(&mut changed);
    let mut duplicate = permission("private_rpc", "private_tool");
    duplicate["request"]["input"]["content"] = json!("changed");
    assert_eq!(
        feed(&mut changed, duplicate),
        Err(SessionError::ConflictingRecord)
    );
    assert!(
        changed
            .respond_to_approval(&challenge, ChatApprovalDecision::AllowOnce)
            .is_err()
    );
}

#[test]
fn sequential_challenges_require_previous_flush_and_retired_ids_cannot_authorize_again() {
    let mut machine = fixture(SessionLimits::default());
    let first = challenge(&mut machine);
    machine
        .respond_to_approval(&first, ChatApprovalDecision::Deny)
        .expect("deny");
    machine
        .mark_approval_written(first.challenge_id)
        .expect("flush");
    assert!(
        feed(&mut machine, permission("private_rpc", "private_tool"))
            .expect("retired duplicate")
            .approvals
            .is_empty()
    );
    feed(&mut machine, tool("second_tool", 2)).expect("second evidence");
    let second = feed(&mut machine, permission("second_rpc", "second_tool"))
        .expect("second request")
        .approvals
        .remove(0);
    assert_ne!(first.challenge_id, second.challenge_id);
    assert_eq!(
        machine.respond_to_approval(&first, ChatApprovalDecision::AllowOnce),
        Err(SessionError::ConflictingRecord)
    );
    machine
        .respond_to_approval(&second, ChatApprovalDecision::Deny)
        .expect("second deny");
}

#[test]
fn buffers_are_bounded_and_debug_does_not_disclose_the_private_challenge() {
    let limits = SessionLimits {
        max_pending_frames: 1,
        ..SessionLimits::default()
    };
    let mut bounded = fixture(limits);
    submit_fixture(&mut bounded);
    feed(&mut bounded, fixture_init()).expect("first retained record");
    assert_eq!(
        feed(&mut bounded, tool("private_tool", 1)),
        Err(SessionError::PendingOverflow)
    );
    let mut machine = fixture(SessionLimits::default());
    let challenge = challenge(&mut machine);
    let debug = format!("{challenge:?} {machine:?}");
    for private in [
        "private_fixture_candidate",
        "private fixture content",
        "private_tool",
        "private_rpc",
        "private_message",
    ] {
        assert!(!debug.contains(private));
    }
    let snapshot = serde_json::to_string(&machine.snapshot()).expect("public snapshot");
    assert!(!snapshot.contains("private fixture content"));
}

#[test]
fn changed_effective_profiles_oversized_input_and_retired_tool_ids_fail_closed() {
    let mut unsafe_profiles = Vec::new();
    let mut profile = fixture_init();
    profile["mcp_servers"] = json!([{"name":"unexpected"}]);
    unsafe_profiles.push(profile);
    let mut profile = fixture_init();
    profile["plugins"] = json!([{"name":"unexpected"}]);
    unsafe_profiles.push(profile);
    let mut profile = fixture_init();
    profile["tools"] = json!(["Write", "Edit"]);
    unsafe_profiles.push(profile);
    for profile in unsafe_profiles {
        let mut machine = fixture(SessionLimits::default());
        submit_fixture(&mut machine);
        assert_eq!(
            feed(&mut machine, profile),
            Err(SessionError::UnsafeProfile)
        );
    }
    let mut oversized = fixture(SessionLimits::default());
    submit_fixture(&mut oversized);
    let mut frame = tool("private_tool", 1);
    frame["message"]["content"][0]["input"]["content"] = json!("x".repeat(256 * 1024));
    assert_eq!(
        feed(&mut oversized, frame),
        Err(SessionError::PendingOverflow)
    );
    let mut retired = fixture(SessionLimits::default());
    let first = challenge(&mut retired);
    retired
        .respond_to_approval(&first, ChatApprovalDecision::Deny)
        .expect("deny");
    retired
        .mark_approval_written(first.challenge_id)
        .expect("flush");
    assert_eq!(
        feed(&mut retired, permission("new_rpc", "private_tool")),
        Err(SessionError::UncorrelatedTurn)
    );
}
