//! Atomic successor admission and quiescent-parent fencing.

use std::collections::BTreeMap;

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_workflow::{
    Sha256Digest, SpecIdentifier,
    canonical_json::canonical_digest,
    checkpoint::ContextSettingsIdentity,
    contract::ContractArtifact,
    contract_edit::validate_semantic_edit,
    gates::WorkflowPhase,
    policy::OperatorPolicy,
    task_spec::{CheckKind, TaskSpec},
    workflow_record::{TaskProgress, WorkflowRecord},
};

use super::{append, millis, uuid_key, workflow_error, workflow_rows};
use crate::{SqliteStore, StoreError};

pub(crate) const MIGRATION_V8: &str = r#"
CREATE TABLE workflow_contract_edits (
    edit_id TEXT PRIMARY KEY,
    parent_workflow_id TEXT NOT NULL UNIQUE REFERENCES workflows(workflow_id),
    successor_workflow_id TEXT NOT NULL UNIQUE REFERENCES workflows(workflow_id),
    request_digest TEXT NOT NULL,
    record_json TEXT NOT NULL,
    created_sequence INTEGER NOT NULL REFERENCES events(sequence)
);
INSERT INTO schema_migrations(version, applied_at)
VALUES (8, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
"#;
const PLAN_DOMAIN: &str = "vibemux.workflow.plan.v1";
const ADMISSION_DOMAIN: &str = "vibemux.workflow.contract_edit_admission.v1";
const MAX_PLAN_BYTES: usize = 512 * 1024;

/// A transient complete authority input, never retained in normal audit rows.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    schema_version: u32,
    request_sha256: Sha256Digest,
    policy: OperatorPolicy,
    specs: BTreeMap<SpecIdentifier, TaskSpec>,
    assignments: BTreeMap<SpecIdentifier, Vec<SpecIdentifier>>,
    reviewer_slot: SpecIdentifier,
    template_overrides: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    negative_control: bool,
}

#[derive(Clone)]
pub struct ContractEditAdmission {
    pub edit_id: Uuid,
    pub parent_workflow_id: Uuid,
    pub expected_parent_version: u64,
    pub request_digest: Sha256Digest,
    pub parent_plan: Value,
    pub successor_plan: Value,
    pub successor: WorkflowRecord,
    pub contracts: Vec<ContractArtifact>,
    pub timestamp: OffsetDateTime,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContractEditRecord {
    pub edit_id: Uuid,
    pub parent_workflow_id: Uuid,
    pub parent_version: u64,
    pub parent_plan_digest: Sha256Digest,
    pub successor_workflow_id: Uuid,
    pub successor_plan_digest: Sha256Digest,
    pub request_digest: Sha256Digest,
    pub admission_digest: Sha256Digest,
    pub context_settings: ContextSettingsIdentity,
}

pub struct ContractEditCommit {
    pub edit: ContractEditRecord,
    pub record: WorkflowRecord,
    pub duplicate: bool,
}

fn decode_plan(value: &Value) -> Result<Plan, StoreError> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_PLAN_BYTES {
        return Err(workflow_error("workflow_edit_invalid_plan"));
    }
    serde_json::from_slice(&bytes).map_err(|_| workflow_error("workflow_edit_invalid_plan"))
}

fn admission_digest(input: &ContractEditAdmission) -> Result<Sha256Digest, StoreError> {
    let mut immutable_record = input.successor.clone();
    // Preparation time is not permission or accounting identity; replay may
    // occur later. All budgets, counters, contracts and configuration remain.
    immutable_record.created_at_ms = 0;
    immutable_record.updated_at_ms = 0;
    canonical_digest(
        ADMISSION_DOMAIN,
        &json!({
            "edit_id":input.edit_id,"parent_workflow_id":input.parent_workflow_id,
            "parent_version":input.expected_parent_version,"request_digest":input.request_digest,
            "parent_plan":input.parent_plan,"successor_plan":input.successor_plan,
            "successor":immutable_record,"contracts":input.contracts,
        }),
    )
    .map_err(|_| workflow_error("workflow_edit_invalid_plan"))
}

fn load_edit(
    connection: &rusqlite::Connection,
    edit_id: Uuid,
) -> Result<Option<ContractEditRecord>, StoreError> {
    let row: Option<(String, String, String, String)> = connection.query_row(
        "SELECT parent_workflow_id,successor_workflow_id,request_digest,record_json FROM workflow_contract_edits WHERE edit_id=?",
        [uuid_key(edit_id)], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).optional()?;
    let Some((parent, successor, digest, text)) = row else {
        return Ok(None);
    };
    let value: ContractEditRecord = serde_json::from_str(&text)
        .map_err(|_| workflow_error("workflow_edit_projection_mismatch"))?;
    if value.edit_id != edit_id
        || uuid_key(value.parent_workflow_id) != parent
        || uuid_key(value.successor_workflow_id) != successor
        || value.request_digest.to_hex() != digest
    {
        return Err(workflow_error("workflow_edit_projection_mismatch"));
    }
    Ok(Some(value))
}

