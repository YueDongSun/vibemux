//! Held-executor audit, arm and joined-outcome transactions. No executor I/O.
use super::{cleanup::*, cleanup_authority::*, cleanup_rows::*, *};
use vibemux_types::workspace_cleanup::*;

#[derive(Debug)]
pub struct WorkspaceCleanupExecutorCommit {
    pub commit: WorkspaceCleanupCommit,
    pub arm_claim: Option<CleanupArmClaim>,
}
#[derive(Debug)]
pub struct WorkspaceCleanupArmCommit {
    pub commit: WorkspaceCleanupCommit,
    pub release_permit: Option<CleanupReleasePermit>,
}

/// A definite evidence-write failure retains the consumed, already-retired proof
/// for a safe persistence retry. It cannot restore spawn, arm or Go authority.
/// After an uncertain acknowledgement, read the durable operation/status first.
pub struct CleanupEvidenceFailure<P> {
    error: StoreError,
    proof: P,
}
impl<P> CleanupEvidenceFailure<P> {
    pub fn code(&self) -> &str {
        self.error.code()
    }
    pub fn into_parts(self) -> (StoreError, P) {
        (self.error, self.proof)
    }
}
impl<P> fmt::Debug for CleanupEvidenceFailure<P> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CleanupEvidenceFailure")
            .field("code", &self.code())
            .finish_non_exhaustive()
    }
}
impl<P> fmt::Display for CleanupEvidenceFailure<P> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("workspace cleanup evidence was not saved")
    }
}
impl<P> std::error::Error for CleanupEvidenceFailure<P> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// Verdict produced by the trusted executor or explicit reconciliation service
/// after physical postcondition reads under the exclusive resource gate.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum WorkspaceCleanupSettlement {
    RemovedExecuted,
    RemovedObserved,
    Refused { error_code: String },
    Unknown { error_code: String },
}

fn mutable_record(
    transaction: &Transaction<'_>,
    input: &WorkspaceCleanupMutation,
) -> Result<(WorkspaceCleanup, ChatExecutionOwnerCommit), StoreError> {
    let stored = required(transaction, input.cleanup_id)?;
    ensure(
        stored.cleanup.version == input.expected_version
            && !stored.cleanup.phase.is_terminal()
            && input.timestamp >= stored.cleanup.updated_at,
        "workspace_cleanup_conflict",
    )?;
    let owner = canonical_owner(transaction, &stored.cleanup.binding)?;
    Ok((stored.cleanup, owner))
}

fn advance(
    record: &mut WorkspaceCleanup,
    input: &WorkspaceCleanupMutation,
    phase: WorkspaceCleanupPhase,
) -> Result<(), StoreError> {
    record
        .phase
        .validate_transition_to(phase)
        .map_err(|_| cleanup_error("workspace_cleanup_invalid_transition"))?;
    record.phase = phase;
    record.version = increment(record.version)?;
    record.updated_at = input.timestamp;
    Ok(())
}

impl SqliteStore {
    pub fn record_workspace_cleanup_executor(
        &mut self,
        input: &WorkspaceCleanupMutation,
        claim: CleanupRegistrationClaim,
        executor: &ExecutorBinding,
    ) -> Result<WorkspaceCleanupExecutorCommit, StoreError> {
        executor
            .validate()
            .map_err(|_| cleanup_error("workspace_cleanup_invalid_input"))?;
        let binding = encode(&(input, executor))?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if let Some(commit) = replay(
            &transaction,
            input.operation_id,
            "workspace_cleanup_executor_registered",
            &binding,
        )? {
            return Ok(WorkspaceCleanupExecutorCommit {
                commit,
                arm_claim: None,
            });
        }
        let (mut record, owner) = mutable_record(&transaction, input)?;
        claim.check(&record, executor.executor_nonce)?;
        ensure(
            record.phase == WorkspaceCleanupPhase::Claimed && record.executor.is_none(),
            "workspace_cleanup_conflict",
        )?;
        record.executor = Some(executor.clone());
        advance(&mut record, input, WorkspaceCleanupPhase::Claimed)?;
        let commit = write(
            &transaction,
            record,
            Some(input.expected_version),
            input.operation_id,
            "workspace_cleanup_executor_registered",
            &binding,
            &owner.session,
        )?;
        transaction.commit()?;
        Ok(WorkspaceCleanupExecutorCommit {
            commit,
            arm_claim: Some(claim.arm_claim()),
        })
    }

