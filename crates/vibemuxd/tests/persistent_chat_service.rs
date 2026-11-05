#![cfg(all(feature = "test_helpers", windows))]
//! Real writer and contained synthetic vendors. No model access is involved.
use serde_json::json;
use std::{collections::BTreeMap, path::PathBuf, time::Duration};
use time::OffsetDateTime;
use tokio::time::{sleep, timeout};
use uuid::Uuid;
use vibemux_harness::HarnessDetection;
use vibemux_store::ChatMutation;
use vibemux_types::{
    ChatId,
    chat::{ChatHarness, ChatState, ChatTurn, ChatTurnOutcome},
};
use vibemuxd::{
    WriterWorker,
    chat_session::{ChatService, ChatSessionAction},
    harness_dispatch::{DispatchServiceSettings, HarnessDispatchService},
};

const WAIT: Duration = Duration::from_secs(12);
const PROMPT: &str = "PRIVATE_SERVICE_PROMPT_中文";
const FIXTURE: &str = env!("CARGO_BIN_EXE_vibemux_chat_fixture");
const TRAMPOLINE: &str = env!("CARGO_BIN_EXE_vibemux_launch_trampoline");

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_file_tools_preserve_paged_history_and_never_apply_reported_changes() {
    use vibemux_types::chat::ChatRecordKind;
    use vibemux_types::chat_file_tool::FileChangeKind;
    use vibemux_types::chat_tool::{CommandToolStatus, ToolRecord};
    use vibemuxd::chat_session::api::{self, ChatContentCursor, ChatRequest, ChatResponse};
    for (mode, expected_files, complete) in [
        ("file_tools", 1, true),
        ("file_tools_overflow", 63, false),
        ("file_tools_truncated", 1, false),
    ] {
        let fixture = Fixture::new(ChatHarness::Codex);
        let guard = fixture.project.join("guarded.txt");
        let guard_bytes = b"PROTECTED_FILE_TOOL_FIXTURE\n";
        std::fs::write(&guard, guard_bytes).expect("guarded fixture file");
        let head_path = fixture.project.join(".git/HEAD");
        let head = std::fs::read(&head_path).expect("fixture HEAD");
        std::fs::write(
            fixture
                .trace
                .parent()
                .expect("vendor")
                .join("chat_fixture.json"),
            json!({"mode":mode,"ready_path":fixture.trace}).to_string(),
        )
        .expect("synthetic mode");
        let (writer, dispatch, chats) = fixture.start().await;
        let chat_id = chats
            .open(Uuid::new_v4(), ChatHarness::Codex, None)
            .await
            .expect("open")
            .session
            .chat_id;
        ready(&chats, chat_id, 1).await;
        let request = Uuid::new_v4();
        let admitted = chats
            .send(chat_id, 1, request, PROMPT.to_owned())
            .await
            .expect("send");
        let terminal = terminal(&chats, chat_id, request).await;
        assert_eq!(terminal.outcome, Some(ChatTurnOutcome::Completed));
        assert_eq!(terminal.output_complete, complete, "{mode}");
        let mut cursor = ChatContentCursor {
            after_sequence: 0,
            byte_offset: 0,
        };
        let mut bodies: BTreeMap<Uuid, String> = BTreeMap::new();
        for _ in 0..256 {
            let ChatResponse::Content { page } =
                api::execute(&chats, ChatRequest::Content { chat_id, cursor })
                    .await
                    .expect("private page")
            else {
                panic!("content")
            };
            if let Some(record) = &page.record {
                assert_eq!(record.turn_id, admitted.turn.turn_id);
                assert!(!format!("{record:?}").contains("PRIVATE_"));
                if let Some(text) = &page.text {
                    assert!(text.len() <= 8192);
                    if record.kind == ChatRecordKind::Tool {
                        bodies.entry(record.record_id).or_default().push_str(text);
                    }
                }
            }
            cursor = page.next_cursor;
            if !page.more {
                break;
            }
        }
        let mut commands = 0;
        let mut files = 0;
        for body in bodies.values() {
            match ToolRecord::from_private_text(body).expect("complete private envelope") {
                ToolRecord::Command(record) => {
                    commands += 1;
                    assert_eq!(record.status, CommandToolStatus::Failed);
                    assert_eq!(record.exit_code, Some(7));
                }
                ToolRecord::FileChange(record) => {
                    files += 1;
                    if mode == "file_tools_truncated" {
                        assert_eq!(record.original_change_count, 17);
                        assert_eq!(record.changes.len(), 2);
                        assert!(
                            record.changes_truncated
                                && record.changes.iter().all(|change| change.diff_truncated)
                        );
                    } else {
                        assert_eq!(record.changes.len(), 3);
                        assert!(!record.changes_truncated);
                        assert_eq!(record.changes[0].kind, FileChangeKind::Update);
                        assert_eq!(
                            record.changes[0].move_path.as_deref(),
                            Some("reported_move.txt")
                        );
                        assert_eq!(record.changes[1].kind, FileChangeKind::Add);
                        assert_eq!(record.changes[2].kind, FileChangeKind::Delete);
                    }
                }
            }
        }
        assert_eq!(commands, 1);
        assert_eq!(
            files, expected_files,
            "{mode}; one shared budget, no duplicate terminal writes"
        );
        chats
            .change_session(mutation(chat_id, 1), ChatSessionAction::Close)
            .await
            .expect("close");
        stop(writer, dispatch, chats).await;
        assert_eq!(std::fs::read(&guard).expect("guard survived"), guard_bytes);
        assert_eq!(std::fs::read(&head_path).expect("HEAD survived"), head);
        assert!(!fixture.project.join("reported_move.txt").exists());
        assert!(!fixture.project.join("reported_add.txt").exists());
        let (writer, dispatch, chats) = fixture.start().await;
        let mut after = 0;
        let mut reopened = BTreeMap::new();
        loop {
            let page = chats
                .records(chat_id, after, 32)
                .await
                .expect("reopened history");
            if page.is_empty() {
                break;
            }
            for row in &page {
                if row.record.kind == ChatRecordKind::Tool {
                    reopened.insert(
                        row.record.record_id,
                        row.text.clone().expect("private body"),
                    );
                }
            }
            after = page.last().expect("page").record.sequence;
        }
        assert_eq!(reopened, bodies);
        stop(writer, dispatch, chats).await;
        assert_eq!(std::fs::read(&guard).expect("still guarded"), guard_bytes);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_command_tools_reach_private_paged_history_without_changing_vendor_outcomes() {
    use vibemux_types::chat::ChatRecordKind;
    use vibemux_types::chat_tool::{CommandToolRecord, CommandToolStatus};
    use vibemuxd::chat_session::api::{self, ChatContentCursor, ChatRequest, ChatResponse};
    for (mode, count, complete) in [
        ("command_tools", 1, true),
        ("command_tools_overflow", 64, false),
        ("command_tools_truncated", 1, false),
    ] {
        let fixture = Fixture::new(ChatHarness::Codex);
        let vendor = fixture.trace.parent().expect("synthetic vendor");
        std::fs::write(
            vendor.join("chat_fixture.json"),
            json!({"mode":mode,"ready_path":fixture.trace}).to_string(),
        )
        .expect("fixed synthetic mode");
        let (writer, dispatch, chats) = fixture.start().await;
        let chat_id = chats
            .open(Uuid::new_v4(), ChatHarness::Codex, None)
            .await
            .expect("open")
            .session
            .chat_id;
        ready(&chats, chat_id, 1).await;
        let request_id = Uuid::new_v4();
        let admitted = chats
            .send(chat_id, 1, request_id, PROMPT.to_owned())
            .await
            .expect("send");
        let terminal = terminal(&chats, chat_id, request_id).await;
        assert_eq!(terminal.outcome, Some(ChatTurnOutcome::Completed));
        assert_eq!(terminal.output_complete, complete, "{mode}");
        let mut cursor = ChatContentCursor {
            after_sequence: 0,
            byte_offset: 0,
        };
        let mut bodies: BTreeMap<Uuid, String> = BTreeMap::new();
        for _ in 0..256 {
            let ChatResponse::Content { page } =
                api::execute(&chats, ChatRequest::Content { chat_id, cursor })
                    .await
                    .expect("bounded content")
            else {
                panic!("content page")
            };
            if let Some(record) = &page.record {
                assert_eq!(record.turn_id, admitted.turn.turn_id);
                assert!(!format!("{record:?}").contains("PRIVATE_COMMAND_OUTPUT"));
                if let Some(text) = page.text.as_ref() {
                    assert!(text.len() <= 8192);
                    if record.kind == ChatRecordKind::Tool {
                        bodies.entry(record.record_id).or_default().push_str(text);
                    }
                }
            }
            cursor = page.next_cursor;
            if !page.more {
                break;
            }
        }
        assert_eq!(
            bodies.len(),
            count,
            "{mode}; duplicate completion must not append twice"
        );
        for body in bodies.values() {
            let tool =
                CommandToolRecord::from_private_text(body).expect("complete private envelope");
            assert_eq!(tool.command, "synthetic read-only command");
            assert_eq!(tool.status, CommandToolStatus::Completed);
            assert_eq!(tool.exit_code, Some(0));
            assert_eq!(tool.output_truncated, mode == "command_tools_truncated");
        }
        chats
            .change_session(mutation(chat_id, 1), ChatSessionAction::Close)
            .await
            .expect("close");
        stop(writer, dispatch, chats).await;
        let (writer, dispatch, chats) = fixture.start().await;
        let mut after = 0;
        let mut reopened_tools = 0;
        loop {
            let page = chats
                .records(chat_id, after, 32)
                .await
                .expect("reopened private records");
            if page.is_empty() {
                break;
            }
            reopened_tools += page
                .iter()
                .filter(|row| row.record.kind == ChatRecordKind::Tool)
                .count();
            after = page.last().expect("nonempty page").record.sequence;
        }
        assert_eq!(reopened_tools, count);
        stop(writer, dispatch, chats).await;
    }
}

struct Fixture {
    temp: tempfile::TempDir,
    state_dir: PathBuf,
    project: PathBuf,
    config: PathBuf,
    trace: PathBuf,
}
impl Fixture {
    fn new(harness: ChatHarness) -> Self {
        let temp = tempfile::tempdir().expect("fixture root");
        let project = temp.path().join("project");
        let state_dir = project.join("state");
        let vendor_dir = temp.path().join("vendor");
        std::fs::create_dir_all(project.join(".git")).expect("Git metadata");
        std::fs::create_dir_all(&state_dir).expect("state");
        std::fs::create_dir_all(&vendor_dir).expect("vendor");
        std::fs::write(
            project.join(".git/HEAD"),
            "0123456789abcdef0123456789abcdef01234567\n",
        )
        .expect("fixture HEAD");
        let executable = vendor_dir.join(format!("{}.exe", harness.as_str()));
        std::fs::copy(FIXTURE, &executable).expect("owned synthetic executable");
        let trace = vendor_dir.join("turns.txt");
        std::fs::write(
            vendor_dir.join("chat_fixture.json"),
            serde_json::to_vec(&json!({"mode":"normal","ready_path":trace})).expect("fixture JSON"),
        )
        .expect("fixture mode");
        let config = state_dir.join("harness_dispatch.json");
        std::fs::write(&config,serde_json::to_vec(&json!({"schema_version":1,"routes":[{
            "harness":harness.as_str(),"protocol":if harness==ChatHarness::Claude {"claude_stream_json"} else {"codex_app_server"},
            "executable":executable,"enabled":true,"allow_execution":true}]})).expect("config JSON")).expect("operator route");
        Self {
            temp,
            state_dir,
            project,
            config,
            trace,
        }
    }
    async fn start(&self) -> (WriterWorker, HarnessDispatchService, ChatService) {
        let database = self.state_dir.join("state.sqlite3");
        let writer = tokio::task::spawn_blocking(move || WriterWorker::start(&database))
            .await
            .expect("start task")
            .expect("writer");
        let handle = writer.handle().expect("handle");
        let seed = handle.clone();
        tokio::task::spawn_blocking(move || {
            seed.commit_harness_snapshot(
                ["claude", "codex"]
                    .into_iter()
                    .map(|name| {
                        (
                            name.to_owned(),
                            HarnessDetection {
                                detected: true,
                                path: None,
                                launcher: None,
                                version: Some("synthetic".to_owned()),
                            },
                        )
                    })
                    .collect::<BTreeMap<_, _>>(),
                "2026-10-31T00:00:00Z".to_owned(),
                "chat_service_detection".to_owned(),
            )
        })
        .await
        .expect("seed task")
        .expect("detection");
        let dispatch = HarnessDispatchService::start(
            handle.clone(),
            DispatchServiceSettings {
                project_root: self.project.clone(),
                config_path: self.config.clone(),
                trampoline: Some(PathBuf::from(TRAMPOLINE)),
            },
        )
        .await;
        let chats = ChatService::new(handle, dispatch.clone(), self.state_dir.clone());
        (writer, dispatch, chats)
    }
}
fn mutation(chat_id: ChatId, generation: u64) -> ChatMutation {
    ChatMutation {
        operation_id: Uuid::new_v4(),
        chat_id,
        expected_generation: generation,
        timestamp: OffsetDateTime::now_utc(),
    }
}
async fn ready(chats: &ChatService, id: ChatId, generation: u64) {
    timeout(WAIT, async {
        loop {
            let session = chats
                .session(id)
                .await
                .expect("session read")
                .expect("session");
            if session.state == ChatState::Ready && session.generation == generation {
                break;
            }
            assert!(
                !matches!(
                    session.state,
                    ChatState::RecoveryRequired | ChatState::Closed
                ),
                "startup ended: {session:?}"
            );
            sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .expect("ready before deadline");
}
async fn terminal(chats: &ChatService, id: ChatId, request_id: Uuid) -> ChatTurn {
    timeout(WAIT, async {
        loop {
            if let Some(turn) = chats
                .turns(id, 0, 32)
                .await
                .expect("turns")
                .into_iter()
                .find(|turn| turn.request_id == request_id)
                && turn.phase.is_terminal()
            {
                break turn;
            }
            sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .expect("terminal before deadline")
}
async fn stop(writer: WriterWorker, dispatch: HarnessDispatchService, chats: ChatService) {
    chats.shutdown().await.expect("chat shutdown");
    dispatch.shutdown().await;
    drop(chats);
    drop(dispatch);
    tokio::task::spawn_blocking(move || writer.shutdown())
        .await
        .expect("join writer task")
        .expect("writer shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_service_runs_repeated_turns_resumes_exact_session_and_reopens_history() {
    for harness in [ChatHarness::Claude, ChatHarness::Codex] {
        let fixture = Fixture::new(harness);
        let (writer, dispatch, chats) = fixture.start().await;
        let operation = Uuid::new_v4();
        let opened = chats
            .open(operation, harness, Some("Private test chat".to_owned()))
            .await
            .expect("open");
        let chat_id = opened.session.chat_id;
        ready(&chats, chat_id, 1).await;
        let mut first = None;
        let mut attempts = Vec::new();
        for _ in 0..2 {
            let request = Uuid::new_v4();
            let receipt = chats
                .send(chat_id, 1, request, PROMPT.to_owned())
                .await
                .expect("admission");
            if first.is_none() {
                first = Some(request);
            }
            let replay = chats
                .send(chat_id, 1, request, PROMPT.to_owned())
                .await
                .expect("duplicate send");
            assert!(replay.duplicate);
            assert_eq!(replay.turn.run_id, receipt.turn.run_id);
            let turn = terminal(&chats, chat_id, request).await;
            assert_eq!(turn.outcome, Some(ChatTurnOutcome::Completed));
            assert!(turn.output_complete);
            attempts.push((turn.task_id, turn.run_id));
            ready(&chats, chat_id, 1).await;
        }
        assert_ne!(attempts[0], attempts[1]);
        let conflicting_close = ChatMutation {
            operation_id: first.expect("first request"),
            chat_id,
            expected_generation: 1,
            timestamp: OffsetDateTime::now_utc(),
        };
        assert_eq!(
            chats
                .change_session(conflicting_close, ChatSessionAction::Close)
                .await
                .expect_err("reused send ID")
                .code(),
            "chat_conflict"
        );
        assert_eq!(
            chats
                .session(chat_id)
                .await
                .expect("still owned")
                .expect("chat")
                .state,
            ChatState::Ready
        );
        let trace = std::fs::read_to_string(&fixture.trace).expect("trace");
        let rows = trace.lines().collect::<Vec<_>>();
        assert_eq!(rows.len(), 2, "duplicate send launched another effect");
        assert_eq!(
            rows[0].split(':').next(),
            rows[1].split(':').next(),
            "turns must share one process"
        );
        let bound = chats
            .session(chat_id)
            .await
            .expect("bound")
            .expect("session")
            .vendor_session_id
            .expect("vendor identity");
        let close = mutation(chat_id, 1);
        chats
            .change_session(close.clone(), ChatSessionAction::Close)
            .await
            .expect("close");
        let resume = mutation(chat_id, 1);
        chats
            .change_session(resume.clone(), ChatSessionAction::Resume)
            .await
            .expect("resume");
        ready(&chats, chat_id, 2).await;
        assert!(
            chats
                .change_session(close, ChatSessionAction::Close)
                .await
                .expect("old close replay")
                .duplicate
        );
        assert!(
            chats
                .change_session(resume, ChatSessionAction::Resume)
                .await
                .expect("resume replay")
                .duplicate
        );
        let third = Uuid::new_v4();
        chats
            .send(chat_id, 2, third, "Another private prompt".to_owned())
            .await
            .expect("third send");
        assert_eq!(
            terminal(&chats, chat_id, third).await.outcome,
            Some(ChatTurnOutcome::Completed)
        );
        let current = chats
            .session(chat_id)
            .await
            .expect("current")
            .expect("session");
        assert_eq!(current.vendor_session_id.as_deref(), Some(bound.as_str()));
        let records = chats
            .records(chat_id, 0, 32)
            .await
            .expect("private transcript");
        assert_eq!(records.len(), 6);
        assert_eq!(records[0].text.as_deref(), Some(PROMPT));
        assert!(records.iter().all(|record| record.content_error.is_none()));
        chats
            .change_session(mutation(chat_id, 2), ChatSessionAction::Close)
            .await
            .expect("close resumed");
        stop(writer, dispatch, chats).await;
        let (writer, dispatch, chats) = fixture.start().await;
        assert_eq!(
            chats
                .records(chat_id, 0, 32)
                .await
                .expect("reopened transcript")
                .len(),
            6
        );
        assert_eq!(
            chats.title(chat_id).await.expect("title").as_deref(),
            Some("Private test chat")
        );
        assert!(
            chats
                .send(chat_id, 1, first.expect("first request"), PROMPT.to_owned())
                .await
                .expect("old send after restart")
                .duplicate
        );
        assert_eq!(
            std::fs::read_to_string(&fixture.trace)
                .expect("trace")
                .lines()
                .count(),
            3
        );
        stop(writer, dispatch, chats).await;
        assert!(fixture.temp.path().exists());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropping_a_shutdown_observer_keeps_join_ownership_and_shared_completion() {
    let fixture = Fixture::new(ChatHarness::Codex);
    std::fs::write(
        fixture.temp.path().join("vendor/chat_fixture.json"),
        serde_json::to_vec(&json!({"mode":"slow_shutdown","ready_path":fixture.trace}))
            .expect("mode JSON"),
    )
    .expect("hanging fixture");
    let (writer, dispatch, chats) = fixture.start().await;
    let chat = chats
        .open(Uuid::new_v4(), ChatHarness::Codex, None)
        .await
        .expect("open")
        .session
        .chat_id;
    ready(&chats, chat, 1).await;
    let request = Uuid::new_v4();
    chats
        .send(chat, 1, request, PROMPT.to_owned())
        .await
        .expect("admit hanging turn");
    timeout(WAIT, async {
        while !fixture.trace.exists() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("input was sent");
    let first = {
        let chats = chats.clone();
        tokio::spawn(async move { chats.shutdown().await })
    };
    timeout(WAIT, async {
        while !fixture.trace.with_extension("closing").exists() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("shutdown handshake");
    assert!(
        !first.is_finished(),
        "fixture must hold the shutdown observation open"
    );
    first.abort();
    assert!(first.await.expect_err("observer cancelled").is_cancelled());
    std::fs::write(fixture.trace.with_extension("release"), b"release")
        .expect("release owned close handshake");
    let (second, third) = tokio::join!(chats.shutdown(), chats.shutdown());
    second.expect("second observer joins original actor");
    third.expect("concurrent observer sees shared terminal evidence");
    let session = chats
        .session(chat)
        .await
        .expect("session")
        .expect("known chat");
    assert_eq!(session.state, ChatState::Suspended);
    let turn = terminal(&chats, chat, request).await;
    // This peer acknowledges the interrupt before its controlled EOF wait.
    assert_eq!(turn.outcome, Some(ChatTurnOutcome::Cancelled));
    assert!(turn.correlated_terminal);
    stop(writer, dispatch, chats).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authenticated_control_preserves_replay_private_history_and_purge() {
    use vibemuxd::chat_session::api::ChatContentCursor;
    use vibemuxd::control::{ChatRequest, ChatResponse, ControlClient, DaemonControlServer};
    let fixture = Fixture::new(ChatHarness::Codex);
    let (writer, dispatch, chats) = fixture.start().await;
    stop(writer, dispatch, chats).await;
    let runtime = fixture.temp.path().join("runtime");
    std::fs::create_dir(&runtime).expect("runtime");
    let server = DaemonControlServer::start_with_dispatch(
        &fixture.state_dir.join("state.sqlite3"),
        &runtime,
        DispatchServiceSettings {
            project_root: fixture.project.clone(),
            config_path: fixture.config.clone(),
            trampoline: Some(PathBuf::from(TRAMPOLINE)),
        },
    )
    .await
    .expect("authenticated server");
    let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
    let operation_id = Uuid::new_v4();
    let open = ChatRequest::Open {
        operation_id,
        harness: ChatHarness::Codex,
        title: Some("PRIVATE_CONTROL_TITLE".to_owned()),
        consent_private_storage: true,
    };
    let ChatResponse::Session { session, .. } = client.chat(&open).await.expect("open") else {
        panic!("session receipt")
    };
    let chat_id = session.chat_id;
    assert!(
        !serde_json::to_string(&session)
            .expect("status JSON")
            .contains("PRIVATE_")
    );
    timeout(WAIT, async {
        loop {
            if let ChatResponse::Status {
                session: Some(current),
            } = client
                .chat(&ChatRequest::Status { chat_id })
                .await
                .expect("status")
                && current.state == ChatState::Ready
            {
                break;
            }
            sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .expect("ready");
    let ChatResponse::Status {
        session: Some(found),
    } = client
        .chat(&ChatRequest::LookupOpen { operation_id })
        .await
        .expect("lookup")
    else {
        panic!("open lookup")
    };
    assert_eq!(found.chat_id, chat_id);
    let request_id = Uuid::new_v4();
    let prompt = "文\u{1}\"\\".repeat(1800);
    let send = ChatRequest::Send {
        chat_id,
        generation: 1,
        request_id,
        prompt: prompt.clone(),
    };
    let ChatResponse::Turn { turn: admitted, .. } = client.chat(&send).await.expect("send") else {
        panic!("admission")
    };
    assert!(matches!(
        client.chat(&send).await.expect("same send"),
        ChatResponse::Turn {
            duplicate: true,
            ..
        }
    ));
    timeout(WAIT, async {
        loop {
            let result = client
                .chat(&ChatRequest::Submission {
                    chat_id,
                    generation: 1,
                    request_id,
                })
                .await
                .expect("submission lookup");
            if let ChatResponse::Submission {
                turn: Some(turn), ..
            } = result
                && turn.phase.is_terminal()
            {
                assert_eq!(turn.run_id, admitted.run_id);
                assert_eq!(turn.outcome, Some(ChatTurnOutcome::Completed));
                break;
            }
            sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .expect("completed");
    let mut cursor = ChatContentCursor::default();
    let mut restored = String::new();
    loop {
        let ChatResponse::Content { page } = client
            .chat(&ChatRequest::Content { chat_id, cursor })
            .await
            .expect("content page")
        else {
            panic!("private page")
        };
        assert!(page.content_error.is_none());
        if page
            .record
            .as_ref()
            .is_some_and(|record| record.kind == vibemux_types::chat::ChatRecordKind::User)
        {
            restored.push_str(page.text.as_deref().expect("private prompt chunk"));
        }
        cursor = page.next_cursor;
        if !page.more {
            break;
        }
    }
    assert_eq!(
        restored, prompt,
        "UTF8 and JSON escapes survive bounded Control chunks"
    );
    assert_eq!(
        std::fs::read_to_string(&fixture.trace)
            .expect("trace")
            .lines()
            .count(),
        1
    );
    for request in [
        ChatRequest::Close {
            operation_id: Uuid::new_v4(),
            chat_id,
            generation: 1,
        },
        ChatRequest::Archive {
            operation_id: Uuid::new_v4(),
            chat_id,
            generation: 1,
        },
        ChatRequest::Purge {
            operation_id: Uuid::new_v4(),
            chat_id,
            generation: 1,
        },
    ] {
        client.chat(&request).await.expect("explicit management");
    }
    assert!(matches!(
        client.chat(&send).await.expect("purged replay"),
        ChatResponse::Turn {
            duplicate: true,
            ..
        }
    ));
    let ChatResponse::Content { page } = client
        .chat(&ChatRequest::Content {
            chat_id,
            cursor: ChatContentCursor::default(),
        })
        .await
        .expect("purged metadata")
    else {
        panic!("content")
    };
    assert!(page.text.is_none());
    server.shutdown().await.expect("owned daemon shutdown");
}
