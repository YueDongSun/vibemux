use std::time::Duration;

use tokio::io::{AsyncWriteExt, stderr, stdin, stdout};
use vibemux_plugin_protocol::{
    PROTOCOL_MAJOR, PROTOCOL_MINOR,
    frame::{FrameCodecConfig, read_envelope, write_envelope},
    wire::{
        Envelope, Event, Heartbeat, Hello, PluginKind, Response, ResponseStatus, Shutdown, envelope,
    },
};

const SESSION_ENVIRONMENT_KEY: &str = "VIBEMUX_PLUGIN_SESSION_ID";

#[tokio::main]
async fn main() {
    let mode = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "normal".to_string());
    if mode == "hang_handshake" {
        tokio::time::sleep(Duration::from_secs(60)).await;
        return;
    }
    if mode == "malformed" {
        let _ = stdout().write_all(b"not-protobuf").await;
        return;
    }
    if mode == "oversized" {
        let _ = stdout().write_all(&u32::MAX.to_be_bytes()).await;
        return;
    }

    let session_id = std::env::var(SESSION_ENVIRONMENT_KEY).unwrap_or_default();
    let config = FrameCodecConfig::default();
    let mut input = stdin();
    let mut output = stdout();
    let hello = envelope(
        "plugin_hello_1",
        None,
        envelope::Body::Hello(Hello {
            plugin_id: "mock.harness".to_string(),
            plugin_version: "1.0.0".to_string(),
            kind: PluginKind::Harness.into(),
            capabilities: vec!["mock:cancel".to_string(), "mock:echo".to_string()],
            requested_permissions: vec![],
            minimum_protocol_minor: 0,
            maximum_protocol_minor: 0,
        }),
    );
    if write_envelope(&mut output, &hello, config).await.is_err() {
        return;
    }
    let core_hello = match read_envelope(&mut input, config).await {
        Ok(core_hello) => core_hello,
        Err(_) => return,
    };
    let accepted_session = match core_hello.body.as_ref() {
        Some(envelope::Body::CoreHello(core_hello)) => core_hello.session_id.clone(),
        _ => return,
    };
    let ready_session = if mode == "wrong_ready" {
        "wrong_session".to_string()
    } else {
        accepted_session.clone()
    };
    let ready = envelope(
        "plugin_ready_1",
        Some(accepted_session.clone()),
        envelope::Body::Ready(vibemux_plugin_protocol::wire::Ready {
            session_id: ready_session,
        }),
    );
    if write_envelope(&mut output, &ready, config).await.is_err() {
        return;
    }
    if mode == "wrong_ready" {
        tokio::time::sleep(Duration::from_secs(1)).await;
        return;
    }
    if mode == "stderr_flood" {
        let mut diagnostics = stderr();
        let _ = diagnostics.write_all(&vec![b'x'; 128 * 1024]).await;
        let _ = diagnostics.flush().await;
    }
    if mode == "crash" {
        let mut diagnostics = stderr();
        let _ = diagnostics.write_all(b"mock crash diagnostic").await;
        let _ = diagnostics.flush().await;
        std::process::exit(7);
    }
    let heartbeat = envelope(
        "plugin_heartbeat_1",
        Some(session_id.clone()),
        envelope::Body::Heartbeat(Heartbeat { sequence: 1 }),
    );
    if write_envelope(&mut output, &heartbeat, config)
        .await
        .is_err()
    {
        return;
    }
    if mode == "duplicate_heartbeat" {
        let duplicate = envelope(
            "plugin_heartbeat_2",
            Some(session_id.clone()),
            envelope::Body::Heartbeat(Heartbeat { sequence: 1 }),
        );
        let _ = write_envelope(&mut output, &duplicate, config).await;
    }
    if mode == "quiet" {
        tokio::time::sleep(Duration::from_secs(60)).await;
        return;
    }

    let mut message_counter = 1_u64;
    loop {
        let incoming = match read_envelope(&mut input, config).await {
            Ok(incoming) => incoming,
            Err(_) => return,
        };
        message_counter = message_counter.saturating_add(1);
        match incoming.body {
            Some(envelope::Body::Request(request)) => {
                let payload = if request.method == "mock:environment" {
                    format!(
                        "allowed={};path_present={}",
                        std::env::var("ALLOWED_VALUE").unwrap_or_default(),
                        std::env::var_os("PATH").is_some()
                    )
                    .into_bytes()
                } else {
                    request.payload
                };
                let response = envelope(
                    &format!("plugin_response_{message_counter}"),
                    incoming.correlation_id,
                    envelope::Body::Response(Response {
                        request_id: request.request_id,
                        status: ResponseStatus::Ok.into(),
                        payload,
                        error_code: None,
                    }),
                );
                let _ = write_envelope(&mut output, &response, config).await;
            }
            Some(envelope::Body::Cancel(cancel)) => {
                let event = envelope(
                    &format!("plugin_event_{message_counter}"),
                    incoming.correlation_id,
                    envelope::Body::Event(Event {
                        event_id: format!("cancelled_{}", cancel.request_id),
                        event_type: "mock:cancelled".to_string(),
                        payload: cancel.reason_code.into_bytes(),
                    }),
                );
                let _ = write_envelope(&mut output, &event, config).await;
            }
            Some(envelope::Body::Drain(_)) => {}
            Some(envelope::Body::Shutdown(_)) => {
                if mode == "hang_shutdown" {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    return;
                }
                let acknowledgement = envelope(
                    &format!("plugin_shutdown_{message_counter}"),
                    incoming.correlation_id,
                    envelope::Body::Shutdown(Shutdown {
                        reason_code: "plugin:ack".to_string(),
                    }),
                );
                let _ = write_envelope(&mut output, &acknowledgement, config).await;
                return;
            }
            _ => {}
        }
    }
}

fn envelope(message_id: &str, correlation_id: Option<String>, body: envelope::Body) -> Envelope {
    Envelope {
        protocol_major: PROTOCOL_MAJOR,
        protocol_minor: PROTOCOL_MINOR,
        message_id: message_id.to_string(),
        correlation_id,
        causation_id: None,
        body: Some(body),
    }
}
