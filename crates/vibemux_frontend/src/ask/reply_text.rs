#![forbid(unsafe_code)]
//! Rebuilds the reply a harness has written so far from its raw dispatch
//! records. Only the documented assistant-text records are read; tool,
//! approval, usage, and unknown records add nothing.
//!
//! - Claude stream-json: the final `result` record holds the whole answer;
//!   until it arrives, streamed `text_delta`s, or whole `assistant` messages
//!   when nothing streamed, stand in for it.
//! - Codex app-server: `item/agentMessage/delta` deltas per item, replaced
//!   by the item's text when `item/completed` reports it.
//! - Codex exec: `agent_message` items carry their full text.

use serde_json::Value;
use vibemux_harness::dispatch::NativeProtocol;

/// The reply never grows past this many bytes; later text is dropped.
pub const MAX_REPLY_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReplyText {
    /// Codex message items in arrival order: item id and its text.
    items: Vec<(String, String)>,
    /// Claude streamed text deltas.
    streamed: String,
    /// Claude whole assistant message text.
    assistant: String,
    /// Claude's final answer, which replaces everything else.
    final_text: Option<String>,
    truncated: bool,
}

impl ReplyText {
    /// Folds one record of `protocol` into the reply.
    pub fn absorb(&mut self, protocol: NativeProtocol, record: &Value) {
        match protocol {
            NativeProtocol::ClaudeStreamJson => self.absorb_claude(record),
            NativeProtocol::CodexAppServer => self.absorb_codex_app_server(record),
            NativeProtocol::CodexExec => self.absorb_codex_exec(record),
            NativeProtocol::Acp | NativeProtocol::DshSdkJsonRpc => {}
        }
    }

    /// The reply so far, trimmed.
    #[must_use]
    pub fn text(&self) -> String {
        let text = if let Some(final_text) = &self.final_text {
            final_text.clone()
        } else if !self.items.is_empty() {
            self.items
                .iter()
                .map(|(_, text)| text.trim())
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n")
        } else if !self.streamed.trim().is_empty() {
            self.streamed.clone()
        } else {
            self.assistant.clone()
        };
        text.trim().to_string()
    }

    /// Whether later text was dropped at [`MAX_REPLY_BYTES`].
    #[must_use]
    pub const fn truncated(&self) -> bool {
        self.truncated
    }

    fn absorb_claude(&mut self, record: &Value) {
        match record["type"].as_str() {
            Some("result") => {
                if let Some(result) = record["result"].as_str() {
                    self.final_text = Some(self.bounded(result));
                }
            }
            Some("stream_event") => {
                let event = &record["event"];
                if event["type"] == "content_block_delta"
                    && event["delta"]["type"] == "text_delta"
                    && let Some(text) = event["delta"]["text"].as_str()
                {
                    let used = self.streamed.len();
                    let text = self.clip(used, text);
                    self.streamed.push_str(&text);
                }
            }
            Some("assistant") => {
                let Some(content) = record["message"]["content"].as_array() else {
                    return;
                };
                for block in content {
                    if block["type"] == "text"
                        && let Some(text) = block["text"].as_str()
                    {
                        if !self.assistant.is_empty() {
                            self.assistant.push_str("\n\n");
                        }
                        let used = self.assistant.len();
                        let text = self.clip(used, text);
                        self.assistant.push_str(&text);
                    }
                }
            }
            _ => {}
        }
    }

    fn absorb_codex_app_server(&mut self, record: &Value) {
        let params = &record["params"];
        match record["method"].as_str() {
            Some("item/agentMessage/delta") => {
                if let (Some(item), Some(delta)) =
                    (params["itemId"].as_str(), params["delta"].as_str())
                {
                    self.append_item(item, delta);
                }
            }
            Some("item/completed") => {
                let item = &params["item"];
                if item["type"] == "agentMessage"
                    && let (Some(id), Some(text)) = (item["id"].as_str(), item["text"].as_str())
                {
                    self.replace_item(id, text);
                }
            }
            _ => {}
        }
    }

    fn absorb_codex_exec(&mut self, record: &Value) {
        if !matches!(
            record["type"].as_str(),
            Some("item.started" | "item.updated" | "item.completed")
        ) {
            return;
        }
        let item = &record["item"];
        if item["type"] == "agent_message"
            && let (Some(id), Some(text)) = (item["id"].as_str(), item["text"].as_str())
        {
            self.replace_item(id, text);
        }
    }

    fn append_item(&mut self, id: &str, delta: &str) {
        let used = self.used_bytes();
        let delta = self.clip(used, delta);
        match self.items.iter_mut().find(|(known, _)| known == id) {
            Some((_, text)) => text.push_str(&delta),
            None => self.items.push((id.to_string(), delta)),
        }
    }

