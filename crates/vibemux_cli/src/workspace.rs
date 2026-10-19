//! Read-only Git inventory. No fetch, branch change, cleanup, or source content.
mod git_compatibility;

use crate::failure::Failure;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command, time::timeout};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_BRANCHES: usize = 4096;
const MAX_WORKTREES: usize = 256;
const MAX_FILTER_DRIVERS: usize = 32;
const MAX_FILTER_NAME_BYTES: usize = 128;
/// Global options every supported Git accepts: no pager, filesystem monitor,
/// untracked cache, or trace2 output.
const PORTABLE_OPTIONS: [&str; 11] = [
    "--no-pager",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.untrackedCache=false",
    "-c",
    "trace2.eventTarget=",
    "-c",
    "trace2.perfTarget=",
    "-c",
    "trace2.normalTarget=",
];
/// Repository options for Git 2.45 or newer.
const LOCAL_ONLY_OPTIONS: &[&str] = &["--no-optional-locks", "--no-lazy-fetch"];
/// Repository options for older Git, used only after
/// `git_compatibility::require_local_objects` found no promisor remote.
const LEGACY_OPTIONS: &[&str] = &["--no-optional-locks"];

#[derive(Debug, Deserialize, Serialize)]
pub struct Branch {
    pub name: String,
    pub commit: String,
    pub upstream: Option<String>,
    pub role: String,
    pub integrated: bool,
}
#[derive(Debug, Deserialize, Serialize)]
pub struct Worktree {
    pub branch: Option<String>,
    pub head: String,
    pub location: String,
    pub locked: bool,
    pub prunable: bool,
}
#[derive(Debug, Deserialize, Serialize)]
pub struct Report {
    pub schema_version: u16,
    pub source: String,
    pub current_branch: Option<String>,
    pub head: String,
    pub baseline: String,
    pub ahead: u64,
    pub behind: u64,
    pub staged: usize,
    pub unstaged: usize,
    pub untracked: usize,
    pub change_detection: String,
    pub branches: Vec<Branch>,
    pub worktrees: Vec<Worktree>,
}

