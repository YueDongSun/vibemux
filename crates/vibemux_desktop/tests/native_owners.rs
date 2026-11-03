#![cfg(windows)]
//! Explicit native acceptance for the desktop's registered-project owner bridge.
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::Path,
    process::{Command, Stdio},
    sync::Arc,
    time::Duration,
};
use uuid::Uuid;
use vibemux_desktop::{app_state::AppState, event_sink::RecordingEventSink};
use vibemux_harness::HarnessDetection;
use vibemux_types::{chat::ChatHarness, chat_owner::ChatOwnerPhase};
use vibemuxd::{
    WriterWorker,
    control::{
        ChatExecutionRequest, ChatExecutionResponse, ChatOwnerRequest, ChatOwnerResponse,
        DaemonControlServer,
    },
    process::DaemonPaths,
};

#[path = "support/link_effects_fixture.rs"]
mod link_effects_fixture;
use link_effects_fixture::{LinkEffect, RecordedLinkEffects};

async fn assert_workspace_presentation(
    state: Arc<AppState>,
    key: String,
    client: &vibemuxd::control::ControlClient,
    query: vibemuxd::control::WorkspaceReadQuery,
    effects: &RecordedLinkEffects,
) {
    assert_native_cleanup_status(Arc::clone(&state), key.clone(), query.task_id, query.run_id)
        .await;
    use vibemux_desktop::link_contract::{OpenLinkRequest, WorktreeAction};
    let expected = client
        .workspace_read(query.clone())
        .await
        .expect("fresh managed directory");
    let directory = expected.directory.expect("directory").path;
    let branch = expected.branch.expect("complete recorded branch");
    let offset = effects.events.lock().expect("effects").len();
    let replies = tokio::task::spawn_blocking(move || {
        [
            WorktreeAction::CopyBranch,
            WorktreeAction::CopyPath,
            WorktreeAction::Open,
        ]
        .map(|action| {
            let reply = state
                .open_link(
                    "main",
                    OpenLinkRequest::Worktree {
                        project_key: key.clone(),
                        task_id: query.task_id,
                        run_id: query.run_id,
                        action,
                    },
                )
                .expect("managed native presentation");
            serde_json::to_value(reply).expect("acknowledgement")
        })
    })
    .await
    .expect("native presentation worker");
    assert_eq!(
        &effects.events.lock().expect("effects")[offset..],
        [
            LinkEffect::Copy(branch),
            LinkEffect::Copy(directory.clone()),
            LinkEffect::Directory(directory.into()),
        ]
    );
    for reply in replies {
        assert_eq!(reply.as_object().expect("closed reply").len(), 6);
        assert!(reply.get("path").is_none() && reply.get("branch").is_none());
    }
}

async fn assert_native_cleanup_status(
    state: Arc<AppState>,
    key: String,
    task_id: vibemux_types::TaskId,
    run_id: vibemux_types::RunId,
) {
    let expected = key.clone();
    let value=tokio::task::spawn_blocking(move || {
        use tauri::ipc::{CallbackFn,InvokeBody};
        let app=tauri::test::mock_builder().manage(state)
            .invoke_handler(tauri::generate_handler![vibemux_desktop::commands::workspace_cleanup_request])
            .build(tauri::generate_context!()).expect("native capability context");
        let window=tauri::WebviewWindowBuilder::new(&app,"main",Default::default()).build().expect("Main");
        tauri::test::get_ipc_response(&window,tauri::webview::InvokeRequest {
            cmd:"workspace_cleanup_request".into(),callback:CallbackFn(0),error:CallbackFn(1),url:"http://tauri.localhost".parse().expect("origin"),
            body:InvokeBody::Json(json!({"request":{"operation":"status","registration_key":key,"task_id":task_id,"run_id":run_id,"cleanup_id":null}})),
            headers:Default::default(),invoke_key:tauri::test::INVOKE_KEY.into(),
        }).map(|response|response.deserialize::<serde_json::Value>().expect("public response")).expect("registered Main-to-daemon status")
    }).await.expect("native worker");
    assert_eq!(value["target"]["registration_key"], expected);
    assert_eq!(value["target"]["task_id"], task_id.to_string());
    assert_eq!(value["target"]["run_id"], run_id.to_string());
    assert!(value["view"].is_null());
    assert_eq!(value.as_object().expect("closed reply").len(), 2);
}

