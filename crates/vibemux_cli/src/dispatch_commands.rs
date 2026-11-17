//! `vibemuxctl dispatch` (ADR 029 §7): a thin Control v5 client for the
//! daemon's harness request dispatch. `dispatch submit --decision` runs a
//! recorded decision (ADR 034) over Control v8.
//!
//! A prompt is read from a file or stdin, never from argv, so it stays out
//! of process listings and shell history, and it is bounded before any
//! byte reaches the daemon. Output pages are reassembled into whole,
//! byte-exact vendor records.
//!
//! `dispatch provision` is the one local subcommand: it derives the route
//! config from the trusted probe cache and never opens a daemon connection.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use uuid::Uuid;
use vibemux_harness::{
    AgentKind,
    dispatch::{
        DispatchRequest, ProvisionProbeRow, plan_provisioning,
        request::{DISPATCH_REQUEST_SCHEMA_VERSION, MAX_PROMPT_BYTES},
        route_config::{ExecutablePolicy, MAX_ROUTE_CONFIG_BYTES},
    },
};
use vibemux_probe::{
    ProbeReport,
    cache::{self, CacheError},
};
use vibemuxd::{
    control::{ControlClient, ControlError, DecisionDispatchArgument, HarnessDispatchOutputQuery},
    harness_dispatch::{DISPATCH_CONFIG_FILE_NAME, OutputReassembler},
    process::DaemonPaths,
};

pub use crate::prompt_input::PromptSource;
use crate::{
    CliCommand, DaemonCliError, harness_client, map_control_error,
    prompt_input::{prompt_source, read_prompt},
};

/// Pages one `dispatch output` call reads at most; a transcript at the
/// 16 MiB capture bound needs fewer than 1024.
const MAX_OUTPUT_PAGES: usize = 4096;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DispatchCommand {
    Catalog,
    Probe {
        harness: AgentKind,
    },
    Submit {
        harness: AgentKind,
        prompt: PromptSource,
        /// Reusing an id makes a retry idempotent.
        request_id: Option<Uuid>,
    },
    /// Runs the harness a recorded decision chose. The daemon re-runs every
    /// dispatch gate; the prompt must be the one the decision was made for.
    SubmitDecided {
        decision_id: Uuid,
        prompt: PromptSource,
        request_id: Option<Uuid>,
    },
    Status {
        request_id: Uuid,
    },
    Output {
        request_id: Uuid,
        after_sequence: u64,
    },
    Cancel {
        request_id: Uuid,
    },
    /// Plans a schema-1 route config from the trusted probe cache (or writes
    /// it with `--write --yes`). Local only: it reads no daemon and sends no
    /// prompt.
    Provision {
        write: bool,
        allow_execution: bool,
        force: bool,
        yes: bool,
    },
}