pub async fn inspect(project_root: Option<PathBuf>) -> Result<Report, Failure> {
    let root = match project_root {
        Some(root) => root,
        None => std::env::current_dir()
            .map_err(|source| Failure::with_source("workspace_unavailable", source))?,
    };
    let (git, config_keys) = Git::open(&root).await?;
    let head = git
        .required(&["rev-parse", "--verify", "HEAD"])
        .await?
        .trim()
        .to_string();
    if !valid_commit(&head) {
        return Err(Failure::new("workspace_invalid_git_output"));
    }
    let current = git
        .query(&["symbolic-ref", "--quiet", "--short", "HEAD"])
        .await?;
    let current_branch = current.success.then(|| current.text.trim().to_string());
    let mut baseline = "HEAD";
    for candidate in ["refs/remotes/origin/main", "refs/heads/main"] {
        if git
            .query(&["rev-parse", "--verify", candidate])
            .await?
            .success
        {
            baseline = candidate;
            break;
        }
    }
    let comparison = format!("{baseline}...HEAD");
    let divergence = git
        .required(&["rev-list", "--left-right", "--count", &comparison])
        .await?;
    let counts = divergence
        .split_whitespace()
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| Failure::with_source("workspace_invalid_git_output", source))?;
    if counts.len() != 2 {
        return Err(Failure::new("workspace_invalid_git_output"));
    }
    let merged_option = format!("--merged={baseline}");
    let merged = git
        .required(&[
            "for-each-ref",
            &merged_option,
            "--format=%(refname:short)",
            "refs/heads",
        ])
        .await?;
    let integrated = merged.lines().collect::<BTreeSet<_>>();
    let refs = git
        .required(&[
            "for-each-ref",
            "--format=%(refname:short)%00%(objectname)%00%(upstream:short)",
            "refs/heads",
        ])
        .await?;
    let mut branches = Vec::new();
    for line in refs.lines() {
        if branches.len() >= MAX_BRANCHES {
            return Err(Failure::new("workspace_inventory_too_large"));
        }
        let fields = line.split('\0').collect::<Vec<_>>();
        if fields.len() != 3 || fields[0].len() > 512 || !valid_commit(fields[1]) {
            return Err(Failure::new("workspace_invalid_git_output"));
        }
        let name = fields[0];
        let merged = integrated.contains(name);
        let role = if current_branch.as_deref() == Some(name) {
            "current"
        } else if name == "main" {
            "main"
        } else if name.starts_with("python_") {
            "reference"
        } else if merged {
            "integrated"
        } else {
            "topic"
        };
        branches.push(Branch {
            name: name.to_string(),
            commit: fields[1].to_string(),
            upstream: (!fields[2].is_empty()).then(|| fields[2].to_string()),
            role: role.to_string(),
            integrated: merged,
        });
    }
    let common = git
        .required(&["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .await?;
    let common = PathBuf::from(common.trim_end_matches(['\r', '\n']));
    let primary = common
        .parent()
        .ok_or_else(|| Failure::new("workspace_invalid_git_output"))?;
    let inventory = git
        .required(&["worktree", "list", "--porcelain", "-z"])
        .await?;
    let worktrees = parse_worktrees(&inventory, primary)?;
    let mut status_arguments = disabled_filter_arguments(&config_keys)?;
    let change_detection = if status_arguments.is_empty() {
        "git_status"
    } else {
        "external_filters_disabled"
    };
    status_arguments.extend(
        [
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=normal",
            "--ignore-submodules=all",
        ]
        .map(str::to_string),
    );
    let status_arguments = status_arguments
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let status = git.required(&status_arguments).await?;
    let (staged, unstaged, untracked) = status_counts(&status)?;
    Ok(Report {
        schema_version: 1,
        source: "local_git_cached_refs".to_string(),
        current_branch,
        head,
        baseline: baseline
            .trim_start_matches("refs/remotes/")
            .trim_start_matches("refs/heads/")
            .to_string(),
        ahead: counts[1],
        behind: counts[0],
        staged,
        unstaged,
        untracked,
        change_detection: change_detection.to_string(),
        branches,
        worktrees,
    })
}

/// The `-c` overrides that disable every configured filter driver, from the
/// NUL-separated configuration key names.
fn disabled_filter_arguments(config_keys: &str) -> Result<Vec<String>, Failure> {
    let mut drivers = BTreeSet::new();
    for key in config_keys.split('\0') {
        let Some((driver, setting)) = key.rsplit_once('.') else {
            continue;
        };
        if !driver.starts_with("filter.")
            || !matches!(setting, "clean" | "process" | "smudge" | "required")
        {
            continue;
        }
        if driver.len() > MAX_FILTER_NAME_BYTES
            || !driver
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        {
            return Err(Failure::new("workspace_filter_name_unsupported"));
        }
        drivers.insert(driver);
        if drivers.len() > MAX_FILTER_DRIVERS {
            return Err(Failure::new("workspace_filter_limit_exceeded"));
        }
    }
    let mut arguments = Vec::new();
    for driver in drivers {
        for (setting, value) in [
            ("clean", ""),
            ("process", ""),
            ("smudge", ""),
            ("required", "false"),
        ] {
            arguments.push("-c".to_string());
            arguments.push(format!("{driver}.{setting}={value}"));
        }
    }
    Ok(arguments)
}

fn parse_worktrees(encoded: &str, primary: &Path) -> Result<Vec<Worktree>, Failure> {
    let mut worktrees = Vec::new();
    let mut current: Option<Worktree> = None;
    for field in encoded.split('\0') {
        if let Some(path) = field.strip_prefix("worktree ") {
            if let Some(previous) = current.take() {
                worktrees.push(previous);
            }
            if worktrees.len() >= MAX_WORKTREES {
                return Err(Failure::new("workspace_inventory_too_large"));
            }
            current = Some(Worktree {
                branch: None,
                head: String::new(),
                location: display_location(Path::new(path), primary),
                locked: false,
                prunable: false,
            });
        } else if let Some(current) = current.as_mut() {
            if let Some(head) = field.strip_prefix("HEAD ") {
                current.head = head.to_string();
            } else if let Some(branch) = field.strip_prefix("branch refs/heads/") {
                current.branch = Some(branch.to_string());
            } else if field == "locked" || field.starts_with("locked ") {
                current.locked = true;
            } else if field == "prunable" || field.starts_with("prunable ") {
                current.prunable = true;
            }
        }
    }
    if let Some(previous) = current {
        worktrees.push(previous);
    }
    if worktrees.is_empty()
        || worktrees
            .iter()
            .any(|worktree| !valid_commit(&worktree.head))
    {
        return Err(Failure::new("workspace_invalid_git_output"));
    }
    Ok(worktrees)
}

fn display_location(path: &Path, primary: &Path) -> String {
    // Component comparison prevents prefix traps. This is display metadata, never a cleanup authority.
    let path_parts = path.components().collect::<Vec<_>>();
    let primary_parts = primary.components().collect::<Vec<_>>();
    if path_parts.len() >= primary_parts.len()
        && path_parts.iter().zip(&primary_parts).all(|(part, root)| {
            if cfg!(windows) {
                part.as_os_str().eq_ignore_ascii_case(root.as_os_str())
            } else {
                part == root
            }
        })
    {
        let relative = path_parts
            .iter()
            .skip(primary_parts.len())
            .map(|part| part.as_os_str())
            .collect::<PathBuf>();
        if relative.as_os_str().is_empty() {
            "<primary>".to_string()
        } else {
            relative.to_string_lossy().replace('\\', "/")
        }
    } else {
        "<external_worktree>".to_string()
    }
}

fn status_counts(encoded: &str) -> Result<(usize, usize, usize), Failure> {
    let (mut staged, mut unstaged, mut untracked) = (0, 0, 0);
    let mut entries = encoded.split('\0').filter(|entry| !entry.is_empty());
    while let Some(entry) = entries.next() {
        let bytes = entry.as_bytes();
        if bytes.len() < 3 || bytes[2] != b' ' {
            return Err(Failure::new("workspace_invalid_git_output"));
        }
        if bytes.starts_with(b"??") {
            untracked += 1;
        } else {
            if bytes[0] != b' ' {
                staged += 1;
            }
            if bytes[1] != b' ' {
                unstaged += 1;
            }
            if matches!(bytes[0], b'R' | b'C') || matches!(bytes[1], b'R' | b'C') {
                entries
                    .next()
                    .ok_or_else(|| Failure::new("workspace_invalid_git_output"))?;
            }
        }
    }
    Ok((staged, unstaged, untracked))
}

fn valid_commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

struct Output {
    success: bool,
    text: String,
}

impl Output {
    fn required(self) -> Result<String, Failure> {
        if !self.success {
            return Err(Failure::new("workspace_git_query_failed"));
        }
        Ok(self.text)
    }
}

/// The inventory's queries in one repository, with the options the installed
/// Git supports. The release is probed once, before any repository query.
struct Git<'a> {
    root: &'a Path,
    options: &'static [&'static str],
}

