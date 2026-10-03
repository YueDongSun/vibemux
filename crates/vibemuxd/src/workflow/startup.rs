//! Startup of the workflow service (ADR 031 §4).
//!
//! The config is read and pinned once, the slot observations are refreshed
//! from it, and workflows a previous daemon left mid-run are reconciled.
//! Nothing resumes on its own: a running or integrating workflow stops at
//! `blocked` with [`DAEMON_RESTART_CODE`] until the operator starts it
//! again, and a requested cancellation is finished. The store has already
//! quarantined every lease whose attempt was interrupted.

use std::path::Path;

use time::OffsetDateTime;
use vibemux_workflow::{gates::WorkflowPhase, leases::LeaseState};

use super::{
    error::WorkflowError,
    runtime::{WorkflowSetup, now_ms, revoke_lease, transition},
    settings::{WORKFLOW_CONFIG_FILE_NAME, load_workflow_config},
    state_files::{StateFiles, WORKFLOW_STATE_DIR_NAME},
    state_protection::protect_state_root,
    turns::startup_slot_facts,
};
use crate::{
    WriterHandle,
    harness_dispatch::{HarnessDispatchService, call_writer},
};

pub const DAEMON_RESTART_CODE: &str = "daemon_restart";
/// Lease end reason of a cancellation finished at startup.
const CANCEL_FINISHED_CODE: &str = "workflow_cancelled";
/// Bound on workflows reconciled at one start.
const RECONCILE_LIMIT: usize = 256;

/// Reads and pins the config next to the daemon state. Blocking.
pub(crate) fn load_setup(
    project_root: &Path,
    state_dir: &Path,
    dispatch: &HarnessDispatchService,
) -> Result<WorkflowSetup, WorkflowError> {
    let canonical_root =
        std::fs::canonicalize(project_root).map_err(|_| WorkflowError::ConfigInvalid)?;
    let config = load_workflow_config(&state_dir.join(WORKFLOW_CONFIG_FILE_NAME), &canonical_root)?;
    let slot_protocols = config
        .config
        .slots
        .iter()
        .filter_map(|slot| {
            dispatch
                .execution_protocol(slot.harness)
                .map(|protocol| (slot.slot_id.clone(), protocol))
        })
        .collect();
    protect_state_root(&state_dir.join(WORKFLOW_STATE_DIR_NAME))?;
    Ok(WorkflowSetup {
        config,
        canonical_root,
        state: StateFiles::new(state_dir),
        slot_protocols,
        trampoline: dispatch.trampoline(),
    })
}

/// Records what the daemon can honestly claim about each configured slot.
pub(crate) async fn observe_slots(
    writer: &WriterHandle,
    setup: &WorkflowSetup,
) -> Result<(), WorkflowError> {
    let facts = startup_slot_facts(
        &setup.config,
        |slot_id| setup.slot_protocols.contains_key(slot_id),
        now_ms(),
    );
    for slot in facts {
        let writer = writer.clone();
        call_writer(move || writer.upsert_workflow_slot(slot.clone(), OffsetDateTime::now_utc()))
            .await?;
    }
    Ok(())
}

/// Stops every workflow a previous daemon left mid-run and finishes every
/// requested cancellation. Needs only the writer, so it runs even when the
/// workflow config is missing.
pub(crate) async fn reconcile(
    writer: &WriterHandle,
    heartbeat_ms: u64,
) -> Result<(), WorkflowError> {
    let reader = writer.clone();
    let workflows = call_writer(move || reader.workflows(RECONCILE_LIMIT)).await?;
    for record in workflows {
        let workflow_id = record.workflow_id;
        match record.phase {
            WorkflowPhase::Running => {
                transition(
                    writer,
                    workflow_id,
                    WorkflowPhase::Blocked,
                    Some(DAEMON_RESTART_CODE),
                )
                .await?;
            }
            WorkflowPhase::Integrating => {
                // Integration restarts from the accepted candidates; the
                // edge to `blocked` passes through `running`.
                transition(writer, workflow_id, WorkflowPhase::Running, None).await?;
                transition(
                    writer,
                    workflow_id,
                    WorkflowPhase::Blocked,
                    Some(DAEMON_RESTART_CODE),
                )
                .await?;
            }
            WorkflowPhase::CancelRequested => {
                finish_cancel(writer, workflow_id, heartbeat_ms).await?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Ends every lease the workflow still holds, then commits `cancelled`.
/// Leases are revoked first so no slot stays reserved by a cancelled
/// workflow.
pub(crate) async fn finish_cancel(
    writer: &WriterHandle,
    workflow_id: uuid::Uuid,
    heartbeat_ms: u64,
) -> Result<(), WorkflowError> {
    let reader = writer.clone();
    let snapshot = call_writer(move || reader.workflow_snapshot(workflow_id))
        .await?
        .ok_or(WorkflowError::NotFound)?;
    for lease in snapshot
        .leases
        .iter()
        .filter(|lease| matches!(lease.state, LeaseState::Active | LeaseState::Quarantined))
    {
        revoke_lease(writer, lease, CANCEL_FINISHED_CODE, heartbeat_ms).await?;
    }
    if snapshot.record.phase != WorkflowPhase::Cancelled {
        transition(writer, workflow_id, WorkflowPhase::Cancelled, None).await?;
    }
    Ok(())
}
