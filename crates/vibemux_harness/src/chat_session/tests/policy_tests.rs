use super::*;

#[test]
fn pinned_claude_builtins_require_exact_vendor_source_identity() {
    let mut initialization = claude_init();
    initialization["claude_code_version"] = json!(CLAUDE_HELP_VERSION);
    initialization["plugins"] = json!([
        {"name":"cc-plugin-agents-md","source":"cc-plugin-agents-md@builtin","path":"builtin"},
        {"name":"cc-plugin-telemetry","source":"cc-plugin-telemetry@builtin","path":"builtin"},
        {"name":"cc-plugin-plugin-authoring","source":"cc-plugin-plugin-authoring@builtin","path":"builtin"}
    ]);
    feed(&mut claude_ready(false), initialization.clone()).expect("pinned builtins");
    for defect in ["version", "source", "name", "duplicate", "tools"] {
        let mut invalid = initialization.clone();
        match defect {
            "version" => invalid["claude_code_version"] = json!("future"),
            "source" => invalid["plugins"][0]["source"] = json!("cc-plugin-agents-md@external"),
            "name" => invalid["plugins"][0]["name"] = json!("unknown_builtin"),
            "duplicate" => invalid["plugins"][1] = invalid["plugins"][0].clone(),
            "tools" => invalid["tools"] = json!(["Bash"]),
            _ => unreachable!(),
        }
        assert_eq!(
            feed(&mut claude_ready(false), invalid),
            Err(SessionError::UnsafeProfile),
            "{defect}"
        );
    }
}

#[test]
fn codex_resume_requests_only_the_exact_owned_thread_and_revalidates_profile() {
    let (mut machine, open) = codex_opening(
        SessionTarget::Codex {
            resume_thread_id: Some("owned_thread".into()),
        },
        SessionLimits::default(),
    );
    assert_eq!(open["method"], "thread/resume");
    assert_eq!(open["params"]["threadId"], "owned_thread");
    assert_eq!(open["params"]["excludeTurns"], true);
    assert_eq!(open["params"]["sandbox"], "read-only");
    assert_eq!(open["params"]["approvalPolicy"], "never");
    assert!(
        open["params"].get("input").is_none(),
        "resume never replays a prompt"
    );
    feed(
        &mut machine,
        codex_open_response(&open["id"], "owned_thread"),
    )
    .expect("exact resume");
    assert_eq!(
        machine.snapshot().vendor_session_id.as_deref(),
        Some("owned_thread")
    );
    assert_eq!(machine.snapshot().turns_started, 0);

    let (mut wrong, open) = codex_opening(
        SessionTarget::Codex {
            resume_thread_id: Some("owned_thread".into()),
        },
        SessionLimits::default(),
    );
    assert_eq!(
        feed(
            &mut wrong,
            codex_open_response(&open["id"], "another_thread")
        ),
        Err(SessionError::UncorrelatedSession)
    );
    assert!(!wrong.snapshot().vendor_session_confirmed);
}

#[test]
fn an_effective_profile_that_widens_authority_never_becomes_ready() {
    for change in [
        "approval",
        "sandbox",
        "network",
        "ephemeral",
        "cwd",
        "busy",
        "nested",
    ] {
        let (mut machine, open) = codex_opening(
            SessionTarget::Codex {
                resume_thread_id: None,
            },
            SessionLimits::default(),
        );
        let mut response = codex_open_response(&open["id"], "root_thread");
        match change {
            "approval" => response["result"]["approvalPolicy"] = json!("on-request"),
            "sandbox" => response["result"]["sandbox"]["type"] = json!("workspaceWrite"),
            "network" => response["result"]["sandbox"]["networkAccess"] = json!(true),
            "ephemeral" => response["result"]["thread"]["ephemeral"] = json!(true),
            "cwd" => response["result"]["cwd"] = json!("OTHER_PRIVATE_PATH"),
            "busy" => response["result"]["thread"]["status"]["type"] = json!("active"),
            "nested" => response["result"]["thread"]["sessionId"] = json!("another_root"),
            _ => unreachable!(),
        }
        assert!(feed(&mut machine, response).is_err(), "{change}");
        assert_eq!(machine.snapshot().phase, SessionPhase::Failed);
        assert!(!machine.snapshot().vendor_session_confirmed);
        assert!(machine.submit(request(1)).is_err());
    }
    for field in ["tools", "mcp_servers", "plugins", "permissionMode"] {
        let mut machine = claude_ready(false);
        let mut initialization = claude_init();
        initialization[field] = if field == "permissionMode" {
            json!("bypassPermissions")
        } else {
            json!(["not_granted"])
        };
        assert_eq!(
            feed(&mut machine, initialization),
            Err(SessionError::UnsafeProfile)
        );
        assert!(!machine.snapshot().vendor_session_confirmed);
    }
}

