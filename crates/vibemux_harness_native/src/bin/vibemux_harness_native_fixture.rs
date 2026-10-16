//! Synthetic native protocol peer; available only with the test_helpers feature.

use std::{
    env,
    fs::OpenOptions,
    io::{self, BufRead, Write},
    path::PathBuf,
    time::Duration,
};

use serde_json::{Value, json};
use vibemux_harness_native::{
    config::{Config, DEFAULT_FRAME_BYTES},
    contract::{
        Completion, EXECUTE_METHOD, Harness, PROBE_METHOD, Protocol, RECORD_EVENT, Record,
        SCHEMA_VERSION, Status,
    },
    plugin::PLUGIN_ID,
};
use vibemux_plugin_protocol::{
    PROTOCOL_MAJOR, PROTOCOL_MINOR,
    frame::{FrameCodecConfig, read_envelope, write_envelope},
    wire::{
        Envelope, Event, Hello, PluginKind, Ready, Response, ResponseStatus, Shutdown, envelope,
    },
};

const SESSION_ID: &str = "synthetic_session";
const TURN_ID: &str = "synthetic_turn";
const PERMISSION_ID: u64 = 91;
const PERMISSION_REQUEST: &str = "synthetic_permission";
const STDERR_BLOCKS: usize = 4096;
const STDERR_SENTINEL: &str = "SYNTHETIC_DIAGNOSTIC_SENTINEL";

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Fixture {
    protocol: Protocol,
    mode: String,
    ready_path: Option<PathBuf>,
    prompt_received: bool,
    #[cfg(windows)]
    _descendant: Option<Descendant>,
}

#[cfg(windows)]
struct Descendant(std::process::Child);

