#![cfg(feature = "test_helpers")]

use std::{collections::BTreeMap, path::PathBuf, time::Duration};
use tokio::time::{Instant, sleep, timeout};
use vibemux_plugin_protocol::{
    manifest::{ManifestPluginKind, PluginManifest, SupportedPlatform},
    negotiation::CorePluginPolicy,
};
use vibemux_plugin_supervisor::{PluginSupervisorConfig, ResolvedPluginLaunch};
use vibemuxd::{
    WriterError, WriterWorker,
    control::{ControlClient, DaemonControlServer},
    plugin_configuration::PluginStartup,
    plugin_registry::{
        PluginRegistry, PluginRegistryConfig, PluginRestartPolicy, PluginState, PluginStatus,
    },
};

const TEST_DEADLINE: Duration = Duration::from_secs(10);

fn config(mode: &str, plugin_id: &str, directory: &std::path::Path) -> PluginSupervisorConfig {
    let manifest = PluginManifest {
        schema_version: 1,
        plugin_id: plugin_id.to_string(),
        plugin_version: "1.0.0".to_string(),
        kind: ManifestPluginKind::Harness,
        entry_point: vec!["vibemux_registry_mock".to_string(), mode.to_string()],
        capabilities: vec!["mock:cancel".to_string(), "mock:echo".to_string()],
        requested_permissions: vec![],
        supported_platforms: vec![
            SupportedPlatform::Windows,
            SupportedPlatform::Linux,
            SupportedPlatform::Macos,
        ],
        protocol_major: 1,
        minimum_protocol_minor: 0,
        maximum_protocol_minor: 0,
    };
    let environment = BTreeMap::from([
        ("VIBEMUX_MOCK_PLUGIN_ID".to_string(), plugin_id.to_string()),
        (
            "PRIVATE_SENTINEL".to_string(),
            "private_mock_prompt".to_string(),
        ),
    ]);
    let launch = ResolvedPluginLaunch::new(
        &manifest,
        PathBuf::from(env!("CARGO_BIN_EXE_vibemux_registry_mock")),
        directory.to_path_buf(),
        environment,
    )
    .expect("launch");
    let mut config = PluginSupervisorConfig::new(
        manifest,
        launch,
        CorePluginPolicy::default(),
        "unused_session".to_string(),
    );
    config.handshake_timeout = Duration::from_secs(2);
    config.shutdown_timeout = Duration::from_millis(500);
    config.receive_timeout = Duration::from_secs(30);
    config
}

fn policy(max_restarts: u32) -> PluginRestartPolicy {
    PluginRestartPolicy {
        max_restarts,
        initial_backoff: Duration::from_millis(100),
        max_backoff: Duration::from_millis(200),
    }
}

