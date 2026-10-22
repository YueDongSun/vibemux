#![forbid(unsafe_code)]
//! The active folder's Git inventory for the Console, read through
//! `vibemuxctl --json workspace status`, the CLI that owns it. The CLI reads
//! cached refs only: no fetch, merge, or cleanup. Every string passes through
//! [`safe_text`] before it reaches a renderer.

use std::{ffi::OsString, path::Path, time::Duration};

use serde::Deserialize;

use crate::{
    cli_runner,
    dashboard::{Branch, Workspace, safe_text},
};

/// How long one inventory read may take.
pub const WORKSPACE_TIMEOUT: Duration = Duration::from_secs(20);
/// Characters of a commit id shown in a row.
const SHORT_COMMIT_CHARS: usize = 8;

/// The JSON report `vibemuxctl workspace status` prints (schema 1).
#[derive(Deserialize)]
struct Report {
    current_branch: Option<String>,
    head: String,
    baseline: String,
    ahead: u64,
    behind: u64,
    staged: usize,
    unstaged: usize,
    untracked: usize,
    change_detection: String,
    #[serde(default)]
    branches: Vec<ReportBranch>,
    #[serde(default)]
    worktrees: Vec<ReportWorktree>,
}

#[derive(Deserialize)]
struct ReportBranch {
    name: String,
    role: String,
    commit: String,
    upstream: Option<String>,
}

#[derive(Deserialize)]
struct ReportWorktree {
    branch: Option<String>,
    location: String,
}

#[must_use]
pub fn arguments(project_root: &Path) -> Vec<OsString> {
    vec![
        "--json".into(),
        "workspace".into(),
        "status".into(),
        "--project-root".into(),
        project_root.as_os_str().to_owned(),
    ]
}

/// The inventory from `vibemuxctl`'s output, or the stable code that says
/// why there is none.
pub fn parse(success: bool, stdout: &[u8], stderr: &[u8]) -> Result<Workspace, String> {
    if success && let Ok(report) = serde_json::from_slice::<Report>(stdout) {
        return Ok(project(report));
    }
    Err(cli_runner::reported_error(stdout, stderr).map_or_else(
        || "workspace_unreadable".to_string(),
        |(code, _)| safe_text(&code),
    ))
}

/// Reads the inventory for `project_root`. Blocking: call it off the UI
/// thread.
pub fn load(project_root: &Path) -> Result<Workspace, String> {
    let output = cli_runner::run(&arguments(project_root), WORKSPACE_TIMEOUT)
        .map_err(|failure| failure.code().to_string())?;
    parse(output.success, &output.stdout, &output.stderr)
}

fn short(commit: &str) -> String {
    safe_text(commit).chars().take(SHORT_COMMIT_CHARS).collect()
}

fn project(report: Report) -> Workspace {
    Workspace {
        current_branch: report
            .current_branch
            .as_deref()
            .map_or_else(|| "detached".to_string(), safe_text),
        head: short(&report.head),
        baseline: safe_text(&report.baseline),
        ahead: report.ahead,
        behind: report.behind,
        staged: report.staged,
        unstaged: report.unstaged,
        untracked: report.untracked,
        change_detection: safe_text(&report.change_detection),
        branches: report
            .branches
            .iter()
            .map(|branch| Branch {
                name: safe_text(&branch.name),
                role: safe_text(&branch.role),
                commit: short(&branch.commit),
                upstream: branch
                    .upstream
                    .as_deref()
                    .map_or_else(|| "-".to_string(), safe_text),
            })
            .collect(),
        worktrees: report
            .worktrees
            .iter()
            .map(|worktree| {
                format!(
                    "{} -> {}",
                    worktree
                        .branch
                        .as_deref()
                        .map_or_else(|| "detached".to_string(), safe_text),
                    safe_text(&worktree.location)
                )
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    const REPORT: &str = r#"{"schema_version":1,"source":"local_git_cached_refs",
        "current_branch":"feat/console","head":"b68f381f5221219fe374f50e677da8bb718ab391",
        "baseline":"origin/main","ahead":2,"behind":1,"staged":0,"unstaged":3,"untracked":1,
        "change_detection":"external_filters_disabled",
        "branches":[{"name":"main","role":"baseline","commit":"08c33230caab60c29c0f27656b250a1c43a7ede2",
            "upstream":"origin/main","integrated":true},
            {"name":"feat/console","role":"current","commit":"b68f381f5221219fe374f50e677da8bb718ab391",
            "upstream":null,"integrated":false}],
        "worktrees":[{"branch":"feat/console","head":"b68f381f","location":"I:/VibeMux",
            "locked":false,"prunable":false},
            {"branch":null,"head":"08c33230","location":"I:/elsewhere","locked":false,"prunable":true}]}"#;

    #[test]
    fn arguments_ask_for_json_workspace_status() {
        assert_eq!(
            arguments(&PathBuf::from("project")),
            ["--json", "workspace", "status", "--project-root", "project"]
                .map(OsString::from)
                .to_vec()
        );
    }

    #[test]
    fn a_report_becomes_the_console_inventory() {
        let workspace = parse(true, REPORT.as_bytes(), b"").expect("inventory");
        assert_eq!(workspace.current_branch, "feat/console");
        assert_eq!(workspace.head, "b68f381f");
        assert_eq!((workspace.ahead, workspace.behind), (2, 1));
        assert_eq!(
            (workspace.staged, workspace.unstaged, workspace.untracked),
            (0, 3, 1)
        );
        assert_eq!(workspace.branches.len(), 2);
        assert_eq!(workspace.branches[0].commit, "08c33230");
        assert_eq!(workspace.branches[1].upstream, "-");
        assert_eq!(
            workspace.worktrees,
            ["feat/console -> I:/VibeMux", "detached -> I:/elsewhere"]
        );
    }

    #[test]
    fn errors_keep_the_cli_code() {
        let error = br#"{"ok":false,"error_code":"workspace_git_query_failed"}"#;
        assert_eq!(
            parse(false, b"", error).err(),
            Some("workspace_git_query_failed".to_string())
        );
        assert_eq!(
            parse(true, b"not json", b"").err(),
            Some("workspace_unreadable".to_string())
        );
        // A report with a failing exit is still a failure.
        assert_eq!(
            parse(false, REPORT.as_bytes(), b"").err(),
            Some("workspace_unreadable".to_string())
        );
    }
}
