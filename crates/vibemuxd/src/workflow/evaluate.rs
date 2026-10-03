//! Prompt-policy evaluation (ADR 031 §7): one bounded optimizer cycle over
//! a fixed operator suite, where every case is a real workflow the daemon
//! prepares, runs, reviews, verifies, and integrates.
//!
//! Inputs are operator files beside the workflow config:
//! `<state dir>/prompt_suites/<suite_id>.json` holds the fixed train, dev,
//! and holdout case lists (each case a workflow request and an operator
//! policy) and the cycle limits; `<state dir>/prompt_candidates/<id>.json`
//! holds candidate diffs against the active policy. A diff can only name
//! optimizer-allowlisted fields; a case is validated like any
//! `workflow prepare`.
//!
//! A case result comes from the case workflow's recorded evidence only:
//! verified success is the `accepted` phase; a hard gate failed when a
//! candidate was refused for scope, protected paths, file types, or a
//! moved base, or the run failed on one; model requests are the recorded
//! dispatch attempts. The request key of a case workflow is derived from
//! the suite bytes, the base commit, the case, and the evaluated policy, so
//! a repeated evaluation reuses finished case workflows instead of paying
//! for them again.
//!
//! The cycle is recorded before it starts and again before its holdout is
//! consulted; a cycle whose holdout was consulted never runs again.
//! Promotion goes through the single writer, needs the base policy to be
//! still active, and affects future admissions only.

use std::{cell::RefCell, collections::BTreeMap, collections::BTreeSet, sync::Arc, time::Duration};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_store::WorkflowSnapshot;
use vibemux_workflow::{
    Sha256Digest, SpecIdentifier,
    gates::WorkflowPhase,
    optimizer::{
        CandidateDiff, CandidateOutcome, CaseResult, CycleBaseline, DatasetSplit,
        EvaluationDataset, EvaluationRun, EvaluationSummary, HoldoutLedger, OptimizablePolicy,
        OptimizerLimits, OptimizerRejection, apply_candidate, run_cycle, summarize,
    },
    receipts::WorkflowReceipt,
    workflow_record::WorkflowRecord,
};

use super::{
    error::WorkflowError,
    service::{WorkflowService, active_policy},
    state_files::StateFiles,
    turns::derived_id,
};
use crate::harness_dispatch::{call_writer, git_head::read_head_commit};

pub const PROMPT_SUITES_DIR_NAME: &str = "prompt_suites";
pub const PROMPT_CANDIDATES_DIR_NAME: &str = "prompt_candidates";
pub const PROMPT_SUITE_SCHEMA_VERSION: u32 = 1;
pub const PROMPT_CANDIDATES_SCHEMA_VERSION: u32 = 1;
const EVALUATION_REPORT_SCHEMA_VERSION: u32 = 1;
const CYCLE_RECORD_SCHEMA_VERSION: u32 = 1;
const MAX_SUITE_BYTES: u64 = 256 * 1024;
const MAX_CANDIDATES_BYTES: u64 = 64 * 1024;
/// Bounds that keep one cycle's report inside a Control frame.
pub const MAX_SUITE_CASES: usize = 16;
pub const MAX_CANDIDATE_DIFFS: usize = 4;
const DEFAULT_CASE_DEADLINE_MS: u64 = 600_000;
const MAX_CASE_DEADLINE_MS: u64 = 3_600_000;
/// How long a cancelled case may take to stop.
const CANCEL_GRACE: Duration = Duration::from_secs(30);
const CASE_KEY_DOMAIN: &str = "vibemux.workflow.evaluation_case.v1";
const CASE_START_DOMAIN: &str = "vibemux.workflow.evaluation_start.v1";
const CYCLE_ID_DOMAIN: &str = "vibemux.workflow.evaluation_cycle.v1";
const CASE_KEY_HEX_CHARS: usize = 40;
/// A candidate refused for one of these failed a hard gate: the policy led
/// a worker outside its scope or around a protection.
const HARD_GATE_CODES: [&str; 9] = [
    "snapshot_invalid_path",
    "snapshot_outside_owned_paths",
    "snapshot_protected_path",
    "snapshot_forbidden_path",
    "snapshot_symlink",
    "snapshot_special_file",
    "snapshot_mode_change",
    "snapshot_head_moved",
    "workflow_verifier_changed",
];
/// Why promotion was withheld although the cycle selected a candidate.
pub const PROMOTION_WITHHELD_CODE: &str = "optimizer_policy_mismatch";

