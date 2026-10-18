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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::{
        context_bundle::{AttributedClaim, CONTEXT_BUNDLE_SCHEMA_VERSION, RedactionReport},
        messages::SourceFileRef,
        supervisor::{SupervisorExecutionState, SupervisorMode, SupervisorTaskBinding},
        task_spec::fixtures::{id, path, track_a_spec},
    };

    fn execution(workflow: u128, slots: &[&str]) -> SupervisorExecution {
        let binding = SupervisorTaskBinding {
            contract_id: Sha256Digest::of(b"contract"),
            contract_version: 1,
            allowed_slots: slots.iter().map(|slot| id(slot)).collect(),
            dependencies: Vec::new(),
        };
        SupervisorExecution {
            workflow_id: Uuid::from_u128(workflow),
            generation: 1,
            start_request_id: Uuid::from_u128(1),
            mode: SupervisorMode::Deterministic,
            state: SupervisorExecutionState::Running,
            plan_digest: Sha256Digest::of(b"plan"),
            config_digest: Sha256Digest::of(b"config"),
            tasks: BTreeMap::from([(id("track_a"), binding)]),
            slot_harnesses: BTreeMap::new(),
            reviewer_slot: id("reviewer"),
            task_order: vec![id("track_a")],
            task_gates: BTreeMap::new(),
            next_round: 0,
            max_model_calls: 1,
            deadline_unix_ms: 0,
            active_round: None,
        }
    }

    /// The worker session a single-slot execution addresses.
    fn worker(workflow: u128, slot: &str) -> Participant {
        recipient(&execution(workflow, &[slot]), &id("track_a")).expect("single slot")
    }

    fn bundle() -> ContextBundle {
        let snapshot_digest = Sha256Digest::of(b"snapshot");
        ContextBundle {
            schema_version: CONTEXT_BUNDLE_SCHEMA_VERSION,
            bundle_id: Uuid::from_u128(2),
            version: 1,
            workflow_id: Uuid::from_u128(7),
            source: Participant::Supervisor,
            source_task: id("track_a"),
            base_commit: "a".repeat(40),
            snapshot_digest,
            audience: Vec::new(),
            request_message_id: None,
            source_refs: vec![SourceFileRef {
                path: "src/server.mjs".to_string(),
                start_line: 1,
                end_line: 3,
                snapshot_digest,
            }],
            excerpts: Vec::new(),
            claims: Vec::new(),
            redaction: RedactionReport::default(),
            byte_count: 100,
            content_digest: Sha256Digest::of(b"content"),
        }
    }

    fn bundle_with(change: impl FnOnce(&mut ContextBundle)) -> ContextBundle {
        let mut bundle = bundle();
        change(&mut bundle);
        bundle
    }

    fn claim(attribution: Attribution) -> AttributedClaim {
        AttributedClaim {
            attribution,
            text: "the store keeps titles under 120 characters".to_string(),
        }
    }

    fn spec_with(change: impl FnOnce(&mut TaskSpec)) -> TaskSpec {
        let mut spec = track_a_spec();
        change(&mut spec);
        spec
    }

    #[test]
    fn effect_ids_are_stable_and_distinct_per_action_and_kind() {
        let action = Uuid::from_u128(10);
        let effect = effect_id(action, "share_context");
        assert_eq!(effect, effect_id(action, "share_context"));
        assert_ne!(effect, effect_id(action, "grant_read"));
        assert_ne!(effect, effect_id(Uuid::from_u128(11), "share_context"));
        assert_eq!(effect.get_version_num(), 4);
    }

    #[test]
    fn a_task_addressed_tool_reaches_only_a_single_slot_worker() {
        let first = worker(7, "slot_a");
        let Participant::Worker { task_key, .. } = &first else {
            panic!("worker expected: {first:?}");
        };
        assert_eq!(task_key, &id("track_a"));
        assert_eq!(first, worker(7, "slot_a"));
        assert_ne!(first, worker(7, "slot_b"));
        assert_ne!(first, worker(8, "slot_a"));
        // Comparison tasks have several sessions, so no tool may pick one of them.
        for slots in [&["slot_a", "slot_b"][..], &[]] {
            assert_eq!(recipient(&execution(7, slots), &id("track_a")), None);
        }
        assert_eq!(recipient(&execution(7, &["slot_a"]), &id("track_b")), None);
    }

    #[test]
    fn only_a_session_of_a_current_slot_is_a_current_worker() {
        let comparison = execution(7, &["slot_a", "slot_b"]);
        for current in [
            worker(7, "slot_a"),
            worker(7, "slot_b"),
            Participant::Supervisor,
            Participant::Operator,
            Participant::Broker,
        ] {
            assert!(participant_is_current(&comparison, &current), "{current:?}");
        }
        for stale in [
            worker(7, "slot_c"),
            worker(8, "slot_a"),
            Participant::Worker {
                task_key: id("track_a"),
                session_id: Uuid::from_u128(9),
            },
            Participant::Worker {
                task_key: id("track_b"),
                session_id: match worker(7, "slot_a") {
                    Participant::Worker { session_id, .. } => session_id,
                    _ => Uuid::nil(),
                },
            },
        ] {
            assert!(!participant_is_current(&comparison, &stale), "{stale:?}");
        }
    }

    #[test]
    fn a_path_is_readable_only_in_scope_and_outside_forbidden_scopes_of_any_case() {
        let spec = spec_with(|spec| {
            spec.owned_paths.push(path("config/**"));
            spec.forbidden_paths
                .push(path("tests/worker_a/Fixtures/**"));
        });
        let protected = [path("config/keys/**")];
        for permitted in [
            "src/server.mjs",
            "tests/worker_a/case.mjs",
            "CONTRACT.md",
            "config/app.json",
        ] {
            assert!(path_permitted(&spec, permitted, &protected), "{permitted}");
        }
        for refused in [
            "src/other.mjs",
            "src/Server.mjs",
            "package.json",
            "tests/worker_a/Fixtures/a.json",
            "tests/worker_a/fixtures/a.json",
            "config/keys/a.pem",
            "config/Keys/a.pem",
            "/src/server.mjs",
            "src/../package.json",
            "tests/worker_a/../../package.json",
            "config/./keys/a.pem",
            "./CONTRACT.md",
            "src\\server.mjs",
            "",
        ] {
            assert!(!path_permitted(&spec, refused, &protected), "{refused}");
        }
        let unreadable = spec_with(|spec| {
            spec.permitted_tools
                .retain(|tool| *tool != ToolName::ReadFiles)
        });
        assert!(!path_permitted(&unreadable, "src/server.mjs", &protected));
    }

    #[test]
    fn a_bundle_is_shared_only_within_budget_policy_and_scope() {
        let spec = track_a_spec();
        let at_most = |bytes: u32| spec_with(|spec| spec.context_policy.max_bundle_bytes = bytes);
        assert!(bundle_permitted(&bundle(), &spec, &[]));
        assert!(bundle_permitted(
            &bundle_with(|b| b.byte_count = 8192),
            &spec,
            &[]
        ));
        let cap = MAX_SUPERVISOR_CONTEXT_BYTES as u64;
        assert!(bundle_permitted(
            &bundle_with(|b| b.byte_count = cap),
            &at_most(u32::MAX),
            &[]
        ));
        let verified = bundle_with(|b| {
            b.claims.push(claim(Attribution::VerifierConfirmed {
                receipt_digest: Sha256Digest::of(b"receipt"),
            }));
        });
        assert!(bundle_permitted(&verified, &spec, &[]));
        let claims_only = bundle_with(|b| {
            b.source_refs.clear();
            b.claims.push(claim(Attribution::VerifierConfirmed {
                receipt_digest: Sha256Digest::of(b"receipt"),
            }));
        });
        assert!(bundle_permitted(&claims_only, &spec, &[]));

        let worker_claim = claim(Attribution::WorkerClaim {
            task_key: id("track_b"),
        });
        for (index, (refused_bundle, refused_spec)) in [
            (bundle_with(|b| b.byte_count = 0), spec.clone()),
            (bundle_with(|b| b.byte_count = 8193), spec.clone()),
            (bundle_with(|b| b.byte_count = cap + 1), at_most(u32::MAX)),
            (
                bundle(),
                spec_with(|spec| spec.context_policy.allow_public_excerpts = false),
            ),
            (
                bundle(),
                spec_with(|spec| {
                    spec.context_policy
                        .retrieval_order
                        .retain(|kind| *kind != ContextSourceKind::CandidateSnapshot);
                }),
            ),
            (
                bundle(),
                spec_with(|spec| {
                    spec.permitted_tools
                        .retain(|tool| *tool != ToolName::ReadFiles)
                }),
            ),
            (
                bundle_with(|b| b.source_refs[0].snapshot_digest = Sha256Digest::of(b"other")),
                spec.clone(),
            ),
            (
                bundle_with(|b| b.source_refs[0].start_line = 0),
                spec.clone(),
            ),
            (bundle_with(|b| b.source_refs[0].end_line = 0), spec.clone()),
            (
                bundle_with(|b| b.source_refs[0].path = "package.json".to_string()),
                spec.clone(),
            ),
            (
                bundle_with(|b| b.claims.push(worker_claim.clone())),
                spec.clone(),
            ),
            (bundle_with(|b| b.source_refs.clear()), spec.clone()),
        ]
        .into_iter()
        .enumerate()
        {
            assert!(
                !bundle_permitted(&refused_bundle, &refused_spec, &[]),
                "{index}"
            );
        }
        assert!(!bundle_permitted(&bundle(), &spec, &[path("src/**")]));
        let with_worker_claims = spec_with(|spec| {
            spec.context_policy
                .retrieval_order
                .push(ContextSourceKind::WorkerClaims);
        });
        assert!(bundle_permitted(
            &bundle_with(|b| b.claims.push(worker_claim)),
            &with_worker_claims,
            &[]
        ));
    }
}
