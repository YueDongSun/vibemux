#![forbid(unsafe_code)]
//! Starts and stops a project's daemon through `vibemuxctl`, the CLI that
//! owns daemon lifecycle. The GUI never spawns `vibemuxd` itself and never
//! acts without an explicit click.
//!
//! `vibemuxctl` runs with no console window, and its output goes to two
//! private temporary files rather than pipes: the daemon it starts may
//! inherit open handles and outlive it, and a pipe it holds open would
//! never reach end-of-file. A file is read after `vibemuxctl` exits.

use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// How long one start or stop may take, including the daemon's health wait.
pub const CONTROL_TIMEOUT: Duration = Duration::from_secs(60);
/// The most output read from either stream.
const MAX_OUTPUT_BYTES: u64 = 64 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DaemonCommand {
    Start,
    Stop,
}

impl DaemonCommand {
    #[must_use]
    pub const fn verb(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
        }
    }

    #[must_use]
    pub const fn progress_label(self) -> &'static str {
        match self {
            Self::Start => "Starting VibeMux…",
            Self::Stop => "Stopping VibeMux…",
        }
    }
}

/// What `vibemuxctl` reported.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DaemonControlResult {
    /// `started`, `already_running`, or `stopped`.
    Done {
        status: String,
    },
    Failed {
        code: String,
        message: String,
    },
}

impl DaemonControlResult {
    fn failed(code: &str, message: &str) -> Self {
        Self::Failed {
            code: code.to_string(),
            message: message.to_string(),
        }
    }
}

/// `vibemuxctl` next to the running GUI executable, where a workspace
/// build places both.
#[must_use]
pub fn vibemuxctl_path() -> Option<PathBuf> {
    let gui = std::env::current_exe().ok()?;
    let candidate = gui.with_file_name(format!("vibemuxctl{}", std::env::consts::EXE_SUFFIX));
    candidate.is_file().then_some(candidate)
}

/// The arguments for one command: JSON output, then the daemon verb for
/// `project_root`.
#[must_use]
pub fn command_arguments(command: DaemonCommand, project_root: &Path) -> Vec<OsString> {
    vec![
        "--json".into(),
        "daemon".into(),
        command.verb().into(),
        "--project-root".into(),
        project_root.as_os_str().to_owned(),
    ]
}

/// Reads `vibemuxctl`'s JSON: `{"ok":true,"status":...}` on stdout, or
/// `{"ok":false,"error_code":...,"error":...}` on stderr.
#[must_use]
pub fn parse_outcome(exit_success: bool, stdout: &[u8], stderr: &[u8]) -> DaemonControlResult {
    let parse = |bytes: &[u8]| serde_json::from_slice::<serde_json::Value>(bytes).ok();
    if exit_success
        && let Some(value) = parse(stdout)
        && value["ok"] == true
    {
        let status = value["status"].as_str().unwrap_or("done").to_string();
        return DaemonControlResult::Done { status };
    }
    for value in [parse(stderr), parse(stdout)].into_iter().flatten() {
        if let Some(code) = value["error_code"].as_str() {
            let message = value["error"]
                .as_str()
                .unwrap_or("vibemuxctl reported an error.");
            return DaemonControlResult::failed(code, message);
        }
    }
    DaemonControlResult::failed(
        "daemon_control_unreadable",
        "vibemuxctl did not report a readable result.",
    )
}

/// Runs one command for `project_root` and waits for it, bounded by
/// [`CONTROL_TIMEOUT`]. Blocking: call it off the UI thread.
#[must_use]
pub fn run(command: DaemonCommand, project_root: &Path) -> DaemonControlResult {
    let Some(executable) = vibemuxctl_path() else {
        return DaemonControlResult::failed(
            "vibemuxctl_unavailable",
            "vibemuxctl was not found next to the GUI. Build it with: cargo build -p vibemux_cli --bin vibemuxctl -p vibemuxd --bin vibemuxd",
        );
    };
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let base =
        std::env::temp_dir().join(format!("vibemux_gui_daemon_{}_{stamp}", std::process::id()));
    let stdout_path = base.with_extension("out");
    let stderr_path = base.with_extension("err");
    let result = run_with_files(
        &executable,
        command,
        project_root,
        &stdout_path,
        &stderr_path,
    );
    let _ = fs::remove_file(&stdout_path);
    let _ = fs::remove_file(&stderr_path);
    result
}

