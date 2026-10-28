//! Closed MCP 2025-06-18 subset. Client roots cannot change owner grants.

use crate::{
    contract::{Operation, ToolError},
    test_session::TestCall,
    workspace::WorkerTools,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use vibemux_harness::dispatch::writable_profile::WritableGrant;

const PROTOCOL_VERSION: &str = "2025-06-18";
const MAX_REQUESTS: usize = 1024;
const MAX_MESSAGES: usize = 2048;

#[derive(Clone)]
pub struct ToolSurface {
    pub grants: WritableGrant,
    pub test_suites: Vec<String>,
}

pub enum ProtocolAction {
    None,
    Reply(Value),
    File { id: Value, operation: Operation },
    RunTest { id: Value, call: TestCall },
    Cancel { id: Value },
}

#[derive(Default)]
pub struct Protocol {
    initialized: bool,
    initialization_sent: bool,
    request_ids: BTreeSet<String>,
    messages: usize,
}

impl Protocol {
    pub fn handle(
        &mut self,
        message: Value,
        tools: &mut WorkerTools,
    ) -> Result<Option<Value>, &'static str> {
        let surface = ToolSurface {
            grants: tools.grants(),
            test_suites: vec![],
        };
        Ok(match self.accept(message, &surface, false)? {
            ProtocolAction::None | ProtocolAction::Cancel { .. } => None,
            ProtocolAction::Reply(reply) => Some(reply),
            ProtocolAction::File { id, operation } => {
                Some(execute_file(id, operation, tools, false))
            }
            ProtocolAction::RunTest { id, .. } => {
                Some(tool_error(id, ToolError::ExecutionUnsupported))
            }
        })
    }

    /// Pure admission/parsing. Effects are owned by the serving session.
    pub fn accept(
        &mut self,
        message: Value,
        surface: &ToolSurface,
        busy: bool,
    ) -> Result<ProtocolAction, &'static str> {
        if self.messages >= MAX_MESSAGES {
            return Err("message_limit");
        }
        self.messages += 1;
        let Some(object) = message.as_object() else {
            return Ok(ProtocolAction::Reply(error(
                Value::Null,
                -32600,
                "invalid_request",
            )));
        };
        let id = object.get("id").cloned();
        if object.get("jsonrpc") != Some(&json!("2.0"))
            || !object.get("method").is_some_and(Value::is_string)
            || !object
                .keys()
                .all(|key| ["jsonrpc", "id", "method", "params"].contains(&key.as_str()))
        {
            return Ok(ProtocolAction::Reply(error(
                Value::Null,
                -32600,
                "invalid_request",
            )));
        }
        let method = object["method"].as_str().unwrap_or_default();
        let params = object.get("params").cloned().unwrap_or_else(|| json!({}));
        let Some(id) = id else {
            if method == "notifications/cancelled"
                && self.initialized
                && params.as_object().is_some_and(|fields| {
                    fields
                        .keys()
                        .all(|key| ["requestId", "reason", "_meta"].contains(&key.as_str()))
                })
                && valid_meta(&params)
                && valid_id(&params["requestId"])
                && params
                    .get("reason")
                    .is_none_or(|reason| reason.as_str().is_some_and(|reason| reason.len() <= 256))
            {
                return Ok(ProtocolAction::Cancel {
                    id: params["requestId"].clone(),
                });
            }
            if method == "notifications/initialized"
                && self.initialization_sent
                && metadata_only(&params)
            {
                self.initialized = true;
            }
            return Ok(ProtocolAction::None);
        };
        if !valid_id(&id) {
            return Ok(ProtocolAction::Reply(error(
                Value::Null,
                -32600,
                "invalid_request_id",
            )));
        }
        if self.request_ids.len() >= MAX_REQUESTS {
            return Err("request_limit");
        }
        if !self.request_ids.insert(id.to_string()) {
            return Ok(ProtocolAction::Reply(error(
                Value::Null,
                -32600,
                "duplicate_request_id",
            )));
        }
        let reply = match method {
            "initialize" => {
                if self.initialization_sent {
                    error(id, -32600, "already_initialized")
                } else if !params.as_object().is_some_and(|fields| {
                    fields.keys().all(|key| {
                        ["protocolVersion", "capabilities", "clientInfo", "_meta"]
                            .contains(&key.as_str())
                    })
                }) || !params["protocolVersion"]
                    .as_str()
                    .is_some_and(|value| !value.is_empty() && value.len() <= 32)
                    || !params["capabilities"].is_object()
                    || !params["clientInfo"]["name"]
                        .as_str()
                        .is_some_and(|name| !name.is_empty() && name.len() <= 128)
                    || !params["clientInfo"]["version"]
                        .as_str()
                        .is_some_and(|version| !version.is_empty() && version.len() <= 64)
                    || !valid_meta(&params)
                {
                    error(id, -32602, "invalid_arguments")
                } else {
                    self.initialization_sent = true;
                    result(
                        id,
                        json!({"protocolVersion":PROTOCOL_VERSION,"capabilities":{"tools":{}},"serverInfo":{"name":"vibemux_worker_tools","version":env!("CARGO_PKG_VERSION")}}),
                    )
                }
            }
            "ping" if metadata_only(&params) => result(id, json!({})),
            _ if !self.initialized => error(id, -32002, "not_initialized"),
            "tools/list" if metadata_only(&params) => result(id, list_tools(surface)),
            "tools/call" if busy => error(id, -32000, "worker_tools_busy"),
            "tools/call" => return Ok(parse_call(id, &params, surface)),
            "tools/list" | "ping" => error(id, -32602, "invalid_arguments"),
            _ => error(id, -32601, "method_not_found"),
        };
        Ok(ProtocolAction::Reply(reply))
    }
}

