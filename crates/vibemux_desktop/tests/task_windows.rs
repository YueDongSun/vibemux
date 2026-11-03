//! Caller identity is supplied by Tauri, never the request body or URL.
use serde_json::json;
use std::sync::Arc;
use tauri::{
    ipc::{CallbackFn, InvokeBody},
    test::{INVOKE_KEY, get_ipc_response, mock_builder, mock_context, noop_assets},
};
use vibemux_desktop::{app_state::AppState, commands, event_sink::RecordingEventSink};

#[path = "support/link_effects_fixture.rs"]
mod link_effects_fixture;
use link_effects_fixture::{LinkEffect, RecordedLinkEffects};

#[test]
fn authenticated_task_window_reads_keep_their_binding_across_selection_and_revocation() {
    use vibemux_desktop::read_contract::{TaskReadQuery, TaskReadResult};
    use vibemux_types::{
        Run, RunSpec, Task, TaskSpec,
        a2a::{A2aRunStart, RunWorkspace},
    };
    use vibemuxd::{
        WriterWorker,
        control::{ControlClient, DaemonControlServer},
        process::DaemonPaths,
    };
    let temp = tempfile::tempdir().expect("project");
    let other = tempfile::tempdir().expect("other project");
    let paths = DaemonPaths::from_project_root(temp.path()).expect("paths");
    paths.ensure_runtime_dir().expect("private runtime");
    let writer = WriterWorker::start(paths.database_path()).expect("writer");
    let handle = writer.handle().expect("writer handle");
    let project_id = handle.project_id().expect("query").expect("project");
    let task = Task::new(TaskSpec {
        project_id,
        title: "Task-window read fixture".into(),
        description: String::new(),
    })
    .expect("task");
    let task_id = task.task_id();
    let run = Run::new(RunSpec {
        project_id,
        task_id,
        harness: "fixture".into(),
        role: "worker".into(),
        protocol: "a2a".into(),
        base_commit: "a".repeat(40),
    })
    .expect("run");
    let run_id = run.run_id();
    std::fs::create_dir(temp.path().join("owned_worktree")).expect("recorded workspace exists");
    handle
        .start_a2a_run(A2aRunStart {
            task,
            run,
            peer_id: "fixture".into(),
            external_task_id: "fixture-task".into(),
            transport: "http_json".into(),
            protocol_version: "1.0".into(),
            workspace: RunWorkspace {
                path: temp
                    .path()
                    .join("owned_worktree")
                    .to_string_lossy()
                    .into_owned(),
                branch: "fixture".into(),
                base_commit: "a".repeat(40),
                ownership_token: "fixture-owned".into(),
            },
            timestamp: std::time::SystemTime::now().into(),
            idempotency_key: "fixture-task-window".into(),
        })
        .expect("seed through writer");
    drop(handle);
    writer.shutdown().expect("setup writer joined");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let server = runtime
        .block_on(DaemonControlServer::start_for_paths(&paths))
        .expect("daemon");
    let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
    let before = runtime
        .block_on(client.frontend_task(task_id))
        .expect("read")
        .expect("task");
    let descriptor_before = std::fs::read(server.descriptor_path()).expect("descriptor");
    let effects = Arc::new(RecordedLinkEffects::default());
    let state = Arc::new(
        AppState::new(
            temp.path().join("settings.json"),
            Arc::new(RecordingEventSink::default()),
        )
        .with_link_effects(effects.clone()),
    );
    let key = state.register_project(temp.path()).expect("registered");
    let reservation = state
        .prepare_task_window(&key, task_id)
        .expect("authenticated preflight");
    assert_eq!(reservation.binding.project_id, project_id);
    state
        .task_windows
        .mark_ready(&reservation)
        .expect("ready view");
    let other_key = state
        .register_project(other.path())
        .expect("other registered");
    state
        .select_project(&other_key)
        .expect("main selection changes");
    let result = state
        .tasks_snapshot_for_window(&reservation.label, &key, TaskReadQuery::Detail { task_id })
        .expect("bound read");
    assert!(
        matches!(result,TaskReadResult::Detail{task,..} if task.task.task_id==task_id && task.task.project_id==project_id)
    );
    assert_eq!(
        state
            .tasks_snapshot_for_window(
                &reservation.label,
                &other_key,
                TaskReadQuery::Detail { task_id }
            )
            .expect_err("foreign key")
            .code,
        "task_window_binding_invalid"
    );
    let direct_error = runtime
        .block_on(
            client.terminal_inspect(vibemuxd::terminal_observer::TerminalQuery {
                project_id,
                task_id,
                run_id,
                plugin_id: vibemux_client::supervisor_client::TERMINAL_PLUGIN_ID.into(),
            }),
        )
        .expect_err("fixture has no configured terminal plugin");
    assert_eq!(direct_error.code(), "terminal_plugin_unavailable");
    let app = mock_builder()
        .manage(Arc::clone(&state))
        .invoke_handler(tauri::generate_handler![
            commands::supervisor_action,
            commands::open_link
        ])
        .build(tauri::generate_context!())
        .expect("actual capability context");
    let main = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("main caller");
    let invoke_terminal = |run_id, request| {
        get_ipc_response(
            &main,
            tauri::webview::InvokeRequest {
                cmd: "supervisor_action".into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: "http://tauri.localhost".parse().expect("origin"),
                body: InvokeBody::Json(
                    json!({"project_key":key,"task_id":task_id,"run_id":run_id,"request":request}),
                ),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.into(),
            },
        )
        .expect_err("terminal request refuses safely")
    };
    for request in [
        json!({"operation":"inspect"}),
        json!({"operation":"inspect","previous_observation_id":"00000000-0000-0000-0000-000000000003"}),
    ] {
        let error = invoke_terminal(run_id, request);
        assert_eq!(error["code"], direct_error.code());
        assert!(
            !error["message"]
                .as_str()
                .expect("safe error message")
                .contains(temp.path().to_string_lossy().as_ref())
        );
    }
    assert_eq!(
        invoke_terminal(vibemux_types::RunId::new(), json!({"operation":"inspect"}))["code"],
        "terminal_run_unavailable"
    );
    let child = tauri::WebviewWindowBuilder::new(&app, &reservation.label, Default::default())
        .build()
        .expect("bound child caller");
    let invoke_link = |window: &tauri::WebviewWindow<tauri::test::MockRuntime>, request| {
        get_ipc_response(
            window,
            tauri::webview::InvokeRequest {
                cmd: "open_link".into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: "http://tauri.localhost".parse().expect("origin"),
                body: InvokeBody::Json(json!({"request":request})),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.into(),
            },
        )
    };
    let branch_request = json!({"kind":"worktree","project_key":key,"task_id":task_id,"run_id":run_id,"action":"copy_branch"});
    for window in [&main, &child] {
        let reply = invoke_link(window, branch_request.clone())
            .expect("canonical branch through Control v15")
            .deserialize::<serde_json::Value>()
            .expect("reply");
        assert_eq!(
            reply,
            json!({"kind":"worktree","project_id":project_id,"task_id":task_id,"run_id":run_id,"action":"copy_branch","outcome":"branch_copied"})
        );
    }
    assert_eq!(
        *effects.events.lock().expect("effects"),
        [
            LinkEffect::Copy("fixture".into()),
            LinkEffect::Copy("fixture".into())
        ]
    );
    for action in ["open", "copy_path"] {
        let mut request = branch_request.clone();
        request["action"] = json!(action);
        assert_eq!(
            invoke_link(&child, request).expect_err("unmanaged legacy directory")["code"],
            "workspace_directory_unavailable"
        );
    }
    assert_eq!(
        invoke_link(&child, json!({"kind":"https","url":"https://example.org"}))
            .expect_err("main-only HTTPS")["code"],
        "link_window_forbidden"
    );
    let mut foreign = branch_request.clone();
    foreign["task_id"] = json!(vibemux_types::TaskId::new());
    assert_eq!(
        invoke_link(&child, foreign).expect_err("bound task")["code"],
        "task_window_binding_invalid"
    );
    let mut foreign = branch_request.clone();
    foreign["run_id"] = json!(vibemux_types::RunId::new());
    assert_eq!(
        invoke_link(&child, foreign).expect_err("loaded run membership")["code"],
        "workspace_run_unavailable"
    );
    let https_reply = invoke_link(
        &main,
        json!({"kind":"https","url":"HTTPS://Example.org/a?q=one#ref"}),
    )
    .expect("fake default browser")
    .deserialize::<serde_json::Value>()
    .expect("https reply");
    assert_eq!(
        https_reply,
        json!({"kind":"https","outcome":"open_requested"})
    );
    assert_eq!(
        effects.events.lock().expect("effects").last(),
        Some(&LinkEffect::Https("https://example.org/a?q=one#ref".into()))
    );
    let effect_count = effects.events.lock().expect("effects").len();
    state.forget_project(&key).expect("revoke registration");
    assert_eq!(
        invoke_link(&child, branch_request).expect_err("revoked worktree access")["code"],
        "unknown_project"
    );
    assert_eq!(effects.events.lock().expect("effects").len(), effect_count);
    assert_eq!(
        state
            .tasks_snapshot_for_window(&reservation.label, &key, TaskReadQuery::Detail { task_id })
            .expect_err("revoked")
            .code,
        "unknown_project"
    );
    state
        .task_windows
        .release(&reservation.label, reservation.generation);
    let after = runtime
        .block_on(client.frontend_task(task_id))
        .expect("read after")
        .expect("task");
    assert_eq!(before.task, after.task);
    assert_eq!(before.runs, after.runs);
    assert_eq!(
        descriptor_before,
        std::fs::read(server.descriptor_path()).expect("unchanged descriptor")
    );
    runtime
        .block_on(server.shutdown())
        .expect("owned daemon joined");
}

