//! `vibemuxctl terminal`: the Control v4 observation-only terminal operations.
//!
//! The daemon matches the panes of one explicitly configured terminal plugin
//! (WezTerm by default) to a Run's worktree. `link` records an in-memory
//! observation lease on one pane, `focus` raises that pane, and `unlink` drops
//! the lease. Nothing here attaches to a pane, sends input, launches a harness,
//! or infers a Run's outcome from a pane. A lease lives in the daemon for 30
//! minutes at most and is lost on restart.

use std::ffi::OsString;

use serde_json::{Value, json};
use uuid::Uuid;
use vibemux_types::{RunId, TaskId};
use vibemuxd::{
    control::ControlClient,
    process::DaemonPaths,
    terminal_observer::{TerminalLinkRequest, TerminalQuery},
};

use crate::{
    CliCommand, DaemonCliError,
    argument_scan::{scan_tail, split_action},
    dispatch_commands::{map_dispatch_error, parse_id},
    harness_client,
};

/// The terminal plugin used unless `--plugin-id` names another registered one.
const DEFAULT_PLUGIN_ID: &str = "wezterm_observer";
/// Upper bound the daemon applies to a plugin id and a terminal instance id.
const MAX_LABEL_BYTES: usize = 128;
/// Upper bound the daemon applies to a native pane id (decimal digits).
const MAX_PANE_ID_DIGITS: usize = 20;
/// Error code for a task or run the daemon cannot resolve to a worktree.
const RUN_UNAVAILABLE_CODE: &str = "terminal_run_unavailable";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalCommand {
    /// Lists the plugin's panes that match the Run and reports its lease.
    Inspect {
        task_id: TaskId,
        run_id: RunId,
        plugin_id: String,
    },
    /// Records an observation lease on one pane from an `inspect` listing.
    Link {
        task_id: TaskId,
        run_id: RunId,
        plugin_id: String,
        instance_id: String,
        pane_id: String,
    },
    Focus {
        binding_id: Uuid,
    },
    Unlink {
        binding_id: Uuid,
    },
}

/// Parses the arguments after the `terminal` verb.
pub(crate) fn parse_terminal_arguments(
    arguments: &[OsString],
) -> Result<CliCommand, DaemonCliError> {
    let (action, rest) = split_action(arguments)?;
    let command_flags: &[&str] = match action {
        "inspect" => &["--plugin-id"],
        "link" => &["--plugin-id", "--instance-id", "--pane-id"],
        _ => &[],
    };
    let tail = scan_tail(rest, command_flags)?;
    let plugin_id = || match tail.flag("--plugin-id") {
        Some(text) => label(text),
        None => Ok(DEFAULT_PLUGIN_ID.to_string()),
    };
    let command = match action {
        "inspect" => {
            let [task_id, run_id] = tail.positionals()?;
            TerminalCommand::Inspect {
                task_id: parse_task_id(task_id)?,
                run_id: parse_run_id(run_id)?,
                plugin_id: plugin_id()?,
            }
        }
        "link" => {
            let [task_id, run_id] = tail.positionals()?;
            TerminalCommand::Link {
                task_id: parse_task_id(task_id)?,
                run_id: parse_run_id(run_id)?,
                plugin_id: plugin_id()?,
                instance_id: label(
                    tail.flag("--instance-id")
                        .ok_or(DaemonCliError::InvalidArguments)?,
                )?,
                pane_id: pane_id(
                    tail.flag("--pane-id")
                        .ok_or(DaemonCliError::InvalidArguments)?,
                )?,
            }
        }
        "focus" | "unlink" => {
            let [binding_id] = tail.positionals()?;
            let binding_id = parse_id(binding_id)?;
            if action == "focus" {
                TerminalCommand::Focus { binding_id }
            } else {
                TerminalCommand::Unlink { binding_id }
            }
        }
        _ => return Err(DaemonCliError::InvalidArguments),
    };
    Ok(CliCommand::Terminal {
        project_root: tail.project_root,
        command,
    })
}

