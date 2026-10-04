//! Compiles an operator request into an admissible workflow (ADR 031 §3).
//!
//! Every compiled TaskSpec candidate passes, in order: strict parsing,
//! normalization, structural validation, source-coverage validation against
//! the exact request bytes (and any shared contract text read from the base
//! checkout), policy intersection, and cross-task coverage. Then the plan
//! is checked against the pinned workflow config (slots, suites, protocol
//! profiles, base commit) and each contract is rendered once, so its
//! identity pins the TaskSpec, policy, template, harness profile, prompt
//! capabilities, and base commit. Nothing here performs I/O except reading
//! shared contract files from the project checkout.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vibemux_harness::dispatch::{NativeProtocol, writable_profile::writable_profile_id};
use vibemux_workflow::{
    Sha256Digest, SpecIdentifier,
    canonical_json::canonical_digest,
    compiler_validation::{SharedContractText, validate_coverage, validate_workflow_coverage},
    contract::{ContractArtifact, ContractIdentityInputs, RouteTransformation},
    policy::{OperatorPolicy, intersect},
    renderer::{PromptCapabilities, WorkerRenderInputs, render_worker_contract},
    source_request::SourceRequest,
    task_spec::{CheckKind, TaskSpec, WorkflowMode},
    templates::ResolvedTemplate,
    workflow_record::{
        ContentStoreMode, MAX_WORKFLOW_TASKS, TaskProgress, WORKFLOW_RECORD_SCHEMA_VERSION,
        WorkflowRecord, WorkflowTask,
    },
};
use vibemux_workflow::{gates::WorkflowPhase, supervisor::LoopBudget};

use super::{error::WorkflowError, request::WorkflowRequest, settings::LoadedWorkflowConfig};

pub const WORKFLOW_PLAN_SCHEMA_VERSION: u32 = 1;
const PLAN_DIGEST_DOMAIN: &str = "vibemux.workflow.plan.v1";
const START_BINDING_DOMAIN: &str = "vibemux.workflow.start_binding.v1";
const MAX_SHARED_CONTRACT_BYTES: u64 = 64 * 1024;

/// Template version to optimizable block to replacement wording.
pub type TemplateOverrides = BTreeMap<String, BTreeMap<String, String>>;

/// What [`compile`] needs besides the request and policy.
pub(crate) struct PrepareContext<'a> {
    pub config: &'a LoadedWorkflowConfig,
    pub canonical_root: &'a Path,
    pub head_commit: &'a str,
    /// The execution protocol of each slot whose harness route may execute.
    pub slot_protocols: &'a BTreeMap<SpecIdentifier, NativeProtocol>,
    /// Wording of the active promoted prompt policy.
    pub template_overrides: &'a TemplateOverrides,
    pub now_ms: u64,
}

/// The daemon-private plan of one workflow. Every later turn re-renders
/// from these narrowed specs, so they are persisted next to the workflow;
/// they are never emitted by status or export.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowPlan {
    pub schema_version: u32,
    pub request_sha256: Sha256Digest,
    pub policy: OperatorPolicy,
    pub specs: BTreeMap<SpecIdentifier, TaskSpec>,
    pub assignments: BTreeMap<SpecIdentifier, Vec<SpecIdentifier>>,
    pub reviewer_slot: SpecIdentifier,
    pub template_overrides: TemplateOverrides,
    #[serde(default)]
    pub negative_control: bool,
}

impl WorkflowPlan {
    pub fn digest(&self) -> Result<Sha256Digest, WorkflowError> {
        canonical_digest(PLAN_DIGEST_DOMAIN, self).map_err(|_| WorkflowError::Internal)
    }

    /// The overrides of one template version.
    #[must_use]
    pub fn overrides_for(&self, template_version: &str) -> BTreeMap<String, String> {
        self.template_overrides
            .get(template_version)
            .cloned()
            .unwrap_or_default()
    }
}

pub(crate) struct PreparedWorkflow {
    pub record: WorkflowRecord,
    pub contracts: Vec<ContractArtifact>,
    pub plan: WorkflowPlan,
    pub narrowing_count: usize,
}

/// The digest `workflow start` must present: the admission fingerprint
/// bound to the plan, so neither the contracts nor the slot plan can
/// change between prepare and start.
pub fn start_binding(
    record: &WorkflowRecord,
    plan: &WorkflowPlan,
) -> Result<Sha256Digest, WorkflowError> {
    let fingerprint = record
        .admission_fingerprint()
        .map_err(|_| WorkflowError::Internal)?;
    let plan_digest = plan.digest()?;
    Ok(Sha256Digest::of_fields(
        START_BINDING_DOMAIN,
        &[fingerprint.as_bytes(), plan_digest.as_bytes()],
    ))
}

