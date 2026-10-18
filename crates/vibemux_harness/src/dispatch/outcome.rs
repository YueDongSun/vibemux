//! The single rule that turns exchange, protocol, and process evidence into
//! an attempt outcome (ADR 029 §3).
//!
//! `Completed` requires all of: the exchange ended without error, the
//! protocol reported correlated `Completed` evidence, the process exited with
//! code 0, and it was not force-terminated. Reaching EOF without terminal
//! evidence is `Unverified`, never success.

use serde::{Deserialize, Serialize};

use super::{DispatchError, SessionTerminal};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptOutcome {
    Completed,
    Failed,
    Unverified,
    Cancelled,
    /// Initialize-only probe succeeded; never persisted as a dispatch.
    Probed,
}

impl AttemptOutcome {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Unverified => "unverified",
            Self::Cancelled => "cancelled",
            Self::Probed => "probed",
        }
    }
}

/// How the vendor process ended, as observed by the daemon.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessExit {
    /// `None` when the platform reports no code (for example, a POSIX signal).
    pub exit_code: Option<i32>,
    /// The daemon killed the process tree after the grace period.
    pub forced_termination: bool,
}

impl ProcessExit {
    #[must_use]
    pub const fn is_clean(&self) -> bool {
        matches!(self.exit_code, Some(0)) && !self.forced_termination
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OutcomeDecision {
    pub outcome: AttemptOutcome,
    pub error_code: Option<DispatchError>,
}

/// Decides the outcome. `exchange` is the result of the daemon's read/write
/// loop; [`DispatchError::Cancelled`] there means cancellation ended the
/// exchange before the protocol confirmed it.
#[must_use]
pub fn decide_outcome(
    exchange: Result<(), DispatchError>,
    terminal: Option<SessionTerminal>,
    exit: ProcessExit,
) -> OutcomeDecision {
    let decision = |outcome, error_code| OutcomeDecision {
        outcome,
        error_code,
    };
    if let Err(error) = exchange {
        let outcome = if error == DispatchError::Cancelled {
            AttemptOutcome::Cancelled
        } else {
            AttemptOutcome::Failed
        };
        return decision(outcome, Some(error));
    }
    if !exit.is_clean() {
        return decision(AttemptOutcome::Failed, Some(DispatchError::ProcessFailed));
    }
    match terminal {
        None => decision(
            AttemptOutcome::Unverified,
            Some(DispatchError::EofWithoutTerminal),
        ),
        Some(SessionTerminal::Completed) => decision(AttemptOutcome::Completed, None),
        Some(SessionTerminal::Failed) => {
            decision(AttemptOutcome::Failed, Some(DispatchError::TurnFailed))
        }
        Some(SessionTerminal::Cancelled) => decision(AttemptOutcome::Cancelled, None),
        Some(SessionTerminal::Probed) => decision(AttemptOutcome::Probed, None),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const OUTCOME_NAMES: [(AttemptOutcome, &str); 5] = [
        (AttemptOutcome::Completed, "completed"),
        (AttemptOutcome::Failed, "failed"),
        (AttemptOutcome::Unverified, "unverified"),
        (AttemptOutcome::Cancelled, "cancelled"),
        (AttemptOutcome::Probed, "probed"),
    ];

    #[test]
    fn outcome_names_match_their_serde_form() {
        for (outcome, name) in OUTCOME_NAMES {
            assert_eq!(outcome.as_str(), name);
            assert_eq!(
                serde_json::to_value(outcome).expect("serialize"),
                json!(name)
            );
            let parsed: AttemptOutcome = serde_json::from_value(json!(name)).expect("deserialize");
            assert_eq!(parsed, outcome);
        }
        assert!(serde_json::from_value::<AttemptOutcome>(json!("Completed")).is_err());
    }

    #[test]
    fn exits_and_decisions_round_trip_and_refuse_unknown_fields() {
        let exit = ProcessExit {
            exit_code: Some(0),
            forced_termination: false,
        };
        let mut exit_value = serde_json::to_value(exit).expect("serialize exit");
        let parsed: ProcessExit =
            serde_json::from_value(exit_value.clone()).expect("deserialize exit");
        assert_eq!(parsed, exit);
        exit_value["signal"] = json!(9);
        assert!(serde_json::from_value::<ProcessExit>(exit_value).is_err());

        let decision = OutcomeDecision {
            outcome: AttemptOutcome::Unverified,
            error_code: Some(DispatchError::EofWithoutTerminal),
        };
        let mut decision_value = serde_json::to_value(decision).expect("serialize decision");
        let parsed: OutcomeDecision =
            serde_json::from_value(decision_value.clone()).expect("deserialize decision");
        assert_eq!(parsed, decision);
        decision_value["detail"] = json!("synthetic");
        assert!(serde_json::from_value::<OutcomeDecision>(decision_value).is_err());
    }
}
