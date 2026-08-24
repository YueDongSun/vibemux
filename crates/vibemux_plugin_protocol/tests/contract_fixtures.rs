use vibemux_plugin_protocol::{
    PROTOCOL_MAJOR, PROTOCOL_MINOR,
    frame::{FrameCodecConfig, decode_frame, encode_frame},
    manifest::PluginManifest,
    wire::{Envelope, Hello, PluginKind, envelope},
};

const MANIFEST_FIXTURE: &str = include_str!("fixtures/plugin_manifest_v1.toml");
const HELLO_FIXTURE_HEX: &str = include_str!("fixtures/hello_envelope_v1.hex");

#[test]
fn checked_in_manifest_and_wire_fixture_match_canonical_contract() {
    let manifest = PluginManifest::from_toml(MANIFEST_FIXTURE).expect("manifest fixture");
    assert_eq!(manifest.plugin_id, "mock.harness");

    let expected = hello_envelope();
    let fixture = decode_hex(HELLO_FIXTURE_HEX.trim());
    assert_eq!(
        encode_frame(&expected, FrameCodecConfig::default()).expect("encode canonical fixture"),
        fixture
    );
    let decoded = decode_frame(&fixture, FrameCodecConfig::default()).expect("decode fixture");
    assert_eq!(decoded.message_id, "hello_fixture_1");
    assert_eq!(
        decoded.correlation_id.as_deref(),
        Some("correlation_fixture_1")
    );
    assert_eq!(decoded, expected);
}

fn hello_envelope() -> Envelope {
    Envelope {
        protocol_major: PROTOCOL_MAJOR,
        protocol_minor: PROTOCOL_MINOR,
        message_id: "hello_fixture_1".to_string(),
        correlation_id: Some("correlation_fixture_1".to_string()),
        causation_id: None,
        body: Some(envelope::Body::Hello(Hello {
            plugin_id: "mock.harness".to_string(),
            plugin_version: "1.0.0".to_string(),
            kind: PluginKind::Harness.into(),
            capabilities: vec!["harness:cancel".to_string(), "harness:run".to_string()],
            requested_permissions: vec!["workspace:read".to_string()],
            minimum_protocol_minor: 0,
            maximum_protocol_minor: 0,
        })),
    }
}

fn decode_hex(encoded: &str) -> Vec<u8> {
    assert_eq!(encoded.len() % 2, 0, "hex fixture length");
    encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).expect("ASCII hex pair");
            u8::from_str_radix(pair, 16).expect("valid hex pair")
        })
        .collect()
}
