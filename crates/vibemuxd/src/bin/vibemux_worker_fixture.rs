#![forbid(unsafe_code)]
//! Scripted writable worker and reviewer for the offline workflow tests
//! (ADR 031); built only with the `test_helpers` feature.
//!
//! Tests copy this binary as `codex` or `claude` into a directory outside
//! the project root, next to a `worker_fixture_script.json`. Each turn is
//! looked up by the task key, purpose, and turn number the rendered prompt
//! states (`<executable>/<task>/<purpose>_<turn>`, then
//! `<task>/<purpose>_<turn>`, then `<task>/<purpose>`); the step writes
//! files relative to the working directory the daemon chose and ends with
//! the fenced checkpoint or review block. A step's variants select a
//! different step when the prompt contains a marker, which is how a prompt
//! policy change becomes observable offline. A step's `sequence` gives
//! successive invocations of the same key successive entries (the last
//! repeats); each invocation claims its index with an exclusively created
//! file, so parallel turns never share an entry. Every turn appends a
//! `started` record with the prompt and a `finished` record to
//! `worker_fixture_log.jsonl` next to the executable, so tests can check
//! what a session was actually sent and whether turns overlapped. A step's
//! `rendezvous` holds the turn until the other turns of its group are alive
//! too, which makes overlap a property the turns prove by meeting rather than
//! one inferred from how close their wall-clock timestamps happen to be.
//!
//! This is synthetic evidence only: it proves the daemon's orchestration,
//! never a vendor model's behavior.

#[path = "../fixtures/worker_broker.rs"]
mod worker_broker;

use std::{
    collections::BTreeMap,
    env,
    fs::OpenOptions,
    io::{self, BufRead, Read, Write},
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};

use serde::Deserialize;
use serde_json::{Value, json};

const SCRIPT_FILE_NAME: &str = "worker_fixture_script.json";
const LOG_FILE_NAME: &str = "worker_fixture_log.jsonl";
const CLAIMS_DIR_NAME: &str = "worker_fixture_claims";
/// Upper bound on the invocations of one key.
const MAX_CLAIMS: usize = 64;
const CHECKPOINT_INFO_STRING: &str = "vibemux_checkpoint";
const REVIEW_INFO_STRING: &str = "vibemux_review";
const SESSION_ID: &str = "worker_fixture_session";
/// Upper bound on a scripted delay.
const MAX_DELAY_MS: u64 = 60_000;
const RENDEZVOUS_DIR_NAME: &str = "worker_fixture_rendezvous";
/// Upper bound on the turns of one rendezvous group.
const MAX_RENDEZVOUS_PARTIES: usize = 16;
/// How often a waiting turn looks for the rest of its group.
const RENDEZVOUS_POLL_INTERVAL: Duration = Duration::from_millis(20);

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Script {
    steps: BTreeMap<String, Step>,
}

#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Step {
    #[serde(default)]
    read: Vec<String>,
    #[serde(default)]
    search: Vec<String>,
    #[serde(default)]
    run_dev_test: bool,
    #[serde(default)]
    broker_output: Option<String>,
    /// Relative path to new content.
    #[serde(default)]
    write: BTreeMap<String, String>,
    #[serde(default)]
    remove: Vec<String>,
    /// Waits for the other turns of a group before the delay and the
    /// terminal record.
    #[serde(default)]
    rendezvous: Option<Rendezvous>,
    /// Sleeps before the terminal record, so a test can cancel mid-turn.
    #[serde(default)]
    delay_ms: u64,
    /// The turn fails at the protocol level.
    #[serde(default)]
    fail: bool,
    /// Ends without a report block.
    #[serde(default)]
    no_report: bool,
    #[serde(default)]
    checkpoint: Option<CheckpointStep>,
    #[serde(default)]
    review: Option<ReviewStep>,
    /// The first variant whose marker the prompt contains replaces this
    /// step.
    #[serde(default)]
    variants: Vec<Variant>,
    /// Successive invocations of this key take successive entries.
    #[serde(default)]
    sequence: Vec<Step>,
}

