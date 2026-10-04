//! Candidate collection, the content-addressed blob store, and
//! materialization (ADR 031 §5).
//!
//! The daemon, never the worker, decides what a candidate is: it asks Git
//! for every changed path of the owned worktree relative to the base commit
//! (untracked and ignored files included, renames split), inspects each
//! path without following links, and builds the manifest under the task's
//! scope policy. A worker's checkpoint list of changed files is a claim and
//! is not consulted. Written bytes are kept by digest so review,
//! verification, and integration run on exactly the collected bytes.

use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use vibemux_types::a2a::RunWorkspace;
use vibemux_workflow::{
    PathPattern, Sha256Digest,
    identifiers::is_relative_path,
    snapshot::{
        ObservedChange, ObservedChangeKind, ObservedContent, ObservedFileType, ScopePolicy,
        SnapshotChange, SnapshotManifest, build_manifest,
    },
};
use vibemux_workspace::WorkspaceManager;

use super::{error::WorkflowError, settings::CandidateLimits};

/// A worker commit moved `HEAD` away from the base commit.
pub const HEAD_MOVED_CODE: &str = "snapshot_head_moved";

/// The scope one collection is checked against.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CollectionScope<'a> {
    pub owned: &'a [PathPattern],
    pub forbidden: &'a [PathPattern],
    pub protected: &'a [PathPattern],
    pub limits: CandidateLimits,
}

/// What one collection observed.
#[derive(Debug)]
pub(crate) struct Collection {
    /// The manifest, or the content-free violation codes.
    pub manifest: Result<SnapshotManifest, Vec<String>>,
    /// Bytes of every collected text write, by path.
    pub texts: BTreeMap<String, String>,
}

impl Collection {
    pub fn digest(&self) -> Option<Sha256Digest> {
        self.manifest
            .as_ref()
            .ok()
            .and_then(|manifest| manifest.digest().ok())
    }
}

/// Collects the changes of one owned worktree and stores their bytes.
pub(crate) async fn collect(
    manager: &WorkspaceManager,
    workspace: &RunWorkspace,
    scope: CollectionScope<'_>,
    blobs: &BlobStore,
) -> Result<Collection, WorkflowError> {
    let head = manager
        .head_commit(workspace)
        .await
        .map_err(|_| WorkflowError::Snapshot)?;
    if head != workspace.base_commit {
        return Ok(Collection {
            manifest: Err(vec![HEAD_MOVED_CODE.to_string()]),
            texts: BTreeMap::new(),
        });
    }
    let status = manager
        .change_status(workspace)
        .await
        .map_err(|_| WorkflowError::Snapshot)?;
    let raw = manager
        .tracked_raw_diff(workspace)
        .await
        .map_err(|_| WorkflowError::Snapshot)?;
    let root = PathBuf::from(&workspace.path);
    let base_commit = workspace.base_commit.clone();
    let owned = scope.owned.to_vec();
    let forbidden = scope.forbidden.to_vec();
    let protected = scope.protected.to_vec();
    let limits = scope.limits;
    let blobs = blobs.clone();
    tokio::task::spawn_blocking(move || {
        let entries = parse_status(&status)?;
        let modes = parse_mode_changes(&raw)?;
        let mut changes = Vec::with_capacity(entries.len());
        let mut contents: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let mut total = 0_u64;
        for (path, kind) in entries {
            let observed = observe(&root, &path, kind, &limits, &mut total)?;
            if let Some(bytes) = observed.1 {
                contents.insert(path.clone(), bytes);
            }
            let mut change = observed.0;
            change.mode_changed = modes.contains(&path);
            changes.push(change);
        }
        let policy = ScopePolicy {
            owned: &owned,
            forbidden: &forbidden,
            protected: &protected,
            max_files: limits.max_files,
            max_file_bytes: limits.max_file_bytes,
            max_total_bytes: limits.max_total_bytes,
        };
        match build_manifest(&base_commit, &changes, policy) {
            Ok(manifest) => {
                let mut texts = BTreeMap::new();
                for entry in &manifest.entries {
                    if let SnapshotChange::Write { sha256, .. } = entry.change {
                        let bytes = contents
                            .remove(&entry.path)
                            .ok_or(WorkflowError::Snapshot)?;
                        if blobs.put(&bytes)? != sha256 {
                            return Err(WorkflowError::Snapshot);
                        }
                        let text = String::from_utf8(bytes).map_err(|_| WorkflowError::Snapshot)?;
                        texts.insert(entry.path.clone(), text);
                    }
                }
                Ok(Collection {
                    manifest: Ok(manifest),
                    texts,
                })
            }
            Err(violations) => {
                let mut codes: Vec<String> = violations
                    .iter()
                    .map(|violation| violation.code().to_string())
                    .collect();
                codes.dedup();
                Ok(Collection {
                    manifest: Err(codes),
                    texts: BTreeMap::new(),
                })
            }
        }
    })
    .await
    .map_err(|_| WorkflowError::Internal)?
}

