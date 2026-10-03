//! The daemon-private workflow state directory (ADR 031 §7).
//!
//! Layout under `<state dir>/workflow_state`:
//!
//! - `blobs/`: content-addressed candidate bytes ([`BlobStore`]);
//! - `content/`: the opt-in private content store (bundle text), indexed
//!   by `content_index.json` so retention and deletion find every blob;
//! - `<workflow>/plan.json`: the narrowed TaskSpecs and slot plan, written
//!   once at prepare;
//! - `<workflow>/workspaces.json`: the ledger of owned worktrees, so a
//!   resumed workflow reuses its worktrees instead of creating new ones;
//! - `<workflow>/outbox/<id>.txt`: admitted message bodies and routed
//!   bundle text until the recipient acknowledges them;
//! - `<workflow>/scratch/`: verifier result files;
//! - `evaluations/<cycle>.json`: the record of one prompt-policy
//!   evaluation cycle, so its holdout is consulted at most once.
//!
//! Operator-authored evaluation inputs live beside the workflow config:
//! `<state dir>/prompt_suites/<suite>.json` and
//! `<state dir>/prompt_candidates/<candidate>.json`.
//!
//! Nothing here is ever emitted by status or export. Every write goes to a
//! staging file that is renamed into place.

use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;
use vibemux_types::a2a::RunWorkspace;
use vibemux_workflow::{Sha256Digest, SpecIdentifier};

use super::{collector::BlobStore, error::WorkflowError, prepare::WorkflowPlan};

pub const WORKFLOW_STATE_DIR_NAME: &str = "workflow_state";
const LEDGER_SCHEMA_VERSION: u32 = 1;
const LEGACY_CONTENT_INDEX_SCHEMA_VERSION: u32 = 1;
const CONTENT_INDEX_SCHEMA_VERSION: u32 = 2;
const CONTENT_INDEX_FILE_NAME: &str = "content_index.json";
const MAX_STATE_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_OUTBOX_BYTES: u64 = 64 * 1024;
const EVALUATIONS_DIR_NAME: &str = "evaluations";

/// Owned worktrees of one workflow by unit key (for example
/// `worker:track_a:slot_a` or `review:track_a:slot_a:2`).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkspaceLedger {
    pub schema_version: u32,
    pub workspaces: BTreeMap<String, RunWorkspace>,
}

/// One blob of the opt-in content store.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContentIndexEntry {
    pub sha256: Sha256Digest,
    pub workflow_id: Uuid,
    pub bundle_id: Option<Uuid>,
    /// Per-owner expiry. Legacy entries have no reliable per-owner expiry
    /// and are retained until explicitly purged.
    #[serde(default)]
    pub retain_until_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ContentIndex {
    schema_version: u32,
    entries: Vec<ContentIndexEntry>,
}

#[derive(Clone, Debug)]
pub(crate) struct StateFiles {
    /// The daemon state directory, where operator inputs live.
    state_dir: PathBuf,
    root: PathBuf,
    /// Serializes read-modify-write of the shared content index.
    index_lock: Arc<Mutex<()>>,
    /// Serializes content file/index changes with their writer receipts.
    content_gate: Arc<Semaphore>,
}

impl StateFiles {
    pub fn new(state_dir: &Path) -> Self {
        Self {
            state_dir: state_dir.to_path_buf(),
            root: state_dir.join(WORKFLOW_STATE_DIR_NAME),
            index_lock: Arc::new(Mutex::new(())),
            content_gate: Arc::new(Semaphore::new(1)),
        }
    }

    pub async fn content_permit(&self) -> Result<OwnedSemaphorePermit, WorkflowError> {
        Arc::clone(&self.content_gate)
            .acquire_owned()
            .await
            .map_err(|_| WorkflowError::Internal)
    }

    pub fn blobs(&self) -> BlobStore {
        BlobStore::new(self.root.join("blobs"))
    }

    pub fn content_dir(&self) -> PathBuf {
        self.root.join("content")
    }

    fn workflow_dir(&self, workflow_id: Uuid) -> PathBuf {
        self.root.join(workflow_id.simple().to_string())
    }

    pub fn scratch(&self, workflow_id: Uuid) -> PathBuf {
        self.workflow_dir(workflow_id).join("scratch")
    }

    /// Writes the plan once. A plan already on disk must have the same
    /// digest: one request key never binds two plans.
    pub fn write_plan(&self, workflow_id: Uuid, plan: &WorkflowPlan) -> Result<(), WorkflowError> {
        let path = self.workflow_dir(workflow_id).join("plan.json");
        if let Some(existing) = read_json::<WorkflowPlan>(&path)? {
            return if existing.digest()? == plan.digest()? {
                Ok(())
            } else {
                Err(WorkflowError::RequestConflict)
            };
        }
        write_json(&path, plan)
    }

