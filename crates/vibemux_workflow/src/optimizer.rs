//! Bounded, versioned self-optimization of prompt, context, and scheduling
//! parameters (ADR 031 §7).
//!
//! The optimizer may change only the fields of [`OptimizablePolicy`]:
//! wording of optimizable template blocks, context retrieval order and
//! summary budget, preapproved decomposition hints, and slot ranking
//! weights. Requirements, permissions, tests, held-out cases, hard budgets,
//! providers, state-machine rules, accepted history, and active contracts
//! are not representable in a candidate diff, and a diff naming any other
//! field is rejected before evaluation.
//!
//! Candidates are evaluated on fixed train/dev datasets; only the final
//! eligible candidate may touch the holdout, and each consultation is
//! counted. Selection applies hard gates first, then verified success, then
//! known cost. "No improvement" is a valid result. Promotion affects future
//! admissions only and keeps a rollback pointer.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::{
    Sha256Digest, SpecIdentifier,
    canonical_json::{CanonicalError, canonical_digest},
    slots::RankingWeights,
    task_spec::{ContextSourceKind, MAX_CONTEXT_BUNDLE_BYTES},
    templates::{ResolvedTemplate, TemplateError},
};

pub const OPTIMIZABLE_POLICY_DOMAIN: &str = "vibemux.workflow.optimizable_policy.v1";
pub const MIN_SUMMARY_BUDGET_BYTES: u32 = 512;
pub const MAX_RANKING_WEIGHT: u32 = 100;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecompositionHint {
    InterfaceFirst,
    TestsBeforeImplementation,
    SmallestChangeFirst,
    AskBeforeAssuming,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OptimizablePolicy {
    /// Template version -> optimizable block ID -> replacement wording.
    pub template_overrides: BTreeMap<String, BTreeMap<String, String>>,
    pub retrieval_order: Vec<ContextSourceKind>,
    pub summary_budget_bytes: u32,
    pub decomposition_hints: Vec<DecompositionHint>,
    pub ranking_weights: RankingWeights,
}

impl OptimizablePolicy {
    #[must_use]
    pub fn baseline() -> Self {
        Self {
            template_overrides: BTreeMap::new(),
            retrieval_order: vec![
                ContextSourceKind::VerifierReceipts,
                ContextSourceKind::CandidateSnapshot,
                ContextSourceKind::WorkerClaims,
            ],
            summary_budget_bytes: 8192,
            decomposition_hints: vec![],
            ranking_weights: RankingWeights::BASELINE,
        }
    }

    pub fn digest(&self) -> Result<Sha256Digest, CanonicalError> {
        canonical_digest(OPTIMIZABLE_POLICY_DOMAIN, self)
    }

    pub fn validate(&self) -> Result<(), OptimizerRejection> {
        for (template_version, overrides) in &self.template_overrides {
            let version = SpecIdentifier::new(template_version.clone())
                .map_err(|_| OptimizerRejection::ForbiddenTemplateChange)?;
            ResolvedTemplate::resolve(&version, overrides).map_err(|error| match error {
                TemplateError::ForbiddenOverride | TemplateError::UnknownTemplate => {
                    OptimizerRejection::ForbiddenTemplateChange
                }
                TemplateError::InvalidOverride => OptimizerRejection::InvalidValue,
            })?;
        }
        let mut sources = self.retrieval_order.clone();
        sources.sort();
        sources.dedup();
        if sources.len() != self.retrieval_order.len() || self.retrieval_order.is_empty() {
            return Err(OptimizerRejection::InvalidValue);
        }
        if !(MIN_SUMMARY_BUDGET_BYTES..=MAX_CONTEXT_BUNDLE_BYTES)
            .contains(&self.summary_budget_bytes)
        {
            return Err(OptimizerRejection::InvalidValue);
        }
        let weights = self.ranking_weights;
        if [
            weights.known_budget,
            weights.live_evidence,
            weights.native_resume,
        ]
        .iter()
        .any(|weight| *weight > MAX_RANKING_WEIGHT)
        {
            return Err(OptimizerRejection::InvalidValue);
        }
        Ok(())
    }
}

/// One proposed change, addressed by a dotted field path.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyChange {
    pub field: String,
    pub value: Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateDiff {
    pub changes: Vec<PolicyChange>,
    pub expected_benefit: String,
    pub possible_regressions: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Error, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OptimizerRejection {
    #[error("candidate changes a field outside the allowlist")]
    ForbiddenField,
    #[error("candidate changes an immutable template block")]
    ForbiddenTemplateChange,
    #[error("candidate value is invalid")]
    InvalidValue,
    #[error("candidate makes no change")]
    EmptyDiff,
    #[error("evaluation cases differ from the fixed dataset")]
    DatasetMismatch,
    #[error("evaluation was run against a different policy")]
    PolicyMismatch,
    #[error("a hard gate failed")]
    HardGateFailure,
    #[error("no verified improvement over the baseline")]
    NoImprovement,
    #[error("regression on the protected holdout")]
    HoldoutRegression,
    #[error("holdout was already consulted for this cycle")]
    HoldoutContaminated,
    #[error("optimizer budget is exhausted")]
    BudgetExhausted,
    #[error("version history does not allow this operation")]
    InvalidHistory,
}

impl OptimizerRejection {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::ForbiddenField => "optimizer_forbidden_field",
            Self::ForbiddenTemplateChange => "optimizer_forbidden_template_change",
            Self::InvalidValue => "optimizer_invalid_value",
            Self::EmptyDiff => "optimizer_empty_diff",
            Self::DatasetMismatch => "optimizer_dataset_mismatch",
            Self::PolicyMismatch => "optimizer_policy_mismatch",
            Self::HardGateFailure => "optimizer_hard_gate_failure",
            Self::NoImprovement => "optimizer_no_improvement",
            Self::HoldoutRegression => "optimizer_holdout_regression",
            Self::HoldoutContaminated => "optimizer_holdout_contaminated",
            Self::BudgetExhausted => "optimizer_budget_exhausted",
            Self::InvalidHistory => "optimizer_invalid_history",
        }
    }
}

