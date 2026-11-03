#![cfg(all(feature = "test_helpers", windows))]
//! Full private writer -> owned actor -> contained fixture -> file effect loop.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use time::OffsetDateTime;
use tokio::{
    process::Command,
    time::{sleep, timeout},
};
use uuid::Uuid;
use vibemux_harness::{
    HarnessDetection,
    chat_session::{SessionConfig, SessionTarget, TurnRequest},
    dispatch::{attempt, launch_spec::interpreter_script_path},
};
use vibemux_store::{
    ChatExecutionOwnerOpen, ChatExecutionStart, ChatOwnerChange, ChatOwnerMutation,
};
use vibemux_types::{
    RunStatus, ToolName,
    a2a::RunWorkspace,
    chat::{ChatHarness, ChatTurnOutcome},
    chat_approval::{ChatApproval, ChatApprovalDecision, ChatApprovalDelivery},
    chat_owner::ChatExecutionOwner,
};
use vibemux_workspace::WorkspaceManager;
use vibemuxd::{
    WriterWorker,
    chat_session::{
        RuntimeApprovalDecision, RuntimeBinding, RuntimeLimits, RuntimeOwner,
        fixture_runtime::{
            FIXTURE_APPROVAL_CONTENT, FIXTURE_APPROVAL_FILE, FixtureApprovalSettings,
            spawn_fixture_approval_runtime,
        },
    },
    chat_writer::{ChatExecutionPrompt, ChatWriter},
};

const WAIT: Duration = Duration::from_secs(30);