/// A fixed evaluation suite.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptSuite {
    pub schema_version: u32,
    pub suite_id: SpecIdentifier,
    /// Kept disjoint from dev and holdout; candidates are authored against
    /// it, never selected on it.
    pub train: EvaluationDataset,
    pub dev: EvaluationDataset,
    pub holdout: EvaluationDataset,
    pub cases: BTreeMap<SpecIdentifier, SuiteCase>,
    pub limits: OptimizerLimits,
    #[serde(default)]
    pub case_deadline_ms: Option<u64>,
}

/// One case: the workflow request (its `request_key` is replaced per
/// evaluated policy) and the operator policy it is prepared with.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuiteCase {
    pub request: Value,
    pub policy: Value,
}

impl PromptSuite {
    pub fn parse(bytes: &[u8], suite_id: &SpecIdentifier) -> Result<Self, WorkflowError> {
        let suite: Self =
            serde_json::from_slice(bytes).map_err(|_| WorkflowError::EvaluationInvalid)?;
        suite.validate(suite_id)?;
        Ok(suite)
    }

    fn validate(&self, suite_id: &SpecIdentifier) -> Result<(), WorkflowError> {
        let invalid = Err(WorkflowError::EvaluationInvalid);
        if self.schema_version != PROMPT_SUITE_SCHEMA_VERSION || &self.suite_id != suite_id {
            return invalid;
        }
        let mut listed = BTreeSet::new();
        for (dataset, split) in [
            (&self.train, DatasetSplit::Train),
            (&self.dev, DatasetSplit::Dev),
            (&self.holdout, DatasetSplit::Holdout),
        ] {
            if dataset.split != split {
                return invalid;
            }
            for case_id in &dataset.case_ids {
                // Disjoint splits, every listed case defined.
                if !listed.insert(case_id) || !self.cases.contains_key(case_id) {
                    return invalid;
                }
            }
        }
        let limits = self.limits;
        let candidates_bounded = usize::try_from(limits.max_candidates)
            .is_ok_and(|count| (1..=MAX_CANDIDATE_DIFFS).contains(&count));
        if self.dev.case_ids.is_empty()
            || self.holdout.case_ids.is_empty()
            || listed.len() != self.cases.len()
            || self.cases.len() > MAX_SUITE_CASES
            || self.cases.values().any(|case| !case.request.is_object())
            || !candidates_bounded
            || limits.max_model_requests == 0
            || !(1..=MAX_CASE_DEADLINE_MS).contains(&self.case_deadline_ms())
        {
            return invalid;
        }
        Ok(())
    }

    fn case_deadline_ms(&self) -> u64 {
        self.case_deadline_ms.unwrap_or(DEFAULT_CASE_DEADLINE_MS)
    }
}

/// Candidate diffs against the policy active when the cycle starts.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptCandidates {
    pub schema_version: u32,
    pub candidate_id: SpecIdentifier,
    pub diffs: Vec<CandidateDiff>,
}

impl PromptCandidates {
    pub fn parse(bytes: &[u8], candidate_id: &SpecIdentifier) -> Result<Self, WorkflowError> {
        let candidates: Self =
            serde_json::from_slice(bytes).map_err(|_| WorkflowError::EvaluationInvalid)?;
        if candidates.schema_version != PROMPT_CANDIDATES_SCHEMA_VERSION
            || &candidates.candidate_id != candidate_id
            || !(1..=MAX_CANDIDATE_DIFFS).contains(&candidates.diffs.len())
        {
            return Err(WorkflowError::EvaluationInvalid);
        }
        Ok(candidates)
    }
}

/// The content-free result of one cycle.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationReport {
    pub schema_version: u32,
    pub cycle_id: Sha256Digest,
    pub suite_id: SpecIdentifier,
    pub candidate_id: SpecIdentifier,
    pub base_commit: String,
    pub base_policy_digest: Sha256Digest,
    pub baseline_dev: EvaluationSummary,
    pub baseline_holdout: EvaluationSummary,
    /// One per candidate diff, in file order.
    pub outcomes: Vec<CandidateOutcome>,
    pub promoted_policy_digest: Option<Sha256Digest>,
    pub promoted_version: Option<u32>,
    /// Set when the cycle selected a candidate but did not promote it.
    pub promotion_withheld: Option<String>,
    pub optimization_improved: bool,
    pub holdout_consultations: u32,
    /// Model requests this cycle paid for; reused case workflows cost none.
    pub model_requests_spent: u64,
    pub max_model_requests: u32,
    pub cases: Vec<CaseEvidence>,
}