/// Parses the arguments after the `dispatch` verb.
pub(crate) fn parse_dispatch_arguments(
    arguments: &[OsString],
) -> Result<CliCommand, DaemonCliError> {
    let (action, rest) = arguments
        .split_first()
        .ok_or(DaemonCliError::InvalidArguments)?;
    let action = action.to_str().ok_or(DaemonCliError::InvalidArguments)?;
    let mut project_root = None;
    let mut prompt = None;
    let mut request_id = None;
    let mut decision_id = None;
    let mut after_sequence = None;
    let mut write = false;
    let mut force = false;
    let mut yes = false;
    let mut allow_execution = None;
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
            "--prompt-file" if action == "submit" && prompt.is_none() => {
                prompt = Some(prompt_source(value)?);
                index += 2;
            }
            "--request-id" if action == "submit" && request_id.is_none() => {
                request_id = Some(parse_id(text_value(value)?)?);
                index += 2;
            }
            "--decision" if action == "submit" && decision_id.is_none() => {
                decision_id = Some(parse_id(text_value(value)?)?);
                index += 2;
            }
            "--after-sequence" if action == "output" && after_sequence.is_none() => {
                after_sequence = Some(
                    text_value(value)?
                        .parse::<u64>()
                        .map_err(|_| DaemonCliError::InvalidArguments)?,
                );
                index += 2;
            }
            "--write" if action == "provision" && !write => {
                write = true;
                index += 1;
            }
            "--force" if action == "provision" && !force => {
                force = true;
                index += 1;
            }
            "--yes" if action == "provision" && !yes => {
                yes = true;
                index += 1;
            }
            "--allow-execution" if action == "provision" && allow_execution.is_none() => {
                allow_execution = Some(true);
                index += 1;
            }
            "--no-execution" if action == "provision" && allow_execution.is_none() => {
                allow_execution = Some(false);
                index += 1;
            }
            _ if !token.starts_with("--") && positional.is_none() => {
                positional = Some(token);
                index += 1;
            }
            _ => return Err(DaemonCliError::InvalidArguments),
        }
    }
    let target = || positional.ok_or(DaemonCliError::InvalidArguments);
    let command = match action {
        "catalog" if positional.is_none() => DispatchCommand::Catalog,
        "probe" => DispatchCommand::Probe {
            harness: parse_harness(target()?)?,
        },
        // The decision names the harness, so an explicit one is refused.
        "submit" if decision_id.is_some() && positional.is_some() => {
            return Err(DaemonCliError::InvalidArguments);
        }
        "submit" => {
            let prompt = prompt.ok_or(DaemonCliError::InvalidArguments)?;
            match decision_id {
                Some(decision_id) => DispatchCommand::SubmitDecided {
                    decision_id,
                    prompt,
                    request_id,
                },
                None => DispatchCommand::Submit {
                    harness: parse_harness(target()?)?,
                    prompt,
                    request_id,
                },
            }
        }
        "status" => DispatchCommand::Status {
            request_id: parse_id(target()?)?,
        },
        "output" => DispatchCommand::Output {
            request_id: parse_id(target()?)?,
            after_sequence: after_sequence.unwrap_or(0),
        },
        "cancel" => DispatchCommand::Cancel {
            request_id: parse_id(target()?)?,
        },
        // Provision takes flags only; none of the request-bearing options
        // belong to it, and `--force`/`--yes` must accompany `--write`.
        "provision"
            if positional.is_none()
                && prompt.is_none()
                && request_id.is_none()
                && decision_id.is_none()
                && after_sequence.is_none() =>
        {
            if (force || yes) && !write {
                return Err(DaemonCliError::InvalidArguments);
            }
            DispatchCommand::Provision {
                write,
                allow_execution: allow_execution.unwrap_or(true),
                force,
                yes,
            }
        }
        _ => return Err(DaemonCliError::InvalidArguments),
    };
    Ok(CliCommand::Dispatch {
        project_root,
        command,
    })
}

/// Runs one dispatch command and returns its JSON output. Every subcommand
/// except `provision` talks to the running daemon; a submitted prompt is
/// read and validated first.
pub async fn run_dispatch(
    paths: &DaemonPaths,
    command: DispatchCommand,
) -> Result<Value, DaemonCliError> {
    // Provisioning reads the probe cache and writes the operator config; it
    // never contacts the daemon, so it must not require a running one.
    if let DispatchCommand::Provision {
        write,
        allow_execution,
        force,
        yes,
    } = &command
    {
        return run_provision(paths, *write, *allow_execution, *force, *yes).await;
    }
    let request = match &command {
        DispatchCommand::Submit {
            harness,
            prompt,
            request_id,
        } => Some(read_request(*harness, prompt.clone(), *request_id).await?),
        _ => None,
    };
    let decided = match &command {
        DispatchCommand::SubmitDecided {
            decision_id,
            prompt,
            request_id,
        } => Some(DecisionDispatchArgument {
            decision_id: *decision_id,
            request_id: request_id.unwrap_or_else(Uuid::new_v4),
            prompt: read_valid_prompt(prompt.clone()).await?,
        }),
        _ => None,
    };
    let client = harness_client(paths).await?;
    match command {
        DispatchCommand::Catalog => {
            let routes = client
                .harness_dispatch_catalog()
                .await
                .map_err(map_dispatch_error)?;
            Ok(json!({"ok": true, "routes": routes}))
        }
        DispatchCommand::Probe { harness } => {
            let probe = client
                .harness_dispatch_probe(harness)
                .await
                .map_err(map_dispatch_error)?;
            Ok(json!({"ok": true, "probe": probe}))
        }
        DispatchCommand::Submit { .. } => {
            let request = request.ok_or(DaemonCliError::InvalidArguments)?;
            let receipt = client
                .harness_dispatch_submit(&request)
                .await
                .map_err(map_dispatch_error)?;
            Ok(json!({"ok": true, "receipt": receipt}))
        }
        DispatchCommand::SubmitDecided { .. } => {
            let argument = decided.ok_or(DaemonCliError::InvalidArguments)?;
            let receipt = client
                .decision_dispatch_submit(&argument)
                .await
                .map_err(map_dispatch_error)?;
            Ok(json!({"ok": true, "receipt": receipt}))
        }
        DispatchCommand::Status { request_id } => {
            let status = client
                .harness_dispatch_status(request_id)
                .await
                .map_err(map_dispatch_error)?;
            Ok(json!({"ok": true, "dispatch": status}))
        }
        DispatchCommand::Cancel { request_id } => {
            let status = client
                .harness_dispatch_cancel(request_id)
                .await
                .map_err(map_dispatch_error)?;
            Ok(json!({"ok": true, "dispatch": status}))
        }
        DispatchCommand::Output {
            request_id,
            after_sequence,
        } => read_output(&client, request_id, after_sequence).await,
        DispatchCommand::Provision {
            write,
            allow_execution,
            force,
            yes,
        } => run_provision(paths, write, allow_execution, force, yes).await,
    }
}

