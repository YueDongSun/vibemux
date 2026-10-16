//! Bounded loopback AAG transport for workflow model calls (ADR 031 §3).
//!
//! This is an effect boundary, not a route attestation service. A health/model
//! snapshot is an observation only. Until AAG exposes request-scoped upstream
//! evidence, live calls fail closed. Fixture calls exercise the same wire path
//! against a local fake server without contacting a model.

#![allow(non_camel_case_types)]

use std::{fmt, future::Future, net::SocketAddr, time::Duration};

#[cfg(feature = "test_helpers")]
use std::sync::Arc;

use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
#[cfg(feature = "test_helpers")]
use tokio::net::TcpListener;
use tokio::{sync::watch, time::Instant};
use vibemux_workflow::{
    Sha256Digest,
    gateway::{
        CORRELATION_HEADER, CallCorrelation, CallOutcome, FailureStage, GatewayConfig,
        GatewayHealth, GatewayProtocol, ModelCallReceipt, ParsedCompletion, ProviderAttribution,
        ReportedUsage, ResponseError, RetryDecision, RouteRole, StreamAccumulator, StreamEnd,
        evaluate_route, parse_completion, request_body,
    },
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const OBSERVATION_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_OBSERVATION_BYTES: usize = 64 * 1024;
const MAX_REQUEST_BYTES: usize = 256 * 1024;
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_SSE_EVENT_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Aag_Call_Class {
    Fixture,
    Live,
}

/// A nonserializable proof that a test harness owns the exact bound listener.
/// Normal builds cannot construct one. Its retained handle keeps the address
/// reserved for the fake server throughout the call.
#[derive(Clone)]
pub struct Aag_Fixture_Permit {
    address: SocketAddr,
    #[cfg(feature = "test_helpers")]
    _listener: Arc<TcpListener>,
}

impl fmt::Debug for Aag_Fixture_Permit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Aag_Fixture_Permit([owned loopback listener])")
    }
}

#[cfg(feature = "test_helpers")]
impl Aag_Fixture_Permit {
    pub fn from_owned_listener(listener: Arc<TcpListener>) -> Result<Self, Aag_Gateway_Error> {
        let address = listener
            .local_addr()
            .map_err(|_| Aag_Gateway_Error::FixturePermitInvalid)?;
        if !address.ip().is_loopback() || address.port() == 0 {
            return Err(Aag_Gateway_Error::FixturePermitInvalid);
        }
        Ok(Self {
            address,
            _listener: listener,
        })
    }
}

/// The token is never part of a status, receipt, or Debug value.
pub struct Aag_Credential(String);

impl fmt::Debug for Aag_Credential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Aag_Credential([redacted])")
    }
}

impl Aag_Credential {
    pub fn explicit(token: String) -> Result<Self, Aag_Gateway_Error> {
        if token.is_empty()
            || token.len() > 4096
            || !token.is_ascii()
            || token.bytes().any(|byte| byte.is_ascii_control())
        {
            return Err(Aag_Gateway_Error::CredentialInvalid);
        }
        Ok(Self(token))
    }

