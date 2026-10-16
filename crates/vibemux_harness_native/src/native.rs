use std::time::Duration;

use serde_json::Value;
use tokio::{
    io::AsyncWriteExt,
    sync::{mpsc, watch},
    time::{Instant, timeout_at},
};

use crate::{
    Error,
    config::{Limits, Route},
    contract::{Completion, Record, Request, SCHEMA_VERSION, Status},
    json_lines::Reader,
    process::Process,
    session::Session,
};

/// Runs a configured native adapter. All vendor records cross the bounded channel intact.
/// Probe performs initialization only: no authenticate, session/new, or model request.
pub async fn run(
    mut route: Route,
    limits: Limits,
    request: Request,
    probe: bool,
    records: mpsc::Sender<Record>,
    mut cancellation: watch::Receiver<bool>,
) -> Result<Completion, Error> {
    limits.validate()?;
    if request.schema_version != SCHEMA_VERSION || request.request_id.is_nil() {
        return Err(Error::new("harness_invalid_request"));
    }
    if !route.enabled
        || !route.protocol.accepts(request.harness)
        || route.harness != request.harness
    {
        return Err(Error::new("harness_route_unavailable"));
    }
    if !probe {
        request.validate()?;
        if !route.allow_execution {
            return Err(Error::new("harness_execution_disabled"));
        }
    }
    route.working_directory = route
        .working_directory
        .canonicalize()
        .map_err(|source| Error::with_source("harness_invalid_cwd", source))?;
    let mut session = Session::new(route.clone(), request.clone(), probe);
    let arguments = session.arguments()?;
    let mut completion = Completion {
        schema_version: SCHEMA_VERSION,
        status: Status::Unverified,
        record_count: 0,
        stdout_bytes: 0,
        stderr_bytes: 0,
        exit_code: None,
        forced_termination: false,
        error_code: None,
    };
    if *cancellation.borrow() {
        completion.status = Status::Cancelled;
        return Ok(completion);
    }
    let deadline = Instant::now() + Duration::from_millis(limits.timeout_ms);
    let mut process = Process::spawn(&route, arguments).await?;
    let exchange = async {
        if let Some(initial) = session.initial() {
            write_json(&mut process.input, &initial, deadline, Some(&mut cancellation)).await?;
        } else {
            write_bytes(&mut process.input, request.prompt.as_bytes(), deadline, Some(&mut cancellation)).await?;
            process.input.take();
        }
        let output = process.output.take().ok_or_else(|| Error::new("harness_missing_pipe"))?;
        let mut reader = Reader::new(output, limits.frame_bytes)?;
        let mut read_deadline = deadline;
        let mut cancel_sent = false;
        let mut parent_exited = false;
        loop {
            let line = tokio::select! {
                biased;
                changed = cancellation.changed(), if !cancel_sent => {
                    if changed.is_err() || *cancellation.borrow() {
                        cancel_sent = true;
                        read_deadline = read_deadline.min(Instant::now() + Duration::from_millis(limits.shutdown_ms));
                        if let Some(cancel) = session.cancel() {
                            write_json(&mut process.input, &cancel, read_deadline, None).await?;
                        } else {
                            return Err(Error::new("harness_cancelled"));
                        }
                    }
                    continue;
                }
                exited = process.observe_exit(), if session.status.is_some() && !parent_exited => {
                    exited?;
                    parent_exited = true;
                    continue;
                }
                line = timeout_at(read_deadline, reader.next()) => {
                    match line {
                        Ok(line) => line?,
                        Err(_) if cancel_sent => return Err(Error::new("harness_cancelled")),
                        Err(_) => return Err(Error::new("harness_deadline_exceeded")),
                    }
                }
            };
            let Some(raw_json) = line else { break; };
            completion.stdout_bytes = completion.stdout_bytes.checked_add(raw_json.len() as u64)
                .ok_or_else(|| Error::new("harness_capture_too_large"))?;
            if completion.stdout_bytes > limits.capture_bytes as u64
                || completion.record_count >= limits.record_count as u64 {
                return Err(Error::new("harness_capture_too_large"));
            }
            let record = Record::parse(route.protocol, completion.record_count + 1, raw_json)?;
            let value = record.value()?;
            let kind = record.kind;
            let send_record = timeout_at(read_deadline, records.send(record));
            let sent = if cancel_sent {
                send_record.await
            } else {
                tokio::select! {
                    _ = wait_cancel(&mut cancellation) => return Err(Error::new("harness_cancelled")),
                    sent = send_record => sent,
                }
            };
            sent.map_err(|_| Error::new("harness_backpressure_timeout"))?
                .map_err(|source| Error::with_source("harness_receiver_closed", source))?;
            completion.record_count += 1;
            let responses = session.receive(&value, kind)?;
            for response in responses {
                let cancel = if cancel_sent { None } else { Some(&mut cancellation) };
                write_json(&mut process.input, &response, read_deadline, cancel).await?;
            }
            if session.status.is_some() {
                process.input.take();
                read_deadline = read_deadline.min(Instant::now() + Duration::from_millis(limits.shutdown_ms));
            }
        }
        Ok::<(), Error>(())
    }.await;

    let force = exchange.is_err();
    let exit = process
        .finish(Duration::from_millis(limits.shutdown_ms), force)
        .await?;
    completion.exit_code = exit.code;
    completion.stderr_bytes = exit.stderr_bytes;
    completion.forced_termination = exit.forced;
    completion.status = session.status.unwrap_or(Status::Unverified);
    if let Err(error) = exchange {
        completion.status = if error.code() == "harness_cancelled" {
            Status::Cancelled
        } else {
            Status::Failed
        };
        completion.error_code = Some(error.code().to_string());
    } else if !exit.success || exit.forced {
        completion.status = Status::Failed;
        completion.error_code = Some("harness_process_failed".to_string());
    } else if completion.status == Status::Unverified {
        completion.error_code = Some("harness_eof_without_terminal".to_string());
    }
    Ok(completion)
}

async fn write_json(
    input: &mut Option<tokio::process::ChildStdin>,
    value: &Value,
    deadline: Instant,
    cancellation: Option<&mut watch::Receiver<bool>>,
) -> Result<(), Error> {
    let mut bytes = serde_json::to_vec(value)
        .map_err(|source| Error::with_source("harness_encode_failed", source))?;
    bytes.push(b'\n');
    write_bytes(input, &bytes, deadline, cancellation).await
}

async fn write_bytes(
    input: &mut Option<tokio::process::ChildStdin>,
    bytes: &[u8],
    deadline: Instant,
    cancellation: Option<&mut watch::Receiver<bool>>,
) -> Result<(), Error> {
    let input = input
        .as_mut()
        .ok_or_else(|| Error::new("harness_stdin_closed"))?;
    let write = timeout_at(deadline, async {
        input.write_all(bytes).await?;
        input.flush().await
    });
    let result = if let Some(cancellation) = cancellation {
        tokio::select! {
            _ = wait_cancel(cancellation) => return Err(Error::new("harness_cancelled")),
            result = write => result,
        }
    } else {
        write.await
    };
    result
        .map_err(|_| Error::new("harness_write_timeout"))?
        .map_err(|source| Error::with_source("harness_write_failed", source))
}

async fn wait_cancel(cancellation: &mut watch::Receiver<bool>) {
    loop {
        if *cancellation.borrow() || cancellation.changed().await.is_err() {
            return;
        }
    }
}
