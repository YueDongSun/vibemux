use std::{ffi::OsString, path::PathBuf};

use vibemuxd::{control::DaemonControlServer, process::DaemonPaths};

const PROJECT_ROOT_ENV: &str = "VIBEMUX_DAEMON_PROJECT_ROOT";

#[tokio::main]
async fn main() {
    let project_root = project_root(std::env::args_os().skip(1).collect())
        .expect("recovery fixture requires a project root");
    let paths = DaemonPaths::from_project_root(&project_root).expect("recovery fixture paths");
    paths
        .ensure_runtime_dir()
        .expect("recovery fixture runtime directory");
    let server = DaemonControlServer::start_for_paths(&paths)
        .await
        .expect("recovery fixture control server");
    server.wait().await.expect("recovery fixture wait");
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
