//! Immutable content-free ownership records for one-shot planning.
//!
//! Claims, positive preparation receipts, deterministic rejections and start
//! markers use create-new publication. A partial record is retained and refused;
//! losing an acknowledgment never supplies another compiler or start permit.

use std::{
    fs::{File, Metadata},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use uuid::Uuid;
use vibemux_workflow::{Sha256Digest, SpecIdentifier, canonical_json::canonical_digest};

use super::{
    error::WorkflowError,
    planning_profile::{PLANNING_SCHEMA_VERSION, PlanningQuery},
    state_protection::protect_state_root,
};
use crate::bounded_file::{BoundedReadError, read_regular_file};

const MAX_PLANNING_STATE_BYTES: u64 = 16 * 1024;
const INTENT_DIGEST_DOMAIN: &str = "vibemux.workflow.planning_intent.v1";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PlanningIntent {
    pub schema_version: u32,
    pub profile_id: SpecIdentifier,
    pub profile_digest: Sha256Digest,
    pub request_id: Uuid,
    pub request_sha256: Sha256Digest,
    pub execute: bool,
    pub config_digest: Sha256Digest,
}

impl PlanningIntent {
    pub fn new(query: &PlanningQuery, config_digest: Sha256Digest) -> Self {
        Self {
            schema_version: PLANNING_SCHEMA_VERSION,
            profile_id: query.profile_id.clone(),
            profile_digest: query.profile_digest,
            request_id: query.request_id,
            request_sha256: query.request_sha256,
            execute: query.execute,
            config_digest,
        }
    }

    pub fn digest(&self) -> Result<Sha256Digest, WorkflowError> {
        canonical_digest(INTENT_DIGEST_DOMAIN, self).map_err(|_| WorkflowError::Internal)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PlanningCompiler {
    pub schema_version: u32,
    pub intent_digest: Sha256Digest,
    pub call_id: Uuid,
    pub request_fingerprint: Sha256Digest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PlanningPrepared {
    pub schema_version: u32,
    pub intent_digest: Sha256Digest,
    pub workflow_id: Uuid,
    pub start_contract: Sha256Digest,
    pub plan_digest: Sha256Digest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PlanningRejected {
    pub schema_version: u32,
    pub intent_digest: Sha256Digest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PlanningStartMarker {
    pub schema_version: u32,
    pub intent_digest: Sha256Digest,
    pub start_contract: Sha256Digest,
    pub request_id: Uuid,
}

#[derive(Clone, Debug)]
pub(super) struct PlanningFiles {
    root: PathBuf,
}

impl PlanningFiles {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn path(&self, request_id: Uuid, kind: &str) -> PathBuf {
        self.root
            .join(format!("{}.{kind}.json", request_id.simple()))
    }

    /// True grants the first owner permission to continue. Existing exact claims
    /// are observations only, including claims with no retained compiler output.
    pub fn claim(&self, intent: &PlanningIntent) -> Result<bool, WorkflowError> {
        self.create_once(&self.path(intent.request_id, "intent"), intent)
    }

    pub fn intent(&self, request_id: Uuid) -> Result<Option<PlanningIntent>, WorkflowError> {
        self.read(&self.path(request_id, "intent"))
    }

    pub fn compiler(&self, request_id: Uuid) -> Result<Option<PlanningCompiler>, WorkflowError> {
        self.read(&self.path(request_id, "compiler"))
    }

    pub fn publish_compiler(
        &self,
        intent: &PlanningIntent,
        compiler: &PlanningCompiler,
    ) -> Result<(), WorkflowError> {
        if compiler.schema_version != PLANNING_SCHEMA_VERSION
            || compiler.intent_digest != intent.digest()?
            || compiler.call_id.is_nil()
        {
            return Err(WorkflowError::RequestConflict);
        }
        self.create_once(&self.path(intent.request_id, "compiler"), compiler)?;
        Ok(())
    }

    pub fn prepared(&self, request_id: Uuid) -> Result<Option<PlanningPrepared>, WorkflowError> {
        self.read(&self.path(request_id, "prepared"))
    }

    pub fn rejected(&self, request_id: Uuid) -> Result<Option<PlanningRejected>, WorkflowError> {
        self.read(&self.path(request_id, "rejected"))
    }

    pub fn start_marker(
        &self,
        request_id: Uuid,
    ) -> Result<Option<PlanningStartMarker>, WorkflowError> {
        self.read(&self.path(request_id, "start"))
    }

    pub fn publish_prepared(
        &self,
        intent: &PlanningIntent,
        prepared: &PlanningPrepared,
    ) -> Result<(), WorkflowError> {
        if self.rejected(intent.request_id)?.is_some()
            || prepared.schema_version != PLANNING_SCHEMA_VERSION
            || prepared.intent_digest != intent.digest()?
        {
            return Err(WorkflowError::RequestConflict);
        }
        self.create_once(&self.path(intent.request_id, "prepared"), prepared)?;
        Ok(())
    }

    pub fn publish_rejected(&self, intent: &PlanningIntent) -> Result<(), WorkflowError> {
        if self.prepared(intent.request_id)?.is_some() {
            return Err(WorkflowError::RequestConflict);
        }
        let rejected = PlanningRejected {
            schema_version: PLANNING_SCHEMA_VERSION,
            intent_digest: intent.digest()?,
        };
        self.create_once(&self.path(intent.request_id, "rejected"), &rejected)?;
        Ok(())
    }

    /// The marker is deliberately written before start admission. Any existing
    /// marker, even one without an execution receipt, forbids another send.
    pub fn mark_start(
        &self,
        intent: &PlanningIntent,
        start_contract: Sha256Digest,
    ) -> Result<bool, WorkflowError> {
        self.create_once(
            &self.path(intent.request_id, "start"),
            &PlanningStartMarker {
                schema_version: PLANNING_SCHEMA_VERSION,
                intent_digest: intent.digest()?,
                start_contract,
                request_id: intent.request_id,
            },
        )
    }

    fn read<T: DeserializeOwned>(&self, path: &Path) -> Result<Option<T>, WorkflowError> {
        if !self.check_root()? {
            return Ok(None);
        }
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_file() && !linked(&metadata) => {}
            Ok(_) => return Err(WorkflowError::StateUnprotected),
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(WorkflowError::Internal),
        }
        let bytes =
            read_regular_file(path, MAX_PLANNING_STATE_BYTES).map_err(|error| match error {
                BoundedReadError::Missing | BoundedReadError::Io => WorkflowError::Internal,
                BoundedReadError::Refused => WorkflowError::StateUnprotected,
            })?;
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| WorkflowError::Internal)
    }

    /// Observation uses this check without creating directories or changing ACLs.
    fn check_root(&self) -> Result<bool, WorkflowError> {
        let parent = self.root.parent().ok_or(WorkflowError::StateUnprotected)?;
        for path in [parent, self.root.as_path()] {
            match std::fs::symlink_metadata(path) {
                Ok(metadata) if metadata.is_dir() && !linked(&metadata) => {}
                Ok(_) => return Err(WorkflowError::StateUnprotected),
                Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
                Err(_) => return Err(WorkflowError::Internal),
            }
        }
        Ok(true)
    }

    fn ensure_root(&self) -> Result<(), WorkflowError> {
        let parent = self.root.parent().ok_or(WorkflowError::StateUnprotected)?;
        let metadata =
            std::fs::symlink_metadata(parent).map_err(|_| WorkflowError::StateUnprotected)?;
        if !metadata.is_dir() || linked(&metadata) {
            return Err(WorkflowError::StateUnprotected);
        }
        match std::fs::create_dir(&self.root) {
            Ok(()) => sync_directory(parent)?,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(_) => return Err(WorkflowError::StateUnprotected),
        }
        if !self.check_root()? {
            return Err(WorkflowError::StateUnprotected);
        }
        protect_state_root(&self.root)
    }

    fn create_once<T>(&self, path: &Path, value: &T) -> Result<bool, WorkflowError>
    where
        T: DeserializeOwned + Eq + Serialize,
    {
        self.ensure_root()?;
        let bytes = serde_json::to_vec(value).map_err(|_| WorkflowError::Internal)?;
        if bytes.len() as u64 > MAX_PLANNING_STATE_BYTES {
            return Err(WorkflowError::RequestInvalid);
        }
        let mut options = File::options();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = match options.open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                return if self.read::<T>(path)?.as_ref() == Some(value) {
                    Ok(false)
                } else {
                    Err(WorkflowError::RequestConflict)
                };
            }
            Err(_) => return Err(WorkflowError::Internal),
        };
        // Keep failed/partial claims for reconciliation; never erase a send fence.
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| WorkflowError::Internal)?;
        sync_directory(&self.root)?;
        Ok(true)
    }
}

/// POSIX syncs directory entries as well as file bytes. Windows claims are a
/// process-crash fence; no power-failure directory-flush guarantee is asserted.
fn sync_directory(path: &Path) -> Result<(), WorkflowError> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| WorkflowError::Internal)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

