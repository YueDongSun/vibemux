//! Serialized effects in a newly constructed workspace; no source-tree import.

use crate::contract::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
use tempfile::TempDir;
use vibemux_harness::dispatch::writable_profile::WritableGrant;
use vibemux_workflow::{PathPattern, Sha256Digest, identifiers::is_relative_path};

const MAX_PATH_DEPTH: usize = 16;
const PROTECTED_COMPONENTS: [&str; 5] = [".git", ".vibemux", ".claude", ".codex", ".mcp.json"];
const SNAPSHOT_DOMAIN: &str = "vibemux.worker_tools.snapshot.v1";
const OWNER_RECORD: &str = ".vibemux_owner.json";

struct Entry {
    revision: u64,
    digest: Sha256Digest,
    byte_count: usize,
}

/// The trusted caller must not mutate this private directory or run code in it.
/// No filesystem path is returned through a model-visible result.
pub struct WorkerTools {
    directory: Option<TempDir>,
    root: PathBuf,
    bootstrap: Bootstrap,
    base_snapshot: Sha256Digest,
    files: BTreeMap<String, Entry>,
    changed: BTreeSet<String>,
    total_bytes: usize,
    receipts: Vec<OperationReceipt>,
    quarantined: bool,
}

impl WorkerTools {
    /// Only the version-2 owner session uses this constructor, after validating
    /// its fixed test policy. Version-1 creation continues to refuse RunDevTests.
    pub(crate) fn create_test_session(
        parent: &Path,
        mut bootstrap: Bootstrap,
    ) -> Result<Self, ToolError> {
        let grants = bootstrap.grants;
        bootstrap.grants.run_dev_tests = false;
        let mut tools = Self::create(parent, bootstrap)?;
        tools.bootstrap.grants = grants;
        Ok(tools)
    }

    pub fn create(parent: &Path, mut bootstrap: Bootstrap) -> Result<Self, ToolError> {
        let base_snapshot = snapshot_digest(&bootstrap)?;
        bootstrap
            .files
            .sort_by(|left, right| left.path.cmp(&right.path));
        let parent = verify_private_parent(parent)?;
        let directory = tempfile::Builder::new()
            .prefix("vibemux_worker_tools_")
            .tempdir_in(parent)
            .map_err(|_| ToolError::Io)?;
        let root = directory.path().canonicalize().map_err(|_| ToolError::Io)?;
        // Establish privacy before importing any admitted content.
        #[cfg(windows)]
        {
            vibemux_platform::secure_user_directory(&root).map_err(|_| ToolError::Io)?;
            vibemux_platform::verify_restricted_path_acl(&root).map_err(|_| ToolError::Io)?;
        }
        let owner = serde_json::to_vec(&serde_json::json!({"schema_version":SCHEMA_VERSION,"attempt_id":bootstrap.attempt_id,"base_snapshot":base_snapshot})).map_err(|_| ToolError::InvalidBootstrap)?;
        let mut owner_file = open_options(false, true)
            .create_new(true)
            .open(root.join(OWNER_RECORD))
            .map_err(|_| ToolError::Io)?;
        owner_file
            .write_all(&owner)
            .and_then(|_| owner_file.sync_all())
            .map_err(|_| ToolError::Io)?;
        drop(owner_file);
        let mut files = BTreeMap::new();
        let mut total_bytes = 0;
        for file in &bootstrap.files {
            let path = root.join(&file.path);
            fs::create_dir_all(path.parent().ok_or(ToolError::InvalidPath)?)
                .map_err(|_| ToolError::Io)?;
            let mut output = open_options(false, true)
                .create_new(true)
                .open(path)
                .map_err(|_| ToolError::Io)?;
            output
                .write_all(file.content.as_bytes())
                .and_then(|_| output.sync_all())
                .map_err(|_| ToolError::Io)?;
            total_bytes += file.content.len();
            files.insert(
                file.path.clone(),
                Entry {
                    revision: 0,
                    digest: Sha256Digest::of(file.content.as_bytes()),
                    byte_count: file.content.len(),
                },
            );
        }
        bootstrap.files.clear();
        Ok(Self {
            directory: Some(directory),
            root,
            bootstrap,
            base_snapshot,
            files,
            changed: BTreeSet::new(),
            total_bytes,
            receipts: Vec::new(),
            quarantined: false,
        })
    }

    pub fn grants(&self) -> WritableGrant {
        self.bootstrap.grants
    }

