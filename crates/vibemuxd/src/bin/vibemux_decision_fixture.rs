#![forbid(unsafe_code)]
//! Synthetic decision peer for the decision routing tests (ADR 034).
//!
//! Speaks the same one-line protocol as `vibemux_decision_peer` but never
//! touches a network. The request's `request_model` selects the behaviour,
//! so a test picks it through `decision_config.json`:
//!
//! - `fixture_valid`: a confident answer for the last harness option.
//! - `fixture_escalate`: a confident `escalate` answer.
//! - `fixture_low`: the last harness option, barely above uniform.
//! - `fixture_model`: a confident answer from an unpinned model.
//! - `fixture_failed`: a reported `rate_limited` failure.
//! - `fixture_malformed`: a line that is not a result.
//! - `fixture_oversized`: a line above the result limit.
//! - `fixture_hang`: never answers.
//! - `fixture_env`: answers like `fixture_valid` only if its environment
//!   holds nothing but the allowed system variables and a non-empty
//!   credential variable; otherwise reports `request_invalid`.

use std::{
    collections::BTreeMap,
    io::{BufRead, Read, Write},
    time::Duration,
};

use vibemux_decision::{
    BackendAnswer, ExternalError, PeerRequest, PeerResult, wire::MAX_PEER_REQUEST_BYTES,
};

const PINNED_MODEL: &str = "jev-1.13.0";
const CONFIDENT: f64 = 0.97;
const ALLOWED_SYSTEM_NAMES: [&str; 4] = ["SYSTEMROOT", "WINDIR", "TEMP", "TMP"];
const HANG: Duration = Duration::from_secs(600);

fn main() {
    let mut line = Vec::new();
    let mut reader = std::io::stdin()
        .lock()
        .take(MAX_PEER_REQUEST_BYTES as u64 + 1);
    if reader.read_until(b'\n', &mut line).is_err() {
        std::process::exit(4);
    }
    let result = match PeerRequest::decode_line(&line) {
        Ok(request) => respond(&request),
        Err(error) => Response::Result(PeerResult::failed(error)),
    };
    let bytes = match result {
        Response::Result(result) => match result.encode_line() {
            Ok(bytes) => bytes,
            Err(_) => std::process::exit(4),
        },
        Response::Raw(bytes) => bytes,
        Response::Hang => {
            std::thread::sleep(HANG);
            std::process::exit(4);
        }
    };
    let mut stdout = std::io::stdout().lock();
    if stdout
        .write_all(&bytes)
        .and_then(|()| stdout.flush())
        .is_err()
    {
        std::process::exit(4);
    }
}

enum Response {
    Result(PeerResult),
    Raw(Vec<u8>),
    Hang,
}

fn respond(request: &PeerRequest) -> Response {
    let harness_options: Vec<&str> = request
        .options
        .iter()
        .map(|option| option.id.as_str())
        .filter(|id| !id.eq_ignore_ascii_case("escalate"))
        .collect();
    let Some(last_harness) = harness_options.last().copied() else {
        return Response::Result(PeerResult::failed(ExternalError::RequestInvalid));
    };
    let count = request.options.len() as f64;
    let answered = |choice: &str, peak: f64, model: &str| {
        Response::Result(PeerResult::answered(answer(request, choice, peak, model)))
    };
    match request.request_model.as_str() {
        "fixture_escalate" => answered("escalate", CONFIDENT, PINNED_MODEL),
        "fixture_low" => answered(last_harness, 1.0 / count + 0.05, PINNED_MODEL),
        "fixture_model" => answered(last_harness, CONFIDENT, "jev-9.9.9"),
        "fixture_failed" => Response::Result(PeerResult::failed(ExternalError::RateLimited)),
        "fixture_malformed" => Response::Raw(b"not a result\n".to_vec()),
        "fixture_oversized" => {
            let mut bytes = vec![b'x'; 20 * 1024];
            bytes.push(b'\n');
            Response::Raw(bytes)
        }
        "fixture_hang" => Response::Hang,
        "fixture_env" if !environment_is_minimal(&request.credential_env) => {
            Response::Result(PeerResult::failed(ExternalError::RequestInvalid))
        }
        _ => answered(last_harness, CONFIDENT, PINNED_MODEL),
    }
}

/// `peak` for `choice`, the rest spread evenly; the reported confidence is
/// the normalized peak.
fn answer(request: &PeerRequest, choice: &str, peak: f64, model: &str) -> BackendAnswer {
    let count = request.options.len() as f64;
    let rest = (1.0 - peak) / (count - 1.0);
    let probabilities: BTreeMap<String, f64> = request
        .options
        .iter()
        .map(|option| {
            let id = option.id.as_str().to_owned();
            let probability = if id == choice { peak } else { rest };
            (id, probability)
        })
        .collect();
    let uniform = 1.0 / count;
    BackendAnswer {
        model: model.to_owned(),
        choice: choice.to_owned(),
        probabilities,
        confidence: ((peak - uniform) / (1.0 - uniform)).clamp(0.0, 1.0),
    }
}

fn environment_is_minimal(credential_env: &str) -> bool {
    let credential_present =
        std::env::var_os(credential_env).is_some_and(|value| !value.is_empty());
    let only_allowed = std::env::vars_os().all(|(name, _)| {
        let name = name.to_string_lossy().to_ascii_uppercase();
        name == credential_env.to_ascii_uppercase() || ALLOWED_SYSTEM_NAMES.contains(&name.as_str())
    });
    credential_present && only_allowed
}