/// One case workflow behind a summary.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CaseEvidence {
    pub split: DatasetSplit,
    pub case_id: SpecIdentifier,
    pub policy_digest: Sha256Digest,
    pub workflow_id: Uuid,
    pub phase: WorkflowPhase,
    pub hard_gates_passed: bool,
    pub verified_success: bool,
    pub model_requests: Option<u32>,
    /// A previous cycle already ran this workflow.
    pub reused: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct EvaluateResponse {
    /// This cycle already finished; the stored report is returned.
    pub duplicate: bool,
    #[serde(flatten)]
    pub report: EvaluationReport,
}

/// The persisted state of one cycle.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CycleRecord {
    schema_version: u32,
    cycle_id: Sha256Digest,
    holdout_consulted: bool,
    report: Option<EvaluationReport>,
}

impl CycleRecord {
    fn new(cycle_id: Sha256Digest, holdout_consulted: bool) -> Self {
        Self {
            schema_version: CYCLE_RECORD_SCHEMA_VERSION,
            cycle_id,
            holdout_consulted,
            report: None,
        }
    }
}

/// What one cycle reads once and shares with its blocking half.
struct CycleInputs {
    suite: PromptSuite,
    suite_sha256: Sha256Digest,
    base_commit: String,
    files: StateFiles,
    cycle_id: Sha256Digest,
}

/// Budget accounting and case evidence of one cycle.
struct Spending {
    limit: u64,
    spent: u64,
    cases: Vec<CaseEvidence>,
    /// The first infrastructure failure; it aborts the cycle.
    failure: Option<WorkflowError>,
}

enum CaseError {
    Rejected(OptimizerRejection),
    Failed(WorkflowError),
}

impl From<WorkflowError> for CaseError {
    fn from(error: WorkflowError) -> Self {
        Self::Failed(error)
    }
}

/// The request key of one case under one policy.
#[must_use]
pub fn case_request_key(
    suite_sha256: Sha256Digest,
    base_commit: &str,
    case_id: &SpecIdentifier,
    policy_digest: Sha256Digest,
) -> String {
    let digest = Sha256Digest::of_fields(
        CASE_KEY_DOMAIN,
        &[
            suite_sha256.as_bytes(),
            base_commit.as_bytes(),
            case_id.as_str().as_bytes(),
            policy_digest.as_bytes(),
        ],
    );
    format!("eval_{}", &digest.to_hex()[..CASE_KEY_HEX_CHARS])
}

/// The result of a finished (or stopped) case workflow, from its record.
#[must_use]
pub fn case_result(case_id: &SpecIdentifier, snapshot: &WorkflowSnapshot) -> CaseResult {
    let hard_gate = |code: &str| HARD_GATE_CODES.contains(&code);
    let refused = snapshot.receipts.iter().any(|receipt| {
        matches!(receipt, WorkflowReceipt::Candidate(candidate)
            if candidate.violation_codes.iter().any(|code| hard_gate(code)))
    });
    let record = &snapshot.record;
    let failed = record
        .tasks
        .iter()
        .filter_map(|task| task.failure_code.as_deref())
        .chain(record.blocked_reason.as_deref())
        .any(hard_gate);
    CaseResult {
        case_id: case_id.clone(),
        hard_gates_passed: !refused && !failed,
        verified_success: record.phase == WorkflowPhase::Accepted,
        model_requests: u32::try_from(snapshot.attempts.len()).ok(),
        elapsed_ms: record.updated_at_ms.saturating_sub(record.created_at_ms),
    }
}

/// The most model requests a case can make: worker turns are charged per
/// task up to its bound, and each candidate gets at most one review.
#[must_use]
pub fn worst_case_requests(record: &WorkflowRecord) -> u64 {
    record
        .tasks
        .iter()
        .map(|task| u64::from(task.max_turns).saturating_mul(2))
        .sum()
}

fn finished(phase: WorkflowPhase) -> bool {
    matches!(
        phase,
        WorkflowPhase::Accepted | WorkflowPhase::Failed | WorkflowPhase::Cancelled
    )
}

