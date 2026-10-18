//! Conservative classification of vendor JSON records; raw bytes stay intact.
//!
//! Only verified discriminants are classified. A tool step, content block, or
//! auxiliary RPC error never completes a request, and unknown or future
//! events remain `Opaque`. RPC correlation, process exit, and whole-request
//! policy belong to [`super::protocol_session`] and [`super::outcome`].

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{DispatchError, NativeProtocol};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationKind {
    Started,
    Text,
    Tool,
    Approval,
    Usage,
    Completed,
    Failed,
    Cancelled,
    Opaque,
}

impl ObservationKind {
    pub const ALL: [Self; 9] = [
        Self::Started,
        Self::Text,
        Self::Tool,
        Self::Approval,
        Self::Usage,
        Self::Completed,
        Self::Failed,
        Self::Cancelled,
        Self::Opaque,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Text => "text",
            Self::Tool => "tool",
            Self::Approval => "approval",
            Self::Usage => "usage",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Opaque => "opaque",
        }
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// One complete vendor JSON record, byte-exact, including unknown fields.
///
/// Deliberately not `Serialize`: raw vendor content leaves the daemon only
/// through the explicit transcript output operation, never through events,
/// logs, or `Debug`.
#[derive(Clone, Eq, PartialEq)]
pub struct ObservedRecord {
    sequence: u64,
    kind: ObservationKind,
    raw_json: String,
}

impl ObservedRecord {
    /// Parses one frame as a JSON object and classifies it. Returns the parsed
    /// value too, so the session does not parse the frame a second time.
    pub fn parse(
        protocol: NativeProtocol,
        sequence: u64,
        raw_json: String,
    ) -> Result<(Self, Value), DispatchError> {
        if sequence == 0 {
            return Err(DispatchError::Internal);
        }
        let value: Value =
            serde_json::from_str(&raw_json).map_err(|_| DispatchError::InvalidJson)?;
        if !value.is_object() {
            return Err(DispatchError::InvalidRecord);
        }
        let kind = classify(protocol, &value);
        Ok((
            Self {
                sequence,
                kind,
                raw_json,
            },
            value,
        ))
    }

    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    #[must_use]
    pub const fn kind(&self) -> ObservationKind {
        self.kind
    }

    #[must_use]
    pub fn raw_json(&self) -> &str {
        &self.raw_json
    }

    #[must_use]
    pub fn byte_count(&self) -> usize {
        self.raw_json.len()
    }
}

impl fmt::Debug for ObservedRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ObservedRecord")
            .field("sequence", &self.sequence)
            .field("kind", &self.kind)
            .field("bytes", &self.raw_json.len())
            .finish()
    }
}

/// Classify only verified discriminants; future extensions remain opaque.
#[must_use]
pub fn classify(protocol: NativeProtocol, value: &Value) -> ObservationKind {
    match protocol {
        NativeProtocol::CodexExec => classify_codex_exec(value),
        NativeProtocol::CodexAppServer => classify_codex_app_server(value),
        NativeProtocol::ClaudeStreamJson => classify_claude(value),
        NativeProtocol::Acp => classify_acp(value),
        NativeProtocol::DshSdkJsonRpc => classify_dsh(value),
    }
}

// Contract: openai/codex sdk/typescript/src/{events,items}.ts.
fn classify_codex_exec(value: &Value) -> ObservationKind {
    match value["type"].as_str() {
        Some("thread.started" | "turn.started") => ObservationKind::Started,
        Some("turn.completed") => ObservationKind::Completed,
        Some("turn.failed" | "error") => ObservationKind::Failed,
        Some("item.started" | "item.updated" | "item.completed") => {
            match value["item"]["type"].as_str() {
                Some("agent_message" | "reasoning") => ObservationKind::Text,
                Some("command_execution" | "file_change" | "mcp_tool_call" | "web_search") => {
                    ObservationKind::Tool
                }
                // The SDK defines an error item as nonfatal.
                _ => ObservationKind::Opaque,
            }
        }
        _ => ObservationKind::Opaque,
    }
}

