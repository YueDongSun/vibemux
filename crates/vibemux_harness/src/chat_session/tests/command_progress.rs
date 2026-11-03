use super::*;
use serde_json::{Value, json};
use uuid::Uuid;
use vibemux_types::chat_tool::ToolRecord;

fn config() -> SessionConfig {
    SessionConfig {
        target: SessionTarget::Codex {
            resume_thread_id: None,
        },
        generation_id: Uuid::from_u128(1),
        working_directory: if cfg!(windows) {
            "C:/synthetic"
        } else {
            "/synthetic"
        }
        .into(),
        model: None,
    }
}

fn feed(machine: &mut SessionMachine, value: Value) -> Result<SessionTransition, SessionError> {
    machine.receive(
        machine.snapshot().records_received + 1,
        value.to_string().as_bytes(),
    )
}

fn write(transition: SessionTransition) -> Value {
    let SessionEffect::Write(frame) = transition.effects.last().expect("protocol write") else {
        panic!("protocol write")
    };
    serde_json::from_slice(frame.as_bytes()).expect("JSON")
}

fn ready() -> SessionMachine {
    let mut machine = SessionMachine::new(config(), SessionLimits::default()).expect("machine");
    let initialize = write(machine.start().expect("start"));
    let open = write(
        feed(
            &mut machine,
            json!({"id":initialize["id"],"result":{"userAgent":"codex/0.166.0"}}),
        )
        .expect("initialize"),
    );
    feed(&mut machine, json!({"id":open["id"],"result":{
        "thread":{"id":"thread","sessionId":"thread","ephemeral":false,"status":{"type":"idle"}},
        "approvalPolicy":"never","sandbox":{"type":"readOnly","networkAccess":false},"cwd":config().working_directory
    }})).expect("open");
    machine
}

fn submit(machine: &mut SessionMachine, ordinal: u128) -> Value {
    write(
        machine
            .submit(TurnRequest {
                request_id: Uuid::from_u128(100 + ordinal),
                turn_id: Uuid::from_u128(200 + ordinal),
                prompt: "synthetic".into(),
            })
            .expect("submit"),
    )
}

fn ack(machine: &mut SessionMachine, input: &Value, turn: &str) -> SessionTransition {
    feed(
        machine,
        json!({"id":input["id"],"result":{"turn":{"id":turn,"status":"inProgress"}}}),
    )
    .expect("ack")
}

fn begin(machine: &mut SessionMachine, ordinal: u128, turn: &str) {
    let input = submit(machine, ordinal);
    ack(machine, &input, turn);
}

fn started_item(id: &str) -> Value {
    json!({"type":"commandExecution","id":id,"command":"private command","cwd":"private directory",
        "status":"inProgress","aggregatedOutput":null})
}

fn item(method: &str, item: Value, turn: &str) -> Value {
    json!({"method":method,"params":{"threadId":"thread","turnId":turn,"item":item}})
}

fn start(id: &str, turn: &str) -> Value {
    item("item/started", started_item(id), turn)
}

fn delta(id: &str, text: &str, turn: &str) -> Value {
    json!({"method":"item/commandExecution/outputDelta","params":{"threadId":"thread","turnId":turn,"itemId":id,"delta":text}})
}

fn completed_item(id: &str) -> Value {
    json!({"type":"commandExecution","id":id,"command":"authoritative command","cwd":"C:/authoritative",
        "status":"completed","aggregatedOutput":"authoritative output","exitCode":0,"durationMs":3})
}

fn finish(turn: &str, items: Vec<Value>) -> Value {
    json!({"method":"turn/completed","params":{"threadId":"thread","turn":{"id":turn,"status":"completed","items":items}}})
}

#[test]
fn command_start_is_private_progress_without_a_durable_tool_or_authority() {
    let mut machine = ready();
    begin(&mut machine, 1, "one");
    let started = feed(&mut machine, start("private call", "one")).expect("start");
    assert!(started.tools.is_empty() && started.events.is_empty() && started.effects.is_empty());
    assert!(!started.tool_capture_incomplete && !started.command_progress_limited);
    let progress = &started.command_progress[0];
    assert_eq!(progress.request_id, Uuid::from_u128(101));
    assert_eq!(progress.turn_id, Uuid::from_u128(201));
    assert_eq!(progress.command, "private command");
    assert_eq!(progress.command_original_bytes, 15);
    assert_eq!(progress.working_directory, "private directory");
    assert_eq!(progress.directory_original_bytes, 17);
    assert!(progress.output_tail.is_none() && progress.output_original_bytes.is_none());
    assert_eq!(progress.revision, 1);
    let debug = format!("{started:?}");
    for secret in ["private call", "private command", "private directory"] {
        assert!(!debug.contains(secret));
    }
}

