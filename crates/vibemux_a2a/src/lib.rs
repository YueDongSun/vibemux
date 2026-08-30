#![forbid(unsafe_code)]
//! Minimal, loopback-only A2A v1 information sharing boundary.

pub mod task_client;
pub mod task_contract;
pub mod task_grpc;
pub mod task_runtime;
pub mod task_server;
pub use task_client::{TaskClient, TaskClientConfig};
pub use task_contract::*;
pub use task_grpc::{GrpcTaskClient, GrpcTaskServer};
pub use task_server::{TaskServer, TaskServerConfig};
pub(crate) mod task_wire;

use std::{
    collections::{BTreeMap, VecDeque},
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use a2a::{
    A2AError, AgentCapabilities, AgentCard, AgentInterface, AgentSkill, Message, Part, PartContent,
    Role, SendMessageRequest, SendMessageResponse, StreamResponse, TRANSPORT_PROTOCOL_HTTP_JSON,
};
use a2a_client::{A2AClientFactory, agent_card::AgentCardResolver, rest::RestTransportFactory};
use a2a_server::{
    AgentExecutor, DefaultRequestHandler, ExecutorContext, InMemoryTaskStore, StaticAgentCard,
    agent_card::agent_card_router, rest::rest_router,
};
use axum::{Router, extract::DefaultBodyLimit};
use futures::{stream, stream::BoxStream};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle, time::timeout};

pub const A2A_SPEC_VERSION: &str = "1.0";
pub const INFORMATION_SHARE_CONTRACT: &str = "vibemux.info.share.v1";
pub const INFORMATION_ACK_CONTRACT: &str = "vibemux.info.ack.v1";
pub const MAX_INFORMATION_TEXT_BYTES: usize = 4 * 1024;
pub const MAX_METADATA_ENTRIES: usize = 16;
pub const MAX_METADATA_KEY_BYTES: usize = 64;
pub const MAX_METADATA_VALUE_BYTES: usize = 256;
pub const MAX_IDENTIFIER_BYTES: usize = 128;
pub const MAX_HTTP_BODY_BYTES: usize = 16 * 1024;
pub const MAX_CAPTURED_SHARES: usize = 32;
pub const CLIENT_DEADLINE: Duration = Duration::from_secs(5);
pub const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InformationShare {
    pub correlation_id: String,
    pub sender_peer_id: String,
    pub information_kind: String,
    pub text: String,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

impl InformationShare {
    pub fn validate(&self) -> Result<(), InformationShareError> {
        validate_identifier("correlation_id", &self.correlation_id)?;
        validate_identifier("sender_peer_id", &self.sender_peer_id)?;
        validate_identifier("information_kind", &self.information_kind)?;
        if self.text.is_empty() || self.text.len() > MAX_INFORMATION_TEXT_BYTES {
            return Err(InformationShareError::InvalidText);
        }
        if self.metadata.len() > MAX_METADATA_ENTRIES {
            return Err(InformationShareError::InvalidMetadata);
        }
        if self.metadata.iter().any(|(key, value)| {
            key.is_empty()
                || key.len() > MAX_METADATA_KEY_BYTES
                || value.len() > MAX_METADATA_VALUE_BYTES
                || key.contains('\0')
                || value.contains('\0')
        }) {
            return Err(InformationShareError::InvalidMetadata);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InformationAcknowledgement {
    pub correlation_id: String,
    pub receiver_peer_id: String,
    pub accepted: bool,
}

#[derive(Debug, Error)]
pub enum InformationShareError {
    #[error("invalid A2A information identifier: {0}")]
    InvalidIdentifier(&'static str),
    #[error("A2A information text is empty or too large")]
    InvalidText,
    #[error("A2A information metadata is invalid or too large")]
    InvalidMetadata,
    #[error("unsupported A2A message role or part shape")]
    UnsupportedMessage,
    #[error("A2A information correlation does not match the message context")]
    CorrelationMismatch,
    #[error("A2A SDK operation failed")]
    Sdk,
    #[error("A2A local server operation failed")]
    Server,
    #[error("A2A operation exceeded its deadline")]
    Deadline,
    #[error("A2A peer URL is not an allowed loopback HTTP URL")]
    NonLoopbackUrl,
}

impl InformationShareError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidIdentifier(_) => "a2a_invalid_identifier",
            Self::InvalidText => "a2a_invalid_text",
            Self::InvalidMetadata => "a2a_invalid_metadata",
            Self::UnsupportedMessage => "a2a_unsupported_message",
            Self::CorrelationMismatch => "a2a_correlation_mismatch",
            Self::Sdk => "a2a_sdk_error",
            Self::Server => "a2a_server_error",
            Self::Deadline => "a2a_deadline",
            Self::NonLoopbackUrl => "a2a_non_loopback_url",
        }
    }
}

#[derive(Clone, Serialize)]
struct InformationShareEnvelope<'a> {
    contract: &'static str,
    share: &'a InformationShare,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedInformationShareEnvelope {
    contract: String,
    share: InformationShare,
}

#[derive(Clone, Serialize)]
struct InformationAckEnvelope<'a> {
    contract: &'static str,
    acknowledgement: &'a InformationAcknowledgement,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedInformationAckEnvelope {
    contract: String,
    acknowledgement: InformationAcknowledgement,
}

