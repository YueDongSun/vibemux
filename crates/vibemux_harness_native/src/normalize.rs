//! Conservative observations over vendor JSON; raw records remain untouched.
//!
//! A tool, content block, or OpenCode step ending never completes a request.
//! RPC correlation, process exit, and whole-request policy belong to transport.
//!
//! Codex, Claude, and ACP records use the shared ADR 029 classifiers, so the
//! daemon and this crate cannot disagree about one vendor record. Only the
//! direct OpenCode JSON output, which the daemon never reads, is classified
//! here.

use serde_json::Value;
use vibemux_harness::dispatch::observation;

use crate::contract::{Kind, Protocol};

/// Classify only verified discriminants; future extensions remain opaque.
pub fn classify(protocol: Protocol, value: &Value) -> Kind {
    if let Some(shared) = protocol.native_protocol() {
        return observation::classify(shared, value);
    }
    match protocol {
        Protocol::Opencode => classify_opencode(value),
        // No verified public event contract is assumed for these direct CLIs.
        _ => Kind::Opaque,
    }
}

// Official producer: anomalyco/opencode packages/opencode/src/cli/cmd/run.ts.
fn classify_opencode(value: &Value) -> Kind {
    match value["type"].as_str() {
        Some("step_start") => Kind::Started,
        Some("text" | "reasoning") => Kind::Text,
        Some("tool_use") => Kind::Tool,
        Some("error") => Kind::Failed,
        Some("step_finish")
            if value["part"]["tokens"].is_object() || value["part"]["cost"].is_number() =>
        {
            Kind::Usage
        }
        // A step_finish with reason=stop still ends only one step.
        _ => Kind::Opaque,
    }
}
