//! Opt-in installed-Claude qualification against an owner-held synthetic API.
//! This guard never enables a Live workflow or supplies sandbox qualification.

use super::{
    DispatchError, DispatchRoute, NativeProtocol,
    fixture_guard::{image_digest_bounded, regular_canonical},
    launch_spec::LaunchSpec,
    writable_profile::WritableGrant,
};
use crate::AgentKind;
use std::{
    ffi::OsString,
    net::{Ipv4Addr, SocketAddrV4},
    path::{Path, PathBuf},
};

pub const CLAUDE_BROKER_QUALIFICATION_PROFILE: &str = "claude_broker_qualification_v1";
pub const QUALIFICATION_MODEL: &str = "fixture-model";
const SYNTHETIC_KEY: &str = "synthetic-fixture-key";
const MAX_CONTROLLER_IMAGE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_BROKER_IMAGE_BYTES: u64 = 256 * 1024 * 1024;
const INVALID: DispatchError = DispatchError::ConfigExecutableInvalid;

/// A process-local registration; no JSON label or ordinary route can create it.
pub struct InstalledClaudeGuard {
    controller: PinnedImage,
    broker: PinnedImage,
    git_bash: PinnedImage,
    endpoint: SocketAddrV4,
}

struct PinnedImage {
    path: PathBuf,
    digest: [u8; 32],
    maximum: u64,
}

impl PinnedImage {
    fn new(path: &Path, maximum: u64) -> Result<Self, DispatchError> {
        let path = regular_canonical(path)?;
        Ok(Self {
            digest: image_digest_bounded(&path, maximum)?,
            path,
            maximum,
        })
    }
    fn verify(&self) -> Result<(), DispatchError> {
        if regular_canonical(&self.path)? != self.path
            || image_digest_bounded(&self.path, self.maximum)? != self.digest
        {
            return Err(INVALID);
        }
        Ok(())
    }
}

impl std::fmt::Debug for InstalledClaudeGuard {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InstalledClaudeGuard")
            .finish_non_exhaustive()
    }
}

impl InstalledClaudeGuard {
    /// The caller owns the listener for this exact numeric loopback address.
    /// Only test_helpers can construct the corresponding daemon service.
    pub fn register(
        controller: &Path,
        broker: &Path,
        git_bash: &Path,
        endpoint: SocketAddrV4,
    ) -> Result<Self, DispatchError> {
        if !cfg!(windows) || *endpoint.ip() != Ipv4Addr::LOCALHOST || endpoint.port() == 0 {
            return Err(DispatchError::ExecutionDisabled);
        }
        let controller = PinnedImage::new(controller, MAX_CONTROLLER_IMAGE_BYTES)?;
        let broker = PinnedImage::new(broker, MAX_BROKER_IMAGE_BYTES)?;
        let git_bash = PinnedImage::new(git_bash, MAX_BROKER_IMAGE_BYTES)?;
        if controller.path == broker.path
            || controller.path == git_bash.path
            || broker.path == git_bash.path
        {
            return Err(INVALID);
        }
        Ok(Self {
            controller,
            broker,
            git_bash,
            endpoint,
        })
    }

