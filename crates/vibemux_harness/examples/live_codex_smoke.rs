//! Opt-in model roundtrip. Never run as part of the offline test suite.

use std::path::Path;

use tokio::sync::watch;
use uuid::Uuid;
use vibemux_harness::{
    Error,
    contract::{Harness, Kind, Request, SCHEMA_VERSION, Status},
    dispatch::Dispatcher,
};

const EXPECTED_TEXT: &str = "VIBEMUX_NATIVE_SMOKE_OK";
const PROMPT: &str = "Respond with exactly VIBEMUX_NATIVE_SMOKE_OK and nothing else. Do not call any tools, inspect files, change files, access network tools, or start other agents.";

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{}", error.code());
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Error> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.len() != 2 {
        return Err(Error::new("harness_smoke_requires_config_and_plugin"));
    }
    let dispatcher = Dispatcher::new(Path::new(&arguments[0]), Path::new(&arguments[1]))?;
    let (_sender, receiver) = watch::channel(false);
    let outcome = dispatcher
        .execute(
            Request {
                schema_version: SCHEMA_VERSION,
                request_id: Uuid::new_v4(),
                harness: Harness::Codex,
                prompt: PROMPT.to_string(),
            },
            receiver,
        )
        .await?;
    let mut streamed_text = String::new();
    let mut final_text = None;
    let mut tool_events = 0;
    for record in &outcome.records {
        if matches!(record.kind, Kind::Tool | Kind::Approval) {
            tool_events += 1;
        }
        let value = record.value()?;
        if value["method"] == "item/agentMessage/delta" {
            if let Some(text) = value
                .pointer("/params/delta")
                .and_then(serde_json::Value::as_str)
            {
                streamed_text.push_str(text);
            }
        }
        if value["method"] == "item/completed"
            && value
                .pointer("/params/item/type")
                .is_some_and(|kind| kind == "agentMessage")
        {
            final_text = value
                .pointer("/params/item/text")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
        }
    }
    let text_matches = final_text.as_deref().unwrap_or(&streamed_text).trim() == EXPECTED_TEXT;
    let verified =
        outcome.completion.status == Status::Completed && text_matches && tool_events == 0;
    println!(
        "{}",
        serde_json::json!({"verified":verified,"text_matches":text_matches,
        "tool_events":tool_events,"record_count":outcome.records.len(),"completion":outcome.completion})
    );
    if !verified {
        return Err(Error::new("harness_live_smoke_failed"));
    }
    Ok(())
}
