//! Responsive version-2 stdio owner. One test slot; no deferred file operations.

use crate::{
    contract::{MAX_FRAME_BYTES, RecoveryRecord, ToolError},
    protocol::{Protocol, ProtocolAction, ToolSurface, execute_file, tool_error, tool_result},
    server::{OUTPUT_DEADLINE, SESSION_DEADLINE},
    test_session::TestSession,
};
use serde_json::Value;
use std::{
    collections::VecDeque,
    future::{Future, pending},
    num::NonZeroUsize,
    pin::Pin,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::watch,
    time::{Instant, sleep_until},
};
use vibemux_harness::dispatch::json_line_framer::JsonLineFramer;

const MAX_QUEUED_REPLIES: usize = 8;
const INPUT_CHUNK: usize = 4096;
type RunResult = Result<(TestSession, Result<Value, ToolError>), &'static str>;

struct ActiveTest {
    id: Value,
    cancellation: watch::Sender<bool>,
    suppress_reply: bool,
    future: Pin<Box<dyn Future<Output = RunResult> + Send>>,
}

impl Drop for ActiveTest {
    fn drop(&mut self) {
        let _ = self.cancellation.send(true);
    }
}

struct PendingReply {
    bytes: Vec<u8>,
    offset: usize,
    deadline: Instant,
}

pub struct TestTransportOutcome {
    pub session: Option<TestSession>,
    pub recovery_hint: RecoveryRecord,
    pub transport: Result<(), &'static str>,
}

fn enqueue(queue: &mut VecDeque<PendingReply>, value: Value) -> Result<(), &'static str> {
    if queue.len() >= MAX_QUEUED_REPLIES {
        return Err("output_backpressure");
    }
    let mut bytes = serde_json::to_vec(&value).map_err(|_| "invalid_output")?;
    bytes.push(b'\n');
    if bytes.len() > MAX_FRAME_BYTES {
        return Err("output_limit");
    }
    queue.push_back(PendingReply {
        bytes,
        offset: 0,
        deadline: Instant::now() + OUTPUT_DEADLINE,
    });
    Ok(())
}

async fn write_one<W: AsyncWrite + Unpin>(
    writer: &mut W,
    reply: Option<&mut PendingReply>,
) -> std::io::Result<bool> {
    let Some(reply) = reply else {
        return pending().await;
    };
    if reply.offset == reply.bytes.len() {
        writer.flush().await?;
        return Ok(true);
    }
    let count = writer.write(&reply.bytes[reply.offset..]).await?;
    if count == 0 {
        return Err(std::io::ErrorKind::WriteZero.into());
    }
    reply.offset += count;
    Ok(false)
}

async fn poll_test(active: Option<&mut ActiveTest>) -> RunResult {
    match active {
        Some(active) => active.future.as_mut().await,
        None => pending().await,
    }
}

