//! Consistent terminal text and JSON without terminal-control injection.
use crate::{command::Format, failure::Failure};
use serde_json::{Value, json};
use std::fmt::Write;
use vibemux_frontend::safe_text;

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
    Ok(output)
}

pub fn error_text(error: &Failure, format: Format) -> String {
    if format == Format::Json {
        json!({"ok":false,"error_code":error.code()}).to_string()
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
        _ => "Use --format json for structured output.".to_string(),
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
            "  {:<10} launcher={:<11} auth={:<11} inference={:<11} version={}",
            field(agent, "agent"),
            field(agent, "launcher_state"),
            field(agent, "authentication_state"),
            field(agent, "inference_state"),
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

fn harness_text(value: &Value) -> String {
    if value["harnesses"].is_array() {
        let mut lines = vec![
            "Harness routes | configuration only, not a live readiness check".to_string(),
            "  Harness     Protocol               Enabled  Execution".to_string(),
        ];
        for route in rows(value, "harnesses").iter().take(MAX_TEXT_ROWS) {
            lines.push(format!(
                "  {:<11} {:<22} {:<8} {}",
                field(route, "harness"),
                field(route, "protocol"),
                field(route, "enabled"),
                field(route, "allow_execution")
            ));
        }
        lines.push(
            "Native execution requires explicit local configuration and authorization.".to_string(),
        );
        return lines.join("\n");
    }
    let completion = if value["completion"].is_object() {
        &value["completion"]
    } else {
        value
    };
    let mut lines = vec![
        format!(
            "Harness {} | {}",
            field(value, "harness"),
            field(completion, "status")
        ),
        format!(
            "Records: {}   Bytes: {}   Native exit: {}",
            field(completion, "record_count"),
            field(completion, "stdout_bytes"),
            field(completion, "exit_code")
        ),
    ];
    if !completion["error_code"].is_null() {
        lines.push(format!("Error: {}", field(completion, "error_code")));
    }
    for observation in rows(value, "observations").iter().take(MAX_TEXT_ROWS) {
        lines.push(format!(
            "  {:>4} {:<12} {} bytes",
            field(observation, "sequence"),
            field(observation, "kind"),
            field(observation, "bytes")
        ));
    }
    let count = rows(value, "observations").len();
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
}
