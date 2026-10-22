//! `vibemuxctl tasks`: a read-only Control v4 client for the daemon's bounded,
//! sanitized Task/Run read models, the same ones the console, GUI, web, and MCP
//! frontends show. It writes nothing and starts no daemon; a task is created by
//! a dispatch or a workflow, not here.

use std::ffi::OsString;

use serde_json::{Value, json};
use vibemux_types::{
    TaskId,
    frontend::{FrontendTasksQuery, MAX_FRONTEND_TASKS},
};
use vibemuxd::process::DaemonPaths;

use crate::{
    CliCommand, DaemonCliError,
    argument_scan::{scan_tail, split_action},
    dispatch_commands::map_dispatch_error,
    harness_client,
};

/// Tasks one `tasks list` asks for unless `--limit` says otherwise. The daemon
/// bounds the limit at [`MAX_FRONTEND_TASKS`].
const DEFAULT_LIST_LIMIT: u32 = 20;
/// Error code for a task id the daemon has no record of.
const TASK_NOT_FOUND_CODE: &str = "frontend_task_not_found";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TaskCommand {
    /// Tasks after the `after` cursor, in the daemon's order.
    List {
        after: Option<TaskId>,
        limit: u32,
    },
    Show {
        task_id: TaskId,
    },
}

/// Parses the arguments after the `tasks` verb.
pub(crate) fn parse_task_arguments(arguments: &[OsString]) -> Result<CliCommand, DaemonCliError> {
    let (action, rest) = split_action(arguments)?;
    let command_flags: &[&str] = if action == "list" {
        &["--after", "--limit"]
    } else {
        &[]
    };
    let tail = scan_tail(rest, command_flags)?;
    let command = match action {
        "list" => {
            tail.without_positionals()?;
            TaskCommand::List {
                after: tail.flag("--after").map(parse_task_id).transpose()?,
                limit: match tail.flag("--limit") {
                    Some(text) => parse_limit(text)?,
                    None => DEFAULT_LIST_LIMIT,
                },
            }
        }
        "show" => {
            let [task_id] = tail.positionals()?;
            TaskCommand::Show {
                task_id: parse_task_id(task_id)?,
            }
        }
        _ => return Err(DaemonCliError::InvalidArguments),
    };
    Ok(CliCommand::Tasks {
        project_root: tail.project_root,
        command,
    })
}

/// Runs one task command against the running daemon and returns its JSON.
pub async fn run_task(paths: &DaemonPaths, command: TaskCommand) -> Result<Value, DaemonCliError> {
    let client = harness_client(paths).await?;
    match command {
        TaskCommand::List { after, limit } => {
            let page = client
                .frontend_tasks(FrontendTasksQuery { after, limit })
                .await
                .map_err(map_dispatch_error)?;
            Ok(json!({
                "ok": true,
                "project_id": page.project_id,
                "tasks": page.tasks,
                "next_cursor": page.next_cursor,
            }))
        }
        TaskCommand::Show { task_id } => {
            let detail = client
                .frontend_task(task_id)
                .await
                .map_err(map_dispatch_error)?
                .ok_or_else(|| DaemonCliError::Control {
                    code: TASK_NOT_FOUND_CODE.to_string(),
                })?;
            Ok(json!({
                "ok": true,
                "task": detail.task,
                "runs": detail.runs,
                "has_more_runs": detail.has_more_runs,
                "events": detail.events,
                "has_more_events": detail.has_more_events,
                "artifacts": detail.artifacts,
                "has_more_artifacts": detail.has_more_artifacts,
            }))
        }
    }
}

fn parse_task_id(text: &str) -> Result<TaskId, DaemonCliError> {
    text.parse().map_err(|_| DaemonCliError::InvalidArguments)
}

fn parse_limit(text: &str) -> Result<u32, DaemonCliError> {
    text.parse::<u32>()
        .ok()
        .filter(|limit| (1..=MAX_FRONTEND_TASKS).contains(limit))
        .ok_or(DaemonCliError::InvalidArguments)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn parse(arguments: &[&str]) -> Result<CliCommand, DaemonCliError> {
        let arguments: Vec<OsString> = arguments.iter().map(OsString::from).collect();
        parse_task_arguments(&arguments)
    }

    fn task(arguments: &[&str]) -> TaskCommand {
        match parse(arguments).expect("valid arguments") {
            CliCommand::Tasks { command, .. } => command,
            other => panic!("unexpected command {other:?}"),
        }
    }

    #[test]
    fn list_defaults_to_one_page_and_accepts_a_cursor_and_limit() {
        assert_eq!(
            task(&["list"]),
            TaskCommand::List {
                after: None,
                limit: DEFAULT_LIST_LIMIT,
            }
        );
        let cursor = TaskId::new();
        assert_eq!(
            task(&["list", "--limit", "5", "--after", &cursor.to_string()]),
            TaskCommand::List {
                after: Some(cursor),
                limit: 5,
            }
        );
        assert_eq!(
            task(&["list", "--limit", &MAX_FRONTEND_TASKS.to_string()]),
            TaskCommand::List {
                after: None,
                limit: MAX_FRONTEND_TASKS,
            }
        );
    }

    #[test]
    fn show_takes_one_task_id_and_every_action_takes_a_project_root() {
        let task_id = TaskId::new();
        assert_eq!(
            task(&["show", &task_id.to_string()]),
            TaskCommand::Show { task_id }
        );
        match parse(&["list", "--project-root", "C:\\project"]).expect("root") {
            CliCommand::Tasks { project_root, .. } => {
                assert_eq!(project_root, Some(PathBuf::from("C:\\project")));
            }
            other => panic!("unexpected command {other:?}"),
        }
    }

    #[test]
    fn malformed_arguments_are_refused() {
        let id = TaskId::new().to_string();
        let nil = uuid::Uuid::nil().to_string();
        let over_limit = (MAX_FRONTEND_TASKS + 1).to_string();
        for arguments in [
            &[][..],
            &["status"][..],
            &["list", &id][..],
            &["list", "--limit", "0"][..],
            &["list", "--limit", &over_limit][..],
            &["list", "--limit", "-1"][..],
            &["list", "--limit", "x"][..],
            &["list", "--limit", "1", "--limit", "2"][..],
            &["list", "--after", "not_a_uuid"][..],
            &["list", "--after", &nil][..],
            &["list", "--prompt-file", "-"][..],
            &["show"][..],
            &["show", "not_a_uuid"][..],
            &["show", &nil][..],
            &["show", &id, &id][..],
            &["show", &id, "--limit", "5"][..],
            &["show", &id, "--after", &id][..],
        ] {
            assert_eq!(
                parse(arguments).map(|_| ()),
                Err(DaemonCliError::InvalidArguments),
                "{arguments:?}"
            );
        }
    }
}
