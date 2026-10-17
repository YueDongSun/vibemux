#[cfg(any(windows, test))]
use std::path::Path;
use std::{collections::BTreeMap, env::VarError, process::Stdio, time::Duration};

use tokio::{
    io::{AsyncReadExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    task::JoinHandle,
    time::timeout,
};
use vibemux_harness::dispatch::launch_spec::SYSTEM_ENVIRONMENT_NAMES;
use vibemux_platform::ProcessTree;
use vibemux_plugin_protocol::manifest::{ManifestPluginKind, PluginManifest, SupportedPlatform};
use vibemux_plugin_supervisor::ResolvedPluginLaunch;

use crate::{Error, config::Route};

pub(crate) fn environment(route: &Route) -> Result<BTreeMap<String, String>, Error> {
    allowlisted_environment(&route.environment_names, |name| std::env::var(name))
}

/// Builds the whole environment a launch receives: each OS startup name that
/// is set, then each route name, which must be set and valid Unicode. A route
/// name from the startup set stays optional, as in the daemon's launch.
fn allowlisted_environment(
    route_names: &[String],
    lookup: impl Fn(&str) -> Result<String, VarError>,
) -> Result<BTreeMap<String, String>, Error> {
    let mut values = BTreeMap::new();
    for name in SYSTEM_ENVIRONMENT_NAMES {
        if let Ok(value) = lookup(name) {
            values.insert(name.to_string(), value);
        }
    }
    for name in route_names {
        if SYSTEM_ENVIRONMENT_NAMES.contains(&name.as_str()) {
            continue;
        }
        let value = lookup(name)
            .map_err(|source| Error::with_source("harness_environment_unavailable", source))?;
        values.insert(name.clone(), value);
    }
    Ok(values)
}

/// Whether Windows starts `path` itself rather than through a command
/// interpreter. Only an `.exe` file qualifies, in any letter case.
#[cfg(any(windows, test))]
fn is_native_executable(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
}

pub(crate) struct Process {
    pub input: Option<ChildStdin>,
    pub output: Option<BufReader<ChildStdout>>,
    child: Child,
    ownership: Option<ProcessTree>,
    stderr_task: Option<JoinHandle<Result<u64, Error>>>,
}

pub(crate) struct Exit {
    pub code: Option<i32>,
    pub success: bool,
    pub forced: bool,
    pub stderr_bytes: u64,
}

impl Process {
    pub async fn spawn(route: &Route, arguments: Vec<String>) -> Result<Self, Error> {
        let mut entry_point = vec!["native".to_string()];
        entry_point.extend(arguments);
        let manifest = PluginManifest {
            schema_version: 1,
            plugin_id: "vibemux.native".to_string(),
            plugin_version: "1.0.0".to_string(),
            kind: ManifestPluginKind::Harness,
            entry_point,
            capabilities: vec![],
            requested_permissions: vec![],
            protocol_major: 1,
            minimum_protocol_minor: 0,
            maximum_protocol_minor: 0,
            supported_platforms: vec![
                SupportedPlatform::Windows,
                SupportedPlatform::Linux,
                SupportedPlatform::Macos,
            ],
        };
        let launch = ResolvedPluginLaunch::new(
            &manifest,
            route.executable.clone(),
            route.working_directory.clone(),
            environment(route)?,
        )
        .map_err(|source| Error::with_source("harness_invalid_launch", source))?;
        #[cfg(windows)]
        if !is_native_executable(&launch.executable) {
            return Err(Error::new("harness_native_executable_required"));
        }
        let mut command = Command::new(&launch.executable);
        command
            .args(&launch.arguments)
            .current_dir(&launch.working_directory)
            .env_clear()
            .envs(&launch.environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000 | 0x0000_0200);
        ProcessTree::prepare_command(command.as_std_mut());
        let mut child = command
            .spawn()
            .map_err(|source| Error::with_source("harness_spawn_failed", source))?;
        let ownership = match contain(&child) {
            Ok(ownership) => ownership,
            Err(error) => {
                let _ = child.start_kill();
                let _ = timeout(Duration::from_secs(2), child.wait()).await;
                return Err(error);
            }
        };
        let input = child
            .stdin
            .take()
            .ok_or_else(|| Error::new("harness_missing_pipe"))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| Error::new("harness_missing_pipe"))?;
        let mut diagnostics = child
            .stderr
            .take()
            .ok_or_else(|| Error::new("harness_missing_pipe"))?;
        let stderr_task = tokio::spawn(async move {
            let mut count = 0_u64;
            let mut buffer = [0_u8; 8192];
            loop {
                match diagnostics.read(&mut buffer).await {
                    Ok(0) => return Ok(count),
                    Err(source) => {
                        return Err(Error::with_source("harness_stderr_read_failed", source));
                    }
                    Ok(bytes) => count = count.saturating_add(bytes as u64),
                }
            }
        });
        Ok(Self {
            input: Some(input),
            output: Some(BufReader::new(output)),
            child,
            ownership: Some(ownership),
            stderr_task: Some(stderr_task),
        })
    }

    pub async fn observe_exit(&mut self) -> Result<(), Error> {
        self.child
            .wait()
            .await
            .map_err(|source| Error::with_source("harness_wait_failed", source))?;
        // Descendants may inherit stdout. Release the owned group after the
        // parent exits so these handles cannot keep the final drain open.
        self.ownership.take();
        Ok(())
    }

    pub async fn finish(&mut self, grace: Duration, force: bool) -> Result<Exit, Error> {
        self.input.take();
        let mut forced = force;
        if force {
            self.ownership.take();
            self.child
                .start_kill()
                .map_err(|source| Error::with_source("harness_kill_failed", source))?;
        }
        let status = match timeout(grace, self.child.wait()).await {
            Ok(result) => {
                result.map_err(|source| Error::with_source("harness_wait_failed", source))?
            }
            Err(_) => {
                forced = true;
                self.ownership.take();
                self.child
                    .start_kill()
                    .map_err(|source| Error::with_source("harness_kill_failed", source))?;
                timeout(grace, self.child.wait())
                    .await
                    .map_err(|_| Error::new("harness_reap_timeout"))?
                    .map_err(|source| Error::with_source("harness_wait_failed", source))?
            }
        };
        self.ownership.take();
        let mut stderr_task = self
            .stderr_task
            .take()
            .ok_or_else(|| Error::new("harness_missing_stderr"))?;
        let stderr_bytes = match timeout(grace, &mut stderr_task).await {
            Ok(result) => {
                result.map_err(|source| Error::with_source("harness_stderr_failed", source))??
            }
            Err(_) => {
                stderr_task.abort();
                return Err(Error::new("harness_stderr_timeout"));
            }
        };
        Ok(Exit {
            code: status.code(),
            success: status.success(),
            forced,
            stderr_bytes,
        })
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.ownership.take();
        if let Some(task) = &self.stderr_task {
            task.abort();
        }
    }
}