// Contract: Codex app-server protocol (thread/turn/item notifications).
fn classify_codex_app_server(value: &Value) -> ObservationKind {
    match value["method"].as_str() {
        Some("thread/started" | "turn/started") => ObservationKind::Started,
        Some("turn/completed") => match value["params"]["turn"]["status"].as_str() {
            Some("completed") => ObservationKind::Completed,
            Some("failed") => ObservationKind::Failed,
            Some("interrupted") => ObservationKind::Cancelled,
            _ => ObservationKind::Opaque,
        },
        Some("thread/tokenUsage/updated") => ObservationKind::Usage,
        Some(
            "item/commandExecution/requestApproval"
            | "item/fileChange/requestApproval"
            | "item/permissions/requestApproval"
            | "item/tool/requestUserInput"
            | "mcpServer/elicitation/request",
        ) => ObservationKind::Approval,
        Some("item/agentMessage/delta" | "item/plan/delta") => ObservationKind::Text,
        Some(
            "item/commandExecution/outputDelta" | "item/fileChange/outputDelta" | "item/tool/call",
        ) => ObservationKind::Tool,
        Some("item/started" | "item/completed") => match value["params"]["item"]["type"].as_str() {
            Some("agentMessage" | "plan") => ObservationKind::Text,
            Some(
                "commandExecution" | "fileChange" | "mcpToolCall" | "dynamicToolCall"
                | "collabToolCall" | "webSearch" | "imageView",
            ) => ObservationKind::Tool,
            _ => ObservationKind::Opaque,
        },
        // An error notification or a failed auxiliary RPC is not a turn result.
        _ => ObservationKind::Opaque,
    }
}

// Contracts: Claude Code headless stream-json and the Claude Agent SDK
// control protocol.
fn classify_claude(value: &Value) -> ObservationKind {
    match value["type"].as_str() {
        Some("system") if value["subtype"] == "init" => ObservationKind::Started,
        Some("result") => classify_claude_result(value),
        Some("control_request") if value["request"]["subtype"] == "can_use_tool" => {
            ObservationKind::Approval
        }
        Some("assistant" | "user") => {
            let Some(content) = value["message"]["content"].as_array() else {
                return ObservationKind::Opaque;
            };
            if content
                .iter()
                .any(|block| matches!(block["type"].as_str(), Some("tool_use" | "tool_result")))
            {
                ObservationKind::Tool
            } else if value["type"] == "assistant"
                && content.iter().any(|block| block["type"] == "text")
            {
                ObservationKind::Text
            } else {
                ObservationKind::Opaque
            }
        }
        Some("stream_event") => classify_claude_stream(&value["event"]),
        _ => ObservationKind::Opaque,
    }
}

fn classify_claude_result(value: &Value) -> ObservationKind {
    // Nested agent results cannot terminate the root request.
    if !value["parent_tool_use_id"].is_null() {
        return ObservationKind::Opaque;
    }
    match value["terminal_reason"].as_str() {
        Some("aborted_streaming" | "aborted_tools") => return ObservationKind::Cancelled,
        Some("max_turns") => return ObservationKind::Failed,
        _ => {}
    }
    if value["is_error"] == true {
        return ObservationKind::Failed;
    }
    match value["subtype"].as_str() {
        Some("success")
            if (value["is_error"].is_null() || value["is_error"] == false)
                && (value["terminal_reason"].is_null()
                    || value["terminal_reason"] == "completed") =>
        {
            ObservationKind::Completed
        }
        Some("error_during_execution" | "error_max_turns") => ObservationKind::Failed,
        _ => ObservationKind::Opaque,
    }
}

fn classify_claude_stream(event: &Value) -> ObservationKind {
    match event["type"].as_str() {
        Some("content_block_delta") => match event["delta"]["type"].as_str() {
            Some("text_delta") => ObservationKind::Text,
            Some("input_json_delta") => ObservationKind::Tool,
            _ => ObservationKind::Opaque,
        },
        Some("content_block_start") if event["content_block"]["type"] == "tool_use" => {
            ObservationKind::Tool
        }
        Some("message_delta") if event["usage"].is_object() => ObservationKind::Usage,
        // message_stop and content_block_stop do not end the CLI request.
        _ => ObservationKind::Opaque,
    }
}

// Contract: agentclientprotocol/agent-client-protocol schema v1.
fn classify_acp(value: &Value) -> ObservationKind {
    match value["method"].as_str() {
        Some("session/request_permission") => ObservationKind::Approval,
        Some("session/update") => {
            let update = &value["params"]["update"];
            match update["sessionUpdate"].as_str() {
                Some("agent_message_chunk" | "agent_thought_chunk")
                    if update["content"]["type"] == "text" =>
                {
                    ObservationKind::Text
                }
                Some("tool_call" | "tool_call_update") => ObservationKind::Tool,
                Some("usage_update") => ObservationKind::Usage,
                _ => ObservationKind::Opaque,
            }
        }
        None if value.get("method").is_none() => match value["result"]["stopReason"].as_str() {
            Some("end_turn") => ObservationKind::Completed,
            Some("cancelled") => ObservationKind::Cancelled,
            Some("max_tokens" | "max_turn_requests" | "refusal") => ObservationKind::Failed,
            _ => ObservationKind::Opaque,
        },
        // session/cancel is a request to cancel, not a cancellation result.
        _ => ObservationKind::Opaque,
    }
}

