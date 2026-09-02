#![cfg(feature = "test_helpers")]
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Command,
};
use tempfile::TempDir;
use tokio::time::{Duration, sleep, timeout};
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
            .prefix("vibemux_supervisor_test_")
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
async fn start(project: &Project, mode: &str) -> (DaemonControlServer, TaskClient) {
    let server = DaemonControlServer::start_for_paths_with_supervisor(
        &project.paths,
        PluginStartup::default(),
        project.config(mode),
    )
    .await
    .expect("supervisor daemon");
    assert!(
        matches!(
            WriterWorker::start(project.paths.database_path()),
            Err(WriterError::LockHeld)
        ),
        "daemon remains sole writer"
    );
    let url = server.a2a_base_url().expect("supervisor endpoint");
    let client = TaskClient::connect(
        url,
        TaskClientConfig {
            allowed_origins: vec![
                url.to_string(),
                server.a2a_grpc_base_url().expect("grpc origin").to_string(),
            ],
            credential: PeerCredential {
                subject: "operator".into(),
                bearer_token: token(),
            },
            preferred_bindings: vec![TaskBinding::JsonRpc],
        },
    )
    .await
    .expect("supervisor client");
    (server, client)
}
async fn terminal(client: &TaskClient, id: &str) -> TaskSnapshot {
    timeout(Duration::from_secs(45), async {
        loop {
            let task = client.get(id).await.expect("task state");
            if task.state.is_terminal() {
                return task;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("supervisor completion deadline")
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
    let writer = WriterWorker::start(project.paths.database_path())
        .expect("writer released after daemon shutdown");
    let events = writer.events().expect("events");
    let ids = events
        .iter()
        .filter_map(EventEnvelope::task_id)
        .collect::<BTreeSet<_>>();
    assert_eq!(ids.len(), 1);
    let task_id = *ids.iter().next().expect("task id");
    let records = writer.a2a_runs(task_id).expect("records");
    let task = writer
        .projection("task", task_id.to_string())
        .expect("projection")
        .expect("task");
    writer.shutdown().expect("close inspection writer");
    let reopened =
        WriterWorker::start(project.paths.database_path()).expect("reopen persisted state");
    assert_eq!(
        reopened.a2a_runs(task_id).expect("verified replay"),
        records
    );
    assert_eq!(reopened.events().expect("immutable event replay"), events);
    reopened.shutdown().expect("close replay writer");
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
async fn supervisor_closed_loop_verifies_result_persists_evidence_and_joins_children() {
    let project = Project::new();
    let (server, client) = start(&project, "valid").await;
    let accepted = client.send(&request(0)).await.expect("work order accepted");
    let done = terminal(&client, &accepted.task_id).await;
    assert_eq!(
        done.state,
        TaskState::Completed,
        "supervisor result: {done:?}"
    );
    let report = report(&done);
    assert_eq!(report["verified"], true);
    assert_eq!(report["result"]["sorted"][0].as_f64(), Some(1.0));
    assert_eq!(report["attempts"].as_array().expect("attempts").len(), 1);
    assert_children_joined(&project, 1);
    assert!(
        ControlClient::from_descriptor(server.descriptor_path())
            .expect("control")
            .health()
            .await
            .expect("healthy")
            .healthy
    );
    server.shutdown().await.expect("joined daemon");
    let (records, events, task) = inspect(&project);
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .all(|record| record.run.status() == RunStatus::Succeeded)
    );
    assert_eq!(task["status"], "done");
    assert!(
        events
            .iter()
            .any(|event| event.event_type().as_str() == "a2a_verification_accepted")
    );
    let canonical: TaskId = report["canonical_task_id"]
        .as_str()
        .expect("task id")
        .parse()
        .expect("opaque task id");
    assert!(
        records
            .iter()
            .all(|record| record.task.task_id() == canonical)
    );
    assert_artifacts_immutable(&project, &records).await;
    project.cleanup(&records).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rejected_verification_creates_distinct_repair_runs_and_preserves_failure_history() {
    let project = Project::new();
    let (server, client) = start(&project, "repair_once").await;
    let accepted = client.send(&request(1)).await.expect("accepted");
    let done = terminal(&client, &accepted.task_id).await;
    assert_eq!(done.state, TaskState::Completed, "repair result: {done:?}");
    let report = report(&done);
    let attempts = report["attempts"].as_array().expect("attempts");
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0]["accepted"], false);
    assert_eq!(attempts[1]["accepted"], true);
    assert_ne!(attempts[0]["worker_run_id"], attempts[1]["worker_run_id"]);
    assert_ne!(
        attempts[0]["reviewer_run_id"],
        attempts[1]["reviewer_run_id"]
    );
    assert_children_joined(&project, 2);
    server.shutdown().await.expect("daemon shutdown");
    let (records, events, task) = inspect(&project);
    assert_eq!(records.len(), 4);
    let workers = records
        .iter()
        .filter(|record| record.run.role() == "worker")
        .collect::<Vec<_>>();
    assert_eq!(workers.len(), 2);
    assert_eq!(workers[0].run.status(), RunStatus::Failed);
    assert!(
        !workers[0]
            .binding
            .verification
            .as_ref()
            .expect("negative receipt")
            .accepted
    );
    assert_eq!(workers[1].run.status(), RunStatus::Succeeded);
    assert_eq!(task["status"], "done");
    let rejected = events
        .iter()
        .position(|event| event.event_type().as_str() == "a2a_verification_rejected")
        .expect("rejected event");
    let approved = events
        .iter()
        .position(|event| event.event_type().as_str() == "a2a_verification_accepted")
        .expect("accepted event");
    assert!(rejected < approved);
    assert!(
        events
            .windows(2)
            .all(|pair| pair[0].sequence().get() < pair[1].sequence().get())
    );
    assert_artifacts_immutable(&project, &records).await;
    project.cleanup(&records).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_leaves_no_running_or_successful_run_and_reaps_all_children() {
    let project = Project::new();
    let (server, client) = start(&project, "hang").await;
    let accepted = client.send(&request(0)).await.expect("accepted");
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
    .expect("worker active before cancel");
    let canceled = client
        .cancel(&accepted.task_id)
        .await
        .expect("joined workflow cancellation");
    assert_eq!(canceled.state, TaskState::Canceled);
    assert!(canceled.artifacts.is_empty());
    server.shutdown().await.expect("daemon shutdown");
    let stopped = fixture_receipts(&project, "_shutdown.json");
    assert_eq!(stopped.len(), 3);
    for receipt in stopped {
        assert_eq!(receipt["shutdown_joined"], true);
        assert_eq!(receipt["active_calls"], 0);
        let pid = sysinfo::Pid::from_u32(
            u32::try_from(receipt["process_id"].as_u64().expect("pid")).expect("PID range"),
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
    assert!(
        events
            .iter()
            .any(|event| event.event_type().as_str() == "a2a_cancellation_confirmed")
    );
    project.cleanup(&records).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_during_review_preserves_worker_completion_observation_and_cancels_local_task() {
    let project = Project::new();
    let mut config = project.config("valid");
    config.supervisor.reviewer.selection.provider_id = "hang".into();
    let server = DaemonControlServer::start_for_paths_with_supervisor(
        &project.paths,
        PluginStartup::default(),
        config,
    )
    .await
    .expect("server");
    let url = server.a2a_base_url().expect("url");
    let client = TaskClient::connect(
        url,
        TaskClientConfig {
            allowed_origins: vec![
                url.into(),
                server.a2a_grpc_base_url().expect("grpc origin").to_string(),
            ],
            credential: PeerCredential {
                subject: "operator".into(),
                bearer_token: token(),
            },
            preferred_bindings: vec![TaskBinding::HttpJson],
        },
    )
    .await
    .expect("client");
    let accepted = client.send(&request(0)).await.expect("submit");
    timeout(Duration::from_secs(20), async {
        while !fixture_receipts(&project, "_call_1.json")
            .iter()
            .any(|receipt| receipt["role"] == "reviewer")
        {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("review started after worker completion");
    assert_eq!(
        client
            .cancel(&accepted.task_id)
            .await
            .expect("cancel")
            .state,
        TaskState::Canceled
    );
    server.shutdown().await.expect("joined shutdown");
    let (records, events, task) = inspect(&project);
    assert_eq!(task["status"], "cancelled");
    let worker = records
        .iter()
        .find(|record| record.run.role() == "worker")
        .expect("worker");
    assert_eq!(
        worker.binding.last_remote_state,
        vibemux_types::a2a::A2aRemoteState::Completed
    );
    assert_eq!(
        worker.binding.cancellation_state,
        vibemux_types::a2a::A2aCancellationState::None
    );
    assert_eq!(worker.run.status(), RunStatus::Failed);
    assert!(
        events
            .iter()
            .any(|event| event.event_type().as_str() == "a2a_workflow_cancelled")
    );
    assert!(
        !events
            .iter()
            .any(|event| event.event_type().as_str() == "a2a_verification_accepted")
    );
    project.cleanup(&records).await;
}
