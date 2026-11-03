#![forbid(unsafe_code)]
//! Owned Git worktrees and immutable artifact writes for daemon Runs.
//!
//! Owner receipts live in Git's private per-worktree metadata directory, never
//! in tracked files. These checks are not a hostile same-user filesystem sandbox.
//! The daemon must await operations to completion before acting on cancellation.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    ffi::OsString,
    fmt,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::{Child, Command},
    time::timeout,
};
use vibemux_types::{
    ArtifactId, RunId,
    a2a::{ArtifactReference, RunWorkspace},
};

const MAX_GIT_OUTPUT: usize = 1024 * 1024;
const GIT_DEADLINE: Duration = Duration::from_secs(30);
const REAP_DEADLINE: Duration = Duration::from_secs(2);
const MAX_ARTIFACT_BYTES: usize = 32 * 1024;
const MAX_OWNER_RECEIPT_BYTES: usize = 16 * 1024;
const OWNER_RECEIPT_NAME: &str = "vibemux_owner.json";
const MAX_INVENTORY_RECORD_ATTRIBUTES: usize = 5;

#[derive(Clone)]
pub struct WorkspaceManager {
    git: PathBuf,
    repo: PathBuf,
    managed: PathBuf,
    common_git_dir: PathBuf,
    base_commit: String,
}

/// Inert cleanup observations. Callers retain OS identity pins, canonical
/// ownership, execution authority and their resource gate independently.
#[derive(Clone)]
pub struct CleanupObservation {
    pub repository_root: PathBuf,
    pub common_git_dir: PathBuf,
    pub head_commit: String,
}

impl fmt::Debug for CleanupObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CleanupObservation(<redacted>)")
    }
}

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("workspace path or ownership is invalid")]
    InvalidPath,
    #[error("workspace Git operation failed")]
    Git,
    #[error("workspace operation exceeded deadline")]
    Deadline,
    #[error("workspace process output exceeded its limit")]
    OutputLimit,
    #[error("workspace child termination could not be confirmed")]
    ProcessCleanup,
    #[error("workspace is dirty or mismatched")]
    UnsafeCleanup,
    #[error("workspace artifact write failed")]
    Artifact,
}

impl WorkspaceError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidPath => "workspace_invalid_path",
            Self::Git => "workspace_git_failed",
            Self::Deadline => "workspace_deadline",
            Self::OutputLimit => "workspace_output_limit",
            Self::ProcessCleanup => "workspace_process_cleanup_failed",
            Self::UnsafeCleanup => "workspace_unsafe_cleanup",
            Self::Artifact => "workspace_artifact_failed",
        }
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct OwnerReceipt {
    schema_version: u32,
    run_id: RunId,
    workspace: RunWorkspace,
    common_git_dir: String,
    worktree_git_dir: String,
}

struct OwnedPaths {
    path: PathBuf,
    git_dir: PathBuf,
    receipt: OwnerReceipt,
}

impl WorkspaceManager {
    pub async fn new(
        git: PathBuf,
        repo: PathBuf,
        base_commit: String,
    ) -> Result<Self, WorkspaceError> {
        Self::open(git, repo, base_commit, true).await
    }

    /// Opens existing managed metadata without creating a missing namespace.
    pub async fn open_existing(
        git: PathBuf,
        repo: PathBuf,
        base_commit: String,
    ) -> Result<Self, WorkspaceError> {
        Self::open(git, repo, base_commit, false).await
    }