pub fn information_share_message(
    share: &InformationShare,
) -> Result<Message, InformationShareError> {
    share.validate()?;
    let data = serde_json::to_value(InformationShareEnvelope {
        contract: INFORMATION_SHARE_CONTRACT,
        share,
    })
    .map_err(|_| InformationShareError::Sdk)?;
    let mut message = Message::new(Role::User, vec![Part::data(data)]);
    message.context_id = Some(share.correlation_id.clone());
    Ok(message)
}

pub fn parse_information_share(
    message: &Message,
) -> Result<InformationShare, InformationShareError> {
    if message.role != Role::User || message.parts.len() != 1 {
        return Err(InformationShareError::UnsupportedMessage);
    }
    let PartContent::Data(data) = &message.parts[0].content else {
        return Err(InformationShareError::UnsupportedMessage);
    };
    let envelope: OwnedInformationShareEnvelope = serde_json::from_value(data.clone())
        .map_err(|_| InformationShareError::UnsupportedMessage)?;
    if envelope.contract != INFORMATION_SHARE_CONTRACT {
        return Err(InformationShareError::UnsupportedMessage);
    }
    envelope.share.validate()?;
    if message.context_id.as_deref() != Some(envelope.share.correlation_id.as_str()) {
        return Err(InformationShareError::CorrelationMismatch);
    }
    Ok(envelope.share)
}

pub fn acknowledgement_message(
    acknowledgement: &InformationAcknowledgement,
) -> Result<Message, InformationShareError> {
    validate_identifier("correlation_id", &acknowledgement.correlation_id)?;
    validate_identifier("receiver_peer_id", &acknowledgement.receiver_peer_id)?;
    let data = serde_json::to_value(InformationAckEnvelope {
        contract: INFORMATION_ACK_CONTRACT,
        acknowledgement,
    })
    .map_err(|_| InformationShareError::Sdk)?;
    let mut message = Message::new(Role::Agent, vec![Part::data(data)]);
    message.context_id = Some(acknowledgement.correlation_id.clone());
    Ok(message)
}

pub fn parse_acknowledgement(
    message: &Message,
) -> Result<InformationAcknowledgement, InformationShareError> {
    if message.role != Role::Agent || message.parts.len() != 1 {
        return Err(InformationShareError::UnsupportedMessage);
    }
    let PartContent::Data(data) = &message.parts[0].content else {
        return Err(InformationShareError::UnsupportedMessage);
    };
    let envelope: OwnedInformationAckEnvelope = serde_json::from_value(data.clone())
        .map_err(|_| InformationShareError::UnsupportedMessage)?;
    if envelope.contract != INFORMATION_ACK_CONTRACT {
        return Err(InformationShareError::UnsupportedMessage);
    }
    validate_identifier("correlation_id", &envelope.acknowledgement.correlation_id)?;
    validate_identifier(
        "receiver_peer_id",
        &envelope.acknowledgement.receiver_peer_id,
    )?;
    if message.context_id.as_deref() != Some(envelope.acknowledgement.correlation_id.as_str()) {
        return Err(InformationShareError::CorrelationMismatch);
    }
    Ok(envelope.acknowledgement)
}

#[derive(Clone)]
struct InformationExecutor {
    receiver_peer_id: String,
    received: Arc<Mutex<VecDeque<InformationShare>>>,
}

impl AgentExecutor for InformationExecutor {
    fn execute(
        &self,
        context: ExecutorContext,
    ) -> BoxStream<'static, Result<StreamResponse, A2AError>> {
        let result = context
            .message
            .ok_or_else(|| A2AError::invalid_request("message is required"))
            .and_then(|message| {
                let share = parse_information_share(&message)
                    .map_err(|error| A2AError::invalid_request(error.code()))?;
                let acknowledgement = InformationAcknowledgement {
                    correlation_id: share.correlation_id.clone(),
                    receiver_peer_id: self.receiver_peer_id.clone(),
                    accepted: true,
                };
                let response = acknowledgement_message(&acknowledgement)
                    .map_err(|error| A2AError::internal(error.code()))?;
                let mut received = self
                    .received
                    .lock()
                    .map_err(|_| A2AError::internal("information capture lock failed"))?;
                if received.len() == MAX_CAPTURED_SHARES {
                    received.pop_front();
                }
                received.push_back(share);
                Ok(StreamResponse::Message(response))
            });
        Box::pin(stream::once(async move { result }))
    }

    fn cancel(
        &self,
        _context: ExecutorContext,
    ) -> BoxStream<'static, Result<StreamResponse, A2AError>> {
        Box::pin(stream::once(async {
            Err(A2AError::unsupported_operation(
                "information sharing has no cancellable task",
            ))
        }))
    }
}