/// A group of turns that must all be running at the same moment. The daemon
/// may launch them arbitrarily far apart on a slow host; a turn waits here
/// for the rest and fails if they never arrive, so a scheduler that runs the
/// group one turn at a time cannot pass.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Rendezvous {
    /// Lowercase letters, digits, and underscores; names the group.
    group: String,
    /// How many distinct turns the group needs, this one included.
    parties: usize,
    /// How long this turn waits for the rest.
    timeout_ms: u64,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Variant {
    prompt_contains: String,
    step: Step,
}

#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckpointStep {
    /// Defaults to the written and removed paths.
    #[serde(default)]
    changed_files: Option<Vec<String>>,
    #[serde(default)]
    development_tests: Vec<Value>,
    #[serde(default)]
    unresolved_issues: Vec<String>,
    #[serde(default)]
    messages: Vec<Value>,
    #[serde(default)]
    summary: Option<String>,
    /// Reports every bundle the prompt delivered as consumed.
    #[serde(default)]
    consume_context: bool,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewStep {
    verdict: String,
    #[serde(default)]
    findings: Vec<Value>,
    #[serde(default)]
    checks_executed: Vec<String>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Protocol {
    CodexExec,
    Claude,
}

/// What the rendered prompt states about the turn.
struct TurnFacts {
    task_key: String,
    purpose: String,
    turn_number: u32,
    task_spec_digest: String,
    bundles: Vec<String>,
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("worker_fixture_failed: {error}");
            std::process::exit(2);
        }
    }
}

fn run() -> Result<i32> {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let protocol = protocol(&arguments)?;
    let grant = fixture_grant(&arguments, protocol)?;
    let development_runtime = fixture_development_runtime(&arguments)?;
    let mut broker = worker_broker::BrokerClient::launch(&arguments)?;
    let executable = env::current_exe()?;
    let directory = executable.parent().ok_or("fixture_directory_missing")?;
    let script: Script = serde_json::from_slice(&std::fs::read(directory.join(SCRIPT_FILE_NAME))?)?;
    let name = executable
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or("fixture_name_invalid")?
        .to_string();
    let mut output = io::stdout().lock();
    match protocol {
        Protocol::CodexExec => {
            let mut prompt = String::new();
            io::stdin().lock().read_to_string(&mut prompt)?;
            write_json(
                &mut output,
                json!({"type":"thread.started","thread_id":SESSION_ID}),
            )?;
            write_json(&mut output, json!({"type":"turn.started"}))?;
            let turn = perform(
                &script,
                &name,
                directory,
                &prompt,
                grant,
                development_runtime.as_deref(),
                broker.as_mut(),
            )?;
            finish(protocol, &mut output, &turn)?;
        }
        Protocol::Claude => {
            for line in io::stdin().lock().lines() {
                let message: Value = serde_json::from_str(&line?)?;
                if message["type"] == "control_request"
                    && message["request"]["subtype"] == "initialize"
                {
                    write_json(
                        &mut output,
                        json!({"type":"control_response","response":{
                        "subtype":"success","request_id":message["request_id"],"response":{}}}),
                    )?;
                } else if message["type"] == "user" {
                    let prompt = message["message"]["content"]
                        .as_str()
                        .ok_or("fixture_prompt_invalid")?;
                    let turn = perform(
                        &script,
                        &name,
                        directory,
                        prompt,
                        grant,
                        development_runtime.as_deref(),
                        broker.as_mut(),
                    )?;
                    finish(protocol, &mut output, &turn)?;
                    break;
                }
            }
        }
    }
    Ok(0)
}

fn protocol(arguments: &[String]) -> Result<Protocol> {
    let has = |value: &str| arguments.iter().any(|argument| argument == value);
    if has("--input-format") {
        Ok(Protocol::Claude)
    } else if has("exec") && has("--json") {
        Ok(Protocol::CodexExec)
    } else {
        Err("fixture_protocol_invalid".into())
    }
}

/// The outcome of one scripted turn.
struct Turn {
    final_text: String,
    fail: bool,
}