/// The measured on-disk state of the target config relative to the planned
/// bytes. `Unsafe` means the path exists but is not a regular file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExistingConfig {
    Absent,
    Identical,
    Differs,
    Unsafe,
}

impl ExistingConfig {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Identical => "identical",
            Self::Differs => "differs",
            Self::Unsafe => "unsafe",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConfigWriteOutcome {
    Created,
    Replaced,
    Unchanged,
}

impl ConfigWriteOutcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Replaced => "replaced",
            Self::Unchanged => "unchanged",
        }
    }
}

/// Plans the dispatch route config from the trusted probe cache and, with
/// `write`, stores it as `<state dir>/harness_dispatch.json`. Execution is
/// never started; `allow_execution` only records the operator's consent for
/// later turns, which is why writing demands `--yes`.
async fn run_provision(
    paths: &DaemonPaths,
    write: bool,
    allow_execution: bool,
    force: bool,
    yes: bool,
) -> Result<Value, DaemonCliError> {
    if write && !yes {
        return Err(DaemonCliError::DispatchProvisionConfirmationRequired);
    }
    let cache_path = paths.probe_cache_path();
    let read_path = cache_path.clone();
    let report = tokio::task::spawn_blocking(move || cache::read_cache(&read_path))
        .await
        .map_err(|_| DaemonCliError::DispatchProvisionIo)?
        .map_err(|error| map_provision_cache_error(&cache_path, error))?;
    let plan = plan_provisioning(
        &provision_rows(&report),
        paths.project_root(),
        allow_execution,
        ExecutablePolicy::for_host(),
    );
    let mut bytes =
        serde_json::to_vec_pretty(&plan.config).map_err(|_| DaemonCliError::DispatchProvisionIo)?;
    bytes.push(b'\n');
    let target = paths.state_dir().join(DISPATCH_CONFIG_FILE_NAME);
    let task_target = target.clone();
    let canonical_project_root = paths.project_root().to_path_buf();
    let (existing, written) = if write {
        if plan.is_empty() {
            return Err(DaemonCliError::DispatchProvisionEmpty);
        }
        let outcome = tokio::task::spawn_blocking(move || {
            write_config(&task_target, &bytes, force, &canonical_project_root)
        })
        .await
        .map_err(|_| DaemonCliError::DispatchProvisionIo)??;
        let existing = match outcome {
            ConfigWriteOutcome::Created => ExistingConfig::Absent,
            ConfigWriteOutcome::Replaced => ExistingConfig::Differs,
            ConfigWriteOutcome::Unchanged => ExistingConfig::Identical,
        };
        (existing, Some(outcome))
    } else {
        let existing = tokio::task::spawn_blocking(move || inspect_config(&task_target, &bytes))
            .await
            .map_err(|_| DaemonCliError::DispatchProvisionIo)??;
        (existing, None)
    };
    let written_flag = written.is_some();
    let outcome = written.map_or("planned", ConfigWriteOutcome::as_str);
    Ok(json!({
        "ok": true,
        "written": written_flag,
        "outcome": outcome,
        "existing": existing.as_str(),
        "config": plan.config,
        "skipped": plan.skipped_json(),
    }))
}

/// Reduces the probe report to the three fields provisioning reads.
fn provision_rows(report: &ProbeReport) -> Vec<ProvisionProbeRow> {
    report
        .agents
        .iter()
        .map(|agent| {
            ProvisionProbeRow::new(
                agent.agent,
                agent.launcher_state,
                agent.path.as_ref().map(PathBuf::from),
            )
        })
        .collect()
}

/// A missing cache keeps the existing `harness_probe_cache_missing`
/// remediation; every other failure is a provisioning error.
fn map_provision_cache_error(path: &Path, error: CacheError) -> DaemonCliError {
    match error {
        CacheError::Missing => {
            DaemonCliError::HarnessProbeCacheMissing(path.to_string_lossy().into_owned())
        }
        CacheError::Io => DaemonCliError::DispatchProvisionIo,
        CacheError::UnsafeArtifact | CacheError::TooLarge | CacheError::Invalid => {
            DaemonCliError::DispatchProvisionCacheInvalid
        }
    }
}

