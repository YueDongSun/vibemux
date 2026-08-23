use std::collections::BTreeSet;

use semver::Version;

use crate::{
    PROTOCOL_MAJOR, PROTOCOL_MINOR, PluginProtocolError,
    limits::{
        HARD_MAX_PLUGIN_FRAME_BYTES, HARD_MAXIMUM_IN_FLIGHT, MAXIMUM_HEARTBEAT_INTERVAL_MS,
        MINIMUM_HEARTBEAT_INTERVAL_MS,
    },
};

pub mod generated {
    include!(concat!(env!("OUT_DIR"), "/vibemux.plugin.v1.rs"));
}

pub use generated::*;

pub const MAX_IDENTIFIER_BYTES: usize = 128;
pub const MAX_POLICY_IDENTIFIER_BYTES: usize = 64;
pub const MAX_IDENTIFIER_COLLECTION_ITEMS: usize = 64;

pub fn validate_envelope(envelope: &Envelope) -> Result<(), PluginProtocolError> {
    if envelope.protocol_major != PROTOCOL_MAJOR {
        return Err(PluginProtocolError::UnsupportedMajor);
    }
    if envelope.protocol_minor > PROTOCOL_MINOR {
        return Err(PluginProtocolError::UnsupportedMinor);
    }
    validate_identifier(&envelope.message_id)?;
    validate_optional_identifier(envelope.correlation_id.as_deref())?;
    validate_optional_identifier(envelope.causation_id.as_deref())?;
    let body = envelope
        .body
        .as_ref()
        .ok_or(PluginProtocolError::MissingBody)?;
    validate_body(body)
}

pub(crate) fn validate_policy_identifiers(
    identifiers: &[String],
) -> Result<(), PluginProtocolError> {
    if identifiers.len() > MAX_IDENTIFIER_COLLECTION_ITEMS {
        return Err(PluginProtocolError::CollectionTooLarge);
    }
    let mut unique = BTreeSet::new();
    for identifier in identifiers {
        validate_policy_identifier(identifier)?;
        if !unique.insert(identifier.as_str()) {
            return Err(PluginProtocolError::DuplicateIdentifier);
        }
    }
    Ok(())
}

pub(crate) fn validate_policy_identifier(identifier: &str) -> Result<(), PluginProtocolError> {
    let bytes = identifier.as_bytes();
    if bytes.is_empty()
        || bytes.len() > MAX_POLICY_IDENTIFIER_BYTES
        || !bytes[0].is_ascii_lowercase()
        || !bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'.' | b':')
        })
    {
        return Err(PluginProtocolError::InvalidIdentifier);
    }
    Ok(())
}

pub(crate) fn validate_identifier(identifier: &str) -> Result<(), PluginProtocolError> {
    let bytes = identifier.as_bytes();
    if bytes.is_empty()
        || bytes.len() > MAX_IDENTIFIER_BYTES
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
    {
        return Err(PluginProtocolError::InvalidIdentifier);
    }
    Ok(())
}

fn validate_optional_identifier(identifier: Option<&str>) -> Result<(), PluginProtocolError> {
    if let Some(identifier) = identifier {
        validate_identifier(identifier)?;
    }
    Ok(())
}

