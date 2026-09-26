#![forbid(unsafe_code)]

use prost::Message;
use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{stdin, stdout},
    task::JoinSet,
    time::{interval, timeout},
};
use vibemux_plugin_protocol::{
    PROTOCOL_MAJOR, PROTOCOL_MINOR,
    frame::{FrameCodecConfig, read_envelope, write_envelope},
    terminal::{
        FOCUS_METHOD, FOCUS_PERMISSION, INVENTORY_METHOD, MAX_TERMINAL_PAYLOAD_BYTES,
        OBSERVE_PERMISSION,
    },
    wire::{
        self, Envelope, Heartbeat, Hello, Ready, Response, ResponseStatus, Shutdown,
        TerminalFocusRequest, TerminalFocusResult, TerminalInventoryRequest, envelope,
    },
};
use vibemux_terminal_observer::{backend, config::ObserverConfig};

const PLUGIN_ID: &str = "wezterm_observer";
type PendingResponse = (String, Option<String>, Result<Vec<u8>, &'static str>);

#[tokio::main]
async fn main() {
    if let Err(code) = run().await {
        eprintln!("{code}");
        std::process::exit(4);
    }
}

async fn run() -> Result<(), &'static str> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() != 2 || arguments[0] != "--config" {
        return Err("terminal_invalid_arguments");
    }
    let config = ObserverConfig::load(&PathBuf::from(&arguments[1]))?;
    let codec = FrameCodecConfig::default();
    let mut input = stdin();
    let mut output = stdout();
    let mut counter = 0_u64;
    let hello = message(
        &mut counter,
        None,
        envelope::Body::Hello(Hello {
            plugin_id: PLUGIN_ID.into(),
            plugin_version: "1.0.0".into(),
            kind: wire::PluginKind::Terminal.into(),
            capabilities: vec![INVENTORY_METHOD.into(), FOCUS_METHOD.into()],
            requested_permissions: vec![
                OBSERVE_PERMISSION.into(),
                FOCUS_PERMISSION.into(),
                "process:execute".into(),
            ],
            minimum_protocol_minor: 0,
            maximum_protocol_minor: 0,
        }),
    );
    write_envelope(&mut output, &hello, codec)
        .await
        .map_err(|_| "terminal_protocol_error")?;
    let envelope = timeout(Duration::from_secs(5), read_envelope(&mut input, codec))
        .await
        .map_err(|_| "terminal_handshake_timeout")?
        .map_err(|_| "terminal_protocol_error")?;
    let Some(envelope::Body::CoreHello(core)) = envelope.body else {
        return Err("terminal_protocol_error");
    };
    if core.heartbeat_interval_ms == 0 || core.heartbeat_interval_ms > 60_000 {
        return Err("terminal_protocol_error");
    }
    let ready = message(
        &mut counter,
        Some(core.session_id.clone()),
        envelope::Body::Ready(Ready {
            session_id: core.session_id.clone(),
        }),
    );
    write_envelope(&mut output, &ready, codec)
        .await
        .map_err(|_| "terminal_protocol_error")?;
    let mut heartbeat = interval(Duration::from_millis(core.heartbeat_interval_ms));
    let mut heartbeat_sequence = 0_u64;
    let mut tasks: JoinSet<PendingResponse> = JoinSet::new();
    let mut draining = false;
    loop {
        // Keep the frame future alive across heartbeats and request completions.
        let incoming = {
            let read = read_envelope(&mut input, codec);
            tokio::pin!(read);
            loop {
                tokio::select! {
                    incoming = &mut read => break incoming.map_err(|_| "terminal_protocol_error")?,
                    _ = heartbeat.tick() => {
                        heartbeat_sequence = heartbeat_sequence.saturating_add(1);
                        let beat = message(&mut counter, Some(core.session_id.clone()), envelope::Body::Heartbeat(Heartbeat { sequence: heartbeat_sequence }));
                        write_envelope(&mut output, &beat, codec).await.map_err(|_| "terminal_protocol_error")?;
                    }
                    finished = tasks.join_next(), if !tasks.is_empty() => {
                        if let Some(Ok((request_id, correlation_id, result))) = finished {
                            let reply = response(&mut counter, request_id, correlation_id, result);
                            write_envelope(&mut output, &reply, codec).await.map_err(|_| "terminal_protocol_error")?;
                        }
                    }
                }
            }
        };
        match incoming.body {
            Some(envelope::Body::Request(request)) => {
                let permission = match request.method.as_str() {
                    INVENTORY_METHOD => OBSERVE_PERMISSION,
                    FOCUS_METHOD => FOCUS_PERMISSION,
                    _ => "",
                };
                let granted = !permission.is_empty()
                    && core.granted_capabilities.contains(&request.method)
                    && core
                        .granted_permissions
                        .iter()
                        .any(|value| value == permission)
                    && core
                        .granted_permissions
                        .iter()
                        .any(|value| value == "process:execute");
                if !granted
                    || draining
                    || !tasks.is_empty()
                    || request.payload.len() > MAX_TERMINAL_PAYLOAD_BYTES
                {
                    let reply = response(
                        &mut counter,
                        request.request_id,
                        incoming.correlation_id,
                        Err("terminal_request_denied"),
                    );
                    write_envelope(&mut output, &reply, codec)
                        .await
                        .map_err(|_| "terminal_protocol_error")?;
                    continue;
                }
                let config = config.clone();
                tasks.spawn(async move {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis();
                    let remaining = (request.deadline_unix_ms as u128)
                        .saturating_sub(now)
                        .min(3500) as u64;
                    let result = if remaining == 0 {
                        Err("terminal_request_timeout")
                    } else {
                        timeout(Duration::from_millis(remaining), execute(&config, &request))
                            .await
                            .unwrap_or(Err("terminal_request_timeout"))
                    };
                    (request.request_id, incoming.correlation_id, result)
                });
            }
            Some(envelope::Body::Cancel(_)) => {
                tasks.abort_all();
                while tasks.join_next().await.is_some() {}
            }
            Some(envelope::Body::Drain(_)) => draining = true,
            Some(envelope::Body::Shutdown(_)) => {
                tasks.abort_all();
                while tasks.join_next().await.is_some() {}
                let ack = message(
                    &mut counter,
                    incoming.correlation_id,
                    envelope::Body::Shutdown(Shutdown {
                        reason_code: "plugin:ack".into(),
                    }),
                );
                write_envelope(&mut output, &ack, codec)
                    .await
                    .map_err(|_| "terminal_protocol_error")?;
                return Ok(());
            }
            _ => return Err("terminal_protocol_error"),
        }
    }
}