/// Reads the target without following a symlink and compares it with the
/// planned bytes. A non-regular target is reported as `Unsafe`; the writer
/// refuses it even with `--force`.
fn inspect_config(path: &Path, planned: &[u8]) -> Result<ExistingConfig, DaemonCliError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ExistingConfig::Absent);
        }
        Err(_) => return Err(DaemonCliError::DispatchProvisionIo),
    };
    if !metadata.file_type().is_file() {
        return Ok(ExistingConfig::Unsafe);
    }
    if metadata.len() > MAX_ROUTE_CONFIG_BYTES as u64 {
        return Ok(ExistingConfig::Differs);
    }
    let existing = std::fs::read(path).map_err(|_| DaemonCliError::DispatchProvisionIo)?;
    Ok(if existing == planned {
        ExistingConfig::Identical
    } else {
        ExistingConfig::Differs
    })
}

/// Writes `bytes` atomically: a private sibling temp file, synced, loaded
/// through the daemon's own loader, then renamed over the target. An
/// identical existing file is left untouched; a different one needs `force`.
/// The loader gate refuses a plan the daemon would reject before it can
/// replace a working config.
fn write_config(
    path: &Path,
    bytes: &[u8],
    force: bool,
    canonical_project_root: &Path,
) -> Result<ConfigWriteOutcome, DaemonCliError> {
    let existing = inspect_config(path, bytes)?;
    match existing {
        ExistingConfig::Identical => return Ok(ConfigWriteOutcome::Unchanged),
        ExistingConfig::Unsafe => return Err(DaemonCliError::DispatchProvisionUnsafeTarget),
        ExistingConfig::Differs if !force => {
            return Err(DaemonCliError::DispatchProvisionConflict);
        }
        ExistingConfig::Absent | ExistingConfig::Differs => {}
    }
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    create_private_dir(parent)?;
    let temp = temp_path(parent);
    if let Err(error) = write_temp(&temp, bytes) {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    // The exact planned bytes are on disk under a private temp name; loading
    // them here runs the daemon's canonicalization and trust checks.
    if vibemuxd::harness_dispatch::load_dispatch_config(&temp, canonical_project_root).is_err() {
        let _ = std::fs::remove_file(&temp);
        return Err(DaemonCliError::DispatchProvisionConfigRejected);
    }
    if std::fs::rename(&temp, path).is_err() {
        let _ = std::fs::remove_file(&temp);
        return Err(DaemonCliError::DispatchProvisionIo);
    }
    Ok(match existing {
        ExistingConfig::Absent => ConfigWriteOutcome::Created,
        _ => ConfigWriteOutcome::Replaced,
    })
}

fn create_private_dir(dir: &Path) -> Result<(), DaemonCliError> {
    if dir.is_dir() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder
            .create(dir)
            .map_err(|_| DaemonCliError::DispatchProvisionIo)?;
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir).map_err(|_| DaemonCliError::DispatchProvisionIo)?;
    }
    Ok(())
}

fn write_temp(path: &Path, bytes: &[u8]) -> Result<(), DaemonCliError> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| DaemonCliError::DispatchProvisionIo)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| DaemonCliError::DispatchProvisionIo)
}

fn temp_path(parent: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nonce = COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(
        ".{DISPATCH_CONFIG_FILE_NAME}.tmp-{}-{nonce}",
        std::process::id()
    ))
}

/// Reads pages until the transcript is complete or the reader has caught
/// up with a running attempt; `next_after_sequence` resumes the read.
async fn read_output(
    client: &ControlClient,
    request_id: Uuid,
    after_sequence: u64,
) -> Result<Value, DaemonCliError> {
    let mut reassembler = OutputReassembler::starting_after(after_sequence);
    let mut records = Vec::new();
    let mut complete = false;
    for _ in 0..MAX_OUTPUT_PAGES {
        let page = client
            .harness_dispatch_output(HarnessDispatchOutputQuery {
                request_id,
                cursor: reassembler.cursor(),
            })
            .await
            .map_err(map_dispatch_error)?;
        complete = page.complete;
        let caught_up = page.fragments.is_empty();
        records.extend(
            reassembler
                .accept(page)
                .map_err(|_| map_control_error(ControlError::InvalidFrame))?,
        );
        if complete || caught_up {
            break;
        }
    }
    let records: Vec<Value> = records
        .into_iter()
        .map(|record| {
            json!({"sequence": record.sequence, "kind": record.kind, "raw_json": record.raw_json})
        })
        .collect();
    Ok(json!({
        "ok": true,
        "request_id": request_id,
        "records": records,
        "next_after_sequence": reassembler.cursor().after_sequence,
        "complete": complete,
    }))
}

