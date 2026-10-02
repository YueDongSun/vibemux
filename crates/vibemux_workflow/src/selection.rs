//! Same-task comparison selection (ADR 031 §6).
//!
//! Hard gates first: a candidate whose gate failed, whose contract differs
//! from the comparison contract, or which is a labeled mutation artifact is
//! excluded. Among valid candidates the predeclared metrics decide in
//! order; a metric that is unknown for either side cannot decide. The
//! stable tie-breaker is the lowest candidate digest. With no valid
//! candidate there is no winner; a persuasive report cannot substitute.

use serde::{Deserialize, Serialize};

use crate::{
    Sha256Digest,
    gates::{CandidateAcceptance, CandidateOrigin, CandidateRecord, GateFailure},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct CandidateMetrics {
    pub tests_passed: u32,
    /// `None` when usage is unknown; unknown is never treated as zero.
    pub model_requests: Option<u32>,
    pub elapsed_ms: Option<u64>,
    pub changed_bytes: u64,
}

#[derive(Clone, Debug)]
pub struct ComparisonEntry {
    pub candidate: CandidateRecord,
    pub gate: Result<CandidateAcceptance, Vec<GateFailure>>,
    pub metrics: CandidateMetrics,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricKey {
    /// More passed trusted tests is better.
    TestsPassed,
    /// Fewer model requests is better (known values only).
    ModelRequests,
    /// Less elapsed time is better (known values only).
    ElapsedMs,
    /// A smaller change is better.
    ChangedBytes,
}

/// Predeclared before workers are admitted; part of the comparison contract.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionPolicy {
    pub metric_order: Vec<MetricKey>,
}

impl SelectionPolicy {
    #[must_use]
    pub fn baseline() -> Self {
        Self {
            metric_order: vec![
                MetricKey::TestsPassed,
                MetricKey::ModelRequests,
                MetricKey::ElapsedMs,
                MetricKey::ChangedBytes,
            ],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "by", content = "metric")]
pub enum DecidedBy {
    OnlyValidCandidate,
    Metric(MetricKey),
    TieBreaker,
    NoValidCandidate,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ExcludedCandidate {
    pub candidate_digest: Sha256Digest,
    pub reasons: Vec<&'static str>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SelectionOutcome {
    pub winner: Option<Sha256Digest>,
    pub decided_by: DecidedBy,
    /// Valid candidates, best first.
    pub ranking: Vec<Sha256Digest>,
    pub excluded: Vec<ExcludedCandidate>,
}

#[must_use]
pub fn select(
    entries: &[ComparisonEntry],
    policy: &SelectionPolicy,
    contract_id: Sha256Digest,
) -> SelectionOutcome {
    let mut valid: Vec<&ComparisonEntry> = Vec::new();
    let mut excluded = Vec::new();
    for entry in entries {
        let mut reasons: Vec<&'static str> = Vec::new();
        if let Err(failures) = &entry.gate {
            reasons.extend(failures.iter().map(GateFailure::code));
        }
        if entry.candidate.contract_id != contract_id {
            reasons.push("selection_different_contract");
        }
        if matches!(
            entry.candidate.origin,
            CandidateOrigin::InjectedMutation { .. }
        ) {
            reasons.push("selection_mutation_artifact");
        }
        if reasons.is_empty() {
            valid.push(entry);
        } else {
            reasons.sort_unstable();
            reasons.dedup();
            excluded.push(ExcludedCandidate {
                candidate_digest: entry.candidate.candidate_digest,
                reasons,
            });
        }
    }
    excluded.sort_by(|left, right| left.candidate_digest.cmp(&right.candidate_digest));
    if valid.is_empty() {
        return SelectionOutcome {
            winner: None,
            decided_by: DecidedBy::NoValidCandidate,
            ranking: Vec::new(),
            excluded,
        };
    }
    let mut decided_by = if valid.len() == 1 {
        DecidedBy::OnlyValidCandidate
    } else {
        DecidedBy::TieBreaker
    };
    valid.sort_by(|left, right| {
        for metric in &policy.metric_order {
            let ordering = compare_metric(*metric, &left.metrics, &right.metrics);
            if ordering != std::cmp::Ordering::Equal {
                return ordering;
            }
        }
        left.candidate
            .candidate_digest
            .cmp(&right.candidate.candidate_digest)
    });
    if valid.len() > 1 {
        let (first, second) = (&valid[0].metrics, &valid[1].metrics);
        if let Some(metric) = policy
            .metric_order
            .iter()
            .find(|metric| compare_metric(**metric, first, second) != std::cmp::Ordering::Equal)
        {
            decided_by = DecidedBy::Metric(*metric);
        }
    }
    SelectionOutcome {
        winner: Some(valid[0].candidate.candidate_digest),
        decided_by,
        ranking: valid
            .iter()
            .map(|entry| entry.candidate.candidate_digest)
            .collect(),
        excluded,
    }
}

/// `Less` means `left` is better.
fn compare_metric(
    metric: MetricKey,
    left: &CandidateMetrics,
    right: &CandidateMetrics,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let known = |left: Option<u64>, right: Option<u64>| match (left, right) {
        (Some(left), Some(right)) => left.cmp(&right),
        _ => Ordering::Equal,
    };
    match metric {
        MetricKey::TestsPassed => right.tests_passed.cmp(&left.tests_passed),
        MetricKey::ModelRequests => known(
            left.model_requests.map(u64::from),
            right.model_requests.map(u64::from),
        ),
        MetricKey::ElapsedMs => known(left.elapsed_ms, right.elapsed_ms),
        MetricKey::ChangedBytes => left.changed_bytes.cmp(&right.changed_bytes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gates::{AcceptanceLevel, fixtures::candidate};

    fn entry(label: &[u8], passed: bool, metrics: CandidateMetrics) -> ComparisonEntry {
        let mut record = candidate();
        record.candidate_digest = Sha256Digest::of(label);
        let gate = if passed {
            Ok(CandidateAcceptance {
                candidate_digest: record.candidate_digest,
                contract_id: record.contract_id,
                review_receipt: Sha256Digest::of(b"review"),
                verifier_receipt: Sha256Digest::of(b"verify"),
                level: AcceptanceLevel::LocalVerifierPassed,
            })
        } else {
            Err(vec![GateFailure::SuiteFailed("store".into())])
        };
        ComparisonEntry {
            candidate: record,
            gate,
            metrics,
        }
    }

    const METRICS: CandidateMetrics = CandidateMetrics {
        tests_passed: 20,
        model_requests: Some(5),
        elapsed_ms: Some(1_000),
        changed_bytes: 900,
    };

    fn contract() -> Sha256Digest {
        candidate().contract_id
    }

    #[test]
    fn the_defective_candidate_is_refused_even_when_cheaper() {
        let good = entry(b"good", true, METRICS);
        let cheap_but_failing = entry(
            b"bad",
            false,
            CandidateMetrics {
                model_requests: Some(1),
                ..METRICS
            },
        );
        let outcome = select(
            &[cheap_but_failing, good.clone()],
            &SelectionPolicy::baseline(),
            contract(),
        );
        assert_eq!(outcome.winner, Some(good.candidate.candidate_digest));
        assert_eq!(outcome.decided_by, DecidedBy::OnlyValidCandidate);
        assert_eq!(outcome.excluded[0].reasons, vec!["gate_suite_failed"]);
    }

    #[test]
    fn both_failing_selects_no_winner() {
        let outcome = select(
            &[entry(b"one", false, METRICS), entry(b"two", false, METRICS)],
            &SelectionPolicy::baseline(),
            contract(),
        );
        assert_eq!(outcome.winner, None);
        assert_eq!(outcome.decided_by, DecidedBy::NoValidCandidate);
        assert_eq!(outcome.excluded.len(), 2);
    }

    #[test]
    fn metrics_decide_in_order_and_unknown_usage_cannot_decide() {
        let fewer_requests = entry(
            b"a",
            true,
            CandidateMetrics {
                model_requests: Some(3),
                ..METRICS
            },
        );
        let more_requests = entry(b"b", true, METRICS);
        let outcome = select(
            &[more_requests.clone(), fewer_requests.clone()],
            &SelectionPolicy::baseline(),
            contract(),
        );
        assert_eq!(
            outcome.winner,
            Some(fewer_requests.candidate.candidate_digest)
        );
        assert_eq!(
            outcome.decided_by,
            DecidedBy::Metric(MetricKey::ModelRequests)
        );
        let unknown = entry(
            b"c",
            true,
            CandidateMetrics {
                model_requests: None,
                elapsed_ms: Some(2_000),
                ..METRICS
            },
        );
        let outcome = select(
            &[unknown, more_requests.clone()],
            &SelectionPolicy::baseline(),
            contract(),
        );
        assert_eq!(
            outcome.winner,
            Some(more_requests.candidate.candidate_digest)
        );
        assert_eq!(outcome.decided_by, DecidedBy::Metric(MetricKey::ElapsedMs));
    }

    #[test]
    fn identical_metrics_use_the_stable_digest_tie_breaker() {
        let first = entry(b"x", true, METRICS);
        let second = entry(b"y", true, METRICS);
        let expected = first
            .candidate
            .candidate_digest
            .min(second.candidate.candidate_digest);
        for order in [[first.clone(), second.clone()], [second, first]] {
            let outcome = select(&order, &SelectionPolicy::baseline(), contract());
            assert_eq!(outcome.winner, Some(expected));
            assert_eq!(outcome.decided_by, DecidedBy::TieBreaker);
        }
    }

    #[test]
    fn mutation_artifacts_and_foreign_contracts_are_excluded() {
        let mut mutation = entry(b"mutant", true, METRICS);
        mutation.candidate.origin = CandidateOrigin::InjectedMutation {
            source_candidate: Sha256Digest::of(b"good"),
            mutation_id: "lost_concurrent_updates".into(),
        };
        let mut foreign = entry(b"foreign", true, METRICS);
        foreign.candidate.contract_id = Sha256Digest::of(b"another contract");
        let outcome = select(
            &[mutation, foreign],
            &SelectionPolicy::baseline(),
            contract(),
        );
        assert_eq!(outcome.winner, None);
        let reasons: Vec<&str> = outcome
            .excluded
            .iter()
            .flat_map(|excluded| excluded.reasons.clone())
            .collect();
        assert!(reasons.contains(&"selection_mutation_artifact"));
        assert!(reasons.contains(&"selection_different_contract"));
    }
}