    async fn open(
        git: PathBuf,
        repo: PathBuf,
        base_commit: String,
        create_managed: bool,
    ) -> Result<Self, WorkspaceError> {
        if !matches!(base_commit.len(), 40 | 64)
            || !base_commit.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(WorkspaceError::InvalidPath);
        }
        let (git, repo) = blocking(move || {
            if !git.is_absolute() || !git.is_file() {
                return Err(WorkspaceError::InvalidPath);
            }
            let git = std::fs::canonicalize(git).map_err(|_| WorkspaceError::InvalidPath)?;
            #[cfg(windows)]
            if !git
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("exe"))
            {
                return Err(WorkspaceError::InvalidPath);
            }
            let repo = std::fs::canonicalize(repo).map_err(|_| WorkspaceError::InvalidPath)?;
            require_plain_directory(&repo)?;
            Ok((git, repo))
        })
        .await?;
        let root = git_output(&git, &repo, &["rev-parse".into(), "--show-toplevel".into()]).await?;
        let common = git_output(
            &git,
            &repo,
            &["rev-parse".into(), "--git-common-dir".into()],
        )
        .await?;
        let actual = git_output(
            &git,
            &repo,
            &[
                "rev-parse".into(),
                "--verify".into(),
                format!("{base_commit}^{{commit}}").into(),
            ],
        )
        .await?;
        let base_commit = base_commit.to_ascii_lowercase();
        if actual.trim() != base_commit {
            return Err(WorkspaceError::Git);
        }
        let owned = repo.clone();
        let (managed, common_git_dir) = blocking(move || {
            let reported_root = canonical_output_path(&owned, &root)?;
            if reported_root != owned {
                return Err(WorkspaceError::InvalidPath);
            }
            let common_git_dir = canonical_output_path(&owned, &common)?;
            require_plain_directory(&common_git_dir)?;
            let state = owned.join(".vibemux");
            let managed = state.join("worktrees");
            if create_managed {
                ensure_plain_directory(&state)?;
                ensure_plain_directory(&managed)?;
            } else {
                require_plain_directory(&state)?;
                require_plain_directory(&managed)?;
            }
            let managed =
                std::fs::canonicalize(managed).map_err(|_| WorkspaceError::InvalidPath)?;
            if managed.parent().and_then(Path::parent) != Some(owned.as_path()) {
                return Err(WorkspaceError::InvalidPath);
            }
            Ok((managed, common_git_dir))
        })
        .await?;
        Ok(Self {
            git,
            repo,
            managed,
            common_git_dir,
            base_commit,
        })
    }

    pub fn base_commit(&self) -> &str {
        &self.base_commit
    }

    /// The deterministic record for a planned worktree creation: the managed
    /// `<run_id>` path, the `codex/run_<uuid>` branch, this manager's base
    /// commit, and a fresh ownership token. Planning has no Git or filesystem
    /// effect; the caller persists the plan, then `create_planned` executes
    /// exactly that record.
    pub fn plan(&self, run_id: RunId) -> Result<RunWorkspace, WorkspaceError> {
        Ok(RunWorkspace {
            path: path_text(&self.managed.join(run_id.to_string()))?,
            branch: format!("codex/run_{}", run_id.as_uuid().simple()),
            base_commit: self.base_commit.clone(),
            ownership_token: uuid::Uuid::new_v4().simple().to_string(),
        })
    }

    pub async fn create(&self, run_id: RunId) -> Result<RunWorkspace, WorkspaceError> {
        let record = self.plan(run_id)?;
        self.create_planned(run_id, &record).await
    }

    /// Executes a persisted plan. The record must bind exactly to this
    /// manager and `run_id` (path, branch, base commit) before any effect; the
    /// supplied ownership token is the one written into the receipt. Any
    /// existing destination — including one matching the plan — is refused.
    pub async fn create_planned(
        &self,
        run_id: RunId,
        record: &RunWorkspace,
    ) -> Result<RunWorkspace, WorkspaceError> {
        self.validate_plan_binding(run_id, record)?;
        let path = PathBuf::from(&record.path);
        let manager = self.clone();
        let prospective = path.clone();
        blocking(move || {
            manager.validate_ancestry()?;
            match std::fs::symlink_metadata(prospective) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                _ => Err(WorkspaceError::InvalidPath),
            }
        })
        .await?;
        self.git(
            &self.repo,
            &[
                "worktree".into(),
                "add".into(),
                "-b".into(),
                record.branch.clone().into(),
                git_path(&path)?,
                self.base_commit.clone().into(),
            ],
        )
        .await?;
        let paths = self.resolve_owned_paths(record).await?;
        let manager = self.clone();
        blocking(move || {
            manager.validate_owned_paths(&paths.path, &paths.git_dir)?;
            let bytes = bounded_json(&paths.receipt, MAX_OWNER_RECEIPT_BYTES)
                .map_err(|_| WorkspaceError::InvalidPath)?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(paths.git_dir.join(OWNER_RECEIPT_NAME))
                .map_err(|_| WorkspaceError::InvalidPath)?;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|_| WorkspaceError::InvalidPath)
        })
        .await?;
        self.inspect(record).await?;
        Ok(record.clone())
    }

    /// Exact binding of a planned record to this manager and `run_id`, before
    /// any effect. The ownership token is not manager-bound: it is persisted
    /// by the caller and used verbatim in the receipt.
    fn validate_plan_binding(
        &self,
        run_id: RunId,
        record: &RunWorkspace,
    ) -> Result<(), WorkspaceError> {
        record.validate().map_err(|_| WorkspaceError::InvalidPath)?;
        if record.path != path_text(&self.managed.join(run_id.to_string()))?
            || record.branch != format!("codex/run_{}", run_id.as_uuid().simple())
            || record.base_commit != self.base_commit
        {
            return Err(WorkspaceError::InvalidPath);
        }
        Ok(())
    }

    pub async fn inspect(&self, record: &RunWorkspace) -> Result<(), WorkspaceError> {
        let paths = self.resolve_owned_paths(record).await?;
        let manager = self.clone();
        blocking(move || manager.verify_receipt(&paths)).await
    }

    async fn resolve_owned_paths(
        &self,
        record: &RunWorkspace,
    ) -> Result<OwnedPaths, WorkspaceError> {
        record.validate().map_err(|_| WorkspaceError::InvalidPath)?;
        let path = PathBuf::from(&record.path);
        let manager = self.clone();
        let input = path.clone();
        let base_commit = record.base_commit.clone();
        let run_id = blocking(move || {
            manager.validate_ancestry()?;
            require_plain_directory(&input)?;
            if std::fs::canonicalize(&input).map_err(|_| WorkspaceError::InvalidPath)? != input
                || input.parent() != Some(manager.managed.as_path())
                || input == manager.repo
                || base_commit != manager.base_commit
            {
                return Err(WorkspaceError::InvalidPath);
            }
            input
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or(WorkspaceError::InvalidPath)?
                .parse::<RunId>()
                .map_err(|_| WorkspaceError::InvalidPath)
        })
        .await?;
        if record.branch != format!("codex/run_{}", run_id.as_uuid().simple()) {
            return Err(WorkspaceError::InvalidPath);
        }
        let branch = self
            .git(
                &path,
                &["symbolic-ref".into(), "--short".into(), "HEAD".into()],
            )
            .await?;
        if branch.trim() != record.branch {
            return Err(WorkspaceError::InvalidPath);
        }
        // Line-delimited porcelain (no -z): the -z flag needs git 2.37+,
        // but WSL2/Ubuntu 22.04 — the documented development environment —
        // ships git 2.34 as its LTS default and fails with
        // `error: unknown switch 'z'`. inventory_matches rejects any
        // path/branch containing '\n' fail-closed, and owned worktree paths
        // are generated (UUID-suffixed) and validated against the record, so
        // they never contain a newline.
        let inventory = self
            .git(
                &self.repo,
                &["worktree".into(), "list".into(), "--porcelain".into()],
            )
            .await?;
        if !inventory_matches(&inventory, &record.path, &record.branch) {
            return Err(WorkspaceError::InvalidPath);
        }
        let directory = self
            .git(&path, &["rev-parse".into(), "--absolute-git-dir".into()])
            .await?;
        let common = self
            .git(&path, &["rev-parse".into(), "--git-common-dir".into()])
            .await?;
        let manager = self.clone();
        let worktree = path.clone();
        let git_dir = blocking(move || {
            let git_dir = canonical_output_path(&worktree, &directory)?;
            let common = canonical_output_path(&worktree, &common)?;
            if common != manager.common_git_dir {
                return Err(WorkspaceError::InvalidPath);
            }
            manager.validate_owned_paths(&worktree, &git_dir)?;
            Ok(git_dir)
        })
        .await?;
        let receipt = OwnerReceipt {
            schema_version: 1,
            run_id,
            workspace: record.clone(),
            common_git_dir: path_text(&self.common_git_dir)?,
            worktree_git_dir: path_text(&git_dir)?,
        };
        Ok(OwnedPaths {
            path,
            git_dir,
            receipt,
        })
    }

    pub async fn write_artifact(
        &self,
        record: &RunWorkspace,
        name: &str,
        value: &impl Serialize,
    ) -> Result<ArtifactReference, WorkspaceError> {
        if !matches!(name, "result.json" | "review.json" | "verification.json") {
            return Err(WorkspaceError::Artifact);
        }
        let bytes =
            bounded_json(value, MAX_ARTIFACT_BYTES).map_err(|_| WorkspaceError::Artifact)?;
        let paths = self.resolve_owned_paths(record).await?;
        let reference = ArtifactReference {
            artifact_id: ArtifactId::new(),
            sha256: sha256(&bytes),
            size_bytes: u64::try_from(bytes.len()).map_err(|_| WorkspaceError::Artifact)?,
            media_type: "application/json".to_string(),
        };
        let manager = self.clone();
        let name = name.to_string();
        blocking(move || {
            manager.verify_receipt(&paths)?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(paths.path.join(name))
                .map_err(|_| WorkspaceError::Artifact)?;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|_| WorkspaceError::Artifact)
        })
        .await?;
        Ok(reference)
    }

    pub async fn plan_cleanup(&self, record: &RunWorkspace) -> Result<(), WorkspaceError> {
        self.inspect(record).await?;
        // Ignored artifacts are still user data and must also block deletion.
        let status = self
            .git(
                Path::new(&record.path),
                &[
                    "status".into(),
                    "--porcelain".into(),
                    "-z".into(),
                    "--untracked-files=all".into(),
                    "--ignored=matching".into(),
                ],
            )
            .await?;
        if !status.is_empty() {
            return Err(WorkspaceError::UnsafeCleanup);
        }
        Ok(())
    }

    /// Reads an exact owned, fully clean worktree without deleting or reserving
    /// it. A clean committed HEAD may differ from the original base commit.
    pub async fn observe_cleanup(
        &self,
        record: &RunWorkspace,
    ) -> Result<CleanupObservation, WorkspaceError> {
        self.plan_cleanup(record).await?;
        let head_commit = self.head_commit(record).await?;
        if !valid_commit_id(&head_commit) {
            return Err(WorkspaceError::Git);
        }
        Ok(CleanupObservation {
            repository_root: self.repo.clone(),
            common_git_dir: self.common_git_dir.clone(),
            head_commit,
        })
    }

    /// Constructor-validated canonical repository/common-directory binding.
    /// A cleanup caller compares and pins these even when the target is absent;
    /// `observe_removed` then rechecks Git's live binding against this manager.
    pub fn cleanup_repository_paths(&self) -> (&Path, &Path) {
        (&self.repo, &self.common_git_dir)
    }

    /// Observes postconditions without removal, pruning or branch mutation.
    /// The recorded branch must be unattached at every listed worktree path.
    /// A positive result is not executor quiescence or OS identity authority.
    pub async fn observe_removed(
        &self,
        record: &RunWorkspace,
        approved_head: &str,
    ) -> Result<bool, WorkspaceError> {
        if !valid_commit_id(approved_head) {
            return Err(WorkspaceError::InvalidPath);
        }
        let manager = self.clone();
        let binding = record.clone();
        if !blocking(move || manager.absent_cleanup_slot(&binding)).await? {
            return Ok(false);
        }

        let root = self
            .git(&self.repo, &["rev-parse".into(), "--show-toplevel".into()])
            .await?;
        let common = self
            .git(&self.repo, &["rev-parse".into(), "--git-common-dir".into()])
            .await?;
        let manager = self.clone();
        blocking(move || {
            manager.validate_ancestry()?;
            if canonical_output_path(&manager.repo, &root)? != manager.repo
                || canonical_output_path(&manager.repo, &common)? != manager.common_git_dir
            {
                return Err(WorkspaceError::InvalidPath);
            }
            Ok(())
        })
        .await?;

        // Keep Git 2.34-compatible line mode. Parsing validates the complete
        // bounded source before absence is considered, including foreign rows.
        let inventory = self
            .git(
                &self.repo,
                &["worktree".into(), "list".into(), "--porcelain".into()],
            )
            .await?;
        let entries = parse_inventory(&inventory)?;
        let target = path_key(&record.path);
        let target_branch = format!("refs/heads/{}", record.branch);
        if entries.iter().any(|entry| {
            entry.path == target
                || entry.branch.as_ref().is_some_and(|branch| {
                    if cfg!(windows) {
                        branch.eq_ignore_ascii_case(&target_branch)
                    } else {
                        branch == &target_branch
                    }
                })
        }) {
            return Ok(false);
        }
        let retained_head = self
            .git(
                &self.repo,
                &[
                    "rev-parse".into(),
                    "--verify".into(),
                    format!("refs/heads/{}^{{commit}}", record.branch).into(),
                ],
            )
            .await?;
        let retained_head = retained_head.trim();
        if !valid_commit_id(retained_head) {
            return Err(WorkspaceError::Git);
        }
        if !retained_head.eq_ignore_ascii_case(approved_head) {
            return Ok(false);
        }
        let manager = self.clone();
        let binding = record.clone();
        blocking(move || manager.absent_cleanup_slot(&binding)).await
    }

    fn absent_cleanup_slot(&self, record: &RunWorkspace) -> Result<bool, WorkspaceError> {
        self.validate_ancestry()?;
        let run_id = Path::new(&record.path)
            .file_name()
            .and_then(|value| value.to_str())
            .and_then(|value| value.parse::<RunId>().ok())
            .ok_or(WorkspaceError::InvalidPath)?;
        self.validate_plan_binding(run_id, record)?;
        match std::fs::symlink_metadata(&record.path) {
            Ok(_) => Ok(false),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
            Err(_) => Err(WorkspaceError::InvalidPath),
        }
    }

    /// The commit an owned worktree's `HEAD` points at. A candidate is
    /// collected relative to the base commit, so a moved `HEAD` (a worker
    /// commit) must be refused by the caller.
    pub async fn head_commit(&self, record: &RunWorkspace) -> Result<String, WorkspaceError> {
        self.inspect(record).await?;
        let head = self
            .git(
                Path::new(&record.path),
                &[
                    "rev-parse".into(),
                    "--verify".into(),
                    "HEAD^{commit}".into(),
                ],
            )
            .await?;
        Ok(head.trim().to_ascii_lowercase())
    }

    /// NUL-delimited porcelain v1 status of an owned worktree, untracked and
    /// ignored files included and renames split into delete plus add, so
    /// every changed path is reported (fail-closed candidate collection).
    pub async fn change_status(&self, record: &RunWorkspace) -> Result<String, WorkspaceError> {
        self.inspect(record).await?;
        self.git(
            Path::new(&record.path),
            &[
                "status".into(),
                "--porcelain=v1".into(),
                "-z".into(),
                "--untracked-files=all".into(),
                "--ignored=matching".into(),
                "--no-renames".into(),
            ],
        )
        .await
    }

    /// NUL-delimited raw diff of tracked paths against the base commit; it
    /// carries the file modes that the porcelain status does not.
    pub async fn tracked_raw_diff(&self, record: &RunWorkspace) -> Result<String, WorkspaceError> {
        self.inspect(record).await?;
        self.git(
            Path::new(&record.path),
            &[
                "diff".into(),
                "--raw".into(),
                "-z".into(),
                "--no-renames".into(),
                "--no-abbrev".into(),
                record.base_commit.clone().into(),
                "--".into(),
            ],
        )
        .await
    }

    /// Explicit execution only after fresh clean ownership checks. Branch retained.
    pub async fn cleanup(&self, record: &RunWorkspace) -> Result<(), WorkspaceError> {
        self.plan_cleanup(record).await?;
        self.inspect(record).await?;
        self.git(
            &self.repo,
            &[
                "worktree".into(),
                "remove".into(),
                "--".into(),
                git_path(Path::new(&record.path))?,
            ],
        )
        .await
        .map(|_| ())
    }

    fn validate_ancestry(&self) -> Result<(), WorkspaceError> {
        require_plain_directory(&self.repo)?;
        require_plain_directory(&self.repo.join(".vibemux"))?;
        require_plain_directory(&self.managed)?;
        require_plain_directory(&self.common_git_dir)?;
        if std::fs::canonicalize(&self.managed).map_err(|_| WorkspaceError::InvalidPath)?
            != self.managed
        {
            return Err(WorkspaceError::InvalidPath);
        }
        Ok(())
    }

    fn validate_owned_paths(&self, path: &Path, git_dir: &Path) -> Result<(), WorkspaceError> {
        self.validate_ancestry()?;
        require_plain_directory(path)?;
        let metadata_root = self.common_git_dir.join("worktrees");
        require_plain_directory(&metadata_root)?;
        require_plain_directory(git_dir)?;
        if path.parent() != Some(self.managed.as_path())
            || git_dir.parent() != Some(metadata_root.as_path())
            || std::fs::canonicalize(path).map_err(|_| WorkspaceError::InvalidPath)? != path
            || std::fs::canonicalize(git_dir).map_err(|_| WorkspaceError::InvalidPath)? != git_dir
        {
            return Err(WorkspaceError::InvalidPath);
        }
        let git_file = path.join(".git");
        let metadata = plain_metadata(&git_file)?;
        if !metadata.is_file() || metadata.len() > MAX_OWNER_RECEIPT_BYTES as u64 {
            return Err(WorkspaceError::InvalidPath);
        }
        let mut bytes = Vec::new();
        std::fs::File::open(git_file)
            .map_err(|_| WorkspaceError::InvalidPath)?
            .take(MAX_OWNER_RECEIPT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| WorkspaceError::InvalidPath)?;
        if bytes.len() > MAX_OWNER_RECEIPT_BYTES {
            return Err(WorkspaceError::InvalidPath);
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| WorkspaceError::InvalidPath)?;
        let pointer = text
            .trim_end_matches(['\r', '\n'])
            .strip_prefix("gitdir: ")
            .ok_or(WorkspaceError::InvalidPath)?;
        if canonical_output_path(path, pointer)? != git_dir {
            return Err(WorkspaceError::InvalidPath);
        }
        Ok(())
    }

    fn verify_receipt(&self, paths: &OwnedPaths) -> Result<(), WorkspaceError> {
        self.validate_owned_paths(&paths.path, &paths.git_dir)?;
        let receipt_path = paths.git_dir.join(OWNER_RECEIPT_NAME);
        let metadata = plain_metadata(&receipt_path)?;
        if !metadata.is_file() || metadata.len() > MAX_OWNER_RECEIPT_BYTES as u64 {
            return Err(WorkspaceError::InvalidPath);
        }
        let mut bytes = Vec::new();
        std::fs::File::open(receipt_path)
            .map_err(|_| WorkspaceError::InvalidPath)?
            .take(MAX_OWNER_RECEIPT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| WorkspaceError::InvalidPath)?;
        if bytes.len() > MAX_OWNER_RECEIPT_BYTES {
            return Err(WorkspaceError::InvalidPath);
        }
        let receipt: OwnerReceipt =
            serde_json::from_slice(&bytes).map_err(|_| WorkspaceError::InvalidPath)?;
        if receipt != paths.receipt {
            return Err(WorkspaceError::InvalidPath);
        }
        Ok(())
    }

    async fn git(&self, cwd: &Path, arguments: &[OsString]) -> Result<String, WorkspaceError> {
        git_output(&self.git, cwd, arguments).await
    }
}

