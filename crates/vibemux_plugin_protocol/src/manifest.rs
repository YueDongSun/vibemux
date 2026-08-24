use std::{collections::BTreeSet, fs::File, io::Read, path::Path};

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::{
    PROTOCOL_MAJOR, PluginProtocolError,
    wire::{PluginKind, validate_policy_identifier, validate_policy_identifiers},
};

pub const MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
pub const MAX_ENTRY_POINT_ARGUMENTS: usize = 32;
pub const MAX_ENTRY_POINT_ARGUMENT_BYTES: usize = 4096;
pub const MAX_ENTRY_POINT_TOTAL_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestPluginKind {
    Harness,
    Terminal,
    Sandbox,
    Integration,
    Ui,
    Reporter,
    Policy,
}

impl From<ManifestPluginKind> for PluginKind {
    fn from(kind: ManifestPluginKind) -> Self {
        match kind {
            ManifestPluginKind::Harness => Self::Harness,
            ManifestPluginKind::Terminal => Self::Terminal,
            ManifestPluginKind::Sandbox => Self::Sandbox,
            ManifestPluginKind::Integration => Self::Integration,
            ManifestPluginKind::Ui => Self::Ui,
            ManifestPluginKind::Reporter => Self::Reporter,
            ManifestPluginKind::Policy => Self::Policy,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportedPlatform {
    Windows,
    Linux,
    Macos,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub schema_version: u32,
    pub plugin_id: String,
    pub plugin_version: String,
    pub kind: ManifestPluginKind,
    pub entry_point: Vec<String>,
    pub capabilities: Vec<String>,
    pub requested_permissions: Vec<String>,
    pub supported_platforms: Vec<SupportedPlatform>,
    pub protocol_major: u32,
    pub minimum_protocol_minor: u32,
    pub maximum_protocol_minor: u32,
}

impl PluginManifest {
    pub fn from_toml(encoded: &str) -> Result<Self, PluginProtocolError> {
        if encoded.is_empty() {
            return Err(PluginProtocolError::InvalidManifest);
        }
        if encoded.len() > MAX_MANIFEST_BYTES {
            return Err(PluginProtocolError::ManifestTooLarge);
        }
        let manifest: Self =
            toml::from_str(encoded).map_err(|_| PluginProtocolError::InvalidManifest)?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn from_path(path: &Path) -> Result<Self, PluginProtocolError> {
        let metadata =
            std::fs::symlink_metadata(path).map_err(|_| PluginProtocolError::InvalidManifest)?;
        if metadata.file_type().is_symlink()
            || !metadata.file_type().is_file()
            || metadata.len() == 0
        {
            return Err(PluginProtocolError::InvalidManifest);
        }
        if metadata.len() > MAX_MANIFEST_BYTES as u64 {
            return Err(PluginProtocolError::ManifestTooLarge);
        }
        let file = File::open(path).map_err(|_| PluginProtocolError::InvalidManifest)?;
        let mut encoded = Vec::new();
        file.take((MAX_MANIFEST_BYTES + 1) as u64)
            .read_to_end(&mut encoded)
            .map_err(|_| PluginProtocolError::InvalidManifest)?;
        if encoded.len() > MAX_MANIFEST_BYTES {
            return Err(PluginProtocolError::ManifestTooLarge);
        }
        let encoded =
            std::str::from_utf8(&encoded).map_err(|_| PluginProtocolError::InvalidManifest)?;
        Self::from_toml(encoded)
    }

    pub fn validate(&self) -> Result<(), PluginProtocolError> {
        if self.schema_version != MANIFEST_SCHEMA_VERSION
            || self.protocol_major != PROTOCOL_MAJOR
            || self.minimum_protocol_minor > self.maximum_protocol_minor
        {
            return Err(PluginProtocolError::InvalidManifest);
        }
        validate_policy_identifier(&self.plugin_id)
            .map_err(|_| PluginProtocolError::InvalidManifest)?;
        Version::parse(&self.plugin_version).map_err(|_| PluginProtocolError::InvalidManifest)?;
        validate_entry_point(&self.entry_point)?;
        validate_policy_identifiers(&self.capabilities)
            .map_err(|_| PluginProtocolError::InvalidManifest)?;
        validate_policy_identifiers(&self.requested_permissions)
            .map_err(|_| PluginProtocolError::InvalidManifest)?;
        if self.supported_platforms.is_empty() || self.supported_platforms.len() > 8 {
            return Err(PluginProtocolError::InvalidManifest);
        }
        let unique = self.supported_platforms.iter().collect::<BTreeSet<_>>();
        if unique.len() != self.supported_platforms.len() {
            return Err(PluginProtocolError::InvalidManifest);
        }
        Ok(())
    }
}

fn validate_entry_point(entry_point: &[String]) -> Result<(), PluginProtocolError> {
    if entry_point.is_empty() || entry_point.len() > MAX_ENTRY_POINT_ARGUMENTS {
        return Err(PluginProtocolError::InvalidManifest);
    }
    if entry_point[0].trim().is_empty() || is_forbidden_shell(&entry_point[0]) {
        return Err(PluginProtocolError::InvalidManifest);
    }
    let mut total_bytes = 0_usize;
    for argument in entry_point {
        if argument.is_empty()
            || argument.len() > MAX_ENTRY_POINT_ARGUMENT_BYTES
            || argument.contains('\0')
        {
            return Err(PluginProtocolError::InvalidManifest);
        }
        total_bytes = total_bytes
            .checked_add(argument.len())
            .ok_or(PluginProtocolError::InvalidManifest)?;
    }
    if total_bytes > MAX_ENTRY_POINT_TOTAL_BYTES {
        return Err(PluginProtocolError::InvalidManifest);
    }
    Ok(())
}

fn is_forbidden_shell(executable: &str) -> bool {
    let file_name = executable
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(executable)
        .to_ascii_lowercase();
    matches!(
        file_name.as_str(),
        "cmd"
            | "cmd.exe"
            | "powershell"
            | "powershell.exe"
            | "pwsh"
            | "pwsh.exe"
            | "sh"
            | "bash"
            | "dash"
            | "zsh"
            | "fish"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_MANIFEST: &str = r#"
schema_version = 1
plugin_id = "mock.harness"
plugin_version = "1.2.3"
kind = "harness"
entry_point = ["mock_plugin", "--stdio"]
capabilities = ["harness:run", "harness:cancel"]
requested_permissions = ["workspace:read"]
supported_platforms = ["windows", "linux"]
protocol_major = 1
minimum_protocol_minor = 0
maximum_protocol_minor = 0
"#;

    #[test]
    fn valid_manifest_round_trips_without_shell_command() {
        let manifest = PluginManifest::from_toml(VALID_MANIFEST).expect("valid manifest");
        assert_eq!(manifest.entry_point, ["mock_plugin", "--stdio"]);
        assert_eq!(PluginKind::from(manifest.kind), PluginKind::Harness);
    }

    #[test]
    fn unknown_duplicate_and_command_string_manifests_fail_closed() {
        let unknown = format!("{VALID_MANIFEST}\nunknown_field = true\n");
        assert_eq!(
            PluginManifest::from_toml(&unknown).expect_err("unknown field"),
            PluginProtocolError::InvalidManifest
        );

        let duplicate = VALID_MANIFEST.replace(
            "capabilities = [\"harness:run\", \"harness:cancel\"]",
            "capabilities = [\"harness:run\", \"harness:run\"]",
        );
        assert_eq!(
            PluginManifest::from_toml(&duplicate).expect_err("duplicate capability"),
            PluginProtocolError::InvalidManifest
        );

        let command_string = VALID_MANIFEST.replace(
            "entry_point = [\"mock_plugin\", \"--stdio\"]",
            "entry_point = []\ncommand = \"mock_plugin --stdio\"",
        );
        assert_eq!(
            PluginManifest::from_toml(&command_string).expect_err("command string"),
            PluginProtocolError::InvalidManifest
        );

        let shell_entry = VALID_MANIFEST.replace(
            "entry_point = [\"mock_plugin\", \"--stdio\"]",
            "entry_point = [\"cmd.exe\", \"/c\", \"echo unsafe\"]",
        );
        assert_eq!(
            PluginManifest::from_toml(&shell_entry).expect_err("shell entry point"),
            PluginProtocolError::InvalidManifest
        );
    }

    #[test]
    fn manifest_file_size_is_checked_before_parse() {
        let temp = tempfile::tempdir().expect("temp directory");
        let path = temp.path().join("plugin.toml");
        std::fs::write(&path, vec![b'x'; MAX_MANIFEST_BYTES + 1]).expect("large manifest");
        assert_eq!(
            PluginManifest::from_path(&path).expect_err("large manifest must fail"),
            PluginProtocolError::ManifestTooLarge
        );
    }
}
