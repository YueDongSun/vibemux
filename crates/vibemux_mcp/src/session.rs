//! One in-flight metadata call, with responsive cancellation, EOF and idle faults.

use crate::{
    config::{HostConfig, MAX_INPUT_BYTES},
    framing::{LineReader, write_json},
    protocol::{Action, InFlightAction, Protocol, error, tool_error, tool_result, while_reading},
    supervisor::Supervisor,
    tools::ReadOperation,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufRead, AsyncWrite},
    time::{Instant, sleep_until, timeout_at},
};

async fn changed<W: AsyncWrite + Unpin>(
    writer: &mut W,
    protocol: &Protocol,
) -> Result<(), &'static str> {
    if protocol.initialized && protocol.metadata_read {
        write_json(
            writer,
            &json!({"jsonrpc":"2.0","method":"notifications/tools/list_changed"}),
        )
        .await?;
    }
    Ok(())
}

enum ReadCompletion {
    Finished(Result<Value, &'static str>),
    Cancelled,
    Closed,
}

async fn write_during_cleanup<W: AsyncWrite + Unpin>(
    writer: &mut W,
    supervisor: &mut Supervisor,
    value: &Value,
) -> Result<bool, &'static str> {
    let write = write_json(writer, value);
    tokio::pin!(write);
    let mut restored = false;
    let mut cleanup_error = None;
    loop {
        tokio::select! {
            biased;
            result = supervisor.lifecycle(), if cleanup_error.is_none() => {
                match result {
                    Ok(changed) => restored |= changed,
                    Err(code) => cleanup_error = Some(code),
                }
            }
            result = &mut write => {
                result?;
                if let Some(code) = cleanup_error { return Err(code); }
                return Ok(restored);
            }
        }
    }
}

async fn read_with_control<R: AsyncBufRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut LineReader<R>,
    writer: &mut W,
    supervisor: &mut Supervisor,
    id: &Value,
    operation: ReadOperation,
) -> Result<ReadCompletion, &'static str> {
    let deadline = Instant::now() + supervisor.read_budget();
    let read = supervisor.read(operation);
    tokio::pin!(read);
    loop {
        tokio::select! {
            biased;
            _ = sleep_until(deadline) => return Ok(ReadCompletion::Finished(Err("reader_timeout"))),
            result = &mut read => return Ok(ReadCompletion::Finished(result)),
            bytes = reader.next() => {
                let Some(bytes) = bytes? else { return Ok(ReadCompletion::Closed); };
                let action = match serde_json::from_slice::<Value>(&bytes) {
                    Ok(message) => while_reading(message, id),
                    Err(_) => InFlightAction::Reply(error(Value::Null, -32700, "parse_error")),
                };
                match action {
                    InFlightAction::Cancel => return Ok(ReadCompletion::Cancelled),
                    InFlightAction::Ignore => {},
                    InFlightAction::Reply(reply) => {
                        timeout_at(deadline, write_json(writer, &reply)).await.map_err(|_| "reader_timeout")??;
                    }
                }
            }
        }
    }
}

pub async fn run<R: AsyncBufRead + Unpin, W: AsyncWrite + Unpin>(
    reader: R,
    writer: W,
    config: HostConfig,
) -> Result<(), &'static str> {
    let supervisor = Supervisor::new(config.clone());
    run_owned(reader, writer, config, supervisor).await
}

#[cfg(feature = "test_helpers")]
pub async fn run_fixture<R: AsyncBufRead + Unpin, W: AsyncWrite + Unpin>(
    reader: R,
    writer: W,
    config: HostConfig,
    supervisor: Supervisor,
) -> Result<(), &'static str> {
    run_owned(reader, writer, config, supervisor).await
}

async fn run_owned<R: AsyncBufRead + Unpin, W: AsyncWrite + Unpin>(
    reader: R,
    mut writer: W,
    config: HostConfig,
    mut supervisor: Supervisor,
) -> Result<(), &'static str> {
    let mut reader = LineReader::new(reader, MAX_INPUT_BYTES);
    let mut protocol = Protocol::default();
    let result = async {
        loop {
            let bytes = tokio::select! {
                biased;
                result = supervisor.lifecycle() => {
                    if result? { changed(&mut writer, &protocol).await?; }
                    continue;
                },
                bytes = reader.next() => bytes?,
            };
            let Some(bytes) = bytes else {
                break;
            };
            let message = match serde_json::from_slice::<Value>(&bytes) {
                Ok(message) => message,
                Err(_) => {
                    write_json(&mut writer, &error(Value::Null, -32700, "parse_error")).await?;
                    continue;
                }
            };
            match protocol.handle(message, config.allow_metadata_read, &supervisor.view()) {
                Action::None => {}
                Action::Reply(reply) => write_json(&mut writer, &reply).await?,
                Action::Read { id, operation } => {
                    match read_with_control(
                        &mut reader,
                        &mut writer,
                        &mut supervisor,
                        &id,
                        operation,
                    )
                    .await?
                    {
                        ReadCompletion::Finished(Ok(value)) => {
                            write_json(&mut writer, &tool_result(id, value)).await?
                        }
                        ReadCompletion::Finished(Err(code)) => {
                            supervisor.begin_unload(code);
                            let restored = write_during_cleanup(
                                &mut writer,
                                &mut supervisor,
                                &tool_error(id, code),
                            )
                            .await?;
                            changed(&mut writer, &protocol).await?;
                            if restored {
                                changed(&mut writer, &protocol).await?;
                            }
                        }
                        ReadCompletion::Cancelled => {
                            supervisor.begin_unload("request_cancelled");
                            changed(&mut writer, &protocol).await?;
                        }
                        ReadCompletion::Closed => break,
                    }
                }
            }
        }
        Ok(())
    }
    .await;
    // Every normal completion/error/cancel/EOF path keeps and joins the owner.
    supervisor.shutdown().await?;
    result
}
