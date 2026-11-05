//! Authenticated native integration against the repository's synthetic vendor.
//! Build vibemux_native_fixture and vibemux_launch_trampoline before explicitly
//! running this native acceptance. The test requires the sibling trampoline
//! artifact that Cargo materializes in the profile's deps directory.
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use uuid::Uuid;
use vibemux_client::{
    ask::worker::{ObservedAskHandle, spawn_ask_for_paths, spawn_observer_for_paths},
    change_notifier::NoopNotifier,
    daemon_control::DaemonCommand,
};
use vibemux_desktop::{
    app_state::AppState,
    ask_registry::AskSpawner,
    command_error::CommandError,
    event_sink::RecordingEventSink,
    project_status::{ProjectServices, SharedProjectServices},
    ui_contract::{AskHarness, AskStatus, AskTargets, DaemonState},
};
use vibemux_harness::{AgentKind, HarnessDetection, dispatch::DispatchPhase};
use vibemux_types::frontend::FrontendTasksQuery;
use vibemuxd::{
    WriterWorker,
    control::{ControlClient, DaemonControlServer},
    process::DaemonPaths,
};

const WAIT_LIMIT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(20);

struct PrivateRuntime(DaemonPaths);
impl ProjectServices for PrivateRuntime {
    fn status(&self, paths: &DaemonPaths) -> (DaemonState, Option<String>) {
        assert_eq!(paths.project_root(), self.0.project_root());
        SharedProjectServices.status(&self.0)
    }
    fn targets(&self, paths: &DaemonPaths) -> AskTargets {
        assert_eq!(paths.project_root(), self.0.project_root());
        SharedProjectServices.targets(&self.0)
    }
    fn lifecycle(&self, _: &DaemonPaths, _: DaemonCommand) -> Result<(), CommandError> {
        panic!("no lifecycle action is part of listing or sending")
    }
    fn setup(&self, _: &DaemonPaths) -> Result<(), CommandError> {
        panic!("setup must remain explicit")
    }
}
impl AskSpawner for PrivateRuntime {
    fn spawn(
        &self,
        root: PathBuf,
        harness: AgentKind,
        prompt: String,
        id: Uuid,
    ) -> std::io::Result<ObservedAskHandle> {
        assert_eq!(root, self.0.project_root());
        spawn_ask_for_paths(self.0.clone(), harness, prompt, id, Arc::new(NoopNotifier))
    }
    fn observe(
        &self,
        root: PathBuf,
        harness: AgentKind,
        id: Uuid,
    ) -> std::io::Result<ObservedAskHandle> {
        assert_eq!(root, self.0.project_root());
        spawn_observer_for_paths(self.0.clone(), harness, id, Arc::new(NoopNotifier))
    }
}

async fn blocking<T: Send + 'static>(call: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(call)
        .await
        .expect("blocking service")
}

