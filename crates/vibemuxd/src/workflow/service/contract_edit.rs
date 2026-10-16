//! Explicit operator semantic edits create immutable successor workflows.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use vibemux_workflow::{canonical_json::canonical_digest, contract_edit::validate_semantic_edit};

use super::*;

const EDIT_SCHEMA_VERSION: u32 = 1;
const MAX_EDIT_BYTES: usize = 48 * 1024;
const EDIT_REQUEST_DOMAIN: &str = "vibemux.workflow.operator_contract_edit.v1";

#[derive(Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EditDisposition {
    RetireQuiescentParent,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContractEditRequest {
    pub schema_version: u32,
    pub edit_id: Uuid,
    pub parent_workflow_id: Uuid,
    pub expected_parent_version: u64,
    pub expected_contracts: std::collections::BTreeMap<SpecIdentifier, Sha256Digest>,
    pub disposition: EditDisposition,
    /// Exact original source plus fully supplied replacement TaskSpecs.
    pub request: Value,
}

#[derive(Serialize)]
pub struct ContractEditResponse {
    pub schema_version: u32,
    pub parent_workflow_id: Uuid,
    pub edit_id: Uuid,
    pub successor: PrepareResponse,
}

fn edit_error(code: &str) -> WorkflowError {
    WorkflowError::Dispatch { code: code.into() }
}

fn compile_successor(
    request: WorkflowRequest,
    parent: &WorkflowSnapshot,
    parent_plan: &WorkflowPlan,
    setup: &WorkflowSetup,
) -> Result<super::super::prepare::PreparedWorkflow, WorkflowError> {
    if request.task_specs.is_empty()
        || Sha256Digest::of(request.request_text.as_bytes()) != parent_plan.request_sha256
        || request.workflow_key != parent.record.workflow_key
        || request.mode != parent.record.mode
        || request.assignments != parent_plan.assignments
        || request.reviewer_slot != parent_plan.reviewer_slot
        || request.integration_suites != parent.record.integration_suites
        || request.negative_control != parent_plan.negative_control
    {
        return Err(edit_error("workflow_edit_immutable_authority"));
    }
    let head = read_head_commit(&setup.canonical_root).map_err(|_| WorkflowError::Git)?;
    if head != parent.record.base_commit {
        return Err(edit_error("workflow_edit_base_changed"));
    }
    let mut prepared = compile(
        &request,
        &parent_plan.policy,
        workflow_id_for(&request.request_key),
        &PrepareContext {
            config: &setup.config,
            canonical_root: &setup.canonical_root,
            head_commit: &head,
            slot_protocols: &setup.slot_protocols,
            slot_profiles: &setup.slot_profiles,
            template_overrides: &parent_plan.template_overrides,
            now_ms: now_ms(),
        },
    )?;
    for artifact in &mut prepared.contracts {
        let key = SpecIdentifier::new(&artifact.task_key).map_err(|_| WorkflowError::Internal)?;
        let old = parent_plan
            .specs
            .get(&key)
            .ok_or_else(|| edit_error("workflow_edit_immutable_authority"))?;
        let new = prepared
            .plan
            .specs
            .get(&key)
            .ok_or_else(|| edit_error("workflow_edit_immutable_authority"))?;
        let diff = validate_semantic_edit(old, new).map_err(|error| edit_error(error.code()))?;
        let old_id = parent
            .record
            .task(&key)
            .ok_or(WorkflowError::Internal)?
            .contract_id;
        let previous = parent
            .contracts
            .iter()
            .find(|contract| contract.contract_id == old_id)
            .ok_or(WorkflowError::Internal)?;
        if artifact.harness_profile != previous.harness_profile
            || artifact.route_transformation != previous.route_transformation
            || artifact.template_digest != previous.template_digest
            || artifact.template_version != previous.template_version
            || artifact.context_bundle_digests != previous.context_bundle_digests
        {
            return Err(edit_error("workflow_edit_profile_changed"));
        }
        if diff.is_empty() {
            if artifact.contract_id != previous.contract_id {
                return Err(edit_error("workflow_edit_profile_changed"));
            }
            artifact.clone_from(previous);
        } else {
            artifact.previous_contract_id = Some(previous.contract_id);
            artifact.semantic_diff = Some(diff);
        }
    }
    prepared.record.budget.decisions_remaining = prepared
        .record
        .budget
        .decisions_remaining
        .min(parent.record.budget.decisions_remaining);
    prepared.record.budget.malformed_repairs_remaining = prepared
        .record
        .budget
        .malformed_repairs_remaining
        .min(parent.record.budget.malformed_repairs_remaining);
    prepared.record.budget.model_requests_remaining = prepared
        .record
        .budget
        .model_requests_remaining
        .min(parent.record.budget.model_requests_remaining);
    let max_elapsed_seconds = prepared
        .plan
        .specs
        .values()
        .map(|spec| spec.resource_budget.max_elapsed_seconds)
        .max()
        .unwrap_or(0);
    let narrowed_deadline = parent
        .record
        .created_at_ms
        .saturating_add(max_elapsed_seconds.saturating_mul(1000));
    prepared.record.budget.deadline_unix_ms =
        narrowed_deadline.min(parent.record.budget.deadline_unix_ms);
    for task in &mut prepared.record.tasks {
        let previous = parent
            .record
            .task(&task.task_key)
            .ok_or(WorkflowError::Internal)?;
        task.turns_used = previous.turns_used;
        task.repairs_used = previous.repairs_used;
    }
    prepared
        .record
        .validate()
        .map_err(|_| edit_error("workflow_edit_budget_exhausted"))?;
    Ok(prepared)
}