fn run_with_files(
    executable: &Path,
    command: DaemonCommand,
    project_root: &Path,
    stdout_path: &Path,
    stderr_path: &Path,
) -> DaemonControlResult {
    let create = |path: &Path| OpenOptions::new().write(true).create_new(true).open(path);
    let (Ok(stdout), Ok(stderr)) = (create(stdout_path), create(stderr_path)) else {
        return DaemonControlResult::failed(
            "daemon_control_output_unavailable",
            "Could not create temporary files for vibemuxctl output.",
        );
    };
    let mut process = Command::new(executable);
    process
        .args(command_arguments(command, project_root))
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        process.creation_flags(CREATE_NO_WINDOW);
    }
    let Ok(mut child) = process.spawn() else {
        return DaemonControlResult::failed(
            "vibemuxctl_spawn_failed",
            "vibemuxctl could not be started.",
        );
    };
    let deadline = Instant::now() + CONTROL_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL_INTERVAL),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return DaemonControlResult::failed(
                    "daemon_control_timeout",
                    "vibemuxctl did not finish in time.",
                );
            }
            Err(_) => {
                return DaemonControlResult::failed(
                    "daemon_control_wait_failed",
                    "Waiting for vibemuxctl failed.",
                );
            }
        }
    };
    parse_outcome(
        status.success(),
        &read_bounded(stdout_path),
        &read_bounded(stderr_path),
    )
}

fn read_bounded(path: &Path) -> Vec<u8> {
    let mut bytes = Vec::new();
    if let Ok(file) = File::open(path) {
        let _ = file.take(MAX_OUTPUT_BYTES).read_to_end(&mut bytes);
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_ask_for_json_and_name_the_project() {
        let root = PathBuf::from("project");
        let arguments = command_arguments(DaemonCommand::Start, &root);
        assert_eq!(
            arguments,
            ["--json", "daemon", "start", "--project-root", "project"]
                .map(OsString::from)
                .to_vec()
        );
        assert_eq!(command_arguments(DaemonCommand::Stop, &root)[2], "stop");
    }

    #[test]
    fn successful_output_reports_its_status() {
        let started = br#"{"ok":true,"status":"started","process_id":7}"#;
        assert_eq!(
            parse_outcome(true, started, b""),
            DaemonControlResult::Done {
                status: "started".into()
            }
        );
        let stopped = br#"{"ok":true,"status":"stopped","process_id":7}"#;
        assert_eq!(
            parse_outcome(true, stopped, b""),
            DaemonControlResult::Done {
                status: "stopped".into()
            }
        );
    }

    #[test]
    fn errors_keep_their_code_and_message() {
        let error = br#"{"ok":false,"error_code":"daemon_not_running","error":"The daemon is not running."}"#;
        assert_eq!(
            parse_outcome(false, b"", error),
            DaemonControlResult::Failed {
                code: "daemon_not_running".into(),
                message: "The daemon is not running.".into()
            }
        );
        // A non-zero exit never counts as success, even with ok output.
        let ok = br#"{"ok":true,"status":"started"}"#;
        assert!(matches!(
            parse_outcome(false, ok, b""),
            DaemonControlResult::Failed { .. }
        ));
    }

    #[test]
    fn unreadable_output_is_a_failure() {
        assert_eq!(
            parse_outcome(true, b"not json", b"also not json"),
            DaemonControlResult::Failed {
                code: "daemon_control_unreadable".into(),
                message: "vibemuxctl did not report a readable result.".into()
            }
        );
    }

    #[test]
    fn verbs_and_labels_match_the_command() {
        assert_eq!(DaemonCommand::Start.verb(), "start");
        assert_eq!(DaemonCommand::Stop.verb(), "stop");
        assert!(
            DaemonCommand::Start
                .progress_label()
                .starts_with("Starting")
        );
    }
}
