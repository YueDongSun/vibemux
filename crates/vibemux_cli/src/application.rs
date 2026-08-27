//! Application orchestration. UI and inspection never start a daemon or model turn.
use crate::{
    CliCommand, DaemonBootstrapConfig, DaemonCliError, DaemonStartOutcome,
    command::{self, Command, Format},
    daemon_executable, daemon_health, daemon_paths,
    failure::Failure,
    probe,
    recovery::{inspect_runtime, recover_runtime},
    start_daemon, stop_daemon, workspace,
};
use serde_json::{Value, json};
use std::{
    io::{self, IsTerminal},
    path::PathBuf,
};
use vibemux_frontend::{Context, DashboardModel, State};
use vibemuxd::control::DaemonHealth;

pub enum Outcome {
    Data {
        kind: &'static str,
        body: Value,
        success: bool,
    },
    Text(String),
    Interactive,
}

pub async fn run(command: Command, format: Format) -> Result<Outcome, Failure> {
    match command {
        Command::Help(topic) => Ok(Outcome::Text(command::help(topic))),
        Command::Version => Ok(Outcome::Text(format!("{}\n", env!("CARGO_PKG_VERSION")))),
        Command::Daemon(command) => {
            let body = run_daemon(command)
                .await
                .map_err(|source| Failure::with_source(source.code(), source.clone()))?;
            Ok(Outcome::Data {
                kind: "daemon",
                body,
                success: true,
            })
        }
        Command::Harness(command) => {
            let reply = vibemux_harness::cli::run(command)
                .await
                .map_err(|source| Failure::with_source(source.code(), source))?;
            Ok(Outcome::Data {
                kind: "harness",
                body: reply.body,
                success: reply.success,
            })
        }
        Command::Probe { report_path } => {
            let snapshot = probe::load(report_path)
                .await
                .map_err(|source| Failure::with_source(source.code(), source))?;
            Ok(Outcome::Data {
                kind: "probe",
                body: snapshot.public_json(),
                success: true,
            })
        }
        Command::Workspace { project_root } => {
            let report = workspace::inspect(project_root).await?;
            Ok(Outcome::Data {
                kind: "workspace",
                body: encode(report)?,
                success: true,
            })
        }
        Command::Status { project_root } => {
            let report = workspace::inspect(project_root.clone()).await?;
            let daemon = existing_daemon(project_root).await;
            Ok(Outcome::Data {
                kind: "status",
                body: json!({"schema_version":1,"status":"inspected",
                "workspace":report,"daemon":daemon}),
                success: true,
            })
        }
        Command::Ui {
            project_root,
            report_path,
            once,
            width,
            height,
        } => {
            if !once
                && (!io::stdin().is_terminal()
                    || !io::stdout().is_terminal()
                    || format == Format::Json)
            {
                return Err(Failure::new("ui_terminal_required"));
            }
            let report = workspace::inspect(project_root).await?;
            let snapshot = probe::load(report_path)
                .await
                .map_err(|source| Failure::with_source(source.code(), source))?;
            if once && format == Format::Json {
                return Ok(Outcome::Data {
                    kind: "ui",
                    body: json!({"schema_version":1,"source":snapshot.source,
                    "probe":snapshot.public_json(),"workspace":report}),
                    success: true,
                });
            }
            let model = DashboardModel::from_report(&snapshot.report);
            let context = Context {
                source: snapshot.source.to_string(),
                workspace: Some(frontend_workspace(&report)),
            };
            if once {
                let rendered =
                    vibemux_frontend::snapshot(&model, &State::default(), &context, width, height)
                        .map_err(|source| Failure::with_source("ui_render_failed", source))?;
                return Ok(Outcome::Text(rendered));
            }
            tokio::task::spawn_blocking(move || vibemux_frontend::run_interactive(model, context))
                .await
                .map_err(|source| Failure::with_source("ui_worker_failed", source))?
                .map_err(|source| Failure::with_source("ui_terminal_failed", source))?;
            Ok(Outcome::Interactive)
        }
    }
}

async fn existing_daemon(project_root: Option<PathBuf>) -> Value {
    let paths = match tokio::task::spawn_blocking(move || daemon_paths(project_root.as_deref()))
        .await
    {
        Ok(Ok(paths)) => paths,
        Ok(Err(error)) => {
            return json!({"status":"unavailable","healthy":false,"error_code":error.code()});
        }
        Err(_) => {
            return json!({"status":"unavailable","healthy":false,"error_code":"cli_worker_failed"});
        }
    };
    match daemon_health(&paths).await {
        Ok(health) => health_json("running", &health),
        Err(error) => {
            json!({"status":if error == DaemonCliError::NotRunning {"not_running"} else {"unavailable"},
            "healthy":false,"error_code":error.code()})
        }
    }
}

