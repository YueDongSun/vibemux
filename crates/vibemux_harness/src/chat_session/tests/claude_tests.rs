use super::*;

fn input_ack(ordinal: u128) -> Value {
    json!({"type":"user","uuid":request(ordinal).request_id,"session_id":CLAUDE_SESSION,
        "isReplay":true,"parent_tool_use_id":null,"message":{"role":"user","content":format!("synthetic turn {ordinal}")}})
}

#[test]
fn root_init_proof_is_required_before_text_or_a_successful_terminal() {
    for kind in ["assistant", "stream_event", "result"] {
        let mut machine = claude_ready(false);
        let input = machine
            .submit(request(1))
            .expect("first input is still permitted while transport-ready");
        assert_eq!(input.effects.len(), 1);
        let reply = match kind {
            "assistant" => {
                json!({"type":"assistant","uuid":Uuid::from_u128(9001),"session_id":CLAUDE_SESSION,
                "user_message_uuid":request(1).request_id,"parent_tool_use_id":null,
                "message":{"id":"message_1","role":"assistant","content":[{"type":"text","text":"unqualified text"}]}})
            }
            "stream_event" => {
                json!({"type":"stream_event","uuid":Uuid::from_u128(9002),"session_id":CLAUDE_SESSION,
                "user_message_uuid":request(1).request_id,"parent_tool_use_id":null,
                "event":{"type":"message_start","message":{"id":"message_1","role":"assistant"}}})
            }
            "result" => claude_result(1, "completed", "unqualified success"),
            _ => unreachable!(),
        };
        assert_eq!(
            feed(&mut machine, reply),
            Err(SessionError::UnsafeProfile),
            "{kind}"
        );
        let snapshot = machine.snapshot();
        assert!(
            !snapshot.vendor_session_confirmed,
            "a reply cannot replace root initialization proof"
        );
        assert!(snapshot.last_turn.is_none());
        assert_eq!(
            snapshot
                .active_turn
                .expect("uncertain ownership retained")
                .outcome,
            None
        );
        assert_eq!(snapshot.phase, SessionPhase::RecoveryRequired);
    }
}

#[test]
fn an_input_ack_before_root_init_waits_for_profile_proof() {
    let mut machine = claude_ready(false);
    machine.submit(request(1)).expect("submit");
    let deferred = feed(&mut machine, input_ack(1)).expect("defer early acknowledgment");
    assert!(deferred.events.is_empty());
    assert!(deferred.text.is_empty());
    assert!(!machine.snapshot().vendor_session_confirmed);
    assert!(!machine.snapshot().active_turn.expect("active").acknowledged);
    let initialized = feed(&mut machine, claude_init()).expect("validated root init");
    assert!(
        initialized
            .events
            .iter()
            .any(|event| matches!(event, SessionEvent::SessionBound { .. }))
    );
    assert!(initialized.events.iter().any(|event| matches!(event, SessionEvent::TurnAcknowledged { turn } if turn.request_id == request(1).request_id)));
    assert!(machine.snapshot().vendor_session_confirmed);
    feed(
        &mut machine,
        claude_result(1, "completed", "qualified answer"),
    )
    .expect("qualified terminal");
    assert_eq!(
        machine.snapshot().last_turn.expect("last").outcome,
        Some(TurnOutcome::Completed)
    );
}

#[test]
fn late_or_nested_init_cannot_retroactively_qualify_a_result() {
    let mut late = claude_ready(false);
    late.submit(request(1)).expect("submit");
    assert_eq!(
        feed(&mut late, claude_result(1, "completed", "too early")),
        Err(SessionError::UnsafeProfile)
    );
    assert!(feed(&mut late, claude_init()).is_err());
    assert!(!late.snapshot().vendor_session_confirmed);
    assert!(late.snapshot().last_turn.is_none());

    let mut nested = claude_ready(false);
    nested.submit(request(1)).expect("submit");
    let mut initialization = claude_init();
    initialization["parent_tool_use_id"] = json!("subagent_tool");
    let ignored = feed(&mut nested, initialization).expect("nested init ignored");
    assert!(ignored.events.contains(&SessionEvent::RecordIgnored {
        reason: IgnoredReason::Nested
    }));
    assert_eq!(
        feed(&mut nested, claude_result(1, "completed", "no root proof")),
        Err(SessionError::UnsafeProfile)
    );
    assert!(!nested.snapshot().vendor_session_confirmed);
}

