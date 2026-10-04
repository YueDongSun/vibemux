//! Integration of accepted candidates and workspace cleanup (ADR 031 §6).
//!
//! Accepted candidates are applied from the blob store into a fresh owned
//! worktree at the base commit; the operator's checkout is never written
//! and nothing is committed, merged, or pushed. The integration tree is
//! re-collected, verified by the trusted suites, and only then offered to
//! the writer's acceptance gate. Cleanup removes only worktrees that are
//! provably clean; anything dirty or unknown is retained and reported.

use std::path::PathBuf;

use serde::Serialize;
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_workflow::{
    PathPattern, Sha256Digest,
    gates::{IntegrationReceipt, RECEIPT_SCHEMA_VERSION, WorkflowPhase},
    receipts::WorkflowReceipt,
};

use super::{
    candidate::{CollectedCandidate, VerifySubject, materialize, verify_subject},
    collector::{CollectionScope, overlapping_paths},
    error::WorkflowError,
    runtime::{RunContext, Unit},
};
use crate::harness_dispatch::call_writer;

const PATCH_DOMAIN: &str = "vibemux.workflow.integration_patch.v1";
const INTEGRATION_KEY_DOMAIN: &str = "vibemux.workflow.integration_key.v1";
pub const INTEGRATION_CONFLICTS_CODE: &str = "integration_conflicts";
pub const INTEGRATION_GATE_CODE: &str = "integration_gate_failed";

/// Integrates the accepted candidates, verifies the tree, and asks the
/// writer to accept the workflow. Returns `None` when the workflow was
/// accepted, else the failure code it ended with. Both receipts are
/// recorded either way.
pub(crate) async fn integrate(
    context: &RunContext,
    accepted: &[(Unit, CollectedCandidate)],
) -> Result<Option<String>, WorkflowError> {
    let record = context.record().await?;
    if record.phase == WorkflowPhase::Running {
        context.transition(WorkflowPhase::Integrating, None).await?;
    }
    let mut ordered: Vec<&(Unit, CollectedCandidate)> = accepted.iter().collect();
    ordered.sort_by(|left, right| left.0.task_key.cmp(&right.0.task_key));
    let manifests = ordered
        .iter()
        .map(|(_, candidate)| candidate.manifest().cloned().ok_or(WorkflowError::Snapshot))
        .collect::<Result<Vec<_>, _>>()?;
    let digests: Vec<Sha256Digest> = ordered
        .iter()
        .map(|(_, candidate)| candidate.digest())
        .collect();
    let mut contract_ids: Vec<Sha256Digest> =
        ordered.iter().map(|(unit, _)| unit.contract_id).collect();
    contract_ids.sort();
    contract_ids.dedup();
    let conflicts = overlapping_paths(&manifests.iter().collect::<Vec<_>>());
    // The integration tree may hold every accepted task's owned paths; the
    // operator's protected paths stay protected.
    let mut owned: Vec<PathPattern> = ordered
        .iter()
        .filter_map(|(unit, _)| context.plan.specs.get(&unit.task_key))
        .flat_map(|spec| spec.owned_paths.iter().cloned())
        .collect();
    owned.sort();
    owned.dedup();
    let scope = CollectionScope {
        owned: &owned,
        forbidden: &[],
        protected: &context.plan.policy.protected_paths,
        limits: context.setup.config.config.candidate_limits,
    };
    let digest_bytes: Vec<&[u8]> = digests
        .iter()
        .map(|digest| digest.as_bytes().as_slice())
        .collect();
    let key_id = Sha256Digest::of_fields(INTEGRATION_KEY_DOMAIN, &digest_bytes);
    let key = format!("integration:{}", key_id.to_hex());
    let (workspace, collection) = materialize(context, &key, &manifests, scope).await?;
    let tree_digest = collection.digest().ok_or(WorkflowError::Snapshot)?;
    let receipt = IntegrationReceipt {
        schema_version: RECEIPT_SCHEMA_VERSION,
        receipt_id: Uuid::new_v4(),
        workflow_id: context.workflow_id,
        base_commit: record.base_commit.clone(),
        applied_candidates: digests.clone(),
        contract_ids: contract_ids.clone(),
        integration_tree_digest: tree_digest,
        patch_sha256: Sha256Digest::of_fields(PATCH_DOMAIN, &digest_bytes),
        conflicts: conflicts.clone(),
    };
    record_receipt(context, WorkflowReceipt::Integration(receipt)).await?;
    if !conflicts.is_empty() {
        context
            .transition(WorkflowPhase::Failed, Some(INTEGRATION_CONFLICTS_CODE))
            .await?;
        return Ok(Some(INTEGRATION_CONFLICTS_CODE.to_string()));
    }
    let verification = verify_subject(
        context,
        VerifySubject {
            workspace: &workspace,
            scope,
            digest: tree_digest,
            task_key: None,
            contract_ids,
            suites: &record.integration_suites,
        },
    )
    .await?;
    record_receipt(context, WorkflowReceipt::Verification(verification)).await?;
    let workflow_id = context.workflow_id;
    match context
        .versioned(move |handle, version| {
            handle.accept_workflow(workflow_id, version, OffsetDateTime::now_utc())
        })
        .await
    {
        Ok(_) => Ok(None),
        Err(_) => {
            context
                .transition(WorkflowPhase::Failed, Some(INTEGRATION_GATE_CODE))
                .await?;
            Ok(Some(INTEGRATION_GATE_CODE.to_string()))
        }
    }
}

async fn record_receipt(
    context: &RunContext,
    receipt: WorkflowReceipt,
) -> Result<(), WorkflowError> {
    let writer = context.writer.clone();
    let workflow_id = context.workflow_id;
    call_writer(move || {
        writer.record_workflow_receipt(
            workflow_id,
            receipt.clone(),
            None,
            OffsetDateTime::now_utc(),
        )
    })
    .await?;
    Ok(())
}

/// The fate of one owned worktree after a workflow ended.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CleanupEntry {
    /// The ledger key (`worker:<task>:<slot>`, `review:...`, ...), never a
    /// path.
    pub key: String,
    pub removed: bool,
    pub reason_code: Option<String>,
}

/// Removes every recorded worktree that is provably clean and retains the
/// rest. Candidate bytes stay in the blob store either way.
pub(crate) async fn clean_up_workspaces(
    context: &RunContext,
) -> Result<Vec<CleanupEntry>, WorkflowError> {
    let workspaces: Vec<(String, vibemux_types::a2a::RunWorkspace)> = context
        .ledger
        .lock()
        .map_err(|_| WorkflowError::Internal)?
        .workspaces
        .iter()
        .map(|(key, workspace)| (key.clone(), workspace.clone()))
        .collect();
    let mut entries = Vec::new();
    for (key, workspace) in workspaces {
        if !PathBuf::from(&workspace.path).exists() {
            entries.push(CleanupEntry {
                key,
                removed: true,
                reason_code: Some("workspace_already_absent".to_string()),
            });
            continue;
        }
        let entry = match context.manager.cleanup(&workspace).await {
            Ok(()) => CleanupEntry {
                key,
                removed: true,
                reason_code: None,
            },
            Err(error) => CleanupEntry {
                key,
                removed: false,
                reason_code: Some(error.code().to_string()),
            },
        };
        entries.push(entry);
    }
    Ok(entries)
}
