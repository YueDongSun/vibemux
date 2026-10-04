//! Fenced slot and worktree leases (ADR 031 §4).
//!
//! A lease reserves one slot and one worktree for one task attempt under a
//! monotonically increasing generation. A missed heartbeat never releases a
//! lease: it quarantines it, and the reservation holds until the attempt
//! settles or reconciliation proves the owned process is gone. Every
//! completion carries the generation it was issued under; a completion with
//! any other generation is stale and is refused.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseState {
    Active,
    /// Ownership is uncertain (missed heartbeat, daemon restart while the
    /// attempt ran). The reservation holds; no second writer may enter.
    Quarantined,
    Released,
    /// Ended by the operator or a cancellation after settlement.
    Revoked,
}

impl LeaseState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Quarantined => "quarantined",
            Self::Released => "released",
            Self::Revoked => "revoked",
        }
    }

    /// Whether the slot and worktree stay reserved.
    #[must_use]
    pub const fn holds_reservation(self) -> bool {
        matches!(self, Self::Active | Self::Quarantined)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeaseEvent {
    Heartbeat {
        generation: u64,
        now_ms: u64,
    },
    HeartbeatMissed {
        now_ms: u64,
    },
    /// The attempt under this generation reached a terminal, fenced state.
    AttemptSettled {
        generation: u64,
    },
    /// Reconciliation of a quarantined lease; `process_gone` is true only
    /// when the owned process tree is confirmed absent.
    Reconciled {
        generation: u64,
        process_gone: bool,
    },
    Revoke {
        generation: u64,
        attempt_settled: bool,
    },
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum LeaseError {
    #[error("lease generation is stale")]
    StaleGeneration,
    #[error("lease is not active")]
    NotActive,
    #[error("lease already ended")]
    Ended,
    #[error("heartbeat deadline has not passed")]
    HeartbeatNotMissed,
    #[error("a lease with an unsettled attempt cannot be revoked")]
    AttemptUnsettled,
}

impl LeaseError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::StaleGeneration => "workflow_lease_stale_generation",
            Self::NotActive => "workflow_lease_not_active",
            Self::Ended => "workflow_lease_ended",
            Self::HeartbeatNotMissed => "workflow_lease_heartbeat_not_missed",
            Self::AttemptUnsettled => "workflow_lease_attempt_unsettled",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeaseView {
    pub generation: u64,
    pub state: LeaseState,
    pub heartbeat_deadline_ms: u64,
}

/// The next lease state, or why the event is refused. `heartbeat_ms` is the
/// heartbeat interval the deadline extends by.
pub fn apply(
    lease: LeaseView,
    event: LeaseEvent,
    heartbeat_ms: u64,
) -> Result<LeaseView, LeaseError> {
    if !lease.state.holds_reservation() {
        return Err(LeaseError::Ended);
    }
    let fenced = |generation: u64| {
        if generation == lease.generation {
            Ok(())
        } else {
            Err(LeaseError::StaleGeneration)
        }
    };
    let next = |state: LeaseState, deadline: u64| LeaseView {
        state,
        heartbeat_deadline_ms: deadline,
        ..lease
    };
    match event {
        LeaseEvent::Heartbeat { generation, now_ms } => {
            fenced(generation)?;
            if lease.state != LeaseState::Active {
                return Err(LeaseError::NotActive);
            }
            Ok(next(
                LeaseState::Active,
                now_ms.saturating_add(heartbeat_ms),
            ))
        }
        LeaseEvent::HeartbeatMissed { now_ms } => {
            if now_ms <= lease.heartbeat_deadline_ms {
                return Err(LeaseError::HeartbeatNotMissed);
            }
            Ok(next(LeaseState::Quarantined, lease.heartbeat_deadline_ms))
        }
        LeaseEvent::AttemptSettled { generation } => {
            fenced(generation)?;
            Ok(next(LeaseState::Released, lease.heartbeat_deadline_ms))
        }
        LeaseEvent::Reconciled {
            generation,
            process_gone,
        } => {
            fenced(generation)?;
            if lease.state != LeaseState::Quarantined {
                return Err(LeaseError::NotActive);
            }
            let state = if process_gone {
                LeaseState::Released
            } else {
                LeaseState::Quarantined
            };
            Ok(next(state, lease.heartbeat_deadline_ms))
        }
        LeaseEvent::Revoke {
            generation,
            attempt_settled,
        } => {
            fenced(generation)?;
            if !attempt_settled {
                return Err(LeaseError::AttemptUnsettled);
            }
            Ok(next(LeaseState::Revoked, lease.heartbeat_deadline_ms))
        }
    }
}

/// A completion report's generation must equal the lease's, and the lease
/// must still hold its reservation (an active or quarantined lease whose
/// attempt is now settling).
pub fn check_fence(lease: LeaseView, claimed_generation: u64) -> Result<(), LeaseError> {
    if !lease.state.holds_reservation() {
        return Err(LeaseError::Ended);
    }
    if claimed_generation != lease.generation {
        return Err(LeaseError::StaleGeneration);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACTIVE: LeaseView = LeaseView {
        generation: 4,
        state: LeaseState::Active,
        heartbeat_deadline_ms: 1_000,
    };

    #[test]
    fn missed_heartbeat_quarantines_and_never_releases() {
        assert_eq!(
            apply(ACTIVE, LeaseEvent::HeartbeatMissed { now_ms: 900 }, 500),
            Err(LeaseError::HeartbeatNotMissed)
        );
        let quarantined =
            apply(ACTIVE, LeaseEvent::HeartbeatMissed { now_ms: 1_001 }, 500).expect("quarantine");
        assert_eq!(quarantined.state, LeaseState::Quarantined);
        assert!(quarantined.state.holds_reservation());
        // A heartbeat after quarantine cannot silently restore ownership.
        assert_eq!(
            apply(
                quarantined,
                LeaseEvent::Heartbeat {
                    generation: 4,
                    now_ms: 1_100
                },
                500
            ),
            Err(LeaseError::NotActive)
        );
        // Reconciliation that cannot prove the process gone keeps it held.
        let still = apply(
            quarantined,
            LeaseEvent::Reconciled {
                generation: 4,
                process_gone: false,
            },
            500,
        )
        .expect("reconcile");
        assert_eq!(still.state, LeaseState::Quarantined);
        let released = apply(
            still,
            LeaseEvent::Reconciled {
                generation: 4,
                process_gone: true,
            },
            500,
        )
        .expect("reconcile");
        assert_eq!(released.state, LeaseState::Released);
        assert!(!released.state.holds_reservation());
    }

    #[test]
    fn stale_generations_are_fenced_everywhere() {
        for event in [
            LeaseEvent::Heartbeat {
                generation: 3,
                now_ms: 10,
            },
            LeaseEvent::AttemptSettled { generation: 3 },
            LeaseEvent::Revoke {
                generation: 5,
                attempt_settled: true,
            },
        ] {
            assert_eq!(apply(ACTIVE, event, 500), Err(LeaseError::StaleGeneration));
        }
        assert_eq!(check_fence(ACTIVE, 3), Err(LeaseError::StaleGeneration));
        assert_eq!(check_fence(ACTIVE, 4), Ok(()));
        let released =
            apply(ACTIVE, LeaseEvent::AttemptSettled { generation: 4 }, 500).expect("settle");
        assert_eq!(check_fence(released, 4), Err(LeaseError::Ended));
        assert_eq!(
            apply(released, LeaseEvent::AttemptSettled { generation: 4 }, 500),
            Err(LeaseError::Ended)
        );
    }

    #[test]
    fn revocation_requires_a_settled_attempt() {
        assert_eq!(
            apply(
                ACTIVE,
                LeaseEvent::Revoke {
                    generation: 4,
                    attempt_settled: false
                },
                500
            ),
            Err(LeaseError::AttemptUnsettled)
        );
        let heartbeat = apply(
            ACTIVE,
            LeaseEvent::Heartbeat {
                generation: 4,
                now_ms: 2_000,
            },
            500,
        )
        .expect("beat");
        assert_eq!(heartbeat.heartbeat_deadline_ms, 2_500);
    }
}