/// Runs one terminal command against the running daemon and returns its JSON.
pub async fn run_terminal(
    paths: &DaemonPaths,
    command: TerminalCommand,
) -> Result<Value, DaemonCliError> {
    let client = harness_client(paths).await?;
    match command {
        TerminalCommand::Inspect {
            task_id,
            run_id,
            plugin_id,
        } => {
            let query = terminal_query(&client, task_id, run_id, plugin_id).await?;
            let snapshot = client
                .terminal_inspect(query)
                .await
                .map_err(map_dispatch_error)?;
            Ok(json!({"ok": true, "terminal": snapshot}))
        }
        TerminalCommand::Link {
            task_id,
            run_id,
            plugin_id,
            instance_id,
            pane_id,
        } => {
            let query = terminal_query(&client, task_id, run_id, plugin_id).await?;
            let binding = client
                .terminal_link(TerminalLinkRequest {
                    query,
                    instance_id,
                    pane_id,
                })
                .await
                .map_err(map_dispatch_error)?;
            Ok(json!({"ok": true, "binding": binding}))
        }
        TerminalCommand::Focus { binding_id } => {
            client
                .terminal_focus(&binding_id.to_string())
                .await
                .map_err(map_dispatch_error)?;
            Ok(json!({"ok": true, "binding_id": binding_id, "status": "focused"}))
        }
        TerminalCommand::Unlink { binding_id } => {
            client
                .terminal_unlink(&binding_id.to_string())
                .await
                .map_err(map_dispatch_error)?;
            Ok(json!({"ok": true, "binding_id": binding_id, "status": "unlinked"}))
        }
    }
}

/// The daemon's query for one Run. Only the project id is looked up here; the
/// daemon itself checks that the Run belongs to the task and the project and
/// that its worktree exists.
async fn terminal_query(
    client: &ControlClient,
    task_id: TaskId,
    run_id: RunId,
    plugin_id: String,
) -> Result<TerminalQuery, DaemonCliError> {
    let detail = client
        .frontend_task(task_id)
        .await
        .map_err(map_dispatch_error)?
        .ok_or_else(|| DaemonCliError::Control {
            code: RUN_UNAVAILABLE_CODE.to_string(),
        })?;
    Ok(TerminalQuery {
        project_id: detail.task.project_id,
        task_id,
        run_id,
        plugin_id,
    })
}

fn parse_task_id(text: &str) -> Result<TaskId, DaemonCliError> {
    text.parse().map_err(|_| DaemonCliError::InvalidArguments)
}

fn parse_run_id(text: &str) -> Result<RunId, DaemonCliError> {
    text.parse().map_err(|_| DaemonCliError::InvalidArguments)
}

/// A plugin or instance id: non-empty, bounded, and free of control characters.
fn label(text: &str) -> Result<String, DaemonCliError> {
    if text.is_empty() || text.len() > MAX_LABEL_BYTES || text.chars().any(char::is_control) {
        return Err(DaemonCliError::InvalidArguments);
    }
    Ok(text.to_string())
}

