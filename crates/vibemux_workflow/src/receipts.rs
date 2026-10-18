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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::{
        CallOutcome, GatewayProtocol, ProviderAttribution, ReportedUsage, RouteRole,
    };

    const DECISION_ID: Uuid = Uuid::from_u128(1);
    const CALL_ID: Uuid = Uuid::from_u128(2);

    fn decision() -> DecisionRecord {
        DecisionRecord {
            decision_id: DECISION_ID,
            tool: "assign_task".to_string(),
            idempotency_key: "round_1".to_string(),
            workflow_version: 3,
            affected: vec!["task_a".to_string()],
            rationale_sha256: Sha256Digest::of(b"rationale"),
            rationale_bytes: 9,
            evidence_refs: Vec::new(),
            accepted: true,
            rejection_codes: Vec::new(),
        }
    }

    fn model_call() -> ModelCallReceipt {
        ModelCallReceipt {
            call_id: CALL_ID,
            role: RouteRole::Supervisor,
            requested_alias: "supervisor_model".to_string(),
            protocol: GatewayProtocol::AnthropicMessages,
            reported_model: None,
            effective_provider: ProviderAttribution::Unknown,
            usage: ReportedUsage::default(),
            http_status: Some(200),
            outcome: CallOutcome::Completed,
            failure: None,
            elapsed_ms: 10,
            request_sha256: Sha256Digest::of(b"request"),
            response_sha256: Some(Sha256Digest::of(b"response")),
            route_generation: Sha256Digest::of(b"routes"),
            attempt: 1,
        }
    }

    // No production path or other test builds these two kinds as a
    // `WorkflowReceipt`, yet the store files any receipt under these accessors.
    #[test]
    fn decision_and_model_call_receipts_name_their_own_id_and_subject() {
        let receipt = WorkflowReceipt::Decision(decision());
        assert_eq!(receipt.kind(), "decision");
        assert_eq!(receipt.receipt_id(), DECISION_ID);
        assert_eq!(
            receipt.subject_digest().ok(),
            Some(Sha256Digest::of(b"rationale"))
        );
        assert_eq!((receipt.workflow_id(), receipt.task_key()), (None, None));

        let receipt = WorkflowReceipt::ModelCall(model_call());
        assert_eq!(receipt.kind(), "model_call");
        assert_eq!(receipt.receipt_id(), CALL_ID);
        // The call is bound to the request it sent, not to the response or
        // the route table it ran under.
        assert_eq!(
            receipt.subject_digest().ok(),
            Some(Sha256Digest::of(b"request"))
        );
        assert_eq!((receipt.workflow_id(), receipt.task_key()), (None, None));
    }

    #[test]
    fn the_stored_kind_is_the_serialized_tag() {
        for receipt in [
            WorkflowReceipt::Decision(decision()),
            WorkflowReceipt::ModelCall(model_call()),
        ] {
            let encoded = serde_json::to_value(&receipt).expect("encode receipt");
            assert_eq!(encoded["kind"], receipt.kind());
            let decoded: WorkflowReceipt = serde_json::from_value(encoded).expect("decode receipt");
            assert_eq!(decoded, receipt);
            assert_eq!(decoded.digest(), receipt.digest());
        }
    }

    #[test]
    fn the_receipt_digest_covers_the_kind_and_every_field() {
        let receipt = WorkflowReceipt::Decision(decision());
        let digest = receipt.digest().expect("receipt digest");
        // The tagged receipt, not the bare record, is what is digested.
        assert_ne!(
            canonical_digest(RECEIPT_DIGEST_DOMAIN, &decision()).ok(),
            Some(digest)
        );
        let mut rejected = decision();
        rejected.accepted = false;
        assert_ne!(
            WorkflowReceipt::Decision(rejected).digest().ok(),
            Some(digest)
        );
        let mut retried = model_call();
        let original = WorkflowReceipt::ModelCall(retried.clone()).digest().ok();
        retried.attempt = 2;
        assert_ne!(WorkflowReceipt::ModelCall(retried).digest().ok(), original);
    }
}