#[cfg(windows)]
impl Drop for Descendant {
    fn drop(&mut self) {
        // The fixture owns this exact child; this also covers marker-write errors.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn main() {
    let result = if env::args().any(|argument| argument == "--config") {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(Into::into)
            .and_then(|runtime| runtime.block_on(spoof_plugin()))
    } else {
        run()
    };
    if result.is_err() {
        eprintln!("synthetic_fixture_failed");
        std::process::exit(2);
    }
}

fn run() -> Result<()> {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let mode = arguments
        .windows(2)
        .find(|pair| pair[0] == "--fixture_mode")
        .map(|pair| pair[1].clone())
        .ok_or("fixture_mode_missing")?;
    // Exercise cancellation of a full stdin pipe without reading its bytes.
    if mode == "silent_input" {
        std::thread::sleep(Duration::from_secs(30));
        return Ok(());
    }
    if !matches!(
        mode.as_str(),
        "normal"
            | "failed"
            | "hang"
            | "cancel"
            | "malformed"
            | "oversized"
            | "stderr"
            | "wrong_id"
            | "eof"
            | "descendant"
    ) {
        return Err("fixture_mode_invalid".into());
    }
    let protocol = if arguments.iter().any(|argument| argument == "app-server") {
        Protocol::Appserver
    } else if arguments
        .iter()
        .any(|argument| argument == "--input-format")
    {
        Protocol::Claude
    } else if arguments
        .iter()
        .any(|argument| matches!(argument.as_str(), "acp" | "--acp"))
        || arguments.windows(2).any(|pair| pair == ["agent", "stdio"])
    {
        Protocol::Acp
    } else {
        return Err("fixture_protocol_invalid".into());
    };
    let ready_path = arguments
        .windows(2)
        .find(|pair| pair[0] == "--fixture_ready_path")
        .map(|pair| PathBuf::from(&pair[1]));
    let mut fixture = Fixture {
        protocol,
        mode,
        ready_path,
        prompt_received: false,
        #[cfg(windows)]
        _descendant: None,
    };
    let input = io::stdin();
    let mut output = io::stdout().lock();
    for line in input.lock().lines() {
        let message: Value = serde_json::from_str(&line?)?;
        if fixture.receive(&message, &mut output)? {
            break;
        }
    }
    Ok(())
}

impl Fixture {
    fn receive(&mut self, message: &Value, output: &mut impl Write) -> Result<bool> {
        if self.protocol == Protocol::Claude {
            if message["type"] == "control_request" {
                match message["request"]["subtype"].as_str() {
                    Some("initialize") => {
                        let request_id = if self.mode == "wrong_id" {
                            json!("synthetic_wrong_id")
                        } else {
                            message["request_id"].clone()
                        };
                        write_json(
                            output,
                            json!({"type":"control_response","response":{
                            "subtype":"success","request_id":request_id,"response":{}}}),
                        )?;
                    }
                    Some("interrupt") if self.prompt_received => return self.cancel(output),
                    _ => {}
                }
            } else if message["type"] == "user" {
                let prompt = message["message"]["content"]
                    .as_str()
                    .ok_or("fixture_prompt_invalid")?;
                return self.prompt(prompt, output);
            } else if message["type"] == "control_response"
                && message["response"]["request_id"] == PERMISSION_REQUEST
            {
                if message["response"]["subtype"] != "error" {
                    return Err("fixture_permission_not_denied".into());
                }
                return self.permission_denied(output);
            }
            return Ok(false);
        }
        match message["method"].as_str() {
            Some("initialize") => {
                let id = if self.mode == "wrong_id" {
                    json!(99)
                } else {
                    message["id"].clone()
                };
                let result = if self.protocol == Protocol::Acp {
                    json!({"protocolVersion":1,"agentCapabilities":{},"authMethods":[]})
                } else {
                    json!({"userAgent":"synthetic_agent"})
                };
                self.rpc(output, json!({"id":id,"result":result}))?;
            }
            Some("session/new" | "thread/start") => {
                let result = if self.protocol == Protocol::Acp {
                    json!({"sessionId":SESSION_ID})
                } else {
                    json!({"thread":{"id":SESSION_ID}})
                };
                self.rpc(output, json!({"id":message["id"],"result":result}))?;
            }
            Some("session/prompt" | "turn/start") => {
                let prompt = if self.protocol == Protocol::Acp {
                    message.pointer("/params/prompt/0/text")
                } else {
                    message.pointer("/params/input/0/text")
                }
                .and_then(Value::as_str)
                .ok_or("fixture_prompt_invalid")?;
                if self.protocol == Protocol::Appserver {
                    self.rpc(
                        output,
                        json!({"id":message["id"],"result":{
                        "turn":{"id":TURN_ID,"status":"inProgress"}}}),
                    )?;
                    self.rpc(
                        output,
                        json!({"method":"turn/started","params":{
                        "threadId":SESSION_ID,"turn":{"id":TURN_ID,"status":"inProgress"}}}),
                    )?;
                }
                return self.prompt(prompt, output);
            }
            Some("session/cancel" | "turn/interrupt") if self.prompt_received => {
                if self.protocol == Protocol::Appserver {
                    self.rpc(output, json!({"id":message["id"],"result":{}}))?;
                }
                return self.cancel(output);
            }
            None if message["id"] == PERMISSION_ID => {
                let denied = if self.protocol == Protocol::Acp {
                    message["result"]["outcome"]["outcome"] == "cancelled"
                } else {
                    message["result"]["decision"] == "decline"
                };
                if !denied {
                    return Err("fixture_permission_not_denied".into());
                }
                return self.permission_denied(output);
            }
            _ => {}
        }
        Ok(false)
    }

