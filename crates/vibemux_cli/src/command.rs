//! One command tree. The daemon lifecycle, daemon-backed control verbs and native
//! harness parsers remain their boundary owners; this module only routes to them.
use crate::{CliCommand, failure::Failure, parse_cli_arguments};
use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

pub const DEFAULT_WIDTH: u16 = 100;
pub const DEFAULT_HEIGHT: u16 = 28;
/// Strict bound for the daemon-free inspection and native harness commands.
const MAX_ARGUMENTS: usize = 32;
/// Hard bound for the daemon-backed control verbs, whose parsers own their own
/// grammar (repeated `--label` and similar options) and have no tighter limit.
const MAX_CONTROL_ARGUMENTS: usize = 1024;
/// The daemon lifecycle and daemon-backed control verbs routed to
/// [`parse_cli_arguments`].
const CONTROL_VERBS: [&str; 11] = [
    "harnesses",
    "switch",
    "dispatch",
    "decide",
    "decisions",
    "dsh",
    "workflow",
    "slots",
    "context",
    "session",
    "prompt",
];

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
    Control,
    Workspace,
    Probe,
    Ui,
}

pub enum Command {
    Help(Topic),
    Version,
    /// Daemon lifecycle plus every daemon-backed control verb.
    Control(CliCommand),
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
        .take(MAX_CONTROL_ARGUMENTS + 1)
        .collect::<Vec<_>>();
    if arguments.len() > MAX_CONTROL_ARGUMENTS {
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
    if !first.is_some_and(is_control_verb)
        && (arguments.len() > MAX_ARGUMENTS
            || arguments.iter().any(|argument| argument.len() > 8192))
    {
        return Err(usage());
    }
    let command = match first {
        None => Command::Help(Topic::Root),
        Some("--version") if arguments.len() == 1 => Command::Version,
        Some("--help" | "-h") if arguments.len() == 1 => Command::Help(Topic::Root),
        Some("help") if arguments.len() <= 2 => {
            Command::Help(topic(arguments.get(1).and_then(|value| value.to_str()))?)
        }
        Some(
            "daemon" | "harness" | "workspace" | "probe" | "ui" | "harnesses" | "switch"
            | "dispatch" | "decide" | "decisions" | "dsh" | "workflow" | "slots" | "context"
            | "session" | "prompt",
        ) if arguments.len() == 2 && matches!(arguments[1].to_str(), Some("--help" | "-h")) => {
            Command::Help(topic(first)?)
        }
        Some("daemon") if arguments.len() == 1 => Command::Help(Topic::Daemon),
        Some("daemon") => Command::Control(
            parse_cli_arguments(arguments.iter().cloned())
                .map_err(|source| Failure::with_source("cli_invalid_arguments", source))?,
        ),
        Some(verb) if is_control_verb(verb) => Command::Control(
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

fn is_control_verb(verb: &str) -> bool {
    CONTROL_VERBS.contains(&verb)
}

fn topic(value: Option<&str>) -> Result<Topic, Failure> {
    match value {
        None => Ok(Topic::Root),
        Some("daemon") => Ok(Topic::Daemon),
        Some("harness") => Ok(Topic::Harness),
        Some(verb) if is_control_verb(verb) => Ok(Topic::Control),
        Some("workspace" | "status") => Ok(Topic::Workspace),
        Some("probe") => Ok(Topic::Probe),
        Some("ui") => Ok(Topic::Ui),
        _ => Err(usage()),
    }
}
fn usage() -> Failure {
    Failure::new("cli_invalid_arguments")
}

const CONTROL_HELP: &str = "Usage:\n  vibemuxctl daemon start [--project-root <path>] [--daemon-executable <path>]\n  vibemuxctl daemon health [--project-root <path>]\n  vibemuxctl daemon stop [--project-root <path>]\n  vibemuxctl daemon inspect [--project-root <path>]\n  vibemuxctl daemon recover --confirmation <sha256> [--project-root <path>]\n  vibemuxctl harnesses [--cached] [--project-root <path>]\n  vibemuxctl switch <harness> [--project-root <path>]
  vibemuxctl dispatch catalog [--project-root <path>]
  vibemuxctl dispatch probe <harness> [--project-root <path>]
  vibemuxctl dispatch submit <harness> --prompt-file <path|-> [--request-id <uuid>] [--project-root <path>]
  vibemuxctl dispatch submit --decision <decision_id> --prompt-file <path|-> [--request-id <uuid>] [--project-root <path>]
  vibemuxctl dispatch status <request_id> [--project-root <path>]
  vibemuxctl dispatch output <request_id> [--after-sequence <n>] [--project-root <path>]
  vibemuxctl dispatch cancel <request_id> [--project-root <path>]
  vibemuxctl decide route --prompt-file <path|-> [--label <label>]... [--allow-external] [--project-root <path>]
  vibemuxctl decisions list [--after-sequence <n>] [--limit <n>] [--project-root <path>]
  vibemuxctl decisions show <decision_id> [--project-root <path>]
  vibemuxctl workflow prepare --request-file <path> --policy-file <path> [--project-root <path>]
  vibemuxctl workflow start --contract <sha256> --request-id <uuid> [--project-root <path>]
  vibemuxctl workflow status|pause|cancel|purge <workflow_id> [--project-root <path>]
  vibemuxctl workflow export <workflow_id> --out <new-or-empty-dir> [--project-root <path>]
  vibemuxctl slots list [--project-root <path>]
  vibemuxctl context share --bundle <bundle_id> --to <session_id> [--project-root <path>]
  vibemuxctl session inspect <session_id> [--prompts] [--project-root <path>]
  vibemuxctl session attach <session_id> [--project-root <path>]
  vibemuxctl prompt evaluate --candidate <candidate_id> --suite <suite_id> [--project-root <path>]
  vibemuxctl prompt versions|rollback [--project-root <path>]
  vibemuxctl dsh open [--session-id <id>] [--project-root <path>]
  vibemuxctl dsh prompt --prompt-file <path|-> [--project-root <path>]
  vibemuxctl dsh poll [--after-sequence <n>] [--project-root <path>]
  vibemuxctl dsh status|cancel|close [--project-root <path>]
";

pub fn help(topic: Topic) -> String {
    match topic {
        Topic::Harness => vibemux_harness_native::cli::help("vibemuxctl harness"),
        Topic::Daemon => "Daemon lifecycle (explicit operations)\n\n  vibemuxctl daemon start [--project-root <path>] [--daemon-executable <path>]\n  vibemuxctl daemon health [--project-root <path>]\n  vibemuxctl daemon stop [--project-root <path>]\n  vibemuxctl daemon inspect [--project-root <path>]\n  vibemuxctl daemon recover --confirmation <sha256> [--project-root <path>]\n\nNo daemon starts automatically. Recovery remains confirmation-gated.\n".to_string(),
        Topic::Workspace => "Workspace inventory (read-only)\n\n  vibemuxctl workspace status [--project-root <path>]\n  vibemuxctl status [--project-root <path>]\n\nShows local branches, worktrees, and cached-ref ancestry; no fetch, merge, or cleanup.\nGlobal output flags go before the command: --format text|json|auto.\n".to_string(),
        Topic::Probe => "Diagnostic metadata\n\n  vibemuxctl probe [--report <saved_report.json>]\n\nSaved reports are bounded, validated and redacted. Without --report, fixed local\nread-only probes run. Authentication and inference are separate, untested claims.\n".to_string(),
        Topic::Ui => "Read-only dashboard\n\n  vibemuxctl ui [--report <saved_report.json>] [--project-root <path>]\n  vibemuxctl --format text ui --once [--report <file>] [--width <40..240>] [--height <12..80>]\n\nTab/1/2/3: sections | j/k/arrows: selection | Enter: details | ?: help | q/Esc: quit\nNo native TUI attach, daemon start, or model request is performed.\n".to_string(),
        Topic::Control => CONTROL_HELP.to_string(),
        Topic::Root => format!("VibeMux {}

Usage: vibemuxctl [--format auto|text|json] <command>

Inspect (no daemon)
  status              Workspace and existing daemon health
  workspace status    Local branch/worktree inventory (cached refs)
  probe               Fixed read-only probes or a saved report
  ui                  Navigable read-only dashboard (--once for snapshots)

Operate explicitly (daemon)
  daemon              start | health | stop | inspect | recover
  harnesses, switch   Harness registry and project default
  dispatch            Daemon-owned harness dispatch (ADR 029)
  decide, decisions   Advisory harness routing (ADR 034)
  workflow, slots, context, session, prompt
                      Dual-track workflows (ADR 031)
  dsh                 DSH sessions (ADR 032)
  help control        Full syntax for the daemon-backed verbs

Native harness (standalone, no daemon)
  harness             catalog | replay | probe | dispatch

Output
  auto                Text in a terminal, JSON when redirected
  --json              Shorthand for --format json (before command)
  help <command>      Detailed command help
  --version           Package version

Help and version are always text. Native harness execution remains opt-in.
Python reference CLI and standalone Rust binaries remain available.
", env!("CARGO_PKG_VERSION")),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

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
            matches!(invocation.command, Command::Workspace { project_root: Some(path) } if path.as_path() == Path::new("literal ; & path"))
        );
        let invocation =
            parse(["harness", "catalog", "--config", "--format"].map(OsString::from)).unwrap();
        assert!(
            matches!(invocation.command, Command::Harness(vibemux_harness_native::cli::Command::Catalog { config_path }) if config_path.as_path() == Path::new("--format"))
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

    /// The documented value for a `<placeholder>` in a command line.
    fn placeholder_value(name: &str) -> &'static str {
        match name {
            "sha256" | "confirmation_from_inspect" => {
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            }
            "new_dir" => "export_dir",
            "uuid" | "id" | "request_id" | "decision_id" | "workflow_id" | "bundle_id"
            | "session_id" | "candidate_id" | "suite_id" => "11111111-1111-4111-8111-111111111111",
            other => panic!("undocumented placeholder <{other}>"),
        }
    }

    /// Every argument list one documented command line stands for: a
    /// `status|pause` token is each alternative, a `[--prompts]` token is
    /// present or absent, and a `<placeholder>` gets a valid value.
    fn documented_argument_lists(command_line: &str) -> Vec<Vec<String>> {
        let mut lists = vec![Vec::new()];
        for token in command_line.split_whitespace() {
            let token = token.trim_matches('\'');
            let alternatives: Vec<&str> = match token.strip_prefix('[') {
                Some(optional) => vec![optional.trim_end_matches(']'), ""],
                None => token.split('|').collect(),
            };
            lists = lists
                .into_iter()
                .flat_map(|list| {
                    alternatives.iter().map(move |alternative| {
                        let mut list = list.clone();
                        match alternative
                            .strip_prefix('<')
                            .and_then(|name| name.strip_suffix('>'))
                        {
                            Some(name) => list.push(placeholder_value(name).to_string()),
                            None if alternative.is_empty() => {}
                            None => list.push((*alternative).to_string()),
                        }
                        list
                    })
                })
                .collect();
        }
        lists
    }

    /// AGENTS.md requires documented commands to match the current CLI.
    /// Every `vibemuxctl.exe` line in these documents must parse. Parsing
    /// is pure, so nothing runs and no path is opened.
    #[test]
    fn every_documented_command_line_parses() {
        let documents = [
            ("CLAUDE.md", include_str!("../../../CLAUDE.md")),
            ("README.md", include_str!("../../../README.md")),
            (
                "unified_cli_guide.md",
                include_str!("../../../docs/guides/unified_cli_guide.md"),
            ),
            (
                "branch_workflow.md",
                include_str!("../../../docs/workflow/branch_workflow.md"),
            ),
        ];
        for (document, text) in documents {
            let mut command_lines = 0;
            for line in text.lines() {
                let Some((_, rest)) = line.split_once("vibemuxctl.exe ") else {
                    continue;
                };
                let command_line = rest.split(['#', '`']).next().unwrap_or_default();
                command_lines += 1;
                for arguments in documented_argument_lists(command_line) {
                    assert!(
                        parse(arguments.iter().map(OsString::from)).is_ok(),
                        "{document}: {arguments:?}"
                    );
                }
            }
            assert!(command_lines > 0, "{document}");
        }
    }
}
