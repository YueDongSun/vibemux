//! Agent slots: capability evidence, eligibility, ranking, and parallelism
//! (ADR 031 §4).
//!
//! A detected executable is not a coding worker. Each capability carries
//! evidence with a level, a class (fixture or live), and an expiry;
//! eligibility requires the levels the operator policy names for the run
//! class. Ranking is a simple deterministic score with the slot ID as the
//! stable tie-breaker; a model may only recommend from the eligible set.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use vibemux_harness::AgentKind;

use crate::SpecIdentifier;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotCapability {
    StructuredTurn,
    WritableWorktree,
    NativeTui,
    ContextExport,
    Cancellation,
    NativeResume,
    VerbatimPrompt,
    ModelRouting,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceLevel {
    Declared,
    InstallationDetected,
    VersionVerified,
    HandshakeVerified,
    AuthenticatedTurnVerified,
    WriteModeCertified,
}

/// Fixture evidence certifies a capability for deterministic fixture runs
/// only; live runs require live evidence.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceClass {
    Fixture,
    Live,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityEvidence {
    pub level: EvidenceLevel,
    pub class: EvidenceClass,
    pub evidence_ref: String,
    pub checked_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InterfaceMode {
    Structured,
    NativeTui,
}

/// Readiness and liveness facts are distinct states, not one "busy" flag.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotHealth {
    Idle,
    Busy,
    WaitingForInput,
    WaitingForPermission,
    Unresponsive,
    Exited,
    QuotaUnknown,
    Cooldown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "remaining")]
