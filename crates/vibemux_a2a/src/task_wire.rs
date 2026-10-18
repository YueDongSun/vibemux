//! Official SDK conversions stay inside this module.
use crate::task_contract::*;
use a2a::{
    A2AError, Artifact, Message, Part, PartContent, Role, SendMessageRequest, SendMessageResponse,
    Task, TaskStatus,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestEnvelope {
    contract: String,
    request: TaskRequest,
}

pub(crate) fn request_to_wire(
    request: &TaskRequest,
) -> Result<SendMessageRequest, TaskGatewayError> {
    request.validate()?;
    let data = serde_json::to_value(RequestEnvelope {
        contract: TASK_MESSAGE_CONTRACT.to_string(),
        request: request.clone(),
    })
    .map_err(|_| TaskGatewayError::InvalidRequest)?;
    let mut message = Message::new(Role::User, vec![Part::data(data)]);
    message.message_id = request.request_id.clone();
    message.context_id = Some(request.context_id.clone());
    message.task_id = request.task_id.clone();
    Ok(SendMessageRequest {
        message,
        configuration: Some(a2a::SendMessageConfiguration {
            accepted_output_modes: None,
            task_push_notification_config: None,
            history_length: Some(0),
            return_immediately: Some(true),
        }),
        metadata: None,
        tenant: None,
    })
}
pub(crate) fn request_from_wire(
    request: SendMessageRequest,
) -> Result<TaskRequest, TaskGatewayError> {
    if request.message.role != Role::User
        || request.message.parts.is_empty()
        || request.message.parts.len() > MAX_TASK_PARTS
        || request.tenant.is_some()
    {
        return Err(TaskGatewayError::InvalidRequest);
    }
    if request
        .configuration
        .as_ref()
        .is_some_and(|c| c.task_push_notification_config.is_some())
    {
        return Err(TaskGatewayError::Unsupported);
    }
    if request.message.parts.len() == 1
        && let PartContent::Data(data) = &request.message.parts[0].content
        && data.get("contract").is_some()
    {
        let envelope: RequestEnvelope =
            serde_json::from_value(data.clone()).map_err(|_| TaskGatewayError::InvalidRequest)?;
        if envelope.contract != TASK_MESSAGE_CONTRACT
            || envelope.request.request_id != request.message.message_id
            || Some(&envelope.request.context_id) != request.message.context_id.as_ref()
            || envelope.request.task_id != request.message.task_id
        {
            return Err(TaskGatewayError::InvalidRequest);
        }
        envelope.request.validate()?;
        return Ok(envelope.request);
    }
    // Native A2A messages are mapped as bounded data; no fixture-specific rules.
    let parts = request
        .message
        .parts
        .iter()
        .map(part_from_wire)
        .collect::<Result<Vec<_>, _>>()?;
    let result = TaskRequest {
        request_id: request.message.message_id.clone(),
        context_id: request
            .message
            .context_id
            .unwrap_or_else(|| request.message.message_id.clone()),
        task_id: request.message.task_id,
        idempotency_key: request.message.message_id,
        payload: json!({"parts":parts,"metadata":request.metadata}),
    };
    result.validate()?;
    Ok(result)
}
pub(crate) fn part_to_wire(part: &TaskPart) -> Part {
    match part {
        TaskPart::Text(value) => Part::text(value),
        TaskPart::Data(value) => Part::data(value.clone()),
        TaskPart::Raw {
            bytes,
            media_type,
            filename,
        } => {
            let mut part = Part::raw(bytes.clone());
            part.media_type = media_type.clone();
            part.filename = filename.clone();
            part
        }
        TaskPart::Url {
            url,
            media_type,
            filename,
        } => {
            let mut part = Part::url(url);
            part.media_type = media_type.clone();
            part.filename = filename.clone();
            part
        }
    }
}
pub(crate) fn part_from_wire(part: &Part) -> Result<TaskPart, TaskGatewayError> {
    let result = match &part.content {
        PartContent::Text(v) => TaskPart::Text(v.clone()),
        PartContent::Data(v) => TaskPart::Data(v.clone()),
        PartContent::Raw(v) => TaskPart::Raw {
            bytes: v.clone(),
            media_type: part.media_type.clone(),
            filename: part.filename.clone(),
        },
        PartContent::Url(v) => TaskPart::Url {
            url: v.clone(),
            media_type: part.media_type.clone(),
            filename: part.filename.clone(),
        },
    };
    result.validate()?;
    Ok(result)
}
pub(crate) fn snapshot_to_wire(snapshot: TaskSnapshot) -> Result<Task, TaskGatewayError> {
    snapshot.validate()?;
    let metadata = snapshot
        .error_code
        .map(|code| HashMap::from([("error_code".to_string(), Value::String(code))]));
    Ok(Task {
        id: snapshot.task_id,
        context_id: snapshot.context_id,
        status: TaskStatus {
            state: state_to_wire(snapshot.state),
            message: None,
            timestamp: None,
        },
        artifacts: (!snapshot.artifacts.is_empty()).then(|| {
            snapshot
                .artifacts
                .into_iter()
                .map(|artifact| Artifact {
                    artifact_id: artifact.artifact_id,
                    name: artifact.name,
                    description: None,
                    parts: artifact.parts.iter().map(part_to_wire).collect(),
                    metadata: None,
                    extensions: None,
                })
                .collect()
        }),
        history: None,
        metadata,
    })
}
pub(crate) fn snapshot_from_wire(task: Task) -> Result<TaskSnapshot, TaskGatewayError> {
    let artifacts = task
        .artifacts
        .unwrap_or_default()
        .into_iter()
        .map(artifact_from_wire)
        .collect::<Result<Vec<_>, _>>()?;
    let error_code = task
        .metadata
        .as_ref()
        .and_then(|m| m.get("error_code"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let snapshot = TaskSnapshot {
        task_id: task.id,
        context_id: task.context_id,
        state: state_from_wire(task.status.state)?,
        artifacts,
        error_code,
    };
    snapshot.validate()?;
    Ok(snapshot)
}
pub(crate) fn artifact_from_wire(artifact: Artifact) -> Result<TaskArtifact, TaskGatewayError> {
    let artifact = TaskArtifact {
        artifact_id: artifact.artifact_id,
        name: artifact.name,
        parts: artifact
            .parts
            .iter()
            .map(part_from_wire)
            .collect::<Result<Vec<_>, _>>()?,
    };
    artifact.validate()?;
    Ok(artifact)
}
pub(crate) fn reply_to_wire(reply: TaskReply) -> Result<SendMessageResponse, TaskGatewayError> {
    match reply {
        TaskReply::Task(snapshot) => snapshot_to_wire(snapshot).map(SendMessageResponse::Task),
        TaskReply::Message(message) => {
            message.validate()?;
            let mut wire = Message::new(
                Role::Agent,
                message.parts.iter().map(part_to_wire).collect(),
            );
            wire.message_id = message.message_id;
            wire.context_id = Some(message.context_id);
            Ok(SendMessageResponse::Message(wire))
        }
    }
}
pub(crate) fn reply_from_wire(reply: SendMessageResponse) -> Result<TaskReply, TaskGatewayError> {
    match reply {
        SendMessageResponse::Task(snapshot) => snapshot_from_wire(snapshot).map(TaskReply::Task),
        SendMessageResponse::Message(message) => {
            if message.role != Role::Agent {
                return Err(TaskGatewayError::Protocol);
            }
            let message = TaskMessage {
                message_id: message.message_id,
                context_id: message.context_id.ok_or(TaskGatewayError::Protocol)?,
                parts: message
                    .parts
                    .iter()
                    .map(part_from_wire)
                    .collect::<Result<Vec<_>, _>>()?,
            };
            message.validate()?;
            Ok(TaskReply::Message(message))
        }
    }
}
pub(crate) fn state_to_wire(state: TaskState) -> a2a::TaskState {
    match state {
        TaskState::Submitted => a2a::TaskState::Submitted,
        TaskState::Working => a2a::TaskState::Working,
        TaskState::Completed => a2a::TaskState::Completed,
        TaskState::Failed => a2a::TaskState::Failed,
        TaskState::Canceled => a2a::TaskState::Canceled,
        TaskState::InputRequired => a2a::TaskState::InputRequired,
        TaskState::Rejected => a2a::TaskState::Rejected,
        TaskState::AuthRequired => a2a::TaskState::AuthRequired,
    }
}
pub(crate) fn state_from_wire(state: a2a::TaskState) -> Result<TaskState, TaskGatewayError> {
    Ok(match state {
        a2a::TaskState::Submitted => TaskState::Submitted,
        a2a::TaskState::Working => TaskState::Working,
        a2a::TaskState::Completed => TaskState::Completed,
        a2a::TaskState::Failed => TaskState::Failed,
        a2a::TaskState::Canceled => TaskState::Canceled,
        a2a::TaskState::InputRequired => TaskState::InputRequired,
        a2a::TaskState::Rejected => TaskState::Rejected,
        a2a::TaskState::AuthRequired => TaskState::AuthRequired,
        a2a::TaskState::Unspecified => return Err(TaskGatewayError::Protocol),
    })
}
pub(crate) fn sdk_error(error: TaskGatewayError) -> A2AError {
    let code = match error {
        TaskGatewayError::InvalidRequest => a2a::error_code::INVALID_PARAMS,
        TaskGatewayError::NotFound => a2a::error_code::TASK_NOT_FOUND,
        TaskGatewayError::Unsupported => a2a::error_code::UNSUPPORTED_OPERATION,
        TaskGatewayError::Unauthorized => -32040,
        TaskGatewayError::Forbidden => -32041,
        _ => a2a::error_code::INTERNAL_ERROR,
    };
    A2AError::new(code, error.code())
}
pub(crate) fn local_error(error: &A2AError) -> TaskGatewayError {
    match error.message.as_str() {
        "a2a_task_invalid_request" => TaskGatewayError::InvalidRequest,
        "a2a_task_unauthorized" => TaskGatewayError::Unauthorized,
        "a2a_task_forbidden" => TaskGatewayError::Forbidden,
        "a2a_task_not_found" => TaskGatewayError::NotFound,
        "a2a_task_unsupported" => TaskGatewayError::Unsupported,
        "a2a_task_busy" => TaskGatewayError::Busy,
        "a2a_task_deadline" => TaskGatewayError::Deadline,
        _ => match error.code {
            a2a::error_code::TASK_NOT_FOUND => TaskGatewayError::NotFound,
            a2a::error_code::UNSUPPORTED_OPERATION | a2a::error_code::TASK_NOT_CANCELABLE => {
                TaskGatewayError::Unsupported
            }
            _ => TaskGatewayError::Protocol,
        },
    }
}
/// Apply only validated, identity-consistent SDK stream updates. The first event
/// must supply a Task snapshot; unsupported or truncated streams fail explicitly.
pub(crate) fn apply_stream_response(
    previous: &mut Option<TaskSnapshot>,
    event: a2a::StreamResponse,
) -> Result<TaskSnapshot, TaskGatewayError> {
    let next = match event {
        a2a::StreamResponse::Task(task) => snapshot_from_wire(task)?,
        a2a::StreamResponse::StatusUpdate(update) => {
            let mut snapshot = previous.clone().ok_or(TaskGatewayError::Protocol)?;
            if snapshot.task_id != update.task_id || snapshot.context_id != update.context_id {
                return Err(TaskGatewayError::Protocol);
            }
            snapshot.state = state_from_wire(update.status.state)?;
            snapshot.error_code = update
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("error_code"))
                .and_then(Value::as_str)
                .map(str::to_string);
            snapshot
        }
        a2a::StreamResponse::ArtifactUpdate(update) => {
            let mut snapshot = previous.clone().ok_or(TaskGatewayError::Protocol)?;
            if snapshot.task_id != update.task_id || snapshot.context_id != update.context_id {
                return Err(TaskGatewayError::Protocol);
            }
            let artifact = artifact_from_wire(update.artifact)?;
            if let Some(existing) = snapshot
                .artifacts
                .iter_mut()
                .find(|a| a.artifact_id == artifact.artifact_id)
            {
                if update.append == Some(true) {
                    existing.parts.extend(artifact.parts);
                } else {
                    *existing = artifact;
                }
            } else {
                snapshot.artifacts.push(artifact);
            }
            snapshot
        }
        a2a::StreamResponse::Message(_) => return Err(TaskGatewayError::Protocol),
    };
    next.validate()?;
    if let Some(previous) = previous
        && (previous.task_id != next.task_id || previous.context_id != next.context_id)
    {
        return Err(TaskGatewayError::Protocol);
    }
    *previous = Some(next.clone());
    Ok(next)
}

pub(crate) fn artifact_to_wire(artifact: &TaskArtifact) -> Artifact {
    Artifact {
        artifact_id: artifact.artifact_id.clone(),
        name: artifact.name.clone(),
        description: None,
        parts: artifact.parts.iter().map(part_to_wire).collect(),
        metadata: None,
        extensions: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> TaskRequest {
        TaskRequest {
            request_id: "request_1".to_string(),
            context_id: "context_1".to_string(),
            task_id: Some("task_1".to_string()),
            idempotency_key: "key_1".to_string(),
            payload: json!({"goal": "review"}),
        }
    }

    fn snapshot() -> TaskSnapshot {
        TaskSnapshot {
            task_id: "task_1".to_string(),
            context_id: "context_1".to_string(),
            state: TaskState::Failed,
            artifacts: vec![TaskArtifact::data("artifact_1", json!({"n": 1}))],
            error_code: Some("worker_failed".to_string()),
        }
    }

    fn native(message_id: &str, parts: Vec<Part>) -> SendMessageRequest {
        let mut message = Message::new(Role::User, parts);
        message.message_id = message_id.to_string();
        SendMessageRequest {
            message,
            configuration: None,
            metadata: None,
            tenant: None,
        }
    }

    fn status(
        task_id: &str,
        state: a2a::TaskState,
        error_code: Option<&str>,
    ) -> a2a::StreamResponse {
        a2a::StreamResponse::StatusUpdate(a2a::TaskStatusUpdateEvent {
            task_id: task_id.to_string(),
            context_id: "context_1".to_string(),
            status: TaskStatus {
                state,
                message: None,
                timestamp: None,
            },
            metadata: error_code
                .map(|code| HashMap::from([("error_code".to_string(), json!(code))])),
        })
    }

    fn artifact_update(
        task_id: &str,
        artifact: &TaskArtifact,
        append: Option<bool>,
    ) -> a2a::StreamResponse {
        a2a::StreamResponse::ArtifactUpdate(a2a::TaskArtifactUpdateEvent {
            task_id: task_id.to_string(),
            context_id: "context_1".to_string(),
            artifact: artifact_to_wire(artifact),
            append,
            last_chunk: None,
            metadata: None,
        })
    }

    #[test]
    fn every_local_state_maps_to_its_sdk_state_and_unspecified_is_refused() {
        for (local, wire) in [
            (TaskState::Submitted, a2a::TaskState::Submitted),
            (TaskState::Working, a2a::TaskState::Working),
            (TaskState::Completed, a2a::TaskState::Completed),
            (TaskState::Failed, a2a::TaskState::Failed),
            (TaskState::Canceled, a2a::TaskState::Canceled),
            (TaskState::InputRequired, a2a::TaskState::InputRequired),
            (TaskState::Rejected, a2a::TaskState::Rejected),
            (TaskState::AuthRequired, a2a::TaskState::AuthRequired),
        ] {
            assert_eq!(state_to_wire(local), wire);
            assert_eq!(state_from_wire(wire), Ok(local));
        }
        assert_eq!(
            state_from_wire(a2a::TaskState::Unspecified),
            Err(TaskGatewayError::Protocol)
        );
    }

    #[test]
    fn errors_keep_their_code_across_the_sdk_boundary() {
        use a2a::error_code::{
            INTERNAL_ERROR, INVALID_PARAMS, TASK_NOT_CANCELABLE, TASK_NOT_FOUND,
            UNSUPPORTED_OPERATION,
        };
        for (error, code) in [
            (TaskGatewayError::InvalidRequest, INVALID_PARAMS),
            (TaskGatewayError::Unauthorized, -32040),
            (TaskGatewayError::Forbidden, -32041),
            (TaskGatewayError::NotFound, TASK_NOT_FOUND),
            (TaskGatewayError::Unsupported, UNSUPPORTED_OPERATION),
            (TaskGatewayError::Busy, INTERNAL_ERROR),
            (TaskGatewayError::Deadline, INTERNAL_ERROR),
        ] {
            let wire = sdk_error(error.clone());
            assert_eq!((wire.code, wire.message.as_str()), (code, error.code()));
            assert_eq!(local_error(&wire), error);
        }
        // Failures only the local side can name come back as a protocol failure.
        for error in [
            TaskGatewayError::Protocol,
            TaskGatewayError::Transport,
            TaskGatewayError::Server,
            TaskGatewayError::Internal,
        ] {
            let wire = sdk_error(error.clone());
            assert_eq!(wire.code, INTERNAL_ERROR, "{error:?}");
            assert_eq!(local_error(&wire), TaskGatewayError::Protocol);
        }
        // An error from another A2A implementation maps by its code alone.
        for (code, error) in [
            (TASK_NOT_FOUND, TaskGatewayError::NotFound),
            (UNSUPPORTED_OPERATION, TaskGatewayError::Unsupported),
            (TASK_NOT_CANCELABLE, TaskGatewayError::Unsupported),
            (INVALID_PARAMS, TaskGatewayError::Protocol),
            (INTERNAL_ERROR, TaskGatewayError::Protocol),
        ] {
            assert_eq!(
                local_error(&A2AError::new(code, "foreign failure")),
                error,
                "{code}"
            );
        }
    }

    #[test]
    fn parts_round_trip_and_are_validated_when_read() {
        for part in [
            TaskPart::Text("é text".to_string()),
            TaskPart::Data(json!({"k": [1, 2]})),
            TaskPart::Raw {
                bytes: vec![0, 255],
                media_type: Some("application/octet-stream".to_string()),
                filename: Some("a.bin".to_string()),
            },
            TaskPart::Url {
                url: "https://example.test/a".to_string(),
                media_type: Some("text/html".to_string()),
                filename: Some("a.html".to_string()),
            },
        ] {
            assert_eq!(part_from_wire(&part_to_wire(&part)), Ok(part.clone()));
        }
        let mut named = Part::raw(vec![1]);
        named.filename = Some("a\nb".to_string());
        let mut typed = Part::url("https://example.test/a");
        typed.media_type = Some("t".repeat(129));
        for refused in [
            Part::text("x".repeat(MAX_TASK_PAYLOAD_BYTES + 1)),
            Part::url("ftp://example.test/a"),
            Part::url("https://user@example.test/a"),
            named,
            typed,
        ] {
            assert_eq!(
                part_from_wire(&refused),
                Err(TaskGatewayError::InvalidRequest)
            );
        }
    }

    #[test]
    fn a_request_round_trips_in_its_envelope_and_mismatched_ids_are_refused() {
        let wire = request_to_wire(&request()).expect("valid request");
        let configuration = wire.configuration.clone().expect("configuration");
        assert_eq!(
            (
                configuration.history_length,
                configuration.return_immediately
            ),
            (Some(0), Some(true))
        );
        assert_eq!(
            (&wire.message.role, wire.message.parts.len()),
            (&Role::User, 1)
        );
        assert_eq!(request_from_wire(wire.clone()), Ok(request()));

        let envelope = |contract: &str, request: TaskRequest| {
            Part::data(json!({"contract": contract, "request": request}))
        };
        let changes: [fn(&mut SendMessageRequest); 10] = [
            |wire| wire.message.message_id = "request_2".to_string(),
            |wire| wire.message.context_id = Some("context_2".to_string()),
            |wire| wire.message.context_id = None,
            |wire| wire.message.task_id = None,
            |wire| wire.tenant = Some("tenant_1".to_string()),
            |wire| wire.message.role = Role::Agent,
            |wire| wire.message.parts.clear(),
            |wire| {
                wire.message.parts[0] = Part::data(
                    json!({"contract": "vibemux.task.request.v2", "request": request()}),
                );
            },
            |wire| {
                wire.message.parts[0] = Part::data(
                    json!({"contract": TASK_MESSAGE_CONTRACT, "request": request(), "extra": 1}),
                );
            },
            |wire| {
                let mut invalid = request();
                invalid.idempotency_key = "key 1".to_string();
                wire.message.parts[0] =
                    Part::data(json!({"contract": TASK_MESSAGE_CONTRACT, "request": invalid}));
            },
        ];
        for (index, change) in changes.into_iter().enumerate() {
            let mut changed = wire.clone();
            change(&mut changed);
            assert_eq!(
                request_from_wire(changed),
                Err(TaskGatewayError::InvalidRequest),
                "change {index}"
            );
        }
        let mut rebuilt = wire.clone();
        rebuilt.message.parts[0] = envelope(TASK_MESSAGE_CONTRACT, request());
        assert_eq!(request_from_wire(rebuilt), Ok(request()));

        let mut pushed = wire.clone();
        if let Some(configuration) = pushed.configuration.as_mut() {
            configuration.task_push_notification_config = Some(a2a::TaskPushNotificationConfig {
                url: "https://example.test/hook".to_string(),
                id: None,
                task_id: String::new(),
                token: None,
                authentication: None,
                tenant: None,
            });
        }
        assert_eq!(
            request_from_wire(pushed),
            Err(TaskGatewayError::Unsupported)
        );

        let mut invalid = request();
        invalid.context_id = "context 1".to_string();
        assert_eq!(
            request_to_wire(&invalid).err(),
            Some(TaskGatewayError::InvalidRequest)
        );
    }

    #[test]
    fn a_native_message_becomes_bounded_part_data() {
        let parts = vec![Part::text("review this"), Part::data(json!({"n": 1}))];
        let mut wire = native("message_1", parts);
        assert_eq!(
            request_from_wire(wire.clone()),
            Ok(TaskRequest {
                request_id: "message_1".to_string(),
                context_id: "message_1".to_string(),
                task_id: None,
                idempotency_key: "message_1".to_string(),
                payload: json!({
                    "parts": [
                        {"kind": "text", "value": "review this"},
                        {"kind": "data", "value": {"n": 1}},
                    ],
                    "metadata": null,
                }),
            })
        );
        wire.message.context_id = Some("context_1".to_string());
        wire.message.task_id = Some("task_1".to_string());
        wire.metadata = Some(HashMap::from([("origin".to_string(), json!("peer"))]));
        let request = request_from_wire(wire).expect("native message with context");
        assert_eq!(
            (request.context_id.as_str(), request.task_id.as_deref()),
            ("context_1", Some("task_1"))
        );
        assert_eq!(request.payload["metadata"], json!({"origin": "peer"}));

        for refused in [
            native("message_1", vec![Part::text("x"); MAX_TASK_PARTS + 1]),
            native("message_1", vec![Part::url("ftp://example.test/a")]),
            native("message 1", vec![Part::text("x")]),
        ] {
            assert_eq!(
                request_from_wire(refused),
                Err(TaskGatewayError::InvalidRequest)
            );
        }
    }

    #[test]
    fn snapshots_and_replies_keep_their_error_code_and_artifacts() {
        let wire = snapshot_to_wire(snapshot()).expect("valid snapshot");
        assert_eq!(
            wire.metadata,
            Some(HashMap::from([(
                "error_code".to_string(),
                json!("worker_failed")
            )]))
        );
        assert_eq!(snapshot_from_wire(wire.clone()), Ok(snapshot()));
        let bare = TaskSnapshot {
            state: TaskState::Working,
            artifacts: Vec::new(),
            error_code: None,
            ..snapshot()
        };
        let bare_wire = snapshot_to_wire(bare.clone()).expect("bare snapshot");
        assert_eq!((&bare_wire.artifacts, &bare_wire.metadata), (&None, &None));
        assert_eq!(snapshot_from_wire(bare_wire), Ok(bare));
        let mut unspecified = wire;
        unspecified.status.state = a2a::TaskState::Unspecified;
        assert_eq!(
            snapshot_from_wire(unspecified),
            Err(TaskGatewayError::Protocol)
        );
        let invalid = TaskSnapshot {
            error_code: Some("worker failed".to_string()),
            ..snapshot()
        };
        assert_eq!(
            snapshot_to_wire(invalid).err(),
            Some(TaskGatewayError::InvalidRequest)
        );

        let task_reply = reply_to_wire(TaskReply::Task(snapshot())).expect("task reply");
        assert_eq!(reply_from_wire(task_reply), Ok(TaskReply::Task(snapshot())));
        let message = TaskMessage {
            message_id: "message_1".to_string(),
            context_id: "context_1".to_string(),
            parts: vec![TaskPart::Text("done".to_string())],
        };
        let reply = reply_to_wire(TaskReply::Message(message.clone())).expect("message reply");
        let SendMessageResponse::Message(agent) = reply else {
            panic!("a message reply stays a message");
        };
        assert_eq!(agent.role, Role::Agent);
        assert_eq!(
            reply_from_wire(SendMessageResponse::Message(agent.clone())),
            Ok(TaskReply::Message(message))
        );
        let changes: [fn(&mut Message); 2] = [
            |message| message.role = Role::User,
            |message| message.context_id = None,
        ];
        for change in changes {
            let mut changed = agent.clone();
            change(&mut changed);
            assert_eq!(
                reply_from_wire(SendMessageResponse::Message(changed)),
                Err(TaskGatewayError::Protocol)
            );
        }
    }

    #[test]
    fn stream_updates_apply_only_to_the_task_they_continue() {
        let mut previous = None;
        let second = TaskArtifact::data("artifact_2", json!(2));
        // A stream opens with a task snapshot.
        for event in [
            status("task_1", a2a::TaskState::Working, None),
            artifact_update("task_1", &second, None),
        ] {
            assert_eq!(
                apply_stream_response(&mut previous, event),
                Err(TaskGatewayError::Protocol)
            );
        }
        assert_eq!(previous, None);
        let opening = TaskSnapshot {
            state: TaskState::Working,
            error_code: None,
            ..snapshot()
        };
        let opened = snapshot_to_wire(opening.clone()).expect("opening snapshot");
        assert_eq!(
            apply_stream_response(&mut previous, a2a::StreamResponse::Task(opened)),
            Ok(opening.clone())
        );

        // A status update sets the state and the error code it carries, and
        // clears a code it does not carry.
        let failed = apply_stream_response(
            &mut previous,
            status("task_1", a2a::TaskState::Failed, Some("worker_failed")),
        )
        .expect("failed status");
        assert_eq!(
            (failed.state, failed.error_code.as_deref()),
            (TaskState::Failed, Some("worker_failed"))
        );
        let working = apply_stream_response(
            &mut previous,
            status("task_1", a2a::TaskState::Working, None),
        )
        .expect("working status");
        assert_eq!(
            (working.state, working.error_code),
            (TaskState::Working, None)
        );

        // A chunk extends its artifact only when it says so, and otherwise
        // replaces it; a chunk of a new artifact adds it.
        let chunk = TaskArtifact::data("artifact_1", json!({"n": 2}));
        let appended =
            apply_stream_response(&mut previous, artifact_update("task_1", &chunk, Some(true)))
                .expect("appended chunk");
        assert_eq!(
            appended.artifacts[0].parts,
            vec![
                TaskPart::Data(json!({"n": 1})),
                TaskPart::Data(json!({"n": 2}))
            ]
        );
        let replaced = apply_stream_response(
            &mut previous,
            artifact_update("task_1", &chunk, Some(false)),
        )
        .expect("replacing chunk");
        assert_eq!(replaced.artifacts, vec![chunk.clone()]);
        let added = apply_stream_response(&mut previous, artifact_update("task_1", &second, None))
            .expect("new artifact");
        assert_eq!(added.artifacts, vec![chunk.clone(), second]);

        // Nothing that belongs to another task or context, or that is not a
        // task update, changes the stream.
        let mut other_context = status("task_1", a2a::TaskState::Completed, None);
        if let a2a::StreamResponse::StatusUpdate(update) = &mut other_context {
            update.context_id = "context_2".to_string();
        }
        let other_task = TaskSnapshot {
            task_id: "task_2".to_string(),
            ..opening
        };
        for event in [
            status("task_2", a2a::TaskState::Completed, None),
            other_context,
            artifact_update("task_2", &chunk, None),
            status("task_1", a2a::TaskState::Unspecified, None),
            a2a::StreamResponse::Message(Message::new(Role::Agent, vec![Part::text("hi")])),
            a2a::StreamResponse::Task(snapshot_to_wire(other_task).expect("other task")),
        ] {
            assert_eq!(
                apply_stream_response(&mut previous, event),
                Err(TaskGatewayError::Protocol)
            );
        }
        assert_eq!(
            apply_stream_response(
                &mut previous,
                status("task_1", a2a::TaskState::Completed, Some("worker failed")),
            ),
            Err(TaskGatewayError::InvalidRequest)
        );
        assert_eq!(previous, Some(added));
    }
}
