//! Explicit authenticated v17 read for private transient command observations.
use super::*;
use crate::chat_session::{api, execution_api};
use vibemux_types::{
    ChatId,
    chat_command_progress::{
        CommandProgressScope, CommandProgressSnapshot, MAX_COMMAND_PROGRESS_INTEGER,
    },
};

const COMMAND_PROGRESS_DEADLINE: Duration = Duration::from_secs(5);
const COMMAND_PROGRESS_ARGUMENT_BYTES: usize = 1024;
const RESPONSE_HEADROOM: usize = 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatCommandProgressRequest {
    pub scope: CommandProgressScope,
    pub chat_id: ChatId,
    pub generation: u64,
    pub request_id: Uuid,
}
impl ChatCommandProgressRequest {
    pub fn validate(&self) -> Result<(), ControlError> {
        if self.generation == 0
            || self.generation > MAX_COMMAND_PROGRESS_INTEGER
            || self.request_id.is_nil()
        {
            return Err(ControlError::InvalidRequest);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatCommandProgressResponse {
    pub schema_version: u32,
    pub scope: CommandProgressScope,
    pub chat_id: ChatId,
    pub generation: u64,
    pub request_id: Uuid,
    pub snapshot: Option<CommandProgressSnapshot>,
}
impl ChatCommandProgressResponse {
    fn validate_binding(&self, request: &ChatCommandProgressRequest) -> Result<(), ControlError> {
        if self.schema_version != 1
            || self.scope != request.scope
            || self.chat_id != request.chat_id
            || self.generation != request.generation
            || self.request_id != request.request_id
        {
            return Err(ControlError::InvalidFrame);
        }
        if let Some(snapshot) = &self.snapshot {
            snapshot
                .validate()
                .map_err(|_| ControlError::InvalidFrame)?;
        }
        Ok(())
    }
}

impl ControlClient {
    pub async fn chat_command_progress(
        &self,
        request: &ChatCommandProgressRequest,
    ) -> Result<ChatCommandProgressResponse, ControlError> {
        self.require_operation(ControlOperation::ChatCommandProgress)?;
        request.validate()?;
        let argument = serde_json::to_string(request).map_err(|_| ControlError::InvalidRequest)?;
        if argument.len() > COMMAND_PROGRESS_ARGUMENT_BYTES {
            return Err(ControlError::InvalidRequest);
        }
        match self
            .request_with_argument(
                ControlOperation::ChatCommandProgress,
                Some(argument),
                COMMAND_PROGRESS_DEADLINE,
            )
            .await?
        {
            ControlPayload::ChatCommandProgress(response) => {
                response.validate_binding(request)?;
                Ok(*response)
            }
            _ => Err(ControlError::InvalidFrame),
        }
    }
}

pub(super) async fn dispatch(
    service: &ChatService,
    argument: Option<String>,
) -> Result<ControlPayload, ControlError> {
    let argument = argument.ok_or(ControlError::InvalidRequest)?;
    if argument.len() > COMMAND_PROGRESS_ARGUMENT_BYTES {
        return Err(ControlError::InvalidRequest);
    }
    let request: ChatCommandProgressRequest =
        serde_json::from_str(&argument).map_err(|_| ControlError::InvalidRequest)?;
    request.validate()?;
    let snapshot = match request.scope {
        CommandProgressScope::Saved => match api::execute(
            service,
            ChatRequest::CommandProgress {
                chat_id: request.chat_id,
                generation: request.generation,
                request_id: request.request_id,
            },
        )
        .await
        .map_err(|error| ControlError::Plugin {
            code: error.code().to_owned(),
        })? {
            ChatResponse::CommandProgress {
                chat_id,
                generation,
                request_id,
                snapshot,
            } if chat_id == request.chat_id
                && generation == request.generation
                && request_id == request.request_id =>
            {
                snapshot
            }
            _ => return Err(ControlError::InvalidFrame),
        },
        CommandProgressScope::Workspace => match execution_api::execute(
            service,
            ChatExecutionRequest::CommandProgress {
                chat_id: request.chat_id,
                generation: request.generation,
                request_id: request.request_id,
            },
        )
        .await
        .map_err(|error| ControlError::Plugin {
            code: error.code().to_owned(),
        })? {
            ChatExecutionResponse::CommandProgress {
                chat_id,
                generation,
                request_id,
                snapshot,
            } if chat_id == request.chat_id
                && generation == request.generation
                && request_id == request.request_id =>
            {
                snapshot
            }
            _ => return Err(ControlError::InvalidFrame),
        },
    };
    let response = ChatCommandProgressResponse {
        schema_version: 1,
        scope: request.scope,
        chat_id: request.chat_id,
        generation: request.generation,
        request_id: request.request_id,
        snapshot,
    };
    response.validate_binding(&request)?;
    let payload = ControlPayload::ChatCommandProgress(Box::new(response));
    if serde_json::to_vec(&payload)
        .map_err(|_| ControlError::InvalidFrame)?
        .len()
        > MAX_CONTROL_FRAME_BYTES - RESPONSE_HEADROOM
    {
        return Err(ControlError::InvalidFrame);
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vibemux_types::chat_command_progress::{
        CommandProgressContent, CommandProgressEntry, CommandProgressPhase,
    };

    fn request() -> ChatCommandProgressRequest {
        ChatCommandProgressRequest {
            scope: CommandProgressScope::Saved,
            chat_id: ChatId::new(),
            generation: 1,
            request_id: Uuid::new_v4(),
        }
    }
    #[test]
    fn progress_requires_v17_and_closed_identifier_only_arguments() {
        assert_eq!(
            ControlOperation::ChatCommandProgress.minimum_protocol_version(),
            17
        );
        assert_eq!(ControlOperation::Chat.minimum_protocol_version(), 11);
        assert_eq!(
            ControlOperation::ChatExecution.minimum_protocol_version(),
            13
        );
        let request = request();
        request.validate().expect("valid identifiers");
        let mut value = serde_json::to_value(&request).expect("request");
        value["command"] = serde_json::json!("untrusted command");
        assert!(serde_json::from_value::<ChatCommandProgressRequest>(value).is_err());
        let mut invalid = request;
        invalid.generation = 0;
        assert!(invalid.validate().is_err());
        invalid.generation = 1;
        invalid.request_id = Uuid::nil();
        assert!(invalid.validate().is_err());
    }
    #[test]
    fn old_assistant_live_json_stays_unchanged() {
        let chat_id = ChatId::new();
        let request_id = Uuid::new_v4();
        let value = serde_json::to_value(ChatRequest::Live {
            chat_id,
            generation: 7,
            request_id,
            revision: Some(2),
            byte_offset: 8,
        })
        .expect("old request");
        assert_eq!(
            value,
            serde_json::json!({"operation":"live","chat_id":chat_id,"generation":7,"request_id":request_id,"revision":2,"byte_offset":8})
        );
        let value = serde_json::to_value(ChatResponse::Live {
            chat_id,
            generation: 7,
            request_id,
            snapshot: None,
        })
        .expect("old reply");
        assert_eq!(
            value,
            serde_json::json!({"operation":"live","chat_id":chat_id,"generation":7,"request_id":request_id,"snapshot":null})
        );
    }
    #[test]
    fn full_outer_progress_frame_bounds_escaping_and_redacts_debug() {
        let request = request();
        let rows = (0..4)
            .map(|_| CommandProgressEntry {
                record_id: Uuid::new_v4(),
                phase: CommandProgressPhase::InProgress,
                content: CommandProgressContent {
                    command: "\u{1}".repeat(8 * 1024),
                    command_original_bytes: 8 * 1024,
                    command_truncated: false,
                    working_directory: "\0".repeat(32 * 1024),
                    directory_original_bytes: 32 * 1024,
                    directory_truncated: false,
                    output_tail: Some("PRIVATE_OUTPUT".repeat(1024)),
                    output_original_bytes: Some(14 * 1024),
                    output_omitted_bytes: 0,
                },
            })
            .collect();
        let snapshot = CommandProgressSnapshot::bounded(Uuid::new_v4(), 1, rows, 0, false)
            .expect("bounded escaped snapshot");
        let response = ChatCommandProgressResponse {
            schema_version: 1,
            scope: request.scope,
            chat_id: request.chat_id,
            generation: request.generation,
            request_id: request.request_id,
            snapshot: Some(snapshot),
        };
        response.validate_binding(&request).expect("same binding");
        let outer = ControlResponse::success(
            "\\".repeat(128),
            ControlPayload::ChatCommandProgress(Box::new(response)),
        );
        let encoded = serde_json::to_vec(&outer).expect("full frame");
        assert!(encoded.len() < MAX_CONTROL_FRAME_BYTES);
        assert!(!format!("{outer:?}").contains("PRIVATE_OUTPUT"));
    }
}
