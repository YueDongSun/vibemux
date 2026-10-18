//! MCP lifecycle contracts, independent of process or daemon I/O.

use crate::{
    config::{MAX_RESTARTS, PROTOCOL_VERSION},
    tools::{READ_TOOLS, ReadOperation, list_tools, parse_operation},
};
use serde_json::{Value, json};

pub fn error(id: Value, number: i32, code: &'static str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":number,"message":code,"data":{"code":code}}})
}

pub fn result(id: Value, value: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":value})
}

pub fn tool_result(id: Value, value: Value) -> Value {
    result(
        id,
        json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":false}),
    )
}

pub fn tool_error(id: Value, code: &'static str) -> Value {
    result(
        id,
        json!({"content":[{"type":"text","text":json!({"error":{"code":code}}).to_string()}],"isError":true}),
    )
}

pub enum Action {
    None,
    Reply(Value),
    Read { id: Value, operation: ReadOperation },
}

pub enum InFlightAction {
    Cancel,
    Reply(Value),
    Ignore,
}

fn valid_id(id: &Value) -> bool {
    id.as_str().is_some_and(|value| value.len() <= 128)
        || id.as_i64().is_some()
        || id.as_u64().is_some()
}

fn valid_envelope(object: &serde_json::Map<String, Value>) -> bool {
    object.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
        && object.get("method").and_then(Value::as_str).is_some()
        && object.get("id").is_none_or(valid_id)
        && object
            .keys()
            .all(|key| ["jsonrpc", "id", "method", "params"].contains(&key.as_str()))
}

pub fn while_reading(message: Value, active_id: &Value) -> InFlightAction {
    let Some(object) = message.as_object().filter(|object| valid_envelope(object)) else {
        return InFlightAction::Reply(error(Value::Null, -32600, "invalid_request"));
    };
    let params = object.get("params").cloned().unwrap_or_else(|| json!({}));
    let Some(id) = object.get("id") else {
        let cancel = object["method"] == "notifications/cancelled"
            && params.as_object().is_some_and(|params| {
                params
                    .keys()
                    .all(|key| ["requestId", "reason", "_meta"].contains(&key.as_str()))
            })
            && valid_metadata(&params)
            && params.get("requestId") == Some(active_id)
            && params.get("reason").is_none_or(Value::is_string);
        return if cancel {
            InFlightAction::Cancel
        } else {
            InFlightAction::Ignore
        };
    };
    if id == active_id {
        return InFlightAction::Reply(error(Value::Null, -32600, "duplicate_request_id"));
    }
    if object["method"] == "ping" && metadata_only(&params) {
        InFlightAction::Reply(result(id.clone(), json!({})))
    } else {
        InFlightAction::Reply(error(id.clone(), -32000, "request_busy"))
    }
}

pub struct BackendView {
    pub available: bool,
    pub state: &'static str,
    pub launches: u32,
    pub restarts: u32,
    pub last_error: Option<&'static str>,
}

impl Default for BackendView {
    fn default() -> Self {
        Self {
            available: true,
            state: "not_started",
            launches: 0,
            restarts: 0,
            last_error: None,
        }
    }
}

#[derive(Default)]
pub struct Protocol {
    initialization_sent: bool,
    pub initialized: bool,
    pub metadata_read: bool,
}

