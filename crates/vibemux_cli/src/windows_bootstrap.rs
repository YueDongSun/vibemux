//! Native Windows process creation for the daemon lifecycle client.

use std::{
    fs, io,
    path::Path,
    process::{Child, Command, Stdio},
};

use std::os::windows::process::CommandExt;

const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Spawns the daemon with typed arguments and no inherited standard streams.
///
/// The executable and project root are canonicalized before process creation.
/// The returned `Child` remains owned by the caller until authenticated health
/// confirms that the daemon is ready.
pub(super) fn spawn_daemon(executable: &Path, project_root: &Path) -> io::Result<Child> {
    let executable = fs::canonicalize(executable)?;
    if !executable.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "daemon executable is not a file",
        ));
    }
    let project_root = fs::canonicalize(project_root)?;

    let mut command = Command::new(executable);
    command
        .arg("--project-root")
        .arg(&project_root)
        .current_dir(project_root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    command.spawn()
}
