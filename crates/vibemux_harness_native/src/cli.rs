//! Reusable command boundary for standalone and unified CLI frontends.
use crate::{
    Error,
    config::{Config, Limits, MAX_CAPTURE_BYTES},
    contract::{Harness, MAX_REQUEST_BYTES, Protocol, Request, Status},
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
                let bytes = read_bounded(&request_path, MAX_REQUEST_BYTES)?;
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
    use crate::contract::MAX_PROMPT_BYTES;
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

    fn parsed(arguments: &[&str]) -> Result<Command, &'static str> {
        parse_arguments(arguments.iter().map(OsString::from)).map_err(|error| error.code())
    }

    /// Help takes no options. A command needs each of its required options
    /// exactly once with a non-empty value, accepts no other option, and
    /// names protocols and harnesses by their exact lowercase ids.
    #[test]
    fn parser_accepts_only_complete_well_formed_commands() {
        for arguments in [
            &[][..],
            &["help"],
            &["--help"],
            &["-h"],
            &["catalog", "--help"],
            &["dispatch", "-h"],
        ] {
            assert!(
                matches!(parsed(arguments), Ok(Command::Help)),
                "{arguments:?}"
            );
        }
        assert!(matches!(parsed(&["--version"]), Ok(Command::Version)));
        for arguments in [
            &["shell"][..],
            &["--version", "catalog"],
            &["help", "catalog"],
            &["catalog", "--config"],
            &["catalog", "--config", ""],
            &["catalog", "--config", "a", "extra"],
            &["catalog", "--harness", "codex"],
            &["replay", "--protocol", "codex_exec"],
            &["replay", "--protocol", "Codex_Exec", "--input", "a"],
            &["replay", "--protocol", "codex", "--input", "a"],
            &["probe", "--config", "a"],
            &["probe", "--config", "a", "--harness", "Codex"],
            &["dispatch", "--config", "a"],
            &[
                "dispatch",
                "--config",
                "a",
                "--request",
                "b",
                "--harness",
                "codex",
            ],
        ] {
            assert_eq!(
                parsed(arguments).err(),
                Some("harness_cli_usage"),
                "{arguments:?}"
            );
        }
        let Ok(Command::Replay {
            protocol,
            input_path,
        }) = parsed(&["replay", "--input", "in.jsonl", "--protocol", "acp"])
        else {
            panic!("replay");
        };
        assert_eq!(
            (protocol, input_path),
            (Protocol::Acp, PathBuf::from("in.jsonl"))
        );
        let Ok(Command::Probe {
            config_path,
            harness,
            plugin,
        }) = parsed(&["probe", "--harness", "grok", "--config", "c.json"])
        else {
            panic!("probe");
        };
        assert_eq!(
            (config_path, harness, plugin),
            (PathBuf::from("c.json"), Harness::Grok, None)
        );
        let Ok(Command::Dispatch {
            config_path,
            request_path,
            plugin,
        }) = parsed(&[
            "dispatch",
            "--plugin",
            "p.exe",
            "--request",
            "r.json",
            "--config",
            "c.json",
        ])
        else {
            panic!("dispatch");
        };
        assert_eq!(
            (config_path, request_path, plugin),
            (
                PathBuf::from("c.json"),
                PathBuf::from("r.json"),
                Some(PathBuf::from("p.exe"))
            )
        );
    }

    #[cfg(windows)]
    fn non_unicode() -> OsString {
        use std::os::windows::ffi::OsStringExt;
        OsString::from_wide(&[0x61, 0xD800])
    }

    #[cfg(unix)]
    fn non_unicode() -> OsString {
        use std::os::unix::ffi::OsStringExt;
        OsString::from_vec(vec![b'a', 0xff])
    }

    /// A path may be any operating system string, but a command, an option
    /// name, or a fixed id that is not Unicode is a usage error.
    #[test]
    fn parser_keeps_non_unicode_paths_and_refuses_non_unicode_names() {
        let parse =
            |arguments: Vec<OsString>| parse_arguments(arguments).map_err(|error| error.code());
        let strings = |values: &[&str]| values.iter().map(OsString::from).collect::<Vec<_>>();
        let catalog = [strings(&["catalog", "--config"]), vec![non_unicode()]].concat();
        let Ok(Command::Catalog { config_path }) = parse(catalog) else {
            panic!("catalog");
        };
        assert_eq!(config_path.as_os_str(), non_unicode());
        for arguments in [
            vec![non_unicode()],
            [strings(&["catalog"]), vec![non_unicode(), "a".into()]].concat(),
            [
                strings(&["probe", "--config", "a", "--harness"]),
                vec![non_unicode()],
            ]
            .concat(),
        ] {
            assert_eq!(parse(arguments).err(), Some("harness_cli_usage"));
        }
    }

    /// Input files are read up to a fixed bound plus one byte. A dispatch
    /// request is parsed and validated before its configuration is opened,
    /// and its bound fits a maximum prompt written entirely in six-byte
    /// `\u00XX` escapes.
    #[tokio::test]
    async fn dispatch_requests_are_bounded_and_validated_before_the_configuration() {
        let directory = tempfile::tempdir().unwrap();
        let file = |name: &str, bytes: &[u8]| {
            let path = directory.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        };
        let read = |path: &Path| {
            read_bounded(path, 16)
                .map(|bytes| bytes.len())
                .map_err(|error| error.code())
        };
        assert_eq!(read(&file("exact", &[b'a'; 16])), Ok(16));
        assert_eq!(
            read(&file("over", &[b'a'; 17])),
            Err("harness_input_too_large")
        );
        assert_eq!(
            read(&directory.path().join("missing")),
            Err("harness_input_unavailable")
        );
        let request = |prompt: &str, request_id| {
            serde_json::to_vec(&Request {
                schema_version: 1,
                request_id,
                harness: Harness::Codex,
                prompt: prompt.to_string(),
            })
            .unwrap()
        };
        let escaped = "\u{1}".repeat(MAX_PROMPT_BYTES);
        let escaped_request = request(&escaped, uuid::Uuid::new_v4());
        assert!(escaped_request.len() > MAX_PROMPT_BYTES * 6);
        let cases = [
            (b"{".to_vec(), "harness_invalid_request"),
            (
                request("hello", uuid::Uuid::nil()),
                "harness_invalid_request",
            ),
            (request(" ", uuid::Uuid::new_v4()), "harness_invalid_prompt"),
            (vec![b' '; MAX_REQUEST_BYTES + 1], "harness_input_too_large"),
            // Only a valid request reaches the missing configuration.
            (escaped_request, "harness_config_unavailable"),
        ];
        for (index, (bytes, code)) in cases.into_iter().enumerate() {
            let outcome = run(Command::Dispatch {
                config_path: directory.path().join("missing_config.json"),
                request_path: file(&format!("request_{index}.json"), &bytes),
                plugin: Some(directory.path().join("missing_plugin.exe")),
            })
            .await;
            assert_eq!(
                outcome.err().map(|error| error.code()),
                Some(code),
                "{index}"
            );
        }
        let guide = include_str!("../../../docs/guides/harness_core_guide.md");
        assert!(guide.contains("six-byte"));
    }
}
