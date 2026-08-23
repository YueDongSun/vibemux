use std::{ffi::OsString, path::PathBuf, time::Duration};

const PROJECT_ROOT_ENV: &str = "VIBEMUX_DAEMON_PROJECT_ROOT";
const RUNTIME_DIR_NAME: &str = ".vibemux";
const STARTED_FILE_NAME: &str = "hang_fixture_started";
const COMPLETED_FILE_NAME: &str = "hang_fixture_completed";
const HANG_DURATION: Duration = Duration::from_millis(600);

fn main() {
    let project_root = project_root(std::env::args_os().skip(1).collect())
        .expect("hang fixture requires a project root");
    let runtime_dir = project_root.join(RUNTIME_DIR_NAME);
    std::fs::create_dir_all(&runtime_dir).expect("hang fixture runtime directory");
    std::fs::write(
        runtime_dir.join(STARTED_FILE_NAME),
        std::process::id().to_string(),
    )
    .expect("hang fixture started marker");
    std::thread::sleep(HANG_DURATION);
    std::fs::write(runtime_dir.join(COMPLETED_FILE_NAME), b"completed")
        .expect("hang fixture completion marker");
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