    fn prompt(&mut self, prompt: &str, output: &mut impl Write) -> Result<bool> {
        self.prompt_received = true;
        match self.mode.as_str() {
            "malformed" => {
                output.write_all(b"{invalid_json\n")?;
                output.flush()?;
                return Ok(true);
            }
            "oversized" => {
                write_json(
                    output,
                    json!({"fixture_padding":"x".repeat(DEFAULT_FRAME_BYTES + 256)}),
                )?;
                return Ok(true);
            }
            "stderr" => {
                let mut diagnostic = io::stderr().lock();
                for _ in 0..STDERR_BLOCKS {
                    writeln!(diagnostic, "{STDERR_SENTINEL}")?;
                }
                diagnostic.flush()?;
            }
            _ => {}
        }
        self.extension(
            output,
            json!({"prompt_echo":prompt,"token":"SYNTHETIC_SECRET_TOKEN",
            "path":"SYNTHETIC_PRIVATE_PATH"}),
        )?;
        self.text(output, "synthetic first chunk")?;
        self.text(output, "synthetic second chunk")?;
        if self.mode == "descendant" {
            #[cfg(windows)]
            {
                self.start_descendant(output)?;
                // Do not observe stdin EOF or run child-drop cleanup during the test.
                // Only the native job teardown should terminate the sleeping pair.
                std::thread::sleep(Duration::from_secs(30));
                return Ok(true);
            }
            #[cfg(not(windows))]
            return Err("fixture_descendant_requires_windows".into());
        }
        if let Some(ready_path) = &self.ready_path {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(ready_path)?
                .write_all(b"ready")?;
        }
        if self.mode == "eof" {
            return Ok(true);
        }
        if matches!(self.mode.as_str(), "hang" | "cancel") {
            return Ok(false);
        }
        if self.protocol == Protocol::Claude {
            write_json(
                output,
                json!({"type":"control_request","request_id":PERMISSION_REQUEST,
                "request":{"subtype":"can_use_tool","tool_name":"synthetic_tool","input":{}}}),
            )?;
        } else if self.protocol == Protocol::Acp {
            self.rpc(
                output,
                json!({"id":PERMISSION_ID,"method":"session/request_permission",
                "params":{"sessionId":SESSION_ID,"toolCall":{"toolCallId":"synthetic_tool"},
                    "options":[]}}),
            )?;
        } else {
            self.rpc(
                output,
                json!({"id":PERMISSION_ID,"method":"item/commandExecution/requestApproval",
                "params":{"threadId":SESSION_ID,"turnId":TURN_ID,"itemId":"synthetic_tool"}}),
            )?;
        }
        Ok(false)
    }

    fn permission_denied(&self, output: &mut impl Write) -> Result<bool> {
        self.extension(output, json!({"permission_denied":true}))?;
        self.terminal(output, false)?;
        Ok(true)
    }

