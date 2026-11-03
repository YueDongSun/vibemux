#![forbid(unsafe_code)]
//! Test-only persistent vendor. Approval mode writes one configured test artifact.
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
};
use uuid::Uuid;
use vibemuxd::chat_session::fixture_runtime::{
    FIXTURE_APPROVAL_CONTENT as APPROVED_FIXTURE_CONTENT, FIXTURE_APPROVAL_FILE,
};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureConfig {
    mode: String,
    ready_path: Option<PathBuf>,
    launch_path: Option<PathBuf>,
    cwd_path: Option<PathBuf>,
    effect_path: Option<PathBuf>,
}

struct PendingClaudeApproval {
    request_id: String,
    input: Value,
    session: String,
    request: Value,
    ordinal: u64,
}

fn main() {
    if run().is_err() {
        eprintln!("chat_fixture_failed");
        std::process::exit(2);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().any(|argument| argument == "--fixture_hold_descendant") {
        std::thread::sleep(std::time::Duration::from_secs(30));
        return Ok(());
    }
    let executable = std::env::current_exe()?;
    let config: FixtureConfig = serde_json::from_slice(&std::fs::read(
        executable
            .parent()
            .ok_or("fixture_directory")?
            .join("chat_fixture.json"),
    )?)?;
    if let Some(path) = &config.launch_path {
        std::fs::write(path, b"fixture_started")?;
    }
    if let Some(path) = &config.cwd_path {
        std::fs::write(path, std::env::current_dir()?.to_string_lossy().as_bytes())?;
    }
    let mut ordinal = 0_u64;
    let mut active = String::new();
    let mut claude_request = Value::Null;
    let mut claude_session = String::new();
    let mut pending_claude_approval: Option<PendingClaudeApproval> = None;
    let mut output = io::stdout().lock();
    for line in io::stdin().lock().lines() {
        let value: Value = serde_json::from_str(&line?)?;
        match value["method"].as_str() {
            Some("initialize") => emit(
                &mut output,
                json!({"id":value["id"],"result":{"userAgent":"codex/0.166.0"}}),
            )?,
            Some("thread/start" | "thread/resume") => {
                let new_thread = Uuid::new_v4().to_string();
                let id = value["params"]["threadId"].as_str().unwrap_or(&new_thread);
                emit(
                    &mut output,
                    json!({"id":value["id"],"result":{
                    "thread":{"id":id,"sessionId":id,"ephemeral":false,"status":{"type":"idle"},"turns":[]},
                    "approvalPolicy":"never","sandbox":{"type":"readOnly","networkAccess":false},
                    "cwd":value["params"]["cwd"],"model":"fixture_model"}}),
                )?;
            }
            Some("turn/start") => {
                ordinal += 1;
                active = format!("fixture_turn_{ordinal}");
                mark_ready(&config, ordinal)?;
                if config.mode == "crash" {
                    std::process::exit(7);
                }
                emit(
                    &mut output,
                    json!({"id":value["id"],"result":{"turn":{"id":active,"status":"inProgress","items":[]}}}),
                )?;
                if config.mode == "descendant_pipe" {
                    let mut child = std::process::Command::new(std::env::current_exe()?);
                    child
                        .arg("--fixture_hold_descendant")
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::inherit())
                        .stderr(std::process::Stdio::null());
                    #[cfg(windows)]
                    std::os::windows::process::CommandExt::creation_flags(&mut child, 0x0800_0000);
                    let child = child.spawn()?;
                    if let Some(path) = &config.ready_path {
                        std::fs::write(path.with_extension("descendant"), child.id().to_string())?;
                    }
                    return Ok(());
                }
                if config.mode == "hang"
                    || config.mode == "cancel_race"
                    || config.mode == "slow_shutdown"
                {
                    continue;
                }
                let thread = if config.mode == "wrong_correlation" {
                    "foreign_thread"
                } else {
                    value["params"]["threadId"].as_str().ok_or("thread")?
                };
                if config.mode == "denial" {
                    emit(
                        &mut output,
                        json!({"id":format!("approval_{ordinal}"),"method":"item/commandExecution/requestApproval","params":{"threadId":thread,"turnId":active,"itemId":"command","command":"NEVER_EXECUTE"}}),
                    )?;
                }
                emit(
                    &mut output,
                    json!({"method":"item/agentMessage/delta","params":{"threadId":thread,"turnId":active,"itemId":format!("message_{ordinal}"),"delta":synthetic_answer(&config.mode, ordinal)}}),
                )?;
                let mut terminal_items = Vec::new();
                if matches!(
                    config.mode.as_str(),
                    "command_tools" | "command_tools_overflow" | "command_tools_truncated"
                ) {
                    let count = if config.mode == "command_tools_overflow" {
                        65
                    } else {
                        1
                    };
                    let command_output = if config.mode == "command_tools_truncated" {
                        "PRIVATE_COMMAND_OUTPUT_中文\n".repeat(2048)
                    } else {
                        "PRIVATE_COMMAND_OUTPUT_中文".to_owned()
                    };
                    let cwd = std::env::current_dir()?.to_string_lossy().into_owned();
                    for index in 0..count {
                        let item = json!({"type":"commandExecution","id":format!("command_{ordinal}_{index}"),
                            "pluginId":null,"scriptPath":null,"processId":null,"source":"agent","commandActions":[],
                            "command":"synthetic read-only command","cwd":cwd,"status":"completed",
                            "aggregatedOutput":command_output,"exitCode":0,"durationMs":42});
                        emit(
                            &mut output,
                            json!({"method":"item/completed","params":{"threadId":thread,"turnId":active,"item":item}}),
                        )?;
                        // Terminal replay exercises deduplication across both paths.
                        terminal_items.push(item);
                    }
                }
                if matches!(
                    config.mode.as_str(),
                    "file_tools"
                        | "file_tools_overflow"
                        | "file_tools_truncated"
                        | "native_surfaces"
                ) {
                    let command = json!({"type":"commandExecution","id":format!("mixed_command_{ordinal}"),
                        "command":"synthetic reported command","cwd":std::env::current_dir()?.to_string_lossy(),
                        "status":"failed","aggregatedOutput":"PRIVATE_MIXED_OUTPUT","exitCode":7,"durationMs":1});
                    emit(
                        &mut output,
                        json!({"method":"item/completed","params":{"threadId":thread,"turnId":active,"item":command}}),
                    )?;
                    terminal_items.push(command);
                    let count = if config.mode == "file_tools_overflow" {
                        64
                    } else {
                        1
                    };
                    for index in 0..count {
                        let changes = if config.mode == "file_tools_truncated" {
                            (0..17).map(|change| json!({"path":format!("reported_{change}.txt"),"kind":{"type":"update","move_path":null},
                                "diff":"+PRIVATE_FILE_NEW_中文\n".repeat(2048)})).collect::<Vec<_>>()
                        } else if config.mode == "native_surfaces" {
                            vec![
                                json!({"path":"DEMO_surface.txt","kind":{"type":"update","move_path":"DEMO_moved_surface.txt"},"diff":native_surface_diff()}),
                                json!({"path":"DEMO_added.txt","kind":{"type":"add"},"diff":"+DEMO added line\n"}),
                                json!({"path":"DEMO_deleted.txt","kind":{"type":"delete"},"diff":"-DEMO deleted line\n"}),
                            ]
                        } else {
                            vec![
                                json!({"path":"guarded.txt","kind":{"type":"update","move_path":"reported_move.txt"},"diff":"-PRIVATE_OLD\n+PRIVATE_NEW\n"}),
                                json!({"path":"reported_add.txt","kind":{"type":"add"},"diff":"+PRIVATE_ADD\n"}),
                                json!({"path":"guarded.txt","kind":{"type":"delete"},"diff":"-PRIVATE_DELETE\n"}),
                            ]
                        };
                        let item = json!({"type":"fileChange","id":format!("file_{ordinal}_{index}"),"status":"completed","changes":changes});
                        emit(
                            &mut output,
                            json!({"method":"item/completed","params":{"threadId":thread,"turnId":active,"item":item}}),
                        )?;
                        terminal_items.push(item);
                    }
                }
                emit(
                    &mut output,
                    json!({"method":"turn/completed","params":{"threadId":thread,"turn":{"id":active,"status":"completed","items":terminal_items}}}),
                )?;
            }
            Some("turn/interrupt") => {
                emit(&mut output, json!({"id":value["id"],"result":{}}))?;
                let status = if config.mode == "cancel_race" {
                    "completed"
                } else {
                    "interrupted"
                };
                emit(
                    &mut output,
                    json!({"method":"turn/completed","params":{"threadId":value["params"]["threadId"],"turn":{"id":active,"status":status,"items":[]}}}),
                )?;
            }
            _ if value["type"] == "control_request" => {
                emit(
                    &mut output,
                    json!({"type":"control_response","response":{"subtype":"success","request_id":value["request_id"],"response":{"claude_code_version":"2.1.304"}}}),
                )?;
                if value["request"]["subtype"] == "interrupt" && !claude_session.is_empty() {
                    pending_claude_approval = None;
                    claude_result(
                        &mut output,
                        &claude_session,
                        &claude_request,
                        ordinal,
                        "aborted_streaming",
                    )?;
                }
            }
            _ if value["type"] == "control_response" && is_approval_mode(&config.mode) => {
                let Some(pending) = pending_claude_approval.as_ref() else {
                    continue;
                };
                let response = &value["response"];
                if response["subtype"] != "success" || response["request_id"] != pending.request_id
                {
                    continue;
                }
                let decision = &response["response"];
                let allowed = decision["behavior"] == "allow"
                    && decision["updatedInput"] == pending.input
                    && decision.get("updatedPermissions").is_none();
                if !allowed && decision["behavior"] != "deny" {
                    continue;
                }
                if let Some(pending) = pending_claude_approval.take() {
                    if allowed {
                        let path = config.effect_path.as_ref().ok_or("approval_effect_path")?;
                        if config.mode == "claude_runtime_approval" {
                            let cwd = std::fs::canonicalize(std::env::current_dir()?)?;
                            if path.file_name() != Some(std::ffi::OsStr::new(FIXTURE_APPROVAL_FILE))
                                || std::fs::canonicalize(path.parent().ok_or("effect_parent")?)?
                                    != cwd
                            {
                                return Err("runtime_effect_scope".into());
                            }
                            let mut file = std::fs::OpenOptions::new()
                                .write(true)
                                .create_new(true)
                                .open(FIXTURE_APPROVAL_FILE)?;
                            file.write_all(APPROVED_FIXTURE_CONTENT.as_bytes())?;
                            file.sync_all()?;
                        } else {
                            std::fs::write(path, APPROVED_FIXTURE_CONTENT)?;
                        }
                    }
                    claude_result(
                        &mut output,
                        &pending.session,
                        &pending.request,
                        pending.ordinal,
                        "completed",
                    )?;
                }
            }
            _ if value["type"] == "user" => {
                if config.mode == "claude_runtime_approval" && ordinal != 0 {
                    return Err("runtime_second_input".into());
                }
                ordinal += 1;
                claude_request = value["uuid"].clone();
                claude_session = value["session_id"].as_str().ok_or("session")?.to_owned();
                mark_ready(&config, ordinal)?;
                if config.mode == "crash" {
                    std::process::exit(7);
                }
                if ordinal == 1 {
                    let approval_mode = is_approval_mode(&config.mode);
                    emit(
                        &mut output,
                        json!({"type":"system","subtype":"init","uuid":Uuid::new_v4(),"session_id":claude_session,
                    "cwd":std::env::current_dir()?,"tools":if approval_mode {json!(["Write"])} else {json!([])},
                    "mcp_servers":[],"permissionMode":if approval_mode {"manual"} else {"dontAsk"},"plugins":[]}),
                    )?;
                }
                if config.mode == "hang" {
                    continue;
                }
                if is_approval_mode(&config.mode) {
                    pending_claude_approval = Some(claude_approval(
                        &mut output,
                        &config,
                        &claude_session,
                        &claude_request,
                        ordinal,
                    )?);
                    if config.mode == "claude_runtime_approval" {
                        emit(
                            &mut output,
                            json!({"type":"assistant","uuid":Uuid::new_v4(),"session_id":claude_session,
                            "parent_tool_use_id":null,"message":{"id":Uuid::new_v4(),"role":"assistant","content":[{"type":"text","text":"synthetic waiting output"}]}}),
                        )?;
                    }
                    continue;
                }
                let session = if config.mode == "wrong_correlation" {
                    Uuid::new_v4().to_string()
                } else {
                    claude_session.clone()
                };
                claude_result(&mut output, &session, &claude_request, ordinal, "completed")?;
            }
            _ => {}
        }
    }
    if config.mode == "slow_shutdown" {
        let path = config.ready_path.as_ref().ok_or("shutdown marker")?;
        std::fs::write(path.with_extension("closing"), b"closing")?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !path.with_extension("release").exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    Ok(())
}

fn is_approval_mode(mode: &str) -> bool {
    matches!(mode, "claude_approval" | "claude_runtime_approval")
}

fn synthetic_answer(mode: &str, ordinal: u64) -> String {
    let answer = format!("synthetic answer {ordinal}");
    if matches!(mode, "native_links" | "native_surfaces") {
        format!("{answer}\n\n[DEMO HTTPS reference](https://example.org/)")
    } else {
        answer
    }
}

fn native_surface_diff() -> String {
    (0..300)
        .map(|index| {
            if index == 270 {
                "+DEMO_SURFACE_FIND_TARGET".to_owned()
            } else {
                format!("+DEMO surface line {index}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn claude_approval(
    output: &mut impl Write,
    config: &FixtureConfig,
    session: &str,
    request: &Value,
    ordinal: u64,
) -> Result<PendingClaudeApproval, Box<dyn std::error::Error>> {
    let path = config.effect_path.as_ref().ok_or("approval_effect_path")?;
    let input = json!({"file_path":path,"content":APPROVED_FIXTURE_CONTENT});
    let request_id = Uuid::new_v4().to_string();
    let tool_use_id = Uuid::new_v4().to_string();
    emit(
        output,
        json!({"type":"assistant","uuid":Uuid::new_v4(),"session_id":session,
            "parent_tool_use_id":null,"message":{"id":Uuid::new_v4(),"type":"message",
            "role":"assistant","model":"fixture_model","content":[{"type":"tool_use",
            "id":tool_use_id,"name":"Write","input":input}],"stop_reason":"tool_use",
            "stop_sequence":null,"usage":{"input_tokens":0,"output_tokens":0}}}),
    )?;
    emit(
        output,
        json!({"type":"control_request","request_id":request_id,"request":{
            "subtype":"can_use_tool","tool_name":"Write","input":input,
            "tool_use_id":tool_use_id}}),
    )?;
    Ok(PendingClaudeApproval {
        request_id,
        input,
        session: session.to_owned(),
        request: request.clone(),
        ordinal,
    })
}

fn claude_result(
    output: &mut impl Write,
    session: &str,
    request: &Value,
    ordinal: u64,
    reason: &str,
) -> io::Result<()> {
    let mut result = json!({"type":"result","uuid":Uuid::new_v4(),"session_id":session,
        "subtype":"success","is_error":false,"terminal_reason":reason,
        "result":format!("synthetic answer {ordinal}"),"result_index":ordinal-1});
    // A fixture cannot invent correlation the actual caller did not provide.
    if request.is_string() {
        result["user_message_uuid"] = request.clone();
        result["user_message_uuids"] = json!([request]);
    }
    emit(output, result)
}

fn mark_ready(config: &FixtureConfig, ordinal: u64) -> io::Result<()> {
    if let Some(path) = &config.ready_path {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(file, "{}:{ordinal}", std::process::id())?;
        file.flush()?;
    }
    Ok(())
}

fn emit(output: &mut impl Write, value: Value) -> io::Result<()> {
    serde_json::to_writer(&mut *output, &value)?;
    output.write_all(b"\n")?;
    output.flush()
}

#[cfg(test)]
mod presentation_tests {
    #[test]
    fn surface_diff_keeps_a_unique_late_marker_inside_the_retained_limit() {
        let diff = super::native_surface_diff();
        assert_eq!(diff.lines().count(), 300);
        assert!(diff.len() < 16 * 1024);
        assert_eq!(diff.matches("DEMO_SURFACE_FIND_TARGET").count(), 1);
        assert!(
            diff.lines()
                .nth(270)
                .expect("late marker")
                .contains("DEMO_SURFACE_FIND_TARGET")
        );
        assert_eq!(
            super::synthetic_answer("native_surfaces", 1),
            super::synthetic_answer("native_links", 1)
        );
    }
    #[test]
    fn native_link_fixture_adds_only_its_fixed_synthetic_reference() {
        assert_eq!(super::synthetic_answer("normal", 2), "synthetic answer 2");
        assert_eq!(
            super::synthetic_answer("native_links", 1),
            "synthetic answer 1\n\n[DEMO HTTPS reference](https://example.org/)"
        );
    }
}