fn linked(metadata: &Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent() -> PlanningIntent {
        PlanningIntent::new(
            &PlanningQuery {
                schema_version: 1,
                profile_id: SpecIdentifier::new("development").expect("profile"),
                profile_digest: Sha256Digest::of(b"profile"),
                request_id: Uuid::new_v4(),
                request_sha256: Sha256Digest::of(b"goal"),
                execute: false,
            },
            Sha256Digest::of(b"config"),
        )
    }

    #[test]
    fn absent_observation_does_not_create_state_and_claim_is_immutable() {
        let temporary = tempfile::tempdir().expect("directory");
        let state_root = temporary.path().join("workflow_state");
        protect_state_root(&state_root).expect("protected state");
        let files = PlanningFiles::new(state_root.join("planning"));
        let claimed = intent();
        assert!(files.intent(claimed.request_id).expect("read").is_none());
        assert!(!files.root.exists());
        assert!(files.claim(&claimed).expect("first claim"));
        assert!(!files.claim(&claimed).expect("duplicate only"));
        let mut changed = claimed.clone();
        changed.execute = true;
        assert_eq!(files.claim(&changed), Err(WorkflowError::RequestConflict));
        changed = claimed.clone();
        changed.request_sha256 = Sha256Digest::of(b"changed goal");
        assert_eq!(files.claim(&changed), Err(WorkflowError::RequestConflict));
        let raw = std::fs::read_to_string(files.path(claimed.request_id, "intent"))
            .expect("claim metadata");
        assert!(!raw.contains("changed goal"));
        assert!(!raw.contains("request_text"));
    }

    #[test]
    fn partial_claims_and_start_markers_are_retained_and_never_replaced() {
        let temporary = tempfile::tempdir().expect("directory");
        let state_root = temporary.path().join("workflow_state");
        protect_state_root(&state_root).expect("protected state");
        let files = PlanningFiles::new(state_root.join("planning"));
        let claimed = intent();
        files.claim(&claimed).expect("claim");
        let contract = Sha256Digest::of(b"contract");
        assert!(files.mark_start(&claimed, contract).expect("first marker"));
        assert!(
            !files
                .mark_start(&claimed, contract)
                .expect("existing marker")
        );
        let path = files.path(claimed.request_id, "intent");
        std::fs::write(&path, b"{").expect("simulate incomplete publication");
        assert!(files.claim(&claimed).is_err());
        assert_eq!(std::fs::read(&path).expect("retained bytes"), b"{");
    }
}