/// Validates the request and builds the record, contracts, and plan.
pub(crate) fn compile(
    request: &WorkflowRequest,
    policy: &OperatorPolicy,
    workflow_id: Uuid,
    context: &PrepareContext<'_>,
) -> Result<PreparedWorkflow, WorkflowError> {
    if request.task_specs.is_empty() {
        // A compiler route would fill the candidates; none is wired for
        // the deterministic coordinator, so an uncompiled request stops.
        return Err(WorkflowError::CompilerUnavailable);
    }
    if request.task_specs.len() > MAX_WORKFLOW_TASKS {
        return Err(WorkflowError::RequestInvalid);
    }
    policy
        .validate()
        .map_err(|_| WorkflowError::PolicyInvalid)?;
    let policy_digest = policy.digest().map_err(|_| WorkflowError::PolicyInvalid)?;
    let source = SourceRequest::new(request.request_text.as_bytes().to_vec())
        .map_err(|_| WorkflowError::RequestInvalid)?;
    let shared = read_shared_contracts(request, context.canonical_root)?;
    let mut codes = Vec::new();
    let mut specs: BTreeMap<SpecIdentifier, TaskSpec> = BTreeMap::new();
    let mut narrowing_count = 0;
    for value in &request.task_specs {
        match admit_candidate(value, &source, &shared, policy) {
            Ok((spec, narrowings)) => {
                narrowing_count += narrowings;
                if specs.insert(spec.task_key.clone(), spec).is_some() {
                    codes.push("workflow_duplicate_task_key".to_string());
                }
            }
            Err(rejected) => codes.extend(rejected),
        }
    }
    if codes.is_empty() {
        let refs: Vec<&TaskSpec> = specs.values().collect();
        if let Err(violations) = validate_workflow_coverage(&refs, &source) {
            codes.extend(
                violations
                    .iter()
                    .map(|violation| violation.code.to_string()),
            );
        }
    }
    if !codes.is_empty() {
        return Err(rejected(codes));
    }
    check_plan(request, policy, &specs, context)?;
    let mut contracts = Vec::new();
    let mut tasks = Vec::new();
    let mut deadline_seconds = 0_u64;
    for (task_key, spec) in &specs {
        let slot = request
            .assignments
            .get(task_key)
            .and_then(|slots| slots.first())
            .ok_or(WorkflowError::RequestInvalid)?;
        let protocol = context
            .slot_protocols
            .get(slot)
            .copied()
            .ok_or(WorkflowError::SlotUnavailable)?;
        let contract = render_contract(
            spec,
            policy_digest,
            protocol,
            context.template_overrides,
            context.head_commit,
        )?;
        tasks.push(WorkflowTask {
            task_key: task_key.clone(),
            contract_id: contract.contract_id,
            contract_version: spec.contract_version,
            required_suites: required_suites(spec),
            progress: TaskProgress::Pending,
            turns_used: 0,
            repairs_used: 0,
            max_turns: spec.resource_budget.max_turns,
            max_repairs: spec.resource_budget.max_repairs,
            accepted_candidate: None,
            failure_code: None,
        });
        deadline_seconds = deadline_seconds.max(spec.resource_budget.max_elapsed_seconds);
        contracts.push(contract);
    }
    let content_store = match (
        policy.content_store_opt_in,
        context.config.config.content_retention_days,
    ) {
        (true, Some(retention_days)) => ContentStoreMode::Enabled { retention_days },
        _ => ContentStoreMode::Disabled,
    };
    let record = WorkflowRecord {
        schema_version: WORKFLOW_RECORD_SCHEMA_VERSION,
        workflow_id,
        request_key: request.request_key.clone(),
        workflow_key: request.workflow_key.clone(),
        mode: request.mode,
        phase: WorkflowPhase::Prepared,
        version: 1,
        evidence_class: context.config.config.evidence_class,
        base_commit: context.head_commit.to_string(),
        policy_digest,
        verifier_digest: context.config.verifier_digest,
        integration_suites: request.integration_suites.clone(),
        tasks,
        budget: LoopBudget {
            decisions_remaining: policy.max_supervisor_decisions,
            malformed_repairs_remaining: policy.budget_caps.max_repairs,
            model_requests_remaining: policy.budget_caps.max_model_requests,
            deadline_unix_ms: context
                .now_ms
                .saturating_add(deadline_seconds.saturating_mul(1000)),
        },
        content_store,
        blocked_reason: None,
        created_at_ms: context.now_ms,
        updated_at_ms: context.now_ms,
    };
    record
        .validate()
        .map_err(|_| WorkflowError::RequestInvalid)?;
    let plan = WorkflowPlan {
        schema_version: WORKFLOW_PLAN_SCHEMA_VERSION,
        request_sha256: source.digest(),
        policy: policy.clone(),
        specs,
        assignments: request.assignments.clone(),
        reviewer_slot: request.reviewer_slot.clone(),
        template_overrides: context.template_overrides.clone(),
        negative_control: request.negative_control,
    };
    Ok(PreparedWorkflow {
        record,
        contracts,
        plan,
        narrowing_count,
    })
}

