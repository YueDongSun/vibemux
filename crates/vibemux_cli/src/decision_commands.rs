//! `vibemuxctl decide` and `vibemuxctl decisions` (ADR 034): a thin Control
//! v8 client for the daemon's advisory harness routing.
//!
//! A decision is advice and runs nothing; `dispatch submit --decision` runs
//! it through every dispatch gate again. The prompt is read from a file or
//! stdin and bounded like a dispatch prompt; the daemon only hashes and
//! measures it, and no output of these commands carries it or its digest.

use std::{ffi::OsString, path::PathBuf};

use serde_json::{Value, json};
use uuid::Uuid;
use vibemuxd::{
    control::{DecisionAdviseArgument, DecisionListQuery},
    process::DaemonPaths,
};

use crate::{
    CliCommand, DaemonCliError,
    dispatch_commands::{map_dispatch_error, parse_id, read_valid_prompt, text_value},
    harness_client,
    prompt_input::{PromptSource, prompt_source},
};

/// Decisions one `decisions list` asks for unless `--limit` says otherwise;
/// the daemon bounds the limit.
const DEFAULT_LIST_LIMIT: usize = 20;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecisionCommand {
    /// Records which harness should run the prompt; runs nothing.
    Route {
        prompt: PromptSource,
        /// Operator labels; the daemon checks them against its allowlist.
        labels: Vec<String>,
        /// The per-request consent to an external backend. The operator
        /// config must enable the backend too.
        allow_external: bool,
    },
    /// Decisions recorded after `after_sequence`, oldest first.
    List {
        after_sequence: u64,
        limit: usize,
    },
    Show {
        decision_id: Uuid,
    },
}

/// Parses the arguments after the `decide` or `decisions` verb.
pub(crate) fn parse_decision_arguments(
    verb: &str,
    arguments: &[OsString],
) -> Result<CliCommand, DaemonCliError> {
    let (action, rest) = arguments
        .split_first()
        .ok_or(DaemonCliError::InvalidArguments)?;
    let action = action.to_str().ok_or(DaemonCliError::InvalidArguments)?;
    let routing = (verb, action) == ("decide", "route");
    let listing = (verb, action) == ("decisions", "list");
    let mut project_root = None;
    let mut prompt = None;
    let mut labels = Vec::new();
    let mut allow_external = false;
    let mut after_sequence = None;
    let mut limit = None;
    let mut positional: Option<&str> = None;
    let mut index = 0;
    while index < rest.len() {
        let token = rest[index]
            .to_str()
            .ok_or(DaemonCliError::InvalidArguments)?;
        let value = rest.get(index + 1);
        match token {
            "--project-root" if project_root.is_none() => {
                project_root = Some(PathBuf::from(
                    value.ok_or(DaemonCliError::InvalidArguments)?,
                ));
                index += 2;
            }
            "--prompt-file" if routing && prompt.is_none() => {
                prompt = Some(prompt_source(value)?);
                index += 2;
            }
            "--label" if routing => {
                labels.push(text_value(value)?.to_string());
                index += 2;
            }
            "--allow-external" if routing && !allow_external => {
                allow_external = true;
                index += 1;
            }
            "--after-sequence" if listing && after_sequence.is_none() => {
                after_sequence = Some(parse_number(value)?);
                index += 2;
            }
            "--limit" if listing && limit.is_none() => {
                limit = Some(parse_number(value)?);
                index += 2;
            }
            _ if !token.starts_with("--") && positional.is_none() => {
                positional = Some(token);
                index += 1;
            }
            _ => return Err(DaemonCliError::InvalidArguments),
        }
    }
    let command = match (verb, action) {
        ("decide", "route") if positional.is_none() => DecisionCommand::Route {
            prompt: prompt.ok_or(DaemonCliError::InvalidArguments)?,
            labels,
            allow_external,
        },
        ("decisions", "list") if positional.is_none() => DecisionCommand::List {
            after_sequence: after_sequence.unwrap_or(0),
            limit: limit.unwrap_or(DEFAULT_LIST_LIMIT),
        },
        ("decisions", "show") => DecisionCommand::Show {
            decision_id: parse_id(positional.ok_or(DaemonCliError::InvalidArguments)?)?,
        },
        _ => return Err(DaemonCliError::InvalidArguments),
    };
    Ok(CliCommand::Decision {
        project_root,
        command,
    })
}