/// Porcelain v1 `-z` entries: `XY path`. Paths Git reports as directories
/// (an ignored or untracked directory) keep their trailing slash, which the
/// manifest refuses.
fn parse_status(status: &str) -> Result<Vec<(String, ObservedChangeKind)>, WorkflowError> {
    let mut entries = Vec::new();
    for record in status.split('\0').filter(|record| !record.is_empty()) {
        let bytes = record.as_bytes();
        if bytes.len() < 4 || bytes[2] != b' ' {
            return Err(WorkflowError::Snapshot);
        }
        let (index, tree) = (bytes[0], bytes[1]);
        let path = record[3..].to_string();
        let kind = match (index, tree) {
            (b'?', b'?') | (b'!', b'!') | (b'A', _) => ObservedChangeKind::Added,
            (b'D', _) | (_, b'D') => ObservedChangeKind::Deleted,
            _ => ObservedChangeKind::Modified,
        };
        entries.push((path, kind));
    }
    Ok(entries)
}

/// Paths whose tracked file mode differs from the base commit.
fn parse_mode_changes(raw: &str) -> Result<Vec<String>, WorkflowError> {
    let mut changed = Vec::new();
    let mut fields = raw.split('\0').filter(|field| !field.is_empty());
    while let Some(header) = fields.next() {
        let path = fields.next().ok_or(WorkflowError::Snapshot)?;
        let header = header.strip_prefix(':').ok_or(WorkflowError::Snapshot)?;
        let mut parts = header.split(' ');
        let (Some(old_mode), Some(new_mode)) = (parts.next(), parts.next()) else {
            return Err(WorkflowError::Snapshot);
        };
        let is_zero = |mode: &str| mode.bytes().all(|byte| byte == b'0');
        if !is_zero(old_mode) && !is_zero(new_mode) && old_mode != new_mode {
            changed.push(path.to_string());
        }
    }
    Ok(changed)
}

/// Inspects one path without following links. Content is read only for
/// regular files within the per-file and total bounds.
fn observe(
    root: &Path,
    path: &str,
    kind: ObservedChangeKind,
    limits: &CandidateLimits,
    total: &mut u64,
) -> Result<(ObservedChange, Option<Vec<u8>>), WorkflowError> {
    let mut change = ObservedChange {
        path: path.to_string(),
        kind,
        file_type: ObservedFileType::Regular,
        mode_changed: false,
        content: None,
    };
    if kind == ObservedChangeKind::Deleted || !is_relative_path(path) {
        return Ok((change, None));
    }
    let full = join_relative(root, path);
    let metadata = match std::fs::symlink_metadata(&full) {
        Ok(metadata) => metadata,
        // Deleted between status and inspection: report it as deleted.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            change.kind = ObservedChangeKind::Deleted;
            return Ok((change, None));
        }
        Err(_) => return Err(WorkflowError::Snapshot),
    };
    if metadata.file_type().is_symlink() {
        change.file_type = ObservedFileType::Symlink;
        return Ok((change, None));
    }
    if !metadata.is_file() {
        change.file_type = ObservedFileType::Special;
        return Ok((change, None));
    }
    let byte_count = metadata.len();
    *total = total.saturating_add(byte_count);
    if byte_count > limits.max_file_bytes || *total > limits.max_total_bytes {
        // Refused by the manifest bounds; the bytes are never read.
        change.content = Some(ObservedContent {
            sha256: Sha256Digest::of(b""),
            byte_count,
            is_text: false,
        });
        return Ok((change, None));
    }
    let bytes = read_bounded(&full, limits.max_file_bytes)?;
    let is_text = std::str::from_utf8(&bytes).is_ok() && !bytes.contains(&0);
    change.content = Some(ObservedContent {
        sha256: Sha256Digest::of(&bytes),
        byte_count: bytes.len() as u64,
        is_text,
    });
    Ok((change, Some(bytes)))
}