impl SqliteStore {
    pub fn workflow_contract_edit(
        &self,
        edit_id: Uuid,
    ) -> Result<Option<ContractEditRecord>, StoreError> {
        load_edit(&self.connection, edit_id)
    }

    /// The parent fence, successor and provenance event share one transaction.
    /// Replay checks complete request identity before checking the now-retired parent.
    pub fn prepare_contract_edit(
        &mut self,
        input: &ContractEditAdmission,
    ) -> Result<ContractEditCommit, StoreError> {
        if input.edit_id.is_nil()
            || input.expected_parent_version == 0
            || input.parent_workflow_id == input.successor.workflow_id
        {
            return Err(workflow_error("workflow_edit_invalid_request"));
        }
        let transaction = self.immediate()?;
        let complete_admission_digest = admission_digest(input)?;
        if let Some(edit) = load_edit(&transaction, input.edit_id)? {
            if edit.parent_workflow_id != input.parent_workflow_id
                || edit.parent_version != input.expected_parent_version
                || edit.request_digest != input.request_digest
                || edit.admission_digest != complete_admission_digest
                || edit.successor_workflow_id != input.successor.workflow_id
                || edit.parent_plan_digest
                    != canonical_digest(PLAN_DOMAIN, &input.parent_plan)
                        .map_err(|_| workflow_error("workflow_edit_invalid_plan"))?
                || edit.successor_plan_digest
                    != canonical_digest(PLAN_DOMAIN, &input.successor_plan)
                        .map_err(|_| workflow_error("workflow_edit_invalid_plan"))?
            {
                return Err(workflow_error("workflow_edit_request_conflict"));
            }
            let stored =
                workflow_rows::load_existing_workflow(&transaction, edit.successor_workflow_id)?;
            return Ok(ContractEditCommit {
                edit,
                record: stored.record,
                duplicate: true,
            });
        }
        let already_edited: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM workflow_contract_edits WHERE parent_workflow_id=?)",
            [uuid_key(input.parent_workflow_id)],
            |row| row.get(0),
        )?;
        if already_edited {
            return Err(workflow_error("workflow_edit_parent_already_retired"));
        }
        let stored = workflow_rows::load_existing_workflow(&transaction, input.parent_workflow_id)?;
        let parent = &stored.record;
        if parent.version != input.expected_parent_version {
            return Err(workflow_error("store_workflow_version_conflict"));
        }
        if !matches!(
            parent.phase,
            WorkflowPhase::Prepared
                | WorkflowPhase::Paused
                | WorkflowPhase::Blocked
                | WorkflowPhase::Accepted
                | WorkflowPhase::Failed
                | WorkflowPhase::Cancelled
        ) {
            return Err(workflow_error("workflow_edit_parent_not_quiescent"));
        }
        let parent_key = uuid_key(parent.workflow_id);
        // No active or uncertain effect is transferred or implicitly approved.
        for sql in [
            "SELECT COUNT(*) FROM workflow_leases WHERE workflow_id=? AND state NOT IN ('released','revoked')",
            "SELECT COUNT(*) FROM workflow_attempts a LEFT JOIN harness_dispatches d ON d.request_id=a.request_id WHERE a.workflow_id=? AND (d.phase IS NULL OR d.phase NOT IN ('completed','failed','cancelled','unverified'))",
            "SELECT COUNT(*) FROM workflow_model_calls WHERE owner_key IN (SELECT owner_key FROM workflow_model_call_owners WHERE workflow_id=?) AND state NOT IN ('completed','blocked','rejected')",
            "SELECT COUNT(*) FROM workflow_supervisor_rounds WHERE workflow_id=? AND COALESCE(json_extract(record_json,'$.state'),'unknown') NOT IN ('accepted','rejected')",
            "SELECT COUNT(*) FROM workflow_supervisor_actions WHERE workflow_id=? AND state NOT IN ('applied','blocked')",
            "SELECT COUNT(*) FROM workflow_supervisors WHERE workflow_id=? AND state IN ('running','recovery_required')",
            "SELECT COUNT(*) FROM workflow_controllers WHERE workflow_id=?",
            "SELECT COUNT(*) FROM workflow_messages WHERE workflow_id=? AND state='delivered'",
        ] {
            let count: u64 = transaction.query_row(sql, [&parent_key], |row| row.get(0))?;
            if count != 0 {
                return Err(workflow_error("workflow_edit_unsettled_effects"));
            }
        }
        let binding_text: String = transaction
            .query_row(
                "SELECT record_json FROM workflow_recovery_bindings WHERE workflow_id=?",
                [&parent_key],
                |row| row.get(0),
            )
            .map_err(|_| workflow_error("workflow_edit_parent_plan_unbound"))?;
        let binding: super::RecoveryPlanBinding = serde_json::from_str(&binding_text)?;
        let parent_plan_digest = canonical_digest(PLAN_DOMAIN, &input.parent_plan)
            .map_err(|_| workflow_error("workflow_edit_invalid_plan"))?;
        if binding.workflow_id != parent.workflow_id || binding.plan_digest != parent_plan_digest {
            return Err(workflow_error("workflow_edit_parent_plan_changed"));
        }
        let before = decode_plan(&input.parent_plan)?;
        let after = decode_plan(&input.successor_plan)?;
        if before.schema_version != 1
            || after.schema_version != 1
            || before.request_sha256 != after.request_sha256
            || before.policy != after.policy
            || before.assignments != after.assignments
            || before.reviewer_slot != after.reviewer_slot
            || before.template_overrides != after.template_overrides
            || before.negative_control != after.negative_control
            || before.specs.keys().ne(after.specs.keys())
        {
            return Err(workflow_error("workflow_edit_immutable_authority"));
        }
        input
            .successor
            .validate()
            .map_err(|error| workflow_error(error.code()))?;
        if input.successor.phase != WorkflowPhase::Prepared
            || input.successor.version != 1
            || input.successor.mode != parent.mode
            || input.successor.workflow_key != parent.workflow_key
            || input.successor.base_commit != parent.base_commit
            || input.successor.policy_digest != parent.policy_digest
            || input.successor.verifier_digest != parent.verifier_digest
            || input.successor.content_store != parent.content_store
            || input.successor.evidence_class != parent.evidence_class
            || input.successor.integration_suites != parent.integration_suites
            || input.successor.tasks.len() != parent.tasks.len()
            || input.contracts.len() != parent.tasks.len()
            || input.successor.budget.decisions_remaining > parent.budget.decisions_remaining
            || input.successor.budget.malformed_repairs_remaining
                > parent.budget.malformed_repairs_remaining
            || input.successor.budget.model_requests_remaining
                > parent.budget.model_requests_remaining
            || input.successor.budget.deadline_unix_ms > parent.budget.deadline_unix_ms
        {
            return Err(workflow_error("workflow_edit_immutable_authority"));
        }
        let mut changed = false;
        for task in &parent.tasks {
            let old = before
                .specs
                .get(&task.task_key)
                .ok_or_else(|| workflow_error("workflow_edit_invalid_plan"))?;
            let new = after
                .specs
                .get(&task.task_key)
                .ok_or_else(|| workflow_error("workflow_edit_invalid_plan"))?;
            let diff =
                validate_semantic_edit(old, new).map_err(|error| workflow_error(error.code()))?;
            let successor_task = input
                .successor
                .task(&task.task_key)
                .ok_or_else(|| workflow_error("workflow_edit_invalid_plan"))?;
            let old_artifact = workflow_rows::load_contract(&transaction, task.contract_id)?
                .ok_or_else(|| workflow_error("workflow_edit_invalid_plan"))?;
            let artifact = input
                .contracts
                .iter()
                .find(|value| value.task_key == task.task_key.as_str())
                .ok_or_else(|| workflow_error("workflow_edit_invalid_plan"))?;
            let mut required_suites: Vec<_> = new
                .acceptance_checks
                .iter()
                .filter_map(|check| match &check.kind {
                    CheckKind::ExistingVerifierSuite => check.verifier_suite.clone(),
                    _ => None,
                })
                .collect();
            required_suites.sort();
            required_suites.dedup();
            if old
                .digest()
                .map_err(|_| workflow_error("workflow_edit_invalid_plan"))?
                != old_artifact.task_spec_digest
                || new
                    .digest()
                    .map_err(|_| workflow_error("workflow_edit_invalid_plan"))?
                    != artifact.task_spec_digest
                || successor_task.contract_id != artifact.contract_id
                || successor_task.contract_version != new.contract_version
                || successor_task.turns_used != task.turns_used
                || successor_task.repairs_used != task.repairs_used
                || successor_task.progress != TaskProgress::Pending
                || successor_task.accepted_candidate.is_some()
                || successor_task.failure_code.is_some()
                || successor_task.max_turns != new.resource_budget.max_turns
                || successor_task.max_repairs != new.resource_budget.max_repairs
                || successor_task.required_suites != required_suites
                || artifact.workflow_key != parent.workflow_key.as_str()
                || artifact.contract_version != new.contract_version
                || artifact.base_commit != parent.base_commit
                || artifact.policy_digest != parent.policy_digest
            {
                return Err(workflow_error("workflow_edit_invalid_plan"));
            }
            if artifact.harness_profile != old_artifact.harness_profile
                || artifact.route_transformation != old_artifact.route_transformation
                || artifact.template_digest != old_artifact.template_digest
                || artifact.template_version != old_artifact.template_version
                || artifact.context_bundle_digests != old_artifact.context_bundle_digests
            {
                return Err(workflow_error("workflow_edit_profile_changed"));
            }
            if diff.is_empty() {
                if artifact != &old_artifact {
                    return Err(workflow_error("workflow_edit_invalid_generation"));
                }
            } else {
                changed = true;
                if artifact.previous_contract_id != Some(old_artifact.contract_id)
                    || artifact.semantic_diff.as_ref() != Some(&diff)
                {
                    return Err(workflow_error("workflow_edit_invalid_generation"));
                }
            }
        }
        if !changed {
            return Err(workflow_error("workflow_edit_unchanged"));
        }
        if workflow_rows::load_workflow_by_request(&transaction, &input.successor.request_key)?
            .is_some()
        {
            return Err(workflow_error("workflow_edit_request_conflict"));
        }
        if matches!(
            parent.phase,
            WorkflowPhase::Prepared | WorkflowPhase::Paused | WorkflowPhase::Blocked
        ) {
            let mut retired = parent.clone();
            let phases: &[WorkflowPhase] = if parent.phase == WorkflowPhase::Prepared {
                &[WorkflowPhase::Cancelled]
            } else {
                &[WorkflowPhase::CancelRequested, WorkflowPhase::Cancelled]
            };
            for phase in phases {
                let previous_version = retired.version;
                let from = retired.phase;
                retired = retired
                    .with_phase(*phase, millis(input.timestamp))
                    .map_err(|error| workflow_error(error.code()))?;
                let (_, sequence) = append(
                    &transaction,
                    "workflow_phase_changed",
                    format!(
                        "workflow_edit_retired:{}:{}",
                        uuid_key(input.edit_id),
                        phase.as_str()
                    ),
                    json!({"workflow_id":parent.workflow_id,"from":from,"to":retired.phase,"version":retired.version,"reason_code":"contract_edited"}),
                    input.timestamp,
                )?;
                workflow_rows::update_workflow(&transaction, &retired, previous_version, sequence)?;
            }
        }
        let edit = ContractEditRecord {
            edit_id: input.edit_id,
            parent_workflow_id: parent.workflow_id,
            parent_version: parent.version,
            parent_plan_digest,
            successor_workflow_id: input.successor.workflow_id,
            successor_plan_digest: canonical_digest(PLAN_DOMAIN, &input.successor_plan)
                .map_err(|_| workflow_error("workflow_edit_invalid_plan"))?,
            request_digest: input.request_digest,
            admission_digest: complete_admission_digest,
            context_settings: binding.context_settings,
        };
        let (_, sequence) = append(
            &transaction,
            "workflow_contract_edited",
            format!("workflow_contract_edited:{}", uuid_key(input.edit_id)),
            json!({"edit_id":edit.edit_id,"parent_workflow_id":parent.workflow_id,"parent_version":parent.version,"successor_workflow_id":edit.successor_workflow_id,"request_digest":edit.request_digest,"successor_plan_digest":edit.successor_plan_digest}),
            input.timestamp,
        )?;
        for artifact in &input.contracts {
            workflow_rows::insert_contract_if_absent(&transaction, artifact, sequence)?;
        }
        let fingerprint = input
            .successor
            .admission_fingerprint()
            .map_err(|_| workflow_error("workflow_edit_invalid_plan"))?;
        workflow_rows::insert_workflow(&transaction, &input.successor, fingerprint, sequence)?;
        transaction.execute("INSERT INTO workflow_contract_edits(edit_id,parent_workflow_id,successor_workflow_id,request_digest,record_json,created_sequence) VALUES(?,?,?,?,?,?)",
            params![uuid_key(edit.edit_id),parent_key,uuid_key(edit.successor_workflow_id),edit.request_digest.to_hex(),serde_json::to_string(&edit)?,sequence])?;
        transaction.commit()?;
        Ok(ContractEditCommit {
            edit,
            record: input.successor.clone(),
            duplicate: false,
        })
    }
}
