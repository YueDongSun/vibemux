use std::{
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tokio::{
    io::{stdin, stdout},
    sync::{mpsc, watch},
    task::JoinHandle,
    time::{interval, timeout},
};
use vibemux_plugin_protocol::{
    PROTOCOL_MAJOR, PROTOCOL_MINOR,
    frame::{FrameCodecConfig, read_envelope, write_envelope},
    wire::{
        Envelope, Event, Heartbeat, Hello, PluginKind, Ready, Response, ResponseStatus, Shutdown,
        envelope,
    },
};

use crate::{
    Error,
    config::Config,
    contract::{
        Completion, EXECUTE_METHOD, Harness, PROBE_METHOD, RECORD_EVENT, Record, Request, Status,
    },
};

pub const PLUGIN_ID: &str = "vibemux.harness";
const HANDSHAKE_SECONDS: u64 = 5;

struct Task<T> {
    handle: JoinHandle<T>,
}

impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

pub(crate) fn envelope(
    message_id: String,
    correlation_id: Option<String>,
    body: envelope::Body,
) -> Envelope {
    Envelope {
        protocol_major: PROTOCOL_MAJOR,
        protocol_minor: PROTOCOL_MINOR,
        message_id,
        correlation_id,
        causation_id: None,
        body: Some(body),
    }
}

/// Private VibeMux protocol endpoint. Only one request may run in this process.
pub async fn serve(
    config_path: &Path,
    harness: Harness,
    expected_digest: &str,
) -> Result<(), Error> {
    let (config, actual_digest) = Config::snapshot(config_path)?;
    if actual_digest != expected_digest {
        return Err(Error::new("harness_config_changed"));
    }
    let route = config.route(harness)?.clone();
    let mut codec = FrameCodecConfig::default();
    let mut input = stdin();
    let mut output = stdout();
    let mut capabilities = vec![PROBE_METHOD.to_string()];
    if route.allow_execution {
        capabilities.push(EXECUTE_METHOD.to_string());
    }
    let hello = envelope(
        "harness_hello".to_string(),
        None,
        envelope::Body::Hello(Hello {
            plugin_id: PLUGIN_ID.to_string(),
            plugin_version: "1.0.0".to_string(),
            kind: PluginKind::Harness.into(),
            capabilities,
            requested_permissions: vec![],
            minimum_protocol_minor: 0,
            maximum_protocol_minor: 0,
        }),
    );
    write_envelope(&mut output, &hello, codec)
        .await
        .map_err(|source| Error::with_source("harness_plugin_write_failed", source))?;
    let incoming = timeout(
        Duration::from_secs(HANDSHAKE_SECONDS),
        read_envelope(&mut input, codec),
    )
    .await
    .map_err(|_| Error::new("harness_handshake_timeout"))?
    .map_err(|source| Error::with_source("harness_handshake_failed", source))?;
    let Some(envelope::Body::CoreHello(core)) = incoming.body else {
        return Err(Error::new("harness_handshake_failed"));
    };
    if core.session_id != std::env::var("VIBEMUX_PLUGIN_SESSION_ID").unwrap_or_default()
        || core.heartbeat_interval_ms < 100
        || core.maximum_in_flight == 0
    {
        return Err(Error::new("harness_handshake_failed"));
    }
    codec = FrameCodecConfig::new(core.maximum_frame_bytes as usize)
        .map_err(|source| Error::with_source("harness_handshake_failed", source))?;
    let ready = envelope(
        "harness_ready".to_string(),
        Some(core.session_id.clone()),
        envelope::Body::Ready(Ready {
            session_id: core.session_id.clone(),
        }),
    );
    write_envelope(&mut output, &ready, codec)
        .await
        .map_err(|source| Error::with_source("harness_plugin_write_failed", source))?;
    let (incoming_sender, mut incoming_receiver) = mpsc::channel(config.limits.queue_capacity);
    let _reader = Task {
        handle: tokio::spawn(async move {
            loop {
                let incoming = read_envelope(&mut input, codec).await;
                let failed = incoming.is_err();
                if incoming_sender.send(incoming).await.is_err() || failed {
                    break;
                }
            }
        }),
    };
    let mut heartbeat = interval(Duration::from_millis(core.heartbeat_interval_ms));
    let mut heartbeat_sequence = 0_u64;
    let mut message_sequence = 0_u64;
    let mut used = false;

    loop {
        let incoming = tokio::select! {
            _ = heartbeat.tick(), if !used => {
                heartbeat_sequence += 1;
                send(&mut output, codec, &mut message_sequence, Some(core.session_id.clone()),
                    envelope::Body::Heartbeat(Heartbeat { sequence: heartbeat_sequence })).await?;
                continue;
            }
            incoming = incoming_receiver.recv() => incoming
                .ok_or_else(|| Error::new("harness_core_disconnected"))?
                .map_err(|source| Error::with_source("harness_core_disconnected", source))?,
        };
        match incoming.body {
            Some(envelope::Body::Request(wire_request)) => {
                let correlation_id = incoming.correlation_id;
                let probe = wire_request.method == PROBE_METHOD;
                let request: Result<Request, _> = serde_json::from_slice(&wire_request.payload);
                let remaining = wire_request.deadline_unix_ms.saturating_sub(unix_ms()?);
                let rejection = if used {
                    Some("harness_session_used")
                } else if !core.granted_capabilities.contains(&wire_request.method)
                    || !matches!(wire_request.method.as_str(), EXECUTE_METHOD | PROBE_METHOD)
                {
                    Some("harness_capability_denied")
                } else if remaining < 100 {
                    Some("harness_deadline_exceeded")
                } else if wire_request.payload.len() > crate::contract::MAX_PROMPT_BYTES * 6 + 4096
                {
                    Some("harness_request_too_large")
                } else {
                    None
                };
                let request = match (rejection, request) {
                    (None, Ok(request))
                        if request.schema_version == crate::contract::SCHEMA_VERSION
                            && !request.request_id.is_nil()
                            && request.request_id.to_string() == wire_request.request_id
                            && request.harness == harness
                            && correlation_id.as_deref()
                                == Some(wire_request.request_id.as_str())
                            && (probe || request.validate().is_ok()) =>
                    {
                        request
                    }
                    (reason, _) => {
                        send(
                            &mut output,
                            codec,
                            &mut message_sequence,
                            correlation_id,
                            envelope::Body::Response(Response {
                                request_id: wire_request.request_id,
                                status: ResponseStatus::Error.into(),
                                payload: vec![],
                                error_code: Some(
                                    reason.unwrap_or("harness_invalid_request").to_string(),
                                ),
                            }),
                        )
                        .await?;
                        continue;
                    }
                };
                used = true;
                let mut limits = config.limits.clone();
                limits.timeout_ms = limits.timeout_ms.min(remaining);
                let (record_sender, mut record_receiver) =
                    mpsc::channel::<Record>(limits.queue_capacity);
                let (cancel_sender, cancel_receiver) = watch::channel(false);
                let mut execution = Task {
                    handle: tokio::spawn(crate::native::run(
                        route.clone(),
                        limits,
                        request,
                        probe,
                        record_sender,
                        cancel_receiver,
                    )),
                };
                let mut records_open = true;
                let completion = loop {
                    tokio::select! {
                        record = record_receiver.recv(), if records_open => {
                            match record {
                                Some(record) => {
                                    let payload = serde_json::to_vec(&record)
                                        .map_err(|source| Error::with_source("harness_encode_failed", source))?;
                                    send(&mut output, codec, &mut message_sequence, correlation_id.clone(),
                                        envelope::Body::Event(Event {
                                            event_id: format!("record_{}", record.sequence),
                                            event_type: RECORD_EVENT.to_string(), payload,
                                        })).await?;
                                }
                                None => records_open = false,
                            }
                        }
                        result = &mut execution.handle, if !records_open => {
                            break result.map_err(|source| Error::with_source("harness_worker_failed", source))?;
                        }
                        _ = heartbeat.tick() => {
                            heartbeat_sequence += 1;
                            send(&mut output, codec, &mut message_sequence, Some(core.session_id.clone()),
                                envelope::Body::Heartbeat(Heartbeat { sequence: heartbeat_sequence })).await?;
                        }
                        incoming = incoming_receiver.recv() => {
                            let incoming = incoming.ok_or_else(|| Error::new("harness_core_disconnected"))?
                                .map_err(|source| Error::with_source("harness_core_disconnected", source))?;
                            match incoming.body {
                                Some(envelope::Body::Cancel(cancel)) if cancel.request_id == wire_request.request_id => {
                                    let _ = cancel_sender.send(true);
                                }
                                Some(envelope::Body::Request(other)) => {
                                    send(&mut output, codec, &mut message_sequence, incoming.correlation_id,
                                        envelope::Body::Response(Response { request_id: other.request_id,
                                            status: ResponseStatus::Overloaded.into(), payload: vec![],
                                            error_code: Some("harness_busy".to_string()) })).await?;
                                }
                                Some(envelope::Body::Shutdown(_)) | Some(envelope::Body::Drain(_)) => {
                                    let _ = cancel_sender.send(true);
                                }
                                _ => return Err(Error::new("harness_unexpected_core_message")),
                            }
                        }
                    }
                };
                let (status, payload, error_code) = match completion {
                    Ok(completion) => completion_response(completion)?,
                    Err(error) => (
                        ResponseStatus::Error,
                        vec![],
                        Some(error.code().to_string()),
                    ),
                };
                send(
                    &mut output,
                    codec,
                    &mut message_sequence,
                    correlation_id,
                    envelope::Body::Response(Response {
                        request_id: wire_request.request_id,
                        status: status.into(),
                        payload,
                        error_code,
                    }),
                )
                .await?;
            }
            Some(envelope::Body::Drain(_)) => {}
            Some(envelope::Body::Shutdown(_)) => {
                send(
                    &mut output,
                    codec,
                    &mut message_sequence,
                    incoming.correlation_id,
                    envelope::Body::Shutdown(Shutdown {
                        reason_code: "harness:shutdown".to_string(),
                    }),
                )
                .await?;
                return Ok(());
            }
            Some(envelope::Body::Cancel(_)) => {}
            _ => return Err(Error::new("harness_unexpected_core_message")),
        }
    }
}

async fn send(
    output: &mut tokio::io::Stdout,
    codec: FrameCodecConfig,
    sequence: &mut u64,
    correlation_id: Option<String>,
    body: envelope::Body,
) -> Result<(), Error> {
    *sequence += 1;
    timeout(
        Duration::from_secs(5),
        write_envelope(
            output,
            &envelope(format!("harness_{}", *sequence), correlation_id, body),
            codec,
        ),
    )
    .await
    .map_err(|_| Error::new("harness_plugin_write_timeout"))?
    .map_err(|source| Error::with_source("harness_plugin_write_failed", source))
}

fn completion_response(
    completion: Completion,
) -> Result<(ResponseStatus, Vec<u8>, Option<String>), Error> {
    let status = match completion.status {
        Status::Completed | Status::Probed => ResponseStatus::Ok,
        Status::Cancelled => ResponseStatus::Cancelled,
        Status::Failed | Status::Unverified => ResponseStatus::Error,
    };
    let error_code = if status == ResponseStatus::Error {
        Some(
            completion
                .error_code
                .clone()
                .unwrap_or_else(|| "harness_vendor_failed".to_string()),
        )
    } else {
        completion.error_code.clone()
    };
    let payload = serde_json::to_vec(&completion)
        .map_err(|source| Error::with_source("harness_encode_failed", source))?;
    Ok((status, payload, error_code))
}

pub(crate) fn unix_ms() -> Result<u64, Error> {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|source| Error::with_source("harness_clock_error", source))?
            .as_millis(),
    )
    .map_err(|_| Error::new("harness_clock_error"))
}