impl WorkflowService {
    /// Runs one bounded optimizer cycle of `candidate_id` on `suite_id`
    /// and promotes the selected candidate, if any.
    pub async fn evaluate_prompt_policy(
        &self,
        candidate_id: SpecIdentifier,
        suite_id: SpecIdentifier,
    ) -> Result<EvaluateResponse, WorkflowError> {
        self.accepting()?;
        let setup = self.setup_handle()?;
        let _running = self
            .evaluation_lock()
            .try_lock()
            .map_err(|_| WorkflowError::AlreadyRunning)?;
        let files = setup.state.clone();
        let root = setup.canonical_root.clone();
        let (suite, suite_sha256, candidates, candidates_sha256, base_commit) =
            tokio::task::spawn_blocking(move || {
                let suite_bytes = files
                    .read_operator_input(PROMPT_SUITES_DIR_NAME, &suite_id, MAX_SUITE_BYTES)
                    .map_err(|_| WorkflowError::EvaluationInvalid)?
                    .ok_or(WorkflowError::EvaluationInvalid)?;
                let candidate_bytes = files
                    .read_operator_input(
                        PROMPT_CANDIDATES_DIR_NAME,
                        &candidate_id,
                        MAX_CANDIDATES_BYTES,
                    )
                    .map_err(|_| WorkflowError::EvaluationInvalid)?
                    .ok_or(WorkflowError::EvaluationInvalid)?;
                let suite = PromptSuite::parse(&suite_bytes, &suite_id)?;
                let candidates = PromptCandidates::parse(&candidate_bytes, &candidate_id)?;
                let head = read_head_commit(&root).map_err(|_| WorkflowError::Git)?;
                Ok::<_, WorkflowError>((
                    suite,
                    Sha256Digest::of(&suite_bytes),
                    candidates,
                    Sha256Digest::of(&candidate_bytes),
                    head,
                ))
            })
            .await
            .map_err(|_| WorkflowError::Internal)??;
        let versions = self.policy_versions().await?;
        let base = active_policy(&versions)
            .map_or_else(OptimizablePolicy::baseline, |record| record.policy.clone());
        let base_digest = base.digest().map_err(|_| WorkflowError::Internal)?;
        let cycle_id = Sha256Digest::of_fields(
            CYCLE_ID_DOMAIN,
            &[
                suite_sha256.as_bytes(),
                candidates_sha256.as_bytes(),
                base_commit.as_bytes(),
                base_digest.as_bytes(),
            ],
        );
        let files = setup.state.clone();
        let existing = blocking(move || files.read_evaluation::<CycleRecord>(cycle_id)).await?;
        match existing {
            Some(CycleRecord {
                report: Some(report),
                ..
            }) => {
                return Ok(EvaluateResponse {
                    duplicate: true,
                    report,
                });
            }
            Some(record) if record.holdout_consulted => {
                return Err(WorkflowError::Optimizer {
                    code: OptimizerRejection::HoldoutContaminated.code(),
                });
            }
            _ => {}
        }
        let files = setup.state.clone();
        blocking(move || files.write_evaluation(cycle_id, &CycleRecord::new(cycle_id, false)))
            .await?;
        let inputs = Arc::new(CycleInputs {
            suite,
            suite_sha256,
            base_commit,
            files: setup.state.clone(),
            cycle_id,
        });
        let limits = inputs.suite.limits;
        let mut spending = Spending {
            limit: u64::from(limits.max_model_requests),
            spent: 0,
            cases: Vec::new(),
            failure: None,
        };
        let baseline = CycleBaseline {
            dev: self
                .baseline_summary(&inputs, &inputs.suite.dev, &base, &mut spending)
                .await?,
            holdout: self
                .baseline_summary(&inputs, &inputs.suite.holdout, &base, &mut spending)
                .await?,
        };
        let service = self.clone();
        let cycle_inputs = Arc::clone(&inputs);
        let cycle_base = base.clone();
        let diffs = candidates.diffs.clone();
        let runtime = tokio::runtime::Handle::current();
        let (decision, holdout, spending) = tokio::task::spawn_blocking(move || {
            let inputs = cycle_inputs;
            let spending = RefCell::new(spending);
            let mut holdout = HoldoutLedger::default();
            let evaluate = |dataset: &EvaluationDataset, policy: &OptimizablePolicy| {
                let mut spending = spending.borrow_mut();
                if spending.failure.is_some() {
                    return Err(OptimizerRejection::InvalidValue);
                }
                let evaluated = runtime.block_on(service.evaluate_dataset(
                    &inputs,
                    dataset,
                    policy,
                    &mut spending,
                ));
                match evaluated {
                    Ok(summary) => Ok(summary),
                    Err(CaseError::Rejected(rejection)) => Err(rejection),
                    Err(CaseError::Failed(error)) => {
                        spending.failure = Some(error);
                        Err(OptimizerRejection::InvalidValue)
                    }
                }
            };
            let decision = run_cycle(
                &cycle_base,
                baseline,
                &diffs,
                limits,
                &mut holdout,
                |policy| evaluate(&inputs.suite.dev, policy),
                |policy| {
                    // Recorded before the first holdout case runs: a crash
                    // after this point never lets the cycle run again.
                    let consulted = CycleRecord::new(inputs.cycle_id, true);
                    if let Err(error) = inputs.files.write_evaluation(inputs.cycle_id, &consulted) {
                        spending.borrow_mut().failure = Some(error);
                        return Err(OptimizerRejection::InvalidValue);
                    }
                    evaluate(&inputs.suite.holdout, policy)
                },
            );
            (decision, holdout, spending.into_inner())
        })
        .await
        .map_err(|_| WorkflowError::Internal)?;
        if let Some(error) = spending.failure {
            return Err(error);
        }
        let (promoted_version, promotion_withheld) = match decision.promoted {
            Some(digest) => {
                self.promote(&base, base_digest, &candidates.diffs, digest)
                    .await?
            }
            None => (None, None),
        };
        let report = EvaluationReport {
            schema_version: EVALUATION_REPORT_SCHEMA_VERSION,
            cycle_id,
            suite_id: inputs.suite.suite_id.clone(),
            candidate_id: candidates.candidate_id.clone(),
            base_commit: inputs.base_commit.clone(),
            base_policy_digest: base_digest,
            baseline_dev: baseline.dev,
            baseline_holdout: baseline.holdout,
            outcomes: decision.outcomes,
            promoted_policy_digest: promoted_version.and(decision.promoted),
            promoted_version,
            promotion_withheld,
            optimization_improved: promoted_version.is_some(),
            holdout_consultations: holdout.consultations,
            model_requests_spent: spending.spent,
            max_model_requests: limits.max_model_requests,
            cases: spending.cases,
        };
        let files = setup.state.clone();
        let finished_record = CycleRecord {
            report: Some(report.clone()),
            ..CycleRecord::new(cycle_id, holdout.consultations > 0)
        };
        blocking(move || files.write_evaluation(cycle_id, &finished_record)).await?;
        Ok(EvaluateResponse {
            duplicate: false,
            report,
        })
    }

