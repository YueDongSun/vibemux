//! The peer's one-shot run loop (ADR 034 §6): read one request line, ask the
//! backend, write exactly one result line. Never prints the credential, the
//! request, or a response body.

use std::io::{BufRead, BufReader, Read, Write};

use vibemux_decision::wire::{MAX_PEER_REQUEST_BYTES, PEER_FAILURE_EXIT_CODE};
use vibemux_decision::{ExternalError, PeerRequest, PeerResult};

use crate::jev_client::{ClientLimits, read_credential_env};

/// Printed to stderr, and only to stderr, when no result line could be
/// written at all. Carries no request/response content.
const PROTOCOL_FAILURE_MESSAGE: &str = "decision_peer_failed: peer_protocol";

/// The daemon clears the peer's environment apart from the platform's
/// system variables and the one named credential variable, so this variable
/// never reaches a production peer. It exists only for `test_endpoint`
/// builds driven by the integration tests.
#[cfg(feature = "test_endpoint")]
const TEST_ENDPOINT_ENV_VAR: &str = "VIBEMUX_DECISION_PEER_TEST_ENDPOINT";

/// Reads one request line from `stdin`, executes it, and writes exactly one
/// result line to `stdout`. Returns the process exit code: `0` whenever a
/// result line was written (answered or failed), [`PEER_FAILURE_EXIT_CODE`]
/// only when no result line could be written at all.
pub fn run(
    stdin: impl Read,
    mut stdout: impl Write,
    mut stderr: impl Write,
    env: &dyn Fn(&str) -> Option<String>,
) -> i32 {
    let result = match read_request(stdin) {
        Ok(request) => execute(&request, env),
        Err(error) => PeerResult::failed(error),
    };
    match result.encode_line() {
        Ok(line) => match stdout.write_all(&line).and_then(|()| stdout.flush()) {
            Ok(()) => 0,
            Err(_) => {
                report_protocol_failure(&mut stderr);
                PEER_FAILURE_EXIT_CODE
            }
        },
        Err(_) => {
            report_protocol_failure(&mut stderr);
            PEER_FAILURE_EXIT_CODE
        }
    }
}

fn report_protocol_failure(stderr: &mut impl Write) {
    let _ = writeln!(stderr, "{PROTOCOL_FAILURE_MESSAGE}");
}

/// Reads at most one capped line from `stdin`. Stops reading once the cap is
/// exceeded instead of buffering unbounded input.
fn read_request(stdin: impl Read) -> Result<PeerRequest, ExternalError> {
    let bounded = stdin.take(MAX_PEER_REQUEST_BYTES as u64 + 1);
    let mut reader = BufReader::new(bounded);
    let mut line = Vec::new();
    reader
        .read_until(b'\n', &mut line)
        .map_err(|_| ExternalError::RequestInvalid)?;
    PeerRequest::decode_line(&line)
}

fn execute(request: &PeerRequest, env: &dyn Fn(&str) -> Option<String>) -> PeerResult {
    let credential = match read_credential_env(&request.credential_env, env) {
        Ok(credential) => credential,
        Err(error) => return PeerResult::failed(error),
    };
    let client = match build_client(env, ClientLimits::default()) {
        Ok(client) => client,
        Err(error) => return PeerResult::failed(error),
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => return PeerResult::failed(ExternalError::Transport),
    };
    match runtime.block_on(client.ask(request, &credential)) {
        Ok(answer) => PeerResult::answered(answer),
        Err(error) => PeerResult::failed(error),
    }
}

#[cfg(feature = "test_endpoint")]
fn build_client(
    env: &dyn Fn(&str) -> Option<String>,
    limits: ClientLimits,
) -> Result<crate::jev_client::JevClient, ExternalError> {
    match env(TEST_ENDPOINT_ENV_VAR) {
        Some(endpoint) => crate::jev_client::JevClient::with_test_endpoint(limits, &endpoint),
        None => crate::jev_client::JevClient::new(limits),
    }
}

#[cfg(not(feature = "test_endpoint"))]
fn build_client(
    _env: &dyn Fn(&str) -> Option<String>,
    limits: ClientLimits,
) -> Result<crate::jev_client::JevClient, ExternalError> {
    crate::jev_client::JevClient::new(limits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_invalid_request_line_produces_a_failed_result_and_writes_it() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run(&b"not json\n"[..], &mut stdout, &mut stderr, &|_| None);
        assert_eq!(code, 0);
        assert!(stderr.is_empty());
        assert_eq!(
            PeerResult::decode_line(&stdout),
            Ok(Err(ExternalError::RequestInvalid))
        );
    }

    #[test]
    fn oversized_stdin_without_a_newline_is_rejected() {
        let oversized = vec![b'x'; MAX_PEER_REQUEST_BYTES + 256];
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run(&oversized[..], &mut stdout, &mut stderr, &|_| None);
        assert_eq!(code, 0);
        assert_eq!(
            PeerResult::decode_line(&stdout),
            Ok(Err(ExternalError::RequestInvalid))
        );
    }
}
