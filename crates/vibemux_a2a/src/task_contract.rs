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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const REFUSED: Result<(), TaskGatewayError> = Err(TaskGatewayError::InvalidRequest);

    fn request() -> TaskRequest {
        TaskRequest {
            request_id: "request_1".to_string(),
            context_id: "context_1".to_string(),
            task_id: Some("task_1".to_string()),
            idempotency_key: "key_1".to_string(),
            payload: json!({"goal": "review"}),
        }
    }

    fn snapshot(artifacts: Vec<TaskArtifact>) -> TaskSnapshot {
        TaskSnapshot {
            task_id: "task_1".to_string(),
            context_id: "context_1".to_string(),
            state: TaskState::Working,
            artifacts,
            error_code: None,
        }
    }

    /// A JSON string of exactly `bytes` serialized bytes, quotes included.
    fn json_text(bytes: usize) -> Value {
        json!("x".repeat(bytes - 2))
    }

    fn large_artifact(artifact_id: &str) -> TaskArtifact {
        TaskArtifact::data(artifact_id, json_text(12 * 1024))
    }

    #[test]
    fn task_identifiers_are_short_ascii_names() {
        for valid in ["a", "A-z_0.9", "a".repeat(128).as_str()] {
            assert_eq!(validate_task_identifier(valid), Ok(()), "{valid}");
        }
        for invalid in [
            "",
            "a".repeat(129).as_str(),
            "a b",
            "a/b",
            "a:b",
            "é",
            "a\0",
        ] {
            assert_eq!(validate_task_identifier(invalid), REFUSED, "{invalid:?}");
        }
    }

    #[test]
    fn a_request_bounds_its_identifiers_and_payload() {
        assert_eq!(request().validate(), Ok(()));
        assert_eq!(
            TaskRequest {
                task_id: None,
                ..request()
            }
            .validate(),
            Ok(())
        );
        let changes: [fn(&mut TaskRequest); 4] = [
            |request| request.request_id = "request 1".to_string(),
            |request| request.context_id = String::new(),
            |request| request.task_id = Some("task/1".to_string()),
            |request| request.idempotency_key = "k".repeat(129),
        ];
        for (index, change) in changes.into_iter().enumerate() {
            let mut changed = request();
            change(&mut changed);
            assert_eq!(changed.validate(), REFUSED, "change {index}");
        }
        let with_payload = |bytes| TaskRequest {
            payload: json_text(bytes),
            ..request()
        };
        assert_eq!(with_payload(MAX_TASK_PAYLOAD_BYTES).validate(), Ok(()));
        assert_eq!(with_payload(MAX_TASK_PAYLOAD_BYTES + 1).validate(), REFUSED);
    }

    #[test]
    fn parts_bound_their_content_and_names() {
        let raw =
            |bytes: usize, media_type: Option<String>, filename: Option<String>| TaskPart::Raw {
                bytes: vec![0; bytes],
                media_type,
                filename,
            };
        let url = |url: String| TaskPart::Url {
            url,
            media_type: None,
            filename: None,
        };
        let long_url = |bytes: usize| {
            let prefix = "https://example.test/";
            url(format!("{prefix}{}", "a".repeat(bytes - prefix.len())))
        };
        for valid in [
            TaskPart::Text("x".repeat(MAX_TASK_PAYLOAD_BYTES)),
            TaskPart::Data(json_text(MAX_TASK_PAYLOAD_BYTES)),
            raw(MAX_TASK_PAYLOAD_BYTES, None, None),
            raw(1, Some("t".repeat(128)), Some("f".repeat(256))),
            url("http://example.test/a".to_string()),
            long_url(2048),
        ] {
            assert_eq!(valid.validate(), Ok(()), "{valid:?}");
        }
        for invalid in [
            TaskPart::Text("x".repeat(MAX_TASK_PAYLOAD_BYTES + 1)),
            TaskPart::Data(json_text(MAX_TASK_PAYLOAD_BYTES + 1)),
            raw(MAX_TASK_PAYLOAD_BYTES + 1, None, None),
            raw(1, Some("t".repeat(129)), None),
            raw(1, Some("text/plain\r".to_string()), None),
            raw(1, None, Some("f".repeat(257))),
            raw(1, None, Some("a\0b".to_string())),
            long_url(2049),
            url("not a url".to_string()),
            url("ftp://example.test/a".to_string()),
            url("https://user@example.test/a".to_string()),
            url("https://:secret@example.test/a".to_string()),
            TaskPart::Url {
                url: "https://example.test/a".to_string(),
                media_type: Some("text/plain\n".to_string()),
                filename: None,
            },
            TaskPart::Url {
                url: "https://example.test/a".to_string(),
                media_type: None,
                filename: Some("a\rb".to_string()),
            },
        ] {
            assert_eq!(invalid.validate(), REFUSED);
        }
        let text = TaskPart::Text("x".to_string());
        assert_eq!(validate_parts(&vec![text.clone(); MAX_TASK_PARTS]), Ok(()));
        assert_eq!(
            validate_parts(&vec![text.clone(); MAX_TASK_PARTS + 1]),
            REFUSED
        );
        assert_eq!(validate_parts(&[]), REFUSED);
        assert_eq!(
            validate_parts(&[text, raw(1, None, Some("a\nb".to_string()))]),
            REFUSED
        );
    }

    #[test]
    fn artifacts_snapshots_and_messages_are_bounded() {
        let artifact = TaskArtifact::data("artifact_1", json!(1));
        assert_eq!(
            (artifact.name.as_deref(), artifact.parts.as_slice()),
            (None, &[TaskPart::Data(json!(1))][..])
        );
        let named = |name: String| TaskArtifact {
            name: Some(name),
            ..artifact.clone()
        };
        assert_eq!(named("n".repeat(256)).validate(), Ok(()));
        for invalid in [
            named("n".repeat(257)),
            named("a\0b".to_string()),
            TaskArtifact::data("artifact 1", json!(1)),
            TaskArtifact {
                parts: Vec::new(),
                ..artifact.clone()
            },
        ] {
            assert_eq!(invalid.validate(), REFUSED, "{invalid:?}");
        }

        let numbered = |count: usize| {
            (0..count)
                .map(|index| TaskArtifact::data(format!("artifact_{index}"), json!(index)))
                .collect::<Vec<_>>()
        };
        assert_eq!(snapshot(numbered(MAX_TASK_ARTIFACTS)).validate(), Ok(()));
        // Each large artifact is valid; two fit the 32 KiB snapshot, three do not.
        let large = |count: usize| {
            (0..count)
                .map(|index| large_artifact(&format!("artifact_{index}")))
                .collect::<Vec<_>>()
        };
        assert_eq!(snapshot(large(2)).validate(), Ok(()));
        for invalid in [
            snapshot(numbered(MAX_TASK_ARTIFACTS + 1)),
            snapshot(large(3)),
            snapshot(vec![named("a\0b".to_string())]),
            TaskSnapshot {
                task_id: "task 1".to_string(),
                ..snapshot(Vec::new())
            },
            TaskSnapshot {
                context_id: String::new(),
                ..snapshot(Vec::new())
            },
            TaskSnapshot {
                error_code: Some("worker failed".to_string()),
                ..snapshot(Vec::new())
            },
        ] {
            assert_eq!(invalid.validate(), REFUSED);
        }

        let message = |parts: Vec<TaskPart>| TaskMessage {
            message_id: "message_1".to_string(),
            context_id: "context_1".to_string(),
            parts,
        };
        let large_text = || TaskPart::Text("x".repeat(12 * 1024));
        assert_eq!(message(vec![large_text(), large_text()]).validate(), Ok(()));
        for invalid in [
            message(Vec::new()),
            message(vec![large_text(), large_text(), large_text()]),
            TaskMessage {
                message_id: "message 1".to_string(),
                ..message(vec![large_text()])
            },
            TaskMessage {
                context_id: String::new(),
                ..message(vec![large_text()])
            },
        ] {
            assert_eq!(invalid.validate(), REFUSED);
        }
    }

    #[test]
    fn a_list_request_and_a_list_are_bounded() {
        let list_request = |page_size: u32| TaskListRequest {
            context_id: Some("context_1".to_string()),
            state: None,
            page_size,
            page_token: Some("t".repeat(128)),
            include_artifacts: false,
        };
        for page_size in [1, 32] {
            assert_eq!(list_request(page_size).validate(), Ok(()), "{page_size}");
        }
        for invalid in [
            list_request(0),
            list_request(33),
            TaskListRequest {
                context_id: Some("context 1".to_string()),
                ..list_request(1)
            },
            TaskListRequest {
                page_token: Some("t".repeat(129)),
                ..list_request(1)
            },
            TaskListRequest {
                page_token: Some("a\0b".to_string()),
                ..list_request(1)
            },
        ] {
            assert_eq!(invalid.validate(), REFUSED, "{invalid:?}");
        }

        let list = |tasks: Vec<TaskSnapshot>| TaskList {
            tasks,
            next_page_token: Some("t".repeat(128)),
            total_size: i32::MAX as u32,
        };
        assert_eq!(list(vec![snapshot(Vec::new()); 32]).validate(), Ok(()));
        let large = |index: usize| snapshot(vec![large_artifact(&format!("artifact_{index}"))]);
        assert_eq!(list((0..2).map(large).collect()).validate(), Ok(()));
        for invalid in [
            list(vec![snapshot(Vec::new()); 33]),
            list((0..3).map(large).collect()),
            TaskList {
                total_size: i32::MAX as u32 + 1,
                ..list(Vec::new())
            },
            TaskList {
                next_page_token: Some("t".repeat(129)),
                ..list(Vec::new())
            },
            TaskList {
                next_page_token: Some("a\0b".to_string()),
                ..list(Vec::new())
            },
            list(vec![TaskSnapshot {
                task_id: "task 1".to_string(),
                ..snapshot(Vec::new())
            }]),
        ] {
            assert_eq!(invalid.validate(), REFUSED);
        }
    }

    #[test]
    fn a_peer_credential_is_validated_and_its_token_never_printed() {
        let credential = |subject: &str, bearer_token: String| PeerCredential {
            subject: subject.to_string(),
            bearer_token,
        };
        for token in ["a".repeat(32), "a".repeat(256), "aA0-_.".repeat(6)] {
            assert_eq!(
                credential("alice", token.clone()).validate(),
                Ok(()),
                "{token}"
            );
        }
        for (subject, token) in [
            ("alice", "a".repeat(31)),
            ("alice", "a".repeat(257)),
            ("alice", format!("{} ", "a".repeat(32))),
            ("alice", format!("{}+", "a".repeat(32))),
            ("alice smith", "a".repeat(32)),
            ("", "a".repeat(32)),
        ] {
            assert_eq!(
                credential(subject, token.clone()).validate(),
                REFUSED,
                "{subject:?} {}",
                token.len()
            );
        }
        let secret = "s3cr3t_token_value_".repeat(2);
        let printed = format!("{:?}", credential("alice", secret.clone()));
        assert!(printed.contains("alice") && printed.contains("[redacted]"));
        assert!(!printed.contains(&secret) && !printed.contains("s3cr3t"));
    }

    #[test]
    fn states_bindings_and_errors_keep_their_names() {
        for (state, terminal, name) in [
            (TaskState::Submitted, false, "submitted"),
            (TaskState::Working, false, "working"),
            (TaskState::InputRequired, false, "input_required"),
            (TaskState::AuthRequired, false, "auth_required"),
            (TaskState::Completed, true, "completed"),
            (TaskState::Failed, true, "failed"),
            (TaskState::Canceled, true, "canceled"),
            (TaskState::Rejected, true, "rejected"),
        ] {
            assert_eq!(state.is_terminal(), terminal, "{state:?}");
            assert_eq!(json!(state), json!(name));
        }
        assert_eq!(
            (
                TaskBinding::HttpJson.protocol(),
                TaskBinding::JsonRpc.protocol()
            ),
            ("HTTP+JSON", "JSONRPC")
        );
        assert_eq!(
            (json!(TaskBinding::HttpJson), json!(TaskBinding::JsonRpc)),
            (json!("http_json"), json!("json_rpc"))
        );
        for (error, code) in [
            (TaskGatewayError::InvalidRequest, "a2a_task_invalid_request"),
            (TaskGatewayError::Unauthorized, "a2a_task_unauthorized"),
            (TaskGatewayError::Forbidden, "a2a_task_forbidden"),
            (TaskGatewayError::NotFound, "a2a_task_not_found"),
            (TaskGatewayError::Unsupported, "a2a_task_unsupported"),
            (TaskGatewayError::Busy, "a2a_task_busy"),
            (TaskGatewayError::Deadline, "a2a_task_deadline"),
            (TaskGatewayError::Protocol, "a2a_task_protocol"),
            (TaskGatewayError::Transport, "a2a_task_transport"),
            (TaskGatewayError::Server, "a2a_task_server"),
            (TaskGatewayError::Internal, "a2a_task_internal"),
        ] {
            assert_eq!(error.code(), code);
        }
    }
}
