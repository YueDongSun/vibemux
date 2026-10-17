//! Consistent terminal text and JSON without terminal-control injection.
use crate::{command::Format, failure::Failure};
use serde_json::{Value, json};
use std::fmt::Write;
use vibemux_frontend::dashboard::safe_text;
use vibemux_harness::{
    AgentKind,
    dispatch::{ObservationKind, adapter_capability},
};

const MAX_TEXT_ROWS: usize = 32;

pub fn resolve(format: Format, terminal: bool) -> Format {
    match format {
        Format::Auto if terminal => Format::Text,
        Format::Auto => Format::Json,
        format => format,
    }
}

pub fn json_text(value: &Value) -> Result<String, Failure> {
    let encoded = serde_json::to_string(value)
        .map_err(|source| Failure::with_source("cli_output_encoding_failed", source))?;
    Ok(escape_controls(&encoded))
}

/// Escapes terminal controls and bidirectional marks as `\uXXXX`; the JSON
/// value is unchanged while terminal-control injection is prevented.
fn escape_controls(encoded: &str) -> String {
    let mut output = String::with_capacity(encoded.len());
    for character in encoded.chars() {
        if character.is_control()
            || matches!(character, '\u{061c}' | '\u{200e}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            // String formatting cannot fail; escapes preserve the JSON value exactly.
            write!(&mut output, "\\u{:04x}", u32::from(character))
                .expect("String writes are infallible");
        } else {
            output.push(character);
        }
    }
    output
}

pub fn error_text(error: &Failure, format: Format) -> String {
    if format == Format::Json {
        let mut body = json!({"ok":false,"error_code":error.code()});
        if let Some(message) = error.message() {
            body["error"] = Value::String(message.to_string());
        }
        escape_controls(&body.to_string())
    } else if let Some(message) = error.message() {
        format!("ERROR {}\n{}", error.code(), safe_text(message))
    } else {
        format!(
            "ERROR {}\nUse vibemuxctl help for command syntax.",
            error.code()
        )
    }
}

pub fn report(kind: &str, body: &Value, format: Format) -> Result<String, Failure> {
    if format == Format::Json {
        return json_text(body);
    }
    Ok(match kind {
        "workspace" => workspace_text(body),
        "status" => format!(
            "{}\n\n{}",
            workspace_text(&body["workspace"]),
            daemon_text(&body["daemon"])
        ),
        "daemon" => daemon_text(body),
        "probe" => probe_text(body),
        "harness" => harness_text(body),
        // The daemon-backed control verbs have no text view; their JSON is the contract.
        _ => json_text(body)?,
    })
}

fn field(value: &Value, name: &str) -> String {
    match &value[name] {
        Value::String(value) => safe_text(value),
        Value::Number(value) => value.to_string(),
        Value::Bool(value) => value.to_string(),
        _ => "-".to_string(),
    }
}
fn rows<'a>(value: &'a Value, name: &str) -> Vec<&'a Value> {
    value[name]
        .as_array()
        .map(|rows| rows.iter().collect())
        .unwrap_or_default()
}

