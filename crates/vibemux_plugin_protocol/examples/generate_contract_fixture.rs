use vibemux_plugin_protocol::{
    PROTOCOL_MAJOR, PROTOCOL_MINOR,
    frame::{FrameCodecConfig, encode_frame},
    wire::{Envelope, Hello, PluginKind, envelope},
};

fn main() {
    let envelope = Envelope {
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
    };
    let frame = encode_frame(&envelope, FrameCodecConfig::default()).expect("encode fixture");
    for byte in frame {
        print!("{byte:02x}");
    }
    println!();
}
