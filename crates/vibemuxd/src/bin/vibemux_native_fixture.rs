#![forbid(unsafe_code)]
//! Synthetic native vendor CLI for the harness dispatch tests (ADR 029);
//! built only with the `test_helpers` feature.
//!
//! Tests copy this binary as `codex`, `claude`, and so on, into a
//! directory outside the project root, because a route never carries
//! operator argv. The mode therefore comes from `fixture_mode.json` next to
//! the executable, unless `--fixture_mode <mode>` overrides it (used for the
//! fixture's own descendants and the trampoline tests). The protocol comes
//! from the argv that the launch profile generates.

use std::{
    env,
    fs::OpenOptions,
    io::{self, BufRead, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use serde::Deserialize;
use serde_json::{Value, json};

const MODE_FILE_NAME: &str = "fixture_mode.json";
const SESSION_ID: &str = "synthetic_session";
const TURN_ID: &str = "synthetic_turn";
const PERMISSION_ID: u64 = 91;
const PERMISSION_REQUEST: &str = "synthetic_permission";
const STDERR_BLOCKS: usize = 4096;
const STDERR_SENTINEL: &str = "SYNTHETIC_DIAGNOSTIC_SENTINEL";
/// Larger than the default 128 KiB native frame.
const OVERSIZED_PADDING_BYTES: usize = 128 * 1024 + 256;
/// Long enough to outlive every test deadline; the tree kills it first.
const SLEEP_LIMIT: Duration = Duration::from_secs(30);
const EXIT_CODE_MODE_CODE: i32 = 7;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModeFile {
    mode: String,
    #[serde(default)]
    ready_path: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Protocol {
    CodexExec,
    AppServer,
    Claude,
    Acp,
}

struct Fixture {
    protocol: Protocol,
    mode: String,
    ready_path: Option<PathBuf>,
    prompt_received: bool,
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(_) => {
            eprintln!("synthetic_fixture_failed");
            std::process::exit(2);
        }
    }
}

fn run() -> Result<i32> {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let settings = settings(&arguments)?;
    // Modes that never speak a protocol.
    match settings.mode.as_str() {
        // Holds stdin unread, so a prompt write can fill the pipe.
        "silent_input" => {
            std::thread::sleep(SLEEP_LIMIT);
            return Ok(0);
        }
        "echo_stdin" => {
            io::copy(&mut io::stdin().lock(), &mut io::stdout().lock())?;
            return Ok(0);
        }
        "exit_code" => return Ok(EXIT_CODE_MODE_CODE),
        // Starts a descendant before reading any input, then echoes stdin.
        "early_descendant" => {
            let ready_path = settings.ready_path.ok_or("fixture_ready_path_missing")?;
            let child = spawn_silent_descendant(Stdio::inherit())?;
            write_ready(&ready_path, &child.id().to_string())?;
            io::copy(&mut io::stdin().lock(), &mut io::stdout().lock())?;
            return Ok(0);
        }
        "normal" | "failed" | "hang" | "cancel" | "malformed" | "oversized" | "stderr"
        | "wrong_id" | "eof" | "descendant" => {}
        _ => return Err("fixture_mode_invalid".into()),
    }
    let mut fixture = Fixture {
        protocol: protocol(&arguments)?,
        mode: settings.mode,
        ready_path: settings.ready_path,
        prompt_received: false,
    };
    let mut output = io::stdout().lock();
    if fixture.protocol == Protocol::CodexExec {
        let mut prompt = String::new();
        io::stdin().lock().read_to_string(&mut prompt)?;
        fixture.prompt(&prompt, &mut output)?;
        return Ok(0);
    }
    for line in io::stdin().lock().lines() {
        let message: Value = serde_json::from_str(&line?)?;
        if fixture.receive(&message, &mut output)? {
            break;
        }
    }
    Ok(0)
}

fn settings(arguments: &[String]) -> Result<ModeFile> {
    if let Some(pair) = arguments
        .windows(2)
        .find(|pair| pair[0] == "--fixture_mode")
    {
        let ready_path = arguments
            .windows(2)
            .find(|pair| pair[0] == "--fixture_ready_path")
            .map(|pair| PathBuf::from(&pair[1]));
        return Ok(ModeFile {
            mode: pair[1].clone(),
            ready_path,
        });
    }
    let executable = env::current_exe()?;
    let directory = executable.parent().ok_or("fixture_directory_missing")?;
    let bytes = std::fs::read(directory.join(MODE_FILE_NAME))?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn protocol(arguments: &[String]) -> Result<Protocol> {
    let has = |value: &str| arguments.iter().any(|argument| argument == value);
    if has("app-server") {
        Ok(Protocol::AppServer)
    } else if has("--input-format") {
        Ok(Protocol::Claude)
    } else if has("acp")
        || has("--acp")
        || arguments.windows(2).any(|pair| pair == ["agent", "stdio"])
    {
        Ok(Protocol::Acp)
    } else if has("exec") && has("--json") {
        Ok(Protocol::CodexExec)
    } else {
        Err("fixture_protocol_invalid".into())
    }
}

fn spawn_silent_descendant(stdout: Stdio) -> Result<std::process::Child> {
    let mut command = Command::new(env::current_exe()?);
    command
        .args(["--fixture_mode", "silent_input"])
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::null());
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut command, 0x0800_0000);
    Ok(command.spawn()?)
}

fn write_ready(path: &Path, content: &str) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(content.as_bytes())?;
    file.flush()?;
    Ok(())
}