async fn read_request(
    harness: AgentKind,
    source: PromptSource,
    request_id: Option<Uuid>,
) -> Result<DispatchRequest, DaemonCliError> {
    let prompt = read_valid_prompt(source).await?;
    let request = DispatchRequest {
        schema_version: DISPATCH_REQUEST_SCHEMA_VERSION,
        request_id: request_id.unwrap_or_else(Uuid::new_v4),
        harness,
        prompt,
    };
    request
        .validate()
        .map_err(|_| DaemonCliError::InvalidPrompt)?;
    Ok(request)
}

/// Reads a dispatch-bounded prompt off the async runtime and refuses a
/// blank, oversized, or non-UTF-8 one before any byte reaches the daemon.
/// Decision commands read theirs here too.
pub(crate) async fn read_valid_prompt(source: PromptSource) -> Result<String, DaemonCliError> {
    read_prompt(source, MAX_PROMPT_BYTES)
        .await
        .map_err(|error| error.into_cli_error(DaemonCliError::InvalidPrompt))
}

/// A daemon older than the control version an operation needs lacks it;
/// say so instead of reporting stale runtime state.
pub(crate) fn map_dispatch_error(error: ControlError) -> DaemonCliError {
    match error {
        ControlError::UnsupportedVersion => DaemonCliError::Control {
            code: error.code().to_string(),
        },
        other => map_control_error(other),
    }
}

/// A harness by command name (`opencode`) or wire name (`open_code`).
fn parse_harness(text: &str) -> Result<AgentKind, DaemonCliError> {
    AgentKind::all()
        .into_iter()
        .find(|kind| kind.command_name() == text)
        .or_else(|| serde_json::from_value(Value::String(text.to_string())).ok())
        .ok_or_else(|| DaemonCliError::UnknownHarness(text.to_string()))
}

/// A request or decision id: any UUID but the nil one.
pub(crate) fn parse_id(text: &str) -> Result<Uuid, DaemonCliError> {
    Uuid::parse_str(text)
        .ok()
        .filter(|request_id| !request_id.is_nil())
        .ok_or(DaemonCliError::InvalidArguments)
}