impl Protocol {
    pub fn handle(&mut self, message: Value, host_grant: bool, backend: &BackendView) -> Action {
        let object = match message.as_object() {
            Some(object) => object,
            None => return Action::Reply(error(Value::Null, -32600, "invalid_request")),
        };
        let id = object.get("id").cloned();
        if !valid_envelope(object) {
            return Action::Reply(error(Value::Null, -32600, "invalid_request"));
        }
        let method = object["method"].as_str().unwrap_or_default();
        let params = object.get("params").cloned().unwrap_or_else(|| json!({}));
        let Some(id) = id else {
            if method == "notifications/initialized"
                && self.initialization_sent
                && metadata_only(&params)
            {
                self.initialized = true;
            }
            return Action::None;
        };
        if method == "initialize" {
            return Action::Reply(self.initialize(id, &params, host_grant));
        }
        if method == "ping" {
            return Action::Reply(if metadata_only(&params) {
                result(id, json!({}))
            } else {
                error(id, -32602, "invalid_arguments")
            });
        }
        if !self.initialized {
            return Action::Reply(error(id, -32002, "not_initialized"));
        }
        match method {
            "tools/list" if metadata_only(&params) => Action::Reply(result(
                id,
                list_tools(self.metadata_read && backend.available),
            )),
            "tools/list" => Action::Reply(error(id, -32602, "invalid_arguments")),
            "tools/call" => self.call(id, &params, backend),
            _ => Action::Reply(error(id, -32601, "method_not_found")),
        }
    }

    fn initialize(&mut self, id: Value, params: &Value, host_grant: bool) -> Value {
        if self.initialization_sent {
            return error(id, -32600, "already_initialized");
        }
        let valid = params.as_object().is_some_and(|object| {
            object.keys().all(|key| {
                ["protocolVersion", "capabilities", "clientInfo", "_meta"].contains(&key.as_str())
            }) && params["protocolVersion"]
                .as_str()
                .is_some_and(|value| !value.is_empty() && value.len() <= 32)
                && params["capabilities"].is_object()
                && params["capabilities"]
                    .get("experimental")
                    .is_none_or(Value::is_object)
                && valid_metadata(params)
                && params["clientInfo"]["name"]
                    .as_str()
                    .is_some_and(|value| !value.is_empty() && value.len() <= 128)
                && params["clientInfo"]["version"]
                    .as_str()
                    .is_some_and(|value| !value.is_empty() && value.len() <= 64)
        });
        let requested = params.pointer("/capabilities/experimental/vibemux_metadata_read");
        if !valid || requested.is_some_and(|value| !value.is_boolean()) {
            return error(id, -32602, "invalid_arguments");
        }
        self.metadata_read = host_grant && requested.and_then(Value::as_bool).unwrap_or(true);
        self.initialization_sent = true;
        result(
            id,
            json!({"protocolVersion":PROTOCOL_VERSION,
            "capabilities":{"tools":{"listChanged":true}},
            "serverInfo":{"name":"vibemux_mcp","version":env!("CARGO_PKG_VERSION")}}),
        )
    }

    fn call(&self, id: Value, params: &Value, backend: &BackendView) -> Action {
        if !params.as_object().is_some_and(|object| {
            object
                .keys()
                .all(|key| ["name", "arguments", "_meta"].contains(&key.as_str()))
        }) || !valid_metadata(params)
        {
            return Action::Reply(error(id, -32602, "invalid_arguments"));
        }
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let Some(name) = params["name"].as_str() else {
            return Action::Reply(error(id, -32602, "invalid_arguments"));
        };
        if name == "host_status" {
            return Action::Reply(if empty_object(&arguments) {
                tool_result(
                    id,
                    json!({"metadata_read":self.metadata_read, "backend_state":backend.state,
                    "worker_launches":backend.launches,"launch_count_kind":"attempts","restart_limit":MAX_RESTARTS,"restarts_used":backend.restarts,
                    "last_error":backend.last_error,"evidence":"host_configuration_only"}),
                )
            } else {
                error(id, -32602, "invalid_arguments")
            });
        }
        if READ_TOOLS.contains(&name) && !self.metadata_read {
            return Action::Reply(error(id, -32001, "metadata_read_denied"));
        }
        let operation = match parse_operation(name, &arguments) {
            Ok(operation) => operation,
            Err(code) => return Action::Reply(error(id, -32602, code)),
        };
        if !backend.available {
            return Action::Reply(tool_error(id, "reader_unavailable"));
        }
        Action::Read { id, operation }
    }
}

pub fn empty_object(value: &Value) -> bool {
    value.as_object().is_some_and(|value| value.is_empty())
}

