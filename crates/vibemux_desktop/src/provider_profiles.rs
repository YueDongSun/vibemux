//! Native provider metadata, not a credential store or an execution route.
//!
//! Configuration is separately versioned and never imported into browser
//! conversation records, desktop appearance settings, or canonical state.
//! Loading and saving resolve no environment variables and perform no network
//! requests. The selected profile cannot make a workflow gateway live-ready.

use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use url::{Host, Url};

use crate::command_error::CommandError;
use crate::config_file::replace_config_file;

pub const PROVIDER_PROFILES_FILE_NAME: &str = "provider_profiles.json";
pub const PROVIDER_PROFILES_SCHEMA_VERSION: u16 = 1;
pub const MAX_PROVIDER_PROFILES: usize = 16;
pub const MAX_PROVIDER_PROFILES_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderApiType {
    AnthropicMessages,
    OpenaiChatCompletions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAuthType {
    ApiKey,
    Bearer,
    None,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderProfile {
    pub id: String,
    pub name: String,
    pub api_type: ProviderApiType,
    pub base_url: String,
    pub model: String,
    pub credential_env: Option<String>,
    pub auth_type: ProviderAuthType,
}

impl fmt::Debug for ProviderProfile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderProfile")
            .field("api_type", &self.api_type)
            .field("auth_type", &self.auth_type)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderProfilesConfig {
    pub schema_version: u16,
    pub profiles: Vec<ProviderProfile>,
    pub orchestrator_profile_id: Option<String>,
}

impl Default for ProviderProfilesConfig {
    fn default() -> Self {
        Self {
            schema_version: PROVIDER_PROFILES_SCHEMA_VERSION,
            profiles: Vec::new(),
            orchestrator_profile_id: None,
        }
    }
}

impl fmt::Debug for ProviderProfilesConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderProfilesConfig")
            .field("schema_version", &self.schema_version)
            .field("profile_count", &self.profiles.len())
            .field(
                "orchestrator_selected",
                &self.orchestrator_profile_id.is_some(),
            )
            .finish_non_exhaustive()
    }
}

impl ProviderProfilesConfig {
    pub fn validate(&self) -> Result<(), ProviderProfilesError> {
        if self.schema_version != PROVIDER_PROFILES_SCHEMA_VERSION {
            return Err(ProviderProfilesError::UnsupportedSchema);
        }
        if self.profiles.len() > MAX_PROVIDER_PROFILES {
            return Err(ProviderProfilesError::TooManyProfiles);
        }
        let mut ids = BTreeSet::new();
        for profile in &self.profiles {
            profile.validate()?;
            if !ids.insert(profile.id.as_str()) {
                return Err(ProviderProfilesError::DuplicateId);
            }
        }
        if self
            .orchestrator_profile_id
            .as_ref()
            .is_some_and(|id| !ids.contains(id.as_str()))
        {
            return Err(ProviderProfilesError::UnknownSelection);
        }
        Ok(())
    }

    /// Tauri has already decoded its envelope. Bound the application payload
    /// before materializing its typed fields, and never expose serde's input
    /// fragments through a command error.
    pub fn from_command_value(value: serde_json::Value) -> Result<Self, ProviderProfilesError> {
        let bytes = encode_bounded(&value, false)?;
        decode(&bytes)
    }
}

impl ProviderProfile {
    fn validate(&self) -> Result<(), ProviderProfilesError> {
        if self.id.is_empty()
            || self.id.len() > 64
            || !self
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(ProviderProfilesError::InvalidId);
        }
        if !bounded_text(&self.name, 128) {
            return Err(ProviderProfilesError::InvalidName);
        }
        if !bounded_text(&self.model, 256) {
            return Err(ProviderProfilesError::InvalidModel);
        }
        validate_base_url(&self.base_url)?;
        match (&self.auth_type, &self.credential_env) {
            (ProviderAuthType::None, None) => Ok(()),
            (ProviderAuthType::ApiKey | ProviderAuthType::Bearer, Some(name))
                if valid_environment_name(name) =>
            {
                Ok(())
            }
            _ => Err(ProviderProfilesError::InvalidCredentialReference),
        }
    }
}

