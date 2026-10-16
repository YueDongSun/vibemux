//! The observed DSH SDK wire vocabulary, decoded strictly.
//!
//! Source of truth: `@deepseek-ai/dsh-sdk-protocol` (0.1.2-rc.1) —
//! "the newline-delimited JSON-RPC stdio transport and the named request,
//! result, and notification types spoken between the runtime server and SDK
//! clients". Frame dispatch is that package's documented rule: a frame with
//! both `id` and `method` is a request, `id` alone is a response, and `method`
//! alone is a notification.
//!
//! Nothing here interprets a vendor payload beyond the discriminants this
//! adapter has verified. `session.event` bodies stay opaque values so a
//! merge-extensible event vocabulary cannot become accidental internal state
//! (AGENTS.md §2 invariant 16).

use serde_json::Value;

use super::DshError;

/// JSON-RPC version the reference client writes. Frames that omit it are
/// accepted, because the transport's dispatch rule keys on `id`/`method`
/// presence rather than on this field; frames that carry a different value are
/// refused.
pub const JSONRPC_VERSION: &str = "2.0";

/// Wire-stable server identity reported by `initialize`
/// (`serverInfo.name`). A different value means this is not the SDK runtime
/// this adapter binds to.
pub const SERVER_NAME: &str = "deepseek-harness-sdk-runtime";

/// Client-to-server request methods.
pub const METHOD_INITIALIZE: &str = "initialize";
pub const METHOD_SESSION_PROMPT: &str = "session/prompt";
pub const METHOD_SHUTDOWN: &str = "shutdown";

/// Server-to-client notification methods.
pub const NOTIFICATION_SESSION_EVENT: &str = "session.event";
pub const NOTIFICATION_SESSION_STATUS: &str = "session.status";
pub const NOTIFICATION_SUBAGENT_STARTED: &str = "subagent.started";
pub const NOTIFICATION_SUBAGENT_FINISHED: &str = "subagent.finished";

/// Session-event discriminants this adapter reads.
pub const EVENT_TURN_START: &str = "turn/start";
pub const EVENT_TURN_END: &str = "turn/end";

/// `session.status` payload states. The value is read for diagnostics only:
/// an idle whole-agent state is never completion authority.
pub const STATUS_IDLE: &str = "idle";
pub const STATUS_RUNNING: &str = "running";

/// Every method this adapter recognizes as a server notification. Unknown
/// methods stay tolerated, because the protocol permits extension.
pub const NOTIFICATION_METHODS: [&str; 4] = [
    NOTIFICATION_SESSION_EVENT,
    NOTIFICATION_SESSION_STATUS,
    NOTIFICATION_SUBAGENT_STARTED,
    NOTIFICATION_SUBAGENT_FINISHED,
];

/// Which JSON-RPC frame shape an object carries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DshFrameKind {
    /// `id` and `method`.
    Request,
    /// `id` alone.
    Response,
    /// `method` alone.
    Notification,
}

/// Server identity reported by a successful handshake.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DshServerInfo {
    pub name: String,
    pub version: String,
}

/// Classifies one frame. An object with neither `id` nor `method` is not a
/// frame the transport would dispatch, so it is refused rather than ignored.
pub fn frame_kind(value: &Value) -> Result<DshFrameKind, DshError> {
    let object = value.as_object().ok_or(DshError::InvalidJsonRpc)?;
    if let Some(version) = object.get("jsonrpc")
        && version.as_str() != Some(JSONRPC_VERSION)
    {
        return Err(DshError::InvalidJsonRpc);
    }
    let has_id = object.contains_key("id");
    let has_method = object.contains_key("method");
    match (has_id, has_method) {
        (true, true) => Ok(DshFrameKind::Request),
        (true, false) => Ok(DshFrameKind::Response),
        (false, true) => Ok(DshFrameKind::Notification),
        (false, false) => Err(DshError::InvalidJsonRpc),
    }
}