async fn blocking<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, WorkspaceError> + Send + 'static,
) -> Result<T, WorkspaceError> {
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|_| WorkspaceError::InvalidPath)?
}

async fn git_output(
    git: &Path,
    cwd: &Path,
    arguments: &[OsString],
) -> Result<String, WorkspaceError> {
    let mut command = Command::new(git);
    command
        .arg("-c")
        .arg(if cfg!(windows) {
            "core.hooksPath=NUL"
        } else {
            "core.hooksPath=/dev/null"
        })
        .arg("-c")
        .arg("core.autocrlf=false")
        .arg("-c")
        .arg("core.fsmonitor=false")
        .args(arguments)
        .current_dir(cwd)
        .env_clear()
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        );
    for key in ["SystemRoot", "WINDIR", "TEMP", "TMP", "PATH"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    String::from_utf8(run_command(command, GIT_DEADLINE, MAX_GIT_OUTPUT).await?)
        .map_err(|_| WorkspaceError::Git)
}

async fn run_command(
    mut command: Command,
    deadline: Duration,
    maximum_output: usize,
) -> Result<Vec<u8>, WorkspaceError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command.spawn().map_err(|_| WorkspaceError::Git)?;
    let pipes = child.stdout.take().zip(child.stderr.take());
    let Some((stdout, stderr)) = pipes else {
        terminate_and_reap(&mut child).await?;
        return Err(WorkspaceError::Git);
    };
    let execution = async {
        let (output, _diagnostics, status) = tokio::try_join!(
            read_bounded(stdout, maximum_output),
            read_bounded(stderr, maximum_output),
            async { child.wait().await.map_err(|_| WorkspaceError::Git) }
        )?;
        if !status.success() {
            return Err(WorkspaceError::Git);
        }
        Ok(output)
    };
    let failure = match timeout(deadline, execution).await {
        Ok(Ok(output)) => return Ok(output),
        Ok(Err(error)) => error,
        Err(_) => WorkspaceError::Deadline,
    };
    terminate_and_reap(&mut child).await?;
    Err(failure)
}

