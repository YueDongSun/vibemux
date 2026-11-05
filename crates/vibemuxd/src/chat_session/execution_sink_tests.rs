use super::*;
use crate::{WriterWorker, chat_writer::ChatExecutionPrompt};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};
use vibemux_harness::{
    HarnessDetection,
    chat_session::{SessionProtocol, TurnRequest},
    dispatch::ProcessExit,
};
use vibemux_store::{
    ChatExecutionOwnerOpen, ChatExecutionStart, ChatOwnerChange, ChatOwnerMutation,
};
use vibemux_types::chat::ChatHarness;
use vibemux_types::{ChatId, RunId, RunStatus, TaskStatus, a2a::RunWorkspace};

use super::super::RuntimeCleanup;

// Cold ACL setup on a fresh database needs a longer writer timeout than the
// production default; only these fixture tests use it.
const WRITER_FIXTURE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const BASE_COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PROMPT_ONE: &str = "PRIVATE_EXEC_FIRST_PROMPT_中文";
const PROMPT_TWO: &str = "PRIVATE_EXEC_SECOND_PROMPT_中文";
const REPLY_ONE: &str = "PRIVATE_EXEC_FIRST_REPLY_中文";
const REPLY_TWO: &str = "PRIVATE_EXEC_SECOND_REPLY_中文";
const VENDOR: &str = "private_fixture_execution_vendor";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_progress_is_memory_only_and_file_completion_reuses_the_reserved_uuid() {
    use super::super::tests::progress_batch;
    use vibemux_types::chat_file_tool::{FileChangeToolRecord, FileChangeToolStatus};
    use vibemux_types::chat_tool::ToolRecord;
    let temp = tempfile::tempdir().expect("state");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (sink, binding) = fixture(&chats).await;
    ready(&sink, &binding).await;
    let request = admit(&chats, &binding, PROMPT_ONE).await;
    sink.claim_turn(&binding, &request).await.expect("claim");
    sink.event(&binding, RuntimeEvent::TurnAcknowledged(summary(&request)))
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
            .execution_records(binding.chat_id, 0, 32)
            .await
            .expect("history")
            .iter()
            .all(|row| row.record.kind != ChatRecordKind::Tool)
    );
    {
        let _held = sink.state.lock().expect("held memory lock");
        assert_eq!(
            sink.try_command_progress(&binding, progress_batch(&request, 2)),
            Ok(false)
        );
    }
    let mut wrong = binding.clone();
    wrong.generation += 1;
    assert_eq!(
        sink.try_command_progress(&wrong, progress_batch(&request, 3)),
        Err(RuntimeError::ClaimRefused)
    );
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        sink.cancel_turn(&binding, summary(&request)),
    )
    .await
    .expect("cancel not delayed")
    .expect("cancel");
    let observation = ToolObservation {
        request_id: request.request_id,
        turn_id: request.turn_id,
        record: ToolRecord::FileChange(
            FileChangeToolRecord::from_observation(
                "live_command".into(),
                FileChangeToolStatus::Completed,
                vec![],
            )
            .expect("file completion"),
        ),
    };
    sink.event(&binding, RuntimeEvent::ToolCompleted(observation.clone()))
        .await
        .expect("append");
    let retired = sink
        .command_progress(request.request_id)
        .expect("read")
        .expect("retired snapshot");
    assert!(retired.entries.is_empty());
    assert!(retired.revision > first.revision);
    assert_eq!(
        sink.try_command_progress(&binding, progress_batch(&request, 4)),
        Ok(true)
    );
    assert_eq!(
        sink.command_progress(request.request_id)
            .expect("late ignored"),
        Some(retired)
    );
    let records = chats
        .execution_records(binding.chat_id, 0, 32)
        .await
        .expect("history");
    let tools: Vec<_> = records
        .iter()
        .filter(|row| row.record.kind == ChatRecordKind::Tool)
        .collect();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].record.record_id, first.entries[0].record_id);
    assert_eq!(
        tools[0].text.as_deref(),
        Some(observation.record.to_private_text().expect("body").as_str())
    );
    finish_flow(&sink, &binding, &request, REPLY_ONE).await;
    assert_eq!(
        sink.command_progress(request.request_id).expect("released"),
        None
    );
    end(&sink, &binding, RuntimeStopReason::Closed, true, None).await;
    let _ = worker.handle().expect("writer alive");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_progress_poison_is_an_error_distinct_from_absence() {
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
    let _ = worker.handle().expect("writer alive");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_change_tool_records_stay_bound_to_the_workspace_attempt() {
    use vibemux_harness::chat_session::ToolObservation;
    use vibemux_types::chat_file_tool::{
        FileChangeKind, FileChangeToolRecord, FileChangeToolStatus, FileToolChange,
    };
    use vibemux_types::chat_tool::ToolRecord;
    let temp = tempfile::tempdir().expect("state");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (sink, binding) = fixture(&chats).await;
    ready(&sink, &binding).await;
    let request = admit(&chats, &binding, PROMPT_ONE).await;
    sink.claim_turn(&binding, &request).await.expect("claim");
    sink.event(&binding, RuntimeEvent::TurnAcknowledged(summary(&request)))
        .await
        .expect("ack");
    let observation = ToolObservation {
        request_id: request.request_id,
        turn_id: request.turn_id,
        record: ToolRecord::FileChange(
            FileChangeToolRecord::from_observation(
                "workspace_file_call".to_owned(),
                FileChangeToolStatus::Declined,
                vec![
                    FileToolChange::from_observation(
                        "PRIVATE_reported_delete.txt".to_owned(),
                        FileChangeKind::Delete,
                        None,
                        "-PRIVATE_CONTENT\n".to_owned(),
                    )
                    .expect("change"),
                ],
            )
            .expect("record"),
        ),
    };
    let mut foreign = observation.clone();
    foreign.turn_id = Uuid::new_v4();
    assert_eq!(
        sink.event(&binding, RuntimeEvent::ToolCompleted(foreign))
            .await,
        Err(RuntimeError::WrongTurn)
    );
    sink.event(&binding, RuntimeEvent::ToolCompleted(observation.clone()))
        .await
        .expect("private append");
    sink.event(&binding, RuntimeEvent::ToolCompleted(observation.clone()))
        .await
        .expect("duplicate");
    finish_flow(&sink, &binding, &request, REPLY_ONE).await;
    let records = chats
        .execution_records(binding.chat_id, 0, 32)
        .await
        .expect("private history");
    let tools: Vec<_> = records
        .iter()
        .filter(|row| row.record.kind == ChatRecordKind::Tool)
        .collect();
    assert_eq!(tools.len(), 1);
    assert_eq!(*tools[0].record.attempt_id.as_uuid(), request.turn_id);
    assert_eq!(
        ToolRecord::from_private_text(tools[0].text.as_deref().expect("body")).expect("decoded"),
        observation.record
    );
    assert_eq!(
        chats
            .execution_attempt(request.request_id)
            .await
            .expect("read")
            .expect("attempt")
            .outcome,
        Some(ChatTurnOutcome::Completed)
    );
    end(&sink, &binding, RuntimeStopReason::Closed, true, None).await;
    let _ = worker.handle().expect("writer alive");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn command_tool_records_use_the_workspace_attempt_and_survive_process_release() {
    use vibemux_harness::chat_session::ToolObservation;
    use vibemux_types::chat_tool::{CommandToolRecord, CommandToolStatus, ToolRecord};

    let temp = tempfile::tempdir().expect("state");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (sink, binding) = fixture(&chats).await;
    ready(&sink, &binding).await;
    let request = admit(&chats, &binding, PROMPT_ONE).await;
    sink.claim_turn(&binding, &request).await.expect("claim");
    sink.event(&binding, RuntimeEvent::TurnAcknowledged(summary(&request)))
        .await
        .expect("ack");
    let observation = ToolObservation {
        request_id: request.request_id,
        turn_id: request.turn_id,
        record: ToolRecord::Command(
            CommandToolRecord::from_observation(
                "workspace_command".to_owned(),
                CommandToolStatus::Failed,
                "PRIVATE_WORKSPACE_COMMAND".to_owned(),
                "C:\\private_workspace".to_owned(),
                Some("PRIVATE_WORKSPACE_OUTPUT".to_owned()),
                Some(7),
                Some(12),
            )
            .expect("record"),
        ),
    };
    let mut foreign = observation.clone();
    foreign.turn_id = Uuid::new_v4();
    assert_eq!(
        sink.event(&binding, RuntimeEvent::ToolCompleted(foreign))
            .await,
        Err(RuntimeError::WrongTurn)
    );
    sink.event(&binding, RuntimeEvent::ToolCompleted(observation.clone()))
        .await
        .expect("persist");
    sink.event(&binding, RuntimeEvent::ToolCompleted(observation.clone()))
        .await
        .expect("duplicate");
    finish_flow(&sink, &binding, &request, REPLY_ONE).await;
    let records = chats
        .execution_records(binding.chat_id, 0, 32)
        .await
        .expect("history");
    let tools: Vec<_> = records
        .iter()
        .filter(|row| row.record.kind == ChatRecordKind::Tool)
        .collect();
    assert_eq!(tools.len(), 1);
    assert_eq!(*tools[0].record.attempt_id.as_uuid(), request.turn_id);
    assert_eq!(
        ToolRecord::from_private_text(tools[0].text.as_deref().expect("body")).expect("parsed"),
        observation.record
    );
    assert_eq!(
        chats
            .execution_attempt(request.request_id)
            .await
            .expect("read")
            .expect("attempt")
            .outcome,
        Some(ChatTurnOutcome::Completed),
        "command failure does not invent turn failure"
    );
    end(&sink, &binding, RuntimeStopReason::Closed, true, None).await;
    let _ = worker.handle().expect("writer alive");
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
            "execution_sink_fixture".to_owned(),
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

/// Metadata-only fixture workspace, structurally bound to the lifetime run
/// exactly as the owner validator requires (branch and path tail carry the
/// run id, base commit matches); no Git or process proof is claimed.
fn fixture_workspace(run_id: &RunId, base_commit: &str) -> RunWorkspace {
    RunWorkspace {
        path: format!(r"C:\fixture\{run_id}"),
        branch: format!("codex/run_{}", run_id.as_uuid().simple()),
        base_commit: base_commit.to_owned(),
        ownership_token: "fixture_owner_token_1".to_owned(),
    }
}

async fn owner_change(
    chats: &ChatWriter,
    chat_id: ChatId,
    expected_version: u64,
    change: ChatOwnerChange,
) -> u64 {
    chats
        .change_execution_owner(
            ChatOwnerMutation {
                operation_id: Uuid::new_v4(),
                chat_id,
                expected_version,
                timestamp: OffsetDateTime::now_utc(),
            },
            change,
        )
        .await
        .expect("owner change")
        .owner
        .version
}

/// Opens an execution owner, settles its metadata fixture workspace, and
/// starts the process generation; the returned sink owns the live fence.
async fn fixture(chats: &ChatWriter) -> (ChatExecutionSink, RuntimeBinding) {
    let opened = chats
        .open_execution_owner(
            ChatExecutionOwnerOpen {
                operation_id: Uuid::new_v4(),
                harness: ChatHarness::Codex,
                profile_sha256: "a".repeat(64),
                base_commit: BASE_COMMIT.to_owned(),
                admitted_tools: BTreeSet::new(),
                title_ref: None,
                timestamp: OffsetDateTime::now_utc(),
            },
            None,
        )
        .await
        .expect("open owner");
    let chat_id = opened.owner.chat_id;
    let resource_key = Uuid::new_v4().simple().to_string().repeat(2);
    let version = owner_change(
        chats,
        chat_id,
        opened.owner.version,
        ChatOwnerChange::Plan {
            workspace: fixture_workspace(&opened.run.run_id(), BASE_COMMIT),
            resource_key: resource_key.clone(),
        },
    )
    .await;
    let version = owner_change(chats, chat_id, version, ChatOwnerChange::ClaimCreation).await;
    let version = owner_change(chats, chat_id, version, ChatOwnerChange::VerifyWorkspace).await;
    let started = chats
        .start_execution_process(ChatExecutionStart {
            operation_id: Uuid::new_v4(),
            chat_id,
            expected_generation: opened.session.generation,
            expected_owner_version: version,
            profile_sha256: "a".repeat(64),
            resource_key,
            observed_commit: BASE_COMMIT.to_owned(),
            expected_vendor_id: None,
            timestamp: OffsetDateTime::now_utc(),
        })
        .await
        .expect("start process");
    let fence = started.fence.expect("process fence");
    let binding = RuntimeBinding {
        chat_id,
        generation: started.session.generation,
        owner_fence: fence,
    };
    (
        ChatExecutionSink::new(chats.clone(), binding.clone()),
        binding,
    )
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

async fn ready(sink: &ChatExecutionSink, binding: &RuntimeBinding) {
    sink.event(
        binding,
        RuntimeEvent::VendorBound {
            vendor_session_id: VENDOR.to_owned(),
        },
    )
    .await
    .expect("vendor binding");
    sink.event(
        binding,
        RuntimeEvent::Ready(snapshot(SessionPhase::Ready, None)),
    )
    .await
    .expect("ready");
}

async fn admit(chats: &ChatWriter, binding: &RuntimeBinding, prompt: &str) -> TurnRequest {
    let owner_version = chats
        .execution_owner(binding.chat_id)
        .await
        .expect("owner read")
        .expect("owner present")
        .owner
        .version;
    let request_id = Uuid::new_v4();
    let commit = chats
        .admit_execution_attempt(ChatExecutionPrompt {
            chat_id: binding.chat_id,
            request_id,
            generation: binding.generation,
            owner_version,
            observed_commit: BASE_COMMIT.to_owned(),
            effective_tools: BTreeSet::new(),
            prompt: prompt.to_owned(),
        })
        .await
        .expect("admit attempt");
    TurnRequest {
        request_id,
        // The runtime turn ID is the durable attempt ID.
        turn_id: *commit.attempt.attempt_id.as_uuid(),
        prompt: prompt.to_owned(),
    }
}

fn summary(request: &TurnRequest) -> TurnSummary {
    TurnSummary {
        request_id: request.request_id,
        turn_id: request.turn_id,
        vendor_turn_id: Some(format!("vendor_{}", request.request_id)),
        phase: TurnPhase::Running,
        outcome: None,
        acknowledged: true,
        cancel_requested: false,
        prompt_bytes: request.prompt.len(),
    }
}

async fn finish_flow(
    sink: &ChatExecutionSink,
    binding: &RuntimeBinding,
    request: &TurnRequest,
    text: &str,
) {
    sink.event(binding, RuntimeEvent::TurnAcknowledged(summary(request)))
        .await
        .expect("acknowledged");
    for (revision, reply) in [(1, "obsolete partial"), (2, text)] {
        sink.event(
            binding,
            RuntimeEvent::AssistantSnapshot(AssistantText {
                request_id: request.request_id,
                turn_id: request.turn_id,
                revision,
                text: reply.to_owned(),
                original_bytes: reply.len(),
                truncated: false,
            }),
        )
        .await
        .expect("snapshot");
    }
    let mut terminal = summary(request);
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

fn cleanup(verified: bool) -> RuntimeCleanup {
    RuntimeCleanup {
        exit: ProcessExit {
            exit_code: Some(0),
            forced_termination: false,
        },
        root_reaped: true,
        tree_empty: true,
        cleanup_verified: verified,
        stderr_bytes: 0,
    }
}

async fn end(
    sink: &ChatExecutionSink,
    binding: &RuntimeBinding,
    reason: RuntimeStopReason,
    verified: bool,
    pending: Option<TurnSummary>,
) {
    sink.event(
        binding,
        RuntimeEvent::TransportEnded {
            snapshot: snapshot(SessionPhase::Closed, None),
            pending_turn: pending,
            reason,
            cleanup: cleanup(verified),
        },
    )
    .await
    .expect("transport ended");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_attempts_share_one_lifetime_run_with_distinct_private_records() {
    let temp = tempfile::tempdir().expect("state");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (sink, binding) = fixture(&chats).await;
    ready(&sink, &binding).await;
    let lifetime_run = chats
        .execution_owner(binding.chat_id)
        .await
        .expect("owner read")
        .expect("owner")
        .run
        .run_id();

    let first = admit(&chats, &binding, PROMPT_ONE).await;
    sink.claim_turn(&binding, &first)
        .await
        .expect("one use claim");
    assert_eq!(sink.live_text(first.request_id).map(|text| text.text), None);
    finish_flow(&sink, &binding, &first, REPLY_ONE).await;
    let after_first = chats
        .execution_owner(binding.chat_id)
        .await
        .expect("owner read")
        .expect("owner");
    assert_eq!(after_first.session.state, ChatState::Ready);
    assert_eq!(after_first.run.run_id(), lifetime_run);
    assert_eq!(after_first.run.status(), RunStatus::Running);
    let stored_first = chats
        .execution_attempt(first.request_id)
        .await
        .expect("attempt read")
        .expect("attempt");
    assert_eq!(stored_first.phase, ChatTurnPhase::Completed);
    assert!(stored_first.correlated_terminal);
    assert!(stored_first.output_complete);
    // The execution path never synthesizes legacy chat-turn rows.
    assert!(
        chats
            .turn(first.request_id)
            .await
            .expect("legacy read")
            .is_none()
    );

    let second = admit(&chats, &binding, PROMPT_TWO).await;
    sink.claim_turn(&binding, &second)
        .await
        .expect("second claim");
    finish_flow(&sink, &binding, &second, REPLY_TWO).await;
    let after_second = chats
        .execution_owner(binding.chat_id)
        .await
        .expect("owner read")
        .expect("owner");
    assert_eq!(after_second.session.state, ChatState::Ready);
    assert_eq!(after_second.run.run_id(), lifetime_run);
    assert_eq!(after_second.run.status(), RunStatus::Running);
    assert_ne!(first.request_id, second.request_id);

    let records = chats
        .execution_records(binding.chat_id, 0, 32)
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
    let _ = worker.handle().expect("writer alive");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_prompt_binding_and_ack_are_refused() {
    let temp = tempfile::tempdir().expect("state");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (sink, binding) = fixture(&chats).await;
    ready(&sink, &binding).await;
    let request = admit(&chats, &binding, PROMPT_ONE).await;

    // The exact private prompt must match before the one-use claim.
    let mut wrong_prompt = request.clone();
    wrong_prompt.prompt = format!("{PROMPT_ONE}X");
    assert_eq!(
        sink.claim_turn(&binding, &wrong_prompt).await,
        Err(RuntimeError::ClaimRefused)
    );

    // A turn ID that is not the durable attempt ID is refused before the
    // claim; the attempt must stay admitted for the exact identity.
    let mut wrong_turn_id = request.clone();
    wrong_turn_id.turn_id = Uuid::new_v4();
    assert_eq!(
        sink.claim_turn(&binding, &wrong_turn_id).await,
        Err(RuntimeError::ClaimRefused)
    );
    let stored = chats
        .execution_attempt(request.request_id)
        .await
        .expect("attempt read")
        .expect("attempt");
    assert_eq!(stored.phase, ChatTurnPhase::Admitted);
    assert_eq!(*stored.attempt_id.as_uuid(), request.turn_id);

    // A different chat, generation, or fence never reaches the writer.
    let wrong_generation = RuntimeBinding {
        generation: binding.generation + 1,
        ..binding.clone()
    };
    assert_eq!(
        sink.claim_turn(&wrong_generation, &request).await,
        Err(RuntimeError::ClaimRefused)
    );
    let wrong_chat = RuntimeBinding {
        chat_id: ChatId::new(),
        ..binding.clone()
    };
    assert_eq!(
        sink.claim_turn(&wrong_chat, &request).await,
        Err(RuntimeError::ClaimRefused)
    );

    sink.claim_turn(&binding, &request)
        .await
        .expect("exact claim");
    assert_eq!(
        sink.claim_turn(&binding, &request).await,
        Err(RuntimeError::ClaimRefused)
    );

    // Acks must carry the exact captured attempt identity.
    let mut foreign_turn = summary(&request);
    foreign_turn.turn_id = Uuid::new_v4();
    assert_eq!(
        sink.event(&binding, RuntimeEvent::TurnAcknowledged(foreign_turn))
            .await,
        Err(RuntimeError::WrongTurn)
    );
    let mut unacknowledged = summary(&request);
    unacknowledged.acknowledged = false;
    assert_eq!(
        sink.event(&binding, RuntimeEvent::TurnAcknowledged(unacknowledged))
            .await,
        Err(RuntimeError::WrongTurn)
    );
    let mut foreign_request = summary(&request);
    foreign_request.request_id = Uuid::new_v4();
    foreign_request.turn_id = Uuid::new_v4();
    assert_eq!(
        sink.event(&binding, RuntimeEvent::TurnAcknowledged(foreign_request))
            .await,
        Err(RuntimeError::WrongTurn)
    );
    sink.event(&binding, RuntimeEvent::TurnAcknowledged(summary(&request)))
        .await
        .expect("exact acknowledgement");
    end(
        &sink,
        &binding,
        RuntimeStopReason::Closed,
        true,
        Some(summary(&request)),
    )
    .await;
    let _ = worker.handle().expect("writer alive");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_output_finishes_incomplete_and_clears_live_text() {
    let temp = tempfile::tempdir().expect("state");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (sink, binding) = fixture(&chats).await;
    ready(&sink, &binding).await;
    let request = admit(&chats, &binding, PROMPT_ONE).await;
    sink.claim_turn(&binding, &request)
        .await
        .expect("one use claim");
    sink.event(&binding, RuntimeEvent::TurnAcknowledged(summary(&request)))
        .await
        .expect("acknowledged");
    assert_eq!(sink.live_text(request.request_id), None);

    let mut terminal = summary(&request);
    terminal.phase = TurnPhase::Finished;
    terminal.outcome = Some(TurnOutcome::Completed);
    sink.event(
        &binding,
        RuntimeEvent::TurnFinished {
            snapshot: snapshot(SessionPhase::Ready, Some(terminal.clone())),
            turn: terminal,
            output_complete: false,
        },
    )
    .await
    .expect("terminal without output");
    let stored = chats
        .execution_attempt(request.request_id)
        .await
        .expect("attempt read")
        .expect("attempt");
    assert_eq!(stored.phase, ChatTurnPhase::Completed);
    assert!(stored.correlated_terminal);
    assert!(!stored.output_complete);
    let records = chats
        .execution_records(binding.chat_id, 0, 32)
        .await
        .expect("private history");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].record.kind, ChatRecordKind::User);
    // The cleared active state refuses late snapshots and reads.
    assert_eq!(sink.live_text(request.request_id), None);
    assert_eq!(
        sink.event(
            &binding,
            RuntimeEvent::AssistantSnapshot(AssistantText {
                request_id: request.request_id,
                turn_id: request.turn_id,
                revision: 1,
                text: REPLY_ONE.to_owned(),
                original_bytes: REPLY_ONE.len(),
                truncated: false,
            }),
        )
        .await,
        Err(RuntimeError::WrongTurn)
    );
    let _ = worker.handle().expect("writer alive");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unverified_cleanup_retains_fence_recovery_and_blocks_task() {
    let temp = tempfile::tempdir().expect("state");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (sink, binding) = fixture(&chats).await;
    ready(&sink, &binding).await;
    let request = admit(&chats, &binding, PROMPT_ONE).await;
    sink.claim_turn(&binding, &request)
        .await
        .expect("one use claim");
    sink.event(&binding, RuntimeEvent::TurnAcknowledged(summary(&request)))
        .await
        .expect("acknowledged");

    end(
        &sink,
        &binding,
        RuntimeStopReason::ProcessLost,
        false,
        Some(summary(&request)),
    )
    .await;
    let stored = chats
        .execution_attempt(request.request_id)
        .await
        .expect("attempt read")
        .expect("attempt");
    assert_eq!(stored.phase, ChatTurnPhase::RecoveryRequired);
    assert_eq!(stored.error_code, Some(ChatFailureCode::OwnershipUncertain));
    assert!(!stored.correlated_terminal);
    let owner = chats
        .execution_owner(binding.chat_id)
        .await
        .expect("owner read")
        .expect("owner");
    assert_eq!(owner.session.state, ChatState::RecoveryRequired);
    assert_eq!(owner.task.status(), TaskStatus::Blocked);
    // The ended sink refuses further callbacks; no guessed completion.
    assert_eq!(
        sink.event(
            &binding,
            RuntimeEvent::Ready(snapshot(SessionPhase::Ready, None)),
        )
        .await,
        Err(RuntimeError::Stopped)
    );
    let _ = worker.handle().expect("writer alive");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_claim_result_releases_on_exact_pending_evidence_without_success() {
    let temp = tempfile::tempdir().expect("state");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (sink, binding) = fixture(&chats).await;
    ready(&sink, &binding).await;
    let request = admit(&chats, &binding, PROMPT_ONE).await;
    // The writer committed the durable claim, but the claim future was
    // dropped before the sink recorded any local active state.
    chats
        .claim_execution_attempt(request.request_id, binding.generation, binding.owner_fence)
        .await
        .expect("durable claim");
    let stored = chats
        .execution_attempt(request.request_id)
        .await
        .expect("attempt read")
        .expect("attempt");
    assert_eq!(stored.phase, ChatTurnPhase::Running);

    // Verified teardown correlates through the exact pending identity and
    // releases through the owner store path; the claimed attempt is settled
    // Interrupted, never completed.
    sink.event(
        &binding,
        RuntimeEvent::TransportEnded {
            snapshot: snapshot(SessionPhase::Closed, None),
            pending_turn: Some(summary(&request)),
            reason: RuntimeStopReason::Closed,
            cleanup: cleanup(true),
        },
    )
    .await
    .expect("lost-claim release");
    let settled = chats
        .execution_attempt(request.request_id)
        .await
        .expect("attempt read")
        .expect("attempt");
    assert_eq!(settled.phase, ChatTurnPhase::Interrupted);
    assert_eq!(settled.outcome, Some(ChatTurnOutcome::Interrupted));
    assert!(!settled.correlated_terminal);
    let owner = chats
        .execution_owner(binding.chat_id)
        .await
        .expect("owner read")
        .expect("owner");
    assert_eq!(owner.session.state, ChatState::Closed);

    // A mismatched pending attempt ID still refuses the release, leaving
    // the claimed attempt and fence untouched for recovery.
    let (strict_sink, strict_binding) = fixture(&chats).await;
    ready(&strict_sink, &strict_binding).await;
    let strict_request = admit(&chats, &strict_binding, PROMPT_TWO).await;
    chats
        .claim_execution_attempt(
            strict_request.request_id,
            strict_binding.generation,
            strict_binding.owner_fence,
        )
        .await
        .expect("durable claim");
    let mut wrong_pending = summary(&strict_request);
    wrong_pending.turn_id = Uuid::new_v4();
    assert_eq!(
        strict_sink
            .event(
                &strict_binding,
                RuntimeEvent::TransportEnded {
                    snapshot: snapshot(SessionPhase::Closed, None),
                    pending_turn: Some(wrong_pending),
                    reason: RuntimeStopReason::Closed,
                    cleanup: cleanup(true),
                },
            )
            .await,
        Err(RuntimeError::WrongTurn)
    );
    let retained = chats
        .execution_attempt(strict_request.request_id)
        .await
        .expect("attempt read")
        .expect("attempt");
    assert_eq!(retained.phase, ChatTurnPhase::Running);
    let strict_owner = chats
        .execution_owner(strict_binding.chat_id)
        .await
        .expect("owner read")
        .expect("owner");
    assert_eq!(strict_owner.session.state, ChatState::Busy);
    let _ = worker.handle().expect("writer alive");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn verified_release_flushes_output_settles_and_closes_or_suspends() {
    let temp = tempfile::tempdir().expect("state");
    let (worker, chats) = start(temp.path().to_owned()).await;
    let (sink, binding) = fixture(&chats).await;
    ready(&sink, &binding).await;
    let request = admit(&chats, &binding, PROMPT_ONE).await;
    sink.claim_turn(&binding, &request)
        .await
        .expect("one use claim");
    sink.event(&binding, RuntimeEvent::TurnAcknowledged(summary(&request)))
        .await
        .expect("acknowledged");
    sink.event(
        &binding,
        RuntimeEvent::AssistantSnapshot(AssistantText {
            request_id: request.request_id,
            turn_id: request.turn_id,
            revision: 1,
            text: REPLY_ONE.to_owned(),
            original_bytes: REPLY_ONE.len(),
            truncated: false,
        }),
    )
    .await
    .expect("streaming snapshot");
    assert_eq!(
        sink.live_text(request.request_id).map(|text| text.text),
        Some(REPLY_ONE.to_owned())
    );

    // Verified cleanup with Closed flushes the available active output first,
    // then settles the claimed attempt and closes the chat.
    end(&sink, &binding, RuntimeStopReason::Closed, true, None).await;
    let stored = chats
        .execution_attempt(request.request_id)
        .await
        .expect("attempt read")
        .expect("attempt");
    assert_eq!(stored.phase, ChatTurnPhase::Interrupted);
    assert_eq!(stored.outcome, Some(ChatTurnOutcome::Interrupted));
    let owner = chats
        .execution_owner(binding.chat_id)
        .await
        .expect("owner read")
        .expect("owner");
    assert_eq!(owner.session.state, ChatState::Closed);
    assert_ne!(owner.task.status(), TaskStatus::Blocked);
    let records = chats
        .execution_records(binding.chat_id, 0, 32)
        .await
        .expect("private history");
    assert_eq!(
        records
            .iter()
            .find(|record| record.record.kind == ChatRecordKind::Assistant)
            .and_then(|record| record.text.as_deref()),
        Some(REPLY_ONE)
    );
    assert_eq!(sink.live_text(request.request_id), None);

    // A released idle process suspends instead of closing.
    let (idle_sink, idle_binding) = fixture(&chats).await;
    ready(&idle_sink, &idle_binding).await;
    end(
        &idle_sink,
        &idle_binding,
        RuntimeStopReason::Idle,
        true,
        None,
    )
    .await;
    let idle_owner = chats
        .execution_owner(idle_binding.chat_id)
        .await
        .expect("owner read")
        .expect("owner");
    assert_eq!(idle_owner.session.state, ChatState::Suspended);
    let _ = worker.handle().expect("writer alive");
}
