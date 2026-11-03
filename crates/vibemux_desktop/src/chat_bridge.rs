//! Blocking Tauri boundary around the authenticated typed Control client.
use crate::command_error::CommandError;
use vibemux_client::daemon_session::DaemonSession;
use vibemuxd::{
    control::{
        ChatApprovalRequest, ChatApprovalResponse, ChatExecutionRequest, ChatExecutionResponse,
        ChatOwnerRequest, ChatOwnerResponse, ChatRequest, ChatResponse, ControlClient,
        ControlOperation,
    },
    process::DaemonPaths,
};

pub(crate) fn request(
    paths: &DaemonPaths,
    request: &ChatRequest,
) -> Result<ChatResponse, CommandError> {
    validate_request(request)?;
    let (runtime, client) = connect(paths)?;
    runtime.block_on(client.chat(request)).map_err(|error|CommandError::new(error.code(),
        "The conversation request could not be completed. Check its status before retrying a send."))
}

pub(crate) fn owner_request(
    paths: &DaemonPaths,
    request: &ChatOwnerRequest,
) -> Result<ChatOwnerResponse, CommandError> {
    request.validate().map_err(|_| {
        CommandError::new(
            "invalid_arguments",
            "Check the workspace request and required consent.",
        )
    })?;
    validate_control_frame(ControlOperation::ChatOwner, request)?;
    let (runtime, client) = connect(paths)?;
    runtime.block_on(client.chat_owner(request)).map_err(|error| CommandError::new(error.code(),
        "The workspace request could not be confirmed. Check its status or retry the same operation."))
}

pub(crate) fn execution_request(
    paths: &DaemonPaths,
    request: &ChatExecutionRequest,
) -> Result<ChatExecutionResponse, CommandError> {
    request.validate().map_err(|_| {
        CommandError::new(
            "invalid_arguments",
            "Check the conversation request and required consent.",
        )
    })?;
    validate_control_frame(ControlOperation::ChatExecution, request)?;
    let (runtime, client) = connect(paths)?;
    runtime.block_on(client.chat_execution(request)).map_err(|error|CommandError::new(error.code(),"The conversation request could not be confirmed. Refresh its receipt before retrying the same action."))
}

pub(crate) fn connect(
    paths: &DaemonPaths,
) -> Result<(tokio::runtime::Runtime, ControlClient), CommandError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| {
            CommandError::new(
                "desktop_runtime_unavailable",
                "The conversation client could not start.",
            )
        })?;
    let (client, _) = DaemonSession::default().connect(paths).map_err(|_| {
        CommandError::new(
            "chat_daemon_unavailable",
            "Start the project's daemon, then refresh the conversation.",
        )
    })?;
    Ok((runtime, client))
}

pub(crate) fn search_request(
    paths: &DaemonPaths,
    request: &vibemux_client::chat_search::SearchRequest,
) -> Result<vibemux_client::chat_search::SearchPage, CommandError> {
    request
        .validate()
        .map_err(|e| CommandError::new(e.code(), e.message()))?;
    let (runtime, client) = connect(paths)?;
    runtime
        .block_on(vibemux_client::chat_search::search_conversations(
            &client, request,
        ))
        .map_err(|e| CommandError::new(e.code(), e.message()))
}

pub(crate) fn approval_request(
    paths: &DaemonPaths,
    request: &ChatApprovalRequest,
) -> Result<ChatApprovalResponse, CommandError> {
    request.validate().map_err(|_| {
        CommandError::new(
            "invalid_arguments",
            "Check the approval identity, version and decision.",
        )
    })?;
    validate_control_frame(ControlOperation::ChatApproval, request)?;
    let (runtime, client) = connect(paths)?;
    runtime.block_on(client.chat_approval(request)).map_err(|error|CommandError::new(error.code(),
        "The approval could not be confirmed. Check its receipt before making another decision."))
}

