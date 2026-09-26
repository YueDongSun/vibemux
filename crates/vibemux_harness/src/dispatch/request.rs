//! The typed dispatch request and its content-free identities.
//!
//! A request carries only a UUID, a harness, and a text prompt. It cannot pick
//! an executable, argument, environment variable, shell, working directory,
//! or RPC method; those come from the trusted route config and the protocol
//! profile. The prompt is data: it is placed inside a JSON string or written
//! to stdin, never interpolated into argv or a shell.

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vibemux_types::ProjectId;

use crate::AgentKind;

use super::{DispatchError, NativeProtocol, Sha256Digest, digest::hash_fields};

pub const DISPATCH_REQUEST_SCHEMA_VERSION: u32 = 1;
/// Keeps a submit inside the 64 KiB Control IPC frame for ordinary text.
pub const MAX_PROMPT_BYTES: usize = 32 * 1024;

const FINGERPRINT_DOMAIN: &str = "vibemux.harness_dispatch.fingerprint.v1";

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchRequest {
    pub schema_version: u32,
    pub request_id: Uuid,
    pub harness: AgentKind,
    pub prompt: String,
}

impl DispatchRequest {
    pub fn validate(&self) -> Result<(), DispatchError> {
        if self.schema_version != DISPATCH_REQUEST_SCHEMA_VERSION || self.request_id.is_nil() {
            return Err(DispatchError::InvalidRequest);
        }
        if self.prompt.trim().is_empty() || self.prompt.len() > MAX_PROMPT_BYTES {
            return Err(DispatchError::InvalidPrompt);
        }
        Ok(())
    }

    #[must_use]
    pub fn prompt_digest(&self) -> PromptDigest {
        PromptDigest {
            sha256: Sha256Digest::of(self.prompt.as_bytes()),
            byte_count: self.prompt.len() as u64,
        }
    }

    /// Idempotency fingerprint (ADR 029 §2). The same request UUID with the
    /// same fingerprint is a duplicate; a different fingerprint is a conflict.
    #[must_use]
    pub fn fingerprint(
        &self,
        project_id: ProjectId,
        protocol: NativeProtocol,
        config_digest: Sha256Digest,
    ) -> Sha256Digest {
        request_fingerprint(
            project_id,
            self.request_id,
            self.harness,
            protocol,
            self.prompt_digest().sha256,
            config_digest,
        )
    }
}

/// [`DispatchRequest::fingerprint`] from the persisted facts alone, so the
/// store can derive it without ever seeing the prompt.
#[must_use]
pub fn request_fingerprint(
    project_id: ProjectId,
    request_id: Uuid,
    harness: AgentKind,
    protocol: NativeProtocol,
    prompt_sha256: Sha256Digest,
    config_digest: Sha256Digest,
) -> Sha256Digest {
    let project = project_id.to_string();
    let request_id = request_id.as_hyphenated().to_string();
    hash_fields(
        FINGERPRINT_DOMAIN,
        &[
            project.as_bytes(),
            request_id.as_bytes(),
            harness.command_name().as_bytes(),
            protocol.as_str().as_bytes(),
            prompt_sha256.as_bytes(),
            config_digest.as_bytes(),
        ],
    )
}

impl fmt::Debug for DispatchRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DispatchRequest")
            .field("request_id", &self.request_id)
            .field("harness", &self.harness)
            .field("prompt_bytes", &self.prompt.len())
            .finish_non_exhaustive()
    }
}

/// Hash and size of a prompt; the only prompt facts that may be persisted.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PromptDigest {
    pub sha256: Sha256Digest,
    pub byte_count: u64,
}
