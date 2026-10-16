use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::Error;

pub const SCHEMA_VERSION: u32 = 1;
pub const EXECUTE_METHOD: &str = "harness:execute";
pub const PROBE_METHOD: &str = "harness:probe";
pub const RECORD_EVENT: &str = "harness:record";
pub const MAX_PROMPT_BYTES: usize = 128 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Harness {
    Codex,
    Claude,
    Opencode,
    Copilot,
    Grok,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Protocol {
    #[serde(rename = "codex_exec")]
    Codex,
    #[serde(rename = "codex_app_server")]
    Appserver,
    #[serde(rename = "claude_stream_json")]
    Claude,
    #[serde(rename = "opencode_json")]
    Opencode,
    #[serde(rename = "copilot_json")]
    Copilot,
    #[serde(rename = "acp")]
    Acp,
    #[serde(rename = "grok_stream_json")]
    Grok,
}

impl Protocol {
    pub fn accepts(self, harness: Harness) -> bool {
        matches!(
            (self, harness),
            (Self::Codex | Self::Appserver, Harness::Codex)
                | (Self::Claude, Harness::Claude)
                | (Self::Opencode | Self::Acp, Harness::Opencode)
                | (Self::Copilot | Self::Acp, Harness::Copilot)
                | (Self::Grok | Self::Acp, Harness::Grok)
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Started,
    Text,
    Tool,
    Approval,
    Usage,
    Completed,
    Failed,
    Cancelled,
    Opaque,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Completed,
    Failed,
    Cancelled,
    Unverified,
    Probed,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub schema_version: u32,
    pub request_id: Uuid,
    pub harness: Harness,
    pub prompt: String,
}

impl Request {
    pub fn validate(&self) -> Result<(), Error> {
        if self.schema_version != SCHEMA_VERSION || self.request_id.is_nil() {
            return Err(Error::new("harness_invalid_request"));
        }
        if self.prompt.trim().is_empty() || self.prompt.len() > MAX_PROMPT_BYTES {
            return Err(Error::new("harness_invalid_prompt"));
        }
        Ok(())
    }
}

impl fmt::Debug for Request {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Request")
            .field("request_id", &self.request_id)
            .field("harness", &self.harness)
            .field("prompt_bytes", &self.prompt.len())
            .finish_non_exhaustive()
    }
}

/// Complete vendor JSON bytes, including unknown fields, held in bounded memory.
/// Serialization is for the private local plugin pipe, never public reports.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub sequence: u64,
    pub kind: Kind,
    raw_json: String,
}

impl Record {
    pub fn parse(protocol: Protocol, sequence: u64, raw_json: String) -> Result<Self, Error> {
        let value: Value = serde_json::from_str(&raw_json)
            .map_err(|source| Error::with_source("harness_invalid_json", source))?;
        if !value.is_object() || sequence == 0 {
            return Err(Error::new("harness_invalid_record"));
        }
        let kind = crate::normalize::classify(protocol, &value);
        Ok(Self {
            sequence,
            kind,
            raw_json,
        })
    }

    pub fn raw_json(&self) -> &str {
        &self.raw_json
    }

    pub fn value(&self) -> Result<Value, Error> {
        serde_json::from_str(&self.raw_json)
            .map_err(|source| Error::with_source("harness_invalid_json", source))
    }

    pub fn summary(&self) -> Observation {
        Observation {
            sequence: self.sequence,
            kind: self.kind,
            bytes: self.raw_json.len(),
        }
    }
}

impl fmt::Debug for Record {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.summary().fmt(formatter)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub sequence: u64,
    pub kind: Kind,
    pub bytes: usize,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Completion {
    pub schema_version: u32,
    pub status: Status,
    pub record_count: u64,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub exit_code: Option<i32>,
    pub forced_termination: bool,
    pub error_code: Option<String>,
}

pub struct Outcome {
    pub request_id: Uuid,
    pub harness: Harness,
    pub records: Vec<Record>,
    pub completion: Completion,
}

impl Outcome {
    pub fn summary(&self) -> Value {
        serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "request_id": self.request_id,
            "harness": self.harness,
            "completion": self.completion,
            "observations": self.records.iter().map(Record::summary).collect::<Vec<_>>()
        })
    }
}

impl fmt::Debug for Outcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.summary().fmt(formatter)
    }
}
