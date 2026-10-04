//! `vibemuxctl workflow|slots|context|session|prompt` (ADR 031 §8): a thin
//! Control v6 client for the daemon's dual-track workflow service.
//!
//! Request and policy documents are read from files, bounded before any
//! byte reaches the daemon. Every answer is printed as the daemon's JSON;
//! only `session inspect --prompts` prints prompt text, and only the
//! export writes files, into a directory it creates and owns.

use std::{
    ffi::OsString,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use uuid::Uuid;
use vibemuxd::{
    control::{ControlClient, ControlError},
    process::DaemonPaths,
    workflow::request::{MAX_POLICY_BYTES, MAX_WORKFLOW_REQUEST_BYTES},
};

use crate::{CliCommand, DaemonCliError, harness_client, map_control_error};

/// Pages one export reads at most.
const MAX_EXPORT_PAGES: usize = 4096;
/// Prompts one `session inspect --prompts` reads at most.
const MAX_SESSION_PROMPTS: usize = 1024;
/// File the export writes inside its output directory.
pub const EXPORT_FILE_NAME: &str = "workflow_export.json";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkflowCommand {
    Prepare {
        request_file: PathBuf,
        policy_file: PathBuf,
    },
    Start {
        contract: String,
        request_id: Uuid,
    },
    Status {
        workflow_id: Uuid,
    },
    Pause {
        workflow_id: Uuid,
    },
    Cancel {
        workflow_id: Uuid,
    },
    Export {
        workflow_id: Uuid,
        out: PathBuf,
    },
    Purge {
        workflow_id: Uuid,
    },
    SlotsList,
    ContextShare {
        bundle_id: Uuid,
        to_session: Uuid,
    },
    SessionInspect {
        session_id: Uuid,
        prompts: bool,
    },
    SessionAttach {
        session_id: Uuid,
    },
    PromptEvaluate {
        candidate: String,
        suite: String,
    },
    PromptVersions,
    PromptRollback,
}

/// Flags one invocation may carry.
#[derive(Default)]
struct Flags {
    project_root: Option<PathBuf>,
    request_file: Option<PathBuf>,
    policy_file: Option<PathBuf>,
    contract: Option<String>,
    request_id: Option<Uuid>,
    out: Option<PathBuf>,
    bundle: Option<Uuid>,
    to: Option<Uuid>,
    candidate: Option<String>,
    suite: Option<String>,
    prompts: bool,
}

/// Parses the arguments after one of the workflow verbs.
pub(crate) fn parse_workflow_arguments(
    verb: &str,
    arguments: &[OsString],
) -> Result<CliCommand, DaemonCliError> {
    let (action, rest) = arguments
        .split_first()
        .ok_or(DaemonCliError::InvalidArguments)?;
    let action = action.to_str().ok_or(DaemonCliError::InvalidArguments)?;
    let mut flags = Flags::default();
    let mut positional: Option<&str> = None;
    let mut index = 0;
    while index < rest.len() {
        let token = rest[index]
            .to_str()
            .ok_or(DaemonCliError::InvalidArguments)?;
        let value = rest.get(index + 1);
        let path = || {
            value
                .map(PathBuf::from)
                .ok_or(DaemonCliError::InvalidArguments)
        };
        let step = match (verb, action, token) {
            (_, _, "--project-root") if flags.project_root.is_none() => {
                flags.project_root = Some(path()?);
                2
            }
            ("workflow", "prepare", "--request-file") if flags.request_file.is_none() => {
                flags.request_file = Some(path()?);
                2
            }
            ("workflow", "prepare", "--policy-file") if flags.policy_file.is_none() => {
                flags.policy_file = Some(path()?);
                2
            }
            ("workflow", "start", "--contract") if flags.contract.is_none() => {
                flags.contract = Some(text_value(value)?.to_string());
                2
            }
            ("workflow", "start", "--request-id") if flags.request_id.is_none() => {
                flags.request_id = Some(parse_id(text_value(value)?)?);
                2
            }
            ("workflow", "export", "--out") if flags.out.is_none() => {
                flags.out = Some(path()?);
                2
            }
            ("context", "share", "--bundle") if flags.bundle.is_none() => {
                flags.bundle = Some(parse_id(text_value(value)?)?);
                2
            }
            ("context", "share", "--to") if flags.to.is_none() => {
                flags.to = Some(parse_id(text_value(value)?)?);
                2
            }
            ("session", "inspect", "--prompts") if !flags.prompts => {
                flags.prompts = true;
                1
            }
            ("prompt", "evaluate", "--candidate") if flags.candidate.is_none() => {
                flags.candidate = Some(text_value(value)?.to_string());
                2
            }
            ("prompt", "evaluate", "--suite") if flags.suite.is_none() => {
                flags.suite = Some(text_value(value)?.to_string());
                2
            }
            _ if !token.starts_with("--") && positional.is_none() => {
                positional = Some(token);
                1
            }
            _ => return Err(DaemonCliError::InvalidArguments),
        };
        index += step;
    }
    let id = || {
        positional
            .ok_or(DaemonCliError::InvalidArguments)
            .and_then(parse_id)
    };
    let none = |present: bool| {
        if present {
            Err(DaemonCliError::InvalidArguments)
        } else {
            Ok(())
        }
    };
    let command = match (verb, action) {
        ("workflow", "prepare") => {
            none(positional.is_some())?;
            WorkflowCommand::Prepare {
                request_file: flags.request_file.ok_or(DaemonCliError::InvalidArguments)?,
                policy_file: flags.policy_file.ok_or(DaemonCliError::InvalidArguments)?,
            }
        }
        ("workflow", "start") => {
            none(positional.is_some())?;
            WorkflowCommand::Start {
                contract: flags.contract.ok_or(DaemonCliError::InvalidArguments)?,
                request_id: flags.request_id.ok_or(DaemonCliError::InvalidArguments)?,
            }
        }
        ("workflow", "status") => WorkflowCommand::Status { workflow_id: id()? },
        ("workflow", "pause") => WorkflowCommand::Pause { workflow_id: id()? },
        ("workflow", "cancel") => WorkflowCommand::Cancel { workflow_id: id()? },
        ("workflow", "purge") => WorkflowCommand::Purge { workflow_id: id()? },
        ("workflow", "export") => WorkflowCommand::Export {
            workflow_id: id()?,
            out: flags.out.ok_or(DaemonCliError::InvalidArguments)?,
        },
        ("slots", "list") => {
            none(positional.is_some())?;
            WorkflowCommand::SlotsList
        }
        ("context", "share") => {
            none(positional.is_some())?;
            WorkflowCommand::ContextShare {
                bundle_id: flags.bundle.ok_or(DaemonCliError::InvalidArguments)?,
                to_session: flags.to.ok_or(DaemonCliError::InvalidArguments)?,
            }
        }
        ("session", "inspect") => WorkflowCommand::SessionInspect {
            session_id: id()?,
            prompts: flags.prompts,
        },
        ("session", "attach") => WorkflowCommand::SessionAttach { session_id: id()? },
        ("prompt", "evaluate") => {
            none(positional.is_some())?;
            WorkflowCommand::PromptEvaluate {
                candidate: flags.candidate.ok_or(DaemonCliError::InvalidArguments)?,
                suite: flags.suite.ok_or(DaemonCliError::InvalidArguments)?,
            }
        }
        ("prompt", "versions") => {
            none(positional.is_some())?;
            WorkflowCommand::PromptVersions
        }
        ("prompt", "rollback") => {
            none(positional.is_some())?;
            WorkflowCommand::PromptRollback
        }
        _ => return Err(DaemonCliError::InvalidArguments),
    };
    Ok(CliCommand::Workflow {
        project_root: flags.project_root,
        command,
    })
}

/// Runs one workflow command against the running daemon and returns its
/// JSON output. Input files are read and bounded first; an export
/// destination is checked before any page is requested.
pub async fn run_workflow(
    paths: &DaemonPaths,
    command: WorkflowCommand,
) -> Result<Value, DaemonCliError> {
    let documents = match &command {
        WorkflowCommand::Prepare {
            request_file,
            policy_file,
        } => Some((
            read_document(request_file.clone(), MAX_WORKFLOW_REQUEST_BYTES).await?,
            read_document(policy_file.clone(), MAX_POLICY_BYTES).await?,
        )),
        WorkflowCommand::Export { out, .. } => {
            check_export_destination(out)?;
            None
        }
        _ => None,
    };
    let client = harness_client(paths).await?;
    let answer = match command {
        WorkflowCommand::Prepare { .. } => {
            let (request, policy) = documents.ok_or(DaemonCliError::InvalidArguments)?;
            let prepared = client
                .workflow_prepare(request, policy)
                .await
                .map_err(map_workflow_error)?;
            if prepared["outcome"] == "rejected" {
                let codes = prepared["violation_codes"]
                    .as_array()
                    .map(|codes| {
                        codes
                            .iter()
                            .filter_map(|code| code.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                return Err(DaemonCliError::ContractRejected(codes));
            }
            json!({"ok": true, "workflow": prepared})
        }
        WorkflowCommand::Start {
            contract,
            request_id,
        } => {
            json!({"ok": true, "workflow": client.workflow_start(&contract, request_id).await.map_err(map_workflow_error)?})
        }
        WorkflowCommand::Status { workflow_id } => {
            json!({"ok": true, "workflow": client.workflow_status(workflow_id).await.map_err(map_workflow_error)?})
        }
        WorkflowCommand::Pause { workflow_id } => {
            json!({"ok": true, "workflow": client.workflow_pause(workflow_id).await.map_err(map_workflow_error)?})
        }
        WorkflowCommand::Cancel { workflow_id } => {
            json!({"ok": true, "workflow": client.workflow_cancel(workflow_id).await.map_err(map_workflow_error)?})
        }
        WorkflowCommand::Purge { workflow_id } => {
            json!({"ok": true, "purge": client.workflow_purge_content(workflow_id).await.map_err(map_workflow_error)?})
        }
        WorkflowCommand::Export { workflow_id, out } => export(&client, workflow_id, &out).await?,
        WorkflowCommand::SlotsList => {
            json!({"ok": true, "slots": client.workflow_slots().await.map_err(map_workflow_error)?})
        }
        WorkflowCommand::ContextShare {
            bundle_id,
            to_session,
        } => {
            json!({"ok": true, "share": client.workflow_share(bundle_id, to_session).await.map_err(map_workflow_error)?})
        }
        WorkflowCommand::SessionInspect {
            session_id,
            prompts,
        } => {
            let inspection = client
                .workflow_session_inspect(session_id)
                .await
                .map_err(map_workflow_error)?;
            if prompts {
                let prompts = read_prompts(&client, session_id).await?;
                json!({"ok": true, "session": inspection, "prompts": prompts})
            } else {
                json!({"ok": true, "session": inspection})
            }
        }
        WorkflowCommand::SessionAttach { session_id } => {
            client
                .workflow_session_attach(session_id)
                .await
                .map_err(map_workflow_error)?;
            json!({"ok": true})
        }
        WorkflowCommand::PromptEvaluate { candidate, suite } => {
            json!({"ok": true, "evaluation": client.workflow_policy_evaluate(&candidate, &suite).await.map_err(map_workflow_error)?})
        }
        WorkflowCommand::PromptVersions => {
            json!({"ok": true, "versions": client.workflow_policy_versions().await.map_err(map_workflow_error)?})
        }
        WorkflowCommand::PromptRollback => {
            json!({"ok": true, "active": client.workflow_policy_rollback().await.map_err(map_workflow_error)?})
        }
    };
    Ok(answer)
}

/// Reads every prompt of a session; the last one is the next prompt.
async fn read_prompts(
    client: &ControlClient,
    session_id: Uuid,
) -> Result<Vec<Value>, DaemonCliError> {
    let mut prompts = Vec::new();
    let mut index = 0;
    while index < MAX_SESSION_PROMPTS {
        let prompt = client
            .workflow_session_prompt(session_id, index)
            .await
            .map_err(map_workflow_error)?;
        let total = prompt["total"].as_u64().unwrap_or(0);
        prompts.push(prompt);
        index += 1;
        if u64::try_from(index).unwrap_or(u64::MAX) >= total {
            break;
        }
    }
    Ok(prompts)
}

/// Reads every export page and writes one document into `out`, which it
/// creates.
async fn export(
    client: &ControlClient,
    workflow_id: Uuid,
    out: &Path,
) -> Result<Value, DaemonCliError> {
    let mut items = Vec::new();
    let mut cursor = 0usize;
    let mut first: Option<Value> = None;
    let mut complete = false;
    for _ in 0..MAX_EXPORT_PAGES {
        let page = client
            .workflow_export(workflow_id, cursor)
            .await
            .map_err(map_workflow_error)?;
        if let Some(page_items) = page["items"].as_array() {
            items.extend(page_items.iter().cloned());
        }
        let next = page["next"].as_u64();
        if first.is_none() {
            first = Some(page);
        }
        match next {
            Some(next) => {
                cursor = usize::try_from(next).map_err(|_| DaemonCliError::InvalidArguments)?
            }
            None => {
                complete = true;
                break;
            }
        }
    }
    if !complete {
        return Err(DaemonCliError::Control {
            code: "cli_export_incomplete".to_string(),
        });
    }
    let first = first.unwrap_or(Value::Null);
    let document = json!({
        "schema_version": first["schema_version"],
        "workflow_id": workflow_id,
        "total_items": first["total_items"],
        "items": items,
    });
    let out = out.to_path_buf();
    let written = tokio::task::spawn_blocking(move || write_export(&out, &document))
        .await
        .map_err(|_| export_error())??;
    Ok(json!({
        "ok": true,
        "workflow_id": workflow_id,
        "items": written,
        "file": EXPORT_FILE_NAME,
    }))
}

/// The destination must not exist, or be an empty directory, so the
/// export never mixes with or overwrites existing files.
fn check_export_destination(out: &Path) -> Result<(), DaemonCliError> {
    match std::fs::symlink_metadata(out) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(export_error()),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            let mut entries = std::fs::read_dir(out).map_err(|_| export_error())?;
            if entries.next().is_some() {
                Err(DaemonCliError::Control {
                    code: "cli_export_destination_not_empty".to_string(),
                })
            } else {
                Ok(())
            }
        }
        Ok(_) => Err(DaemonCliError::Control {
            code: "cli_export_destination_not_empty".to_string(),
        }),
    }
}

fn write_export(out: &Path, document: &Value) -> Result<usize, DaemonCliError> {
    check_export_destination(out)?;
    std::fs::create_dir_all(out).map_err(|_| export_error())?;
    let encoded = serde_json::to_vec_pretty(document).map_err(|_| export_error())?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out.join(EXPORT_FILE_NAME))
        .map_err(|_| export_error())?;
    file.write_all(&encoded).map_err(|_| export_error())?;
    file.sync_all().map_err(|_| export_error())?;
    Ok(document["items"].as_array().map_or(0, Vec::len))
}

fn export_error() -> DaemonCliError {
    DaemonCliError::Control {
        code: "cli_export_write_failed".to_string(),
    }
}

/// Reads at most one byte beyond `limit` and parses one JSON document.
async fn read_document(path: PathBuf, limit: usize) -> Result<Value, DaemonCliError> {
    let bytes = tokio::task::spawn_blocking(move || {
        let mut bytes = Vec::new();
        std::fs::File::open(&path)
            .and_then(|file| file.take(limit as u64 + 1).read_to_end(&mut bytes))
            .map(|_| bytes)
    })
    .await
    .map_err(|_| input_error("cli_input_unavailable"))?
    .map_err(|_| input_error("cli_input_unavailable"))?;
    if bytes.len() > limit {
        return Err(input_error("cli_input_too_large"));
    }
    serde_json::from_slice(&bytes).map_err(|_| input_error("cli_input_invalid_json"))
}

fn input_error(code: &str) -> DaemonCliError {
    DaemonCliError::Control {
        code: code.to_string(),
    }
}

/// A daemon older than Control v6 lacks the operations; say so instead of
/// reporting stale runtime state.
fn map_workflow_error(error: ControlError) -> DaemonCliError {
    match error {
        ControlError::UnsupportedVersion => DaemonCliError::Control {
            code: error.code().to_string(),
        },
        other => map_control_error(other),
    }
}

fn parse_id(text: &str) -> Result<Uuid, DaemonCliError> {
    Uuid::parse_str(text)
        .ok()
        .filter(|id| !id.is_nil())
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

    fn parse(verb: &str, arguments: &[&str]) -> Result<WorkflowCommand, DaemonCliError> {
        let arguments: Vec<OsString> = arguments.iter().map(OsString::from).collect();
        match parse_workflow_arguments(verb, &arguments)? {
            CliCommand::Workflow { command, .. } => Ok(command),
            _ => Err(DaemonCliError::InvalidArguments),
        }
    }

    #[test]
    fn every_command_parses_its_documented_form() {
        let id = Uuid::new_v4();
        let text = id.to_string();
        assert_eq!(
            parse(
                "workflow",
                &[
                    "prepare",
                    "--request-file",
                    "r.json",
                    "--policy-file",
                    "p.json"
                ]
            ),
            Ok(WorkflowCommand::Prepare {
                request_file: PathBuf::from("r.json"),
                policy_file: PathBuf::from("p.json"),
            })
        );
        assert_eq!(
            parse(
                "workflow",
                &["start", "--contract", "ab", "--request-id", &text]
            ),
            Ok(WorkflowCommand::Start {
                contract: "ab".to_string(),
                request_id: id,
            })
        );
        assert_eq!(
            parse("workflow", &["status", &text]),
            Ok(WorkflowCommand::Status { workflow_id: id })
        );
        assert_eq!(
            parse("workflow", &["export", &text, "--out", "dir"]),
            Ok(WorkflowCommand::Export {
                workflow_id: id,
                out: PathBuf::from("dir"),
            })
        );
        assert_eq!(parse("slots", &["list"]), Ok(WorkflowCommand::SlotsList));
        assert_eq!(
            parse("context", &["share", "--bundle", &text, "--to", &text]),
            Ok(WorkflowCommand::ContextShare {
                bundle_id: id,
                to_session: id,
            })
        );
        assert_eq!(
            parse("session", &["inspect", &text, "--prompts"]),
            Ok(WorkflowCommand::SessionInspect {
                session_id: id,
                prompts: true,
            })
        );
        assert_eq!(
            parse("session", &["attach", &text]),
            Ok(WorkflowCommand::SessionAttach { session_id: id })
        );
        assert_eq!(
            parse(
                "prompt",
                &["evaluate", "--candidate", "c1", "--suite", "s1"]
            ),
            Ok(WorkflowCommand::PromptEvaluate {
                candidate: "c1".to_string(),
                suite: "s1".to_string(),
            })
        );
        assert_eq!(
            parse("prompt", &["rollback"]),
            Ok(WorkflowCommand::PromptRollback)
        );
    }

    #[test]
    fn misplaced_or_missing_arguments_are_refused() {
        let text = Uuid::new_v4().to_string();
        for (verb, arguments) in [
            ("workflow", vec!["prepare", "--request-file", "r.json"]),
            ("workflow", vec!["start", "--contract", "ab"]),
            ("workflow", vec!["status"]),
            ("workflow", vec!["status", "not-a-uuid"]),
            ("workflow", vec!["status", &Uuid::nil().to_string()]),
            ("workflow", vec!["export", &text]),
            ("workflow", vec!["status", &text, "--prompts"]),
            ("slots", vec!["list", &text]),
            ("session", vec!["attach", &text, "--prompts"]),
            ("prompt", vec!["evaluate", "--suite", "s1"]),
            ("workflow", vec!["merge", &text]),
        ] {
            assert_eq!(
                parse(verb, &arguments),
                Err(DaemonCliError::InvalidArguments),
                "{verb} {arguments:?}"
            );
        }
    }

    #[test]
    fn the_export_destination_must_be_absent_or_empty() {
        let temp = tempfile::tempdir().expect("tempdir");
        let absent = temp.path().join("absent");
        assert_eq!(check_export_destination(&absent), Ok(()));
        let empty = temp.path().join("empty");
        std::fs::create_dir(&empty).expect("dir");
        assert_eq!(check_export_destination(&empty), Ok(()));
        std::fs::write(empty.join("keep.txt"), "user file").expect("file");
        assert_eq!(
            check_export_destination(&empty).map_err(|error| error.code().to_string()),
            Err("cli_export_destination_not_empty".to_string())
        );
        let file = temp.path().join("file.txt");
        std::fs::write(&file, "x").expect("file");
        assert!(check_export_destination(&file).is_err());
        let document = json!({"schema_version": 1, "items": [1, 2]});
        assert_eq!(write_export(&absent, &document), Ok(2));
        assert!(absent.join(EXPORT_FILE_NAME).is_file());
        // A second export into the now non-empty directory is refused.
        assert!(write_export(&absent, &document).is_err());
    }
}