fn read_bounded(path: &Path, maximum: u64) -> Result<Vec<u8>, WorkflowError> {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(maximum + 1).read_to_end(&mut bytes))
        .map_err(|_| WorkflowError::Snapshot)?;
    if bytes.len() as u64 > maximum {
        return Err(WorkflowError::Snapshot);
    }
    Ok(bytes)
}

fn join_relative(root: &Path, path: &str) -> PathBuf {
    path.split('/')
        .fold(root.to_path_buf(), |joined, segment| joined.join(segment))
}

/// Content-addressed bytes under the daemon's private workflow state.
#[derive(Clone, Debug)]
pub(crate) struct BlobStore {
    root: PathBuf,
}

impl BlobStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Stores `bytes` once; an existing blob must hash to its name.
    pub fn put(&self, bytes: &[u8]) -> Result<Sha256Digest, WorkflowError> {
        let digest = Sha256Digest::of(bytes);
        let path = self.root.join(digest.to_hex());
        if path.is_file() {
            if self.get(digest).is_ok() {
                return Ok(digest);
            }
            std::fs::remove_file(&path).map_err(|_| WorkflowError::Snapshot)?;
        }
        std::fs::create_dir_all(&self.root).map_err(|_| WorkflowError::Snapshot)?;
        let staging = self
            .root
            .join(format!(".staging_{}", uuid::Uuid::new_v4().simple()));
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
            return Err(WorkflowError::Snapshot);
        }
        std::fs::rename(&staging, &path).map_err(|_| {
            let _ = std::fs::remove_file(&staging);
            WorkflowError::Snapshot
        })?;
        Ok(digest)
    }

    /// The bytes of `digest`, re-hashed.
    pub fn get(&self, digest: Sha256Digest) -> Result<Vec<u8>, WorkflowError> {
        let path = self.root.join(digest.to_hex());
        let metadata = std::fs::symlink_metadata(&path).map_err(|_| WorkflowError::Snapshot)?;
        if !metadata.is_file() {
            return Err(WorkflowError::Snapshot);
        }
        let mut bytes = Vec::new();
        File::open(&path)
            .and_then(|mut file| file.read_to_end(&mut bytes))
            .map_err(|_| WorkflowError::Snapshot)?;
        if Sha256Digest::of(&bytes) != digest {
            return Err(WorkflowError::Snapshot);
        }
        Ok(bytes)
    }
}

/// Paths written or deleted by more than one manifest.
#[must_use]
pub(crate) fn overlapping_paths(manifests: &[&SnapshotManifest]) -> Vec<String> {
    // Separate worktrees can each admit a differently cased spelling of the
    // same path. Unicode folding conservatively rejects aliases before
    // materialization on Windows, including non-ASCII case pairs.
    let mut seen: BTreeMap<String, (String, usize)> = BTreeMap::new();
    for manifest in manifests {
        for entry in &manifest.entries {
            let alias = entry.path.to_uppercase();
            let (_, count) = seen.entry(alias).or_insert_with(|| (entry.path.clone(), 0));
            *count += 1;
        }
    }
    seen.into_iter()
        .filter(|(_, (_, count))| *count > 1)
        .map(|(_, (path, _))| path)
        .collect()
}

