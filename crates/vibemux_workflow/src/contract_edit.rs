//! Conservative operator edits of an admitted contract.
//!
//! Edits may change wording or narrow authority. A new task, source, role,
//! route, broader grant or weaker acceptance requires separate admission.

use thiserror::Error;

use crate::{
    contract::{SemanticDiff, next_generation},
    task_spec::TaskSpec,
};

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ContractEditError {
    #[error("contract edit changes immutable authority")]
    ImmutableAuthority,
    #[error("contract edit broadens a grant or weakens a requirement")]
    AuthorityWidened,
    #[error("contract edit has an invalid generation")]
    Generation,
}

impl ContractEditError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::ImmutableAuthority => "workflow_edit_immutable_authority",
            Self::AuthorityWidened => "workflow_edit_authority_widened",
            Self::Generation => "workflow_edit_invalid_generation",
        }
    }
}

/// Both inputs must already have passed structural/source/policy admission.
/// The caller separately checks exact parent version, source bytes and plan.
pub fn validate_semantic_edit(
    old: &TaskSpec,
    new: &TaskSpec,
) -> Result<SemanticDiff, ContractEditError> {
    let mut identity = new.clone();
    identity.contract_version = old.contract_version;
    identity.requirements.clone_from(&old.requirements);
    identity.objective.clone_from(&old.objective);
    identity.non_goals.clone_from(&old.non_goals);
    identity.owned_paths.clone_from(&old.owned_paths);
    identity.read_scopes.clone_from(&old.read_scopes);
    identity.forbidden_paths.clone_from(&old.forbidden_paths);
    identity.permitted_tools.clone_from(&old.permitted_tools);
    identity
        .acceptance_checks
        .clone_from(&old.acceptance_checks);
    identity.resource_budget = old.resource_budget;
    if identity != *old {
        return Err(ContractEditError::ImmutableAuthority);
    }
    let retained = old
        .requirements
        .iter()
        .all(|value| new.requirements.contains(value))
        && old
            .non_goals
            .iter()
            .all(|value| new.non_goals.contains(value))
        && old
            .acceptance_checks
            .iter()
            .all(|value| new.acceptance_checks.contains(value));
    let narrower = new
        .owned_paths
        .iter()
        .all(|path| old.owned_paths.iter().any(|root| path.is_within(root)))
        && new
            .read_scopes
            .iter()
            .all(|path| old.read_scopes.iter().any(|root| path.is_within(root)))
        && old
            .forbidden_paths
            .iter()
            .all(|path| new.forbidden_paths.iter().any(|root| path.is_within(root)))
        && new
            .permitted_tools
            .iter()
            .all(|tool| old.permitted_tools.contains(tool));
    let before = old.resource_budget;
    let after = new.resource_budget;
    let bounded = after.max_turns <= before.max_turns
        && after.max_repairs <= before.max_repairs
        && after.max_elapsed_seconds <= before.max_elapsed_seconds
        && after.max_model_requests <= before.max_model_requests;
    if !retained || !narrower || !bounded {
        return Err(ContractEditError::AuthorityWidened);
    }
    next_generation(old, new).map_err(|_| ContractEditError::Generation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_spec::{ToolName, fixtures::track_a_spec};

    #[test]
    fn wording_edit_advances_generation_and_keeps_source_authority() {
        let old = track_a_spec().normalized();
        let mut new = old.clone();
        new.objective
            .push_str(" Preserve deterministic error reporting.");
        new.contract_version += 1;
        let diff = validate_semantic_edit(&old, &new).expect("bounded edit");
        assert!(diff.objective_changed);
        assert_eq!(new.original_request_ref, old.original_request_ref);
        assert_eq!(new.requirements, old.requirements);
        new.contract_version = old.contract_version;
        assert_eq!(
            validate_semantic_edit(&old, &new),
            Err(ContractEditError::Generation)
        );
    }

    #[test]
    fn edits_cannot_restore_tools_drop_checks_or_reset_caps() {
        let mut old = track_a_spec().normalized();
        old.permitted_tools
            .retain(|tool| *tool != ToolName::SearchFiles);
        let mut new = old.clone();
        new.contract_version += 1;
        new.permitted_tools.push(ToolName::SearchFiles);
        assert_eq!(
            validate_semantic_edit(&old, &new),
            Err(ContractEditError::AuthorityWidened)
        );
        new = old.clone();
        new.acceptance_checks.pop();
        assert_eq!(
            validate_semantic_edit(&old, &new),
            Err(ContractEditError::AuthorityWidened)
        );
        new = old.clone();
        new.resource_budget.max_turns += 1;
        assert_eq!(
            validate_semantic_edit(&old, &new),
            Err(ContractEditError::AuthorityWidened)
        );
        new = old.clone();
        new.role = crate::task_spec::WorkerRole::Reviewer;
        assert_eq!(
            validate_semantic_edit(&old, &new),
            Err(ContractEditError::ImmutableAuthority)
        );
    }
}