// Contract: @deepseek-ai/dsh-sdk-protocol (newline-delimited JSON-RPC 2.0 over
// stdio) and @deepseek-ai/dsh-session `SessionEventMap` (ADR 032).
//
// `session.status` is deliberately never terminal: an idle whole-agent state is
// not proof of task completion (AGENTS.md §2 invariant 5). Only `turn/end`
// carries a structured reason, and an unrecognized reason stays `Opaque` so a
// merge-extensible vocabulary degrades instead of failing.
fn classify_dsh(value: &Value) -> ObservationKind {
    match value["method"].as_str() {
        Some("session.event") => match value["params"]["event"]["type"].as_str() {
            Some("turn/start") => ObservationKind::Started,
            Some("turn/end") => classify_dsh_turn_end(value),
            Some("assistant/message" | "assistant/chunk") => ObservationKind::Text,
            Some("tool/call" | "tool/result") => ObservationKind::Tool,
            Some("approval/asked") => ObservationKind::Approval,
            _ => ObservationKind::Opaque,
        },
        // Progress, subagent lifecycle, and handshake notifications are not
        // turn results.
        _ => ObservationKind::Opaque,
    }
}

fn classify_dsh_turn_end(value: &Value) -> ObservationKind {
    match value["params"]["event"]["data"]["reason"]["kind"].as_str() {
        Some("completed") => ObservationKind::Completed,
        Some("aborted") => ObservationKind::Cancelled,
        Some("blocked" | "error" | "max-tokens" | "interrupted") => ObservationKind::Failed,
        _ => ObservationKind::Opaque,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    use ObservationKind::{
        Approval, Cancelled, Completed, Failed, Opaque, Started, Text, Tool, Usage,
    };

    fn assert_table(protocol: NativeProtocol, table: &[(Value, ObservationKind)]) {
        for (payload, expected) in table {
            assert_eq!(classify(protocol, payload), *expected, "{payload}");
        }
    }

    #[test]
    fn kind_names_match_their_serde_form_and_only_results_are_terminal() {
        let names = [
            "started",
            "text",
            "tool",
            "approval",
            "usage",
            "completed",
            "failed",
            "cancelled",
            "opaque",
        ];
        for (kind, name) in ObservationKind::ALL.into_iter().zip(names) {
            assert_eq!(kind.as_str(), name);
            assert_eq!(serde_json::to_value(kind).ok(), Some(json!(name)));
            assert_eq!(
                kind.is_terminal(),
                [Completed, Failed, Cancelled].contains(&kind),
                "{name}"
            );
        }
    }

    #[test]
    fn parse_keeps_the_exact_bytes_and_refuses_bad_frames_by_code() {
        let raw = r#"{"type":"turn.completed","secret_field":"do_not_print"}"#;
        let Ok((record, value)) =
            ObservedRecord::parse(NativeProtocol::CodexExec, 7, raw.to_owned())
        else {
            panic!("a JSON object frame must parse");
        };
        assert_eq!(record.sequence(), 7);
        assert_eq!(record.kind(), Completed);
        assert_eq!(record.raw_json(), raw);
        assert_eq!(record.byte_count(), raw.len());
        assert_eq!(value["secret_field"], "do_not_print");
        assert_eq!(
            format!("{record:?}"),
            format!(
                "ObservedRecord {{ sequence: 7, kind: Completed, bytes: {} }}",
                raw.len()
            )
        );
        let refused = |sequence: u64, frame: &str| {
            ObservedRecord::parse(NativeProtocol::CodexExec, sequence, frame.to_owned()).err()
        };
        assert!(matches!(refused(0, raw), Some(DispatchError::Internal)));
        assert!(matches!(refused(1, "{"), Some(DispatchError::InvalidJson)));
        for frame in ["[]", "7", "\"text\"", "null"] {
            assert!(
                matches!(refused(1, frame), Some(DispatchError::InvalidRecord)),
                "{frame}"
            );
        }
    }

    #[test]
    fn codex_exec_classifies_every_verified_discriminant() {
        let mut table = vec![
            (json!({"type": "thread.started"}), Started),
            (json!({"type": "turn.started"}), Started),
            (json!({"type": "turn.completed"}), Completed),
            (json!({"type": "turn.failed"}), Failed),
            (json!({"type": "error"}), Failed),
            (json!({"type": "item.future"}), Opaque),
        ];
        for event in ["item.started", "item.updated", "item.completed"] {
            for (item_type, expected) in [
                ("agent_message", Text),
                ("reasoning", Text),
                ("command_execution", Tool),
                ("file_change", Tool),
                ("mcp_tool_call", Tool),
                ("web_search", Tool),
                ("error", Opaque),
                ("future_item", Opaque),
            ] {
                table.push((
                    json!({"type": event, "item": {"type": item_type}}),
                    expected,
                ));
            }
        }
        assert_table(NativeProtocol::CodexExec, &table);
    }

    #[test]
    fn codex_app_server_classifies_every_verified_discriminant() {
        let mut table = vec![
            (json!({"method": "thread/started"}), Started),
            (json!({"method": "turn/started"}), Started),
            (json!({"method": "thread/tokenUsage/updated"}), Usage),
            (json!({"method": "item/agentMessage/delta"}), Text),
            (json!({"method": "item/plan/delta"}), Text),
            (json!({"method": "item/commandExecution/outputDelta"}), Tool),
            (json!({"method": "item/fileChange/outputDelta"}), Tool),
            (json!({"method": "item/tool/call"}), Tool),
            (json!({"method": "error"}), Opaque),
        ];
        for method in [
            "item/commandExecution/requestApproval",
            "item/fileChange/requestApproval",
            "item/permissions/requestApproval",
            "item/tool/requestUserInput",
            "mcpServer/elicitation/request",
        ] {
            table.push((json!({"method": method}), Approval));
        }
        for (status, expected) in [
            ("completed", Completed),
            ("failed", Failed),
            ("interrupted", Cancelled),
            ("inProgress", Opaque),
        ] {
            table.push((
                json!({"method": "turn/completed", "params": {"turn": {"status": status}}}),
                expected,
            ));
        }
        for method in ["item/started", "item/completed"] {
            for (item_type, expected) in [
                ("agentMessage", Text),
                ("plan", Text),
                ("commandExecution", Tool),
                ("fileChange", Tool),
                ("mcpToolCall", Tool),
                ("dynamicToolCall", Tool),
                ("collabToolCall", Tool),
                ("webSearch", Tool),
                ("imageView", Tool),
                ("userMessage", Opaque),
            ] {
                table.push((
                    json!({"method": method, "params": {"item": {"type": item_type}}}),
                    expected,
                ));
            }
        }
        assert_table(NativeProtocol::CodexAppServer, &table);
    }

    #[test]
    fn claude_classifies_every_verified_discriminant() {
        let message = |role: &str, block: &str| json!({"type": role, "message": {"content": [{"type": block}]}});
        let stream = |event: Value| json!({"type": "stream_event", "event": event});
        let table = [
            (json!({"type": "system", "subtype": "init"}), Started),
            (
                json!({"type": "system", "subtype": "compact_boundary"}),
                Opaque,
            ),
            (
                json!({"type": "control_request", "request": {"subtype": "can_use_tool"}}),
                Approval,
            ),
            (
                json!({"type": "control_request", "request": {"subtype": "interrupt"}}),
                Opaque,
            ),
            (message("assistant", "text"), Text),
            (message("assistant", "tool_use"), Tool),
            (message("assistant", "thinking"), Opaque),
            (message("user", "tool_result"), Tool),
            (message("user", "text"), Opaque),
            (
                json!({"type": "assistant", "message": {"content": "text"}}),
                Opaque,
            ),
            (
                stream(json!({"type": "content_block_delta", "delta": {"type": "text_delta"}})),
                Text,
            ),
            (
                stream(
                    json!({"type": "content_block_delta", "delta": {"type": "input_json_delta"}}),
                ),
                Tool,
            ),
            (
                stream(json!({"type": "content_block_delta", "delta": {"type": "thinking_delta"}})),
                Opaque,
            ),
            (
                stream(
                    json!({"type": "content_block_start", "content_block": {"type": "tool_use"}}),
                ),
                Tool,
            ),
            (
                stream(json!({"type": "content_block_start", "content_block": {"type": "text"}})),
                Opaque,
            ),
            (stream(json!({"type": "message_delta", "usage": {}})), Usage),
            (stream(json!({"type": "message_delta"})), Opaque),
            (stream(json!({"type": "message_stop"})), Opaque),
        ];
        assert_table(NativeProtocol::ClaudeStreamJson, &table);
    }

    #[test]
    fn claude_results_need_root_scope_and_a_clean_success() {
        let result = |fields: Value| {
            let mut value = json!({"type": "result"});
            if let (Some(target), Some(source)) = (value.as_object_mut(), fields.as_object()) {
                target.extend(source.clone());
            }
            value
        };
        let table = [
            (result(json!({"subtype": "success"})), Completed),
            (
                result(
                    json!({"subtype": "success", "is_error": false, "terminal_reason": "completed"}),
                ),
                Completed,
            ),
            (
                result(json!({"subtype": "success", "terminal_reason": "future_reason"})),
                Opaque,
            ),
            (
                result(json!({"subtype": "success", "is_error": "false"})),
                Opaque,
            ),
            (result(json!({"subtype": "error_during_execution"})), Failed),
            (result(json!({"subtype": "error_max_turns"})), Failed),
            (
                result(json!({"subtype": "future_error", "is_error": true})),
                Failed,
            ),
            (result(json!({"subtype": "future_error"})), Opaque),
            (
                result(json!({"terminal_reason": "aborted_streaming"})),
                Cancelled,
            ),
            (
                result(json!({"terminal_reason": "aborted_tools"})),
                Cancelled,
            ),
            (result(json!({"terminal_reason": "max_turns"})), Failed),
            (
                result(json!({"subtype": "error_max_turns", "parent_tool_use_id": "nested"})),
                Opaque,
            ),
            (
                result(json!({"terminal_reason": "aborted_tools", "parent_tool_use_id": "nested"})),
                Opaque,
            ),
        ];
        assert_table(NativeProtocol::ClaudeStreamJson, &table);
    }

    #[test]
    fn acp_classifies_every_verified_discriminant() {
        let update = |kind: &str, content: Value| {
            json!({
                "method": "session/update",
                "params": {"update": {"sessionUpdate": kind, "content": content}}
            })
        };
        let mut table = vec![
            (json!({"method": "session/request_permission"}), Approval),
            (update("agent_message_chunk", json!({"type": "text"})), Text),
            (update("agent_thought_chunk", json!({"type": "text"})), Text),
            (
                update("agent_message_chunk", json!({"type": "image"})),
                Opaque,
            ),
            (update("agent_thought_chunk", Value::Null), Opaque),
            (update("tool_call", Value::Null), Tool),
            (update("tool_call_update", Value::Null), Tool),
            (update("usage_update", Value::Null), Usage),
            (update("plan", Value::Null), Opaque),
            (json!({"method": "session/cancel"}), Opaque),
            (
                json!({"method": null, "result": {"stopReason": "end_turn"}}),
                Opaque,
            ),
            (
                json!({"method": 5, "result": {"stopReason": "end_turn"}}),
                Opaque,
            ),
        ];
        for (stop_reason, expected) in [
            ("end_turn", Completed),
            ("cancelled", Cancelled),
            ("max_tokens", Failed),
            ("max_turn_requests", Failed),
            ("refusal", Failed),
            ("future_stop", Opaque),
        ] {
            table.push((
                json!({"id": 3, "result": {"stopReason": stop_reason}}),
                expected,
            ));
        }
        assert_table(NativeProtocol::Acp, &table);
    }

    #[test]
    fn dsh_classifies_turn_events_and_never_ends_on_status() {
        let event = |event: Value| json!({"method": "session.event", "params": {"event": event}});
        let mut table = vec![
            (event(json!({"type": "turn/start"})), Started),
            (event(json!({"type": "assistant/message"})), Text),
            (event(json!({"type": "assistant/chunk"})), Text),
            (event(json!({"type": "tool/call"})), Tool),
            (event(json!({"type": "tool/result"})), Tool),
            (event(json!({"type": "approval/asked"})), Approval),
            (event(json!({"type": "subagent/start"})), Opaque),
            (event(json!({"type": "turn/end"})), Opaque),
            (
                json!({"method": "session.status", "params": {"status": "idle"}}),
                Opaque,
            ),
            (
                json!({"method": "turn/end", "params": {"event": {"type": "turn/end"}}}),
                Opaque,
            ),
        ];
        for (reason, expected) in [
            ("completed", Completed),
            ("aborted", Cancelled),
            ("blocked", Failed),
            ("error", Failed),
            ("max-tokens", Failed),
            ("interrupted", Failed),
            ("future_reason", Opaque),
        ] {
            table.push((
                event(json!({"type": "turn/end", "data": {"reason": {"kind": reason}}})),
                expected,
            ));
        }
        assert_table(NativeProtocol::DshSdkJsonRpc, &table);
    }
}