fn validate_body(body: &envelope::Body) -> Result<(), PluginProtocolError> {
    match body {
        envelope::Body::Hello(hello) => {
            validate_policy_identifier(&hello.plugin_id)?;
            Version::parse(&hello.plugin_version)
                .map_err(|_| PluginProtocolError::InvalidMessage)?;
            if PluginKind::try_from(hello.kind)
                .ok()
                .is_none_or(|kind| kind == PluginKind::Unspecified)
                || hello.minimum_protocol_minor > hello.maximum_protocol_minor
            {
                return Err(PluginProtocolError::InvalidMessage);
            }
            validate_policy_identifiers(&hello.capabilities)?;
            validate_policy_identifiers(&hello.requested_permissions)
        }
        envelope::Body::CoreHello(core_hello) => {
            validate_identifier(&core_hello.session_id)?;
            if core_hello.accepted_protocol_minor > PROTOCOL_MINOR
                || core_hello.maximum_frame_bytes == 0
                || core_hello.maximum_frame_bytes as usize > HARD_MAX_PLUGIN_FRAME_BYTES
                || core_hello.maximum_in_flight == 0
                || core_hello.maximum_in_flight > HARD_MAXIMUM_IN_FLIGHT
                || !(MINIMUM_HEARTBEAT_INTERVAL_MS..=MAXIMUM_HEARTBEAT_INTERVAL_MS)
                    .contains(&core_hello.heartbeat_interval_ms)
            {
                return Err(PluginProtocolError::InvalidMessage);
            }
            validate_policy_identifiers(&core_hello.granted_capabilities)?;
            validate_policy_identifiers(&core_hello.granted_permissions)
        }
        envelope::Body::Ready(ready) => validate_identifier(&ready.session_id),
        envelope::Body::Request(request) => {
            validate_identifier(&request.request_id)?;
            validate_policy_identifier(&request.method)?;
            validate_optional_identifier(request.idempotency_key.as_deref())?;
            if request.deadline_unix_ms == 0 {
                return Err(PluginProtocolError::InvalidMessage);
            }
            Ok(())
        }
        envelope::Body::Response(response) => {
            validate_identifier(&response.request_id)?;
            let status = ResponseStatus::try_from(response.status)
                .map_err(|_| PluginProtocolError::InvalidMessage)?;
            if status == ResponseStatus::Unspecified {
                return Err(PluginProtocolError::InvalidMessage);
            }
            if status == ResponseStatus::Error {
                validate_policy_identifier(
                    response
                        .error_code
                        .as_deref()
                        .ok_or(PluginProtocolError::InvalidMessage)?,
                )?;
            } else if response.error_code.is_some() {
                return Err(PluginProtocolError::InvalidMessage);
            }
            Ok(())
        }
        envelope::Body::Event(event) => {
            validate_identifier(&event.event_id)?;
            validate_policy_identifier(&event.event_type)
        }
        envelope::Body::Heartbeat(heartbeat) => {
            if heartbeat.sequence == 0 {
                Err(PluginProtocolError::InvalidMessage)
            } else {
                Ok(())
            }
        }
        envelope::Body::Cancel(cancel) => {
            validate_identifier(&cancel.request_id)?;
            validate_policy_identifier(&cancel.reason_code)
        }
        envelope::Body::Drain(drain) => {
            if drain.deadline_unix_ms == 0 {
                Err(PluginProtocolError::InvalidMessage)
            } else {
                Ok(())
            }
        }
        envelope::Body::Shutdown(shutdown) => validate_policy_identifier(&shutdown.reason_code),
        envelope::Body::ProtocolError(error) => {
            validate_policy_identifier(&error.code)?;
            validate_optional_identifier(error.related_message_id.as_deref())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello_envelope() -> Envelope {
        Envelope {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            message_id: "message_1".to_string(),
            correlation_id: Some("correlation_1".to_string()),
            causation_id: None,
            body: Some(envelope::Body::Hello(Hello {
                plugin_id: "mock.harness".to_string(),
                plugin_version: "1.0.0".to_string(),
                kind: PluginKind::Harness.into(),
                capabilities: vec!["harness:run".to_string()],
                requested_permissions: vec!["workspace:read".to_string()],
                minimum_protocol_minor: 0,
                maximum_protocol_minor: 0,
            })),
        }
    }

    #[test]
    fn valid_hello_and_identity_round_trip_validate() {
        validate_envelope(&hello_envelope()).expect("valid hello");
    }

    #[test]
    fn version_body_and_duplicate_identifiers_fail_closed() {
        let mut envelope = hello_envelope();
        envelope.protocol_major += 1;
        assert_eq!(
            validate_envelope(&envelope).expect_err("major version must fail"),
            PluginProtocolError::UnsupportedMajor
        );

        let mut envelope = hello_envelope();
        envelope.body = None;
        assert_eq!(
            validate_envelope(&envelope).expect_err("missing body must fail"),
            PluginProtocolError::MissingBody
        );

        let mut envelope = hello_envelope();
        let Some(envelope::Body::Hello(hello)) = envelope.body.as_mut() else {
            panic!("hello body");
        };
        hello.capabilities.push("harness:run".to_string());
        assert_eq!(
            validate_envelope(&envelope).expect_err("duplicate capability must fail"),
            PluginProtocolError::DuplicateIdentifier
        );
    }
}
