//! Authenticated Control v14 approval adapter; no grants or vendor data in decisions.
use super::*;
use crate::chat_session::{ChatService, approval_api};

const APPROVAL_COMMAND_DEADLINE: Duration = Duration::from_secs(35);
const RESPONSE_HEADROOM: usize = 1024;

impl ControlClient {
    pub async fn chat_approval(
        &self,
        request: &ChatApprovalRequest,
    ) -> Result<ChatApprovalResponse, ControlError> {
        self.require_operation(ControlOperation::ChatApproval)?;
        request
            .validate()
            .map_err(|_| ControlError::InvalidRequest)?;
        let argument = serde_json::to_string(request).map_err(|_| ControlError::InvalidRequest)?;
        match self
            .request_with_argument(
                ControlOperation::ChatApproval,
                Some(argument),
                APPROVAL_COMMAND_DEADLINE,
            )
            .await?
        {
            ControlPayload::ChatApproval(response) => Ok(*response),
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
    let request: ChatApprovalRequest =
        serde_json::from_str(&argument).map_err(|_| ControlError::InvalidRequest)?;
    request
        .validate()
        .map_err(|_| ControlError::InvalidRequest)?;
    let response = approval_api::execute(service, request)
        .await
        .map_err(|error| ControlError::Plugin {
            code: error.code().to_owned(),
        })?;
    let payload = ControlPayload::ChatApproval(Box::new(response));
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
    async fn approvals_require_v14_authentication_and_closed_decision_fields() {
        let temp = tempfile::tempdir().expect("temporary daemon");
        let server = DaemonControlServer::start(
            &temp.path().join("state.sqlite3"),
            &temp.path().join("runtime"),
        )
        .await
        .expect("daemon");
        let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
        let query = ChatApprovalRequest::Pending {
            chat_id: vibemux_types::ChatId::new(),
            generation: 1,
            request_id: Uuid::new_v4(),
        };
        assert!(matches!(
            client.chat_approval(&query).await.expect("empty project"),
            ChatApprovalResponse::Pending { approval: None, .. }
        ));
        let mut old = client.clone();
        old.descriptor.version = CONTROL_PROTOCOL_V13;
        assert_eq!(
            old.chat_approval(&query).await.expect_err("minimum"),
            ControlError::UnsupportedVersion
        );
        let mut frame = ControlRequest {
            version: CONTROL_PROTOCOL_V14,
            request_id: Uuid::new_v4().to_string(),
            token: "wrong".into(),
            operation: ControlOperation::ChatApproval,
            argument: Some(serde_json::to_string(&query).expect("query")),
        };
        assert_eq!(
            client.request_raw(frame.clone()).await.expect_err("auth"),
            ControlError::Unauthorized
        );
        frame.token = client.descriptor.token.clone();
        frame.version = CONTROL_PROTOCOL_V13;
        assert_eq!(
            client
                .request_raw(frame.clone())
                .await
                .expect_err("old frame"),
            ControlError::UnsupportedVersion
        );
        frame.version = CONTROL_PROTOCOL_V14;
        let base = json!({"operation":"decide","chat_id":vibemux_types::ChatId::new(),"approval_id":vibemux_types::ChatApprovalId::new(),
            "generation":1,"request_id":Uuid::new_v4(),"expected_version":1,"operation_id":Uuid::new_v4(),"decision":"allow_once"});
        for key in [
            "path",
            "input",
            "vendor_request_id",
            "tool_grants",
            "profile_sha256",
            "fence",
        ] {
            let mut forged = base.clone();
            forged[key] = json!("untrusted");
            frame.argument = Some(forged.to_string());
            assert_eq!(
                client
                    .request_raw(frame.clone())
                    .await
                    .expect_err("closed arguments"),
                ControlError::InvalidRequest
            );
        }
        server.shutdown().await.expect("shutdown");
    }
}