async fn wait_for(
    state: &AppState,
    ask_id: &str,
    predicate: impl Fn(&AskStatus) -> bool,
) -> AskStatus {
    let deadline = Instant::now() + WAIT_LIMIT;
    loop {
        let status = state.asks.status(ask_id).expect("known Ask");
        if predicate(&status) {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "Ask did not reach the expected state: {status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn mode(vendor: &Path, value: &str) {
    std::fs::write(
        vendor.join("fixture_mode.json"),
        serde_json::to_vec(&json!({"mode":value})).expect("mode"),
    )
    .expect("write mode");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit native acceptance: requires the prebuilt synthetic fixture and sibling launch trampoline"]
async fn desktop_services_reach_one_canonical_attempt_per_send_and_cancel_owned_work() {
    let test_binary = std::env::current_exe().expect("test executable");
    let deps = test_binary.parent().expect("deps");
    let target = deps.parent().expect("target profile");
    let fixture = target.join(format!(
        "vibemux_native_fixture{}",
        std::env::consts::EXE_SUFFIX
    ));
    let trampoline = deps.join(format!(
        "vibemux_launch_trampoline{}",
        std::env::consts::EXE_SUFFIX
    ));
    assert!(
        fixture.is_file() && trampoline.is_file(),
        "build the test-only fixture and stage its trampoline before this acceptance test"
    );
    let temp = tempfile::tempdir().expect("temporary acceptance root");
    let project = temp.path().join("project 空间");
    let vendor = temp.path().join("vendor 空间");
    std::fs::create_dir_all(project.join(".git")).expect("project");
    std::fs::create_dir_all(&vendor).expect("vendor");
    std::fs::write(
        project.join(".git/HEAD"),
        "0123456789abcdef0123456789abcdef01234567\n",
    )
    .expect("synthetic HEAD");
    #[cfg(windows)]
    let paths = {
        let local_app_data = temp.path().join("private_local_app_data");
        std::fs::create_dir(&local_app_data).expect("private runtime base");
        DaemonPaths::from_project_root_with_local_app_data(&project, &local_app_data)
            .expect("private paths")
    };
    #[cfg(not(windows))]
    let paths = DaemonPaths::from_project_root(&project).expect("paths");
    paths.ensure_runtime_dir().expect("private secured runtime");
    let mut routes = Vec::new();
    let mut detection = BTreeMap::new();
    for (name, protocol) in [
        ("claude", "claude_stream_json"),
        ("codex", "codex_app_server"),
    ] {
        let executable = vendor.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        std::fs::copy(&fixture, &executable).expect("copy synthetic vendor");
        routes.push(json!({"harness":name,"protocol":protocol,"executable":executable,"enabled":true,"allow_execution":true}));
        detection.insert(
            name.to_owned(),
            HarnessDetection {
                detected: true,
                path: None,
                launcher: None,
                version: Some("synthetic".to_owned()),
            },
        );
    }
    let config_path = paths.state_dir().join("harness_dispatch.json");
    let config = serde_json::to_vec(&json!({"schema_version":1,"routes":routes,"limits":{"deadline_ms":20_000,"shutdown_grace_ms":1000}})).expect("config");
    std::fs::write(&config_path, &config).expect("config");
    mode(&vendor, "normal");
    let database = paths.database_path().to_owned();
    blocking(move || {
        let writer = WriterWorker::start(&database).expect("seed writer");
        writer
            .handle()
            .expect("writer handle")
            .commit_harness_snapshot(
                detection,
                "2026-10-31T00:00:00Z".to_owned(),
                "native_desktop_fixture_detection".to_owned(),
            )
            .expect("detection");
        writer.shutdown().expect("seed shutdown");
    })
    .await;
    let server = DaemonControlServer::start_for_paths(&paths)
        .await
        .expect("authenticated daemon");
    let client =
        ControlClient::from_descriptor(server.descriptor_path()).expect("authenticated client");
    let services = Arc::new(PrivateRuntime(paths.clone()));
    let sink = Arc::new(RecordingEventSink::default());
    let state = Arc::new(AppState::with_services(
        temp.path().join("desktop_settings.json"),
        sink.clone(),
        services.clone(),
        services,
    ));
    let key = state
        .register_project(&project)
        .expect("register native project");
    state.select_project(&key).expect("selection");
    let listing = {
        let state = state.clone();
        blocking(move || state.projects_list()).await.expect("list")
    };
    assert_eq!(listing[0].daemon, DaemonState::Running);
    assert!(!listing[0].ask_confirmed);
    let refused = state
        .ask_submit(
            &key,
            AskHarness::Claude,
            "synthetic prompt".to_owned(),
            Uuid::new_v4(),
        )
        .expect_err("consent required");
    assert_eq!(refused.code, "ask_consent_required");
    assert!(
        client
            .frontend_tasks(FrontendTasksQuery::default())
            .await
            .expect("tasks")
            .tasks
            .is_empty()
    );
    state.ask_confirm(&key).expect("explicit test consent");

    for harness in [AskHarness::Claude, AskHarness::Codex] {
        let submission_key = Uuid::new_v4();
        let prompt = "synthetic 中文 ; $(echo untrusted) <script>text</script>".to_owned();
        let first = {
            let state = state.clone();
            let key = key.clone();
            let prompt = prompt.clone();
            tokio::task::spawn_blocking(move || {
                state.ask_submit(&key, harness, prompt, submission_key)
            })
        };
        let second = {
            let state = state.clone();
            let key = key.clone();
            tokio::task::spawn_blocking(move || {
                state.ask_submit(&key, harness, prompt, submission_key)
            })
        };
        let first = first.await.expect("first join").expect("first receipt");
        let second = second
            .await
            .expect("duplicate join")
            .expect("duplicate receipt");
        assert_eq!(first.ask_id, second.ask_id);
        assert_eq!(first.request_id, second.request_id);
        assert_ne!(first.duplicate, second.duplicate);
        // No renderer needs to poll reply to unblock terminal delivery.
        let status = wait_for(&state, &first.ask_id, |status| {
            status.terminal_observed && status.delivery_complete
        })
        .await;
        let canonical = status.canonical.expect("daemon correlation");
        let authoritative = client
            .harness_dispatch_status(Uuid::parse_str(&first.request_id).expect("request ID"))
            .await
            .expect("authoritative status");
        assert_eq!(canonical.task_id, authoritative.task_id.to_string());
        assert_eq!(canonical.run_id, authoritative.run_id.to_string());
        assert_eq!(canonical.phase, DispatchPhase::Completed);
        assert_eq!(canonical.phase, authoritative.phase);
        assert!(
            !state
                .asks
                .reply(&first.ask_id)
                .expect("reply")
                .reply
                .is_empty()
        );
        assert!(
            !serde_json::to_string(&state.asks.status(&first.ask_id).expect("status"))
                .expect("status JSON")
                .contains("untrusted")
        );
    }
    assert_eq!(
        client
            .frontend_tasks(FrontendTasksQuery::default())
            .await
            .expect("task count")
            .tasks
            .len(),
        2
    );
    assert_eq!(
        std::fs::read(&config_path).expect("config unchanged"),
        config
    );
    assert!(
        !serde_json::to_string(&sink.events())
            .expect("events")
            .contains("synthetic result")
    );

    mode(&vendor, "hang");
    let hanging = {
        let state = state.clone();
        let key = key.clone();
        blocking(move || {
            state.ask_submit(
                &key,
                AskHarness::Codex,
                "synthetic cancellation".to_owned(),
                Uuid::new_v4(),
            )
        })
        .await
        .expect("hanging receipt")
    };
    wait_for(&state, &hanging.ask_id, |status| {
        status
            .canonical
            .as_ref()
            .is_some_and(|canonical| canonical.phase == DispatchPhase::Running)
    })
    .await;
    state
        .select_project(&key)
        .expect("selection is not cancellation");
    assert!(state.asks.has_active(&key));
    state.asks.cancel(&hanging.ask_id).expect("explicit cancel");
    let cancelled = wait_for(&state, &hanging.ask_id, |status| status.terminal_observed).await;
    assert_eq!(
        cancelled.canonical.expect("canonical cancellation").phase,
        DispatchPhase::Cancelled
    );

    mode(&vendor, "failed");
    let failed = {
        let state = state.clone();
        let key = key.clone();
        blocking(move || {
            state.ask_submit(
                &key,
                AskHarness::Claude,
                "synthetic failure".to_owned(),
                Uuid::new_v4(),
            )
        })
        .await
        .expect("failure receipt")
    };
    let failure = wait_for(&state, &failed.ask_id, |status| status.terminal_observed).await;
    assert_eq!(
        failure.canonical.expect("canonical failure").phase,
        DispatchPhase::Failed
    );
    let closing = state.clone();
    assert_eq!(blocking(move || closing.asks.shutdown()).await, 0);
    drop(state);
    server.shutdown().await.expect("owned daemon shutdown");
    assert!(!paths.descriptor_path().exists());
}