#[test]
fn all_codex_server_capabilities_are_denied_without_echoing_private_arguments() {
    let mut machine = codex_ready();
    codex_turn(&mut machine, 1, "vendor_turn");
    for (index, method) in [
        "item/commandExecution/requestApproval",
        "item/fileChange/requestApproval",
        "item/permissions/requestApproval",
        "item/tool/requestUserInput",
        "item/tool/call",
        "mcpServer/elicitation/request",
        "unknown/execute",
    ]
    .into_iter()
    .enumerate()
    {
        let response = feed(&mut machine, json!({"id":format!("server_{index}"),"method":method,
            "params":{"threadId":"unowned_thread","turnId":"unowned_turn","input":"PRIVATE_REQUEST_BODY"}})).expect("closed denial");
        let frame = write(&response);
        assert!(
            frame.get("error").is_some()
                || frame.pointer("/result/decision") == Some(&json!("decline"))
        );
        assert!(!frame.to_string().contains("PRIVATE_REQUEST_BODY"));
        assert!(response.text.is_empty());
        assert_eq!(machine.snapshot().phase, SessionPhase::Busy);
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

#[test]
fn request_turn_and_rpc_budgets_refuse_before_emitting_new_input() {
    let mut machine = codex_with_limits(SessionLimits {
        max_turns: 1,
        ..Default::default()
    });
    codex_turn(&mut machine, 1, "vendor_turn");
    feed(&mut machine, codex_completed("vendor_turn", "completed")).expect("completed");
    assert_eq!(
        machine.submit(request(1)),
        Err(SessionError::DuplicateRequest)
    );
    assert_eq!(
        machine.submit(request(2)),
        Err(SessionError::TurnBudgetExhausted)
    );
    assert_eq!(machine.snapshot().turns_started, 1);
    assert_eq!(machine.snapshot().phase, SessionPhase::Ready);

    let mut rpc_limited = codex_with_limits(SessionLimits {
        max_rpc_requests: 3,
        ..Default::default()
    });
    assert_eq!(
        rpc_limited.submit(request(1)),
        Err(SessionError::RpcBudgetExhausted)
    );
    assert_eq!(
        rpc_limited.snapshot().turns_started,
        0,
        "reserve room for cancellation before the send"
    );
}

#[test]
fn parallel_turns_or_duplicate_canonical_ids_cannot_send_again() {
    let mut machine = codex_ready();
    codex_turn(&mut machine, 1, "vendor_turn");
    assert_eq!(
        machine.submit(request(2)),
        Err(SessionError::UnexpectedState)
    );
    assert_eq!(
        machine.cancel(request(2).turn_id),
        Err(SessionError::UncorrelatedTurn)
    );
    feed(&mut machine, codex_completed("vendor_turn", "completed")).expect("complete");
    let mut reused_request = request(2);
    reused_request.request_id = request(1).request_id;
    let mut reused_turn = request(2);
    reused_turn.turn_id = request(1).turn_id;
    assert_eq!(
        machine.submit(reused_request),
        Err(SessionError::DuplicateRequest)
    );
    assert_eq!(
        machine.submit(reused_turn),
        Err(SessionError::DuplicateRequest)
    );
}

#[test]
fn finite_pending_and_record_budgets_fence_unresolved_work() {
    let mut pending = codex_with_limits(SessionLimits {
        max_pending_frames: 1,
        ..Default::default()
    });
    pending.submit(request(1)).expect("submit");
    let early = json!({"method":"turn/started","params":{"threadId":"root_thread","turn":{"id":"vendor_turn","status":"inProgress","items":[]}}});
    feed(&mut pending, early.clone()).expect("one pending frame");
    assert_eq!(
        feed(&mut pending, early),
        Err(SessionError::PendingOverflow)
    );
    assert_eq!(pending.snapshot().phase, SessionPhase::RecoveryRequired);

    let mut records = codex_with_limits(SessionLimits {
        max_records: 3,
        ..Default::default()
    });
    codex_turn(&mut records, 1, "vendor_turn");
    assert_eq!(
        feed(&mut records, codex_completed("vendor_turn", "completed")),
        Err(SessionError::RecordBudgetExhausted)
    );
    assert_eq!(
        records.snapshot().active_turn.expect("unknown").outcome,
        None
    );
}

#[test]
fn changed_duplicate_records_and_stale_vendor_turn_ids_are_rejected() {
    let mut machine = claude_ready(false);
    machine.submit(request(1)).expect("first");
    feed(&mut machine, claude_init()).expect("validated root init");
    let result = claude_result(1, "completed", "first answer");
    feed(&mut machine, result.clone()).expect("first result");
    machine.submit(request(2)).expect("second");
    let mut forged = result;
    forged["result"] = json!("changed answer");
    assert_eq!(
        feed(&mut machine, forged),
        Err(SessionError::ConflictingRecord)
    );
    assert_eq!(
        machine
            .snapshot()
            .active_turn
            .expect("second not completed")
            .turn_id,
        request(2).turn_id
    );

    let mut codex = codex_ready();
    codex_turn(&mut codex, 1, "old_turn");
    feed(&mut codex, codex_completed("old_turn", "completed")).expect("completed");
    let input = write(&codex.submit(request(2)).expect("second"));
    assert_eq!(
        feed(
            &mut codex,
            json!({"id":input["id"],"result":{"turn":{"id":"old_turn","status":"inProgress","items":[]}}})
        ),
        Err(SessionError::UncorrelatedTurn)
    );
}

#[test]
fn prompts_are_structured_data_and_private_snapshots_are_bounded_and_redacted() {
    let mut machine = codex_with_limits(SessionLimits {
        max_reply_bytes: 7,
        ..Default::default()
    });
    let mut turn = request(1);
    turn.prompt = "PRIVATE_PROMPT ; & $(whoami) `echo` 你好\n\"quoted\"".into();
    let request_debug = format!("{turn:?}");
    let submitted = machine.submit(turn.clone()).expect("structured prompt");
    let input = write(&submitted);
    assert_eq!(input["params"]["input"][0]["text"], turn.prompt);
    assert!(!format!("{submitted:?} {request_debug}").contains("PRIVATE_PROMPT"));
    feed(&mut machine, json!({"id":input["id"],"result":{"turn":{"id":"vendor_turn","status":"inProgress","items":[]}}})).expect("ack");
    let delta = feed(&mut machine, json!({"method":"item/agentMessage/delta","params":{"threadId":"root_thread","turnId":"vendor_turn","itemId":"item","delta":"你好PRIVATE_REPLY"}})).expect("bounded text");
    let text = delta.text.last().expect("text snapshot");
    assert!(text.text.len() <= 7);
    assert!(text.truncated);
    assert_eq!(text.original_bytes, "你好PRIVATE_REPLY".len());
    assert!(!format!("{delta:?} {machine:?}").contains("PRIVATE_REPLY"));
    let public = serde_json::to_string(&machine.snapshot()).expect("public state");
    assert!(!public.contains("PRIVATE_PROMPT"));
    assert!(!public.contains("synthetic_project"));
}

#[test]
fn result_index_gaps_do_not_silently_become_complete_delivery() {
    let mut machine = claude_ready(false);
    machine.submit(request(1)).expect("submit");
    feed(&mut machine, claude_init()).expect("validated root init");
    let mut result = claude_result(1, "completed", "answer");
    result["result_index"] = json!(2);
    assert_eq!(feed(&mut machine, result), Err(SessionError::SequenceGap));
    assert_eq!(
        machine
            .snapshot()
            .active_turn
            .expect("unknown turn")
            .outcome,
        None
    );
}
