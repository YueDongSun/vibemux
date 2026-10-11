//! Explicit launch grants and fixed resource budgets; parsing performs no I/O.

use std::{ffi::OsString, path::PathBuf, time::Duration};

pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const MAX_INPUT_BYTES: usize = 64 * 1024;
pub const MAX_OUTPUT_BYTES: usize = 256 * 1024;
pub const MAX_DESCRIPTOR_PATH_BYTES: usize = 4096;
pub const OUTPUT_DEADLINE: Duration = Duration::from_secs(2);
pub const READ_DEADLINE: Duration = Duration::from_secs(12);
pub const WORKER_IDLE_DEADLINE: Duration = Duration::from_secs(60);
pub const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(2);
pub const MAX_RESTARTS: u32 = 3;
pub const RESTART_BACKOFF: Duration = Duration::from_millis(200);
pub const CHILD_EXIT_POLL_INTERVAL: Duration = Duration::from_millis(50);
pub const HELP: &str = "vibemux_mcp --descriptor PATH [--allow_metadata_read]\nRead-only MCP stdio frontend; metadata access is denied unless explicitly granted.\n";

#[derive(Clone)]
pub struct HostConfig {
    pub descriptor: PathBuf,
    pub allow_metadata_read: bool,
}

pub enum LaunchMode {
    Help,
    Host(HostConfig),
    IpcReader,
}

pub fn parse_arguments(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<LaunchMode, &'static str> {
    let arguments: Vec<_> = arguments.into_iter().collect();
    if arguments.as_slice() == [OsString::from("--help")] {
        return Ok(LaunchMode::Help);
    }
    if arguments.as_slice() == [OsString::from("--ipc_reader")] {
        return Ok(LaunchMode::IpcReader);
    }
    let mut descriptor = None;
    let mut allow_metadata_read = false;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        if argument == "--descriptor" && descriptor.is_none() {
            let path = arguments.next().ok_or("invalid_arguments")?;
            if path.is_empty()
                || path.to_string_lossy().len() > MAX_DESCRIPTOR_PATH_BYTES
                || path.to_string_lossy().starts_with("--")
            {
                return Err("invalid_arguments");
            }
            descriptor = Some(PathBuf::from(path));
        } else if argument == "--allow_metadata_read" && !allow_metadata_read {
            allow_metadata_read = true;
        } else {
            return Err("invalid_arguments");
        }
    }
    Ok(LaunchMode::Host(HostConfig {
        descriptor: descriptor.ok_or("descriptor_required")?,
        allow_metadata_read,
    }))
}
