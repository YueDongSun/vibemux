#![forbid(unsafe_code)]
//! Versioned canonical event envelopes independent of storage and transport implementations.

use serde::{Deserialize, Serialize, de};
use serde_json::Value;
use thiserror::Error;
use time::OffsetDateTime;
use vibemux_types::{EventId, ProjectId, RunId, TaskId};

pub const EVENT_SCHEMA_VERSION: u16 = 1;
pub const MAX_EVENT_TYPE_BYTES: usize = 128;
pub const MAX_ACTOR_BYTES: usize = 128;
pub const MAX_IDEMPOTENCY_KEY_BYTES: usize = 256;
pub const MAX_EVENT_PAYLOAD_BYTES: usize = 64 * 1024;
pub const MAX_EVENT_ENVELOPE_BYTES: usize = 96 * 1024;

const FORBIDDEN_PAYLOAD_KEYS: [&str; 9] = [
    "authorization",
    "content",
    "cookie",
    "environment",
    "message",
    "password",
    "prompt",
    "secret",
    "token",
];

/// Stable validation and encoding failures for canonical events.
#[derive(Debug, Error)]
pub enum EventError {
    #[error("event sequence must be greater than zero")]
    InvalidSequence,
    #[error("event type must be lowercase snake_case and at most {MAX_EVENT_TYPE_BYTES} bytes")]
    InvalidEventType,
    #[error("actor must be lowercase snake_case and at most {MAX_ACTOR_BYTES} bytes")]
    InvalidActor,
    #[error("idempotency key must not exceed {MAX_IDEMPOTENCY_KEY_BYTES} bytes")]
    InvalidIdempotencyKey,
    #[error("event payload exceeds {MAX_EVENT_PAYLOAD_BYTES} bytes")]
    PayloadTooLarge,
    #[error("event envelope exceeds {MAX_EVENT_ENVELOPE_BYTES} bytes")]
    EnvelopeTooLarge,
    #[error("event payload contains forbidden plaintext field: {0}")]
    ForbiddenPayloadKey(String),
    #[error("unsupported event schema version: {0}")]
    UnsupportedSchemaVersion(u16),
    #[error("invalid event JSON")]
    InvalidJson(#[source] serde_json::Error),
}

impl EventError {
    /// Stable machine-readable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidSequence => "invalid_event_sequence",
            Self::InvalidEventType => "invalid_event_type",
            Self::InvalidActor => "invalid_event_actor",
            Self::InvalidIdempotencyKey => "invalid_idempotency_key",
            Self::PayloadTooLarge => "event_payload_too_large",
            Self::EnvelopeTooLarge => "event_envelope_too_large",
            Self::ForbiddenPayloadKey(_) => "forbidden_event_payload_key",
            Self::UnsupportedSchemaVersion(_) => "unsupported_event_schema_version",
            Self::InvalidJson(_) => "invalid_event_json",
        }
    }
}

/// Store-owned, monotonically increasing event sequence.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct EventSequence(u64);