impl WorkflowService {
    /// No live turn is interrupted implicitly. Stop and settle it before editing.
    pub async fn edit_contract(&self, value: Value) -> Result<ContractEditResponse, WorkflowError> {
        self.accepting()?;
        let bytes = serde_json::to_vec(&value).map_err(|_| WorkflowError::RequestInvalid)?;
        if bytes.len() > MAX_EDIT_BYTES {
            return Err(WorkflowError::RequestInvalid);
        }
        let edit: ContractEditRequest =
            serde_json::from_slice(&bytes).map_err(|_| WorkflowError::RequestInvalid)?;
        if edit.schema_version != EDIT_SCHEMA_VERSION
            || edit.edit_id.is_nil()
            || edit.parent_workflow_id.is_nil()
            || edit.expected_parent_version == 0
            || edit.expected_contracts.is_empty()
        {
            return Err(WorkflowError::RequestInvalid);
        }
        let request_digest = canonical_digest(EDIT_REQUEST_DOMAIN, &edit)
            .map_err(|_| WorkflowError::RequestInvalid)?;
        let _lifecycle = self
            .inner
            .lifecycle_gate
            .acquire()
            .await
            .map_err(|_| WorkflowError::ShuttingDown)?;
        self.accepting()?;
        if self
            .inner
            .runs()
            .active
            .contains_key(&edit.parent_workflow_id)
        {
            return Err(WorkflowError::AlreadyRunning);
        }
        let setup = self.inner.setup()?;
        let writer = self.inner.writer.clone();
        let parent_id = edit.parent_workflow_id;
        let parent = call_writer(move || writer.workflow_snapshot(parent_id))
            .await?
            .ok_or(WorkflowError::NotFound)?;
        let parent_contracts: std::collections::BTreeMap<_, _> = parent
            .record
            .tasks
            .iter()
            .map(|task| (task.task_key.clone(), task.contract_id))
            .collect();
        if parent_contracts != edit.expected_contracts {
            return Err(WorkflowError::ContractMismatch);
        }
        let state = setup.state.clone();
        let parent_plan = tokio::task::spawn_blocking(move || state.read_plan(parent_id))
            .await
            .map_err(|_| WorkflowError::Internal)??;
        let request = WorkflowRequest::parse(edit.request.clone())?;
        let compile_setup = setup.clone();
        let original = parent.clone();
        let old_plan = parent_plan.clone();
        let prepared = tokio::task::spawn_blocking(move || {
            compile_successor(request, &original, &old_plan, &compile_setup)
        })
        .await
        .map_err(|_| WorkflowError::Internal)??;
        let writer = self.inner.writer.clone();
        let admission = vibemux_store::ContractEditAdmission {
            edit_id: edit.edit_id,
            parent_workflow_id: parent_id,
            expected_parent_version: edit.expected_parent_version,
            request_digest,
            parent_plan: serde_json::to_value(&parent_plan).map_err(|_| WorkflowError::Internal)?,
            successor_plan: serde_json::to_value(&prepared.plan)
                .map_err(|_| WorkflowError::Internal)?,
            successor: prepared.record.clone(),
            contracts: prepared.contracts.clone(),
            timestamp: OffsetDateTime::now_utc(),
        };
        let commit = call_writer(move || writer.prepare_contract_edit(admission.clone())).await?;
        // A crash during binding leaves an admitted but non-executable successor.
        // An identical edit retry restores these same immutable bindings.
        for contract in &prepared.contracts {
            let key =
                SpecIdentifier::new(&contract.task_key).map_err(|_| WorkflowError::Internal)?;
            let spec = prepared
                .plan
                .specs
                .get(&key)
                .ok_or(WorkflowError::Internal)?
                .clone();
            let writer = self.inner.writer.clone();
            let contract_id = contract.contract_id;
            call_writer(move || {
                writer.bind_workflow_contract_authority(
                    contract_id,
                    spec.clone(),
                    OffsetDateTime::now_utc(),
                )
            })
            .await?;
        }
        let workflow_id = commit.record.workflow_id;
        let writer = self.inner.writer.clone();
        let plan_value =
            serde_json::to_value(&prepared.plan).map_err(|_| WorkflowError::Internal)?;
        let settings = commit.edit.context_settings;
        call_writer(move || {
            writer.bind_recovery_plan(
                workflow_id,
                plan_value.clone(),
                1,
                settings,
                OffsetDateTime::now_utc(),
            )
        })
        .await?;
        let state = setup.state.clone();
        let plan = prepared.plan.clone();
        tokio::task::spawn_blocking(move || state.write_plan(workflow_id, &plan))
            .await
            .map_err(|_| WorkflowError::Internal)??;
        Ok(ContractEditResponse {
            schema_version: EDIT_SCHEMA_VERSION,
            parent_workflow_id: parent_id,
            edit_id: edit.edit_id,
            successor: PrepareResponse {
                workflow_id,
                phase: commit.record.phase,
                start_contract: start_binding(&commit.record, &prepared.plan)?,
                contracts: prepared
                    .contracts
                    .iter()
                    .map(|contract| PreparedContract {
                        task_key: contract.task_key.clone(),
                        contract_id: contract.contract_id,
                        contract_version: contract.contract_version,
                        template_version: contract.template_version.clone(),
                        rendered_prompt_digest: contract.rendered_prompt_digest,
                        rendered_prompt_bytes: contract.rendered_prompt_bytes,
                    })
                    .collect(),
                narrowing_count: prepared.narrowing_count,
                duplicate: commit.duplicate,
            },
        })
    }
}
