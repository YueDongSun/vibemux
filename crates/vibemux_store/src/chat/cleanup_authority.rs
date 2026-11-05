//! Linear live authority. None of these tokens can be decoded from stored JSON.
use super::cleanup::{cleanup_error, ensure};
use crate::StoreError;
use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use uuid::Uuid;
use vibemux_types::{WorkspaceCleanupId, workspace_cleanup::WorkspaceCleanup};

struct Lineage {
    cleanup_id: WorkspaceCleanupId,
    claim_id: Uuid,
    incarnation: Uuid,
    retired: AtomicBool,
}

pub struct CleanupExecutionClaim {
    lineage: Arc<Lineage>,
}
pub struct CleanupNeverSpawnedClaim {
    lineage: Arc<Lineage>,
}
pub struct CleanupRegistrationClaim {
    lineage: Arc<Lineage>,
    nonce: Uuid,
}
pub struct CleanupCompletionClaim {
    lineage: Arc<Lineage>,
    nonce: Uuid,
}
pub struct CleanupArmClaim {
    lineage: Arc<Lineage>,
    nonce: Uuid,
}
pub struct CleanupReleasePermit {
    lineage: Arc<Lineage>,
    nonce: Uuid,
}

macro_rules! redacted_debug {
    ($($ty:ident),+ $(,)?) => {$(impl fmt::Debug for $ty {
        fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result { f.write_str(concat!(stringify!($ty),"(<redacted>)")) }
    })+};
}
redacted_debug!(
    CleanupExecutionClaim,
    CleanupNeverSpawnedClaim,
    CleanupRegistrationClaim,
    CleanupCompletionClaim,
    CleanupArmClaim,
    CleanupReleasePermit
);

impl Lineage {
    fn matches(&self, record: &WorkspaceCleanup) -> Result<(), StoreError> {
        ensure(
            record.id == self.cleanup_id
                && record.claim_id == Some(self.claim_id)
                && record.daemon_incarnation == self.incarnation,
            "workspace_cleanup_stale_claim",
        )
    }
    fn live(&self) -> Result<(), StoreError> {
        ensure(
            !self.retired.load(Ordering::Acquire),
            "workspace_cleanup_retired_claim",
        )
    }
}

impl CleanupExecutionClaim {
    pub(super) fn new(cleanup_id: WorkspaceCleanupId, claim_id: Uuid, incarnation: Uuid) -> Self {
        Self {
            lineage: Arc::new(Lineage {
                cleanup_id,
                claim_id,
                incarnation,
                retired: AtomicBool::new(false),
            }),
        }
    }
    /// Consume before any child-producing attempt. The two linear lanes allow a
    /// joined receipt even when executor-registration or arm acknowledgement is lost.
    pub fn begin_spawn(
        self,
        executor_nonce: Uuid,
    ) -> Result<(CleanupRegistrationClaim, CleanupCompletionClaim), StoreError> {
        ensure(!executor_nonce.is_nil(), "workspace_cleanup_invalid_input")?;
        self.lineage.live()?;
        Ok((
            CleanupRegistrationClaim {
                lineage: Arc::clone(&self.lineage),
                nonce: executor_nonce,
            },
            CleanupCompletionClaim {
                lineage: self.lineage,
                nonce: executor_nonce,
            },
        ))
    }
    /// Consume the unused spawn authority. Missing executor fields cannot do this.
    pub fn retire_without_spawn(self) -> CleanupNeverSpawnedClaim {
        self.lineage.retired.store(true, Ordering::Release);
        CleanupNeverSpawnedClaim {
            lineage: self.lineage,
        }
    }
}
impl CleanupNeverSpawnedClaim {
    pub(super) fn check(&self, record: &WorkspaceCleanup) -> Result<Uuid, StoreError> {
        self.lineage.matches(record)?;
        Ok(self.lineage.claim_id)
    }
}
impl CleanupRegistrationClaim {
    pub(super) fn check(&self, record: &WorkspaceCleanup, nonce: Uuid) -> Result<(), StoreError> {
        self.lineage.matches(record)?;
        self.lineage.live()?;
        ensure(self.nonce == nonce, "workspace_cleanup_stale_claim")
    }
    pub(super) fn arm_claim(self) -> CleanupArmClaim {
        CleanupArmClaim {
            lineage: self.lineage,
            nonce: self.nonce,
        }
    }
}
impl CleanupArmClaim {
    pub(super) fn check(&self, record: &WorkspaceCleanup) -> Result<(), StoreError> {
        self.lineage.matches(record)?;
        self.lineage.live()?;
        ensure(
            record.executor.as_ref().map(|e| e.executor_nonce) == Some(self.nonce),
            "workspace_cleanup_stale_claim",
        )
    }
    pub(super) fn release_permit(self) -> Option<CleanupReleasePermit> {
        self.lineage.live().ok()?;
        Some(CleanupReleasePermit {
            lineage: self.lineage,
            nonce: self.nonce,
        })
    }
}
impl CleanupCompletionClaim {
    /// Retire release deliveries before handing completed evidence to an
    /// asynchronous writer custodian. The returned token permits persistence
    /// only; it cannot recreate registration, Arm or release authority.
    pub fn retire_for_persistence(self) -> Self {
        self.retire();
        self
    }
    /// Revoke pending registration/arm/release deliveries before persisting join.
    pub(super) fn retire(&self) {
        self.lineage.retired.store(true, Ordering::Release);
    }
    pub(super) fn check(&self, record: &WorkspaceCleanup, nonce: Uuid) -> Result<(), StoreError> {
        self.lineage.matches(record)?;
        ensure(self.nonce == nonce, "workspace_cleanup_stale_claim")?;
        Ok(())
    }
}
impl CleanupReleasePermit {
    /// Consumed inside the held-process release callback immediately before Go.
    /// A late writer acknowledgement after joined completion cannot authorize Go.
    pub fn consume(self, executor_nonce: Uuid, daemon_incarnation: Uuid) -> Result<(), StoreError> {
        self.lineage.live()?;
        if self.nonce != executor_nonce || self.lineage.incarnation != daemon_incarnation {
            return Err(cleanup_error("workspace_cleanup_stale_claim"));
        }
        Ok(())
    }
}
