//! The one-shot decision peer (ADR 034 §6 and §7).
//!
//! `vibemuxd` makes no network call itself: each external decision spawns
//! `vibemux_decision_peer` with a cleared environment that holds only the
//! system variables Windows networking needs plus the one configured
//! credential variable, writes one request line, reads one bounded result
//! line, and waits for the exit, all under one deadline. The request names
//! the credential variable; its value travels only in the child environment.

use std::{
    ffi::OsString,
    fmt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    time::timeout,
};
use vibemux_decision::{
    BackendAnswer, ExternalError, PeerRequest, PeerResult, wire::MAX_PEER_RESULT_BYTES,
};

/// File name of the decision peer next to the daemon executable.
pub const DECISION_PEER_NAME: &str = "vibemux_decision_peer";
/// Longer than the peer's own total backend budget, so a slow backend is
/// reported by the peer before the daemon gives up on it.
pub const PEER_DEADLINE: Duration = Duration::from_secs(50);
/// Bound on reaping the peer once it was killed.
const REAP_DEADLINE: Duration = Duration::from_secs(2);
/// Passed through when present; nothing else from the daemon environment
/// reaches the peer except the credential variable.
const SYSTEM_ENVIRONMENT_NAMES: [&str; 4] = ["SystemRoot", "WINDIR", "TEMP", "TMP"];
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The peer bundled next to the running daemon executable.
#[must_use]
pub fn bundled_peer() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let name = format!("{DECISION_PEER_NAME}{}", std::env::consts::EXE_SUFFIX);
    Some(executable.parent()?.join(name))
}

/// The configured credential variable read from the daemon environment.
/// `Debug` is redacted so the value never reaches a log.
pub struct PeerCredential {
    name: String,
    value: OsString,
}

impl fmt::Debug for PeerCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PeerCredential")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl PeerCredential {
    /// A missing or empty variable is `credential_unavailable`.
    pub fn from_environment(name: &str) -> Result<Self, ExternalError> {
        match std::env::var_os(name) {
            Some(value) if !value.is_empty() => Ok(Self {
                name: name.to_owned(),
                value,
            }),
            _ => Err(ExternalError::CredentialUnavailable),
        }
    }
}

/// Runs one peer exchange. The answer is returned unvalidated; the caller
/// validates it against the question (ADR 034 §8).
pub async fn run_peer(
    executable: &Path,
    request: &PeerRequest,
    credential: PeerCredential,
    deadline: Duration,
) -> Result<BackendAnswer, ExternalError> {
    // A relative path would resolve against the daemon's working directory
    // and the platform search rules.
    if !executable.is_absolute() {
        return Err(ExternalError::PeerUnavailable);
    }
    let line = request.encode_line()?;
    let mut command = Command::new(executable);
    command
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for name in SYSTEM_ENVIRONMENT_NAMES {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command.env(&credential.name, &credential.value);
    drop(credential);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let mut child = command
        .spawn()
        .map_err(|_| ExternalError::PeerUnavailable)?;
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.start_kill();
        return Err(ExternalError::PeerUnavailable);
    };
    let limit = MAX_PEER_RESULT_BYTES as u64;
    let exchange = async {
        stdin
            .write_all(&line)
            .await
            .map_err(|_| ExternalError::PeerProtocol)?;
        stdin
            .flush()
            .await
            .map_err(|_| ExternalError::PeerProtocol)?;
        drop(stdin);
        let mut output = Vec::new();
        stdout
            .take(limit + 1)
            .read_to_end(&mut output)
            .await
            .map_err(|_| ExternalError::PeerProtocol)?;
        if output.len() as u64 > limit {
            return Err(ExternalError::PeerProtocol);
        }
        let status = child
            .wait()
            .await
            .map_err(|_| ExternalError::PeerProtocol)?;
        Ok((output, status))
    };
    let outcome = timeout(deadline, exchange).await;
    let (output, status) = match outcome {
        Ok(Ok(finished)) => finished,
        Ok(Err(error)) => {
            reap(&mut child).await;
            return Err(error);
        }
        Err(_) => {
            reap(&mut child).await;
            return Err(ExternalError::PeerTimeout);
        }
    };
    let result = PeerResult::decode_line(&output)?;
    if !status.success() {
        return Err(ExternalError::PeerProtocol);
    }
    result
}

async fn reap(child: &mut tokio::process::Child) {
    let _ = child.start_kill();
    let _ = timeout(REAP_DEADLINE, child.wait()).await;
}
