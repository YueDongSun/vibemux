use super::*;
use vibemux_types::{chat_file_tool::FileChangeKind, chat_tool::ToolRecord};

fn file(id: &str) -> Value {
    json!({"type":"fileChange","id":id,"status":"completed","changes":[
        {"path":"reported/path.rs","kind":{"type":"update","move_path":null},"diff":"opaque reported diff"}]})
}
fn command(id: &str) -> Value {
    json!({"type":"commandExecution","id":id,"status":"completed","command":"echo hi","cwd":"C:/cwd","aggregatedOutput":null,"exitCode":0,"durationMs":null})
}
fn item(item: Value, turn: &str) -> Value {
    json!({"method":"item/completed","params":{"threadId":"root_thread","turnId":turn,"item":item}})
}
fn terminal(items: Vec<Value>, turn: &str) -> Value {
    json!({"method":"turn/completed","params":{"threadId":"root_thread","turn":{"id":turn,"status":"completed","items":items}}})
}
fn assert_completed(transition: &SessionTransition) {
    assert!(transition.events.iter().any(|event| matches!(event, SessionEvent::TurnFinished { turn } if turn.outcome == Some(TurnOutcome::Completed))));
}

#[test]
fn file_and_command_observations_share_order_dedup_and_normalized_optional_moves() {
    let mut machine = codex_ready();
    codex_turn(&mut machine, 1, "one");
    let result = feed(&mut machine, item(file("file"), "one")).expect("file");
    let ToolRecord::FileChange(record) = &result.tools[0].record else {
        panic!("file record")
    };
    assert_eq!(record.changes[0].kind, FileChangeKind::Update);
    assert_eq!(record.changes[0].diff, "opaque reported diff");
    let mut duplicate = file("file");
    duplicate["changes"][0]["kind"]
        .as_object_mut()
        .expect("kind")
        .remove("move_path");
    assert!(
        feed(&mut machine, item(duplicate, "one"))
            .expect("equivalent missing/null")
            .tools
            .is_empty()
    );
    let terminal = feed(
        &mut machine,
        terminal(
            vec![file("file"), command("command"), file("second_file")],
            "one",
        ),
    )
    .expect("ordered terminal");
    assert_eq!(terminal.tools.len(), 2);
    assert!(matches!(terminal.tools[0].record, ToolRecord::Command(_)));
    assert!(matches!(
        terminal.tools[1].record,
        ToolRecord::FileChange(_)
    ));
    assert_completed(&terminal);
}

#[test]
fn cross_kind_ids_and_hidden_file_tails_report_nonfatal_capture_loss() {
    for first_file in [false, true] {
        let mut machine = codex_ready();
        codex_turn(&mut machine, 1, "one");
        let (first, second) = if first_file {
            (file("same"), command("same"))
        } else {
            (command("same"), file("same"))
        };
        feed(&mut machine, item(first, "one")).expect("first");
        assert!(
            feed(&mut machine, item(second, "one"))
                .expect("cross-kind conflict")
                .tool_capture_incomplete
        );
        let terminal = feed(&mut machine, terminal(vec![], "one")).expect("terminal");
        assert!(terminal.tool_capture_incomplete);
        assert_completed(&terminal);
    }
    let mut machine = codex_ready();
    codex_turn(&mut machine, 1, "one");
    let mut first = file("tail");
    first["changes"][0]["diff"] = json!(format!("{}first", "界".repeat(6000)));
    feed(&mut machine, item(first.clone(), "one")).expect("truncated prefix");
    first["changes"][0]["diff"] = json!(format!("{}other", "界".repeat(6000)));
    assert!(
        feed(&mut machine, item(first, "one"))
            .expect("tail conflict")
            .tool_capture_incomplete
    );
}

#[test]
fn mixed_sixty_five_terminal_items_use_one_display_budget_without_failing_turn() {
    let mut machine = codex_with_limits(SessionLimits {
        max_text_items: 64,
        ..SessionLimits::default()
    });
    codex_turn(&mut machine, 1, "one");
    let items = (0..65)
        .map(|index| {
            if index % 2 == 0 {
                file(&format!("file_{index}"))
            } else {
                command(&format!("command_{index}"))
            }
        })
        .collect();
    let result = feed(&mut machine, terminal(items, "one")).expect("bounded mixed batch");
    assert_eq!(result.tools.len(), 64);
    assert!(result.tool_capture_incomplete);
    assert_completed(&result);
}

