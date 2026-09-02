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
    if request.message.parts.len() == 1 {
        if let PartContent::Data(data) = &request.message.parts[0].content {
            if data.get("contract").is_some() {
                let envelope: RequestEnvelope = serde_json::from_value(data.clone())
                    .map_err(|_| TaskGatewayError::InvalidRequest)?;
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
        }
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
    if let Some(previous) = previous {
        if previous.task_id != next.task_id || previous.context_id != next.context_id {
            return Err(TaskGatewayError::Protocol);
        }
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
