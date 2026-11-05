//! Exercise actual Tauri invoke deserialization and public response projection.
use serde_json::{Value, json};
use std::sync::Arc;
use tauri::{
    ipc::{CallbackFn, InvokeBody},
    test::{INVOKE_KEY, get_ipc_response, mock_builder, mock_context, noop_assets},
};
use vibemux_desktop::{app_state::AppState, commands, event_sink::RecordingEventSink};

fn invoke(
    window: &tauri::WebviewWindow<tauri::test::MockRuntime>,
    command: &str,
    body: Value,
) -> Result<Value, Value> {
    get_ipc_response(
        window,
        tauri::webview::InvokeRequest {
            cmd: command.to_owned(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "http://tauri.localhost".parse().expect("app origin"),
            body: InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_owned(),
        },
    )
    .map(|body| body.deserialize::<Value>().expect("JSON response"))
}

#[test]
fn native_link_requests_reject_unknown_authority_and_unsafe_urls_before_effects() {
    let temp = tempfile::tempdir().expect("temp");
    let state = Arc::new(AppState::new(
        temp.path().join("settings.json"),
        Arc::new(RecordingEventSink::default()),
    ));
    let app = mock_builder()
        .manage(state)
        .invoke_handler(tauri::generate_handler![commands::open_link])
        .build(tauri::generate_context!())
        .expect("actual capability context");
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("main");
    for body in [
        json!({}),
        json!({"request":{"kind":"https","url":"https://example.org","path":"C:/forged"}}),
        json!({"request":{"kind":"worktree","project_key":"forged","task_id":"00000000-0000-0000-0000-000000000001","run_id":"00000000-0000-0000-0000-000000000002","action":"open","path":"C:/forged"}}),
    ] {
        assert_eq!(
            invoke(&window, "open_link", body).expect_err("closed authority")["code"],
            "invalid_arguments"
        );
    }
    for url in [
        "http://example.org",
        "file:///C:/forged",
        "javascript:alert(1)",
        "mailto:user@example.org",
        "//example.org",
        "/relative",
        "https://user:secret@example.org/",
        " https://example.org/",
        "https://example.org/\n",
    ] {
        let error = invoke(
            &window,
            "open_link",
            json!({"request":{"kind":"https","url":url}}),
        )
        .expect_err("unsafe URL");
        assert_eq!(error["code"], "invalid_link_url");
        assert!(
            !error["message"]
                .as_str()
                .expect("safe message")
                .contains("secret")
        );
    }
}

#[test]
fn native_cleanup_uses_actual_main_caller_and_closed_identifier_inputs() {
    let temp = tempfile::tempdir().expect("owned fixture");
    let state = Arc::new(AppState::new(
        temp.path().join("settings.json"),
        Arc::new(RecordingEventSink::default()),
    ));
    let app = mock_builder()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::workspace_cleanup_request
        ])
        .build(tauri::generate_context!())
        .expect("actual capability context");
    let main = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("main");
    let request = json!({"operation":"plan","registration_key":"unknown","task_id":"00000000-0000-0000-0000-000000000001",
        "run_id":"00000000-0000-0000-0000-000000000002","operation_id":"00000000-0000-0000-0000-000000000003"});
    for field in [
        "path",
        "force",
        "plugin_id",
        "project_id",
        "caller",
        "window",
    ] {
        let mut forged = request.clone();
        forged[field] = json!("forged");
        assert_eq!(
            invoke(
                &main,
                "workspace_cleanup_request",
                json!({"request":forged})
            )
            .expect_err("unknown authority")["code"],
            "invalid_arguments"
        );
    }
    assert_eq!(
        invoke(
            &main,
            "workspace_cleanup_request",
            json!({"request":request.clone(),"caller":"main"})
        )
        .expect_err("outer extra")["code"],
        "invalid_arguments"
    );
    assert_eq!(
        invoke(
            &main,
            "workspace_cleanup_request",
            json!({"request":request.clone()})
        )
        .expect_err("unregistered target")["code"],
        "unknown_project"
    );
    let child = tauri::WebviewWindowBuilder::new(&app, "task_forged", Default::default())
        .build()
        .expect("child");
    assert!(
        invoke(
            &child,
            "workspace_cleanup_request",
            json!({"request":request})
        )
        .is_err(),
        "Task capability has no cleanup command"
    );
}