/// A native pane id: a bounded run of ASCII digits.
fn pane_id(text: &str) -> Result<String, DaemonCliError> {
    if text.is_empty()
        || text.len() > MAX_PANE_ID_DIGITS
        || !text.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(DaemonCliError::InvalidArguments);
    }
    Ok(text.to_string())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn parse(arguments: &[&str]) -> Result<CliCommand, DaemonCliError> {
        let arguments: Vec<OsString> = arguments.iter().map(OsString::from).collect();
        parse_terminal_arguments(&arguments)
    }

    fn terminal(arguments: &[&str]) -> TerminalCommand {
        match parse(arguments).expect("valid arguments") {
            CliCommand::Terminal { command, .. } => command,
            other => panic!("unexpected command {other:?}"),
        }
    }

    #[test]
    fn inspect_and_link_name_a_run_and_default_to_the_wezterm_plugin() {
        let task_id = TaskId::new();
        let run_id = RunId::new();
        let (task, run) = (task_id.to_string(), run_id.to_string());
        assert_eq!(
            terminal(&["inspect", &task, &run]),
            TerminalCommand::Inspect {
                task_id,
                run_id,
                plugin_id: DEFAULT_PLUGIN_ID.to_string(),
            }
        );
        assert_eq!(
            terminal(&["inspect", &task, &run, "--plugin-id", "other_observer"]),
            TerminalCommand::Inspect {
                task_id,
                run_id,
                plugin_id: "other_observer".to_string(),
            }
        );
        assert_eq!(
            terminal(&[
                "link",
                &task,
                &run,
                "--pane-id",
                "7",
                "--instance-id",
                "wezterm 1",
            ]),
            TerminalCommand::Link {
                task_id,
                run_id,
                plugin_id: DEFAULT_PLUGIN_ID.to_string(),
                instance_id: "wezterm 1".to_string(),
                pane_id: "7".to_string(),
            }
        );
    }

    #[test]
    fn focus_and_unlink_take_a_binding_id_and_every_action_takes_a_project_root() {
        let binding_id = Uuid::new_v4();
        let text = binding_id.to_string();
        assert_eq!(
            terminal(&["focus", &text]),
            TerminalCommand::Focus { binding_id }
        );
        assert_eq!(
            terminal(&["unlink", &text]),
            TerminalCommand::Unlink { binding_id }
        );
        match parse(&["unlink", &text, "--project-root", "C:\\project"]).expect("root") {
            CliCommand::Terminal { project_root, .. } => {
                assert_eq!(project_root, Some(PathBuf::from("C:\\project")));
            }
            other => panic!("unexpected command {other:?}"),
        }
    }

    #[test]
    fn malformed_arguments_are_refused() {
        let task = TaskId::new().to_string();
        let run = RunId::new().to_string();
        let binding = Uuid::new_v4().to_string();
        let nil = Uuid::nil().to_string();
        let long_label = "x".repeat(MAX_LABEL_BYTES + 1);
        let long_pane = "1".repeat(MAX_PANE_ID_DIGITS + 1);
        for arguments in [
            &[][..],
            &["list"][..],
            &["inspect"][..],
            &["inspect", &task][..],
            &["inspect", &run, &task, "extra"][..],
            &["inspect", "not_a_uuid", &run][..],
            &["inspect", &nil, &run][..],
            &["inspect", &task, &nil][..],
            &["inspect", &task, &run, "--plugin-id"][..],
            &["inspect", &task, &run, "--plugin-id", ""][..],
            &["inspect", &task, &run, "--plugin-id", &long_label][..],
            &["inspect", &task, &run, "--plugin-id", "a\nb"][..],
            &["inspect", &task, &run, "--pane-id", "7"][..],
            &["link", &task, &run][..],
            &["link", &task, &run, "--pane-id", "7"][..],
            &["link", &task, &run, "--instance-id", "i"][..],
            &["link", &task, &run, "--instance-id", "i", "--pane-id", "x"][..],
            &["link", &task, &run, "--instance-id", "i", "--pane-id", "-1"][..],
            &[
                "link",
                &task,
                &run,
                "--instance-id",
                "i",
                "--pane-id",
                &long_pane,
            ][..],
            &["link", &task, &run, "--instance-id", "", "--pane-id", "7"][..],
            &[
                "link",
                &task,
                &run,
                "--instance-id",
                "i",
                "--pane-id",
                "7",
                "--pane-id",
                "8",
            ][..],
            &["focus"][..],
            &["focus", "not_a_uuid"][..],
            &["focus", &nil][..],
            &["focus", &binding, &binding][..],
            &["focus", &binding, "--pane-id", "7"][..],
            &["unlink"][..],
            &["unlink", &task, &run][..],
        ] {
            assert_eq!(
                parse(arguments).map(|_| ()),
                Err(DaemonCliError::InvalidArguments),
                "{arguments:?}"
            );
        }
    }
}
