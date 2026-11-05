use super::*;
use vibemux_types::chat_tool::{
    CommandToolRecord, CommandToolStatus, MAX_TOOL_RECORDS, ToolRecord,
};

fn as_command(record: &ToolRecord) -> &CommandToolRecord {
    let ToolRecord::Command(record) = record else {
        panic!("command record")
    };
    record
}

fn command(id: &str) -> Value {
    json!({"type":"commandExecution","id":id,"command":"private command",
        "cwd":"C:/private/cwd","status":"completed","aggregatedOutput":"private output",
        "exitCode":0,"durationMs":7})
}

fn completed(item: Value, turn: &str) -> Value {
    json!({"method":"item/completed","params":{"threadId":"root_thread","turnId":turn,"item":item}})
}

fn terminal(items: Vec<Value>, turn: &str) -> Value {
    json!({"method":"turn/completed","params":{"threadId":"root_thread","turn":{"id":turn,"status":"completed","items":items}}})
}

fn completed_with_capture_loss(machine: &mut SessionMachine, turn: &str) {
    let terminal = feed(machine, terminal(vec![], turn)).expect("vendor terminal remains valid");
    assert!(terminal.tool_capture_incomplete);
    assert!(terminal.events.iter().any(|event| matches!(event,
        SessionEvent::TurnFinished { turn } if turn.outcome == Some(TurnOutcome::Completed))));
}

#[test]
fn completed_command_publishes_once_and_terminal_replay_is_deduplicated() {
    let mut machine = codex_ready();
    codex_turn(&mut machine, 1, "turn_one");
    let first =
        feed(&mut machine, completed(command("call"), "turn_one")).expect("tool completion");
    assert_eq!(first.tools.len(), 1);
    assert_eq!(first.tools[0].request_id, request(1).request_id);
    assert_eq!(first.tools[0].turn_id, request(1).turn_id);
    assert_eq!(
        as_command(&first.tools[0].record).status,
        CommandToolStatus::Completed
    );
    assert!(first.text.is_empty() && first.effects.is_empty());
    assert!(!format!("{first:?}").contains("private"));
    assert!(
        feed(&mut machine, completed(command("call"), "turn_one"))
            .expect("duplicate")
            .tools
            .is_empty()
    );
    assert!(
        feed(&mut machine, terminal(vec![command("call")], "turn_one"))
            .expect("terminal replay")
            .tools
            .is_empty()
    );
    assert!(
        feed(&mut machine, completed(command("retired"), "turn_one"))
            .expect("retired ignored")
            .tools
            .is_empty()
    );
    codex_turn(&mut machine, 2, "turn_two");
    assert_eq!(
        feed(&mut machine, terminal(vec![command("call")], "turn_two"))
            .expect("new turn resets dedup")
            .tools
            .len(),
        1
    );
}

#[test]
fn terminal_items_map_failed_and_declined_nullable_commands_only() {
    let mut machine = codex_ready();
    codex_turn(&mut machine, 1, "turn_one");
    let mut failed = command("failed");
    failed["status"] = json!("failed");
    failed["exitCode"] = json!(3);
    let mut declined = command("declined");
    declined["status"] = json!("declined");
    for field in ["aggregatedOutput", "exitCode", "durationMs"] {
        declined.as_object_mut().expect("item").remove(field);
    }
    let result = feed(
        &mut machine,
        terminal(
            vec![
                failed,
                declined,
                json!({"type":"unknownKind","id":"ignored"}),
            ],
            "turn_one",
        ),
    )
    .expect("terminal items");
    assert_eq!(result.tools.len(), 2);
    assert_eq!(
        as_command(&result.tools[0].record).status,
        CommandToolStatus::Failed
    );
    assert_eq!(
        as_command(&result.tools[1].record).status,
        CommandToolStatus::Declined
    );
    assert!(as_command(&result.tools[1].record).output.is_none());
}