    pub fn arm_workspace_cleanup(
        &mut self,
        input: &WorkspaceCleanupMutation,
        claim: CleanupArmClaim,
    ) -> Result<WorkspaceCleanupArmCommit, StoreError> {
        let binding = encode(input)?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if let Some(commit) = replay(
            &transaction,
            input.operation_id,
            "workspace_cleanup_armed",
            &binding,
        )? {
            return Ok(WorkspaceCleanupArmCommit {
                commit,
                release_permit: None,
            });
        }
        let (mut record, owner) = mutable_record(&transaction, input)?;
        claim.check(&record)?;
        ensure(
            record.phase == WorkspaceCleanupPhase::Claimed
                && record.executor.is_some()
                && record.arm_operation_id.is_none(),
            "workspace_cleanup_conflict",
        )?;
        record.arm_operation_id = Some(input.operation_id);
        advance(&mut record, input, WorkspaceCleanupPhase::Armed)?;
        let commit = write(
            &transaction,
            record,
            Some(input.expected_version),
            input.operation_id,
            "workspace_cleanup_armed",
            &binding,
            &owner.session,
        )?;
        transaction.commit()?;
        Ok(WorkspaceCleanupArmCommit {
            commit,
            release_permit: claim.release_permit(),
        })
    }

    pub fn record_workspace_cleanup_never_spawned(
        &mut self,
        input: &WorkspaceCleanupMutation,
        proof: CleanupNeverSpawnedClaim,
    ) -> Result<WorkspaceCleanupCommit, CleanupEvidenceFailure<CleanupNeverSpawnedClaim>> {
        let outcome = (|| {
            let binding = encode(input)?;
            let transaction = self
                .connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            if let Some(commit) = replay(
                &transaction,
                input.operation_id,
                "workspace_cleanup_quiescent_unspawned",
                &binding,
            )? {
                return Ok(commit);
            }
            let (mut record, owner) = mutable_record(&transaction, input)?;
            let retired_claim_id = proof.check(&record)?;
            ensure(
                matches!(
                    record.phase,
                    WorkspaceCleanupPhase::Claimed | WorkspaceCleanupPhase::Unknown
                ) && record.executor.is_none()
                    && record.arm_operation_id.is_none(),
                "workspace_cleanup_conflict",
            )?;
            record.quiescence = Some(CleanupQuiescence::NeverSpawned { retired_claim_id });
            record.error_code = None;
            advance(&mut record, input, WorkspaceCleanupPhase::Quiescent)?;
            let commit = write(
                &transaction,
                record,
                Some(input.expected_version),
                input.operation_id,
                "workspace_cleanup_quiescent_unspawned",
                &binding,
                &owner.session,
            )?;
            transaction.commit()?;
            Ok(commit)
        })();
        outcome.map_err(|error| CleanupEvidenceFailure { error, proof })
    }

    /// Completion authority exists before spawn/registration. It retires pending
    /// arm/release deliveries and can atomically save a lost registration plus join.
    pub fn record_workspace_cleanup_joined(
        &mut self,
        input: &WorkspaceCleanupMutation,
        claim: CleanupCompletionClaim,
        executor: &ExecutorBinding,
        result: Option<&CleanupProcessResult>,
        quiescence: &CleanupQuiescence,
    ) -> Result<WorkspaceCleanupCommit, CleanupEvidenceFailure<CleanupCompletionClaim>> {
        // Retire before even acquiring a transaction or checking a stale version.
        // A failed write returns only this retired completion proof.
        claim.retire();
        let outcome = (|| {
            let binding = encode(&(input, executor, result, quiescence))?;
            let transaction = self
                .connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            if let Some(commit) = replay(
                &transaction,
                input.operation_id,
                "workspace_cleanup_executor_joined",
                &binding,
            )? {
                return Ok(commit);
            }
            let (mut record, owner) = mutable_record(&transaction, input)?;
            ensure(
                matches!(
                    record.phase,
                    WorkspaceCleanupPhase::Claimed
                        | WorkspaceCleanupPhase::Armed
                        | WorkspaceCleanupPhase::Unknown
                ),
                "workspace_cleanup_invalid_transition",
            )?;
            ensure(
                matches!(quiescence,CleanupQuiescence::JoinedProcess {executor_nonce,..} if *executor_nonce == executor.executor_nonce),
                "workspace_cleanup_invalid_input",
            )?;
            ensure(
                record
                    .executor
                    .as_ref()
                    .is_none_or(|known| known == executor)
                    && record
                        .process_result
                        .as_ref()
                        .is_none_or(|known| Some(known) == result),
                "workspace_cleanup_binding_conflict",
            )?;
            // Retirement already preceded the transaction. Check this proof's
            // exact lineage without restoring any registration/release authority.
            claim.check(&record, executor.executor_nonce)?;
            record.executor = Some(executor.clone());
            record.process_result = result.cloned();
            record.quiescence = Some(quiescence.clone());
            let phase = if quiescence.is_positive() {
                record.error_code = None;
                WorkspaceCleanupPhase::Quiescent
            } else {
                record.error_code = Some("workspace_cleanup_quiescence_unverified".into());
                WorkspaceCleanupPhase::Unknown
            };
            advance(&mut record, input, phase)?;
            let commit = write(
                &transaction,
                record,
                Some(input.expected_version),
                input.operation_id,
                "workspace_cleanup_executor_joined",
                &binding,
                &owner.session,
            )?;
            transaction.commit()?;
            Ok(commit)
        })();
        outcome.map_err(|error| CleanupEvidenceFailure {
            error,
            proof: claim,
        })
    }