/// Applies an allowlisted diff to `base` and validates the result.
pub fn apply_candidate(
    base: &OptimizablePolicy,
    diff: &CandidateDiff,
) -> Result<OptimizablePolicy, OptimizerRejection> {
    if diff.changes.is_empty() {
        return Err(OptimizerRejection::EmptyDiff);
    }
    let mut candidate = base.clone();
    for change in &diff.changes {
        let parts: Vec<&str> = change.field.split('.').collect();
        let invalid = |_| OptimizerRejection::InvalidValue;
        match parts.as_slice() {
            ["template_overrides", template, block] => {
                let text = change
                    .value
                    .as_str()
                    .ok_or(OptimizerRejection::InvalidValue)?;
                candidate
                    .template_overrides
                    .entry((*template).to_string())
                    .or_default()
                    .insert((*block).to_string(), text.to_string());
            }
            ["retrieval_order"] => {
                candidate.retrieval_order =
                    serde_json::from_value(change.value.clone()).map_err(invalid)?;
            }
            ["summary_budget_bytes"] => {
                candidate.summary_budget_bytes =
                    serde_json::from_value(change.value.clone()).map_err(invalid)?;
            }
            ["decomposition_hints"] => {
                candidate.decomposition_hints =
                    serde_json::from_value(change.value.clone()).map_err(invalid)?;
            }
            ["ranking_weights", "known_budget"] => {
                candidate.ranking_weights.known_budget =
                    serde_json::from_value(change.value.clone()).map_err(invalid)?;
            }
            ["ranking_weights", "live_evidence"] => {
                candidate.ranking_weights.live_evidence =
                    serde_json::from_value(change.value.clone()).map_err(invalid)?;
            }
            ["ranking_weights", "native_resume"] => {
                candidate.ranking_weights.native_resume =
                    serde_json::from_value(change.value.clone()).map_err(invalid)?;
            }
            _ => return Err(OptimizerRejection::ForbiddenField),
        }
    }
    candidate.validate()?;
    if candidate == *base {
        return Err(OptimizerRejection::EmptyDiff);
    }
    Ok(candidate)
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DatasetSplit {
    Train,
    Dev,
    Holdout,
}

/// A fixed, versioned evaluation set.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationDataset {
    pub dataset_id: SpecIdentifier,
    pub split: DatasetSplit,
    pub case_ids: Vec<SpecIdentifier>,
}