async fn terminate_and_reap(child: &mut Child) -> Result<(), WorkspaceError> {
    let _ = child.start_kill();
    timeout(REAP_DEADLINE, child.wait())
        .await
        .map_err(|_| WorkspaceError::ProcessCleanup)?
        .map_err(|_| WorkspaceError::ProcessCleanup)?;
    Ok(())
}

async fn read_bounded(
    mut stream: impl AsyncRead + Unpin,
    maximum: usize,
) -> Result<Vec<u8>, WorkspaceError> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let count = stream
            .read(&mut buffer)
            .await
            .map_err(|_| WorkspaceError::Git)?;
        if count == 0 {
            return Ok(output);
        }
        if count > maximum.saturating_sub(output.len()) {
            return Err(WorkspaceError::OutputLimit);
        }
        output.extend_from_slice(&buffer[..count]);
    }
}

fn canonical_output_path(cwd: &Path, output: &str) -> Result<PathBuf, WorkspaceError> {
    let path = PathBuf::from(output.trim_end_matches(['\r', '\n']));
    let path = if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    };
    plain_metadata(&path)?;
    std::fs::canonicalize(path).map_err(|_| WorkspaceError::InvalidPath)
}

fn ensure_plain_directory(path: &Path) -> Result<(), WorkspaceError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => require_plain_directory(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match std::fs::create_dir(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err(WorkspaceError::InvalidPath),
            }
            require_plain_directory(path)
        }
        Err(_) => Err(WorkspaceError::InvalidPath),
    }
}