    /// Resolve a validated config's named reference only at this effect edge.
    pub fn named(config: &GatewayConfig) -> Result<Self, Aag_Gateway_Error> {
        config
            .validate()
            .map_err(|_| Aag_Gateway_Error::ConfigInvalid)?;
        let token = std::env::var(&config.credential_env)
            .map_err(|_| Aag_Gateway_Error::CredentialUnavailable)?;
        Self::explicit(token)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Aag_Gateway_Error {
    ConfigInvalid,
    CredentialInvalid,
    CredentialUnavailable,
    OriginNotNumericLoopback,
    LiveAttestationUnavailable,
    FixturePermitRequired,
    FixturePermitInvalid,
    FixturePermitMismatch,
    ConfigChanged,
    RouteChanged,
    RouteUnavailable,
    UnsupportedProtocol,
    RequestTooLarge,
    ObservationUnavailable,
    HttpStatus,
    ConnectFailed,
    TransportAmbiguous,
    Deadline,
    Cancelled,
    ResponseTooLarge,
    ResponseMalformed,
    ResponseEmpty,
}

impl Aag_Gateway_Error {
    pub const fn code(self) -> &'static str {
        match self {
            Self::ConfigInvalid => "gateway_config_invalid",
            Self::CredentialInvalid => "gateway_credential_invalid",
            Self::CredentialUnavailable => "gateway_credential_unavailable",
            Self::OriginNotNumericLoopback => "gateway_origin_not_numeric_loopback",
            Self::LiveAttestationUnavailable => "gateway_live_attestation_unavailable",
            Self::FixturePermitRequired => "gateway_fixture_permit_required",
            Self::FixturePermitInvalid => "gateway_fixture_permit_invalid",
            Self::FixturePermitMismatch => "gateway_fixture_permit_mismatch",
            Self::ConfigChanged => "gateway_config_changed",
            Self::RouteChanged => "gateway_route_changed",
            Self::RouteUnavailable => "gateway_route_unavailable",
            Self::UnsupportedProtocol => "gateway_protocol_unsupported",
            Self::RequestTooLarge => "gateway_request_too_large",
            Self::ObservationUnavailable => "gateway_observation_unavailable",
            Self::HttpStatus => "gateway_http_status",
            Self::ConnectFailed => "gateway_connect_failed",
            Self::TransportAmbiguous => "gateway_transport_ambiguous",
            Self::Deadline => "gateway_deadline",
            Self::Cancelled => "gateway_cancelled",
            Self::ResponseTooLarge => "gateway_response_too_large",
            Self::ResponseMalformed => "gateway_response_malformed",
            Self::ResponseEmpty => "gateway_response_empty",
        }
    }
}

#[derive(Clone)]
pub struct Aag_Call_Request {
    pub class: Aag_Call_Class,
    pub role: RouteRole,
    pub correlation: CallCorrelation,
    pub expected_config_digest: Sha256Digest,
    pub expected_route_generation: Option<Sha256Digest>,
    pub system: String,
    pub user: String,
    pub stream: bool,
}

impl fmt::Debug for Aag_Call_Request {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Aag_Call_Request")
            .field("class", &self.class)
            .field("role", &self.role)
            .field("correlation", &self.correlation)
            .field("system_bytes", &self.system.len())
            .field("user_bytes", &self.user.len())
            .field("stream", &self.stream)
            .finish()
    }
}

/// The caller may persist the receipt only when present; a blocked route has
/// no invented requested alias or effective provider.
pub struct Aag_Call_Result {
    pub outcome: CallOutcome,
    pub observation: Option<GatewayHealth>,
    pub completion: Option<ParsedCompletion>,
    pub receipt: Option<ModelCallReceipt>,
    pub error: Option<Aag_Gateway_Error>,
    pub retry: RetryDecision,
}

impl fmt::Debug for Aag_Call_Result {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Aag_Call_Result")
            .field("outcome", &self.outcome)
            .field("has_observation", &self.observation.is_some())
            .field("has_completion", &self.completion.is_some())
            .field("has_receipt", &self.receipt.is_some())
            .field("error", &self.error)
            .field("retry", &self.retry)
            .finish()
    }
}

impl Aag_Call_Result {
    fn blocked(error: Aag_Gateway_Error, observation: Option<GatewayHealth>) -> Self {
        Self {
            outcome: CallOutcome::Blocked,
            observation,
            completion: None,
            receipt: None,
            error: Some(error),
            retry: RetryDecision::NoAutomaticRetry("blocked"),
        }
    }
}

/// An immutable route configuration and a client with no ambient proxy,
/// redirect, cookie store, or non-loopback destination.
pub struct Aag_Model_Gateway {
    client: Client,
    config: GatewayConfig,
    config_digest: Sha256Digest,
    credential: Aag_Credential,
    origin: String,
    origin_address: SocketAddr,
    fixture_permit: Option<Aag_Fixture_Permit>,
}