pub(crate) fn text_value(value: Option<&OsString>) -> Result<&str, DaemonCliError> {
    value
        .and_then(|value| value.to_str())
        .ok_or(DaemonCliError::InvalidArguments)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> Result<CliCommand, DaemonCliError> {
        let arguments: Vec<OsString> = arguments.iter().map(OsString::from).collect();
        parse_dispatch_arguments(&arguments)
    }

    fn dispatch(arguments: &[&str]) -> DispatchCommand {
        match parse(arguments).expect("valid arguments") {
            CliCommand::Dispatch { command, .. } => command,
            other => panic!("unexpected command {other:?}"),
        }
    }

    #[test]
    fn every_action_parses_with_its_own_flags_only() {
        let request_id = Uuid::new_v4();
        let id = request_id.to_string();
        assert_eq!(dispatch(&["catalog"]), DispatchCommand::Catalog);
        assert_eq!(
            dispatch(&["probe", "opencode"]),
            DispatchCommand::Probe {
                harness: AgentKind::OpenCode
            }
        );
        assert_eq!(
            dispatch(&["probe", "open_code"]),
            DispatchCommand::Probe {
                harness: AgentKind::OpenCode
            }
        );
        assert_eq!(
            dispatch(&["submit", "codex", "--prompt-file", "-", "--request-id", &id]),
            DispatchCommand::Submit {
                harness: AgentKind::Codex,
                prompt: PromptSource::Stdin,
                request_id: Some(request_id),
            }
        );
        assert_eq!(
            dispatch(&["submit", "--prompt-file", "C:\\prompts\\a b.txt", "claude"]),
            DispatchCommand::Submit {
                harness: AgentKind::Claude,
                prompt: PromptSource::File(PathBuf::from("C:\\prompts\\a b.txt")),
                request_id: None,
            }
        );
        let decision_id = Uuid::new_v4();
        assert_eq!(
            dispatch(&[
                "submit",
                "--decision",
                &decision_id.to_string(),
                "--prompt-file",
                "-",
                "--request-id",
                &id,
            ]),
            DispatchCommand::SubmitDecided {
                decision_id,
                prompt: PromptSource::Stdin,
                request_id: Some(request_id),
            }
        );
        assert_eq!(
            dispatch(&["status", &id]),
            DispatchCommand::Status { request_id }
        );
        assert_eq!(
            dispatch(&["output", &id, "--after-sequence", "7"]),
            DispatchCommand::Output {
                request_id,
                after_sequence: 7
            }
        );
        assert_eq!(
            dispatch(&["cancel", &id]),
            DispatchCommand::Cancel { request_id }
        );
        match parse(&["catalog", "--project-root", "C:\\project"]).expect("root") {
            CliCommand::Dispatch { project_root, .. } => {
                assert_eq!(project_root, Some(PathBuf::from("C:\\project")));
            }
            other => panic!("unexpected command {other:?}"),
        }
    }

    #[test]
    fn malformed_arguments_are_refused() {
        let id = Uuid::new_v4().to_string();
        let nil = Uuid::nil().to_string();
        for arguments in [
            &[][..],
            &["unknown"][..],
            &["catalog", "codex"][..],
            &["probe"][..],
            &["submit", "codex"][..],
            &["submit", "codex", "--prompt-file"][..],
            &[
                "submit",
                "codex",
                "--prompt-file",
                "-",
                "--prompt-file",
                "-",
            ][..],
            &[
                "submit",
                "codex",
                "--prompt-file",
                "-",
                "--request-id",
                &nil,
            ][..],
            &["status", "not_a_uuid"][..],
            &["status", &id, "--prompt-file", "-"][..],
            &["output", &id, "--after-sequence", "-1"][..],
            &["cancel", &id, "extra"][..],
            &["catalog", "--prompt", "inline prompt"][..],
            // A decided submission takes its harness from the decision.
            &["submit", "codex", "--decision", &id, "--prompt-file", "-"][..],
            &["submit", "--decision", &id][..],
            &["submit", "--decision", &nil, "--prompt-file", "-"][..],
            &[
                "submit",
                "--decision",
                &id,
                "--decision",
                &id,
                "--prompt-file",
                "-",
            ][..],
            &["status", &id, "--decision", &id][..],
        ] {
            assert_eq!(
                parse(arguments),
                Err(DaemonCliError::InvalidArguments),
                "{arguments:?}"
            );
        }
        assert_eq!(
            parse(&["probe", "vim"]),
            Err(DaemonCliError::UnknownHarness("vim".to_string()))
        );
    }

    #[tokio::test]
    async fn a_dispatch_prompt_is_bounded_utf8_and_readable() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("prompt.txt");
        let largest = "p".repeat(MAX_PROMPT_BYTES);
        std::fs::write(&path, &largest).expect("write prompt");
        assert_eq!(
            read_valid_prompt(PromptSource::File(path.clone())).await,
            Ok(largest)
        );
        for invalid in [
            "p".repeat(MAX_PROMPT_BYTES + 1).into_bytes(),
            vec![0xff, 0xfe],
        ] {
            std::fs::write(&path, invalid).expect("write prompt");
            assert_eq!(
                read_valid_prompt(PromptSource::File(path.clone())).await,
                Err(DaemonCliError::InvalidPrompt)
            );
        }
        assert_eq!(
            read_valid_prompt(PromptSource::File(temp.path().join("missing.txt"))).await,
            Err(DaemonCliError::PromptUnavailable)
        );
    }

    #[test]
    fn provision_flags_parse_only_for_provision() {
        assert_eq!(
            dispatch(&["provision"]),
            DispatchCommand::Provision {
                write: false,
                allow_execution: true,
                force: false,
                yes: false,
            }
        );
        assert_eq!(
            dispatch(&["provision", "--allow-execution"]),
            DispatchCommand::Provision {
                write: false,
                allow_execution: true,
                force: false,
                yes: false,
            }
        );
        assert_eq!(
            dispatch(&["provision", "--write", "--yes", "--force", "--no-execution",]),
            DispatchCommand::Provision {
                write: true,
                allow_execution: false,
                force: true,
                yes: true,
            }
        );
        match parse(&["provision", "--project-root", "C:\\project"]).expect("root") {
            CliCommand::Dispatch { project_root, .. } => {
                assert_eq!(project_root, Some(PathBuf::from("C:\\project")));
            }
            other => panic!("unexpected command {other:?}"),
        }
        for arguments in [
            &["provision", "--allow-execution", "--no-execution"][..],
            &["provision", "--force"][..],
            &["provision", "--yes"][..],
            &["provision", "codex"][..],
            &["provision", "--prompt-file", "-"][..],
            &["catalog", "--write"][..],
            &["catalog", "--force"][..],
        ] {
            assert_eq!(
                parse(arguments),
                Err(DaemonCliError::InvalidArguments),
                "{arguments:?}"
            );
        }
    }

    fn probe_report(agents: Vec<vibemux_probe::AgentProbe>) -> ProbeReport {
        ProbeReport {
            schema_version: vibemux_probe::PROBE_SCHEMA_VERSION,
            observed_at_epoch_seconds: 0,
            platform: "test".to_string(),
            agents,
            gateway: vibemux_probe::GatewayProbe {
                state: vibemux_harness::ProbeState::NotRun,
                host: "127.0.0.1".to_string(),
                port: 0,
                tcp_reachable: false,
                health_status: None,
                telemetry_state: vibemux_harness::ProbeState::NotRun,
                telemetry: Vec::new(),
                code: "not_run".to_string(),
            },
            a2a: vibemux_probe::A2aSelfTestProbe {
                state: vibemux_harness::ProbeState::NotRun,
                correlation_preserved: false,
                listener_closed: true,
                code: "not_run".to_string(),
            },
        }
    }

    fn probed_agent(
        harness: AgentKind,
        state: vibemux_harness::ProbeState,
        executable: Option<PathBuf>,
    ) -> vibemux_probe::AgentProbe {
        vibemux_probe::AgentProbe {
            agent: harness,
            launcher_state: state,
            authentication_state: vibemux_harness::ProbeState::NotRun,
            inference_state: vibemux_harness::ProbeState::NotRun,
            launcher: vibemux_harness::LauncherKind::DirectExecutable,
            path: executable.map(|path| path.to_string_lossy().into_owned()),
            version: None,
            route: vibemux_probe::RouteKind::Unknown,
            endpoints: Vec::new(),
            code: "verified".to_string(),
        }
    }

    fn provision_executable(root: &Path, name: &str) -> PathBuf {
        if cfg!(windows) {
            root.join(format!("{name}.exe"))
        } else {
            root.join(name)
        }
    }

    #[tokio::test]
    async fn provisioning_requires_the_probe_cache_and_writes_idempotently() {
        use vibemux_harness::ProbeState;

        let temp = tempfile::tempdir().expect("temp project");
        let vendor = tempfile::tempdir().expect("temp vendor");
        let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
        assert_eq!(
            run_provision(&paths, false, true, false, false)
                .await
                .err()
                .map(|error| error.code().to_string()),
            Some("harness_probe_cache_missing".to_string())
        );
        let cache_path = paths.probe_cache_path();
        let cache_path_for_write = cache_path.clone();
        let codex = provision_executable(vendor.path(), "codex");
        std::fs::write(&codex, b"fixture image").expect("executable");
        let report = probe_report(vec![
            probed_agent(AgentKind::Codex, ProbeState::Verified, Some(codex.clone())),
            probed_agent(
                AgentKind::Claude,
                ProbeState::Failed,
                Some(provision_executable(vendor.path(), "claude")),
            ),
        ]);
        tokio::task::spawn_blocking(move || cache::write_cache(&report, &cache_path_for_write))
            .await
            .expect("join")
            .expect("write cache");

        let dry = run_provision(&paths, false, true, false, false)
            .await
            .expect("dry run");
        assert_eq!(dry["written"], json!(false));
        assert_eq!(dry["outcome"], json!("planned"));
        assert_eq!(dry["existing"], json!("absent"));
        assert_eq!(dry["config"]["schema_version"], json!(1));
        assert_eq!(dry["config"]["routes"].as_array().map(Vec::len), Some(1));
        assert_eq!(dry["config"]["routes"][0]["harness"], json!("codex"));
        assert_eq!(
            dry["config"]["routes"][0]["protocol"],
            json!("codex_app_server")
        );
        assert_eq!(
            dry["config"]["routes"][0]["executable"],
            json!(
                std::fs::canonicalize(&codex)
                    .expect("canonical")
                    .to_string_lossy()
                    .into_owned()
            )
        );
        let skipped = dry["skipped"].as_array().expect("skips");
        assert!(
            skipped
                .iter()
                .any(|entry| entry["harness"] == json!("claude")
                    && entry["reason"] == json!("not_verified"))
        );

        // Writing may enable paid vendor turns, so it needs --yes.
        assert_eq!(
            run_provision(&paths, true, true, false, false)
                .await
                .err()
                .map(|error| error.code().to_string()),
            Some("harness_dispatch_provision_confirmation_required".to_string())
        );
        let config_path = paths.state_dir().join(DISPATCH_CONFIG_FILE_NAME);
        assert!(!config_path.exists());

        let created = run_provision(&paths, true, true, false, true)
            .await
            .expect("write");
        assert_eq!(created["outcome"], json!("created"));
        assert_eq!(created["written"], json!(true));
        let bytes = std::fs::read(&config_path).expect("config file");
        let parsed =
            vibemux_harness::dispatch::DispatchConfig::parse(&bytes, ExecutablePolicy::for_host())
                .expect("daemon accepts the written config");
        assert_eq!(parsed.config.routes.len(), 1);
        let loaded =
            vibemuxd::harness_dispatch::load_dispatch_config(&config_path, paths.project_root())
                .expect("the daemon loader accepts the written config");
        assert_eq!(
            loaded.executable(AgentKind::Codex).expect("codex"),
            std::fs::canonicalize(&codex).expect("canonical")
        );

        let again = run_provision(&paths, true, true, false, true)
            .await
            .expect("idempotent write");
        assert_eq!(again["outcome"], json!("unchanged"));

        std::fs::write(&config_path, b"{}\n").expect("differing config");
        assert_eq!(
            run_provision(&paths, true, true, false, true)
                .await
                .err()
                .map(|error| error.code().to_string()),
            Some("harness_dispatch_provision_conflict".to_string())
        );
        let replaced = run_provision(&paths, true, true, true, true)
            .await
            .expect("forced write");
        assert_eq!(replaced["outcome"], json!("replaced"));
    }

    #[tokio::test]
    async fn a_non_regular_target_has_its_own_provisioning_code() {
        use vibemux_harness::ProbeState;

        let temp = tempfile::tempdir().expect("temp project");
        let vendor = tempfile::tempdir().expect("temp vendor");
        let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
        let cache_path = paths.probe_cache_path();
        let cache_path_for_write = cache_path.clone();
        let codex = provision_executable(vendor.path(), "codex");
        std::fs::write(&codex, b"fixture image").expect("executable");
        let report = probe_report(vec![probed_agent(
            AgentKind::Codex,
            ProbeState::Verified,
            Some(codex),
        )]);
        tokio::task::spawn_blocking(move || cache::write_cache(&report, &cache_path_for_write))
            .await
            .expect("join")
            .expect("write cache");
        let config_path = paths.state_dir().join(DISPATCH_CONFIG_FILE_NAME);
        std::fs::create_dir(&config_path).expect("directory target");
        assert_eq!(
            run_provision(&paths, true, true, false, true)
                .await
                .err()
                .map(|error| error.code().to_string()),
            Some("harness_dispatch_provision_unsafe_target".to_string())
        );
    }

    #[test]
    fn a_plan_the_daemon_would_reject_never_replaces_the_config() {
        let temp = tempfile::tempdir().expect("temp");
        let project = temp.path().join("project");
        let state = project.join(".vibemux");
        std::fs::create_dir_all(&state).expect("state dir");
        let root = std::fs::canonicalize(&project).expect("canonical root");
        let target = state.join(DISPATCH_CONFIG_FILE_NAME);
        let bytes = serde_json::to_vec(&json!({
            "schema_version": 1,
            "routes": [{
                "harness": "codex",
                "protocol": "codex_app_server",
                "executable": project.join("missing-executable"),
                "enabled": true,
                "allow_execution": true
            }]
        }))
        .expect("config bytes");
        assert_eq!(
            write_config(&target, &bytes, true, &root)
                .err()
                .map(|error| error.code().to_string()),
            Some("harness_dispatch_provision_config_rejected".to_string())
        );
        assert!(!target.exists());
        let temporaries: Vec<_> = std::fs::read_dir(&state)
            .expect("state listing")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(".harness_dispatch.json.tmp-"))
            .collect();
        assert!(temporaries.is_empty(), "temporary left behind");
    }

    #[tokio::test]
    async fn provisioning_refuses_an_empty_plan_but_still_reports_skips() {
        use vibemux_harness::ProbeState;

        let temp = tempfile::tempdir().expect("temp project");
        let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
        let cache_path = paths.probe_cache_path();
        let cache_path_for_write = cache_path.clone();
        let report = probe_report(vec![probed_agent(
            AgentKind::OpenCode,
            ProbeState::Verified,
            Some(provision_executable(
                cache_path.parent().expect("state dir"),
                "opencode",
            )),
        )]);
        tokio::task::spawn_blocking(move || cache::write_cache(&report, &cache_path_for_write))
            .await
            .expect("join")
            .expect("write cache");
        let dry = run_provision(&paths, false, true, false, false)
            .await
            .expect("dry run");
        assert_eq!(dry["config"]["routes"], json!([]));
        assert!(
            dry["skipped"]
                .as_array()
                .expect("skips")
                .iter()
                .any(|entry| entry["harness"] == json!("open_code")
                    && entry["reason"] == json!("no_executing_protocol"))
        );
        assert_eq!(
            run_provision(&paths, true, true, false, true)
                .await
                .err()
                .map(|error| error.code().to_string()),
            Some("harness_dispatch_provision_empty".to_string())
        );
        assert!(!paths.state_dir().join(DISPATCH_CONFIG_FILE_NAME).exists());
    }

    #[tokio::test]
    async fn a_blank_prompt_is_refused_before_contacting_the_daemon() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("blank.txt");
        std::fs::write(&path, " \n\t").expect("write prompt");
        assert_eq!(
            read_request(AgentKind::Codex, PromptSource::File(path.clone()), None)
                .await
                .map(|request| request.request_id),
            Err(DaemonCliError::InvalidPrompt)
        );
        assert_eq!(
            read_valid_prompt(PromptSource::File(path)).await,
            Err(DaemonCliError::InvalidPrompt)
        );
    }
}