/// The trusted suites a task's acceptance checks name.
#[must_use]
pub fn required_suites(spec: &TaskSpec) -> Vec<SpecIdentifier> {
    let mut suites: Vec<SpecIdentifier> = spec
        .acceptance_checks
        .iter()
        .filter(|check| check.kind == CheckKind::ExistingVerifierSuite)
        .filter_map(|check| check.verifier_suite.clone())
        .collect();
    suites.sort();
    suites.dedup();
    suites
}

/// Renders the contract text (no turn section) and derives its identity.
pub(crate) fn render_contract(
    spec: &TaskSpec,
    policy_digest: Sha256Digest,
    protocol: NativeProtocol,
    overrides: &TemplateOverrides,
    base_commit: &str,
) -> Result<ContractArtifact, WorkflowError> {
    let task_spec_digest = spec.digest().map_err(|_| WorkflowError::Internal)?;
    let template = ResolvedTemplate::resolve(
        &spec.template_version,
        &overrides
            .get(spec.template_version.as_str())
            .cloned()
            .unwrap_or_default(),
    )
    .map_err(|error| rejected(vec![error.code().to_string()]))?;
    let rendered = render_worker_contract(WorkerRenderInputs {
        spec,
        task_spec_digest,
        policy_digest,
        template: &template,
        capabilities: PromptCapabilities::UNCERTIFIED,
        context_blocks: &[],
        turn: None,
    })
    .map_err(|error| rejected(vec![error.code().to_string()]))?;
    let harness_profile =
        writable_profile_id(protocol).map_err(|_| WorkflowError::SlotUnavailable)?;
    let identity = ContractIdentityInputs {
        task_spec_digest,
        policy_digest,
        template_digest: template.digest,
        harness_profile,
        prompt_capabilities: PromptCapabilities::UNCERTIFIED,
        route_transformation: &RouteTransformation::None,
        context_bundle_digests: &[],
        base_commit,
    };
    Ok(ContractArtifact::new(spec, &identity, &rendered, None))
}

fn rejected(mut codes: Vec<String>) -> WorkflowError {
    codes.sort();
    codes.dedup();
    WorkflowError::ContractRejected { codes }
}

/// One candidate through parsing, structure, coverage, and the policy.
fn admit_candidate(
    value: &serde_json::Value,
    source: &SourceRequest,
    shared: &BTreeMap<SpecIdentifier, String>,
    policy: &OperatorPolicy,
) -> Result<(TaskSpec, usize), Vec<String>> {
    let bytes = serde_json::to_vec(value).map_err(|_| vec!["task_spec_unparseable".to_string()])?;
    let spec = TaskSpec::parse_candidate(&bytes)
        .map_err(|violation| vec![violation.code.to_string()])?
        .normalized();
    spec.validate_structure().map_err(|violations| {
        violations
            .iter()
            .map(|violation| violation.code.to_string())
            .collect::<Vec<_>>()
    })?;
    let mut missing = Vec::new();
    let texts: Vec<SharedContractText<'_>> = spec
        .shared_contract_refs
        .iter()
        .filter_map(|reference| match shared.get(&reference.artifact_id) {
            Some(text) => Some(SharedContractText { reference, text }),
            None => {
                missing.push("workflow_shared_contract_missing".to_string());
                None
            }
        })
        .collect();
    if !missing.is_empty() {
        return Err(missing);
    }
    validate_coverage(&spec, source, &texts).map_err(|violations| {
        violations
            .iter()
            .map(|violation| violation.code.to_string())
            .collect::<Vec<_>>()
    })?;
    let intersection = intersect(spec, policy).map_err(|violations| {
        violations
            .iter()
            .map(|violation| violation.code.to_string())
            .collect::<Vec<_>>()
    })?;
    Ok((intersection.spec, intersection.narrowings.len()))
}

