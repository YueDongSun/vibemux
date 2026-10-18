//! `vibemuxctl dsh` (ADR 032 §9): a thin Control v7 client for the daemon's
//! DSH session.
//!
//! A prompt is read from a file or stdin, never from argv, so it stays out of
//! process listings and shell history, and it is bounded before any byte
//! reaches the daemon. `dsh poll` is the only command that prints runtime
//! content, and it prints byte-exact records; every other command is
//! content-free.

use std::{ffi::OsString, path::PathBuf};

use serde_json::{Value, json};
use vibemuxd::{
    control::{
        ControlClient, ControlError, DshSessionOpenRequest, DshSessionPollQuery,
        DshSessionPromptRequest, DshSessionStatus,
    },
    process::DaemonPaths,
};

pub use crate::prompt_input::PromptSource;
use crate::{
    CliCommand, DaemonCliError, harness_client, map_control_error,
    prompt_input::{prompt_source, read_prompt},
};

/// Pages one `dsh poll` call reads at most before it stops advancing.
const MAX_POLL_PAGES: usize = 4096;
/// Keeps a prompt inside the control frame for ordinary text.
const MAX_PROMPT_BYTES: usize = vibemux_harness::dsh::MAX_PROMPT_BYTES;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DshCommand {
    /// Opens a session. `session_id` is optional; the daemon allocates one.
    Open {
        session_id: Option<String>,
    },
    Prompt {
        source: PromptSource,
    },
    /// Prints records after `--after-sequence`, following pages to the tail.
    Poll {
        after_sequence: u64,
    },
    Status,
    Cancel,
    Close,
}

/// Parses `vibemuxctl dsh <verb> [...]`, returning `None` when the arguments
/// are not a DSH invocation.
pub fn parse_dsh_arguments(arguments: &[OsString]) -> Result<Option<CliCommand>, DaemonCliError> {
    let Some(verb) = arguments.first().and_then(|value| value.to_str()) else {
        return Ok(None);
    };
    // `--project-root` may appear before the verb only for the subcommands
    // that take no positional argument; keep the grammar small and require
    // the verb first, as `dispatch` does.
    let mut project_root = None;
    let mut session_id = None;
    let mut prompt_file: Option<OsString> = None;
    let mut after_sequence = 0_u64;
    let mut index = 1;
    while index < arguments.len() {
        let argument = arguments[index]
            .to_str()
            .ok_or(DaemonCliError::InvalidArguments)?;
        match argument {
            "--project-root" => {
                index += 1;
                let value = arguments
                    .get(index)
                    .ok_or(DaemonCliError::InvalidArguments)?;
                project_root = Some(PathBuf::from(value));
            }
            "--session-id" => {
                index += 1;
                let value = arguments
                    .get(index)
                    .and_then(|value| value.to_str())
                    .ok_or(DaemonCliError::InvalidArguments)?;
                if !vibemux_harness::dsh::session_id_is_valid(value) {
                    return Err(DaemonCliError::InvalidArguments);
                }
                session_id = Some(value.to_string());
            }
            "--prompt-file" => {
                index += 1;
                let value = arguments
                    .get(index)
                    .ok_or(DaemonCliError::InvalidArguments)?;
                prompt_file = Some(value.clone());
            }
            "--after-sequence" => {
                index += 1;
                let value = arguments
                    .get(index)
                    .and_then(|value| value.to_str())
                    .ok_or(DaemonCliError::InvalidArguments)?;
                after_sequence = value
                    .parse::<u64>()
                    .map_err(|_| DaemonCliError::InvalidArguments)?;
            }
            _ => return Err(DaemonCliError::InvalidArguments),
        }
        index += 1;
    }

    let command = match verb {
        "open" => {
            if prompt_file.is_some() {
                return Err(DaemonCliError::InvalidArguments);
            }
            DshCommand::Open { session_id }
        }
        "prompt" => {
            if session_id.is_some() || after_sequence != 0 {
                return Err(DaemonCliError::InvalidArguments);
            }
            DshCommand::Prompt {
                source: prompt_source(prompt_file.as_ref())?,
            }
        }
        "poll" => {
            if session_id.is_some() || prompt_file.is_some() {
                return Err(DaemonCliError::InvalidArguments);
            }
            DshCommand::Poll { after_sequence }
        }
        "status" => {
            if session_id.is_some() || prompt_file.is_some() || after_sequence != 0 {
                return Err(DaemonCliError::InvalidArguments);
            }
            DshCommand::Status
        }
        "cancel" => {
            if session_id.is_some() || prompt_file.is_some() || after_sequence != 0 {
                return Err(DaemonCliError::InvalidArguments);
            }
            DshCommand::Cancel
        }
        "close" => {
            if session_id.is_some() || prompt_file.is_some() || after_sequence != 0 {
                return Err(DaemonCliError::InvalidArguments);
            }
            DshCommand::Close
        }
        _ => return Ok(None),
    };
    Ok(Some(CliCommand::Dsh {
        project_root,
        command,
    }))
}

