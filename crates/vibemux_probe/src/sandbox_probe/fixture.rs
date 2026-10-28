//! Fixed syscall workload. This is trusted probe code, never agent-supplied code.

use std::{
    ffi::OsString,
    fs, io,
    net::{Ipv4Addr, SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use super::evidence::{CheckId, CheckObservation, FixtureReport, Observation, SCHEMA_VERSION};

const SOCKET_DEADLINE: Duration = Duration::from_secs(2);
const NONCE_LENGTH: usize = 32;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn observe(check: CheckId, result: io::Result<()>) -> CheckObservation {
    match result {
        Ok(()) => CheckObservation {
            check,
            observation: Observation::Allowed,
            os_code: None,
        },
        Err(error) => CheckObservation {
            check,
            observation: if error.kind() == io::ErrorKind::PermissionDenied {
                Observation::PermissionDenied
            } else {
                Observation::UnexpectedError
            },
            os_code: error.raw_os_error(),
        },
    }
}

fn read_equal(path: &Path, nonce: &str) -> io::Result<()> {
    if fs::read(path)? == nonce.as_bytes() {
        Ok(())
    } else {
        Err(io::Error::other("fixture mismatch"))
    }
}

fn loopback_connection(port: u16) -> io::Result<()> {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    // Once connected, a later socket error cannot establish connection denial.
    TcpStream::connect_timeout(&address, SOCKET_DEADLINE).map(drop)
}

fn child_observation(workspace: &Path, denied: &Path, nonce: &str) -> CheckObservation {
    let failure = || {
        observe(
            CheckId::ChildOutsideWrite,
            Err(io::Error::other("child failed")),
        )
    };
    let Ok(executable) = std::env::current_exe() else {
        return failure();
    };
    let mut command = Command::new(executable);
    command.args([
        OsString::from("__fixture_child"),
        workspace.as_os_str().into(),
        denied.as_os_str().into(),
        OsString::from(nonce),
    ]);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut command, CREATE_NO_WINDOW);
    // The outer runner bounds and contains this fixed, synchronous child.
    let Ok(output) = command.output() else {
        return failure();
    };
    if !output.status.success() || output.stdout.len() > 1024 {
        return failure();
    }
    serde_json::from_slice::<CheckObservation>(&output.stdout)
        .ok()
        .filter(|record| record.check == CheckId::ChildOutsideWrite)
        .unwrap_or_else(failure)
}

fn fixture(workspace: &Path, denied: &Path, nonce: &str, port: u16) -> FixtureReport {
    let checks = vec![
        observe(
            CheckId::WorkspaceRead,
            read_equal(&workspace.join("readable"), nonce),
        ),
        observe(
            CheckId::WorkspaceWrite,
            fs::write(workspace.join("written"), nonce),
        ),
        observe(
            CheckId::ScratchWrite,
            fs::write(workspace.join("scratch/written"), nonce),
        ),
        observe(
            CheckId::ProtectedRead,
            read_equal(&workspace.join(".git/sentinel"), nonce),
        ),
        observe(
            CheckId::ProtectedWrite,
            fs::write(workspace.join(".git/sentinel"), "modified"),
        ),
        observe(
            CheckId::SealedRead,
            read_equal(&workspace.join("sealed/sentinel"), nonce),
        ),
        observe(
            CheckId::OutsideRead,
            read_equal(&denied.join("sentinel"), nonce),
        ),
        observe(
            CheckId::OutsideWrite,
            fs::write(denied.join("written"), "modified"),
        ),
        observe(
            CheckId::LinkRead,
            read_equal(&workspace.join("link/sentinel"), nonce),
        ),
        observe(
            CheckId::LinkWrite,
            fs::write(workspace.join("link/link_written"), "modified"),
        ),
        child_observation(workspace, denied, nonce),
        observe(CheckId::LoopbackConnect, loopback_connection(port)),
    ];
    FixtureReport {
        schema_version: SCHEMA_VERSION,
        nonce: nonce.into(),
        checks,
    }
}

/// Hidden fixed-fixture modes used only by this package's explicit probe binary.
/// They accept no command, script, environment or network target from an agent.
#[doc(hidden)]
pub fn fixture_entry(arguments: &[OsString]) -> Option<i32> {
    let mode = arguments.first()?.to_str()?;
    if !matches!(mode, "__fixture" | "__fixture_child") {
        return None;
    }
    let expected = if mode == "__fixture" { 5 } else { 4 };
    if arguments.len() != expected {
        return Some(4);
    }
    let workspace = PathBuf::from(&arguments[1]);
    let denied = PathBuf::from(&arguments[2]);
    let Some(nonce) = arguments[3].to_str() else {
        return Some(4);
    };
    if !workspace.is_absolute()
        || !denied.is_absolute()
        || nonce.len() != NONCE_LENGTH
        || !nonce.bytes().all(|c| c.is_ascii_hexdigit())
        || workspace.file_name().is_none_or(|name| name != "workspace")
        || denied.file_name().is_none_or(|name| name != "denied")
        || workspace.parent() != denied.parent()
    {
        return Some(4);
    }
    if mode == "__fixture_child" {
        if fs::write(workspace.join("child_started"), nonce).is_err() {
            return Some(2);
        }
        let record = observe(
            CheckId::ChildOutsideWrite,
            fs::write(denied.join("child_written"), "modified"),
        );
        let Ok(encoded) = serde_json::to_string(&record) else {
            return Some(2);
        };
        println!("{encoded}");
        return Some(0);
    }
    let Some(port) = arguments[4]
        .to_str()
        .and_then(|text| text.parse::<u16>().ok())
        .filter(|p| *p != 0)
    else {
        return Some(4);
    };
    let Ok(encoded) = serde_json::to_string(&fixture(&workspace, &denied, nonce, port)) else {
        return Some(2);
    };
    println!("{encoded}");
    Some(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_successful_connection_is_allowed_even_if_the_listener_closes() {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let result = loopback_connection(port);
        drop(listener);
        assert_eq!(
            observe(CheckId::LoopbackConnect, result).observation,
            Observation::Allowed
        );
    }
    #[test]
    fn a_missing_file_is_an_unexpected_error_not_a_denial() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(
            observe(
                CheckId::OutsideRead,
                read_equal(&directory.path().join("absent"), "nonce")
            )
            .observation,
            Observation::UnexpectedError
        );
    }
    #[test]
    fn internal_mode_rejects_unbound_paths_and_nonces() {
        assert_eq!(
            fixture_entry(&[
                "__fixture_child".into(),
                "relative".into(),
                "outside".into(),
                "nonce".into()
            ]),
            Some(4)
        );
        assert_eq!(fixture_entry(&["help".into()]), None);
    }
}
