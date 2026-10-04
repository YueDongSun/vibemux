//! Contract identity, immutable contract artifacts, and contract
//! generations (ADR 031 §2).
//!
//! The contract identity is a digest of the complete immutable input: the
//! canonical TaskSpec, the operator policy, the resolved template, the
//! harness prompt profile, any route-side prompt transformation, the
//! context bundles delivered with it, and the base commit. Admission caches
//! accepted artifacts by this identity, so replaying identical inputs reuses
//! the accepted artifact, and any changed input yields a new identity.
//!
//! A semantic edit (requirements, paths, tools, checks, budgets) produces a
//! new generation with `contract_version + 1` and a recorded diff; work
//! admitted under an older generation keeps its contract.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    Sha256Digest,
    renderer::{PromptCapabilities, RenderedPrompt},
    task_spec::{Requirement, TaskSpec},
};

pub const CONTRACT_IDENTITY_DOMAIN: &str = "vibemux.workflow.contract_identity.v1";
pub const CONTRACT_ARTIFACT_SCHEMA_VERSION: u32 = 1;

/// A route-side prompt transformation that the gateway applies. `none` is
/// the only value admitted for strict routes; anything else must be part of
/// the identity so the effective prompt is accounted for.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "identifier")]
pub enum RouteTransformation {
    None,
    Declared(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractIdentityInputs<'a> {
    pub task_spec_digest: Sha256Digest,
    pub policy_digest: Sha256Digest,
    pub template_digest: Sha256Digest,
    pub harness_profile: &'a str,
    pub prompt_capabilities: PromptCapabilities,
    pub route_transformation: &'a RouteTransformation,
    pub context_bundle_digests: &'a [Sha256Digest],
    pub base_commit: &'a str,
}

#[must_use]
pub fn contract_identity(inputs: &ContractIdentityInputs<'_>) -> Sha256Digest {
    let capabilities =
        serde_json::to_vec(&inputs.prompt_capabilities).unwrap_or_else(|_| b"unencodable".to_vec());
    let transformation =
        serde_json::to_vec(inputs.route_transformation).unwrap_or_else(|_| b"unencodable".to_vec());
    let mut bundles: Vec<Sha256Digest> = inputs.context_bundle_digests.to_vec();
    bundles.sort();
    let bundle_bytes: Vec<u8> = bundles
        .iter()
        .flat_map(|digest| *digest.as_bytes())
        .collect();
    Sha256Digest::of_fields(
        CONTRACT_IDENTITY_DOMAIN,
        &[
            inputs.task_spec_digest.as_bytes(),
            inputs.policy_digest.as_bytes(),
            inputs.template_digest.as_bytes(),
            inputs.harness_profile.as_bytes(),
            &capabilities,
            &transformation,
            &bundle_bytes,
            inputs.base_commit.as_bytes(),
        ],
    )
}

/// The immutable, content-free record of an admitted contract. The rendered
/// bytes live in the opt-in content store under `rendered_prompt_digest`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContractArtifact {
    pub schema_version: u32,
    pub contract_id: Sha256Digest,
    pub workflow_key: String,
    pub task_key: String,
    pub contract_version: u32,
    pub task_spec_digest: Sha256Digest,
    pub policy_digest: Sha256Digest,
    pub template_digest: Sha256Digest,
    pub template_version: String,
    pub harness_profile: String,
    pub route_transformation: RouteTransformation,
    pub context_bundle_digests: Vec<Sha256Digest>,
    pub base_commit: String,
    pub rendered_prompt_digest: Sha256Digest,
    pub rendered_prompt_bytes: u64,
    pub previous_contract_id: Option<Sha256Digest>,
    pub semantic_diff: Option<SemanticDiff>,
}