fn workspace_text(value: &Value) -> String {
    let mut lines = vec![
        "VibeMux workspace | local Git, cached refs".to_string(),
        format!(
            "Branch: {}   HEAD: {}",
            field(value, "current_branch"),
            field(value, "head").chars().take(12).collect::<String>()
        ),
        format!(
            "Baseline: {}   ahead={} behind={}",
            field(value, "baseline"),
            field(value, "ahead"),
            field(value, "behind")
        ),
        format!(
            "Changes: staged={} unstaged={} untracked={}",
            field(value, "staged"),
            field(value, "unstaged"),
            field(value, "untracked")
        ),
        String::new(),
        "Branches".to_string(),
    ];
    if value["change_detection"] == "external_filters_disabled" {
        lines.insert(
            4,
            "External filters disabled; change counts may differ from normal Git status."
                .to_string(),
        );
    }
    let branches = rows(value, "branches");
    for branch in branches.iter().take(MAX_TEXT_ROWS) {
        lines.push(format!(
            "  {:<11} {:<36} {}",
            field(branch, "role"),
            field(branch, "name"),
            field(branch, "commit").chars().take(8).collect::<String>()
        ));
    }
    if branches.len() > MAX_TEXT_ROWS {
        lines.push(format!(
            "  {} more; use --format json",
            branches.len() - MAX_TEXT_ROWS
        ));
    }
    lines.push(String::new());
    lines.push("Worktrees".to_string());
    for worktree in rows(value, "worktrees").iter().take(MAX_TEXT_ROWS) {
        lines.push(format!(
            "  {} -> {}{}{}",
            field(worktree, "branch"),
            field(worktree, "location"),
            if worktree["locked"] == true {
                " [locked]"
            } else {
                ""
            },
            if worktree["prunable"] == true {
                " [prunable; inspect only]"
            } else {
                ""
            }
        ));
    }
    lines.push("No fetch, merge, branch deletion or cleanup was performed.".to_string());
    lines.join("\n")
}

fn daemon_text(value: &Value) -> String {
    let mut lines = vec![format!("Daemon | {}", field(value, "status"))];
    for name in [
        "healthy",
        "process_id",
        "queue_capacity",
        "queue_depth",
        "queue_high_watermark",
        "queue_saturated",
        "store_schema_version",
        "error_code",
        "state",
        "confirmation",
    ] {
        if !value[name].is_null() {
            lines.push(format!("  {name}: {}", field(value, name)));
        }
    }
    if value["status"].is_null() {
        lines.push("  Inspect --format json for the full recovery contract.".to_string());
    }
    lines.join("\n")
}

fn probe_text(value: &Value) -> String {
    let mut lines = vec![
        format!(
            "Diagnostic snapshot | source={} platform={}",
            field(value, "source"),
            field(value, "platform")
        ),
        "Launcher / authentication / inference are separate observations.".to_string(),
        String::new(),
    ];
    for agent in rows(value, "agents").iter().take(MAX_TEXT_ROWS) {
        lines.push(format!(
            "  {:<10} launcher={:<11} auth={:<11} inference={:<11} adapter={:<13} version={}",
            field(agent, "agent"),
            field(agent, "launcher_state"),
            field(agent, "authentication_state"),
            field(agent, "inference_state"),
            adapter_support(agent),
            field(agent, "version")
        ));
    }
    lines.push(format!(
        "Gateway: {}   A2A: {}",
        field(&value["gateway"], "state"),
        field(&value["a2a"], "state")
    ));
    lines.push(
        "Endpoint details are withheld. This is not model-generation verification.".to_string(),
    );
    lines.join("\n")
}

/// The build's structured adapter support for a probe row's agent; an agent
/// name this build does not know reads `unknown` rather than a guess.
fn adapter_support(agent: &Value) -> &'static str {
    serde_json::from_value::<AgentKind>(agent["agent"].clone())
        .map_or("unknown", |kind| adapter_capability(kind).support.as_str())
}

/// What `vibemuxctl harness` would do with one catalog route: only enabled
/// routes are looked up, a probe needs an initialize-only handshake, and
/// execution needs both an engine session and the operator's
/// `allow_execution`. Still a configuration fact, not a readiness check.
fn route_actions(route: &Value) -> &'static str {
    if route["enabled"] != true {
        return "disabled";
    }
    let probe = route["structured_probe"] == true;
    let execute = route["native_execution"] == true && route["allow_execution"] == true;
    match (execute, probe) {
        (true, true) => "execute,probe",
        (true, false) => "execute",
        (false, true) => "probe",
        (false, false) => "none",
    }
}