#[test]
fn malformed_known_command_fields_and_in_progress_completion_mark_capture_loss() {
    for (field, value) in [
        ("command", json!(null)),
        ("cwd", json!(false)),
        ("status", json!("inProgress")),
        ("exitCode", json!(2_147_483_648_u64)),
        ("exitCode", json!("0")),
        ("durationMs", json!(-1)),
        ("durationMs", json!(9_007_199_254_740_992_u64)),
        ("aggregatedOutput", json!({"text":"wrong type"})),
    ] {
        let mut machine = codex_ready();
        codex_turn(&mut machine, 1, "turn_one");
        let mut item = command("call");
        item[field] = value;
        let refused =
            feed(&mut machine, completed(item, "turn_one")).expect("nonfatal display projection");
        assert!(
            refused.tools.is_empty() && refused.tool_capture_incomplete,
            "{field}"
        );
        completed_with_capture_loss(&mut machine, "turn_one");
    }
}

#[test]
fn wrong_thread_and_wrong_turn_never_publish_tool_content() {
    for field in ["threadId", "turnId"] {
        let mut machine = codex_ready();
        codex_turn(&mut machine, 1, "turn_one");
        let mut notification = completed(command("call"), "turn_one");
        notification["params"][field] = json!("foreign");
        assert!(feed(&mut machine, notification).is_err());
    }
}

#[test]
fn conflicting_tool_replay_and_agent_message_id_collisions_are_refused() {
    for first_is_tool in [false, true] {
        let mut machine = codex_ready();
        codex_turn(&mut machine, 1, "turn_one");
        let text = json!({"type":"agentMessage","id":"same","text":"answer"});
        let (first, second) = if first_is_tool {
            (command("same"), text)
        } else {
            (text, command("same"))
        };
        feed(&mut machine, completed(first, "turn_one")).expect("first kind");
        let refused = feed(&mut machine, completed(second, "turn_one"))
            .expect("nonfatal cross-kind projection conflict");
        assert!(refused.tools.is_empty() && refused.tool_capture_incomplete);
        completed_with_capture_loss(&mut machine, "turn_one");
    }
    let mut machine = codex_ready();
    codex_turn(&mut machine, 1, "turn_one");
    feed(&mut machine, completed(command("call"), "turn_one")).expect("first");
    let mut changed = command("call");
    changed["exitCode"] = json!(1);
    assert!(
        feed(&mut machine, completed(changed, "turn_one"))
            .expect("nonfatal replay conflict")
            .tool_capture_incomplete
    );
    completed_with_capture_loss(&mut machine, "turn_one");
}

#[test]
fn tool_count_and_aggregate_private_body_are_bounded() {
    let mut machine = codex_ready();
    codex_turn(&mut machine, 1, "turn_one");
    for ordinal in 0..MAX_TOOL_RECORDS {
        feed(
            &mut machine,
            completed(command(&format!("call_{ordinal}")), "turn_one"),
        )
        .expect("bounded tool");
    }
    let overflow = feed(&mut machine, completed(command("overflow"), "turn_one"))
        .expect("tool display budget");
    assert!(overflow.tools.is_empty() && overflow.tool_capture_incomplete);
    completed_with_capture_loss(&mut machine, "turn_one");
    let mut machine = codex_ready();
    codex_turn(&mut machine, 1, "turn_one");
    let mut rejected = false;
    for ordinal in 0..MAX_TOOL_RECORDS {
        let mut item = command(&format!("large_{ordinal}"));
        item["cwd"] = json!("x".repeat(32 * 1024));
        item["command"] = json!("x".repeat(8 * 1024));
        item["aggregatedOutput"] = json!("x".repeat(16 * 1024));
        let result =
            feed(&mut machine, completed(item, "turn_one")).expect("bounded display capture");
        if result.tool_capture_incomplete {
            assert!(result.tools.is_empty());
            rejected = true;
            break;
        }
    }
    assert!(
        rejected,
        "private tool bytes must cap before 64 large records"
    );
    completed_with_capture_loss(&mut machine, "turn_one");
}