#[test]
fn root_init_before_control_reply_is_retained_without_granting_transport_readiness() {
    let mut machine = SessionMachine::new(
        config(SessionTarget::Claude {
            session_id: CLAUDE_SESSION,
            resume: false,
        }),
        SessionLimits::default(),
    )
    .expect("machine");
    let initialize = write(&machine.start().expect("initialize"));
    let early = feed(&mut machine, claude_init()).expect("validate reordered root init");
    assert!(early.events.is_empty());
    assert_eq!(machine.snapshot().phase, SessionPhase::Initializing);
    assert!(!machine.snapshot().vendor_session_confirmed);
    assert_eq!(
        machine.submit(request(1)),
        Err(SessionError::UnexpectedState)
    );
    let ready = feed(&mut machine, json!({"type":"control_response","response":{"subtype":"success","request_id":initialize["request_id"]}})).expect("correlated control reply");
    assert!(
        ready
            .events
            .iter()
            .any(|event| matches!(event, SessionEvent::SessionBound { .. }))
    );
    assert_eq!(machine.snapshot().phase, SessionPhase::Ready);
    assert!(machine.snapshot().vendor_session_confirmed);
    machine.submit(request(1)).expect("first explicit input");
    feed(
        &mut machine,
        claude_result(1, "completed", "qualified answer"),
    )
    .expect("qualified result");
}

#[test]
fn an_early_ack_cannot_bypass_invalid_effective_profile_fields() {
    for field in [
        "tools",
        "mcp_servers",
        "plugins",
        "permissionMode",
        "cwd",
        "model",
    ] {
        let mut settings = config(SessionTarget::Claude {
            session_id: CLAUDE_SESSION,
            resume: false,
        });
        settings.model = Some("pinned_model".into());
        let mut machine = SessionMachine::new(settings, SessionLimits::default()).expect("machine");
        let initialize = write(&machine.start().expect("initialize"));
        feed(&mut machine, json!({"type":"control_response","response":{"subtype":"success","request_id":initialize["request_id"]}})).expect("transport ready");
        machine.submit(request(1)).expect("explicit input");
        feed(&mut machine, input_ack(1)).expect("deferred ack");
        let mut initialization = claude_init();
        initialization["model"] = json!("pinned_model");
        initialization[field] = match field {
            "tools" | "mcp_servers" | "plugins" => json!(["not_granted"]),
            "permissionMode" => json!("bypassPermissions"),
            "cwd" => json!("unverified_directory"),
            "model" => json!("another_model"),
            _ => unreachable!(),
        };
        assert_eq!(
            feed(&mut machine, initialization),
            Err(SessionError::UnsafeProfile),
            "{field}"
        );
        assert!(!machine.snapshot().vendor_session_confirmed, "{field}");
        assert!(
            !machine
                .snapshot()
                .active_turn
                .as_ref()
                .is_some_and(|turn| turn.acknowledged),
            "{field}"
        );
        assert!(machine.snapshot().last_turn.is_none());
    }
}

