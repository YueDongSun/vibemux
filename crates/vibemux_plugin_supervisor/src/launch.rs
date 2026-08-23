use std::{collections::BTreeMap, fmt, path::PathBuf};

use vibemux_plugin_protocol::manifest::PluginManifest;

use crate::PluginSupervisorError;

pub const MAX_PLUGIN_ENVIRONMENT_ITEMS: usize = 32;
pub const MAX_PLUGIN_ENVIRONMENT_KEY_BYTES: usize = 128;
pub const MAX_PLUGIN_ENVIRONMENT_VALUE_BYTES: usize = 4096;

#[derive(Clone, Eq, PartialEq)]
pub struct ResolvedPluginLaunch {
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub working_directory: PathBuf,
    pub environment: BTreeMap<String, String>,
}

impl fmt::Debug for ResolvedPluginLaunch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedPluginLaunch")
            .field("executable", &"[redacted]")
            .field("argument_count", &self.arguments.len())
            .field("working_directory", &"[redacted]")
            .field(
                "environment_keys",
                &self.environment.keys().collect::<Vec<_>>(),
            )
            .field("environment_values", &"[redacted]")
            .finish()
    }
}

impl ResolvedPluginLaunch {
    pub fn new(
        manifest: &PluginManifest,
        executable: PathBuf,
        working_directory: PathBuf,
        environment: BTreeMap<String, String>,
    ) -> Result<Self, PluginSupervisorError> {
        let executable =
            std::fs::canonicalize(executable).map_err(|_| PluginSupervisorError::InvalidLaunch)?;
        let working_directory = std::fs::canonicalize(working_directory)
            .map_err(|_| PluginSupervisorError::InvalidLaunch)?;
        let launch = Self {
            executable,
            arguments: manifest.entry_point.iter().skip(1).cloned().collect(),
            working_directory,
            environment,
        };
        launch.validate()?;
        Ok(launch)
    }

    pub fn validate(&self) -> Result<(), PluginSupervisorError> {
        if !self.executable.is_absolute()
            || !self.executable.is_file()
            || is_forbidden_shell(&self.executable)
            || !self.working_directory.is_absolute()
            || !self.working_directory.is_dir()
            || self.arguments.len()
                > vibemux_plugin_protocol::manifest::MAX_ENTRY_POINT_ARGUMENTS - 1
            || self.environment.len() > MAX_PLUGIN_ENVIRONMENT_ITEMS
        {
            return Err(PluginSupervisorError::InvalidLaunch);
        }
        for argument in &self.arguments {
            if argument.is_empty()
                || argument.len()
                    > vibemux_plugin_protocol::manifest::MAX_ENTRY_POINT_ARGUMENT_BYTES
                || argument.contains('\0')
            {
                return Err(PluginSupervisorError::InvalidLaunch);
            }
        }
        for (key, value) in &self.environment {
            if key.is_empty()
                || key.len() > MAX_PLUGIN_ENVIRONMENT_KEY_BYTES
                || value.len() > MAX_PLUGIN_ENVIRONMENT_VALUE_BYTES
                || !key
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
                || value.contains('\0')
            {
                return Err(PluginSupervisorError::InvalidLaunch);
            }
        }
        Ok(())
    }
}

fn is_forbidden_shell(executable: &std::path::Path) -> bool {
    let Some(file_name) = executable.file_name().and_then(std::ffi::OsStr::to_str) else {
        return true;
    };
    matches!(
        file_name.to_ascii_lowercase().as_str(),
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
    use vibemux_plugin_protocol::{
        PROTOCOL_MAJOR,
        manifest::{ManifestPluginKind, PluginManifest, SupportedPlatform},
    };

    use super::*;

    #[test]
    fn invalid_environment_and_shell_executable_are_rejected() {
        let temp = tempfile::tempdir().expect("temp directory");
        let manifest = manifest();
        let mut environment = BTreeMap::new();
        environment.insert("bad-key".to_string(), "value".to_string());
        assert_eq!(
            ResolvedPluginLaunch::new(
                &manifest,
                std::env::current_exe().expect("current executable"),
                temp.path().to_path_buf(),
                environment,
            )
            .expect_err("invalid environment key"),
            PluginSupervisorError::InvalidLaunch
        );

        let shell = if cfg!(windows) {
            PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"))
                .join("System32")
                .join("cmd.exe")
        } else {
            PathBuf::from("/bin/sh")
        };
        assert_eq!(
            ResolvedPluginLaunch::new(&manifest, shell, temp.path().to_path_buf(), BTreeMap::new())
                .expect_err("shell executable"),
            PluginSupervisorError::InvalidLaunch
        );
    }

    #[test]
    fn launch_debug_redacts_paths_arguments_and_environment_values() {
        let temp = tempfile::tempdir().expect("temp directory");
        let mut environment = BTreeMap::new();
        environment.insert("SECRET_VALUE".to_string(), "do_not_log".to_string());
        let launch = ResolvedPluginLaunch::new(
            &manifest(),
            std::env::current_exe().expect("current executable"),
            temp.path().to_path_buf(),
            environment,
        )
        .expect("resolved launch");
        let rendered = format!("{launch:?}");
        assert!(!rendered.contains("do_not_log"));
        let working_directory = temp.path().to_string_lossy();
        assert!(!rendered.contains(working_directory.as_ref()));
        assert!(rendered.contains("SECRET_VALUE"));
    }

    fn manifest() -> PluginManifest {
        PluginManifest {
            schema_version: 1,
            plugin_id: "mock.harness".to_string(),
            plugin_version: "1.0.0".to_string(),
            kind: ManifestPluginKind::Harness,
            entry_point: vec!["mock_plugin".to_string()],
            capabilities: vec![],
            requested_permissions: vec![],
            supported_platforms: vec![if cfg!(windows) {
                SupportedPlatform::Windows
            } else {
                SupportedPlatform::Linux
            }],
            protocol_major: PROTOCOL_MAJOR,
            minimum_protocol_minor: 0,
            maximum_protocol_minor: 0,
        }
    }
}