#[test]
fn repeated_equal_deltas_append_and_duplicate_starts_preserve_accumulated_output() {
    let mut machine = ready();
    begin(&mut machine, 1, "one");
    feed(&mut machine, start("call", "one")).expect("start");
    let first = feed(&mut machine, delta("call", "repeat", "one")).expect("first");
    assert_eq!(first.command_progress[0].revision, 2);
    let duplicate = feed(&mut machine, start("call", "one")).expect("duplicate start");
    assert!(duplicate.command_progress.is_empty() && !duplicate.command_progress_limited);
    let second = feed(&mut machine, delta("call", "repeat", "one")).expect("second");
    assert_eq!(
        second.command_progress[0].output_tail.as_deref(),
        Some("repeatrepeat")
    );
    assert_eq!(second.command_progress[0].output_original_bytes, Some(12));
    assert_eq!(second.command_progress[0].output_omitted_bytes, 0);
    assert_eq!(second.command_progress[0].revision, 3);
    assert!(!format!("{second:?}").contains("repeatrepeat"));
}

#[test]
fn utf8_prefixes_and_output_suffix_preserve_original_and_omitted_byte_counts() {
    let mut machine = ready();
    begin(&mut machine, 1, "one");
    let mut value = started_item("call");
    value["command"] = json!("界".repeat(3000));
    value["cwd"] = json!("界".repeat(12000));
    value["aggregatedOutput"] = json!(format!("head{}", "界".repeat(6000)));
    let initial = feed(&mut machine, item("item/started", value, "one")).expect("bounded start");
    let progress = &initial.command_progress[0];
    assert_eq!(progress.command.len(), 8190);
    assert_eq!(progress.command_original_bytes, 9000);
    assert!(progress.command_truncated);
    assert_eq!(progress.working_directory.len(), 32766);
    assert_eq!(progress.directory_original_bytes, 36000);
    assert!(progress.directory_truncated);
    assert_eq!(progress.output_original_bytes, Some(18004));
    assert_eq!(progress.output_tail.as_ref().expect("tail").len(), 16383);
    assert_eq!(progress.output_omitted_bytes, 1621);
    let next = feed(&mut machine, delta("call", "终", "one")).expect("UTF8 delta");
    let progress = &next.command_progress[0];
    assert!(progress.output_tail.as_ref().expect("tail").ends_with('终'));
    assert_eq!(progress.output_original_bytes, Some(18007));
    assert_eq!(progress.output_omitted_bytes, 1624);
    let large_delta = format!("{}end", "界".repeat(6000));
    let next = feed(&mut machine, delta("call", &large_delta, "one")).expect("large delta");
    assert!(
        next.command_progress[0]
            .output_tail
            .as_ref()
            .expect("tail")
            .ends_with("end")
    );
    assert_eq!(next.command_progress[0].output_original_bytes, Some(36010));
}

#[test]
fn pre_ack_progress_coalesces_and_keeps_thread_turn_and_retired_fences() {
    let mut machine = ready();
    let input = submit(&mut machine, 1);
    assert!(
        feed(&mut machine, start("call", "one"))
            .expect("buffered start")
            .command_progress
            .is_empty()
    );
    feed(&mut machine, delta("call", "a", "one")).expect("buffered delta");
    feed(&mut machine, delta("call", "a", "one")).expect("buffered repeat");
    let acknowledged = ack(&mut machine, &input, "one");
    assert_eq!(acknowledged.command_progress.len(), 1);
    assert_eq!(
        acknowledged.command_progress[0].output_tail.as_deref(),
        Some("aa")
    );
    feed(&mut machine, finish("one", vec![])).expect("finish");
    let retired = feed(&mut machine, delta("call", "late", "one")).expect("retired");
    assert!(retired.command_progress.is_empty());
    assert!(retired.events.iter().any(|event| matches!(
        event,
        SessionEvent::RecordIgnored {
            reason: IgnoredReason::RetiredTurn
        }
    )));
    for field in ["threadId", "turnId"] {
        let mut machine = ready();
        begin(&mut machine, 1, "one");
        let mut wrong = delta("call", "foreign", "one");
        wrong["params"][field] = json!("foreign");
        assert_eq!(
            feed(&mut machine, wrong),
            Err(if field == "threadId" {
                SessionError::UncorrelatedSession
            } else {
                SessionError::UncorrelatedTurn
            })
        );
    }
}

#[test]
fn progress_uses_the_existing_finite_pre_ack_buffer() {
    let mut machine = ready();
    submit(&mut machine, 1);
    for _ in 0..64 {
        feed(&mut machine, delta("call", "a", "one")).expect("bounded buffer");
    }
    assert_eq!(
        feed(&mut machine, delta("call", "a", "one")),
        Err(SessionError::PendingOverflow)
    );
}

