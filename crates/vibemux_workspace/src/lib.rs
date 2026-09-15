#![forbid(unsafe_code)]
//! Owned Git worktrees and immutable artifact writes for daemon Runs.
//!
//! Owner receipts live in Git's private per-worktree metadata directory, never
//! in tracked files. These checks are not a hostile same-user filesystem sandbox.
//! The daemon must await operations to completion before acting on cancellation.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
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

#[derive(Clone)]
pub struct WorkspaceManager {
    git: PathBuf,
    repo: PathBuf,
    managed: PathBuf,
    common_git_dir: PathBuf,
    base_commit: String,
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
            ensure_plain_directory(&state)?;
            let managed = state.join("worktrees");
            ensure_plain_directory(&managed)?;
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

    pub async fn create(&self, run_id: RunId) -> Result<RunWorkspace, WorkspaceError> {
        let path = self.managed.join(run_id.to_string());
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
        let branch = format!("codex/run_{}", run_id.as_uuid().simple());
        self.git(
            &self.repo,
            &[
                "worktree".into(),
                "add".into(),
                "-b".into(),
                branch.clone().into(),
                git_path(&path)?,
                self.base_commit.clone().into(),
            ],
        )
        .await?;
        let record = RunWorkspace {
            path: path_text(&path)?,
            branch,
            base_commit: self.base_commit.clone(),
            ownership_token: uuid::Uuid::new_v4().simple().to_string(),
        };
        let paths = self.resolve_owned_paths(&record).await?;
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
        self.inspect(&record).await?;
        Ok(record)
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
    if path.contains('\n') || branch.contains('\n') {
        return false;
    }
    let expected = path_key(path);
    let expected_branch = format!("refs/heads/{branch}");
    let mut listed_path = None;
    let mut listed_branch = None;
    let mut matches = 0;
    let mut unsafe_entry = false;
    for line in inventory.split('\n').chain(std::iter::once("")) {
        if line.is_empty() {
            if listed_path.as_deref() == Some(expected.as_str())
                && listed_branch == Some(expected_branch.as_str())
                && !unsafe_entry
            {
                matches += 1;
            }
            listed_path = None;
            listed_branch = None;
            unsafe_entry = false;
        } else if let Some(value) = line.strip_prefix("worktree ") {
            listed_path = Some(path_key(value));
        } else if let Some(value) = line.strip_prefix("branch ") {
            listed_branch = Some(value);
        } else if line == "bare"
            || line == "detached"
            || line.starts_with("locked")
            || line.starts_with("prunable")
        {
            unsafe_entry = true;
        }
    }
    matches == 1
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
}
