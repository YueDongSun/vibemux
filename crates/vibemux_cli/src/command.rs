//! One command tree; legacy daemon and harness parsers remain their boundary owners.
use crate::{CliCommand, failure::Failure, parse_cli_arguments};
use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

pub const DEFAULT_WIDTH: u16 = 100;
pub const DEFAULT_HEIGHT: u16 = 28;
const MAX_ARGUMENTS: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Format {
    Auto,
    Text,
    Json,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Topic {
    Root,
    Daemon,
    Harness,
    Workspace,
    Probe,
    Ui,
}

pub enum Command {
    Help(Topic),
    Version,
    Daemon(CliCommand),
    Harness(vibemux_harness_native::cli::Command),
    Probe {
        report_path: Option<PathBuf>,
    },
    Workspace {
        project_root: Option<PathBuf>,
    },
    Status {
        project_root: Option<PathBuf>,
    },
    Ui {
        project_root: Option<PathBuf>,
        report_path: Option<PathBuf>,
        once: bool,
        width: u16,
        height: u16,
    },
}

pub struct Invocation {
    pub format: Format,
    pub command: Command,
}

pub fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Invocation, Failure> {
    let arguments = arguments
        .into_iter()
        .take(MAX_ARGUMENTS + 1)
        .collect::<Vec<_>>();
    if arguments.len() > MAX_ARGUMENTS || arguments.iter().any(|argument| argument.len() > 8192) {
        return Err(usage());
    }
    let mut format = Format::Auto;
    let mut start = 0;
    if arguments
        .first()
        .is_some_and(|argument| argument == "--json")
    {
        format = Format::Json;
        start = 1;
    } else if arguments
        .first()
        .is_some_and(|argument| argument == "--format")
    {
        format = match arguments.get(1).and_then(|value| value.to_str()) {
            Some("auto") => Format::Auto,
            Some("text") => Format::Text,
            Some("json") => Format::Json,
            _ => return Err(usage()),
        };
        start = 2;
    }
    let arguments = &arguments[start..];
    let first = arguments.first().and_then(|argument| argument.to_str());
    let command = match first {
        None => Command::Help(Topic::Root),
        Some("--version") if arguments.len() == 1 => Command::Version,
        Some("--help" | "-h") if arguments.len() == 1 => Command::Help(Topic::Root),
        Some("help") if arguments.len() <= 2 => {
            Command::Help(topic(arguments.get(1).and_then(|value| value.to_str()))?)
        }
        Some("daemon" | "harness" | "workspace" | "probe" | "ui")
            if arguments.len() == 2 && matches!(arguments[1].to_str(), Some("--help" | "-h")) =>
        {
            Command::Help(topic(first)?)
        }
        Some("daemon") if arguments.len() == 1 => Command::Help(Topic::Daemon),
        Some("daemon") => Command::Daemon(
            parse_cli_arguments(arguments.iter().cloned())
                .map_err(|source| Failure::with_source("cli_invalid_arguments", source))?,
        ),
        Some("harness") => {
            match vibemux_harness_native::cli::parse_arguments(arguments[1..].iter().cloned())
                .map_err(|source| Failure::with_source("cli_invalid_arguments", source))?
            {
                vibemux_harness_native::cli::Command::Help => Command::Help(Topic::Harness),
                vibemux_harness_native::cli::Command::Version => Command::Version,
                command => Command::Harness(command),
            }
        }
        Some("workspace") if arguments.len() == 1 => Command::Help(Topic::Workspace),
        Some("workspace")
            if arguments
                .get(1)
                .is_some_and(|argument| argument == "status") =>
        {
            let options = options(&arguments[2..], &["--project-root"], false)?;
            Command::Workspace {
                project_root: options.get("--project-root").map(PathBuf::from),
            }
        }
        Some("status") => {
            let options = options(&arguments[1..], &["--project-root"], false)?;
            Command::Status {
                project_root: options.get("--project-root").map(PathBuf::from),
            }
        }
        Some("probe") => {
            let options = options(&arguments[1..], &["--report"], false)?;
            Command::Probe {
                report_path: options.get("--report").map(PathBuf::from),
            }
        }
        Some("ui") => {
            let options = options(
                &arguments[1..],
                &["--report", "--project-root", "--width", "--height"],
                true,
            )?;
            let once = options.contains_key("--once");
            if !once
                && (format == Format::Json
                    || options.contains_key("--width")
                    || options.contains_key("--height"))
            {
                return Err(usage());
            }
            let width = dimension(options.get("--width"), DEFAULT_WIDTH, 40..=240)?;
            let height = dimension(options.get("--height"), DEFAULT_HEIGHT, 12..=80)?;
            Command::Ui {
                project_root: options.get("--project-root").map(PathBuf::from),
                report_path: options.get("--report").map(PathBuf::from),
                once,
                width,
                height,
            }
        }
        _ => return Err(usage()),
    };
    Ok(Invocation { format, command })
}

fn options(
    arguments: &[OsString],
    allowed: &[&str],
    allow_once: bool,
) -> Result<BTreeMap<String, OsString>, Failure> {
    let mut options = BTreeMap::new();
    let mut index = 0;
    while index < arguments.len() {
        let name = arguments[index].to_str().ok_or_else(usage)?;
        if options.contains_key(name) {
            return Err(usage());
        }
        if allow_once && name == "--once" {
            options.insert(name.to_string(), OsString::new());
            index += 1;
            continue;
        }
        if !allowed.contains(&name) {
            return Err(usage());
        }
        let value = arguments
            .get(index + 1)
            .filter(|value| !value.is_empty())
            .ok_or_else(usage)?;
        options.insert(name.to_string(), value.clone());
        index += 2;
    }
    Ok(options)
}

fn dimension(
    value: Option<&OsString>,
    default: u16,
    bounds: std::ops::RangeInclusive<u16>,
) -> Result<u16, Failure> {
    let value = match value {
        Some(value) => value
            .to_str()
            .and_then(|value| value.parse().ok())
            .ok_or_else(usage)?,
        None => default,
    };
    if !bounds.contains(&value) {
        return Err(usage());
    }
    Ok(value)
}

fn topic(value: Option<&str>) -> Result<Topic, Failure> {
    match value {
        None => Ok(Topic::Root),
        Some("daemon") => Ok(Topic::Daemon),
        Some("harness") => Ok(Topic::Harness),
        Some("workspace" | "status") => Ok(Topic::Workspace),
        Some("probe") => Ok(Topic::Probe),
        Some("ui") => Ok(Topic::Ui),
        _ => Err(usage()),
    }
}
fn usage() -> Failure {
    Failure::new("cli_invalid_arguments")
}

pub fn help(topic: Topic) -> String {
    match topic {
        Topic::Harness => vibemux_harness_native::cli::help("vibemuxctl harness"),
        Topic::Daemon => "Daemon lifecycle (explicit operations)\n\n  vibemuxctl daemon start [--project-root <path>] [--daemon-executable <path>]\n  vibemuxctl daemon health [--project-root <path>]\n  vibemuxctl daemon stop [--project-root <path>]\n  vibemuxctl daemon inspect [--project-root <path>]\n  vibemuxctl daemon recover --confirmation <sha256> [--project-root <path>]\n\nNo daemon starts automatically. Recovery remains confirmation-gated.\n".to_string(),
        Topic::Workspace => "Workspace inventory (read-only)\n\n  vibemuxctl workspace status [--project-root <path>]\n  vibemuxctl status [--project-root <path>]\n\nShows local branches, worktrees, and cached-ref ancestry; no fetch, merge, or cleanup.\nGlobal output flags go before the command: --format text|json|auto.\n".to_string(),
        Topic::Probe => "Diagnostic metadata\n\n  vibemuxctl probe [--report <saved_report.json>]\n\nSaved reports are bounded, validated and redacted. Without --report, fixed local\nread-only probes run. Authentication and inference are separate, untested claims.\n".to_string(),
        Topic::Ui => "Read-only dashboard\n\n  vibemuxctl ui [--report <saved_report.json>] [--project-root <path>]\n  vibemuxctl --format text ui --once [--report <file>] [--width <40..240>] [--height <12..80>]\n\nTab/1/2/3: sections | j/k/arrows: selection | Enter: details | ?: help | q/Esc: quit\nNo native TUI attach, daemon start, or model request is performed.\n".to_string(),
        Topic::Root => format!("VibeMux {}\n\nUsage: vibemuxctl [--format auto|text|json] <command>\n\nInspect\n  status              Workspace and existing daemon health\n  workspace status    Local branch/worktree inventory (cached refs)\n  probe               Fixed read-only probes or a saved report\n  ui                  Navigable read-only dashboard (--once for snapshots)\n\nOperate explicitly\n  daemon              start | health | stop | inspect | recover\n  harness             catalog | replay | probe | dispatch\n\nOutput\n  auto                Text in a terminal, JSON when redirected\n  --json              Shorthand for --format json (before command)\n  help <command>      Detailed command help\n  --version           Package version\n\nHelp and version are always text. Native harness execution remains opt-in.\nPython reference CLI and standalone Rust binaries remain available.\n", env!("CARGO_PKG_VERSION")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grouped_commands_preserve_literal_paths_and_format_is_explicit() {
        let invocation = parse(
            [
                "--format",
                "text",
                "workspace",
                "status",
                "--project-root",
                "literal ; & path",
            ]
            .map(OsString::from),
        )
        .unwrap();
        assert_eq!(invocation.format, Format::Text);
        assert!(
            matches!(invocation.command, Command::Workspace { project_root: Some(path) } if path == PathBuf::from("literal ; & path"))
        );
        let invocation =
            parse(["harness", "catalog", "--config", "--format"].map(OsString::from)).unwrap();
        assert!(
            matches!(invocation.command, Command::Harness(vibemux_harness_native::cli::Command::Catalog { config_path }) if config_path == PathBuf::from("--format"))
        );
    }
    #[test]
    fn flags_and_noninteractive_ui_are_validated_without_io() {
        for args in [
            vec!["workspace", "clean"],
            vec!["ui", "--width", "80"],
            vec!["ui", "--once", "--once"],
            vec!["ui", "--once", "--width", "9999"],
            vec!["--format", "other", "probe"],
            vec!["--json", "ui"],
        ] {
            assert!(parse(args.into_iter().map(OsString::from)).is_err());
        }
    }
}
