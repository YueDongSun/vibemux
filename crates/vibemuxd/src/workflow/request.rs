//! The operator's workflow request (ADR 031 §3): the original request text,
//! the compiled TaskSpec candidates, and the slot plan.
//!
//! The request is data. Its text is only ever hashed, span-checked, and
//! rendered as quoted contract data; nothing in it can select a slot, a
//! route, a suite, or a path outside what the policy and the pinned
//! workflow config allow.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use vibemux_workflow::{SpecIdentifier, task_spec::WorkflowMode};

use super::error::WorkflowError;

pub const WORKFLOW_REQUEST_SCHEMA_VERSION: u32 = 1;
/// The request travels in one Control frame with the policy.
pub const MAX_WORKFLOW_REQUEST_BYTES: usize = 48 * 1024;
pub const MAX_POLICY_BYTES: usize = 8 * 1024;
const MAX_REQUEST_KEY_BYTES: usize = 128;
const MAX_SHARED_CONTRACTS: usize = 8;
/// Compare mode needs at least two competitors.
const MIN_COMPETITORS: usize = 2;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRequest {
    pub schema_version: u32,
    /// Operator idempotency key: a repeated prepare returns the same
    /// workflow, a different request under the same key is refused.
    pub request_key: String,
    pub workflow_key: SpecIdentifier,
    pub mode: WorkflowMode,
    /// The original user request; its UTF-8 bytes are the source of truth.
    pub request_text: String,
    /// Compiled candidates. Empty means "compile through the configured
    /// compiler route", which is refused when no route is configured.
    #[serde(default)]
    pub task_specs: Vec<Value>,
    pub integration_suites: Vec<SpecIdentifier>,
    /// Task key to the slot(s) that run it. Cooperative tasks name one
    /// slot; the single compare task names every competitor.
    pub assignments: BTreeMap<SpecIdentifier, Vec<SpecIdentifier>>,
    pub reviewer_slot: SpecIdentifier,
    /// Shared contract artifact ID to a repository-relative file at the
    /// base commit, read by the daemon from the project checkout.
    #[serde(default)]
    pub shared_contract_paths: BTreeMap<SpecIdentifier, String>,
    /// Compare mode only: derive a separately identified, never promotable
    /// negative-control mutation of a collected candidate and prove the
    /// selector refuses it (V04).
    #[serde(default)]
    pub negative_control: bool,
}

impl WorkflowRequest {
    /// Parses and checks the request shape; semantic checks happen in
    /// [`super::prepare`].
    pub fn parse(value: Value) -> Result<Self, WorkflowError> {
        let encoded = serde_json::to_vec(&value).map_err(|_| WorkflowError::RequestInvalid)?;
        if encoded.len() > MAX_WORKFLOW_REQUEST_BYTES {
            return Err(WorkflowError::RequestInvalid);
        }
        let request: Self =
            serde_json::from_value(value).map_err(|_| WorkflowError::RequestInvalid)?;
        request.validate()?;
        Ok(request)
    }

    fn validate(&self) -> Result<(), WorkflowError> {
        let key_valid = !self.request_key.is_empty()
            && self.request_key.len() <= MAX_REQUEST_KEY_BYTES
            && self
                .request_key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
        let shape_valid = self.schema_version == WORKFLOW_REQUEST_SCHEMA_VERSION
            && key_valid
            && !self.request_text.trim().is_empty()
            && !self.assignments.is_empty()
            && self.shared_contract_paths.len() <= MAX_SHARED_CONTRACTS
            && self.assignments.values().all(|slots| {
                let mut unique = slots.clone();
                unique.sort();
                unique.dedup();
                !slots.is_empty() && unique.len() == slots.len()
            });
        if !shape_valid {
            return Err(WorkflowError::RequestInvalid);
        }
        match self.mode {
            WorkflowMode::Cooperate => {
                // One slot per task, and no slot runs two tasks at once.
                let mut slots: Vec<&SpecIdentifier> = self.assignments.values().flatten().collect();
                let count = slots.len();
                slots.sort();
                slots.dedup();
                if self.assignments.values().any(|slots| slots.len() != 1) || slots.len() != count {
                    return Err(WorkflowError::RequestInvalid);
                }
            }
            WorkflowMode::Compare => {
                if self.assignments.len() != 1
                    || self
                        .assignments
                        .values()
                        .any(|slots| slots.len() < MIN_COMPETITORS)
                {
                    return Err(WorkflowError::RequestInvalid);
                }
            }
            WorkflowMode::Review => return Err(WorkflowError::RequestInvalid),
        }
        if self.negative_control && self.mode != WorkflowMode::Compare {
            return Err(WorkflowError::RequestInvalid);
        }
        if self
            .assignments
            .values()
            .flatten()
            .any(|slot| slot == &self.reviewer_slot)
        {
            return Err(WorkflowError::RequestInvalid);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn request() -> Value {
        json!({
            "schema_version": 1,
            "request_key": "taskboard-1",
            "workflow_key": "taskboard",
            "mode": "cooperate",
            "request_text": "Build it.",
            "integration_suites": ["api"],
            "assignments": {"track_a": ["slot_a"], "track_b": ["slot_b"]},
            "reviewer_slot": "review",
        })
    }

    #[test]
    fn cooperative_tasks_take_one_distinct_slot_and_never_the_reviewer() {
        WorkflowRequest::parse(request()).expect("valid");
        let mut shared = request();
        shared["assignments"]["track_b"] = json!(["slot_a"]);
        assert!(WorkflowRequest::parse(shared).is_err());
        let mut reviewer = request();
        reviewer["assignments"]["track_b"] = json!(["review"]);
        assert!(WorkflowRequest::parse(reviewer).is_err());
        let mut key = request();
        key["request_key"] = json!("bad key");
        assert!(WorkflowRequest::parse(key).is_err());
    }

    #[test]
    fn compare_needs_one_task_with_two_competitors() {
        let mut compare = request();
        compare["mode"] = json!("compare");
        assert!(WorkflowRequest::parse(compare.clone()).is_err());
        compare["assignments"] = json!({"store": ["slot_a", "slot_b"]});
        WorkflowRequest::parse(compare).expect("valid compare");
    }
}