#[test]
fn task_window_invoke_arguments_are_closed_and_auxiliary_capabilities_deny_mutation() {
    let temp = tempfile::tempdir().expect("temp");
    let state = Arc::new(AppState::new(
        temp.path().join("settings.json"),
        Arc::new(RecordingEventSink::default()),
    ));
    let app = mock_builder()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::supervisor_action,
            commands::tasks_snapshot,
            commands::settings_get,
            commands::settings_set,
            commands::task_window_open,
            commands::chat_request,
            commands::ask_submit,
            commands::project_forget,
            commands::daemon_start,
        ])
        .build(tauri::generate_context!())
        .expect("real capability context");
    let window = tauri::WebviewWindowBuilder::new(&app, "task_forged", Default::default())
        .build()
        .expect("window");
    let call = |command: &str, body| {
        get_ipc_response(
            &window,
            tauri::webview::InvokeRequest {
                cmd: command.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: "http://tauri.localhost".parse().expect("origin"),
                body: InvokeBody::Json(body),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.into(),
            },
        )
    };
    assert!(call("settings_get", json!({})).is_ok());
    let terminal_body = json!({"project_key":"foreign","task_id":"00000000-0000-0000-0000-000000000001",
        "run_id":"00000000-0000-0000-0000-000000000002","request":{"operation":"inspect"}});
    assert_eq!(
        call("supervisor_action", terminal_body.clone()).expect_err("native task binding")["code"],
        "task_window_binding_invalid"
    );
    let mut acknowledged_body = terminal_body.clone();
    acknowledged_body["request"] = json!({"operation":"inspect", "previous_observation_id":"00000000-0000-0000-0000-000000000003"});
    assert_eq!(
        call("supervisor_action", acknowledged_body)
            .expect_err("acknowledged request still requires native task binding")["code"],
        "task_window_binding_invalid"
    );
    for request in [
        json!({"operation":"send_text","text":"never sent"}),
        json!({"operation":"inspect","cwd":"C:/forged"}),
        json!({"operation":"inspect","previous_observation_id":"invalid"}),
        json!({"operation":"inspect","previous_observation_id":"00000000-0000-0000-0000-000000000003","plugin_id":"forged"}),
        json!({"operation":"focus","observation_id":"00000000-0000-0000-0000-000000000003","binding_id":"forged"}),
    ] {
        let mut body = terminal_body.clone();
        body["request"] = request;
        assert_eq!(
            call("supervisor_action", body).expect_err("closed terminal operation")["code"],
            "invalid_arguments"
        );
    }
    for command in [
        "settings_set",
        "task_window_open",
        "chat_request",
        "ask_submit",
        "project_forget",
        "daemon_start",
    ] {
        let error = call(command, json!({})).expect_err("capability denied before handler");
        assert!(
            error
                .as_str()
                .is_some_and(|message| message.contains("not allowed")),
            "{command}: {error}"
        );
    }
    let main = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("main");
    for body in [
        json!({}),
        json!({"project_key":"x","task_id":"00000000-0000-0000-0000-000000000000"}),
        json!({"project_key":"x","task_id":"00000000-0000-0000-0000-000000000001","url":"file:///forged"}),
    ] {
        let error = get_ipc_response(
            &main,
            tauri::webview::InvokeRequest {
                cmd: "task_window_open".into(),
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

#[test]
fn unbound_auxiliary_windows_cannot_list_or_read_a_forged_task() {
    let temp = tempfile::tempdir().expect("temp");
    let state = Arc::new(AppState::new(
        temp.path().join("settings.json"),
        Arc::new(RecordingEventSink::default()),
    ));
    let app = mock_builder()
        .manage(state)
        .invoke_handler(tauri::generate_handler![commands::tasks_snapshot])
        .build(mock_context(noop_assets()))
        .expect("app");
    let window = tauri::WebviewWindowBuilder::new(&app, "task_forged", Default::default())
        .build()
        .expect("window");
    for query in [
        json!({"kind":"list","limit":32,"after":null}),
        json!({"kind":"detail","task_id":"00000000-0000-0000-0000-000000000001"}),
    ] {
        let error = get_ipc_response(
            &window,
            tauri::webview::InvokeRequest {
                cmd: "tasks_snapshot".into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: "http://tauri.localhost".parse().expect("origin"),
                body: InvokeBody::Json(json!({"project_key":"foreign", "query":query})),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.into(),
            },
        )
        .expect_err("native binding required");
        assert_eq!(error["code"], "task_window_binding_invalid");
    }
}