#[test]
fn malformed_file_source_stays_incomplete_and_next_turn_resets_capture() {
    for (field, bad) in [
        ("status", json!("inProgress")),
        ("changes", json!(null)),
        (
            "changes",
            json!([{"path":"bad\npath","kind":{"type":"add"},"diff":"x"}]),
        ),
    ] {
        let mut machine = codex_ready();
        codex_turn(&mut machine, 1, "one");
        let mut invalid = file("bad");
        invalid[field] = bad;
        assert!(
            feed(&mut machine, item(invalid, "one"))
                .expect("invalid display data")
                .tool_capture_incomplete
        );
        let result = feed(&mut machine, terminal(vec![], "one")).expect("vendor completion");
        assert!(result.tool_capture_incomplete);
        assert_completed(&result);
        codex_turn(&mut machine, 2, "two");
        let next = feed(&mut machine, item(file("bad"), "two")).expect("new turn");
        assert_eq!(next.tools.len(), 1);
        assert!(!next.tool_capture_incomplete);
    }
}

#[test]
fn wrong_and_retired_correlations_never_publish_and_started_files_are_ignored() {
    for field in ["threadId", "turnId"] {
        let mut machine = codex_ready();
        codex_turn(&mut machine, 1, "one");
        let mut bad = item(file("bad"), "one");
        bad["params"][field] = json!("foreign");
        assert!(feed(&mut machine, bad).is_err());
    }
    let mut machine = codex_ready();
    codex_turn(&mut machine, 1, "one");
    let mut started = item(file("file"), "one");
    started["method"] = json!("item/started");
    assert!(
        feed(&mut machine, started)
            .expect("started ignored")
            .tools
            .is_empty()
    );
    feed(&mut machine, terminal(vec![], "one")).expect("complete");
    assert!(
        feed(&mut machine, item(file("retired"), "one"))
            .expect("retired")
            .tools
            .is_empty()
    );
}

#[test]
fn file_and_command_records_consume_the_same_retained_private_byte_budget() {
    let mut large_file = file("large_file");
    large_file["changes"] = json!(
        (0..16)
            .map(|_| json!({"path":"\"".repeat(4096),
        "kind":{"type":"update","move_path":"\"".repeat(4096)},"diff":""}))
            .collect::<Vec<_>>()
    );
    let mut large_command = command("large_command");
    large_command["command"] = json!("x".repeat(8 * 1024));
    large_command["cwd"] = json!("x".repeat(32 * 1024));
    large_command["aggregatedOutput"] = json!("x".repeat(16 * 1024));
    for file_first in [false, true] {
        let mut machine = codex_ready();
        codex_turn(&mut machine, 1, "one");
        let (first, second) = if file_first {
            (large_file.clone(), large_command.clone())
        } else {
            (large_command.clone(), large_file.clone())
        };
        assert_eq!(
            feed(&mut machine, item(first, "one"))
                .expect("one body fits")
                .tools
                .len(),
            1
        );
        let refused = feed(&mut machine, item(second, "one")).expect("combined display cap");
        assert!(refused.tools.is_empty() && refused.tool_capture_incomplete);
        let terminal = feed(&mut machine, terminal(vec![], "one")).expect("terminal unaffected");
        assert!(terminal.tool_capture_incomplete);
        assert_completed(&terminal);
    }
}

#[test]
fn omitted_source_entries_remain_in_replay_identity_and_invalid_tails_are_not_hidden() {
    let mut complete = file("source_list");
    complete["changes"] = json!(
        (0..17)
            .map(|index| json!({"path":format!("{index}.rs"),
        "kind":{"type":"add"},"diff":""}))
            .collect::<Vec<_>>()
    );
    let mut machine = codex_ready();
    codex_turn(&mut machine, 1, "one");
    let published =
        feed(&mut machine, item(complete.clone(), "one")).expect("retained list prefix");
    assert!(published.tools[0].record.is_truncated());
    complete["changes"][16]["path"] = json!("other.rs");
    assert!(
        feed(&mut machine, item(complete.clone(), "one"))
            .expect("hidden list tail conflict")
            .tool_capture_incomplete
    );
    let mut machine = codex_ready();
    codex_turn(&mut machine, 1, "one");
    complete["changes"][16]["diff"] = json!(false);
    let refused = feed(&mut machine, item(complete, "one")).expect("invalid omitted tail");
    assert!(refused.tools.is_empty() && refused.tool_capture_incomplete);
}