/// Runs one DSH command against the daemon. A prompt is read and checked
/// first, as `dispatch submit` reads its own, so a missing, unreadable, or
/// invalid prompt is refused before the daemon is selected.
pub async fn run_dsh(paths: &DaemonPaths, command: DshCommand) -> Result<Value, DaemonCliError> {
    let prompt = match &command {
        DshCommand::Prompt { source } => Some(
            read_prompt(source.clone(), MAX_PROMPT_BYTES)
                .await
                .map_err(|error| error.into_cli_error(DaemonCliError::InvalidArguments))?,
        ),
        _ => None,
    };
    let client = harness_client(paths).await?;
    match command {
        DshCommand::Open { session_id } => {
            let status = client
                .dsh_session_open(&DshSessionOpenRequest { session_id })
                .await
                .map_err(map_control_error)?;
            Ok(status_json("opened", &status))
        }
        DshCommand::Prompt { .. } => {
            let prompt = prompt.ok_or(DaemonCliError::InvalidArguments)?;
            let byte_count = prompt.len();
            let status = client
                .dsh_session_prompt(&DshSessionPromptRequest { prompt })
                .await
                .map_err(map_control_error)?;
            let mut value = status_json("prompted", &status);
            value["prompt_bytes"] = json!(byte_count);
            Ok(value)
        }
        DshCommand::Poll { after_sequence } => poll_to_tail(&client, after_sequence).await,
        DshCommand::Status => {
            let status = client
                .dsh_session_status()
                .await
                .map_err(map_control_error)?;
            Ok(status_json("live", &status))
        }
        DshCommand::Cancel => {
            let status = client
                .dsh_session_cancel()
                .await
                .map_err(map_control_error)?;
            Ok(status_json("cancelled", &status))
        }
        DshCommand::Close => {
            let status = client
                .dsh_session_close()
                .await
                .map_err(map_control_error)?;
            Ok(status_json("closed", &status))
        }
    }
}

/// Follows poll pages to the tail so one command prints every available
/// record in order, then reports the cursor to resume from.
async fn poll_to_tail(client: &ControlClient, cursor: u64) -> Result<Value, DaemonCliError> {
    let mut cursor = cursor;
    let mut records: Vec<Value> = Vec::new();
    let mut truncated = false;
    for _ in 0..MAX_POLL_PAGES {
        let page = client
            .dsh_session_poll(DshSessionPollQuery { cursor })
            .await
            .map_err(map_control_error)?;
        truncated |= page.truncated;
        let progressed = page.next > cursor;
        for record in &page.records {
            records.push(json!({
                "sequence": record.sequence,
                "kind": record.kind,
                "raw_json": record.raw_json,
            }));
        }
        cursor = page.next;
        if !page.more || !progressed {
            return Ok(json!({
                "ok": true,
                "status": "live",
                "session": serde_json::to_value(&page.status)
                    .map_err(|_| DaemonCliError::InvalidArguments)?,
                "records": records,
                "next": cursor,
                "truncated": truncated,
            }));
        }
    }
    // An unusual amount of pending output: report what was read rather than
    // looping forever, and let the caller resume from the cursor.
    Err(DaemonCliError::Control {
        code: "dsh_poll_page_limit".to_string(),
    })
}

