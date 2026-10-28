//! Operator-owned planning profiles and content-free planning observations.
//!
//! Profiles are pinned configuration. A request supplies source text and an exact
//! profile binding, never worker slots, policy, verifier arguments or host paths.
//! Validation and hashing here are pure; service admission owns every effect.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;
use vibemux_workflow::{
    Sha256Digest, SpecIdentifier,
    canonical_json::canonical_digest,
    gates::WorkflowPhase,
    gateway::{LiveAdmission, RouteRole},
    identifiers::is_relative_path,
    policy::OperatorPolicy,
    task_spec::WorkflowMode,
};

use super::{error::WorkflowError, service::StartObservation, settings::WorkflowConfig};

pub const PLANNING_SCHEMA_VERSION: u32 = 1;
pub const MAX_PLANNING_PROFILES: usize = 16;
pub const MAX_PLANNING_TASKS: usize = 8;
pub const MAX_PLANNING_REQUEST_BYTES: usize = 16 * 1024;
const MAX_WORKER_SLOTS: usize = 8;
const MAX_INTEGRATION_SUITES: usize = 16;
const MAX_SHARED_CONTRACTS: usize = 8;
const MAX_POLICY_BYTES: usize = 8 * 1024;
const PROFILE_DIGEST_DOMAIN: &str = "vibemux.workflow.planning_profile.v1";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlanningProfile {
    pub profile_id: SpecIdentifier,
    pub workflow_key: SpecIdentifier,
    pub mode: WorkflowMode,
    pub worker_slots: Vec<SpecIdentifier>,
    pub reviewer_slot: SpecIdentifier,
    pub integration_suites: Vec<SpecIdentifier>,
    pub shared_contract_paths: BTreeMap<SpecIdentifier, String>,
    pub policy: OperatorPolicy,
    pub max_tasks: usize,
    pub allow_execution: bool,
}

impl PlanningProfile {
    pub fn validate(&self, config: &WorkflowConfig) -> Result<(), WorkflowError> {
        if self.worker_slots.is_empty()
            || self.worker_slots.len() > MAX_WORKER_SLOTS
            || self.worker_slots.iter().collect::<BTreeSet<_>>().len() != self.worker_slots.len()
            || self.worker_slots.contains(&self.reviewer_slot)
            || self.worker_slots.iter().any(|worker| {
                !config.slots.iter().any(|slot| {
                    &slot.slot_id == worker
                        && matches!(slot.route_role, RouteRole::WorkerA | RouteRole::WorkerB)
                })
            })
            || !config
                .slots
                .iter()
                .any(|slot| slot.slot_id == self.reviewer_slot && slot.is_reviewer())
            || self.integration_suites.is_empty()
            || self.integration_suites.len() > MAX_INTEGRATION_SUITES
            || self
                .integration_suites
                .iter()
                .collect::<BTreeSet<_>>()
                .len()
                != self.integration_suites.len()
            || self.integration_suites.iter().any(|suite| {
                !config.verifier.suites.contains_key(suite) || !self.policy.declares_suite(suite)
            })
            || self.shared_contract_paths.len() > MAX_SHARED_CONTRACTS
            || self
                .shared_contract_paths
                .values()
                .any(|path| !is_relative_path(path))
            || !(1..=MAX_PLANNING_TASKS).contains(&self.max_tasks)
        {
            return Err(WorkflowError::ConfigInvalid);
        }
        match self.mode {
            WorkflowMode::Cooperate if self.max_tasks <= self.worker_slots.len() => {}
            WorkflowMode::Compare if self.max_tasks == 1 && self.worker_slots.len() >= 2 => {}
            _ => return Err(WorkflowError::ConfigInvalid),
        }
        self.policy
            .validate()
            .map_err(|_| WorkflowError::PolicyInvalid)?;
        let policy_bytes =
            serde_json::to_vec(&self.policy).map_err(|_| WorkflowError::PolicyInvalid)?;
        if policy_bytes.len() > MAX_POLICY_BYTES {
            return Err(WorkflowError::PolicyInvalid);
        }
        let gateway = config
            .gateway
            .as_ref()
            .ok_or(WorkflowError::ConfigInvalid)?;
        gateway
            .validate()
            .map_err(|_| WorkflowError::ConfigInvalid)?;
        if gateway.live_admission != LiveAdmission::BoundReceiptV2
            || gateway.binding(RouteRole::Compiler).is_none()
        {
            return Err(WorkflowError::ConfigInvalid);
        }
        Ok(())
    }

