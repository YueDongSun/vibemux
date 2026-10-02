//! Operator policy and its intersection with a TaskSpec candidate (ADR 031
//! §2).
//!
//! The policy comes from a trusted operator file, never from the request or
//! a model. Intersection can only narrow: tools outside the policy are
//! removed, budgets are capped, and protected paths are added to the
//! forbidden set, each recorded as a visible narrowing. A candidate that
//! asks for a write scope outside the policy, a protected path, or an
//! undeclared verifier suite is rejected instead, because silently dropping
//! a requested write scope would change the task's meaning.

use serde::{Deserialize, Serialize};

use crate::{
    PathPattern, Sha256Digest, SpecIdentifier,
    canonical_json::{CanonicalError, canonical_digest},
    task_spec::{CheckKind, ResourceBudget, SpecViolation, TaskSpec, ToolName},
};

pub const OPERATOR_POLICY_SCHEMA_VERSION: u32 = 1;
pub const POLICY_DIGEST_DOMAIN: &str = "vibemux.workflow.operator_policy.v1";
pub const MAX_POLICY_ITEMS: usize = 64;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VerifierSuiteDecl {
    pub suite_id: SpecIdentifier,
    pub description: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorPolicy {
    pub schema_version: u32,
    pub policy_version: SpecIdentifier,
    /// Every owned path must lie inside one of these.
    pub writable_roots: Vec<PathPattern>,
    /// Never writable by a worker: verifier, contract, manifests, lockfiles.
    pub protected_paths: Vec<PathPattern>,
    pub allowed_tools: Vec<ToolName>,
    pub verifier_suites: Vec<VerifierSuiteDecl>,
    pub budget_caps: ResourceBudget,
    pub max_parallel_slots: u32,
    pub max_supervisor_decisions: u32,
    pub max_optimizer_candidates: u32,
    /// Opt-in private content store for prompts, transcripts and bundles.
    pub content_store_opt_in: bool,
    /// Model calls may only use configured AAG routes.
    pub aag_only: bool,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Narrowing {
    ToolRemoved { tool: ToolName },
    BudgetCapped { field: &'static str },
    ForbiddenPathAdded { path: PathPattern },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyIntersection {
    /// The narrowed, normalized spec that may be rendered.
    pub spec: TaskSpec,
    pub narrowings: Vec<Narrowing>,
}

impl OperatorPolicy {
    pub fn parse(bytes: &[u8]) -> Result<Self, SpecViolation> {
        let policy: Self = serde_json::from_slice(bytes)
            .map_err(|_| SpecViolation::new("policy_unparseable", "$"))?;
        policy.validate()?;
        Ok(policy)
    }

    pub fn validate(&self) -> Result<(), SpecViolation> {
        if self.schema_version != OPERATOR_POLICY_SCHEMA_VERSION {
            return Err(SpecViolation::new(
                "policy_schema_unsupported",
                "schema_version",
            ));
        }
        for (name, count) in [
            ("writable_roots", self.writable_roots.len()),
            ("protected_paths", self.protected_paths.len()),
            ("verifier_suites", self.verifier_suites.len()),
        ] {
            if count > MAX_POLICY_ITEMS {
                return Err(SpecViolation::new("policy_list_too_long", name));
            }
        }
        if self.writable_roots.is_empty() {
            return Err(SpecViolation::new(
                "policy_no_writable_roots",
                "writable_roots",
            ));
        }
        for root in &self.writable_roots {
            if self
                .protected_paths
                .iter()
                .any(|protected| root.is_within(protected))
            {
                return Err(SpecViolation::new(
                    "policy_root_protected",
                    format!("writable_roots:{root}"),
                ));
            }
        }
        if self.max_parallel_slots == 0
            || self.max_supervisor_decisions == 0
            || self.budget_caps.max_turns == 0
            || self.budget_caps.max_model_requests == 0
            || self.budget_caps.max_elapsed_seconds == 0
        {
            return Err(SpecViolation::new("policy_invalid_limits", "budget_caps"));
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<Sha256Digest, CanonicalError> {
        canonical_digest(POLICY_DIGEST_DOMAIN, self)
    }

    #[must_use]
    pub fn declares_suite(&self, suite: &SpecIdentifier) -> bool {
        self.verifier_suites
            .iter()
            .any(|declared| &declared.suite_id == suite)
    }
}

/// Narrows a structurally valid spec to the policy, or rejects it.
pub fn intersect(
    spec: TaskSpec,
    policy: &OperatorPolicy,
) -> Result<PolicyIntersection, Vec<SpecViolation>> {
    let mut violations = Vec::new();
    if spec.policy_version != policy.policy_version {
        violations.push(SpecViolation::new(
            "policy_version_mismatch",
            "policy_version",
        ));
    }
    for owned in &spec.owned_paths {
        if !policy
            .writable_roots
            .iter()
            .any(|root| owned.is_within(root))
        {
            violations.push(SpecViolation::new(
                "owned_path_outside_policy",
                format!("owned_paths:{owned}"),
            ));
        }
        if policy
            .protected_paths
            .iter()
            .any(|protected| owned.overlaps(protected))
        {
            violations.push(SpecViolation::new(
                "owned_path_protected",
                format!("owned_paths:{owned}"),
            ));
        }
    }
    for check in &spec.acceptance_checks {
        if check.kind == CheckKind::ExistingVerifierSuite
            && check
                .verifier_suite
                .as_ref()
                .is_none_or(|suite| !policy.declares_suite(suite))
        {
            violations.push(SpecViolation::new(
                "unknown_verifier_suite",
                format!("acceptance_checks:{}", check.check_id),
            ));
        }
    }
    if !violations.is_empty() {
        violations.sort();
        violations.dedup();
        return Err(violations);
    }

    let mut narrowings = Vec::new();
    let mut spec = spec;
    spec.permitted_tools.retain(|tool| {
        let allowed = policy.allowed_tools.contains(tool);
        if !allowed {
            narrowings.push(Narrowing::ToolRemoved { tool: *tool });
        }
        allowed
    });
    let caps = policy.budget_caps;
    let budget = &mut spec.resource_budget;
    for (field, value, cap) in [
        ("max_turns", &mut budget.max_turns, caps.max_turns),
        ("max_repairs", &mut budget.max_repairs, caps.max_repairs),
        (
            "max_model_requests",
            &mut budget.max_model_requests,
            caps.max_model_requests,
        ),
    ] {
        if *value > cap {
            *value = cap;
            narrowings.push(Narrowing::BudgetCapped { field });
        }
    }
    if budget.max_elapsed_seconds > caps.max_elapsed_seconds {
        budget.max_elapsed_seconds = caps.max_elapsed_seconds;
        narrowings.push(Narrowing::BudgetCapped {
            field: "max_elapsed_seconds",
        });
    }
    for protected in &policy.protected_paths {
        if !spec.forbidden_paths.contains(protected) {
            spec.forbidden_paths.push(protected.clone());
            narrowings.push(Narrowing::ForbiddenPathAdded {
                path: protected.clone(),
            });
        }
    }
    narrowings.sort();
    Ok(PolicyIntersection {
        spec: spec.normalized(),
        narrowings,
    })
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use crate::task_spec::fixtures::{id, path};

    pub fn taskboard_policy() -> OperatorPolicy {
        OperatorPolicy {
            schema_version: OPERATOR_POLICY_SCHEMA_VERSION,
            policy_version: id("policy_v1"),
            writable_roots: vec![
                path("src/**"),
                path("public/**"),
                path("tests/worker_a/**"),
                path("tests/worker_b/**"),
                path("tests/worker_store/**"),
            ],
            protected_paths: vec![
                path("package.json"),
                path("CONTRACT.md"),
                path("verifier/**"),
            ],
            allowed_tools: vec![
                ToolName::ReadFiles,
                ToolName::SearchFiles,
                ToolName::EditFiles,
                ToolName::WriteFiles,
                ToolName::RunDevTests,
            ],
            verifier_suites: vec![
                VerifierSuiteDecl {
                    suite_id: id("api"),
                    description: "Trusted API suite".into(),
                },
                VerifierSuiteDecl {
                    suite_id: id("store"),
                    description: "Trusted store suite".into(),
                },
                VerifierSuiteDecl {
                    suite_id: id("browser"),
                    description: "Real browser suite".into(),
                },
            ],
            budget_caps: ResourceBudget {
                max_turns: 6,
                max_repairs: 2,
                max_elapsed_seconds: 1800,
                max_model_requests: 80,
            },
            max_parallel_slots: 2,
            max_supervisor_decisions: 32,
            max_optimizer_candidates: 3,
            content_store_opt_in: true,
            aag_only: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{fixtures::taskboard_policy, *};
    use crate::task_spec::fixtures::{id, path, track_a_spec};

    #[test]
    fn a_faithful_spec_only_gains_protected_paths() {
        let result =
            intersect(track_a_spec().normalized(), &taskboard_policy()).expect("intersect");
        assert_eq!(
            result.narrowings,
            vec![
                Narrowing::ForbiddenPathAdded {
                    path: path("CONTRACT.md")
                },
                Narrowing::ForbiddenPathAdded {
                    path: path("verifier/**")
                },
            ]
        );
        assert!(result.spec.forbidden_paths.contains(&path("verifier/**")));
    }

    #[test]
    fn requested_grants_beyond_policy_are_narrowed_visibly() {
        let mut policy = taskboard_policy();
        policy.allowed_tools = vec![ToolName::ReadFiles, ToolName::EditFiles];
        let mut spec = track_a_spec();
        spec.resource_budget.max_model_requests = 10_000;
        let result = intersect(spec.normalized(), &policy).expect("intersect");
        assert_eq!(
            result.spec.permitted_tools,
            vec![ToolName::ReadFiles, ToolName::EditFiles]
        );
        assert_eq!(result.spec.resource_budget.max_model_requests, 80);
        assert!(result.narrowings.contains(&Narrowing::ToolRemoved {
            tool: ToolName::RunDevTests
        }));
        assert!(result.narrowings.contains(&Narrowing::BudgetCapped {
            field: "max_model_requests"
        }));
    }

    #[test]
    fn write_scope_outside_policy_or_protected_is_rejected() {
        let mut spec = track_a_spec();
        spec.owned_paths.push(path("verifier/api_suite.test.mjs"));
        spec.owned_paths.push(path("scripts/deploy.ps1"));
        spec.forbidden_paths.clear();
        let codes: Vec<&str> = intersect(spec.normalized(), &taskboard_policy())
            .expect_err("rejected")
            .into_iter()
            .map(|violation| violation.code)
            .collect();
        assert!(codes.contains(&"owned_path_outside_policy"));
        assert!(codes.contains(&"owned_path_protected"));
    }

    #[test]
    fn undeclared_verifier_suites_and_policy_versions_are_rejected() {
        let mut spec = track_a_spec();
        spec.acceptance_checks[0].verifier_suite = Some(id("self_reported"));
        spec.policy_version = id("policy_v9");
        let codes: Vec<&str> = intersect(spec.normalized(), &taskboard_policy())
            .expect_err("rejected")
            .into_iter()
            .map(|violation| violation.code)
            .collect();
        assert_eq!(
            codes,
            vec!["policy_version_mismatch", "unknown_verifier_suite"]
        );
    }

    #[test]
    fn policies_validate_and_digest_deterministically() {
        let policy = taskboard_policy();
        policy.validate().expect("valid policy");
        assert_eq!(
            policy.digest().expect("digest"),
            taskboard_policy().digest().expect("digest")
        );
        let bytes = serde_json::to_vec(&policy).expect("bytes");
        assert_eq!(OperatorPolicy::parse(&bytes).expect("parse"), policy);
        let mut protected_root = taskboard_policy();
        protected_root.protected_paths.push(path("src/**"));
        assert_eq!(
            protected_root.validate().expect_err("protected root").code,
            "policy_root_protected"
        );
    }
}
