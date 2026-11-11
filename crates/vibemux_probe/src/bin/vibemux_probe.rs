//! Read-only probe entry point. With --write-cache the private report is
//! additionally persisted atomically before the JSON is printed.

use std::{path::PathBuf, process::exit};

use vibemux_probe::{
    AgentKind, ProbeConfig, cache, report_json, run_probe, version_only::run_version_probe,
};

const USAGE: &str =
    "usage: vibemux_probe [--version-only --harness <name>] [--write-cache] [--project-root <dir>]";

#[tokio::main]
async fn main() {
    let mut write_cache = false;
    let mut version_only = false;
    let mut harness = None;
    let mut project_root = PathBuf::from(".");
    let mut arguments = std::env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--help" | "-h") => {
                println!("{USAGE}");
                return;
            }
            Some("--write-cache") => write_cache = true,
            Some("--version-only") => version_only = true,
            Some("--harness") if harness.is_none() => {
                let name = arguments.next().unwrap_or_default();
                harness = AgentKind::all()
                    .into_iter()
                    .find(|agent| name == agent.command_name());
                if harness.is_none() {
                    usage_error("unknown or missing harness");
                }
            }
            Some("--project-root") => {
                let Some(value) = arguments
                    .next()
                    .filter(|value| !value.to_str().is_some_and(|value| value.starts_with('-')))
                else {
                    usage_error("--project-root requires a directory argument");
                };
                project_root = PathBuf::from(value);
            }
            _ => usage_error("unknown or malformed argument"),
        }
    }
    if version_only != harness.is_some() {
        usage_error("--version-only and --harness must be used together");
    }

    let config = ProbeConfig::from_environment();
    let path = cache::default_cache_path(&project_root);
    let encoded = if let Some(agent) = harness {
        let report = run_version_probe(&config, agent).await;
        let encoded = serde_json::to_string_pretty(&report).unwrap_or_else(|error| fail(&error));
        if write_cache {
            cache::write_version_cache(&report, &path).unwrap_or_else(|error| fail(&error));
        }
        encoded
    } else {
        let report = run_probe(&config).await;
        let encoded = report_json(&report).unwrap_or_else(|error| fail(&error));
        if write_cache {
            cache::write_cache(&report, &path).unwrap_or_else(|error| fail(&error));
        }
        encoded
    };
    println!("{encoded}");
}

fn usage_error(message: &str) -> ! {
    eprintln!("vibemux_probe: {message}");
    eprintln!("{USAGE}");
    exit(4)
}

fn fail(error: &dyn std::fmt::Display) -> ! {
    eprintln!("vibemux_probe: {error}");
    exit(4)
}