    pub fn revision(&self) -> u64 {
        self.files
            .values()
            .map(|entry| entry.revision)
            .max()
            .unwrap_or(0)
    }

    /// Owner-only verified snapshot. No executable is ever given this root.
    pub(crate) fn test_snapshot(&mut self, expected_revision: u64) -> Result<Bootstrap, ToolError> {
        if self.quarantined {
            return Err(ToolError::Quarantined);
        }
        if !self.bootstrap.grants.read_files || !self.bootstrap.grants.run_dev_tests {
            return Err(ToolError::Denied);
        }
        if expected_revision != self.revision() {
            return Err(ToolError::StaleRevision);
        }
        let result = (|| {
            self.verify_root()?;
            let mut snapshot = self.bootstrap.clone();
            for (path, entry) in &self.files {
                self.authorize_path(path, false)?;
                snapshot.files.push(TextFile {
                    path: path.clone(),
                    content: read_verified(&mut self.open_existing(path, false)?, entry)?,
                });
            }
            Ok(snapshot)
        })();
        if matches!(result, Err(ToolError::WorkspaceChanged | ToolError::Io)) {
            self.retain_for_recovery();
        }
        result
    }

    /// Private journal entry; file creation is exclusive and the namespace is
    /// already protected from all model file operations.
    pub(crate) fn journal_test(
        &mut self,
        operation_id: uuid::Uuid,
        stage: &str,
        value: &impl serde::Serialize,
    ) -> Result<(), ToolError> {
        if !["admitted", "completed"].contains(&stage) {
            return Err(ToolError::InvalidPath);
        }
        let result = (|| {
            self.verify_root()?;
            let directory = self.root.join(".vibemux");
            match fs::create_dir(&directory) {
                Ok(()) => (),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let metadata = fs::symlink_metadata(&directory).map_err(|_| ToolError::Io)?;
                    if !metadata.is_dir() || special(&metadata) {
                        return Err(ToolError::WorkspaceChanged);
                    }
                }
                Err(_) => return Err(ToolError::Io),
            }
            let bytes = serde_json::to_vec(value).map_err(|_| ToolError::Io)?;
            let mut file = open_options(false, true)
                .create_new(true)
                .open(directory.join(format!("{operation_id}_{stage}.json")))
                .map_err(|_| ToolError::Io)?;
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(|_| ToolError::MutationUnknown)
        })();
        if result.is_err() {
            self.retain_for_recovery();
        }
        result
    }

    pub(crate) fn owner_recovery_hint(&self) -> RecoveryRecord {
        RecoveryRecord {
            schema_version: SCHEMA_VERSION,
            attempt_id: self.bootstrap.attempt_id,
            base_snapshot: self.base_snapshot,
            retained_directory: self.root.clone(),
            operations: self.receipts.clone(),
        }
    }

    pub fn execute(&mut self, operation: Operation) -> Result<OperationResult, ToolError> {
        if self.quarantined {
            return Err(ToolError::Quarantined);
        }
        if self.receipts.len() >= MAX_OPERATIONS {
            return Err(ToolError::ResourceLimit);
        }
        let sequence = self.receipts.len() as u64 + 1;
        let name = operation.name();
        let result = self.apply(operation, sequence);
        if matches!(
            result,
            Err(ToolError::MutationUnknown | ToolError::WorkspaceChanged)
        ) {
            self.retain_for_recovery();
        }
        self.receipts.push(OperationReceipt {
            sequence,
            operation: name.into(),
            status: if result.is_ok() {
                "succeeded"
            } else if self.quarantined {
                "outcome_unknown"
            } else {
                "refused"
            }
            .into(),
            code: result.as_ref().err().map(|error| error.code().into()),
        });
        result
    }

    fn apply(&mut self, operation: Operation, sequence: u64) -> Result<OperationResult, ToolError> {
        let grants = self.bootstrap.grants;
        let granted = match &operation {
            Operation::Read { .. } => grants.read_files,
            Operation::Search { .. } => grants.search_files,
            Operation::Create { .. } => grants.write_files,
            Operation::Edit { .. } => grants.edit_files,
        };
        if !granted {
            return Err(ToolError::Denied);
        }
        self.verify_root()?;
        match operation {
            Operation::Read { path } => {
                self.authorize_path(&path, false)?;
                let entry = self.files.get(&path).ok_or(ToolError::Missing)?;
                let mut file = self.open_existing(&path, false)?;
                let content = read_verified(&mut file, entry)?;
                Ok(OperationResult::Read {
                    file: version(&path, entry),
                    content,
                })
            }
            Operation::Search { prefix } => {
                if !prefix.is_empty() {
                    validate_path(&prefix)?;
                }
                let files = self
                    .files
                    .iter()
                    .filter(|(path, _)| {
                        prefix.is_empty()
                            || *path == &prefix
                            || path
                                .strip_prefix(&prefix)
                                .is_some_and(|rest| rest.starts_with('/'))
                    })
                    .filter(|(path, _)| self.authorize_path(path, false).is_ok())
                    .map(|(path, entry)| version(path, entry))
                    .collect();
                Ok(OperationResult::Search { files })
            }
            Operation::Create { path, content } => {
                self.authorize_path(&path, true)?;
                validate_content(&content)?;
                if self.files.contains_key(&path) {
                    return Err(ToolError::AlreadyExists);
                }
                validate_names(
                    self.files
                        .keys()
                        .map(String::as_str)
                        .chain(std::iter::once(path.as_str())),
                )?;
                if self.files.len() >= MAX_FILES
                    || self.total_bytes + content.len() > MAX_TOTAL_BYTES
                {
                    return Err(ToolError::ResourceLimit);
                }
                self.prepare_parents(&path).map_err(|error| {
                    if error == ToolError::Io {
                        ToolError::MutationUnknown
                    } else {
                        error
                    }
                })?;
                let mut file = open_options(false, true)
                    .create_new(true)
                    .open(self.root.join(&path))
                    .map_err(|error| {
                        if error.kind() == std::io::ErrorKind::AlreadyExists {
                            ToolError::WorkspaceChanged
                        } else {
                            ToolError::MutationUnknown
                        }
                    })?;
                file.write_all(content.as_bytes())
                    .and_then(|_| file.sync_all())
                    .map_err(|_| ToolError::MutationUnknown)?;
                self.record_change(path, content, sequence)
            }
            Operation::Edit {
                path,
                expected_revision,
                content,
            } => {
                self.authorize_path(&path, true)?;
                validate_content(&content)?;
                let entry = self.files.get(&path).ok_or(ToolError::Missing)?;
                if entry.revision != expected_revision {
                    return Err(ToolError::StaleRevision);
                }
                if self.total_bytes - entry.byte_count + content.len() > MAX_TOTAL_BYTES {
                    return Err(ToolError::ResourceLimit);
                }
                let mut file = self.open_existing(&path, true)?;
                read_verified(&mut file, entry)?;
                // Existing-only handle: a disappearing path cannot turn this into creation.
                file.seek(SeekFrom::Start(0)).map_err(|_| ToolError::Io)?;
                file.write_all(content.as_bytes())
                    .and_then(|_| file.set_len(content.len() as u64))
                    .and_then(|_| file.sync_all())
                    .map_err(|_| ToolError::MutationUnknown)?;
                self.record_change(path, content, sequence)
            }
        }
    }

    fn record_change(
        &mut self,
        path: String,
        content: String,
        revision: u64,
    ) -> Result<OperationResult, ToolError> {
        if let Some(old) = self.files.get(&path) {
            self.total_bytes -= old.byte_count;
        }
        self.total_bytes += content.len();
        let entry = Entry {
            revision,
            digest: Sha256Digest::of(content.as_bytes()),
            byte_count: content.len(),
        };
        let result = OperationResult::Changed {
            file: version(&path, &entry),
        };
        self.files.insert(path.clone(), entry);
        self.changed.insert(path);
        Ok(result)
    }

    /// Owner-only export. Deliberately absent from MCP tools.
    pub fn candidate(&mut self) -> Result<Candidate, ToolError> {
        let result = self.export_candidate();
        if result.is_err() {
            self.retain_for_recovery();
        }
        result
    }

    fn export_candidate(&self) -> Result<Candidate, ToolError> {
        if self.quarantined {
            return Err(ToolError::Quarantined);
        }
        self.verify_root()?;
        let mut changed_files = Vec::new();
        for path in &self.changed {
            let entry = self.files.get(path).ok_or(ToolError::WorkspaceChanged)?;
            let content = read_verified(&mut self.open_existing(path, false)?, entry)?;
            changed_files.push(TextFile {
                path: path.clone(),
                content,
            });
        }
        Ok(Candidate {
            schema_version: SCHEMA_VERSION,
            attempt_id: self.bootstrap.attempt_id,
            base_snapshot: self.base_snapshot,
            operations: self.receipts.clone(),
            changed_files,
        })
    }

    /// Owner lifecycle operation for uncertain effects or a failed transport.
    pub fn retain_for_recovery(&mut self) {
        self.quarantined = true;
        if let Some(directory) = self.directory.take() {
            let _ = directory.keep();
        }
    }

    pub fn recovery_record(&self) -> Option<RecoveryRecord> {
        self.quarantined.then(|| RecoveryRecord {
            schema_version: SCHEMA_VERSION,
            attempt_id: self.bootstrap.attempt_id,
            base_snapshot: self.base_snapshot,
            retained_directory: self.root.clone(),
            operations: self.receipts.clone(),
        })
    }

    fn authorize_path(&self, path: &str, mutation: bool) -> Result<(), ToolError> {
        validate_path(path)?;
        if forbidden(&self.bootstrap.forbidden_paths, path) {
            return Err(ToolError::ProtectedPath);
        }
        if self
            .bootstrap
            .owned_paths
            .iter()
            .any(|pattern| pattern.matches(path))
            || (!mutation
                && self
                    .bootstrap
                    .read_scopes
                    .iter()
                    .any(|pattern| pattern.matches(path)))
        {
            Ok(())
        } else {
            Err(ToolError::OutsideScope)
        }
    }

    fn verify_root(&self) -> Result<(), ToolError> {
        let metadata = fs::symlink_metadata(&self.root).map_err(|_| ToolError::WorkspaceChanged)?;
        if !metadata.is_dir()
            || special(&metadata)
            || self
                .root
                .canonicalize()
                .map_err(|_| ToolError::WorkspaceChanged)?
                != self.root
        {
            return Err(ToolError::WorkspaceChanged);
        }
        Ok(())
    }

    fn prepare_parents(&self, path: &str) -> Result<(), ToolError> {
        let mut target = self.root.clone();
        let components: Vec<_> = path.split('/').collect();
        for component in &components[..components.len() - 1] {
            target.push(component);
            match fs::symlink_metadata(&target) {
                Ok(metadata) if metadata.is_dir() && !special(&metadata) => {}
                Ok(_) => return Err(ToolError::WorkspaceChanged),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    fs::create_dir(&target).map_err(|_| ToolError::Io)?;
                }
                Err(_) => return Err(ToolError::Io),
            }
        }
        Ok(())
    }

    fn open_existing(&self, path: &str, write: bool) -> Result<File, ToolError> {
        let mut target = self.root.clone();
        for component in path.split('/') {
            target.push(component);
            let metadata =
                fs::symlink_metadata(&target).map_err(|_| ToolError::WorkspaceChanged)?;
            if special(&metadata) {
                return Err(ToolError::WorkspaceChanged);
            }
        }
        let file = open_options(true, write)
            .open(target)
            .map_err(|_| ToolError::WorkspaceChanged)?;
        let metadata = file.metadata().map_err(|_| ToolError::WorkspaceChanged)?;
        if !metadata.is_file() || special(&metadata) {
            return Err(ToolError::WorkspaceChanged);
        }
        Ok(file)
    }
}

