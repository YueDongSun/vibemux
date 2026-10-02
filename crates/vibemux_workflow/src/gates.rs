//! Acceptance levels, receipts, and the workflow gate (ADR 031 §6).
//!
//! The levels are distinct and only the last is workflow acceptance:
//! `turn_completed` < `candidate_artifact_collected` <
//! `independent_review_passed` < `local_verifier_passed` <
//! `integration_candidate_verified` < `workflow_accepted`. A worker's own
//! "tests passed" is a claim; a protocol-completed turn proves only that the
//! turn ended. A candidate passes its gate only with a passing review from
//! an independent session and route and a passing trusted verification,
//! both bound to the exact candidate and contract, with the verifier files
//! unchanged and the candidate bytes unchanged by review and testing. A
//! cancelled or failed workflow is never accepted, whatever a delayed child
//! result says.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vibemux_harness::AgentKind;

use crate::{
    Sha256Digest, SpecIdentifier,
    canonical_json::{CanonicalError, canonical_digest},
};

pub const RECEIPT_SCHEMA_VERSION: u32 = 1;
pub const RECEIPT_DIGEST_DOMAIN: &str = "vibemux.workflow.receipt.v1";

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptanceLevel {
    TurnCompleted,
    CandidateArtifactCollected,
    IndependentReviewPassed,
    LocalVerifierPassed,
    IntegrationCandidateVerified,
    WorkflowAccepted,
}