async fn execute(
    config: &ObserverConfig,
    request: &wire::Request,
) -> Result<Vec<u8>, &'static str> {
    match request.method.as_str() {
        INVENTORY_METHOD => {
            let request = TerminalInventoryRequest::decode(request.payload.as_slice())
                .map_err(|_| "terminal_invalid_request")?;
            Ok(backend::inventory(config, &request.expected_cwd)
                .await?
                .encode_to_vec())
        }
        FOCUS_METHOD => {
            let request = TerminalFocusRequest::decode(request.payload.as_slice())
                .map_err(|_| "terminal_invalid_request")?;
            backend::focus(config, &request).await?;
            Ok(TerminalFocusResult { focused: true }.encode_to_vec())
        }
        _ => Err("terminal_invalid_request"),
    }
}

fn response(
    counter: &mut u64,
    request_id: String,
    correlation_id: Option<String>,
    result: Result<Vec<u8>, &'static str>,
) -> Envelope {
    let (status, payload, error_code) = match result {
        Ok(payload) => (ResponseStatus::Ok, payload, None),
        Err(code) => (ResponseStatus::Error, Vec::new(), Some(code.into())),
    };
    message(
        counter,
        correlation_id,
        envelope::Body::Response(Response {
            request_id,
            status: status.into(),
            payload,
            error_code,
        }),
    )
}

fn message(counter: &mut u64, correlation_id: Option<String>, body: envelope::Body) -> Envelope {
    *counter = counter.saturating_add(1);
    Envelope {
        protocol_major: PROTOCOL_MAJOR,
        protocol_minor: PROTOCOL_MINOR,
        message_id: format!("observer_{counter}"),
        correlation_id,
        causation_id: None,
        body: Some(body),
    }
}