/// The plan against the pinned config: slots, suites, profiles, commit.
fn check_plan(
    request: &WorkflowRequest,
    policy: &OperatorPolicy,
    specs: &BTreeMap<SpecIdentifier, TaskSpec>,
    context: &PrepareContext<'_>,
) -> Result<(), WorkflowError> {
    let mut codes = Vec::new();
    let planned: Vec<&SpecIdentifier> = request.assignments.keys().collect();
    let compiled: Vec<&SpecIdentifier> = specs.keys().collect();
    if planned != compiled {
        codes.push("workflow_plan_task_mismatch");
    }
    for spec in specs.values() {
        if spec.workflow_key != request.workflow_key || spec.workflow_mode != request.mode {
            codes.push("workflow_spec_mode_mismatch");
        }
        if spec.base_commit != context.head_commit {
            codes.push("workflow_base_commit_not_head");
        }
        let suites = required_suites(spec);
        if suites.is_empty() {
            // Without a trusted suite, the gate would accept untested code.
            codes.push("workflow_task_without_verifier_suite");
        }
        if suites
            .iter()
            .any(|suite| !suite_available(suite, policy, context))
        {
            codes.push("workflow_suite_unavailable");
        }
    }
    if request.integration_suites.is_empty()
        || request
            .integration_suites
            .iter()
            .any(|suite| !suite_available(suite, policy, context))
    {
        codes.push("workflow_suite_unavailable");
    }
    let reviewer = context.config.slot(&request.reviewer_slot);
    if !reviewer.is_some_and(|slot| slot.is_reviewer())
        || !context.slot_protocols.contains_key(&request.reviewer_slot)
    {
        return Err(WorkflowError::SlotUnavailable);
    }
    for slots in request.assignments.values() {
        let mut profiles = Vec::new();
        for slot_id in slots {
            let configured = context.config.slot(slot_id);
            let protocol = context.slot_protocols.get(slot_id);
            match (configured, protocol) {
                (Some(slot), Some(protocol)) if !slot.is_reviewer() => {
                    profiles.push(
                        writable_profile_id(*protocol)
                            .map_err(|_| WorkflowError::SlotUnavailable)?,
                    );
                }
                _ => return Err(WorkflowError::SlotUnavailable),
            }
        }
        profiles.dedup();
        if request.mode == WorkflowMode::Compare && profiles.len() != 1 {
            codes.push("workflow_compare_profile_mismatch");
        }
    }
    if codes.is_empty() {
        Ok(())
    } else {
        Err(rejected(codes.into_iter().map(str::to_string).collect()))
    }
}

fn suite_available(
    suite: &SpecIdentifier,
    policy: &OperatorPolicy,
    context: &PrepareContext<'_>,
) -> bool {
    policy.declares_suite(suite) && context.config.config.verifier.suites.contains_key(suite)
}

/// Shared contract text from the project checkout: a relative, plain path
/// inside the root, read with a size bound. Its digest is checked against
/// the TaskSpec reference by coverage validation.
fn read_shared_contracts(
    request: &WorkflowRequest,
    canonical_root: &Path,
) -> Result<BTreeMap<SpecIdentifier, String>, WorkflowError> {
    let mut texts = BTreeMap::new();
    for (artifact_id, relative) in &request.shared_contract_paths {
        if !vibemux_workflow::identifiers::is_relative_path(relative) {
            return Err(WorkflowError::RequestInvalid);
        }
        let path: PathBuf = relative
            .split('/')
            .fold(canonical_root.to_path_buf(), |path, segment| {
                path.join(segment)
            });
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|_| WorkflowError::RequestInvalid)?;
        if !metadata.is_file() || metadata.len() > MAX_SHARED_CONTRACT_BYTES {
            return Err(WorkflowError::RequestInvalid);
        }
        let canonical = std::fs::canonicalize(&path).map_err(|_| WorkflowError::RequestInvalid)?;
        if !canonical.starts_with(canonical_root) {
            return Err(WorkflowError::RequestInvalid);
        }
        let text =
            std::fs::read_to_string(&canonical).map_err(|_| WorkflowError::RequestInvalid)?;
        texts.insert(artifact_id.clone(), text);
    }
    Ok(texts)
}