    /// No execution/release authority is returned. Removed/Refused require an
    /// already-durable positive quiescence receipt; Unknown keeps the reservation.
    pub fn settle_workspace_cleanup(
        &mut self,
        input: &WorkspaceCleanupMutation,
        settlement: &WorkspaceCleanupSettlement,
    ) -> Result<WorkspaceCleanupCommit, StoreError> {
        self.settle_cleanup(input, settlement, false)
    }

    /// Explicit recovery reads may settle an already-durable positive receipt.
    /// This cannot attribute an unobserved execution or mint live authority.
    pub fn reconcile_workspace_cleanup(
        &mut self,
        input: &WorkspaceCleanupMutation,
        settlement: &WorkspaceCleanupSettlement,
    ) -> Result<WorkspaceCleanupCommit, StoreError> {
        ensure(
            !matches!(settlement, WorkspaceCleanupSettlement::RemovedExecuted),
            "workspace_cleanup_invalid_input",
        )?;
        self.settle_cleanup(input, settlement, true)
    }

    fn settle_cleanup(
        &mut self,
        input: &WorkspaceCleanupMutation,
        settlement: &WorkspaceCleanupSettlement,
        reconcile: bool,
    ) -> Result<WorkspaceCleanupCommit, StoreError> {
        let binding = encode(&(input, settlement))?;
        let kind = if reconcile {
            "workspace_cleanup_reconciled"
        } else {
            "workspace_cleanup_settled"
        };
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if let Some(commit) = replay(&transaction, input.operation_id, kind, &binding)? {
            return Ok(commit);
        }
        let (mut record, owner) = mutable_record(&transaction, input)?;
        if reconcile
            && record.phase == WorkspaceCleanupPhase::Unknown
            && !matches!(settlement, WorkspaceCleanupSettlement::Unknown { .. })
            && record
                .quiescence
                .as_ref()
                .is_some_and(CleanupQuiescence::is_positive)
        {
            record
                .phase
                .validate_transition_to(WorkspaceCleanupPhase::Quiescent)
                .map_err(|_| cleanup_error("workspace_cleanup_invalid_transition"))?;
            // The two valid edges are committed atomically with one reconciliation
            // event; no intermediate recovery state or authority escapes.
            record.phase = WorkspaceCleanupPhase::Quiescent;
            record.error_code = None;
        }
        if !matches!(settlement, WorkspaceCleanupSettlement::Unknown { .. }) {
            ensure(
                record.phase == WorkspaceCleanupPhase::Quiescent
                    && record
                        .quiescence
                        .as_ref()
                        .is_some_and(CleanupQuiescence::is_positive),
                "workspace_cleanup_quiescence_required",
            )?;
        }
        let target = match settlement {
            WorkspaceCleanupSettlement::Unknown { error_code } => {
                record.error_code = Some(error_code.clone());
                WorkspaceCleanupPhase::Unknown
            }
            WorkspaceCleanupSettlement::RemovedExecuted => {
                record.outcome = Some(WorkspaceCleanupOutcome::RemovedExecuted);
                record.error_code = None;
                WorkspaceCleanupPhase::Removed
            }
            WorkspaceCleanupSettlement::RemovedObserved => {
                record.outcome = Some(WorkspaceCleanupOutcome::RemovedObserved);
                record.error_code = None;
                WorkspaceCleanupPhase::Removed
            }
            WorkspaceCleanupSettlement::Refused { error_code } => {
                record.outcome = Some(WorkspaceCleanupOutcome::Refused);
                record.error_code = Some(error_code.clone());
                WorkspaceCleanupPhase::Refused
            }
        };
        advance(&mut record, input, target)?;
        let commit = write(
            &transaction,
            record,
            Some(input.expected_version),
            input.operation_id,
            kind,
            &binding,
            &owner.session,
        )?;
        transaction.commit()?;
        Ok(commit)
    }
}