pub enum SlotBudget {
    Unknown,
    Remaining(u32),
    Exhausted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SlotFacts {
    pub slot_id: SpecIdentifier,
    pub harness: AgentKind,
    pub interface_mode: InterfaceMode,
    pub capabilities: BTreeMap<SlotCapability, CapabilityEvidence>,
    pub health: SlotHealth,
    /// Route label bound to this slot's worker (for example `worker_a`).
    pub route_role: Option<SpecIdentifier>,
    pub cooldown_until_ms: Option<u64>,
    /// Generation of the slot's active or quarantined lease, if any.
    pub leased_generation: Option<u64>,
    pub budget: SlotBudget,
}

#[derive(Clone, Copy, Debug)]
pub struct EligibilityRequest<'a> {
    pub required: &'a [(SlotCapability, EvidenceLevel)],
    pub class: EvidenceClass,
    pub interface_mode: InterfaceMode,
    pub now_ms: u64,
    pub excluded_slots: &'a [SpecIdentifier],
    /// Operator opt-in to schedule on slots whose quota is unknown, under
    /// the explicit concurrency limit. Unknown is never "unlimited".
    pub allow_unknown_quota: bool,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case", tag = "reason", content = "detail")]
pub enum Ineligibility {
    MissingCapability(SlotCapability),
    EvidenceTooWeak(SlotCapability),
    EvidenceWrongClass(SlotCapability),
    EvidenceExpired(SlotCapability),
    InterfaceMismatch,
    Unhealthy(SlotHealth),
    CoolingDown,
    Leased,
    Excluded,
    QuotaExhausted,
    QuotaUnknownNotAllowed,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct EligibilityReport {
    pub eligible: Vec<SpecIdentifier>,
    pub ineligible: Vec<(SpecIdentifier, Vec<Ineligibility>)>,
}

#[must_use]
pub fn evaluate_eligibility(
    slots: &[SlotFacts],
    request: EligibilityRequest<'_>,
) -> EligibilityReport {
    let mut report = EligibilityReport::default();
    for slot in slots {
        let reasons = ineligibility(slot, request);
        if reasons.is_empty() {
            report.eligible.push(slot.slot_id.clone());
        } else {
            report.ineligible.push((slot.slot_id.clone(), reasons));
        }
    }
    report.eligible.sort();
    report.ineligible.sort();
    report
}

fn ineligibility(slot: &SlotFacts, request: EligibilityRequest<'_>) -> Vec<Ineligibility> {
    let mut reasons = Vec::new();
    for (capability, minimum) in request.required {
        match slot.capabilities.get(capability) {
            None => reasons.push(Ineligibility::MissingCapability(*capability)),
            Some(evidence) => {
                if evidence.level < *minimum {
                    reasons.push(Ineligibility::EvidenceTooWeak(*capability));
                }
                if evidence.class != request.class {
                    reasons.push(Ineligibility::EvidenceWrongClass(*capability));
                }
                if evidence.expires_at_ms <= request.now_ms {
                    reasons.push(Ineligibility::EvidenceExpired(*capability));
                }
            }
        }
    }
    if slot.interface_mode != request.interface_mode {
        reasons.push(Ineligibility::InterfaceMismatch);
    }
    match slot.health {
        SlotHealth::Idle => {}
        SlotHealth::QuotaUnknown if request.allow_unknown_quota => {}
        SlotHealth::QuotaUnknown => reasons.push(Ineligibility::QuotaUnknownNotAllowed),
        SlotHealth::Cooldown => reasons.push(Ineligibility::CoolingDown),
        other => reasons.push(Ineligibility::Unhealthy(other)),
    }
    if slot
        .cooldown_until_ms
        .is_some_and(|until| until > request.now_ms)
        && slot.health != SlotHealth::Cooldown
    {
        reasons.push(Ineligibility::CoolingDown);
    }
    if slot.leased_generation.is_some() {
        reasons.push(Ineligibility::Leased);
    }
    if request.excluded_slots.contains(&slot.slot_id) {
        reasons.push(Ineligibility::Excluded);
    }
    match slot.budget {
        SlotBudget::Exhausted | SlotBudget::Remaining(0) => {
            reasons.push(Ineligibility::QuotaExhausted)
        }
        SlotBudget::Unknown if !request.allow_unknown_quota => {
            reasons.push(Ineligibility::QuotaUnknownNotAllowed);
        }
        SlotBudget::Unknown | SlotBudget::Remaining(_) => {}
    }
    reasons.sort();
    reasons.dedup();
    reasons
}

/// Integer weights inside the eligibility rules; the optimizer may tune
/// them, but they only order already-eligible slots.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RankingWeights {
    pub known_budget: u32,
    pub live_evidence: u32,
    pub native_resume: u32,
}

impl RankingWeights {
    pub const BASELINE: Self = Self {
        known_budget: 2,
        live_evidence: 1,
        native_resume: 0,
    };
}

/// Eligible slots in preference order; ties break on the slot ID.
#[must_use]
pub fn rank_slots(
    slots: &[SlotFacts],
    eligible: &[SpecIdentifier],
    weights: RankingWeights,
) -> Vec<SpecIdentifier> {
    let mut scored: Vec<(u64, &SpecIdentifier)> = eligible
        .iter()
        .filter_map(|id| slots.iter().find(|slot| &slot.slot_id == id))
        .map(|slot| {
            let mut score = 0_u64;
            if matches!(slot.budget, SlotBudget::Remaining(_)) {
                score += u64::from(weights.known_budget);
            }
            if slot
                .capabilities
                .values()
                .all(|evidence| evidence.class == EvidenceClass::Live)
            {
                score += u64::from(weights.live_evidence);
            }
            if slot
                .capabilities
                .contains_key(&SlotCapability::NativeResume)
            {
                score += u64::from(weights.native_resume);
            }
            (score, &slot.slot_id)
        })
        .collect();
    scored.sort_by(|(left_score, left_id), (right_score, right_id)| {
        right_score
            .cmp(left_score)
            .then_with(|| left_id.cmp(right_id))
    });
    scored.into_iter().map(|(_, id)| id.clone()).collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "plan")]
pub enum ParallelismPlan {
    /// Nothing can start; the work stays queued with this reason.
    Blocked {
        reason: &'static str,
    },
    /// One task at a time; no parallelism is claimed.
    Sequential,
    Parallel {
        width: u32,
    },
}