/// The content-free status projection every non-poll command prints.
fn status_json(state: &str, status: &DshSessionStatus) -> Value {
    json!({
        "ok": true,
        "status": state,
        "session_id": status.session_id,
        "phase": status.phase,
        "provider": status.provider,
        "model": status.model,
        "server_version": status.server_version,
        "turns_started": status.turns_started,
        "completed_turns": status.completed_turns,
        "failed_turns": status.failed_turns,
        "cancelled_turns": status.cancelled_turns,
        "unverified_turns": status.unverified_turns,
        "last_outcome": status.last_outcome,
        "records": status.records,
        "record_bytes": status.record_bytes,
        "foreign_notifications": status.foreign_notifications,
        "transport_open": status.transport_open,
        "prompt_bytes_total": status.prompt_bytes_total,
        "created_at": status.created_at
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        "profile": status.config.profile,
    })
}

/// Maps a control failure onto the CLI's error vocabulary. Kept next to the
/// command so `main` stays a thin dispatcher.
#[must_use]
pub fn dsh_error_code(error: &ControlError) -> String {
    error.code().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vibemux_harness::dsh::{DshPhase, DshTurnOutcome};

    fn parse(arguments: &[&str]) -> Result<Option<CliCommand>, DaemonCliError> {
        let owned: Vec<OsString> = arguments.iter().map(OsString::from).collect();
        parse_dsh_arguments(&owned)
    }

    #[test]
    fn the_grammar_accepts_each_verb_and_its_flags() {
        assert!(matches!(
            parse(&["open"]),
            Ok(Some(CliCommand::Dsh {
                command: DshCommand::Open { session_id: None },
                ..
            }))
        ));
        assert!(matches!(
            parse(&["open", "--session-id", "vibemux_1"]),
            Ok(Some(CliCommand::Dsh {
                command: DshCommand::Open {
                    session_id: Some(_)
                },
                ..
            }))
        ));
        assert!(matches!(
            parse(&["prompt", "--prompt-file", "-"]),
            Ok(Some(CliCommand::Dsh {
                command: DshCommand::Prompt {
                    source: PromptSource::Stdin
                },
                ..
            }))
        ));
        assert!(matches!(
            parse(&[
                "prompt",
                "--prompt-file",
                "p.txt",
                "--project-root",
                "C:\\x"
            ]),
            Ok(Some(CliCommand::Dsh {
                command: DshCommand::Prompt {
                    source: PromptSource::File(_)
                },
                project_root: Some(_),
            }))
        ));
        assert!(matches!(
            parse(&["poll", "--after-sequence", "7"]),
            Ok(Some(CliCommand::Dsh {
                command: DshCommand::Poll { after_sequence: 7 },
                ..
            }))
        ));
        assert!(matches!(
            parse(&["status"]),
            Ok(Some(CliCommand::Dsh {
                command: DshCommand::Status,
                ..
            }))
        ));
        assert!(matches!(
            parse(&["cancel"]),
            Ok(Some(CliCommand::Dsh {
                command: DshCommand::Cancel,
                ..
            }))
        ));
        assert!(matches!(
            parse(&["close"]),
            Ok(Some(CliCommand::Dsh {
                command: DshCommand::Close,
                ..
            }))
        ));
        // Not a DSH invocation: another verb keeps parsing.
        assert_eq!(parse(&["daemon"]), Ok(None));
        assert_eq!(parse(&[]), Ok(None));
    }

    #[test]
    fn the_grammar_refuses_missing_or_misfiled_flags() {
        for arguments in [
            vec!["open", "--prompt-file", "-"],
            vec!["prompt"],
            vec!["prompt", "--prompt-file"],
            vec!["prompt", "--prompt-file", "-", "--session-id", "vibemux_1"],
            vec!["poll", "--after-sequence", "not-a-number"],
            vec!["poll", "--session-id", "vibemux_1"],
            vec!["status", "--after-sequence", "1"],
            vec!["cancel", "--prompt-file", "-"],
            vec!["close", "--session-id", "vibemux_1"],
            vec!["open", "--session-id", "has space"],
            vec!["open", "--session-id", ""],
            vec!["open", "--unknown"],
            vec!["open", "--project-root"],
        ] {
            assert_eq!(
                parse(&arguments),
                Err(DaemonCliError::InvalidArguments),
                "{arguments:?}"
            );
        }
        // `poll` without a flag is valid: the cursor starts at the beginning.
        assert!(matches!(
            parse(&["poll"]),
            Ok(Some(CliCommand::Dsh {
                command: DshCommand::Poll { after_sequence: 0 },
                ..
            }))
        ));
    }

    #[test]
    fn the_status_projection_is_content_free_and_allowlisted() {
        let status = DshSessionStatus {
            session_id: "vibemux_1".to_string(),
            phase: DshPhase::Ready,
            provider: "deepseek-official".to_string(),
            model: "deepseek-v4-flash".to_string(),
            server_version: Some("0.0.1".to_string()),
            turns_started: 2,
            completed_turns: 2,
            failed_turns: 0,
            cancelled_turns: 0,
            unverified_turns: 0,
            last_outcome: Some(DshTurnOutcome::Completed),
            records: 9,
            record_bytes: 1024,
            foreign_notifications: 1,
            transport_open: true,
            prompt_bytes_total: 42,
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            config: vibemux_dsh::DshSessionConfigSummary {
                profile: "sdk".to_string(),
                provider: "deepseek-official".to_string(),
                model: "deepseek-v4-flash".to_string(),
                has_reasoning_effort: false,
                max_tokens: None,
                environment_name_count: 1,
                limits: vibemux_harness::dsh::DshSessionLimits::default(),
            },
        };
        let value = status_json("live", &status);
        let encoded = serde_json::to_string(&value).expect("encode");
        for hidden in ["executable", "entry_script", "bin.js", "node.exe", "path"] {
            assert!(!encoded.contains(hidden), "{hidden}");
        }
        assert_eq!(value["prompt_bytes_total"], 42);
        assert_eq!(value["phase"], "ready");
        assert_eq!(value["last_outcome"], "completed");
        assert_eq!(value["profile"], "sdk");
    }

    #[tokio::test]
    async fn the_prompt_is_checked_before_the_daemon_is_selected() {
        let temp = tempfile::tempdir().expect("temp project");
        let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
        let path = temp.path().join("prompt.txt");
        let run_with = |content: Vec<u8>| {
            std::fs::write(&path, content).expect("write prompt");
            run_dsh(
                &paths,
                DshCommand::Prompt {
                    source: PromptSource::File(path.clone()),
                },
            )
        };
        // No daemon runs for this project, so only a prompt that passes every
        // check reaches daemon selection and reports `NotRunning`.
        assert_eq!(
            run_with("p".repeat(MAX_PROMPT_BYTES).into_bytes()).await,
            Err(DaemonCliError::NotRunning)
        );
        for invalid in [
            b" \n".to_vec(),
            "p".repeat(MAX_PROMPT_BYTES + 1).into_bytes(),
            vec![0xff, 0xfe],
        ] {
            assert_eq!(
                run_with(invalid).await,
                Err(DaemonCliError::InvalidArguments)
            );
        }
        // A missing file and a directory are unreadable sources, not bad
        // arguments, which is what `dispatch submit` reports for them too.
        for unreadable in [temp.path().join("missing.txt"), temp.path().to_path_buf()] {
            let command = DshCommand::Prompt {
                source: PromptSource::File(unreadable),
            };
            assert_eq!(
                run_dsh(&paths, command).await,
                Err(DaemonCliError::PromptUnavailable)
            );
        }
    }

    #[test]
    fn remote_error_codes_keep_their_identity() {
        assert_eq!(
            dsh_error_code(&ControlError::Remote {
                code: "dsh_session_busy".to_string()
            }),
            "dsh_session_busy"
        );
    }
}
