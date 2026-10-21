//! Application orchestration. UI and inspection never start a daemon or model turn.
use crate::{
    CliCommand, DaemonBootstrapConfig, DaemonCliError, DaemonStartOutcome,
    command::{self, Command, Format},
    daemon_executable, daemon_health, daemon_paths,
    decision_commands::run_decision,
    dispatch_commands::run_dispatch,
    dsh_commands::run_dsh,
    failure::Failure,
    harness_list, harness_switch, probe,
    recovery::{inspect_runtime, recover_runtime},
    start_daemon, stop_daemon,
    workflow_commands::run_workflow,
    workspace,
};
use serde_json::{Value, json};
use std::{
    io::{self, IsTerminal},
    path::PathBuf,
};
use vibemux_frontend::dashboard::{self, Context, DashboardModel, State};
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
        Command::Control(command) => {
            let kind = control_kind(&command);
            let body = run_control(command).await.map_err(|source| {
                Failure::with_message(source.code(), source.to_string(), source.clone())
            })?;
            Ok(Outcome::Data {
                kind,
                body,
                success: true,
            })
        }
        Command::Harness(command) => {
            let reply = vibemux_harness_native::cli::run(command)
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
                ..Context::default()
            };
            if once {
                let rendered =
                    dashboard::snapshot(&model, &State::default(), &context, width, height)
                        .map_err(|source| Failure::with_source("ui_render_failed", source))?;
                return Ok(Outcome::Text(rendered));
            }
            tokio::task::spawn_blocking(move || dashboard::run_interactive(model, context))
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

/// Output kind for the text renderer: only the lifecycle verbs have a text view.
fn control_kind(command: &CliCommand) -> &'static str {
    match command {
        CliCommand::DaemonStart { .. }
        | CliCommand::DaemonHealth { .. }
        | CliCommand::DaemonStop { .. }
        | CliCommand::DaemonInspect { .. }
        | CliCommand::DaemonRecover { .. } => "daemon",
        _ => "control",
    }
}

async fn run_control(command: CliCommand) -> Result<Value, DaemonCliError> {
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
        CliCommand::Harnesses {
            project_root,
            cached,
        } => {
            let paths = paths(project_root).await?;
            let rows = harness_list(&paths, cached).await?;
            Ok(json!({"ok":true,"harnesses":rows}))
        }
        CliCommand::Switch {
            project_root,
            harness,
        } => {
            let paths = paths(project_root).await?;
            let (from, to) = harness_switch(&paths, &harness).await?;
            Ok(json!({"ok":true,"default_harness":to,"from":from,"to":to}))
        }
        CliCommand::Dispatch {
            project_root,
            command,
        } => {
            let paths = paths(project_root).await?;
            run_dispatch(&paths, command).await
        }
        CliCommand::Decision {
            project_root,
            command,
        } => {
            let paths = paths(project_root).await?;
            run_decision(&paths, command).await
        }
        CliCommand::Dsh {
            project_root,
            command,
        } => {
            let paths = paths(project_root).await?;
            run_dsh(&paths, command).await
        }
        CliCommand::Workflow {
            project_root,
            command,
        } => {
            let paths = paths(project_root).await?;
            run_workflow(&paths, command).await
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
        "store_schema_version":health.store_schema_version,"queue_capacity":health.queue_capacity,
        "queue_depth":health.queue_depth,"queue_high_watermark":health.queue_high_watermark,
        "queue_saturated":health.queue_saturated})
}

fn frontend_workspace(report: &workspace::Report) -> dashboard::Workspace {
    dashboard::Workspace {
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
            .map(|branch| dashboard::Branch {
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
    fn health_output_contains_only_allowlisted_non_secret_fields() {
        let output = health_json(
            "running",
            &DaemonHealth {
                healthy: true,
                process_id: 42,
                store_schema_version: 1,
                queue_capacity: 64,
                queue_depth: 3,
                queue_high_watermark: 7,
                queue_saturated: false,
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
                "queue_depth",
                "queue_high_watermark",
                "queue_saturated",
                "status",
                "store_schema_version",
            ])
        );
        let encoded = output.to_string();
        for forbidden in ["token", "endpoint", "database", "project_root", "prompt"] {
            assert!(!encoded.contains(forbidden));
        }
    }

    #[test]
    fn harness_rows_contain_no_secret_fields() {
        // The harness listing is machine-readable context; it must never
        // leak tokens, endpoints, full environments, or database paths.
        let rows = vec![vibemux_harness::HarnessRow {
            name: "claude".to_string(),
            command: vec!["claude".to_string()],
            protocol: vibemux_harness::HarnessProtocol::Pty,
            provider: "anthropic".to_string(),
            available: true,
            path: Some("claude".to_string()),
            roles: vec!["worker".to_string()],
            default: true,
            launcher: Some(vibemux_probe::LauncherKind::DirectExecutable),
            version: Some("1.0.0".to_string()),
        }];
        let encoded = serde_json::to_string(&rows).expect("rows JSON");
        for forbidden in [
            "token",
            "endpoint",
            "database_path",
            "env",
            "api_key",
            "secret",
        ] {
            assert!(
                !encoded.contains(forbidden),
                "harness rows leaked {forbidden}"
            );
        }
    }

    #[test]
    fn control_errors_keep_the_actionable_human_message() {
        let error = DaemonCliError::UnknownHarness("bogus-harness".to_string());
        let failure = Failure::with_message(error.code(), error.to_string(), error.clone());
        let value: Value =
            serde_json::from_str(&crate::output::error_text(&failure, Format::Json)).unwrap();
        assert_eq!(value["ok"], Value::Bool(false));
        assert_eq!(value["error_code"], "store_unknown_harness");
        assert!(
            value["error"]
                .as_str()
                .is_some_and(|message| message.contains("bogus-harness")),
            "error message must name the harness"
        );

        let error =
            DaemonCliError::HarnessProbeCacheMissing("C:\\tmp\\.vibemux\\probe_cache.json".into());
        let failure = Failure::with_message(error.code(), error.to_string(), error.clone());
        let value: Value =
            serde_json::from_str(&crate::output::error_text(&failure, Format::Json)).unwrap();
        assert_eq!(value["error_code"], "harness_probe_cache_missing");
        let message = value["error"].as_str().expect("error message present");
        assert!(message.contains("--write-cache --project-root <root>"));
        assert!(message.contains("C:\\tmp\\.vibemux\\probe_cache.json"));
    }
}