/// How many independent tasks may run at once.
#[must_use]
pub fn plan_parallelism(
    eligible_slots: usize,
    runnable_tasks: usize,
    max_parallel: u32,
) -> ParallelismPlan {
    if runnable_tasks == 0 {
        return ParallelismPlan::Blocked {
            reason: "no_runnable_task",
        };
    }
    if eligible_slots == 0 {
        return ParallelismPlan::Blocked {
            reason: "no_eligible_slot",
        };
    }
    if max_parallel == 0 {
        return ParallelismPlan::Blocked {
            reason: "zero_concurrency_limit",
        };
    }
    let width = eligible_slots
        .min(runnable_tasks)
        .min(usize::try_from(max_parallel).unwrap_or(usize::MAX));
    if width <= 1 {
        ParallelismPlan::Sequential
    } else {
        ParallelismPlan::Parallel {
            width: u32::try_from(width).unwrap_or(max_parallel),
        }
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    pub fn evidence(level: EvidenceLevel, class: EvidenceClass) -> CapabilityEvidence {
        CapabilityEvidence {
            level,
            class,
            evidence_ref: "fixture".into(),
            checked_at_ms: 0,
            expires_at_ms: 10_000,
        }
    }

    pub fn writable_slot(id: &str, harness: AgentKind) -> SlotFacts {
        let mut capabilities = BTreeMap::new();
        capabilities.insert(
            SlotCapability::StructuredTurn,
            evidence(EvidenceLevel::WriteModeCertified, EvidenceClass::Fixture),
        );
        capabilities.insert(
            SlotCapability::WritableWorktree,
            evidence(EvidenceLevel::WriteModeCertified, EvidenceClass::Fixture),
        );
        capabilities.insert(
            SlotCapability::Cancellation,
            evidence(EvidenceLevel::HandshakeVerified, EvidenceClass::Fixture),
        );
        SlotFacts {
            slot_id: SpecIdentifier::new(id).expect("slot id"),
            harness,
            interface_mode: InterfaceMode::Structured,
            capabilities,
            health: SlotHealth::Idle,
            route_role: None,
            cooldown_until_ms: None,
            leased_generation: None,
            budget: SlotBudget::Remaining(10),
        }
    }

    pub const WRITABLE: &[(SlotCapability, EvidenceLevel)] = &[
        (
            SlotCapability::StructuredTurn,
            EvidenceLevel::WriteModeCertified,
        ),
        (
            SlotCapability::WritableWorktree,
            EvidenceLevel::WriteModeCertified,
        ),
    ];

    pub fn request(class: EvidenceClass) -> EligibilityRequest<'static> {
        EligibilityRequest {
            required: WRITABLE,
            class,
            interface_mode: InterfaceMode::Structured,
            now_ms: 1_000,
            excluded_slots: &[],
            allow_unknown_quota: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{fixtures::*, *};

    #[test]
    fn zero_one_and_two_slots_plan_blocked_sequential_and_parallel() {
        assert_eq!(
            plan_parallelism(0, 2, 2),
            ParallelismPlan::Blocked {
                reason: "no_eligible_slot"
            }
        );
        assert_eq!(plan_parallelism(1, 2, 2), ParallelismPlan::Sequential);
        assert_eq!(
            plan_parallelism(2, 2, 2),
            ParallelismPlan::Parallel { width: 2 }
        );
        assert_eq!(
            plan_parallelism(5, 2, 2),
            ParallelismPlan::Parallel { width: 2 }
        );
        assert_eq!(plan_parallelism(5, 3, 1), ParallelismPlan::Sequential);
        assert_eq!(
            plan_parallelism(2, 0, 2),
            ParallelismPlan::Blocked {
                reason: "no_runnable_task"
            }
        );
    }

    #[test]
    fn probe_only_and_detected_only_slots_are_not_coding_workers() {
        let mut acp = writable_slot("opencode_acp", AgentKind::OpenCode);
        acp.capabilities.remove(&SlotCapability::WritableWorktree);
        acp.capabilities.insert(
            SlotCapability::StructuredTurn,
            evidence(EvidenceLevel::HandshakeVerified, EvidenceClass::Fixture),
        );
        let mut detected = writable_slot("grok_detected", AgentKind::Grok);
        for evidence in detected.capabilities.values_mut() {
            evidence.level = EvidenceLevel::InstallationDetected;
        }
        let report = evaluate_eligibility(
            &[acp, detected, writable_slot("claude_a", AgentKind::Claude)],
            request(EvidenceClass::Fixture),
        );
        assert_eq!(
            report.eligible,
            vec![SpecIdentifier::new("claude_a").expect("id")]
        );
        let reasons: BTreeMap<String, Vec<Ineligibility>> = report
            .ineligible
            .into_iter()
            .map(|(id, reasons)| (id.to_string(), reasons))
            .collect();
        assert!(
            reasons["opencode_acp"].contains(&Ineligibility::MissingCapability(
                SlotCapability::WritableWorktree
            ))
        );
        assert!(
            reasons["opencode_acp"].contains(&Ineligibility::EvidenceTooWeak(
                SlotCapability::StructuredTurn
            ))
        );
        assert!(
            reasons["grok_detected"].contains(&Ineligibility::EvidenceTooWeak(
                SlotCapability::WritableWorktree
            ))
        );
    }

    #[test]
    fn fixture_evidence_never_qualifies_a_live_run() {
        let report = evaluate_eligibility(
            &[writable_slot("claude_a", AgentKind::Claude)],
            request(EvidenceClass::Live),
        );
        assert!(report.eligible.is_empty());
        assert!(
            report.ineligible[0]
                .1
                .contains(&Ineligibility::EvidenceWrongClass(
                    SlotCapability::WritableWorktree
                ))
        );
    }

    #[test]
    fn leased_expired_unhealthy_and_unknown_quota_slots_are_excluded() {
        let mut leased = writable_slot("leased", AgentKind::Codex);
        leased.leased_generation = Some(3);
        let mut expired = writable_slot("expired", AgentKind::Codex);
        for evidence in expired.capabilities.values_mut() {
            evidence.expires_at_ms = 500;
        }
        let mut waiting = writable_slot("waiting", AgentKind::Claude);
        waiting.health = SlotHealth::WaitingForPermission;
        let mut unknown = writable_slot("unknown", AgentKind::Claude);
        unknown.budget = SlotBudget::Unknown;
        let slots = [leased, expired, waiting, unknown.clone()];
        let report = evaluate_eligibility(&slots, request(EvidenceClass::Fixture));
        assert!(report.eligible.is_empty());
        let mut opted_in = request(EvidenceClass::Fixture);
        opted_in.allow_unknown_quota = true;
        let report = evaluate_eligibility(&[unknown], opted_in);
        assert_eq!(report.eligible.len(), 1);
    }

    #[test]
    fn ranking_is_deterministic_with_slot_id_tie_break() {
        let slots = [
            writable_slot("slot_b", AgentKind::Codex),
            writable_slot("slot_a", AgentKind::Claude),
            {
                let mut unknown = writable_slot("slot_c", AgentKind::Claude);
                unknown.budget = SlotBudget::Unknown;
                unknown
            },
        ];
        let ids: Vec<SpecIdentifier> = slots.iter().map(|slot| slot.slot_id.clone()).collect();
        let ranked = rank_slots(&slots, &ids, RankingWeights::BASELINE);
        let names: Vec<&str> = ranked.iter().map(SpecIdentifier::as_str).collect();
        assert_eq!(names, vec!["slot_a", "slot_b", "slot_c"]);
        assert_eq!(rank_slots(&slots, &ids, RankingWeights::BASELINE), ranked);
    }
}
