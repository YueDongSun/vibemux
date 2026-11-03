//! Authenticated Control v12 boundary for lifetime chat ownership.
use super::*;
use crate::chat_session::{ChatService, owner_api};

const OWNER_COMMAND_DEADLINE: Duration = Duration::from_secs(35);
const RESPONSE_HEADROOM: usize = 1024;

impl ControlClient {
    pub async fn chat_owner(
        &self,
        request: &ChatOwnerRequest,
    ) -> Result<ChatOwnerResponse, ControlError> {
        self.require_operation(ControlOperation::ChatOwner)?;
        request
            .validate()
            .map_err(|_| ControlError::InvalidRequest)?;
        let argument = serde_json::to_string(request).map_err(|_| ControlError::InvalidRequest)?;
        match self
            .request_with_argument(
                ControlOperation::ChatOwner,
                Some(argument),
                OWNER_COMMAND_DEADLINE,
            )
            .await?
        {
            ControlPayload::ChatOwner(response) => Ok(*response),
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
    let request: ChatOwnerRequest =
        serde_json::from_str(&argument).map_err(|_| ControlError::InvalidRequest)?;
    let response = owner_api::execute(service, request)
        .await
        .map_err(|error| ControlError::Plugin {
            code: error.code().to_owned(),
        })?;
    let payload = ControlPayload::ChatOwner(Box::new(response));
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

    #[tokio::test]
    async fn owner_commands_require_v12_authentication_and_closed_arguments() {
        let temp = tempfile::tempdir().expect("temporary daemon");
        let server = DaemonControlServer::start(
            &temp.path().join("state.sqlite3"),
            &temp.path().join("runtime"),
        )
        .await
        .expect("server");
        let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
        let list = ChatOwnerRequest::List {
            after: None,
            limit: 16,
            include_finalized: false,
        };
        assert!(
            matches!(client.chat_owner(&list).await.expect("read-only empty listing"),ChatOwnerResponse::List {owners,..} if owners.is_empty())
        );
        let mut old = client.clone();
        old.descriptor.version = CONTROL_PROTOCOL_V11;
        assert_eq!(
            old.chat_owner(&list).await.expect_err("v12 minimum"),
            ControlError::UnsupportedVersion
        );
        let mut request = ControlRequest {
            version: CONTROL_PROTOCOL_V12,
            request_id: Uuid::new_v4().to_string(),
            token: "incorrect".into(),
            operation: ControlOperation::ChatOwner,
            argument: Some(serde_json::to_string(&list).expect("JSON")),
        };
        assert_eq!(
            client.request_raw(request.clone()).await.expect_err("auth"),
            ControlError::Unauthorized
        );
        request.token = client.descriptor.token.clone();
        request.version = CONTROL_PROTOCOL_V11;
        assert_eq!(
            client
                .request_raw(request.clone())
                .await
                .expect_err("old frame"),
            ControlError::UnsupportedVersion
        );
        request.version = CONTROL_PROTOCOL_V12;
        request.argument = Some(r#"{"operation":"list","root":"C:/forged"}"#.into());
        assert_eq!(
            client
                .request_raw(request)
                .await
                .expect_err("closed schema"),
            ControlError::InvalidRequest
        );
        server.shutdown().await.expect("shutdown");
    }
}