impl ContractArtifact {
    #[must_use]
    pub fn new(
        spec: &TaskSpec,
        identity: &ContractIdentityInputs<'_>,
        rendered: &RenderedPrompt,
        previous: Option<(&ContractArtifact, SemanticDiff)>,
    ) -> Self {
        let mut bundles = identity.context_bundle_digests.to_vec();
        bundles.sort();
        let (previous_contract_id, semantic_diff) = match previous {
            Some((artifact, diff)) => (Some(artifact.contract_id), Some(diff)),
            None => (None, None),
        };
        Self {
            schema_version: CONTRACT_ARTIFACT_SCHEMA_VERSION,
            contract_id: contract_identity(identity),
            workflow_key: spec.workflow_key.to_string(),
            task_key: spec.task_key.to_string(),
            contract_version: spec.contract_version,
            task_spec_digest: identity.task_spec_digest,
            policy_digest: identity.policy_digest,
            template_digest: identity.template_digest,
            template_version: spec.template_version.to_string(),
            harness_profile: identity.harness_profile.to_string(),
            route_transformation: identity.route_transformation.clone(),
            context_bundle_digests: bundles,
            base_commit: identity.base_commit.to_string(),
            rendered_prompt_digest: rendered.digest,
            rendered_prompt_bytes: rendered.byte_count as u64,
            previous_contract_id,
            semantic_diff,
        }
    }
}

/// What changed in meaning between two generations of one task.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticDiff {
    pub requirements_added: Vec<String>,
    pub requirements_removed: Vec<String>,
    pub requirements_changed: Vec<String>,
    pub scope_changed: bool,
    pub tools_changed: bool,
    pub checks_changed: bool,
    pub budget_changed: bool,
    pub objective_changed: bool,
}

impl SemanticDiff {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum GenerationError {
    #[error("a new generation must keep the workflow and task keys")]
    DifferentTask,
    #[error("a semantic edit must advance the contract version by one")]
    VersionNotAdvanced,
    #[error("an unchanged contract must not advance its version")]
    SpuriousGeneration,
}

impl GenerationError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::DifferentTask => "workflow_contract_different_task",
            Self::VersionNotAdvanced => "workflow_contract_version_not_advanced",
            Self::SpuriousGeneration => "workflow_contract_spurious_generation",
        }
    }
}

#[must_use]
pub fn semantic_diff(old: &TaskSpec, new: &TaskSpec) -> SemanticDiff {
    let index = |spec: &TaskSpec| -> BTreeMap<String, Requirement> {
        spec.requirements
            .iter()
            .map(|requirement| (requirement.requirement_id.to_string(), requirement.clone()))
            .collect()
    };
    let (before, after) = (index(old), index(new));
    let before_ids: BTreeSet<&String> = before.keys().collect();
    let after_ids: BTreeSet<&String> = after.keys().collect();
    SemanticDiff {
        requirements_added: after_ids
            .difference(&before_ids)
            .map(|id| (*id).clone())
            .collect(),
        requirements_removed: before_ids
            .difference(&after_ids)
            .map(|id| (*id).clone())
            .collect(),
        requirements_changed: before_ids
            .intersection(&after_ids)
            .filter(|id| before.get(**id) != after.get(**id))
            .map(|id| (*id).clone())
            .collect(),
        scope_changed: old.owned_paths != new.owned_paths
            || old.read_scopes != new.read_scopes
            || old.forbidden_paths != new.forbidden_paths,
        tools_changed: old.permitted_tools != new.permitted_tools,
        checks_changed: old.acceptance_checks != new.acceptance_checks,
        budget_changed: old.resource_budget != new.resource_budget,
        objective_changed: old.objective != new.objective || old.non_goals != new.non_goals,
    }
}