pub struct LocalInformationServer {
    base_url: String,
    address: SocketAddr,
    received: Arc<Mutex<VecDeque<InformationShare>>>,
    shutdown_sender: Option<oneshot::Sender<()>>,
    join_handle: Option<JoinHandle<Result<(), InformationShareError>>>,
}

impl LocalInformationServer {
    pub async fn start(receiver_peer_id: impl Into<String>) -> Result<Self, InformationShareError> {
        let receiver_peer_id = receiver_peer_id.into();
        validate_identifier("receiver_peer_id", &receiver_peer_id)?;
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| InformationShareError::Server)?;
        let address = listener
            .local_addr()
            .map_err(|_| InformationShareError::Server)?;
        let base_url = format!("http://{address}");
        let received = Arc::new(Mutex::new(VecDeque::new()));
        let executor = InformationExecutor {
            receiver_peer_id: receiver_peer_id.clone(),
            received: received.clone(),
        };
        let handler = Arc::new(DefaultRequestHandler::new(
            executor,
            InMemoryTaskStore::new(),
        ));
        let card = build_agent_card(&base_url, &receiver_peer_id);
        let encoded_card = serde_json::to_vec(&card).map_err(|_| InformationShareError::Sdk)?;
        serde_json::from_slice::<AgentCard>(&encoded_card)
            .map_err(|_| InformationShareError::Sdk)?;
        let app = Router::new()
            .merge(rest_router(handler))
            .merge(agent_card_router(Arc::new(StaticAgentCard::new(card))))
            .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES));
        let (shutdown_sender, shutdown_receiver) = oneshot::channel();
        let join_handle = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_receiver.await;
                })
                .await
                .map_err(|_| InformationShareError::Server)
        });
        Ok(Self {
            base_url,
            address,
            received,
            shutdown_sender: Some(shutdown_sender),
            join_handle: Some(join_handle),
        })
    }

    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    #[must_use]
    pub const fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn received(&self) -> Result<Vec<InformationShare>, InformationShareError> {
        self.received
            .lock()
            .map(|items| items.iter().cloned().collect())
            .map_err(|_| InformationShareError::Server)
    }

    pub async fn shutdown(mut self) -> Result<(), InformationShareError> {
        if let Some(sender) = self.shutdown_sender.take() {
            let _ = sender.send(());
        }
        let Some(handle) = self.join_handle.take() else {
            return Ok(());
        };
        timeout(SHUTDOWN_DEADLINE, handle)
            .await
            .map_err(|_| InformationShareError::Deadline)?
            .map_err(|_| InformationShareError::Server)??;
        Ok(())
    }
}

impl Drop for LocalInformationServer {
    fn drop(&mut self) {
        if let Some(sender) = self.shutdown_sender.take() {
            let _ = sender.send(());
        }
        if let Some(handle) = self.join_handle.take() {
            handle.abort();
        }
    }
}

pub async fn discover_agent_card(base_url: &str) -> Result<AgentCard, InformationShareError> {
    ensure_loopback_url(base_url)?;
    timeout(
        CLIENT_DEADLINE,
        AgentCardResolver::new(None).resolve(base_url),
    )
    .await
    .map_err(|_| InformationShareError::Deadline)?
    .map_err(|_| InformationShareError::Sdk)
}

pub async fn share_information(
    base_url: &str,
    share: &InformationShare,
) -> Result<InformationAcknowledgement, InformationShareError> {
    share.validate()?;
    let card = discover_agent_card(base_url).await?;
    validate_agent_card(&card)?;
    let factory = A2AClientFactory::builder()
        .no_defaults()
        .register(Arc::new(RestTransportFactory::new(None)))
        .preferred_bindings(vec![TRANSPORT_PROTOCOL_HTTP_JSON.to_string()])
        .build();
    let client = factory
        .create_from_card(&card)
        .await
        .map_err(|_| InformationShareError::Sdk)?;
    let request = SendMessageRequest {
        message: information_share_message(share)?,
        configuration: None,
        metadata: None,
        tenant: None,
    };
    let response = timeout(CLIENT_DEADLINE, client.send_message(&request))
        .await
        .map_err(|_| InformationShareError::Deadline)?
        .map_err(|_| InformationShareError::Sdk)?;
    let SendMessageResponse::Message(message) = response else {
        return Err(InformationShareError::UnsupportedMessage);
    };
    let acknowledgement = parse_acknowledgement(&message)?;
    if acknowledgement.correlation_id != share.correlation_id || !acknowledgement.accepted {
        return Err(InformationShareError::CorrelationMismatch);
    }
    Ok(acknowledgement)
}

