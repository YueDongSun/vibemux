use super::*;

#[test]
fn codex_notifications_before_the_turn_receipt_are_bounded_and_not_published_early() {
    let mut machine = codex_ready();
    let input = write(&machine.submit(request(1)).expect("submit"));
    let early = feed(&mut machine, json!({"method":"item/agentMessage/delta","params":{"threadId":"root_thread","turnId":"vendor_turn","itemId":"item","delta":"early"}})).expect("buffered");
    assert!(early.text.is_empty());
    assert!(!machine.snapshot().active_turn.expect("active").acknowledged);
    let acknowledged = feed(&mut machine, json!({"id":input["id"],"result":{"turn":{"id":"vendor_turn","status":"inProgress","items":[]}}})).expect("receipt binds buffered frames");
    assert_eq!(
        acknowledged.text.last().expect("correlated text").text,
        "early"
    );
}

#[test]
fn codex_cancel_before_vendor_turn_id_waits_then_interrupts_only_that_turn() {
    let mut machine = codex_ready();
    let input = write(&machine.submit(request(1)).expect("submit"));
    assert!(
        machine
            .cancel(request(1).turn_id)
            .expect("pending cancel")
            .effects
            .is_empty()
    );
    let acknowledged = feed(&mut machine, json!({"id":input["id"],"result":{"turn":{"id":"vendor_turn","status":"inProgress","items":[]}}})).expect("ack");
    let interrupt = write(&acknowledged);
    assert_eq!(interrupt["method"], "turn/interrupt");
    assert_eq!(
        interrupt["params"],
        json!({"threadId":"root_thread","turnId":"vendor_turn"})
    );
    assert_ne!(interrupt["id"], input["id"]);
    feed(&mut machine, json!({"id":interrupt["id"],"result":{}})).expect("ack only");
    assert_eq!(machine.snapshot().phase, SessionPhase::Busy);
    feed(&mut machine, codex_completed("vendor_turn", "interrupted")).expect("cancelled terminal");
    assert_eq!(
        machine.snapshot().last_turn.expect("last").outcome,
        Some(TurnOutcome::Cancelled)
    );
}

#[test]
fn a_completion_cancel_race_never_overwrites_success_with_a_late_rpc_error() {
    let mut machine = codex_ready();
    codex_turn(&mut machine, 1, "vendor_turn");
    let interrupt = write(&machine.cancel(request(1).turn_id).expect("cancel"));
    feed(&mut machine, codex_completed("vendor_turn", "completed")).expect("completion wins");
    feed(
        &mut machine,
        json!({"id":interrupt["id"],"error":{"code":-1,"message":"already finished"}}),
    )
    .expect("late cancel response is inert");
    assert_eq!(
        machine.snapshot().last_turn.expect("last").outcome,
        Some(TurnOutcome::Completed)
    );
    assert_eq!(machine.snapshot().phase, SessionPhase::Ready);
}

#[test]
fn wrong_thread_or_turn_never_updates_private_text_or_finishes_work() {
    for (thread, turn) in [
        ("other_thread", "vendor_turn"),
        ("root_thread", "other_turn"),
    ] {
        let mut machine = codex_ready();
        codex_turn(&mut machine, 1, "vendor_turn");
        assert!(feed(&mut machine, json!({"method":"item/agentMessage/delta","params":{"threadId":thread,"turnId":turn,"itemId":"item","delta":"not ours"}})).is_err());
        assert_eq!(machine.snapshot().phase, SessionPhase::RecoveryRequired);
        assert_eq!(
            machine
                .snapshot()
                .active_turn
                .expect("active ownership")
                .outcome,
            None
        );
        assert!(machine.submit(request(2)).is_err());
    }
}

#[test]
fn record_replay_is_rejected_without_deduplicating_legitimate_equal_deltas() {
    let mut machine = codex_ready();
    codex_turn(&mut machine, 1, "vendor_turn");
    let delta = json!({"method":"item/agentMessage/delta","params":{"threadId":"root_thread","turnId":"vendor_turn","itemId":"item","delta":"ha"}}).to_string();
    let sequence = machine.snapshot().records_received + 1;
    let first = machine
        .receive(sequence, delta.as_bytes())
        .expect("first delta");
    assert_eq!(first.text.last().expect("text").text, "ha");
    assert_eq!(
        machine.receive(sequence, delta.as_bytes()),
        Err(SessionError::SequenceReplay)
    );
    let second = machine
        .receive(sequence + 1, delta.as_bytes())
        .expect("same bytes, new legitimate record");
    assert_eq!(second.text.last().expect("text").text, "haha");
}

#[test]
fn duplicate_terminal_cannot_finish_the_next_turn() {
    let mut machine = codex_ready();
    codex_turn(&mut machine, 1, "old_turn");
    feed(&mut machine, codex_completed("old_turn", "completed")).expect("first completes");
    codex_turn(&mut machine, 2, "new_turn");
    let late =
        feed(&mut machine, codex_completed("old_turn", "completed")).expect("retired terminal");
    assert!(late.events.contains(&SessionEvent::RecordIgnored {
        reason: IgnoredReason::RetiredTurn
    }));
    assert_eq!(
        machine
            .snapshot()
            .active_turn
            .expect("new turn remains active")
            .turn_id,
        request(2).turn_id
    );
}

