//! The versioned TaskSpec: the typed intermediate representation between an
//! operator's request and the English worker contract (ADR 031 §2).
//!
//! A TaskSpec candidate may come from a model; nothing in it is trusted until
//! [`TaskSpec::validate_structure`], [`crate::compiler_validation`], and
//! [`crate::policy`] accept it. Set-like fields are normalized (sorted and
//! deduplicated) so that semantically identical candidates have identical
//! canonical bytes; requirement order follows requirement IDs.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{
    PathPattern, Sha256Digest, SpecIdentifier,
    canonical_json::{CanonicalError, canonical_digest},
    gates::AcceptanceLevel,
    messages::MessageKind,
    slots::SlotCapability,
};

pub const TASK_SPEC_SCHEMA_VERSION: u32 = 1;
pub const TASK_SPEC_DIGEST_DOMAIN: &str = "vibemux.workflow.task_spec.v1";
pub const MAX_REQUIREMENTS: usize = 64;
pub const MAX_LIST_ITEMS: usize = 64;
pub const MAX_LITERALS_PER_REQUIREMENT: usize = 16;
pub const MAX_STATEMENT_BYTES: usize = 2048;
pub const MAX_OBJECTIVE_BYTES: usize = 4096;
pub const MAX_LITERAL_BYTES: usize = 4096;
pub const MAX_SHORT_TEXT_BYTES: usize = 1024;
pub const MAX_CONTEXT_BUNDLE_BYTES: u32 = 16 * 1024;
pub const MAX_MESSAGES_PER_TURN: u32 = 8;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceLanguage {
    Zh,
    En,
    Mixed,
    Other,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NormativeForce {
    Must,
    MustNot,
    Should,
    May,
}

impl NormativeForce {
    #[must_use]
    pub const fn as_upper(self) -> &'static str {
        match self {
            Self::Must => "MUST",
            Self::MustNot => "MUST NOT",
            Self::Should => "SHOULD",
            Self::May => "MAY",
        }
    }

    /// `must` and `must_not` bind the worker; the others do not.
    #[must_use]
    pub const fn is_binding(self) -> bool {
        matches!(self, Self::Must | Self::MustNot)
    }
}

/// A byte range of the original request and the exact text it covers.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSpan {
    pub start: u32,
    pub end: u32,
    pub quoted: String,
}

/// An exact excerpt of a shared contract artifact (`shared_contract_refs`)
/// that a requirement relies on, for example a frozen status code.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContractExcerpt {
    pub artifact_id: SpecIdentifier,
    pub quoted: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Requirement {
    pub requirement_id: SpecIdentifier,
    /// English instruction text. Literal data belongs in `literals`.
    pub statement: String,
    pub force: NormativeForce,
    /// Spans of the original request. At least one is required.
    pub source_refs: Vec<SourceSpan>,
    /// Supporting excerpts of the frozen shared contract.
    pub contract_excerpts: Vec<ContractExcerpt>,
    /// Values that must reach the worker byte for byte (identifiers, paths,
    /// numbers with units, quoted examples, Unicode product data).
    pub literals: Vec<String>,
    pub acceptance_check_ids: Vec<SpecIdentifier>,
}

/// A labeled inference. Never normative and never rendered as a requirement.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Assumption {
    pub assumption_id: SpecIdentifier,
    pub statement: String,
    pub rationale: String,
}