/// Applies manifests to a fresh owned worktree at the base commit: every
/// write comes from the blob store and every delete removes the file.
/// Blocking.
pub(crate) fn apply_manifests(
    target: &Path,
    manifests: &[&SnapshotManifest],
    blobs: &BlobStore,
) -> Result<(), WorkflowError> {
    for manifest in manifests {
        for entry in &manifest.entries {
            if !is_relative_path(&entry.path) {
                return Err(WorkflowError::Snapshot);
            }
            let full = join_relative(target, &entry.path);
            reject_linked_ancestors(target, &entry.path)?;
            match entry.change {
                SnapshotChange::Write { sha256, .. } => {
                    let bytes = blobs.get(sha256)?;
                    if let Some(parent) = full.parent() {
                        std::fs::create_dir_all(parent).map_err(|_| WorkflowError::Snapshot)?;
                    }
                    std::fs::write(&full, bytes).map_err(|_| WorkflowError::Snapshot)?;
                }
                SnapshotChange::Delete => match std::fs::remove_file(&full) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => return Err(WorkflowError::Snapshot),
                },
            }
        }
    }
    Ok(())
}

/// Refuses a write through an existing link or special directory.
fn reject_linked_ancestors(root: &Path, path: &str) -> Result<(), WorkflowError> {
    let mut current = root.to_path_buf();
    let segments: Vec<&str> = path.split('/').collect();
    for segment in &segments[..segments.len().saturating_sub(1)] {
        current = current.join(segment);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            _ => return Err(WorkflowError::Snapshot),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vibemux_workflow::snapshot::SnapshotEntry;

    #[test]
    fn status_entries_map_untracked_ignored_and_deleted_paths() {
        let status = "?? src/new.mjs\0!! cache/\0 M src/store.mjs\0D  old.txt\0 D gone.txt\0";
        let entries = parse_status(status).expect("parse");
        assert_eq!(
            entries,
            vec![
                ("src/new.mjs".to_string(), ObservedChangeKind::Added),
                ("cache/".to_string(), ObservedChangeKind::Added),
                ("src/store.mjs".to_string(), ObservedChangeKind::Modified),
                ("old.txt".to_string(), ObservedChangeKind::Deleted),
                ("gone.txt".to_string(), ObservedChangeKind::Deleted),
            ]
        );
        assert!(parse_status("bad").is_err());
    }

    #[test]
    fn raw_diff_reports_only_real_mode_changes() {
        let zero = "0".repeat(40);
        let one = "1".repeat(40);
        let raw = format!(
            ":100644 100755 {one} {zero} M\0run.sh\0:100644 100644 {one} {zero} M\0a.txt\0:000000 100644 {zero} {zero} A\0b.txt\0"
        );
        assert_eq!(parse_mode_changes(&raw).expect("raw"), vec!["run.sh"]);
    }

    #[test]
    fn blobs_are_content_addressed_and_rehashed() {
        let directory = tempfile::tempdir().expect("dir");
        let blobs = BlobStore::new(directory.path().join("blobs"));
        let digest = blobs.put(b"export const x = 1;\n").expect("put");
        assert_eq!(blobs.get(digest).expect("get"), b"export const x = 1;\n");
        std::fs::write(
            directory.path().join("blobs").join(digest.to_hex()),
            b"tampered",
        )
        .expect("tamper");
        assert!(blobs.get(digest).is_err());
        assert_eq!(blobs.put(b"export const x = 1;\n").expect("heal"), digest);
    }

    #[test]
    fn integration_conflicts_include_windows_case_aliases() {
        let manifest = |path: &str| SnapshotManifest {
            schema_version: vibemux_workflow::snapshot::SNAPSHOT_SCHEMA_VERSION,
            base_commit: "a".repeat(40),
            entries: vec![SnapshotEntry {
                path: path.to_string(),
                change: SnapshotChange::Delete,
            }],
        };
        let first = manifest("src/Foo.mjs");
        let second = manifest("src/foo.mjs");
        assert_eq!(overlapping_paths(&[&first, &second]), vec!["src/Foo.mjs"]);
        let first = manifest("src/\u{00c4}pfel.mjs");
        let second = manifest("src/\u{00e4}pfel.mjs");
        assert_eq!(
            overlapping_paths(&[&first, &second]),
            vec!["src/\u{00c4}pfel.mjs"]
        );
    }
}
