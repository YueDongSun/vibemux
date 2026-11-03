//! Native project handles are capabilities registered by Rust, never paths from JS.
use std::sync::Arc;

use vibemux_desktop::{app_state::AppState, event_sink::RecordingEventSink};

fn state_at(directory: &std::path::Path) -> AppState {
    AppState::new(
        directory.join("settings.json"),
        Arc::new(RecordingEventSink::default()),
    )
}

#[test]
fn aliases_share_a_registered_handle_and_settings_hide_private_paths() {
    let directory = tempfile::tempdir().expect("temporary project");
    let state = AppState::new(
        directory.path().join("settings.json"),
        Arc::new(RecordingEventSink::default()),
    );
    let first = state
        .register_project(directory.path())
        .expect("registered");
    let alias = state
        .register_project(&directory.path().join("."))
        .expect("alias");
    assert_eq!(first, alias);
    let public = serde_json::to_value(state.public_settings()).expect("public DTO");
    assert!(public.get("project_folders").is_none());
    assert!(public.get("ask_confirmed_projects").is_none());
    assert!(
        !public
            .to_string()
            .contains(&directory.path().display().to_string())
    );
}

#[test]
fn a_path_is_never_a_registered_handle() {
    let directory = tempfile::tempdir().expect("temporary project");
    let state = AppState::new(
        directory.path().join("settings.json"),
        Arc::new(RecordingEventSink::default()),
    );
    state
        .register_project(directory.path())
        .expect("registered");
    let error = state
        .select_project(&directory.path().display().to_string())
        .expect_err("forged handle");
    assert_eq!(error.code, "unknown_project");
}

#[test]
fn missing_and_replaced_directories_do_not_reuse_registration_or_consent() {
    let temp = tempfile::tempdir().expect("temp");
    let folder = temp.path().join("project 中文");
    std::fs::create_dir(&folder).expect("project");
    let state = state_at(temp.path());
    let key = state.register_project(&folder).expect("register");
    state.ask_confirm(&key).expect("consent");
    std::fs::rename(&folder, temp.path().join("original")).expect("move original directory");
    assert_eq!(
        state.select_project(&key).expect_err("missing").code,
        "project_folder_missing"
    );
    std::fs::create_dir(&folder).expect("different directory at same path");
    assert_eq!(
        state
            .select_project(&key)
            .expect_err("changed identity")
            .code,
        "project_identity_changed"
    );
    let rows = state.projects_list().expect("rows");
    assert!(rows[0].missing);
    assert!(!rows[0].ask_confirmed);
    state
        .forget_project(&key)
        .expect("forget missing identity without deleting anything");
    assert!(folder.is_dir());
    assert!(temp.path().join("original").is_dir());
}

#[test]
fn consent_survives_app_restart_only_with_its_directory_receipt() {
    let temp = tempfile::tempdir().expect("temp");
    let folder = temp.path().join("registered");
    std::fs::create_dir(&folder).expect("project");
    {
        let state = state_at(temp.path());
        let key = state.register_project(&folder).expect("register");
        state.ask_confirm(&key).expect("consent");
        assert!(!state.projects_list().expect("rows")[0].chat_confirmed);
        state
            .chat_confirm(&key)
            .expect("separate persistence consent");
    }
    let state = state_at(temp.path());
    assert!(state.projects_list().expect("rows")[0].ask_confirmed);
    assert!(state.projects_list().expect("rows")[0].chat_confirmed);
    assert!(
        serde_json::to_value(state.public_settings())
            .expect("public settings")
            .get("chat_consent_identities")
            .is_none()
    );
    drop(state);
    std::fs::rename(&folder, temp.path().join("old")).expect("move original");
    std::fs::create_dir(&folder).expect("replacement");
    let replacement = state_at(temp.path());
    assert!(!replacement.projects_list().expect("rows")[0].ask_confirmed);
    assert!(!replacement.projects_list().expect("rows")[0].chat_confirmed);
}

#[test]
fn one_turn_consent_does_not_authorize_persistent_inference_or_storage() {
    use vibemux_types::chat::ChatHarness;
    use vibemuxd::control::ChatRequest;
    let directory = tempfile::tempdir().expect("project");
    let state = state_at(directory.path());
    let key = state.register_project(directory.path()).expect("register");
    state.ask_confirm(&key).expect("one turn consent");
    let request = ChatRequest::Open {
        operation_id: uuid::Uuid::new_v4(),
        harness: ChatHarness::Claude,
        title: None,
        consent_private_storage: true,
    };
    assert_eq!(
        state
            .chat_request(&key, request)
            .expect_err("separate consent required")
            .code,
        "chat_consent_required"
    );
    assert!(!directory.path().join("chat_private").exists());
}

