#![forbid(unsafe_code)]
//! Starts and stops a project's daemon through `vibemuxctl`, the CLI that
//! owns daemon lifecycle. The GUI never spawns `vibemuxd` itself and never
//! acts without an explicit click. [`crate::cli_runner`] runs the CLI with
//! no console window and its output in private temporary files.

use std::{ffi::OsString, path::Path, time::Duration};

use crate::cli_runner;

/// How long one start or stop may take, including the daemon's health wait.
pub const CONTROL_TIMEOUT: Duration = Duration::from_secs(60);

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
    if let Some((code, message)) = cli_runner::reported_error(stdout, stderr) {
        let message = message.unwrap_or_else(|| "vibemuxctl reported an error.".to_string());
        return DaemonControlResult::failed(&code, &message);
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
    match cli_runner::run(&command_arguments(command, project_root), CONTROL_TIMEOUT) {
        Ok(output) => parse_outcome(output.success, &output.stdout, &output.stderr),
        Err(failure) => DaemonControlResult::failed(failure.code(), failure.message()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

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
