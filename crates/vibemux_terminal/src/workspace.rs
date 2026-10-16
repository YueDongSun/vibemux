//! Explicit, isolated manual-session worktrees. No automatic cleanup or commit.
use crate::protocol::{Result, TerminalError};
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize)]
pub struct WorkspaceReceipt {
    pub schema_version: u32,
    pub session_id: String,
    pub project_root: PathBuf,
    pub worktree: PathBuf,
    pub branch: String,
    pub base_commit: String,
}

pub fn checked_directory(path: &Path) -> Result<PathBuf> {
    let canonical =
        fs::canonicalize(path).map_err(|_| TerminalError("terminal_directory_unavailable"))?;
    let metadata =
        fs::symlink_metadata(path).map_err(|_| TerminalError("terminal_directory_unavailable"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(TerminalError("terminal_path_unsafe"));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(TerminalError("terminal_path_unsafe"));
        }
    }
    Ok(canonical)
}

pub fn is_within(root: &Path, path: &Path) -> bool {
    let mut parts = path.components();
    root.components().all(|expected| {
        parts.next().is_some_and(|actual| {
            #[cfg(windows)]
            {
                expected.as_os_str().to_string_lossy().to_lowercase()
                    == actual.as_os_str().to_string_lossy().to_lowercase()
            }
            #[cfg(not(windows))]
            {
                expected == actual
            }
        })
    })
}

fn private_directory(parent: &Path, name: &str) -> Result<PathBuf> {
    let path = parent.join(name);
    if !path.exists() {
        fs::create_dir(&path).map_err(|_| TerminalError("terminal_workspace_create_failed"))?;
    }
    let actual = checked_directory(&path)?;
    if actual.parent() != Some(parent) {
        return Err(TerminalError("terminal_path_unsafe"));
    }
    Ok(actual)
}

fn git_argument_path(path: &Path) -> Result<std::ffi::OsString> {
    let text = path.to_str().ok_or(TerminalError("terminal_path_unsafe"))?;
    // Git for Windows rejects Rust's canonical extended-length argv syntax.
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

fn git(git_executable: &Path, root: &Path, arguments: &[&std::ffi::OsStr]) -> Result<Vec<u8>> {
    let mut command = Command::new(git_executable);
    command
        .args(["-c", "core.hooksPath=", "-c", "core.fsmonitor=false"])
        .args(arguments)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .spawn()
        .map_err(|_| TerminalError("terminal_git_failed"))?;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match child
            .try_wait()
            .map_err(|_| TerminalError("terminal_git_failed"))?
        {
            Some(status) => {
                if !status.success() {
                    return Err(TerminalError("terminal_git_failed"));
                }
                use std::io::Read;
                let mut output = Vec::new();
                if let Some(stdout) = child.stdout.take() {
                    stdout
                        .take(4097)
                        .read_to_end(&mut output)
                        .map_err(|_| TerminalError("terminal_git_failed"))?;
                }
                if output.len() > 4096 {
                    return Err(TerminalError("terminal_git_output_limit"));
                }
                return Ok(output);
            }
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(TerminalError("terminal_git_timeout"));
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

pub fn create(project_root: &Path, git_executable: &Path) -> Result<WorkspaceReceipt> {
    let requested_root = checked_directory(project_root)?;
    if !git_executable.is_absolute()
        || !git_executable.is_file()
        || git_executable.file_stem().and_then(|value| value.to_str()) != Some("git")
    {
        return Err(TerminalError("terminal_git_executable_invalid"));
    }
    let git_executable = fs::canonicalize(git_executable)
        .map_err(|_| TerminalError("terminal_git_executable_invalid"))?;
    let top_level = git(
        &git_executable,
        &requested_root,
        &["rev-parse".as_ref(), "--show-toplevel".as_ref()],
    )?;
    let top_level = std::str::from_utf8(&top_level)
        .map_err(|_| TerminalError("terminal_git_failed"))?
        .trim();
    let project_root = checked_directory(Path::new(top_level))?;
    if !is_within(&project_root, &requested_root) || is_within(&project_root, &git_executable) {
        return Err(TerminalError("terminal_git_executable_invalid"));
    }
    // Runtime receipts and nested worktrees must never appear as source changes.
    git(
        &git_executable,
        &project_root,
        &[
            "check-ignore".as_ref(),
            "--quiet".as_ref(),
            "--no-index".as_ref(),
            ".vibemux/native_terminal_sessions/ownership_probe".as_ref(),
        ],
    )
    .map_err(|_| TerminalError("terminal_runtime_directory_not_ignored"))?;
    let head = git(
        &git_executable,
        &project_root,
        &["rev-parse".as_ref(), "--verify".as_ref(), "HEAD".as_ref()],
    )?;
    let base_commit = std::str::from_utf8(&head)
        .map_err(|_| TerminalError("terminal_git_failed"))?
        .trim()
        .to_string();
    if !matches!(base_commit.len(), 40 | 64)
        || !base_commit
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(TerminalError("terminal_git_failed"));
    }
    let state = private_directory(&project_root, ".vibemux")?;
    let worktrees = private_directory(&state, "native_terminal_worktrees")?;
    let receipts = private_directory(&state, "native_terminal_sessions")?;
    let session_id = Uuid::new_v4().simple().to_string();
    let branch = format!("codex/claude_tui_{session_id}");
    let worktree = worktrees.join(format!("claude_{session_id}"));
    // Persist a recovery record before Git creates any resource. It remains on failure.
    let receipt = WorkspaceReceipt {
        schema_version: 1,
        session_id,
        project_root,
        worktree,
        branch,
        base_commit,
    };
    let record = serde_json::to_vec_pretty(&receipt)
        .map_err(|_| TerminalError("terminal_receipt_failed"))?;
    fs::write(
        receipts.join(format!("{}.json", receipt.session_id)),
        record,
    )
    .map_err(|_| TerminalError("terminal_receipt_failed"))?;
    let worktree_argument = git_argument_path(&receipt.worktree)?;
    git(
        &git_executable,
        &receipt.project_root,
        &[
            "worktree".as_ref(),
            "add".as_ref(),
            "--quiet".as_ref(),
            "-b".as_ref(),
            receipt.branch.as_ref(),
            &worktree_argument,
            receipt.base_commit.as_ref(),
        ],
    )?;
    let actual = checked_directory(&receipt.worktree)?;
    if actual != receipt.worktree {
        return Err(TerminalError("terminal_path_unsafe"));
    }
    Ok(receipt)
}