fn validate_request(request: &ChatRequest) -> Result<(), CommandError> {
    request.validate().map_err(|_| {
        CommandError::new(
            "invalid_arguments",
            "Check the conversation request and its text limits.",
        )
    })?;
    validate_control_frame(ControlOperation::Chat, request)
}

fn validate_control_frame(
    operation: ControlOperation,
    request: &impl serde::Serialize,
) -> Result<(), CommandError> {
    use vibemuxd::control::{CONTROL_PROTOCOL_VERSION, ControlRequest, MAX_CONTROL_FRAME_BYTES};
    let argument = serde_json::to_string(request).map_err(|_| {
        CommandError::new(
            "invalid_arguments",
            "The conversation request could not be encoded.",
        )
    })?;
    let frame = ControlRequest {
        version: CONTROL_PROTOCOL_VERSION,
        request_id: "0".repeat(36),
        token: "0".repeat(64),
        operation,
        argument: Some(argument),
    };
    let length = serde_json::to_vec(&frame)
        .map_err(|_| {
            CommandError::new(
                "invalid_arguments",
                "The conversation request could not be encoded.",
            )
        })?
        .len();
    if length > MAX_CONTROL_FRAME_BYTES {
        return Err(CommandError::new(
            "prompt_frame_too_large",
            "The message contains too much escaped text for one request. Shorten it before sending.",
        ));
    }
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{collections::BTreeMap, time::Duration};
    use uuid::Uuid;
    use vibemux_harness::HarnessDetection;
    use vibemux_types::chat::{ChatHarness, ChatState, ChatTurnOutcome};
    use vibemuxd::{WriterWorker, control::DaemonControlServer};

    #[test]
    fn prompt_frame_is_refused_before_connecting_or_admitting() {
        let request = ChatRequest::Send {
            chat_id: vibemux_types::ChatId::new(),
            generation: 1,
            request_id: Uuid::new_v4(),
            prompt: "\u{1}".repeat(20_000),
        };
        assert!(request.validate().is_ok());
        assert_eq!(
            validate_request(&request)
                .expect_err("outer JSON frame")
                .code,
            "prompt_frame_too_large"
        );
        let normal = ChatRequest::Send {
            chat_id: vibemux_types::ChatId::new(),
            generation: 1,
            request_id: Uuid::new_v4(),
            prompt: "中".repeat(10_000),
        };
        validate_request(&normal).expect("bounded Unicode request");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "build vibemux_chat_fixture and stage the sibling launch trampoline before native acceptance"]
    async fn desktop_chat_bridge_reaches_authenticated_saved_conversation() {
        let image = std::env::current_exe().expect("test image");
        let profile = image.parent().expect("deps").parent().expect("profile");
        let fixture = profile.join("vibemux_chat_fixture.exe");
        assert!(fixture.is_file());
        let temp = tempfile::tempdir().expect("fixture root");
        let project = temp.path().join("project 中文");
        let vendor = temp.path().join("vendor");
        let local = temp.path().join("private_local_app_data");
        std::fs::create_dir_all(project.join(".git")).expect("project");
        std::fs::create_dir(&vendor).expect("vendor");
        std::fs::create_dir(&local).expect("isolated runtime");
        std::fs::write(
            project.join(".git/HEAD"),
            "0123456789abcdef0123456789abcdef01234567\n",
        )
        .expect("head");
        let executable = vendor.join("codex.exe");
        std::fs::copy(fixture, &executable).expect("synthetic vendor");
        std::fs::write(vendor.join("chat_fixture.json"), br#"{"mode":"normal"}"#)
            .expect("fixture mode");
        let paths =
            DaemonPaths::from_project_root_with_local_app_data(&project, &local).expect("paths");
        paths.ensure_runtime_dir().expect("runtime");
        std::fs::write(paths.state_dir().join("harness_dispatch.json"),serde_json::to_vec(&json!({"schema_version":1,"routes":[{
            "harness":"codex","protocol":"codex_app_server","executable":executable,"enabled":true,"allow_execution":true}]})).expect("config")).expect("operator config");
        let database = paths.database_path().to_owned();
        tokio::task::spawn_blocking(move || {
            let writer = WriterWorker::start(&database).expect("writer");
            writer
                .handle()
                .expect("handle")
                .commit_harness_snapshot(
                    BTreeMap::from([(
                        "codex".to_owned(),
                        HarnessDetection {
                            detected: true,
                            path: None,
                            launcher: None,
                            version: Some("synthetic".to_owned()),
                        },
                    )]),
                    "2026-11-01T00:00:00Z".to_owned(),
                    "desktop_chat_test".to_owned(),
                )
                .expect("detection");
            writer.shutdown().expect("seed shutdown");
        })
        .await
        .expect("seed task");
        let server = DaemonControlServer::start_for_paths(&paths)
            .await
            .expect("server");
        let call = |query: ChatRequest| {
            let paths = paths.clone();
            async move {
                tokio::task::spawn_blocking(move || request(&paths, &query))
                    .await
                    .expect("blocking bridge")
                    .expect("bridge result")
            }
        };
        let ChatResponse::Session { session, .. } = call(ChatRequest::Open {
            operation_id: Uuid::new_v4(),
            harness: ChatHarness::Codex,
            title: Some("PRIVATE_BRIDGE_TITLE".to_owned()),
            consent_private_storage: true,
        })
        .await
        else {
            panic!("receipt")
        };
        let chat_id = session.chat_id;
        tokio::time::timeout(Duration::from_secs(12), async {
            loop {
                if let ChatResponse::Status {
                    session: Some(session),
                } = call(ChatRequest::Status { chat_id }).await
                    && session.state == ChatState::Ready
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("ready");
        let request_id = Uuid::new_v4();
        call(ChatRequest::Send {
            chat_id,
            generation: 1,
            request_id,
            prompt: "PRIVATE_BRIDGE_PROMPT".to_owned(),
        })
        .await;
        tokio::time::timeout(Duration::from_secs(12), async {
            loop {
                if let ChatResponse::Submission {
                    turn: Some(turn), ..
                } = call(ChatRequest::Submission {
                    chat_id,
                    generation: 1,
                    request_id,
                })
                .await
                    && turn.phase.is_terminal()
                {
                    assert_eq!(turn.outcome, Some(ChatTurnOutcome::Completed));
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("completion");
        let response = call(ChatRequest::Content {
            chat_id,
            cursor: Default::default(),
        })
        .await;
        assert!(!format!("{response:?}").contains("PRIVATE_BRIDGE"));
        let ChatResponse::Content { page } = response else {
            panic!("content")
        };
        assert_eq!(page.text.as_deref(), Some("PRIVATE_BRIDGE_PROMPT"));
        let export_session = tokio::time::timeout(Duration::from_secs(12), async {
            loop {
                if let ChatResponse::Status {
                    session: Some(session),
                } = call(ChatRequest::Status { chat_id }).await
                    && session.state == ChatState::Ready
                    && session.active_request_id.is_none()
                {
                    break session;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("quiescent export");
        for format in [
            vibemux_client::chat_export::ExportFormat::Json,
            vibemux_client::chat_export::ExportFormat::Text,
        ] {
            let export_request = vibemux_client::chat_export::ExportRequest {
                kind: vibemux_client::chat_export::ExportKind::Chat,
                format,
                chat_id,
                generation: export_session.generation,
                expected_version: export_session.version,
            };
            let destination = temp
                .path()
                .join(format!("selected_history.{}", format.extension()));
            let export_paths = paths.clone();
            let selected = destination.clone();
            let receipt = tokio::task::spawn_blocking(move || {
                crate::chat_export::request(
                    &export_paths,
                    &export_request,
                    |_| Ok(Some(selected)),
                    || Ok(()),
                )
            })
            .await
            .expect("export worker")
            .expect("native selected save");
            assert_eq!(
                receipt.status,
                vibemux_client::chat_export::ExportStatus::Saved
            );
            assert!(receipt.record_count >= 2 && receipt.byte_count > 0);
            let copied = std::fs::read_to_string(&destination).expect("saved conversation");
            assert_eq!(receipt.byte_count, copied.len() as u64);
            assert!(
                copied.contains("PRIVATE_BRIDGE_TITLE") && copied.contains("PRIVATE_BRIDGE_PROMPT")
            );
            assert!(
                !copied.contains("blob_id")
                    && !copied.contains("sha256")
                    && !copied.contains("vendor_session")
            );
            if format == vibemux_client::chat_export::ExportFormat::Json {
                let value: serde_json::Value =
                    serde_json::from_str(&copied).expect("versioned JSON");
                assert_eq!(value["schema_version"], 1);
                assert_eq!(value["session"]["chat_id"], chat_id.to_string());
            }
        }
        let ChatResponse::Status {
            session: Some(after_export),
        } = call(ChatRequest::Status { chat_id }).await
        else {
            panic!("export status")
        };
        assert_eq!(
            after_export, export_session,
            "export creates no canonical chat mutation"
        );
        let search_paths = paths.clone();
        let search_page = tokio::task::spawn_blocking(move || {
            search_request(
                &search_paths,
                &vibemux_client::chat_search::SearchRequest {
                    query: "PRIVATE_BRIDGE".into(),
                    case_sensitive: false,
                    scope: vibemux_client::chat_search::SearchScope::All,
                    include_archived: false,
                    limit: 25,
                    cursor: None,
                },
            )
        })
        .await
        .expect("native search worker")
        .expect("authenticated retained search");
        assert!(search_page.complete && search_page.hits.len() >= 2);
        assert!(search_page.hits.iter().all(|hit| hit.chat_id == chat_id
            && hit.kind == vibemux_client::chat_history::ConversationKind::Chat));
        assert!(search_page.hits.iter().any(|hit| matches!(
            hit.location,
            vibemux_client::chat_search::SearchTarget::Title
        )));
        assert!(search_page.hits.iter().any(|hit| matches!(
            hit.location,
            vibemux_client::chat_search::SearchTarget::Record { .. }
        )));
        assert!(!format!("{search_page:?}").contains("PRIVATE_BRIDGE"));
        let ChatResponse::Status {
            session: Some(after_search),
        } = call(ChatRequest::Status { chat_id }).await
        else {
            panic!("search status")
        };
        assert_eq!(
            after_search, export_session,
            "search creates no canonical mutation"
        );
        call(ChatRequest::Close {
            operation_id: Uuid::new_v4(),
            chat_id,
            generation: 1,
        })
        .await;
        call(ChatRequest::Archive {
            operation_id: Uuid::new_v4(),
            chat_id,
            generation: 1,
        })
        .await;
        for include_archived in [false, true] {
            let search_paths = paths.clone();
            let page = tokio::task::spawn_blocking(move || {
                search_request(
                    &search_paths,
                    &vibemux_client::chat_search::SearchRequest {
                        query: "PRIVATE_BRIDGE".into(),
                        case_sensitive: false,
                        scope: vibemux_client::chat_search::SearchScope::Saved,
                        include_archived,
                        limit: 25,
                        cursor: None,
                    },
                )
            })
            .await
            .expect("archive search worker")
            .expect("archived history search");
            assert!(page.complete);
            assert_eq!(
                page.hits.is_empty(),
                !include_archived,
                "archive option controls retained discovery"
            );
        }
        server.shutdown().await.expect("shutdown");
        assert_eq!(
            std::fs::read_to_string(project.join(".git/HEAD")).expect("head after"),
            "0123456789abcdef0123456789abcdef01234567\n"
        );
    }
}