fn build_agent_card(base_url: &str, peer_id: &str) -> AgentCard {
    AgentCard {
        name: format!("VibeMux {peer_id}"),
        description: "Loopback-only VibeMux basic information-share peer".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        supported_interfaces: vec![AgentInterface::new(base_url, TRANSPORT_PROTOCOL_HTTP_JSON)],
        capabilities: AgentCapabilities::default(),
        default_input_modes: vec!["application/json".to_string()],
        default_output_modes: vec!["application/json".to_string()],
        skills: vec![AgentSkill {
            id: "vibemux_basic_information_share".to_string(),
            name: "Basic information share".to_string(),
            description: "Receive one bounded non-secret VibeMux information message".to_string(),
            tags: vec!["information".to_string(), "vibemux".to_string()],
            examples: None,
            input_modes: Some(vec!["application/json".to_string()]),
            output_modes: Some(vec!["application/json".to_string()]),
            security_requirements: None,
        }],
        provider: None,
        documentation_url: None,
        icon_url: None,
        security_schemes: None,
        security_requirements: None,
        signatures: None,
    }
}

fn validate_agent_card(card: &AgentCard) -> Result<(), InformationShareError> {
    if card
        .skills
        .iter()
        .all(|skill| skill.id != "vibemux_basic_information_share")
        || card.supported_interfaces.len() != 1
        || card.supported_interfaces[0].protocol_binding != TRANSPORT_PROTOCOL_HTTP_JSON
    {
        return Err(InformationShareError::UnsupportedMessage);
    }
    ensure_loopback_url(&card.supported_interfaces[0].url)
}

fn validate_identifier(field: &'static str, value: &str) -> Result<(), InformationShareError> {
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || value.contains('\0')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(InformationShareError::InvalidIdentifier(field));
    }
    Ok(())
}

fn ensure_loopback_url(url: &str) -> Result<(), InformationShareError> {
    let parsed = url::Url::parse(url).map_err(|_| InformationShareError::NonLoopbackUrl)?;
    let allowed_host = matches!(parsed.host_str(), Some("127.0.0.1" | "localhost"));
    let allowed_path = parsed.path().is_empty() || parsed.path() == "/";
    if parsed.scheme() != "http"
        || !allowed_host
        || parsed.port().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !allowed_path
    {
        return Err(InformationShareError::NonLoopbackUrl);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_share() -> InformationShare {
        InformationShare {
            correlation_id: "correlation_1".to_string(),
            sender_peer_id: "sender_1".to_string(),
            information_kind: "status".to_string(),
            text: "VIBEMUX_A2A_INFO_SHARE_OK".to_string(),
            metadata: BTreeMap::from([("scope".to_string(), "basic".to_string())]),
        }
    }

    #[test]
    fn share_round_trips_through_official_message() {
        let share = valid_share();
        let message = information_share_message(&share).expect("valid share message");
        assert_eq!(
            parse_information_share(&message).expect("parse share"),
            share
        );
    }

    #[test]
    fn oversized_share_fails_closed() {
        let mut share = valid_share();
        share.text = "x".repeat(MAX_INFORMATION_TEXT_BYTES + 1);
        assert!(matches!(
            share.validate(),
            Err(InformationShareError::InvalidText)
        ));
    }

    #[test]
    fn correlation_mismatch_is_rejected() {
        let share = valid_share();
        let mut message = information_share_message(&share).expect("valid share message");
        message.context_id = Some("different".to_string());
        assert!(matches!(
            parse_information_share(&message),
            Err(InformationShareError::CorrelationMismatch)
        ));
    }

    #[test]
    fn non_loopback_discovery_is_rejected() {
        assert!(matches!(
            ensure_loopback_url("https://example.com"),
            Err(InformationShareError::NonLoopbackUrl)
        ));
        assert!(matches!(
            ensure_loopback_url("http://127.0.0.1:80@evil.example:8080"),
            Err(InformationShareError::NonLoopbackUrl)
        ));
    }

    #[test]
    fn selected_sdk_matches_recorded_a2a_service_version() {
        assert_eq!(a2a::VERSION, A2A_SPEC_VERSION);
    }
}
