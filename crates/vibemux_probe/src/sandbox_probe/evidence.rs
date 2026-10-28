//! Closed observations from the fixed probe and independent owner postconditions.

use serde::{Deserialize, Serialize};

pub const SUITE: &str = "offline_mxc_native_v1";
pub const SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckId {
    WorkspaceRead,
    WorkspaceWrite,
    ScratchWrite,
    ProtectedRead,
    ProtectedWrite,
    SealedRead,
    OutsideRead,
    OutsideWrite,
    LinkRead,
    LinkWrite,
    ChildOutsideWrite,
    LoopbackConnect,
}

pub const CHECKS: [CheckId; 12] = [
    CheckId::WorkspaceRead,
    CheckId::WorkspaceWrite,
    CheckId::ScratchWrite,
    CheckId::ProtectedRead,
    CheckId::ProtectedWrite,
    CheckId::SealedRead,
    CheckId::OutsideRead,
    CheckId::OutsideWrite,
    CheckId::LinkRead,
    CheckId::LinkWrite,
    CheckId::ChildOutsideWrite,
    CheckId::LoopbackConnect,
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Observation {
    Allowed,
    PermissionDenied,
    UnexpectedError,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CheckObservation {
    pub check: CheckId,
    pub observation: Observation,
    pub os_code: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureReport {
    pub schema_version: u16,
    pub nonce: String,
    pub checks: Vec<CheckObservation>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct IndependentEvidence {
    pub fixture_preflight: bool,
    pub path_aliases_rejected: bool,
    pub listener_challenge: bool,
    pub workspace_output: bool,
    pub scratch_output: bool,
    pub child_started: bool,
    pub protected_unchanged: bool,
    pub sealed_unchanged: bool,
    pub outside_unchanged: bool,
    pub forbidden_outputs_absent: bool,
    pub loopback_accepts: u64,
    pub process_exited: bool,
    pub execution_completed: bool,
    pub process_tree_empty: bool,
    pub output_complete: bool,
    pub listener_stopped: bool,
    pub fixture_removed: bool,
}

pub fn validate_evidence(
    report: &FixtureReport,
    nonce: &str,
    independent: &IndependentEvidence,
) -> Result<(), &'static str> {
    if report.schema_version != SCHEMA_VERSION || report.nonce != nonce {
        return Err("sandbox_evidence_binding_invalid");
    }
    if report.checks.len() != CHECKS.len() {
        return Err("sandbox_evidence_checks_incomplete");
    }
    for (index, (record, expected)) in report.checks.iter().zip(CHECKS).enumerate() {
        if record.check != expected {
            return Err("sandbox_evidence_checks_invalid");
        }
        let valid = if index < 4 {
            record.observation == Observation::Allowed && record.os_code.is_none()
        } else {
            let code = if expected == CheckId::LoopbackConnect {
                10013
            } else {
                5
            };
            record.observation == Observation::PermissionDenied && record.os_code == Some(code)
        };
        if !valid {
            return Err("sandbox_observation_failed");
        }
    }
    if !independent.fixture_preflight
        || !independent.path_aliases_rejected
        || !independent.listener_challenge
        || !independent.workspace_output
        || !independent.scratch_output
        || !independent.child_started
        || !independent.protected_unchanged
        || !independent.sealed_unchanged
        || !independent.outside_unchanged
        || !independent.forbidden_outputs_absent
        || independent.loopback_accepts != 0
        || !independent.process_exited
        || !independent.execution_completed
        || !independent.process_tree_empty
        || !independent.output_complete
        || !independent.listener_stopped
        || !independent.fixture_removed
    {
        return Err("sandbox_independent_evidence_failed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> (FixtureReport, IndependentEvidence) {
        let checks = CHECKS
            .into_iter()
            .enumerate()
            .map(|(index, check)| CheckObservation {
                check,
                observation: if index < 4 {
                    Observation::Allowed
                } else {
                    Observation::PermissionDenied
                },
                os_code: if index < 4 {
                    None
                } else if check == CheckId::LoopbackConnect {
                    Some(10013)
                } else {
                    Some(5)
                },
            })
            .collect();
        (
            FixtureReport {
                schema_version: 1,
                nonce: "run_nonce".into(),
                checks,
            },
            IndependentEvidence {
                fixture_preflight: true,
                path_aliases_rejected: true,
                listener_challenge: true,
                workspace_output: true,
                scratch_output: true,
                child_started: true,
                protected_unchanged: true,
                sealed_unchanged: true,
                outside_unchanged: true,
                forbidden_outputs_absent: true,
                loopback_accepts: 0,
                process_exited: true,
                execution_completed: true,
                process_tree_empty: true,
                output_complete: true,
                listener_stopped: true,
                fixture_removed: true,
            },
        )
    }

    #[test]
    fn complete_observations_and_independent_effects_pass() {
        let (report, independent) = valid();
        assert_eq!(
            validate_evidence(&report, "run_nonce", &independent),
            Ok(())
        );
    }

    #[test]
    fn missing_duplicate_stale_and_future_reports_fail() {
        for variant in 0..5 {
            let (mut report, independent) = valid();
            match variant {
                0 => {
                    report.checks.pop();
                }
                1 => report.checks[1].check = report.checks[0].check,
                2 => report.nonce = "previous_run".into(),
                3 => report.schema_version = 2,
                _ => report.checks.swap(1, 2),
            }
            assert!(validate_evidence(&report, "run_nonce", &independent).is_err());
        }
    }

    #[test]
    fn missing_fixture_startup_failure_and_timeout_are_not_denials() {
        for observation in [Observation::Allowed, Observation::UnexpectedError] {
            let (mut report, independent) = valid();
            report.checks[6].observation = observation;
            assert!(validate_evidence(&report, "run_nonce", &independent).is_err());
        }
        for code in [None, Some(2), Some(10060), Some(10061)] {
            let (mut report, independent) = valid();
            report.checks[11].os_code = code;
            assert!(validate_evidence(&report, "run_nonce", &independent).is_err());
        }
    }

    #[test]
    fn child_claim_cannot_override_parent_postconditions() {
        for variant in 0..17 {
            let (report, mut independent) = valid();
            match variant {
                0 => independent.fixture_preflight = false,
                1 => independent.listener_challenge = false,
                2 => independent.workspace_output = false,
                3 => independent.scratch_output = false,
                4 => independent.child_started = false,
                5 => independent.protected_unchanged = false,
                6 => independent.sealed_unchanged = false,
                7 => independent.outside_unchanged = false,
                8 => independent.forbidden_outputs_absent = false,
                9 => independent.loopback_accepts = 1,
                10 => independent.process_exited = false,
                11 => independent.process_tree_empty = false,
                12 => independent.output_complete = false,
                13 => independent.listener_stopped = false,
                14 => independent.fixture_removed = false,
                15 => independent.execution_completed = false,
                _ => independent.path_aliases_rejected = false,
            }
            assert!(validate_evidence(&report, "run_nonce", &independent).is_err());
        }
    }

    #[test]
    fn provider_or_extra_fields_cannot_become_probe_evidence() {
        let (report, _) = valid();
        let mut value = serde_json::to_value(report).unwrap();
        value["passed"] = true.into();
        assert!(serde_json::from_value::<FixtureReport>(value).is_err());
    }
}