impl EventSequence {
    pub fn new(value: u64) -> Result<Self, EventError> {
        if value == 0 {
            return Err(EventError::InvalidSequence);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for EventSequence {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = u64::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Validated backend-neutral event name.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct EventType(String);

impl EventType {
    pub fn new(value: impl Into<String>) -> Result<Self, EventError> {
        let value = value.into();
        if !is_snake_case_name(&value, MAX_EVENT_TYPE_BYTES) {
            return Err(EventError::InvalidEventType);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for EventType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Validated event actor name. Identity details belong in policy-owned references.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ActorName(String);

impl ActorName {
    pub fn new(value: impl Into<String>) -> Result<Self, EventError> {
        let value = value.into();
        if !is_snake_case_name(&value, MAX_ACTOR_BYTES) {
            return Err(EventError::InvalidActor);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ActorName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Sanitized event payload. Plain prompt, message, credential, and environment fields are rejected.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct EventPayload(Value);

impl EventPayload {
    pub fn new(value: Value) -> Result<Self, EventError> {
        validate_payload_value(&value)?;
        let encoded = serde_json::to_vec(&value).map_err(EventError::InvalidJson)?;
        if encoded.len() > MAX_EVENT_PAYLOAD_BYTES {
            return Err(EventError::PayloadTooLarge);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn value(&self) -> &Value {
        &self.0
    }

    fn validate(&self) -> Result<(), EventError> {
        Self::new(self.0.clone()).map(|_| ())
    }
}

impl<'de> Deserialize<'de> for EventPayload {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Event data before the authoritative store allocates a sequence.
#[derive(Clone, Debug)]
pub struct EventDraft {
    pub event_id: EventId,
    pub event_type: EventType,
    pub project_id: ProjectId,
    pub task_id: Option<TaskId>,
    pub run_id: Option<RunId>,
    pub causation_id: Option<EventId>,
    pub actor: ActorName,
    pub timestamp: OffsetDateTime,
    pub idempotency_key: Option<String>,
    pub payload: EventPayload,
}

impl EventDraft {
    pub fn validate(&self) -> Result<(), EventError> {
        validate_idempotency_key(self.idempotency_key.as_deref())?;
        self.payload.validate()
    }
}

/// Immutable canonical event committed by the authoritative store.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EventEnvelope {
    schema_version: u16,
    event_id: EventId,
    sequence: EventSequence,
    event_type: EventType,
    project_id: ProjectId,
    task_id: Option<TaskId>,
    run_id: Option<RunId>,
    causation_id: Option<EventId>,
    actor: ActorName,
    #[serde(with = "time::serde::rfc3339")]
    timestamp: OffsetDateTime,
    idempotency_key: Option<String>,
    payload: EventPayload,
}

#[derive(Deserialize)]
struct DecodedEventEnvelope {
    schema_version: u16,
    event_id: EventId,
    sequence: EventSequence,
    event_type: EventType,
    project_id: ProjectId,
    task_id: Option<TaskId>,
    run_id: Option<RunId>,
    causation_id: Option<EventId>,
    actor: ActorName,
    #[serde(with = "time::serde::rfc3339")]
    timestamp: OffsetDateTime,
    idempotency_key: Option<String>,
    payload: EventPayload,
}

impl EventEnvelope {
    pub fn commit(draft: EventDraft, sequence: EventSequence) -> Result<Self, EventError> {
        draft.validate()?;
        Ok(Self {
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: draft.event_id,
            sequence,
            event_type: draft.event_type,
            project_id: draft.project_id,
            task_id: draft.task_id,
            run_id: draft.run_id,
            causation_id: draft.causation_id,
            actor: draft.actor,
            timestamp: draft.timestamp,
            idempotency_key: draft.idempotency_key,
            payload: draft.payload,
        })
    }

    pub fn from_json_slice(input: &[u8]) -> Result<Self, EventError> {
        if input.len() > MAX_EVENT_ENVELOPE_BYTES {
            return Err(EventError::EnvelopeTooLarge);
        }
        let decoded: Value = serde_json::from_slice(input).map_err(EventError::InvalidJson)?;
        if decoded.get("sequence").and_then(Value::as_u64) == Some(0) {
            return Err(EventError::InvalidSequence);
        }
        let decoded: DecodedEventEnvelope =
            serde_json::from_value(decoded).map_err(EventError::InvalidJson)?;
        let event = Self {
            schema_version: decoded.schema_version,
            event_id: decoded.event_id,
            sequence: decoded.sequence,
            event_type: decoded.event_type,
            project_id: decoded.project_id,
            task_id: decoded.task_id,
            run_id: decoded.run_id,
            causation_id: decoded.causation_id,
            actor: decoded.actor,
            timestamp: decoded.timestamp,
            idempotency_key: decoded.idempotency_key,
            payload: decoded.payload,
        };
        event.validate()?;
        Ok(event)
    }

    pub fn to_json_vec(&self) -> Result<Vec<u8>, EventError> {
        self.validate()?;
        let encoded = serde_json::to_vec(self).map_err(EventError::InvalidJson)?;
        if encoded.len() > MAX_EVENT_ENVELOPE_BYTES {
            return Err(EventError::EnvelopeTooLarge);
        }
        Ok(encoded)
    }

    pub fn validate(&self) -> Result<(), EventError> {
        if self.schema_version != EVENT_SCHEMA_VERSION {
            return Err(EventError::UnsupportedSchemaVersion(self.schema_version));
        }
        if self.sequence.get() == 0 {
            return Err(EventError::InvalidSequence);
        }
        if !is_snake_case_name(self.event_type.as_str(), MAX_EVENT_TYPE_BYTES) {
            return Err(EventError::InvalidEventType);
        }
        if !is_snake_case_name(self.actor.as_str(), MAX_ACTOR_BYTES) {
            return Err(EventError::InvalidActor);
        }
        validate_idempotency_key(self.idempotency_key.as_deref())?;
        self.payload.validate()
    }

    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    #[must_use]
    pub const fn event_id(&self) -> EventId {
        self.event_id
    }

    #[must_use]
    pub const fn sequence(&self) -> EventSequence {
        self.sequence
    }

    #[must_use]
    pub const fn event_type(&self) -> &EventType {
        &self.event_type
    }

    #[must_use]
    pub const fn project_id(&self) -> ProjectId {
        self.project_id
    }

    #[must_use]
    pub const fn task_id(&self) -> Option<TaskId> {
        self.task_id
    }

    #[must_use]
    pub const fn run_id(&self) -> Option<RunId> {
        self.run_id
    }

    #[must_use]
    pub const fn causation_id(&self) -> Option<EventId> {
        self.causation_id
    }

    #[must_use]
    pub const fn actor(&self) -> &ActorName {
        &self.actor
    }

    #[must_use]
    pub const fn timestamp(&self) -> OffsetDateTime {
        self.timestamp
    }

    #[must_use]
    pub fn idempotency_key(&self) -> Option<&str> {
        self.idempotency_key.as_deref()
    }

    #[must_use]
    pub const fn payload(&self) -> &EventPayload {
        &self.payload
    }
}

fn is_snake_case_name(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        && value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
        && !value.contains("__")
}

fn validate_idempotency_key(value: Option<&str>) -> Result<(), EventError> {
    if value.is_some_and(|key| key.is_empty() || key.len() > MAX_IDEMPOTENCY_KEY_BYTES) {
        return Err(EventError::InvalidIdempotencyKey);
    }
    Ok(())
}

fn validate_payload_value(value: &Value) -> Result<(), EventError> {
    match value {
        Value::Array(items) => {
            for item in items {
                validate_payload_value(item)?;
            }
        }
        Value::Object(fields) => {
            for (key, field_value) in fields {
                if FORBIDDEN_PAYLOAD_KEYS
                    .iter()
                    .any(|forbidden| key.eq_ignore_ascii_case(forbidden))
                {
                    return Err(EventError::ForbiddenPayloadKey(key.clone()));
                }
                validate_payload_value(field_value)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use serde_json::json;

    use super::*;

    const EVENT_FIXTURE: &str =
        include_str!("../../../tests/fixtures/python_reference/event_envelope_v1.json");

    #[test]
    fn event_fixture_round_trips_without_contract_drift() {
        let event = EventEnvelope::from_json_slice(EVENT_FIXTURE.as_bytes())
            .expect("checked-in event fixture must be valid");
        let actual: Value = serde_json::from_slice(&event.to_json_vec().expect("serialize event"))
            .expect("serialized event is JSON");
        let expected: Value = serde_json::from_str(EVENT_FIXTURE).expect("fixture is JSON");
        assert_eq!(actual, expected);
    }

    #[test]
    fn plaintext_message_fields_are_rejected_recursively() {
        let result = EventPayload::new(json!({"safe": {"message": "do not persist"}}));
        assert!(matches!(result, Err(EventError::ForbiddenPayloadKey(key)) if key == "message"));
    }

    #[test]
    fn digest_and_size_metadata_are_allowed() {
        let payload = EventPayload::new(json!({
            "message_sha256": "906055e56391a9362ff2e354e21a9e0ded69135ecadbea28eabcdf931686acbd",
            "message_bytes": 4
        }));
        assert!(payload.is_ok());
    }

    #[test]
    fn deserialization_validates_store_owned_sequence() {
        let invalid = EVENT_FIXTURE.replace("\"sequence\": 1", "\"sequence\": 0");
        assert!(matches!(
            EventEnvelope::from_json_slice(invalid.as_bytes()),
            Err(EventError::InvalidSequence)
        ));
    }

    proptest! {
        #[test]
        fn event_type_accepts_only_declared_grammar(value in ".{0,160}") {
            if let Ok(event_type) = EventType::new(value) {
                prop_assert!(is_snake_case_name(event_type.as_str(), MAX_EVENT_TYPE_BYTES));
            }
        }
    }
}