    /// The contract consumes both the complete profile and the pinned configuration.
    pub fn digest(&self, config_digest: Sha256Digest) -> Result<Sha256Digest, WorkflowError> {
        canonical_digest(
            PROFILE_DIGEST_DOMAIN,
            &json!({
                "profile": self,
                "config_digest": config_digest,
            }),
        )
        .map_err(|_| WorkflowError::Internal)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlanningRequest {
    pub schema_version: u32,
    pub profile_id: SpecIdentifier,
    pub profile_digest: Sha256Digest,
    pub request_id: Uuid,
    pub request_text: String,
    pub execute: bool,
}

impl PlanningRequest {
    pub fn validate(&self) -> Result<(), WorkflowError> {
        if self.schema_version != PLANNING_SCHEMA_VERSION
            || self.request_id.is_nil()
            || self.request_text.trim().is_empty()
            || self.request_text.len() > MAX_PLANNING_REQUEST_BYTES
            || self
                .request_text
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\t' | '\n' | '\r'))
        {
            return Err(WorkflowError::RequestInvalid);
        }
        Ok(())
    }

    /// Preserve the exact original UTF-8 bytes; no whitespace or line-ending normalization.
    pub fn query(&self) -> Result<PlanningQuery, WorkflowError> {
        self.validate()?;
        Ok(PlanningQuery {
            schema_version: PLANNING_SCHEMA_VERSION,
            profile_id: self.profile_id.clone(),
            profile_digest: self.profile_digest,
            request_id: self.request_id,
            request_sha256: Sha256Digest::of(self.request_text.as_bytes()),
            execute: self.execute,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlanningQuery {
    pub schema_version: u32,
    pub profile_id: SpecIdentifier,
    pub profile_digest: Sha256Digest,
    pub request_id: Uuid,
    pub request_sha256: Sha256Digest,
    pub execute: bool,
}

impl PlanningQuery {
    pub fn validate(&self) -> Result<(), WorkflowError> {
        if self.schema_version != PLANNING_SCHEMA_VERSION || self.request_id.is_nil() {
            return Err(WorkflowError::RequestInvalid);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanningStatus {
    Prepared,
    NotObserved,
    Rejected,
    OutcomeUnknown,
}

#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedWorkflow {
    pub workflow_id: Uuid,
    pub start_contract: Sha256Digest,
    pub phase: WorkflowPhase,
    pub version: u64,
}

/// Metadata only. The service decides outcome and retry authority from durable state.
#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlanningObservation {
    pub schema_version: u32,
    pub request_id: Uuid,
    pub profile_id: SpecIdentifier,
    pub profile_digest: Sha256Digest,
    pub request_sha256: Sha256Digest,
    pub execute: bool,
    pub status: PlanningStatus,
    pub retry_allowed: bool,
    pub workflow: Option<PlannedWorkflow>,
    pub execution: Option<StartObservation>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn profile_json() -> Value {
        json!({
            "profile_id":"development", "workflow_key":"feature", "mode":"cooperate",
            "worker_slots":["slot_a","slot_b"], "reviewer_slot":"review",
            "integration_suites":["api"], "shared_contract_paths":{"api_contract":"docs/api.md"},
            "max_tasks":2, "allow_execution":false,
            "policy":{
                "schema_version":1, "policy_version":"policy_v1",
                "writable_roots":["src/**"], "protected_paths":["docs/**"],
                "allowed_tools":["read_files","search_files","edit_files"],
                "verifier_suites":[{"suite_id":"api","description":"Trusted API verifier"}],
                "budget_caps":{"max_turns":4,"max_repairs":1,"max_elapsed_seconds":600,"max_model_requests":10},
                "max_parallel_slots":2, "max_supervisor_decisions":8,"max_optimizer_candidates":0,
                "content_store_opt_in":false, "aag_only":true
            }
        })
    }

    fn profile() -> PlanningProfile {
        serde_json::from_value(profile_json()).expect("profile shape")
    }

    fn config_json() -> Value {
        json!({
            "schema_version":1, "evidence_class":"fixture", "git_executable":"fixture_git",
            "slots":[
                {"slot_id":"slot_a","harness":"claude","route_role":"worker_a"},
                {"slot_id":"slot_b","harness":"claude","route_role":"worker_b"},
                {"slot_id":"review","harness":"codex","route_role":"reviewer"}
            ],
            "verifier":{"directory":"fixture_verifier","executable":"fixture_tool",
                "suites":{"api":["{candidate}","{out}"]}, "timeout_ms":60000},
            "heartbeat_ms":30000,
            "candidate_limits":{"max_files":64,"max_file_bytes":262144,"max_total_bytes":1048576},
            "gateway":{"schema_version":1,"endpoint":"http://127.0.0.1:8989","credential_env":"AAG_TOKEN",
                "strict_routing":true,"live_admission":"bound_receipt_v2",
                "bindings":[
                    {"role":"compiler","model_alias":"alias:compiler","protocol":"openai_chat_completions","max_output_tokens":4096,"timeout_ms":60000},
                    {"role":"worker_a","model_alias":"alias:worker_a","protocol":"openai_chat_completions","max_output_tokens":4096,"timeout_ms":60000},
                    {"role":"worker_b","model_alias":"alias:worker_b","protocol":"openai_chat_completions","max_output_tokens":4096,"timeout_ms":60000},
                    {"role":"reviewer","model_alias":"alias:review","protocol":"openai_chat_completions","max_output_tokens":4096,"timeout_ms":60000}
                ]}
        })
    }

    fn workflow_config() -> WorkflowConfig {
        serde_json::from_value(config_json()).expect("config shape")
    }

    fn request() -> PlanningRequest {
        PlanningRequest {
            schema_version: 1,
            profile_id: SpecIdentifier::new("development").unwrap(),
            profile_digest: Sha256Digest::of(b"profile"),
            request_id: Uuid::from_u128(1),
            request_text: "Build the feature.\r\nPreserve existing API.\t\u{00e9}".into(),
            execute: false,
        }
    }

    #[test]
    fn profiles_validate_only_operator_owned_slot_suite_and_compiler_bindings() {
        let config = workflow_config();
        profile()
            .validate(&config)
            .expect("valid cooperate profile");
        for (pointer, value) in [
            ("/worker_slots", json!([])),
            ("/worker_slots", json!(["slot_a", "slot_a"])),
            ("/worker_slots", json!(["missing"])),
            ("/worker_slots", json!(["review"])),
            ("/reviewer_slot", json!("slot_a")),
            ("/reviewer_slot", json!("missing")),
            ("/integration_suites", json!([])),
            ("/integration_suites", json!(["api", "api"])),
            ("/integration_suites", json!(["missing"])),
        ] {
            let mut value_profile = profile_json();
            *value_profile.pointer_mut(pointer).unwrap() = value;
            let value_profile: PlanningProfile = serde_json::from_value(value_profile).unwrap();
            assert!(
                value_profile.validate(&config).is_err(),
                "accepted invalid {pointer}"
            );
        }
        let mut missing_suite = workflow_config();
        missing_suite.verifier.suites.clear();
        assert!(profile().validate(&missing_suite).is_err());
        let mut wrong_role = workflow_config();
        wrong_role.slots[0].route_role = RouteRole::Supervisor;
        assert!(profile().validate(&wrong_role).is_err());
    }

    #[test]
    fn compare_and_cooperate_modes_have_distinct_bounded_task_shapes() {
        let config = workflow_config();
        let mut value = profile();
        value.mode = WorkflowMode::Compare;
        value.max_tasks = 1;
        value
            .validate(&config)
            .expect("two competitors with one task");
        value.worker_slots.pop();
        assert!(value.validate(&config).is_err());
        value = profile();
        value.mode = WorkflowMode::Compare;
        assert!(value.validate(&config).is_err());
        value = profile();
        value.mode = WorkflowMode::Review;
        assert!(value.validate(&config).is_err());
        for max_tasks in [0, 3, MAX_PLANNING_TASKS + 1] {
            value = profile();
            value.max_tasks = max_tasks;
            assert!(value.validate(&config).is_err());
        }
        value = profile();
        value.worker_slots = (0..9)
            .map(|index| SpecIdentifier::new(format!("slot_{index}")).unwrap())
            .collect();
        assert!(value.validate(&config).is_err());
    }

    #[test]
    fn suite_membership_policy_validation_and_policy_size_remain_mandatory() {
        let config = workflow_config();
        let mut value = profile();
        value.policy.verifier_suites.clear();
        assert!(value.validate(&config).is_err());
        value = profile();
        value.policy.budget_caps.max_turns = 0;
        assert_eq!(value.validate(&config), Err(WorkflowError::PolicyInvalid));
        value = profile();
        value.policy.verifier_suites[0].description = "x".repeat(MAX_POLICY_BYTES);
        assert_eq!(value.validate(&config), Err(WorkflowError::PolicyInvalid));
        value = profile();
        value.integration_suites = (0..17)
            .map(|index| SpecIdentifier::new(format!("suite_{index}")).unwrap())
            .collect();
        assert!(value.validate(&config).is_err());
    }

    #[test]
    fn shared_contracts_are_bounded_relative_files_without_traversal_or_wildcards() {
        let config = workflow_config();
        for path in [
            "",
            "../api.md",
            "/api.md",
            "C:/api.md",
            "docs\\api.md",
            "docs//api.md",
            "docs/./api.md",
            "docs/**",
        ] {
            let mut value = profile();
            *value.shared_contract_paths.values_mut().next().unwrap() = path.into();
            assert!(value.validate(&config).is_err(), "accepted path {path}");
        }
        let mut value = profile();
        value.shared_contract_paths = (0..9)
            .map(|index| {
                (
                    SpecIdentifier::new(format!("contract_{index}")).unwrap(),
                    format!("docs/api_{index}.md"),
                )
            })
            .collect();
        assert!(value.validate(&config).is_err());
    }

    #[test]
    fn planning_even_without_execution_requires_bound_compiler_admission() {
        assert!(!profile().allow_execution);
        let mut config = workflow_config();
        config.gateway = None;
        assert!(profile().validate(&config).is_err());
        let mut config = config_json();
        config["gateway"]["live_admission"] = json!("disabled");
        assert!(
            profile()
                .validate(&serde_json::from_value(config).unwrap())
                .is_err()
        );
        let mut config = config_json();
        config["gateway"]["bindings"]
            .as_array_mut()
            .unwrap()
            .remove(0);
        assert!(
            profile()
                .validate(&serde_json::from_value(config).unwrap())
                .is_err()
        );
    }

    #[test]
    fn profile_digest_binds_every_option_and_the_exact_config_identity() {
        let value = profile();
        let config_digest = Sha256Digest::of(b"config");
        let digest = value.digest(config_digest).unwrap();
        assert_eq!(digest, value.clone().digest(config_digest).unwrap());
        assert_ne!(
            digest,
            value.digest(Sha256Digest::of(b"changed config")).unwrap()
        );
        let mut changed = value.clone();
        changed.allow_execution = true;
        assert_ne!(digest, changed.digest(config_digest).unwrap());
        changed = value.clone();
        changed.worker_slots.reverse();
        assert_ne!(digest, changed.digest(config_digest).unwrap());
        changed = value.clone();
        changed.policy.budget_caps.max_model_requests += 1;
        assert_ne!(digest, changed.digest(config_digest).unwrap());
        changed = value;
        changed
            .shared_contract_paths
            .values_mut()
            .next()
            .unwrap()
            .push_str(".new");
        assert_ne!(digest, changed.digest(config_digest).unwrap());
    }

    #[test]
    fn planning_request_hashes_exact_utf8_source_and_keeps_execution_in_query_identity() {
        let value = request();
        let query = value.query().unwrap();
        query.validate().unwrap();
        assert_eq!(
            query.request_sha256,
            Sha256Digest::of(value.request_text.as_bytes())
        );
        assert_eq!(query.profile_digest, value.profile_digest);
        assert_eq!(query.request_id, value.request_id);
        let mut changed = value.clone();
        changed.execute = true;
        let executed = changed.query().unwrap();
        assert_ne!(query, executed);
        assert_eq!(query.request_sha256, executed.request_sha256);
        for text in [
            value.request_text.replace("\r\n", "\n"),
            format!("{} ", value.request_text),
        ] {
            changed = value.clone();
            changed.request_text = text;
            assert_ne!(
                query.request_sha256,
                changed.query().unwrap().request_sha256
            );
        }
        let serialized = serde_json::to_value(&query).unwrap();
        assert_eq!(serialized.as_object().unwrap().len(), 6);
        assert!(serialized.get("request_text").is_none());
    }

    #[test]
    fn planning_requests_enforce_byte_bounds_controls_schema_and_nonnil_identity() {
        let mut value = request();
        value.request_text = "x".repeat(MAX_PLANNING_REQUEST_BYTES);
        value.validate().unwrap();
        value.request_text.push('x');
        assert!(value.validate().is_err());
        value = request();
        value.request_text = "\u{00e9}".repeat(MAX_PLANNING_REQUEST_BYTES / 2);
        value.validate().unwrap();
        value.request_text.push('\u{00e9}');
        assert!(value.validate().is_err());
        for text in [
            "",
            " \t\r\n",
            "nul\0data",
            "control\u{0001}",
            "delete\u{007f}",
            "control\u{0085}",
        ] {
            value = request();
            value.request_text = text.into();
            assert!(value.query().is_err());
        }
        value = request();
        value.schema_version = 2;
        assert!(value.validate().is_err());
        value = request();
        value.request_id = Uuid::nil();
        assert!(value.validate().is_err());
        let mut query = request().query().unwrap();
        query.request_id = Uuid::nil();
        assert!(query.validate().is_err());
        query = request().query().unwrap();
        query.schema_version = 2;
        assert!(query.validate().is_err());
    }

    #[test]
    fn closed_inputs_reject_model_supplied_policy_slots_paths_and_unsupported_fields() {
        let mut profile_value = profile_json();
        profile_value["command"] = json!("not permitted");
        assert!(serde_json::from_value::<PlanningProfile>(profile_value).is_err());
        for key in ["policy", "worker_slots", "project_root", "extra"] {
            let mut value = serde_json::to_value(request()).unwrap();
            value[key] = json!("not permitted");
            assert!(serde_json::from_value::<PlanningRequest>(value).is_err());
            let mut value = serde_json::to_value(request().query().unwrap()).unwrap();
            value[key] = json!("not permitted");
            assert!(serde_json::from_value::<PlanningQuery>(value).is_err());
        }
        let mut value = serde_json::to_value(request()).unwrap();
        value["profile_digest"] = json!("not a digest");
        assert!(serde_json::from_value::<PlanningRequest>(value).is_err());
    }

    #[test]
    fn observations_emit_only_fixed_metadata_and_explicit_unknown_execution() {
        let query = request().query().unwrap();
        let observation = PlanningObservation {
            schema_version: 1,
            request_id: query.request_id,
            profile_id: query.profile_id,
            profile_digest: query.profile_digest,
            request_sha256: query.request_sha256,
            execute: query.execute,
            status: PlanningStatus::NotObserved,
            retry_allowed: false,
            workflow: None,
            execution: None,
        };
        let value = serde_json::to_value(observation).unwrap();
        assert_eq!(value["status"], json!("not_observed"));
        assert_eq!(value["retry_allowed"], json!(false));
        assert_eq!(value["workflow"], Value::Null);
        assert_eq!(value["execution"], Value::Null);
        let fields: BTreeSet<_> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            fields,
            [
                "schema_version",
                "request_id",
                "profile_id",
                "profile_digest",
                "request_sha256",
                "execute",
                "status",
                "retry_allowed",
                "workflow",
                "execution"
            ]
            .into_iter()
            .collect()
        );
    }
}
