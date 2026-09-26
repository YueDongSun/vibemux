//! Explicit process and socket identity; never discover a user's default pane.

use serde::Deserialize;
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

const MAX_CONFIG_BYTES: u64 = 16 * 1024;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObserverConfig {
    pub schema_version: u32,
    pub wezterm_executable: PathBuf,
    pub backend_executable: PathBuf,
    pub socket_path: PathBuf,
    pub backend_process_id: u32,
    pub backend_started_at: u64,
    #[serde(skip)]
    socket_identity: Option<SocketIdentity>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SocketIdentity {
    created: SystemTime,
    modified: SystemTime,
    length: u64,
}

impl ObserverConfig {
    pub fn load(path: &Path) -> Result<Self, &'static str> {
        let metadata =
            std::fs::symlink_metadata(path).map_err(|_| "terminal_configuration_invalid")?;
        if !path.is_absolute() || !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES {
            return Err("terminal_configuration_invalid");
        }
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| "terminal_configuration_invalid")?
            .take(MAX_CONFIG_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "terminal_configuration_invalid")?;
        if bytes.len() as u64 > MAX_CONFIG_BYTES {
            return Err("terminal_configuration_invalid");
        }
        let mut config: Self =
            serde_json::from_slice(&bytes).map_err(|_| "terminal_configuration_invalid")?;
        config.validate()?;
        socket_identity(&config.socket_path, config.backend_process_id)?;
        // Pin the actual per-PID endpoint, not an alias or default mux socket.
        config.socket_path = std::fs::canonicalize(&config.socket_path)
            .map_err(|_| "terminal_socket_unavailable")?;
        config.socket_identity = Some(socket_identity(
            &config.socket_path,
            config.backend_process_id,
        )?);
        config.verify_instance()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != 1
            || self.backend_process_id == 0
            || self.backend_started_at == 0
            || !self.socket_path.is_absolute()
            || !self.wezterm_executable.is_absolute()
            || !self.backend_executable.is_absolute()
            || !self.wezterm_executable.is_file()
            || !self.backend_executable.is_file()
        {
            return Err("terminal_configuration_invalid");
        }
        let cli_name = self
            .wezterm_executable
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let backend_name = self
            .backend_executable
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !matches!(cli_name.as_str(), "wezterm.exe" | "wezterm")
            || !matches!(backend_name.as_str(), "wezterm-gui.exe" | "wezterm-gui")
        {
            return Err("terminal_configuration_invalid");
        }
        Ok(())
    }

    pub fn verify_instance(&self) -> Result<String, &'static str> {
        self.validate()?;
        let mut system = System::new();
        let pid = Pid::from_u32(self.backend_process_id);
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
        );
        let process = system.process(pid).ok_or("terminal_instance_changed")?;
        let expected_executable = std::fs::canonicalize(&self.backend_executable)
            .map_err(|_| "terminal_instance_changed")?;
        let actual_executable = process
            .exe()
            .and_then(|path| std::fs::canonicalize(path).ok())
            .ok_or("terminal_instance_changed")?;
        let current_socket = socket_identity(&self.socket_path, self.backend_process_id)?;
        if actual_executable != expected_executable
            || process.start_time() != self.backend_started_at
            || self.socket_identity.as_ref() != Some(&current_socket)
        {
            return Err("terminal_instance_changed");
        }
        let socket_epoch = current_socket
            .created
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "terminal_instance_changed")?
            .as_nanos();
        Ok(format!(
            "wezterm_{}_{}_{}",
            self.backend_process_id, self.backend_started_at, socket_epoch
        ))
    }
}

fn socket_identity(path: &Path, process_id: u32) -> Result<SocketIdentity, &'static str> {
    // WezTerm's native GUI publishes gui-sock-<pid>. Background mux and class
    // aliases do not provide that binding and are deliberately unsupported.
    if path.file_name().and_then(|name| name.to_str())
        != Some(format!("gui-sock-{process_id}").as_str())
    {
        return Err("terminal_socket_identity_mismatch");
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|_| "terminal_socket_unavailable")?;
    if metadata.file_type().is_symlink() || metadata.is_dir() {
        return Err("terminal_socket_identity_mismatch");
    }
    Ok(SocketIdentity {
        created: metadata
            .created()
            .map_err(|_| "terminal_socket_identity_unavailable")?,
        modified: metadata
            .modified()
            .map_err(|_| "terminal_socket_identity_unavailable")?,
        length: metadata.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn socket_identity_is_bound_to_the_backend_pid_and_generation() {
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("gui-sock-42");
        std::fs::write(&socket, b"first").unwrap();
        assert!(socket_identity(&socket, 43).is_err());
        let before = socket_identity(&socket, 42).unwrap();
        std::fs::write(&socket, b"replacement_generation").unwrap();
        assert_ne!(socket_identity(&socket, 42).unwrap(), before);
        assert!(socket_identity(temp.path(), 42).is_err());
    }
}