fn fixture_grant(
    arguments: &[String],
    protocol: Protocol,
) -> Result<vibemux_harness::dispatch::writable_profile::WritableGrant> {
    let argument = |name: &str| -> Result<&str> {
        let indices: Vec<usize> = arguments
            .iter()
            .enumerate()
            .filter_map(|(index, value)| (value == name).then_some(index))
            .collect();
        if indices.len() != 1 {
            return Err("fixture_grant_missing_or_duplicate".into());
        }
        arguments
            .get(indices[0] + 1)
            .map(String::as_str)
            .ok_or_else(|| "fixture_grant_missing".into())
    };
    let broker = arguments.iter().any(|arg| arg == "--fixture_broker");
    let expected = match (protocol, broker) {
        (Protocol::CodexExec, false) => "codex_fixture_writable_v1",
        (Protocol::Claude, false) => "claude_fixture_writable_v1",
        (Protocol::CodexExec, true) => "codex_fixture_broker_v1",
        (Protocol::Claude, true) => "claude_fixture_broker_v1",
    };
    if argument("--fixture_profile")? != expected {
        return Err("fixture_profile_mismatch".into());
    }
    Ok(serde_json::from_str(argument("--fixture_grant_json")?)?)
}

fn fixture_development_runtime(arguments: &[String]) -> Result<Option<PathBuf>> {
    let indices: Vec<usize> = arguments
        .iter()
        .enumerate()
        .filter_map(|(index, value)| (value == "--fixture_node").then_some(index))
        .collect();
    match indices.as_slice() {
        [] => Ok(None),
        [index] => {
            let path = PathBuf::from(arguments.get(index + 1).ok_or("fixture_runtime_missing")?);
            if !path.is_absolute() || !path.is_file() {
                return Err("fixture_runtime_invalid".into());
            }
            Ok(Some(path))
        }
        _ => Err("fixture_runtime_duplicate".into()),
    }
}

fn perform(
    script: &Script,
    name: &str,
    directory: &Path,
    prompt: &str,
    grant: vibemux_harness::dispatch::writable_profile::WritableGrant,
    development_runtime: Option<&Path>,
    broker: Option<&mut worker_broker::BrokerClient>,
) -> Result<Turn> {
    let facts = turn_facts(prompt)?;
    let key = format!("{}/{}_{}", facts.task_key, facts.purpose, facts.turn_number);
    let (matched, step) = [
        format!("{name}/{key}"),
        key.clone(),
        format!("{}/{}", facts.task_key, facts.purpose),
    ]
    .into_iter()
    .find_map(|candidate| script.steps.get(&candidate).map(|step| (candidate, step)))
    .ok_or_else(|| format!("fixture_step_missing {key}"))?;
    let claim = if step.sequence.is_empty() {
        None
    } else {
        Some(claim_index(directory, &matched)?)
    };
    log(
        directory,
        json!({"event": "started", "harness": name, "key": key, "claim": claim,
            "at_ms": now_ms(), "prompt": prompt, "grant": grant}),
    )?;
    let step = match claim {
        Some(index) => &step.sequence[index.min(step.sequence.len() - 1)],
        None => step,
    };
    let step = select_variant(step, prompt);
    if let Some(broker) = broker {
        if step.run_dev_test || !step.remove.is_empty() {
            return Err("fixture_broker_operation_unsupported".into());
        }
        for path in &step.read {
            broker.read(path)?;
        }
        for path in &step.search {
            broker.search(path)?;
        }
        for (path, content) in &step.write {
            broker.write(path, content, grant)?;
        }
        broker.finish(step.broker_output.as_deref())?;
        log(
            directory,
            json!({"event":"broker_published", "key":key, "at_ms":now_ms()}),
        )?;
    } else {
        if step.broker_output.is_some() {
            return Err("fixture_broker_fault_without_broker".into());
        }
        perform_direct_effects(step, grant, development_runtime)?;
    }
    if let Some(rendezvous) = &step.rendezvous {
        let met = wait_for_rendezvous(directory, &key, rendezvous);
        let event = if met.is_ok() {
            "rendezvous_met"
        } else {
            "rendezvous_failed"
        };
        log(
            directory,
            json!({"event": event, "key": key, "group": rendezvous.group, "at_ms": now_ms()}),
        )?;
        met?;
    }
    if step.delay_ms > 0 {
        std::thread::sleep(Duration::from_millis(step.delay_ms.min(MAX_DELAY_MS)));
    }
    let report = if step.no_report {
        None
    } else if facts.purpose == "review" {
        let review = step.review.clone().ok_or("fixture_review_missing")?;
        let block = json!({
            "schema_version": 1,
            "task_spec_digest": facts.task_spec_digest,
            "verdict": review.verdict,
            "findings": review.findings,
            "checks_executed": review.checks_executed,
        });
        Some((REVIEW_INFO_STRING, block))
    } else {
        let checkpoint = step.checkpoint.clone().unwrap_or_default();
        let changed_files = checkpoint.changed_files.unwrap_or_else(|| {
            step.write
                .keys()
                .cloned()
                .chain(step.remove.iter().cloned())
                .collect()
        });
        let consumed: Vec<&String> = if checkpoint.consume_context {
            facts.bundles.iter().collect()
        } else {
            Vec::new()
        };
        let block = json!({
            "schema_version": 1,
            "task_spec_digest": facts.task_spec_digest,
            "changed_files": changed_files,
            "development_tests": checkpoint.development_tests,
            "unresolved_issues": checkpoint.unresolved_issues,
            "context_refs_consumed": consumed,
            "messages": checkpoint.messages,
            "summary": checkpoint.summary.unwrap_or_else(|| format!("fixture {key}")),
        });
        Some((CHECKPOINT_INFO_STRING, block))
    };
    let mut final_text = format!("Fixture turn {key} finished.\n");
    if let Some((info, block)) = report {
        final_text.push_str(&format!("```{info}\n{block}\n```\n"));
    }
    log(
        directory,
        json!({"event": "finished", "harness": name, "key": key, "claim": claim,
            "at_ms": now_ms()}),
    )?;
    Ok(Turn {
        final_text,
        fail: step.fail,
    })
}