/// Places the un-reaped child and its future descendants in an owned process
/// tree before any prompt is sent. Dropping the tree terminates every member.
fn contain(child: &Child) -> Result<ProcessTree, Error> {
    let process_id = child
        .id()
        .ok_or_else(|| Error::new("harness_child_exited"))?;
    ProcessTree::contain(process_id)
        .map_err(|source| Error::with_source("harness_containment_failed", source))
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, env::VarError, ffi::OsString, path::Path};

    use super::{SYSTEM_ENVIRONMENT_NAMES, allowlisted_environment, is_native_executable};

    const NOT_UNICODE: &str = "NOT_UNICODE";

    /// A host environment where the value `NOT_UNICODE` stands for a value
    /// that is set but is not valid Unicode.
    fn launch_environment(
        host: &[(&str, &str)],
        route_names: &[&str],
    ) -> Result<BTreeMap<String, String>, &'static str> {
        let route_names: Vec<String> = route_names.iter().map(ToString::to_string).collect();
        allowlisted_environment(&route_names, |name| {
            match host.iter().find(|(key, _)| *key == name) {
                Some((_, value)) if *value == NOT_UNICODE => {
                    Err(VarError::NotUnicode(OsString::from(name)))
                }
                Some((_, value)) => Ok((*value).to_string()),
                None => Err(VarError::NotPresent),
            }
        })
        .map_err(|error| error.code())
    }

    fn expected(pairs: &[(&str, &str)]) -> Result<BTreeMap<String, String>, &'static str> {
        Ok(pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect())
    }

    #[test]
    fn a_launch_inherits_only_the_startup_set_and_the_route_names() {
        let mut host: Vec<(&str, &str)> = SYSTEM_ENVIRONMENT_NAMES
            .iter()
            .map(|name| (*name, "startup_value"))
            .collect();
        host.extend([
            ("PATH", "path_value"),
            ("HOME", "home_value"),
            ("OPENAI_API_KEY", "key_value"),
            ("UNLISTED_SECRET", "secret_value"),
        ]);
        assert_eq!(
            launch_environment(&host, &["OPENAI_API_KEY"]),
            expected(&[
                ("OPENAI_API_KEY", "key_value"),
                ("SYSTEMROOT", "startup_value"),
                ("TEMP", "startup_value"),
                ("TMP", "startup_value"),
                ("WINDIR", "startup_value"),
            ])
        );
        // A startup name is passed only when it is set and valid Unicode.
        assert_eq!(
            launch_environment(&[("TEMP", "temp_value"), ("TMP", NOT_UNICODE)], &[]),
            expected(&[("TEMP", "temp_value")])
        );
        // A route name must be set and valid Unicode.
        for host in [vec![], vec![("OPENAI_API_KEY", NOT_UNICODE)]] {
            assert_eq!(
                launch_environment(&host, &["OPENAI_API_KEY"]),
                Err("harness_environment_unavailable"),
                "{host:?}"
            );
        }
        // A route name from the startup set stays optional, as in the daemon.
        assert_eq!(launch_environment(&[], &["TEMP"]), expected(&[]));
        assert_eq!(
            launch_environment(&[("TEMP", "temp_value")], &["TEMP"]),
            expected(&[("TEMP", "temp_value")])
        );
        let guide = include_str!("../../../docs/guides/harness_core_guide.md");
        for name in SYSTEM_ENVIRONMENT_NAMES {
            assert!(guide.contains(&format!("`{name}`")), "{name}");
        }
        assert!(guide.contains("not even `PATH`"));
        assert!(guide.contains("harness_environment_unavailable"));
    }

    #[test]
    fn only_an_exe_starts_without_a_command_interpreter() {
        for accepted in ["codex.exe", "CODEX.EXE", "tools/claude.Exe", "tool.cmd.exe"] {
            assert!(is_native_executable(Path::new(accepted)), "{accepted}");
        }
        for refused in [
            "codex",
            "codex.cmd",
            "codex.bat",
            "codex.ps1",
            "codex.com",
            "codex.exe.cmd",
            "codex.exe.",
            "codex.ex",
            "exe",
            ".exe",
            "",
        ] {
            assert!(!is_native_executable(Path::new(refused)), "{refused}");
        }
    }
}
