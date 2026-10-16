//! Reusable command boundary for standalone and unified CLI frontends.
use crate::{
    Error,
    config::{Config, Limits, MAX_CAPTURE_BYTES},
    contract::{Harness, MAX_PROMPT_BYTES, Protocol, Request, Status},
    dispatch::Dispatcher,
    json_lines,
};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fmt,
    io::Read,
    path::{Path, PathBuf},
};
use tokio::{io::BufReader, sync::watch};

#[derive(Clone)]
pub enum Command {
    Help,
    Version,
    Catalog {
        config_path: PathBuf,
    },
    Replay {
        protocol: Protocol,
        input_path: PathBuf,
    },
    Probe {
        config_path: PathBuf,
        harness: Harness,
        plugin: Option<PathBuf>,
    },
    Dispatch {
        config_path: PathBuf,
        request_path: PathBuf,
        plugin: Option<PathBuf>,
    },
}
impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Help => "help",
            Self::Version => "version",
            Self::Catalog { .. } => "catalog",
            Self::Replay { .. } => "replay",
            Self::Probe { .. } => "probe",
            Self::Dispatch { .. } => "dispatch",
        }
    }
}
impl fmt::Debug for Command {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Command")
            .field("name", &self.name())
            .finish_non_exhaustive()
    }
}
pub struct Reply {
    pub body: serde_json::Value,
    pub success: bool,
}
pub fn help(program: &str) -> String {
    format!(
        "VibeMux structured harness commands\n\nUsage:\n  {program} catalog --config <file>\n  {program} replay --protocol <name> --input <jsonl_file>\n  {program} probe --config <file> --harness <name> [--plugin <executable>]\n  {program} dispatch --config <file> --request <json_file> [--plugin <executable>]\n\nProtocols: codex_app_server, codex_exec, claude_stream_json, acp,\n           opencode_json, copilot_json, grok_stream_json.\nHarnesses: codex, claude, opencode, copilot, grok.\n\nReports omit raw content and local configuration. Execution requires allow_execution.\nProbe initializes a protocol connection only; it never submits a model prompt.\n"
    )
}
pub fn parse_arguments(arguments: impl IntoIterator<Item = OsString>) -> Result<Command, Error> {
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    if arguments.is_empty()
        || (arguments.len() == 1 && matches!(arguments[0].to_str(), Some("help" | "--help" | "-h")))
    {
        return Ok(Command::Help);
    }
    if arguments.len() == 1 && arguments[0] == "--version" {
        return Ok(Command::Version);
    }
    let command = arguments[0].to_str().ok_or_else(usage)?;
    if !matches!(command, "catalog" | "replay" | "probe" | "dispatch") {
        return Err(usage());
    }
    if arguments.len() == 2 && matches!(arguments[1].to_str(), Some("--help" | "-h")) {
        return Ok(Command::Help);
    }
    if arguments.len() % 2 != 1 || arguments.len() > 9 {
        return Err(usage());
    }
    let allowed: &[&str] = match command {
        "catalog" => &["--config"],
        "replay" => &["--protocol", "--input"],
        "probe" => &["--config", "--harness", "--plugin"],
        _ => &["--config", "--request", "--plugin"],
    };
    let mut options = BTreeMap::<String, OsString>::new();
    for pair in arguments[1..].chunks_exact(2) {
        let name = pair[0].to_str().ok_or_else(usage)?;
        if !allowed.contains(&name)
            || pair[1].is_empty()
            || options.insert(name.to_string(), pair[1].clone()).is_some()
        {
            return Err(usage());
        }
    }
    let required = |name: &str| options.get(name).ok_or_else(usage);
    Ok(match command {
        "catalog" => Command::Catalog {
            config_path: PathBuf::from(required("--config")?),
        },
        "replay" => Command::Replay {
            protocol: parse_enum(required("--protocol")?)?,
            input_path: PathBuf::from(required("--input")?),
        },
        "probe" => Command::Probe {
            config_path: PathBuf::from(required("--config")?),
            harness: parse_enum(required("--harness")?)?,
            plugin: options.get("--plugin").map(PathBuf::from),
        },
        _ => Command::Dispatch {
            config_path: PathBuf::from(required("--config")?),
            request_path: PathBuf::from(required("--request")?),
            plugin: options.get("--plugin").map(PathBuf::from),
        },
    })
}
pub async fn run(command: Command) -> Result<Reply, Error> {
    match command {
        Command::Help | Command::Version => Err(usage()),
        Command::Catalog { config_path } => {
            let body =
                blocking(move || Config::load(&config_path).map(|config| config.catalog())).await?;
            Ok(Reply {
                body,
                success: true,
            })
        }
        Command::Replay {
            protocol,
            input_path,
        } => {
            let bytes = blocking(move || read_bounded(&input_path, MAX_CAPTURE_BYTES)).await?;
            let mut input = BufReader::new(bytes.as_slice());
            let records = json_lines::replay(&mut input, protocol, &Limits::default()).await?;
            Ok(Reply {
                body: serde_json::json!({"schema_version":1,"status":"replayed","protocol":protocol,
                "record_count":records.len(),"observations":records.iter().map(|record| record.summary()).collect::<Vec<_>>()}),
                success: true,
            })
        }
        Command::Probe {
            config_path,
            harness,
            plugin,
        } => {
            let dispatcher = blocking(move || dispatcher(&config_path, plugin)).await?;
            let outcome = dispatcher.probe(harness).await?;
            Ok(Reply {
                success: outcome.completion.status == Status::Probed,
                body: outcome.summary(),
            })
        }
        Command::Dispatch {
            config_path,
            request_path,
            plugin,
        } => {
            let (dispatcher, request) = blocking(move || {
                let bytes = read_bounded(&request_path, MAX_PROMPT_BYTES * 6 + 4096)?;
                let request: Request = serde_json::from_slice(&bytes)
                    .map_err(|source| Error::with_source("harness_invalid_request", source))?;
                request.validate()?;
                Ok((dispatcher(&config_path, plugin)?, request))
            })
            .await?;
            let (_sender, receiver) = watch::channel(false);
            let outcome = dispatcher.execute(request, receiver).await?;
            Ok(Reply {
                success: outcome.completion.status == Status::Completed,
                body: outcome.summary(),
            })
        }
    }
}
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|source| Error::with_source("harness_worker_failed", source))?
}
fn dispatcher(config_path: &Path, plugin: Option<PathBuf>) -> Result<Dispatcher, Error> {
    let plugin = match plugin {
        Some(plugin) => plugin,
        None => std::env::current_exe()
            .map_err(|source| Error::with_source("harness_executable_unavailable", source))?
            .with_file_name(if cfg!(windows) {
                "vibemux_harness_native_plugin.exe"
            } else {
                "vibemux_harness_native_plugin"
            }),
    };
    Dispatcher::new(config_path, &plugin)
}
fn read_bounded(path: &Path, maximum: usize) -> Result<Vec<u8>, Error> {
    let file = std::fs::File::open(path)
        .map_err(|source| Error::with_source("harness_input_unavailable", source))?;
    let mut bytes = Vec::new();
    file.take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| Error::with_source("harness_input_unavailable", source))?;
    if bytes.len() > maximum {
        return Err(Error::new("harness_input_too_large"));
    }
    Ok(bytes)
}
fn parse_enum<T: serde::de::DeserializeOwned>(value: &OsString) -> Result<T, Error> {
    serde_json::from_value(serde_json::Value::String(
        value.to_str().ok_or_else(usage)?.to_string(),
    ))
    .map_err(|source| Error::with_source("harness_cli_usage", source))
}
fn usage() -> Error {
    Error::new("harness_cli_usage")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parser_preserves_paths_without_exposing_them_in_debug() {
        let command = parse_arguments(
            ["catalog", "--config", "private_config ; & path.json"].map(OsString::from),
        )
        .unwrap();
        assert!(!format!("{command:?}").contains("private_config"));
        let Command::Catalog { config_path } = command else {
            panic!("catalog");
        };
        assert_eq!(config_path, PathBuf::from("private_config ; & path.json"));
    }
    #[test]
    fn parser_rejects_unknown_duplicate_and_missing_options() {
        for arguments in [
            vec!["catalog"],
            vec!["catalog", "--config", "a", "--config", "b"],
            vec!["probe", "--config", "a", "--harness", "unknown"],
            vec!["dispatch", "--shell", "data"],
        ] {
            assert_eq!(
                parse_arguments(arguments.into_iter().map(OsString::from))
                    .unwrap_err()
                    .code(),
                "harness_cli_usage"
            );
        }
    }
}