async fn wait_status(client: &ControlClient, plugin_id: &str, state: PluginState) -> PluginStatus {
    timeout(TEST_DEADLINE, async {
        loop {
            let statuses = client.plugin_status().await.expect("status IPC");
            if let Some(status) = statuses
                .into_iter()
                .find(|status| status.plugin_id == plugin_id && status.state == state)
            {
                return status;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("plugin state deadline")
}

async fn wait_registry(registry: &PluginRegistry, state: PluginState) -> PluginStatus {
    timeout(TEST_DEADLINE, async {
        loop {
            if let Some(status) = registry
                .statuses()
                .into_iter()
                .find(|status| status.state == state)
            {
                return status;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("registry state deadline")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crash_budget_exhaustion_quarantine_and_sibling_visibility_over_real_ipc() {
    let temp = tempfile::tempdir().expect("temp");
    let database = temp.path().join("state.sqlite3");
    let startup = PluginStartup {
        registry_config: PluginRegistryConfig::default(),
        registrations: vec![
            (
                config("crash_after_heartbeat", "mock.crash", temp.path()),
                policy(2),
            ),
            (config("normal", "mock.healthy", temp.path()), policy(2)),
        ],
    };
    let server = DaemonControlServer::start_with_plugins(&database, temp.path(), startup)
        .await
        .expect("server");
    let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
    assert!(matches!(
        WriterWorker::start(&database),
        Err(WriterError::LockHeld)
    ));
    let healthy = wait_status(&client, "mock.healthy", PluginState::Active).await;
    let exhausted = wait_status(&client, "mock.crash", PluginState::Quarantined).await;
    assert_eq!(exhausted.restarts, 2);
    assert_eq!(exhausted.max_restarts, 2);
    assert!(exhausted.last_error_code.is_some());
    assert!(exhausted.process_id.is_none());
    // Read calls cannot reset budgets, restart quarantine, or disrupt siblings.
    for _ in 0..3 {
        sleep(Duration::from_millis(150)).await;
        let statuses = client.plugin_status().await.expect("status");
        assert_eq!(
            statuses.iter().find(|s| s.plugin_id == "mock.crash"),
            Some(&exhausted)
        );
        assert_eq!(
            statuses
                .iter()
                .find(|s| s.plugin_id == "mock.healthy")
                .expect("healthy")
                .session_id,
            healthy.session_id
        );
        let json = serde_json::to_string(&statuses).expect("json");
        assert!(!json.contains("private_mock_prompt"));
        assert!(!json.contains(&temp.path().display().to_string()));
    }
    assert!(client.health().await.expect("health after crash").healthy);
    server.shutdown().await.expect("join daemon");
    let writer = WriterWorker::start(&database).expect("writer released only after cleanup");
    assert!(writer.events().expect("events").is_empty());
    writer.shutdown().expect("close writer");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_and_task_shaped_plugin_events_quarantine_without_canonical_mutation() {
    use serde_json::json;
    use time::OffsetDateTime;
    use vibemux_events::{ActorName, EventDraft, EventPayload, EventType};
    use vibemux_types::{EventId, ProjectId, Task, TaskSpec};
    for mode in ["malformed", "unsolicited_event"] {
        let temp = tempfile::tempdir().expect("temp");
        let database = temp.path().join("state.sqlite3");
        let project_id = ProjectId::new();
        let task = Task::new(TaskSpec {
            project_id,
            title: "unchanged".to_string(),
            description: String::new(),
        })
        .expect("task");
        let writer = WriterWorker::start(&database).expect("seed writer");
        writer
            .commit_task(
                task.clone(),
                EventDraft {
                    event_id: EventId::new(),
                    event_type: EventType::new("task_created").expect("type"),
                    project_id,
                    task_id: Some(task.task_id()),
                    run_id: None,
                    causation_id: None,
                    actor: ActorName::new("daemon").expect("actor"),
                    timestamp: OffsetDateTime::now_utc(),
                    idempotency_key: Some("seed".to_string()),
                    payload: EventPayload::new(json!({"source":"seed"})).expect("payload"),
                },
            )
            .expect("seed task");
        let before = writer.events().expect("events");
        let projection = writer
            .projection("task", task.task_id().to_string())
            .expect("projection");
        writer.shutdown().expect("close seed writer");
        let startup = PluginStartup {
            registry_config: PluginRegistryConfig::default(),
            registrations: vec![(config(mode, "mock.harness", temp.path()), policy(3))],
        };
        let server = DaemonControlServer::start_with_plugins(&database, temp.path(), startup)
            .await
            .expect("server");
        let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
        let quarantined = wait_status(&client, "mock.harness", PluginState::Quarantined).await;
        assert_eq!(
            quarantined.restarts, 0,
            "protocol/authority violations never retry"
        );
        assert!(
            !serde_json::to_string(&quarantined)
                .expect("json")
                .contains("private_mock_prompt")
        );
        assert!(client.health().await.expect("healthy core").healthy);
        server.shutdown().await.expect("shutdown");
        let writer = WriterWorker::start(&database).expect("inspect after owner stopped");
        assert_eq!(writer.events().expect("events"), before);
        assert_eq!(
            writer
                .projection("task", task.task_id().to_string())
                .expect("task"),
            projection
        );
        assert!(
            writer
                .projection("run", "mock_run")
                .expect("no run")
                .is_none()
        );
        writer.shutdown().expect("close");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_joins_active_plugin_and_does_not_consume_restart_budget() {
    let temp = tempfile::tempdir().expect("temp");
    let mut registry = PluginRegistry::new(PluginRegistryConfig::default()).expect("registry");
    registry
        .register(config("normal", "mock.harness", temp.path()), policy(3))
        .expect("register");
    let active = wait_registry(&registry, PluginState::Active).await;
    assert!(active.process_id.is_some());
    registry
        .cancel("mock.harness")
        .await
        .expect("cancel and join");
    let stopped = registry.statuses().remove(0);
    assert_eq!(stopped.state, PluginState::Stopped);
    assert_eq!(stopped.restarts, 0);
    assert!(stopped.process_id.is_none());
    assert_eq!(stopped.graceful_shutdown, Some(true));
    registry
        .cancel("mock.harness")
        .await
        .expect("idempotent cancel");
    registry.shutdown().await.expect("idempotent shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_during_backoff_and_handshake_prevents_further_launches() {
    let temp = tempfile::tempdir().expect("temp");
    let mut registry = PluginRegistry::new(PluginRegistryConfig::default()).expect("registry");
    let backoff = PluginRestartPolicy {
        max_restarts: 3,
        initial_backoff: Duration::from_secs(5),
        max_backoff: Duration::from_secs(5),
    };
    registry
        .register(config("crash", "mock.harness", temp.path()), backoff)
        .expect("register");
    wait_registry(&registry, PluginState::Backoff).await;
    timeout(Duration::from_secs(2), registry.cancel("mock.harness"))
        .await
        .expect("backoff cancellation deadline")
        .expect("cancel");
    assert_eq!(registry.statuses()[0].restarts, 0);
    assert_eq!(registry.statuses()[0].state, PluginState::Stopped);
    let mut registry = PluginRegistry::new(PluginRegistryConfig::default()).expect("registry");
    let mut hanging = config("hang_handshake", "mock.harness", temp.path());
    hanging.handshake_timeout = Duration::from_millis(150);
    registry.register(hanging, policy(3)).expect("register");
    tokio::task::yield_now().await;
    timeout(Duration::from_secs(2), registry.cancel("mock.harness"))
        .await
        .expect("handshake cancel deadline")
        .expect("cancel");
    assert_eq!(registry.statuses()[0].restarts, 0);
    assert_eq!(registry.statuses()[0].state, PluginState::Stopped);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_reaps_hanging_plugin_and_reports_forced_cleanup() {
    let temp = tempfile::tempdir().expect("temp");
    let mut registry = PluginRegistry::new(PluginRegistryConfig::default()).expect("registry");
    registry
        .register(
            config("hang_shutdown", "mock.harness", temp.path()),
            policy(3),
        )
        .expect("register");
    let active = wait_registry(&registry, PluginState::Active).await;
    let started = Instant::now();
    let result = registry.shutdown().await;
    assert!(started.elapsed() < Duration::from_secs(5));
    // The status preserves failure even when exact-child forced cleanup succeeded.
    let status = registry.statuses().remove(0);
    assert_eq!(status.state, PluginState::Stopped);
    assert_eq!(status.restarts, 0);
    assert!(status.process_id.is_none());
    assert_eq!(status.graceful_shutdown, Some(false));
    assert!(status.last_error_code.is_some());
    assert!(
        result.is_err(),
        "forced cleanup must preserve the shutdown error"
    );
    assert_process_reaped(active.process_id.expect("owned process"));
}

fn assert_process_reaped(process_id: u32) {
    let mut system = sysinfo::System::new();
    let pid = sysinfo::Pid::from_u32(process_id);
    system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
    assert!(
        system.process(pid).is_none(),
        "owned mock child remains after joined cleanup"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn standalone_daemon_loads_explicit_config_and_joins_plugins_before_exit() {
    use vibemuxd::process::DaemonPaths;
    let temp = tempfile::tempdir().expect("temp");
    let paths = DaemonPaths::from_project_root(temp.path()).expect("paths");
    #[cfg(windows)]
    let _cleanup = RuntimeCleanup(paths.runtime_dir().to_path_buf());
    let manifest_path = temp.path().join("mock.toml");
    std::fs::write(
        &manifest_path,
        r#"schema_version = 1
plugin_id = "mock.harness"
plugin_version = "1.0.0"
kind = "harness"
entry_point = ["vibemux_registry_mock", "normal"]
capabilities = ["mock:cancel", "mock:echo"]
requested_permissions = []
supported_platforms = ["windows", "linux", "macos"]
protocol_major = 1
minimum_protocol_minor = 0
maximum_protocol_minor = 0
"#,
    )
    .expect("manifest");
    let config_path = temp.path().join("plugins.json");
    std::fs::write(&config_path, serde_json::to_vec(&serde_json::json!({
        "schema_version":1,"plugins":[{"manifest_path":manifest_path,
        "executable":env!("CARGO_BIN_EXE_vibemux_registry_mock"),"working_directory":temp.path(),
        "restart":{"max_restarts":0,"initial_backoff_ms":100,"max_backoff_ms":100}}]
    })).expect("config JSON")).expect("config");
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_vibemuxd"));
    command
        .args(["--project-root"])
        .arg(temp.path())
        .arg("--plugin-config")
        .arg(config_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command.spawn().expect("daemon child");
    let client = timeout(TEST_DEADLINE, async {
        loop {
            assert!(
                child.try_wait().expect("daemon status").is_none(),
                "daemon exited early"
            );
            if let Ok(client) = ControlClient::from_descriptor(paths.descriptor_path()) {
                if client
                    .health_with_deadline(Duration::from_millis(200))
                    .await
                    .is_ok()
                {
                    break client;
                }
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("daemon startup");
    let active = wait_status(&client, "mock.harness", PluginState::Active).await;
    client.shutdown().await.expect("shutdown accepted");
    let output = timeout(TEST_DEADLINE, child.wait_with_output())
        .await
        .expect("daemon exit deadline")
        .expect("daemon output");
    assert!(
        output.status.success(),
        "daemon error: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_process_reaped(active.process_id.expect("plugin pid"));
    assert!(!paths.writer_lock_path().exists());
    assert!(!paths.descriptor_path().exists());
}

#[cfg(windows)]
struct RuntimeCleanup(PathBuf);
#[cfg(windows)]
impl Drop for RuntimeCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn periodic_heartbeats_survive_multiple_deadlines_and_silence_exhausts_budget() {
    use vibemux_plugin_protocol::frame::FrameCodecConfig;
    let temp = tempfile::tempdir().expect("temp");
    for (mode, expected) in [
        ("periodic_heartbeat", PluginState::Active),
        ("normal", PluginState::Quarantined),
    ] {
        let mut config = config(mode, "mock.harness", temp.path());
        config.policy = CorePluginPolicy::new(
            vec![],
            vec![],
            FrameCodecConfig::default(),
            8,
            100,
            if cfg!(windows) {
                SupportedPlatform::Windows
            } else if cfg!(target_os = "macos") {
                SupportedPlatform::Macos
            } else {
                SupportedPlatform::Linux
            },
        )
        .expect("policy");
        config.receive_timeout = Duration::from_millis(400);
        let mut registry = PluginRegistry::new(PluginRegistryConfig::default()).expect("registry");
        registry.register(config, policy(0)).expect("register");
        let active = wait_registry(&registry, PluginState::Active).await;
        sleep(Duration::from_millis(1100)).await;
        assert_eq!(registry.statuses()[0].state, expected);
        assert_eq!(registry.statuses()[0].restarts, 0);
        assert_eq!(registry.statuses()[0].session_id, active.session_id);
        registry.shutdown().await.expect("shutdown");
        assert_process_reaped(active.process_id.expect("owned process"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_startup_batch_never_publishes_or_launches_and_duplicate_cannot_reset() {
    let temp = tempfile::tempdir().expect("temp");
    let config = config("normal", "mock.harness", temp.path());
    let invalid_batches = [
        PluginStartup {
            registry_config: PluginRegistryConfig::default(),
            registrations: vec![(config.clone(), policy(1)), (config.clone(), policy(1))],
        },
        PluginStartup {
            registry_config: PluginRegistryConfig { max_plugins: 1 },
            registrations: vec![
                (config.clone(), policy(1)),
                (
                    {
                        let mut c = config.clone();
                        c.manifest.plugin_id = "mock.other".to_string();
                        c
                    },
                    policy(1),
                ),
            ],
        },
        PluginStartup {
            registry_config: PluginRegistryConfig::default(),
            registrations: vec![(
                {
                    let mut c = config.clone();
                    c.receive_timeout = Duration::from_millis(c.policy.heartbeat_interval_ms());
                    c
                },
                policy(1),
            )],
        },
    ];
    let database = temp.path().join("state.sqlite3");
    for startup in invalid_batches {
        assert!(
            DaemonControlServer::start_with_plugins(&database, temp.path(), startup)
                .await
                .is_err()
        );
        assert!(!database.exists());
        assert!(!temp.path().join("control.json").exists());
        assert!(!vibemuxd::writer_lock_path_for_database(&database).exists());
    }
    let mut registry = PluginRegistry::new(PluginRegistryConfig::default()).expect("registry");
    registry
        .register(config.clone(), policy(0))
        .expect("register");
    let active = wait_registry(&registry, PluginState::Active).await;
    assert_eq!(
        registry.register(config, policy(3)),
        Err(vibemuxd::plugin_registry::PluginRegistryError::DuplicatePlugin)
    );
    assert_eq!(registry.statuses()[0].session_id, active.session_id);
    assert_eq!(registry.statuses()[0].max_restarts, 0);
    registry.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_daemon_wait_signals_owned_cleanup_without_aborting_it() {
    let temp = tempfile::tempdir().expect("temp");
    let startup = PluginStartup {
        registry_config: PluginRegistryConfig::default(),
        registrations: vec![(config("normal", "mock.harness", temp.path()), policy(3))],
    };
    let server = DaemonControlServer::start_with_plugins(
        &temp.path().join("state.sqlite3"),
        temp.path(),
        startup,
    )
    .await
    .expect("server");
    let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
    let active = wait_status(&client, "mock.harness", PluginState::Active).await;
    let descriptor = server.descriptor_path().to_path_buf();
    let lock = server.writer_lock_path().to_path_buf();
    assert!(
        timeout(Duration::from_millis(20), server.wait())
            .await
            .is_err()
    );
    timeout(TEST_DEADLINE, async {
        while descriptor.exists() || lock.exists() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("cancelled owner cleanup");
    assert_process_reaped(active.process_id.expect("owned child"));
}
