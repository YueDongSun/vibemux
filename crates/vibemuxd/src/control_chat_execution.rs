//! Authenticated Control v13 adapter for owner execution, without raw store rows.
use super::*;
use crate::chat_session::{ChatService, execution_api};

const EXECUTION_COMMAND_DEADLINE: Duration = Duration::from_secs(35);
const RESPONSE_HEADROOM: usize = 1024;

impl ControlClient {
    pub async fn chat_execution(
        &self,
        request: &ChatExecutionRequest,
    ) -> Result<ChatExecutionResponse, ControlError> {
        if let ChatExecutionRequest::CommandProgress {
            chat_id,
            generation,
            request_id,
        } = request
        {
            let result = self
                .chat_command_progress(&ChatCommandProgressRequest {
                    scope: vibemux_types::chat_command_progress::CommandProgressScope::Workspace,
                    chat_id: *chat_id,
                    generation: *generation,
                    request_id: *request_id,
                })
                .await?;
            return Ok(ChatExecutionResponse::CommandProgress {
                chat_id: result.chat_id,
                generation: result.generation,
                request_id: result.request_id,
                snapshot: result.snapshot,
            });
        }
        self.require_operation(ControlOperation::ChatExecution)?;
        request
            .validate()
            .map_err(|_| ControlError::InvalidRequest)?;
        let argument = serde_json::to_string(request).map_err(|_| ControlError::InvalidRequest)?;
        match self
            .request_with_argument(
                ControlOperation::ChatExecution,
                Some(argument),
                EXECUTION_COMMAND_DEADLINE,
            )
            .await?
        {
            ControlPayload::ChatExecution(response) => Ok(*response),
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
    let request: ChatExecutionRequest =
        serde_json::from_str(&argument).map_err(|_| ControlError::InvalidRequest)?;
    request
        .validate()
        .map_err(|_| ControlError::InvalidRequest)?;
    // New variants never inherit the older outer operation's v13 admission.
    if matches!(&request, ChatExecutionRequest::CommandProgress { .. }) {
        return Err(ControlError::InvalidRequest);
    }
    let response = execution_api::execute(service, request)
        .await
        .map_err(|error| ControlError::Plugin {
            code: error.code().to_owned(),
        })?;
    let payload = ControlPayload::ChatExecution(Box::new(response));
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
    use serde_json::json;

    #[tokio::test]
    async fn execution_requires_v13_authentication_consent_and_closed_arguments() {
        let temp = tempfile::tempdir().expect("temporary daemon");
        let server = DaemonControlServer::start(
            &temp.path().join("state.sqlite3"),
            &temp.path().join("runtime"),
        )
        .await
        .expect("daemon");
        let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
        let status = ChatExecutionRequest::Status {
            chat_id: vibemux_types::ChatId::new(),
        };
        assert!(matches!(
            client
                .chat_execution(&status)
                .await
                .expect("unknown owner read"),
            ChatExecutionResponse::Status { runtime: None }
        ));
        let mut old = client.clone();
        old.descriptor.version = CONTROL_PROTOCOL_V12;
        assert_eq!(
            old.chat_execution(&status)
                .await
                .expect_err("minimum version"),
            ControlError::UnsupportedVersion
        );
        let mut request = ControlRequest {
            version: CONTROL_PROTOCOL_V13,
            request_id: Uuid::new_v4().to_string(),
            token: "wrong".into(),
            operation: ControlOperation::ChatExecution,
            argument: Some(serde_json::to_string(&status).expect("JSON")),
        };
        assert_eq!(
            client.request_raw(request.clone()).await.expect_err("auth"),
            ControlError::Unauthorized
        );
        request.token = client.descriptor.token.clone();
        request.version = CONTROL_PROTOCOL_V12;
        assert_eq!(
            client
                .request_raw(request.clone())
                .await
                .expect_err("old frame"),
            ControlError::UnsupportedVersion
        );
        request.version = CONTROL_PROTOCOL_V13;
        let base = json!({"operation":"start","operation_id":Uuid::new_v4(),"chat_id":vibemux_types::ChatId::new(),"generation":1,"owner_version":1,"consent_execution":true});
        let mut forged = base.clone();
        forged["workspace_path"] = json!("C:/forged");
        request.argument = Some(forged.to_string());
        assert_eq!(
            client
                .request_raw(request.clone())
                .await
                .expect_err("forged path"),
            ControlError::InvalidRequest
        );
        let mut forged = base.clone();
        forged["tool_grants"] = json!(["process_exec"]);
        request.argument = Some(forged.to_string());
        assert_eq!(
            client
                .request_raw(request.clone())
                .await
                .expect_err("forged grants"),
            ControlError::InvalidRequest
        );
        let mut denied = base;
        denied["consent_execution"] = json!(false);
        request.argument = Some(denied.to_string());
        assert_eq!(
            client.request_raw(request).await.expect_err("consent"),
            ControlError::InvalidRequest
        );
        server.shutdown().await.expect("shutdown");
    }
}