    pub fn build_launch_spec(
        &self,
        route: &DispatchRoute,
        grant: WritableGrant,
    ) -> Result<LaunchSpec, DispatchError> {
        if route.harness != AgentKind::Claude
            || route.protocol != NativeProtocol::ClaudeStreamJson
            || !route.enabled
            || !route.allow_execution
            || grant.run_dev_tests
            || !route.environment_names.is_empty()
            || route.model.as_deref() != Some(QUALIFICATION_MODEL)
        {
            return Err(DispatchError::ExecutionDisabled);
        }
        self.controller.verify()?;
        self.broker.verify()?;
        self.git_bash.verify()?;
        if regular_canonical(&route.executable)? != self.controller.path {
            return Err(INVALID);
        }
        let mut arguments: Vec<String> = [
            "--bare",
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--permission-mode",
            "dontAsk",
            "--tools=",
            "--strict-mcp-config",
            "--restricted",
            "--no-session-persistence",
            "--no-chrome",
            "--setting-sources",
            "",
            "--model",
            QUALIFICATION_MODEL,
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let allowed = [
            (grant.read_files, "read_file"),
            (grant.search_files, "search_paths"),
            (grant.edit_files, "edit_file"),
            (grant.write_files, "create_file"),
        ]
        .into_iter()
        .filter(|(enabled, _)| *enabled)
        .map(|(_, name)| format!("mcp__worker__{name}"))
        .collect::<Vec<_>>()
        .join(",");
        arguments.push(format!("--allowedTools={allowed}"));
        Ok(LaunchSpec {
            executable: self.controller.path.clone(),
            arguments,
            environment_names: vec![],
        })
    }

    pub fn broker_executable(&self) -> &Path {
        &self.broker.path
    }

    /// Fixed values per attempt, never inherited route credentials or mutated
    /// daemon-global HOME/provider settings. System runtime values are copied.
    pub fn environment(&self, home: &Path, temporary: &Path) -> Vec<(String, OsString)> {
        let mut values = Vec::new();
        for name in ["SystemRoot", "WINDIR", "SystemDrive"] {
            if let Some(value) = std::env::var_os(name) {
                values.push((name.into(), value));
            }
        }
        for name in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "CLAUDE_CONFIG_DIR",
        ] {
            values.push((name.into(), home.as_os_str().to_owned()));
        }
        for name in ["TEMP", "TMP"] {
            values.push((name.into(), temporary.as_os_str().to_owned()));
        }
        for (name, value) in [
            ("ANTHROPIC_API_KEY", SYNTHETIC_KEY.to_owned()),
            ("ANTHROPIC_BASE_URL", format!("http://{}", self.endpoint)),
            ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1".into()),
            ("DISABLE_AUTOUPDATER", "1".into()),
            ("NO_PROXY", "127.0.0.1,localhost".into()),
            ("ENABLE_TOOL_SEARCH", "false".into()),
        ] {
            values.push((name.into(), value.into()));
        }
        values.push((
            "CLAUDE_CODE_GIT_BASH_PATH".into(),
            self.git_bash.path.as_os_str().to_owned(),
        ));
        values
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use serde_json::json;

    fn setup() -> (PathBuf, InstalledClaudeGuard, DispatchRoute) {
        let root = std::env::temp_dir().join(format!(
            "controller_guard_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir(&root).unwrap();
        for name in ["claude.exe", "broker.exe", "bash.exe"] {
            std::fs::write(root.join(name), name).unwrap();
        }
        let guard = InstalledClaudeGuard::register(
            &root.join("claude.exe"),
            &root.join("broker.exe"),
            &root.join("bash.exe"),
            SocketAddrV4::new(Ipv4Addr::LOCALHOST, 12345),
        )
        .unwrap();
        let route = serde_json::from_value(json!({"harness":"claude","protocol":"claude_stream_json",
            "executable":root.join("claude.exe"),"enabled":true,"allow_execution":true,"model":QUALIFICATION_MODEL})).unwrap();
        (root, guard, route)
    }

    #[test]
    fn closed_profile_and_environment_do_not_adopt_route_credentials() {
        let (root, guard, mut route) = setup();
        let spec = guard
            .build_launch_spec(&route, WritableGrant::READ_ONLY)
            .unwrap();
        assert!(spec.arguments.contains(&"--tools=".into()));
        assert!(
            spec.arguments.contains(
                &"--allowedTools=mcp__worker__read_file,mcp__worker__search_paths".into()
            )
        );
        assert!(spec.environment_names.is_empty());
        let environment = guard.environment(&root.join("home"), &root.join("temp"));
        assert!(
            environment
                .iter()
                .any(|(key, value)| key == "ANTHROPIC_API_KEY" && value == SYNTHETIC_KEY)
        );
        assert!(
            !environment
                .iter()
                .any(|(key, _)| key == "PATH" || key == "ANTHROPIC_AUTH_TOKEN")
        );
        route.environment_names.push("ANTHROPIC_API_KEY".into());
        assert_eq!(
            guard.build_launch_spec(&route, WritableGrant::NONE),
            Err(DispatchError::ExecutionDisabled)
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn commands_wrong_models_and_replaced_images_refuse() {
        let (root, guard, mut route) = setup();
        assert_eq!(
            guard.build_launch_spec(
                &route,
                WritableGrant {
                    run_dev_tests: true,
                    ..WritableGrant::NONE
                }
            ),
            Err(DispatchError::ExecutionDisabled)
        );
        route.model = Some("ordinary-model".into());
        assert_eq!(
            guard.build_launch_spec(&route, WritableGrant::NONE),
            Err(DispatchError::ExecutionDisabled)
        );
        route.model = Some(QUALIFICATION_MODEL.into());
        std::fs::write(root.join("broker.exe"), "changed").unwrap();
        assert_eq!(
            guard.build_launch_spec(&route, WritableGrant::NONE),
            Err(INVALID)
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