    /// The baseline's summary on one dataset; any refusal ends the cycle.
    async fn baseline_summary(
        &self,
        inputs: &CycleInputs,
        dataset: &EvaluationDataset,
        base: &OptimizablePolicy,
        spending: &mut Spending,
    ) -> Result<EvaluationSummary, WorkflowError> {
        match self.evaluate_dataset(inputs, dataset, base, spending).await {
            Ok(summary) => Ok(summary),
            Err(CaseError::Rejected(rejection)) => Err(WorkflowError::Optimizer {
                code: rejection.code(),
            }),
            Err(CaseError::Failed(error)) => Err(error),
        }
    }

    /// Runs (or reuses) every case of `dataset` under `policy`.
    async fn evaluate_dataset(
        &self,
        inputs: &CycleInputs,
        dataset: &EvaluationDataset,
        policy: &OptimizablePolicy,
        spending: &mut Spending,
    ) -> Result<EvaluationSummary, CaseError> {
        let policy_digest = policy.digest().map_err(|_| WorkflowError::Internal)?;
        let mut results = Vec::with_capacity(dataset.case_ids.len());
        for case_id in &dataset.case_ids {
            let case = inputs
                .suite
                .cases
                .get(case_id)
                .ok_or(WorkflowError::EvaluationInvalid)?;
            let request_key = case_request_key(
                inputs.suite_sha256,
                &inputs.base_commit,
                case_id,
                policy_digest,
            );
            let (snapshot, reused) = self
                .run_case(
                    case,
                    request_key,
                    policy,
                    inputs.suite.case_deadline_ms(),
                    spending,
                )
                .await?;
            let result = case_result(case_id, &snapshot);
            spending.cases.push(CaseEvidence {
                split: dataset.split,
                case_id: case_id.clone(),
                policy_digest,
                workflow_id: snapshot.record.workflow_id,
                phase: snapshot.record.phase,
                hard_gates_passed: result.hard_gates_passed,
                verified_success: result.verified_success,
                model_requests: result.model_requests,
                reused,
            });
            results.push(result);
        }
        let run = EvaluationRun {
            policy_digest,
            dataset_digest: dataset.digest().map_err(|_| WorkflowError::Internal)?,
            results,
        };
        summarize(&run, dataset, policy).map_err(CaseError::Rejected)
    }