/// Where candidate bytes came from. Mutations are negative-test artifacts
/// derived by the trusted fixture and are never promotable.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CandidateOrigin {
    LiveWorker,
    FixtureWorker,
    InjectedMutation {
        source_candidate: Sha256Digest,
        mutation_id: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateRecord {
    pub candidate_digest: Sha256Digest,
    pub contract_id: Sha256Digest,
    pub task_key: SpecIdentifier,
    pub worker_run_id: Uuid,
    pub worker_session_id: Uuid,
    pub worker_route: String,
    pub worker_harness: AgentKind,
    pub lease_generation: u64,
    pub origin: CandidateOrigin,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewVerdict {
    Pass,
    ChangesRequested,
    Blocked,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewReceipt {
    pub schema_version: u32,
    pub receipt_id: Uuid,
    pub workflow_id: Uuid,
    pub task_key: SpecIdentifier,
    pub contract_id: Sha256Digest,
    pub candidate_digest: Sha256Digest,
    pub reviewer_run_id: Uuid,
    pub reviewer_session_id: Uuid,
    pub reviewer_route: String,
    pub reviewer_harness: AgentKind,
    pub verdict: ReviewVerdict,
    pub findings_count: u32,
    pub findings_digest: Sha256Digest,
    /// The candidate bytes re-hashed after the review equal the collected
    /// candidate.
    pub candidate_unchanged: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SuiteStatus {
    Passed,
    Failed,
    /// A prerequisite (for example a browser) is missing. Never a pass.
    Blocked,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SuiteResult {
    pub suite_id: SpecIdentifier,
    pub status: SuiteStatus,
    pub tests_total: u32,
    pub tests_passed: u32,
    pub tests_failed: u32,
    pub failed_tests: Vec<String>,
    pub command: Vec<String>,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub output_sha256: Sha256Digest,
    pub blocked_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VerifierReceipt {
    pub schema_version: u32,
    pub receipt_id: Uuid,
    pub workflow_id: Uuid,
    /// The task verified, or `None` for an integration candidate.
    pub task_key: Option<SpecIdentifier>,
    pub contract_ids: Vec<Sha256Digest>,
    /// Digest of the snapshot or integration tree actually tested.
    pub subject_digest: Sha256Digest,
    pub suites: Vec<SuiteResult>,
    pub verifier_files_digest: Sha256Digest,
    pub tool_versions: BTreeMap<String, String>,
    /// The tested bytes re-hashed after the run equal the subject.
    pub subject_unchanged: bool,
}

impl VerifierReceipt {
    pub fn digest(&self) -> Result<Sha256Digest, CanonicalError> {
        canonical_digest(RECEIPT_DIGEST_DOMAIN, self)
    }

    #[must_use]
    pub fn suite(&self, suite_id: &SpecIdentifier) -> Option<&SuiteResult> {
        self.suites.iter().find(|suite| &suite.suite_id == suite_id)
    }
}

impl ReviewReceipt {
    pub fn digest(&self) -> Result<Sha256Digest, CanonicalError> {
        canonical_digest(RECEIPT_DIGEST_DOMAIN, self)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationReceipt {
    pub schema_version: u32,
    pub receipt_id: Uuid,
    pub workflow_id: Uuid,
    pub base_commit: String,
    pub applied_candidates: Vec<Sha256Digest>,
    pub contract_ids: Vec<Sha256Digest>,
    pub integration_tree_digest: Sha256Digest,
    pub patch_sha256: Sha256Digest,
    pub conflicts: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct GateRequirements<'a> {
    pub required_suites: &'a [SpecIdentifier],
    pub admitted_verifier_digest: Sha256Digest,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case", tag = "failure", content = "detail")]
pub enum GateFailure {
    ReviewMissing,
    ReviewNotPassed,
    ReviewWrongCandidate,
    ReviewWrongContract,
    ReviewerNotIndependent,
    ReviewMutatedCandidate,
    VerificationMissing,
    VerificationWrongCandidate,
    VerificationWrongContract,
    SuiteMissing(String),
    SuiteFailed(String),
    SuiteBlocked(String),
    VerifierModified,
    CandidateMutatedDuringVerification,
    MutationArtifactNotPromotable,
    WorkflowNotActive(String),
    TaskMissing(String),
    IntegrationMissing,
    IntegrationMismatch,
    IntegrationConflicts,
}

impl GateFailure {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ReviewMissing => "gate_review_missing",
            Self::ReviewNotPassed => "gate_review_not_passed",
            Self::ReviewWrongCandidate => "gate_review_wrong_candidate",
            Self::ReviewWrongContract => "gate_review_wrong_contract",
            Self::ReviewerNotIndependent => "gate_reviewer_not_independent",
            Self::ReviewMutatedCandidate => "gate_review_mutated_candidate",
            Self::VerificationMissing => "gate_verification_missing",
            Self::VerificationWrongCandidate => "gate_verification_wrong_candidate",
            Self::VerificationWrongContract => "gate_verification_wrong_contract",
            Self::SuiteMissing(_) => "gate_suite_missing",
            Self::SuiteFailed(_) => "gate_suite_failed",
            Self::SuiteBlocked(_) => "gate_suite_blocked",
            Self::VerifierModified => "gate_verifier_modified",
            Self::CandidateMutatedDuringVerification => "gate_candidate_mutated",
            Self::MutationArtifactNotPromotable => "gate_mutation_not_promotable",
            Self::WorkflowNotActive(_) => "gate_workflow_not_active",
            Self::TaskMissing(_) => "gate_task_missing",
            Self::IntegrationMissing => "gate_integration_missing",
            Self::IntegrationMismatch => "gate_integration_mismatch",
            Self::IntegrationConflicts => "gate_integration_conflicts",
        }
    }
}

/// Proof that one candidate passed its gate.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CandidateAcceptance {
    pub candidate_digest: Sha256Digest,
    pub contract_id: Sha256Digest,
    pub review_receipt: Sha256Digest,
    pub verifier_receipt: Sha256Digest,
    pub level: AcceptanceLevel,
}

/// Checks the suites of `receipt` for `subject` under `contract_ids`.
fn verification_failures(
    receipt: &VerifierReceipt,
    subject: Sha256Digest,
    contract_ids: &[Sha256Digest],
    requirements: GateRequirements<'_>,
) -> Vec<GateFailure> {
    let mut failures = Vec::new();
    if receipt.subject_digest != subject {
        failures.push(GateFailure::VerificationWrongCandidate);
    }
    let mut expected = contract_ids.to_vec();
    expected.sort();
    let mut actual = receipt.contract_ids.clone();
    actual.sort();
    if expected != actual {
        failures.push(GateFailure::VerificationWrongContract);
    }
    if receipt.verifier_files_digest != requirements.admitted_verifier_digest {
        failures.push(GateFailure::VerifierModified);
    }
    if !receipt.subject_unchanged {
        failures.push(GateFailure::CandidateMutatedDuringVerification);
    }
    for suite_id in requirements.required_suites {
        match receipt.suite(suite_id) {
            None => failures.push(GateFailure::SuiteMissing(suite_id.to_string())),
            Some(suite) => match suite.status {
                SuiteStatus::Passed if suite.tests_failed == 0 && suite.tests_passed > 0 => {}
                SuiteStatus::Blocked => {
                    failures.push(GateFailure::SuiteBlocked(suite_id.to_string()))
                }
                _ => failures.push(GateFailure::SuiteFailed(suite_id.to_string())),
            },
        }
    }
    failures
}

/// The gate one candidate must pass before it can be integrated or selected.
pub fn candidate_gate(
    candidate: &CandidateRecord,
    review: Option<&ReviewReceipt>,
    verification: Option<&VerifierReceipt>,
    requirements: GateRequirements<'_>,
) -> Result<CandidateAcceptance, Vec<GateFailure>> {
    let mut failures = Vec::new();
    if matches!(candidate.origin, CandidateOrigin::InjectedMutation { .. }) {
        failures.push(GateFailure::MutationArtifactNotPromotable);
    }
    match review {
        None => failures.push(GateFailure::ReviewMissing),
        Some(review) => {
            if review.verdict != ReviewVerdict::Pass {
                failures.push(GateFailure::ReviewNotPassed);
            }
            if review.candidate_digest != candidate.candidate_digest {
                failures.push(GateFailure::ReviewWrongCandidate);
            }
            if review.contract_id != candidate.contract_id {
                failures.push(GateFailure::ReviewWrongContract);
            }
            if review.reviewer_run_id == candidate.worker_run_id
                || review.reviewer_session_id == candidate.worker_session_id
                || review.reviewer_route == candidate.worker_route
            {
                failures.push(GateFailure::ReviewerNotIndependent);
            }
            if !review.candidate_unchanged {
                failures.push(GateFailure::ReviewMutatedCandidate);
            }
        }
    }
    match verification {
        None => failures.push(GateFailure::VerificationMissing),
        Some(receipt) => failures.extend(verification_failures(
            receipt,
            candidate.candidate_digest,
            &[candidate.contract_id],
            requirements,
        )),
    }
    if !failures.is_empty() {
        failures.sort();
        failures.dedup();
        return Err(failures);
    }
    let (Some(review), Some(verification)) = (review, verification) else {
        return Err(vec![GateFailure::ReviewMissing]);
    };
    Ok(CandidateAcceptance {
        candidate_digest: candidate.candidate_digest,
        contract_id: candidate.contract_id,
        review_receipt: review
            .digest()
            .map_err(|_| vec![GateFailure::ReviewMissing])?,
        verifier_receipt: verification
            .digest()
            .map_err(|_| vec![GateFailure::VerificationMissing])?,
        level: AcceptanceLevel::LocalVerifierPassed,
    })
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowPhase {
    Prepared,
    Running,
    Paused,
    Integrating,
    Accepted,
    Failed,
    CancelRequested,
    Cancelled,
    Blocked,
}

impl WorkflowPhase {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Integrating => "integrating",
            Self::Accepted => "accepted",
            Self::Failed => "failed",
            Self::CancelRequested => "cancel_requested",
            Self::Cancelled => "cancelled",
            Self::Blocked => "blocked",
        }
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Accepted | Self::Failed | Self::Cancelled)
    }

    /// Whether new work may be admitted and results promoted.
    #[must_use]
    pub const fn admits_work(self) -> bool {
        matches!(self, Self::Running | Self::Integrating)
    }
}

/// Cooperative acceptance: every expected task has an accepted candidate,
/// the integration applied exactly those candidates without conflicts, and
/// the trusted verifier passed on the integration tree.
pub fn cooperative_acceptance(
    phase: WorkflowPhase,
    expected_tasks: &[SpecIdentifier],
    accepted: &[(SpecIdentifier, CandidateAcceptance)],
    integration: Option<&IntegrationReceipt>,
    integration_verification: Option<&VerifierReceipt>,
    requirements: GateRequirements<'_>,
) -> Result<AcceptanceLevel, Vec<GateFailure>> {
    let mut failures = Vec::new();
    if phase != WorkflowPhase::Integrating {
        failures.push(GateFailure::WorkflowNotActive(phase.as_str().to_string()));
    }
    for task in expected_tasks {
        if !accepted.iter().any(|(key, _)| key == task) {
            failures.push(GateFailure::TaskMissing(task.to_string()));
        }
    }
    match integration {
        None => failures.push(GateFailure::IntegrationMissing),
        Some(integration) => {
            let mut applied = integration.applied_candidates.clone();
            applied.sort();
            let mut expected: Vec<Sha256Digest> = accepted
                .iter()
                .map(|(_, acceptance)| acceptance.candidate_digest)
                .collect();
            expected.sort();
            if applied != expected {
                failures.push(GateFailure::IntegrationMismatch);
            }
            if !integration.conflicts.is_empty() {
                failures.push(GateFailure::IntegrationConflicts);
            }
            let contracts: Vec<Sha256Digest> = accepted
                .iter()
                .map(|(_, acceptance)| acceptance.contract_id)
                .collect();
            match integration_verification {
                None => failures.push(GateFailure::VerificationMissing),
                Some(receipt) => failures.extend(verification_failures(
                    receipt,
                    integration.integration_tree_digest,
                    &contracts,
                    requirements,
                )),
            }
        }
    }
    if failures.is_empty() {
        Ok(AcceptanceLevel::WorkflowAccepted)
    } else {
        failures.sort();
        failures.dedup();
        Err(failures)
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    pub fn suite(id: &str, status: SuiteStatus, failed: u32) -> SuiteResult {
        SuiteResult {
            suite_id: SpecIdentifier::new(id).expect("suite"),
            status,
            tests_total: 10,
            tests_passed: 10 - failed,
            tests_failed: failed,
            failed_tests: vec![],
            command: vec!["node".into(), "run_verifier.mjs".into()],
            exit_code: Some(i32::from(failed > 0)),
            duration_ms: 100,
            output_sha256: Sha256Digest::of(b"output"),
            blocked_reason: None,
        }
    }

    pub fn candidate() -> CandidateRecord {
        CandidateRecord {
            candidate_digest: Sha256Digest::of(b"candidate"),
            contract_id: Sha256Digest::of(b"contract"),
            task_key: SpecIdentifier::new("track_a").expect("key"),
            worker_run_id: Uuid::from_u128(1),
            worker_session_id: Uuid::from_u128(2),
            worker_route: "worker_a".into(),
            worker_harness: AgentKind::Claude,
            lease_generation: 1,
            origin: CandidateOrigin::FixtureWorker,
        }
    }

    pub fn review(candidate: &CandidateRecord) -> ReviewReceipt {
        ReviewReceipt {
            schema_version: RECEIPT_SCHEMA_VERSION,
            receipt_id: Uuid::from_u128(10),
            workflow_id: Uuid::from_u128(9),
            task_key: candidate.task_key.clone(),
            contract_id: candidate.contract_id,
            candidate_digest: candidate.candidate_digest,
            reviewer_run_id: Uuid::from_u128(3),
            reviewer_session_id: Uuid::from_u128(4),
            reviewer_route: "reviewer".into(),
            reviewer_harness: AgentKind::Codex,
            verdict: ReviewVerdict::Pass,
            findings_count: 0,
            findings_digest: Sha256Digest::of(b"[]"),
            candidate_unchanged: true,
        }
    }

    pub fn verification(
        subject: Sha256Digest,
        contracts: Vec<Sha256Digest>,
        suites: Vec<SuiteResult>,
    ) -> VerifierReceipt {
        VerifierReceipt {
            schema_version: RECEIPT_SCHEMA_VERSION,
            receipt_id: Uuid::from_u128(11),
            workflow_id: Uuid::from_u128(9),
            task_key: None,
            contract_ids: contracts,
            subject_digest: subject,
            suites,
            verifier_files_digest: Sha256Digest::of(b"verifier"),
            tool_versions: BTreeMap::new(),
            subject_unchanged: true,
        }
    }

    pub fn api_suite() -> Vec<SpecIdentifier> {
        vec![SpecIdentifier::new("api").expect("suite")]
    }
}

#[cfg(test)]
mod tests {
    use super::{fixtures::*, *};

    fn requirements(suites: &[SpecIdentifier]) -> GateRequirements<'_> {
        GateRequirements {
            required_suites: suites,
            admitted_verifier_digest: Sha256Digest::of(b"verifier"),
        }
    }

    #[test]
    fn a_reviewed_and_verified_candidate_passes() {
        let candidate = candidate();
        let suites = api_suite();
        let receipt = verification(
            candidate.candidate_digest,
            vec![candidate.contract_id],
            vec![suite("api", SuiteStatus::Passed, 0)],
        );
        let acceptance = candidate_gate(
            &candidate,
            Some(&review(&candidate)),
            Some(&receipt),
            requirements(&suites),
        )
        .expect("accepted");
        assert_eq!(acceptance.level, AcceptanceLevel::LocalVerifierPassed);
    }

    #[test]
    fn a_protocol_completed_but_failing_candidate_cannot_pass() {
        let candidate = candidate();
        let suites = api_suite();
        let failing = verification(
            candidate.candidate_digest,
            vec![candidate.contract_id],
            vec![suite("api", SuiteStatus::Failed, 3)],
        );
        let failures = candidate_gate(
            &candidate,
            Some(&review(&candidate)),
            Some(&failing),
            requirements(&suites),
        )
        .expect_err("rejected");
        assert_eq!(failures, vec![GateFailure::SuiteFailed("api".into())]);
        // Blocked is never a pass, and a "passed" status with failures is not.
        let blocked = verification(
            candidate.candidate_digest,
            vec![candidate.contract_id],
            vec![suite("api", SuiteStatus::Blocked, 0)],
        );
        assert_eq!(
            candidate_gate(
                &candidate,
                Some(&review(&candidate)),
                Some(&blocked),
                requirements(&suites)
            ),
            Err(vec![GateFailure::SuiteBlocked("api".into())])
        );
        let lying = verification(
            candidate.candidate_digest,
            vec![candidate.contract_id],
            vec![suite("api", SuiteStatus::Passed, 1)],
        );
        assert!(
            candidate_gate(
                &candidate,
                Some(&review(&candidate)),
                Some(&lying),
                requirements(&suites)
            )
            .is_err()
        );
        assert!(
            candidate_gate(
                &candidate,
                Some(&review(&candidate)),
                None,
                requirements(&suites)
            )
            .is_err()
        );
    }

    #[test]
    fn receipts_must_bind_the_same_candidate_and_contract() {
        let candidate = candidate();
        let suites = api_suite();
        let mut other_review = review(&candidate);
        other_review.candidate_digest = Sha256Digest::of(b"other");
        other_review.contract_id = Sha256Digest::of(b"old contract");
        let other_verification = verification(
            Sha256Digest::of(b"other"),
            vec![Sha256Digest::of(b"old contract")],
            vec![suite("api", SuiteStatus::Passed, 0)],
        );
        let failures = candidate_gate(
            &candidate,
            Some(&other_review),
            Some(&other_verification),
            requirements(&suites),
        )
        .expect_err("rejected");
        for expected in [
            GateFailure::ReviewWrongCandidate,
            GateFailure::ReviewWrongContract,
            GateFailure::VerificationWrongCandidate,
            GateFailure::VerificationWrongContract,
        ] {
            assert!(failures.contains(&expected), "{expected:?}");
        }
    }

    #[test]
    fn reviewers_must_be_independent_and_must_not_mutate() {
        let candidate = candidate();
        let suites = api_suite();
        let receipt = verification(
            candidate.candidate_digest,
            vec![candidate.contract_id],
            vec![suite("api", SuiteStatus::Passed, 0)],
        );
        for change in 0..3 {
            let mut same = review(&candidate);
            match change {
                0 => same.reviewer_run_id = candidate.worker_run_id,
                1 => same.reviewer_session_id = candidate.worker_session_id,
                _ => same.reviewer_route = candidate.worker_route.clone(),
            }
            assert_eq!(
                candidate_gate(
                    &candidate,
                    Some(&same),
                    Some(&receipt),
                    requirements(&suites)
                ),
                Err(vec![GateFailure::ReviewerNotIndependent])
            );
        }
        let mut mutating = review(&candidate);
        mutating.candidate_unchanged = false;
        assert_eq!(
            candidate_gate(
                &candidate,
                Some(&mutating),
                Some(&receipt),
                requirements(&suites)
            ),
            Err(vec![GateFailure::ReviewMutatedCandidate])
        );
    }

    #[test]
    fn modified_verifiers_mutated_subjects_and_mutation_artifacts_fail() {
        let mut candidate = candidate();
        let suites = api_suite();
        let mut receipt = verification(
            candidate.candidate_digest,
            vec![candidate.contract_id],
            vec![suite("api", SuiteStatus::Passed, 0)],
        );
        receipt.verifier_files_digest = Sha256Digest::of(b"edited verifier");
        receipt.subject_unchanged = false;
        let failures = candidate_gate(
            &candidate,
            Some(&review(&candidate)),
            Some(&receipt),
            requirements(&suites),
        )
        .expect_err("rejected");
        assert!(failures.contains(&GateFailure::VerifierModified));
        assert!(failures.contains(&GateFailure::CandidateMutatedDuringVerification));
        candidate.origin = CandidateOrigin::InjectedMutation {
            source_candidate: Sha256Digest::of(b"live"),
            mutation_id: "title_length_off_by_one".into(),
        };
        let clean = verification(
            candidate.candidate_digest,
            vec![candidate.contract_id],
            vec![suite("api", SuiteStatus::Passed, 0)],
        );
        assert_eq!(
            candidate_gate(
                &candidate,
                Some(&review(&candidate)),
                Some(&clean),
                requirements(&suites)
            ),
            Err(vec![GateFailure::MutationArtifactNotPromotable])
        );
    }

    #[test]
    fn cancelled_workflows_are_never_accepted_by_late_results() {
        let candidate = candidate();
        let suites = api_suite();
        let receipt = verification(
            candidate.candidate_digest,
            vec![candidate.contract_id],
            vec![suite("api", SuiteStatus::Passed, 0)],
        );
        let acceptance = candidate_gate(
            &candidate,
            Some(&review(&candidate)),
            Some(&receipt),
            requirements(&suites),
        )
        .expect("candidate");
        let integration = IntegrationReceipt {
            schema_version: RECEIPT_SCHEMA_VERSION,
            receipt_id: Uuid::from_u128(12),
            workflow_id: Uuid::from_u128(9),
            base_commit: "abc".into(),
            applied_candidates: vec![candidate.candidate_digest],
            contract_ids: vec![candidate.contract_id],
            integration_tree_digest: Sha256Digest::of(b"tree"),
            patch_sha256: Sha256Digest::of(b"patch"),
            conflicts: vec![],
        };
        let integrated = verification(
            Sha256Digest::of(b"tree"),
            vec![candidate.contract_id],
            vec![suite("api", SuiteStatus::Passed, 0)],
        );
        let expected = [candidate.task_key.clone()];
        let accepted = [(candidate.task_key.clone(), acceptance)];
        assert_eq!(
            cooperative_acceptance(
                WorkflowPhase::Integrating,
                &expected,
                &accepted,
                Some(&integration),
                Some(&integrated),
                requirements(&suites)
            ),
            Ok(AcceptanceLevel::WorkflowAccepted)
        );
        for phase in [
            WorkflowPhase::CancelRequested,
            WorkflowPhase::Cancelled,
            WorkflowPhase::Failed,
        ] {
            assert_eq!(
                cooperative_acceptance(
                    phase,
                    &expected,
                    &accepted,
                    Some(&integration),
                    Some(&integrated),
                    requirements(&suites)
                ),
                Err(vec![GateFailure::WorkflowNotActive(
                    phase.as_str().to_string()
                )])
            );
        }
        let mut partial = integration.clone();
        partial
            .applied_candidates
            .push(Sha256Digest::of(b"unapproved"));
        partial.conflicts.push("src/server.mjs".into());
        let failures = cooperative_acceptance(
            WorkflowPhase::Integrating,
            &expected,
            &accepted,
            Some(&partial),
            Some(&integrated),
            requirements(&suites),
        )
        .expect_err("rejected");
        assert!(failures.contains(&GateFailure::IntegrationMismatch));
        assert!(failures.contains(&GateFailure::IntegrationConflicts));
    }
}
