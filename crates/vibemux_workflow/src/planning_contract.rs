//! Transport-neutral planning-profile request identity and observation.
//!
//! This is the contract the Hub consumes for a planning request: a pinned
//! profile digest, a retained original request UUID, a deterministic request
//! key, a request content hash, and a content-free observation that never
//! returns request text. It performs no I/O and compiles no task spec; the
//! daemon planning profile supplies a precompiled template in the fixture
//! slice.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const PLANNING_CONTRACT_SCHEMA_VERSION: u32 = 1;
pub const MAX_PLANNING_REQUEST_BYTES: usize = 48 * 1024;
const PROFILE_DIGEST_DOMAIN: &[u8] = b"vibemux-planning-profile-v1\0";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlanningRequest {
    pub schema_version: u32,
    pub profile_id: String,
    pub profile_digest: String,
    pub request_id: Uuid,
    pub request_text: String,
    pub execute: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlanningQuery {
    pub schema_version: u32,
    pub profile_id: String,
    pub profile_digest: String,
    pub request_id: Uuid,
    pub request_sha256: String,
    pub execute: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlanningWorkflowRef {
    pub workflow_id: Uuid,
    pub start_contract: String,
    pub phase: String,
    pub version: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanningStatus {
    Prepared,
    NotObserved,
    Rejected,
    OutcomeUnknown,
}

impl PlanningStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::NotObserved => "not_observed",
            Self::Rejected => "rejected",
            Self::OutcomeUnknown => "outcome_unknown",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlanningObservation {
    pub schema_version: u32,
    pub request_id: Uuid,
    pub profile_id: String,
    pub profile_digest: String,
    pub request_sha256: String,
    pub execute: bool,
    pub status: PlanningStatus,
    pub retry_allowed: bool,
    pub workflow: Option<PlanningWorkflowRef>,
    /// The correlated live-execution observation. Always `None` in this
    /// revision: the R&D start-observation surface is not present, and a
    /// consumer must not treat planning alone as execution authority.
    #[serde(default)]
    pub execution: Option<serde_json::Value>,
}

#[must_use]
pub fn valid_profile_id(value: &str) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() || value.len() > 64 {
        return false;
    }
    bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        && !matches!(value, "constructor" | "prototype" | "__proto__")
}

#[must_use]
pub fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[must_use]
pub fn request_sha256(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The deterministic workflow request key for one retained planning request.
#[must_use]
pub fn request_key(request_id: Uuid) -> String {
    format!("profile_{request_id}")
}

/// The digest binds a profile to its workflow key, template, policy, and
/// execution permission. The daemon compares the operator's configured digest
/// with this value before any prepare.
#[must_use]
pub fn profile_digest(
    profile_id: &str,
    workflow_key: &str,
    task_spec_sha256: &str,
    policy_sha256: &str,
    execute_allowed: bool,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(PROFILE_DIGEST_DOMAIN);
    for field in [
        profile_id,
        workflow_key,
        task_spec_sha256,
        policy_sha256,
        if execute_allowed {
            "execute"
        } else {
            "prepare_only"
        },
    ] {
        hasher.update((field.len() as u64).to_be_bytes());
        hasher.update(field.as_bytes());
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

impl PlanningRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != PLANNING_CONTRACT_SCHEMA_VERSION
            || !valid_profile_id(&self.profile_id)
            || !valid_digest(&self.profile_digest)
            || self.request_id.is_nil()
        {
            return Err("planning_request_invalid");
        }
        if self.request_text.is_empty() || self.request_text.len() > MAX_PLANNING_REQUEST_BYTES {
            return Err("planning_request_text_invalid");
        }
        Ok(())
    }

    #[must_use]
    pub fn content_sha256(&self) -> String {
        request_sha256(&self.request_text)
    }
}

impl PlanningQuery {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != PLANNING_CONTRACT_SCHEMA_VERSION
            || !valid_profile_id(&self.profile_id)
            || !valid_digest(&self.profile_digest)
            || !valid_digest(&self.request_sha256)
            || self.request_id.is_nil()
        {
            return Err("planning_query_invalid");
        }
        Ok(())
    }
}

impl PlanningObservation {
    #[must_use]
    pub fn for_request(
        request: &PlanningRequest,
        status: PlanningStatus,
        workflow: Option<PlanningWorkflowRef>,
    ) -> Self {
        Self {
            schema_version: PLANNING_CONTRACT_SCHEMA_VERSION,
            request_id: request.request_id,
            profile_id: request.profile_id.clone(),
            profile_digest: request.profile_digest.clone(),
            request_sha256: request.content_sha256(),
            execute: request.execute,
            status,
            retry_allowed: false,
            workflow,
            execution: None,
        }
    }

    /// The observation must belong to this request and never mix statuses with
    /// an impossible workflow presence.
    pub fn validate_against(&self, request: &PlanningRequest) -> Result<(), &'static str> {
        if self.schema_version != PLANNING_CONTRACT_SCHEMA_VERSION
            || self.request_id != request.request_id
            || self.profile_id != request.profile_id
            || self.profile_digest != request.profile_digest
            || self.request_sha256 != request.content_sha256()
            || self.execute != request.execute
            || self.retry_allowed
        {
            return Err("planning_observation_mismatch");
        }
        match self.status {
            PlanningStatus::Prepared | PlanningStatus::OutcomeUnknown => {
                if self.workflow.is_none() {
                    return Err("planning_observation_missing_workflow");
                }
            }
            PlanningStatus::NotObserved | PlanningStatus::Rejected => {
                if self.workflow.is_some() {
                    return Err("planning_observation_unexpected_workflow");
                }
            }
        }
        if let Some(workflow) = &self.workflow
            && (!valid_digest(&workflow.start_contract) || workflow.version == 0)
        {
            return Err("planning_observation_workflow_invalid");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> PlanningRequest {
        PlanningRequest {
            schema_version: 1,
            profile_id: "fixture_store".to_string(),
            profile_digest: "a".repeat(64),
            request_id: Uuid::from_u128(0x42),
            request_text: "Ship the store.".to_string(),
            execute: false,
        }
    }

    #[test]
    fn request_key_is_stable_and_profile_scoped() {
        let id = Uuid::from_u128(0x42);
        assert_eq!(request_key(id), format!("profile_{id}"));
        assert!(request_key(id).starts_with("profile_"));
    }

    #[test]
    fn profile_digest_binds_every_field() {
        let base = profile_digest(
            "fixture_store",
            "taskboard_lite",
            &"b".repeat(64),
            &"c".repeat(64),
            true,
        );
        assert!(valid_digest(&base));
        for changed in [
            profile_digest(
                "other",
                "taskboard_lite",
                &"b".repeat(64),
                &"c".repeat(64),
                true,
            ),
            profile_digest(
                "fixture_store",
                "other",
                &"b".repeat(64),
                &"c".repeat(64),
                true,
            ),
            profile_digest(
                "fixture_store",
                "taskboard_lite",
                &"d".repeat(64),
                &"c".repeat(64),
                true,
            ),
            profile_digest(
                "fixture_store",
                "taskboard_lite",
                &"b".repeat(64),
                &"d".repeat(64),
                true,
            ),
            profile_digest(
                "fixture_store",
                "taskboard_lite",
                &"b".repeat(64),
                &"c".repeat(64),
                false,
            ),
        ] {
            assert_ne!(base, changed);
        }
    }

    #[test]
    fn request_validation_is_closed_and_bounded() {
        let valid = request();
        assert_eq!(valid.validate(), Ok(()));
        let mut wrong_schema = valid.clone();
        wrong_schema.schema_version = 2;
        assert_eq!(wrong_schema.validate(), Err("planning_request_invalid"));
        let mut wrong_profile = valid.clone();
        wrong_profile.profile_id = "Bad Profile".to_string();
        assert_eq!(wrong_profile.validate(), Err("planning_request_invalid"));
        let mut oversized = valid.clone();
        oversized.request_text = "x".repeat(MAX_PLANNING_REQUEST_BYTES + 1);
        assert_eq!(oversized.validate(), Err("planning_request_text_invalid"));
        assert!(
            serde_json::from_str::<PlanningRequest>(r#"{"schema_version":1,"profile_id":"a","profile_digest":"b","request_id":"00000000-0000-0000-0000-000000000001","request_text":"x","execute":false,"extra":1}"#).is_err()
        );
    }

    #[test]
    fn observation_wire_keys_match_the_hub_closed_record() {
        let request = request();
        let observation = PlanningObservation::for_request(
            &request,
            PlanningStatus::Prepared,
            Some(PlanningWorkflowRef {
                workflow_id: Uuid::from_u128(0x99),
                start_contract: "c".repeat(64),
                phase: "prepared".to_string(),
                version: 1,
            }),
        );
        let value = serde_json::to_value(&observation).expect("encode");
        let mut keys: Vec<&str> = value
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "execute",
                "execution",
                "profile_digest",
                "profile_id",
                "request_id",
                "request_sha256",
                "retry_allowed",
                "schema_version",
                "status",
                "workflow"
            ]
        );
        assert!(value["execution"].is_null());
    }

    #[test]
    fn observation_carries_no_request_text_and_matches_its_request() {
        let request = request();
        let observation = PlanningObservation::for_request(
            &request,
            PlanningStatus::Prepared,
            Some(PlanningWorkflowRef {
                workflow_id: Uuid::from_u128(0x99),
                start_contract: "c".repeat(64),
                phase: "prepared".to_string(),
                version: 1,
            }),
        );
        assert_eq!(observation.validate_against(&request), Ok(()));
        let encoded = serde_json::to_string(&observation).expect("encode");
        assert!(!encoded.contains("Ship the store."));
        assert!(!observation.retry_allowed);
        let not_observed =
            PlanningObservation::for_request(&request, PlanningStatus::NotObserved, None);
        assert_eq!(not_observed.validate_against(&request), Ok(()));
        let mut mismatched = observation.clone();
        mismatched.request_sha256 = "d".repeat(64);
        assert_eq!(
            mismatched.validate_against(&request),
            Err("planning_observation_mismatch")
        );
        let mut missing = observation;
        missing.workflow = None;
        assert_eq!(
            missing.validate_against(&request),
            Err("planning_observation_missing_workflow")
        );
    }
}
