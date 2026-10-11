//! Option ids and operator labels: short lowercase snake_case identifiers.
//!
//! Harness option ids are `AgentKind::command_name()` values (`opencode`, not
//! the serde form `open_code`), so they match what operators type and what
//! the probe reports. Every question also carries [`ESCALATE_OPTION`].

use std::fmt;

use serde::{Deserialize, Serialize};
use vibemux_harness::AgentKind;

use crate::error::DecisionError;

/// The escape option every question carries: no option clearly fits.
pub const ESCALATE_OPTION: &str = "escalate";
pub const MAX_OPTION_ID_BYTES: usize = 64;
pub const MAX_LABEL_BYTES: usize = 32;

/// One allowed answer of a decision question.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct OptionId(String);

impl OptionId {
    pub fn new(value: impl Into<String>) -> Result<Self, DecisionError> {
        let value = value.into();
        if is_snake_identifier(&value, MAX_OPTION_ID_BYTES) {
            Ok(Self(value))
        } else {
            Err(DecisionError::InvalidRequest)
        }
    }

    #[must_use]
    pub fn escalate() -> Self {
        Self(ESCALATE_OPTION.to_string())
    }

    #[must_use]
    pub fn harness(harness: AgentKind) -> Self {
        Self(harness.command_name().to_string())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn is_escalate(&self) -> bool {
        self.0 == ESCALATE_OPTION
    }

    /// The harness this option names, if it names one.
    #[must_use]
    pub fn to_harness(&self) -> Option<AgentKind> {
        harness_from_command_name(&self.0)
    }
}

impl TryFrom<String> for OptionId {
    type Error = DecisionError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<OptionId> for String {
    fn from(value: OptionId) -> Self {
        value.0
    }
}

impl fmt::Display for OptionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// An operator label from the decision config's allowlist. Labels are the
/// only caller-chosen words a backend ever sees.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct Label(String);

impl Label {
    pub fn new(value: impl Into<String>) -> Result<Self, DecisionError> {
        let value = value.into();
        if is_snake_identifier(&value, MAX_LABEL_BYTES) {
            Ok(Self(value))
        } else {
            Err(DecisionError::UnknownLabel)
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Label {
    type Error = DecisionError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<Label> for String {
    fn from(value: Label) -> Self {
        value.0
    }
}

impl fmt::Display for Label {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Maps a command name (`opencode`) to its harness.
#[must_use]
pub fn harness_from_command_name(name: &str) -> Option<AgentKind> {
    AgentKind::all()
        .into_iter()
        .find(|harness| harness.command_name() == name)
}

/// `[a-z][a-z0-9_]*`, at most `max_bytes`, no trailing or doubled underscore.
fn is_snake_identifier(value: &str, max_bytes: usize) -> bool {
    let bytes = value.as_bytes();
    let Some(first) = bytes.first() else {
        return false;
    };
    bytes.len() <= max_bytes
        && first.is_ascii_lowercase()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
        && !value.ends_with('_')
        && !value.contains("__")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_harness_command_name_is_a_valid_option_id() {
        for harness in AgentKind::all() {
            let option = OptionId::harness(harness);
            assert_eq!(OptionId::new(option.as_str()), Ok(option.clone()));
            assert_eq!(option.to_harness(), Some(harness));
        }
        assert_eq!(
            harness_from_command_name("opencode"),
            Some(AgentKind::OpenCode)
        );
        assert_eq!(harness_from_command_name("open_code"), None);
    }

    #[test]
    fn escalate_is_an_option_but_not_a_harness() {
        let escalate = OptionId::escalate();
        assert!(escalate.is_escalate());
        assert_eq!(escalate.to_harness(), None);
    }

    #[test]
    fn identifiers_reject_non_snake_case() {
        for bad in [
            "", "Rust", "1st", "_x", "x_", "a__b", "a-b", "a b", "ü", "a.b",
        ] {
            assert!(OptionId::new(bad).is_err(), "{bad:?} must be rejected");
            assert!(Label::new(bad).is_err(), "{bad:?} must be rejected");
        }
        assert!(Label::new("a".repeat(MAX_LABEL_BYTES)).is_ok());
        assert!(Label::new("a".repeat(MAX_LABEL_BYTES + 1)).is_err());
        assert!(OptionId::new("a".repeat(MAX_OPTION_ID_BYTES + 1)).is_err());
    }

    #[test]
    fn identifiers_round_trip_through_json() {
        let label: Label = serde_json::from_str("\"refactor_rust\"").unwrap();
        assert_eq!(label.as_str(), "refactor_rust");
        assert!(serde_json::from_str::<Label>("\"Refactor\"").is_err());
        assert_eq!(serde_json::to_string(&label).unwrap(), "\"refactor_rust\"");
    }
}
