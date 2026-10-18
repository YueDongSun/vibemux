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
    use serde_json::{Value, json};

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

    const UNSUPPORTED: &str = "a2a_unsupported_message";
    const MISMATCH: &str = "a2a_correlation_mismatch";

    /// A change to a valid share and the outcome it should have.
    type ShareCase = (fn(&mut InformationShare), &'static str);
    /// A change to a valid agent card and the outcome it should have.
    type CardCase = (fn(&mut AgentCard), &'static str);

    /// "ok", or the error code of a refusal, with the field of an identifier error.
    fn outcome<T>(result: Result<T, InformationShareError>) -> String {
        match result {
            Ok(_) => "ok".to_string(),
            Err(InformationShareError::InvalidIdentifier(field)) => {
                format!("a2a_invalid_identifier:{field}")
            }
            Err(error) => error.code().to_string(),
        }
    }

    fn data_message(role: Role, context_id: &str, data: Value) -> Message {
        let mut message = Message::new(role, vec![Part::data(data)]);
        message.context_id = Some(context_id.to_string());
        message
    }

    fn the_only_data_part(message: &Message) -> &Value {
        let [part] = message.parts.as_slice() else {
            panic!("expected one part: {message:?}");
        };
        let PartContent::Data(data) = &part.content else {
            panic!("expected a data part: {message:?}");
        };
        data
    }

    fn numbered_metadata(count: usize) -> BTreeMap<String, String> {
        (0..count)
            .map(|index| (format!("key_{index}"), "value".to_string()))
            .collect()
    }

    fn valid_acknowledgement() -> InformationAcknowledgement {
        InformationAcknowledgement {
            correlation_id: "correlation_1".to_string(),
            receiver_peer_id: "receiver_1".to_string(),
            accepted: true,
        }
    }

    #[test]
    fn a_share_bounds_its_identifiers_text_and_metadata() {
        let invalid_correlation = "a2a_invalid_identifier:correlation_id";
        let cases: [ShareCase; 22] = [
            (|_| {}, "ok"),
            (|share| share.correlation_id = "A-z_0.9".to_string(), "ok"),
            (|share| share.correlation_id = "c".repeat(128), "ok"),
            (
                |share| share.correlation_id = "c".repeat(129),
                invalid_correlation,
            ),
            (
                |share| share.correlation_id = String::new(),
                invalid_correlation,
            ),
            (
                |share| share.sender_peer_id = "sender 1".to_string(),
                "a2a_invalid_identifier:sender_peer_id",
            ),
            (
                |share| share.information_kind = "kind\0".to_string(),
                "a2a_invalid_identifier:information_kind",
            ),
            (
                |share| share.information_kind = "kind/1".to_string(),
                "a2a_invalid_identifier:information_kind",
            ),
            (|share| share.text = "x".to_string(), "ok"),
            (
                |share| share.text = "x".repeat(MAX_INFORMATION_TEXT_BYTES),
                "ok",
            ),
            (|share| share.text = String::new(), "a2a_invalid_text"),
            (
                |share| share.text = "x".repeat(MAX_INFORMATION_TEXT_BYTES + 1),
                "a2a_invalid_text",
            ),
            (|share| share.metadata.clear(), "ok"),
            (
                |share| share.metadata = numbered_metadata(MAX_METADATA_ENTRIES),
                "ok",
            ),
            (
                |share| share.metadata = numbered_metadata(MAX_METADATA_ENTRIES + 1),
                "a2a_invalid_metadata",
            ),
            (
                |share| {
                    share.metadata.insert(
                        "k".repeat(MAX_METADATA_KEY_BYTES),
                        "v".repeat(MAX_METADATA_VALUE_BYTES),
                    );
                },
                "ok",
            ),
            (
                |share| {
                    share.metadata.insert("note".to_string(), String::new());
                },
                "ok",
            ),
            (
                |share| {
                    share.metadata.insert(String::new(), "v".to_string());
                },
                "a2a_invalid_metadata",
            ),
            (
                |share| {
                    share
                        .metadata
                        .insert("k".repeat(MAX_METADATA_KEY_BYTES + 1), "v".to_string());
                },
                "a2a_invalid_metadata",
            ),
            (
                |share| {
                    share
                        .metadata
                        .insert("note".to_string(), "v".repeat(MAX_METADATA_VALUE_BYTES + 1));
                },
                "a2a_invalid_metadata",
            ),
            (
                |share| {
                    share.metadata.insert("no\0te".to_string(), "v".to_string());
                },
                "a2a_invalid_metadata",
            ),
            (
                |share| {
                    share.metadata.insert("note".to_string(), "v\0".to_string());
                },
                "a2a_invalid_metadata",
            ),
        ];
        for (index, (change, expected)) in cases.into_iter().enumerate() {
            let mut share = valid_share();
            change(&mut share);
            assert_eq!(outcome(share.validate()), expected, "case {index}");
        }
    }

    #[test]
    fn a_share_message_is_one_data_part_from_the_user() {
        let share = valid_share();
        let message = information_share_message(&share).expect("share message");
        assert_eq!(message.role, Role::User);
        assert_eq!(message.context_id.as_deref(), Some("correlation_1"));
        let share_data = || json!({"contract": "vibemux.info.share.v1", "share": valid_share()});
        assert_eq!(the_only_data_part(&message), &share_data());
        let empty_text = InformationShare {
            text: String::new(),
            ..valid_share()
        };
        assert_eq!(
            outcome(information_share_message(&empty_text)),
            "a2a_invalid_text"
        );

        let parse = |message: &Message| outcome(parse_information_share(message));
        let mut without_metadata = share_data();
        without_metadata["share"]
            .as_object_mut()
            .expect("share object")
            .remove("metadata");
        let parsed =
            parse_information_share(&data_message(Role::User, "correlation_1", without_metadata))
                .expect("share without metadata");
        assert!(parsed.metadata.is_empty());

        let mut two_parts = data_message(Role::User, "correlation_1", share_data());
        two_parts.parts.push(Part::data(share_data()));
        let mut text_part = data_message(Role::User, "correlation_1", share_data());
        text_part.parts = vec![Part::text("VIBEMUX_A2A_INFO_SHARE_OK")];
        let mut no_context = data_message(Role::User, "correlation_1", share_data());
        no_context.context_id = None;
        let mut unknown_field = share_data();
        unknown_field["extra"] = json!(1);
        let mut unknown_share_field = share_data();
        unknown_share_field["share"]["extra"] = json!(1);
        let mut invalid_text = share_data();
        invalid_text["share"]["text"] = json!("");
        let ack_contract = json!({"contract": "vibemux.info.ack.v1", "share": valid_share()});
        for (message, expected) in [
            (
                data_message(Role::Agent, "correlation_1", share_data()),
                UNSUPPORTED,
            ),
            (two_parts, UNSUPPORTED),
            (text_part, UNSUPPORTED),
            (
                data_message(Role::User, "correlation_1", ack_contract),
                UNSUPPORTED,
            ),
            (
                data_message(Role::User, "correlation_1", unknown_field),
                UNSUPPORTED,
            ),
            (
                data_message(Role::User, "correlation_1", unknown_share_field),
                UNSUPPORTED,
            ),
            (
                data_message(Role::User, "correlation_2", invalid_text),
                "a2a_invalid_text",
            ),
            (
                data_message(Role::User, "correlation_2", share_data()),
                MISMATCH,
            ),
            (no_context, MISMATCH),
        ] {
            assert_eq!(parse(&message), expected, "{message:?}");
        }
    }

    #[test]
    fn an_acknowledgement_message_is_one_data_part_from_the_agent() {
        for accepted in [true, false] {
            let acknowledgement = InformationAcknowledgement {
                accepted,
                ..valid_acknowledgement()
            };
            let message = acknowledgement_message(&acknowledgement).expect("ack message");
            assert_eq!(message.role, Role::Agent);
            assert_eq!(message.context_id.as_deref(), Some("correlation_1"));
            assert_eq!(
                the_only_data_part(&message),
                &json!({"contract": "vibemux.info.ack.v1", "acknowledgement": acknowledgement})
            );
            assert_eq!(
                parse_acknowledgement(&message).expect("parse ack"),
                acknowledgement
            );
        }
        let build = |change: fn(&mut InformationAcknowledgement)| {
            let mut acknowledgement = valid_acknowledgement();
            change(&mut acknowledgement);
            outcome(acknowledgement_message(&acknowledgement))
        };
        assert_eq!(
            build(|ack| ack.correlation_id = "correlation 1".to_string()),
            "a2a_invalid_identifier:correlation_id"
        );
        assert_eq!(
            build(|ack| ack.receiver_peer_id = String::new()),
            "a2a_invalid_identifier:receiver_peer_id"
        );

        let parse = |message: &Message| outcome(parse_acknowledgement(message));
        let ack_data = || json!({"contract": "vibemux.info.ack.v1", "acknowledgement": valid_acknowledgement()});
        let mut two_parts = data_message(Role::Agent, "correlation_1", ack_data());
        two_parts.parts.push(Part::data(ack_data()));
        let mut text_part = data_message(Role::Agent, "correlation_1", ack_data());
        text_part.parts = vec![Part::text("accepted")];
        let mut no_context = data_message(Role::Agent, "correlation_1", ack_data());
        no_context.context_id = None;
        let mut unknown_field = ack_data();
        unknown_field["extra"] = json!(1);
        let mut unknown_ack_field = ack_data();
        unknown_ack_field["acknowledgement"]["extra"] = json!(1);
        let mut invalid_correlation = ack_data();
        invalid_correlation["acknowledgement"]["correlation_id"] = json!("");
        let mut invalid_receiver = ack_data();
        invalid_receiver["acknowledgement"]["receiver_peer_id"] = json!("receiver 1");
        let share_contract = json!({
            "contract": "vibemux.info.share.v1",
            "acknowledgement": valid_acknowledgement()
        });
        for (message, expected) in [
            (
                data_message(Role::User, "correlation_1", ack_data()),
                UNSUPPORTED,
            ),
            (two_parts, UNSUPPORTED),
            (text_part, UNSUPPORTED),
            (
                data_message(Role::Agent, "correlation_1", share_contract),
                UNSUPPORTED,
            ),
            (
                data_message(Role::Agent, "correlation_1", unknown_field),
                UNSUPPORTED,
            ),
            (
                data_message(Role::Agent, "correlation_1", unknown_ack_field),
                UNSUPPORTED,
            ),
            (
                data_message(Role::Agent, "correlation_1", invalid_correlation),
                "a2a_invalid_identifier:correlation_id",
            ),
            (
                data_message(Role::Agent, "correlation_1", invalid_receiver),
                "a2a_invalid_identifier:receiver_peer_id",
            ),
            (
                data_message(Role::Agent, "correlation_2", ack_data()),
                MISMATCH,
            ),
            (no_context, MISMATCH),
        ] {
            assert_eq!(parse(&message), expected, "{message:?}");
        }
        // A share message is never read as an acknowledgement.
        let share_message = information_share_message(&valid_share()).expect("share message");
        assert_eq!(parse(&share_message), UNSUPPORTED);
    }

    #[test]
    fn only_a_loopback_card_offering_the_share_skill_is_accepted() {
        let card = build_agent_card("http://127.0.0.1:4100/", "peer_1");
        assert_eq!(card.name, "VibeMux peer_1");
        assert_eq!(
            card.skills
                .iter()
                .map(|skill| skill.id.as_str())
                .collect::<Vec<_>>(),
            ["vibemux_basic_information_share"]
        );
        assert_eq!(card.supported_interfaces.len(), 1);
        assert_eq!(
            (
                card.supported_interfaces[0].url.as_str(),
                card.supported_interfaces[0].protocol_binding.as_str()
            ),
            ("http://127.0.0.1:4100/", "HTTP+JSON")
        );
        assert_eq!(outcome(validate_agent_card(&card)), "ok");

        let cases: [CardCase; 7] = [
            (
                |card| {
                    let mut other = card.skills[0].clone();
                    other.id = "other_skill".to_string();
                    card.skills.insert(0, other);
                },
                "ok",
            ),
            (|card| card.skills.clear(), UNSUPPORTED),
            (
                |card| card.skills[0].id = "other_skill".to_string(),
                UNSUPPORTED,
            ),
            (|card| card.supported_interfaces.clear(), UNSUPPORTED),
            (
                |card| {
                    let copy = card.supported_interfaces[0].clone();
                    card.supported_interfaces.push(copy);
                },
                UNSUPPORTED,
            ),
            (
                |card| card.supported_interfaces[0].protocol_binding = "JSONRPC".to_string(),
                UNSUPPORTED,
            ),
            (
                |card| card.supported_interfaces[0].url = "http://192.168.1.2:4100/".to_string(),
                "a2a_non_loopback_url",
            ),
        ];
        for (index, (change, expected)) in cases.into_iter().enumerate() {
            let mut changed = card.clone();
            change(&mut changed);
            assert_eq!(
                outcome(validate_agent_card(&changed)),
                expected,
                "case {index}"
            );
        }
    }

    #[test]
    fn a_peer_url_is_plain_http_on_loopback_with_an_explicit_port() {
        for url in [
            "http://127.0.0.1:4100",
            "http://127.0.0.1:4100/",
            "http://localhost:4100/",
            "http://LOCALHOST:4100",
        ] {
            assert_eq!(outcome(ensure_loopback_url(url)), "ok", "{url}");
        }
        // Port 80 is the http default, which the parser reports as no port.
        for url in [
            "",
            "not a url",
            "https://127.0.0.1:4100/",
            "http://127.0.0.1/",
            "http://127.0.0.1:80/",
            "http://127.0.0.2:4100/",
            "http://[::1]:4100/",
            "http://example.test:4100/",
            "http://user@127.0.0.1:4100/",
            "http://:secret@127.0.0.1:4100/",
            "http://127.0.0.1:4100/?a=1",
            "http://127.0.0.1:4100/#a",
            "http://127.0.0.1:4100/a2a",
        ] {
            assert_eq!(
                outcome(ensure_loopback_url(url)),
                "a2a_non_loopback_url",
                "{url}"
            );
        }
    }

    #[test]
    fn information_errors_keep_their_codes() {
        for (error, code) in [
            (
                InformationShareError::InvalidIdentifier("correlation_id"),
                "a2a_invalid_identifier",
            ),
            (InformationShareError::InvalidText, "a2a_invalid_text"),
            (
                InformationShareError::InvalidMetadata,
                "a2a_invalid_metadata",
            ),
            (InformationShareError::UnsupportedMessage, UNSUPPORTED),
            (InformationShareError::CorrelationMismatch, MISMATCH),
            (InformationShareError::Sdk, "a2a_sdk_error"),
            (InformationShareError::Server, "a2a_server_error"),
            (InformationShareError::Deadline, "a2a_deadline"),
            (
                InformationShareError::NonLoopbackUrl,
                "a2a_non_loopback_url",
            ),
        ] {
            assert_eq!(error.code(), code);
        }
    }
}