    /// Prepares the case workflow under `policy`'s wording and runs it to a
    /// stop, unless a previous cycle already finished it.
    async fn run_case(
        &self,
        case: &SuiteCase,
        request_key: String,
        policy: &OptimizablePolicy,
        deadline_ms: u64,
        spending: &mut Spending,
    ) -> Result<(WorkflowSnapshot, bool), CaseError> {
        let mut request = case.request.clone();
        let object = request
            .as_object_mut()
            .ok_or(WorkflowError::EvaluationInvalid)?;
        object.insert("request_key".to_string(), Value::String(request_key));
        let prepared = self
            .prepare_with(
                request,
                case.policy.clone(),
                policy.template_overrides.clone(),
            )
            .await?;
        let workflow_id = prepared.workflow_id;
        let snapshot = self.snapshot(workflow_id).await?;
        if finished(snapshot.record.phase) {
            return Ok((snapshot, true));
        }
        let worst_case = worst_case_requests(&snapshot.record);
        if spending.spent.saturating_add(worst_case) > spending.limit {
            return Err(CaseError::Rejected(OptimizerRejection::BudgetExhausted));
        }
        let paid_before = snapshot.attempts.len();
        let start_id = derived_id(CASE_START_DOMAIN, &[workflow_id.as_bytes()]);
        self.start_workflow(prepared.start_contract, start_id)
            .await?;
        if !self
            .wait_idle(workflow_id, Duration::from_millis(deadline_ms))
            .await
        {
            match self.cancel(workflow_id).await {
                // It stopped on its own after the deadline passed.
                Ok(_) | Err(WorkflowError::InvalidPhase) => {}
                Err(error) => return Err(error.into()),
            }
            self.wait_idle(workflow_id, CANCEL_GRACE).await;
        }
        let snapshot = self.snapshot(workflow_id).await?;
        let paid = snapshot.attempts.len().saturating_sub(paid_before);
        spending.spent = spending
            .spent
            .saturating_add(u64::try_from(paid).unwrap_or(u64::MAX));
        Ok((snapshot, false))
    }

