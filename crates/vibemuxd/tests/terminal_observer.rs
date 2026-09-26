#![cfg(feature = "test_helpers")]

use prost::Message;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};
use vibemux_plugin_protocol::{
    frame::FrameCodecConfig,
    manifest::{ManifestPluginKind, PluginManifest, SupportedPlatform},
    negotiation::CorePluginPolicy,
    terminal::{FOCUS_METHOD, FOCUS_PERMISSION, INVENTORY_METHOD, OBSERVE_PERMISSION},
    wire::TerminalInventoryRequest,
};
use vibemux_plugin_supervisor::{PluginSupervisorConfig, ResolvedPluginLaunch};
use vibemux_types::{
    ProjectId, Run, RunSpec, Task, TaskSpec,
    a2a::{A2aRunStart, RunWorkspace},
};
use vibemuxd::{
    WriterWorker,
    plugin_registry::{PluginRegistry, PluginRegistryConfig, PluginRestartPolicy, PluginState},
    terminal_observer::{TerminalLinkRequest, TerminalObserver, TerminalQuery},
};

async fn registry(root: &Path, mode: &str, grants: bool) -> PluginRegistry {
    let manifest = PluginManifest {
        schema_version: 1,
        plugin_id: "wezterm_observer".into(),
        plugin_version: "1.0.0".into(),
        kind: ManifestPluginKind::Terminal,
        entry_point: vec!["vibemux_registry_mock".into(), mode.into()],
        capabilities: vec![INVENTORY_METHOD.into(), FOCUS_METHOD.into()],
        requested_permissions: vec![
            OBSERVE_PERMISSION.into(),
            FOCUS_PERMISSION.into(),
            "process:execute".into(),
        ],
        supported_platforms: vec![
            SupportedPlatform::Windows,
            SupportedPlatform::Linux,
            SupportedPlatform::Macos,
        ],
        protocol_major: 1,
        minimum_protocol_minor: 0,
        maximum_protocol_minor: 0,
    };
    let launch = ResolvedPluginLaunch::new(
        &manifest,
        PathBuf::from(env!("CARGO_BIN_EXE_vibemux_registry_mock")),
        root.to_path_buf(),
        BTreeMap::from([("VIBEMUX_MOCK_PLUGIN_ID".into(), "wezterm_observer".into())]),
    )
    .unwrap();
    let platform = if cfg!(windows) {
        SupportedPlatform::Windows
    } else if cfg!(target_os = "macos") {
        SupportedPlatform::Macos
    } else {
        SupportedPlatform::Linux
    };
    let policy = CorePluginPolicy::new(
        manifest.capabilities.clone(),
        if grants {
            manifest.requested_permissions.clone()
        } else {
            vec![]
        },
        FrameCodecConfig::default(),
        8,
        1000,
        platform,
    )
    .unwrap();
    let mut config = PluginSupervisorConfig::new(manifest, launch, policy, "initial".into());
    config.handshake_timeout = Duration::from_secs(2);
    config.shutdown_timeout = Duration::from_secs(2);
    config.receive_timeout = Duration::from_secs(5);
    let mut registry = PluginRegistry::new(PluginRegistryConfig::default()).unwrap();
    registry
        .register(
            config,
            PluginRestartPolicy {
                max_restarts: 0,
                ..Default::default()
            },
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while registry.statuses()[0].state != PluginState::Active {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    registry
}

fn start(writer: &WriterWorker, root: &Path) -> TerminalQuery {
    let task = Task::new(TaskSpec {
        project_id: writer.handle().unwrap().project_id().unwrap().unwrap(),
        title: "Terminal observation fixture".into(),
        description: String::new(),
    })
    .unwrap();
    let run = Run::new(RunSpec {
        project_id: task.project_id(),
        task_id: task.task_id(),
        harness: "fixture".into(),
        role: "worker".into(),
        protocol: "a2a".into(),
        base_commit: "a".repeat(40),
    })
    .unwrap();
    let query = TerminalQuery {
        project_id: task.project_id(),
        task_id: task.task_id(),
        run_id: run.run_id(),
        plugin_id: "wezterm_observer".into(),
    };
    writer
        .handle()
        .unwrap()
        .start_a2a_run(A2aRunStart {
            task,
            run,
            peer_id: "fixture_peer".into(),
            external_task_id: "fixture_task".into(),
            transport: "http_json".into(),
            protocol_version: "1.0".into(),
            workspace: RunWorkspace {
                path: root.to_string_lossy().into_owned(),
                branch: "codex/fixture".into(),
                base_commit: "a".repeat(40),
                ownership_token: "private_owner".into(),
            },
            timestamp: time::OffsetDateTime::now_utc(),
            idempotency_key: "fixture_start".into(),
        })
        .unwrap();
    query
}

#[tokio::test]
async fn native_surface_observation_never_changes_execution_and_requires_identity() {
    let temp = tempfile::tempdir().unwrap();
    let mut registry = registry(temp.path(), "terminal_normal", true).await;
    let writer = WriterWorker::start(&temp.path().join("state.sqlite3")).unwrap();
    let query = start(&writer, temp.path());
    let handle = writer.handle().unwrap();
    let before = handle.a2a_run(query.run_id).unwrap().unwrap();
    let observer = TerminalObserver::new(registry.request_client());
    let snapshot = observer
        .inspect(handle.clone(), query.clone())
        .await
        .unwrap();
    assert_eq!(snapshot.inventory.panes.len(), 1);
    let request = TerminalLinkRequest {
        query: query.clone(),
        instance_id: snapshot.inventory.instance_id,
        pane_id: "1".into(),
    };
    let binding = observer
        .link(handle.clone(), request.clone())
        .await
        .unwrap();
    assert_eq!(
        observer
            .link(handle.clone(), request.clone())
            .await
            .unwrap()
            .binding_id,
        binding.binding_id
    );
    observer
        .focus(handle.clone(), &binding.binding_id)
        .await
        .unwrap();
    let mut invalid = request.clone();
    invalid.instance_id = "stale_instance".into();
    assert_eq!(
        observer.link(handle.clone(), invalid).await.unwrap_err(),
        "terminal_instance_changed"
    );
    let mut foreign = query.clone();
    foreign.project_id = ProjectId::new();
    assert_eq!(
        observer.inspect(handle.clone(), foreign).await.unwrap_err(),
        "terminal_project_mismatch"
    );
    observer.unlink(&binding.binding_id).unwrap();
    assert!(
        observer
            .focus(handle.clone(), &binding.binding_id)
            .await
            .is_err()
    );
    let rebound = observer.link(handle.clone(), request).await.unwrap();
    registry.shutdown().await.unwrap();
    assert!(
        observer
            .focus(handle.clone(), &rebound.binding_id)
            .await
            .is_err()
    );
    assert_eq!(handle.a2a_run(query.run_id).unwrap().unwrap(), before);
    writer.shutdown().unwrap();
}

#[tokio::test]
async fn permissions_and_response_correlation_fail_closed() {
    for (mode, grants, expected) in [
        ("terminal_normal", false, "terminal_permission_denied"),
        (
            "terminal_wrong_correlation",
            true,
            "terminal_invalid_response",
        ),
        ("terminal_timeout", true, "terminal_request_timeout"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let mut registry = registry(temp.path(), mode, grants).await;
        let client = registry.request_client();
        let result = client
            .request(
                "wezterm_observer",
                INVENTORY_METHOD,
                TerminalInventoryRequest {
                    expected_cwd: temp.path().to_string_lossy().into_owned(),
                }
                .encode_to_vec(),
            )
            .await;
        assert_eq!(result.err().unwrap(), expected);
        assert!(
            client
                .request("wezterm_observer", "terminal:send_text", vec![])
                .await
                .is_err()
        );
        registry.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn control_v4_returns_terminal_capability_errors_without_state_mutation() {
    use vibemuxd::control::{ControlClient, DaemonControlServer};
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("state.sqlite3");
    let writer = WriterWorker::start(&database).unwrap();
    let query = start(&writer, temp.path());
    let before = writer.handle().unwrap().a2a_run(query.run_id).unwrap();
    writer.shutdown().unwrap();
    let runtime = temp.path().join("runtime");
    std::fs::create_dir(&runtime).unwrap();
    let server = DaemonControlServer::start(&database, &runtime)
        .await
        .unwrap();
    let client = ControlClient::from_descriptor(server.descriptor_path()).unwrap();
    assert_eq!(
        client
            .terminal_inspect(query.clone())
            .await
            .unwrap_err()
            .code(),
        "terminal_plugin_unavailable"
    );
    assert_eq!(
        client
            .terminal_focus("invalid_binding")
            .await
            .unwrap_err()
            .code(),
        "terminal_link_expired"
    );
    assert_eq!(
        client
            .terminal_unlink("invalid_binding")
            .await
            .unwrap_err()
            .code(),
        "terminal_invalid_request"
    );
    assert!(client.health().await.unwrap().healthy);
    server.shutdown().await.unwrap();
    let writer = WriterWorker::start(&database).unwrap();
    assert_eq!(
        writer.handle().unwrap().a2a_run(query.run_id).unwrap(),
        before
    );
    writer.shutdown().unwrap();
}
