//! Loads and pins the operator decision config (ADR 034 §5). The daemon reads
//! `<state dir>/decision_config.json` once at startup; a changed file takes
//! effect only after a restart, and every decision records the digest of the
//! config it was made under. A missing file leaves decisions unconfigured; a
//! symlink or other non-regular file is refused rather than followed.

use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

use thiserror::Error;
use vibemux_decision::{
    DecisionConfig, DecisionConfigError, DecisionError, Sha256Digest,
    config::{DECISION_CONFIG_FILE_NAME, MAX_DECISION_CONFIG_BYTES},
};

/// A validated config and the digest of its exact bytes.
#[derive(Debug)]
pub struct LoadedDecisionConfig {
    pub config: DecisionConfig,
    pub digest: Sha256Digest,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum DecisionConfigLoadError {
    #[error("{0}")]
    Decision(DecisionError),
    #[error("{0}")]
    Config(DecisionConfigError),
    #[error("decision config is not a readable regular file")]
    Unreadable,
    #[error("decision config exceeds its size limit")]
    TooLarge,
}

impl DecisionConfigLoadError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Decision(error) => error.code(),
            Self::Config(error) => error.code(),
            Self::Unreadable => "decision_config_unreadable",
            Self::TooLarge => "decision_config_too_large",
        }
    }
}

/// The config path inside the daemon state directory.
#[must_use]
pub fn decision_config_path(state_dir: &Path) -> std::path::PathBuf {
    state_dir.join(DECISION_CONFIG_FILE_NAME)
}

pub fn load_decision_config(
    config_path: &Path,
) -> Result<LoadedDecisionConfig, DecisionConfigLoadError> {
    let metadata = match std::fs::symlink_metadata(config_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(DecisionConfigLoadError::Decision(
                DecisionError::NotConfigured,
            ));
        }
        Err(_) => return Err(DecisionConfigLoadError::Unreadable),
    };
    if !metadata.is_file() {
        return Err(DecisionConfigLoadError::Unreadable);
    }
    if metadata.len() > MAX_DECISION_CONFIG_BYTES {
        return Err(DecisionConfigLoadError::TooLarge);
    }
    let file = File::open(config_path).map_err(|_| DecisionConfigLoadError::Unreadable)?;
    let mut bytes = Vec::new();
    file.take(MAX_DECISION_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| DecisionConfigLoadError::Unreadable)?;
    if bytes.len() as u64 > MAX_DECISION_CONFIG_BYTES {
        return Err(DecisionConfigLoadError::TooLarge);
    }
    let config = DecisionConfig::parse(&bytes).map_err(DecisionConfigLoadError::Config)?;
    Ok(LoadedDecisionConfig {
        config,
        digest: Sha256Digest::of(&bytes),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_oversized_and_invalid_configs_have_fixed_codes() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = decision_config_path(directory.path());
        assert_eq!(
            load_decision_config(&path).unwrap_err().code(),
            "decision_not_configured"
        );
        std::fs::write(&path, vec![b' '; MAX_DECISION_CONFIG_BYTES as usize + 1])
            .expect("write oversized");
        assert_eq!(
            load_decision_config(&path).unwrap_err().code(),
            "decision_config_too_large"
        );
        std::fs::write(&path, br#"{"schema_version":1,"surprise":true}"#).expect("write");
        assert_eq!(
            load_decision_config(&path).unwrap_err().code(),
            "decision_config_malformed"
        );
        std::fs::remove_file(&path).expect("remove");
        std::fs::create_dir(&path).expect("directory in its place");
        assert_eq!(
            load_decision_config(&path).unwrap_err().code(),
            "decision_config_unreadable"
        );
    }

    #[test]
    fn a_valid_config_is_pinned_by_the_digest_of_its_bytes() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = decision_config_path(directory.path());
        let bytes = br#"{"schema_version":1,"labels":["rust"]}"#;
        std::fs::write(&path, bytes).expect("write");
        let loaded = load_decision_config(&path).expect("load");
        assert_eq!(loaded.digest, Sha256Digest::of(bytes));
        assert!(!loaded.config.jev.enabled);
    }
}
