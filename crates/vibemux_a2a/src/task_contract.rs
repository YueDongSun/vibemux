//! Bounded SDK-independent contracts. Backends own authorization and state.
use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fmt, time::Duration};
use thiserror::Error;

pub const MAX_TASK_WIRE_BYTES: usize = 64 * 1024;
pub const MAX_TASK_PAYLOAD_BYTES: usize = 16 * 1024;
pub const MAX_TASK_PARTS: usize = 16;
pub const MAX_TASK_ARTIFACTS: usize = 16;
pub const MAX_TASK_IDENTITIES: usize = 32;
pub const TASK_CALL_DEADLINE: Duration = Duration::from_secs(30);
pub const TASK_SHUTDOWN_DEADLINE: Duration = Duration::from_secs(5);
pub const TASK_MESSAGE_CONTRACT: &str = "vibemux.task.request.v1";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Submitted,
    Working,
    Completed,
    Failed,
    Canceled,
    InputRequired,
    Rejected,
    AuthRequired,
}
impl TaskState {
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Canceled | Self::Rejected
        )
    }
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRequest {
    pub request_id: String,
    pub context_id: String,
    pub task_id: Option<String>,
    pub idempotency_key: String,
    pub payload: Value,
}
impl TaskRequest {
    pub fn validate(&self) -> Result<(), TaskGatewayError> {
        for id in [&self.request_id, &self.context_id, &self.idempotency_key] {
            validate_task_identifier(id)?;
        }
        if let Some(id) = &self.task_id {
            validate_task_identifier(id)?;
        }
        bounded_json(&self.payload, MAX_TASK_PAYLOAD_BYTES)
    }
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum TaskPart {
    Text(String),
    Data(Value),
    Raw {
        bytes: Vec<u8>,
        media_type: Option<String>,
        filename: Option<String>,
    },
    Url {
        url: String,
        media_type: Option<String>,
        filename: Option<String>,
    },
}
impl TaskPart {
    pub fn validate(&self) -> Result<(), TaskGatewayError> {
        match self {
            Self::Text(text) if text.len() > MAX_TASK_PAYLOAD_BYTES => {
                return Err(TaskGatewayError::InvalidRequest);
            }
            Self::Data(data) => bounded_json(data, MAX_TASK_PAYLOAD_BYTES)?,
            Self::Raw {
                bytes,
                media_type,
                filename,
            } => {
                if bytes.len() > MAX_TASK_PAYLOAD_BYTES {
                    return Err(TaskGatewayError::InvalidRequest);
                }
                validate_media_type(media_type)?;
                if filename
                    .as_ref()
                    .is_some_and(|name| name.len() > 256 || name.contains(['\0', '\r', '\n']))
                {
                    return Err(TaskGatewayError::InvalidRequest);
                }
            }
            Self::Url {
                url,
                media_type,
                filename,
            } => {
                if url.len() > 2048 {
                    return Err(TaskGatewayError::InvalidRequest);
                }
                let parsed = url::Url::parse(url).map_err(|_| TaskGatewayError::InvalidRequest)?;
                if !matches!(parsed.scheme(), "http" | "https")
                    || !parsed.username().is_empty()
                    || parsed.password().is_some()
                {
                    return Err(TaskGatewayError::InvalidRequest);
                }
                validate_media_type(media_type)?;
                if filename
                    .as_ref()
                    .is_some_and(|name| name.len() > 256 || name.contains(['\0', '\r', '\n']))
                {
                    return Err(TaskGatewayError::InvalidRequest);
                }
            }
            _ => {}
        }
        Ok(())
    }
}
fn validate_media_type(media_type: &Option<String>) -> Result<(), TaskGatewayError> {
    if media_type
        .as_ref()
        .is_some_and(|value| value.len() > 128 || value.contains(['\r', '\n', '\0']))
    {
        return Err(TaskGatewayError::InvalidRequest);
    }
    Ok(())
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskArtifact {
    pub artifact_id: String,
    pub name: Option<String>,
    pub parts: Vec<TaskPart>,
}
impl TaskArtifact {
    pub fn data(artifact_id: impl Into<String>, data: Value) -> Self {
        Self {
            artifact_id: artifact_id.into(),
            name: None,
            parts: vec![TaskPart::Data(data)],
        }
    }
    pub fn validate(&self) -> Result<(), TaskGatewayError> {
        validate_task_identifier(&self.artifact_id)?;
        if self
            .name
            .as_ref()
            .is_some_and(|name| name.len() > 256 || name.contains('\0'))
        {
            return Err(TaskGatewayError::InvalidRequest);
        }
        validate_parts(&self.parts)
    }
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSnapshot {
    pub task_id: String,
    pub context_id: String,
    pub state: TaskState,
    pub artifacts: Vec<TaskArtifact>,
    pub error_code: Option<String>,
}
impl TaskSnapshot {
    pub fn validate(&self) -> Result<(), TaskGatewayError> {
        validate_task_identifier(&self.task_id)?;
        validate_task_identifier(&self.context_id)?;
        if self.artifacts.len() > MAX_TASK_ARTIFACTS {
            return Err(TaskGatewayError::InvalidRequest);
        }
        for artifact in &self.artifacts {
            artifact.validate()?;
        }
        if let Some(code) = &self.error_code {
            validate_task_identifier(code)?;
        }
        bounded_json(self, MAX_TASK_WIRE_BYTES / 2)
    }
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskMessage {
    pub message_id: String,
    pub context_id: String,
    pub parts: Vec<TaskPart>,
}
impl TaskMessage {
    pub fn validate(&self) -> Result<(), TaskGatewayError> {
        validate_task_identifier(&self.message_id)?;
        validate_task_identifier(&self.context_id)?;
        validate_parts(&self.parts)?;
        bounded_json(self, MAX_TASK_WIRE_BYTES / 2)
    }
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "data")]
pub enum TaskReply {
    Task(TaskSnapshot),
    Message(TaskMessage),
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskListRequest {
    pub context_id: Option<String>,
    pub state: Option<TaskState>,
    pub page_size: u32,
    pub page_token: Option<String>,
    pub include_artifacts: bool,
}
impl TaskListRequest {
    pub fn validate(&self) -> Result<(), TaskGatewayError> {
        if !(1..=32).contains(&self.page_size) {
            return Err(TaskGatewayError::InvalidRequest);
        }
        if let Some(id) = &self.context_id {
            validate_task_identifier(id)?;
        }
        if self
            .page_token
            .as_ref()
            .is_some_and(|token| token.len() > 128 || token.contains('\0'))
        {
            return Err(TaskGatewayError::InvalidRequest);
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskList {
    pub tasks: Vec<TaskSnapshot>,
    pub next_page_token: Option<String>,
    pub total_size: u32,
}
impl TaskList {
    pub fn validate(&self) -> Result<(), TaskGatewayError> {
        if self.tasks.len() > 32
            || self.total_size > i32::MAX as u32
            || self
                .next_page_token
                .as_ref()
                .is_some_and(|token| token.len() > 128 || token.contains('\0'))
        {
            return Err(TaskGatewayError::InvalidRequest);
        }
        for task in &self.tasks {
            task.validate()?;
        }
        bounded_json(self, MAX_TASK_WIRE_BYTES / 2)
    }
}
pub type TaskEventStream = BoxStream<'static, Result<TaskSnapshot, TaskGatewayError>>;

/// Implementations MUST authorize subject atomically with reads and mutations,
/// validate canonical transitions, deduplicate idempotency keys, and own all work.
/// Dropping a subscriber does not cancel a task. Streams yield full snapshots;
/// each subscription begins with the current snapshot and terminates on a terminal
/// snapshot. A lagged subscriber must receive an explicit error, never silent loss.
#[async_trait]
pub trait TaskBackend: Send + Sync + 'static {
    async fn list(
        &self,
        _subject: &str,
        _request: TaskListRequest,
    ) -> Result<TaskList, TaskGatewayError> {
        Err(TaskGatewayError::Unsupported)
    }
    async fn send(
        &self,
        subject: &str,
        request: TaskRequest,
    ) -> Result<TaskSnapshot, TaskGatewayError>;
    async fn send_reply(
        &self,
        subject: &str,
        request: TaskRequest,
    ) -> Result<TaskReply, TaskGatewayError> {
        self.send(subject, request).await.map(TaskReply::Task)
    }
    async fn get(&self, subject: &str, task_id: &str) -> Result<TaskSnapshot, TaskGatewayError>;
    async fn cancel(&self, subject: &str, task_id: &str) -> Result<TaskSnapshot, TaskGatewayError>;
    async fn subscribe(
        &self,
        subject: &str,
        task_id: &str,
    ) -> Result<TaskEventStream, TaskGatewayError>;
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum TaskGatewayError {
    #[error("invalid task request")]
    InvalidRequest,
    #[error("task authentication required")]
    Unauthorized,
    #[error("task access denied")]
    Forbidden,
    #[error("task not found")]
    NotFound,
    #[error("task operation unsupported")]
    Unsupported,
    #[error("task gateway capacity exhausted")]
    Busy,
    #[error("task operation deadline exceeded")]
    Deadline,
    #[error("task protocol validation failed")]
    Protocol,
    #[error("task transport failed")]
    Transport,
    #[error("task server failed")]
    Server,
    #[error("task backend failed")]
    Internal,
}
impl TaskGatewayError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequest => "a2a_task_invalid_request",
            Self::Unauthorized => "a2a_task_unauthorized",
            Self::Forbidden => "a2a_task_forbidden",
            Self::NotFound => "a2a_task_not_found",
            Self::Unsupported => "a2a_task_unsupported",
            Self::Busy => "a2a_task_busy",
            Self::Deadline => "a2a_task_deadline",
            Self::Protocol => "a2a_task_protocol",
            Self::Transport => "a2a_task_transport",
            Self::Server => "a2a_task_server",
            Self::Internal => "a2a_task_internal",
        }
    }
}
#[derive(Clone)]
pub struct PeerCredential {
    pub subject: String,
    pub bearer_token: String,
}
impl PeerCredential {
    pub fn validate(&self) -> Result<(), TaskGatewayError> {
        validate_task_identifier(&self.subject)?;
        if !(32..=256).contains(&self.bearer_token.len())
            || !self
                .bearer_token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        {
            return Err(TaskGatewayError::InvalidRequest);
        }
        Ok(())
    }
}
impl fmt::Debug for PeerCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PeerCredential")
            .field("subject", &self.subject)
            .field("bearer_token", &"[redacted]")
            .finish()
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskBinding {
    HttpJson,
    JsonRpc,
}
impl TaskBinding {
    pub(crate) const fn protocol(self) -> &'static str {
        match self {
            Self::HttpJson => "HTTP+JSON",
            Self::JsonRpc => "JSONRPC",
        }
    }
}

pub(crate) fn validate_task_identifier(value: &str) -> Result<(), TaskGatewayError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
    {
        return Err(TaskGatewayError::InvalidRequest);
    }
    Ok(())
}
pub(crate) fn validate_parts(parts: &[TaskPart]) -> Result<(), TaskGatewayError> {
    if parts.is_empty() || parts.len() > MAX_TASK_PARTS {
        return Err(TaskGatewayError::InvalidRequest);
    }
    for part in parts {
        part.validate()?;
    }
    Ok(())
}
pub(crate) fn bounded_json(value: &impl Serialize, limit: usize) -> Result<(), TaskGatewayError> {
    let bytes = serde_json::to_vec(value).map_err(|_| TaskGatewayError::InvalidRequest)?;
    if bytes.len() > limit {
        return Err(TaskGatewayError::InvalidRequest);
    }
    Ok(())
}
