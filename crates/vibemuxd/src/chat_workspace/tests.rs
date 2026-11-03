use super::*;
use crate::{WriterHandle, WriterWorker, harness_dispatch::child_path_text};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use time::OffsetDateTime;
use tokio::{process::Command, time::timeout};
use uuid::Uuid;
use vibemux_store::{ChatExecutionOwnerOpen, ChatOwnerChange, ChatOwnerMutation};
use vibemux_types::{chat::ChatHarness, chat_owner::ChatExecutionOwner};

const WAIT: Duration = Duration::from_secs(25);

#[tokio::test]
async fn missing_managed_namespace_is_not_recreated_by_observation() {
    let fixture = OwnedFixture::new().await;
    let managed = fixture
        .worktree()
        .parent()
        .expect("managed parent")
        .to_path_buf();
    let held = fixture.state_dir.join("held_fixture_worktrees");
    let canonical_root = std::fs::canonicalize(&fixture.root).expect("owned root");
    let canonical_managed = std::fs::canonicalize(&managed).expect("owned managed namespace");
    let canonical_state = std::fs::canonicalize(&fixture.state_dir).expect("owned state");
    assert!(canonical_managed.strip_prefix(&canonical_root).is_ok());
    assert_eq!(canonical_managed.parent(), Some(canonical_state.as_path()));
    assert_eq!(held.parent(), Some(fixture.state_dir.as_path()));
    assert!(!held.exists());
    // Move only this fixture-owned namespace, then restore the exact fault.
    std::fs::rename(&managed, &held).expect("inject missing namespace");
    let observed = inspect_owner_workspace(&fixture.root, &fixture.owner).await;
    let recreated = managed.exists();
    if recreated {
        std::fs::remove_dir(&managed).expect("remove only newly created empty test namespace");
    }
    std::fs::rename(&held, &managed).expect("restore fixture namespace");
    fixture.finish().await;
    assert!(observed.is_err());
    assert!(
        !recreated,
        "observation must not recreate a missing managed namespace"
    );
}

