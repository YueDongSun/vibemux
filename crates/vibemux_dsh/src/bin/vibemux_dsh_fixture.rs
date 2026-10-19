#![forbid(unsafe_code)]
//! A stand-in DSH SDK runtime for driver and terminal-UI tests.
//!
//! It speaks the newline-delimited JSON-RPC 2.0 shapes of
//! `@deepseek-ai/dsh-sdk-protocol` (ADR 032) and nothing else: no model, no
//! network, no filesystem. Tests never need the real vendor runtime, and no
//! test can spend inference.
//!
//! Usage: `vibemux_dsh_fixture <mode>`
//!
//! The daemon launches the real launcher pair (`node <entry script>
//! --profile <profile>`), so when a `--profile` argument is present its value
//! selects the mode. A daemon test therefore chooses a behaviour the same way
//! an operator chooses a profile, without inventing a second launch shape.
//!
//! | mode | behaviour |
//! |---|---|
//! | `serve` | `initialize`, then one completed turn per prompt, then `shutdown` |
//! | `hang` | accepts stdin and never answers `initialize` |
//! | `error-turn` | completes the handshake, then ends each turn with `error` |
//! | `foreign` | emits another session's `turn/end` before this session's |
//! | `garbage` | writes a non-JSON line to stdout |
//! | `no-turn-end` | streams text but never ends the turn |
//! | `flood` | streams [`FLOOD_CHUNKS`] text chunks in one burst before each turn ends |
//! | `exit-early` | exits before answering anything |
//! | `exit-after-ready` | answers `initialize`, then exits successfully |
//! | `crash-after-ready` | answers `initialize`, then exits with status 3 |

use std::io::{self, BufRead, Write};

/// Wire-stable server identity this protocol pins.
const SERVER_NAME: &str = "deepseek-harness-sdk-runtime";
/// The protocol's own hardcoded version literal; clients must treat it as
/// opaque.
const SERVER_VERSION: &str = "0.0.1";
/// Text chunks one `flood` turn streams. Enough to keep the client's reader
/// appending records for many poll round trips, and few enough to stay well
/// inside the default record bounds.
const FLOOD_CHUNKS: u64 = 400;

/// The mode this run serves: `--profile <value>` when the launcher pair
/// supplies one, else the first positional argument.
fn mode() -> String {
    let arguments: Vec<String> = std::env::args().collect();
    if let Some(index) = arguments.iter().position(|value| value == "--profile")
        && let Some(profile) = arguments.get(index + 1)
    {
        return profile.clone();
    }
    arguments
        .get(1)
        .cloned()
        .unwrap_or_else(|| "serve".to_string())
}

fn main() {
    let mode = mode();
    if mode == "exit-early" {
        return;
    }
    let stdin = io::stdin();
    let mut turn: u64 = 0;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { return };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(frame) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(id) = frame.get("id").cloned() else {
            continue;
        };
        let method = frame
            .get("method")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        match method {
            "initialize" => {
                if mode == "hang" {
                    continue;
                }
                reply(
                    id,
                    &serde_json::json!({"serverInfo": {
                        "name": SERVER_NAME, "version": SERVER_VERSION}}),
                );
                if mode == "exit-after-ready" {
                    return;
                }
                if mode == "crash-after-ready" {
                    std::process::exit(3);
                }
            }
            "session/prompt" => {
                let session_id = frame
                    .pointer("/params/sessionId")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                reply(id, &serde_json::json!({"messageId": format!("msg_{turn}")}));
                turn += 1;
                if mode == "foreign" {
                    // The real runtime streams every session in the process,
                    // so a sibling's completion can arrive first.
                    notify(
                        "session.event",
                        &serde_json::json!({"sessionId": "another_session",
                            "event": {"type": "turn/end", "seq": 1, "time": 1,
                                "data": {"turn": 99, "reason": {"kind": "completed"}}}}),
                    );
                }
                if mode == "garbage" {
                    let mut stdout = io::stdout();
                    let _ = stdout.write_all(b"this is not a JSON frame\n");
                    let _ = stdout.flush();
                    continue;
                }
                notify(
                    "session.event",
                    &serde_json::json!({"sessionId": session_id,
                        "event": {"type": "turn/start", "seq": 2, "time": 1,
                            "data": {"turn": turn}}}),
                );
                notify(
                    "session.event",
                    &serde_json::json!({"sessionId": session_id,
                        "event": {"type": "assistant/chunk", "seq": 3, "time": 2,
                            "data": {"turn": turn, "step": 1, "chunk": {
                                "type": "text-delta", "index": 0,
                                "text": "fixture reply"}}}}),
                );
                if mode == "flood" {
                    flood(&session_id, turn);
                }
                notify(
                    "session.status",
                    &serde_json::json!({"sessionId": session_id, "status": "idle"}),
                );
                if mode == "no-turn-end" {
                    continue;
                }
                let reason = if mode == "error-turn" {
                    serde_json::json!({"kind": "error", "error": {
                        "message": "fixture failure", "code": "FIXTURE"}})
                } else {
                    serde_json::json!({"kind": "completed"})
                };
                notify(
                    "session.event",
                    &serde_json::json!({"sessionId": session_id,
                        "event": {"type": "turn/end", "seq": 4, "time": 3,
                            "data": {"turn": turn, "reason": reason}}}),
                );
            }
            "shutdown" => {
                reply(id, &serde_json::json!({}));
                let _ = io::stdout().flush();
                return;
            }
            _ => {
                let mut stdout = io::stdout();
                let _ = writeln!(
                    stdout,
                    "{}",
                    serde_json::json!({"jsonrpc": "2.0", "id": id,
                        "error": {"code": -32603,
                            "message": "unknown DeepSeek Harness SDK runtime method"}})
                );
                let _ = stdout.flush();
            }
        }
    }
}

fn reply(id: serde_json::Value, result: &serde_json::Value) {
    let mut stdout = io::stdout();
    let _ = writeln!(
        stdout,
        "{}",
        serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result})
    );
    let _ = stdout.flush();
}

/// Writes [`FLOOD_CHUNKS`] text chunks through one buffer, so they reach the
/// client in large reads and its reader appends records back to back.
fn flood(session_id: &str, turn: u64) {
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    for index in 0..FLOOD_CHUNKS {
        let _ = writeln!(
            stdout,
            "{}",
            serde_json::json!({"jsonrpc": "2.0", "method": "session.event",
                "params": {"sessionId": session_id,
                    "event": {"type": "assistant/chunk", "seq": 3, "time": 2,
                        "data": {"turn": turn, "step": 1, "chunk": {
                            "type": "text-delta", "index": index,
                            "text": "flood"}}}}})
        );
    }
    let _ = stdout.flush();
}

fn notify(method: &str, params: &serde_json::Value) {
    let mut stdout = io::stdout();
    let _ = writeln!(
        stdout,
        "{}",
        serde_json::json!({"jsonrpc": "2.0", "method": method, "params": params})
    );
    let _ = stdout.flush();
}
