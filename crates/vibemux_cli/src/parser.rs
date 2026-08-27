use crate::DaemonCliError;
use std::{ffi::OsString, path::PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CliCommand {
    DaemonStart {
        project_root: Option<PathBuf>,
        daemon_executable: Option<PathBuf>,
    },
    DaemonHealth {
        project_root: Option<PathBuf>,
    },
    DaemonStop {
        project_root: Option<PathBuf>,
    },
    DaemonInspect {
        project_root: Option<PathBuf>,
    },
    DaemonRecover {
        project_root: Option<PathBuf>,
        confirmation: String,
    },
    Help,
    Version,
}

pub fn parse_cli_arguments(
    arguments: impl Iterator<Item = OsString>,
) -> Result<CliCommand, DaemonCliError> {
    let arguments = arguments.collect::<Vec<_>>();
    if arguments.len() == 1 && arguments[0] == "--help" {
        return Ok(CliCommand::Help);
    }
    if arguments.len() == 1 && arguments[0] == "--version" {
        return Ok(CliCommand::Version);
    }
    if arguments.len() < 2 || arguments[0] != "daemon" {
        return Err(DaemonCliError::InvalidArguments);
    }
    let action = arguments[1]
        .to_str()
        .ok_or(DaemonCliError::InvalidArguments)?;
    let mut project_root = None;
    let mut daemon_executable = None;
    let mut confirmation = None;
    let mut index = 2;
    while index < arguments.len() {
        let flag = arguments[index]
            .to_str()
            .ok_or(DaemonCliError::InvalidArguments)?;
        let value = arguments
            .get(index + 1)
            .ok_or(DaemonCliError::InvalidArguments)?;
        match flag {
            "--project-root" if project_root.is_none() => {
                project_root = Some(PathBuf::from(value));
            }
            "--daemon-executable" if action == "start" && daemon_executable.is_none() => {
                daemon_executable = Some(PathBuf::from(value));
            }
            "--confirmation" if action == "recover" && confirmation.is_none() => {
                confirmation = Some(
                    value
                        .to_str()
                        .ok_or(DaemonCliError::InvalidArguments)?
                        .to_string(),
                );
            }
            _ => return Err(DaemonCliError::InvalidArguments),
        }
        index += 2;
    }
    match action {
        "start" => Ok(CliCommand::DaemonStart {
            project_root,
            daemon_executable,
        }),
        "health" if daemon_executable.is_none() => Ok(CliCommand::DaemonHealth { project_root }),
        "stop" if daemon_executable.is_none() => Ok(CliCommand::DaemonStop { project_root }),
        "inspect" if daemon_executable.is_none() && confirmation.is_none() => {
            Ok(CliCommand::DaemonInspect { project_root })
        }
        "recover" if daemon_executable.is_none() => Ok(CliCommand::DaemonRecover {
            project_root,
            confirmation: confirmation.ok_or(DaemonCliError::InvalidArguments)?,
        }),
        _ => Err(DaemonCliError::InvalidArguments),
    }
}
