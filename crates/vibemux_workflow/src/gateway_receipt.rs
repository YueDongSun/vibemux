//! Strict content-free AAG receipt evidence, bound to a pre-request v2 admission.
//!
//! The nested v1 wire schema remains producer-compatible. This module records
//! gateway observations, not independent upstream model attestation. Transport
//! authentication, fresh capability retrieval and response exhaustion remain the
//! caller's responsibilities; metadata alone cannot establish those events.

use serde::{Deserialize, Deserializer, Serialize};
use std::net::IpAddr;
use thiserror::Error;

pub const RECEIPT_VERSION: u16 = 1;
pub const MAX_ATTEMPTS: usize = 32;
pub const MAX_MODEL_BYTES: usize = 256;
pub const MAX_REFERENCE_BYTES: usize = 128;

const CREDENTIAL_PREFIXES: [&str; 5] = ["sk-", "ghp_", "github_pat_", "xoxb-", "xoxp-"];

pub const MAX_AAG_RECEIPT_BYTES: usize = 128 * 1024;
pub const MAX_AAG_CAPABILITY_BYTES: usize = 8 * 1024;
const MAX_JSON_INTEGER: u64 = (1_u64 << 53) - 1;
const RECEIPT_CAPACITY: u16 = 128;
const TERMINAL_RETENTION_SECONDS: u16 = 600;
const ERROR_CODES: [&str; 8] = [
    "upstream_http_error",
    "upstream_timeout",
    "upstream_transport_error",
    "upstream_decode_error",
    "upstream_stream_error",
    "upstream_error",
    "routing_receipt_attempt_limit",
    "provider_error",
];

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptPhase {
    Admitted,
    Routing,
    Streaming,
    Settled,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BodyState {
    Pending,
    Exhausted,
    Dropped,
    Error,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransportState {
    Sending,
    HeadersReceived,
    ResponseParsed,
    StreamEnded,
    Failed,
    Abandoned,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ValueState {
    Missing,
    Reported,
    Rejected,
    Conflicting,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AssociationKind {
    SupplierEndpoint,
    ManualProvider,
    LegacyQueue,
    Unmapped,
    Ambiguous,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WireProtocol {
    OpenaiChat,
    Anthropic,
    OpenaiResponses,
    Gemini,
    Unknown,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialMode {
    StoredKey,
    ClientPassthrough,
    None,
    Unknown,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelSource {
    Model,
    MessageModel,
    ResponseModel,
    ModelVersion,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalSignal {
    OpenaiFinish,
    OpenaiDone,
    AnthropicMessageStop,
    ResponsesCompleted,
    ResponsesIncomplete,
    ResponsesFailed,
    ResponsesCancelled,
    GeminiFinish,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttestationState {
    Unavailable,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RoutingIdentity {
    #[serde(deserialize_with = "required_option")]
    pub provider_ref: Option<String>,
    #[serde(deserialize_with = "required_option")]
    pub supplier_ref: Option<String>,
    #[serde(deserialize_with = "required_option")]
    pub endpoint_ref: Option<String>,
    pub association: AssociationKind,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttemptRecord {
    pub index: u32,
    pub hop_index: u32,
    pub subattempt: u32,
    pub identity: RoutingIdentity,
    pub protocol: WireProtocol,
    pub credential_mode: CredentialMode,
    #[serde(deserialize_with = "required_option")]
    pub wire_model: Option<String>,
    pub wire_model_state: ValueState,
    #[serde(deserialize_with = "required_option")]
    pub provider_model: Option<String>,
    pub provider_model_state: ValueState,
    #[serde(deserialize_with = "required_option")]
    pub provider_model_source: Option<ModelSource>,
    #[serde(deserialize_with = "required_option")]
    pub http_status: Option<u16>,
    pub send_started: bool,
    pub transport_state: TransportState,
    pub terminal_signals: Vec<TerminalSignal>,
    #[serde(deserialize_with = "required_option")]
    pub error_code: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AagRoutingReceipt {
    pub schema_version: u16,
    pub issuer: String,
    pub evidence_kind: String,
    pub server_instance_id: String,
    pub request_nonce: String,
    pub request_id: String,
    pub surface: String,
    #[serde(deserialize_with = "required_option")]
    pub generation: Option<u64>,
    #[serde(deserialize_with = "required_option")]
    pub requested_selector: Option<String>,
    pub selector_state: ValueState,
    pub phase: ReceiptPhase,
    #[serde(deserialize_with = "required_option")]
    pub http_status: Option<u16>,
    pub body_state: BodyState,
    #[serde(deserialize_with = "required_option")]
    pub selected_attempt: Option<u32>,
    pub attempts: Vec<AttemptRecord>,
    pub capture_complete: bool,
    pub capture_truncated: bool,
    pub upstream_model_attestation: AttestationState,
    pub issued_at_ms: u64,
    #[serde(deserialize_with = "required_option")]
    pub expires_at_ms: Option<u64>,
}

pub fn safe_reference(value: &str) -> Option<String> {
    if value.is_empty()
        || value.len() > MAX_REFERENCE_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        || has_credential_prefix(value)
    {
        return None;
    }
    Some(value.to_owned())
}

pub fn safe_model(value: &str) -> Option<String> {
    let address_candidate = value
        .strip_prefix('[')
        .and_then(|address| address.strip_suffix(']'))
        .unwrap_or(value);
    if value.is_empty()
        || value.len() > MAX_MODEL_BYTES
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'_' | b'-' | b'.' | b'/' | b':' | b'@' | b'[' | b']')
        })
        || value.starts_with('/')
        || value.contains("://")
        || has_credential_prefix(value)
        || address_candidate.parse::<IpAddr>().is_ok()
    {
        return None;
    }
    Some(value.to_owned())
}

fn has_credential_prefix(value: &str) -> bool {
    CREDENTIAL_PREFIXES.iter().any(|prefix| {
        value
            .get(..prefix.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(prefix))
    })
}

fn required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AagSurface {
    Chat,
    Messages,
}

impl AagSurface {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Messages => "messages",
        }
    }

    fn parse(value: &str) -> Result<Self, ReceiptValidationError> {
        match value {
            "chat" => Ok(Self::Chat),
            "messages" => Ok(Self::Messages),
            _ => Err(ReceiptValidationError::SurfaceMismatch),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessagesCredentialScope {
    StoredKeyOnlySnapshot,
}

/// Safe failure classifications never incorporate remote metadata or parse errors.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ReceiptValidationError {
    #[error("aag_receipt_too_large")]
    TooLarge,
    #[error("aag_receipt_malformed")]
    Malformed,
    #[error("aag_receipt_contract_unsupported")]
    ContractUnsupported,
    #[error("aag_receipt_admission_invalid")]
    AdmissionInvalid,
    #[error("aag_receipt_admission_mismatch")]
    AdmissionMismatch,
    #[error("aag_receipt_request_mismatch")]
    RequestMismatch,
    #[error("aag_receipt_surface_mismatch")]
    SurfaceMismatch,
    #[error("aag_receipt_status_mismatch")]
    StatusMismatch,
    #[error("aag_receipt_metadata_invalid")]
    MetadataInvalid,
    #[error("aag_receipt_incomplete")]
    Incomplete,
    #[error("aag_receipt_selected_attempt_invalid")]
    SelectedAttemptInvalid,
}

impl ReceiptValidationError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::TooLarge => "aag_receipt_too_large",
            Self::Malformed => "aag_receipt_malformed",
            Self::ContractUnsupported => "aag_receipt_contract_unsupported",
            Self::AdmissionInvalid => "aag_receipt_admission_invalid",
            Self::AdmissionMismatch => "aag_receipt_admission_mismatch",
            Self::RequestMismatch => "aag_receipt_request_mismatch",
            Self::SurfaceMismatch => "aag_receipt_surface_mismatch",
            Self::StatusMismatch => "aag_receipt_status_mismatch",
            Self::MetadataInvalid => "aag_receipt_metadata_invalid",
            Self::Incomplete => "aag_receipt_incomplete",
            Self::SelectedAttemptInvalid => "aag_receipt_selected_attempt_invalid",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AagAdmission {
    pub server_instance_id: String,
    pub generation: u64,
    pub pinned_provider_present: bool,
}

impl AagAdmission {
    pub fn validate(&self) -> Result<(), ReceiptValidationError> {
        if !lower_hex_32(&self.server_instance_id) || self.generation == 0 {
            return Err(ReceiptValidationError::AdmissionInvalid);
        }
        Ok(())
    }

    pub fn request_precondition(&self) -> Result<String, ReceiptValidationError> {
        self.validate()?;
        Ok(format!(
            "v2.{}.{}.{}",
            self.server_instance_id,
            self.generation,
            u8::from(self.pinned_provider_present)
        ))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AagReceiptCapabilities {
    pub schema_version: u16,
    pub issuer: String,
    pub supported_versions: Vec<u16>,
    pub server_instance_id: String,
    pub generation: u64,
    pub pinned_provider_present: bool,
    pub supported_surfaces: Vec<AagSurface>,
    pub receipt_capacity: u16,
    pub max_retained_wire_attempts: u16,
    pub terminal_retention_seconds: u16,
    pub upstream_model_attestation: AttestationState,
    pub messages_credential_scope: MessagesCredentialScope,
}

impl AagReceiptCapabilities {
    pub fn parse(bytes: &[u8]) -> Result<Self, ReceiptValidationError> {
        let value: Self = parse_bounded(bytes, MAX_AAG_CAPABILITY_BYTES)?;
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), ReceiptValidationError> {
        if self.schema_version != 2
            || self.issuer != "aag"
            || self.supported_versions != [2]
            || self.supported_surfaces != [AagSurface::Chat, AagSurface::Messages]
            || self.receipt_capacity != RECEIPT_CAPACITY
            || usize::from(self.max_retained_wire_attempts) != MAX_ATTEMPTS
            || self.terminal_retention_seconds != TERMINAL_RETENTION_SECONDS
        {
            return Err(ReceiptValidationError::ContractUnsupported);
        }
        self.admission().validate()
    }

    #[must_use]
    pub fn admission(&self) -> AagAdmission {
        AagAdmission {
            server_instance_id: self.server_instance_id.clone(),
            generation: self.generation,
            pinned_provider_present: self.pinned_provider_present,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AagBoundReceipt {
    pub schema_version: u16,
    pub issuer: String,
    #[serde(deserialize_with = "required_option")]
    pub admission: Option<AagAdmission>,
    pub routing_receipt: AagRoutingReceipt,
}

impl AagBoundReceipt {
    pub fn parse(bytes: &[u8]) -> Result<Self, ReceiptValidationError> {
        let value: Self = parse_bounded(bytes, MAX_AAG_RECEIPT_BYTES)?;
        value.validate_shape()?;
        Ok(value)
    }

    pub fn validate_shape(&self) -> Result<(), ReceiptValidationError> {
        if self.schema_version != 2 || self.issuer != "aag" {
            return Err(ReceiptValidationError::ContractUnsupported);
        }
        if let Some(admission) = &self.admission {
            admission.validate()?;
        }
        self.routing_receipt.validate_shape()
    }

    /// Binds a terminal successful response to the exact pre-send snapshot and nonce.
    /// A selected provider's reported model remains an unverified claim.
    pub fn validate_success(
        self,
        expected: &AagAdmission,
        request_nonce: &str,
        requested_selector: &str,
        surface: AagSurface,
        response_http_status: u16,
        stream: bool,
    ) -> Result<GatewayObservedRouting, ReceiptValidationError> {
        self.validate_shape()?;
        expected.validate()?;
        let receipt = &self.routing_receipt;
        if self.admission.as_ref() != Some(expected)
            || receipt.server_instance_id != expected.server_instance_id
            || receipt.generation != Some(expected.generation)
        {
            return Err(ReceiptValidationError::AdmissionMismatch);
        }
        if !lower_hex_32(request_nonce)
            || receipt.request_nonce != request_nonce
            || safe_model(requested_selector).is_none()
            || receipt.selector_state != ValueState::Reported
            || receipt.requested_selector.as_deref() != Some(requested_selector)
        {
            return Err(ReceiptValidationError::RequestMismatch);
        }
        if receipt.surface != surface.as_str() {
            return Err(ReceiptValidationError::SurfaceMismatch);
        }
        if !(200..=299).contains(&response_http_status)
            || receipt.http_status != Some(response_http_status)
        {
            return Err(ReceiptValidationError::StatusMismatch);
        }
        if receipt.phase != ReceiptPhase::Settled
            || receipt.body_state != BodyState::Exhausted
            || !receipt.capture_complete
            || receipt.capture_truncated
        {
            return Err(ReceiptValidationError::Incomplete);
        }
        let selected = receipt
            .selected_attempt
            .and_then(|index| {
                usize::try_from(index)
                    .ok()
                    .and_then(|index| index.checked_sub(1))
                    .and_then(|index| receipt.attempts.get(index))
            })
            .ok_or(ReceiptValidationError::SelectedAttemptInvalid)?;
        if !selected.send_started
            || !selected
                .http_status
                .is_some_and(|status| (200..=299).contains(&status))
            || selected.error_code.is_some()
            || if stream {
                selected.transport_state != TransportState::StreamEnded
                    || !successful_terminal(selected)
            } else {
                selected.transport_state != TransportState::ResponseParsed
            }
        {
            return Err(ReceiptValidationError::SelectedAttemptInvalid);
        }
        if surface == AagSurface::Messages
            && receipt
                .attempts
                .iter()
                .any(|attempt| attempt.credential_mode != CredentialMode::StoredKey)
        {
            return Err(ReceiptValidationError::MetadataInvalid);
        }
        Ok(GatewayObservedRouting {
            receipt: self,
            response_http_status,
            response_stream: stream,
        })
    }
}

impl AagRoutingReceipt {
    pub fn parse(bytes: &[u8]) -> Result<Self, ReceiptValidationError> {
        let value: Self = parse_bounded(bytes, MAX_AAG_RECEIPT_BYTES)?;
        value.validate_shape()?;
        Ok(value)
    }

    pub fn validate_shape(&self) -> Result<(), ReceiptValidationError> {
        if self.schema_version != RECEIPT_VERSION
            || self.issuer != "aag"
            || self.evidence_kind != "gateway_observed"
        {
            return Err(ReceiptValidationError::ContractUnsupported);
        }
        AagSurface::parse(&self.surface)?;
        if !lower_hex_32(&self.server_instance_id)
            || !lower_hex_32(&self.request_nonce)
            || safe_reference(&self.request_id).is_none()
            || !model_claim(self.requested_selector.as_deref(), self.selector_state)
            || !valid_http_status(self.http_status)
            || self.attempts.len() > MAX_ATTEMPTS
            || self.issued_at_ms > MAX_JSON_INTEGER
            || self
                .expires_at_ms
                .is_some_and(|value| value > MAX_JSON_INTEGER || value < self.issued_at_ms)
            || self
                .selected_attempt
                .is_some_and(|index| index == 0 || index as usize > self.attempts.len())
        {
            return Err(ReceiptValidationError::MetadataInvalid);
        }
        for (index, attempt) in self.attempts.iter().enumerate() {
            validate_attempt(attempt, index)?;
        }
        if if self.phase == ReceiptPhase::Settled {
            self.body_state == BodyState::Pending || self.expires_at_ms.is_none()
        } else {
            self.body_state != BodyState::Pending
                || self.expires_at_ms.is_some()
                || self.http_status.is_some()
        } {
            return Err(ReceiptValidationError::MetadataInvalid);
        }
        if self.capture_truncated && (self.capture_complete || self.selected_attempt.is_some()) {
            return Err(ReceiptValidationError::MetadataInvalid);
        }
        if self.capture_complete
            && (self.phase != ReceiptPhase::Settled
                || self.selector_state != ValueState::Reported
                || self.attempts.is_empty()
                || !matches!(self.body_state, BodyState::Exhausted | BodyState::Error)
                || self.attempts.iter().any(|attempt| {
                    let terminal_transport_valid = match attempt.transport_state {
                        TransportState::ResponseParsed | TransportState::Failed => true,
                        TransportState::StreamEnded => !attempt.terminal_signals.is_empty(),
                        _ => false,
                    };
                    attempt.wire_model_state != ValueState::Reported
                        || attempt.identity.provider_ref.is_none()
                        || attempt.protocol == WireProtocol::Unknown
                        || !terminal_transport_valid
                }))
        {
            return Err(ReceiptValidationError::MetadataInvalid);
        }
        Ok(())
    }
}

/// Owned, validated gateway-observed metadata. Private fields prevent unchecked construction.
/// Deserialization rechecks internal consistency; it cannot authenticate external storage.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GatewayObservedRouting {
    receipt: AagBoundReceipt,
    response_http_status: u16,
    response_stream: bool,
}

impl<'de> Deserialize<'de> for GatewayObservedRouting {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct StoredEvidence {
            receipt: AagBoundReceipt,
            response_http_status: u16,
            response_stream: bool,
        }
        let stored = StoredEvidence::deserialize(deserializer)?;
        let admission = stored.receipt.admission.clone().ok_or_else(|| {
            <D::Error as serde::de::Error>::custom(ReceiptValidationError::AdmissionMismatch.code())
        })?;
        let nonce = stored.receipt.routing_receipt.request_nonce.clone();
        let selector = stored
            .receipt
            .routing_receipt
            .requested_selector
            .clone()
            .ok_or_else(|| {
                <D::Error as serde::de::Error>::custom(
                    ReceiptValidationError::RequestMismatch.code(),
                )
            })?;
        let surface = AagSurface::parse(&stored.receipt.routing_receipt.surface)
            .map_err(|error| <D::Error as serde::de::Error>::custom(error.code()))?;
        stored
            .receipt
            .validate_success(
                &admission,
                &nonce,
                &selector,
                surface,
                stored.response_http_status,
                stored.response_stream,
            )
            .map_err(|error| <D::Error as serde::de::Error>::custom(error.code()))
    }
}

impl GatewayObservedRouting {
    #[must_use]
    pub const fn evidence_kind(&self) -> &'static str {
        "gateway_observed"
    }
    #[must_use]
    pub const fn upstream_model_attestation(&self) -> AttestationState {
        AttestationState::Unavailable
    }
    #[must_use]
    pub const fn receipt(&self) -> &AagBoundReceipt {
        &self.receipt
    }
    #[must_use]
    pub fn admission(&self) -> &AagAdmission {
        self.receipt
            .admission
            .as_ref()
            .expect("validated admission")
    }
    #[must_use]
    pub fn selected_attempt(&self) -> &AttemptRecord {
        let index = self
            .receipt
            .routing_receipt
            .selected_attempt
            .expect("validated selection");
        &self.receipt.routing_receipt.attempts[index as usize - 1]
    }
    #[must_use]
    pub fn selected_identity(&self) -> &RoutingIdentity {
        &self.selected_attempt().identity
    }
    #[must_use]
    pub fn wire_model(&self) -> &str {
        self.selected_attempt()
            .wire_model
            .as_deref()
            .expect("validated wire model")
    }
    #[must_use]
    pub fn provider_reported_model(&self) -> Option<&str> {
        self.selected_attempt().provider_model.as_deref()
    }
}

fn parse_bounded<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    maximum: usize,
) -> Result<T, ReceiptValidationError> {
    if bytes.len() > maximum {
        return Err(ReceiptValidationError::TooLarge);
    }
    serde_json::from_slice(bytes).map_err(|_| ReceiptValidationError::Malformed)
}
fn lower_hex_32(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn valid_http_status(value: Option<u16>) -> bool {
    value.is_none_or(|status| (100..=599).contains(&status))
}
fn model_claim(value: Option<&str>, state: ValueState) -> bool {
    if state == ValueState::Reported {
        value.is_some_and(|value| safe_model(value).is_some())
    } else {
        value.is_none()
    }
}
fn validate_attempt(attempt: &AttemptRecord, index: usize) -> Result<(), ReceiptValidationError> {
    let identity = &attempt.identity;
    if attempt.index as usize != index + 1
        || [
            &identity.provider_ref,
            &identity.supplier_ref,
            &identity.endpoint_ref,
        ]
        .into_iter()
        .flatten()
        .any(|value| safe_reference(value).is_none())
        || !model_claim(attempt.wire_model.as_deref(), attempt.wire_model_state)
        || !model_claim(
            attempt.provider_model.as_deref(),
            attempt.provider_model_state,
        )
        || (attempt.provider_model_state == ValueState::Missing)
            != attempt.provider_model_source.is_none()
        || !valid_http_status(attempt.http_status)
        || attempt.terminal_signals.len() > 8
        || attempt
            .terminal_signals
            .iter()
            .enumerate()
            .any(|(index, signal)| attempt.terminal_signals[..index].contains(signal))
        || attempt
            .error_code
            .as_deref()
            .is_some_and(|code| !ERROR_CODES.contains(&code))
    {
        return Err(ReceiptValidationError::MetadataInvalid);
    }
    Ok(())
}
fn successful_terminal(attempt: &AttemptRecord) -> bool {
    let signals = &attempt.terminal_signals;
    !signals.is_empty()
        && signals.iter().all(|signal| match attempt.protocol {
            WireProtocol::OpenaiChat => matches!(
                signal,
                TerminalSignal::OpenaiFinish | TerminalSignal::OpenaiDone
            ),
            WireProtocol::Anthropic => *signal == TerminalSignal::AnthropicMessageStop,
            WireProtocol::OpenaiResponses => *signal == TerminalSignal::ResponsesCompleted,
            WireProtocol::Gemini => *signal == TerminalSignal::GeminiFinish,
            WireProtocol::Unknown => false,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    const INSTANCE: &str = "0123456789abcdef0123456789abcdef";
    const NONCE: &str = "abcdef0123456789abcdef0123456789";
    const SELECTOR: &str = "alias:code_best";

    fn admission() -> AagAdmission {
        AagAdmission {
            server_instance_id: INSTANCE.into(),
            generation: 7,
            pinned_provider_present: false,
        }
    }

    fn capabilities() -> Value {
        json!({"schema_version":2,"issuer":"aag","supported_versions":[2],
            "server_instance_id":INSTANCE,"generation":7,"pinned_provider_present":false,
            "supported_surfaces":["chat","messages"],"receipt_capacity":128,
            "max_retained_wire_attempts":32,"terminal_retention_seconds":600,
            "upstream_model_attestation":"unavailable","messages_credential_scope":"stored_key_only_snapshot"})
    }

    fn receipt(surface: AagSurface, stream: bool) -> Value {
        let protocol = if surface == AagSurface::Chat {
            "openai_chat"
        } else {
            "anthropic"
        };
        let signals = if !stream {
            json!([])
        } else if surface == AagSurface::Chat {
            json!(["openai_finish", "openai_done"])
        } else {
            json!(["anthropic_message_stop"])
        };
        json!({"schema_version":2,"issuer":"aag","admission":admission(),"routing_receipt":{
            "schema_version":1,"issuer":"aag","evidence_kind":"gateway_observed",
            "server_instance_id":INSTANCE,"request_nonce":NONCE,"request_id":"request_42",
            "surface":surface.as_str(),"generation":7,"requested_selector":SELECTOR,
            "selector_state":"reported","phase":"settled","http_status":200,
            "body_state":"exhausted","selected_attempt":1,"attempts":[{
                "index":1,"hop_index":0,"subattempt":0,
                "identity":{"provider_ref":"provider_1","supplier_ref":null,"endpoint_ref":null,"association":"manual_provider"},
                "protocol":protocol,"credential_mode":"stored_key","wire_model":"vendor/wire-model",
                "wire_model_state":"reported","provider_model":"provider-reported-v3","provider_model_state":"reported",
                "provider_model_source":"model","http_status":200,"send_started":true,
                "transport_state":if stream {"stream_ended"} else {"response_parsed"},
                "terminal_signals":signals,"error_code":null}],
            "capture_complete":true,"capture_truncated":false,"upstream_model_attestation":"unavailable",
            "issued_at_ms":1700000000000_u64,"expires_at_ms":1700000600000_u64}})
    }

    fn decode(value: Value) -> Result<AagBoundReceipt, ReceiptValidationError> {
        AagBoundReceipt::parse(&serde_json::to_vec(&value).expect("serialize fixture"))
    }

    fn successful(
        value: Value,
        surface: AagSurface,
        stream: bool,
    ) -> Result<GatewayObservedRouting, ReceiptValidationError> {
        decode(value)?.validate_success(&admission(), NONCE, SELECTOR, surface, 200, stream)
    }

    #[test]
    fn capabilities_pin_the_exact_v2_contract_and_header_precondition() {
        let value =
            AagReceiptCapabilities::parse(&serde_json::to_vec(&capabilities()).unwrap()).unwrap();
        assert_eq!(value.admission(), admission());
        assert_eq!(
            value.admission().request_precondition().unwrap(),
            format!("v2.{INSTANCE}.7.0")
        );
        let mut pinned = admission();
        pinned.pinned_provider_present = true;
        assert_eq!(
            pinned.request_precondition().unwrap(),
            format!("v2.{INSTANCE}.7.1")
        );
        for (pointer, invalid_value) in [
            ("/schema_version", json!(1)),
            ("/schema_version", json!(3)),
            ("/supported_versions", json!([1, 2])),
            ("/supported_versions", json!([2, 2])),
            ("/supported_surfaces", json!(["chat"])),
            (
                "/supported_surfaces",
                json!(["chat", "messages", "responses"]),
            ),
            ("/receipt_capacity", json!(129)),
            ("/max_retained_wire_attempts", json!(33)),
            ("/terminal_retention_seconds", json!(601)),
            ("/messages_credential_scope", json!("passthrough")),
            ("/generation", json!(0)),
            ("/server_instance_id", json!("not_an_instance")),
            ("/upstream_model_attestation", json!("verified")),
        ] {
            let mut value = capabilities();
            *value.pointer_mut(pointer).unwrap() = invalid_value;
            assert!(
                AagReceiptCapabilities::parse(&serde_json::to_vec(&value).unwrap()).is_err(),
                "accepted {pointer}"
            );
        }
        let mut invalid = admission();
        invalid.server_instance_id = "bad\r\nproof".into();
        assert_eq!(
            invalid.request_precondition(),
            Err(ReceiptValidationError::AdmissionInvalid)
        );
    }

    #[test]
    fn bound_json_roundtrip_preserves_nested_v1_without_changing_its_shape() {
        let value = receipt(AagSurface::Chat, false);
        let parsed = decode(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), value);
        assert_eq!(
            serde_json::to_value(
                AagRoutingReceipt::parse(&serde_json::to_vec(&value["routing_receipt"]).unwrap())
                    .unwrap()
            )
            .unwrap(),
            value["routing_receipt"]
        );
        assert!(
            AagBoundReceipt::parse(&serde_json::to_vec(&value["routing_receipt"]).unwrap())
                .is_err()
        );
        let evidence = successful(value, AagSurface::Chat, false).unwrap();
        assert_eq!(evidence.evidence_kind(), "gateway_observed");
        assert_eq!(
            evidence.upstream_model_attestation(),
            AttestationState::Unavailable
        );
        assert_eq!(
            evidence.selected_identity().provider_ref.as_deref(),
            Some("provider_1")
        );
        assert_eq!(evidence.wire_model(), "vendor/wire-model");
        assert_eq!(
            evidence.provider_reported_model(),
            Some("provider-reported-v3")
        );
        let stored = serde_json::to_vec(&evidence).unwrap();
        assert_eq!(
            serde_json::from_slice::<GatewayObservedRouting>(&stored).unwrap(),
            evidence
        );
    }

    #[test]
    fn unknown_versions_fields_and_enum_values_are_refused() {
        for pointer in [
            "",
            "/admission",
            "/routing_receipt",
            "/routing_receipt/attempts/0",
            "/routing_receipt/attempts/0/identity",
        ] {
            let mut value = receipt(AagSurface::Chat, false);
            value
                .pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("untrusted_payload".into(), json!("excluded"));
            assert!(decode(value).is_err(), "accepted field at {pointer}");
        }
        for pointer in [
            "/routing_receipt/phase",
            "/routing_receipt/body_state",
            "/routing_receipt/selector_state",
            "/routing_receipt/upstream_model_attestation",
            "/routing_receipt/attempts/0/protocol",
            "/routing_receipt/attempts/0/credential_mode",
            "/routing_receipt/attempts/0/identity/association",
            "/routing_receipt/attempts/0/provider_model_source",
            "/routing_receipt/attempts/0/transport_state",
        ] {
            let mut value = receipt(AagSurface::Chat, false);
            *value.pointer_mut(pointer).unwrap() = json!("future_variant");
            assert!(decode(value).is_err(), "accepted enum at {pointer}");
        }
        for pointer in ["/schema_version", "/routing_receipt/schema_version"] {
            let mut value = receipt(AagSurface::Chat, false);
            *value.pointer_mut(pointer).unwrap() = json!(99);
            assert_eq!(
                decode(value).unwrap_err(),
                ReceiptValidationError::ContractUnsupported
            );
        }
    }

    #[test]
    fn nullable_wire_fields_must_be_present_and_duplicate_fields_are_refused() {
        for (parent, key) in [
            ("", "admission"),
            ("/routing_receipt", "generation"),
            ("/routing_receipt", "requested_selector"),
            ("/routing_receipt", "http_status"),
            ("/routing_receipt", "selected_attempt"),
            ("/routing_receipt", "expires_at_ms"),
            ("/routing_receipt/attempts/0", "wire_model"),
            ("/routing_receipt/attempts/0", "provider_model"),
            ("/routing_receipt/attempts/0", "provider_model_source"),
            ("/routing_receipt/attempts/0", "http_status"),
            ("/routing_receipt/attempts/0", "error_code"),
            ("/routing_receipt/attempts/0/identity", "supplier_ref"),
        ] {
            let mut value = receipt(AagSurface::Chat, false);
            value
                .pointer_mut(parent)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(key);
            assert_eq!(
                decode(value).unwrap_err(),
                ReceiptValidationError::Malformed,
                "accepted missing {key}"
            );
        }
        let encoded = serde_json::to_string(&receipt(AagSurface::Chat, false)).unwrap();
        let duplicate = encoded.replacen(
            "\"issuer\":\"aag\"",
            "\"issuer\":\"aag\",\"issuer\":\"aag\"",
            1,
        );
        assert_eq!(
            AagBoundReceipt::parse(duplicate.as_bytes()).unwrap_err(),
            ReceiptValidationError::Malformed
        );
    }

    #[test]
    fn admission_nonce_selector_surface_and_outer_status_bind_success() {
        for (pointer, change) in [
            ("/admission", Value::Null),
            ("/admission/generation", json!(8)),
            ("/admission/pinned_provider_present", json!(true)),
            (
                "/admission/server_instance_id",
                json!("11111111111111111111111111111111"),
            ),
            (
                "/routing_receipt/server_instance_id",
                json!("11111111111111111111111111111111"),
            ),
            ("/routing_receipt/generation", json!(8)),
            ("/routing_receipt/generation", Value::Null),
            (
                "/routing_receipt/request_nonce",
                json!("11111111111111111111111111111111"),
            ),
            (
                "/routing_receipt/requested_selector",
                json!("alias:another"),
            ),
            ("/routing_receipt/surface", json!("messages")),
            ("/routing_receipt/http_status", json!(201)),
        ] {
            let mut value = receipt(AagSurface::Chat, false);
            *value.pointer_mut(pointer).unwrap() = change;
            assert!(
                successful(value, AagSurface::Chat, false).is_err(),
                "accepted wrong binding {pointer}"
            );
        }
        let parsed = decode(receipt(AagSurface::Chat, false)).unwrap();
        assert_eq!(
            parsed
                .validate_success(&admission(), NONCE, SELECTOR, AagSurface::Chat, 500, false)
                .unwrap_err(),
            ReceiptValidationError::StatusMismatch
        );
    }

    #[test]
    fn partial_dropped_and_forged_complete_records_never_become_owned_evidence() {
        for (pointer, change) in [
            ("/routing_receipt/capture_complete", json!(false)),
            ("/routing_receipt/capture_truncated", json!(true)),
            ("/routing_receipt/body_state", json!("dropped")),
            ("/routing_receipt/body_state", json!("error")),
            ("/routing_receipt/phase", json!("streaming")),
            ("/routing_receipt/selected_attempt", Value::Null),
            ("/routing_receipt/selected_attempt", json!(0)),
            ("/routing_receipt/selected_attempt", json!(2)),
            ("/routing_receipt/attempts/0/send_started", json!(false)),
            ("/routing_receipt/attempts/0/http_status", json!(503)),
            (
                "/routing_receipt/attempts/0/transport_state",
                json!("headers_received"),
            ),
            (
                "/routing_receipt/attempts/0/transport_state",
                json!("failed"),
            ),
            (
                "/routing_receipt/attempts/0/error_code",
                json!("provider_error"),
            ),
            (
                "/routing_receipt/attempts/0/identity/provider_ref",
                Value::Null,
            ),
            ("/routing_receipt/attempts/0/protocol", json!("unknown")),
        ] {
            let mut value = receipt(AagSurface::Chat, false);
            *value.pointer_mut(pointer).unwrap() = change;
            assert!(
                successful(value, AagSurface::Chat, false).is_err(),
                "accepted incomplete {pointer}"
            );
        }
        let mut value = receipt(AagSurface::Chat, false);
        value["routing_receipt"]["attempts"] = json!([]);
        assert!(successful(value, AagSurface::Chat, false).is_err());
    }

    #[test]
    fn wire_attempt_sequence_bounds_and_safe_metadata_are_checked() {
        for (pointer, change) in [
            ("/routing_receipt/attempts/0/index", json!(2)),
            ("/routing_receipt/attempts/0/hop_index", json!(-1)),
            (
                "/routing_receipt/attempts/0/subattempt",
                json!(4294967296_u64),
            ),
            ("/routing_receipt/request_id", json!("sk-excluded")),
            (
                "/routing_receipt/attempts/0/identity/provider_ref",
                json!("https://private.invalid"),
            ),
            (
                "/routing_receipt/attempts/0/wire_model",
                json!("/private/model"),
            ),
            (
                "/routing_receipt/attempts/0/provider_model",
                json!("127.0.0.1"),
            ),
            (
                "/routing_receipt/attempts/0/error_code",
                json!("arbitrary upstream text"),
            ),
            ("/routing_receipt/issued_at_ms", json!(MAX_JSON_INTEGER + 1)),
            ("/routing_receipt/expires_at_ms", json!(0)),
        ] {
            let mut value = receipt(AagSurface::Chat, false);
            *value.pointer_mut(pointer).unwrap() = change;
            assert!(decode(value).is_err(), "accepted unsafe metadata {pointer}");
        }
        let mut value = receipt(AagSurface::Chat, false);
        let attempt = value["routing_receipt"]["attempts"][0].clone();
        value["routing_receipt"]["attempts"] = json!(vec![attempt; MAX_ATTEMPTS + 1]);
        assert!(decode(value).is_err());
        assert_eq!(
            AagBoundReceipt::parse(&vec![b' '; MAX_AAG_RECEIPT_BYTES + 1]).unwrap_err(),
            ReceiptValidationError::TooLarge
        );
    }

    #[test]
    fn absent_rejected_and_conflicting_provider_models_are_preserved_without_alias_fallback() {
        for (state, source) in [
            ("missing", Value::Null),
            ("rejected", json!("model")),
            ("conflicting", json!("model")),
        ] {
            let mut value = receipt(AagSurface::Chat, false);
            value["routing_receipt"]["attempts"][0]["provider_model"] = Value::Null;
            value["routing_receipt"]["attempts"][0]["provider_model_state"] = json!(state);
            value["routing_receipt"]["attempts"][0]["provider_model_source"] = source;
            let evidence = successful(value, AagSurface::Chat, false).unwrap();
            assert_eq!(evidence.provider_reported_model(), None);
            assert_eq!(evidence.wire_model(), "vendor/wire-model");
            assert_ne!(evidence.wire_model(), SELECTOR);
            assert_eq!(
                evidence.upstream_model_attestation(),
                AttestationState::Unavailable
            );
        }
        let mut invalid = receipt(AagSurface::Chat, false);
        invalid["routing_receipt"]["attempts"][0]["provider_model_state"] = json!("conflicting");
        assert!(decode(invalid).is_err());
    }

    #[test]
    fn stream_success_requires_protocol_appropriate_nonduplicated_terminal_signals() {
        for surface in [AagSurface::Chat, AagSurface::Messages] {
            assert!(successful(receipt(surface, true), surface, true).is_ok());
            assert!(successful(receipt(surface, true), surface, false).is_err());
            assert!(successful(receipt(surface, false), surface, true).is_err());
        }
        for signals in [
            json!([]),
            json!(["openai_done", "openai_done"]),
            json!(["anthropic_message_stop"]),
            json!(["openai_done", "anthropic_message_stop"]),
            json!(["responses_failed"]),
        ] {
            let mut value = receipt(AagSurface::Chat, true);
            value["routing_receipt"]["attempts"][0]["terminal_signals"] = signals;
            assert!(successful(value, AagSurface::Chat, true).is_err());
        }
        let mut messages = receipt(AagSurface::Messages, false);
        messages["routing_receipt"]["attempts"][0]["credential_mode"] = json!("client_passthrough");
        assert_eq!(
            successful(messages, AagSurface::Messages, false).unwrap_err(),
            ReceiptValidationError::MetadataInvalid
        );
    }

    #[test]
    fn serialized_owned_evidence_cannot_bypass_success_validation() {
        let evidence =
            successful(receipt(AagSurface::Chat, false), AagSurface::Chat, false).unwrap();
        let mut stored = serde_json::to_value(evidence).unwrap();
        stored["receipt"]["routing_receipt"]["capture_complete"] = json!(false);
        assert!(serde_json::from_value::<GatewayObservedRouting>(stored).is_err());
        assert_eq!(
            ReceiptValidationError::AdmissionMismatch.to_string(),
            ReceiptValidationError::AdmissionMismatch.code()
        );
    }
}
