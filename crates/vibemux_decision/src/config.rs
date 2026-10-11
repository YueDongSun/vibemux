//! Operator decision config, `.vibemux/decision_config.json` (schema 1).
//!
//! It names the credential's environment variable, never the credential.
//! A missing file means decisions are not configured. The daemon pins the
//! digest of the file it loaded at start; edits need a restart.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::error::DecisionError;
use crate::identifiers::{Label, OptionId};
use crate::probability::Threshold;
use crate::question::is_valid_description;
use crate::rules::LocalRule;

pub const DECISION_CONFIG_SCHEMA_VERSION: u32 = 1;
pub const DECISION_CONFIG_FILE_NAME: &str = "decision_config.json";
pub const MAX_DECISION_CONFIG_BYTES: u64 = 64 * 1024;
pub const MAX_CONFIG_LABELS: usize = 64;
pub const MAX_LOCAL_RULES: usize = 128;
pub const MAX_CREDENTIAL_ENV_BYTES: usize = 128;
pub const MAX_CONFIG_MODEL_BYTES: usize = 64;

pub const DEFAULT_HARNESS_ROUTE_THRESHOLD: f64 = 0.9;
/// The request model; the provider documents `jev-latest`.
pub const DEFAULT_JEV_REQUEST_MODEL: &str = "jev-latest";
/// The only response model whose answers are accepted until re-pinned.
pub const DEFAULT_JEV_PINNED_MODEL: &str = "jev-1.13.0";
pub const DEFAULT_JEV_CREDENTIAL_ENV: &str = "TYPESAFE_API_KEY";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionConfig {
    pub schema_version: u32,
    #[serde(default)]
    pub thresholds: ThresholdConfig,
    /// The only labels a request may carry.
    #[serde(default)]
    pub labels: Vec<Label>,
    #[serde(default)]
    pub local_rules: Vec<LocalRule>,
    /// Keyed by harness command name or `escalate`; sent to the backend.
    #[serde(default)]
    pub option_descriptions: BTreeMap<OptionId, String>,
    #[serde(default)]
    pub jev: JevConfig,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ThresholdConfig {
    #[serde(default = "default_harness_route_threshold")]
    pub harness_route: f64,
}

impl Default for ThresholdConfig {
    fn default() -> Self {
        Self {
            harness_route: DEFAULT_HARNESS_ROUTE_THRESHOLD,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JevConfig {
    /// Off by default; each request must also allow the external call.
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_request_model")]
    pub request_model: String,
    #[serde(default = "default_pinned_model")]
    pub pinned_model: String,
    #[serde(default = "default_credential_env")]
    pub credential_env: String,
}

impl Default for JevConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            request_model: default_request_model(),
            pinned_model: default_pinned_model(),
            credential_env: default_credential_env(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Error, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionConfigError {
    #[error("decision config is not valid JSON for schema 1")]
    Malformed,
    #[error("decision config schema is unsupported")]
    SchemaUnsupported,
    #[error("a threshold is outside [0.5, 1.0]")]
    InvalidThreshold,
    #[error("labels are duplicated or too many")]
    InvalidLabels,
    #[error("a local rule names an unknown label or a non-harness option, or there are too many")]
    InvalidRule,
    #[error("an option description key or value is invalid")]
    InvalidDescription,
    #[error("a model id is empty, too long, or has unsupported characters")]
    InvalidModel,
    #[error("credential reference must be an environment variable name")]
    InvalidCredentialReference,
}

impl DecisionConfigError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Malformed => "decision_config_malformed",
            Self::SchemaUnsupported => "decision_config_schema_unsupported",
            Self::InvalidThreshold => "decision_config_invalid_threshold",
            Self::InvalidLabels => "decision_config_invalid_labels",
            Self::InvalidRule => "decision_config_invalid_rule",
            Self::InvalidDescription => "decision_config_invalid_description",
            Self::InvalidModel => "decision_config_invalid_model",
            Self::InvalidCredentialReference => "decision_config_invalid_credential_reference",
        }
    }
}

impl DecisionConfig {
    pub fn parse(bytes: &[u8]) -> Result<Self, DecisionConfigError> {
        let config: Self =
            serde_json::from_slice(bytes).map_err(|_| DecisionConfigError::Malformed)?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), DecisionConfigError> {
        if self.schema_version != DECISION_CONFIG_SCHEMA_VERSION {
            return Err(DecisionConfigError::SchemaUnsupported);
        }
        if Threshold::from_unit(self.thresholds.harness_route).is_none() {
            return Err(DecisionConfigError::InvalidThreshold);
        }
        let labels: BTreeSet<&Label> = self.labels.iter().collect();
        if labels.len() != self.labels.len() || self.labels.len() > MAX_CONFIG_LABELS {
            return Err(DecisionConfigError::InvalidLabels);
        }
        if self.local_rules.len() > MAX_LOCAL_RULES
            || self
                .local_rules
                .iter()
                .any(|rule| !labels.contains(&rule.label) || rule.harness.to_harness().is_none())
        {
            return Err(DecisionConfigError::InvalidRule);
        }
        for (option, description) in &self.option_descriptions {
            if (!option.is_escalate() && option.to_harness().is_none())
                || !is_valid_description(description)
            {
                return Err(DecisionConfigError::InvalidDescription);
            }
        }
        if !is_valid_model(&self.jev.request_model) || !is_valid_model(&self.jev.pinned_model) {
            return Err(DecisionConfigError::InvalidModel);
        }
        if !is_valid_credential_env(&self.jev.credential_env) {
            return Err(DecisionConfigError::InvalidCredentialReference);
        }
        Ok(())
    }

    /// The validated `harness_route` threshold.
    pub fn harness_route_threshold(&self) -> Result<Threshold, DecisionError> {
        Threshold::from_unit(self.thresholds.harness_route).ok_or(DecisionError::Internal)
    }

    /// Parses request labels and checks each against the allowlist.
    pub fn check_labels(&self, requested: &[String]) -> Result<Vec<Label>, DecisionError> {
        if requested.len() > crate::facts::MAX_REQUEST_LABELS {
            return Err(DecisionError::InvalidRequest);
        }
        requested
            .iter()
            .map(|value| {
                let label = Label::new(value.clone())?;
                if self.labels.contains(&label) {
                    Ok(label)
                } else {
                    Err(DecisionError::UnknownLabel)
                }
            })
            .collect()
    }
}

/// `[A-Z0-9_]{1,128}` without the reserved `VIBEMUX_` prefix.
#[must_use]
pub fn is_valid_credential_env(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_CREDENTIAL_ENV_BYTES
        && name
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        && !name.starts_with("VIBEMUX_")
}

/// `[a-z0-9._-]{1,64}` starting with a letter or digit.
#[must_use]
pub fn is_valid_model(model: &str) -> bool {
    model.len() <= MAX_CONFIG_MODEL_BYTES
        && model
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && model.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn default_harness_route_threshold() -> f64 {
    DEFAULT_HARNESS_ROUTE_THRESHOLD
}

fn default_request_model() -> String {
    DEFAULT_JEV_REQUEST_MODEL.to_string()
}

fn default_pinned_model() -> String {
    DEFAULT_JEV_PINNED_MODEL.to_string()
}

fn default_credential_env() -> String {
    DEFAULT_JEV_CREDENTIAL_ENV.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"{
        "schema_version": 1,
        "thresholds": {"harness_route": 0.85},
        "labels": ["refactor", "rust", "docs"],
        "local_rules": [
            {"label": "rust", "harness": "codex", "priority": 10},
            {"label": "docs", "harness": "claude", "priority": 5}
        ],
        "option_descriptions": {"codex": "Large Rust refactors", "escalate": "Ask the operator"},
        "jev": {"enabled": false}
    }"#;

    #[test]
    fn a_full_config_parses_and_fills_defaults() {
        let config = DecisionConfig::parse(FULL.as_bytes()).unwrap();
        assert_eq!(config.labels.len(), 3);
        assert_eq!(
            config.harness_route_threshold().unwrap().millionths().get(),
            850_000
        );
        assert_eq!(config.jev.request_model, DEFAULT_JEV_REQUEST_MODEL);
        assert_eq!(config.jev.pinned_model, DEFAULT_JEV_PINNED_MODEL);
        assert_eq!(config.jev.credential_env, DEFAULT_JEV_CREDENTIAL_ENV);
        assert!(!config.jev.enabled);
    }

    #[test]
    fn a_minimal_config_is_valid_and_disabled() {
        let config = DecisionConfig::parse(br#"{"schema_version":1}"#).unwrap();
        assert!(!config.jev.enabled);
        assert_eq!(
            config.harness_route_threshold().unwrap().millionths().get(),
            900_000
        );
    }

    #[test]
    fn rejection_table() {
        let cases = [
            (
                r#"{"schema_version":2}"#,
                DecisionConfigError::SchemaUnsupported,
            ),
            (
                r#"{"schema_version":1,"extra":true}"#,
                DecisionConfigError::Malformed,
            ),
            (
                r#"{"schema_version":1,"labels":["Rust"]}"#,
                DecisionConfigError::Malformed,
            ),
            (
                r#"{"schema_version":1,"thresholds":{"harness_route":0.4}}"#,
                DecisionConfigError::InvalidThreshold,
            ),
            (
                r#"{"schema_version":1,"thresholds":{"harness_route":1.2}}"#,
                DecisionConfigError::InvalidThreshold,
            ),
            (
                r#"{"schema_version":1,"labels":["rust","rust"]}"#,
                DecisionConfigError::InvalidLabels,
            ),
            (
                r#"{"schema_version":1,"labels":["rust"],"local_rules":[{"label":"docs","harness":"codex","priority":1}]}"#,
                DecisionConfigError::InvalidRule,
            ),
            (
                r#"{"schema_version":1,"labels":["rust"],"local_rules":[{"label":"rust","harness":"escalate","priority":1}]}"#,
                DecisionConfigError::InvalidRule,
            ),
            (
                r#"{"schema_version":1,"labels":["rust"],"local_rules":[{"label":"rust","harness":"open_code","priority":1}]}"#,
                DecisionConfigError::InvalidRule,
            ),
            (
                r#"{"schema_version":1,"option_descriptions":{"gpt":"x"}}"#,
                DecisionConfigError::InvalidDescription,
            ),
            (
                r#"{"schema_version":1,"option_descriptions":{"codex":""}}"#,
                DecisionConfigError::InvalidDescription,
            ),
            (
                r#"{"schema_version":1,"jev":{"pinned_model":"Jev 1"}}"#,
                DecisionConfigError::InvalidModel,
            ),
            (
                r#"{"schema_version":1,"jev":{"credential_env":"sk-live-abc"}}"#,
                DecisionConfigError::InvalidCredentialReference,
            ),
            (
                r#"{"schema_version":1,"jev":{"credential_env":"VIBEMUX_KEY"}}"#,
                DecisionConfigError::InvalidCredentialReference,
            ),
            (
                r#"{"schema_version":1,"jev":{"api_key":"x"}}"#,
                DecisionConfigError::Malformed,
            ),
        ];
        for (raw, expected) in cases {
            assert_eq!(
                DecisionConfig::parse(raw.as_bytes()),
                Err(expected),
                "{raw}"
            );
        }
    }

    #[test]
    fn request_labels_must_be_allowlisted() {
        let config = DecisionConfig::parse(FULL.as_bytes()).unwrap();
        assert_eq!(
            config
                .check_labels(&["rust".into(), "docs".into()])
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            config.check_labels(&["secret_project".into()]),
            Err(DecisionError::UnknownLabel)
        );
        assert_eq!(
            config.check_labels(&["Not A Label".into()]),
            Err(DecisionError::UnknownLabel)
        );
        let too_many: Vec<String> = (0..=crate::facts::MAX_REQUEST_LABELS)
            .map(|_| "rust".to_string())
            .collect();
        assert_eq!(
            config.check_labels(&too_many),
            Err(DecisionError::InvalidRequest)
        );
    }
}