/// Decodes the `initialize` result. A result whose `serverInfo.name` is not
/// the wire-stable SDK identity is [`DshError::InvalidServerIdentity`], which
/// ends the session instead of guessing at a foreign protocol.
pub fn decode_server_info(result: &Value) -> Result<DshServerInfo, DshError> {
    let Some(server_info) = result.get("serverInfo").and_then(Value::as_object) else {
        return Err(DshError::InvalidServerInfo);
    };
    let Some(name) = server_info.get("name").and_then(Value::as_str) else {
        return Err(DshError::InvalidServerInfo);
    };
    if name != SERVER_NAME {
        return Err(DshError::InvalidServerIdentity);
    }
    let Some(version) = server_info.get("version").and_then(Value::as_str) else {
        return Err(DshError::InvalidServerInfo);
    };
    if version.is_empty() || version.len() > super::wire::MAX_IDENTITY_BYTES {
        return Err(DshError::InvalidServerInfo);
    }
    Ok(DshServerInfo {
        name: name.to_string(),
        version: version.to_string(),
    })
}

/// Decodes the `session/prompt` receipt. `messageId` identifies the queued
/// user message; it is not a turn result, so it never ends a turn.
pub fn decode_prompt_receipt(result: &Value) -> Result<String, DshError> {
    identity(result.get("messageId"))
}

/// The `sessionId` a notification carries, when it carries one. Only
/// `session.event` and `session.status` carry this field; `subagent.*` payloads
/// use `parentSessionId` and `childSessionId` instead.
#[must_use]
pub fn notification_session_id(value: &Value) -> Option<&str> {
    value.pointer("/params/sessionId").and_then(Value::as_str)
}

/// The `parentSessionId` a `subagent.*` notification carries.
#[must_use]
pub fn parent_session_id(value: &Value) -> Option<&str> {
    value
        .pointer("/params/parentSessionId")
        .and_then(Value::as_str)
}

/// The `session.event` discriminant, when the frame is that notification.
#[must_use]
pub fn session_event_type(value: &Value) -> Option<&str> {
    value.pointer("/params/event/type").and_then(Value::as_str)
}

/// The `session.status` state, when the frame is that notification.
#[must_use]
pub fn session_status(value: &Value) -> Option<&str> {
    value.pointer("/params/status").and_then(Value::as_str)
}

/// The `turn` number of a `turn/start` or `turn/end` event. Both variants carry
/// the same `{ turn }` payload shape.
pub fn event_turn_number(value: &Value) -> Result<u64, DshError> {
    value
        .pointer("/params/event/data/turn")
        .and_then(Value::as_u64)
        .ok_or(DshError::InvalidTurnEnd)
}

/// The `reason.kind` of a `turn/end` event, or `None` when the vocabulary
/// extension this runtime speaks is newer than this adapter knows.
#[must_use]
pub fn turn_end_reason_kind(value: &Value) -> Option<&str> {
    value
        .pointer("/params/event/data/reason/kind")
        .and_then(Value::as_str)
}

/// Bound on an identity the runtime echoes back (server version, message id).
pub const MAX_IDENTITY_BYTES: usize = 512;

fn identity(value: Option<&Value>) -> Result<String, DshError> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty() && text.len() <= MAX_IDENTITY_BYTES)
        .map(str::to_string)
        .ok_or(DshError::InvalidPromptReceipt)
}

