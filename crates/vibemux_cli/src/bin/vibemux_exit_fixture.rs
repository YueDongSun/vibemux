use std::{ffi::OsString, path::PathBuf};

const PROJECT_ROOT_ENV: &str = "VIBEMUX_DAEMON_PROJECT_ROOT";
const RUNTIME_DIR_NAME: &str = ".vibemux";
const STARTED_FILE_NAME: &str = "exit_fixture_started";

/// A "daemon" that exits immediately WITHOUT publishing any runtime
/// artifact. Pins the launcher's daemon-exit watchdog: `vibemuxctl daemon
/// start` must observe this exit and fail fast with the classified cause
/// (`daemon_start_failed` on a healthy runtime) instead of waiting out the
/// full startup deadline (issue #7).
fn main() {
    let project_root = project_root(std::env::args_os().skip(1).collect())
        .expect("exit fixture requires a project root");
    let runtime_dir = project_root.join(RUNTIME_DIR_NAME);
    std::fs::create_dir_all(&runtime_dir).expect("exit fixture runtime directory");
    std::fs::write(
        runtime_dir.join(STARTED_FILE_NAME),
        std::process::id().to_string(),
    )
    .expect("exit fixture started marker");
    std::process::exit(3);
}

fn project_root(arguments: Vec<OsString>) -> Option<PathBuf> {
    if arguments.len() == 2 && arguments[0] == "--project-root" {
        return Some(PathBuf::from(&arguments[1]));
    }
    if arguments.is_empty() {
        return std::env::var_os(PROJECT_ROOT_ENV).map(PathBuf::from);
    }
    None
}