fn text_padding(character: char) -> bool {
    character.is_whitespace() || u32::from(character) == 0xfeff
}

fn bounded_text(value: &str, limit: usize) -> bool {
    !value.is_empty()
        && value.len() <= limit
        && value.trim_matches(text_padding) == value
        && !value.chars().any(char::is_control)
}

fn valid_environment_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= 128
        && bytes
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn validate_base_url(value: &str) -> Result<(), ProviderProfilesError> {
    if !bounded_text(value, 2048) || value.contains('\\') || value.chars().any(text_padding) {
        return Err(ProviderProfilesError::InvalidBaseUrl);
    }
    let valid_authority = value.split_once("://").is_some_and(|(_, rest)| {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        !authority.is_empty() && !authority.contains('@')
    });
    if !valid_authority {
        return Err(ProviderProfilesError::InvalidBaseUrl);
    }
    let url = Url::parse(value).map_err(|_| ProviderProfilesError::InvalidBaseUrl)?;
    let numeric_loopback = match url.host() {
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        _ => false,
    };
    let allowed_scheme = url.scheme() == "https" || (url.scheme() == "http" && numeric_loopback);
    if !allowed_scheme
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.port_or_known_default().is_none_or(|port| port == 0)
    {
        return Err(ProviderProfilesError::InvalidBaseUrl);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderProfilesReadiness {
    ConfigurationOnly,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProviderProfilesView {
    pub config: ProviderProfilesConfig,
    pub readiness: ProviderProfilesReadiness,
}

impl From<ProviderProfilesConfig> for ProviderProfilesView {
    fn from(config: ProviderProfilesConfig) -> Self {
        Self {
            config,
            readiness: ProviderProfilesReadiness::ConfigurationOnly,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderProfilesError {
    Unavailable,
    Busy,
    ReadFailed,
    WriteFailed,
    UnsafeFile,
    TooLarge,
    InvalidDocument,
    UnsupportedSchema,
    TooManyProfiles,
    InvalidId,
    DuplicateId,
    InvalidName,
    InvalidModel,
    InvalidBaseUrl,
    InvalidCredentialReference,
    UnknownSelection,
}

impl ProviderProfilesError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Unavailable => "provider_profiles_unavailable",
            Self::Busy => "provider_profiles_busy",
            Self::ReadFailed => "provider_profiles_read_failed",
            Self::WriteFailed => "provider_profiles_write_failed",
            Self::UnsafeFile => "provider_profiles_unsafe_file",
            Self::TooLarge => "provider_profiles_too_large",
            Self::InvalidDocument => "provider_profiles_invalid_document",
            Self::UnsupportedSchema => "provider_profiles_unsupported_schema",
            Self::TooManyProfiles => "provider_profiles_too_many",
            Self::InvalidId => "provider_profiles_invalid_id",
            Self::DuplicateId => "provider_profiles_duplicate_id",
            Self::InvalidName => "provider_profiles_invalid_name",
            Self::InvalidModel => "provider_profiles_invalid_model",
            Self::InvalidBaseUrl => "provider_profiles_invalid_base_url",
            Self::InvalidCredentialReference => "provider_profiles_invalid_credential_reference",
            Self::UnknownSelection => "provider_profiles_unknown_selection",
        }
    }

    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Unavailable => "Native provider settings are unavailable.",
            Self::Busy => "Another provider settings operation is in progress. Try again.",
            Self::ReadFailed => {
                "Provider settings could not be read. No configuration was changed."
            }
            Self::WriteFailed => {
                "Provider settings could not be saved. The previous configuration was preserved."
            }
            Self::UnsafeFile => {
                "Provider settings must be a regular file, not a link or directory. No configuration was changed."
            }
            Self::TooLarge => "Provider settings exceed the 64 KiB limit.",
            Self::InvalidDocument => {
                "Provider settings are malformed or contain unsupported fields. No configuration was changed."
            }
            Self::UnsupportedSchema => {
                "This provider settings version is not supported. No configuration was changed."
            }
            Self::TooManyProfiles => "Keep at most 16 provider profiles.",
            Self::InvalidId => {
                "Each profile needs a unique ID of 1 to 64 ASCII letters, digits, underscores, or hyphens."
            }
            Self::DuplicateId => "Provider profile IDs must be unique.",
            Self::InvalidName => {
                "Each profile needs a name of at most 128 UTF-8 bytes, without padding or control characters."
            }
            Self::InvalidModel => {
                "Each profile needs a model ID of at most 256 UTF-8 bytes, without padding or control characters."
            }
            Self::InvalidBaseUrl => {
                "Use an HTTPS base URL, or HTTP at a numeric loopback address, without credentials, query, fragment, or backslashes."
            }
            Self::InvalidCredentialReference => {
                "Choose a valid environment-variable name for authentication, or no credential for anonymous access. Do not enter a key value."
            }
            Self::UnknownSelection => {
                "The selected orchestrator profile must exist in the profile list."
            }
        }
    }
}

impl fmt::Display for ProviderProfilesError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for ProviderProfilesError {}

impl From<ProviderProfilesError> for CommandError {
    fn from(error: ProviderProfilesError) -> Self {
        Self::new(error.code(), error.message())
    }
}

pub(crate) fn load_provider_profiles_at(
    path: &Path,
) -> Result<ProviderProfilesConfig, ProviderProfilesError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(ProviderProfilesConfig::default());
        }
        Err(_) => return Err(ProviderProfilesError::ReadFailed),
    };
    if !metadata.is_file() {
        return Err(ProviderProfilesError::UnsafeFile);
    }
    if metadata.len() > MAX_PROVIDER_PROFILES_BYTES as u64 {
        return Err(ProviderProfilesError::TooLarge);
    }
    let file = File::open(path).map_err(|_| ProviderProfilesError::ReadFailed)?;
    if !file
        .metadata()
        .map_err(|_| ProviderProfilesError::ReadFailed)?
        .is_file()
    {
        return Err(ProviderProfilesError::UnsafeFile);
    }
    let mut bytes = Vec::new();
    file.take(MAX_PROVIDER_PROFILES_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ProviderProfilesError::ReadFailed)?;
    if bytes.len() > MAX_PROVIDER_PROFILES_BYTES {
        return Err(ProviderProfilesError::TooLarge);
    }
    decode(&bytes)
}

