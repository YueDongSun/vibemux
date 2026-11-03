#![cfg(all(feature = "test_helpers", windows))]
//! Authenticated owner operations against real temporary Git worktrees.
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    process::Command,
    time::{sleep, timeout},
};
use uuid::Uuid;
use vibemux_harness::HarnessDetection;
use vibemux_store::{ChatExecutionOwnerOpen, ChatOwnerChange, ChatOwnerMutation, SqliteStore};
use vibemux_types::{
    RunStatus,
    chat::ChatHarness,
    chat_owner::{ChatExecutionOwner, ChatOwnerPhase, ChatWorkspaceEffect},
};
use vibemux_workspace::WorkspaceManager;
use vibemuxd::{
    WriterWorker,
    control::{
        ChatOwnerRequest, ChatOwnerResponse, ChatOwnerStatus, ChatRequest, ChatResponse,
        ControlClient, DaemonControlServer,
    },
    harness_dispatch::DispatchServiceSettings,
};

const WAIT: Duration = Duration::from_secs(25);
const FIXTURE: &str = env!("CARGO_BIN_EXE_vibemux_chat_fixture");
const TRAMPOLINE: &str = env!("CARGO_BIN_EXE_vibemux_launch_trampoline");

async fn git(executable: &Path, root: &Path, arguments: &[&str]) -> String {
    let mut command = Command::new(executable);
    command
        .current_dir(root)
        .args([
            "-c",
            "user.name=Owner Fixture",
            "-c",
            "user.email=fixture@invalid.test",
            "-c",
            "core.hooksPath=NUL",
            "-c",
            "commit.gpgSign=false",
        ])
        .args(arguments)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .env_clear()
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "NUL")
        .creation_flags(0x08000000);
    for key in ["SystemRoot", "WINDIR", "TEMP", "TMP", "PATH"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    let output = timeout(WAIT, command.output())
        .await
        .expect("bounded fixture Git")
        .expect("Git process");
    assert!(
        output.status.success(),
        "Git fixture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("UTF8")
}

struct Fixture {
    temp: tempfile::TempDir,
    project: PathBuf,
    state: PathBuf,
    config: PathBuf,
    git: PathBuf,
    base: String,
    launch: PathBuf,
}
impl Fixture {
    async fn new() -> Self {
        let temp = tempfile::tempdir().expect("temporary fixture");
        let project = temp.path().join("project with spaces_中文");
        let state = project.join(".vibemux");
        std::fs::create_dir_all(&state).expect("state");
        let git_executable = std::env::split_paths(&std::env::var_os("PATH").expect("PATH"))
            .filter(|path| path.is_absolute())
            .map(|path| path.join("git.exe"))
            .find(|path| path.is_file())
            .expect("native Git");
        git(
            &git_executable,
            &project,
            &["init", "--quiet", "--initial-branch=main"],
        )
        .await;
        std::fs::write(project.join("base.txt"), "base remains unchanged\n").expect("base");
        std::fs::write(project.join(".gitignore"), ".vibemux/\n").expect("ignore state");
        git(
            &git_executable,
            &project,
            &["add", "base.txt", ".gitignore"],
        )
        .await;
        git(
            &git_executable,
            &project,
            &["commit", "--quiet", "-m", "fixture base"],
        )
        .await;
        let base = git(&git_executable, &project, &["rev-parse", "HEAD"])
            .await
            .trim()
            .to_owned();
        let vendor = temp.path().join("vendor");
        std::fs::create_dir(&vendor).expect("vendor folder");
        let executable = vendor.join("codex.exe");
        std::fs::copy(FIXTURE, &executable).expect("synthetic image");
        let launch = vendor.join("launch.txt");
        std::fs::write(
            vendor.join("chat_fixture.json"),
            serde_json::to_vec(&json!({"mode":"normal","launch_path":launch})).expect("JSON"),
        )
        .expect("fixture config");
        let config = state.join("harness_dispatch.json");
        std::fs::write(&config,serde_json::to_vec(&json!({"schema_version":1,"routes":[{"harness":"codex","protocol":"codex_app_server","executable":executable,"enabled":true,"allow_execution":true}]})).expect("JSON")).expect("route");
        let database = state.join("state.sqlite3");
        tokio::task::spawn_blocking(move || {
            let writer = WriterWorker::start(&database).expect("writer");
            writer
                .handle()
                .expect("handle")
                .commit_harness_snapshot(
                    BTreeMap::from([(
                        "codex".into(),
                        HarnessDetection {
                            detected: true,
                            path: None,
                            launcher: None,
                            version: Some("synthetic".into()),
                        },
                    )]),
                    "2026-11-01T00:00:00Z".into(),
                    "owner_fixture_detection".into(),
                )
                .expect("record detection");
            writer.shutdown().expect("seed shutdown");
        })
        .await
        .expect("seed task");
        Self {
            temp,
            project,
            state,
            config,
            git: git_executable,
            base,
            launch,
        }
    }
    async fn start(&self) -> DaemonControlServer {
        DaemonControlServer::start_with_dispatch(
            &self.state.join("state.sqlite3"),
            &self.temp.path().join("runtime"),
            DispatchServiceSettings {
                project_root: self.project.clone(),
                config_path: self.config.clone(),
                trampoline: Some(PathBuf::from(TRAMPOLINE)),
            },
        )
        .await
        .expect("daemon")
    }

    async fn seed_plan(&self, claim: bool) -> (ChatExecutionOwner, WorkspaceManager) {
        let database = self.state.join("state.sqlite3");
        let base_commit = self.base.clone();
        let owner = tokio::task::spawn_blocking(move || {
            SqliteStore::open(&database)
                .expect("stopped fixture store")
                .open_chat_execution_owner(&ChatExecutionOwnerOpen {
                    operation_id: Uuid::new_v4(),
                    harness: ChatHarness::Codex,
                    title_ref: None,
                    profile_sha256: "a".repeat(64),
                    base_commit,
                    admitted_tools: Default::default(),
                    timestamp: time::OffsetDateTime::now_utc(),
                })
                .expect("allocate fixture owner")
                .owner
        })
        .await
        .expect("store task");
        let manager =
            WorkspaceManager::new(self.git.clone(), self.project.clone(), self.base.clone())
                .await
                .expect("workspace manager");
        let plan = manager.plan(owner.run_id).expect("planned workspace");
        let path = plan.path.strip_prefix(r"\\?\").unwrap_or(&plan.path);
        let resource_key = vibemux_harness::dispatch::attempt::resource_key(path).to_hex();
        let database = self.state.join("state.sqlite3");
        let owner = tokio::task::spawn_blocking(move || {
            let mut store = SqliteStore::open(&database).expect("stopped store");
            let mutation = |version| ChatOwnerMutation {
                operation_id: Uuid::new_v4(),
                chat_id: owner.chat_id,
                expected_version: version,
                timestamp: time::OffsetDateTime::now_utc(),
            };
            let planned = store
                .change_chat_execution_owner(
                    &mutation(owner.version),
                    &ChatOwnerChange::Plan {
                        workspace: plan,
                        resource_key,
                    },
                )
                .expect("persist plan");
            if claim {
                store
                    .change_chat_execution_owner(
                        &mutation(planned.owner.version),
                        &ChatOwnerChange::ClaimCreation,
                    )
                    .expect("claim effect")
                    .owner
            } else {
                planned.owner
            }
        })
        .await
        .expect("plan task");
        (owner, manager)
    }
}

async fn owner_ready(client: &ControlClient, chat_id: vibemux_types::ChatId) -> ChatOwnerStatus {
    timeout(WAIT, async {
        loop {
            let ChatOwnerResponse::Status { owner: Some(owner) } = client
                .chat_owner(&ChatOwnerRequest::Status { chat_id })
                .await
                .expect("owner status")
            else {
                panic!("missing owner")
            };
            assert_ne!(
                owner.phase,
                ChatOwnerPhase::RecoveryRequired,
                "unexpected provisioning failure: {owner:?}"
            );
            if owner.phase == ChatOwnerPhase::Open && !owner.provisioning_active {
                break owner;
            }
            sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("provision before deadline")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owned_worktree_survives_daemon_restart_and_explicit_stop_retains_files() {
    let fixture = Fixture::new().await;
    let server = fixture.start().await;
    let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
    let open = ChatOwnerRequest::Open {
        operation_id: Uuid::new_v4(),
        harness: ChatHarness::Codex,
        title: Some("PRIVATE_OWNER_TITLE_中文".into()),
        consent_private_storage: true,
        consent_workspace: true,
    };
    assert!(!format!("{open:?}").contains("PRIVATE_"));
    let ChatOwnerResponse::Owner {
        owner: allocated, ..
    } = client.chat_owner(&open).await.expect("open owner")
    else {
        panic!("owner receipt")
    };
    let ready = owner_ready(&client, allocated.chat_id).await;
    assert_eq!(ready.run_id, allocated.run_id);
    assert_eq!(ready.task_id, allocated.task_id);
    assert_eq!(ready.run_status, RunStatus::Preparing);
    assert!(ready.admitted_tools.is_empty());
    assert!(ready.workspace_verified_at.is_some());
    let json = serde_json::to_value(&ready).expect("status JSON");
    for private in [
        "workspace",
        "ownership_token",
        "resource_key",
        "profile_sha256",
        "title",
    ] {
        assert!(json.get(private).is_none(), "private field {private}");
    }
    assert!(
        !serde_json::to_string(&ready)
            .expect("status")
            .contains("PRIVATE_")
    );
    assert!(
        matches!(client.chat_owner(&ChatOwnerRequest::Title {chat_id:ready.chat_id}).await.expect("explicit title"),ChatOwnerResponse::Title {title:Some(title),..} if title=="PRIVATE_OWNER_TITLE_中文")
    );
    assert!(
        matches!(client.chat_owner(&open).await.expect("Open replay"),ChatOwnerResponse::Owner {owner,duplicate:true,..} if owner.run_id==ready.run_id && owner.version==ready.version)
    );
    assert_eq!(
        client
            .chat(&ChatRequest::Status {
                chat_id: ready.chat_id
            })
            .await
            .expect_err("legacy owner access")
            .code(),
        "chat_owner_control_required"
    );
    assert!(
        matches!(client.chat(&ChatRequest::List {after:None,limit:16,archived:true}).await.expect("legacy list"),ChatResponse::List {sessions,..} if sessions.is_empty())
    );
    let worktree = fixture
        .project
        .join(".vibemux/worktrees")
        .join(ready.run_id.to_string());
    assert_eq!(
        git(&fixture.git, &worktree, &["rev-parse", "HEAD"])
            .await
            .trim(),
        fixture.base
    );
    std::fs::write(worktree.join("retained.txt"), "owned candidate\n").expect("owned candidate");
    assert!(
        !fixture.launch.exists(),
        "provisioning never launches the vendor"
    );
    server.shutdown().await.expect("shutdown");
    let server = fixture.start().await;
    let client = ControlClient::from_descriptor(server.descriptor_path()).expect("restart client");
    let resumed = owner_ready(&client, ready.chat_id).await;
    assert_eq!(
        resumed.version, ready.version,
        "restart does not replay provisioning"
    );
    let reconcile = ChatOwnerRequest::Reconcile {
        operation_id: Uuid::new_v4(),
        chat_id: ready.chat_id,
        expected_version: resumed.version,
    };
    client
        .chat_owner(&reconcile)
        .await
        .expect("explicit receipt reconciliation");
    assert!(matches!(
        client
            .chat_owner(&reconcile)
            .await
            .expect("reconcile replay during or after provisioning"),
        ChatOwnerResponse::Owner {
            duplicate: true,
            ..
        }
    ));
    let reconciled = owner_ready(&client, ready.chat_id).await;
    let stop = ChatOwnerRequest::Stop {
        operation_id: Uuid::new_v4(),
        chat_id: ready.chat_id,
        expected_version: reconciled.version,
    };
    let ChatOwnerResponse::Owner { owner: stopped, .. } =
        client.chat_owner(&stop).await.expect("explicit stop")
    else {
        panic!("stop receipt")
    };
    assert_eq!(stopped.phase, ChatOwnerPhase::Finalized);
    assert_eq!(stopped.run_status, RunStatus::Failed);
    assert_eq!(
        std::fs::read_to_string(worktree.join("retained.txt")).expect("retained candidate"),
        "owned candidate\n"
    );
    assert!(!fixture.launch.exists());
    server.shutdown().await.expect("final shutdown");
    let database = fixture.state.join("state.sqlite3");
    let record = tokio::task::spawn_blocking(move || {
        SqliteStore::open(&database)
            .expect("exclusive verification after shutdown")
            .chat_execution_owner(ready.chat_id)
            .expect("owner")
            .expect("known")
    })
    .await
    .expect("read task");
    std::fs::remove_file(worktree.join("retained.txt")).expect("remove only test-owned fixture");
    let manager = WorkspaceManager::new(
        fixture.git.clone(),
        fixture.project.clone(),
        fixture.base.clone(),
    )
    .await
    .expect("cleanup manager");
    manager
        .cleanup(record.workspace.as_ref().expect("persisted plan"))
        .await
        .expect("owned clean worktree cleanup");
    assert_eq!(
        git(&fixture.git, &fixture.project, &["rev-parse", "HEAD"])
            .await
            .trim(),
        fixture.base
    );
    assert!(
        git(&fixture.git, &fixture.project, &["status", "--porcelain"])
            .await
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn persisted_creation_claim_reconciles_the_exact_receipt_after_restart() {
    let fixture = Fixture::new().await;
    let (owner, manager) = fixture.seed_plan(true).await;
    let plan = owner.workspace.as_ref().expect("persisted plan");
    manager
        .create_planned(owner.run_id, plan)
        .await
        .expect("Git effect before crash");
    let worktree = PathBuf::from(&plan.path);
    std::fs::write(worktree.join("candidate.txt"), "retained across recovery\n")
        .expect("candidate");
    let server = fixture.start().await;
    let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
    let ChatOwnerResponse::Status {
        owner: Some(before),
    } = client
        .chat_owner(&ChatOwnerRequest::Status {
            chat_id: owner.chat_id,
        })
        .await
        .expect("pending owner")
    else {
        panic!("owner status")
    };
    assert_eq!(before.workspace_effect, Some(ChatWorkspaceEffect::Claimed));
    assert!(
        !before.provisioning_active,
        "restart does not repeat an external effect"
    );
    assert_eq!(
        client
            .chat_owner(&ChatOwnerRequest::Stop {
                operation_id: Uuid::new_v4(),
                chat_id: owner.chat_id,
                expected_version: before.version,
            })
            .await
            .expect_err("unknown claim blocks finalization")
            .code(),
        "chat_busy"
    );
    client
        .chat_owner(&ChatOwnerRequest::Reconcile {
            operation_id: Uuid::new_v4(),
            chat_id: owner.chat_id,
            expected_version: before.version,
        })
        .await
        .expect("explicit recovery");
    let ready = owner_ready(&client, owner.chat_id).await;
    assert_eq!(ready.run_id, owner.run_id);
    assert_eq!(ready.workspace_effect, Some(ChatWorkspaceEffect::Settled));
    assert_eq!(
        std::fs::read_to_string(worktree.join("candidate.txt")).expect("candidate"),
        "retained across recovery\n"
    );
    assert!(!fixture.launch.exists());
    client
        .chat_owner(&ChatOwnerRequest::Stop {
            operation_id: Uuid::new_v4(),
            chat_id: owner.chat_id,
            expected_version: ready.version,
        })
        .await
        .expect("stop after receipt verification");
    server.shutdown().await.expect("shutdown");
    std::fs::remove_file(worktree.join("candidate.txt")).expect("remove fixture candidate");
    manager.cleanup(plan).await.expect("clean owned workspace");
    assert_eq!(
        git(&fixture.git, &fixture.project, &["rev-parse", "HEAD"])
            .await
            .trim(),
        fixture.base
    );
    assert!(
        git(&fixture.git, &fixture.project, &["status", "--porcelain"])
            .await
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_existing_foreign_destination_is_retained_and_never_adopted() {
    let fixture = Fixture::new().await;
    let (owner, _) = fixture.seed_plan(false).await;
    let path = PathBuf::from(&owner.workspace.as_ref().expect("plan").path);
    std::fs::create_dir_all(&path).expect("foreign fixture directory");
    std::fs::write(path.join("foreign.txt"), "foreign file\n").expect("foreign sentinel");
    let server = fixture.start().await;
    let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
    client
        .chat_owner(&ChatOwnerRequest::Reconcile {
            operation_id: Uuid::new_v4(),
            chat_id: owner.chat_id,
            expected_version: owner.version,
        })
        .await
        .expect("reconcile queued");
    let failed = timeout(WAIT, async {
        loop {
            let ChatOwnerResponse::Status {
                owner: Some(current),
            } = client
                .chat_owner(&ChatOwnerRequest::Status {
                    chat_id: owner.chat_id,
                })
                .await
                .expect("status")
            else {
                panic!("owner")
            };
            if !current.provisioning_active {
                break current;
            }
            sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("bounded refusal");
    assert_eq!(failed.phase, ChatOwnerPhase::RecoveryRequired);
    assert_eq!(failed.workspace_effect, Some(ChatWorkspaceEffect::Settled));
    assert!(failed.workspace_verified_at.is_none());
    assert_eq!(
        std::fs::read_to_string(path.join("foreign.txt")).expect("retained sentinel"),
        "foreign file\n"
    );
    assert!(!fixture.launch.exists());
    client
        .chat_owner(&ChatOwnerRequest::Stop {
            operation_id: Uuid::new_v4(),
            chat_id: owner.chat_id,
            expected_version: failed.version,
        })
        .await
        .expect("stop settled failed provisioning");
    server.shutdown().await.expect("shutdown");
    assert_eq!(
        git(
            &fixture.git,
            &fixture.project,
            &["worktree", "list", "--porcelain"]
        )
        .await
        .lines()
        .filter(|line| line.starts_with("worktree "))
        .count(),
        1
    );
    assert_eq!(
        git(&fixture.git, &fixture.project, &["rev-parse", "HEAD"])
            .await
            .trim(),
        fixture.base
    );
    assert!(
        git(&fixture.git, &fixture.project, &["status", "--porcelain"])
            .await
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reconcile_receipt_replays_after_a_persisted_cleanup_failure() {
    use vibemux_types::chat_owner::ChatOwnerFailure;

    let fixture = Fixture::new().await;
    let (owner, _) = fixture.seed_plan(true).await;
    let mutation = ChatOwnerMutation {
        operation_id: Uuid::new_v4(),
        chat_id: owner.chat_id,
        expected_version: owner.version,
        timestamp: time::OffsetDateTime::now_utc(),
    };
    let database = fixture.state.join("state.sqlite3");
    let admission = mutation.clone();
    let failed = tokio::task::spawn_blocking(move || {
        let mut store = SqliteStore::open(&database).expect("stopped fixture store");
        let reconciled = store
            .change_chat_execution_owner(&admission, &ChatOwnerChange::Reconcile)
            .expect("durable reconcile before crash");
        store
            .change_chat_execution_owner(
                &ChatOwnerMutation {
                    operation_id: Uuid::new_v4(),
                    chat_id: admission.chat_id,
                    expected_version: reconciled.owner.version,
                    timestamp: time::OffsetDateTime::now_utc(),
                },
                &ChatOwnerChange::WorkspaceFailed {
                    code: ChatOwnerFailure::WorkspaceCleanupUnverified,
                    cleanup_verified: false,
                },
            )
            .expect("record unreaped process")
            .owner
    })
    .await
    .expect("fixture transition");
    let server = fixture.start().await;
    let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
    let replay = ChatOwnerRequest::Reconcile {
        operation_id: mutation.operation_id,
        chat_id: mutation.chat_id,
        expected_version: mutation.expected_version,
    };
    assert!(
        matches!(client.chat_owner(&replay).await.expect("known operation receipt"),
        ChatOwnerResponse::Owner {owner, duplicate: true, ..}
            if owner.version == failed.version && !owner.provisioning_active)
    );
    assert_eq!(
        client
            .chat_owner(&ChatOwnerRequest::Reconcile {
                operation_id: mutation.operation_id,
                chat_id: mutation.chat_id,
                expected_version: failed.version,
            })
            .await
            .expect_err("changed replay binding")
            .code(),
        "chat_conflict"
    );
    assert_eq!(
        client
            .chat_owner(&ChatOwnerRequest::Reconcile {
                operation_id: Uuid::new_v4(),
                chat_id: mutation.chat_id,
                expected_version: failed.version,
            })
            .await
            .expect_err("fresh reconcile cannot clear known cleanup uncertainty")
            .code(),
        "workspace_process_cleanup_failed"
    );
    assert!(!Path::new(&owner.workspace.expect("plan").path).exists());
    assert!(!fixture.launch.exists());
    server.shutdown().await.expect("shutdown");
    assert_eq!(
        git(
            &fixture.git,
            &fixture.project,
            &["worktree", "list", "--porcelain"]
        )
        .await
        .lines()
        .filter(|line| line.starts_with("worktree "))
        .count(),
        1
    );
    assert!(
        git(&fixture.git, &fixture.project, &["status", "--porcelain"])
            .await
            .is_empty()
    );
}