fn version(path: &str, entry: &Entry) -> FileVersion {
    FileVersion {
        path: path.into(),
        revision: entry.revision,
        byte_count: entry.byte_count,
    }
}

/// The caller supplies an already-private parent, preventing other accounts from
/// inserting objects between fresh directory creation and its explicit ACL.
pub fn verify_private_parent(parent: &Path) -> Result<PathBuf, ToolError> {
    let metadata = fs::symlink_metadata(parent).map_err(|_| ToolError::Io)?;
    if !metadata.is_dir() || special(&metadata) {
        return Err(ToolError::InvalidPath);
    }
    let canonical = parent.canonicalize().map_err(|_| ToolError::Io)?;
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        if !matches!(canonical.components().next(),Some(Component::Prefix(prefix)) if matches!(prefix.kind(),Prefix::Disk(_)|Prefix::VerbatimDisk(_)))
        {
            return Err(ToolError::InvalidPath);
        }
        vibemux_platform::verify_restricted_path_acl(&canonical).map_err(|_| ToolError::Io)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(ToolError::Io);
        }
    }
    Ok(canonical)
}

fn open_options(read: bool, write: bool) -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(read).write(write);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0).custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    options
}

fn read_verified(file: &mut File, entry: &Entry) -> Result<String, ToolError> {
    let mut bytes = Vec::new();
    file.take((MAX_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| ToolError::Io)?;
    if bytes.len() != entry.byte_count || Sha256Digest::of(&bytes) != entry.digest {
        return Err(ToolError::WorkspaceChanged);
    }
    String::from_utf8(bytes).map_err(|_| ToolError::WorkspaceChanged)
}

fn special(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0 // FILE_ATTRIBUTE_REPARSE_POINT
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

pub fn validate_path(path: &str) -> Result<(), ToolError> {
    if !is_relative_path(path) || path.split('/').count() > MAX_PATH_DEPTH {
        return Err(ToolError::InvalidPath);
    }
    for segment in path.split('/') {
        let lower = segment.to_ascii_lowercase();
        if PROTECTED_COMPONENTS.contains(&lower.as_str()) || lower == OWNER_RECORD {
            return Err(ToolError::ProtectedPath);
        }
        let base = lower.split('.').next().unwrap_or_default();
        let device = ["con", "prn", "aux", "nul"].contains(&base)
            || (base.len() == 4
                && (base.starts_with("com") || base.starts_with("lpt"))
                && matches!(base.as_bytes()[3], b'1'..=b'9'));
        if segment.ends_with('.') || device {
            return Err(ToolError::InvalidPath);
        }
    }
    Ok(())
}

pub(crate) fn validate_content(content: &str) -> Result<(), ToolError> {
    if content.len() > MAX_FILE_BYTES || content.contains('\0') {
        Err(ToolError::ContentLimit)
    } else {
        Ok(())
    }
}

pub(crate) fn forbidden(patterns: &[PathPattern], path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    patterns.iter().any(|pattern| {
        PathPattern::new(pattern.as_str().to_ascii_lowercase())
            .is_ok_and(|pattern| pattern.matches(&lower))
    })
}

pub(crate) fn validate_names<'a>(paths: impl Iterator<Item = &'a str>) -> Result<(), ToolError> {
    let mut names: BTreeMap<String, (String, bool)> = BTreeMap::new();
    for path in paths {
        let segments: Vec<_> = path.split('/').collect();
        for count in 1..=segments.len() {
            let current = segments[..count].join("/");
            let is_file = count == segments.len();
            if let Some((prior, prior_file)) = names.get(&current.to_ascii_lowercase()) {
                if prior != &current || is_file || *prior_file {
                    return Err(ToolError::InvalidPath);
                }
            } else {
                names.insert(current.to_ascii_lowercase(), (current, is_file));
            }
        }
    }
    Ok(())
}

/// Canonical identity of a valid schema-1 input's text snapshot. File order
/// does not affect identity; authority fields remain separately owner-bound.
pub fn snapshot_digest(bootstrap: &Bootstrap) -> Result<Sha256Digest, ToolError> {
    validate_bootstrap(bootstrap)?;
    let mut files: Vec<_> = bootstrap.files.iter().collect();
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let encoded = serde_json::to_vec(&files).map_err(|_| ToolError::InvalidBootstrap)?;
    Ok(Sha256Digest::of_fields(SNAPSHOT_DOMAIN, &[&encoded]))
}

pub(crate) fn validate_bootstrap(bootstrap: &Bootstrap) -> Result<(), ToolError> {
    if bootstrap.schema_version != SCHEMA_VERSION
        || bootstrap.attempt_id.is_nil()
        || bootstrap.files.len() > MAX_FILES
        || bootstrap.owned_paths.len() > MAX_FILES
        || bootstrap.read_scopes.len() > MAX_FILES
        || bootstrap.forbidden_paths.len() > MAX_FILES
    {
        return Err(ToolError::InvalidBootstrap);
    }
    if bootstrap.grants.run_dev_tests {
        return Err(ToolError::ExecutionUnsupported);
    }
    let mut total = 0;
    for pattern in bootstrap.owned_paths.iter().chain(&bootstrap.read_scopes) {
        validate_path(pattern.base())?;
    }
    for file in &bootstrap.files {
        validate_path(&file.path)?;
        validate_content(&file.content)?;
        if forbidden(&bootstrap.forbidden_paths, &file.path) {
            return Err(ToolError::ProtectedPath);
        }
        if !bootstrap
            .owned_paths
            .iter()
            .chain(&bootstrap.read_scopes)
            .any(|pattern| pattern.matches(&file.path))
        {
            return Err(ToolError::OutsideScope);
        }
        total += file.content.len();
        if total > MAX_TOTAL_BYTES {
            return Err(ToolError::ResourceLimit);
        }
    }
    validate_names(bootstrap.files.iter().map(|file| file.path.as_str()))
}