    pub fn read_plan(&self, workflow_id: Uuid) -> Result<WorkflowPlan, WorkflowError> {
        read_json(&self.workflow_dir(workflow_id).join("plan.json"))?.ok_or(WorkflowError::NotFound)
    }

    pub fn read_ledger(&self, workflow_id: Uuid) -> Result<WorkspaceLedger, WorkflowError> {
        let path = self.workflow_dir(workflow_id).join("workspaces.json");
        match read_json::<WorkspaceLedger>(&path)? {
            Some(ledger) if ledger.schema_version == LEDGER_SCHEMA_VERSION => Ok(ledger),
            Some(_) => Err(WorkflowError::Workspace),
            None => Ok(WorkspaceLedger {
                schema_version: LEDGER_SCHEMA_VERSION,
                workspaces: BTreeMap::new(),
            }),
        }
    }

    pub fn write_ledger(
        &self,
        workflow_id: Uuid,
        ledger: &WorkspaceLedger,
    ) -> Result<(), WorkflowError> {
        write_json(
            &self.workflow_dir(workflow_id).join("workspaces.json"),
            ledger,
        )
    }

    fn outbox_path(&self, workflow_id: Uuid, message_id: Uuid) -> PathBuf {
        self.workflow_dir(workflow_id)
            .join("outbox")
            .join(format!("{}.txt", message_id.simple()))
    }

    /// Keeps an admitted message body until its recipient acknowledges it.
    pub fn put_outbox(
        &self,
        workflow_id: Uuid,
        message_id: Uuid,
        body: &str,
    ) -> Result<(), WorkflowError> {
        write_bytes(&self.outbox_path(workflow_id, message_id), body.as_bytes())
    }

    /// The body of an admitted message, re-hashed against its envelope.
    pub fn read_outbox(
        &self,
        workflow_id: Uuid,
        message_id: Uuid,
        expected: Sha256Digest,
    ) -> Result<Option<String>, WorkflowError> {
        self.read_outbox_checked(workflow_id, message_id, |bytes| {
            Sha256Digest::of(bytes) == expected
        })
    }