fn valid_id(id: &Value) -> bool {
    id.as_str()
        .is_some_and(|text| !text.is_empty() && text.len() <= 128)
        || id.as_u64().is_some()
        || id.as_i64().is_some()
}

fn valid_meta(value: &Value) -> bool {
    value.get("_meta").is_none_or(Value::is_object)
}
fn metadata_only(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|object| object.keys().all(|key| key == "_meta"))
        && valid_meta(value)
}
fn result(id: Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}
fn error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}

fn list_tools(surface: &ToolSurface) -> Value {
    let grant = surface.grants;
    let path = json!({"type":"string","minLength":1,"maxLength":240});
    let content = json!({"type":"string","maxLength":65536});
    let mut entries = Vec::new();
    for (enabled, name, description, properties, required, read_only) in [
        (
            grant.read_files,
            "read_file",
            "Read a granted text file and its current revision.",
            json!({"path":path}),
            json!(["path"]),
            true,
        ),
        (
            grant.search_files,
            "search_paths",
            "List granted file paths beneath a relative prefix; empty prefix lists all.",
            json!({"prefix":{"type":"string","maxLength":240}}),
            json!(["prefix"]),
            true,
        ),
        (
            grant.write_files,
            "create_file",
            "Create a missing file. Existing files are always refused.",
            json!({"path":path,"content":content}),
            json!(["path", "content"]),
            false,
        ),
        (
            grant.edit_files,
            "edit_file",
            "Replace an existing text file at the exact expected revision. Missing files are refused.",
            json!({"path":path,"expected_revision":{"type":"integer","minimum":0},"content":content}),
            json!(["path", "expected_revision", "content"]),
            false,
        ),
    ] {
        if enabled {
            entries.push(json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},"annotations":{"readOnlyHint":read_only,"destructiveHint":!read_only,"idempotentHint":false,"openWorldHint":false}}));
        }
    }
    if grant.read_files && grant.run_dev_tests && !surface.test_suites.is_empty() {
        entries.push(json!({"name":"run_dev_tests","description":"Run an owner-selected suite once on this candidate snapshot. Execution output is untrusted; it does not verify assertions. Use workspace_revision from file-tool results (initially zero).",
            "inputSchema":{"type":"object","properties":{"suite_id":{"type":"string","enum":surface.test_suites},"expected_revision":{"type":"integer","minimum":0}},"required":["suite_id","expected_revision"],"additionalProperties":false},
            "annotations":{"readOnlyHint":false,"destructiveHint":false,"idempotentHint":false,"openWorldHint":false}}));
    }
    json!({"tools":entries})
}

fn parse_call(id: Value, params: &Value, surface: &ToolSurface) -> ProtocolAction {
    if !params.as_object().is_some_and(|object| {
        object
            .keys()
            .all(|key| ["name", "arguments", "_meta"].contains(&key.as_str()))
    }) || !valid_meta(params)
    {
        return ProtocolAction::Reply(error(id, -32602, "invalid_arguments"));
    }
    if params["name"] == "run_dev_tests" {
        if surface.test_suites.is_empty() {
            return ProtocolAction::Reply(error(id, -32602, "unknown_tool"));
        }
        if !surface.grants.read_files || !surface.grants.run_dev_tests {
            return ProtocolAction::Reply(tool_error(id, ToolError::Denied));
        }
        return match serde_json::from_value::<TestCall>(params["arguments"].clone()) {
            Ok(call) if surface.test_suites.contains(&call.suite_id) => {
                ProtocolAction::RunTest { id, call }
            }
            _ => ProtocolAction::Reply(error(id, -32602, "invalid_arguments")),
        };
    }
    let operation = match params["name"].as_str() {
        Some("read_file") => "read",
        Some("search_paths") => "search",
        Some("create_file") => "create",
        Some("edit_file") => "edit",
        _ => return ProtocolAction::Reply(error(id, -32602, "unknown_tool")),
    };
    let Some(mut arguments) = params["arguments"].as_object().cloned() else {
        return ProtocolAction::Reply(error(id, -32602, "invalid_arguments"));
    };
    if arguments.contains_key("operation") {
        return ProtocolAction::Reply(error(id, -32602, "invalid_arguments"));
    }
    arguments.insert("operation".into(), json!(operation));
    let Ok(operation) = serde_json::from_value::<Operation>(Value::Object(arguments)) else {
        return ProtocolAction::Reply(error(id, -32602, "invalid_arguments"));
    };
    ProtocolAction::File { id, operation }
}

pub(crate) fn execute_file(
    id: Value,
    operation: Operation,
    tools: &mut WorkerTools,
    include_revision: bool,
) -> Value {
    match tools.execute(operation) {
        Ok(value) => match serde_json::to_value(value) {
            Ok(mut value) => {
                if include_revision {
                    value["workspace_revision"] = json!(tools.revision());
                }
                tool_result(id, value)
            }
            Err(_) => tool_error(id, ToolError::Io),
        },
        Err(error) => tool_error(id, error),
    }
}

pub(crate) fn tool_result(id: Value, value: Value) -> Value {
    result(
        id,
        json!({"content":[{"type":"text","text":value.to_string()}],"isError":false}),
    )
}

pub(crate) fn tool_error(id: Value, error: ToolError) -> Value {
    result(
        id,
        json!({"content":[{"type":"text","text":error.code()}],"isError":true}),
    )
}