const RECEIPT_OBSERVATION: Duration = Duration::from_secs(60);
// The service owns provisioning through its bounded 600-second drain.
const PROVISION_OBSERVATION: Duration = Duration::from_secs(600);

fn git(executable: &Path, root: &Path, args: &[&str]) -> String {
    use std::os::windows::process::CommandExt;
    let mut command = Command::new(executable);
    command
        .current_dir(root)
        .args([
            "-c",
            "user.name=Desktop Owner Fixture",
            "-c",
            "user.email=fixture@invalid.test",
            "-c",
            "core.hooksPath=NUL",
            "-c",
            "commit.gpgSign=false",
        ])
        .args(args)
        .env_clear()
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "NUL")
        .stdin(Stdio::null())
        .creation_flags(0x08000000);
    for key in ["SystemRoot", "WINDIR", "TEMP", "TMP", "PATH"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    let output = command.output().expect("fixture Git process");
    assert!(
        output.status.success(),
        "fixture Git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("UTF8 Git output")
}

async fn request(
    state: Arc<AppState>,
    key: String,
    request: ChatOwnerRequest,
) -> ChatOwnerResponse {
    let call_state = state.clone();
    let call_key = key.clone();
    let call_request = request.clone();
    let result =
        tokio::task::spawn_blocking(move || call_state.chat_owner_request(&call_key, call_request))
            .await
            .expect("desktop worker");
    match (result, request) {
        (Ok(response), _) => response,
        (Err(error), ChatOwnerRequest::Open { operation_id, .. })
            if error.code == "writer_response_timeout" =>
        {
            tokio::time::timeout(RECEIPT_OBSERVATION, async {
                loop {
                    let lookup_state = state.clone();
                    let lookup_key = key.clone();
                    match tokio::task::spawn_blocking(move || {
                        lookup_state.chat_owner_request(
                            &lookup_key,
                            ChatOwnerRequest::LookupOpen { operation_id },
                        )
                    })
                    .await
                    .expect("receipt worker")
                    {
                        Ok(ChatOwnerResponse::Status { owner: Some(owner) }) => {
                            break ChatOwnerResponse::Status { owner: Some(owner) };
                        }
                        Ok(ChatOwnerResponse::Status { owner: None }) => {}
                        Err(error) if error.code == "writer_response_timeout" => {}
                        other => panic!("native Open receipt: {other:?}"),
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .expect("bounded Open receipt observation")
        }
        (other, _) => panic!("owner bridge: {other:?}"),
    }
}

async fn execution_request(
    state: Arc<AppState>,
    key: String,
    request: ChatExecutionRequest,
) -> ChatExecutionResponse {
    tokio::task::spawn_blocking(move || state.chat_execution_request(&key, request))
        .await
        .expect("desktop execution worker")
        .expect("execution bridge")
}

async fn execution_start(state: Arc<AppState>, key: String, request: ChatExecutionRequest) {
    let ChatExecutionRequest::Start {
        operation_id,
        chat_id,
        generation,
        owner_version,
        ..
    } = request.clone()
    else {
        panic!("Start request");
    };
    let call_state = state.clone();
    let call_key = key.clone();
    match tokio::task::spawn_blocking(move || call_state.chat_execution_request(&call_key, request))
        .await
        .expect("native Start worker")
    {
        Ok(ChatExecutionResponse::Process {
            duplicate: false, ..
        }) => {}
        Err(error)
            if matches!(
                error.code.as_str(),
                "chat_acknowledgement_uncertain" | "writer_response_timeout"
            ) =>
        {
            tokio::time::timeout(RECEIPT_OBSERVATION, async {
                loop {
                    let lookup_state = state.clone();
                    let lookup_key = key.clone();
                    match tokio::task::spawn_blocking(move || {
                        lookup_state.chat_execution_request(
                            &lookup_key,
                            ChatExecutionRequest::LookupStart {
                                operation_id,
                                chat_id,
                                generation,
                                owner_version,
                            },
                        )
                    })
                    .await
                    .expect("native Start lookup")
                    {
                        Ok(ChatExecutionResponse::StartReceipt {
                            runtime: Some(_),
                            sequence: Some(_),
                        }) => break,
                        Ok(ChatExecutionResponse::StartReceipt {
                            runtime: None,
                            sequence: None,
                        }) => {}
                        Err(error)
                            if matches!(
                                error.code.as_str(),
                                "chat_acknowledgement_uncertain" | "writer_response_timeout"
                            ) => {}
                        other => panic!("native Start receipt: {other:?}"),
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .expect("bounded native Start receipt observation");
        }
        other => panic!("native Start: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "build vibemux_chat_fixture and stage the sibling launch trampoline before native acceptance"]
async fn registered_desktop_workspace_creates_reopens_and_stops_without_losing_files() {
    let image = std::env::current_exe().expect("test image");
    let profile = image.parent().expect("deps").parent().expect("profile");
    let fixture = profile.join("vibemux_chat_fixture.exe");
    assert!(fixture.is_file(), "build the synthetic fixture first");
    let temp = tempfile::tempdir().expect("temporary fixture");
    let project = temp.path().join("project 中文 with spaces");
    let vendor = temp.path().join("vendor");
    std::fs::create_dir(&project).expect("project");
    std::fs::create_dir(&vendor).expect("vendor");
    let git_executable = std::env::split_paths(&std::env::var_os("PATH").expect("PATH"))
        .filter(|path| path.is_absolute())
        .map(|path| path.join("git.exe"))
        .find(|path| path.is_file())
        .expect("native Git");
    git(
        &git_executable,
        &project,
        &["init", "--quiet", "--initial-branch=main"],
    );
    std::fs::write(project.join(".gitignore"), ".vibemux/\n").expect("ignore state");
    std::fs::write(project.join("base.txt"), "main stays intact\n").expect("base");
    git(
        &git_executable,
        &project,
        &["add", ".gitignore", "base.txt"],
    );
    git(
        &git_executable,
        &project,
        &["commit", "--quiet", "-m", "fixture base"],
    );
    let base = git(&git_executable, &project, &["rev-parse", "HEAD"]);
    let paths = DaemonPaths::from_project_root(&project).expect("paths");
    paths.ensure_runtime_dir().expect("runtime");
    let executable = vendor.join("codex.exe");
    std::fs::copy(fixture, &executable).expect("synthetic vendor");
    let launch_marker = vendor.join("launch.txt");
    std::fs::write(
        vendor.join("chat_fixture.json"),
        serde_json::to_vec(&json!({"mode":"normal","launch_path":launch_marker}))
            .expect("fixture JSON"),
    )
    .expect("fixture settings");
    std::fs::write(paths.state_dir().join("harness_dispatch.json"),serde_json::to_vec(&json!({"schema_version":1,"routes":[{
        "harness":"codex","protocol":"codex_app_server","executable":executable,"enabled":true,"allow_execution":true}]})).expect("route JSON")).expect("route");
    let database = paths.database_path().to_owned();
    tokio::task::spawn_blocking(move || {
        let writer = WriterWorker::start(&database).expect("seed writer");
        writer
            .handle()
            .expect("handle")
            .commit_harness_snapshot(
                BTreeMap::from([(
                    "codex".into(),
                    HarnessDetection {
                        detected: true,
                        path: None,
                        launcher: None,
                        version: Some("synthetic".into()),
                    },
                )]),
                "2026-11-01T00:00:00Z".into(),
                "desktop_owner_detection".into(),
            )
            .expect("detection");
        writer.shutdown().expect("seed shutdown");
    })
    .await
    .expect("seed task");
    let server = DaemonControlServer::start_for_paths(&paths)
        .await
        .expect("daemon");
    let settings = temp.path().join("settings.json");
    let link_effects = Arc::new(RecordedLinkEffects::default());
    let state = Arc::new(
        AppState::new(settings.clone(), Arc::new(RecordingEventSink::default()))
            .with_link_effects(link_effects.clone()),
    );
    let key = state.register_project(&project).expect("register identity");
    let open = ChatOwnerRequest::Open {
        operation_id: Uuid::new_v4(),
        harness: ChatHarness::Codex,
        title: Some("PRIVATE_WORKSPACE_TITLE_中文".into()),
        consent_private_storage: true,
        consent_workspace: true,
    };
    let owner = match request(state.clone(), key.clone(), open.clone()).await {
        ChatOwnerResponse::Owner { owner, .. }
        | ChatOwnerResponse::Status { owner: Some(owner) } => owner,
        _ => panic!("owner receipt"),
    };
    let chat_id = owner.chat_id;
    let mut reconciled = false;
    let ready = tokio::time::timeout(PROVISION_OBSERVATION, async {
        loop {
            let ChatOwnerResponse::Status {
                owner: Some(current),
            } = request(
                state.clone(),
                key.clone(),
                ChatOwnerRequest::Status { chat_id },
            )
            .await
            else {
                panic!("known owner")
            };
            assert_ne!(
                current.phase,
                ChatOwnerPhase::RecoveryRequired,
                "provisioning failure {current:?}"
            );
            if current.phase == ChatOwnerPhase::Provisioning
                && !current.provisioning_active
                && !reconciled
            {
                request(
                    state.clone(),
                    key.clone(),
                    ChatOwnerRequest::Reconcile {
                        operation_id: Uuid::new_v4(),
                        chat_id,
                        expected_version: current.version,
                    },
                )
                .await;
                reconciled = true;
            }
            if current.phase == ChatOwnerPhase::Open && !current.provisioning_active {
                break current;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("bounded provisioning");
    assert!(ready.admitted_tools.is_empty());
    assert!(
        !serde_json::to_string(&ready)
            .expect("public status")
            .contains("PRIVATE_")
    );
    assert!(!launch_marker.exists());
    let workspace_client =
        vibemuxd::control::ControlClient::from_descriptor(server.descriptor_path())
            .expect("workspace client");
    let workspace_query = vibemuxd::control::WorkspaceReadQuery {
        project_id: ready.project_id,
        task_id: ready.task_id,
        run_id: ready.run_id,
        mode: vibemuxd::control::WorkspaceReadMode::Directory,
    };
    assert_workspace_presentation(
        state.clone(),
        key.clone(),
        &workspace_client,
        workspace_query.clone(),
        &link_effects,
    )
    .await;
    assert!(
        !launch_marker.exists(),
        "folder and copy actions cannot launch the execution fixture"
    );
    assert!(
        matches!(request(state.clone(),key,open).await,ChatOwnerResponse::Owner {owner,duplicate:true,..} if owner.run_id==ready.run_id)
    );
    let start = ChatExecutionRequest::Start {
        operation_id: Uuid::new_v4(),
        chat_id,
        generation: 1,
        owner_version: ready.version,
        consent_execution: true,
    };
    let denied_state = state.clone();
    let denied_key = state.projects_list().expect("registered projects")[0]
        .project_key
        .clone();
    let denied_request = start.clone();
    let denied = tokio::task::spawn_blocking(move || {
        denied_state.chat_execution_request(&denied_key, denied_request)
    })
    .await
    .expect("consent check")
    .expect_err("native inference consent required");
    assert_eq!(denied.code, "chat_consent_required");
    assert!(
        !launch_marker.exists(),
        "denied start cannot launch a vendor"
    );
    let key = state.projects_list().expect("registered projects")[0]
        .project_key
        .clone();
    let consent_state = state.clone();
    let consent_key = key.clone();
    tokio::task::spawn_blocking(move || consent_state.chat_confirm(&consent_key))
        .await
        .expect("consent worker")
        .expect("native consent");
    execution_start(state.clone(), key.clone(), start).await;
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if matches!(execution_request(state.clone(), key.clone(), ChatExecutionRequest::Status { chat_id }).await,
                ChatExecutionResponse::Status { runtime: Some(runtime) } if runtime.process_claimed && runtime.session.state == vibemux_types::chat::ChatState::Ready) { break; }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }).await.expect("native execution ready");
    let request_id = Uuid::new_v4();
    let send = ChatExecutionRequest::Send {
        chat_id,
        generation: 1,
        owner_version: ready.version,
        request_id,
        prompt: "PRIVATE_NATIVE_OWNER_PROMPT_中文 ; $(literal)".into(),
    };
    assert!(matches!(
        execution_request(state.clone(), key.clone(), send.clone()).await,
        ChatExecutionResponse::Attempt {
            duplicate: false,
            ..
        }
    ));
    let finished = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let ChatExecutionResponse::Submission {
                runtime,
                attempt: Some(attempt),
            } = execution_request(
                state.clone(),
                key.clone(),
                ChatExecutionRequest::Submission {
                    chat_id,
                    generation: 1,
                    request_id,
                },
            )
            .await
                && attempt.phase == vibemux_types::chat::ChatTurnPhase::Completed
            {
                break (runtime, attempt);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("native correlated completion");
    assert_eq!(finished.0.owner.run_id, ready.run_id);
    assert_eq!(finished.0.owner.phase, ChatOwnerPhase::Open);
    assert!(finished.1.correlated_terminal && finished.1.output_complete);
    assert!(
        !serde_json::to_string(&finished)
            .expect("metadata JSON")
            .contains("PRIVATE_")
    );
    assert!(
        matches!(execution_request(state.clone(), key.clone(), send).await, ChatExecutionResponse::Attempt { duplicate: true, attempt, .. } if attempt.attempt_id == finished.1.attempt_id)
    );
    let ChatExecutionResponse::Content { page } = execution_request(
        state.clone(),
        key.clone(),
        ChatExecutionRequest::Content {
            chat_id,
            cursor: vibemuxd::chat_session::api::ChatContentCursor {
                after_sequence: 0,
                byte_offset: 0,
            },
        },
    )
    .await
    else {
        panic!("private content page");
    };
    assert_eq!(
        page.text.as_deref(),
        Some("PRIVATE_NATIVE_OWNER_PROMPT_中文 ; $(literal)")
    );
    let ChatExecutionResponse::Status {
        runtime: Some(closed),
    } = execution_request(
        state.clone(),
        key.clone(),
        ChatExecutionRequest::Close {
            chat_id,
            generation: 1,
        },
    )
    .await
    else {
        panic!("closed runtime");
    };
    assert!(!closed.process_claimed);
    assert_eq!(closed.owner.phase, ChatOwnerPhase::Open);
    let export_request = vibemux_client::chat_export::ExportRequest {
        kind: vibemux_client::chat_export::ExportKind::Workspace,
        format: vibemux_client::chat_export::ExportFormat::Json,
        chat_id,
        generation: closed.session.generation,
        expected_version: closed.session.version,
    };
    let connect_paths = paths.clone();
    let (export_client, _) = tokio::task::spawn_blocking(move || {
        vibemux_client::daemon_session::DaemonSession::default().connect(&connect_paths)
    })
    .await
    .expect("export connection worker")
    .expect("authenticated export client");
    let snapshot =
        vibemux_client::chat_export::collect_conversation(&export_client, &export_request)
            .await
            .expect("workspace snapshot");
    assert_eq!(snapshot.session, closed.session);
    assert!(snapshot.records.len() >= 2);
    assert!(
        snapshot
            .turns
            .iter()
            .all(|turn| turn.run_id == ready.run_id)
    );
    let bytes = vibemux_client::chat_export::format_export(&snapshot, export_request.format)
        .expect("workspace JSON");
    vibemux_client::chat_export::verify_snapshot(&export_client, &snapshot)
        .await
        .expect("unchanged workspace");
    let destination = temp.path().join("selected_workspace.json");
    let selected = destination.clone();
    tokio::task::spawn_blocking(move || {
        vibemux_client::chat_export::write_export(&selected, &bytes, export_request.format)
    })
    .await
    .expect("native export writer")
    .expect("selected new file");
    let exported = std::fs::read_to_string(destination).expect("workspace export readback");
    assert!(
        exported.contains("PRIVATE_WORKSPACE_TITLE_中文")
            && exported.contains("PRIVATE_NATIVE_OWNER_PROMPT_中文 ; $(literal)")
    );
    assert!(
        !exported.contains("blob_id")
            && !exported.contains("sha256")
            && !exported.contains("vendor_session")
    );
    let mut search_request = vibemux_client::chat_search::SearchRequest {
        query: "PRIVATE_".into(),
        case_sensitive: false,
        scope: vibemux_client::chat_search::SearchScope::All,
        include_archived: false,
        limit: 1,
        cursor: None,
    };
    let mut search_hits = Vec::new();
    let mut search_complete = false;
    for _ in 0..8 {
        let page =
            vibemux_client::chat_search::search_conversations(&export_client, &search_request)
                .await
                .expect("paged owner history search");
        assert!(!format!("{page:?}").contains("PRIVATE_"));
        search_hits.extend(page.hits);
        if page.complete {
            search_complete = true;
            break;
        }
        assert!(page.next_cursor.is_some() && page.next_cursor != search_request.cursor);
        search_request.cursor = page.next_cursor;
    }
    assert!(search_complete && search_hits.len() >= 2);
    assert!(search_hits.iter().all(|hit| hit.chat_id == chat_id
        && hit.kind == vibemux_client::chat_history::ConversationKind::Workspace));
    assert!(
        search_hits
            .iter()
            .any(|hit| hit.preview.matched == "PRIVATE_")
    );
    let status = vibemux_client::chat_history::HistoryClient::new(
        &export_client,
        vibemux_client::chat_history::ConversationKind::Workspace,
        chat_id,
    )
    .status()
    .await
    .expect("unchanged owner history");
    assert_eq!(status, closed.session);
    let stop_version = closed.owner.version;
    let worktree = project
        .join(".vibemux/worktrees")
        .join(ready.run_id.to_string());
    std::fs::write(worktree.join("candidate.txt"), "retained candidate\n").expect("candidate");
    drop(state);
    let reopened = Arc::new(
        AppState::new(settings, Arc::new(RecordingEventSink::default()))
            .with_link_effects(link_effects.clone()),
    );
    let key = reopened.projects_list().expect("saved projects")[0]
        .project_key
        .clone();
    assert!(
        matches!(request(reopened.clone(),key.clone(),ChatOwnerRequest::List {after:None,limit:16,include_finalized:true}).await,
        ChatOwnerResponse::List {owners,..} if owners.len()==1 && owners[0].run_id==ready.run_id)
    );
    assert!(
        matches!(request(reopened.clone(),key.clone(),ChatOwnerRequest::Title {chat_id}).await,
        ChatOwnerResponse::Title {title:Some(title),..} if title=="PRIVATE_WORKSPACE_TITLE_中文")
    );
    let stopped = request(
        reopened.clone(),
        key.clone(),
        ChatOwnerRequest::Stop {
            operation_id: Uuid::new_v4(),
            chat_id,
            expected_version: stop_version,
        },
    )
    .await;
    assert!(
        matches!(stopped,ChatOwnerResponse::Owner {owner,..} if owner.phase==ChatOwnerPhase::Finalized)
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join("candidate.txt")).expect("candidate retained"),
        "retained candidate\n"
    );
    assert_workspace_presentation(
        reopened,
        key,
        &workspace_client,
        workspace_query,
        &link_effects,
    )
    .await;
    for include_archived in [false, true] {
        let page = vibemux_client::chat_search::search_conversations(
            &export_client,
            &vibemux_client::chat_search::SearchRequest {
                query: "PRIVATE_".into(),
                case_sensitive: false,
                scope: vibemux_client::chat_search::SearchScope::Workspace,
                include_archived,
                limit: 25,
                cursor: None,
            },
        )
        .await
        .expect("finalized owner search");
        assert!(page.complete);
        assert_eq!(
            page.hits.is_empty(),
            !include_archived,
            "finalized history requires explicit inclusion"
        );
    }
    server.shutdown().await.expect("shutdown");
    std::fs::remove_file(worktree.join("candidate.txt")).expect("remove test candidate");
    git(
        &git_executable,
        &project,
        &["worktree", "remove", worktree.to_str().expect("UTF8 path")],
    );
    assert_eq!(git(&git_executable, &project, &["rev-parse", "HEAD"]), base);
    assert!(git(&git_executable, &project, &["status", "--porcelain"]).is_empty());
}