#[test]
fn two_claude_turns_bind_root_and_input_uuid_then_keep_the_session_ready() {
    let mut machine = claude_ready(false);
    for ordinal in 1..=2 {
        let input = write(&machine.submit(request(ordinal)).expect("submit"));
        assert_eq!(input["uuid"], request(ordinal).request_id.to_string());
        assert_eq!(input["session_id"], CLAUDE_SESSION.to_string());
        if ordinal == 1 {
            let initialized = feed(&mut machine, claude_init()).expect("root init");
            assert!(initialized.events.iter().any(|event| matches!(event, SessionEvent::SessionBound { vendor_session_id } if vendor_session_id == &CLAUDE_SESSION.to_string())));
        }
        let message_id = format!("message_{ordinal}");
        let mut frame = |index: u128, event: Value, echo: bool| {
            let mut value = json!({"type":"stream_event","uuid":Uuid::from_u128(10_000+ordinal*100+index),"session_id":CLAUDE_SESSION,"parent_tool_use_id":null,"event":event});
            if echo {
                value["user_message_uuid"] = json!(request(ordinal).request_id);
            }
            feed(&mut machine, value).expect("root streaming frame")
        };
        frame(
            1,
            json!({"type":"message_start","message":{"id":message_id,"role":"assistant"}}),
            true,
        );
        frame(
            2,
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            false,
        );
        let delta = frame(
            3,
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"draft"}}),
            false,
        );
        assert_eq!(delta.text.last().expect("private snapshot").text, "draft");
        frame(4, json!({"type":"content_block_stop","index":0}), false);
        frame(5, json!({"type":"message_stop"}), false);
        let assistant = feed(&mut machine, json!({"type":"assistant","uuid":Uuid::from_u128(20_000+ordinal),"session_id":CLAUDE_SESSION,"parent_tool_use_id":null,
            "message":{"id":message_id,"role":"assistant","content":[{"type":"text","text":"corrected"}]}})).expect("bound complete message");
        assert_eq!(
            assistant.text.last().expect("replacement").text,
            "corrected"
        );
        let result = feed(
            &mut machine,
            claude_result(ordinal, "completed", "final answer"),
        )
        .expect("correlated terminal");
        assert_eq!(
            result.text.last().expect("result replacement").text,
            "final answer"
        );
        assert!(result.effects.is_empty());
        assert_eq!(machine.snapshot().phase, SessionPhase::Ready);
        assert_eq!(
            machine.snapshot().last_turn.expect("last turn").outcome,
            Some(TurnOutcome::Completed)
        );
        assert_eq!(
            machine.snapshot().vendor_session_id.as_deref(),
            Some(CLAUDE_SESSION.to_string().as_str())
        );
    }
    assert_eq!(machine.snapshot().turns_started, 2);
}

#[test]
fn exact_claude_resume_never_binds_another_vendor_session() {
    let mut machine = claude_ready(true);
    assert_eq!(
        machine.snapshot().expected_vendor_session_id,
        Some(CLAUDE_SESSION.to_string())
    );
    let mut wrong = claude_init();
    wrong["session_id"] = json!(Uuid::from_u128(501));
    assert_eq!(
        feed(&mut machine, wrong),
        Err(SessionError::UncorrelatedSession)
    );
    assert!(!machine.snapshot().vendor_session_confirmed);
    assert_eq!(machine.snapshot().phase, SessionPhase::Failed);
}

#[test]
fn nested_results_and_retired_results_cannot_finish_a_follow_up_turn() {
    let mut machine = claude_ready(false);
    machine.submit(request(1)).expect("first");
    feed(&mut machine, claude_init()).expect("validated root init");
    let first = claude_result(1, "completed", "first");
    feed(&mut machine, first.clone()).expect("first terminal");
    machine.submit(request(2)).expect("follow-up");
    let mut nested = claude_result(2, "completed", "nested must not display");
    nested["parent_tool_use_id"] = json!("nested_tool");
    let ignored = feed(&mut machine, nested).expect("nested ignored");
    assert!(ignored.text.is_empty());
    assert!(ignored.events.contains(&SessionEvent::RecordIgnored {
        reason: IgnoredReason::Nested
    }));
    assert_eq!(
        machine
            .snapshot()
            .active_turn
            .expect("still active")
            .turn_id,
        request(2).turn_id
    );
    let replayed = feed(&mut machine, first.clone()).expect("duplicate ignored");
    assert!(replayed.events.contains(&SessionEvent::RecordIgnored {
        reason: IgnoredReason::Duplicate
    }));
    let mut late = first;
    late["uuid"] = json!(Uuid::from_u128(90_001));
    let retired = feed(&mut machine, late).expect("retired ignored");
    assert!(retired.events.contains(&SessionEvent::RecordIgnored {
        reason: IgnoredReason::RetiredTurn
    }));
    feed(&mut machine, claude_result(2, "completed", "second"))
        .expect("only current turn completes");
    assert_eq!(
        machine.snapshot().last_turn.expect("last").turn_id,
        request(2).turn_id
    );
}

