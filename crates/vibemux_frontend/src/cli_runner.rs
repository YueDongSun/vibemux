#![forbid(unsafe_code)]
//! Runs `vibemuxctl`, the CLI next to the GUI executable, for the commands
//! the GUI delegates to it (daemon start and stop, the workspace
//! inventory). The CLI stays the owner of those operations.
//!
//! `vibemuxctl` runs with no console window, and its output goes to two
//! private temporary files rather than pipes: a daemon it starts may
//! inherit open handles and outlive it, and a pipe it holds open would
//! never reach end-of-file. The files are read after `vibemuxctl` exits and
//! removed. Every call blocks: run it off the UI thread.

use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// The most output read from either stream.
const MAX_OUTPUT_BYTES: u64 = 64 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Keeps temporary file names unique across concurrent runs.
static RUN_COUNTER: AtomicU64 = AtomicU64::new(0);

/// What a finished `vibemuxctl` wrote, bounded per stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CliOutput {
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Why `vibemuxctl` produced no output to read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CliFailure {
    Unavailable,
    OutputUnavailable,
    SpawnFailed,
    Timeout,
    WaitFailed,
}

impl CliFailure {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Unavailable => "vibemuxctl_unavailable",
            Self::OutputUnavailable => "vibemuxctl_output_unavailable",
            Self::SpawnFailed => "vibemuxctl_spawn_failed",
            Self::Timeout => "vibemuxctl_timeout",
            Self::WaitFailed => "vibemuxctl_wait_failed",
        }
    }

    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Unavailable => {
                "vibemuxctl was not found next to the GUI. Build it with: cargo build -p vibemux_cli --bin vibemuxctl -p vibemuxd --bin vibemuxd"
            }
            Self::OutputUnavailable => "Could not create temporary files for vibemuxctl output.",
            Self::SpawnFailed => "vibemuxctl could not be started.",
            Self::Timeout => "vibemuxctl did not finish in time.",
            Self::WaitFailed => "Waiting for vibemuxctl failed.",
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

/// Runs `vibemuxctl` with `arguments` and waits for it, killing it after
/// `timeout`.
pub fn run(arguments: &[OsString], timeout: Duration) -> Result<CliOutput, CliFailure> {
    let executable = vibemuxctl_path().ok_or(CliFailure::Unavailable)?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let run_number = RUN_COUNTER.fetch_add(1, Ordering::Relaxed);
    let base = std::env::temp_dir().join(format!(
        "vibemux_gui_cli_{}_{stamp}_{run_number}",
        std::process::id()
    ));
    let stdout_path = base.with_extension("out");
    let stderr_path = base.with_extension("err");
    let result = run_with_files(&executable, arguments, timeout, &stdout_path, &stderr_path);
    let _ = fs::remove_file(&stdout_path);
    let _ = fs::remove_file(&stderr_path);
    result
}

fn run_with_files(
    executable: &Path,
    arguments: &[OsString],
    timeout: Duration,
    stdout_path: &Path,
    stderr_path: &Path,
) -> Result<CliOutput, CliFailure> {
    let create = |path: &Path| OpenOptions::new().write(true).create_new(true).open(path);
    let (Ok(stdout), Ok(stderr)) = (create(stdout_path), create(stderr_path)) else {
        return Err(CliFailure::OutputUnavailable);
    };
    let mut process = Command::new(executable);
    process
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        process.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = process.spawn().map_err(|_| CliFailure::SpawnFailed)?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL_INTERVAL),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(CliFailure::Timeout);
            }
            Err(_) => return Err(CliFailure::WaitFailed),
        }
    };
    Ok(CliOutput {
        success: status.success(),
        stdout: read_bounded(stdout_path),
        stderr: read_bounded(stderr_path),
    })
}

fn read_bounded(path: &Path) -> Vec<u8> {
    let mut bytes = Vec::new();
    if let Ok(file) = File::open(path) {
        let _ = file.take(MAX_OUTPUT_BYTES).read_to_end(&mut bytes);
    }
    bytes
}

/// The `error_code` (and `error` message, when present) `vibemuxctl`
/// reported on stderr or stdout.
#[must_use]
pub fn reported_error(stdout: &[u8], stderr: &[u8]) -> Option<(String, Option<String>)> {
    let parse = |bytes: &[u8]| serde_json::from_slice::<serde_json::Value>(bytes).ok();
    [parse(stderr), parse(stdout)]
        .into_iter()
        .flatten()
        .find_map(|value| {
            let code = value["error_code"].as_str()?.to_string();
            Some((code, value["error"].as_str().map(str::to_string)))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_have_stable_codes_and_a_message() {
        for failure in [
            CliFailure::Unavailable,
            CliFailure::OutputUnavailable,
            CliFailure::SpawnFailed,
            CliFailure::Timeout,
            CliFailure::WaitFailed,
        ] {
            assert!(failure.code().starts_with("vibemuxctl_"));
            assert!(!failure.message().is_empty());
        }
    }

    #[test]
    fn reported_errors_prefer_stderr_and_keep_the_message() {
        let stderr = br#"{"ok":false,"error_code":"daemon_not_running","error":"Not running."}"#;
        assert_eq!(
            reported_error(b"", stderr),
            Some(("daemon_not_running".into(), Some("Not running.".into())))
        );
        let stdout = br#"{"ok":false,"error_code":"workspace_git_query_failed"}"#;
        assert_eq!(
            reported_error(stdout, b"noise"),
            Some(("workspace_git_query_failed".into(), None))
        );
        assert_eq!(reported_error(b"{}", b"not json"), None);
    }
}