#[test]
fn long_command_unicode_truncation_does_not_hide_conflicting_replay_tails() {
    let mut machine = codex_ready();
    codex_turn(&mut machine, 1, "turn_one");
    let mut item = command("call");
    item["command"] = json!(format!("{}first", "界".repeat(4000)));
    let published = feed(&mut machine, completed(item.clone(), "turn_one")).expect("long command");
    assert!(as_command(&published.tools[0].record).command_truncated);
    item["command"] = json!(format!("{}other", "界".repeat(4000)));
    assert!(
        feed(&mut machine, completed(item, "turn_one"))
            .expect("nonfatal hidden-tail conflict")
            .tool_capture_incomplete
    );
    completed_with_capture_loss(&mut machine, "turn_one");
}

#[test]
fn terminal_batch_overflow_keeps_the_vendor_completed_outcome() {
    for max_items in [1, 1024] {
        let limits = SessionLimits {
            max_text_items: max_items,
            ..SessionLimits::default()
        };
        let mut machine = codex_with_limits(limits);
        codex_turn(&mut machine, 1, "turn_one");
        let items = (0..65)
            .map(|index| command(&format!("call_{index}")))
            .collect();
        let result = feed(&mut machine, terminal(items, "turn_one"))
            .expect("display budget does not fail vendor terminal");
        assert_eq!(result.tools.len(), max_items.min(MAX_TOOL_RECORDS));
        assert!(result.tool_capture_incomplete);
        assert!(result.events.iter().any(|event| matches!(event,
            SessionEvent::TurnFinished { turn } if turn.outcome == Some(TurnOutcome::Completed))));
    }
}

#[test]
fn capture_loss_is_sticky_within_a_turn_and_resets_for_the_next_turn() {
    let mut machine = codex_ready();
    codex_turn(&mut machine, 1, "turn_one");
    let mut malformed = command("bad");
    malformed["status"] = json!("inProgress");
    assert!(
        feed(&mut machine, completed(malformed, "turn_one"))
            .expect("loss")
            .tool_capture_incomplete
    );
    let ignored =
        feed(&mut machine, completed(command("later"), "turn_one")).expect("bounded after loss");
    assert!(ignored.tools.is_empty() && ignored.tool_capture_incomplete);
    let result = feed(
        &mut machine,
        terminal(
            vec![
                json!({"type":"agentMessage","id":"answer","text":"answer survives display loss"}),
            ],
            "turn_one",
        ),
    )
    .expect("terminal");
    assert!(result.tool_capture_incomplete && !result.text.is_empty());
    codex_turn(&mut machine, 2, "turn_two");
    let next =
        feed(&mut machine, completed(command("bad"), "turn_two")).expect("new turn collection");
    assert_eq!(next.tools.len(), 1);
    assert!(!next.tool_capture_incomplete);
    assert!(
        !feed(&mut machine, terminal(vec![], "turn_two"))
            .expect("new turn terminal")
            .tool_capture_incomplete
    );
}

#[test]
fn started_command_items_are_not_live_tool_observations() {
    let mut machine = codex_ready();
    codex_turn(&mut machine, 1, "turn_one");
    let mut notification = completed(command("call"), "turn_one");
    notification["method"] = json!("item/started");
    notification["params"]["item"]["status"] = json!("inProgress");
    let started = feed(&mut machine, notification).expect("known progress ignored");
    assert!(started.tools.is_empty() && !started.tool_capture_incomplete);
    assert_eq!(
        feed(&mut machine, completed(command("call"), "turn_one"))
            .expect("actual completed observation")
            .tools
            .len(),
        1
    );
}

#[test]
fn command_start_publishes_private_progress_before_completion() {
    let mut machine = codex_ready();
    codex_turn(&mut machine, 1, "turn_one");
    let mut notification = completed(command("live_call"), "turn_one");
    notification["method"] = json!("item/started");
    notification["params"]["item"]["status"] = json!("inProgress");
    let started = feed(&mut machine, notification).expect("correlated command start");
    assert!(started.tools.is_empty() && !started.tool_capture_incomplete);
    assert_eq!(started.command_progress.len(), 1);
    assert_eq!(started.command_progress[0].call_id, "live_call");
    assert!(!started.command_progress_limited);
}
