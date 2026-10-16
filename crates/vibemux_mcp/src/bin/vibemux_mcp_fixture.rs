#![forbid(unsafe_code)]
//! Synthetic owned reader, compiled only for explicit lifecycle qualification.

use serde_json::json;
use std::{process::Stdio, time::Duration};
use tokio::io::{AsyncWriteExt, BufReader};
use vibemux_mcp::{
    config::MAX_INPUT_BYTES,
    control_reader::{ReaderReply, ReaderRequest},
    framing::{LineReader, write_json},
};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    if mode == "descendant" {
        tokio::time::sleep(Duration::from_secs(60)).await;
        return;
    }
    let mut reader = LineReader::new(BufReader::new(tokio::io::stdin()), MAX_INPUT_BYTES);
    let mut writer = tokio::io::stdout();
    let mut count = 0;
    while let Ok(Some(bytes)) = reader.next().await {
        let Ok(request) = serde_json::from_slice::<ReaderRequest>(&bytes) else {
            return;
        };
        count += 1;
        match mode.as_str() {
            "hang" => std::future::pending::<()>().await,
            "healthy_then_hang" if count > 1 => std::future::pending::<()>().await,
            "crash" => std::process::exit(17),
            "malformed" => {
                let _ = writer.write_all(b"{invalid\n").await;
                let _ = writer.flush().await;
                std::future::pending::<()>().await;
            }
            "oversized" => {
                let _ = writer.write_all(&vec![b'x'; MAX_INPUT_BYTES + 1]).await;
                let _ = writer.flush().await;
                std::future::pending::<()>().await;
            }
            _ => {}
        }
        let mut descendant = None;
        if mode == "descendant_owner" {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .arg("descendant")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .env_clear();
            for key in ["SystemRoot", "WINDIR"] {
                if let Some(value) = std::env::var_os(key) {
                    command.env(key, value);
                }
            }
            #[cfg(windows)]
            command.creation_flags(0x0800_0000);
            // This happens only after the containing supervisor sent input.
            descendant = Some(command.spawn().expect("owned descendant fixture"));
        }
        let generation = request.generation + u64::from(mode == "stale_generation");
        let result = if mode == "non_object_result" {
            json!(["fixture_non_object"])
        } else {
            json!({
                "fixture_pid":std::process::id(),"descendant_pid":descendant.as_ref().map(std::process::Child::id),"sequence":request.sequence,
            })
        };
        let reply = ReaderReply {
            generation,
            sequence: request.sequence,
            result: Ok(result),
        };
        if write_json(&mut writer, &serde_json::to_value(reply).unwrap())
            .await
            .is_err()
        {
            return;
        }
        if mode == "idle_crash" {
            tokio::time::sleep(Duration::from_millis(100)).await;
            std::process::exit(19);
        }
        if let Some(mut descendant) = descendant {
            // Deliberately block this fixture on its exact descendant; the
            // supervisor must terminate the containing tree to end both.
            let _ = descendant.wait();
        }
    }
}

#[cfg(windows)]
use std::os::windows::process::CommandExt;
