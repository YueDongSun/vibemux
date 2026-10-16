use std::{collections::BTreeMap, process::Stdio, time::Duration};

use tokio::{
    io::{AsyncReadExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    task::JoinHandle,
    time::timeout,
};
use vibemux_platform::ProcessTree;
use vibemux_plugin_protocol::manifest::{ManifestPluginKind, PluginManifest, SupportedPlatform};
use vibemux_plugin_supervisor::ResolvedPluginLaunch;

use crate::{Error, config::Route};

pub(crate) const SYSTEM_ENVIRONMENT: &[&str] = &["SYSTEMROOT", "WINDIR", "TEMP", "TMP"];

pub(crate) fn environment(route: &Route) -> Result<BTreeMap<String, String>, Error> {
    let mut values = BTreeMap::new();
    for name in SYSTEM_ENVIRONMENT {
        if let Ok(value) = std::env::var(name) {
            values.insert((*name).to_string(), value);
        }
    }
    for name in &route.environment_names {
        let value = std::env::var(name)
            .map_err(|source| Error::with_source("harness_environment_unavailable", source))?;
        values.insert(name.clone(), value);
    }
    Ok(values)
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
        if !launch
            .executable
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
        {
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