#[path = "support/approval_runtime_cases.rs"]
mod boundary_cases;
#[path = "support/approval_control_cases.rs"]
mod control_cases;
async fn git(executable: &Path, root: &Path, args: &[&str]) -> String {
    let mut command = Command::new(executable);
    command
        .current_dir(root)
        .args([
            "-c",
            "user.name=Approval Fixture",
            "-c",
            "user.email=fixture@invalid.test",
            "-c",
            "core.hooksPath=NUL",
            "-c",
            "commit.gpgSign=false",
        ])
        .args(args)
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
        .expect("bounded Git")
        .expect("Git");
    assert!(
        output.status.success(),
        "Git: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("UTF8")
}
enum FixtureWriter {
    Standalone(WriterWorker),
    Daemon(vibemuxd::control::DaemonControlServer),
}
impl FixtureWriter {
    fn handle(&self) -> vibemuxd::WriterHandle {
        match self {
            Self::Standalone(writer) => writer.handle().expect("writer handle"),
            Self::Daemon(server) => {
                server
                    .fixture_chat_context()
                    .expect("same daemon context")
                    .0
            }
        }
    }
    async fn shutdown(self) {
        match self {
            Self::Standalone(writer) => tokio::task::spawn_blocking(move || writer.shutdown())
                .await
                .expect("writer join")
                .expect("writer shutdown"),
            Self::Daemon(server) => server.shutdown().await.expect("daemon shutdown"),
        }
    }
}
struct Fixture {
    _temp: tempfile::TempDir,
    project: PathBuf,
    git: PathBuf,
    vendor: PathBuf,
    writer: FixtureWriter,
    chats: ChatWriter,
    owner: ChatExecutionOwner,
    manager: WorkspaceManager,
    workspace: RunWorkspace,
    binding: RuntimeBinding,
    config: SessionConfig,
    base: String,
}
impl Fixture {
    fn workspace_path(&self) -> &Path {
        Path::new(&self.workspace.path)
    }
    async fn new() -> Self {
        Self::setup(false).await
    }
    async fn with_control() -> Self {
        Self::setup(true).await
    }
    async fn setup(control: bool) -> Self {
        let temp = tempfile::tempdir().expect("fixture");
        let project = temp.path().join("approval project_中文");
        std::fs::create_dir(&project).expect("project");
        let git_executable = std::env::split_paths(&std::env::var_os("PATH").expect("PATH"))
            .filter(|p| p.is_absolute())
            .map(|p| p.join("git.exe"))
            .find(|p| p.is_file())
            .expect("Git executable");
        git(
            &git_executable,
            &project,
            &["init", "--quiet", "--initial-branch=main"],
        )
        .await;
        std::fs::write(project.join("base.txt"), "unchanged base\n").expect("base");
        std::fs::write(project.join(".gitignore"), ".vibemux/\n").expect("ignore");
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
        let state = project.join(".vibemux");
        std::fs::create_dir(&state).expect("state");
        let database = state.join("state.sqlite3");
        let writer = if control {
            FixtureWriter::Daemon(
                vibemuxd::control::DaemonControlServer::start(&database, &state.join("runtime"))
                    .await
                    .expect("owned Control daemon"),
            )
        } else {
            FixtureWriter::Standalone(
                tokio::task::spawn_blocking(move || {
                    WriterWorker::start_with_config(
                        &database,
                        vibemuxd::DEFAULT_WRITER_QUEUE_CAPACITY,
                        WAIT,
                    )
                })
                .await
                .expect("writer task")
                .expect("writer"),
            )
        };
        let handle = writer.handle();
        let seed = handle.clone();
        tokio::task::spawn_blocking(move || {
            seed.commit_harness_snapshot(
                BTreeMap::from([(
                    "claude".into(),
                    HarnessDetection {
                        detected: true,
                        path: None,
                        launcher: None,
                        version: Some("fixture".into()),
                    },
                )]),
                "2026-11-02T00:00:00Z".into(),
                "approval_runtime_detection".into(),
            )
        })
        .await
        .expect("seed task")
        .expect("seed");
        let chats = ChatWriter::attach(handle, state)
            .await
            .expect("chat writer");
        let mut owner = chats
            .open_execution_owner(
                ChatExecutionOwnerOpen {
                    operation_id: Uuid::new_v4(),
                    harness: ChatHarness::Claude,
                    profile_sha256: "a".repeat(64),
                    base_commit: base.clone(),
                    admitted_tools: BTreeSet::from([ToolName::WriteFiles]),
                    title_ref: None,
                    timestamp: OffsetDateTime::now_utc(),
                },
                None,
            )
            .await
            .expect("owner")
            .owner;
        let manager = WorkspaceManager::new(git_executable.clone(), project.clone(), base.clone())
            .await
            .expect("manager");
        let workspace = manager.plan(owner.run_id).expect("plan");
        let resource_key =
            attempt::resource_key(&interpreter_script_path(&workspace.path)).to_hex();
        for change in [
            ChatOwnerChange::Plan {
                workspace: workspace.clone(),
                resource_key,
            },
            ChatOwnerChange::ClaimCreation,
        ] {
            owner = chats
                .change_execution_owner(
                    ChatOwnerMutation {
                        operation_id: Uuid::new_v4(),
                        chat_id: owner.chat_id,
                        expected_version: owner.version,
                        timestamp: OffsetDateTime::now_utc(),
                    },
                    change,
                )
                .await
                .expect("workspace intent")
                .owner;
        }
        manager
            .create_planned(owner.run_id, &workspace)
            .await
            .expect("owned worktree");
        owner = chats
            .change_execution_owner(
                ChatOwnerMutation {
                    operation_id: Uuid::new_v4(),
                    chat_id: owner.chat_id,
                    expected_version: owner.version,
                    timestamp: OffsetDateTime::now_utc(),
                },
                ChatOwnerChange::VerifyWorkspace,
            )
            .await
            .expect("workspace receipt")
            .owner;
        let session_id = Uuid::new_v4();
        let started = chats
            .start_execution_process(ChatExecutionStart {
                operation_id: Uuid::new_v4(),
                chat_id: owner.chat_id,
                expected_generation: 1,
                expected_owner_version: owner.version,
                profile_sha256: owner.profile_sha256.clone(),
                resource_key: owner.resource_key.clone().expect("resource"),
                observed_commit: base.clone(),
                expected_vendor_id: Some(session_id.to_string()),
                timestamp: OffsetDateTime::now_utc(),
            })
            .await
            .expect("process claim");
        let binding = RuntimeBinding {
            chat_id: owner.chat_id,
            generation: 1,
            owner_fence: started.fence.expect("fresh fence"),
        };
        let config = SessionConfig {
            target: SessionTarget::Claude {
                session_id,
                resume: false,
            },
            generation_id: Uuid::new_v4(),
            working_directory: interpreter_script_path(&workspace.path),
            model: None,
        };
        let vendor = temp.path().join("vendor");
        std::fs::create_dir(&vendor).expect("vendor");
        std::fs::copy(
            env!("CARGO_BIN_EXE_vibemux_chat_fixture"),
            vendor.join("claude.exe"),
        )
        .expect("exact fixture copy");
        Self {
            _temp: temp,
            project,
            git: git_executable,
            vendor,
            writer,
            chats,
            owner,
            manager,
            workspace,
            binding,
            config,
            base,
        }
    }
    async fn runtime(&self, limits: RuntimeLimits) -> RuntimeOwner {
        spawn_fixture_approval_runtime(
            self.chats.clone(),
            self.binding.clone(),
            FixtureApprovalSettings {
                project_root: self.project.clone(),
                git: self.git.clone(),
                vendor_directory: self.vendor.clone(),
                trampoline: PathBuf::from(env!("CARGO_BIN_EXE_vibemux_launch_trampoline")),
                runtime_limits: limits,
            },
            self.config.clone(),
        )
        .await
        .expect("guarded fixture runtime")
    }
    async fn send(&self, runtime: &RuntimeOwner) -> Uuid {
        self.send_with_deadline(runtime, Duration::from_secs(10))
            .await
    }
    async fn send_with_deadline(&self, runtime: &RuntimeOwner, deadline: Duration) -> Uuid {
        self.send_handle(&runtime.handle, deadline).await
    }
    async fn send_handle(
        &self,
        handle: &vibemuxd::chat_session::RuntimeHandle,
        deadline: Duration,
    ) -> Uuid {
        handle.wait_ready(WAIT).await.expect("ready");
        let request_id = Uuid::new_v4();
        let admitted = self
            .chats
            .admit_execution_attempt(ChatExecutionPrompt {
                chat_id: self.owner.chat_id,
                request_id,
                generation: 1,
                owner_version: self.owner.version,
                observed_commit: self.base.clone(),
                effective_tools: BTreeSet::from([ToolName::WriteFiles]),
                prompt: "synthetic approval input".into(),
            })
            .await
            .expect("admit input");
        handle
            .send(
                TurnRequest {
                    request_id,
                    turn_id: *admitted.attempt.attempt_id.as_uuid(),
                    prompt: "synthetic approval input".into(),
                },
                deadline,
            )
            .await
            .expect("input queued");
        request_id
    }
    async fn cleanup(self, runtime: RuntimeOwner) -> Vec<vibemux_events::EventEnvelope> {
        runtime.handle.close();
        let ended = runtime.shutdown().await.expect("runtime join");
        assert!(
            ended.cleanup.root_reaped
                && ended.cleanup.tree_empty
                && ended.cleanup.cleanup_verified
                && ended.writer_confirmed,
            "cleanup: {ended:?}"
        );
        self.cleanup_closed().await
    }
    async fn cleanup_closed(self) -> Vec<vibemux_events::EventEnvelope> {
        let owner = self
            .chats
            .execution_owner(self.owner.chat_id)
            .await
            .expect("owner state")
            .expect("owner");
        self.chats
            .change_execution_owner(
                ChatOwnerMutation {
                    operation_id: Uuid::new_v4(),
                    chat_id: self.owner.chat_id,
                    expected_version: owner.owner.version,
                    timestamp: OffsetDateTime::now_utc(),
                },
                ChatOwnerChange::Stop,
            )
            .await
            .expect("retained-file Stop");
        let effect = PathBuf::from(&self.workspace.path).join(FIXTURE_APPROVAL_FILE);
        if effect.exists() {
            assert_eq!(
                std::fs::read(&effect).expect("effect"),
                FIXTURE_APPROVAL_CONTENT.as_bytes()
            );
            std::fs::remove_file(&effect).expect("only owned fixture artifact");
        }
        self.manager
            .cleanup(&self.workspace)
            .await
            .expect("clean owned worktree cleanup");
        assert_eq!(
            std::fs::read_to_string(self.project.join("base.txt")).expect("main base"),
            "unchanged base\n"
        );
        assert_eq!(
            git(&self.git, &self.project, &["rev-parse", "HEAD"])
                .await
                .trim(),
            self.base
        );
        assert!(
            git(&self.git, &self.project, &["status", "--porcelain"])
                .await
                .trim()
                .is_empty()
        );
        let database = self.project.join(".vibemux").join("state.sqlite3");
        drop(self.chats);
        self.writer.shutdown().await;
        tokio::task::spawn_blocking(move || {
            vibemux_store::SqliteStore::open(&database)
                .expect("closed test store")
                .events()
                .expect("retained events")
        })
        .await
        .expect("closed-store audit")
    }
}
async fn challenge(runtime: &RuntimeOwner) -> ChatApproval {
    timeout(WAIT, async {
        loop {
            if let Some(value) = runtime.handle.pending_approval() {
                return value;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("persisted human challenge")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explicit_one_use_decision_reaches_the_contained_file_effect_and_keeps_lifetime_run() {
    for decision in [ChatApprovalDecision::AllowOnce, ChatApprovalDecision::Deny] {
        let fixture = Fixture::new().await;
        let runtime = fixture.runtime(RuntimeLimits::default()).await;
        let request = fixture.send(&runtime).await;
        let approval = challenge(&runtime).await;
        let effect = PathBuf::from(&fixture.workspace.path).join(FIXTURE_APPROVAL_FILE);
        assert!(!effect.exists(), "no effect before a matching decision");
        let choose = RuntimeApprovalDecision {
            approval_id: approval.approval_id,
            operation_id: Uuid::new_v4(),
            expected_version: approval.version,
            decision,
        };
        let receipt = runtime
            .handle
            .decide_approval(choose)
            .await
            .expect("decision");
        assert_eq!(receipt.decision, Some(decision));
        let completed = timeout(WAIT, async {
            loop {
                let attempt = fixture
                    .chats
                    .execution_attempt(request)
                    .await
                    .expect("attempt")
                    .expect("known");
                if attempt.phase.is_terminal() {
                    return attempt;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("settled attempt");
        assert_eq!(completed.outcome, Some(ChatTurnOutcome::Completed));
        let delivery = fixture
            .chats
            .approval_metadata(fixture.owner.chat_id, approval.approval_id)
            .await
            .expect("delivery receipt");
        assert_eq!(delivery.delivery, Some(ChatApprovalDelivery::Written));
        assert_eq!(effect.exists(), decision == ChatApprovalDecision::AllowOnce);
        let owner = fixture
            .chats
            .execution_owner(fixture.owner.chat_id)
            .await
            .expect("owner")
            .expect("known");
        assert_eq!(owner.run.status(), RunStatus::Running);
        assert!(
            runtime.handle.decide_approval(choose).await.is_err(),
            "retired native challenge cannot resend"
        );
        fixture.cleanup(runtime).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_a_human_wait_invalidates_the_challenge_without_a_file_effect() {
    let fixture = Fixture::new().await;
    let runtime = fixture.runtime(RuntimeLimits::default()).await;
    let request = fixture.send(&runtime).await;
    let approval = challenge(&runtime).await;
    let attempt = fixture
        .chats
        .execution_attempt(request)
        .await
        .expect("attempt")
        .expect("known");
    runtime
        .handle
        .cancel(*attempt.attempt_id.as_uuid())
        .await
        .expect("owned cancellation");
    timeout(WAIT, async {
        loop {
            let value = fixture
                .chats
                .execution_attempt(request)
                .await
                .expect("attempt")
                .expect("known");
            if value.phase.is_terminal() {
                assert_ne!(value.outcome, Some(ChatTurnOutcome::Completed));
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("cancelled attempt");
    let value = fixture
        .chats
        .approval_metadata(fixture.owner.chat_id, approval.approval_id)
        .await
        .expect("approval");
    assert_eq!(
        value.state,
        vibemux_types::chat_approval::ChatApprovalState::Invalidated
    );
    assert!(value.decision.is_none());
    assert!(
        !PathBuf::from(&fixture.workspace.path)
            .join(FIXTURE_APPROVAL_FILE)
            .exists()
    );
    fixture.cleanup(runtime).await;
}