#[test]
fn real_invokes_use_snake_case_reject_extra_fields_and_hide_internal_settings() {
    let temp = tempfile::tempdir().expect("temp");
    let state = Arc::new(AppState::new(
        temp.path().join("settings.json"),
        Arc::new(RecordingEventSink::default()),
    ));
    let key = state.register_project(temp.path()).expect("registered");
    let app = mock_builder()
        .manage(Arc::clone(&state))
        .invoke_handler(tauri::generate_handler![
            commands::project_select,
            commands::ask_submit,
            commands::settings_get,
            commands::settings_set,
            commands::tasks_snapshot,
            commands::console_section,
            commands::chat_request,
            commands::chat_owner_request,
            commands::chat_execution_request,
            commands::chat_approval_request,
            commands::chat_export_request,
            commands::chat_search_request,
            commands::chat_confirm,
            commands::chat_pick_files,
        ])
        .build(mock_context(noop_assets()))
        .expect("mock app");
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("window");
    let search = json!({"query":"literal","scope":"all","case_sensitive":false,"include_archived":false,"limit":25,"cursor":null});
    assert_eq!(
        invoke(
            &window,
            "chat_search_request",
            json!({"project_key":"missing","request":search})
        )
        .expect_err("search project binding")["code"],
        "unknown_project"
    );
    for field in [
        "path", "database", "title", "text", "records", "provider", "endpoint",
    ] {
        let mut forged = search.clone();
        forged[field] = json!("untrusted");
        assert_eq!(
            invoke(
                &window,
                "chat_search_request",
                json!({"project_key":key,"request":forged})
            )
            .expect_err("closed search request")["code"],
            "invalid_arguments"
        );
    }
    let mut invalid_search = search.clone();
    invalid_search["query"] = json!(" ");
    assert_eq!(
        invoke(
            &window,
            "chat_search_request",
            json!({"project_key":key,"request":invalid_search})
        )
        .expect_err("no private read on empty query")["code"],
        "chat_search_invalid_request"
    );
    let export = json!({"kind":"chat","format":"text","chat_id":uuid::Uuid::new_v4(),"generation":1,"expected_version":1});
    assert_eq!(
        invoke(
            &window,
            "chat_export_request",
            json!({"project_key":"missing","request":export})
        )
        .expect_err("project-bound export")["code"],
        "unknown_project"
    );
    for field in ["path", "destination", "title", "text", "records", "blob_id"] {
        let mut forged = export.clone();
        forged[field] = json!("untrusted renderer value");
        assert_eq!(
            invoke(
                &window,
                "chat_export_request",
                json!({"project_key":key,"request":forged})
            )
            .expect_err("closed export request")["code"],
            "invalid_arguments"
        );
    }
    let mut invalid = export.clone();
    invalid["generation"] = json!(0);
    assert_eq!(
        invoke(
            &window,
            "chat_export_request",
            json!({"project_key":key,"request":invalid})
        )
        .expect_err("refuse before dialog")["code"],
        "chat_export_invalid_request"
    );
    let start = json!({"operation":"start","operation_id":uuid::Uuid::new_v4(),"chat_id":uuid::Uuid::new_v4(),"generation":1,"owner_version":1,"consent_execution":true});
    let approval = json!({"operation":"decide","operation_id":uuid::Uuid::new_v4(),"chat_id":uuid::Uuid::new_v4(),
        "approval_id":uuid::Uuid::new_v4(),"request_id":uuid::Uuid::new_v4(),"generation":1,"expected_version":1,"decision":"allow_once"});
    assert_eq!(
        invoke(
            &window,
            "chat_approval_request",
            json!({"project_key":"missing","request":approval})
        )
        .expect_err("approval stays project-bound")["code"],
        "unknown_project"
    );
    for field in [
        "path",
        "input",
        "vendor_request_id",
        "tool_grants",
        "profile_sha256",
        "fence",
    ] {
        let mut forged = approval.clone();
        forged[field] = json!("private untrusted value");
        assert_eq!(
            invoke(
                &window,
                "chat_approval_request",
                json!({"project_key":key,"request":forged})
            )
            .expect_err("closed native approval arguments")["code"],
            "invalid_arguments"
        );
    }
    assert_eq!(
        invoke(
            &window,
            "chat_approval_request",
            json!({"project_key":key,"request":approval,"path":"C:/forged"})
        )
        .expect_err("closed outer native arguments")["code"],
        "invalid_arguments"
    );
    assert_eq!(
        invoke(
            &window,
            "chat_execution_request",
            json!({"project_key":"missing","request":start})
        )
        .expect_err("registered identity")["code"],
        "unknown_project"
    );
    assert_eq!(
        invoke(
            &window,
            "chat_execution_request",
            json!({"project_key":key,"request":start})
        )
        .expect_err("native consent before daemon connection")["code"],
        "chat_consent_required"
    );
    let mut forged = start.clone();
    forged["workspace_path"] = json!("C:/forged");
    assert_eq!(
        invoke(
            &window,
            "chat_execution_request",
            json!({"project_key":key,"request":forged})
        )
        .expect_err("closed request")["code"],
        "invalid_arguments"
    );
    let mut denied = start;
    denied["consent_execution"] = json!(false);
    assert_eq!(
        invoke(
            &window,
            "chat_execution_request",
            json!({"project_key":key,"request":denied})
        )
        .expect_err("explicit start consent")["code"],
        "invalid_arguments"
    );
    let owner_list = json!({"operation":"list","after":null,"limit":16,"include_finalized":false});
    assert_eq!(
        invoke(
            &window,
            "chat_owner_request",
            json!({"project_key":"missing","request":owner_list})
        )
        .expect_err("owner request requires registered identity")["code"],
        "unknown_project"
    );
    for request in [
        json!({"operation":"list","root":"C:/forged"}),
        json!({"operation":"open","operation_id":uuid::Uuid::new_v4(),"harness":"codex","consent_private_storage":true,"consent_workspace":true,"tools":["edit_files"]}),
        json!({"operation":"send","chat_id":uuid::Uuid::new_v4(),"prompt":"forged owner execution"}),
    ] {
        assert_eq!(
            invoke(
                &window,
                "chat_owner_request",
                json!({"project_key":key,"request":request})
            )
            .expect_err("closed owner request")["code"],
            "invalid_arguments"
        );
    }
    assert_eq!(
        invoke(
            &window,
            "chat_owner_request",
            json!({"project_key":key,"request":owner_list,"path":"C:/forged"})
        )
        .expect_err("closed command")["code"],
        "invalid_arguments"
    );
    assert_eq!(
        invoke(
            &window,
            "chat_owner_request",
            json!({"project_key":key,"request":{
        "operation":"open","operation_id":uuid::Uuid::new_v4(),"harness":"codex","title":null,
        "consent_private_storage":true,"consent_workspace":false}})
        )
        .expect_err("independent workspace consent before connection")["code"],
        "invalid_arguments"
    );
    assert_eq!(
        invoke(&window, "project_select", json!({"project_key":key})).expect("selected"),
        Value::Null
    );
    for (command, query) in [
        ("tasks_snapshot", json!({"kind":"list","limit":32})),
        (
            "console_section",
            json!({"section":"daemon","after_sequence":null}),
        ),
    ] {
        assert_eq!(
            invoke(
                &window,
                command,
                json!({"project_key":"missing","query":query})
            )
            .expect_err("unknown registered project")["code"],
            "unknown_project"
        );
        assert_eq!(
            invoke(
                &window,
                command,
                json!({"project_key":key,"query":query,"path":"C:/forged"})
            )
            .expect_err("unknown path field")["code"],
            "invalid_arguments"
        );
        let mut forged = query.clone();
        forged["rpc_method"] = json!("shutdown");
        assert_eq!(
            invoke(&window, command, json!({"project_key":key,"query":forged}))
                .expect_err("unknown read field")["code"],
            "invalid_arguments"
        );
    }
    for body in [
        json!({"projectKey":key}),
        json!({"project_key":key,"path":"C:/forged"}),
        json!({"_project_key":key}),
    ] {
        assert_eq!(
            invoke(&window, "project_select", body).expect_err("invalid")["code"],
            "invalid_arguments"
        );
    }
    let settings = invoke(&window, "settings_get", json!({})).expect("settings");
    assert_eq!(
        invoke(
            &window,
            "chat_pick_files",
            json!({"project_key":key,"paths":["C:/forged.txt"]})
        )
        .expect_err("file paths must originate in native picker")["code"],
        "invalid_arguments"
    );
    assert!(settings.get("project_folders").is_none());
    assert!(settings.get("ask_confirmed_projects").is_none());
    assert!(settings.get("ask_consent_identities").is_none());
    let updated = invoke(
        &window,
        "settings_set",
        json!({"patch":{"appearance":"dark"}}),
    )
    .expect("set");
    assert_eq!(updated["appearance"], "dark");
    assert!(updated.get("project_folders").is_none());
    for field in [
        "project_folders",
        "ask_confirmed_projects",
        "ask_consent_identities",
        "chat_consent_identities",
    ] {
        assert_eq!(
            invoke(&window, "settings_set", json!({"patch":{field:[]}}))
                .expect_err("forged consent")["code"],
            "invalid_arguments"
        );
    }
    let submission = json!({"project_key":key,"harness":"claude","prompt":"synthetic","submission_key":uuid::Uuid::new_v4()});
    let chat_request = json!({"project_key":key,"request":{"operation":"open","operation_id":uuid::Uuid::new_v4(),
        "harness":"claude","title":null,"consent_private_storage":true}});
    assert_eq!(
        invoke(&window, "chat_request", chat_request.clone()).expect_err("chat consent")["code"],
        "chat_consent_required"
    );
    let mut forged_chat = chat_request;
    forged_chat["request"]["vendor_session_id"] = json!("foreign-session");
    assert_eq!(
        invoke(&window, "chat_request", forged_chat).expect_err("foreign resume identity")["code"],
        "invalid_arguments"
    );
    assert_eq!(
        invoke(&window, "ask_submit", submission.clone()).expect_err("consent")["code"],
        "ask_consent_required"
    );
    let mut extra = submission.clone();
    extra["executable"] = json!("arbitrary.exe");
    assert_eq!(
        invoke(&window, "ask_submit", extra).expect_err("unknown field")["code"],
        "invalid_arguments"
    );
    let mut target = submission.clone();
    target["harness"] = json!("opencode");
    assert_eq!(
        invoke(&window, "ask_submit", target).expect_err("unsupported harness")["code"],
        "invalid_arguments"
    );
    let mut nil = submission;
    nil["submission_key"] = json!(uuid::Uuid::nil());
    assert_eq!(
        invoke(&window, "ask_submit", nil).expect_err("nil")["code"],
        "invalid_identifier"
    );
}

#[test]
fn rust_prompt_limits_measure_utf8_and_the_double_escaped_control_frame() {
    use vibemux_desktop::{ask_registry::validate_submission, ui_contract::AskHarness};
    let key = uuid::Uuid::new_v4();
    assert!(validate_submission(AskHarness::Claude, &"x".repeat(32 * 1024), key).is_ok());
    assert!(validate_submission(AskHarness::Codex, &"界".repeat(10_922), key).is_ok());
    for prompt in ["x".repeat(32 * 1024 + 1), "界".repeat(10_923)] {
        assert_eq!(
            validate_submission(AskHarness::Claude, &prompt, key)
                .expect_err("UTF-8 byte bound")
                .code,
            "prompt_too_large"
        );
    }
    for prompt in ["", " \r\n\t"] {
        assert_eq!(
            validate_submission(AskHarness::Claude, prompt, key)
                .expect_err("empty")
                .code,
            "prompt_empty"
        );
    }
    let escaped = format!("x{}", "\\".repeat(32 * 1024 - 1));
    assert_eq!(
        validate_submission(AskHarness::Claude, &escaped, key)
            .expect_err("escaped IPC overflow")
            .code,
        "prompt_frame_too_large"
    );
}
