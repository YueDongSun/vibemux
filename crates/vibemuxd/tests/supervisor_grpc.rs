#![cfg(feature = "test_helpers")]
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Command,
};
use tempfile::TempDir;
use tokio::time::{Duration, sleep, timeout};
use vibemux_a2a::task_grpc::GrpcTaskClient;
use vibemux_a2a::{
    PeerCredential, TaskBinding, TaskClient, TaskClientConfig, TaskPart, TaskRequest, TaskSnapshot,
    TaskState,
};
use vibemux_events::EventEnvelope;
use vibemux_model_peer::provider::ProviderSelection;
use vibemux_types::{ProjectId, RunStatus, TaskId, a2a::A2aRunRecord};
use vibemux_workspace::{WorkspaceManager, sha256};
use vibemuxd::{
    WriterError, WriterWorker,
    control::{ControlClient, DaemonControlServer},
    model_peer_process::{ModelPeerConfig, SupervisorConfig},
    plugin_configuration::PluginStartup,
    process::DaemonPaths,
    supervisor_service::SupervisorServiceConfig,
};

struct Project {
    temp: Option<TempDir>,
    root: PathBuf,
    receipts: PathBuf,
    git: PathBuf,
    base: String,
    paths: DaemonPaths,
}
impl Project {
    fn new() -> Self {
        let temp = tempfile::Builder::new()
            .prefix("vibemux_supervisor_grpc_test_")
            .tempdir()
            .expect("temporary project");
        let root = temp.path().join("repo");
        let receipts = temp.path().join("receipts");
        std::fs::create_dir(&root).expect("repo");
        std::fs::create_dir(&receipts).expect("receipts");
        let git = find_git();
        git_output(&git, &root, &["init", "--quiet"]);
        git_output(&git, &root, &["config", "user.name", "Supervisor fixture"]);
        git_output(
            &git,
            &root,
            &["config", "user.email", "fixture@example.invalid"],
        );
        std::fs::write(root.join("seed.txt"), b"unchanged seed\n").expect("seed");
        std::fs::write(root.join(".gitignore"), b".vibemux/\n").expect("ignore");
        git_output(&git, &root, &["add", "seed.txt", ".gitignore"]);
        git_output(
            &git,
            &root,
            &["commit", "--quiet", "-m", "fixture baseline"],
        );
        let base = git_output(&git, &root, &["rev-parse", "HEAD"])
            .trim()
            .to_string();
        let paths = DaemonPaths::from_project_root(&root).expect("daemon paths");
        paths.ensure_runtime_dir().expect("protected runtime");
        Self {
            temp: Some(temp),
            root,
            receipts,
            git,
            base,
            paths,
        }
    }
    fn config(&self, mode: &str) -> SupervisorServiceConfig {
        let role = |provider: &str, binding| ModelPeerConfig {
            selection: ProviderSelection {
                app_type: "fixture".into(),
                provider_id: provider.into(),
            },
            binding,
        };
        SupervisorServiceConfig {
            bearer_token: token(),
            supervisor: SupervisorConfig {
                project_id: ProjectId::new(),
                project_root: self.root.clone(),
                git_executable: self.git.clone(),
                base_commit: self.base.clone(),
                model_peer_executable: PathBuf::from(env!("CARGO_BIN_EXE_vibemux_model_fixture")),
                cc_switch_database: self.receipts.clone(),
                planner: role("planner", TaskBinding::HttpJson),
                worker: role(mode, TaskBinding::HttpJson),
                reviewer: role("reviewer", TaskBinding::JsonRpc),
            },
        }
    }
    async fn cleanup(mut self, records: &[A2aRunRecord]) {
        let manager = WorkspaceManager::new(self.git.clone(), self.root.clone(), self.base.clone())
            .await
            .expect("workspace manager");
        for record in records {
            manager
                .inspect(&record.workspace)
                .await
                .expect("owned test worktree");
            for name in ["result.json", "review.json", "verification.json"] {
                let path = Path::new(&record.workspace.path).join(name);
                if path.exists() {
                    let metadata = std::fs::symlink_metadata(&path).expect("artifact metadata");
                    assert!(metadata.file_type().is_file());
                    std::fs::remove_file(path)
                        .expect("remove only test-owned artifact after assertions");
                }
            }
            manager
                .cleanup(&record.workspace)
                .await
                .expect("normal Git worktree removal");
        }
        assert_eq!(
            git_output(&self.git, &self.root, &["worktree", "list", "--porcelain"])
                .lines()
                .filter(|line| line.starts_with("worktree "))
                .count(),
            1
        );
        assert_eq!(
            git_output(&self.git, &self.root, &["rev-parse", "HEAD"]).trim(),
            self.base
        );
        assert!(
            git_output(
                &self.git,
                &self.root,
                &["status", "--porcelain", "--untracked-files=all"]
            )
            .is_empty()
        );
        assert_eq!(
            std::fs::read(self.root.join("seed.txt")).expect("seed survives"),
            b"unchanged seed\n"
        );
        #[cfg(windows)]
        std::fs::remove_dir(self.paths.runtime_dir()).expect("remove empty test runtime leaf");
        self.temp
            .take()
            .expect("temp ownership")
            .close()
            .expect("remove completed fixture");
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        if let Some(temp) = self.temp.take() {
            let _ = temp.keep();
        }
    }
}
fn find_git() -> PathBuf {
    let filename = if cfg!(windows) { "git.exe" } else { "git" };
    std::env::split_paths(&std::env::var_os("PATH").expect("PATH"))
        .map(|path| path.join(filename))
        .find(|path| path.is_file())
        .map(|path| std::fs::canonicalize(path).expect("git path"))
        .expect("Git is required for the real worktree test")
}
fn git_output(git: &Path, root: &Path, args: &[&str]) -> String {
    let mut command = Command::new(git);
    command.env_clear();
    for key in ["SystemRoot", "WINDIR", "PATH", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_TERMINAL_PROMPT", "0");
    let output = command
        .args(args)
        .current_dir(root)
        .output()
        .expect("git process");
    assert!(
        output.status.success(),
        "Git fixture command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("git UTF-8")
}
fn token() -> String {
    "fixture_operator_token_abcdefghijklmnopqrstuvwxyz0123456789".into()
}
fn request(repairs: u32) -> TaskRequest {
    TaskRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        context_id: uuid::Uuid::new_v4().to_string(),
        task_id: None,
        idempotency_key: uuid::Uuid::new_v4().to_string(),
        payload: json!({"schema_version":1,"task_description":"Sort the supplied numbers ascending","input":{"values":[3,1,2]},"expected_result":{"sorted":[1,2,3]},"max_repairs":repairs}),
    }
}
async fn start(project: &Project, mode: &str) -> (DaemonControlServer, GrpcTaskClient, TaskClient) {
    let server = DaemonControlServer::start_for_paths_with_supervisor(
        &project.paths,
        PluginStartup::default(),
        project.config(mode),
    )
    .await
    .expect("supervisor daemon");
    assert!(matches!(
        WriterWorker::start(project.paths.database_path()),
        Err(WriterError::LockHeld)
    ));
    let http_url = server.a2a_base_url().expect("HTTP endpoint");
    let grpc_url = server.a2a_grpc_base_url().expect("gRPC endpoint");
    assert_ne!(http_url, grpc_url);
    let http = TaskClient::connect(
        http_url,
        TaskClientConfig {
            allowed_origins: vec![http_url.into(), grpc_url.into()],
            credential: PeerCredential {
                subject: "operator".into(),
                bearer_token: token(),
            },
            preferred_bindings: vec![TaskBinding::JsonRpc],
        },
    )
    .await
    .expect("validate all advertised interface origins");
    let grpc = GrpcTaskClient::connect(
        grpc_url,
        PeerCredential {
            subject: "operator".into(),
            bearer_token: token(),
        },
    )
    .await
    .expect("gRPC client");
    (server, grpc, http)
}
async fn terminal(client: &GrpcTaskClient, id: &str) -> TaskSnapshot {
    timeout(Duration::from_secs(45), async {
        loop {
            let task = client.get(id).await.expect("gRPC task state");
            if task.state.is_terminal() {
                return task;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("gRPC completion deadline")
}
fn submitted(reply: vibemux_a2a::TaskReply) -> TaskSnapshot {
    match reply {
        vibemux_a2a::TaskReply::Task(snapshot) => snapshot,
        _ => panic!("task reply required"),
    }
}
async fn assert_endpoints_closed(http: &str, grpc: &str) {
    for endpoint in [http, grpc] {
        let address = endpoint.strip_prefix("http://").expect("loopback URL");
        assert!(
            tokio::net::TcpStream::connect(address).await.is_err(),
            "owned A2A listener survived shutdown"
        );
    }
}
fn report(task: &TaskSnapshot) -> Value {
    assert_eq!(task.artifacts.len(), 1);
    match &task.artifacts[0].parts[..] {
        [TaskPart::Data(value)] => value.clone(),
        _ => panic!("one structured report expected"),
    }
}
fn fixture_receipts(project: &Project, suffix: &str) -> Vec<Value> {
    std::fs::read_dir(&project.receipts)
        .expect("receipts")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(suffix))
        })
        .map(|path| {
            serde_json::from_slice(&std::fs::read(path).expect("receipt bytes"))
                .expect("receipt JSON")
        })
        .collect()
}
fn assert_children_joined(project: &Project, attempts: u64) {
    let ready = fixture_receipts(project, "_ready.json");
    let stopped = fixture_receipts(project, "_shutdown.json");
    assert_eq!(ready.len(), 3);
    assert_eq!(stopped.len(), 3);
    for receipt in stopped {
        assert_eq!(receipt["shutdown_joined"], true);
        assert_eq!(receipt["active_calls"], 0);
        let expected = if receipt["role"] == "planner" {
            1
        } else {
            attempts
        };
        assert_eq!(receipt["call_count"].as_u64(), Some(expected));
        let pid = sysinfo::Pid::from_u32(
            u32::try_from(receipt["process_id"].as_u64().expect("PID")).expect("PID range"),
        );
        let mut system = sysinfo::System::new();
        system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        assert!(
            system.process(pid).is_none(),
            "exact fixture child survived joined shutdown"
        );
    }
}
fn inspect(project: &Project) -> (Vec<A2aRunRecord>, Vec<EventEnvelope>, Value) {
    // The writer no longer forwards read helpers; all inspection goes
    // through a parallel SqliteStore opened alongside the daemon's own
    // writer (WAL keeps readers and the writer cooperative).
    let writer = WriterWorker::start(project.paths.database_path())
        .expect("writer released after daemon shutdown");
    let store = vibemux_store::SqliteStore::open(project.paths.database_path()).expect("store");
    let events = store.events().expect("events");
    let ids = events
        .iter()
        .filter_map(EventEnvelope::task_id)
        .collect::<BTreeSet<_>>();
    assert_eq!(ids.len(), 1);
    let task_id = *ids.iter().next().expect("task id");
    let records = store.a2a_runs(task_id).expect("records");
    let task = store
        .projection("task", &task_id.to_string())
        .expect("projection")
        .expect("task");
    drop(store);
    writer.shutdown().expect("close inspection writer");
    // Reopen the database in a separate writer to validate replay; the
    // writer exposes no read forwarding anymore, so the comparison goes
    // through a second SqliteStore.
    let reopened_writer =
        WriterWorker::start(project.paths.database_path()).expect("reopen persisted state");
    let reopened = vibemux_store::SqliteStore::open(project.paths.database_path()).expect("reopen");
    assert_eq!(
        reopened.a2a_runs(task_id).expect("verified replay"),
        records
    );
    assert_eq!(reopened.events().expect("immutable event replay"), events);
    drop(reopened);
    reopened_writer.shutdown().expect("close replay writer");
    let encoded = serde_json::to_string(&events).expect("events JSON");
    assert!(!encoded.contains(&token()));
    assert!(!encoded.contains("Sort the supplied numbers ascending"));
    (records, events, task)
}
async fn assert_artifacts_immutable(project: &Project, records: &[A2aRunRecord]) {
    let manager = WorkspaceManager::new(
        project.git.clone(),
        project.root.clone(),
        project.base.clone(),
    )
    .await
    .expect("workspace manager");
    let mut branches = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for record in records {
        assert!(branches.insert(record.workspace.branch.clone()));
        assert!(paths.insert(record.workspace.path.clone()));
        assert_eq!(record.workspace.base_commit, project.base);
        assert_eq!(record.run.base_commit(), project.base);
        let name = if record.run.role() == "worker" {
            "result.json"
        } else {
            "review.json"
        };
        let path = Path::new(&record.workspace.path).join(name);
        let before = std::fs::read(&path).expect("immutable artifact");
        assert!(
            record
                .binding
                .artifacts
                .iter()
                .any(|reference| reference.sha256 == sha256(&before))
        );
        assert!(
            manager
                .write_artifact(&record.workspace, name, &json!({"overwrite":true}))
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read(path).expect("artifact after refused overwrite"),
            before
        );
        assert!(
            manager.plan_cleanup(&record.workspace).await.is_err(),
            "artifact-bearing worktree is not silently cleaned"
        );
        if record.run.role() == "reviewer" {
            let receipt = record
                .binding
                .verification
                .as_ref()
                .expect("review binding");
            let verification =
                std::fs::read(Path::new(&record.workspace.path).join("verification.json"))
                    .expect("verification artifact");
            assert_eq!(sha256(&verification), receipt.verification_sha256);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_supervisor_success_shares_http_state_and_the_single_writer() {
    let project = Project::new();
    let (server, grpc, http) = start(&project, "valid").await;
    let rejected = GrpcTaskClient::connect(
        server.a2a_grpc_base_url().expect("endpoint"),
        PeerCredential {
            subject: "operator".into(),
            bearer_token: "invalid_abcdefghijklmnopqrstuvwxyz0123456789".into(),
        },
    )
    .await
    .expect("auth checked on method");
    let accepted = submitted(grpc.send(&request(0)).await.expect("gRPC work order"));
    assert_eq!(
        rejected
            .get(&accepted.task_id)
            .await
            .expect_err("wrong credential"),
        vibemux_a2a::TaskGatewayError::Unauthorized
    );
    assert_eq!(
        http.get(&accepted.task_id)
            .await
            .expect("same actor task")
            .context_id,
        accepted.context_id
    );
    let done = terminal(&grpc, &accepted.task_id).await;
    assert_eq!(done.state, TaskState::Completed, "gRPC workflow: {done:?}");
    assert_eq!(
        http.get(&accepted.task_id)
            .await
            .expect("same completed snapshot"),
        done
    );
    let report = report(&done);
    assert_eq!(report["verified"], true);
    assert_children_joined(&project, 1);
    assert!(
        ControlClient::from_descriptor(server.descriptor_path())
            .expect("control")
            .health()
            .await
            .expect("health")
            .healthy
    );
    let endpoints = (
        server.a2a_base_url().expect("HTTP").to_string(),
        server.a2a_grpc_base_url().expect("gRPC").to_string(),
    );
    server
        .shutdown()
        .await
        .expect("both transports and runtime joined");
    assert_endpoints_closed(&endpoints.0, &endpoints.1).await;
    let (records, events, task) = inspect(&project);
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .all(|record| record.run.status() == RunStatus::Succeeded)
    );
    assert_eq!(task["status"], "done");
    let canonical: TaskId = report["canonical_task_id"]
        .as_str()
        .expect("task id")
        .parse()
        .expect("typed task id");
    assert!(
        records
            .iter()
            .all(|record| record.task.task_id() == canonical)
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type().as_str() == "a2a_verification_accepted")
    );
    assert_artifacts_immutable(&project, &records).await;
    project.cleanup(&records).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_supervisor_cancel_is_visible_over_http_and_leaves_no_running_run() {
    let project = Project::new();
    let (server, grpc, http) = start(&project, "hang").await;
    let accepted = submitted(grpc.send(&request(0)).await.expect("gRPC work order"));
    timeout(Duration::from_secs(20), async {
        loop {
            if fixture_receipts(&project, "_call_1.json")
                .iter()
                .any(|receipt| receipt["role"] == "worker")
            {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("worker active");
    let canceled = grpc
        .cancel(&accepted.task_id)
        .await
        .expect("cancel over gRPC");
    assert_eq!(canceled.state, TaskState::Canceled);
    assert_eq!(
        http.get(&accepted.task_id)
            .await
            .expect("same canceled actor state"),
        canceled
    );
    assert!(canceled.artifacts.is_empty());
    let endpoints = (
        server.a2a_base_url().expect("HTTP").to_string(),
        server.a2a_grpc_base_url().expect("gRPC").to_string(),
    );
    server.shutdown().await.expect("joined shutdown");
    assert_endpoints_closed(&endpoints.0, &endpoints.1).await;
    let stopped = fixture_receipts(&project, "_shutdown.json");
    assert_eq!(stopped.len(), 3);
    for receipt in stopped {
        assert_eq!(receipt["shutdown_joined"], true);
        assert_eq!(receipt["active_calls"], 0);
        let pid = sysinfo::Pid::from_u32(
            u32::try_from(receipt["process_id"].as_u64().expect("pid")).expect("pid range"),
        );
        let mut system = sysinfo::System::new();
        system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        assert!(system.process(pid).is_none());
    }
    let (records, events, task) = inspect(&project);
    assert!(!records.is_empty());
    assert!(
        records
            .iter()
            .all(|record| matches!(record.run.status(), RunStatus::Stopped | RunStatus::Failed))
    );
    assert_eq!(task["status"], "cancelled");
    assert!(
        !events
            .iter()
            .any(|event| event.event_type().as_str() == "a2a_verification_accepted")
    );
    project.cleanup(&records).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_daemon_has_no_a2a_endpoints() {
    let project = Project::new();
    let server = DaemonControlServer::start_for_paths(&project.paths)
        .await
        .expect("default daemon");
    assert!(server.a2a_base_url().is_none());
    assert!(server.a2a_grpc_base_url().is_none());
    server.shutdown().await.expect("shutdown");
    project.cleanup(&[]).await;
}