fn perform_direct_effects(
    step: &Step,
    grant: vibemux_harness::dispatch::writable_profile::WritableGrant,
    development_runtime: Option<&Path>,
) -> Result<()> {
    let working_directory = env::current_dir()?;
    for path in &step.read {
        if !grant.read_files {
            return Err("fixture_tool_permission_denied".into());
        }
        std::fs::read(working_directory.join(relative(path)?))?;
    }
    for path in &step.search {
        if !grant.search_files {
            return Err("fixture_tool_permission_denied".into());
        }
        let directory = working_directory.join(relative(path)?);
        for (index, entry) in std::fs::read_dir(directory)?.enumerate() {
            if index >= 256 {
                return Err("fixture_search_limit".into());
            }
            let _ = entry?.file_name();
        }
    }
    if step.run_dev_test {
        if !grant.run_dev_tests {
            return Err("fixture_tool_permission_denied".into());
        }
        // The fixture offers one fixed development command, never script-supplied argv.
        let status =
            std::process::Command::new(development_runtime.ok_or("fixture_runtime_missing")?)
                .arg("--test")
                .current_dir(&working_directory)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()?;
        if !status.success() {
            return Err("fixture_dev_test_failed".into());
        }
    }
    for (path, content) in &step.write {
        let target = working_directory.join(relative(path)?);
        if (target.exists() && !grant.edit_files) || (!target.exists() && !grant.write_files) {
            return Err("fixture_tool_permission_denied".into());
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(target, content)?;
    }
    for path in &step.remove {
        if !grant.edit_files {
            return Err("fixture_tool_permission_denied".into());
        }
        let target = working_directory.join(relative(path)?);
        if target.is_file() {
            std::fs::remove_file(target)?;
        }
    }
    Ok(())
}

fn select_variant<'a>(step: &'a Step, prompt: &str) -> &'a Step {
    step.variants
        .iter()
        .find(|variant| prompt.contains(&variant.prompt_contains))
        .map_or(step, |variant| &variant.step)
}

/// A relative path that cannot climb out of the working directory.
fn relative(path: &str) -> Result<PathBuf> {
    let path = PathBuf::from(path);
    if path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        Ok(path)
    } else {
        Err("fixture_path_invalid".into())
    }
}