#[test]
fn terminal_in_the_same_pre_ack_batch_retires_the_coalesced_progress_payload() {
    let mut machine = ready();
    let input = submit(&mut machine, 1);
    feed(&mut machine, start("call", "one")).expect("buffered start");
    feed(&mut machine, delta("call", "output", "one")).expect("buffered delta");
    feed(
        &mut machine,
        item("item/completed", completed_item("call"), "one"),
    )
    .expect("buffered terminal");
    let transition = ack(&mut machine, &input, "one");
    assert_eq!(transition.tools.len(), 1);
    assert!(transition.command_progress.is_empty());
    assert!(!transition.command_progress_limited && !transition.tool_capture_incomplete);
}

#[test]
fn progress_conflicts_and_malformed_fields_leave_terminal_capture_authoritative() {
    for malformed in [false, true] {
        let mut machine = ready();
        begin(&mut machine, 1, "one");
        feed(&mut machine, start("call", "one")).expect("start");
        let mut changed = started_item("call");
        changed["command"] = if malformed {
            json!(null)
        } else {
            json!("changed provisional command")
        };
        let refused =
            feed(&mut machine, item("item/started", changed, "one")).expect("optional loss");
        assert!(refused.command_progress_limited && refused.command_progress.is_empty());
        assert!(!refused.tool_capture_incomplete);
        assert!(
            feed(&mut machine, delta("call", "ignored", "one"))
                .expect("blocked progress")
                .command_progress
                .is_empty()
        );
        let final_item = completed_item("call");
        let terminal = feed(
            &mut machine,
            item("item/completed", final_item.clone(), "one"),
        )
        .expect("authoritative terminal");
        assert_eq!(terminal.tools.len(), 1);
        let ToolRecord::Command(record) = &terminal.tools[0].record else {
            panic!("command")
        };
        assert_eq!(record.command, "authoritative command");
        assert_eq!(record.working_directory, "C:/authoritative");
        assert!(terminal.command_progress_limited && !terminal.tool_capture_incomplete);
        assert!(
            feed(&mut machine, delta("call", "late", "one"))
                .expect("late ignored")
                .command_progress
                .is_empty()
        );
        assert!(
            feed(&mut machine, start("call", "one"))
                .expect("late start ignored")
                .command_progress
                .is_empty()
        );
        let finished =
            feed(&mut machine, finish("one", vec![final_item])).expect("terminal replay");
        assert!(finished.tools.is_empty() && !finished.tool_capture_incomplete);
        assert!(finished.events.iter().any(|event| matches!(event, SessionEvent::TurnFinished { turn } if turn.outcome == Some(TurnOutcome::Completed))));
    }
}

#[test]
fn malformed_or_unknown_deltas_limit_only_progress_and_next_turn_resets() {
    for (field, value) in [
        ("itemId", json!(null)),
        ("itemId", json!("unknown")),
        ("delta", json!(false)),
    ] {
        let mut machine = ready();
        begin(&mut machine, 1, "one");
        feed(&mut machine, start("call", "one")).expect("start");
        let mut invalid = delta("call", "output", "one");
        invalid["params"][field] = value;
        let limited = feed(&mut machine, invalid).expect("optional malformed progress");
        assert!(limited.command_progress_limited && !limited.tool_capture_incomplete);
        let finished = feed(&mut machine, finish("one", vec![completed_item("call")]))
            .expect("valid terminal");
        assert_eq!(finished.tools.len(), 1);
        assert!(finished.command_progress_limited && !finished.tool_capture_incomplete);
        begin(&mut machine, 2, "two");
        let next = feed(&mut machine, start("call", "two")).expect("reused call independent");
        assert!(!next.command_progress_limited);
        assert_eq!(next.command_progress[0].revision, 1);
        assert_eq!(next.command_progress[0].request_id, Uuid::from_u128(102));
        assert!(next.command_progress[0].output_tail.is_none());
    }
}

#[test]
fn cumulative_identity_capacity_does_not_reopen_after_terminal_retirement() {
    let mut machine = ready();
    begin(&mut machine, 1, "one");
    for ordinal in 0..64 {
        let id = format!("call_{ordinal}");
        assert_eq!(
            feed(&mut machine, start(&id, "one"))
                .expect("start")
                .command_progress
                .len(),
            1
        );
        feed(
            &mut machine,
            item("item/completed", completed_item(&id), "one"),
        )
        .expect("terminal retirement");
    }
    let overflow =
        feed(&mut machine, start("overflow", "one")).expect("finite progress identities");
    assert!(overflow.command_progress_limited && overflow.command_progress.is_empty());
    assert!(!overflow.tool_capture_incomplete);
}

