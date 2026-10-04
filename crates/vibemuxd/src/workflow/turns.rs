//! Turn prompts, deterministic turn identities, and slot facts
//! (ADR 031 §2, §4).
//!
//! Every turn re-renders the admitted contract from the persisted narrowed
//! TaskSpec with a turn section, so the contract identity is fixed while
//! each attempt records its own prompt digest. Request and session IDs are
//! derived from the workflow, task, slot, purpose, and ordinal, so a
//! retried step resolves to the same dispatch admission instead of a second
//! launch.

use std::collections::BTreeMap;

use uuid::Uuid;
use vibemux_harness::dispatch::Sha256Digest;
use vibemux_workflow::{
    SpecIdentifier,
    renderer::{
        ContextBlock, PromptCapabilities, RenderedPrompt, TurnInputs, TurnPurpose,
        WorkerRenderInputs, render_worker_contract,
    },
    slots::{
        CapabilityEvidence, EvidenceClass, EvidenceLevel, InterfaceMode, SlotBudget,
        SlotCapability, SlotFacts, SlotHealth,
    },
    templates::{REVIEWER_TEMPLATE_V1, ResolvedTemplate},
};

use super::{error::WorkflowError, prepare::WorkflowPlan, settings::LoadedWorkflowConfig};

const REQUEST_ID_DOMAIN: &str = "vibemux.workflow.turn_request.v1";
const SESSION_ID_DOMAIN: &str = "vibemux.workflow.session.v1";
const REVIEW_SESSION_DOMAIN: &str = "vibemux.workflow.review_session.v1";
const LEASE_ID_DOMAIN: &str = "vibemux.workflow.lease.v1";
const ID_DOMAIN: &str = "vibemux.workflow.id.v1";
/// Capabilities a structured coding slot must prove.
pub const WRITABLE_REQUIREMENTS: [(SlotCapability, EvidenceLevel); 2] = [
    (
        SlotCapability::StructuredTurn,
        EvidenceLevel::WriteModeCertified,
    ),
    (
        SlotCapability::WritableWorktree,
        EvidenceLevel::WriteModeCertified,
    ),
];
/// Fixture evidence is re-derived at every daemon start; one day bounds it.
const FIXTURE_EVIDENCE_TTL_MS: u64 = 24 * 60 * 60 * 1000;

/// The workflow ID of an operator request key: a retried prepare resolves
/// to the same workflow and the same private plan path.
#[must_use]
pub fn workflow_id_for(request_key: &str) -> Uuid {
    uuid_from(Sha256Digest::of_fields(
        ID_DOMAIN,
        &[request_key.as_bytes()],
    ))
}

fn uuid_from(digest: Sha256Digest) -> Uuid {
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    uuid::Builder::from_random_bytes(bytes).into_uuid()
}

/// A stable ID derived from `fields` under `domain`, for identities a
/// retried step must resolve to again (message IDs, derived attempts).
#[must_use]
pub fn derived_id(domain: &str, fields: &[&[u8]]) -> Uuid {
    uuid_from(Sha256Digest::of_fields(domain, fields))
}

/// The dispatch request ID of one turn. It is bound to the lease that
/// fences the turn, so a retry under the same lease resolves to the same
/// admission while a new lease generation never collides with an old one.
#[must_use]
pub fn turn_request_id(
    workflow_id: Uuid,
    lease_id: Uuid,
    purpose: TurnPurpose,
    ordinal: u32,
) -> Uuid {
    uuid_from(Sha256Digest::of_fields(
        REQUEST_ID_DOMAIN,
        &[
            workflow_id.as_bytes(),
            lease_id.as_bytes(),
            purpose.as_str().as_bytes(),
            &ordinal.to_be_bytes(),
        ],
    ))
}

/// The lease ID of one unit of work: a retried step reacquires the same
/// lease instead of a second one.
#[must_use]
pub fn lease_id_for(
    workflow_id: Uuid,
    task_key: &SpecIdentifier,
    slot_id: &SpecIdentifier,
    role: &str,
    round: u32,
) -> Uuid {
    uuid_from(Sha256Digest::of_fields(
        LEASE_ID_DOMAIN,
        &[
            workflow_id.as_bytes(),
            task_key.as_str().as_bytes(),
            slot_id.as_str().as_bytes(),
            role.as_bytes(),
            &round.to_be_bytes(),
        ],
    ))
}

/// The session a worker slot holds for one task of one workflow.
#[must_use]
pub fn session_id(workflow_id: Uuid, task_key: &SpecIdentifier, slot_id: &SpecIdentifier) -> Uuid {
    uuid_from(Sha256Digest::of_fields(
        SESSION_ID_DOMAIN,
        &[
            workflow_id.as_bytes(),
            task_key.as_str().as_bytes(),
            slot_id.as_str().as_bytes(),
        ],
    ))
}

/// A fresh reviewer session per review lease, so the reviewer never shares
/// a session with the worker or with another review.
#[must_use]
pub fn review_session_id(workflow_id: Uuid, lease_id: Uuid) -> Uuid {
    uuid_from(Sha256Digest::of_fields(
        REVIEW_SESSION_DOMAIN,
        &[workflow_id.as_bytes(), lease_id.as_bytes()],
    ))
}