pub(crate) async fn fixture_git(git: &Path, directory: &Path, arguments: &[&str]) -> String {
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let mut command = Command::new(git);
    command
        .current_dir(directory)
        .args([
            "-c",
            "user.name=Workspace Fixture",
            "-c",
            "user.email=fixture@invalid.test",
            "-c",
            if cfg!(windows) {
                "core.hooksPath=NUL"
            } else {
                "core.hooksPath=/dev/null"
            },
            "-c",
            "commit.gpgSign=false",
        ])
        .args(arguments)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .env_clear()
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", null)
        .env("GIT_TERMINAL_PROMPT", "0");
    for name in ["SystemRoot", "WINDIR", "TEMP", "TMP", "PATH"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let output = timeout(WAIT, command.output())
        .await
        .expect("bounded fixture Git")
        .expect("Git output");
    assert!(
        output.status.success(),
        "fixture Git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("fixture UTF-8 output")
}

/// All mutable resources belong to this temporary repository. Call finish after
/// restoring injected faults; unexpected teardown retains the root for recovery.
pub(crate) struct OwnedFixture {
    temp: Option<tempfile::TempDir>,
    pub(crate) root: PathBuf,
    pub(crate) state_dir: PathBuf,
    pub(crate) git: PathBuf,
    pub(crate) base: String,
    pub(crate) owner: ChatExecutionOwner,
    pub(crate) writer: WriterHandle,
    pub(crate) manager: WorkspaceManager,
    pub(crate) worker: Option<WriterWorker>,
}

impl OwnedFixture {
    pub(crate) async fn new() -> Self {
        let temp = tempfile::tempdir().expect("owned fixture");
        let root = temp.path().join("owned project with spaces_中文");
        std::fs::create_dir(&root).expect("project");
        let root = std::fs::canonicalize(root).expect("canonical fixture root");
        let git_root = root.clone();
        let git = tokio::task::spawn_blocking(move || native_git(&git_root))
            .await
            .expect("Git lookup")
            .expect("native Git");
        fixture_git(&git, &root, &["init", "--quiet", "--initial-branch=main"]).await;
        std::fs::write(root.join("base.txt"), "owned base remains unchanged\n").expect("base");
        std::fs::write(root.join(".gitignore"), ".vibemux/\n").expect("ignore fixture state");
        fixture_git(&git, &root, &["add", "base.txt", ".gitignore"]).await;
        fixture_git(&git, &root, &["commit", "--quiet", "-m", "fixture base"]).await;
        let base = fixture_git(&git, &root, &["rev-parse", "HEAD"])
            .await
            .trim()
            .to_owned();
        let state_dir = root.join(".vibemux");
        std::fs::create_dir_all(&state_dir).expect("state");
        let database = state_dir.join("state.sqlite3");
        let worker = tokio::task::spawn_blocking(move || WriterWorker::start(&database))
            .await
            .expect("writer startup")
            .expect("writer");
        let writer = worker.handle().expect("writer handle");
        let source = writer.clone();
        let base_commit = base.clone();
        let owner = tokio::task::spawn_blocking(move || {
            source.with_store(move |store| {
                store.open_chat_execution_owner(&ChatExecutionOwnerOpen {
                    operation_id: Uuid::new_v4(),
                    harness: ChatHarness::Codex,
                    profile_sha256: "a".repeat(64),
                    base_commit,
                    admitted_tools: BTreeSet::new(),
                    title_ref: None,
                    timestamp: OffsetDateTime::now_utc(),
                })
            })
        })
        .await
        .expect("owner task")
        .expect("authoritative allocation")
        .owner;
        let manager = WorkspaceManager::new(git.clone(), root.clone(), base.clone())
            .await
            .expect("manager");
        let mut fixture = Self {
            temp: Some(temp),
            root,
            state_dir,
            git,
            base,
            owner,
            writer,
            manager,
            worker: Some(worker),
        };
        let plan = fixture.manager.plan(fixture.owner.run_id).expect("plan");
        let path = child_path_text(Path::new(&plan.path)).expect("child path");
        let resource_key = vibemux_harness::dispatch::attempt::resource_key(&path).to_hex();
        fixture
            .change(ChatOwnerChange::Plan {
                workspace: plan.clone(),
                resource_key,
            })
            .await;
        fixture.change(ChatOwnerChange::ClaimCreation).await;
        fixture
            .manager
            .create_planned(fixture.owner.run_id, &plan)
            .await
            .expect("owned Git effect");
        fixture.change(ChatOwnerChange::VerifyWorkspace).await;
        fixture
    }

    pub(crate) async fn change(&mut self, change: ChatOwnerChange) {
        let source = self.writer.clone();
        let mutation = ChatOwnerMutation {
            operation_id: Uuid::new_v4(),
            chat_id: self.owner.chat_id,
            expected_version: self.owner.version,
            timestamp: OffsetDateTime::now_utc(),
        };
        self.owner = tokio::task::spawn_blocking(move || {
            source.with_store(move |store| store.change_chat_execution_owner(&mutation, &change))
        })
        .await
        .expect("owner mutation task")
        .expect("authoritative mutation")
        .owner;
    }

    pub(crate) fn worktree(&self) -> PathBuf {
        PathBuf::from(&self.owner.workspace.as_ref().expect("owned workspace").path)
    }

    pub(crate) async fn advance_head(&self) -> String {
        fixture_git(
            &self.git,
            &self.worktree(),
            &["commit", "--quiet", "--allow-empty", "-m", "owned advance"],
        )
        .await;
        fixture_git(&self.git, &self.worktree(), &["rev-parse", "HEAD"])
            .await
            .trim()
            .to_owned()
    }

    pub(crate) async fn finish(mut self) {
        let record = self.owner.workspace.as_ref().expect("owned receipt");
        let cleanup = self.manager.cleanup(record).await;
        let head = fixture_git(&self.git, &self.root, &["rev-parse", "HEAD"]).await;
        let status = fixture_git(&self.git, &self.root, &["status", "--porcelain"]).await;
        let intact = head.trim() == self.base && status.is_empty();
        let worker = self.worker.take().expect("owned writer");
        let joined = tokio::task::spawn_blocking(move || worker.shutdown())
            .await
            .expect("writer join task");
        let temp = self.temp.take().expect("owned root");
        if cleanup.is_ok() && intact && joined.is_ok() {
            drop(temp);
        } else {
            eprintln!(
                "retained_owned_workspace_fixture: {}",
                temp.keep().display()
            );
        }
        joined.expect("writer joined");
        cleanup.expect("only verified clean owned worktree removed");
        assert!(intact, "main fixture repository integrity");
    }

    /// Teardown for a cleanup-service test which has already removed the owned
    /// worktree. Verify retained branch/postconditions instead of deleting twice.
    pub(crate) async fn finish_removed(self) {
        let head = self.base.clone();
        self.finish_removed_at(&head).await;
    }
    pub(crate) async fn finish_removed_at(mut self, approved_head: &str) {
        let record = self.owner.workspace.as_ref().expect("owned receipt");
        let observed = self.manager.observe_removed(record, approved_head).await;
        let head = fixture_git(&self.git, &self.root, &["rev-parse", "HEAD"]).await;
        let status = fixture_git(&self.git, &self.root, &["status", "--porcelain"]).await;
        let intact = head.trim() == self.base && status.is_empty();
        let worker = self.worker.take().expect("owned writer");
        let joined = tokio::task::spawn_blocking(move || worker.shutdown())
            .await
            .expect("writer join task");
        let temp = self.temp.take().expect("owned root");
        if matches!(observed, Ok(true)) && intact && joined.is_ok() {
            drop(temp);
        } else {
            eprintln!(
                "retained_removed_workspace_fixture: {}",
                temp.keep().display()
            );
        }
        joined.expect("writer joined");
        assert!(
            matches!(observed, Ok(true)),
            "owned path/registration absent and branch retained"
        );
        assert!(intact, "main fixture repository integrity");
    }
}

impl Drop for OwnedFixture {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take()
            && worker.shutdown().is_err()
        {
            eprintln!("workspace_fixture_writer_shutdown_unconfirmed");
        }
        if let Some(temp) = self.temp.take() {
            eprintln!(
                "retained_unfinished_workspace_fixture: {}",
                temp.keep().display()
            );
        }
    }
}

#[tokio::test]
async fn physical_inspection_rejects_changed_receipts_branches_resources_and_owner_phase() {
    let fixture = OwnedFixture::new().await;
    let path = fixture.worktree();
    let baseline = inspect_owner_workspace(&fixture.root, &fixture.owner).await;
    let git_dir = fixture_git(&fixture.git, &path, &["rev-parse", "--absolute-git-dir"]).await;
    let receipt = PathBuf::from(git_dir.trim()).join("vibemux_owner.json");
    let original = std::fs::read(&receipt).expect("owned receipt");
    let mut foreign_receipt: serde_json::Value =
        serde_json::from_slice(&original).expect("receipt JSON");
    foreign_receipt["workspace"]["ownership_token"] =
        serde_json::json!(Uuid::new_v4().simple().to_string());
    std::fs::write(
        &receipt,
        serde_json::to_vec(&foreign_receipt).expect("foreign receipt"),
    )
    .expect("injected well-formed owned receipt fault");
    let receipt_refused = inspect_owner_workspace(&fixture.root, &fixture.owner)
        .await
        .is_err();
    std::fs::write(&receipt, original).expect("restore exact owned receipt");
    fixture_git(
        &fixture.git,
        &fixture.root,
        &["branch", "qa_unrelated", &fixture.base],
    )
    .await;
    fixture_git(
        &fixture.git,
        &path,
        &["symbolic-ref", "HEAD", "refs/heads/qa_unrelated"],
    )
    .await;
    let branch_refused = inspect_owner_workspace(&fixture.root, &fixture.owner)
        .await
        .is_err();
    let owned_branch = format!(
        "refs/heads/{}",
        fixture.owner.workspace.as_ref().expect("workspace").branch
    );
    fixture_git(
        &fixture.git,
        &path,
        &["symbolic-ref", "HEAD", &owned_branch],
    )
    .await;
    let mut wrong_resource = fixture.owner.clone();
    let mut resource = wrong_resource
        .resource_key
        .as_ref()
        .expect("resource")
        .as_bytes()
        .to_vec();
    resource[0] = if resource[0] == b'0' { b'1' } else { b'0' };
    wrong_resource.resource_key = Some(String::from_utf8(resource).expect("hex"));
    let resource_refused = inspect_owner_workspace(&fixture.root, &wrong_resource)
        .await
        .is_err();
    let mut provisioning = fixture.owner.clone();
    provisioning.phase = ChatOwnerPhase::Provisioning;
    let phase_refused = inspect_owner_workspace(&fixture.root, &provisioning)
        .await
        .is_err();
    fixture.finish().await;
    assert_eq!(baseline.expect("valid physical ownership"), path);
    assert!(receipt_refused && branch_refused && resource_refused && phase_refused);
}

#[tokio::test]
async fn dirty_files_and_advanced_owned_head_remain_observable() {
    let fixture = OwnedFixture::new().await;
    let path = fixture.worktree();
    let dirty = path.join("owned_dirty_fixture.txt");
    std::fs::write(&dirty, "owned test data\n").expect("owned dirty file");
    let status = fixture
        .manager
        .change_status(fixture.owner.workspace.as_ref().expect("workspace"))
        .await
        .expect("actual Git status");
    let dirty_observation = inspect_owner_workspace(&fixture.root, &fixture.owner).await;
    std::fs::remove_file(&dirty).expect("remove only the exact test-created file");
    let advanced = fixture.advance_head().await;
    let advanced_observation = inspect_owner_workspace(&fixture.root, &fixture.owner).await;
    let base = fixture.base.clone();
    fixture.finish().await;
    assert!(status.contains("owned_dirty_fixture.txt"));
    assert_eq!(
        dirty_observation.expect("dirty ownership remains valid"),
        path
    );
    assert_ne!(advanced, base);
    assert_eq!(
        advanced_observation.expect("advanced owned HEAD remains valid"),
        path
    );
}

#[tokio::test]
async fn a_reparse_git_pointer_is_refused_when_the_host_permits_owned_symlinks() {
    let fixture = OwnedFixture::new().await;
    let path = fixture.worktree();
    let pointer = path.join(".git");
    let original = std::fs::read(&pointer).expect("owned Git pointer");
    let saved = fixture.state_dir.join("owned_git_pointer.txt");
    std::fs::write(&saved, &original).expect("owned pointer copy");
    std::fs::remove_file(&pointer).expect("remove only owned pointer for injection");
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(&saved, &pointer);
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&saved, &pointer);
    let observed = if linked.is_ok() {
        let observed = inspect_owner_workspace(&fixture.root, &fixture.owner).await;
        std::fs::remove_file(&pointer).expect("remove exact test-created symlink");
        Some(observed)
    } else {
        let error = linked.expect_err("link creation failed");
        assert!(
            error.kind() == std::io::ErrorKind::PermissionDenied
                || matches!(error.raw_os_error(), Some(5 | 1314))
        );
        eprintln!("owned_symlink_case_unavailable: host permission");
        None
    };
    std::fs::write(&pointer, original).expect("restore exact ordinary Git pointer");
    std::fs::remove_file(&saved).expect("remove exact owned pointer copy");
    fixture.finish().await;
    if let Some(observed) = observed {
        assert!(observed.is_err());
    }
}