#[test]
fn aggregate_retained_text_is_bounded_independently_of_terminal_capture() {
    let mut machine = ready();
    begin(&mut machine, 1, "one");
    let mut accepted_bytes = 0;
    let mut limited = false;
    for ordinal in 0..64 {
        let mut value = started_item(&format!("large_{ordinal}"));
        value["command"] = json!("a".repeat(8 * 1024));
        value["cwd"] = json!("d".repeat(32 * 1024));
        value["aggregatedOutput"] = json!("o".repeat(16 * 1024));
        let result =
            feed(&mut machine, item("item/started", value, "one")).expect("bounded projection");
        if result.command_progress_limited {
            assert!(result.command_progress.is_empty());
            limited = true;
            break;
        }
        let progress = &result.command_progress[0];
        accepted_bytes += progress.command.len()
            + progress.working_directory.len()
            + progress.output_tail.as_ref().expect("output").len();
    }
    assert!(limited && accepted_bytes <= 256 * 1024);
    let result = feed(&mut machine, finish("one", vec![completed_item("final")]))
        .expect("terminal independent");
    assert_eq!(result.tools.len(), 1);
    assert!(!result.tool_capture_incomplete);
}

#[test]
fn delta_that_exhausts_retained_bytes_limits_progress_without_losing_terminal_capture() {
    let mut machine = ready();
    begin(&mut machine, 1, "one");
    // Six command/cwd prefixes consume 240 KiB; one output tail fills the budget.
    for ordinal in 0..6 {
        let mut value = started_item(&format!("call_{ordinal}"));
        value["command"] = json!("c".repeat(8 * 1024));
        value["cwd"] = json!("d".repeat(32 * 1024));
        assert_eq!(
            feed(&mut machine, item("item/started", value, "one"))
                .expect("start")
                .command_progress
                .len(),
            1
        );
    }
    feed(&mut machine, delta("call_0", &"o".repeat(16 * 1024), "one")).expect("exact capacity");
    let limited = feed(&mut machine, delta("call_1", "x", "one")).expect("optional overflow");
    assert!(limited.command_progress_limited && limited.command_progress.is_empty());
    assert!(!limited.tool_capture_incomplete);
    assert!(
        feed(&mut machine, delta("call_1", "later", "one"))
            .expect("retired payload")
            .command_progress
            .is_empty()
    );
    let terminal = feed(&mut machine, finish("one", vec![completed_item("call_1")]))
        .expect("terminal capture remains valid");
    assert_eq!(terminal.tools.len(), 1);
    assert!(!terminal.tool_capture_incomplete);
}

#[test]
fn file_change_terminal_retires_command_progress_without_a_cross_kind_conflict() {
    let mut machine = ready();
    begin(&mut machine, 1, "one");
    feed(&mut machine, start("call", "one")).expect("command start");
    let final_file = json!({"type":"fileChange","id":"call","status":"completed","changes":[
        {"path":"reported.rs","kind":{"type":"update","move_path":null},"diff":"reported diff"}]});
    let terminal = feed(
        &mut machine,
        item("item/completed", final_file.clone(), "one"),
    )
    .expect("authoritative file terminal");
    assert_eq!(terminal.tools.len(), 1);
    assert!(matches!(
        terminal.tools[0].record,
        ToolRecord::FileChange(_)
    ));
    assert!(!terminal.tool_capture_incomplete && !terminal.command_progress_limited);
    assert!(
        feed(&mut machine, delta("call", "late", "one"))
            .expect("late delta")
            .command_progress
            .is_empty()
    );
    assert!(
        feed(&mut machine, finish("one", vec![final_file]))
            .expect("file terminal replay")
            .tools
            .is_empty()
    );
}

#[test]
fn agent_text_namespace_collision_cannot_create_command_authority() {
    for progress_first in [false, true] {
        let mut machine = ready();
        begin(&mut machine, 1, "one");
        let text = item(
            "item/completed",
            json!({"type":"agentMessage","id":"same","text":"assistant text"}),
            "one",
        );
        if progress_first {
            feed(&mut machine, start("same", "one")).expect("progress");
            feed(&mut machine, text).expect("text");
        } else {
            feed(&mut machine, text).expect("text");
            let refused = feed(&mut machine, start("same", "one")).expect("no tool authority");
            assert!(refused.command_progress_limited && refused.command_progress.is_empty());
        }
        let delta =
            feed(&mut machine, delta("same", "ignored", "one")).expect("no progress authority");
        assert!(delta.command_progress.is_empty() && delta.command_progress_limited);
        let terminal = feed(
            &mut machine,
            item("item/completed", completed_item("same"), "one"),
        )
        .expect("existing terminal namespace rules");
        assert!(terminal.tools.is_empty() && terminal.tool_capture_incomplete);
    }
}