/// Runs one decision command against the running daemon and returns its
/// JSON output. A routed prompt is read and checked first.
pub async fn run_decision(
    paths: &DaemonPaths,
    command: DecisionCommand,
) -> Result<Value, DaemonCliError> {
    let advice = match &command {
        DecisionCommand::Route {
            prompt,
            labels,
            allow_external,
        } => Some(DecisionAdviseArgument {
            prompt: read_valid_prompt(prompt.clone()).await?,
            labels: labels.clone(),
            allow_external: *allow_external,
        }),
        _ => None,
    };
    let client = harness_client(paths).await?;
    match command {
        DecisionCommand::Route { .. } => {
            let argument = advice.ok_or(DaemonCliError::InvalidArguments)?;
            let decision = client
                .decision_advise(&argument)
                .await
                .map_err(map_dispatch_error)?;
            Ok(json!({"ok": true, "decision": decision}))
        }
        DecisionCommand::List {
            after_sequence,
            limit,
        } => {
            let page = client
                .decision_list(DecisionListQuery {
                    after_sequence,
                    limit,
                })
                .await
                .map_err(map_dispatch_error)?;
            Ok(json!({
                "ok": true,
                "decisions": page.decisions,
                "next_after_sequence": page.next_after_sequence,
            }))
        }
        DecisionCommand::Show { decision_id } => {
            let decision = client
                .decision_show(decision_id)
                .await
                .map_err(map_dispatch_error)?;
            Ok(json!({"ok": true, "decision": decision}))
        }
    }
}

fn parse_number<T: std::str::FromStr>(value: Option<&OsString>) -> Result<T, DaemonCliError> {
    text_value(value)?
        .parse()
        .map_err(|_| DaemonCliError::InvalidArguments)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(verb: &str, arguments: &[&str]) -> Result<CliCommand, DaemonCliError> {
        let arguments: Vec<OsString> = arguments.iter().map(OsString::from).collect();
        parse_decision_arguments(verb, &arguments)
    }

    fn decision(verb: &str, arguments: &[&str]) -> DecisionCommand {
        match parse(verb, arguments).expect("valid arguments") {
            CliCommand::Decision { command, .. } => command,
            other => panic!("unexpected command {other:?}"),
        }
    }

    #[test]
    fn every_action_parses_with_its_own_flags_only() {
        assert_eq!(
            decision(
                "decide",
                &[
                    "route",
                    "--label",
                    "rust",
                    "--prompt-file",
                    "C:\\prompts\\a b.txt",
                    "--allow-external",
                    "--label",
                    "docs",
                ]
            ),
            DecisionCommand::Route {
                prompt: PromptSource::File(PathBuf::from("C:\\prompts\\a b.txt")),
                labels: vec!["rust".to_string(), "docs".to_string()],
                allow_external: true,
            }
        );
        assert_eq!(
            decision("decide", &["route", "--prompt-file", "-"]),
            DecisionCommand::Route {
                prompt: PromptSource::Stdin,
                labels: Vec::new(),
                allow_external: false,
            }
        );
        assert_eq!(
            decision("decisions", &["list"]),
            DecisionCommand::List {
                after_sequence: 0,
                limit: DEFAULT_LIST_LIMIT,
            }
        );
        assert_eq!(
            decision(
                "decisions",
                &["list", "--limit", "5", "--after-sequence", "12"]
            ),
            DecisionCommand::List {
                after_sequence: 12,
                limit: 5,
            }
        );
        let decision_id = Uuid::new_v4();
        assert_eq!(
            decision("decisions", &["show", &decision_id.to_string()]),
            DecisionCommand::Show { decision_id }
        );
        match parse("decisions", &["list", "--project-root", "C:\\project"]).expect("root") {
            CliCommand::Decision { project_root, .. } => {
                assert_eq!(project_root, Some(PathBuf::from("C:\\project")));
            }
            other => panic!("unexpected command {other:?}"),
        }
    }

    #[test]
    fn malformed_arguments_are_refused() {
        let id = Uuid::new_v4().to_string();
        let nil = Uuid::nil().to_string();
        for (verb, arguments) in [
            ("decide", &[][..]),
            ("decide", &["list"][..]),
            ("decide", &["route"][..]),
            ("decide", &["route", "--prompt-file"][..]),
            ("decide", &["route", "--prompt-file", "-", "--label"][..]),
            ("decide", &["route", "--prompt-file", "-", "claude"][..]),
            (
                "decide",
                &["route", "--prompt-file", "-", "--prompt-file", "-"][..],
            ),
            (
                "decide",
                &[
                    "route",
                    "--prompt-file",
                    "-",
                    "--allow-external",
                    "--allow-external",
                ][..],
            ),
            ("decide", &["route", "--prompt", "inline prompt"][..]),
            ("decisions", &["route", "--prompt-file", "-"][..]),
            ("decisions", &["list", "--limit", "-1"][..]),
            ("decisions", &["list", "--after-sequence", "x"][..]),
            ("decisions", &["list", "--label", "rust"][..]),
            ("decisions", &["list", &id][..]),
            ("decisions", &["show"][..]),
            ("decisions", &["show", "not_a_uuid"][..]),
            ("decisions", &["show", &nil][..]),
            ("decisions", &["show", &id, "--limit", "5"][..]),
            ("decisions", &["show", &id, &id][..]),
        ] {
            assert_eq!(
                parse(verb, arguments),
                Err(DaemonCliError::InvalidArguments),
                "{verb} {arguments:?}"
            );
        }
    }
}
