//! The final assistant text of a completed structured turn (ADR 031 §5).
//!
//! Workflow turns end with a fenced report block in the final assistant
//! message. This module finds that message in the captured vendor records
//! for each executable protocol; it never interprets terminal text or
//! partial deltas as the final message.

use serde_json::Value;

use super::{NativeProtocol, ObservedRecord};

/// The final assistant message, if the protocol reports one.
#[must_use]
pub fn final_assistant_text(
    protocol: NativeProtocol,
    records: &[ObservedRecord],
) -> Option<String> {
    let values = records
        .iter()
        .rev()
        .filter_map(|record| serde_json::from_str::<Value>(record.raw_json()).ok());
    match protocol {
        // The terminal `result` record carries the final message text.
        NativeProtocol::ClaudeStreamJson => values
            .filter(|value| value["type"] == "result" && value["subtype"] == "success")
            .find_map(|value| value["result"].as_str().map(str::to_string)),
        // The last completed agent message item.
        NativeProtocol::CodexExec => values
            .filter(|value| {
                value["type"] == "item.completed" && value["item"]["type"] == "agent_message"
            })
            .find_map(|value| value["item"]["text"].as_str().map(str::to_string)),
        NativeProtocol::CodexAppServer | NativeProtocol::Acp => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn records(protocol: NativeProtocol, values: &[Value]) -> Vec<ObservedRecord> {
        values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                ObservedRecord::parse(protocol, index as u64 + 1, value.to_string())
                    .expect("record")
                    .0
            })
            .collect()
    }

    #[test]
    fn claude_final_text_comes_from_the_success_result_only() {
        let captured = records(
            NativeProtocol::ClaudeStreamJson,
            &[
                json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"partial"}}}),
                json!({"type":"result","subtype":"success","result":"final answer"}),
            ],
        );
        assert_eq!(
            final_assistant_text(NativeProtocol::ClaudeStreamJson, &captured).as_deref(),
            Some("final answer")
        );
        let failed = records(
            NativeProtocol::ClaudeStreamJson,
            &[json!({"type":"result","subtype":"error_during_execution","result":"x"})],
        );
        assert_eq!(
            final_assistant_text(NativeProtocol::ClaudeStreamJson, &failed),
            None
        );
    }

    #[test]
    fn codex_final_text_is_the_last_completed_agent_message() {
        let captured = records(
            NativeProtocol::CodexExec,
            &[
                json!({"type":"item.completed","item":{"type":"agent_message","text":"first"}}),
                json!({"type":"item.updated","item":{"type":"agent_message","text":"streaming"}}),
                json!({"type":"item.completed","item":{"type":"agent_message","text":"last"}}),
                json!({"type":"turn.completed"}),
            ],
        );
        assert_eq!(
            final_assistant_text(NativeProtocol::CodexExec, &captured).as_deref(),
            Some("last")
        );
        assert_eq!(final_assistant_text(NativeProtocol::Acp, &captured), None);
    }
}