    /// Promotes the selected candidate if the base policy is still active.
    /// An empty history first records the baseline, so the promotion can
    /// be rolled back.
    async fn promote(
        &self,
        base: &OptimizablePolicy,
        base_digest: Sha256Digest,
        diffs: &[CandidateDiff],
        selected: Sha256Digest,
    ) -> Result<(Option<u32>, Option<String>), WorkflowError> {
        let policy = diffs
            .iter()
            .filter_map(|diff| apply_candidate(base, diff).ok())
            .find(|policy| policy.digest().is_ok_and(|digest| digest == selected))
            .ok_or(WorkflowError::Internal)?;
        let versions = self.policy_versions().await?;
        if active_policy(&versions)
            .is_some_and(|record| record.version.policy_digest != base_digest)
        {
            return Ok((None, Some(PROMOTION_WITHHELD_CODE.to_string())));
        }
        if versions.is_empty() {
            let writer = self.writer().clone();
            let baseline = base.clone();
            call_writer(move || {
                writer.promote_prompt_policy(baseline.clone(), OffsetDateTime::now_utc())
            })
            .await?;
        }
        let writer = self.writer().clone();
        let record = call_writer(move || {
            writer.promote_prompt_policy(policy.clone(), OffsetDateTime::now_utc())
        })
        .await?;
        Ok((Some(record.version.version), None))
    }
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, WorkflowError> + Send + 'static,
) -> Result<T, WorkflowError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| WorkflowError::Internal)?
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn id(value: &str) -> SpecIdentifier {
        SpecIdentifier::new(value).expect("id")
    }

    fn dataset(split: &str, cases: &[&str]) -> Value {
        json!({"dataset_id": format!("{split}_set"), "split": split, "case_ids": cases})
    }

    fn suite(train: &[&str], dev: &[&str], holdout: &[&str], cases: &[&str]) -> Value {
        let cases: serde_json::Map<String, Value> = cases
            .iter()
            .map(|case| {
                (
                    (*case).to_string(),
                    json!({"request": {"request_key": "ignored"}, "policy": {}}),
                )
            })
            .collect();
        json!({
            "schema_version": 1,
            "suite_id": "taskboard_suite",
            "train": dataset("train", train),
            "dev": dataset("dev", dev),
            "holdout": dataset("holdout", holdout),
            "cases": cases,
            "limits": {"max_candidates": 2, "max_model_requests": 40},
        })
    }

    fn parse(value: &Value) -> Result<PromptSuite, WorkflowError> {
        PromptSuite::parse(
            &serde_json::to_vec(value).expect("JSON"),
            &id("taskboard_suite"),
        )
    }

    #[test]
    fn suites_need_disjoint_defined_splits_and_bounded_limits() {
        let valid = suite(
            &["case_t"],
            &["case_d"],
            &["case_h"],
            &["case_t", "case_d", "case_h"],
        );
        assert!(parse(&valid).is_ok());
        // A holdout case reused on dev.
        let shared = suite(&[], &["case_d"], &["case_d"], &["case_d"]);
        assert_eq!(parse(&shared).err(), Some(WorkflowError::EvaluationInvalid));
        // An undefined case, an unlisted case, an empty holdout.
        for broken in [
            suite(&[], &["case_d"], &["case_x"], &["case_d"]),
            suite(
                &[],
                &["case_d"],
                &["case_h"],
                &["case_d", "case_h", "case_z"],
            ),
            suite(&[], &["case_d"], &[], &["case_d"]),
        ] {
            assert_eq!(parse(&broken).err(), Some(WorkflowError::EvaluationInvalid));
        }
        let mut wrong_split = valid.clone();
        wrong_split["dev"]["split"] = json!("holdout");
        assert!(parse(&wrong_split).is_err());
        let mut unbounded = valid.clone();
        unbounded["limits"]["max_candidates"] = json!(MAX_CANDIDATE_DIFFS + 1);
        assert!(parse(&unbounded).is_err());
        let mut free = valid.clone();
        free["limits"]["max_model_requests"] = json!(0);
        assert!(parse(&free).is_err());
        let mut other_id = valid;
        other_id["suite_id"] = json!("other_suite");
        assert!(parse(&other_id).is_err());
    }

    #[test]
    fn candidate_files_are_bounded_and_named() {
        let diff = json!({"changes": [], "expected_benefit": "x", "possible_regressions": []});
        let file = |id: &str, diffs: Vec<Value>| {
            serde_json::to_vec(&json!({"schema_version": 1, "candidate_id": id, "diffs": diffs}))
                .expect("JSON")
        };
        assert!(
            PromptCandidates::parse(&file("wording_a", vec![diff.clone()]), &id("wording_a"))
                .is_ok()
        );
        assert!(
            PromptCandidates::parse(&file("wording_b", vec![diff.clone()]), &id("wording_a"))
                .is_err()
        );
        assert!(PromptCandidates::parse(&file("wording_a", vec![]), &id("wording_a")).is_err());
        let many = vec![diff; MAX_CANDIDATE_DIFFS + 1];
        assert!(PromptCandidates::parse(&file("wording_a", many), &id("wording_a")).is_err());
    }

    #[test]
    fn case_keys_bind_suite_base_case_and_policy() {
        let suite_sha = Sha256Digest::of(b"suite");
        let policy = Sha256Digest::of(b"policy");
        let key = case_request_key(suite_sha, "abc", &id("case_d"), policy);
        assert!(key.starts_with("eval_"));
        assert_eq!(key.len(), 5 + CASE_KEY_HEX_CHARS);
        assert_eq!(
            key,
            case_request_key(suite_sha, "abc", &id("case_d"), policy)
        );
        assert_ne!(
            key,
            case_request_key(suite_sha, "abd", &id("case_d"), policy)
        );
        assert_ne!(
            key,
            case_request_key(suite_sha, "abc", &id("case_h"), policy)
        );
        assert_ne!(
            key,
            case_request_key(suite_sha, "abc", &id("case_d"), Sha256Digest::of(b"other"))
        );
        assert_ne!(
            key,
            case_request_key(Sha256Digest::of(b"edited"), "abc", &id("case_d"), policy)
        );
    }
}
