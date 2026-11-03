//! Fixed typed webview commands. Paths and daemon credentials stay in Rust.
use std::sync::Arc;
use tauri::{State, ipc::Request};
use vibemux_client::daemon_control::DaemonCommand;

use crate::{
    app_state::AppState,
    command_arguments::{
        self, AskArgs, AskStatusArgs, ChatApprovalArgs, ChatArgs, ChatExecutionArgs,
        ChatExportArgs, ChatOwnerArgs, ChatSearchArgs, ConsoleArgs, EmptyArgs, ProjectArgs,
        SettingsArgs, SubmitArgs, SupervisorArgs, TaskWindowArgs, TasksArgs,
    },
    command_error::CommandError,
    desktop_settings::PublicSettings,
    ui_contract::{AboutInfo, AskReceipt, AskReply, AskStatus, AskTargets, ProjectRow},
};

async fn service<T: Send + 'static>(
    state: &State<'_, Arc<AppState>>,
    action: impl FnOnce(Arc<AppState>) -> Result<T, CommandError> + Send + 'static,
) -> Result<T, CommandError> {
    let state = Arc::clone(state.inner());
    tauri::async_runtime::spawn_blocking(move || action(state))
        .await
        .map_err(|_| {
            CommandError::new(
                "desktop_worker_failed",
                "The local operation worker failed.",
            )
        })?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn chat_confirm(
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<(), CommandError> {
    let args: ProjectArgs = command_arguments::parse(request)?;
    service(&state, move |state| state.chat_confirm(&args.project_key)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn chat_pick_files(
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<Option<Vec<vibemux_client::text_attachment::TextAttachment>>, CommandError> {
    let args: ProjectArgs = command_arguments::parse(request)?;
    service(&state, move |state| {
        state.chat_pick_files(&args.project_key)
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn chat_search_request(
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<vibemux_client::chat_search::SearchPage, CommandError> {
    let args: ChatSearchArgs = command_arguments::parse(request)?;
    service(&state, move |state| {
        state.chat_search_request(&args.project_key, args.request)
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn chat_export_request(
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<vibemux_client::chat_export::ExportReceipt, CommandError> {
    let args: ChatExportArgs = command_arguments::parse(request)?;
    service(&state, move |state| {
        state.chat_export_request(&args.project_key, args.request)
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn chat_request(
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<vibemuxd::control::ChatResponse, CommandError> {
    let args: ChatArgs = command_arguments::parse(request)?;
    service(&state, move |state| {
        state.chat_request(&args.project_key, args.request)
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn chat_owner_request(
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<vibemuxd::control::ChatOwnerResponse, CommandError> {
    let args: ChatOwnerArgs = command_arguments::parse(request)?;
    service(&state, move |state| {
        state.chat_owner_request(&args.project_key, args.request)
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn chat_execution_request(
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<vibemuxd::control::ChatExecutionResponse, CommandError> {
    let args: ChatExecutionArgs = command_arguments::parse(request)?;
    service(&state, move |state| {
        state.chat_execution_request(&args.project_key, args.request)
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn projects_list(
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<Vec<ProjectRow>, CommandError> {
    let _: EmptyArgs = command_arguments::parse(request)?;
    service(&state, |state| state.projects_list()).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn chat_approval_request(
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<vibemuxd::control::ChatApprovalResponse, CommandError> {
    let args: ChatApprovalArgs = command_arguments::parse(request)?;
    service(&state, move |state| {
        state.chat_approval_request(&args.project_key, args.request)
    })
    .await
}
#[tauri::command(rename_all = "snake_case")]
pub async fn project_pick(
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<Option<ProjectRow>, CommandError> {
    let _: EmptyArgs = command_arguments::parse(request)?;
    service(&state, |state| state.pick_project()).await
}
#[tauri::command(rename_all = "snake_case")]
pub async fn project_select(
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<(), CommandError> {
    let args: ProjectArgs = command_arguments::parse(request)?;
    service(&state, move |state| state.select_project(&args.project_key)).await
}
#[tauri::command(rename_all = "snake_case")]
pub async fn project_forget(
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<(), CommandError> {
    let args: ProjectArgs = command_arguments::parse(request)?;
    service(&state, move |state| state.forget_project(&args.project_key)).await
}
#[tauri::command(rename_all = "snake_case")]
pub async fn daemon_start(
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<(), CommandError> {
    let args: ProjectArgs = command_arguments::parse(request)?;
    service(&state, move |state| {
        state.daemon_action(&args.project_key, DaemonCommand::Start)
    })
    .await
}
#[tauri::command(rename_all = "snake_case")]
pub async fn daemon_stop(
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<(), CommandError> {
    let args: ProjectArgs = command_arguments::parse(request)?;
    service(&state, move |state| {
        state.daemon_action(&args.project_key, DaemonCommand::Stop)
    })
    .await
}
#[tauri::command(rename_all = "snake_case")]
pub async fn ask_targets(
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<AskTargets, CommandError> {
    let args: ProjectArgs = command_arguments::parse(request)?;
    service(&state, move |state| state.ask_targets(&args.project_key)).await
}
#[tauri::command(rename_all = "snake_case")]
pub async fn ask_setup(
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<(), CommandError> {
    let args: ProjectArgs = command_arguments::parse(request)?;
    service(&state, move |state| state.ask_setup(&args.project_key)).await
}
#[tauri::command(rename_all = "snake_case")]
pub async fn ask_confirm(
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<(), CommandError> {
    let args: ProjectArgs = command_arguments::parse(request)?;
    service(&state, move |state| state.ask_confirm(&args.project_key)).await
}
#[tauri::command(rename_all = "snake_case")]
pub async fn ask_submit(
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<AskReceipt, CommandError> {
    let args: SubmitArgs = command_arguments::parse(request)?;
    service(&state, move |state| {
        state.ask_submit(
            &args.project_key,
            args.harness,
            args.prompt,
            args.submission_key,
        )
    })
    .await
}
#[tauri::command(rename_all = "snake_case")]
pub async fn ask_cancel(
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<(), CommandError> {
    let args: AskArgs = command_arguments::parse(request)?;
    service(&state, move |state| state.asks.cancel(&args.ask_id)).await
}
#[tauri::command(rename_all = "snake_case")]
pub async fn ask_status(
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<AskStatus, CommandError> {
    let args: AskStatusArgs = command_arguments::parse(request)?;
    service(&state, move |state| {
        if args.reconcile {
            state.asks.reconcile(&args.ask_id)?;
        }
        state.asks.status(&args.ask_id)
    })
    .await
}
#[tauri::command(rename_all = "snake_case")]
pub async fn ask_reply(
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<AskReply, CommandError> {
    let args: AskArgs = command_arguments::parse(request)?;
    state.asks.reply(&args.ask_id)
}
#[tauri::command(rename_all = "snake_case")]
pub async fn settings_get(
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<PublicSettings, CommandError> {
    let _: EmptyArgs = command_arguments::parse(request)?;
    Ok(state.public_settings())
}
#[tauri::command(rename_all = "snake_case")]
pub async fn settings_set(
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<PublicSettings, CommandError> {
    let args: SettingsArgs = command_arguments::parse(request)?;
    service(&state, move |state| {
        state.apply_settings(&args.patch).map_err(|_| {
            CommandError::new(
                "settings_write_failed",
                "The settings file could not be saved.",
            )
        })?;
        Ok(state.public_settings())
    })
    .await
}
#[tauri::command(rename_all = "snake_case")]
pub async fn about_info() -> Result<AboutInfo, CommandError> {
    Ok(AboutInfo {
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        webview_version: tauri::webview_version().unwrap_or_else(|_| "unknown".to_owned()),
    })
}
fn not_implemented(command: &str) -> CommandError {
    CommandError::new(
        "not_implemented",
        format!("{command} is unavailable in this milestone."),
    )
}
// Existing permissions stay fixed. Unfinished screens cannot gain authority.
#[tauri::command(rename_all = "snake_case")]
pub async fn tasks_snapshot<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<crate::read_contract::TaskReadResult, CommandError> {
    let args: TasksArgs = command_arguments::parse(request)?;
    let label = window.label().to_owned();
    service(&state, move |state| {
        state.tasks_snapshot_for_window(&label, &args.project_key, args.query)
    })
    .await
}
#[tauri::command(rename_all = "snake_case")]
pub async fn supervisor_action<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<crate::terminal_bridge::TerminalReply, CommandError> {
    let args: SupervisorArgs = command_arguments::parse(request)?;
    let label = window.label().to_owned();
    service(&state, move |state| state.supervisor_action(&label, args)).await
}
#[tauri::command(rename_all = "snake_case")]
pub async fn task_window_open<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::WebviewWindow<R>,
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<(), CommandError> {
    if window.label() != "main" {
        return Err(crate::task_windows::denied());
    }
    let args: TaskWindowArgs = command_arguments::parse(request)?;
    let reservation = service(&state, move |state| {
        state.prepare_task_window(&args.project_key, args.task_id)
    })
    .await?;
    crate::windows::open_task_window(&app, state.inner(), &reservation)
}
#[tauri::command(rename_all = "snake_case")]
pub async fn console_section(
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<crate::read_contract::ConsoleSnapshot, CommandError> {
    let args: ConsoleArgs = command_arguments::parse(request)?;
    service(&state, move |state| {
        state.console_section(&args.project_key, args.query)
    })
    .await
}
#[tauri::command(rename_all = "snake_case")]
pub async fn probe_rerun<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<crate::probe_contract::ProbeSnapshot, CommandError> {
    if window.label() != "main" {
        return Err(crate::probe_contract::failure("probe_caller_denied"));
    }
    let args: crate::command_arguments::ProbeArgs = command_arguments::parse(request)?;
    let label = window.label().to_owned();
    service(&state, move |state| {
        state.probe_request(&label, &args.project_key, args.request)
    })
    .await
}

#[cfg(test)]
mod probe_invoke_tests {
    use super::*;
    use serde_json::json;
    use tauri::{
        ipc::{CallbackFn, InvokeBody},
        test::{INVOKE_KEY, get_ipc_response, mock_builder},
    };

    #[test]
    fn diagnostic_forwarding_binds_actual_main_and_the_registered_project() {
        use crate::probe_registry::{ProbeOutcome, ProbeRunner};
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Synthetic(AtomicUsize);
        impl ProbeRunner for Synthetic {
            fn run(&self, _: tokio::sync::watch::Receiver<bool>) -> ProbeOutcome {
                self.0.fetch_add(1, Ordering::AcqRel);
                ProbeOutcome {
                    report: None,
                    error_code: Some("local_probe_cancelled".into()),
                    quiescent: true,
                }
            }
        }
        let root = tempfile::tempdir().expect("owned root");
        let folder = root.path().join("project");
        std::fs::create_dir_all(folder.join(".git")).expect("owned repository marker");
        let runner = Arc::new(Synthetic(AtomicUsize::new(0)));
        let state = Arc::new(
            AppState::new(
                root.path().join("settings.json"),
                Arc::new(crate::event_sink::RecordingEventSink::default()),
            )
            .with_probe_runner(runner.clone()),
        );
        let key = state.register_project(&folder).expect("register");
        let app = mock_builder()
            .manage(state.clone())
            .invoke_handler(tauri::generate_handler![probe_rerun])
            .build(tauri::generate_context!())
            .expect("actual capabilities");
        let main = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("main");
        let child = tauri::WebviewWindowBuilder::new(&app, "task_probe_denied", Default::default())
            .build()
            .expect("task child");
        let id = uuid::Uuid::new_v4();
        let body = json!({"project_key":key,"request":{"operation":"start","operation_id":id}});
        let request = |body| tauri::webview::InvokeRequest {
            cmd: "probe_rerun".into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "http://tauri.localhost".parse().expect("origin"),
            body: InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.into(),
        };
        assert!(get_ipc_response(&child, request(body.clone())).is_err());
        assert_eq!(runner.0.load(Ordering::Acquire), 0);
        let reply = get_ipc_response(&main, request(body.clone()))
            .expect("registered main admission")
            .deserialize::<serde_json::Value>()
            .expect("closed reply");
        assert_eq!(reply["project_key"], key);
        assert_eq!(reply["job"]["operation_id"], id.to_string());
        get_ipc_response(&main, request(body)).expect("same-ID receipt");
        assert_eq!(state.probes.shutdown(), 0);
        assert_eq!(runner.0.load(Ordering::Acquire), 1);
    }

    #[test]
    fn diagnostic_arguments_are_closed_before_project_or_process_work() {
        let root = tempfile::tempdir().expect("owned settings");
        let state = Arc::new(AppState::new(
            root.path().join("settings.json"),
            Arc::new(crate::event_sink::RecordingEventSink::default()),
        ));
        let app = mock_builder()
            .manage(state)
            .invoke_handler(tauri::generate_handler![probe_rerun])
            .build(tauri::generate_context!())
            .expect("actual capabilities");
        let main = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("main");
        for body in [
            json!({}),
            json!({"project_key":"unknown","request":{"operation":"start","operation_id":"00000000-0000-0000-0000-000000000001","path":"C:/forged"}}),
        ] {
            let error = get_ipc_response(
                &main,
                tauri::webview::InvokeRequest {
                    cmd: "probe_rerun".into(),
                    callback: CallbackFn(0),
                    error: CallbackFn(1),
                    url: "http://tauri.localhost".parse().expect("origin"),
                    body: InvokeBody::Json(body),
                    headers: Default::default(),
                    invoke_key: INVOKE_KEY.into(),
                },
            )
            .expect_err("closed request");
            assert_eq!(error["code"], "invalid_arguments");
        }
    }
}
#[tauri::command(rename_all = "snake_case")]
pub async fn quick_search() -> Result<(), CommandError> {
    Err(not_implemented("quick_search"))
}
#[tauri::command(rename_all = "snake_case")]
pub async fn open_link<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<crate::link_contract::OpenLinkReply, CommandError> {
    let args: crate::command_arguments::LinkArgs = command_arguments::parse(request)?;
    let label = window.label().to_owned();
    service(&state, move |state| state.open_link(&label, args.request)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn workspace_cleanup_request<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<crate::workspace_cleanup_bridge::NativeCleanupReply, CommandError> {
    let args: crate::command_arguments::WorkspaceCleanupArgs = command_arguments::parse(request)?;
    let label = window.label().to_owned();
    service(&state, move |state| {
        state.workspace_cleanup_request(&label, args.request)
    })
    .await
}