/// A part of the request the compiler deliberately leaves uncovered, with a
/// visible reason. Spans containing prohibitions cannot be excluded.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceExclusion {
    pub span: SourceSpan,
    pub reason: ExclusionReason,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExclusionReason {
    BackgroundContext,
    ExampleData,
    AddressedToOtherTask,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    /// A trusted verifier suite declared by the operator policy.
    ExistingVerifierSuite,
    /// A test the worker is asked to write; never acceptance evidence.
    ProposedTest,
    /// A criterion for the independent reviewer.
    ReviewCriterion,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptanceCheck {
    pub check_id: SpecIdentifier,
    pub kind: CheckKind,
    pub description: String,
    /// Required for, and only allowed on, `existing_verifier_suite`.
    pub verifier_suite: Option<SpecIdentifier>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaType {
    TextMarkdown,
    TextPlain,
    ApplicationJson,
    Javascript,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRef {
    pub artifact_id: SpecIdentifier,
    pub sha256: Sha256Digest,
    pub byte_count: u64,
    pub media_type: MediaType,
}

/// Abstract tool grants; the harness adapter maps them to vendor flags.
/// There is deliberately no unrestricted shell grant.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolName {
    ReadFiles,
    SearchFiles,
    EditFiles,
    WriteFiles,
    RunDevTests,
}

impl ToolName {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadFiles => "read_files",
            Self::SearchFiles => "search_files",
            Self::EditFiles => "edit_files",
            Self::WriteFiles => "write_files",
            Self::RunDevTests => "run_dev_tests",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerRole {
    BackendDomain,
    Frontend,
    StoreComponent,
    Reviewer,
    Integrator,
    Generic,
}

impl WorkerRole {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BackendDomain => "backend_domain",
            Self::Frontend => "frontend",
            Self::StoreComponent => "store_component",
            Self::Reviewer => "reviewer",
            Self::Integrator => "integrator",
            Self::Generic => "generic",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowMode {
    Cooperate,
    Compare,
    Review,
}

impl WorkflowMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cooperate => "cooperate",
            Self::Compare => "compare",
            Self::Review => "review",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextSourceKind {
    VerifierReceipts,
    CandidateSnapshot,
    WorkerClaims,
    PublicMessages,
    ContractText,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OutputContract {
    /// The worker ends its turn with a `vibemux_checkpoint` block.
    pub checkpoint_report_required: bool,
    /// Every changed path must be inside `owned_paths`.
    pub changes_within_owned_paths: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionDefinition {
    pub required_levels: Vec<AcceptanceLevel>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommunicationPolicy {
    /// Task keys this task may address through the broker.
    pub may_message: Vec<SpecIdentifier>,
    pub max_messages_per_turn: u32,
    /// Kinds the deterministic broker may route without the supervisor.
    pub auto_authorized_kinds: Vec<MessageKind>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContextPolicy {
    pub max_bundle_bytes: u32,
    pub retrieval_order: Vec<ContextSourceKind>,
    pub allow_public_excerpts: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceBudget {
    pub max_turns: u32,
    pub max_repairs: u32,
    pub max_elapsed_seconds: u64,
    pub max_model_requests: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSpec {
    pub schema_version: u32,
    pub workflow_key: SpecIdentifier,
    pub task_key: SpecIdentifier,
    pub contract_version: u32,
    /// SHA-256 of the original request bytes.
    pub original_request_ref: Sha256Digest,
    pub source_language: SourceLanguage,
    pub requirements: Vec<Requirement>,
    pub objective: String,
    pub non_goals: Vec<String>,
    pub assumptions: Vec<Assumption>,
    pub unresolved_questions: Vec<String>,
    pub source_exclusions: Vec<SourceExclusion>,
    pub base_commit: String,
    pub input_artifacts: Vec<ArtifactRef>,
    pub shared_contract_refs: Vec<ArtifactRef>,
    pub owned_paths: Vec<PathPattern>,
    pub read_scopes: Vec<PathPattern>,
    pub forbidden_paths: Vec<PathPattern>,
    pub dependencies: Vec<SpecIdentifier>,
    pub required_capabilities: Vec<SlotCapability>,
    pub permitted_tools: Vec<ToolName>,
    pub acceptance_checks: Vec<AcceptanceCheck>,
    pub output_contract: OutputContract,
    pub completion_definition: CompletionDefinition,
    pub communication_policy: CommunicationPolicy,
    pub context_policy: ContextPolicy,
    pub resource_budget: ResourceBudget,
    pub role: WorkerRole,
    pub workflow_mode: WorkflowMode,
    pub template_version: SpecIdentifier,
    pub policy_version: SpecIdentifier,
}

/// One structural defect, as a stable code and the offending field path.
/// Content-free: the field path never carries request text.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct SpecViolation {
    pub code: &'static str,
    pub field: String,
}

impl SpecViolation {
    #[must_use]
    pub fn new(code: &'static str, field: impl Into<String>) -> Self {
        Self {
            code,
            field: field.into(),
        }
    }
}

impl TaskSpec {
    /// Parses a candidate strictly: unknown and duplicate fields are errors.
    pub fn parse_candidate(bytes: &[u8]) -> Result<Self, SpecViolation> {
        serde_json::from_slice(bytes).map_err(|_| SpecViolation::new("task_spec_unparseable", "$"))
    }

    /// Sorted, deduplicated set-like fields and ID-ordered lists, so equal
    /// meaning has equal canonical bytes.
    #[must_use]
    pub fn normalized(mut self) -> Self {
        fn sort_dedup<T: Ord>(values: &mut Vec<T>) {
            values.sort();
            values.dedup();
        }
        self.requirements
            .sort_by(|left, right| left.requirement_id.cmp(&right.requirement_id));
        for requirement in &mut self.requirements {
            sort_dedup(&mut requirement.source_refs);
            sort_dedup(&mut requirement.contract_excerpts);
            sort_dedup(&mut requirement.acceptance_check_ids);
        }
        self.assumptions
            .sort_by(|left, right| left.assumption_id.cmp(&right.assumption_id));
        self.acceptance_checks
            .sort_by(|left, right| left.check_id.cmp(&right.check_id));
        self.source_exclusions
            .sort_by(|left, right| left.span.cmp(&right.span));
        sort_dedup(&mut self.input_artifacts);
        sort_dedup(&mut self.shared_contract_refs);
        sort_dedup(&mut self.owned_paths);
        sort_dedup(&mut self.read_scopes);
        sort_dedup(&mut self.forbidden_paths);
        sort_dedup(&mut self.dependencies);
        sort_dedup(&mut self.required_capabilities);
        sort_dedup(&mut self.permitted_tools);
        sort_dedup(&mut self.completion_definition.required_levels);
        sort_dedup(&mut self.communication_policy.may_message);
        sort_dedup(&mut self.communication_policy.auto_authorized_kinds);
        self
    }

    /// Digest of the canonical bytes of this (normalized) spec.
    pub fn digest(&self) -> Result<Sha256Digest, CanonicalError> {
        canonical_digest(TASK_SPEC_DIGEST_DOMAIN, self)
    }

    /// Structural checks that need no source text or policy. Returns every
    /// violation found, sorted.
    pub fn validate_structure(&self) -> Result<(), Vec<SpecViolation>> {
        let mut violations = Vec::new();
        let mut push = |code: &'static str, field: String| {
            violations.push(SpecViolation::new(code, field));
        };
        if self.schema_version != TASK_SPEC_SCHEMA_VERSION {
            push("task_spec_schema_unsupported", "schema_version".into());
        }
        if self.contract_version == 0 {
            push(
                "task_spec_invalid_contract_version",
                "contract_version".into(),
            );
        }
        if !is_full_commit(&self.base_commit) {
            push("task_spec_invalid_base_commit", "base_commit".into());
        }
        check_text(
            &mut push,
            "objective",
            &self.objective,
            MAX_OBJECTIVE_BYTES,
            false,
        );
        if self.requirements.is_empty() || self.requirements.len() > MAX_REQUIREMENTS {
            push("task_spec_requirement_count", "requirements".into());
        }
        let check_ids: BTreeSet<&SpecIdentifier> = self
            .acceptance_checks
            .iter()
            .map(|check| &check.check_id)
            .collect();
        if check_ids.len() != self.acceptance_checks.len() {
            push("task_spec_duplicate_check_id", "acceptance_checks".into());
        }
        let mut requirement_ids = BTreeSet::new();
        for (index, requirement) in self.requirements.iter().enumerate() {
            let field = format!("requirements[{index}]");
            if !requirement_ids.insert(&requirement.requirement_id) {
                push("task_spec_duplicate_requirement_id", field.clone());
            }
            check_text(
                &mut push,
                &format!("{field}.statement"),
                &requirement.statement,
                MAX_STATEMENT_BYTES,
                false,
            );
            if requirement.literals.len() > MAX_LITERALS_PER_REQUIREMENT {
                push("task_spec_too_many_literals", format!("{field}.literals"));
            }
            for (literal_index, literal) in requirement.literals.iter().enumerate() {
                if literal.is_empty() || literal.len() > MAX_LITERAL_BYTES || literal.contains('\0')
                {
                    push(
                        "task_spec_invalid_literal",
                        format!("{field}.literals[{literal_index}]"),
                    );
                }
            }
            if requirement.acceptance_check_ids.is_empty() {
                push("task_spec_requirement_without_check", field.clone());
            }
            for check_id in &requirement.acceptance_check_ids {
                if !check_ids.contains(check_id) {
                    push("task_spec_unknown_check_reference", field.clone());
                }
            }
            for span in &requirement.source_refs {
                if span.end <= span.start || span.quoted.is_empty() {
                    push(
                        "task_spec_invalid_source_span",
                        format!("{field}.source_refs"),
                    );
                }
            }
        }
        for (index, check) in self.acceptance_checks.iter().enumerate() {
            let field = format!("acceptance_checks[{index}]");
            check_text(
                &mut push,
                &format!("{field}.description"),
                &check.description,
                MAX_SHORT_TEXT_BYTES,
                false,
            );
            let suite_required = check.kind == CheckKind::ExistingVerifierSuite;
            if suite_required != check.verifier_suite.is_some() {
                push("task_spec_check_suite_mismatch", field);
            }
        }
        let mut assumption_ids = BTreeSet::new();
        for (index, assumption) in self.assumptions.iter().enumerate() {
            let field = format!("assumptions[{index}]");
            if !assumption_ids.insert(&assumption.assumption_id) {
                push("task_spec_duplicate_assumption_id", field.clone());
            }
            check_text(
                &mut push,
                &format!("{field}.statement"),
                &assumption.statement,
                MAX_SHORT_TEXT_BYTES,
                false,
            );
            check_text(
                &mut push,
                &format!("{field}.rationale"),
                &assumption.rationale,
                MAX_SHORT_TEXT_BYTES,
                false,
            );
        }
        for (name, items) in [
            ("non_goals", &self.non_goals),
            ("unresolved_questions", &self.unresolved_questions),
        ] {
            if items.len() > MAX_LIST_ITEMS {
                push("task_spec_list_too_long", name.into());
            }
            for (index, item) in items.iter().enumerate() {
                check_text(
                    &mut push,
                    &format!("{name}[{index}]"),
                    item,
                    MAX_SHORT_TEXT_BYTES,
                    false,
                );
            }
        }
        for (name, count) in [
            ("owned_paths", self.owned_paths.len()),
            ("read_scopes", self.read_scopes.len()),
            ("forbidden_paths", self.forbidden_paths.len()),
            ("input_artifacts", self.input_artifacts.len()),
            ("shared_contract_refs", self.shared_contract_refs.len()),
            ("dependencies", self.dependencies.len()),
            ("acceptance_checks", self.acceptance_checks.len()),
            ("source_exclusions", self.source_exclusions.len()),
        ] {
            if count > MAX_LIST_ITEMS {
                push("task_spec_list_too_long", name.into());
            }
        }
        if self.role != WorkerRole::Reviewer && self.owned_paths.is_empty() {
            push("task_spec_no_owned_paths", "owned_paths".into());
        }
        if self.role == WorkerRole::Reviewer
            && (!self.owned_paths.is_empty()
                || self
                    .permitted_tools
                    .iter()
                    .any(|tool| matches!(tool, ToolName::EditFiles | ToolName::WriteFiles)))
        {
            push(
                "task_spec_reviewer_must_not_write",
                "permitted_tools".into(),
            );
        }
        for owned in &self.owned_paths {
            if self
                .forbidden_paths
                .iter()
                .any(|forbidden| owned.overlaps(forbidden))
            {
                push(
                    "task_spec_owned_path_forbidden",
                    format!("owned_paths:{owned}"),
                );
            }
        }
        if self.dependencies.contains(&self.task_key) {
            push("task_spec_self_dependency", "dependencies".into());
        }
        if !self.output_contract.checkpoint_report_required
            || !self.output_contract.changes_within_owned_paths
        {
            push(
                "task_spec_output_contract_weakened",
                "output_contract".into(),
            );
        }
        for required in [
            AcceptanceLevel::CandidateArtifactCollected,
            AcceptanceLevel::IndependentReviewPassed,
            AcceptanceLevel::LocalVerifierPassed,
        ] {
            if self.role != WorkerRole::Reviewer
                && !self
                    .completion_definition
                    .required_levels
                    .contains(&required)
            {
                push(
                    "task_spec_completion_too_weak",
                    "completion_definition".into(),
                );
            }
        }
        if self.communication_policy.max_messages_per_turn > MAX_MESSAGES_PER_TURN {
            push(
                "task_spec_message_budget_too_large",
                "communication_policy".into(),
            );
        }
        if self
            .communication_policy
            .may_message
            .contains(&self.task_key)
        {
            push("task_spec_self_message", "communication_policy".into());
        }
        if self.context_policy.max_bundle_bytes == 0
            || self.context_policy.max_bundle_bytes > MAX_CONTEXT_BUNDLE_BYTES
        {
            push("task_spec_invalid_bundle_budget", "context_policy".into());
        }
        let budget = self.resource_budget;
        if budget.max_turns == 0
            || budget.max_elapsed_seconds == 0
            || budget.max_model_requests == 0
        {
            push("task_spec_invalid_budget", "resource_budget".into());
        }
        if violations.is_empty() {
            Ok(())
        } else {
            violations.sort();
            violations.dedup();
            Err(violations)
        }
    }
}

/// Text fields are bounded, non-empty after trim, and contain no control
/// characters (a statement is one paragraph; literals carry multi-line data).
fn check_text(
    push: &mut impl FnMut(&'static str, String),
    field: &str,
    value: &str,
    max_bytes: usize,
    allow_newline: bool,
) {
    let has_control = value
        .chars()
        .any(|character| character.is_control() && !(allow_newline && character == '\n'));
    if value.trim().is_empty() || value.len() > max_bytes || has_control {
        push("task_spec_invalid_text", field.to_string());
    }
}

/// A full 40- or 64-character lowercase hexadecimal commit ID.
#[must_use]
pub fn is_full_commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
pub(crate) mod fixtures {
    //! A valid TaskBoard Track A spec shared by the module tests.

    use super::*;
    use crate::{gates::AcceptanceLevel, messages::MessageKind, slots::SlotCapability};

    pub const REQUEST: &str = "Build the TaskBoard backend. Titles must have at most 120 characters. Do not modify `package.json`. Use `src/store.mjs` for persistence.";

    pub fn id(value: &str) -> SpecIdentifier {
        SpecIdentifier::new(value).expect("fixture identifier")
    }

    pub fn path(value: &str) -> PathPattern {
        PathPattern::new(value).expect("fixture path")
    }

    pub fn span(request: &str, quoted: &str) -> SourceSpan {
        let start = request.find(quoted).expect("fixture span present");
        SourceSpan {
            start: u32::try_from(start).expect("span start"),
            end: u32::try_from(start + quoted.len()).expect("span end"),
            quoted: quoted.to_string(),
        }
    }

    pub fn track_a_spec() -> TaskSpec {
        TaskSpec {
            schema_version: TASK_SPEC_SCHEMA_VERSION,
            workflow_key: id("taskboard_lite"),
            task_key: id("track_a"),
            contract_version: 1,
            original_request_ref: Sha256Digest::of(REQUEST.as_bytes()),
            source_language: SourceLanguage::En,
            requirements: vec![
                Requirement {
                    requirement_id: id("r01"),
                    statement: "Implement the TaskBoard backend HTTP server.".into(),
                    force: NormativeForce::Must,
                    source_refs: vec![span(REQUEST, "Build the TaskBoard backend.")],
                    contract_excerpts: vec![],
                    literals: vec![],
                    acceptance_check_ids: vec![id("c01")],
                },
                Requirement {
                    requirement_id: id("r02"),
                    statement: "Reject titles longer than 120 characters.".into(),
                    force: NormativeForce::Must,
                    source_refs: vec![span(REQUEST, "Titles must have at most 120 characters.")],
                    contract_excerpts: vec![],
                    literals: vec![],
                    acceptance_check_ids: vec![id("c01")],
                },
                Requirement {
                    requirement_id: id("r03"),
                    statement: "Do not modify the package manifest.".into(),
                    force: NormativeForce::MustNot,
                    source_refs: vec![span(REQUEST, "Do not modify `package.json`.")],
                    contract_excerpts: vec![],
                    literals: vec!["package.json".into()],
                    acceptance_check_ids: vec![id("c02")],
                },
                Requirement {
                    requirement_id: id("r04"),
                    statement: "Persist tasks through the store module.".into(),
                    force: NormativeForce::Must,
                    source_refs: vec![span(REQUEST, "Use `src/store.mjs` for persistence.")],
                    contract_excerpts: vec![],
                    literals: vec!["src/store.mjs".into()],
                    acceptance_check_ids: vec![id("c01")],
                },
            ],
            objective: "Deliver the TaskBoard Lite backend per the frozen contract.".into(),
            non_goals: vec!["Authentication.".into()],
            assumptions: vec![Assumption {
                assumption_id: id("a01"),
                statement: "Characters means Unicode code points.".into(),
                rationale: "The frozen contract counts code points.".into(),
            }],
            unresolved_questions: vec![],
            source_exclusions: vec![],
            base_commit: "a".repeat(40),
            input_artifacts: vec![],
            shared_contract_refs: vec![],
            owned_paths: vec![
                path("src/server.mjs"),
                path("src/store.mjs"),
                path("tests/worker_a/**"),
            ],
            read_scopes: vec![path("CONTRACT.md")],
            forbidden_paths: vec![path("package.json")],
            dependencies: vec![],
            required_capabilities: vec![
                SlotCapability::StructuredTurn,
                SlotCapability::WritableWorktree,
            ],
            permitted_tools: vec![
                ToolName::ReadFiles,
                ToolName::EditFiles,
                ToolName::RunDevTests,
            ],
            acceptance_checks: vec![
                AcceptanceCheck {
                    check_id: id("c01"),
                    kind: CheckKind::ExistingVerifierSuite,
                    description: "Trusted API suite passes.".into(),
                    verifier_suite: Some(id("api")),
                },
                AcceptanceCheck {
                    check_id: id("c02"),
                    kind: CheckKind::ReviewCriterion,
                    description: "No change outside owned paths.".into(),
                    verifier_suite: None,
                },
            ],
            output_contract: OutputContract {
                checkpoint_report_required: true,
                changes_within_owned_paths: true,
            },
            completion_definition: CompletionDefinition {
                required_levels: vec![
                    AcceptanceLevel::CandidateArtifactCollected,
                    AcceptanceLevel::IndependentReviewPassed,
                    AcceptanceLevel::LocalVerifierPassed,
                ],
            },
            communication_policy: CommunicationPolicy {
                may_message: vec![id("track_b")],
                max_messages_per_turn: 2,
                auto_authorized_kinds: vec![MessageKind::Answer, MessageKind::Progress],
            },
            context_policy: ContextPolicy {
                max_bundle_bytes: 8192,
                retrieval_order: vec![
                    ContextSourceKind::VerifierReceipts,
                    ContextSourceKind::CandidateSnapshot,
                ],
                allow_public_excerpts: true,
            },
            resource_budget: ResourceBudget {
                max_turns: 4,
                max_repairs: 2,
                max_elapsed_seconds: 1800,
                max_model_requests: 40,
            },
            role: WorkerRole::BackendDomain,
            workflow_mode: WorkflowMode::Cooperate,
            template_version: id("worker_v1"),
            policy_version: id("policy_v1"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{fixtures::*, *};

    #[test]
    fn fixture_spec_is_structurally_valid() {
        track_a_spec().validate_structure().expect("valid fixture");
    }

    #[test]
    fn normalization_makes_reordered_sets_identical() {
        let first = track_a_spec().normalized();
        let mut reordered = track_a_spec();
        reordered.owned_paths.reverse();
        reordered.requirements.reverse();
        reordered.permitted_tools.reverse();
        reordered.owned_paths.push(path("src/server.mjs"));
        let second = reordered.normalized();
        assert_eq!(first, second);
        assert_eq!(
            first.digest().expect("digest"),
            second.digest().expect("digest")
        );
    }

    #[test]
    fn structural_defects_are_all_reported_with_stable_codes() {
        let mut spec = track_a_spec();
        spec.base_commit = "HEAD".into();
        spec.requirements[0].acceptance_check_ids.clear();
        spec.requirements[1].acceptance_check_ids = vec![id("c99")];
        spec.owned_paths.push(path("package.json"));
        spec.output_contract.checkpoint_report_required = false;
        spec.completion_definition.required_levels = vec![AcceptanceLevel::TurnCompleted];
        let codes: BTreeSet<&str> = spec
            .validate_structure()
            .expect_err("defects")
            .into_iter()
            .map(|violation| violation.code)
            .collect();
        for expected in [
            "task_spec_invalid_base_commit",
            "task_spec_requirement_without_check",
            "task_spec_unknown_check_reference",
            "task_spec_owned_path_forbidden",
            "task_spec_output_contract_weakened",
            "task_spec_completion_too_weak",
        ] {
            assert!(codes.contains(expected), "{expected}");
        }
    }

    #[test]
    fn reviewer_specs_cannot_carry_write_grants() {
        let mut spec = track_a_spec();
        spec.role = WorkerRole::Reviewer;
        let codes: Vec<&str> = spec
            .validate_structure()
            .expect_err("reviewer writes")
            .into_iter()
            .map(|violation| violation.code)
            .collect();
        assert!(codes.contains(&"task_spec_reviewer_must_not_write"));
    }

    #[test]
    fn candidates_with_unknown_fields_are_unparseable() {
        let mut value = serde_json::to_value(track_a_spec()).expect("value");
        value["grant_shell"] = serde_json::json!(true);
        let bytes = serde_json::to_vec(&value).expect("bytes");
        assert_eq!(
            TaskSpec::parse_candidate(&bytes)
                .expect_err("unknown field")
                .code,
            "task_spec_unparseable"
        );
        let valid = serde_json::to_vec(&track_a_spec()).expect("bytes");
        assert_eq!(
            TaskSpec::parse_candidate(&valid).expect("parse"),
            track_a_spec()
        );
    }
}
