//! Conservative observations over vendor JSON; raw records remain untouched.
//!
//! A tool, content block, or OpenCode step ending never completes a request.
//! RPC correlation, process exit, and whole-request policy belong to transport.

use serde_json::Value;

use crate::contract::{Kind, Protocol};

/// Classify only verified discriminants; future extensions remain opaque.
pub fn classify(protocol: Protocol, value: &Value) -> Kind {
    match protocol {
        Protocol::Codex => classify_codex(value),
        Protocol::Appserver => classify_appserver(value),
        Protocol::Claude => classify_claude(value),
        Protocol::Opencode => classify_opencode(value),
        Protocol::Acp => classify_acp(value),
        // No verified public event contract is assumed for these direct CLIs.
        Protocol::Copilot | Protocol::Grok => Kind::Opaque,
    }
}

// Official contract: openai/codex sdk/typescript/src/{events,items}.ts.
fn classify_codex(value: &Value) -> Kind {
    match value["type"].as_str() {
        Some("thread.started" | "turn.started") => Kind::Started,
        Some("turn.completed") => Kind::Completed,
        Some("turn.failed" | "error") => Kind::Failed,
        Some("item.started" | "item.updated" | "item.completed") => {
            match value["item"]["type"].as_str() {
                Some("agent_message" | "reasoning") => Kind::Text,
                Some("command_execution" | "file_change" | "mcp_tool_call" | "web_search") => {
                    Kind::Tool
                }
                // The SDK explicitly defines an error item as nonfatal.
                _ => Kind::Opaque,
            }
        }
        _ => Kind::Opaque,
    }
}

// Official contract: https://learn.chatgpt.com/docs/app-server.
fn classify_appserver(value: &Value) -> Kind {
    match value["method"].as_str() {
        Some("thread/started" | "turn/started") => Kind::Started,
        Some("turn/completed") => match value["params"]["turn"]["status"].as_str() {
            Some("completed") => Kind::Completed,
            Some("failed") => Kind::Failed,
            Some("interrupted") => Kind::Cancelled,
            _ => Kind::Opaque,
        },
        Some("thread/tokenUsage/updated") => Kind::Usage,
        Some(
            "item/commandExecution/requestApproval"
            | "item/fileChange/requestApproval"
            | "item/permissions/requestApproval"
            | "item/tool/requestUserInput"
            | "mcpServer/elicitation/request",
        ) => Kind::Approval,
        Some("item/agentMessage/delta" | "item/plan/delta") => Kind::Text,
        Some(
            "item/commandExecution/outputDelta" | "item/fileChange/outputDelta" | "item/tool/call",
        ) => Kind::Tool,
        Some("item/started" | "item/completed") => match value["params"]["item"]["type"].as_str() {
            Some("agentMessage" | "plan") => Kind::Text,
            Some(
                "commandExecution" | "fileChange" | "mcpToolCall" | "dynamicToolCall"
                | "collabToolCall" | "webSearch" | "imageView",
            ) => Kind::Tool,
            _ => Kind::Opaque,
        },
        // An error notification or a failed auxiliary RPC is not a turn result.
        _ => Kind::Opaque,
    }
}

// Official contracts: code.claude.com/docs/en/headless and
// anthropics/claude-agent-sdk-python src/claude_agent_sdk/{types,_internal/query}.py.
fn classify_claude(value: &Value) -> Kind {
    match value["type"].as_str() {
        Some("system") if value["subtype"] == "init" => Kind::Started,
        Some("result") => classify_claude_result(value),
        Some("control_request") if value["request"]["subtype"] == "can_use_tool" => Kind::Approval,
        Some("assistant" | "user") => {
            let Some(content) = value["message"]["content"].as_array() else {
                return Kind::Opaque;
            };
            if content
                .iter()
                .any(|block| matches!(block["type"].as_str(), Some("tool_use" | "tool_result")))
            {
                Kind::Tool
            } else if value["type"] == "assistant"
                && content.iter().any(|block| block["type"] == "text")
            {
                Kind::Text
            } else {
                Kind::Opaque
            }
        }
        Some("stream_event") => classify_claude_stream(&value["event"]),
        _ => Kind::Opaque,
    }
}

fn classify_claude_result(value: &Value) -> Kind {
    // Nested agent results cannot terminate the root request.
    if !value["parent_tool_use_id"].is_null() {
        return Kind::Opaque;
    }
    match value["terminal_reason"].as_str() {
        Some("aborted_streaming" | "aborted_tools") => return Kind::Cancelled,
        Some("max_turns") => return Kind::Failed,
        _ => {}
    }
    if value["is_error"] == true {
        return Kind::Failed;
    }
    match value["subtype"].as_str() {
        Some("success")
            if (value["is_error"].is_null() || value["is_error"] == false)
                && (value["terminal_reason"].is_null()
                    || value["terminal_reason"] == "completed") =>
        {
            Kind::Completed
        }
        Some("error_during_execution" | "error_max_turns") => Kind::Failed,
        _ => Kind::Opaque,
    }
}

fn classify_claude_stream(event: &Value) -> Kind {
    match event["type"].as_str() {
        Some("content_block_delta") => match event["delta"]["type"].as_str() {
            Some("text_delta") => Kind::Text,
            Some("input_json_delta") => Kind::Tool,
            _ => Kind::Opaque,
        },
        Some("content_block_start") if event["content_block"]["type"] == "tool_use" => Kind::Tool,
        Some("message_delta") if event["usage"].is_object() => Kind::Usage,
        // message_stop and content_block_stop do not end the CLI request.
        _ => Kind::Opaque,
    }
}

// Official producer: anomalyco/opencode packages/opencode/src/cli/cmd/run.ts.
fn classify_opencode(value: &Value) -> Kind {
    match value["type"].as_str() {
        Some("step_start") => Kind::Started,
        Some("text" | "reasoning") => Kind::Text,
        Some("tool_use") => Kind::Tool,
        Some("error") => Kind::Failed,
        Some("step_finish")
            if value["part"]["tokens"].is_object() || value["part"]["cost"].is_number() =>
        {
            Kind::Usage
        }
        // A step_finish with reason=stop still ends only one step.
        _ => Kind::Opaque,
    }
}

// Official contract: agentclientprotocol/agent-client-protocol schema/v1/schema.json.
fn classify_acp(value: &Value) -> Kind {
    match value["method"].as_str() {
        Some("session/request_permission") => Kind::Approval,
        Some("session/update") => {
            let update = &value["params"]["update"];
            match update["sessionUpdate"].as_str() {
                Some("agent_message_chunk" | "agent_thought_chunk")
                    if update["content"]["type"] == "text" =>
                {
                    Kind::Text
                }
                Some("tool_call" | "tool_call_update") => Kind::Tool,
                Some("usage_update") => Kind::Usage,
                _ => Kind::Opaque,
            }
        }
        None if value.get("method").is_none() => match value["result"]["stopReason"].as_str() {
            Some("end_turn") => Kind::Completed,
            Some("cancelled") => Kind::Cancelled,
            Some("max_tokens" | "max_turn_requests" | "refusal") => Kind::Failed,
            _ => Kind::Opaque,
        },
        // session/cancel is a request to cancel, not cancellation confirmation.
        _ => Kind::Opaque,
    }
}