    /// Outbox text accepted only when `verify` holds for its bytes.
    pub fn read_outbox_checked(
        &self,
        workflow_id: Uuid,
        id: Uuid,
        verify: impl FnOnce(&[u8]) -> bool,
    ) -> Result<Option<String>, WorkflowError> {
        let path = self.outbox_path(workflow_id, id);
        let Some(bytes) = read_bounded(&path, MAX_OUTBOX_BYTES)? else {
            return Ok(None);
        };
        if !verify(&bytes) {
            return Err(WorkflowError::ShareInvalid);
        }
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| WorkflowError::ShareInvalid)
    }

    /// Deletes an acknowledged body. Missing is fine.
    pub fn delete_outbox(&self, workflow_id: Uuid, message_id: Uuid) -> Result<(), WorkflowError> {
        match std::fs::remove_file(self.outbox_path(workflow_id, message_id)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(WorkflowError::Internal),
        }
    }

    /// Opt-in private content: stored by digest, never emitted.
    pub fn put_content(&self, bytes: &[u8]) -> Result<Sha256Digest, WorkflowError> {
        let digest = Sha256Digest::of(bytes);
        if self.read_content(digest)?.is_some() {
            return Ok(digest);
        }
        write_bytes(&self.content_dir().join(digest.to_hex()), bytes)?;
        Ok(digest)
    }

    pub fn delete_content(&self, digest: Sha256Digest) -> Result<bool, WorkflowError> {
        match std::fs::remove_file(self.content_dir().join(digest.to_hex())) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(_) => Err(WorkflowError::Internal),
        }
    }

    /// The bytes of a stored blob, re-hashed.
    pub fn read_content(&self, digest: Sha256Digest) -> Result<Option<Vec<u8>>, WorkflowError> {
        let bytes = read_bounded(
            &self.content_dir().join(digest.to_hex()),
            MAX_STATE_FILE_BYTES,
        )?;
        match bytes {
            Some(bytes) if Sha256Digest::of(&bytes) != digest => Err(WorkflowError::ShareInvalid),
            other => Ok(other),
        }
    }

    pub fn content_index(&self) -> Result<Vec<ContentIndexEntry>, WorkflowError> {
        let _guard = self
            .index_lock
            .lock()
            .map_err(|_| WorkflowError::Internal)?;
        self.read_index()
    }

    pub fn add_content_index(&self, entry: ContentIndexEntry) -> Result<(), WorkflowError> {
        let _guard = self
            .index_lock
            .lock()
            .map_err(|_| WorkflowError::Internal)?;
        let mut entries = self.read_index()?;
        if let Some(existing) = entries.iter_mut().find(|existing| {
            existing.sha256 == entry.sha256
                && existing.workflow_id == entry.workflow_id
                && existing.bundle_id == entry.bundle_id
        }) {
            existing.retain_until_ms = match (existing.retain_until_ms, entry.retain_until_ms) {
                (Some(old), Some(new)) => Some(old.max(new)),
                (None, _) | (Some(_), None) => None,
            };
        } else {
            entries.push(entry);
        }
        self.write_index(entries)
    }

    /// Releases only one workflow's references. `expired_before` limits the
    /// release to known expired references; `None` purges all of its refs.
    /// The blob is removed only after its last owner has been released.
    pub fn release_content_owner(
        &self,
        workflow_id: Uuid,
        digest: Sha256Digest,
        expired_before: Option<u64>,
    ) -> Result<(bool, bool), WorkflowError> {
        let _guard = self
            .index_lock
            .lock()
            .map_err(|_| WorkflowError::Internal)?;
        let mut entries = self.read_index()?;
        let before = entries.len();
        entries.retain(|entry| {
            !(entry.sha256 == digest
                && entry.workflow_id == workflow_id
                && expired_before.is_none_or(|now| {
                    entry
                        .retain_until_ms
                        .is_some_and(|deadline| deadline <= now)
                }))
        });
        if entries.len() == before {
            return Ok((false, false));
        }
        let last_owner = !entries.iter().any(|entry| entry.sha256 == digest);
        if last_owner {
            self.delete_content(digest)?;
        }
        self.write_index(entries)?;
        Ok((true, last_owner))
    }

    fn read_index(&self) -> Result<Vec<ContentIndexEntry>, WorkflowError> {
        match read_json::<ContentIndex>(&self.root.join(CONTENT_INDEX_FILE_NAME))? {
            Some(index) if index.schema_version == CONTENT_INDEX_SCHEMA_VERSION => {
                Ok(index.entries)
            }
            Some(index)
                if index.schema_version == LEGACY_CONTENT_INDEX_SCHEMA_VERSION
                    && index
                        .entries
                        .iter()
                        .all(|entry| entry.retain_until_ms.is_none()) =>
            {
                Ok(index.entries)
            }
            Some(_) => Err(WorkflowError::Internal),
            None => Ok(Vec::new()),
        }
    }

    /// The bytes of `<state dir>/<directory>/<id>.json`, bounded and never
    /// through a symlink; `None` when absent. `id` is a snake_case
    /// identifier, so it cannot name another directory.
    pub fn read_operator_input(
        &self,
        directory: &str,
        id: &SpecIdentifier,
        maximum: u64,
    ) -> Result<Option<Vec<u8>>, WorkflowError> {
        read_bounded(
            &self.state_dir.join(directory).join(format!("{id}.json")),
            maximum,
        )
    }

    fn evaluation_path(&self, cycle_id: Sha256Digest) -> PathBuf {
        self.root
            .join(EVALUATIONS_DIR_NAME)
            .join(format!("{}.json", cycle_id.to_hex()))
    }

    /// The record of one evaluation cycle. A record that cannot be read
    /// is an error, never a fresh cycle.
    pub fn read_evaluation<T: DeserializeOwned>(
        &self,
        cycle_id: Sha256Digest,
    ) -> Result<Option<T>, WorkflowError> {
        read_json(&self.evaluation_path(cycle_id))
    }

    pub fn write_evaluation<T: Serialize>(
        &self,
        cycle_id: Sha256Digest,
        record: &T,
    ) -> Result<(), WorkflowError> {
        write_json(&self.evaluation_path(cycle_id), record)
    }

    fn write_index(&self, entries: Vec<ContentIndexEntry>) -> Result<(), WorkflowError> {
        write_json(
            &self.root.join(CONTENT_INDEX_FILE_NAME),
            &ContentIndex {
                schema_version: CONTENT_INDEX_SCHEMA_VERSION,
                entries,
            },
        )
    }
}