/// Validates that `new` is a legitimate successor of `old` and returns the
/// diff to record. Identical meaning must keep the version (replay); any
/// semantic edit must advance it by exactly one.
pub fn next_generation(old: &TaskSpec, new: &TaskSpec) -> Result<SemanticDiff, GenerationError> {
    if old.workflow_key != new.workflow_key || old.task_key != new.task_key {
        return Err(GenerationError::DifferentTask);
    }
    let diff = semantic_diff(old, new);
    if diff.is_empty() {
        if new.contract_version != old.contract_version {
            return Err(GenerationError::SpuriousGeneration);
        }
    } else if old.contract_version.checked_add(1) != Some(new.contract_version) {
        return Err(GenerationError::VersionNotAdvanced);
    }
    Ok(diff)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{renderer::ExpansionBehavior, task_spec::fixtures::track_a_spec};

    const CAPABILITIES: PromptCapabilities = PromptCapabilities {
        at_mention_expansion: ExpansionBehavior::None,
        leading_command_prefixes: true,
        verbatim_mode: false,
    };

    fn inputs<'a>(
        spec_digest: Sha256Digest,
        bundles: &'a [Sha256Digest],
        transformation: &'a RouteTransformation,
    ) -> ContractIdentityInputs<'a> {
        ContractIdentityInputs {
            task_spec_digest: spec_digest,
            policy_digest: Sha256Digest::of(b"policy"),
            template_digest: Sha256Digest::of(b"template"),
            harness_profile: "claude_stream_json_writable_v1",
            prompt_capabilities: CAPABILITIES,
            route_transformation: transformation,
            context_bundle_digests: bundles,
            base_commit: "abc",
        }
    }

    #[test]
    fn identity_is_stable_and_sensitive_to_every_input() {
        let digest = track_a_spec().normalized().digest().expect("digest");
        let none = RouteTransformation::None;
        let base = contract_identity(&inputs(digest, &[], &none));
        assert_eq!(base, contract_identity(&inputs(digest, &[], &none)));
        let bundles = [Sha256Digest::of(b"one"), Sha256Digest::of(b"two")];
        let reversed = [bundles[1], bundles[0]];
        assert_eq!(
            contract_identity(&inputs(digest, &bundles, &none)),
            contract_identity(&inputs(digest, &reversed, &none)),
            "bundle order is not meaning"
        );
        let declared = RouteTransformation::Declared("responses_cot".into());
        let mut variants = vec![
            contract_identity(&inputs(Sha256Digest::of(b"other spec"), &[], &none)),
            contract_identity(&inputs(digest, &bundles, &none)),
            contract_identity(&inputs(digest, &[], &declared)),
        ];
        let mut other_profile = inputs(digest, &[], &none);
        other_profile.harness_profile = "codex_exec_writable_v1";
        variants.push(contract_identity(&other_profile));
        let mut other_base = inputs(digest, &[], &none);
        other_base.base_commit = "def";
        variants.push(contract_identity(&other_base));
        let mut uncertified = inputs(digest, &[], &none);
        uncertified.prompt_capabilities = PromptCapabilities::UNCERTIFIED;
        variants.push(contract_identity(&uncertified));
        for variant in variants {
            assert_ne!(variant, base);
        }
    }

    #[test]
    fn semantic_edits_require_a_new_generation() {
        let old = track_a_spec().normalized();
        let mut edited = old.clone();
        edited.requirements[1].statement = "Reject titles longer than 120 code points.".into();
        assert_eq!(
            next_generation(&old, &edited),
            Err(GenerationError::VersionNotAdvanced)
        );
        edited.contract_version = 2;
        let diff = next_generation(&old, &edited).expect("successor");
        assert_eq!(diff.requirements_changed, vec!["r02".to_string()]);
        assert!(!diff.scope_changed);
        assert_eq!(
            next_generation(&old, &old.clone()),
            Ok(SemanticDiff::default())
        );
        let mut spurious = old.clone();
        spurious.contract_version = 2;
        assert_eq!(
            next_generation(&old, &spurious),
            Err(GenerationError::SpuriousGeneration)
        );
        let mut other = edited.clone();
        other.task_key = crate::task_spec::fixtures::id("track_b");
        assert_eq!(
            next_generation(&old, &other),
            Err(GenerationError::DifferentTask)
        );
    }

    #[test]
    fn scope_and_requirement_set_changes_are_reported() {
        let old = track_a_spec().normalized();
        let mut new = old.clone();
        new.contract_version = 2;
        new.owned_paths.pop();
        new.requirements.pop();
        let diff = next_generation(&old, &new).expect("successor");
        assert!(diff.scope_changed);
        assert_eq!(diff.requirements_removed, vec!["r04".to_string()]);
    }
}