#[test]
fn losing_the_process_between_turns_preserves_the_completed_turn() {
    let mut machine = codex_ready();
    codex_turn(&mut machine, 1, "vendor_turn");
    feed(&mut machine, codex_completed("vendor_turn", "completed")).expect("completed");
    let lost = machine.transport_lost();
    assert!(lost.effects.is_empty());
    assert_eq!(machine.snapshot().phase, SessionPhase::Suspended);
    assert_eq!(
        machine.snapshot().last_turn.expect("last").outcome,
        Some(TurnOutcome::Completed)
    );
    assert!(machine.start().is_err());
    assert!(machine.submit(request(2)).is_err());
}

#[test]
fn process_exit_during_a_turn_retains_uncertainty_and_emits_no_resend() {
    let mut machine = codex_ready();
    let prepared = machine.submit(request(1)).expect("prepared");
    assert_eq!(prepared.effects.len(), 1);
    let lost = machine.process_exited();
    assert!(lost.effects.is_empty());
    assert_eq!(machine.snapshot().phase, SessionPhase::RecoveryRequired);
    let retained = machine
        .snapshot()
        .active_turn
        .expect("unknown turn retained");
    assert_eq!(retained.request_id, request(1).request_id);
    assert_eq!(retained.phase, TurnPhase::Uncertain);
    assert_eq!(retained.outcome, None);
    assert!(machine.submit(request(1)).is_err());
    assert!(machine.submit(request(2)).is_err());
}

#[test]
fn close_cancels_active_work_and_waits_for_terminal_and_actual_process_exit() {
    let mut machine = codex_ready();
    codex_turn(&mut machine, 1, "vendor_turn");
    let closing = machine.close().expect("close request");
    assert_eq!(write(&closing)["method"], "turn/interrupt");
    assert!(!closing.effects.contains(&SessionEffect::CloseInput));
    assert_eq!(machine.snapshot().phase, SessionPhase::Busy);
    let terminal =
        feed(&mut machine, codex_completed("vendor_turn", "interrupted")).expect("cancelled");
    assert_eq!(terminal.effects, [SessionEffect::CloseInput]);
    assert_eq!(machine.snapshot().phase, SessionPhase::Closing);
    machine.process_exited();
    assert_eq!(machine.snapshot().phase, SessionPhase::Closed);
    assert_eq!(
        machine.snapshot().last_turn.expect("last").outcome,
        Some(TurnOutcome::Cancelled)
    );
}

#[test]
fn malformed_and_oversized_frames_fail_closed_without_private_error_text() {
    for frame in [
        b"{PRIVATE_REPLY".to_vec(),
        b"[]".to_vec(),
        vec![b'x'; MAX_FRAME_BYTES + 1],
    ] {
        let mut machine = codex_ready();
        codex_turn(&mut machine, 1, "vendor_turn");
        let error = machine
            .receive(machine.snapshot().records_received + 1, &frame)
            .expect_err("invalid input");
        assert!(!format!("{error:?} {error}").contains("PRIVATE_REPLY"));
        assert_eq!(machine.snapshot().phase, SessionPhase::RecoveryRequired);
    }
}

#[test]
fn jsonl_decoder_handles_split_utf8_crlf_and_rejects_oversized_batches() {
    let mut decoder = SessionDecoder::new(&SessionLimits::default()).expect("decoder");
    let bytes = "{\"text\":\"你好\"}\r\n".as_bytes();
    let split = bytes.iter().position(|byte| *byte > 127).expect("UTF8") + 1;
    assert!(
        decoder
            .push(&bytes[..split])
            .expect("partial UTF8")
            .is_empty()
    );
    assert_eq!(
        decoder.push(&bytes[split..]).expect("record"),
        ["{\"text\":\"你好\"}".to_string()]
    );
    assert!(decoder.finish().expect("EOF").is_none());
    assert_eq!(
        decoder.push(&vec![0; MAX_INPUT_CHUNK_BYTES + 1]),
        Err(SessionError::FrameTooLarge)
    );
    assert!(decoder.push(b"{}\n").is_err());
}

#[test]
fn an_expected_close_eof_waits_for_process_exit_without_reporting_a_transport_failure() {
    let mut machine = codex_ready();
    assert_eq!(
        machine.close().expect("close").effects,
        [SessionEffect::CloseInput]
    );
    machine.transport_lost();
    assert_eq!(machine.snapshot().phase, SessionPhase::Closing);
    assert_eq!(machine.snapshot().error, None);
    machine.process_exited();
    assert_eq!(machine.snapshot().phase, SessionPhase::Closed);
}

#[test]
fn extension_notifications_and_cancelled_unknown_permissions_grant_nothing() {
    let mut claude = claude_ready(false);
    claude.submit(request(1)).expect("turn");
    for value in [
        json!({"type":"keep_alive"}),
        json!({"type":"control_cancel_request","request_id":"never_granted"}),
        json!({"type":"future_notice","payload":"PRIVATE"}),
    ] {
        let ignored = feed(&mut claude, value).expect("nonsemantic extension ignored");
        assert!(ignored.effects.is_empty());
        assert!(ignored.text.is_empty());
        assert_eq!(claude.snapshot().phase, SessionPhase::Busy);
        assert_eq!(
            claude
                .snapshot()
                .active_turn
                .as_ref()
                .and_then(|turn| turn.outcome),
            None
        );
    }
    let mut codex = codex_ready();
    let ignored = feed(&mut codex, json!({"method":"future/notice"}))
        .expect("optional params on unknown notification");
    assert!(ignored.effects.is_empty());
    assert!(ignored.text.is_empty());
    assert_eq!(codex.snapshot().phase, SessionPhase::Ready);
}