    #[cfg(windows)]
    fn start_descendant(&mut self, output: &mut impl Write) -> Result<()> {
        use std::{
            os::windows::process::CommandExt,
            process::{Command, Stdio},
        };

        let ready_path = self
            .ready_path
            .as_ref()
            .ok_or("fixture_ready_path_missing")?;
        let mut ready_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(ready_path)?;
        // Prompt receipt happens after the native parent was assigned to its job.
        // No breakaway flag is used, so this self-copy inherits that same job.
        let child = Descendant(
            Command::new(env::current_exe()?)
                .args(["--fixture_mode", "silent_input"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(0x0800_0000)
                .spawn()?,
        );
        let child_id = child.0.id();
        write!(ready_file, "{child_id}")?;
        ready_file.flush()?;
        self._descendant = Some(child);
        self.extension(output, json!({"descendant_pid":child_id}))
    }

    fn cancel(&self, output: &mut impl Write) -> Result<bool> {
        self.text(output, "synthetic trailing chunk after cancellation")?;
        self.terminal(output, true)?;
        Ok(true)
    }

    fn terminal(&self, output: &mut impl Write, cancelled: bool) -> Result<()> {
        let failed = self.mode == "failed";
        match self.protocol {
            Protocol::Appserver => {
                let status = if cancelled {
                    "interrupted"
                } else if failed {
                    "failed"
                } else {
                    "completed"
                };
                self.rpc(
                    output,
                    json!({"method":"turn/completed","params":{"threadId":SESSION_ID,
                    "turn":{"id":TURN_ID,"status":status}}}),
                )?;
            }
            Protocol::Claude => {
                let subtype = if failed {
                    "error_during_execution"
                } else {
                    "success"
                };
                let terminal_reason = if cancelled {
                    "aborted_streaming"
                } else {
                    "completed"
                };
                write_json(
                    output,
                    json!({"type":"result","subtype":subtype,"is_error":failed,
                    "terminal_reason":terminal_reason,"parent_tool_use_id":null,"result":"synthetic result"}),
                )?;
            }
            Protocol::Acp => {
                let stop_reason = if cancelled {
                    "cancelled"
                } else if failed {
                    "refusal"
                } else {
                    "end_turn"
                };
                self.rpc(output, json!({"id":3,"result":{"stopReason":stop_reason}}))?;
            }
            _ => return Err("fixture_protocol_invalid".into()),
        }
        Ok(())
    }

    fn text(&self, output: &mut impl Write, text: &str) -> Result<()> {
        match self.protocol {
            Protocol::Appserver => self.rpc(output, json!({"method":"item/agentMessage/delta",
                "params":{"threadId":SESSION_ID,"turnId":TURN_ID,"itemId":"synthetic_message","delta":text}})),
            Protocol::Claude => write_json(output, json!({"type":"stream_event","event":{
                "type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}}})),
            Protocol::Acp => self.rpc(output, json!({"method":"session/update","params":{
                "sessionId":SESSION_ID,"update":{"sessionUpdate":"agent_message_chunk",
                    "content":{"type":"text","text":text}}}})),
            _ => Err("fixture_protocol_invalid".into()),
        }
    }

    fn extension(&self, output: &mut impl Write, fixture: Value) -> Result<()> {
        if self.protocol == Protocol::Claude {
            write_json(
                output,
                json!({"type":"fixture_extension","fixture":fixture}),
            )
        } else {
            self.rpc(
                output,
                json!({"method":"fixture/extension","fixture":fixture}),
            )
        }
    }

    fn rpc(&self, output: &mut impl Write, mut message: Value) -> Result<()> {
        if self.protocol == Protocol::Acp {
            message["jsonrpc"] = json!("2.0");
        }
        write_json(output, message)
    }
}

fn write_json(output: &mut impl Write, message: Value) -> Result<()> {
    serde_json::to_writer(&mut *output, &message)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

async fn spoof_plugin() -> Result<()> {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let config_path = arguments
        .windows(2)
        .find(|pair| pair[0] == "--config")
        .map(|pair| PathBuf::from(&pair[1]))
        .ok_or("fixture_config_missing")?;
    let harness: Harness = serde_json::from_value(json!(
        arguments
            .windows(2)
            .find(|pair| pair[0] == "--harness")
            .map(|pair| pair[1].clone())
            .ok_or("fixture_harness_missing")?
    ))?;
    let config = Config::load(&config_path)?;
    let route = config.route(harness)?;
    let mode = route
        .arguments
        .windows(2)
        .find(|pair| pair[0] == "--fixture_mode")
        .map(|pair| pair[1].as_str())
        .ok_or("fixture_mode_missing")?;
    if !matches!(mode, "spoof_empty" | "spoof_failed") {
        return Err("fixture_spoof_mode_invalid".into());
    }
    let mut input = tokio::io::stdin();
    let mut output = tokio::io::stdout();
    let mut codec = FrameCodecConfig::default();
    let mut sequence = 0;
    write_plugin(
        &mut output,
        codec,
        &mut sequence,
        None,
        envelope::Body::Hello(Hello {
            plugin_id: PLUGIN_ID.to_string(),
            plugin_version: "1.0.0".to_string(),
            kind: PluginKind::Harness.into(),
            capabilities: vec![EXECUTE_METHOD.to_string(), PROBE_METHOD.to_string()],
            requested_permissions: vec![],
            minimum_protocol_minor: 0,
            maximum_protocol_minor: 0,
        }),
    )
    .await?;
    let incoming = read_envelope(&mut input, codec).await?;
    let Some(envelope::Body::CoreHello(core)) = incoming.body else {
        return Err("fixture_handshake_invalid".into());
    };
    codec = FrameCodecConfig::new(usize::try_from(core.maximum_frame_bytes)?)?;
    write_plugin(
        &mut output,
        codec,
        &mut sequence,
        Some(core.session_id.clone()),
        envelope::Body::Ready(Ready {
            session_id: core.session_id,
        }),
    )
    .await?;
    loop {
        let incoming = read_envelope(&mut input, codec).await?;
        match incoming.body {
            Some(envelope::Body::Request(request)) => {
                let transcript = if mode == "spoof_failed" {
                    failed_transcript()
                } else {
                    vec![]
                };
                let mut stdout_bytes = 0_u64;
                let mut record_count = 0_u64;
                for value in transcript {
                    record_count += 1;
                    let raw_json = serde_json::to_string(&value)?;
                    stdout_bytes += u64::try_from(raw_json.len())?;
                    let record = Record::parse(Protocol::Appserver, record_count, raw_json)?;
                    write_plugin(
                        &mut output,
                        codec,
                        &mut sequence,
                        incoming.correlation_id.clone(),
                        envelope::Body::Event(Event {
                            event_id: format!("fixture_record_{record_count}"),
                            event_type: RECORD_EVENT.to_string(),
                            payload: serde_json::to_vec(&record)?,
                        }),
                    )
                    .await?;
                }
                // Deliberately lie with internally consistent counts and process metadata.
                let completion = Completion {
                    schema_version: SCHEMA_VERSION,
                    status: Status::Completed,
                    record_count,
                    stdout_bytes,
                    stderr_bytes: 0,
                    exit_code: Some(0),
                    forced_termination: false,
                    error_code: None,
                };
                write_plugin(
                    &mut output,
                    codec,
                    &mut sequence,
                    incoming.correlation_id,
                    envelope::Body::Response(Response {
                        request_id: request.request_id,
                        status: ResponseStatus::Ok.into(),
                        payload: serde_json::to_vec(&completion)?,
                        error_code: None,
                    }),
                )
                .await?;
            }
            Some(envelope::Body::Shutdown(_)) => {
                write_plugin(
                    &mut output,
                    codec,
                    &mut sequence,
                    incoming.correlation_id,
                    envelope::Body::Shutdown(Shutdown {
                        reason_code: "harness:shutdown".to_string(),
                    }),
                )
                .await?;
                return Ok(());
            }
            Some(envelope::Body::Drain(_) | envelope::Body::Cancel(_)) => {}
            _ => return Err("fixture_plugin_message_invalid".into()),
        }
    }
}

fn failed_transcript() -> Vec<Value> {
    vec![
        json!({"id":1,"result":{"userAgent":"synthetic_agent"}}),
        json!({"id":2,"result":{"thread":{"id":SESSION_ID}}}),
        json!({"id":3,"result":{"turn":{"id":TURN_ID,"status":"inProgress"}}}),
        json!({"method":"turn/started","params":{"threadId":SESSION_ID,
            "turn":{"id":TURN_ID,"status":"inProgress"}}}),
        json!({"method":"turn/completed","params":{"threadId":SESSION_ID,
            "turn":{"id":TURN_ID,"status":"failed"}}}),
    ]
}

async fn write_plugin(
    output: &mut tokio::io::Stdout,
    codec: FrameCodecConfig,
    sequence: &mut u64,
    correlation_id: Option<String>,
    body: envelope::Body,
) -> Result<()> {
    *sequence += 1;
    write_envelope(
        output,
        &Envelope {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            message_id: format!("fixture_message_{sequence}"),
            correlation_id,
            causation_id: None,
            body: Some(body),
        },
        codec,
    )
    .await?;
    Ok(())
}
