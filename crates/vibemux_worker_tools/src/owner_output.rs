//! Finalization through an owner-supplied durable sink, separate from MCP output.

use crate::workspace::WorkerTools;

pub fn finalize_test_session(
    outcome: crate::test_server::TestTransportOutcome,
    store: impl FnOnce(&[u8]) -> std::io::Result<()>,
) -> Result<(), &'static str> {
    let Some(mut session) = outcome.session else {
        let bytes = serde_json::to_vec(
            &serde_json::json!({"schema_version":2,"status":"outcome_unknown",
            "code":"worker_tools_test_owner_lost","recovery":outcome.recovery_hint}),
        )
        .map_err(|_| "worker_tools_candidate_invalid")?;
        store(&bytes).map_err(|_| "worker_tools_candidate_outcome_unknown")?;
        return Err("worker_tools_test_owner_lost");
    };
    let receipts = match serde_json::to_value(session.receipts()) {
        Ok(value) => value,
        Err(_) => {
            session.tools.retain_for_recovery();
            return Err("worker_tools_candidate_invalid");
        }
    };
    let publication = finalize(&mut session.tools, outcome.transport.is_ok(), |bytes| {
        let mut value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(std::io::Error::other)?;
        value["schema_version"] = serde_json::json!(2);
        value["test_receipts"] = receipts;
        store(&serde_json::to_vec(&value).map_err(std::io::Error::other)?)
    });
    if publication.is_ok() {
        session.allow_cleanup_after_publication();
    }
    publication
}

/// A sink failure retains the sole complete workspace. The sink must publish
/// atomically; a success is not an authoritative daemon acceptance receipt.
pub fn finalize(
    tools: &mut WorkerTools,
    transport_ok: bool,
    store: impl FnOnce(&[u8]) -> std::io::Result<()>,
) -> Result<(), &'static str> {
    let candidate = if transport_ok {
        tools.candidate().map_err(|error| error.code())
    } else {
        Err("worker_tools_transport_failed")
    };
    let (outcome, code) = match candidate {
        Ok(candidate) => (
            serde_json::json!({"status":"candidate","candidate":candidate}),
            None,
        ),
        Err(code) => {
            tools.retain_for_recovery();
            (
                serde_json::json!({"status":"outcome_unknown","code":code,"recovery":tools.recovery_record()}),
                Some(code),
            )
        }
    };
    let output = match serde_json::to_vec(&outcome) {
        Ok(bytes) => bytes,
        Err(_) => {
            tools.retain_for_recovery();
            return Err("worker_tools_candidate_invalid");
        }
    };
    if store(&output).is_err() {
        tools.retain_for_recovery();
        return Err("worker_tools_candidate_outcome_unknown");
    }
    if let Some(code) = code {
        Err(code)
    } else {
        Ok(())
    }
}
