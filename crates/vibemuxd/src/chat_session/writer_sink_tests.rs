use super::*;
use crate::{
    WriterWorker,
    chat_session::{RuntimeCleanup, RuntimeStopReason},
};
use std::{collections::BTreeMap, path::PathBuf};
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_harness::{
    HarnessDetection,
    chat_session::{
        AssistantText, SessionPhase, SessionProtocol, SessionSnapshot, TurnOutcome, TurnPhase,
        TurnSummary,
    },
};
use vibemux_store::{ChatMutation, ChatOpen};
use vibemux_types::chat::{ChatHarness, ChatRecordKind, ChatState, ChatTurn, ChatTurnPhase};

// Cold private-directory ACL setup waits for the shared helper before its own
// 15s deadline. These fixtures test correlation/cleanup; deadline tests use
// their explicitly configured runtime limits.
const WRITER_FIXTURE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const PROMPT_ONE: &str = "PRIVATE_FIRST_PROMPT_中文";
const PROMPT_TWO: &str = "PRIVATE_SECOND_PROMPT_中文";
const REPLY_ONE: &str = "PRIVATE_FIRST_REPLY_中文";
const REPLY_TWO: &str = "PRIVATE_SECOND_REPLY_中文";
const VENDOR: &str = "private_fixture_vendor";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saved_progress_is_memory_only_nonblocking_and_hands_off_one_terminal_uuid() {
    use super::super::tests::progress_batch;
    use vibemux_types::chat_command_progress::CommandProgressPhase;
    use vibemux_types::chat_tool::{CommandToolRecord, CommandToolStatus, ToolRecord};
    let temp = tempfile::tempdir().expect("state");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (sink, binding) = fixture(&chats).await;
    ready(&sink, &binding).await;
    let (turn, request) = admit(&chats, &binding, PROMPT_ONE).await;
    sink.claim_turn(&binding, &request).await.expect("claim");
    assert_eq!(
        sink.try_command_progress(&binding, progress_batch(&request, 1)),
        Err(RuntimeError::WrongTurn)
    );
    sink.event(&binding, RuntimeEvent::TurnAcknowledged(summary(&turn)))
        .await
        .expect("ack");
    assert_eq!(
        sink.try_command_progress(&binding, progress_batch(&request, 1)),
        Ok(true)
    );
    let first = sink
        .command_progress(request.request_id)
        .expect("read")
        .expect("progress");
    assert_eq!(first.canonical_id, request.turn_id);
    assert_eq!(
        sink.command_progress(Uuid::new_v4())
            .expect("wrong identity"),
        None
    );
    assert!(
        chats
            .records(binding.chat_id, 0, 32)
            .await
            .expect("history")
            .iter()
            .all(|row| row.record.kind != ChatRecordKind::Tool),
        "no append before completion"
    );
    {
        let _held = sink.state.lock().expect("held memory lock");
        assert_eq!(
            sink.try_command_progress(&binding, progress_batch(&request, 2)),
            Ok(false),
            "contention skips synchronously"
        );
    }
    assert_eq!(
        sink.try_command_progress(&binding, progress_batch(&request, 3)),
        Ok(true)
    );
    let mut wrong_binding = binding.clone();
    wrong_binding.generation += 1;
    assert_eq!(
        sink.try_command_progress(&wrong_binding, progress_batch(&request, 4)),
        Err(RuntimeError::ClaimRefused)
    );
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        sink.cancel_turn(&binding, summary(&turn)),
    )
    .await
    .expect("cancel not delayed by skipped progress")
    .expect("cancel");
    let observation = ToolObservation {
        request_id: request.request_id,
        turn_id: request.turn_id,
        record: ToolRecord::Command(
            CommandToolRecord::from_observation(
                "live_command".into(),
                CommandToolStatus::Completed,
                "PRIVATE_FINAL_CHANGED".into(),
                "C:\\final_changed".into(),
                Some("final output".into()),
                Some(0),
                None,
            )
            .expect("terminal"),
        ),
    };
    sink.event(&binding, RuntimeEvent::ToolCompleted(observation.clone()))
        .await
        .expect("terminal append");
    sink.event(&binding, RuntimeEvent::ToolCompleted(observation.clone()))
        .await
        .expect("inert replay");
    let recorded = sink
        .command_progress(request.request_id)
        .expect("read")
        .expect("recorded");
    assert_eq!(recorded.entries[0].record_id, first.entries[0].record_id);
    assert_eq!(recorded.entries[0].phase, CommandProgressPhase::Recorded);
    assert_eq!(recorded.entries[0].content.command, "PRIVATE_FINAL_CHANGED");
    assert_eq!(
        sink.try_command_progress(&binding, progress_batch(&request, 5)),
        Ok(true)
    );
    assert_eq!(
        sink.command_progress(request.request_id).expect("read"),
        Some(recorded)
    );
    let rows = chats
        .records(binding.chat_id, 0, 32)
        .await
        .expect("history");
    let tools: Vec<_> = rows
        .iter()
        .filter(|row| row.record.kind == ChatRecordKind::Tool)
        .collect();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].record.record_id, first.entries[0].record_id);
    assert_eq!(
        tools[0].text.as_deref(),
        Some(
            observation
                .record
                .to_private_text()
                .expect("original body")
                .as_str()
        )
    );
    finish(&sink, &binding, &turn, REPLY_ONE).await;
    assert_eq!(
        sink.command_progress(request.request_id).expect("released"),
        None
    );
    shutdown(worker, sink, chats).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saved_progress_poison_is_an_error_distinct_from_absence() {
    let temp = tempfile::tempdir().expect("state");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (sink, binding) = fixture(&chats).await;
    assert_eq!(
        sink.command_progress(Uuid::new_v4())
            .expect("valid absence"),
        None
    );
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _held = sink.state.lock().expect("lock");
            panic!("synthetic poison");
        }))
        .is_err()
    );
    assert_eq!(
        sink.command_progress(Uuid::new_v4()),
        Err(RuntimeError::WriterUnavailable)
    );
    let request = TurnRequest {
        request_id: Uuid::new_v4(),
        turn_id: Uuid::new_v4(),
        prompt: "fixture".into(),
    };
    assert_eq!(
        sink.try_command_progress(&binding, super::super::tests::progress_batch(&request, 1)),
        Err(RuntimeError::WriterUnavailable)
    );
    shutdown(worker, sink, chats).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_change_tool_records_keep_reported_moves_and_diffs_in_private_saved_history() {
    use vibemux_harness::chat_session::ToolObservation;
    use vibemux_types::chat_file_tool::{
        FileChangeKind, FileChangeToolRecord, FileChangeToolStatus, FileToolChange,
    };
    use vibemux_types::chat_tool::ToolRecord;
    let temp = tempfile::tempdir().expect("state");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (sink, binding) = fixture(&chats).await;
    ready(&sink, &binding).await;
    let (turn, request) = admit(&chats, &binding, PROMPT_ONE).await;
    sink.claim_turn(&binding, &request).await.expect("claim");
    sink.event(&binding, RuntimeEvent::TurnAcknowledged(summary(&turn)))
        .await
        .expect("ack");
    let observation = ToolObservation {
        request_id: request.request_id,
        turn_id: request.turn_id,
        record: ToolRecord::FileChange(
            FileChangeToolRecord::from_observation(
                "private_file_call".to_owned(),
                FileChangeToolStatus::Completed,
                vec![
                    FileToolChange::from_observation(
                        "C:\\PRIVATE_before.txt".to_owned(),
                        FileChangeKind::Update,
                        Some("C:\\PRIVATE_after.txt".to_owned()),
                        "-PRIVATE_OLD\n+PRIVATE_NEW\n".to_owned(),
                    )
                    .expect("change"),
                ],
            )
            .expect("record"),
        ),
    };
    sink.event(&binding, RuntimeEvent::ToolCompleted(observation.clone()))
        .await
        .expect("private append");
    sink.event(&binding, RuntimeEvent::ToolCompleted(observation.clone()))
        .await
        .expect("inert replay");
    finish(&sink, &binding, &turn, REPLY_ONE).await;
    let records = chats
        .records(binding.chat_id, 0, 32)
        .await
        .expect("private history");
    let tools: Vec<_> = records
        .iter()
        .filter(|row| row.record.kind == ChatRecordKind::Tool)
        .collect();
    assert_eq!(tools.len(), 1);
    assert_eq!(
        ToolRecord::from_private_text(tools[0].text.as_deref().expect("body")).expect("decoded"),
        observation.record
    );
    assert_eq!(tools[0].record.turn_id, turn.turn_id);
    assert!(!format!("{observation:?} {:?}", tools[0]).contains("PRIVATE_"));
    shutdown(worker, sink, chats).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn command_tool_records_are_private_once_only_and_bound_to_the_acknowledged_saved_turn() {
    use vibemux_harness::chat_session::ToolObservation;
    use vibemux_types::chat_tool::{CommandToolRecord, CommandToolStatus, ToolRecord};

    let temp = tempfile::tempdir().expect("state");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (sink, binding) = fixture(&chats).await;
    ready(&sink, &binding).await;
    let (turn, request) = admit(&chats, &binding, PROMPT_ONE).await;
    sink.claim_turn(&binding, &request).await.expect("claim");
    let observation = ToolObservation {
        request_id: request.request_id,
        turn_id: request.turn_id,
        record: ToolRecord::Command(
            CommandToolRecord::from_observation(
                "private_command_id".to_owned(),
                CommandToolStatus::Completed,
                "PRIVATE_COMMAND_中文".to_owned(),
                "C:\\private_workspace".to_owned(),
                Some("PRIVATE_COMMAND_OUTPUT_中文".to_owned()),
                Some(0),
                Some(42),
            )
            .expect("private display record"),
        ),
    };
    assert_eq!(
        sink.event(&binding, RuntimeEvent::ToolCompleted(observation.clone()))
            .await,
        Err(RuntimeError::WrongTurn),
        "no tool before actual acknowledgement"
    );
    sink.event(&binding, RuntimeEvent::TurnAcknowledged(summary(&turn)))
        .await
        .expect("ack");
    sink.event(&binding, RuntimeEvent::ToolCompleted(observation.clone()))
        .await
        .expect("tool persisted");
    sink.event(&binding, RuntimeEvent::ToolCompleted(observation.clone()))
        .await
        .expect("inert replay");
    let mut foreign = observation.clone();
    foreign.request_id = Uuid::new_v4();
    assert_eq!(
        sink.event(&binding, RuntimeEvent::ToolCompleted(foreign))
            .await,
        Err(RuntimeError::WrongTurn)
    );
    let mut changed = observation.clone();
    let ToolRecord::Command(record) = &mut changed.record else {
        panic!("command record")
    };
    record.command = "different private command".to_owned();
    record.command_original_bytes = record.command.len() as u64;
    assert_eq!(
        sink.event(&binding, RuntimeEvent::ToolCompleted(changed))
            .await,
        Err(RuntimeError::WrongTurn)
    );
    finish(&sink, &binding, &turn, REPLY_ONE).await;
    let records = chats
        .records(binding.chat_id, 0, 32)
        .await
        .expect("private history");
    let tools: Vec<_> = records
        .iter()
        .filter(|row| row.record.kind == ChatRecordKind::Tool)
        .collect();
    assert_eq!(tools.len(), 1);
    assert_eq!(
        ToolRecord::from_private_text(tools[0].text.as_deref().expect("body")).expect("record"),
        observation.record
    );
    assert_eq!(tools[0].record.turn_id, turn.turn_id);
    assert!(!format!("{observation:?} {sink:?} {:?}", tools[0]).contains("PRIVATE_COMMAND"));
    assert_eq!(
        sink.event(&binding, RuntimeEvent::ToolCompleted(observation))
            .await,
        Err(RuntimeError::WrongTurn)
    );
    shutdown(worker, sink, chats).await;
}

async fn start(state_dir: PathBuf) -> (WriterWorker, ChatWriter) {
    let database = state_dir.join("state.sqlite3");
    let worker = tokio::task::spawn_blocking(move || {
        WriterWorker::start_with_config(
            &database,
            crate::DEFAULT_WRITER_QUEUE_CAPACITY,
            WRITER_FIXTURE_TIMEOUT,
        )
    })
    .await
    .expect("start task")
    .expect("writer");
    let writer = worker.handle().expect("writer handle");
    let seed = writer.clone();
    tokio::task::spawn_blocking(move || {
        seed.commit_harness_snapshot(
            BTreeMap::from(["codex", "claude"].map(|name| {
                (
                    name.to_owned(),
                    HarnessDetection {
                        detected: true,
                        path: None,
                        launcher: None,
                        version: Some("fixture".to_owned()),
                    },
                )
            })),
            "2026-10-31T00:00:00Z".to_owned(),
            "writer_sink_fixture".to_owned(),
        )
    })
    .await
    .expect("seed task")
    .expect("seed");
    let chats = ChatWriter::attach(writer, state_dir)
        .await
        .expect("chat writer");
    (worker, chats)
}

async fn fixture(chats: &ChatWriter) -> (ChatWriterSink, RuntimeBinding) {
    fixture_harness(chats, ChatHarness::Codex).await
}

async fn fixture_harness(
    chats: &ChatWriter,
    harness: ChatHarness,
) -> (ChatWriterSink, RuntimeBinding) {
    let session = chats
        .open(
            ChatOpen {
                operation_id: Uuid::new_v4(),
                harness,
                profile_sha256: "a".repeat(64),
                resource_key: Uuid::new_v4().simple().to_string().repeat(2),
                base_commit: "c".repeat(40),
                title_ref: None,
                timestamp: OffsetDateTime::now_utc(),
            },
            None,
        )
        .await
        .expect("open")
        .session;
    let owner = chats
        .claim_owner(session.chat_id, 1, Some(VENDOR.to_owned()))
        .await
        .expect("owner");
    let binding = RuntimeBinding {
        chat_id: session.chat_id,
        generation: 1,
        owner_fence: owner.fence,
    };
    (ChatWriterSink::new(chats.clone(), binding.clone()), binding)
}

fn snapshot(phase: SessionPhase, turn: Option<TurnSummary>) -> SessionSnapshot {
    SessionSnapshot {
        protocol: SessionProtocol::CodexAppServer,
        generation_id: Uuid::new_v4(),
        phase,
        expected_vendor_session_id: Some(VENDOR.to_owned()),
        vendor_session_id: Some(VENDOR.to_owned()),
        vendor_session_confirmed: true,
        active_turn: None,
        last_turn: turn,
        turns_started: 0,
        records_received: 0,
        record_bytes: 0,
        rpc_requests: 0,
        close_requested: false,
        error: None,
    }
}

async fn ready(sink: &ChatWriterSink, binding: &RuntimeBinding) {
    sink.event(
        binding,
        RuntimeEvent::VendorBound {
            vendor_session_id: VENDOR.to_owned(),
        },
    )
    .await
    .expect("root binding");
    sink.event(
        binding,
        RuntimeEvent::Ready(snapshot(SessionPhase::Ready, None)),
    )
    .await
    .expect("ready");
}

fn summary(turn: &ChatTurn) -> TurnSummary {
    TurnSummary {
        request_id: turn.request_id,
        turn_id: *turn.turn_id.as_uuid(),
        vendor_turn_id: Some(format!("vendor_{}", turn.request_id)),
        phase: TurnPhase::Running,
        outcome: None,
        acknowledged: true,
        cancel_requested: false,
        prompt_bytes: turn.prompt_ref.byte_count as usize,
    }
}

async fn admit(
    chats: &ChatWriter,
    binding: &RuntimeBinding,
    prompt: &str,
) -> (ChatTurn, TurnRequest) {
    let turn = chats
        .admit_turn(
            binding.chat_id,
            Uuid::new_v4(),
            binding.generation,
            "c".repeat(40),
            prompt.to_owned(),
        )
        .await
        .expect("admit")
        .turn;
    let request = TurnRequest {
        request_id: turn.request_id,
        turn_id: *turn.turn_id.as_uuid(),
        prompt: prompt.to_owned(),
    };
    (turn, request)
}

async fn finish(sink: &ChatWriterSink, binding: &RuntimeBinding, turn: &ChatTurn, text: &str) {
    let mut terminal = summary(turn);
    sink.event(binding, RuntimeEvent::TurnAcknowledged(terminal.clone()))
        .await
        .expect("acknowledged");
    for (revision, reply) in [(1, "obsolete partial"), (2, text)] {
        sink.event(
            binding,
            RuntimeEvent::AssistantSnapshot(AssistantText {
                request_id: turn.request_id,
                turn_id: *turn.turn_id.as_uuid(),
                revision,
                text: reply.to_owned(),
                original_bytes: reply.len(),
                truncated: false,
            }),
        )
        .await
        .expect("snapshot");
    }
    terminal.phase = TurnPhase::Finished;
    terminal.outcome = Some(TurnOutcome::Completed);
    sink.event(
        binding,
        RuntimeEvent::TurnFinished {
            snapshot: snapshot(SessionPhase::Ready, Some(terminal.clone())),
            turn: terminal,
            output_complete: true,
        },
    )
    .await
    .expect("terminal");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_turns_retain_one_private_final_record_each_and_content_free_audit() {
    let temp = tempfile::tempdir().expect("state");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (sink, binding) = fixture(&chats).await;
    ready(&sink, &binding).await;
    let (first, request) = admit(&chats, &binding, PROMPT_ONE).await;
    sink.claim_turn(&binding, &request)
        .await
        .expect("one use claim");
    finish(&sink, &binding, &first, REPLY_ONE).await;
    let (second, request) = admit(&chats, &binding, PROMPT_TWO).await;
    sink.claim_turn(&binding, &request)
        .await
        .expect("second claim");
    finish(&sink, &binding, &second, REPLY_TWO).await;
    assert_ne!(first.task_id, second.task_id);
    assert_ne!(first.run_id, second.run_id);
    assert_eq!(
        chats
            .turn(first.request_id)
            .await
            .expect("first")
            .expect("turn")
            .phase,
        ChatTurnPhase::Completed
    );
    let records = chats
        .records(binding.chat_id, 0, 32)
        .await
        .expect("private history");
    let bodies: Vec<_> = records
        .iter()
        .map(|record| record.text.as_deref())
        .collect();
    assert_eq!(
        bodies,
        [
            Some(PROMPT_ONE),
            Some(REPLY_ONE),
            Some(PROMPT_TWO),
            Some(REPLY_TWO)
        ]
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| record.record.kind == ChatRecordKind::Assistant)
            .count(),
        2
    );
    let writer = worker.handle().expect("writer");
    let events = tokio::task::spawn_blocking(move || writer.with_store(|store| store.events()))
        .await
        .expect("events task")
        .expect("events");
    let audit = serde_json::to_string(&events).expect("audit JSON");
    let debug = format!("{sink:?}");
    for marker in [
        PROMPT_ONE,
        PROMPT_TWO,
        REPLY_ONE,
        REPLY_TWO,
        VENDOR,
        "obsolete partial",
    ] {
        assert!(!audit.contains(marker), "private text entered audit");
        assert!(!debug.contains(marker), "private text entered Debug");
    }
    chats
        .release_owner(
            ChatMutation {
                operation_id: Uuid::new_v4(),
                chat_id: binding.chat_id,
                expected_generation: 1,
                timestamp: OffsetDateTime::now_utc(),
            },
            binding.owner_fence,
            true,
            false,
        )
        .await
        .expect("release fixture");
    drop(sink);
    drop(chats);
    tokio::task::spawn_blocking(move || worker.shutdown())
        .await
        .expect("shutdown task")
        .expect("shutdown");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let records = chats
        .records(binding.chat_id, 0, 32)
        .await
        .expect("reopened history");
    assert_eq!(
        records
            .iter()
            .map(|record| record.text.as_deref())
            .collect::<Vec<_>>(),
        [
            Some(PROMPT_ONE),
            Some(REPLY_ONE),
            Some(PROMPT_TWO),
            Some(REPLY_TWO)
        ]
    );
    drop(chats);
    tokio::task::spawn_blocking(move || worker.shutdown())
        .await
        .expect("shutdown task")
        .expect("shutdown");
}
fn cleanup(verified: bool) -> RuntimeCleanup {
    RuntimeCleanup {
        exit: vibemux_harness::dispatch::ProcessExit {
            exit_code: Some(0),
            forced_termination: false,
        },
        root_reaped: true,
        tree_empty: verified,
        cleanup_verified: true,
        stderr_bytes: 0,
    }
}

async fn shutdown(worker: WriterWorker, sink: ChatWriterSink, chats: ChatWriter) {
    drop(sink);
    drop(chats);
    tokio::task::spawn_blocking(move || worker.shutdown())
        .await
        .expect("shutdown task")
        .expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mismatched_identity_and_private_prompt_cannot_consume_the_send_claim() {
    let temp = tempfile::tempdir().expect("state");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (sink, binding) = fixture(&chats).await;
    ready(&sink, &binding).await;
    let (turn, request) = admit(&chats, &binding, PROMPT_ONE).await;
    let mut wrong_chat = binding.clone();
    wrong_chat.chat_id = vibemux_types::ChatId::new();
    let mut wrong_generation = binding.clone();
    wrong_generation.generation += 1;
    for wrong in [&wrong_chat, &wrong_generation] {
        assert_eq!(
            sink.claim_turn(wrong, &request).await,
            Err(RuntimeError::ClaimRefused)
        );
    }
    let (_, other_owner) = fixture(&chats).await;
    let mut wrong_fence = binding.clone();
    wrong_fence.owner_fence = other_owner.owner_fence;
    let forged = ChatWriterSink::new(chats.clone(), wrong_fence.clone());
    assert_eq!(
        forged.claim_turn(&wrong_fence, &request).await,
        Err(RuntimeError::ClaimRefused)
    );
    let mut wrong = request.clone();
    wrong.turn_id = Uuid::new_v4();
    assert_eq!(
        sink.claim_turn(&binding, &wrong).await,
        Err(RuntimeError::ClaimRefused)
    );
    wrong = request.clone();
    wrong.request_id = Uuid::new_v4();
    assert_eq!(
        sink.claim_turn(&binding, &wrong).await,
        Err(RuntimeError::ClaimRefused)
    );
    wrong = request.clone();
    wrong.prompt = "x".repeat(request.prompt.len());
    assert_eq!(
        sink.claim_turn(&binding, &wrong).await,
        Err(RuntimeError::ClaimRefused)
    );
    assert_eq!(
        chats
            .turn(turn.request_id)
            .await
            .expect("read")
            .expect("turn")
            .phase,
        ChatTurnPhase::Admitted
    );
    sink.claim_turn(&binding, &request)
        .await
        .expect("only matching private prompt claims");
    assert_eq!(
        sink.claim_turn(&binding, &request).await,
        Err(RuntimeError::ClaimRefused)
    );
    let duplicate = ChatWriterSink::new(chats.clone(), binding.clone());
    assert_eq!(
        duplicate.claim_turn(&binding, &request).await,
        Err(RuntimeError::ClaimRefused)
    );
    assert_eq!(
        chats
            .turn(turn.request_id)
            .await
            .expect("read")
            .expect("turn")
            .phase,
        ChatTurnPhase::Running
    );
    shutdown(worker, sink, chats).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupted_private_fingerprint_refuses_claim_before_durable_mutation() {
    let temp = tempfile::tempdir().expect("state");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (sink, binding) = fixture(&chats).await;
    ready(&sink, &binding).await;
    let (turn, request) = admit(&chats, &binding, PROMPT_ONE).await;
    let path = temp
        .path()
        .join("chat_private")
        .join(format!("{}.blob", turn.prompt_ref.blob_id.simple()));
    std::fs::write(&path, vec![b'x'; request.prompt.len()]).expect("corrupt only fixture content");
    assert_eq!(
        sink.claim_turn(&binding, &request).await,
        Err(RuntimeError::ClaimRefused)
    );
    assert_eq!(
        chats
            .turn(turn.request_id)
            .await
            .expect("turn")
            .expect("exists")
            .phase,
        ChatTurnPhase::Admitted
    );
    std::fs::write(path, request.prompt.as_bytes()).expect("restore fixture content");
    sink.claim_turn(&binding, &request)
        .await
        .expect("matching integrity restored");
    shutdown(worker, sink, chats).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_snapshot_or_claimed_completion_does_not_replace_the_acknowledgement_callback() {
    let temp = tempfile::tempdir().expect("state");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (sink, binding) = fixture(&chats).await;
    ready(&sink, &binding).await;
    let (turn, request) = admit(&chats, &binding, PROMPT_ONE).await;
    sink.claim_turn(&binding, &request).await.expect("claim");
    let mut claimed_terminal = summary(&turn);
    claimed_terminal.phase = TurnPhase::Finished;
    claimed_terminal.outcome = Some(TurnOutcome::Completed);
    let result = sink
        .event(
            &binding,
            RuntimeEvent::TurnFinished {
                turn: claimed_terminal.clone(),
                snapshot: snapshot(SessionPhase::Ready, Some(claimed_terminal)),
                output_complete: true,
            },
        )
        .await;
    assert_eq!(result, Err(RuntimeError::WriterUnavailable));
    let stored = chats
        .turn(turn.request_id)
        .await
        .expect("read")
        .expect("turn");
    assert_eq!(stored.phase, ChatTurnPhase::Running);
    assert_eq!(stored.outcome, None);
    assert!(!stored.correlated_terminal);
    sink.event(
        &binding,
        RuntimeEvent::TransportEnded {
            snapshot: snapshot(SessionPhase::RecoveryRequired, None),
            pending_turn: None,
            reason: RuntimeStopReason::WriterUnavailable,
            cleanup: cleanup(false),
        },
    )
    .await
    .expect("unknown cleanup");
    assert_eq!(
        chats
            .turn(turn.request_id)
            .await
            .expect("turn")
            .expect("exists")
            .phase,
        ChatTurnPhase::RecoveryRequired
    );
    assert_eq!(
        chats
            .session(binding.chat_id)
            .await
            .expect("read")
            .expect("session")
            .active_request_id,
        Some(turn.request_id)
    );
    shutdown(worker, sink, chats).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_cleanup_preserves_partial_reply_and_active_reservation() {
    let temp = tempfile::tempdir().expect("state");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (sink, binding) = fixture(&chats).await;
    ready(&sink, &binding).await;
    let (turn, request) = admit(&chats, &binding, PROMPT_ONE).await;
    sink.claim_turn(&binding, &request).await.expect("claim");
    sink.event(
        &binding,
        RuntimeEvent::AssistantSnapshot(AssistantText {
            request_id: turn.request_id,
            turn_id: *turn.turn_id.as_uuid(),
            revision: 1,
            text: REPLY_ONE.to_owned(),
            original_bytes: REPLY_ONE.len(),
            truncated: false,
        }),
    )
    .await
    .expect("partial");
    let mut observed = summary(&turn);
    observed.acknowledged = false;
    observed.vendor_turn_id = None;
    observed.phase = TurnPhase::Uncertain;
    let mut ended = snapshot(SessionPhase::RecoveryRequired, None);
    ended.active_turn = Some(observed);
    sink.event(
        &binding,
        RuntimeEvent::TransportEnded {
            snapshot: ended,
            pending_turn: None,
            reason: RuntimeStopReason::ProcessLost,
            cleanup: cleanup(false),
        },
    )
    .await
    .expect("unknown end recorded");
    let stored = chats
        .turn(turn.request_id)
        .await
        .expect("read")
        .expect("turn");
    assert_eq!(stored.phase, ChatTurnPhase::RecoveryRequired);
    assert_eq!(stored.outcome, None);
    assert!(!stored.output_complete);
    let session = chats
        .session(binding.chat_id)
        .await
        .expect("read")
        .expect("session");
    assert_eq!(session.state, ChatState::RecoveryRequired);
    assert_eq!(session.active_request_id, Some(turn.request_id));
    let records = chats
        .records(binding.chat_id, 0, 32)
        .await
        .expect("retained partial");
    assert_eq!(records.len(), 2);
    assert_eq!(records[1].text.as_deref(), Some(REPLY_ONE));
    assert!(
        chats
            .admit_turn(
                binding.chat_id,
                Uuid::new_v4(),
                1,
                "c".repeat(40),
                PROMPT_TWO.to_owned()
            )
            .await
            .is_err()
    );
    assert!(
        chats
            .claim_owner(binding.chat_id, 1, Some(VENDOR.to_owned()))
            .await
            .is_err()
    );
    shutdown(worker, sink, chats).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn verified_cleanup_interrupts_unconfirmed_work_without_inventing_success() {
    let temp = tempfile::tempdir().expect("state");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (sink, binding) = fixture(&chats).await;
    ready(&sink, &binding).await;
    let (turn, request) = admit(&chats, &binding, PROMPT_ONE).await;
    sink.claim_turn(&binding, &request).await.expect("claim");
    sink.event(
        &binding,
        RuntimeEvent::TransportEnded {
            snapshot: snapshot(SessionPhase::Suspended, None),
            pending_turn: Some(summary(&turn)),
            reason: RuntimeStopReason::ProcessLost,
            cleanup: cleanup(true),
        },
    )
    .await
    .expect("verified end");
    let stored = chats
        .turn(turn.request_id)
        .await
        .expect("read")
        .expect("turn");
    assert_eq!(stored.phase, ChatTurnPhase::Interrupted);
    assert_eq!(stored.outcome, Some(ChatTurnOutcome::Interrupted));
    assert!(!stored.correlated_terminal);
    assert!(!stored.output_complete);
    let session = chats
        .session(binding.chat_id)
        .await
        .expect("read")
        .expect("session");
    assert_eq!(session.state, ChatState::Suspended);
    assert_eq!(session.active_request_id, None);
    let resumed = chats
        .resume(ChatMutation {
            operation_id: Uuid::new_v4(),
            chat_id: binding.chat_id,
            expected_generation: 1,
            timestamp: OffsetDateTime::now_utc(),
        })
        .await
        .expect("verified owner can resume");
    assert_eq!(resumed.session.generation, 2);
    assert!(
        chats
            .claim_owner(binding.chat_id, 2, Some(VENDOR.to_owned()))
            .await
            .is_ok()
    );
    shutdown(worker, sink, chats).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_owner_loss_keeps_completed_turn_and_reserves_uncertain_process() {
    let temp = tempfile::tempdir().expect("state");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (sink, binding) = fixture(&chats).await;
    ready(&sink, &binding).await;
    let (turn, request) = admit(&chats, &binding, PROMPT_ONE).await;
    sink.claim_turn(&binding, &request).await.expect("claim");
    finish(&sink, &binding, &turn, REPLY_ONE).await;
    sink.event(
        &binding,
        RuntimeEvent::TransportEnded {
            snapshot: snapshot(SessionPhase::Suspended, None),
            pending_turn: None,
            reason: RuntimeStopReason::Idle,
            cleanup: cleanup(false),
        },
    )
    .await
    .expect("idle loss");
    assert_eq!(
        chats
            .turn(turn.request_id)
            .await
            .expect("read")
            .expect("turn")
            .phase,
        ChatTurnPhase::Completed
    );
    assert_eq!(
        chats
            .session(binding.chat_id)
            .await
            .expect("read")
            .expect("session")
            .state,
        ChatState::RecoveryRequired
    );
    assert!(
        chats
            .claim_owner(binding.chat_id, 1, Some(VENDOR.to_owned()))
            .await
            .is_err()
    );
    shutdown(worker, sink, chats).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn verified_idle_and_shutdown_suspend_while_explicit_close_closes() {
    for (reason, expected) in [
        (RuntimeStopReason::Idle, ChatState::Suspended),
        (RuntimeStopReason::Shutdown, ChatState::Suspended),
        (RuntimeStopReason::Closed, ChatState::Closed),
    ] {
        let temp = tempfile::tempdir().expect("state");
        let (worker, chats) = start(temp.path().to_owned()).await;
        let (sink, binding) = fixture(&chats).await;
        ready(&sink, &binding).await;
        sink.event(
            &binding,
            RuntimeEvent::TransportEnded {
                snapshot: snapshot(SessionPhase::Closed, None),
                pending_turn: None,
                reason,
                cleanup: cleanup(true),
            },
        )
        .await
        .expect("verified release");
        assert_eq!(
            chats
                .session(binding.chat_id)
                .await
                .expect("read")
                .expect("session")
                .state,
            expected
        );
        assert_eq!(
            sink.event(
                &binding,
                RuntimeEvent::Ready(snapshot(SessionPhase::Ready, None))
            )
            .await,
            Err(RuntimeError::Stopped)
        );
        shutdown(worker, sink, chats).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claimed_success_and_output_availability_remain_separate() {
    for mode in ["runtime_gap", "truncated", "private_unavailable"] {
        let temp = tempfile::tempdir().expect("state");
        let (worker, chats) = start(temp.path().to_owned()).await;
        let (sink, binding) = fixture(&chats).await;
        ready(&sink, &binding).await;
        let (turn, request) = admit(&chats, &binding, PROMPT_ONE).await;
        sink.claim_turn(&binding, &request).await.expect("claim");
        let mut terminal = summary(&turn);
        sink.event(&binding, RuntimeEvent::TurnAcknowledged(terminal.clone()))
            .await
            .expect("ack");
        sink.event(
            &binding,
            RuntimeEvent::AssistantSnapshot(AssistantText {
                request_id: turn.request_id,
                turn_id: *turn.turn_id.as_uuid(),
                revision: 1,
                text: REPLY_ONE.to_owned(),
                original_bytes: REPLY_ONE.len() + usize::from(mode == "truncated"),
                truncated: mode == "truncated",
            }),
        )
        .await
        .expect("snapshot");
        let marker = temp.path().join("chat_private").join("owner.json");
        let restore = if mode == "private_unavailable" {
            let bytes = std::fs::read(&marker).expect("fixture marker");
            std::fs::write(&marker, b"{}").expect("fixture protection failure");
            Some(bytes)
        } else {
            None
        };
        terminal.phase = TurnPhase::Finished;
        terminal.outcome = Some(TurnOutcome::Completed);
        sink.event(
            &binding,
            RuntimeEvent::TurnFinished {
                snapshot: snapshot(SessionPhase::Ready, Some(terminal.clone())),
                turn: terminal,
                output_complete: mode != "runtime_gap",
            },
        )
        .await
        .expect("correlated outcome");
        if let Some(bytes) = restore {
            std::fs::write(marker, bytes).expect("restore fixture marker");
        }
        let stored = chats
            .turn(turn.request_id)
            .await
            .expect("read")
            .expect("turn");
        assert_eq!(stored.phase, ChatTurnPhase::Completed, "{mode}");
        assert_eq!(stored.outcome, Some(ChatTurnOutcome::Completed), "{mode}");
        assert_eq!(stored.error_code, None, "{mode}");
        assert!(stored.correlated_terminal, "{mode}");
        assert!(!stored.output_complete, "{mode}");
        let records = chats
            .records(binding.chat_id, 0, 32)
            .await
            .expect("history");
        assert_eq!(
            records.len(),
            if mode == "private_unavailable" { 1 } else { 2 }
        );
        shutdown(worker, sink, chats).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn verified_unsent_teardown_is_fenced_and_cancels_without_claiming() {
    let temp = tempfile::tempdir().expect("state");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (sink, binding) = fixture(&chats).await;
    ready(&sink, &binding).await;
    let (turn, _request) = admit(&chats, &binding, PROMPT_ONE).await;
    let (_, other_owner) = fixture(&chats).await;
    let mut wrong = binding.clone();
    wrong.owner_fence = other_owner.owner_fence;
    let forged = ChatWriterSink::new(chats.clone(), wrong.clone());
    assert_eq!(
        forged
            .event(
                &wrong,
                RuntimeEvent::TransportEnded {
                    snapshot: snapshot(SessionPhase::Closed, None),
                    pending_turn: None,
                    reason: RuntimeStopReason::Closed,
                    cleanup: cleanup(true)
                }
            )
            .await,
        Err(RuntimeError::WriterUnavailable)
    );
    assert_eq!(
        chats
            .turn(turn.request_id)
            .await
            .expect("read")
            .expect("turn")
            .phase,
        ChatTurnPhase::Admitted
    );
    sink.event(
        &binding,
        RuntimeEvent::TransportEnded {
            snapshot: snapshot(SessionPhase::Closed, None),
            pending_turn: None,
            reason: RuntimeStopReason::Closed,
            cleanup: cleanup(true),
        },
    )
    .await
    .expect("verified unsent cancellation");
    assert_eq!(
        chats
            .turn(turn.request_id)
            .await
            .expect("read")
            .expect("turn")
            .phase,
        ChatTurnPhase::Cancelled
    );
    assert_eq!(
        chats
            .session(binding.chat_id)
            .await
            .expect("read")
            .expect("session")
            .state,
        ChatState::Closed
    );
    shutdown(worker, sink, chats).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_terminal_keeps_its_fence_until_the_same_owner_reports_verified_cleanup() {
    let temp = tempfile::tempdir().expect("state");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (sink, binding) = fixture(&chats).await;
    ready(&sink, &binding).await;
    let (turn, request) = admit(&chats, &binding, PROMPT_ONE).await;
    sink.claim_turn(&binding, &request).await.expect("claim");
    sink.event(
        &binding,
        RuntimeEvent::AssistantSnapshot(AssistantText {
            request_id: turn.request_id,
            turn_id: *turn.turn_id.as_uuid(),
            revision: 1,
            text: REPLY_ONE.to_owned(),
            original_bytes: REPLY_ONE.len(),
            truncated: false,
        }),
    )
    .await
    .expect("partial");
    let mut terminal = summary(&turn);
    terminal.phase = TurnPhase::Finished;
    terminal.outcome = Some(TurnOutcome::Completed);
    assert_eq!(
        sink.event(
            &binding,
            RuntimeEvent::TurnFinished {
                snapshot: snapshot(SessionPhase::Ready, Some(terminal.clone())),
                turn: terminal,
                output_complete: true
            }
        )
        .await,
        Err(RuntimeError::WriterUnavailable)
    );
    assert_eq!(
        chats
            .turn(turn.request_id)
            .await
            .expect("read")
            .expect("turn")
            .phase,
        ChatTurnPhase::Running
    );
    sink.event(
        &binding,
        RuntimeEvent::TransportEnded {
            snapshot: snapshot(SessionPhase::Suspended, None),
            pending_turn: None,
            reason: RuntimeStopReason::WriterUnavailable,
            cleanup: cleanup(true),
        },
    )
    .await
    .expect("matching positive cleanup");
    let stored = chats
        .turn(turn.request_id)
        .await
        .expect("read")
        .expect("turn");
    assert_eq!(stored.phase, ChatTurnPhase::Interrupted);
    assert_eq!(stored.outcome, Some(ChatTurnOutcome::Interrupted));
    assert!(!stored.correlated_terminal);
    assert_eq!(
        chats
            .session(binding.chat_id)
            .await
            .expect("read")
            .expect("session")
            .state,
        ChatState::Suspended
    );
    assert_eq!(
        chats
            .records(binding.chat_id, 0, 32)
            .await
            .expect("history")[1]
            .text
            .as_deref(),
        Some(REPLY_ONE)
    );
    shutdown(worker, sink, chats).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_cleanup_proof_is_required_before_owner_release() {
    for missing in ["root", "tree", "verified"] {
        let temp = tempfile::tempdir().expect("state");
        let (worker, chats) = start(temp.path().to_owned()).await;
        let (sink, binding) = fixture(&chats).await;
        ready(&sink, &binding).await;
        let mut proof = cleanup(true);
        match missing {
            "root" => proof.root_reaped = false,
            "tree" => proof.tree_empty = false,
            _ => proof.cleanup_verified = false,
        }
        sink.event(
            &binding,
            RuntimeEvent::TransportEnded {
                snapshot: snapshot(SessionPhase::Closed, None),
                pending_turn: None,
                reason: RuntimeStopReason::Closed,
                cleanup: proof,
            },
        )
        .await
        .expect("unknown ownership recorded");
        assert_eq!(
            chats
                .session(binding.chat_id)
                .await
                .expect("read")
                .expect("session")
                .state,
            ChatState::RecoveryRequired,
            "{missing}"
        );
        assert!(
            chats
                .resume(ChatMutation {
                    operation_id: Uuid::new_v4(),
                    chat_id: binding.chat_id,
                    expected_generation: 1,
                    timestamp: OffsetDateTime::now_utc()
                })
                .await
                .is_err()
        );
        shutdown(worker, sink, chats).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_acknowledgement_uses_its_exact_input_uuid_binding() {
    let temp = tempfile::tempdir().expect("state");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (sink, binding) = fixture_harness(&chats, ChatHarness::Claude).await;
    ready(&sink, &binding).await;
    let (turn, request) = admit(&chats, &binding, PROMPT_ONE).await;
    sink.claim_turn(&binding, &request).await.expect("claim");
    let mut acknowledged = summary(&turn);
    acknowledged.vendor_turn_id = None;
    sink.event(
        &binding,
        RuntimeEvent::TurnAcknowledged(acknowledged.clone()),
    )
    .await
    .expect("Claude exact input acknowledgement");
    assert_eq!(
        chats
            .turn(turn.request_id)
            .await
            .expect("read")
            .expect("turn")
            .vendor_turn_id,
        Some(turn.request_id.to_string())
    );
    acknowledged.phase = TurnPhase::Finished;
    acknowledged.outcome = Some(TurnOutcome::Completed);
    let mut final_snapshot = snapshot(SessionPhase::Ready, Some(acknowledged.clone()));
    final_snapshot.protocol = SessionProtocol::ClaudeStreamJson;
    sink.event(
        &binding,
        RuntimeEvent::TurnFinished {
            turn: acknowledged,
            snapshot: final_snapshot,
            output_complete: true,
        },
    )
    .await
    .expect("Claude terminal");
    assert_eq!(
        chats
            .turn(turn.request_id)
            .await
            .expect("read")
            .expect("turn")
            .phase,
        ChatTurnPhase::Completed
    );
    shutdown(worker, sink, chats).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_text_revisions_and_wrong_turn_acknowledgements_never_replace_history() {
    let temp = tempfile::tempdir().expect("state");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (sink, binding) = fixture(&chats).await;
    ready(&sink, &binding).await;
    let (turn, request) = admit(&chats, &binding, PROMPT_ONE).await;
    sink.claim_turn(&binding, &request).await.expect("claim");
    let mut wrong = summary(&turn);
    wrong.turn_id = Uuid::new_v4();
    assert_eq!(
        sink.event(&binding, RuntimeEvent::TurnAcknowledged(wrong))
            .await,
        Err(RuntimeError::WrongTurn)
    );
    assert_eq!(
        chats
            .turn(turn.request_id)
            .await
            .expect("read")
            .expect("turn")
            .vendor_turn_id,
        None
    );
    let latest = AssistantText {
        request_id: turn.request_id,
        turn_id: *turn.turn_id.as_uuid(),
        revision: 2,
        text: REPLY_ONE.to_owned(),
        original_bytes: REPLY_ONE.len(),
        truncated: false,
    };
    sink.event(&binding, RuntimeEvent::AssistantSnapshot(latest.clone()))
        .await
        .expect("latest");
    sink.event(&binding, RuntimeEvent::AssistantSnapshot(latest.clone()))
        .await
        .expect("exact duplicate snapshot");
    let mut stale = latest.clone();
    stale.revision = 1;
    assert_eq!(
        sink.event(&binding, RuntimeEvent::AssistantSnapshot(stale))
            .await,
        Err(RuntimeError::WrongTurn)
    );
    let mut changed = latest;
    changed.text = REPLY_TWO.to_owned();
    changed.original_bytes = changed.text.len();
    assert_eq!(
        sink.event(&binding, RuntimeEvent::AssistantSnapshot(changed))
            .await,
        Err(RuntimeError::WrongTurn)
    );
    let mut terminal = summary(&turn);
    sink.event(&binding, RuntimeEvent::TurnAcknowledged(terminal.clone()))
        .await
        .expect("ack");
    terminal.phase = TurnPhase::Finished;
    terminal.outcome = Some(TurnOutcome::Completed);
    sink.event(
        &binding,
        RuntimeEvent::TurnFinished {
            snapshot: snapshot(SessionPhase::Ready, Some(terminal.clone())),
            turn: terminal,
            output_complete: true,
        },
    )
    .await
    .expect("terminal");
    let records = chats
        .records(binding.chat_id, 0, 32)
        .await
        .expect("private history");
    assert_eq!(records.len(), 2);
    assert_eq!(records[1].text.as_deref(), Some(REPLY_ONE));
    shutdown(worker, sink, chats).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_text_is_bounded_private_and_scoped_to_only_the_current_request() {
    let temp = tempfile::tempdir().expect("state");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (sink, binding) = fixture(&chats).await;
    ready(&sink, &binding).await;
    let (first, request) = admit(&chats, &binding, PROMPT_ONE).await;
    sink.claim_turn(&binding, &request).await.expect("claim");
    assert_eq!(sink.live_text(first.request_id), None);
    let text = AssistantText {
        request_id: first.request_id,
        turn_id: *first.turn_id.as_uuid(),
        revision: 1,
        text: REPLY_ONE.to_owned(),
        original_bytes: REPLY_ONE.len(),
        truncated: false,
    };
    sink.event(&binding, RuntimeEvent::AssistantSnapshot(text.clone()))
        .await
        .expect("partial");
    assert_eq!(sink.live_text(first.request_id), Some(text));
    assert_eq!(sink.live_text(Uuid::new_v4()), None);
    assert!(!format!("{sink:?}").contains(REPLY_ONE));
    let oversized = AssistantText {
        request_id: first.request_id,
        turn_id: *first.turn_id.as_uuid(),
        revision: 2,
        text: "x".repeat(MAX_CHAT_RECORD_BYTES as usize + 1),
        original_bytes: MAX_CHAT_RECORD_BYTES as usize + 1,
        truncated: false,
    };
    assert_eq!(
        sink.event(&binding, RuntimeEvent::AssistantSnapshot(oversized))
            .await,
        Err(RuntimeError::ContentUnavailable)
    );
    assert_eq!(
        sink.live_text(first.request_id)
            .expect("bounded previous snapshot")
            .text,
        REPLY_ONE
    );
    let mut terminal = summary(&first);
    sink.event(&binding, RuntimeEvent::TurnAcknowledged(terminal.clone()))
        .await
        .expect("ack");
    terminal.phase = TurnPhase::Finished;
    terminal.outcome = Some(TurnOutcome::Completed);
    sink.event(
        &binding,
        RuntimeEvent::TurnFinished {
            snapshot: snapshot(SessionPhase::Ready, Some(terminal.clone())),
            turn: terminal,
            output_complete: true,
        },
    )
    .await
    .expect("terminal");
    assert_eq!(sink.live_text(first.request_id), None);
    assert!(
        !chats
            .turn(first.request_id)
            .await
            .expect("read")
            .expect("turn")
            .output_complete
    );
    let (second, request) = admit(&chats, &binding, PROMPT_TWO).await;
    sink.claim_turn(&binding, &request)
        .await
        .expect("next claim");
    assert_eq!(sink.live_text(first.request_id), None);
    assert_eq!(sink.live_text(second.request_id), None);
    sink.event(
        &binding,
        RuntimeEvent::AssistantSnapshot(AssistantText {
            request_id: second.request_id,
            turn_id: *second.turn_id.as_uuid(),
            revision: 1,
            text: REPLY_TWO.to_owned(),
            original_bytes: REPLY_TWO.len(),
            truncated: false,
        }),
    )
    .await
    .expect("second partial");
    assert_eq!(sink.live_text(first.request_id), None);
    assert_eq!(
        sink.live_text(second.request_id).expect("second only").text,
        REPLY_TWO
    );
    sink.event(
        &binding,
        RuntimeEvent::TransportEnded {
            snapshot: snapshot(SessionPhase::Suspended, None),
            pending_turn: None,
            reason: RuntimeStopReason::ProcessLost,
            cleanup: cleanup(false),
        },
    )
    .await
    .expect("unknown end");
    assert_eq!(sink.live_text(second.request_id), None);
    shutdown(worker, sink, chats).await;
}
