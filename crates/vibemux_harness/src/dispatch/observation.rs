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
