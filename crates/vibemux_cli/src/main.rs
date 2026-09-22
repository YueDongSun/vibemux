use serde_json::json;
use vibemux_cli::{
    CliCommand, DaemonBootstrapConfig, DaemonCliError, DaemonStartOutcome, daemon_executable,
    daemon_health, daemon_paths, harness_list, harness_switch, parse_cli_arguments,
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
        CliCommand::Harnesses {
            project_root,
            cached,
        } => {
            let paths = daemon_paths(project_root.as_deref())?;
            let rows = harness_list(&paths, cached).await?;
            Ok(json!({
                "ok": true,
                "harnesses": rows,
            }))
        }
        CliCommand::Switch {
            project_root,
            harness,
        } => {
            let paths = daemon_paths(project_root.as_deref())?;
            let (from, to) = harness_switch(&paths, &harness).await?;
            Ok(json!({
                "ok": true,
                "default_harness": to,
                "from": from,
                "to": to,
            }))
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
        "queue_depth": health.queue_depth,
        "queue_high_watermark": health.queue_high_watermark,
        "queue_saturated": health.queue_saturated,
    })
}

fn emit_error(error: &DaemonCliError) {
    eprintln!("{}", error_json(error));
}

fn error_json(error: &DaemonCliError) -> serde_json::Value {
    json!({"ok": false, "error_code": error.code(), "error": error.to_string()})
}

fn print_help() {
    println!(
        "Usage:\n  vibemuxctl daemon start [--project-root <path>] [--daemon-executable <path>]\n  vibemuxctl daemon health [--project-root <path>]\n  vibemuxctl daemon stop [--project-root <path>]\n  vibemuxctl daemon inspect [--project-root <path>]\n  vibemuxctl daemon recover --confirmation <sha256> [--project-root <path>]\n  vibemuxctl harnesses [--cached] [--project-root <path>]\n  vibemuxctl switch <harness> [--project-root <path>]"
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
                queue_depth: 3,
                queue_high_watermark: 7,
                queue_saturated: false,
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
    fn emitted_error_json_includes_the_actionable_human_message() {
        let error = DaemonCliError::UnknownHarness("bogus-harness".to_string());
        let value = error_json(&error);
        assert_eq!(value["ok"], serde_json::Value::Bool(false));
        assert_eq!(value["error_code"], "store_unknown_harness");
        assert!(
            value["error"]
                .as_str()
                .is_some_and(|message| message.contains("bogus-harness")),
            "error message must name the harness"
        );

        let error =
            DaemonCliError::HarnessProbeCacheMissing("C:\\tmp\\.vibemux\\probe_cache.json".into());
        let value = error_json(&error);
        assert_eq!(value["error_code"], "harness_probe_cache_missing");
        let message = value["error"].as_str().expect("error message present");
        assert!(message.contains("--write-cache --project-root <root>"));
        assert!(message.contains("C:\\tmp\\.vibemux\\probe_cache.json"));
    }
}