    fn replace_item(&mut self, id: &str, text: &str) {
        let others: usize = self
            .items
            .iter()
            .filter(|(known, _)| known != id)
            .map(|(_, text)| text.len())
            .sum();
        let text = self.clip(others, text);
        match self.items.iter_mut().find(|(known, _)| known == id) {
            Some((_, existing)) => *existing = text,
            None => self.items.push((id.to_string(), text)),
        }
    }

    fn used_bytes(&self) -> usize {
        self.items.iter().map(|(_, text)| text.len()).sum()
    }

    fn bounded(&mut self, text: &str) -> String {
        self.clip(0, text)
    }

    /// `text` cut so that `used + text` stays within [`MAX_REPLY_BYTES`], at
    /// a character boundary.
    fn clip(&mut self, used: usize, text: &str) -> String {
        let room = MAX_REPLY_BYTES.saturating_sub(used);
        if text.len() <= room {
            return text.to_string();
        }
        self.truncated = true;
        let mut end = room;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        text[..end].to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reply(protocol: NativeProtocol, records: &[Value]) -> ReplyText {
        let mut reply = ReplyText::default();
        for record in records {
            reply.absorb(protocol, record);
        }
        reply
    }

    #[test]
    fn claude_streams_deltas_then_its_result_replaces_them() {
        let delta = |text: &str| {
            json!({"type":"stream_event","event":{"type":"content_block_delta",
                "index":0,"delta":{"type":"text_delta","text":text}}})
        };
        let streaming = reply(
            NativeProtocol::ClaudeStreamJson,
            &[
                json!({"type":"system","subtype":"init"}),
                delta("Hello, "),
                delta("world"),
            ],
        );
        assert_eq!(streaming.text(), "Hello, world");
        let finished = reply(
            NativeProtocol::ClaudeStreamJson,
            &[
                delta("draft"),
                json!({"type":"result","subtype":"success","is_error":false,"result":"Final answer"}),
            ],
        );
        assert_eq!(finished.text(), "Final answer");
    }

    #[test]
    fn claude_assistant_messages_stand_in_when_nothing_streamed() {
        let message = json!({"type":"assistant","message":{"content":[
            {"type":"text","text":"Part one"},
            {"type":"tool_use","name":"Read"},
            {"type":"text","text":"Part two"}]}});
        assert_eq!(
            reply(NativeProtocol::ClaudeStreamJson, &[message]).text(),
            "Part one\n\nPart two"
        );
    }

    #[test]
    fn codex_app_server_deltas_join_and_completed_items_replace_them() {
        let delta = |item: &str, text: &str| json!({"method":"item/agentMessage/delta","params":{"itemId":item,"delta":text}});
        let text = reply(
            NativeProtocol::CodexAppServer,
            &[
                delta("one", "First "),
                delta("one", "message"),
                delta("two", "Second"),
                json!({"method":"item/completed","params":{"item":{
                    "id":"two","type":"agentMessage","text":"Second, complete"}}}),
                json!({"method":"item/commandExecution/outputDelta","params":{"delta":"ignored"}}),
            ],
        )
        .text();
        assert_eq!(text, "First message\n\nSecond, complete");
    }

    #[test]
    fn codex_exec_items_carry_their_whole_text() {
        let item = |kind: &str, text: &str| json!({"type":"item.updated","item":{"id":"m1","type":kind,"text":text}});
        let text = reply(
            NativeProtocol::CodexExec,
            &[
                item("agent_message", "Partial"),
                item("agent_message", "Partial and complete"),
                item("reasoning", "not shown"),
            ],
        )
        .text();
        assert_eq!(text, "Partial and complete");
    }

    #[test]
    fn other_protocols_and_records_add_nothing() {
        let record = json!({"method":"session/update","params":{"update":{
            "sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"x"}}}});
        assert_eq!(reply(NativeProtocol::Acp, &[record]).text(), "");
        assert_eq!(
            reply(NativeProtocol::ClaudeStreamJson, &[json!({"type":"user"})]).text(),
            ""
        );
    }

    #[test]
    fn the_reply_is_bounded_at_a_character_boundary() {
        let long = "é".repeat(MAX_REPLY_BYTES);
        let result = json!({"type":"result","result":long});
        let reply = reply(NativeProtocol::ClaudeStreamJson, &[result]);
        assert!(reply.truncated());
        assert!(reply.text().len() <= MAX_REPLY_BYTES);
        assert!(reply.text().chars().all(|character| character == 'é'));
    }
}