async fn run_daemon(command: CliCommand) -> Result<Value, DaemonCliError> {
    match command {
        CliCommand::DaemonStart {
            project_root,
            daemon_executable: explicit_executable,
        } => {
            let (paths, executable) = tokio::task::spawn_blocking(move || {
                Ok::<_, DaemonCliError>((
                    daemon_paths(project_root.as_deref())?,
                    daemon_executable(explicit_executable.as_deref())?,
                ))
            })
            .await
            .map_err(|_| DaemonCliError::StartFailed)??;
            let outcome = start_daemon(&DaemonBootstrapConfig::new(paths, executable)).await?;
            let status = match &outcome {
                DaemonStartOutcome::Started(_) => "started",
                DaemonStartOutcome::AlreadyRunning(_) => "already_running",
            };
            Ok(health_json(status, outcome.health()))
        }
        CliCommand::DaemonHealth { project_root } => {
            let paths = paths(project_root).await?;
            Ok(health_json("running", &daemon_health(&paths).await?))
        }
        CliCommand::DaemonStop { project_root } => {
            let paths = paths(project_root).await?;
            let health = stop_daemon(&paths).await?;
            Ok(json!({"ok":true,"status":"stopped","process_id":health.process_id}))
        }
        CliCommand::DaemonInspect { project_root } => {
            let paths = paths(project_root).await?;
            serde_json::to_value(inspect_runtime(&paths).await?).map_err(|_| output_error())
        }
        CliCommand::DaemonRecover {
            project_root,
            confirmation,
        } => {
            let paths = paths(project_root).await?;
            serde_json::to_value(recover_runtime(&paths, &confirmation).await?)
                .map_err(|_| output_error())
        }
        CliCommand::Help | CliCommand::Version => Err(DaemonCliError::InvalidArguments),
    }
}

async fn paths(
    project_root: Option<PathBuf>,
) -> Result<vibemuxd::process::DaemonPaths, DaemonCliError> {
    tokio::task::spawn_blocking(move || daemon_paths(project_root.as_deref()))
        .await
        .map_err(|_| output_error())?
}
fn output_error() -> DaemonCliError {
    DaemonCliError::Control {
        code: "cli_output_encoding_failed".to_string(),
    }
}

fn encode(value: impl serde::Serialize) -> Result<Value, Failure> {
    serde_json::to_value(value)
        .map_err(|source| Failure::with_source("cli_output_encoding_failed", source))
}

fn health_json(status: &str, health: &DaemonHealth) -> Value {
    json!({"ok":true,"status":status,"healthy":health.healthy,"process_id":health.process_id,
        "store_schema_version":health.store_schema_version,"queue_capacity":health.queue_capacity})
}

fn frontend_workspace(report: &workspace::Report) -> vibemux_frontend::Workspace {
    vibemux_frontend::Workspace {
        current_branch: report
            .current_branch
            .clone()
            .unwrap_or_else(|| "detached".to_string()),
        head: report.head.chars().take(8).collect(),
        baseline: report.baseline.clone(),
        ahead: report.ahead,
        behind: report.behind,
        staged: report.staged,
        unstaged: report.unstaged,
        untracked: report.untracked,
        change_detection: report.change_detection.clone(),
        branches: report
            .branches
            .iter()
            .map(|branch| vibemux_frontend::Branch {
                name: branch.name.clone(),
                role: branch.role.clone(),
                commit: branch.commit.chars().take(8).collect(),
                upstream: branch.upstream.clone().unwrap_or_else(|| "-".to_string()),
            })
            .collect(),
        worktrees: report
            .worktrees
            .iter()
            .map(|worktree| {
                format!(
                    "{} -> {}",
                    worktree.branch.as_deref().unwrap_or("detached"),
                    worktree.location
                )
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_daemon_health_json_keeps_its_allowlisted_contract() {
        let output = health_json(
            "running",
            &DaemonHealth {
                healthy: true,
                process_id: 42,
                store_schema_version: 1,
                queue_capacity: 64,
            },
        );
        let keys = output
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            keys,
            std::collections::BTreeSet::from([
                "healthy",
                "ok",
                "process_id",
                "queue_capacity",
                "status",
                "store_schema_version"
            ])
        );
        assert!(!output.to_string().contains("project_root"));
    }
}