fn require_plain_directory(path: &Path) -> Result<(), WorkspaceError> {
    if !plain_metadata(path)?.is_dir() {
        return Err(WorkspaceError::InvalidPath);
    }
    Ok(())
}

fn plain_metadata(path: &Path) -> Result<std::fs::Metadata, WorkspaceError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| WorkspaceError::InvalidPath)?;
    if metadata.file_type().is_symlink() {
        return Err(WorkspaceError::InvalidPath);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(WorkspaceError::InvalidPath);
        }
    }
    Ok(metadata)
}

fn path_text(path: &Path) -> Result<String, WorkspaceError> {
    path.to_str()
        .map(str::to_string)
        .ok_or(WorkspaceError::InvalidPath)
}

fn git_path(path: &Path) -> Result<OsString, WorkspaceError> {
    let text = path.to_str().ok_or(WorkspaceError::InvalidPath)?;
    // Git for Windows rejects extended-length syntax in worktree path arguments.
    #[cfg(windows)]
    {
        if let Some(unc) = text.strip_prefix("\\\\?\\UNC\\") {
            return Ok(format!("\\\\{unc}").into());
        }
        Ok(text.strip_prefix("\\\\?\\").unwrap_or(text).into())
    }
    #[cfg(not(windows))]
    {
        Ok(text.into())
    }
}