#[test]
fn a_full_project_list_refuses_new_registration_without_dropping_ownership() {
    let temp = tempfile::tempdir().expect("temp");
    let state = state_at(temp.path());
    for index in 0..16 {
        let folder = temp.path().join(format!("project_{index}"));
        std::fs::create_dir(&folder).expect("project");
        state.register_project(&folder).expect("register");
    }
    let extra = temp.path().join("extra");
    std::fs::create_dir(&extra).expect("extra");
    assert_eq!(
        state.register_project(&extra).expect_err("full").code,
        "project_limit"
    );
    assert_eq!(state.projects_list().expect("rows").len(), 16);
    assert!(
        !extra.join(".vibemux").exists(),
        "registration/listing never starts a daemon"
    );
}

#[test]
fn legacy_path_only_consent_requires_confirmation() {
    use vibemux_desktop::desktop_settings::{DesktopSettings, save_settings_at};
    let temp = tempfile::tempdir().expect("temp");
    let settings = DesktopSettings {
        project_folders: vec![temp.path().to_owned()],
        ask_confirmed_projects: vec![std::fs::canonicalize(temp.path()).expect("canonical")],
        ..DesktopSettings::default()
    };
    save_settings_at(&temp.path().join("settings.json"), &settings).expect("legacy settings");
    let state = state_at(temp.path());
    assert!(!state.projects_list().expect("rows")[0].ask_confirmed);
}

#[test]
fn forgetting_revokes_stale_registered_handles_before_a_queued_action_can_start() {
    use std::sync::atomic::Ordering;
    use vibemux_desktop::{desktop_settings::DesktopSettings, project_registry::ProjectRegistry};
    let temp = tempfile::tempdir().expect("temp");
    let mut registry = ProjectRegistry::default();
    let project = registry
        .register(temp.path(), &DesktopSettings::default())
        .expect("registered");
    project.confirmed.store(true, Ordering::Release);
    let held = project.acquire_submission().expect("pending preflight");
    let queued = project.clone();
    let thread = std::thread::spawn(move || {
        let _submission = queued
            .acquire_submission()
            .expect("released submission gate");
        queued.acquire_job().err().map(|error| error.code)
    });
    let _forget_job = project.acquire_job().expect("forget ownership");
    registry.forget(&project.key);
    drop(_forget_job);
    drop(held);
    assert_eq!(
        thread.join().expect("queued action"),
        Some("unknown_project".to_owned())
    );
    assert!(!project.confirmed.load(Ordering::Acquire));
    assert_eq!(
        project.resolve().expect_err("stale handle").code,
        "unknown_project"
    );
    assert!(registry.get(&project.key).is_err());
}

#[test]
fn explicit_setup_preserves_existing_configuration_before_any_probe_or_restart() {
    use vibemux_desktop::project_status::{ProjectServices, SharedProjectServices};
    use vibemuxd::process::DaemonPaths;
    let temp = tempfile::tempdir().expect("temp");
    let state_dir = temp.path().join(".vibemux");
    std::fs::create_dir(&state_dir).expect("state folder");
    let config = state_dir.join("harness_dispatch.json");
    std::fs::write(&config, b"operator-owned configuration").expect("operator configuration");
    let paths = DaemonPaths::from_project_root(temp.path()).expect("project identity");
    assert_eq!(
        SharedProjectServices
            .setup(&paths)
            .expect_err("existing config")
            .code,
        "ask_config_exists"
    );
    assert_eq!(
        std::fs::read(config).expect("config preserved"),
        b"operator-owned configuration"
    );
    assert!(!state_dir.join("probe_cache.json").exists());
    assert!(!state_dir.join("vibemux_rust.sqlite3").exists());
}

#[test]
fn targets_refuse_missing_detection_disabled_execution_and_unsupported_protocols() {
    use vibemux_desktop::project_status::targets_from_catalog;
    use vibemux_harness::{
        AgentKind, HarnessProtocol, HarnessRow,
        dispatch::{NativeProtocol, route_config::DispatchCatalogEntry},
    };
    let mut route = DispatchCatalogEntry {
        harness: AgentKind::Claude,
        protocol: NativeProtocol::ClaudeStreamJson,
        enabled: true,
        allow_execution: true,
        probe_supported: true,
    };
    let detected = HarnessRow {
        name: "claude".to_owned(),
        command: Vec::new(),
        protocol: HarnessProtocol::Pty,
        provider: "synthetic".to_owned(),
        available: true,
        path: None,
        roles: Vec::new(),
        default: false,
        launcher: None,
        version: None,
    };
    assert!(!targets_from_catalog(&[route], &[]).targets[0].available);
    assert!(targets_from_catalog(&[route], std::slice::from_ref(&detected)).targets[0].available);
    route.enabled = false;
    assert!(!targets_from_catalog(&[route], std::slice::from_ref(&detected)).targets[0].available);
    route.enabled = true;
    route.allow_execution = false;
    assert!(!targets_from_catalog(&[route], std::slice::from_ref(&detected)).targets[0].available);
    route.allow_execution = true;
    route.protocol = NativeProtocol::Acp;
    assert!(!targets_from_catalog(&[route], std::slice::from_ref(&detected)).targets[0].available);
}
