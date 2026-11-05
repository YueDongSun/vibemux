#![cfg(all(feature = "test_helpers", windows))]
//! Real owned Git workspaces and synthetic contained vendors, without inference.
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use time::OffsetDateTime;
use tokio::{
    process::Command,
    time::{sleep, timeout},
};
use uuid::Uuid;
use vibemux_harness::HarnessDetection;
use vibemux_store::ChatOwnerMutation;
use vibemux_types::{
    ChatId, RunStatus,
    chat::{ChatHarness, ChatState, ChatTurnOutcome},
    chat_attempt::ChatExecutionAttempt,
    chat_owner::ChatOwnerPhase,
};
use vibemuxd::{
    WriterWorker,
    chat_session::ChatService,
    control::{
        ChatExecutionRequest, ChatExecutionResponse, ChatOwnerRequest, ChatOwnerResponse,
        ControlClient, DaemonControlServer,
    },
    harness_dispatch::{DispatchServiceSettings, HarnessDispatchService},
};

const WAIT: Duration = Duration::from_secs(30);
// Observation of an already-owned cold ACL publication may outlast the RPC.
const RECEIPT_OBSERVATION: Duration = Duration::from_secs(60);
// Matches the service's bounded provisioning drain, not an inference deadline.
const PROVISION_OBSERVATION: Duration = Duration::from_secs(600);
const FIXTURE: &str = env!("CARGO_BIN_EXE_vibemux_chat_fixture");
const TRAMPOLINE: &str = env!("CARGO_BIN_EXE_vibemux_launch_trampoline");
const PROMPT: &str = "PRIVATE_EXECUTION_SERVICE_中文 ; $(literal)";

async fn git(executable: &Path, root: &Path, arguments: &[&str]) -> String {
    let mut command = Command::new(executable);
    command
        .current_dir(root)
        .args([
            "-c",
            "user.name=Execution Fixture",
            "-c",
            "user.email=fixture@invalid.test",
            "-c",
            "core.hooksPath=NUL",
            "-c",
            "commit.gpgSign=false",
        ])
        .args(arguments)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .env_clear()
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "NUL")
        .creation_flags(0x08000000);
    for key in ["SystemRoot", "WINDIR", "TEMP", "TMP", "PATH"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    let output = timeout(WAIT, command.output())
        .await
        .expect("bounded Git")
        .expect("Git");
    assert!(
        output.status.success(),
        "Git failure: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("UTF8")
}

struct Fixture {
    _temp: tempfile::TempDir,
    project: PathBuf,
    state: PathBuf,
    config: PathBuf,
    git: PathBuf,
    trace: PathBuf,
    cwd: PathBuf,
    base: String,
}
impl Fixture {
    async fn control_server(&self) -> DaemonControlServer {
        DaemonControlServer::start_with_dispatch(
            &self.state.join("state.sqlite3"),
            &self._temp.path().join("control_runtime"),
            DispatchServiceSettings {
                project_root: self.project.clone(),
                config_path: self.config.clone(),
                trampoline: Some(PathBuf::from(TRAMPOLINE)),
            },
        )
        .await
        .expect("control daemon")
    }

    async fn new(harness: ChatHarness, mode: &str) -> Self {
        let temp = tempfile::tempdir().expect("fixture");
        let project = temp.path().join("project space_中文");
        let state = project.join(".vibemux");
        let vendor = temp.path().join("vendor");
        std::fs::create_dir_all(&state).expect("state");
        std::fs::create_dir(&vendor).expect("vendor");
        let git_executable = std::env::split_paths(&std::env::var_os("PATH").expect("PATH"))
            .filter(|path| path.is_absolute())
            .map(|path| path.join("git.exe"))
            .find(|path| path.is_file())
            .expect("Git executable");
        git(
            &git_executable,
            &project,
            &["init", "--quiet", "--initial-branch=main"],
        )
        .await;
        std::fs::write(project.join("base.txt"), "unchanged base\n").expect("base");
        std::fs::write(project.join(".gitignore"), ".vibemux/\n").expect("ignore");
        git(
            &git_executable,
            &project,
            &["add", "base.txt", ".gitignore"],
        )
        .await;
        git(
            &git_executable,
            &project,
            &["commit", "--quiet", "-m", "fixture base"],
        )
        .await;
        let base = git(&git_executable, &project, &["rev-parse", "HEAD"])
            .await
            .trim()
            .to_owned();
        let executable = vendor.join(format!("{}.exe", harness.as_str()));
        std::fs::copy(FIXTURE, &executable).expect("synthetic image");
        let trace = vendor.join("turns.txt");
        let cwd = vendor.join("cwd.txt");
        std::fs::write(
            vendor.join("chat_fixture.json"),
            json!({"mode":mode,"ready_path":trace,"cwd_path":cwd}).to_string(),
        )
        .expect("fixture mode");
        let config = state.join("harness_dispatch.json");
        std::fs::write(&config,json!({"schema_version":1,"routes":[{"harness":harness.as_str(),"protocol":if harness==ChatHarness::Claude {"claude_stream_json"} else {"codex_app_server"},"executable":executable,"enabled":true,"allow_execution":true}]}).to_string()).expect("route");
        Self {
            _temp: temp,
            project,
            state,
            config,
            git: git_executable,
            trace,
            cwd,
            base,
        }
    }
    async fn start(&self) -> (WriterWorker, HarnessDispatchService, ChatService) {
        let database = self.state.join("state.sqlite3");
        let writer = tokio::task::spawn_blocking(move || {
            WriterWorker::start_with_config(
                &database,
                vibemuxd::DEFAULT_WRITER_QUEUE_CAPACITY,
                WAIT,
            )
        })
        .await
        .expect("writer task")
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
                                version: Some("synthetic".into()),
                            },
                        )
                    })
                    .collect::<BTreeMap<_, _>>(),
                "2026-11-01T00:00:00Z".into(),
                "execution_fixture".into(),
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
        let chats = ChatService::new(handle, dispatch.clone(), self.state.clone());
        (writer, dispatch, chats)
    }
}