fn path_key(path: &str) -> String {
    if cfg!(windows) {
        path.trim_start_matches("\\\\?\\")
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_lowercase()
    } else {
        path.trim_end_matches('/').to_string()
    }
}

fn inventory_matches(inventory: &str, path: &str, branch: &str) -> bool {
    // Line-delimited parsing requires reject-before-match: a path or branch
    // containing '\n' could otherwise forge field lines inside the inventory.
    // Owned worktree paths are generated (UUID-suffixed) and never contain a
    // newline, so the fail-closed arm is unreachable for legitimate records.
    if path.chars().any(char::is_control) || branch.chars().any(char::is_control) {
        return false;
    }
    let expected = path_key(path);
    let expected_branch = format!("refs/heads/{branch}");
    parse_inventory(inventory).is_ok_and(|entries| {
        entries
            .iter()
            .filter(|entry| {
                entry.path == expected
                    && entry.branch.as_deref() == Some(expected_branch.as_str())
                    && !entry.unsafe_entry
            })
            .count()
            == 1
    })
}

struct InventoryEntry {
    path: String,
    branch: Option<String>,
    unsafe_entry: bool,
}

fn valid_commit_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn parse_inventory(inventory: &str) -> Result<Vec<InventoryEntry>, WorkspaceError> {
    if inventory.len() > MAX_GIT_OUTPUT {
        return Err(WorkspaceError::OutputLimit);
    }
    let mut entries = Vec::new();
    let mut paths = BTreeSet::new();
    let mut branches = BTreeSet::new();
    let mut record = Vec::new();
    for line in inventory.lines().chain(std::iter::once("")) {
        if line.is_empty() {
            if record.is_empty() {
                continue;
            }
            let entry = parse_inventory_entry(&record)?;
            if !paths.insert(entry.path.clone()) {
                return Err(WorkspaceError::Git);
            }
            if let Some(branch) = &entry.branch {
                let key = if cfg!(windows) {
                    branch.to_ascii_lowercase()
                } else {
                    branch.clone()
                };
                if !branches.insert(key) {
                    return Err(WorkspaceError::Git);
                }
            }
            entries.push(entry);
            record.clear();
        } else {
            if record.len() == MAX_INVENTORY_RECORD_ATTRIBUTES || line.chars().any(char::is_control)
            {
                return Err(WorkspaceError::Git);
            }
            record.push(line);
        }
    }
    if entries.is_empty() {
        return Err(WorkspaceError::Git);
    }
    Ok(entries)
}

