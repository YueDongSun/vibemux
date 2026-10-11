//! Fixed metadata capabilities and validation before any reader or descriptor I/O.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;
use vibemux_types::{TaskId, frontend::MAX_FRONTEND_TASKS};

pub const READ_TOOLS: [&str; 4] = [
    "health",
    "frontend_tasks",
    "frontend_task",
    "harness_dispatch_status",
];

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReadOperation {
    Health,
    FrontendTasks { after: Option<TaskId>, limit: u32 },
    FrontendTask { task_id: TaskId },
    HarnessDispatchStatus { request_id: Uuid },
}

pub fn parse_operation(name: &str, arguments: &Value) -> Result<ReadOperation, &'static str> {
    let object = arguments.as_object().ok_or("invalid_arguments")?;
    let allowed: &[&str] = match name {
        "health" => &[],
        "frontend_tasks" => &["after", "limit"],
        "frontend_task" => &["task_id"],
        "harness_dispatch_status" => &["request_id"],
        _ => return Err("unknown_tool"),
    };
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err("invalid_arguments");
    }
    match name {
        "health" => Ok(ReadOperation::Health),
        "frontend_tasks" => {
            let limit = match object.get("limit") {
                None => MAX_FRONTEND_TASKS,
                Some(limit) => limit
                    .as_u64()
                    .and_then(|value| u32::try_from(value).ok())
                    .filter(|value| (1..=MAX_FRONTEND_TASKS).contains(value))
                    .ok_or("invalid_arguments")?,
            };
            let after = object
                .get("after")
                .map(canonical_uuid)
                .transpose()?
                .map(TaskId::from_uuid)
                .transpose()
                .map_err(|_| "invalid_arguments")?;
            Ok(ReadOperation::FrontendTasks { after, limit })
        }
        "frontend_task" => Ok(ReadOperation::FrontendTask {
            task_id: TaskId::from_uuid(canonical_uuid(
                object.get("task_id").ok_or("invalid_arguments")?,
            )?)
            .map_err(|_| "invalid_arguments")?,
        }),
        "harness_dispatch_status" => Ok(ReadOperation::HarnessDispatchStatus {
            request_id: canonical_uuid(object.get("request_id").ok_or("invalid_arguments")?)?,
        }),
        _ => Err("unknown_tool"),
    }
}

fn canonical_uuid(value: &Value) -> Result<Uuid, &'static str> {
    let text = value
        .as_str()
        .filter(|value| value.len() == 36)
        .ok_or("invalid_arguments")?;
    let id = Uuid::parse_str(text).map_err(|_| "invalid_arguments")?;
    if id.is_nil() || id.to_string() != text {
        return Err("invalid_arguments");
    }
    Ok(id)
}

pub fn list_tools(metadata_available: bool) -> Value {
    let mut tools = vec![schema(
        "host_status",
        "Local host configuration and reader state; no producer contact.",
        json!({}),
        vec![],
    )];
    if metadata_available {
        let uuid_schema = json!({"type":"string","minLength":36,"maxLength":36,
            "pattern":"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$",
            "not":{"const":"00000000-0000-0000-0000-000000000000"}});
        tools.extend([
            schema("health", "Read daemon-declared health and queue metadata; no process identity.", json!({}), vec![]),
            schema("frontend_tasks", "Read a bounded task metadata page; titles are untrusted metadata.", json!({"limit":{"type":"integer","minimum":1,"maximum":MAX_FRONTEND_TASKS,"default":MAX_FRONTEND_TASKS},"after":uuid_schema}), vec![]),
            schema("frontend_task", "Read bounded task, run, event and artifact metadata; no source or worktree paths.", json!({"task_id":uuid_schema}), vec!["task_id"]),
            schema("harness_dispatch_status", "Read daemon-declared dispatch state; this is not proof of model completion.", json!({"request_id":uuid_schema}), vec!["request_id"]),
        ]);
    }
    json!({"tools":tools})
}

fn schema(name: &str, description: &str, properties: Value, required: Vec<&str>) -> Value {
    json!({"name":name,"description":description,
        "inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},
        "annotations":{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}})
}