pub(crate) fn save_provider_profiles_at(
    path: &Path,
    config: &ProviderProfilesConfig,
) -> Result<(), ProviderProfilesError> {
    config.validate()?;
    let bytes = encode_bounded(config, true)?;
    // An explicit save may replace a valid snapshot, never a file this version
    // could not interpret. Independent instances publish complete snapshots;
    // like appearance settings, they use last-writer-wins semantics.
    load_provider_profiles_at(path)?;
    replace_config_file(path, &bytes).map_err(|_| ProviderProfilesError::WriteFailed)
}

fn decode(bytes: &[u8]) -> Result<ProviderProfilesConfig, ProviderProfilesError> {
    let config: ProviderProfilesConfig =
        serde_json::from_slice(bytes).map_err(|_| ProviderProfilesError::InvalidDocument)?;
    config.validate()?;
    Ok(config)
}

fn encode_bounded(value: &impl Serialize, pretty: bool) -> Result<Vec<u8>, ProviderProfilesError> {
    let mut output = LimitedJson(Vec::new());
    let encoded = if pretty {
        serde_json::to_writer_pretty(&mut output, value)
    } else {
        serde_json::to_writer(&mut output, value)
    };
    encoded.map_err(|_| ProviderProfilesError::TooLarge)?;
    Ok(output.0)
}

struct LimitedJson(Vec<u8>);

impl Write for LimitedJson {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > MAX_PROVIDER_PROFILES_BYTES {
            return Err(io::Error::other("provider configuration is too large"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