fn valid_metadata(value: &Value) -> bool {
    value.get("_meta").is_none_or(Value::is_object)
}

fn metadata_only(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|object| object.keys().all(|key| key == "_meta"))
        && valid_metadata(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(id: Value, method: &str, params: Value) -> Value {
        json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
    }

    fn notification(method: &str, params: Value) -> Value {
        json!({"jsonrpc":"2.0","method":method,"params":params})
    }

    fn initialize_params() -> Value {
        json!({"protocolVersion":PROTOCOL_VERSION,"capabilities":{},
        "clientInfo":{"name":"fixture","version":"1"}})
    }

    fn reply(action: Action) -> Value {
        match action {
            Action::Reply(value) => value,
            Action::None => panic!("expected a reply, got no action"),
            Action::Read { .. } => panic!("expected a reply, got a read"),
        }
    }

    fn in_flight_reply(action: InFlightAction) -> Value {
        match action {
            InFlightAction::Reply(value) => value,
            InFlightAction::Cancel => panic!("expected a reply, got a cancel"),
            InFlightAction::Ignore => panic!("expected a reply, got an ignore"),
        }
    }

    /// The JSON-RPC number and the stable code of an error reply.
    fn failure(reply: &Value) -> (i64, &str) {
        (
            reply["error"]["code"].as_i64().unwrap_or_default(),
            reply["error"]["data"]["code"].as_str().unwrap_or_default(),
        )
    }

    fn ready(host_grant: bool) -> Protocol {
        let backend = BackendView::default();
        let mut protocol = Protocol::default();
        reply(protocol.handle(
            request(json!(0), "initialize", initialize_params()),
            host_grant,
            &backend,
        ));
        protocol.handle(
            notification("notifications/initialized", json!({})),
            host_grant,
            &backend,
        );
        assert!(protocol.initialized);
        protocol
    }

    fn tool_names(listed: &Value) -> Vec<&str> {
        listed["result"]["tools"]
            .as_array()
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(|tool| tool["name"].as_str())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn the_lifecycle_answers_each_request_in_order() {
        let backend = BackendView::default();
        let mut protocol = Protocol::default();
        let mut send = |message: Value| reply(protocol.handle(message, true, &backend));
        assert_eq!(
            failure(&send(request(json!(1), "tools/list", json!({})))),
            (-32002, "not_initialized")
        );
        assert_eq!(
            send(request(json!(2), "ping", json!({}))),
            json!({"jsonrpc":"2.0","id":2,"result":{}})
        );
        let answer = send(request(json!(3), "initialize", initialize_params()));
        assert_eq!(answer["result"]["protocolVersion"], PROTOCOL_VERSION);
        let again = send(request(json!(4), "initialize", initialize_params()));
        assert_eq!(again["id"], 4);
        assert_eq!(failure(&again), (-32600, "already_initialized"));
        // The initialize answer alone does not open the tools.
        assert_eq!(
            failure(&send(request(json!(5), "tools/list", json!({})))),
            (-32002, "not_initialized")
        );
        let unknown = reply(protocol.handle(
            request(json!("a"), "resources/list", json!({})),
            true,
            &backend,
        ));
        assert_eq!(failure(&unknown), (-32002, "not_initialized"));

        let mut protocol = ready(true);
        let unknown = reply(protocol.handle(
            request(json!("a"), "resources/list", json!({})),
            true,
            &backend,
        ));
        assert_eq!(unknown["id"], "a");
        assert_eq!(failure(&unknown), (-32601, "method_not_found"));
        let listed =
            reply(protocol.handle(request(json!(6), "tools/list", json!({})), true, &backend));
        assert!(tool_names(&listed).contains(&"health"));
    }

    #[test]
    fn an_early_initialized_notification_is_ignored() {
        let backend = BackendView::default();
        let mut protocol = Protocol::default();
        assert!(matches!(
            protocol.handle(
                notification("notifications/initialized", json!({})),
                true,
                &backend
            ),
            Action::None
        ));
        reply(protocol.handle(
            request(json!(1), "initialize", initialize_params()),
            true,
            &backend,
        ));
        assert!(!protocol.initialized);
    }

    #[test]
    fn a_read_needs_the_host_grant_and_an_available_backend() {
        let call =
            |name: &str| request(json!(1), "tools/call", json!({"name":name,"arguments":{}}));
        let available = BackendView::default();
        let mut granted = ready(true);
        assert!(matches!(
            granted.handle(call("health"), true, &available),
            Action::Read {
                operation: ReadOperation::Health,
                ..
            }
        ));
        let unavailable = BackendView {
            available: false,
            ..BackendView::default()
        };
        let refused = reply(granted.handle(call("health"), true, &unavailable));
        assert_eq!(refused["result"]["isError"], true);
        assert!(
            refused["result"]["content"][0]["text"]
                .as_str()
                .is_some_and(|text| text.contains("reader_unavailable"))
        );
        assert_eq!(
            failure(&reply(granted.handle(call("shell"), true, &available))),
            (-32602, "unknown_tool")
        );

        let mut denied = ready(false);
        assert_eq!(
            failure(&reply(denied.handle(call("health"), false, &available))),
            (-32001, "metadata_read_denied")
        );
        let listed = reply(denied.handle(
            request(json!(2), "tools/list", json!({})),
            false,
            &available,
        ));
        assert_eq!(tool_names(&listed), ["host_status"]);
    }

    #[test]
    fn an_in_flight_read_admits_only_ping_and_its_own_cancellation() {
        let active = json!(7);
        let duplicate =
            in_flight_reply(while_reading(request(json!(7), "ping", json!({})), &active));
        assert_eq!(duplicate["id"], Value::Null);
        assert_eq!(failure(&duplicate), (-32600, "duplicate_request_id"));
        assert_eq!(
            in_flight_reply(while_reading(request(json!(8), "ping", json!({})), &active)),
            json!({"jsonrpc":"2.0","id":8,"result":{}})
        );
        let busy = in_flight_reply(while_reading(
            request(json!(9), "tools/list", json!({})),
            &active,
        ));
        assert_eq!(busy["id"], 9);
        assert_eq!(failure(&busy), (-32000, "request_busy"));

        let cancel =
            |params: Value| while_reading(notification("notifications/cancelled", params), &active);
        assert!(matches!(
            cancel(json!({"requestId":7,"reason":"user"})),
            InFlightAction::Cancel
        ));
        for ignored in [
            json!({"requestId":8}),
            json!({"requestId":7,"reason":1}),
            json!({"requestId":7,"extra":true}),
        ] {
            assert!(
                matches!(cancel(ignored.clone()), InFlightAction::Ignore),
                "{ignored}"
            );
        }
    }

    #[test]
    fn envelopes_outside_json_rpc_2_are_invalid_requests() {
        let backend = BackendView::default();
        for message in [
            json!([]),
            json!({"jsonrpc":"1.0","id":1,"method":"ping"}),
            json!({"jsonrpc":"2.0","id":1}),
            json!({"jsonrpc":"2.0","id":true,"method":"ping"}),
            json!({"jsonrpc":"2.0","id":1.5,"method":"ping"}),
            json!({"jsonrpc":"2.0","id":"x".repeat(129),"method":"ping"}),
            json!({"jsonrpc":"2.0","id":1,"method":"ping","extra":0}),
        ] {
            let answer = reply(Protocol::default().handle(message.clone(), true, &backend));
            assert_eq!(answer["id"], Value::Null, "{message}");
            assert_eq!(failure(&answer), (-32600, "invalid_request"), "{message}");
        }
        let longest = json!("x".repeat(128));
        let answer = reply(Protocol::default().handle(
            request(longest.clone(), "ping", json!({})),
            true,
            &backend,
        ));
        assert_eq!(answer["id"], longest);
        assert_eq!(answer["result"], json!({}));
    }
}