#[test]
fn missing_wrong_or_merged_input_uuid_never_becomes_terminal_evidence() {
    for malformed in ["missing", "wrong", "merged", "resumed"] {
        let mut machine = claude_ready(false);
        machine.submit(request(1)).expect("submit");
        feed(&mut machine, claude_init()).expect("validated root init");
        let mut result = claude_result(1, "completed", "untrusted result");
        match malformed {
            "missing" => {
                result
                    .as_object_mut()
                    .expect("object")
                    .remove("user_message_uuid");
            }
            "wrong" => result["user_message_uuid"] = json!(Uuid::from_u128(9999)),
            "merged" => {
                result["user_message_uuids"] = json!([request(1).request_id, request(2).request_id])
            }
            "resumed" => result["resume_reason"] = json!("interrupted_turn"),
            _ => unreachable!(),
        }
        assert!(feed(&mut machine, result).is_err(), "{malformed}");
        assert_eq!(machine.snapshot().phase, SessionPhase::RecoveryRequired);
        assert_eq!(
            machine.snapshot().active_turn.expect("retained").outcome,
            None
        );
        assert!(
            machine.submit(request(2)).is_err(),
            "no follow-up or automatic replay after uncertain correlation"
        );
    }
}

#[test]
fn acknowledgement_alone_does_not_authorize_uncorrelated_assistant_text() {
    let mut machine = claude_ready(false);
    machine.submit(request(1)).expect("submit");
    feed(&mut machine, claude_init()).expect("validated root init");
    feed(&mut machine, json!({"type":"user","uuid":request(1).request_id,"session_id":CLAUDE_SESSION,
        "isReplay":true,"parent_tool_use_id":null,"message":{"role":"user","content":"synthetic turn 1"}})).expect("input acknowledgment");
    assert!(machine.snapshot().active_turn.expect("active").acknowledged);
    let unbound = json!({"type":"assistant","uuid":Uuid::from_u128(5000),"session_id":CLAUDE_SESSION,
        "message":{"id":"unknown_message","role":"assistant","content":[{"type":"text","text":"not correlated"}]}});
    assert_eq!(
        feed(&mut machine, unbound),
        Err(SessionError::MissingCorrelation)
    );
}

#[test]
fn claude_interrupt_acknowledgement_is_not_cancellation_completion() {
    let mut machine = claude_ready(false);
    machine.submit(request(1)).expect("submit");
    feed(&mut machine, claude_init()).expect("validated root init");
    let interrupt = write(&machine.cancel(request(1).turn_id).expect("cancel request"));
    assert_eq!(interrupt["request"]["subtype"], "interrupt");
    feed(&mut machine, json!({"type":"control_response","response":{"subtype":"success","request_id":interrupt["request_id"]}})).expect("interrupt ack");
    assert_eq!(machine.snapshot().phase, SessionPhase::Busy);
    assert_eq!(
        machine
            .snapshot()
            .active_turn
            .as_ref()
            .and_then(|turn| turn.outcome),
        None
    );
    assert!(
        machine
            .cancel(request(1).turn_id)
            .expect("duplicate cancel")
            .effects
            .is_empty()
    );
    let mut cancelled = claude_result(1, "aborted_streaming", "partial");
    cancelled["is_error"] = json!(true);
    feed(&mut machine, cancelled).expect("correlated cancelled result");
    assert_eq!(
        machine.snapshot().last_turn.expect("last").outcome,
        Some(TurnOutcome::Cancelled)
    );
    assert_eq!(machine.snapshot().phase, SessionPhase::Ready);
}

#[test]
fn claude_denies_tool_and_unknown_control_requests_even_while_cancelling() {
    let mut machine = claude_ready(false);
    machine.submit(request(1)).expect("submit");
    machine.cancel(request(1).turn_id).expect("cancelling");
    for (index, subtype) in ["can_use_tool", "unknown_permission"]
        .into_iter()
        .enumerate()
    {
        let denied = feed(
            &mut machine,
            json!({"type":"control_request","request_id":format!("server_{index}"),
            "request":{"subtype":subtype,"input":{"command":"PRIVATE_COMMAND"}}}),
        )
        .expect("deny");
        let response = write(&denied);
        assert_eq!(response["response"]["subtype"], "error");
        assert!(
            !String::from_utf8_lossy(response.to_string().as_bytes()).contains("PRIVATE_COMMAND")
        );
        assert_eq!(
            machine
                .snapshot()
                .active_turn
                .as_ref()
                .and_then(|turn| turn.outcome),
            None
        );
    }
}
