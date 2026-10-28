//! Bounded loopback AAG transport for workflow model calls (ADR 031 §3).
//!
//! This effect boundary keeps health observations distinct from request-bound
//! gateway routing evidence. Live calls require an explicit immutable v2 policy,
//! an admission precondition and a matched terminal receipt before releasing text.
//! Legacy fixture calls retain their separately owned listener requirement.

#![allow(non_camel_case_types)]

use std::{
    fmt,
    future::Future,
    net::SocketAddr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(feature = "test_helpers")]
use std::sync::Arc;

use reqwest::{Client, StatusCode, header::HeaderMap};
use serde_json::{Value, json};
#[cfg(feature = "test_helpers")]
use tokio::net::TcpListener;
use tokio::{sync::watch, time::Instant};
use vibemux_workflow::{
    Sha256Digest,
    gateway::{
        CORRELATION_HEADER, CallCorrelation, CallOutcome, FailureStage, GatewayConfig,
        GatewayHealth, GatewayProtocol, LiveAdmission, ModelCallReceipt, ParsedCompletion,
        ProviderAttribution, ReportedUsage, ResponseError, RetryDecision, RouteRole,
        StreamAccumulator, StreamEnd, evaluate_route, parse_completion, request_body,
    },
    gateway_receipt::{
        AagAdmission, AagBoundReceipt, AagReceiptCapabilities, AagSurface, GatewayObservedRouting,
    },
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const OBSERVATION_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_OBSERVATION_BYTES: usize = 64 * 1024;
const MAX_REQUEST_BYTES: usize = 256 * 1024;
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_SSE_EVENT_BYTES: usize = 64 * 1024;
const RECEIPT_CLOCK_SKEW_MS: u64 = 1_000;
const RECEIPT_CAPABILITIES_PATH: &str = "/_aag/v2/receipt_capabilities";
const RECEIPT_HEADER: &str = "x-aag-receipt";
const RECEIPT_CAPABILITY_HEADER: &str = "x-aag-receipt-capability";
const ROUTE_PRECONDITION_HEADER: &str = "x-aag-route-precondition";

/// Request-local possession material. It is never logged, returned or serialized.
struct ReceiptExchange {
    nonce: String,
    capability: [u8; 32],
    admission: AagAdmission,
    sent_at_ms: u64,
}

impl ReceiptExchange {
    fn new(admission: AagAdmission) -> Result<Self, Aag_Gateway_Error> {
        admission
            .validate()
            .map_err(|_| Aag_Gateway_Error::ReceiptInvalid)?;
        let mut nonce = [0_u8; 16];
        let mut capability = [0_u8; 32];
        getrandom::fill(&mut nonce).map_err(|_| Aag_Gateway_Error::ReceiptEntropyUnavailable)?;
        getrandom::fill(&mut capability)
            .map_err(|_| Aag_Gateway_Error::ReceiptEntropyUnavailable)?;
        Ok(Self {
            nonce: lower_hex(&nonce),
            capability,
            admission,
            sent_at_ms: clock_ms()?,
        })
    }

    fn opt_in(&self) -> String {
        format!("v2.{}.{}", self.nonce, lower_hex(&self.capability))
    }
}

impl Drop for ReceiptExchange {
    fn drop(&mut self) {
        self.capability.fill(0);
    }
}

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
    ReceiptEntropyUnavailable,
    ReceiptUnavailable,
    ReceiptInvalid,
    ReceiptExpired,
    ReceiptClockUnavailable,
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
            Self::ReceiptEntropyUnavailable => "gateway_receipt_entropy_unavailable",
            Self::ReceiptUnavailable => "gateway_receipt_unavailable",
            Self::ReceiptInvalid => "gateway_receipt_invalid",
            Self::ReceiptExpired => "gateway_receipt_expired",
            Self::ReceiptClockUnavailable => "gateway_receipt_clock_unavailable",
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
        if class == Aag_Call_Class::Live
            && self.config.live_admission != LiveAdmission::BoundReceiptV2
        {
            return Err(Aag_Gateway_Error::LiveAttestationUnavailable);
        }
        if class == Aag_Call_Class::Fixture {
            match self.fixture_permit.as_ref() {
                None => return Err(Aag_Gateway_Error::FixturePermitRequired),
                Some(permit) if permit.address != self.origin_address => {
                    return Err(Aag_Gateway_Error::FixturePermitMismatch);
                }
                Some(_) => {}
            }
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
        let receipt_exchange = match observation.admission_snapshot.clone() {
            Some(admission) => match ReceiptExchange::new(admission) {
                Ok(exchange) => Some(exchange),
                Err(error) => return Aag_Call_Result::blocked(error, Some(observation)),
            },
            None => None,
        };
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
        if let Some(exchange) = &receipt_exchange {
            let precondition = match exchange.admission.request_precondition() {
                Ok(value) => value,
                Err(_) => {
                    return Aag_Call_Result::blocked(
                        Aag_Gateway_Error::ReceiptInvalid,
                        Some(observation),
                    );
                }
            };
            http_request = http_request
                .header(RECEIPT_HEADER, exchange.opt_in())
                .header(ROUTE_PRECONDITION_HEADER, precondition);
        }
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
        // Validate after reading the bounded body so any refusal retains the
        // actual response digest and observed-output evidence without releasing text.
        let receipt_request_id = receipt_exchange
            .as_ref()
            .map(|exchange| response_receipt_identity(response.headers(), &exchange.nonce));
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
        if !media_type_is(
            response.headers(),
            if is_sse {
                "text/event-stream"
            } else {
                "application/json"
            },
        ) {
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
                let gateway_evidence = if let Some(exchange) = &receipt_exchange {
                    let expected_request_id = match receipt_request_id {
                        Some(Ok(value)) => value,
                        _ => {
                            return self.body_failure(
                                &request,
                                &observation,
                                request_digest,
                                started,
                                status,
                                &raw,
                                &stream,
                                Aag_Gateway_Error::ReceiptInvalid,
                            );
                        }
                    };
                    match self
                        .fetch_bound_receipt(
                            exchange,
                            &expected_request_id,
                            &binding.model_alias,
                            protocol,
                            status,
                            request.stream,
                            deadline,
                            &mut cancel,
                        )
                        .await
                    {
                        Ok(evidence) => Some(evidence),
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
                    }
                } else {
                    None
                };
                let mut receipt = self.receipt(
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
                if let (Some(receipt), Some(evidence)) = (&mut receipt, gateway_evidence) {
                    receipt.effective_provider =
                        ProviderAttribution::GatewayObserved(Box::new(evidence));
                }
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
        let mut observation = GatewayHealth::from_responses(&health, &models)
            .map_err(|_| Aag_Gateway_Error::ObservationUnavailable)?;
        if self.config.live_admission == LiveAdmission::BoundReceiptV2 {
            let bytes = self
                .fetch_json_bytes(
                    RECEIPT_CAPABILITIES_PATH,
                    None,
                    observation_deadline,
                    cancel,
                )
                .await?;
            let capabilities = AagReceiptCapabilities::parse(&bytes)
                .map_err(|_| Aag_Gateway_Error::ReceiptInvalid)?;
            let admission = capabilities.admission();
            // A stale /health pin cannot be cleared by the later capability read.
            observation.pinned_provider_present |= admission.pinned_provider_present;
            observation
                .bind_admission(admission)
                .map_err(|_| Aag_Gateway_Error::ReceiptInvalid)?;
        }
        Ok(observation)
    }

    #[allow(clippy::too_many_arguments)]
    async fn fetch_bound_receipt(
        &self,
        exchange: &ReceiptExchange,
        response_request_id: &str,
        selector: &str,
        protocol: GatewayProtocol,
        status: StatusCode,
        stream: bool,
        deadline: Instant,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<GatewayObservedRouting, Aag_Gateway_Error> {
        let bytes = self
            .fetch_json_bytes(
                &format!("/_aag/v2/receipts/{}", exchange.nonce),
                Some(exchange),
                deadline,
                cancel,
            )
            .await
            .map_err(|error| match error {
                Aag_Gateway_Error::ObservationUnavailable => Aag_Gateway_Error::ReceiptUnavailable,
                other => other,
            })?;
        let bound =
            AagBoundReceipt::parse(&bytes).map_err(|_| Aag_Gateway_Error::ReceiptInvalid)?;
        let surface = match protocol {
            GatewayProtocol::OpenaiChatCompletions => AagSurface::Chat,
            GatewayProtocol::AnthropicMessages => AagSurface::Messages,
            GatewayProtocol::OpenaiResponses => return Err(Aag_Gateway_Error::UnsupportedProtocol),
        };
        let evidence = bound
            .validate_success(
                &exchange.admission,
                &exchange.nonce,
                selector,
                surface,
                status.as_u16(),
                stream,
            )
            .map_err(|_| Aag_Gateway_Error::ReceiptInvalid)?;
        let routing = &evidence.receipt().routing_receipt;
        if routing.request_id != response_request_id {
            return Err(Aag_Gateway_Error::ReceiptInvalid);
        }
        let now = clock_ms()?;
        if routing.issued_at_ms.saturating_add(RECEIPT_CLOCK_SKEW_MS) < exchange.sent_at_ms
            || routing.issued_at_ms > now.saturating_add(RECEIPT_CLOCK_SKEW_MS)
            || routing.expires_at_ms.is_none_or(|expires| expires <= now)
        {
            return Err(Aag_Gateway_Error::ReceiptExpired);
        }
        Ok(evidence)
    }

    async fn fetch_json(
        &self,
        path: &str,
        deadline: Instant,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<Value, Aag_Gateway_Error> {
        let bytes = self.fetch_json_bytes(path, None, deadline, cancel).await?;
        serde_json::from_slice(&bytes).map_err(|_| Aag_Gateway_Error::ObservationUnavailable)
    }

    async fn fetch_json_bytes(
        &self,
        path: &str,
        receipt_exchange: Option<&ReceiptExchange>,
        deadline: Instant,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<Vec<u8>, Aag_Gateway_Error> {
        let mut request = self
            .client
            .get(format!("{}{}", self.origin, path))
            .bearer_auth(&self.credential.0);
        if let Some(exchange) = receipt_exchange {
            request = request.header(RECEIPT_CAPABILITY_HEADER, lower_hex(&exchange.capability));
        }
        let sent = bounded(request.send(), deadline, cancel)
            .await?
            .map_err(|_| Aag_Gateway_Error::ObservationUnavailable)?;
        if sent.status() != StatusCode::OK || !media_type_is(sent.headers(), "application/json") {
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
        Ok(bytes)
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

fn lower_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(char::from(DIGITS[usize::from(byte >> 4)]));
        result.push(char::from(DIGITS[usize::from(byte & 15)]));
    }
    result
}

fn clock_ms() -> Result<u64, Aag_Gateway_Error> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Aag_Gateway_Error::ReceiptClockUnavailable)?;
    u64::try_from(elapsed.as_millis()).map_err(|_| Aag_Gateway_Error::ReceiptClockUnavailable)
}

fn unique_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    value.to_str().ok()
}

fn media_type_is(headers: &HeaderMap, expected: &str) -> bool {
    unique_header(headers, "content-type")
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case(expected))
}

fn response_receipt_identity(
    headers: &HeaderMap,
    nonce: &str,
) -> Result<String, Aag_Gateway_Error> {
    if unique_header(headers, "x-aag-receipt-version") != Some("2")
        || unique_header(headers, "x-aag-receipt-nonce") != Some(nonce)
    {
        return Err(Aag_Gateway_Error::ReceiptInvalid);
    }
    let request_id = unique_header(headers, "x-aag-request-id")
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
        .ok_or(Aag_Gateway_Error::ReceiptInvalid)?;
    Ok(request_id.to_string())
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