impl<'a> Git<'a> {
    /// Probes the Git release, then lists configuration key names once for
    /// both the promisor check and the filter overrides. A configuration
    /// listing loads no objects, so it is local on every supported release.
    async fn open(root: &'a Path) -> Result<(Self, String), Failure> {
        let reported = run_git(root, &[], &["version"]).await?.required()?;
        let version = git_compatibility::supported_version(&reported)?;
        let git = Self {
            root,
            options: if version.has_no_lazy_fetch() {
                LOCAL_ONLY_OPTIONS
            } else {
                LEGACY_OPTIONS
            },
        };
        // Read key names only. Filter commands and remote settings can contain
        // private values and are never needed.
        let config_keys = git
            .required(&["config", "--null", "--name-only", "--list"])
            .await?;
        git_compatibility::require_local_objects(version, config_keys.split('\0'))?;
        Ok((git, config_keys))
    }

    async fn query(&self, arguments: &[&str]) -> Result<Output, Failure> {
        run_git(self.root, self.options, arguments).await
    }

    async fn required(&self, arguments: &[&str]) -> Result<String, Failure> {
        self.query(arguments).await?.required()
    }
}

async fn run_git(root: &Path, options: &[&str], arguments: &[&str]) -> Result<Output, Failure> {
    let mut command = Command::new("git");
    command
        .args(options)
        .args(PORTABLE_OPTIONS)
        .arg("-C")
        .arg(root)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0")
        .kill_on_drop(true);
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_NAMESPACE",
        "GIT_CONFIG",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
    ] {
        command.env_remove(name);
    }
    for (name, _) in std::env::vars_os() {
        if name
            .to_str()
            .is_some_and(|name| name.starts_with("GIT_TRACE"))
        {
            command.env_remove(name);
        }
    }
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command
        .spawn()
        .map_err(|source| Failure::with_source("workspace_git_unavailable", source))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| Failure::new("workspace_missing_pipe"))?;
    let operation = timeout(COMMAND_TIMEOUT, async {
        let mut bytes = Vec::new();
        (&mut stdout)
            .take(MAX_OUTPUT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|source| Failure::with_source("workspace_git_read_failed", source))?;
        if bytes.len() > MAX_OUTPUT_BYTES {
            return Err(Failure::new("workspace_inventory_too_large"));
        }
        let status = child
            .wait()
            .await
            .map_err(|source| Failure::with_source("workspace_git_wait_failed", source))?;
        let text = String::from_utf8(bytes)
            .map_err(|source| Failure::with_source("workspace_invalid_git_output", source))?;
        Ok(Output {
            success: status.success(),
            text,
        })
    })
    .await;
    match operation {
        Ok(Ok(output)) => Ok(output),
        other => {
            child
                .start_kill()
                .map_err(|source| Failure::with_source("workspace_git_cleanup_failed", source))?;
            timeout(COMMAND_TIMEOUT, child.wait())
                .await
                .map_err(|_| Failure::new("workspace_git_cleanup_timeout"))?
                .map_err(|source| Failure::with_source("workspace_git_cleanup_failed", source))?;
            match other {
                Ok(Err(error)) => Err(error),
                _ => Err(Failure::new("workspace_git_timeout")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_counts_renames_without_exposing_names() {
        assert_eq!(
            status_counts("R  current\0old\0 M edited\0?? private_file\0").unwrap(),
            (1, 1, 1)
        );
        assert!(status_counts("R  current\0").is_err());
    }

    #[test]
    fn filter_overrides_come_from_key_names_alone() {
        let arguments = disabled_filter_arguments(
            "core.bare\0filter.lfs.clean\0filter.lfs.required\0filter.other.smudge\0",
        )
        .unwrap();
        assert_eq!(arguments.len(), 16);
        assert_eq!(arguments[..2], ["-c", "filter.lfs.clean="]);
        assert_eq!(arguments[14..], ["-c", "filter.other.required=false"]);
        assert!(disabled_filter_arguments("core.bare\0").unwrap().is_empty());
        assert_eq!(
            disabled_filter_arguments("filter.bad name.clean\0")
                .unwrap_err()
                .code(),
            "workspace_filter_name_unsupported"
        );
    }

    #[test]
    fn worktree_locations_hide_external_paths_and_prefix_traps() {
        let base = Path::new("/workspace/repo");
        assert_eq!(display_location(base, base), "<primary>");
        assert_eq!(
            display_location(Path::new("/workspace/repo/.vibemux/topic"), base),
            ".vibemux/topic"
        );
        assert_eq!(
            display_location(Path::new("/workspace/repo_other/private"), base),
            "<external_worktree>"
        );
    }
}