/// What one turn should say besides the contract.
pub(crate) struct TurnRequest<'a> {
    pub task_key: &'a SpecIdentifier,
    pub purpose: TurnPurpose,
    pub turn_number: u32,
    pub notes: &'a [String],
    pub context_blocks: &'a [ContextBlock],
    pub reviewed_candidate: Option<Sha256Digest>,
}

/// Renders one turn of the plan's narrowed TaskSpec.
pub(crate) fn render_turn(
    plan: &WorkflowPlan,
    policy_digest: Sha256Digest,
    request: &TurnRequest<'_>,
) -> Result<RenderedPrompt, WorkflowError> {
    let spec = plan
        .specs
        .get(request.task_key)
        .ok_or(WorkflowError::Internal)?;
    let task_spec_digest = spec.digest().map_err(|_| WorkflowError::Internal)?;
    let reviewing = request.purpose == TurnPurpose::Review;
    let version = if reviewing {
        SpecIdentifier::new(REVIEWER_TEMPLATE_V1.template_version)
            .map_err(|_| WorkflowError::Internal)?
    } else {
        spec.template_version.clone()
    };
    let template = ResolvedTemplate::resolve(&version, &plan.overrides_for(version.as_str()))
        .map_err(|error| WorkflowError::ContractRejected {
            codes: vec![error.code().to_string()],
        })?;
    render_worker_contract(WorkerRenderInputs {
        spec,
        task_spec_digest,
        policy_digest,
        template: &template,
        capabilities: PromptCapabilities::UNCERTIFIED,
        context_blocks: request.context_blocks,
        turn: Some(TurnInputs {
            purpose: request.purpose,
            turn_number: request.turn_number,
            notes: request.notes,
            reviewed_candidate: request.reviewed_candidate,
        }),
    })
    .map_err(|error| WorkflowError::ContractRejected {
        codes: vec![error.code().to_string()],
    })
}

/// The slot facts the daemon can honestly claim at startup. A fixture
/// config's slots are write-mode certified only as fixture evidence; a live
/// config has no write-mode certification yet, so its slots stay at
/// `declared` and are never eligible for coding.
pub(crate) fn startup_slot_facts(
    config: &LoadedWorkflowConfig,
    executes: impl Fn(&SpecIdentifier) -> bool,
    now_ms: u64,
) -> Vec<SlotFacts> {
    config
        .config
        .slots
        .iter()
        .map(|slot| {
            let class = config.config.evidence_class;
            let level = match class {
                EvidenceClass::Fixture if executes(&slot.slot_id) => {
                    EvidenceLevel::WriteModeCertified
                }
                _ => EvidenceLevel::Declared,
            };
            let evidence = CapabilityEvidence {
                level,
                class,
                evidence_ref: format!("workflow_config:{}", config.digest),
                checked_at_ms: now_ms,
                expires_at_ms: now_ms.saturating_add(FIXTURE_EVIDENCE_TTL_MS),
            };
            let capabilities: BTreeMap<SlotCapability, CapabilityEvidence> = [
                SlotCapability::StructuredTurn,
                SlotCapability::WritableWorktree,
                SlotCapability::Cancellation,
            ]
            .into_iter()
            .map(|capability| (capability, evidence.clone()))
            .collect();
            SlotFacts {
                slot_id: slot.slot_id.clone(),
                harness: slot.harness,
                interface_mode: InterfaceMode::Structured,
                capabilities,
                health: SlotHealth::Idle,
                route_role: SpecIdentifier::new(slot.route_role.as_str()).ok(),
                cooldown_until_ms: None,
                leased_generation: None,
                budget: SlotBudget::Unknown,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_identities_are_stable_and_distinct() {
        let workflow = workflow_id_for("taskboard-1");
        assert_eq!(workflow, workflow_id_for("taskboard-1"));
        assert_ne!(workflow, workflow_id_for("taskboard-2"));
        let task = SpecIdentifier::new("track_a").expect("task");
        let slot = SpecIdentifier::new("slot_a").expect("slot");
        let lease = lease_id_for(workflow, &task, &slot, "worker", 0);
        assert_eq!(lease, lease_id_for(workflow, &task, &slot, "worker", 0));
        assert_ne!(lease, lease_id_for(workflow, &task, &slot, "worker", 1));
        assert_ne!(lease, lease_id_for(workflow, &task, &slot, "reviewer", 0));
        let first = turn_request_id(workflow, lease, TurnPurpose::Implement, 0);
        assert_eq!(
            first,
            turn_request_id(workflow, lease, TurnPurpose::Implement, 0)
        );
        assert_ne!(
            first,
            turn_request_id(workflow, lease, TurnPurpose::Implement, 1)
        );
        assert_ne!(
            first,
            turn_request_id(workflow, lease, TurnPurpose::Repair, 0)
        );
        let other_lease = lease_id_for(workflow, &task, &slot, "worker", 1);
        assert_ne!(
            first,
            turn_request_id(workflow, other_lease, TurnPurpose::Implement, 0)
        );
        assert_eq!(first.get_version_num(), 4);
        assert_ne!(
            session_id(workflow, &task, &slot),
            session_id(
                workflow,
                &task,
                &SpecIdentifier::new("review").expect("slot")
            )
        );
        assert_ne!(
            review_session_id(workflow, lease),
            review_session_id(workflow, other_lease)
        );
    }
}