fn parse_inventory_entry(record: &[&str]) -> Result<InventoryEntry, WorkspaceError> {
    let path = record
        .first()
        .and_then(|line| line.strip_prefix("worktree "))
        .filter(|value| valid_inventory_path(value))
        .ok_or(WorkspaceError::Git)?;
    let mut head = None;
    let mut branch = None;
    let mut bare = false;
    let mut detached = false;
    let mut locked = false;
    let mut prunable = false;
    for line in &record[1..] {
        if let Some(value) = line.strip_prefix("HEAD ") {
            if head.replace(value).is_some() || !valid_commit_id(value) {
                return Err(WorkspaceError::Git);
            }
        } else if let Some(value) = line.strip_prefix("branch ") {
            if branch.replace(value).is_some()
                || !value
                    .strip_prefix("refs/heads/")
                    .is_some_and(|name| !name.is_empty() && !name.chars().any(char::is_whitespace))
            {
                return Err(WorkspaceError::Git);
            }
        } else if *line == "bare" && !bare {
            bare = true;
        } else if *line == "detached" && !detached {
            detached = true;
        } else if (*line == "locked"
            || line
                .strip_prefix("locked ")
                .is_some_and(|reason| !reason.is_empty()))
            && !locked
        {
            locked = true;
        } else if (*line == "prunable"
            || line
                .strip_prefix("prunable ")
                .is_some_and(|reason| !reason.is_empty()))
            && !prunable
        {
            prunable = true;
        } else {
            return Err(WorkspaceError::Git);
        }
    }
    if (bare && (head.is_some() || branch.is_some() || detached))
        || (!bare && (head.is_none() || (branch.is_some() == detached)))
    {
        return Err(WorkspaceError::Git);
    }
    Ok(InventoryEntry {
        path: path_key(path),
        branch: branch.map(str::to_owned),
        unsafe_entry: bare || detached || locked || prunable,
    })
}

fn valid_inventory_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    let absolute = value.starts_with('/')
        || value.starts_with("\\\\")
        || (bytes.first().is_some_and(u8::is_ascii_alphabetic)
            && bytes.get(1) == Some(&b':')
            && bytes
                .get(2)
                .is_some_and(|byte| matches!(byte, b'/' | b'\\')));
    absolute
        && !value.starts_with('"')
        && !value.chars().any(char::is_control)
        && !value
            .replace('\\', "/")
            .split('/')
            .any(|part| matches!(part, "." | ".."))
}