/// Per-kind totals over every observation, including rows the text view
/// truncates, in the shared ADR 029 kind order. A kind outside that
/// vocabulary is counted as `unrecognized`, never echoed.
fn kind_tally(observations: &[&Value]) -> Option<String> {
    if observations.is_empty() {
        return None;
    }
    let mut counts = [0_usize; ObservationKind::ALL.len()];
    let mut unrecognized = 0_usize;
    for observation in observations {
        match ObservationKind::ALL
            .iter()
            .position(|kind| observation["kind"] == kind.as_str())
        {
            Some(index) => counts[index] += 1,
            None => unrecognized += 1,
        }
    }
    let mut parts = ObservationKind::ALL
        .iter()
        .zip(counts)
        .filter(|(_, count)| *count > 0)
        .map(|(kind, count)| format!("{} {count}", kind.as_str()))
        .collect::<Vec<_>>();
    if unrecognized > 0 {
        parts.push(format!("unrecognized {unrecognized}"));
    }
    Some(format!("Kinds: {}", parts.join(", ")))
}

fn harness_text(value: &Value) -> String {
    if value["harnesses"].is_array() {
        let mut lines = vec![
            "Harness routes | configuration only, not a live readiness check".to_string(),
            format!(
                "  {:<11} {:<22} {:<8} {:<10} Actions",
                "Harness", "Protocol", "Enabled", "Execution"
            ),
        ];
        for route in rows(value, "harnesses").iter().take(MAX_TEXT_ROWS) {
            lines.push(format!(
                "  {:<11} {:<22} {:<8} {:<10} {}",
                field(route, "harness"),
                field(route, "protocol"),
                field(route, "enabled"),
                field(route, "allow_execution"),
                route_actions(route)
            ));
        }
        lines.push(
            "Actions combine each route's settings with this build's protocol engine; a probe sends no prompt."
                .to_string(),
        );
        lines.push(
            "Native execution requires explicit local configuration and authorization.".to_string(),
        );
        return lines.join("\n");
    }
    // A replay report has no harness process: name the protocol and omit the
    // process fields rather than printing placeholders for them.
    let mut lines = if value["completion"].is_object() {
        let completion = &value["completion"];
        let mut lines = vec![
            format!(
                "Harness {} | {}",
                field(value, "harness"),
                field(completion, "status")
            ),
            format!(
                "Records: {}   Stdout bytes: {}   Stderr bytes: {}   Native exit: {}",
                field(completion, "record_count"),
                field(completion, "stdout_bytes"),
                field(completion, "stderr_bytes"),
                field(completion, "exit_code")
            ),
        ];
        // A killed process still reports an exit code, but it is the kill's,
        // not the vendor's.
        if completion["forced_termination"] == true {
            lines.push(
                "Forced termination: the native process was killed after a deadline, cancellation, protocol error, or missed shutdown grace period, so its exit code is not the vendor's."
                    .to_string(),
            );
        }
        lines
    } else {
        vec![
            format!(
                "Replay {} | {}",
                field(value, "protocol"),
                field(value, "status")
            ),
            format!("Records: {}", field(value, "record_count")),
        ]
    };
    let completion = if value["completion"].is_object() {
        &value["completion"]
    } else {
        value
    };
    if !completion["error_code"].is_null() {
        lines.push(format!("Error: {}", field(completion, "error_code")));
    }
    let observations = rows(value, "observations");
    if let Some(tally) = kind_tally(&observations) {
        lines.push(tally);
    }
    if observations
        .iter()
        .any(|observation| observation["kind"] == ObservationKind::Opaque.as_str())
    {
        lines.push(
            "Opaque records are kept intact but not classified by this build: RPC replies, or vendor events without a verified contract. A growing share can mean the vendor output changed."
                .to_string(),
        );
    }
    for observation in observations.iter().take(MAX_TEXT_ROWS) {
        lines.push(format!(
            "  {:>4} {:<12} {} bytes",
            field(observation, "sequence"),
            field(observation, "kind"),
            field(observation, "bytes")
        ));
    }
    let count = observations.len();
    if count > MAX_TEXT_ROWS {
        lines.push(format!(
            "  {} more metadata records; use --format json",
            count - MAX_TEXT_ROWS
        ));
    }
    lines
        .push("Raw prompt, vendor content, and local configuration are not displayed.".to_string());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn automatic_output_preserves_json_for_pipes() {
        assert_eq!(resolve(Format::Auto, false), Format::Json);
        assert_eq!(resolve(Format::Auto, true), Format::Text);
    }
    #[test]
    fn json_escapes_terminal_controls_without_changing_data() {
        let value = json!({"branch":"topic\u{009b}31m\u{202e}text"});
        let encoded = json_text(&value).unwrap();
        assert!(!encoded.contains('\u{009b}'));
        assert!(!encoded.contains('\u{202e}'));
        assert_eq!(serde_json::from_str::<Value>(&encoded).unwrap(), value);
    }
    #[test]
    fn probe_text_names_adapter_support_per_agent() {
        let text = probe_text(&json!({
            "source": "local_probe",
            "platform": "windows",
            "agents": [
                {"agent": "claude", "launcher_state": "verified"},
                {"agent": "grok", "launcher_state": "verified"},
                {"agent": "kimi", "launcher_state": "unavailable"},
                {"agent": "future_vendor", "launcher_state": "verified"}
            ],
            "gateway": {"state": "not_run"},
            "a2a": {"state": "not_run"}
        }));
        let adapter = |agent: &str| {
            text.lines()
                .find(|line| line.trim_start().starts_with(agent))
                .and_then(|line| line.split("adapter=").nth(1))
                .and_then(|rest| rest.split_whitespace().next())
                .map(str::to_string)
        };
        assert_eq!(adapter("claude").as_deref(), Some("execute"));
        assert_eq!(adapter("grok").as_deref(), Some("probe_only"));
        assert_eq!(adapter("kimi").as_deref(), Some("terminal_only"));
        assert_eq!(adapter("future_vendor").as_deref(), Some("unknown"));
    }
    #[test]
    fn harness_catalog_text_states_what_each_route_can_do() {
        let route = |harness: &str, protocol: &str, enabled, allow, probe, native| {
            json!({"harness": harness, "protocol": protocol, "enabled": enabled,
                "allow_execution": allow, "structured_probe": probe,
                "native_execution": native})
        };
        let text = harness_text(&json!({"schema_version": 1, "harnesses": [
            route("claude", "claude", true, true, true, true),
            route("codex", "codex", true, true, false, true),
            route("grok", "acp", true, false, true, true),
            route("copilot", "acp", false, true, true, true),
            route("opencode", "opencode", true, true, false, false),
        ]}));
        let actions = |harness: &str| {
            text.lines()
                .find(|line| line.trim_start().starts_with(harness))
                .and_then(|line| line.split_whitespace().last())
                .map(str::to_string)
        };
        assert_eq!(actions("claude").as_deref(), Some("execute,probe"));
        assert_eq!(actions("codex").as_deref(), Some("execute"));
        assert_eq!(actions("grok").as_deref(), Some("probe"));
        assert_eq!(actions("copilot").as_deref(), Some("disabled"));
        assert_eq!(actions("opencode").as_deref(), Some("none"));
        let header = text.lines().nth(1).unwrap();
        let claude = text.lines().nth(2).unwrap();
        assert_eq!(header.find("Actions"), claude.find("execute,probe"));
        // Both guides list every value the column can show.
        for doc in [
            include_str!("../../../docs/guides/harness_core_guide.md"),
            include_str!("../../../docs/protocol_boundaries.md"),
        ] {
            for value in ["disabled", "none", "probe", "execute", "execute,probe"] {
                assert!(doc.contains(&format!("`{value}`")), "{value}");
            }
        }
    }
    #[test]
    fn harness_report_text_tallies_every_kind_including_truncated_rows() {
        let observation =
            |sequence: usize, kind: &str| json!({"sequence": sequence, "kind": kind, "bytes": 10});
        let mut observations = vec![observation(1, "started")];
        observations.extend((2..=36).map(|sequence| observation(sequence, "text")));
        // Rows past the 32-row text limit still count.
        observations.push(observation(37, "opaque"));
        observations.push(observation(38, "opaque"));
        observations.push(observation(39, "SYNTHETIC_KIND\u{1b}[31m"));
        observations.push(observation(40, "completed"));
        let report = |observations: Vec<Value>| {
            harness_text(&json!({"schema_version": 1, "harness": "codex",
                "completion": {"status": "completed", "record_count": observations.len(),
                    "stdout_bytes": 400, "exit_code": 0, "error_code": null},
                "observations": observations}))
        };
        let text = report(observations);
        assert!(
            text.lines()
                .any(|line| line
                    == "Kinds: started 1, text 35, completed 1, opaque 2, unrecognized 1"),
            "{text}"
        );
        assert!(text.contains("Opaque records are kept intact"));
        assert!(text.contains("8 more metadata records"));
        assert!(!text.contains("SYNTHETIC_KIND"));
        assert!(!text.contains('\u{1b}'));

        let text = report(vec![observation(1, "started"), observation(2, "completed")]);
        assert!(
            text.lines()
                .any(|line| line == "Kinds: started 1, completed 1")
        );
        assert!(!text.contains("Opaque records"));
        let text = report(Vec::new());
        assert!(!text.contains("Kinds:"));

        let replay = harness_text(&json!({"schema_version": 1, "status": "replayed",
            "protocol": "acp", "record_count": 1, "observations": [observation(1, "opaque")]}));
        assert!(
            replay.starts_with("Replay acp | replayed\nRecords: 1\nKinds: opaque 1\n"),
            "{replay}"
        );
        assert!(!replay.contains("Native exit"));
    }

    /// Renders the native crate's own report type, so a renamed completion
    /// field fails here instead of silently printing `-`.
    #[test]
    fn harness_outcome_text_reads_every_native_completion_field() {
        use vibemux_harness_native::contract::{
            Completion, Harness, Outcome, Protocol, Record, SCHEMA_VERSION, Status,
        };
        let records = [
            r#"{"type":"thread.started","thread_id":"SYNTHETIC_THREAD"}"#,
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"SYNTHETIC_VENDOR_TEXT"}}"#,
        ]
        .into_iter()
        .zip(1..)
        .map(|(raw, sequence)| Record::parse(Protocol::Codex, sequence, raw.to_string()))
        .collect::<Result<Vec<_>, _>>()
        .expect("fixture records");
        let request_id = uuid::Uuid::new_v4();
        let outcome = |forced: bool| Outcome {
            request_id,
            harness: Harness::Codex,
            records: records.clone(),
            completion: Completion {
                schema_version: SCHEMA_VERSION,
                status: if forced {
                    Status::Failed
                } else {
                    Status::Completed
                },
                record_count: 2,
                stdout_bytes: 123,
                stderr_bytes: 45,
                exit_code: Some(i32::from(forced)),
                forced_termination: forced,
                error_code: forced.then(|| "harness_deadline_exceeded".to_string()),
            },
        };

        let text = harness_text(&outcome(true).summary());
        let lines = text.lines().collect::<Vec<_>>();
        assert_eq!(lines[0], "Harness codex | failed", "{text}");
        assert_eq!(
            lines[1], "Records: 2   Stdout bytes: 123   Stderr bytes: 45   Native exit: 1",
            "{text}"
        );
        assert!(lines[2].starts_with("Forced termination: "), "{text}");
        assert_eq!(lines[3], "Error: harness_deadline_exceeded", "{text}");
        assert!(lines[4].starts_with("Kinds: "), "{text}");
        assert!(!text.contains("SYNTHETIC_"), "{text}");
        assert!(!text.contains(&request_id.to_string()), "{text}");

        let text = harness_text(&outcome(false).summary());
        assert!(text.starts_with(
            "Harness codex | completed\nRecords: 2   Stdout bytes: 123   Stderr bytes: 45   Native exit: 0\nKinds: "
        ), "{text}");
        assert!(!text.contains("Forced termination"), "{text}");
        assert!(!text.contains("Error:"), "{text}");
    }
}
