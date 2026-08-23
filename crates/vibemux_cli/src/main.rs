use serde_json::json;
use vibemux_cli::{
    CliCommand, DaemonBootstrapConfig, DaemonCliError, DaemonStartOutcome, daemon_executable,
    daemon_health, daemon_paths, parse_cli_arguments,
    recovery::{inspect_runtime, recover_runtime},
    start_daemon, stop_daemon,
};
use vibemuxd::control::DaemonHealth;

const EXIT_USAGE: i32 = 2;
const EXIT_RUNTIME: i32 = 4;

#[tokio::main]
async fn main() {
    let result = match parse_cli_arguments(std::env::args_os().skip(1)) {
        Ok(CliCommand::Help) => {
            print_help();
            return;
        }
        Ok(CliCommand::Version) => {
            println!(env!("CARGO_PKG_VERSION"));
            return;
        }
        Ok(command) => run(command).await,
        Err(error) => {
            emit_error(&error);
            std::process::exit(EXIT_USAGE);
        }
    };
    match result {
        Ok(output) => println!("{output}"),
        Err(error) => {
            emit_error(&error);
            std::process::exit(EXIT_RUNTIME);
        }
    }
}

async fn run(command: CliCommand) -> Result<serde_json::Value, DaemonCliError> {
    match command {
        CliCommand::DaemonStart {
            project_root,
            daemon_executable: explicit_executable,
        } => {
            let paths = daemon_paths(project_root.as_deref())?;
            let executable = daemon_executable(explicit_executable.as_deref())?;
            let outcome = start_daemon(&DaemonBootstrapConfig::new(paths, executable)).await?;
            let status = match &outcome {
                DaemonStartOutcome::Started(_) => "started",
                DaemonStartOutcome::AlreadyRunning(_) => "already_running",
            };
            Ok(health_json(status, outcome.health()))
        }
        CliCommand::DaemonHealth { project_root } => {
            let paths = daemon_paths(project_root.as_deref())?;
            let health = daemon_health(&paths).await?;
            Ok(health_json("running", &health))
        }
        CliCommand::DaemonStop { project_root } => {
            let paths = daemon_paths(project_root.as_deref())?;
            let health = stop_daemon(&paths).await?;
            Ok(json!({
                "ok": true,
                "status": "stopped",
                "process_id": health.process_id,
            }))
        }
        CliCommand::DaemonInspect { project_root } => {
            let paths = daemon_paths(project_root.as_deref())?;
            let inspection = inspect_runtime(&paths).await?;
            serde_json::to_value(inspection).map_err(|_| DaemonCliError::Control {
                code: "cli_output_encoding_failed".to_string(),
            })
        }
        CliCommand::DaemonRecover {
            project_root,
            confirmation,
        } => {
            let paths = daemon_paths(project_root.as_deref())?;
            let outcome = recover_runtime(&paths, &confirmation).await?;
            serde_json::to_value(outcome).map_err(|_| DaemonCliError::Control {
                code: "cli_output_encoding_failed".to_string(),
            })
        }
        CliCommand::Help | CliCommand::Version => Err(DaemonCliError::InvalidArguments),
    }
}

fn health_json(status: &str, health: &DaemonHealth) -> serde_json::Value {
    json!({
        "ok": true,
        "status": status,
        "healthy": health.healthy,
        "process_id": health.process_id,
        "store_schema_version": health.store_schema_version,
        "queue_capacity": health.queue_capacity,
    })
}

fn emit_error(error: &DaemonCliError) {
    eprintln!("{}", json!({"ok": false, "error_code": error.code()}));
}

fn print_help() {
    println!(
        "Usage:\n  vibemuxctl daemon start [--project-root <path>] [--daemon-executable <path>]\n  vibemuxctl daemon health [--project-root <path>]\n  vibemuxctl daemon stop [--project-root <path>]\n  vibemuxctl daemon inspect [--project-root <path>]\n  vibemuxctl daemon recover --confirmation <sha256> [--project-root <path>]"
    );
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
            },
        );
        let keys = output
            .as_object()
            .expect("health output object")
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
                "store_schema_version",
            ])
        );
        let encoded = output.to_string();
        for forbidden in ["token", "endpoint", "database", "project_root", "prompt"] {
            assert!(!encoded.contains(forbidden));
        }
    }
}