/// An orderly EOF cancels and joins a pending test, suppressing its response.
/// All other transport failures also finalize before returning. Caller abortion
/// is an unknown outcome; the operation guard retains the editable workspace.
pub async fn serve_tests<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut R,
    writer: &mut W,
    initial: TestSession,
) -> TestTransportOutcome {
    let surface = ToolSurface {
        grants: initial.tools.grants(),
        test_suites: initial.suites(),
    };
    let mut recovery_hint = initial.recovery_hint();
    let mut session = Some(initial);
    let mut active: Option<ActiveTest> = None;
    let mut protocol = Protocol::default();
    let mut framer =
        JsonLineFramer::new(NonZeroUsize::new(MAX_FRAME_BYTES).expect("positive frame bound"));
    let mut queue = VecDeque::new();
    let mut input = [0; INPUT_CHUNK];
    let deadline = Instant::now() + SESSION_DEADLINE;
    let mut input_open = true;
    let transport = 'session: loop {
        if !input_open && active.is_none() && queue.is_empty() {
            break Ok(());
        }
        let next_deadline = queue.front().map_or(deadline, |reply: &PendingReply| {
            deadline.min(reply.deadline)
        });
        enum Event {
            Input(std::io::Result<usize>),
            Output(std::io::Result<bool>),
            Test(Box<RunResult>),
            Expired,
        }
        let event = tokio::select! {
            biased;
            _ = sleep_until(next_deadline) => Event::Expired,
            result = poll_test(active.as_mut()), if active.is_some() => Event::Test(Box::new(result)),
            read = reader.read(&mut input), if input_open => Event::Input(read),
            write = write_one(writer, queue.front_mut()), if !queue.is_empty() => Event::Output(write),
        };
        match event {
            Event::Expired => {
                break Err(if Instant::now() >= deadline {
                    "session_timeout"
                } else {
                    "output_timeout"
                });
            }
            Event::Output(Err(_)) => break Err("output_unavailable"),
            Event::Output(Ok(true)) => {
                queue.pop_front();
            }
            Event::Output(Ok(false)) => (),
            Event::Test(result) => {
                let completed = active.take().expect("active operation");
                match *result {
                    Err(code) => break Err(code),
                    Ok((returned, result)) => {
                        session = Some(returned);
                        // This enqueue is the response commit point. A later
                        // cancellation references an already completed request.
                        if !completed.suppress_reply {
                            let response = match result {
                                Ok(value) => tool_result(completed.id.clone(), value),
                                Err(error) => tool_error(completed.id.clone(), error),
                            };
                            if let Err(code) = enqueue(&mut queue, response) {
                                break Err(code);
                            }
                        }
                    }
                }
            }
            Event::Input(Err(_)) => break Err("input_unavailable"),
            Event::Input(Ok(0)) => {
                if framer.pending_bytes() != 0 {
                    break Err("incomplete_frame");
                }
                input_open = false;
                if let Some(active) = &mut active {
                    active.suppress_reply = true;
                    let _ = active.cancellation.send(true);
                }
            }
            Event::Input(Ok(count)) => {
                let mut frames = Vec::new();
                if framer.push(&input[..count], &mut frames).is_err() {
                    break Err("invalid_frame");
                }
                for frame in frames {
                    if Instant::now() >= deadline {
                        break 'session Err("session_timeout");
                    }
                    let message = match serde_json::from_str(&frame) {
                        Ok(message) => message,
                        Err(_) => break 'session Err("invalid_json"),
                    };
                    let action = match protocol.accept(message, &surface, active.is_some()) {
                        Ok(action) => action,
                        Err(code) => break 'session Err(code),
                    };
                    let reply = match action {
                        ProtocolAction::None => None,
                        ProtocolAction::Reply(reply) => Some(reply),
                        ProtocolAction::Cancel { id } => {
                            if let Some(active) = &mut active
                                && active.id == id
                            {
                                active.suppress_reply = true;
                                let _ = active.cancellation.send(true);
                            }
                            None
                        }
                        ProtocolAction::File { id, operation } => {
                            let owner = session
                                .as_mut()
                                .expect("file operation admitted only while idle");
                            Some(execute_file(id, operation, &mut owner.tools, true))
                        }
                        ProtocolAction::RunTest { id, call } => {
                            let owner = session.take().expect("one admitted operation");
                            recovery_hint = owner.recovery_hint();
                            let (sender, receiver) = watch::channel(false);
                            active = Some(ActiveTest {
                                id,
                                cancellation: sender,
                                suppress_reply: false,
                                future: Box::pin(owner.run(call, receiver)),
                            });
                            None
                        }
                    };
                    if let Some(reply) = reply
                        && let Err(code) = enqueue(&mut queue, reply)
                    {
                        break 'session Err(code);
                    }
                }
            }
        }
    };
    // A read/write/deadline failure must not drop a running operation. Stop
    // admission, latch cancellation and join its complete owner finalization.
    if let Some(mut operation) = active.take() {
        let _ = operation.cancellation.send(true);
        if let Ok((returned, _)) = operation.future.as_mut().await {
            session = Some(returned);
        }
    }
    if transport.is_err()
        && let Some(session) = &mut session
    {
        session.tools.retain_for_recovery();
    }
    TestTransportOutcome {
        session,
        recovery_hint,
        transport,
    }
}
