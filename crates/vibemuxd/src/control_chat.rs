//! Authenticated Control v11 adapter for the fixed persistent-chat API.
use super::*;
use crate::chat_session::{ChatService, api};

const CHAT_COMMAND_DEADLINE: Duration = Duration::from_secs(35);
const RESPONSE_HEADROOM: usize = 1024;

impl ControlClient {
    pub async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, ControlError> {
        if let ChatRequest::CommandProgress {
            chat_id,
            generation,
            request_id,
        } = request
        {
            let result = self
                .chat_command_progress(&ChatCommandProgressRequest {
                    scope: vibemux_types::chat_command_progress::CommandProgressScope::Saved,
                    chat_id: *chat_id,
                    generation: *generation,
                    request_id: *request_id,
                })
                .await?;
            return Ok(ChatResponse::CommandProgress {
                chat_id: result.chat_id,
                generation: result.generation,
                request_id: result.request_id,
                snapshot: result.snapshot,
            });
        }
        self.require_operation(ControlOperation::Chat)?;
        let argument = serde_json::to_string(request).map_err(|_| ControlError::InvalidRequest)?;
        match self
            .request_with_argument(
                ControlOperation::Chat,
                Some(argument),
                CHAT_COMMAND_DEADLINE,
            )
            .await?
        {
            ControlPayload::Chat(response) => Ok(*response),
            _ => Err(ControlError::InvalidFrame),
        }
    }
}

pub(super) async fn dispatch(
    service: &ChatService,
    argument: Option<String>,
) -> Result<ControlPayload, ControlError> {
    let argument = argument.ok_or(ControlError::InvalidRequest)?;
    if argument.len() > MAX_CONTROL_FRAME_BYTES - RESPONSE_HEADROOM {
        return Err(ControlError::InvalidRequest);
    }
    let request: ChatRequest =
        serde_json::from_str(&argument).map_err(|_| ControlError::InvalidRequest)?;
    // New variants never inherit the older outer operation's v11 admission.
    if matches!(&request, ChatRequest::CommandProgress { .. }) {
        return Err(ControlError::InvalidRequest);
    }
    let result = api::execute(service, request)
        .await
        .map_err(|error| ControlError::Plugin {
            code: error.code().to_owned(),
        })?;
    let payload = ControlPayload::Chat(Box::new(result));
    let size = serde_json::to_vec(&payload)
        .map_err(|_| ControlError::InvalidFrame)?
        .len();
    if size > MAX_CONTROL_FRAME_BYTES - RESPONSE_HEADROOM {
        return Err(ControlError::InvalidFrame);
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn chat_requires_authenticated_v11_and_strict_arguments() {
        let temp = tempfile::tempdir().expect("temporary daemon");
        let server = DaemonControlServer::start(
            &temp.path().join("state.sqlite3"),
            &temp.path().join("runtime"),
        )
        .await
        .expect("daemon");
        let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
        let list = ChatRequest::List {
            after: None,
            limit: 16,
            archived: false,
        };
        let mut old = client.clone();
        old.descriptor.version = CONTROL_PROTOCOL_V10;
        assert_eq!(
            old.chat(&list).await.expect_err("minimum version"),
            ControlError::UnsupportedVersion
        );
        let mut request = ControlRequest {
            version: CONTROL_PROTOCOL_V11,
            request_id: Uuid::new_v4().to_string(),
            token: "wrong".to_owned(),
            operation: ControlOperation::Chat,
            argument: Some(serde_json::to_string(&list).expect("request")),
        };
        assert_eq!(
            client.request_raw(request.clone()).await.expect_err("auth"),
            ControlError::Unauthorized
        );
        request.token = client.descriptor.token.clone();
        request.argument = Some(r#"{"operation":"list","path":"forged"}"#.to_owned());
        assert_eq!(
            client
                .request_raw(request.clone())
                .await
                .expect_err("unknown fields"),
            ControlError::InvalidRequest
        );
        request.argument = Some(serde_json::to_string(&list).expect("request"));
        request.version = CONTROL_PROTOCOL_V10;
        assert_eq!(
            client
                .request_raw(request)
                .await
                .expect_err("server version gate"),
            ControlError::UnsupportedVersion
        );
        assert!(
            matches!(client.chat(&list).await.expect("read"),ChatResponse::List{sessions,..} if sessions.is_empty())
        );
        assert!(
            !temp.path().join("chat_private").exists(),
            "listing must not create private storage"
        );
        server.shutdown().await.expect("shutdown");
    }
}
