//! Pure identity and least-privilege checks for supervisor context effects.

use super::{MAX_SUPERVISOR_CONTEXT_BYTES, SupervisorExecution};
use crate::{
    PathPattern, Sha256Digest, SpecIdentifier,
    context_bundle::{Attribution, ContextBundle},
    identifiers::is_relative_path,
    messages::Participant,
    task_spec::{ContextSourceKind, TaskSpec, ToolName},
};
use uuid::Uuid;

const SESSION_DOMAIN: &str = "vibemux.workflow.session.v1";
const EFFECT_DOMAIN: &str = "vibemux.workflow.supervisor_context_effect.v1";

#[must_use]
pub fn effect_id(action_id: Uuid, kind: &str) -> Uuid {
    uuid_from_digest(Sha256Digest::of_fields(
        EFFECT_DOMAIN,
        &[action_id.as_bytes(), kind.as_bytes()],
    ))
}

fn uuid_from_digest(digest: Sha256Digest) -> Uuid {
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    uuid::Builder::from_random_bytes(bytes).into_uuid()
}

/// A task-addressed tool cannot choose arbitrarily among comparison sessions.
#[must_use]
pub fn recipient(
    execution: &SupervisorExecution,
    task_key: &SpecIdentifier,
) -> Option<Participant> {
    let slots = &execution.tasks.get(task_key)?.allowed_slots;
    let [slot] = slots.as_slice() else {
        return None;
    };
    Some(worker_participant(execution.workflow_id, task_key, slot))
}

fn worker_participant(
    workflow_id: Uuid,
    task_key: &SpecIdentifier,
    slot: &SpecIdentifier,
) -> Participant {
    Participant::Worker {
        task_key: task_key.clone(),
        session_id: uuid_from_digest(Sha256Digest::of_fields(
            SESSION_DOMAIN,
            &[
                workflow_id.as_bytes(),
                task_key.as_str().as_bytes(),
                slot.as_str().as_bytes(),
            ],
        )),
    }
}

#[must_use]
pub fn participant_is_current(execution: &SupervisorExecution, participant: &Participant) -> bool {
    match participant {
        Participant::Worker { task_key, .. } => {
            execution.tasks.get(task_key).is_some_and(|binding| {
                binding.allowed_slots.iter().any(|slot| {
                    worker_participant(execution.workflow_id, task_key, slot) == *participant
                })
            })
        }
        Participant::Supervisor | Participant::Operator | Participant::Broker => true,
    }
}

#[must_use]
pub fn path_permitted(spec: &TaskSpec, path: &str, protected: &[PathPattern]) -> bool {
    is_relative_path(path)
        && spec.permitted_tools.contains(&ToolName::ReadFiles)
        && !spec.forbidden_paths.iter().chain(protected).any(|scope| {
            scope.matches(path)
                || PathPattern::new(scope.as_str().to_ascii_lowercase())
                    .is_ok_and(|folded| folded.matches(&path.to_ascii_lowercase()))
        })
        && spec
            .owned_paths
            .iter()
            .chain(&spec.read_scopes)
            .any(|scope| scope.matches(path))
}

#[must_use]
pub fn bundle_permitted(
    bundle: &ContextBundle,
    spec: &TaskSpec,
    protected: &[PathPattern],
) -> bool {
    let policy = &spec.context_policy;
    bundle.byte_count > 0
        && bundle.byte_count
            <= u64::from(policy.max_bundle_bytes).min(MAX_SUPERVISOR_CONTEXT_BYTES as u64)
        && spec.permitted_tools.contains(&ToolName::ReadFiles)
        && policy.allow_public_excerpts
        && bundle.source_refs.iter().all(|source| {
            policy
                .retrieval_order
                .contains(&ContextSourceKind::CandidateSnapshot)
                && source.snapshot_digest == bundle.snapshot_digest
                && source.start_line > 0
                && source.end_line >= source.start_line
                && path_permitted(spec, &source.path, protected)
        })
        && bundle.claims.iter().all(|claim| {
            policy.retrieval_order.contains(&match claim.attribution {
                Attribution::WorkerClaim { .. } => ContextSourceKind::WorkerClaims,
                Attribution::VerifierConfirmed { .. } => ContextSourceKind::VerifierReceipts,
            })
        })
        && (!bundle.source_refs.is_empty() || !bundle.claims.is_empty())
}
