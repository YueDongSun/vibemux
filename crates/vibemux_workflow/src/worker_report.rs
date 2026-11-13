//! Bounded worker claims, independent of workflow acceptance (ADR 057).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Sha256Digest, SpecIdentifier, canonical_json::canonical_digest};

pub const MAX_HUMAN_ACTION_BYTES: usize = 1000;
pub const MAX_SUMMARY_BYTES: usize = 4096;
pub const MAX_WORKER_REPORT_ENCODED_BYTES: usize = 6144;
pub const WORKER_REPORT_DOMAIN: &str = "vibemux.workflow.worker_report.v1";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerReportState {
    Progress,
    Waiting,
    Ready,
    Blocked,
    Failed,
    Dropped,
}

/// Untrusted checkpoint data. It cannot choose a session, contract, or Run.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerReport {
    pub state: WorkerReportState,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_action: Option<String>,
}

impl std::fmt::Debug for WorkerReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerReport")
            .field("state", &self.state)
            .field("summary_bytes", &self.summary.len())
            .field(
                "human_action_bytes",
                &self.human_action.as_ref().map(String::len),
            )
            .finish()
    }
}

impl WorkerReport {
    #[must_use]
    pub fn valid(&self) -> bool {
        let valid_text = |text: &str, maximum| {
            !text.trim().is_empty() && !text.contains('\0') && text.len() <= maximum
        };
        valid_text(&self.summary, MAX_SUMMARY_BYTES)
            && self
                .human_action
                .as_ref()
                .is_none_or(|text| valid_text(text, MAX_HUMAN_ACTION_BYTES))
            && (!matches!(
                self.state,
                WorkerReportState::Waiting | WorkerReportState::Blocked
            ) || self.human_action.is_some())
            && serde_json::to_vec(self)
                .is_ok_and(|bytes| bytes.len() <= MAX_WORKER_REPORT_ENCODED_BYTES)
    }

    /// An explicit question or a non-ready outcome fences automatic progress.
    #[must_use]
    pub fn needs_attention(&self) -> bool {
        self.human_action.is_some()
            || matches!(
                self.state,
                WorkerReportState::Waiting
                    | WorkerReportState::Blocked
                    | WorkerReportState::Failed
                    | WorkerReportState::Dropped
            )
    }

    pub fn digest(&self) -> Result<Sha256Digest, crate::canonical_json::CanonicalError> {
        canonical_digest(WORKER_REPORT_DOMAIN, self)
    }
}

/// Content-free, immutable facts bound by the writer to an owned attempt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerReportRecord {
    pub schema_version: u32,
    pub workflow_id: Uuid,
    pub request_id: Uuid,
    pub session_id: Uuid,
    pub task_key: SpecIdentifier,
    pub contract_id: Sha256Digest,
    pub lease_id: Uuid,
    pub lease_generation: u64,
    pub state: WorkerReportState,
    pub report_digest: Sha256Digest,
    pub content_sha256: Sha256Digest,
    pub summary_sha256: Sha256Digest,
    pub summary_bytes: u64,
    pub human_action_sha256: Option<Sha256Digest>,
    pub human_action_bytes: u64,
    pub needs_attention: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attention_is_structured_and_ready_is_only_a_claim() {
        for state in [
            WorkerReportState::Progress,
            WorkerReportState::Waiting,
            WorkerReportState::Ready,
            WorkerReportState::Blocked,
            WorkerReportState::Failed,
            WorkerReportState::Dropped,
        ] {
            let mut report = WorkerReport {
                state,
                summary: "result".into(),
                human_action: None,
            };
            assert_eq!(
                report.valid(),
                !matches!(
                    state,
                    WorkerReportState::Waiting | WorkerReportState::Blocked
                )
            );
            assert_eq!(
                report.needs_attention(),
                !matches!(
                    state,
                    WorkerReportState::Progress | WorkerReportState::Ready
                )
            );
            report.human_action = Some("Which interface should we keep?".into());
            assert!(report.valid());
            assert!(report.needs_attention());
        }
    }

    #[test]
    fn report_text_is_bounded_and_debug_is_redacted() {
        let mut report = WorkerReport {
            state: WorkerReportState::Progress,
            summary: "private summary".into(),
            human_action: Some("private question".into()),
        };
        let debug = format!("{report:?}");
        assert!(!debug.contains("private"));
        report.human_action = Some("x".repeat(MAX_HUMAN_ACTION_BYTES + 1));
        assert!(!report.valid());
        report.human_action = None;
        report.summary = "\u{0001}".repeat(MAX_SUMMARY_BYTES);
        assert!(
            !report.valid(),
            "escaped JSON must fit the control frame budget"
        );
        report.summary = "\0".into();
        assert!(!report.valid());
    }
}
