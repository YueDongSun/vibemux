use std::{ffi::OsString, path::PathBuf};

use vibemuxd::{
    control::DaemonControlServer,
    process::{DaemonPathError, DaemonPaths},
};

const EXIT_USAGE: i32 = 2;
const EXIT_RUNTIME: i32 = 4;
const PROJECT_ROOT_ENV: &str = "VIBEMUX_DAEMON_PROJECT_ROOT";

enum DaemonArguments {
    Run { project_root: PathBuf },
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
        Ok(DaemonArguments::Run { project_root }) => match run_daemon(project_root).await {
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

async fn run_daemon(project_root: PathBuf) -> Result<(), String> {
    let paths = DaemonPaths::from_project_root(&project_root)
        .map_err(|error| path_error_code(error).to_string())?;
    paths
        .ensure_runtime_dir()
        .map_err(|error| path_error_code(error).to_string())?;
    paths
        .validate_daemon_start()
        .map_err(|error| path_error_code(error).to_string())?;
    let server = DaemonControlServer::start_for_paths(&paths)
        .await
        .map_err(|error| error.code().to_string())?;
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
            .map(|project_root| DaemonArguments::Run { project_root })
            .ok_or("daemon_invalid_arguments");
    }
    if arguments.len() == 1 && arguments[0] == "--help" {
        return Ok(DaemonArguments::Help);
    }
    if arguments.len() == 1 && arguments[0] == "--version" {
        return Ok(DaemonArguments::Version);
    }
    if arguments.len() != 2 || arguments[0] != "--project-root" {
        return Err("daemon_invalid_arguments");
    }
    Ok(DaemonArguments::Run {
        project_root: PathBuf::from(&arguments[1]),
    })
}

fn path_error_code(error: DaemonPathError) -> &'static str {
    error.code()
}

fn emit_error(code: &str) {
    eprintln!("{{\"ok\":false,\"error_code\":\"{code}\"}}");
}

fn print_help() {
    println!("Usage: vibemuxd --project-root <path>");
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