impl fmt::Debug for Aag_Model_Gateway {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Aag_Model_Gateway")
            .field("configured", &true)
            .finish_non_exhaustive()
    }
}

impl Aag_Model_Gateway {
    pub fn new(
        config: GatewayConfig,
        config_digest: Sha256Digest,
        credential: Aag_Credential,
    ) -> Result<Self, Aag_Gateway_Error> {
        config
            .validate()
            .map_err(|_| Aag_Gateway_Error::ConfigInvalid)?;
        let (origin, origin_address) = numeric_loopback_origin(&config.endpoint)?;
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .pool_max_idle_per_host(0)
            .build()
            .map_err(|_| Aag_Gateway_Error::ConfigInvalid)?;
        Ok(Self {
            client,
            config,
            config_digest,
            credential,
            origin,
            origin_address,
            fixture_permit: None,
        })
    }

    #[cfg(feature = "test_helpers")]
    pub fn with_fixture_permit(
        mut self,
        permit: Aag_Fixture_Permit,
    ) -> Result<Self, Aag_Gateway_Error> {
        if permit.address != self.origin_address {
            return Err(Aag_Gateway_Error::FixturePermitMismatch);
        }
        self.fixture_permit = Some(permit);
        Ok(self)
    }

    /// Checks caller identity and the configured role binding without
    /// network access or credential lookup. AAG health and route generation
    /// remain observations performed only by `call`.
    pub fn validate_request(
        &self,
        class: Aag_Call_Class,
        expected_config_digest: Sha256Digest,
        role: RouteRole,
    ) -> Result<&vibemux_workflow::gateway::RouteBinding, Aag_Gateway_Error> {
        if class == Aag_Call_Class::Live {
            return Err(Aag_Gateway_Error::LiveAttestationUnavailable);
        }
        match self.fixture_permit.as_ref() {
            None => return Err(Aag_Gateway_Error::FixturePermitRequired),
            Some(permit) if permit.address != self.origin_address => {
                return Err(Aag_Gateway_Error::FixturePermitMismatch);
            }
            Some(_) => {}
        }
        if expected_config_digest != self.config_digest {
            return Err(Aag_Gateway_Error::ConfigChanged);
        }
        let binding = self
            .config
            .binding(role)
            .ok_or(Aag_Gateway_Error::RouteUnavailable)?;
        if !binding.protocol.adapter_supported() {
            return Err(Aag_Gateway_Error::UnsupportedProtocol);
        }
        Ok(binding)
    }

