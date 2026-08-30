use std::{ffi::OsString, path::PathBuf};

use vibemuxd::{
    control::DaemonControlServer,
    plugin_configuration::PluginStartup,
    process::{DaemonPathError, DaemonPaths},
    supervisor_service::SupervisorServiceConfig,
};

const EXIT_USAGE: i32 = 2;
const EXIT_RUNTIME: i32 = 4;
const PROJECT_ROOT_ENV: &str = "VIBEMUX_DAEMON_PROJECT_ROOT";

enum DaemonArguments {
    Run {
        project_root: PathBuf,
        plugin_config: Option<PathBuf>,
        supervisor_config: Option<PathBuf>,
    },
    Help,
    Version,
}

#[tokio::main]
async fn main() {
    let exit_code = match parse_arguments(
        std::env::args_os().skip(1),
        std::env::var_os(PROJECT_ROOT_ENV),
    ) {
        Ok(DaemonArguments::Help) => {
            print_help();
            0
        }
        Ok(DaemonArguments::Version) => {
            println!(env!("CARGO_PKG_VERSION"));
            0
        }
        Ok(DaemonArguments::Run {
            project_root,
            plugin_config,
            supervisor_config,
        }) => match run_daemon(project_root, plugin_config, supervisor_config).await {
            Ok(()) => 0,
            Err(code) => {
                emit_error(&code);
                EXIT_RUNTIME
            }
        },
        Err(code) => {
            emit_error(code);
            EXIT_USAGE
        }
    };
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

async fn run_daemon(
    project_root: PathBuf,
    plugin_config: Option<PathBuf>,
    supervisor_config: Option<PathBuf>,
) -> Result<(), String> {
    let supervisor = tokio::task::spawn_blocking(move || {
        supervisor_config
            .map(|path| SupervisorServiceConfig::from_path(&path))
            .transpose()
    })
    .await
    .map_err(|_| "supervisor_configuration_invalid".to_string())?
    .map_err(str::to_string)?;
    let plugins = tokio::task::spawn_blocking(move || match plugin_config {
        Some(path) => PluginStartup::from_path(&path),
        None => Ok(PluginStartup::default()),
    })
    .await
    .map_err(|_| "plugin_configuration_invalid".to_string())?
    .map_err(str::to_string)?;
    let paths = DaemonPaths::from_project_root(&project_root)
        .map_err(|error| path_error_code(error).to_string())?;
    paths
        .ensure_runtime_dir()
        .map_err(|error| path_error_code(error).to_string())?;
    paths
        .validate_daemon_start()
        .map_err(|error| path_error_code(error).to_string())?;
    let server = match supervisor {
        Some(config) => {
            DaemonControlServer::start_for_paths_with_supervisor(&paths, plugins, config).await
        }
        None => DaemonControlServer::start_for_paths_with_plugins(&paths, plugins).await,
    }
    .map_err(|error| error.code().to_string())?;
    if let Some(base_url) = server.a2a_base_url() {
        println!(
            "{}",
            serde_json::json!({"a2a_base_url":base_url,"a2a_grpc_base_url":server.a2a_grpc_base_url(),"process_id":std::process::id()})
        );
    }
    server
        .wait()
        .await
        .map_err(|error| error.code().to_string())
}

fn parse_arguments(
    arguments: impl Iterator<Item = OsString>,
    environment_project_root: Option<OsString>,
) -> Result<DaemonArguments, &'static str> {
    let arguments = arguments.collect::<Vec<_>>();
    if arguments.is_empty() {
        return environment_project_root
            .map(PathBuf::from)
            .map(|project_root| DaemonArguments::Run {
                project_root,
                plugin_config: None,
                supervisor_config: None,
            })
            .ok_or("daemon_invalid_arguments");
    }
    if arguments.len() == 1 && arguments[0] == "--help" {
        return Ok(DaemonArguments::Help);
    }
    if arguments.len() == 1 && arguments[0] == "--version" {
        return Ok(DaemonArguments::Version);
    }
    if !matches!(arguments.len(), 2 | 4 | 6) || arguments[0] != "--project-root" {
        return Err("daemon_invalid_arguments");
    }
    let mut plugin_config = None;
    let mut supervisor_config = None;
    for pair in arguments[2..].chunks_exact(2) {
        if pair[0] == "--plugin-config" && plugin_config.is_none() {
            plugin_config = Some(PathBuf::from(&pair[1]));
        } else if pair[0] == "--supervisor-config" && supervisor_config.is_none() {
            supervisor_config = Some(PathBuf::from(&pair[1]));
        } else {
            return Err("daemon_invalid_arguments");
        }
    }
    Ok(DaemonArguments::Run {
        project_root: PathBuf::from(&arguments[1]),
        plugin_config,
        supervisor_config,
    })
}

fn path_error_code(error: DaemonPathError) -> &'static str {
    error.code()
}

fn emit_error(code: &str) {
    eprintln!("{{\"ok\":false,\"error_code\":\"{code}\"}}");
}

fn print_help() {
    println!(
        "Usage: vibemuxd --project-root <path> [--plugin-config <path>] [--supervisor-config <path>]"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_requires_one_explicit_project_root() {
        assert!(matches!(
            parse_arguments(
                [OsString::from("--project-root"), OsString::from("C:\\repo")].into_iter(),
                None,
            ),
            Ok(DaemonArguments::Run { .. })
        ));
        assert!(parse_arguments([OsString::from("--project-root")].into_iter(), None).is_err());
        assert!(parse_arguments([OsString::from("C:\\repo")].into_iter(), None).is_err());
        assert!(matches!(
            parse_arguments(
                std::iter::empty(),
                Some(OsString::from("C:\\environment repo"))
            ),
            Ok(DaemonArguments::Run { .. })
        ));
    }
}