struct BoundedJson {
    bytes: Vec<u8>,
    maximum: usize,
}
impl Write for BoundedJson {
    fn write(&mut self, input: &[u8]) -> std::io::Result<usize> {
        if input.len() > self.maximum.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "JSON exceeds limit",
            ));
        }
        self.bytes.extend_from_slice(input);
        Ok(input.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn bounded_json(value: &impl Serialize, maximum: usize) -> Result<Vec<u8>, serde_json::Error> {
    let mut output = BoundedJson {
        bytes: Vec::new(),
        maximum,
    };
    serde_json::to_writer_pretty(&mut output, value)?;
    Ok(output.bytes)
}

pub fn sha256(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    Sha256::digest(bytes)
        .iter()
        .flat_map(|byte| {
            [
                HEX[usize::from(byte >> 4)] as char,
                HEX[usize::from(byte & 15)] as char,
            ]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "child entry point executed explicitly by the bounded process test"]
    fn process_fixture() {
        let Some(mode) = std::env::var_os("VIBEMUX_WORKSPACE_PROCESS_FIXTURE") else {
            return;
        };
        let marker = std::env::var_os("VIBEMUX_WORKSPACE_PROCESS_MARKER").expect("fixture marker");
        if mode == "flood" {
            std::fs::write(&marker, "started").expect("marker");
            let block = vec![b'x'; 64 * 1024];
            while std::io::stdout().write_all(&block).is_ok() {}
        } else {
            loop {
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&marker)
                    .expect("owned marker");
                file.write_all(b"x").expect("heartbeat");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    #[tokio::test]
    async fn oversized_output_and_timeout_terminate_and_reap_real_children() {
        for mode in ["flood", "hang"] {
            let directory = tempfile::tempdir().expect("directory");
            let marker = directory.path().join("child_progress.txt");
            let mut command = Command::new(std::env::current_exe().expect("test executable"));
            command
                .args([
                    "--exact",
                    "--ignored",
                    "tests::process_fixture",
                    "--nocapture",
                ])
                .env("VIBEMUX_WORKSPACE_PROCESS_FIXTURE", mode)
                .env("VIBEMUX_WORKSPACE_PROCESS_MARKER", &marker);
            let result = run_command(command, Duration::from_secs(2), 1024).await;
            assert!(matches!(
                (&result, mode),
                (Err(WorkspaceError::OutputLimit), "flood")
                    | (Err(WorkspaceError::Deadline), "hang")
            ));
            let finished = std::fs::read(&marker).expect("real child started");
            tokio::time::sleep(Duration::from_millis(80)).await;
            assert_eq!(
                std::fs::read(marker).expect("marker after owner returned"),
                finished,
                "child must stop before error returns"
            );
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_batch_launcher_is_rejected_before_execution() {
        let directory = tempfile::tempdir().expect("directory");
        let launcher = directory.path().join("git.cmd");
        std::fs::write(&launcher, "@exit /b 0\r\n").expect("batch fixture");
        assert!(matches!(
            WorkspaceManager::new(launcher, directory.path().to_path_buf(), "a".repeat(40)).await,
            Err(WorkspaceError::InvalidPath)
        ));
    }
    #[test]
    fn line_inventory_matches_exactly_once_and_rejects_duplicates_or_locked_entries() {
        let path = "/managed/run_one";
        let entry = format!(
            "worktree {path}\nHEAD {}\nbranch refs/heads/codex/run_one\n\n",
            "a".repeat(40)
        );
        assert!(inventory_matches(&entry, path, "codex/run_one"));
        assert!(!inventory_matches(
            &format!("{entry}{entry}"),
            path,
            "codex/run_one"
        ));
        assert!(!inventory_matches(
            &entry.replace("\n\n", "\nlocked fixture\n\n"),
            path,
            "codex/run_one"
        ));
        // Without a trailing blank line the final entry still terminates.
        assert!(inventory_matches(
            entry.trim_end_matches('\n'),
            path,
            "codex/run_one"
        ));
        // A detached or bare entry never matches.
        assert!(!inventory_matches(
            &entry.replace("branch refs/heads/codex/run_one\n", "detached\n"),
            path,
            "codex/run_one"
        ));
    }

    #[test]
    fn line_inventory_rejects_newline_paths_and_branches_fail_closed() {
        // A newline inside a record path or branch could forge field lines in
        // the line-delimited inventory; reject before matching instead.
        let path = "/managed/run_one";
        let entry = format!(
            "worktree {path}\nHEAD {}\nbranch refs/heads/codex/run_one\n\n",
            "a".repeat(40)
        );
        let forged = format!("worktree {path}\nworktree {path}\n{entry}");
        assert!(!inventory_matches(
            &forged,
            &format!("{path}\nworktree {path}"),
            "codex/run_one"
        ));
        assert!(!inventory_matches(&entry, path, "codex/run_one\n"));
    }

    #[test]
    fn strict_inventory_preserves_valid_git_line_records_and_final_record() {
        let head = "a".repeat(40);
        let inventory = format!(
            "worktree /repo\nHEAD {head}\nbranch refs/heads/main\n\nworktree /bare\nbare\n\nworktree /detached\nHEAD {head}\ndetached\nlocked owned reason\n\nworktree /missing\nHEAD {head}\nbranch refs/heads/old\nprunable gitdir missing"
        );
        let entries = parse_inventory(&inventory).expect("complete line inventory");
        assert_eq!(entries.len(), 4);
        assert!(!entries[0].unsafe_entry);
        assert!(entries[1].unsafe_entry);
        assert!(entries[2].unsafe_entry);
        assert!(entries[3].unsafe_entry);
        assert_eq!(
            parse_inventory(&inventory.replace('\n', "\r\n"))
                .expect("CRLF inventory")
                .len(),
            4
        );
    }

    #[test]
    fn strict_inventory_refuses_malformed_ambiguous_and_duplicate_source_records() {
        let head = "a".repeat(40);
        let entry = format!("worktree /repo\nHEAD {head}\nbranch refs/heads/main\n\n");
        let malformed = [
            String::new(),
            entry.replace("worktree /repo", "worktree \"/repo\""),
            entry.replace("worktree /repo", "worktree /repo\nworktree /second"),
            entry.replace(
                &format!("HEAD {head}"),
                &format!("HEAD {head}\nHEAD {head}"),
            ),
            entry.replace(
                "branch refs/heads/main",
                "branch refs/heads/main\nbranch refs/heads/other",
            ),
            entry.replace(&format!("HEAD {head}\n"), ""),
            entry.replace(&format!("HEAD {head}"), "HEAD invalid"),
            entry.replace("branch refs/heads/main\n", ""),
            entry.replace("branch refs/heads/main", "branch refs/heads/main\ndetached"),
            entry.replace(
                "branch refs/heads/main",
                "branch refs/heads/main\nunknown field",
            ),
            format!("{entry}{entry}"),
            format!(
                "{entry}{}",
                entry.replace("worktree /repo", "worktree /other")
            ),
            format!("HEAD {head}\n{entry}"),
            "x".repeat(MAX_GIT_OUTPUT + 1),
        ];
        for inventory in malformed {
            assert!(
                parse_inventory(&inventory).is_err(),
                "malformed inventory must not establish absence"
            );
        }
    }
}