async fn control_status(
    client: &ControlClient,
    chat_id: ChatId,
) -> vibemuxd::chat_session::execution_api::ChatExecutionStatus {
    let ChatExecutionResponse::Status {
        runtime: Some(runtime),
    } = client
        .chat_execution(&ChatExecutionRequest::Status { chat_id })
        .await
        .expect("status")
    else {
        panic!("runtime status")
    };
    runtime
}

async fn control_open_owner(
    client: &ControlClient,
    request: ChatOwnerRequest,
) -> vibemuxd::control::ChatOwnerStatus {
    let ChatOwnerRequest::Open { operation_id, .. } = &request else {
        panic!("Open request")
    };
    match client.chat_owner(&request).await {
        Ok(ChatOwnerResponse::Owner { owner, .. }) => owner,
        Err(error) if error.code() == "writer_response_timeout" => {
            // The write is still owned by the writer. Observe its exact receipt;
            // issuing a new Open or assuming non-admission would be incorrect.
            timeout(RECEIPT_OBSERVATION, async {
                loop {
                    match client
                        .chat_owner(&ChatOwnerRequest::LookupOpen {
                            operation_id: *operation_id,
                        })
                        .await
                    {
                        Ok(ChatOwnerResponse::Status { owner: Some(owner) }) => break owner,
                        Ok(ChatOwnerResponse::Status { owner: None }) => {}
                        Err(error) if error.code() == "writer_response_timeout" => {}
                        other => panic!("receipt observation: {other:?}"),
                    }
                    sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .expect("late Open receipt")
        }
        other => panic!("owner Open: {other:?}"),
    }
}
async fn control_ready(
    client: &ControlClient,
    chat_id: ChatId,
    generation: u64,
) -> vibemuxd::chat_session::execution_api::ChatExecutionStatus {
    timeout(WAIT, async {
        loop {
            let runtime = control_status(client, chat_id).await;
            assert!(
                !matches!(
                    runtime.session.state,
                    ChatState::RecoveryRequired | ChatState::Suspended | ChatState::Closed
                ),
                "startup ended: {runtime:?}"
            );
            if runtime.session.state == ChatState::Ready && runtime.session.generation == generation
            {
                return runtime;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("control ready")
}
async fn control_start(
    client: &ControlClient,
    request: &ChatExecutionRequest,
) -> (
    vibemuxd::chat_session::execution_api::ChatExecutionStatus,
    u64,
) {
    let ChatExecutionRequest::Start {
        operation_id,
        chat_id,
        generation,
        owner_version,
        ..
    } = request
    else {
        panic!("Start request");
    };
    match client.chat_execution(request).await {
        Ok(ChatExecutionResponse::Process {
            runtime,
            sequence,
            duplicate: false,
        }) => (runtime, sequence),
        Err(error)
            if matches!(
                error.code(),
                "chat_acknowledgement_uncertain" | "writer_response_timeout"
            ) =>
        {
            // Observe the original intent without repeating its launch effect.
            timeout(RECEIPT_OBSERVATION, async {
                loop {
                    match client
                        .chat_execution(&ChatExecutionRequest::LookupStart {
                            operation_id: *operation_id,
                            chat_id: *chat_id,
                            generation: *generation,
                            owner_version: *owner_version,
                        })
                        .await
                    {
                        Ok(ChatExecutionResponse::StartReceipt {
                            runtime: Some(runtime),
                            sequence: Some(sequence),
                        }) => break (runtime, sequence),
                        Ok(ChatExecutionResponse::StartReceipt {
                            runtime: None,
                            sequence: None,
                        }) => {}
                        Err(error)
                            if matches!(
                                error.code(),
                                "chat_acknowledgement_uncertain" | "writer_response_timeout"
                            ) => {}
                        other => panic!("Start receipt observation: {other:?}"),
                    }
                    sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .expect("bounded Start receipt observation")
        }
        other => panic!("Start result: {other:?}"),
    }
}
async fn control_terminal(
    client: &ControlClient,
    chat_id: ChatId,
    generation: u64,
    request_id: Uuid,
) -> vibemuxd::chat_session::execution_api::ChatAttemptStatus {
    timeout(WAIT, async {
        loop {
            let ChatExecutionResponse::Submission {
                attempt: Some(attempt),
                ..
            } = client
                .chat_execution(&ChatExecutionRequest::Submission {
                    chat_id,
                    generation,
                    request_id,
                })
                .await
                .expect("submission")
            else {
                panic!("admitted attempt")
            };
            if attempt.phase.is_terminal() {
                return attempt;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("control terminal")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authenticated_owner_control_streams_private_history_and_replays_after_restart() {
    for harness in [ChatHarness::Codex, ChatHarness::Claude] {
        let fixture = Fixture::new(harness, "normal").await;
        let (writer, dispatch, chats) = fixture.start().await;
        stop(writer, dispatch, chats).await;
        let server = fixture.control_server().await;
        let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
        let owner = control_open_owner(
            &client,
            ChatOwnerRequest::Open {
                operation_id: Uuid::new_v4(),
                harness,
                title: Some("PRIVATE_CONTROL_TITLE".into()),
                consent_private_storage: true,
                consent_workspace: true,
            },
        )
        .await;
        let id = owner.chat_id;
        let observed = control_status(&client, id).await;
        if observed.owner.phase == ChatOwnerPhase::Provisioning
            && !observed.owner.provisioning_active
        {
            client
                .chat_owner(&ChatOwnerRequest::Reconcile {
                    operation_id: Uuid::new_v4(),
                    chat_id: id,
                    expected_version: observed.owner.version,
                })
                .await
                .expect("explicit reconcile of retained unclaimed provisioning");
        }
        let runtime = timeout(PROVISION_OBSERVATION, async {
            let mut previous = None;
            loop {
                let runtime = control_status(&client, id).await;
                let observation = (
                    runtime.owner.phase,
                    runtime.owner.version,
                    runtime.owner.provisioning_active,
                    runtime.owner.error_code,
                );
                if previous != Some(observation) {
                    eprintln!("fixture provisioning: {observation:?}");
                    previous = Some(observation);
                }
                if runtime.owner.phase == ChatOwnerPhase::Open && !runtime.owner.provisioning_active
                {
                    return runtime;
                }
                assert_ne!(runtime.owner.phase, ChatOwnerPhase::RecoveryRequired);
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("provisioned");
        assert!(!runtime.process_claimed);
        let first_owner_version = runtime.owner.version;
        let start = ChatExecutionRequest::Start {
            operation_id: Uuid::new_v4(),
            chat_id: id,
            generation: 1,
            owner_version: first_owner_version,
            consent_execution: true,
        };
        let (started, start_sequence) = control_start(&client, &start).await;
        assert!(started.process_claimed);
        control_ready(&client, id, 1).await;
        let first_request = Uuid::new_v4();
        let prompt = format!("PRIVATE_CONTROL_中文 ; $(literal)\n{}", "中文".repeat(1800));
        let send = ChatExecutionRequest::Send {
            chat_id: id,
            generation: 1,
            owner_version: first_owner_version,
            request_id: first_request,
            prompt: prompt.clone(),
        };
        let response = client.chat_execution(&send).await.expect("send");
        assert!(!format!("{response:?}").contains("PRIVATE_"));
        let ChatExecutionResponse::Attempt { attempt: first, .. } = response else {
            panic!("attempt")
        };
        let finished = control_terminal(&client, id, 1, first_request).await;
        assert_eq!(finished.outcome, Some(ChatTurnOutcome::Completed));
        assert_eq!(first.run_id, owner.run_id);
        assert_eq!(first.task_id, owner.task_id);
        let mut cursor = vibemuxd::chat_session::api::ChatContentCursor::default();
        let mut recovered = String::new();
        for _ in 0..8 {
            let ChatExecutionResponse::Content { page } = client
                .chat_execution(&ChatExecutionRequest::Content {
                    chat_id: id,
                    cursor,
                })
                .await
                .expect("private chunk")
            else {
                panic!("content")
            };
            assert_eq!(
                page.record.as_ref().expect("metadata").kind,
                vibemux_types::chat::ChatRecordKind::User
            );
            assert!(page.text.as_ref().expect("body").len() <= 8192);
            assert!(!format!("{page:?}").contains("PRIVATE_"));
            recovered.push_str(page.text.as_ref().expect("body"));
            cursor = page.next_cursor;
            if page.complete {
                break;
            }
        }
        assert_eq!(recovered, prompt);
        let current = control_status(&client, id).await;
        assert_eq!(current.owner.run_status, RunStatus::Running);
        let second_request = Uuid::new_v4();
        client
            .chat_execution(&ChatExecutionRequest::Send {
                chat_id: id,
                generation: 1,
                owner_version: current.owner.version,
                request_id: second_request,
                prompt: "followup".into(),
            })
            .await
            .expect("second turn");
        control_terminal(&client, id, 1, second_request).await;
        let before = std::fs::read_to_string(&fixture.trace).expect("two inputs");
        assert_eq!(before.lines().count(), 2);
        let ChatExecutionResponse::Attempt {
            attempt,
            duplicate: true,
            ..
        } = client.chat_execution(&send).await.expect("same receipt")
        else {
            panic!("duplicate")
        };
        assert_eq!(attempt.attempt_id, first.attempt_id);
        let ChatExecutionResponse::Attempts {
            attempts,
            next_sequence,
            more: true,
            ..
        } = client
            .chat_execution(&ChatExecutionRequest::Attempts {
                chat_id: id,
                after_sequence: 0,
                limit: 1,
            })
            .await
            .expect("page1")
        else {
            panic!("attempt page")
        };
        assert_eq!(attempts[0].attempt_id, first.attempt_id);
        let ChatExecutionResponse::Attempts {
            attempts,
            more: false,
            ..
        } = client
            .chat_execution(&ChatExecutionRequest::Attempts {
                chat_id: id,
                after_sequence: next_sequence,
                limit: 1,
            })
            .await
            .expect("page2")
        else {
            panic!("attempt page")
        };
        assert_eq!(attempts[0].request_id, second_request);
        let ChatExecutionResponse::Status {
            runtime: Some(closed),
        } = client
            .chat_execution(&ChatExecutionRequest::Close {
                chat_id: id,
                generation: 1,
            })
            .await
            .expect("close")
        else {
            panic!("closed status")
        };
        assert!(!closed.process_claimed);
        assert_eq!(closed.owner.run_status, RunStatus::Running);
        server.shutdown().await.expect("server shutdown");
        let server = fixture.control_server().await;
        let client =
            ControlClient::from_descriptor(server.descriptor_path()).expect("reopened client");
        assert_eq!(
            std::fs::read_to_string(&fixture.trace).expect("no auto launch"),
            before
        );
        let ChatExecutionRequest::Start { operation_id, .. } = start else {
            panic!("start binding")
        };
        let ChatExecutionResponse::StartReceipt {
            runtime: Some(_),
            sequence: Some(sequence),
        } = client
            .chat_execution(&ChatExecutionRequest::LookupStart {
                operation_id,
                chat_id: id,
                generation: 1,
                owner_version: first_owner_version,
            })
            .await
            .expect("lookup original start")
        else {
            panic!("receipt")
        };
        assert_eq!(sequence, start_sequence);
        let current = control_status(&client, id).await;
        assert!(!current.process_claimed);
        client
            .chat_execution(&ChatExecutionRequest::Start {
                operation_id: Uuid::new_v4(),
                chat_id: id,
                generation: 1,
                owner_version: current.owner.version,
                consent_execution: true,
            })
            .await
            .expect("resume");
        let resumed = control_ready(&client, id, 2).await;
        let third = Uuid::new_v4();
        client
            .chat_execution(&ChatExecutionRequest::Send {
                chat_id: id,
                generation: 2,
                owner_version: resumed.owner.version,
                request_id: third,
                prompt: "after restart".into(),
            })
            .await
            .expect("resumed send");
        assert_eq!(
            control_terminal(&client, id, 2, third).await.outcome,
            Some(ChatTurnOutcome::Completed)
        );
        client
            .chat_execution(&ChatExecutionRequest::Close {
                chat_id: id,
                generation: 2,
            })
            .await
            .expect("close generation2");
        assert!(
            client
                .chat_execution(&ChatExecutionRequest::Close {
                    chat_id: id,
                    generation: 1
                })
                .await
                .is_err(),
            "stale Close cannot affect successor"
        );
        let current = control_status(&client, id).await;
        let ChatOwnerResponse::Owner {
            owner: finalized, ..
        } = client
            .chat_owner(&ChatOwnerRequest::Stop {
                operation_id: Uuid::new_v4(),
                chat_id: id,
                expected_version: current.owner.version,
            })
            .await
            .expect("final Stop")
        else {
            panic!("finalized")
        };
        assert_eq!(finalized.run_status, RunStatus::Stopped);
        assert!(matches!(
            client
                .chat_execution(&send)
                .await
                .expect("old submission receipt"),
            ChatExecutionResponse::Attempt {
                duplicate: true,
                ..
            }
        ));
        let legacy_error = client
            .chat(&vibemuxd::control::ChatRequest::Status { chat_id: id })
            .await
            .expect_err("legacy endpoint excludes owners");
        assert_eq!(legacy_error.code(), "chat_owner_control_required");
        let branch = finalized.workspace_branch.expect("owned branch");
        let inventory = git(
            &fixture.git,
            &fixture.project,
            &["worktree", "list", "--porcelain"],
        )
        .await;
        let branch_line = format!("branch refs/heads/{branch}");
        let path = inventory
            .split("\n\n")
            .find(|entry| entry.lines().any(|line| line == branch_line))
            .and_then(|entry| {
                entry
                    .lines()
                    .find_map(|line| line.strip_prefix("worktree "))
            })
            .expect("owned inventory path")
            .to_owned();
        server.shutdown().await.expect("stop fixture daemon");
        git(
            &fixture.git,
            &fixture.project,
            &["worktree", "remove", "--", &path],
        )
        .await;
        assert_eq!(
            git(&fixture.git, &fixture.project, &["rev-parse", "HEAD"])
                .await
                .trim(),
            fixture.base
        );
        assert!(
            git(&fixture.git, &fixture.project, &["status", "--porcelain"])
                .await
                .trim()
                .is_empty()
        );
    }
}

fn mutation(chat_id: ChatId, version: u64) -> ChatOwnerMutation {
    ChatOwnerMutation {
        operation_id: Uuid::new_v4(),
        chat_id,
        expected_version: version,
        timestamp: OffsetDateTime::now_utc(),
    }
}
async fn provisioned(
    chats: &ChatService,
    chat_id: ChatId,
) -> vibemux_store::ChatExecutionOwnerCommit {
    timeout(WAIT, async {
        loop {
            let stored = chats
                .execution_owner(chat_id)
                .await
                .expect("owner read")
                .expect("owner");
            assert_ne!(
                stored.owner.phase,
                ChatOwnerPhase::RecoveryRequired,
                "provisioning: {stored:?}"
            );
            if stored.owner.phase == ChatOwnerPhase::Open
                && !chats
                    .workspace_provisioning_active(chat_id)
                    .expect("active")
            {
                return stored;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("provisioned")
}
async fn ready(chats: &ChatService, chat_id: ChatId, generation: u64) {
    timeout(WAIT, async {
        loop {
            let stored = chats
                .execution_owner(chat_id)
                .await
                .expect("owner")
                .expect("owner");
            if stored.session.state == ChatState::Ready && stored.session.generation == generation {
                break;
            }
            assert!(
                !matches!(
                    stored.session.state,
                    ChatState::RecoveryRequired | ChatState::Closed | ChatState::Suspended
                ),
                "runtime ended: {stored:?}"
            );
            sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .expect("runtime ready")
}
async fn terminal(chats: &ChatService, request_id: Uuid) -> ChatExecutionAttempt {
    timeout(WAIT, async {
        loop {
            let attempt = chats
                .execution_attempt(request_id)
                .await
                .expect("attempt")
                .expect("admitted");
            if attempt.phase.is_terminal() {
                return attempt;
            }
            sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .expect("terminal")
}
async fn cancellable_input(
    chats: &ChatService,
    fixture: &Fixture,
    harness: ChatHarness,
    request_id: Uuid,
) {
    timeout(WAIT, async {
        loop {
            let observed = if harness == ChatHarness::Codex {
                chats
                    .execution_attempt(request_id)
                    .await
                    .expect("attempt")
                    .expect("admitted")
                    .vendor_turn_id
                    .is_some()
            } else {
                // Claude's result carries its acknowledgement; cancellation
                // may precede that result once the fixture received the input.
                std::fs::read_to_string(&fixture.trace).is_ok_and(|text| !text.is_empty())
            };
            if observed {
                break;
            }
            sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .expect("input observed before cancellation");
}

async fn stop(writer: WriterWorker, dispatch: HarnessDispatchService, chats: ChatService) {
    chats.shutdown().await.expect("chat shutdown");
    dispatch.shutdown().await;
    drop(chats);
    drop(dispatch);
    tokio::task::spawn_blocking(move || writer.shutdown())
        .await
        .expect("writer join")
        .expect("writer shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owner_runtime_keeps_one_run_and_workspace_across_turns_and_restart() {
    for harness in [ChatHarness::Codex, ChatHarness::Claude] {
        let fixture = Fixture::new(harness, "normal").await;
        let (writer, dispatch, chats) = fixture.start().await;
        let opened = chats
            .open_execution_owner(Uuid::new_v4(), harness, None)
            .await
            .expect("open");
        let stored = provisioned(&chats, opened.owner.chat_id).await;
        let id = stored.owner.chat_id;
        let workspace = stored.owner.workspace.clone().expect("workspace");
        let start = mutation(id, stored.owner.version);
        let started = chats
            .start_execution(start.clone(), 1)
            .await
            .expect("start");
        assert!(started.fence.is_some());
        ready(&chats, id, 1).await;
        assert_eq!(
            std::fs::canonicalize(std::fs::read_to_string(&fixture.cwd).expect("observed cwd"))
                .expect("cwd"),
            std::fs::canonicalize(&workspace.path).expect("owned cwd")
        );
        assert!(
            chats
                .stop_execution_owner(mutation(id, stored.owner.version))
                .await
                .is_err(),
            "final Stop must refuse live process"
        );
        let first_request = Uuid::new_v4();
        let first = chats
            .send_execution(id, 1, stored.owner.version, first_request, PROMPT.into())
            .await
            .expect("first admission");
        assert_eq!(
            terminal(&chats, first_request).await.outcome,
            Some(ChatTurnOutcome::Completed)
        );
        let next = chats
            .execution_owner(id)
            .await
            .expect("read")
            .expect("owner");
        let second_request = Uuid::new_v4();
        let second = chats
            .send_execution(
                id,
                1,
                next.owner.version,
                second_request,
                "second private prompt".into(),
            )
            .await
            .expect("followup");
        assert_eq!(
            terminal(&chats, second_request).await.outcome,
            Some(ChatTurnOutcome::Completed)
        );
        assert_ne!(first.attempt.attempt_id, second.attempt.attempt_id);
        assert_eq!(first.attempt.owner_run_id, second.attempt.owner_run_id);
        assert_eq!(second.run.status(), RunStatus::Running);
        let before = std::fs::read_to_string(&fixture.trace).expect("two physical inputs");
        let pids: Vec<_> = before
            .lines()
            .map(|line| line.split(':').next().expect("PID"))
            .collect();
        assert_eq!(pids.len(), 2);
        assert_eq!(pids[0], pids[1]);
        assert_eq!(
            chats
                .execution_records(id, 0, 32)
                .await
                .expect("history")
                .len(),
            4
        );
        git(
            &fixture.git,
            Path::new(&workspace.path),
            &["symbolic-ref", "HEAD", "refs/heads/main"],
        )
        .await;
        let replay = chats
            .send_execution(id, 1, stored.owner.version, first_request, PROMPT.into())
            .await
            .expect("receipt before current Git check");
        assert!(replay.duplicate);
        assert_eq!(replay.attempt.attempt_id, first.attempt.attempt_id);
        assert!(
            chats
                .start_execution(start.clone(), 1)
                .await
                .expect("Start replay")
                .fence
                .is_none()
        );
        let current = chats
            .execution_owner(id)
            .await
            .expect("read")
            .expect("owner");
        assert!(
            chats
                .send_execution(
                    id,
                    1,
                    current.owner.version,
                    Uuid::new_v4(),
                    "changed branch must refuse".into()
                )
                .await
                .is_err()
        );
        git(
            &fixture.git,
            Path::new(&workspace.path),
            &[
                "symbolic-ref",
                "HEAD",
                &format!("refs/heads/{}", workspace.branch),
            ],
        )
        .await;
        let closed = chats
            .close_execution_process(id, 1)
            .await
            .expect("close process");
        let vendor_id = closed
            .session
            .vendor_session_id
            .clone()
            .expect("confirmed vendor identity");
        assert_eq!(closed.run.status(), RunStatus::Running);
        assert_eq!(closed.owner.phase, ChatOwnerPhase::Open);
        stop(writer, dispatch, chats).await;
        let (writer, dispatch, chats) = fixture.start().await;
        assert_eq!(
            std::fs::read_to_string(&fixture.trace).expect("no restart replay"),
            before
        );
        let replay = chats
            .send_execution(id, 1, stored.owner.version, first_request, PROMPT.into())
            .await
            .expect("old Send receipt");
        assert!(replay.duplicate);
        let current = chats
            .execution_owner(id)
            .await
            .expect("read")
            .expect("owner");
        let resumed = chats
            .start_execution(mutation(id, current.owner.version), 1)
            .await
            .expect("explicit resume");
        assert_eq!(resumed.session.generation, 2);
        ready(&chats, id, 2).await;
        assert_eq!(
            chats
                .execution_owner(id)
                .await
                .expect("resumed owner")
                .expect("owner")
                .session
                .vendor_session_id
                .as_deref(),
            Some(vendor_id.as_str())
        );
        let third = Uuid::new_v4();
        let admitted = chats
            .send_execution(
                id,
                2,
                current.owner.version,
                third,
                "third after resume".into(),
            )
            .await
            .expect("resumed send");
        assert_eq!(
            terminal(&chats, third).await.outcome,
            Some(ChatTurnOutcome::Completed)
        );
        assert_eq!(admitted.attempt.owner_run_id, first.attempt.owner_run_id);
        assert_eq!(
            chats
                .execution_attempts(id, 0, 32)
                .await
                .expect("attempts")
                .len(),
            3
        );
        let closed = chats
            .close_execution_process(id, 2)
            .await
            .expect("close resumed process");
        std::fs::write(
            Path::new(&workspace.path).join("candidate.txt"),
            "retained candidate",
        )
        .expect("candidate");
        let finalized = chats
            .stop_execution_owner(mutation(id, closed.owner.version))
            .await
            .expect("final Stop");
        assert_eq!(finalized.run.status(), RunStatus::Stopped);
        assert!(Path::new(&workspace.path).join("candidate.txt").is_file());
        assert!(
            chats
                .start_execution(start, 1)
                .await
                .expect("historical Start after finalization")
                .duplicate
        );
        assert!(
            chats
                .send_execution(id, 1, stored.owner.version, first_request, PROMPT.into())
                .await
                .expect("historical Send after finalization")
                .duplicate
        );
        assert_eq!(
            git(&fixture.git, &fixture.project, &["rev-parse", "HEAD"])
                .await
                .trim(),
            fixture.base
        );
        assert!(
            git(&fixture.git, &fixture.project, &["status", "--porcelain"])
                .await
                .trim()
                .is_empty()
        );
        stop(writer, dispatch, chats).await;
        std::fs::remove_file(Path::new(&workspace.path).join("candidate.txt"))
            .expect("remove only fixture candidate");
        git(
            &fixture.git,
            &fixture.project,
            &["worktree", "remove", "--", &workspace.path],
        )
        .await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owner_cancellation_and_process_loss_leave_the_lifetime_run_nonterminal() {
    for harness in [ChatHarness::Codex, ChatHarness::Claude] {
        for mode in ["hang", "crash"] {
            let fixture = Fixture::new(harness, mode).await;
            let (writer, dispatch, chats) = fixture.start().await;
            let opened = chats
                .open_execution_owner(Uuid::new_v4(), harness, None)
                .await
                .expect("open");
            let stored = provisioned(&chats, opened.owner.chat_id).await;
            let id = stored.owner.chat_id;
            chats
                .start_execution(mutation(id, stored.owner.version), 1)
                .await
                .expect("start");
            ready(&chats, id, 1).await;
            let request_id = Uuid::new_v4();
            chats
                .send_execution(id, 1, stored.owner.version, request_id, PROMPT.into())
                .await
                .expect("admit");
            if mode == "hang" {
                cancellable_input(&chats, &fixture, harness, request_id).await;
                chats
                    .cancel_execution(id, 1, request_id)
                    .await
                    .expect("cancel");
            }
            let attempt = terminal(&chats, request_id).await;
            assert_eq!(
                attempt.outcome,
                Some(if mode == "hang" {
                    ChatTurnOutcome::Cancelled
                } else {
                    ChatTurnOutcome::Interrupted
                })
            );
            let owner = chats
                .execution_owner(id)
                .await
                .expect("owner")
                .expect("owner");
            assert_eq!(owner.run.status(), RunStatus::Running);
            assert_eq!(owner.owner.phase, ChatOwnerPhase::Open);
            chats
                .close_execution_process(id, 1)
                .await
                .expect("join process cleanup");
            let current = chats
                .execution_owner(id)
                .await
                .expect("owner")
                .expect("owner");
            if harness == ChatHarness::Claude && mode == "crash" {
                assert_eq!(
                    chats
                        .start_execution(mutation(id, current.owner.version), 1)
                        .await
                        .expect_err("unconfirmed claimed root must not relaunch")
                        .code(),
                    "chat_unconfirmed_session"
                );
            }
            chats
                .stop_execution_owner(mutation(id, current.owner.version))
                .await
                .expect("finalize fixture after process cleanup");
            let workspace = stored.owner.workspace.as_ref().expect("fixture workspace");
            stop(writer, dispatch, chats).await;
            git(
                &fixture.git,
                &fixture.project,
                &["worktree", "remove", "--", &workspace.path],
            )
            .await;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owner_shutdown_drains_active_input_and_refuses_new_effects() {
    for harness in [ChatHarness::Codex, ChatHarness::Claude] {
        let fixture = Fixture::new(harness, "hang").await;
        let (writer, dispatch, chats) = fixture.start().await;
        let opened = chats
            .open_execution_owner(Uuid::new_v4(), harness, None)
            .await
            .expect("open");
        let stored = provisioned(&chats, opened.owner.chat_id).await;
        let id = stored.owner.chat_id;
        chats
            .start_execution(mutation(id, stored.owner.version), 1)
            .await
            .expect("start");
        ready(&chats, id, 1).await;
        let request_id = Uuid::new_v4();
        chats
            .send_execution(id, 1, stored.owner.version, request_id, PROMPT.into())
            .await
            .expect("send");
        cancellable_input(&chats, &fixture, harness, request_id).await;
        chats.begin_shutdown();
        assert_eq!(
            chats
                .send_execution(
                    id,
                    1,
                    stored.owner.version,
                    Uuid::new_v4(),
                    "must not reach vendor".into()
                )
                .await
                .expect_err("closed admission")
                .code(),
            "chat_service_stopped"
        );
        assert_eq!(
            chats
                .start_execution(mutation(id, stored.owner.version), 1)
                .await
                .expect_err("closed start admission")
                .code(),
            "chat_service_stopped"
        );
        chats
            .shutdown()
            .await
            .expect("join active process and cleanup");
        let attempt = chats
            .execution_attempt(request_id)
            .await
            .expect("read")
            .expect("attempt");
        // Closing requests cancellation; this fixture confirms it before exit.
        assert_eq!(attempt.outcome, Some(ChatTurnOutcome::Cancelled));
        assert!(attempt.correlated_terminal);
        let owner = chats
            .execution_owner(id)
            .await
            .expect("read")
            .expect("owner");
        assert_eq!(owner.run.status(), RunStatus::Running);
        assert_eq!(owner.session.state, ChatState::Suspended);
        assert_eq!(
            chats
                .execution_attempts(id, 0, 32)
                .await
                .expect("history")
                .len(),
            1
        );
        assert_eq!(
            std::fs::read_to_string(&fixture.trace)
                .expect("physical input")
                .lines()
                .count(),
            1
        );
        let workspace = stored.owner.workspace.as_ref().expect("fixture workspace");
        stop(writer, dispatch, chats).await;
        git(
            &fixture.git,
            &fixture.project,
            &["worktree", "remove", "--", &workspace.path],
        )
        .await;
    }
}