impl Fixture {
    fn receive(&mut self, message: &Value, output: &mut impl Write) -> Result<bool> {
        if self.protocol == Protocol::Claude {
            return self.receive_claude(message, output);
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
                if self.protocol == Protocol::AppServer {
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
                if self.protocol == Protocol::AppServer {
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

    fn receive_claude(&mut self, message: &Value, output: &mut impl Write) -> Result<bool> {
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
        Ok(false)
    }

    fn prompt(&mut self, prompt: &str, output: &mut impl Write) -> Result<bool> {
        self.prompt_received = true;
        if self.protocol == Protocol::CodexExec {
            write_json(
                output,
                json!({"type":"thread.started","thread_id":SESSION_ID}),
            )?;
            write_json(output, json!({"type":"turn.started"}))?;
        }
        match self.mode.as_str() {
            "malformed" => {
                output.write_all(b"{invalid_json\n")?;
                output.flush()?;
                return Ok(true);
            }
            "oversized" => {
                write_json(
                    output,
                    json!({"fixture_padding":"x".repeat(OVERSIZED_PADDING_BYTES)}),
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
            let ready_path = self
                .ready_path
                .as_ref()
                .ok_or("fixture_ready_path_missing")?;
            // Started in response to input, after the tree was contained.
            let child = spawn_silent_descendant(Stdio::null())?;
            write_ready(ready_path, &child.id().to_string())?;
            self.extension(output, json!({"descendant_pid":child.id()}))?;
            // Only the tree teardown may end the sleeping pair.
            std::thread::sleep(SLEEP_LIMIT);
            return Ok(true);
        }
        if let Some(ready_path) = &self.ready_path {
            write_ready(ready_path, "ready")?;
        }
        if self.mode == "eof" {
            return Ok(true);
        }
        if matches!(self.mode.as_str(), "hang" | "cancel") {
            if self.protocol == Protocol::CodexExec {
                // `codex exec` has no cancel message; only a kill ends it.
                std::thread::sleep(SLEEP_LIMIT);
                return Ok(true);
            }
            return Ok(false);
        }
        match self.protocol {
            Protocol::CodexExec => {
                self.terminal(output, false)?;
                return Ok(true);
            }
            Protocol::Claude => write_json(
                output,
                json!({"type":"control_request","request_id":PERMISSION_REQUEST,
                "request":{"subtype":"can_use_tool","tool_name":"synthetic_tool","input":{}}}),
            )?,
            Protocol::Acp => self.rpc(
                output,
                json!({"id":PERMISSION_ID,"method":"session/request_permission",
                "params":{"sessionId":SESSION_ID,"toolCall":{"toolCallId":"synthetic_tool"},
                    "options":[]}}),
            )?,
            Protocol::AppServer => self.rpc(
                output,
                json!({"id":PERMISSION_ID,"method":"item/commandExecution/requestApproval",
                "params":{"threadId":SESSION_ID,"turnId":TURN_ID,"itemId":"synthetic_tool"}}),
            )?,
        }
        Ok(false)
    }

    fn permission_denied(&self, output: &mut impl Write) -> Result<bool> {
        self.extension(output, json!({"permission_denied":true}))?;
        self.terminal(output, false)?;
        Ok(true)
    }

    fn cancel(&self, output: &mut impl Write) -> Result<bool> {
        self.text(output, "synthetic trailing chunk after cancellation")?;
        self.terminal(output, true)?;
        Ok(true)
    }

    fn terminal(&self, output: &mut impl Write, cancelled: bool) -> Result<()> {
        let failed = self.mode == "failed";
        match self.protocol {
            Protocol::CodexExec => {
                if failed {
                    write_json(
                        output,
                        json!({"type":"turn.failed","error":{"message":"synthetic failure"}}),
                    )
                } else {
                    write_json(
                        output,
                        json!({"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}),
                    )
                }
            }
            Protocol::AppServer => {
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
                )
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
                    "terminal_reason":terminal_reason,"parent_tool_use_id":null,
                    "result":"synthetic result"}),
                )
            }
            Protocol::Acp => {
                let stop_reason = if cancelled {
                    "cancelled"
                } else if failed {
                    "refusal"
                } else {
                    "end_turn"
                };
                self.rpc(output, json!({"id":3,"result":{"stopReason":stop_reason}}))
            }
        }
    }

    fn text(&self, output: &mut impl Write, text: &str) -> Result<()> {
        match self.protocol {
            Protocol::CodexExec => write_json(
                output,
                json!({"type":"item.updated","item":{"id":"synthetic_message",
                "type":"agent_message","text":text}}),
            ),
            Protocol::AppServer => self.rpc(
                output,
                json!({"method":"item/agentMessage/delta","params":{"threadId":SESSION_ID,
                "turnId":TURN_ID,"itemId":"synthetic_message","delta":text}}),
            ),
            Protocol::Claude => write_json(
                output,
                json!({"type":"stream_event","event":{"type":"content_block_delta",
                "index":0,"delta":{"type":"text_delta","text":text}}}),
            ),
            Protocol::Acp => self.rpc(
                output,
                json!({"method":"session/update","params":{"sessionId":SESSION_ID,
                "update":{"sessionUpdate":"agent_message_chunk",
                    "content":{"type":"text","text":text}}}}),
            ),
        }
    }

    fn extension(&self, output: &mut impl Write, fixture: Value) -> Result<()> {
        match self.protocol {
            Protocol::Claude => write_json(
                output,
                json!({"type":"fixture_extension","fixture":fixture}),
            ),
            Protocol::CodexExec => write_json(
                output,
                json!({"type":"fixture.extension","fixture":fixture}),
            ),
            Protocol::AppServer | Protocol::Acp => self.rpc(
                output,
                json!({"method":"fixture/extension","fixture":fixture}),
            ),
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
