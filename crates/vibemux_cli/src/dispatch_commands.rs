//! `vibemuxctl dispatch` (ADR 029 §7): a thin Control v5 client for the
//! daemon's harness request dispatch.
//!
//! A prompt is read from a file or stdin, never from argv, so it stays out
//! of process listings and shell history, and it is bounded before any
//! byte reaches the daemon. Output pages are reassembled into whole,
//! byte-exact vendor records.

use std::{ffi::OsString, io::Read, path::PathBuf};

use serde_json::{Value, json};
use uuid::Uuid;
use vibemux_harness::{
    AgentKind,
    dispatch::{
        DispatchRequest,
        request::{DISPATCH_REQUEST_SCHEMA_VERSION, MAX_PROMPT_BYTES},
    },
};
use vibemuxd::{
    control::{ControlClient, ControlError, HarnessDispatchOutputQuery},
    harness_dispatch::OutputReassembler,
    process::DaemonPaths,
};

use crate::{CliCommand, DaemonCliError, harness_client, map_control_error};

/// Pages one `dispatch output` call reads at most; a transcript at the
/// 16 MiB capture bound needs fewer than 1024.
const MAX_OUTPUT_PAGES: usize = 4096;
/// `--prompt-file` value that reads the prompt from stdin.
const STDIN_PROMPT_SOURCE: &str = "-";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PromptSource {
    Stdin,
    File(PathBuf),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DispatchCommand {
    Catalog,
    Probe {
        harness: AgentKind,
    },
    Submit {
        harness: AgentKind,
        prompt: PromptSource,
        /// Reusing an id makes a retry idempotent.
        request_id: Option<Uuid>,
    },
    Status {
        request_id: Uuid,
    },
    Output {
        request_id: Uuid,
        after_sequence: u64,
    },
    Cancel {
        request_id: Uuid,
    },
}

/// Parses the arguments after the `dispatch` verb.
pub(crate) fn parse_dispatch_arguments(
    arguments: &[OsString],
) -> Result<CliCommand, DaemonCliError> {
    let (action, rest) = arguments
        .split_first()
        .ok_or(DaemonCliError::InvalidArguments)?;
    let action = action.to_str().ok_or(DaemonCliError::InvalidArguments)?;
    let mut project_root = None;
    let mut prompt = None;
    let mut request_id = None;
    let mut after_sequence = None;
    let mut positional: Option<&str> = None;
    let mut index = 0;
    while index < rest.len() {
        let token = rest[index]
            .to_str()
            .ok_or(DaemonCliError::InvalidArguments)?;
        let value = rest.get(index + 1);
        match token {
            "--project-root" if project_root.is_none() => {
                project_root = Some(PathBuf::from(
                    value.ok_or(DaemonCliError::InvalidArguments)?,
                ));
                index += 2;
            }
            "--prompt-file" if action == "submit" && prompt.is_none() => {
                let value = value.ok_or(DaemonCliError::InvalidArguments)?;
                prompt = Some(if value.as_os_str() == STDIN_PROMPT_SOURCE {
                    PromptSource::Stdin
                } else {
                    PromptSource::File(PathBuf::from(value))
                });
                index += 2;
            }
            "--request-id" if action == "submit" && request_id.is_none() => {
                request_id = Some(parse_request_id(text_value(value)?)?);
                index += 2;
            }
            "--after-sequence" if action == "output" && after_sequence.is_none() => {
                after_sequence = Some(
                    text_value(value)?
                        .parse::<u64>()
                        .map_err(|_| DaemonCliError::InvalidArguments)?,
                );
                index += 2;
            }
            _ if !token.starts_with("--") && positional.is_none() => {
                positional = Some(token);
                index += 1;
            }
            _ => return Err(DaemonCliError::InvalidArguments),
        }
    }
    let target = || positional.ok_or(DaemonCliError::InvalidArguments);
    let command = match action {
        "catalog" if positional.is_none() => DispatchCommand::Catalog,
        "probe" => DispatchCommand::Probe {
            harness: parse_harness(target()?)?,
        },
        "submit" => DispatchCommand::Submit {
            harness: parse_harness(target()?)?,
            prompt: prompt.ok_or(DaemonCliError::InvalidArguments)?,
            request_id,
        },
        "status" => DispatchCommand::Status {
            request_id: parse_request_id(target()?)?,
        },
        "output" => DispatchCommand::Output {
            request_id: parse_request_id(target()?)?,
            after_sequence: after_sequence.unwrap_or(0),
        },
        "cancel" => DispatchCommand::Cancel {
            request_id: parse_request_id(target()?)?,
        },
        _ => return Err(DaemonCliError::InvalidArguments),
    };
    Ok(CliCommand::Dispatch {
        project_root,
        command,
    })
}

/// Runs one dispatch command against the running daemon and returns its
/// JSON output. A submitted prompt is read and validated first.
pub async fn run_dispatch(
    paths: &DaemonPaths,
    command: DispatchCommand,
) -> Result<Value, DaemonCliError> {
    let request = match &command {
        DispatchCommand::Submit {
            harness,
            prompt,
            request_id,
        } => Some(read_request(*harness, prompt.clone(), *request_id).await?),
        _ => None,
    };
    let client = harness_client(paths).await?;
    match command {
        DispatchCommand::Catalog => {
            let routes = client
                .harness_dispatch_catalog()
                .await
                .map_err(map_dispatch_error)?;
            Ok(json!({"ok": true, "routes": routes}))
        }
        DispatchCommand::Probe { harness } => {
            let probe = client
                .harness_dispatch_probe(harness)
                .await
                .map_err(map_dispatch_error)?;
            Ok(json!({"ok": true, "probe": probe}))
        }
        DispatchCommand::Submit { .. } => {
            let request = request.ok_or(DaemonCliError::InvalidArguments)?;
            let receipt = client
                .harness_dispatch_submit(&request)
                .await
                .map_err(map_dispatch_error)?;
            Ok(json!({"ok": true, "receipt": receipt}))
        }
        DispatchCommand::Status { request_id } => {
            let status = client
                .harness_dispatch_status(request_id)
                .await
                .map_err(map_dispatch_error)?;
            Ok(json!({"ok": true, "dispatch": status}))
        }
        DispatchCommand::Cancel { request_id } => {
            let status = client
                .harness_dispatch_cancel(request_id)
                .await
                .map_err(map_dispatch_error)?;
            Ok(json!({"ok": true, "dispatch": status}))
        }
        DispatchCommand::Output {
            request_id,
            after_sequence,
        } => read_output(&client, request_id, after_sequence).await,
    }
}

/// Reads pages until the transcript is complete or the reader has caught
/// up with a running attempt; `next_after_sequence` resumes the read.
async fn read_output(
    client: &ControlClient,
    request_id: Uuid,
    after_sequence: u64,
) -> Result<Value, DaemonCliError> {
    let mut reassembler = OutputReassembler::starting_after(after_sequence);
    let mut records = Vec::new();
    let mut complete = false;
    for _ in 0..MAX_OUTPUT_PAGES {
        let page = client
            .harness_dispatch_output(HarnessDispatchOutputQuery {
                request_id,
                cursor: reassembler.cursor(),
            })
            .await
            .map_err(map_dispatch_error)?;
        complete = page.complete;
        let caught_up = page.fragments.is_empty();
        records.extend(
            reassembler
                .accept(page)
                .map_err(|_| map_control_error(ControlError::InvalidFrame))?,
        );
        if complete || caught_up {
            break;
        }
    }
    let records: Vec<Value> = records
        .into_iter()
        .map(|record| {
            json!({"sequence": record.sequence, "kind": record.kind, "raw_json": record.raw_json})
        })
        .collect();
    Ok(json!({
        "ok": true,
        "request_id": request_id,
        "records": records,
        "next_after_sequence": reassembler.cursor().after_sequence,
        "complete": complete,
    }))
}

async fn read_request(
    harness: AgentKind,
    source: PromptSource,
    request_id: Option<Uuid>,
) -> Result<DispatchRequest, DaemonCliError> {
    let prompt = tokio::task::spawn_blocking(move || read_prompt(&source))
        .await
        .map_err(|_| DaemonCliError::PromptUnavailable)??;
    let request = DispatchRequest {
        schema_version: DISPATCH_REQUEST_SCHEMA_VERSION,
        request_id: request_id.unwrap_or_else(Uuid::new_v4),
        harness,
        prompt,
    };
    request
        .validate()
        .map_err(|_| DaemonCliError::InvalidPrompt)?;
    Ok(request)
}

/// Reads at most one byte beyond the prompt bound, so an oversized source
/// is refused without being read whole.
fn read_prompt(source: &PromptSource) -> Result<String, DaemonCliError> {
    let limit = MAX_PROMPT_BYTES as u64 + 1;
    let mut bytes = Vec::new();
    let read = match source {
        PromptSource::Stdin => std::io::stdin().lock().take(limit).read_to_end(&mut bytes),
        PromptSource::File(path) => {
            std::fs::File::open(path).and_then(|file| file.take(limit).read_to_end(&mut bytes))
        }
    };
    read.map_err(|_| DaemonCliError::PromptUnavailable)?;
    if bytes.len() > MAX_PROMPT_BYTES {
        return Err(DaemonCliError::InvalidPrompt);
    }
    String::from_utf8(bytes).map_err(|_| DaemonCliError::InvalidPrompt)
}

/// A daemon older than Control v5 lacks the operations; say so instead of
/// reporting stale runtime state.
fn map_dispatch_error(error: ControlError) -> DaemonCliError {
    match error {
        ControlError::UnsupportedVersion => DaemonCliError::Control {
            code: error.code().to_string(),
        },
        other => map_control_error(other),
    }
}

/// A harness by command name (`opencode`) or wire name (`open_code`).
fn parse_harness(text: &str) -> Result<AgentKind, DaemonCliError> {
    AgentKind::all()
        .into_iter()
        .find(|kind| kind.command_name() == text)
        .or_else(|| serde_json::from_value(Value::String(text.to_string())).ok())
        .ok_or_else(|| DaemonCliError::UnknownHarness(text.to_string()))
}

fn parse_request_id(text: &str) -> Result<Uuid, DaemonCliError> {
    Uuid::parse_str(text)
        .ok()
        .filter(|request_id| !request_id.is_nil())
        .ok_or(DaemonCliError::InvalidArguments)
}

fn text_value(value: Option<&OsString>) -> Result<&str, DaemonCliError> {
    value
        .and_then(|value| value.to_str())
        .ok_or(DaemonCliError::InvalidArguments)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> Result<CliCommand, DaemonCliError> {
        let arguments: Vec<OsString> = arguments.iter().map(OsString::from).collect();
        parse_dispatch_arguments(&arguments)
    }

    fn dispatch(arguments: &[&str]) -> DispatchCommand {
        match parse(arguments).expect("valid arguments") {
            CliCommand::Dispatch { command, .. } => command,
            other => panic!("unexpected command {other:?}"),
        }
    }

    #[test]
    fn every_action_parses_with_its_own_flags_only() {
        let request_id = Uuid::new_v4();
        let id = request_id.to_string();
        assert_eq!(dispatch(&["catalog"]), DispatchCommand::Catalog);
        assert_eq!(
            dispatch(&["probe", "opencode"]),
            DispatchCommand::Probe {
                harness: AgentKind::OpenCode
            }
        );
        assert_eq!(
            dispatch(&["probe", "open_code"]),
            DispatchCommand::Probe {
                harness: AgentKind::OpenCode
            }
        );
        assert_eq!(
            dispatch(&["submit", "codex", "--prompt-file", "-", "--request-id", &id]),
            DispatchCommand::Submit {
                harness: AgentKind::Codex,
                prompt: PromptSource::Stdin,
                request_id: Some(request_id),
            }
        );
        assert_eq!(
            dispatch(&["submit", "--prompt-file", "C:\\prompts\\a b.txt", "claude"]),
            DispatchCommand::Submit {
                harness: AgentKind::Claude,
                prompt: PromptSource::File(PathBuf::from("C:\\prompts\\a b.txt")),
                request_id: None,
            }
        );
        assert_eq!(
            dispatch(&["status", &id]),
            DispatchCommand::Status { request_id }
        );
        assert_eq!(
            dispatch(&["output", &id, "--after-sequence", "7"]),
            DispatchCommand::Output {
                request_id,
                after_sequence: 7
            }
        );
        assert_eq!(
            dispatch(&["cancel", &id]),
            DispatchCommand::Cancel { request_id }
        );
        match parse(&["catalog", "--project-root", "C:\\project"]).expect("root") {
            CliCommand::Dispatch { project_root, .. } => {
                assert_eq!(project_root, Some(PathBuf::from("C:\\project")));
            }
            other => panic!("unexpected command {other:?}"),
        }
    }

    #[test]
    fn malformed_arguments_are_refused() {
        let id = Uuid::new_v4().to_string();
        let nil = Uuid::nil().to_string();
        for arguments in [
            &[][..],
            &["unknown"][..],
            &["catalog", "codex"][..],
            &["probe"][..],
            &["submit", "codex"][..],
            &["submit", "codex", "--prompt-file"][..],
            &[
                "submit",
                "codex",
                "--prompt-file",
                "-",
                "--prompt-file",
                "-",
            ][..],
            &[
                "submit",
                "codex",
                "--prompt-file",
                "-",
                "--request-id",
                &nil,
            ][..],
            &["status", "not_a_uuid"][..],
            &["status", &id, "--prompt-file", "-"][..],
            &["output", &id, "--after-sequence", "-1"][..],
            &["cancel", &id, "extra"][..],
            &["catalog", "--prompt", "inline prompt"][..],
        ] {
            assert_eq!(
                parse(arguments),
                Err(DaemonCliError::InvalidArguments),
                "{arguments:?}"
            );
        }
        assert_eq!(
            parse(&["probe", "vim"]),
            Err(DaemonCliError::UnknownHarness("vim".to_string()))
        );
    }

    #[test]
    fn prompts_are_bounded_utf8_and_non_blank() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("prompt.txt");
        let read = |content: &[u8]| {
            std::fs::write(&path, content).expect("write prompt");
            read_prompt(&PromptSource::File(path.clone()))
        };
        assert_eq!(read("é prompt".as_bytes()), Ok("é prompt".to_string()));
        let largest = "p".repeat(MAX_PROMPT_BYTES);
        assert_eq!(read(largest.as_bytes()), Ok(largest));
        assert_eq!(
            read("p".repeat(MAX_PROMPT_BYTES + 1).as_bytes()),
            Err(DaemonCliError::InvalidPrompt)
        );
        assert_eq!(read(&[0xff, 0xfe]), Err(DaemonCliError::InvalidPrompt));
        assert_eq!(
            read_prompt(&PromptSource::File(temp.path().join("missing.txt"))),
            Err(DaemonCliError::PromptUnavailable)
        );
    }

    #[tokio::test]
    async fn a_blank_prompt_is_refused_before_contacting_the_daemon() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("blank.txt");
        std::fs::write(&path, " \n\t").expect("write prompt");
        assert_eq!(
            read_request(AgentKind::Codex, PromptSource::File(path), None)
                .await
                .map(|request| request.request_id),
            Err(DaemonCliError::InvalidPrompt)
        );
    }
}
