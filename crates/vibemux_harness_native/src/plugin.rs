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
        Envelope, Event, Heartbeat, Hello, PluginKind, Ready, Request as WireRequest, Response,
        ResponseStatus, Shutdown, envelope,
    },
};

use crate::{
    Error,
    config::Config,
    contract::{
        Completion, EXECUTE_METHOD, Harness, MAX_REQUEST_BYTES, PROBE_METHOD, RECORD_EVENT, Record,
        Request, SCHEMA_VERSION, Status,
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
                let admitted = admit(
                    &wire_request,
                    correlation_id.as_deref(),
                    &core.granted_capabilities,
                    harness,
                    used,
                    unix_ms()?,
                );
                let (request, remaining) = match admitted {
                    Ok(admitted) => admitted,
                    Err(code) => {
                        send(
                            &mut output,
                            codec,
                            &mut message_sequence,
                            correlation_id,
                            envelope::Body::Response(Response {
                                request_id: wire_request.request_id,
                                status: ResponseStatus::Error.into(),
                                payload: vec![],
                                error_code: Some(code.to_string()),
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

/// Decides, before any vendor process exists, whether this plugin runs a
/// core request. It refuses with a fixed code, in this order, when the
/// session already ran a request, the method is not a granted harness
/// method, under 100 ms remain before the deadline, or the payload is over
/// `MAX_REQUEST_BYTES`. Otherwise the payload must be a request for this
/// plugin's harness whose UUID matches the wire request and its
/// correlation; only a probe skips the prompt check, because it sends no
/// prompt. Returns the request and the milliseconds left before its deadline.
fn admit(
    wire_request: &WireRequest,
    correlation_id: Option<&str>,
    granted_capabilities: &[String],
    harness: Harness,
    used: bool,
    now_unix_ms: u64,
) -> Result<(Request, u64), &'static str> {
    let probe = wire_request.method == PROBE_METHOD;
    let request: Result<Request, _> = serde_json::from_slice(&wire_request.payload);
    let remaining = wire_request.deadline_unix_ms.saturating_sub(now_unix_ms);
    let rejection = if used {
        Some("harness_session_used")
    } else if !granted_capabilities.contains(&wire_request.method)
        || !matches!(wire_request.method.as_str(), EXECUTE_METHOD | PROBE_METHOD)
    {
        Some("harness_capability_denied")
    } else if remaining < 100 {
        Some("harness_deadline_exceeded")
    } else if wire_request.payload.len() > MAX_REQUEST_BYTES {
        Some("harness_request_too_large")
    } else {
        None
    };
    let request = match (rejection, request) {
        (None, Ok(request))
            if request.schema_version == SCHEMA_VERSION
                && !request.request_id.is_nil()
                && request.request_id.to_string() == wire_request.request_id
                && request.harness == harness
                && correlation_id == Some(wire_request.request_id.as_str())
                && (probe || request.validate().is_ok()) =>
        {
            request
        }
        (reason, _) => return Err(reason.unwrap_or("harness_invalid_request")),
    };
    Ok((request, remaining))
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

#[cfg(test)]
mod tests {
    use uuid::Uuid;
    use vibemux_plugin_protocol::frame::DEFAULT_MAX_PLUGIN_FRAME_BYTES;

    use super::*;

    const NOW: u64 = 1_000_000;

    struct Case {
        wire: WireRequest,
        correlation_id: Option<String>,
        granted: Vec<String>,
        harness: Harness,
        used: bool,
    }

    type CaseEdit = fn(&mut Case);

    fn request(prompt: &str) -> Request {
        Request {
            schema_version: SCHEMA_VERSION,
            request_id: Uuid::new_v4(),
            harness: Harness::Codex,
            prompt: prompt.to_string(),
        }
    }

    fn case(method: &str, request: &Request) -> Case {
        let wire = WireRequest {
            request_id: request.request_id.to_string(),
            method: method.to_string(),
            payload: serde_json::to_vec(request).unwrap(),
            deadline_unix_ms: NOW + 1_000,
            idempotency_key: None,
        };
        Case {
            correlation_id: Some(wire.request_id.clone()),
            wire,
            granted: vec![EXECUTE_METHOD.to_string(), PROBE_METHOD.to_string()],
            harness: Harness::Codex,
            used: false,
        }
    }

    fn admitted(case: &Case) -> Result<(Request, u64), &'static str> {
        admit(
            &case.wire,
            case.correlation_id.as_deref(),
            &case.granted,
            case.harness,
            case.used,
            NOW,
        )
    }

    fn outcome(case: &Case) -> Result<u64, &'static str> {
        admitted(case).map(|(_, remaining)| remaining)
    }

    fn rewrite(case: &mut Case, edit: impl FnOnce(&mut Request)) {
        let mut changed: Request = serde_json::from_slice(&case.wire.payload).unwrap();
        edit(&mut changed);
        case.wire.payload = serde_json::to_vec(&changed).unwrap();
        case.wire.request_id = changed.request_id.to_string();
        case.correlation_id = Some(case.wire.request_id.clone());
    }

    /// A misbehaving core cannot make the plugin run a second request, an
    /// ungranted or unknown method, an expiring or oversized request, or a
    /// payload that does not match the wire request, its correlation, and
    /// this plugin's harness.
    #[test]
    fn the_plugin_admits_only_a_first_granted_timely_matching_request() {
        let execute = request("hello");
        let (returned, remaining) = admitted(&case(EXECUTE_METHOD, &execute)).unwrap();
        assert_eq!(
            (returned.request_id, remaining),
            (execute.request_id, 1_000)
        );
        // Only a probe skips the prompt check.
        let blank = request(" ");
        assert_eq!(outcome(&case(PROBE_METHOD, &blank)), Ok(1_000));
        assert_eq!(
            outcome(&case(EXECUTE_METHOD, &blank)),
            Err("harness_invalid_request")
        );
        let edits: [(&str, CaseEdit); 14] = [
            ("harness_session_used", |case| case.used = true),
            ("harness_capability_denied", |case| {
                case.granted = vec![PROBE_METHOD.to_string()];
            }),
            ("harness_capability_denied", |case| {
                case.wire.method = "harness:shell".to_string();
                case.granted.push("harness:shell".to_string());
            }),
            ("harness_deadline_exceeded", |case| {
                case.wire.deadline_unix_ms = NOW + 99;
            }),
            ("harness_deadline_exceeded", |case| {
                case.wire.deadline_unix_ms = NOW - 1;
            }),
            ("harness_request_too_large", |case| {
                case.wire.payload.resize(MAX_REQUEST_BYTES + 1, b' ');
            }),
            ("harness_invalid_request", |case| {
                case.wire.payload = b"{".to_vec();
            }),
            ("harness_invalid_request", |case| {
                case.wire.request_id = Uuid::new_v4().to_string();
            }),
            ("harness_invalid_request", |case| case.correlation_id = None),
            ("harness_invalid_request", |case| {
                case.correlation_id = Some(Uuid::new_v4().to_string());
            }),
            ("harness_invalid_request", |case| {
                case.harness = Harness::Claude;
            }),
            // An earlier check wins over a later one.
            ("harness_session_used", |case| {
                case.used = true;
                case.granted.clear();
            }),
            ("harness_capability_denied", |case| {
                case.granted.clear();
                case.wire.deadline_unix_ms = 0;
            }),
            ("harness_deadline_exceeded", |case| {
                case.wire.deadline_unix_ms = 0;
                case.wire.payload.resize(MAX_REQUEST_BYTES + 1, b' ');
            }),
        ];
        for (index, (code, edit)) in edits.into_iter().enumerate() {
            let mut checked = case(EXECUTE_METHOD, &execute);
            edit(&mut checked);
            assert_eq!(outcome(&checked), Err(code), "{index}");
        }
        // A probe skips `Request::validate`, so the plugin checks the
        // schema version and the nil UUID itself.
        let probe_edits: [CaseEdit; 2] = [
            |case| rewrite(case, |request| request.schema_version = SCHEMA_VERSION + 1),
            |case| rewrite(case, |request| request.request_id = Uuid::nil()),
        ];
        for edit in probe_edits {
            let mut checked = case(PROBE_METHOD, &blank);
            edit(&mut checked);
            assert_eq!(outcome(&checked), Err("harness_invalid_request"));
        }
        // 100 ms left and a payload of exactly the bound are admitted; JSON
        // allows the trailing whitespace that pads it.
        let mut edge = case(EXECUTE_METHOD, &execute);
        edge.wire.deadline_unix_ms = NOW + 100;
        edge.wire.payload.resize(MAX_REQUEST_BYTES, b' ');
        assert_eq!(outcome(&edge), Ok(100));
    }

    /// Completed and probed requests answer `Ok`, cancelled ones
    /// `Cancelled`, and failed or unverified ones `Error` with a code even
    /// when the completion has none. The payload is the completion as is.
    #[test]
    fn completions_map_to_wire_statuses_and_failures_always_carry_a_code() {
        let completion = |status, error_code: Option<&str>| Completion {
            schema_version: SCHEMA_VERSION,
            status,
            record_count: 2,
            stdout_bytes: 10,
            stderr_bytes: 0,
            exit_code: Some(0),
            forced_termination: false,
            error_code: error_code.map(str::to_string),
        };
        for (status, code, wire_status, wire_code) in [
            (Status::Completed, None, ResponseStatus::Ok, None),
            (Status::Probed, None, ResponseStatus::Ok, None),
            (
                Status::Cancelled,
                Some("harness_cancelled"),
                ResponseStatus::Cancelled,
                Some("harness_cancelled"),
            ),
            (
                Status::Failed,
                None,
                ResponseStatus::Error,
                Some("harness_vendor_failed"),
            ),
            (
                Status::Failed,
                Some("harness_deadline_exceeded"),
                ResponseStatus::Error,
                Some("harness_deadline_exceeded"),
            ),
            (
                Status::Unverified,
                None,
                ResponseStatus::Error,
                Some("harness_vendor_failed"),
            ),
        ] {
            let (mapped, payload, mapped_code) =
                completion_response(completion(status, code)).unwrap();
            assert_eq!(
                (mapped, mapped_code.as_deref()),
                (wire_status, wire_code),
                "{status:?}"
            );
            let decoded: Completion = serde_json::from_slice(&payload).unwrap();
            assert_eq!(
                (
                    decoded.status,
                    decoded.error_code.as_deref(),
                    decoded.record_count
                ),
                (status, code, 2)
            );
        }
    }

    /// The largest request the plugin admits fits the default plugin frame,
    /// which the dispatcher's plugin policy uses.
    #[tokio::test]
    async fn the_largest_admitted_request_fits_the_default_plugin_frame() {
        let fits = |payload_bytes: usize| async move {
            let request_id = Uuid::new_v4().to_string();
            let wire = WireRequest {
                request_id: request_id.clone(),
                method: EXECUTE_METHOD.to_string(),
                payload: vec![b' '; payload_bytes],
                deadline_unix_ms: u64::MAX,
                idempotency_key: Some(request_id.clone()),
            };
            let request = envelope(
                "harness_request".to_string(),
                Some(request_id),
                envelope::Body::Request(wire),
            );
            let mut buffer = Vec::new();
            write_envelope(&mut buffer, &request, FrameCodecConfig::default())
                .await
                .is_ok()
        };
        assert!(fits(MAX_REQUEST_BYTES).await);
        assert!(!fits(DEFAULT_MAX_PLUGIN_FRAME_BYTES).await);
    }
}
