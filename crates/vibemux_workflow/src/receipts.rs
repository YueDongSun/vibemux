//! Typed evidence the single writer persists for a workflow (ADR 031 §6).
//!
//! Every receipt is immutable once recorded and bound to its subject digest:
//! a candidate snapshot, a review or verification of that snapshot, an
//! integration tree, a selection, a supervisor decision, a model call, or a
//! context bundle. Receipts are content-free except where noted; prompt,
//! transcript, and bundle text live in the opt-in content store.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    Sha256Digest, SpecIdentifier,
    canonical_json::{CanonicalError, canonical_digest},
    context_bundle::ContextBundle,
    gates::{
        CandidateRecord, IntegrationReceipt, RECEIPT_DIGEST_DOMAIN, ReviewReceipt, VerifierReceipt,
    },
    gateway::ModelCallReceipt,
    selection::SelectionOutcome,
    snapshot::SnapshotManifest,
    supervisor::DecisionRecord,
};

/// A collected candidate, admitted or not. A rejected candidate is kept as
/// evidence of the failed attempt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateReceipt {
    pub schema_version: u32,
    pub receipt_id: Uuid,
    pub workflow_id: Uuid,
    pub attempt_request_id: Uuid,
    pub lease_id: Uuid,
    pub candidate: CandidateRecord,
    /// `None` when scope validation refused the observed changes.
    pub manifest: Option<SnapshotManifest>,
    pub admitted: bool,
    pub violation_codes: Vec<String>,
    /// The worktree changed again between the checkpoint and collection.
    pub mutated_after_checkpoint: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionRecord {
    pub schema_version: u32,
    pub receipt_id: Uuid,
    pub workflow_id: Uuid,
    pub contract_id: Sha256Digest,
    pub outcome: SelectionOutcome,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "body")]
pub enum WorkflowReceipt {
    Candidate(CandidateReceipt),
    Review(ReviewReceipt),
    Verification(VerifierReceipt),
    Integration(IntegrationReceipt),
    Selection(SelectionRecord),
    Decision(DecisionRecord),
    ModelCall(ModelCallReceipt),
    Bundle(ContextBundle),
}

impl WorkflowReceipt {
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Candidate(_) => "candidate",
            Self::Review(_) => "review",
            Self::Verification(_) => "verification",
            Self::Integration(_) => "integration",
            Self::Selection(_) => "selection",
            Self::Decision(_) => "decision",
            Self::ModelCall(_) => "model_call",
            Self::Bundle(_) => "bundle",
        }
    }

    #[must_use]
    pub fn receipt_id(&self) -> Uuid {
        match self {
            Self::Candidate(receipt) => receipt.receipt_id,
            Self::Review(receipt) => receipt.receipt_id,
            Self::Verification(receipt) => receipt.receipt_id,
            Self::Integration(receipt) => receipt.receipt_id,
            Self::Selection(receipt) => receipt.receipt_id,
            Self::Decision(record) => record.decision_id,
            Self::ModelCall(receipt) => receipt.call_id,
            Self::Bundle(bundle) => bundle.bundle_id,
        }
    }

    /// The workflow the receipt claims to belong to, where it carries one.
    #[must_use]
    pub fn workflow_id(&self) -> Option<Uuid> {
        match self {
            Self::Candidate(receipt) => Some(receipt.workflow_id),
            Self::Review(receipt) => Some(receipt.workflow_id),
            Self::Verification(receipt) => Some(receipt.workflow_id),
            Self::Integration(receipt) => Some(receipt.workflow_id),
            Self::Selection(receipt) => Some(receipt.workflow_id),
            Self::Bundle(bundle) => Some(bundle.workflow_id),
            Self::Decision(_) | Self::ModelCall(_) => None,
        }
    }

    /// The task the receipt concerns, if any.
    #[must_use]
    pub fn task_key(&self) -> Option<&SpecIdentifier> {
        match self {
            Self::Candidate(receipt) => Some(&receipt.candidate.task_key),
            Self::Review(receipt) => Some(&receipt.task_key),
            Self::Verification(receipt) => receipt.task_key.as_ref(),
            Self::Bundle(bundle) => Some(&bundle.source_task),
            Self::Integration(_) | Self::Selection(_) | Self::Decision(_) | Self::ModelCall(_) => {
                None
            }
        }
    }

    /// The digest the receipt is bound to.
    pub fn subject_digest(&self) -> Result<Sha256Digest, CanonicalError> {
        Ok(match self {
            Self::Candidate(receipt) => receipt.candidate.candidate_digest,
            Self::Review(receipt) => receipt.candidate_digest,
            Self::Verification(receipt) => receipt.subject_digest,
            Self::Integration(receipt) => receipt.integration_tree_digest,
            Self::Selection(receipt) => receipt.contract_id,
            Self::Decision(record) => record.rationale_sha256,
            Self::ModelCall(receipt) => receipt.request_sha256,
            Self::Bundle(bundle) => bundle.content_digest,
        })
    }

    pub fn digest(&self) -> Result<Sha256Digest, CanonicalError> {
        canonical_digest(RECEIPT_DIGEST_DOMAIN, self)
    }
}
