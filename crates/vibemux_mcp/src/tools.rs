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

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use super::*;

    const ID: &str = "5f0c6a52-8f3b-4c1e-9a7d-2b1e3c4d5e6f";
    /// The `pattern` every identifier schema declares.
    const ID_PATTERN: &str = "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$";

    fn input_schema(name: &str) -> Value {
        let listed = list_tools(true);
        let tool = listed["tools"]
            .as_array()
            .expect("tool list")
            .iter()
            .find(|tool| tool["name"] == name)
            .cloned()
            .expect("listed tool");
        tool["inputSchema"].clone()
    }

    /// One value each declared argument accepts.
    fn valid_argument(key: &str) -> Value {
        if key == "limit" { json!(1) } else { json!(ID) }
    }

    fn arguments<'a>(keys: impl IntoIterator<Item = &'a str>) -> Value {
        Value::Object(
            keys.into_iter()
                .map(|key| (key.to_string(), valid_argument(key)))
                .collect::<Map<_, _>>(),
        )
    }

    /// `ID_PATTERN`, the length bounds, and the `not` constant, read as the
    /// schema states them.
    fn schema_admits_id(text: &str) -> bool {
        let groups: Vec<&str> = text.split('-').collect();
        groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
            && groups.iter().all(|group| {
                group
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
            })
            && text != Uuid::nil().to_string()
    }

    #[test]
    fn listed_read_tools_are_the_tools_the_parser_knows() {
        let listed = list_tools(true);
        let names: Vec<&str> = listed["tools"]
            .as_array()
            .expect("tool list")
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .filter(|name| *name != "host_status")
            .collect();
        assert_eq!(names, READ_TOOLS);
        assert_eq!(list_tools(false)["tools"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            parse_operation("host_status", &json!({})).err(),
            Some("unknown_tool")
        );
    }

    #[test]
    fn each_schema_declares_exactly_the_arguments_the_parser_accepts() {
        for name in READ_TOOLS {
            let schema = input_schema(name);
            assert_eq!(schema["additionalProperties"], false, "{name}");
            let declared: Vec<&str> = schema["properties"]
                .as_object()
                .expect("properties")
                .keys()
                .map(String::as_str)
                .collect();
            let required: Vec<&str> = schema["required"]
                .as_array()
                .expect("required")
                .iter()
                .filter_map(Value::as_str)
                .collect();
            assert!(
                parse_operation(name, &arguments(declared.clone())).is_ok(),
                "{name}"
            );
            assert!(
                parse_operation(name, &arguments(required.clone())).is_ok(),
                "{name}"
            );
            for missing in &required {
                let rest = declared.iter().copied().filter(|key| key != missing);
                assert_eq!(
                    parse_operation(name, &arguments(rest)).err(),
                    Some("invalid_arguments"),
                    "{name} without {missing}"
                );
            }
            let mut extra = arguments(declared);
            extra["undeclared"] = json!(1);
            assert_eq!(
                parse_operation(name, &extra).err(),
                Some("invalid_arguments"),
                "{name}"
            );
            assert_eq!(
                parse_operation(name, &json!([])).err(),
                Some("invalid_arguments"),
                "{name}"
            );
        }
    }

    #[test]
    fn the_limit_schema_states_the_parser_bounds_and_default() {
        let limit = input_schema("frontend_tasks")["properties"]["limit"].clone();
        assert_eq!(limit["type"], "integer");
        let bound = |field: &str| limit[field].as_u64().expect("integer bound");
        let parse = |arguments: Value| match parse_operation("frontend_tasks", &arguments) {
            Ok(ReadOperation::FrontendTasks { limit, .. }) => Some(u64::from(limit)),
            _ => None,
        };
        let (minimum, maximum) = (bound("minimum"), bound("maximum"));
        assert_eq!((minimum, maximum), (1, u64::from(MAX_FRONTEND_TASKS)));
        assert_eq!(parse(json!({"limit": minimum})), Some(minimum));
        assert_eq!(parse(json!({"limit": maximum})), Some(maximum));
        assert_eq!(parse(json!({})), Some(bound("default")));
        for refused in [
            json!(minimum - 1),
            json!(maximum + 1),
            json!(-1),
            json!(1.5),
            json!("1"),
            Value::Null,
        ] {
            assert_eq!(parse(json!({ "limit": refused })), None, "{refused}");
        }
    }

    #[test]
    fn identifier_schemas_admit_what_the_parser_admits() {
        let id_schema = input_schema("frontend_task")["properties"]["task_id"].clone();
        assert_eq!(id_schema["type"], "string");
        assert_eq!(
            (
                id_schema["minLength"].as_u64(),
                id_schema["maxLength"].as_u64()
            ),
            (Some(36), Some(36))
        );
        assert_eq!(id_schema["pattern"], ID_PATTERN);
        assert_eq!(id_schema["not"]["const"], Uuid::nil().to_string());
        let candidates = [
            ID.to_string(),
            ID.to_uppercase(),
            Uuid::nil().to_string(),
            ID.replace('-', ""),
            format!("{{{ID}}}"),
            format!("urn:uuid:{ID}"),
            format!(" {}", &ID[1..]),
            format!("g{}", &ID[1..]),
        ];
        for (name, key) in [
            ("frontend_tasks", "after"),
            ("frontend_task", "task_id"),
            ("harness_dispatch_status", "request_id"),
        ] {
            assert_eq!(input_schema(name)["properties"][key], id_schema, "{name}");
            for candidate in &candidates {
                let parsed = parse_operation(name, &json!({ key: candidate })).is_ok();
                assert_eq!(parsed, schema_admits_id(candidate), "{name} {candidate}");
            }
            assert!(parse_operation(name, &json!({ key: ID })).is_ok(), "{name}");
            assert!(parse_operation(name, &json!({ key: 7 })).is_err(), "{name}");
        }
    }
}