fn turn_facts(prompt: &str) -> Result<TurnFacts> {
    let mut task_key = None;
    let mut digest = None;
    let mut turn = None;
    let mut bundles = Vec::new();
    for line in prompt.lines() {
        // The header and the turn line come first; later lookalikes in
        // delivered data are ignored.
        if let Some(rest) = line.strip_prefix("Task: ").filter(|_| task_key.is_none()) {
            task_key = rest.split_whitespace().next().map(str::to_string);
        } else if let Some(rest) = line
            .strip_prefix("TaskSpec digest: ")
            .filter(|_| digest.is_none())
        {
            digest = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("turn ").filter(|_| turn.is_none()) {
            // "turn N of at most M: purpose"
            let mut words = rest.split_whitespace();
            let number = words.next().and_then(|word| word.parse::<u32>().ok());
            let shaped = words.next() == Some("of") && words.next() == Some("at");
            let purpose = rest.rsplit(": ").next().map(|word| word.trim().to_string());
            if let (Some(number), Some(purpose), true) = (number, purpose, shaped) {
                turn = Some((number, purpose));
            }
        } else if let Some(rest) = line.strip_prefix("bundle ")
            && let Some(id) = rest.split_whitespace().next()
        {
            bundles.push(id.to_string());
        }
    }
    let (turn_number, purpose) = turn.ok_or("fixture_turn_missing")?;
    Ok(TurnFacts {
        task_key: task_key.ok_or("fixture_task_missing")?,
        purpose,
        turn_number,
        task_spec_digest: digest.ok_or("fixture_digest_missing")?,
        bundles,
    })
}

/// The first unclaimed invocation index of `key`, claimed by creating its
/// marker file exclusively.
fn claim_index(directory: &Path, key: &str) -> Result<usize> {
    let claims = directory.join(CLAIMS_DIR_NAME);
    std::fs::create_dir_all(&claims)?;
    let stem = key.replace('/', "__");
    for index in 0..MAX_CLAIMS {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(claims.join(format!("{stem}__{index}")))
        {
            Ok(_) => return Ok(index),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    Err("fixture_claims_exhausted".into())
}

/// Registers this turn with its group by creating one marker file, then
/// waits until `parties` markers exist. A repeated key reuses its marker, so
/// a retried turn cannot stand in for a second party.
fn wait_for_rendezvous(directory: &Path, key: &str, rendezvous: &Rendezvous) -> Result<()> {
    let group_name = &rendezvous.group;
    if group_name.is_empty()
        || !group_name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        || rendezvous.parties == 0
        || rendezvous.parties > MAX_RENDEZVOUS_PARTIES
    {
        return Err("fixture_rendezvous_invalid".into());
    }
    let group = directory.join(RENDEZVOUS_DIR_NAME).join(group_name);
    std::fs::create_dir_all(&group)?;
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(group.join(key.replace('/', "__")))
    {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let deadline = Instant::now() + Duration::from_millis(rendezvous.timeout_ms.min(MAX_DELAY_MS));
    loop {
        if std::fs::read_dir(&group)?.count() >= rendezvous.parties {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("fixture_rendezvous_timeout".into());
        }
        std::thread::sleep(RENDEZVOUS_POLL_INTERVAL);
    }
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis())
}

/// Appends one JSON line; a single write keeps concurrent lines whole.
fn log(directory: &Path, line: Value) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join(LOG_FILE_NAME))?;
    file.write_all(format!("{line}\n").as_bytes())?;
    file.flush()?;
    Ok(())
}

fn finish(protocol: Protocol, output: &mut impl Write, turn: &Turn) -> Result<()> {
    match protocol {
        Protocol::CodexExec => {
            write_json(
                output,
                json!({"type":"item.completed","item":{"id":"fixture_message",
                "type":"agent_message","text":turn.final_text}}),
            )?;
            if turn.fail {
                write_json(
                    output,
                    json!({"type":"turn.failed","error":{"message":"fixture failure"}}),
                )
            } else {
                write_json(
                    output,
                    json!({"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}),
                )
            }
        }
        Protocol::Claude => {
            write_json(
                output,
                json!({"type":"stream_event","event":{"type":"content_block_delta",
                "index":0,"delta":{"type":"text_delta","text":turn.final_text}}}),
            )?;
            let subtype = if turn.fail {
                "error_during_execution"
            } else {
                "success"
            };
            write_json(
                output,
                json!({"type":"result","subtype":subtype,"is_error":turn.fail,
                "terminal_reason":"completed","parent_tool_use_id":null,
                "result":turn.final_text}),
            )
        }
    }
}

fn write_json(output: &mut impl Write, message: Value) -> Result<()> {
    serde_json::to_writer(&mut *output, &message)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}