impl EvaluationDataset {
    pub fn digest(&self) -> Result<Sha256Digest, CanonicalError> {
        canonical_digest("vibemux.workflow.evaluation_dataset.v1", self)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CaseResult {
    pub case_id: SpecIdentifier,
    /// Security, permission, scope, and verifier-integrity gates.
    pub hard_gates_passed: bool,
    pub verified_success: bool,
    pub model_requests: Option<u32>,
    pub elapsed_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationRun {
    pub policy_digest: Sha256Digest,
    pub dataset_digest: Sha256Digest,
    pub results: Vec<CaseResult>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationSummary {
    pub cases: u32,
    pub hard_gate_failures: u32,
    pub verified_successes: u32,
    /// `None` when any case's usage is unknown.
    pub model_requests: Option<u64>,
    pub elapsed_ms: u64,
}

/// Summarizes a run, checking it covers exactly the dataset's cases under
/// the stated policy.
pub fn summarize(
    run: &EvaluationRun,
    dataset: &EvaluationDataset,
    policy: &OptimizablePolicy,
) -> Result<EvaluationSummary, OptimizerRejection> {
    if run.policy_digest
        != policy
            .digest()
            .map_err(|_| OptimizerRejection::InvalidValue)?
    {
        return Err(OptimizerRejection::PolicyMismatch);
    }
    if run.dataset_digest
        != dataset
            .digest()
            .map_err(|_| OptimizerRejection::InvalidValue)?
    {
        return Err(OptimizerRejection::DatasetMismatch);
    }
    let mut expected = dataset.case_ids.clone();
    expected.sort();
    let mut observed: Vec<SpecIdentifier> = run
        .results
        .iter()
        .map(|result| result.case_id.clone())
        .collect();
    observed.sort();
    if expected != observed {
        return Err(OptimizerRejection::DatasetMismatch);
    }
    let count = |predicate: fn(&CaseResult) -> bool| {
        u32::try_from(
            run.results
                .iter()
                .filter(|result| predicate(result))
                .count(),
        )
        .unwrap_or(u32::MAX)
    };
    Ok(EvaluationSummary {
        cases: u32::try_from(run.results.len()).unwrap_or(u32::MAX),
        hard_gate_failures: count(|result| !result.hard_gates_passed),
        verified_successes: count(|result| result.verified_success && result.hard_gates_passed),
        model_requests: run
            .results
            .iter()
            .map(|result| result.model_requests.map(u64::from))
            .sum(),
        elapsed_ms: run.results.iter().map(|result| result.elapsed_ms).sum(),
    })
}

/// Hard gates first, then verified success, then known cost. Equal success
/// with unknown cost is no improvement.
pub fn compare_to_baseline(
    baseline: &EvaluationSummary,
    candidate: &EvaluationSummary,
) -> Result<(), OptimizerRejection> {
    if candidate.hard_gate_failures > 0 {
        return Err(OptimizerRejection::HardGateFailure);
    }
    if candidate.verified_successes > baseline.verified_successes {
        return Ok(());
    }
    if candidate.verified_successes == baseline.verified_successes {
        if let (Some(candidate_cost), Some(baseline_cost)) =
            (candidate.model_requests, baseline.model_requests)
        {
            if candidate_cost < baseline_cost {
                return Ok(());
            }
        }
    }
    Err(OptimizerRejection::NoImprovement)
}

/// Counts holdout consultations per cycle; a second one contaminates it.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct HoldoutLedger {
    pub consultations: u32,
}

impl HoldoutLedger {
    pub fn consult(&mut self) -> Result<(), OptimizerRejection> {
        if self.consultations > 0 {
            return Err(OptimizerRejection::HoldoutContaminated);
        }
        self.consultations += 1;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OptimizerLimits {
    pub max_candidates: u32,
    pub max_model_requests: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateOutcome {
    pub candidate_digest: Option<Sha256Digest>,
    pub rejection: Option<OptimizerRejection>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CycleDecision {
    pub outcomes: Vec<CandidateOutcome>,
    pub promoted: Option<Sha256Digest>,
    pub optimization_improved: bool,
}

/// The baseline policy's results on the fixed dev and holdout sets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CycleBaseline {
    pub dev: EvaluationSummary,
    pub holdout: EvaluationSummary,
}

/// One bounded cycle over already-evaluated candidates. `evaluate_dev`
/// returns a candidate's dev summary; `evaluate_holdout` is called at most
/// once, for the best dev candidate, through the ledger.
pub fn run_cycle(
    base: &OptimizablePolicy,
    baseline: CycleBaseline,
    diffs: &[CandidateDiff],
    limits: OptimizerLimits,
    ledger: &mut HoldoutLedger,
    mut evaluate_dev: impl FnMut(&OptimizablePolicy) -> Result<EvaluationSummary, OptimizerRejection>,
    mut evaluate_holdout: impl FnMut(
        &OptimizablePolicy,
    ) -> Result<EvaluationSummary, OptimizerRejection>,
) -> CycleDecision {
    let mut outcomes = Vec::new();
    let mut best: Option<(OptimizablePolicy, EvaluationSummary)> = None;
    for (index, diff) in diffs.iter().enumerate() {
        if u32::try_from(index).unwrap_or(u32::MAX) >= limits.max_candidates {
            outcomes.push(CandidateOutcome {
                candidate_digest: None,
                rejection: Some(OptimizerRejection::BudgetExhausted),
            });
            continue;
        }
        let evaluated = apply_candidate(base, diff).and_then(|candidate| {
            let summary = evaluate_dev(&candidate)?;
            compare_to_baseline(&baseline.dev, &summary)?;
            Ok((candidate, summary))
        });
        match evaluated {
            Ok((candidate, summary)) => {
                let digest = candidate.digest().ok();
                outcomes.push(CandidateOutcome {
                    candidate_digest: digest,
                    rejection: None,
                });
                let better = best
                    .as_ref()
                    .is_none_or(|(_, current)| compare_to_baseline(current, &summary).is_ok());
                if better {
                    best = Some((candidate, summary));
                }
            }
            Err(rejection) => outcomes.push(CandidateOutcome {
                candidate_digest: None,
                rejection: Some(rejection),
            }),
        }
    }
    let promoted = best.and_then(|(candidate, _)| {
        ledger.consult().ok()?;
        let holdout = evaluate_holdout(&candidate).ok()?;
        if holdout.hard_gate_failures > 0
            || holdout.verified_successes < baseline.holdout.verified_successes
        {
            return None;
        }
        candidate.digest().ok()
    });
    CycleDecision {
        optimization_improved: promoted.is_some(),
        outcomes,
        promoted,
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyState {
    Active,
    Superseded,
    RolledBack,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyVersion {
    pub version: u32,
    pub parent: Option<u32>,
    pub policy_digest: Sha256Digest,
    pub state: PolicyState,
}

/// Version history for future admissions. Exactly one version is active.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PolicyHistory {
    pub versions: Vec<PolicyVersion>,
}

impl PolicyHistory {
    #[must_use]
    pub fn active(&self) -> Option<&PolicyVersion> {
        self.versions
            .iter()
            .find(|version| version.state == PolicyState::Active)
    }

    /// Promotes a new version; the previous active one is superseded but
    /// kept as the rollback target.
    pub fn promote(&mut self, policy_digest: Sha256Digest) -> Result<u32, OptimizerRejection> {
        let parent = self.active().map(|version| version.version);
        let next = self
            .versions
            .iter()
            .map(|version| version.version)
            .max()
            .map_or(Some(1), |maximum| maximum.checked_add(1))
            .ok_or(OptimizerRejection::InvalidHistory)?;
        for version in &mut self.versions {
            if version.state == PolicyState::Active {
                version.state = PolicyState::Superseded;
            }
        }
        self.versions.push(PolicyVersion {
            version: next,
            parent,
            policy_digest,
            state: PolicyState::Active,
        });
        Ok(next)
    }

    /// Rolls the active version back to its parent.
    pub fn rollback(&mut self) -> Result<u32, OptimizerRejection> {
        let active = self
            .active()
            .cloned()
            .ok_or(OptimizerRejection::InvalidHistory)?;
        let parent = active.parent.ok_or(OptimizerRejection::InvalidHistory)?;
        for version in &mut self.versions {
            if version.version == active.version {
                version.state = PolicyState::RolledBack;
            } else if version.version == parent {
                version.state = PolicyState::Active;
            }
        }
        Ok(parent)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn id(value: &str) -> SpecIdentifier {
        SpecIdentifier::new(value).expect("id")
    }

    fn diff(changes: Vec<(&str, Value)>) -> CandidateDiff {
        CandidateDiff {
            changes: changes
                .into_iter()
                .map(|(field, value)| PolicyChange {
                    field: field.into(),
                    value,
                })
                .collect(),
            expected_benefit: "fewer repair turns".into(),
            possible_regressions: vec![],
        }
    }

    fn summary(successes: u32, failures: u32, requests: Option<u64>) -> EvaluationSummary {
        EvaluationSummary {
            cases: 4,
            hard_gate_failures: failures,
            verified_successes: successes,
            model_requests: requests,
            elapsed_ms: 100,
        }
    }

    #[test]
    fn only_allowlisted_fields_and_optimizable_blocks_can_change() {
        let base = OptimizablePolicy::baseline();
        let wording = diff(vec![(
            "template_overrides.worker_v1.worker_method",
            json!("Write the tests you intend to pass first, then implement."),
        )]);
        apply_candidate(&base, &wording).expect("allowed wording");
        for (field, value) in [
            ("requirements", json!([])),
            ("permitted_tools", json!(["write_files"])),
            ("budget_caps.max_turns", json!(99)),
            ("verifier_suites", json!([])),
            ("holdout.case_ids", json!([])),
            ("accepted_history", json!([])),
            ("providers.worker_a", json!("other")),
        ] {
            assert_eq!(
                apply_candidate(&base, &diff(vec![(field, value)])),
                Err(OptimizerRejection::ForbiddenField),
                "{field}"
            );
        }
        let immutable = diff(vec![(
            "template_overrides.worker_v1.worker_rules",
            json!("Edit anything you like."),
        )]);
        assert_eq!(
            apply_candidate(&base, &immutable),
            Err(OptimizerRejection::ForbiddenTemplateChange)
        );
        let out_of_range = diff(vec![("summary_budget_bytes", json!(1_000_000))]);
        assert_eq!(
            apply_candidate(&base, &out_of_range),
            Err(OptimizerRejection::InvalidValue)
        );
        let duplicate_sources = diff(vec![(
            "retrieval_order",
            json!(["worker_claims", "worker_claims"]),
        )]);
        assert_eq!(
            apply_candidate(&base, &duplicate_sources),
            Err(OptimizerRejection::InvalidValue)
        );
        assert_eq!(
            apply_candidate(&base, &diff(vec![])),
            Err(OptimizerRejection::EmptyDiff)
        );
    }

    #[test]
    fn evaluation_must_cover_exactly_the_fixed_dataset() {
        let policy = OptimizablePolicy::baseline();
        let dataset = EvaluationDataset {
            dataset_id: id("dev_v1"),
            split: DatasetSplit::Dev,
            case_ids: vec![id("case_a"), id("case_b")],
        };
        let result = |case: &str| CaseResult {
            case_id: id(case),
            hard_gates_passed: true,
            verified_success: true,
            model_requests: Some(3),
            elapsed_ms: 10,
        };
        let run = EvaluationRun {
            policy_digest: policy.digest().expect("digest"),
            dataset_digest: dataset.digest().expect("digest"),
            results: vec![result("case_a"), result("case_b")],
        };
        let summary = summarize(&run, &dataset, &policy).expect("summary");
        assert_eq!(summary.verified_successes, 2);
        assert_eq!(summary.model_requests, Some(6));
        let mut cherry_picked = run.clone();
        cherry_picked.results.pop();
        assert_eq!(
            summarize(&cherry_picked, &dataset, &policy),
            Err(OptimizerRejection::DatasetMismatch)
        );
        let mut unknown = run.clone();
        unknown.results[0].model_requests = None;
        assert_eq!(
            summarize(&unknown, &dataset, &policy)
                .expect("summary")
                .model_requests,
            None
        );
        let other = OptimizablePolicy {
            summary_budget_bytes: 4096,
            ..policy.clone()
        };
        assert_eq!(
            summarize(&run, &dataset, &other),
            Err(OptimizerRejection::PolicyMismatch)
        );
    }

    #[test]
    fn harmful_and_no_gain_candidates_are_rejected() {
        let baseline = summary(3, 0, Some(40));
        assert_eq!(
            compare_to_baseline(&baseline, &summary(4, 1, Some(10))),
            Err(OptimizerRejection::HardGateFailure)
        );
        assert_eq!(
            compare_to_baseline(&baseline, &summary(2, 0, Some(10))),
            Err(OptimizerRejection::NoImprovement)
        );
        assert_eq!(
            compare_to_baseline(&baseline, &summary(3, 0, None)),
            Err(OptimizerRejection::NoImprovement)
        );
        assert_eq!(
            compare_to_baseline(&baseline, &summary(3, 0, Some(40))),
            Err(OptimizerRejection::NoImprovement)
        );
        compare_to_baseline(&baseline, &summary(3, 0, Some(30))).expect("cheaper at equal success");
        compare_to_baseline(&baseline, &summary(4, 0, Some(60))).expect("more successes");
    }

    #[test]
    fn a_cycle_is_budget_bounded_consults_holdout_once_and_can_find_nothing() {
        let base = OptimizablePolicy::baseline();
        let candidates = vec![
            diff(vec![("summary_budget_bytes", json!(4096))]),
            diff(vec![("permitted_tools", json!(["write_files"]))]),
            diff(vec![("summary_budget_bytes", json!(2048))]),
            diff(vec![("summary_budget_bytes", json!(1024))]),
        ];
        let limits = OptimizerLimits {
            max_candidates: 3,
            max_model_requests: 30,
        };
        let mut ledger = HoldoutLedger::default();
        let mut holdout_calls = 0;
        let decision = run_cycle(
            &base,
            CycleBaseline {
                dev: summary(3, 0, Some(40)),
                holdout: summary(2, 0, Some(20)),
            },
            &candidates,
            limits,
            &mut ledger,
            |candidate| {
                Ok(if candidate.summary_budget_bytes == 2048 {
                    summary(4, 0, Some(30))
                } else {
                    summary(3, 0, Some(50))
                })
            },
            |_| {
                holdout_calls += 1;
                Ok(summary(2, 0, Some(20)))
            },
        );
        assert!(decision.optimization_improved);
        assert_eq!(holdout_calls, 1);
        assert_eq!(
            decision.outcomes[1].rejection,
            Some(OptimizerRejection::ForbiddenField)
        );
        assert_eq!(
            decision.outcomes[3].rejection,
            Some(OptimizerRejection::BudgetExhausted)
        );
        assert_eq!(
            ledger.consult(),
            Err(OptimizerRejection::HoldoutContaminated)
        );

        let mut fresh = HoldoutLedger::default();
        let nothing = run_cycle(
            &base,
            CycleBaseline {
                dev: summary(3, 0, Some(40)),
                holdout: summary(2, 0, Some(20)),
            },
            &candidates[..1],
            limits,
            &mut fresh,
            |_| Ok(summary(3, 0, Some(50))),
            |_| Ok(summary(2, 0, Some(20))),
        );
        assert!(!nothing.optimization_improved);
        assert_eq!(nothing.promoted, None);
        assert_eq!(
            fresh.consultations, 0,
            "no eligible candidate, no holdout access"
        );
    }

    #[test]
    fn a_holdout_regression_blocks_promotion() {
        let base = OptimizablePolicy::baseline();
        let mut ledger = HoldoutLedger::default();
        let decision = run_cycle(
            &base,
            CycleBaseline {
                dev: summary(3, 0, Some(40)),
                holdout: summary(3, 0, Some(20)),
            },
            &[diff(vec![("summary_budget_bytes", json!(2048))])],
            OptimizerLimits {
                max_candidates: 3,
                max_model_requests: 30,
            },
            &mut ledger,
            |_| Ok(summary(4, 0, Some(30))),
            |_| Ok(summary(2, 0, Some(20))),
        );
        assert!(!decision.optimization_improved);
    }

    #[test]
    fn promotion_is_versioned_and_rollbackable() {
        let mut history = PolicyHistory::default();
        assert_eq!(history.promote(Sha256Digest::of(b"baseline")), Ok(1));
        assert_eq!(history.rollback(), Err(OptimizerRejection::InvalidHistory));
        assert_eq!(history.promote(Sha256Digest::of(b"candidate")), Ok(2));
        assert_eq!(history.active().map(|version| version.version), Some(2));
        assert_eq!(history.rollback(), Ok(1));
        assert_eq!(
            history.active().map(|version| version.policy_digest),
            Some(Sha256Digest::of(b"baseline"))
        );
        assert_eq!(history.versions[1].state, PolicyState::RolledBack);
        assert_eq!(history.promote(Sha256Digest::of(b"third")), Ok(3));
        assert_eq!(history.active().and_then(|version| version.parent), Some(1));
    }
}