    /// One recorded attempt. This method never retries or joins partial text.
    pub async fn call(
        &self,
        request: Aag_Call_Request,
        mut cancel: watch::Receiver<bool>,
    ) -> Aag_Call_Result {
        let binding = match self.validate_request(
            request.class,
            request.expected_config_digest,
            request.role,
        ) {
            Ok(binding) => binding,
            Err(error) => return Aag_Call_Result::blocked(error, None),
        };
        if *cancel.borrow() {
            return Aag_Call_Result::blocked(Aag_Gateway_Error::Cancelled, None);
        }
        if request.system.len().saturating_add(request.user.len()) > MAX_REQUEST_BYTES {
            return Aag_Call_Result::blocked(Aag_Gateway_Error::RequestTooLarge, None);
        }
        let started = Instant::now();
        if !binding.protocol.adapter_supported()
            || (request.stream && binding.protocol != GatewayProtocol::OpenaiChatCompletions)
        {
            return Aag_Call_Result::blocked(Aag_Gateway_Error::UnsupportedProtocol, None);
        }
        let deadline = started + Duration::from_millis(binding.timeout_ms);
        let observation = match self.observe(deadline, &mut cancel).await {
            Ok(observation) => observation,
            Err(error) => return Aag_Call_Result::blocked(error, None),
        };
        if request
            .expected_route_generation
            .is_some_and(|expected| expected != observation.route_generation)
        {
            return Aag_Call_Result::blocked(Aag_Gateway_Error::RouteChanged, Some(observation));
        }
        if evaluate_route(&self.config, &observation, request.role).is_err() {
            return Aag_Call_Result::blocked(
                Aag_Gateway_Error::RouteUnavailable,
                Some(observation),
            );
        }
        let mut body = request_body(binding, &request.system, &request.user);
        if request.stream {
            body["stream"] = json!(true);
        }
        let encoded = match serde_json::to_vec(&body) {
            Ok(encoded) if encoded.len() <= MAX_REQUEST_BYTES => encoded,
            _ => {
                return Aag_Call_Result::blocked(
                    Aag_Gateway_Error::RequestTooLarge,
                    Some(observation),
                );
            }
        };
        let request_digest = Sha256Digest::of(&encoded);
        let protocol = binding.protocol;
        let mut http_request = self
            .client
            .post(format!("{}{}", self.origin, protocol.path()))
            .header(CORRELATION_HEADER, request.correlation.header_value())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(encoded);
        http_request = match protocol {
            GatewayProtocol::OpenaiChatCompletions => http_request.bearer_auth(&self.credential.0),
            GatewayProtocol::AnthropicMessages => http_request
                .header("x-api-key", &self.credential.0)
                .header("anthropic-version", "2023-06-01"),
            GatewayProtocol::OpenaiResponses => unreachable!("checked above"),
        };
        let sent = bounded(http_request.send(), deadline, &mut cancel).await;
        let mut response = match sent {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => {
                let proven_connect = error.is_connect() && !error.is_timeout();
                return self.failed(
                    &request,
                    &observation,
                    request_digest,
                    started,
                    None,
                    None,
                    if proven_connect {
                        Aag_Gateway_Error::ConnectFailed
                    } else {
                        Aag_Gateway_Error::TransportAmbiguous
                    },
                    if proven_connect {
                        FailureStage::Connect
                    } else {
                        FailureStage::BrokenAfterOutput
                    },
                    0,
                    false,
                    proven_connect,
                );
            }
            Err(error) => {
                return self.failed(
                    &request,
                    &observation,
                    request_digest,
                    started,
                    None,
                    None,
                    error,
                    FailureStage::Timeout,
                    0,
                    false,
                    false,
                );
            }
        };
        let status = response.status();
        if !status.is_success() {
            return self.failed(
                &request,
                &observation,
                request_digest,
                started,
                Some(status.as_u16()),
                None,
                Aag_Gateway_Error::HttpStatus,
                FailureStage::Status(status.as_u16()),
                0,
                false,
                false,
            );
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return self.failed(
                &request,
                &observation,
                request_digest,
                started,
                Some(status.as_u16()),
                None,
                Aag_Gateway_Error::ResponseTooLarge,
                FailureStage::BrokenAfterOutput,
                0,
                false,
                false,
            );
        }
        let is_sse = request.stream;
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        let content_type = content_type.to_ascii_lowercase();
        if (is_sse && !content_type.starts_with("text/event-stream"))
            || (!is_sse && !content_type.starts_with("application/json"))
        {
            return self.failed(
                &request,
                &observation,
                request_digest,
                started,
                Some(status.as_u16()),
                None,
                Aag_Gateway_Error::ResponseMalformed,
                FailureStage::BrokenAfterOutput,
                0,
                false,
                false,
            );
        }
        let mut raw = Vec::new();
        let mut stream = StreamAccumulator::default();
        let mut pending = Vec::new();
        loop {
            let next = bounded(response.chunk(), deadline, &mut cancel).await;
            let chunk = match next {
                Ok(Ok(Some(chunk))) => chunk,
                Ok(Ok(None)) => break,
                Ok(Err(_)) => {
                    return self.body_failure(
                        &request,
                        &observation,
                        request_digest,
                        started,
                        status,
                        &raw,
                        &stream,
                        Aag_Gateway_Error::TransportAmbiguous,
                    );
                }
                Err(error) => {
                    return self.body_failure(
                        &request,
                        &observation,
                        request_digest,
                        started,
                        status,
                        &raw,
                        &stream,
                        error,
                    );
                }
            };
            if raw.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return self.body_failure(
                    &request,
                    &observation,
                    request_digest,
                    started,
                    status,
                    &raw,
                    &stream,
                    Aag_Gateway_Error::ResponseTooLarge,
                );
            }
            raw.extend_from_slice(&chunk);
            if is_sse {
                pending.extend_from_slice(&chunk);
                while let Some((end, delimiter_len)) = sse_event_end(&pending) {
                    if end + delimiter_len > MAX_SSE_EVENT_BYTES {
                        return self.body_failure(
                            &request,
                            &observation,
                            request_digest,
                            started,
                            status,
                            &raw,
                            &stream,
                            Aag_Gateway_Error::ResponseTooLarge,
                        );
                    }
                    let event = pending.drain(..end + delimiter_len).collect::<Vec<u8>>();
                    if accept_sse_event(&mut stream, &event).is_err() {
                        return self.body_failure(
                            &request,
                            &observation,
                            request_digest,
                            started,
                            status,
                            &raw,
                            &stream,
                            Aag_Gateway_Error::ResponseMalformed,
                        );
                    }
                }
                if pending.len() > MAX_SSE_EVENT_BYTES {
                    return self.body_failure(
                        &request,
                        &observation,
                        request_digest,
                        started,
                        status,
                        &raw,
                        &stream,
                        Aag_Gateway_Error::ResponseTooLarge,
                    );
                }
            }
        }
        let completion = if is_sse {
            if !pending.is_empty() {
                return self.body_failure(
                    &request,
                    &observation,
                    request_digest,
                    started,
                    status,
                    &raw,
                    &stream,
                    Aag_Gateway_Error::ResponseMalformed,
                );
            }
            match stream.clone().finish(false) {
                StreamEnd::Completed(completion)
                    if !completion.text.trim().is_empty() || completion.tool_call_requested =>
                {
                    Ok(completion)
                }
                StreamEnd::Completed(_) => Err(Aag_Gateway_Error::ResponseEmpty),
                StreamEnd::FailedBeforeOutput | StreamEnd::FailedAfterOutput { .. } => {
                    Err(Aag_Gateway_Error::ResponseMalformed)
                }
            }
        } else {
            parse_completion(protocol, &raw).map_err(map_response_error)
        };
        match completion {
            Ok(completion) => {
                let receipt = self.receipt(
                    &request,
                    &observation,
                    request_digest,
                    started,
                    Some(status.as_u16()),
                    Some(Sha256Digest::of(&raw)),
                    CallOutcome::Completed,
                    None,
                    &completion,
                );
                Aag_Call_Result {
                    outcome: CallOutcome::Completed,
                    observation: Some(observation),
                    completion: Some(completion),
                    receipt,
                    error: None,
                    retry: RetryDecision::NoAutomaticRetry("completed"),
                }
            }
            Err(error) => self.body_failure(
                &request,
                &observation,
                request_digest,
                started,
                status,
                &raw,
                &stream,
                error,
            ),
        }
    }

    async fn observe(
        &self,
        deadline: Instant,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<GatewayHealth, Aag_Gateway_Error> {
        let observation_deadline = std::cmp::min(deadline, Instant::now() + OBSERVATION_TIMEOUT);
        let health = self
            .fetch_json("/health", observation_deadline, cancel)
            .await?;
        let models = self
            .fetch_json("/v1/models", observation_deadline, cancel)
            .await?;
        GatewayHealth::from_responses(&health, &models)
            .map_err(|_| Aag_Gateway_Error::ObservationUnavailable)
    }

    async fn fetch_json(
        &self,
        path: &str,
        deadline: Instant,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<Value, Aag_Gateway_Error> {
        let sent = bounded(
            self.client
                .get(format!("{}{}", self.origin, path))
                .bearer_auth(&self.credential.0)
                .send(),
            deadline,
            cancel,
        )
        .await?
        .map_err(|_| Aag_Gateway_Error::ObservationUnavailable)?;
        if sent.status() != StatusCode::OK {
            return Err(Aag_Gateway_Error::ObservationUnavailable);
        }
        let mut response = sent;
        if response
            .content_length()
            .is_some_and(|length| length > MAX_OBSERVATION_BYTES as u64)
        {
            return Err(Aag_Gateway_Error::ObservationUnavailable);
        }
        let mut bytes = Vec::new();
        loop {
            let chunk = bounded(response.chunk(), deadline, cancel)
                .await?
                .map_err(|_| Aag_Gateway_Error::ObservationUnavailable)?;
            let Some(chunk) = chunk else { break };
            if bytes.len().saturating_add(chunk.len()) > MAX_OBSERVATION_BYTES {
                return Err(Aag_Gateway_Error::ObservationUnavailable);
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| Aag_Gateway_Error::ObservationUnavailable)
    }

    #[allow(clippy::too_many_arguments)]
    fn failed(
        &self,
        request: &Aag_Call_Request,
        observation: &GatewayHealth,
        request_digest: Sha256Digest,
        started: Instant,
        http_status: Option<u16>,
        response_digest: Option<Sha256Digest>,
        error: Aag_Gateway_Error,
        stage: FailureStage,
        observed_bytes: u64,
        tool_effect_possible: bool,
        proven_connect: bool,
    ) -> Aag_Call_Result {
        let outcome = if observed_bytes > 0 || tool_effect_possible {
            CallOutcome::FailedAfterOutput
        } else {
            CallOutcome::FailedBeforeOutput
        };
        let empty = ParsedCompletion {
            text: String::new(),
            reported_model: None,
            usage: ReportedUsage::default(),
            finish_reason: None,
            tool_call_requested: tool_effect_possible,
        };
        let receipt = self.receipt(
            request,
            observation,
            request_digest,
            started,
            http_status,
            response_digest,
            outcome,
            Some(stage),
            &empty,
        );
        Aag_Call_Result {
            outcome,
            observation: Some(observation.clone()),
            completion: None,
            receipt,
            error: Some(error),
            retry: if proven_connect {
                RetryDecision::MayRetryOnce
            } else {
                RetryDecision::NoAutomaticRetry("request_or_effect_ambiguous")
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn body_failure(
        &self,
        request: &Aag_Call_Request,
        observation: &GatewayHealth,
        request_digest: Sha256Digest,
        started: Instant,
        status: StatusCode,
        raw: &[u8],
        stream: &StreamAccumulator,
        error: Aag_Gateway_Error,
    ) -> Aag_Call_Result {
        let observed_bytes = if stream.committed_bytes() > 0 {
            stream.committed_bytes()
        } else {
            raw.len() as u64
        };
        self.failed(
            request,
            observation,
            request_digest,
            started,
            Some(status.as_u16()),
            (!raw.is_empty()).then(|| Sha256Digest::of(raw)),
            error,
            if error == Aag_Gateway_Error::Deadline || error == Aag_Gateway_Error::Cancelled {
                FailureStage::Timeout
            } else {
                FailureStage::BrokenAfterOutput
            },
            observed_bytes,
            stream.tool_call_seen(),
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn receipt(
        &self,
        request: &Aag_Call_Request,
        observation: &GatewayHealth,
        request_digest: Sha256Digest,
        started: Instant,
        http_status: Option<u16>,
        response_digest: Option<Sha256Digest>,
        outcome: CallOutcome,
        failure: Option<FailureStage>,
        completion: &ParsedCompletion,
    ) -> Option<ModelCallReceipt> {
        let binding = self.config.binding(request.role)?;
        Some(ModelCallReceipt {
            call_id: request.correlation.call_id,
            role: request.role,
            requested_alias: binding.model_alias.clone(),
            protocol: binding.protocol,
            reported_model: completion.reported_model.clone(),
            effective_provider: ProviderAttribution::Unknown,
            usage: completion.usage,
            http_status,
            outcome,
            failure,
            elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            request_sha256: request_digest,
            response_sha256: response_digest,
            route_generation: observation.route_generation,
            attempt: request.correlation.attempt,
        })
    }
}

fn map_response_error(error: ResponseError) -> Aag_Gateway_Error {
    match error {
        ResponseError::Malformed => Aag_Gateway_Error::ResponseMalformed,
        ResponseError::TooLarge => Aag_Gateway_Error::ResponseTooLarge,
        ResponseError::Empty => Aag_Gateway_Error::ResponseEmpty,
    }
}

fn numeric_loopback_origin(endpoint: &str) -> Result<(String, SocketAddr), Aag_Gateway_Error> {
    let rest = endpoint
        .strip_prefix("http://127.0.0.1:")
        .or_else(|| endpoint.strip_prefix("http://[::1]:"))
        .ok_or(Aag_Gateway_Error::OriginNotNumericLoopback)?;
    let port = rest.strip_suffix('/').unwrap_or(rest);
    if port.is_empty()
        || port.len() > 5
        || !port.bytes().all(|byte| byte.is_ascii_digit())
        || port.parse::<u16>().ok().filter(|port| *port > 0).is_none()
    {
        return Err(Aag_Gateway_Error::OriginNotNumericLoopback);
    }
    let origin = endpoint.trim_end_matches('/').to_string();
    let address = origin
        .strip_prefix("http://")
        .and_then(|value| value.parse::<SocketAddr>().ok())
        .ok_or(Aag_Gateway_Error::OriginNotNumericLoopback)?;
    if !address.ip().is_loopback() {
        return Err(Aag_Gateway_Error::OriginNotNumericLoopback);
    }
    Ok((origin, address))
}

async fn bounded<T>(
    future: impl Future<Output = T>,
    deadline: Instant,
    cancel: &mut watch::Receiver<bool>,
) -> Result<T, Aag_Gateway_Error> {
    if *cancel.borrow() {
        return Err(Aag_Gateway_Error::Cancelled);
    }
    tokio::select! {
        biased;
        _ = cancellation(cancel) => Err(Aag_Gateway_Error::Cancelled),
        _ = tokio::time::sleep_until(deadline) => Err(Aag_Gateway_Error::Deadline),
        result = future => Ok(result),
    }
}

async fn cancellation(cancel: &mut watch::Receiver<bool>) {
    loop {
        if cancel.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
        if *cancel.borrow_and_update() {
            return;
        }
    }
}

fn accept_sse_event(stream: &mut StreamAccumulator, event: &[u8]) -> Result<(), ResponseError> {
    let event = std::str::from_utf8(event).map_err(|_| ResponseError::Malformed)?;
    let data: Vec<&str> = event
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim_start)
        .collect();
    if data.is_empty() {
        return Ok(());
    }
    stream.accept_data(&data.join("\n"))
}

fn sse_event_end(pending: &[u8]) -> Option<(usize, usize)> {
    let lf = pending
        .windows(2)
        .position(|pair| pair == b"\n\n")
        .map(|end| (end, 2));
    let crlf = pending
        .windows(4)
        .position(|part| part == b"\r\n\r\n")
        .map(|end| (end, 4));
    match (lf, crlf) {
        (Some(left), Some(right)) => Some(if left.0 < right.0 { left } else { right }),
        (Some(found), None) | (None, Some(found)) => Some(found),
        (None, None) => None,
    }
}