fn read_bounded(path: &Path, maximum: u64) -> Result<Option<Vec<u8>>, WorkflowError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(WorkflowError::Internal),
    };
    if !metadata.is_file() || metadata.len() > maximum {
        return Err(WorkflowError::Internal);
    }
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(maximum + 1).read_to_end(&mut bytes))
        .map_err(|_| WorkflowError::Internal)?;
    Ok(Some(bytes))
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, WorkflowError> {
    match read_bounded(path, MAX_STATE_FILE_BYTES)? {
        None => Ok(None),
        Some(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| WorkflowError::Internal),
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), WorkflowError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| WorkflowError::Internal)?;
    write_bytes(path, &bytes)
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), WorkflowError> {
    let parent = path.parent().ok_or(WorkflowError::Internal)?;
    std::fs::create_dir_all(parent).map_err(|_| WorkflowError::Internal)?;
    let staging = parent.join(format!(".staging_{}", Uuid::new_v4().simple()));
    let written = File::options()
        .write(true)
        .create_new(true)
        .open(&staging)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        });
    if written.is_err() {
        let _ = std::fs::remove_file(&staging);
        return Err(WorkflowError::Internal);
    }
    std::fs::rename(&staging, path).map_err(|_| {
        let _ = std::fs::remove_file(&staging);
        WorkflowError::Internal
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbox_bodies_are_rehashed_and_deleted() {
        let directory = tempfile::tempdir().expect("dir");
        let files = StateFiles::new(directory.path());
        let workflow = Uuid::new_v4();
        let message = Uuid::new_v4();
        files
            .put_outbox(workflow, message, "where is the store?")
            .expect("put");
        let digest = Sha256Digest::of(b"where is the store?");
        assert_eq!(
            files
                .read_outbox(workflow, message, digest)
                .expect("read")
                .as_deref(),
            Some("where is the store?")
        );
        assert_eq!(
            files.read_outbox(workflow, message, Sha256Digest::of(b"other")),
            Err(WorkflowError::ShareInvalid)
        );
        files.delete_outbox(workflow, message).expect("delete");
        assert_eq!(
            files.read_outbox(workflow, message, digest).expect("gone"),
            None
        );
        files.delete_outbox(workflow, message).expect("idempotent");
    }

    #[test]
    fn content_release_preserves_other_workflow_and_later_expiry() {
        let directory = tempfile::tempdir().expect("tempdir");
        let files = StateFiles::new(directory.path());
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let digest = files.put_content(b"same prompt").expect("content");
        for (workflow_id, deadline) in [(first, 100), (second, 200)] {
            files
                .add_content_index(ContentIndexEntry {
                    sha256: digest,
                    workflow_id,
                    bundle_id: None,
                    retain_until_ms: Some(deadline),
                })
                .expect("index");
        }
        assert_eq!(
            files
                .release_content_owner(first, digest, None)
                .expect("purge first"),
            (true, false)
        );
        assert_eq!(
            files.read_content(digest).expect("read"),
            Some(b"same prompt".to_vec())
        );
        assert_eq!(files.content_index().expect("index").len(), 1);
        assert_eq!(
            files
                .release_content_owner(second, digest, Some(150))
                .expect("not expired"),
            (false, false)
        );
        assert_eq!(
            files
                .release_content_owner(second, digest, Some(200))
                .expect("expired"),
            (true, true)
        );
        assert_eq!(files.read_content(digest).expect("read"), None);
    }

    #[test]
    fn legacy_content_index_does_not_expire_without_owner_deadline() {
        let directory = tempfile::tempdir().expect("tempdir");
        let files = StateFiles::new(directory.path());
        let workflow_id = Uuid::new_v4();
        let digest = files.put_content(b"legacy").expect("content");
        files
            .add_content_index(ContentIndexEntry {
                sha256: digest,
                workflow_id,
                bundle_id: None,
                retain_until_ms: None,
            })
            .expect("index");
        assert_eq!(
            files
                .release_content_owner(workflow_id, digest, Some(u64::MAX))
                .expect("sweep"),
            (false, false)
        );
        assert_eq!(
            files.read_content(digest).expect("read"),
            Some(b"legacy".to_vec())
        );
    }

    #[test]
    fn content_index_reads_v1_and_writes_v2_but_rejects_unknown_versions() {
        let directory = tempfile::tempdir().expect("tempdir");
        let files = StateFiles::new(directory.path());
        let index_path = files.root.join(CONTENT_INDEX_FILE_NAME);
        std::fs::create_dir_all(&files.root).expect("state dir");
        let workflow_id = Uuid::new_v4();
        let digest = Sha256Digest::of(b"legacy");
        let legacy = serde_json::json!({
            "schema_version": 1,
            "entries": [{"sha256": digest, "workflow_id": workflow_id, "bundle_id": null}]
        });
        std::fs::write(&index_path, serde_json::to_vec(&legacy).expect("json"))
            .expect("legacy index");
        assert_eq!(
            files.content_index().expect("legacy read"),
            vec![ContentIndexEntry {
                sha256: digest,
                workflow_id,
                bundle_id: None,
                retain_until_ms: None,
            }]
        );
        files
            .add_content_index(ContentIndexEntry {
                sha256: Sha256Digest::of(b"new"),
                workflow_id,
                bundle_id: None,
                retain_until_ms: Some(42),
            })
            .expect("upgrade");
        let upgraded: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&index_path).expect("read")).expect("json");
        assert_eq!(upgraded["schema_version"], 2);
        assert_eq!(files.content_index().expect("v2 read").len(), 2);
        let mut unknown = upgraded;
        unknown["schema_version"] = serde_json::json!(3);
        std::fs::write(&index_path, serde_json::to_vec(&unknown).expect("json"))
            .expect("unknown index");
        assert!(files.content_index().is_err());
    }
}