/// Whether `label` is usable as a provider or model route name. The value
/// reaches protocol params, so the alphabet is deliberately narrow; this
/// mirrors the route-config rule so a configured route and a live session can
/// never disagree about what is acceptable.
#[must_use]
pub fn route_label_is_valid(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 256
        && !label.starts_with('-')
        && label.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':' | b'/')
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn frame_kind_follows_the_documented_dispatch_rule() {
        assert_eq!(
            frame_kind(&json!({"jsonrpc": "2.0", "id": "a", "method": "initialize"})),
            Ok(DshFrameKind::Request)
        );
        assert_eq!(
            frame_kind(&json!({"jsonrpc": "2.0", "id": "a", "result": {}})),
            Ok(DshFrameKind::Response)
        );
        assert_eq!(
            frame_kind(&json!({"jsonrpc": "2.0", "method": "session.event"})),
            Ok(DshFrameKind::Notification)
        );
        // A missing version is tolerated; a wrong one is not.
        assert_eq!(
            frame_kind(&json!({"id": "a", "result": {}})),
            Ok(DshFrameKind::Response)
        );
        assert_eq!(
            frame_kind(&json!({"jsonrpc": "1.0", "id": "a", "result": {}})),
            Err(DshError::InvalidJsonRpc)
        );
        for value in [
            json!({}),
            json!({"jsonrpc": "2.0"}),
            json!([1, 2]),
            json!("text"),
        ] {
            assert_eq!(frame_kind(&value), Err(DshError::InvalidJsonRpc), "{value}");
        }
    }

    #[test]
    fn server_info_pins_the_wire_stable_identity() {
        let good = json!({"serverInfo": {"name": SERVER_NAME, "version": "0.0.1"}});
        assert_eq!(
            decode_server_info(&good),
            Ok(DshServerInfo {
                name: SERVER_NAME.to_string(),
                version: "0.0.1".to_string(),
            })
        );
        let foreign = json!({"serverInfo": {"name": "other-runtime", "version": "1"}});
        assert_eq!(
            decode_server_info(&foreign),
            Err(DshError::InvalidServerIdentity)
        );
        for missing in [
            json!({}),
            json!({"serverInfo": {}}),
            json!({"serverInfo": {"name": SERVER_NAME}}),
            json!({"serverInfo": {"name": SERVER_NAME, "version": ""}}),
            json!({"serverInfo": "text"}),
        ] {
            let decoded = decode_server_info(&missing);
            assert!(
                matches!(
                    decoded,
                    Err(DshError::InvalidServerInfo | DshError::InvalidServerIdentity)
                ),
                "{missing} -> {decoded:?}"
            );
        }
    }

    #[test]
    fn prompt_receipt_requires_a_bounded_message_identity() {
        assert_eq!(
            decode_prompt_receipt(&json!({"messageId": "msg_1"})),
            Ok("msg_1".to_string())
        );
        for invalid in [
            json!({}),
            json!({"messageId": ""}),
            json!({"messageId": 7}),
            json!({"messageId": "x".repeat(MAX_IDENTITY_BYTES + 1)}),
        ] {
            assert_eq!(
                decode_prompt_receipt(&invalid),
                Err(DshError::InvalidPromptReceipt)
            );
        }
    }

    #[test]
    fn discriminants_are_read_from_their_documented_paths() {
        let end = json!({"params": {"sessionId": "s1",
            "event": {"type": "turn/end", "seq": 4, "time": 1, "data": {"turn": 3,
                "reason": {"kind": "completed"}}}}});
        assert_eq!(notification_session_id(&end), Some("s1"));
        assert_eq!(session_event_type(&end), Some(EVENT_TURN_END));
        assert_eq!(event_turn_number(&end), Ok(3));
        assert_eq!(turn_end_reason_kind(&end), Some("completed"));
        let status = json!({"params": {"sessionId": "s1", "status": "idle"}});
        assert_eq!(session_status(&status), Some(STATUS_IDLE));

        let truncated = json!({"params": {"sessionId": "s1",
            "event": {"type": "turn/end", "data": {"reason": {"kind": "completed"}}}}});
        assert_eq!(event_turn_number(&truncated), Err(DshError::InvalidTurnEnd));
        // An absent reason is a newer vocabulary, not a malformed frame.
        let unknown = json!({"params": {"event": {"type": "turn/end", "data": {"turn": 1}}}});
        assert_eq!(turn_end_reason_kind(&unknown), None);
    }

    #[test]
    fn subagent_notifications_are_keyed_by_parent_not_session() {
        // The runtime keys subagent payloads by parent/child session id and
        // sends no `sessionId`, so a client that correlates them through the
        // session-id path would fail on the first subagent event.
        let started = json!({"jsonrpc": "2.0", "method": "subagent.started",
            "params": {"parentSessionId": "parent", "childSessionId": "child"}});
        assert_eq!(notification_session_id(&started), None);
        assert_eq!(parent_session_id(&started), Some("parent"));
        let finished = json!({"jsonrpc": "2.0", "method": "subagent.finished",
            "params": {"provider": "p", "agentId": "a", "parentSessionId": "parent",
                "childSessionId": "child", "status": "ok", "stopReason": {}}});
        assert_eq!(notification_session_id(&finished), None);
        assert_eq!(parent_session_id(&finished), Some("parent"));
    }

    #[test]
    fn route_labels_match_the_config_rule() {
        for valid in ["deepseek-official", "deepseek-v4-flash", "a", "a/b:c.d_e"] {
            assert!(route_label_is_valid(valid), "{valid}");
        }
        for invalid in ["", "-x", "with space", "emoji😀", "\u{1}"] {
            assert!(!route_label_is_valid(invalid), "{invalid}");
        }
        assert!(!route_label_is_valid(&"x".repeat(257)));
    }
}
