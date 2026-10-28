//! Private owner of one file-only broker attempt.
//!
//! The admitted working directory remains the dispatch reservation. The controller
//! runs separately and receives only a bounded snapshot through its broker.
//! This is fixture integration, not an OS sandbox against same-user programs.

use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use serde::Deserialize;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::watch;
use uuid::Uuid;
use vibemux_harness::dispatch::{DispatchError, writable_profile::WritableGrant};
use vibemux_worker_tools::{
    contract::{
        Bootstrap, Candidate, MAX_BOOTSTRAP_BYTES, MAX_FILE_BYTES, MAX_FILES, MAX_TOTAL_BYTES,
        SCHEMA_VERSION, TextFile,
    },
    owner_admission::{snapshot_digest, validate_candidate},
    workspace::verify_private_parent,
};
use vibemux_workflow::{PathPattern, identifiers::is_relative_path};

use super::{WorkflowTurnBinding, native_process::cancellation_requested};

#[cfg(feature = "test_helpers")]
use super::native_process::LaunchPlan;

const MAX_VISITED_ENTRIES: usize = 4096;
const INVALID: DispatchError = DispatchError::BrokerCandidateInvalid;

/// Effective task scope, constructed by the coordinator from its admitted plan.
pub(crate) struct BrokerScope {
    pub scratch_parent: PathBuf,
    pub owned_paths: Vec<PathPattern>,
    pub read_scopes: Vec<PathPattern>,
    pub forbidden_paths: Vec<PathPattern>,
}

#[derive(Deserialize)]
#[serde(tag = "status", deny_unknown_fields)]
enum OwnerPublication {
    #[serde(rename = "candidate")]
    Candidate { candidate: Candidate },
}

pub(super) struct BrokerOwner {
    private: Option<TempDir>,
    retained: bool,
    directory: PathBuf,
    workspace: PathBuf,
    bootstrap: Bootstrap,
}

impl BrokerOwner {
    pub(super) fn prepare(
        workspace: &Path,
        scope: BrokerScope,
        request_id: Uuid,
        grants: WritableGrant,
        binding: &WorkflowTurnBinding,
        profile: &str,
    ) -> Result<Self, DispatchError> {
        let multithreaded = tokio::runtime::Handle::try_current().is_ok_and(|handle| {
            handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
        });
        if !cfg!(windows) || grants.run_dev_tests || !multithreaded {
            return Err(DispatchError::ExecutionDisabled);
        }
        reject_links(workspace)?;
        let workspace = fs::canonicalize(workspace).map_err(|_| INVALID)?;
        fs::create_dir_all(&scope.scratch_parent).map_err(|_| INVALID)?;
        verify_private_parent(&scope.scratch_parent).map_err(|_| INVALID)?;
        let private = tempfile::Builder::new()
            .prefix("file_broker_")
            .tempdir_in(&scope.scratch_parent)
            .map_err(|_| INVALID)?;
        let directory = fs::canonicalize(private.path()).map_err(|_| INVALID)?;
        verify_private_parent(&directory).map_err(|_| INVALID)?;
        let mut bootstrap = Bootstrap {
            schema_version: SCHEMA_VERSION,
            attempt_id: request_id,
            grants,
            owned_paths: scope.owned_paths,
            read_scopes: scope.read_scopes,
            forbidden_paths: scope.forbidden_paths,
            files: vec![],
        };
        bootstrap.files = capture_files(&workspace, &bootstrap)?;
        let input_digest = snapshot_digest(&bootstrap).map_err(|_| INVALID)?;
        let bytes = serde_json::to_vec(&bootstrap).map_err(|_| INVALID)?;
        if bytes.len() > MAX_BOOTSTRAP_BYTES {
            return Err(INVALID);
        }
        fs::write(directory.join("bootstrap.json"), bytes).map_err(|_| INVALID)?;
        // This private record is daemon-owned. The candidate cannot supply or
        // replace lease/contract/profile authority through its wire envelope.
        let owner = json!({
            "schema_version": 1, "request_id": request_id,
            "workflow_id": binding.workflow_id, "lease_id": binding.lease_id,
            "lease_generation": binding.lease_generation, "contract_id": binding.contract_id,
            "task_key": binding.task_key, "purpose": binding.purpose,
            "session_id": binding.session_id, "profile": profile,
            "input_snapshot": input_digest,
        });
        fs::write(
            directory.join("owner.json"),
            serde_json::to_vec(&owner).map_err(|_| INVALID)?,
        )
        .map_err(|_| INVALID)?;
        fs::create_dir(directory.join("controller")).map_err(|_| INVALID)?;
        fs::create_dir(directory.join("scratch")).map_err(|_| INVALID)?;
        Ok(Self {
            private: Some(private),
            retained: true,
            directory,
            workspace,
            bootstrap,
        })
    }

    #[cfg(feature = "test_helpers")]
    pub(super) fn configure_launch(&self, launch: &mut LaunchPlan) -> Result<(), DispatchError> {
        launch.working_directory = self.directory.join("controller");
        for (flag, name) in [
            ("--broker_bootstrap", "bootstrap.json"),
            ("--broker_scratch", "scratch"),
            ("--broker_candidate", "candidate.json"),
        ] {
            launch.spec.arguments.extend([
                flag.to_owned(),
                super::child_path_text(&self.directory.join(name)).ok_or(INVALID)?,
            ]);
        }
        Ok(())
    }

    #[cfg(feature = "test_helpers")]
    pub(super) fn configure_installed_controller(
        &self,
        launch: &mut LaunchPlan,
        guard: &vibemux_harness::dispatch::installed_controller_guard::InstalledClaudeGuard,
    ) -> Result<(), DispatchError> {
        let home = self.directory.join("controller_home");
        let temporary = self.directory.join("controller_temp");
        fs::create_dir(&home).map_err(|_| INVALID)?;
        fs::create_dir(&temporary).map_err(|_| INVALID)?;
        let mcp_path = self.directory.join("controller_mcp.json");
        let settings_path = self.directory.join("controller_settings.json");
        let child_text = |path: &Path| super::child_path_text(path).ok_or(INVALID);
        let mcp = json!({"mcpServers":{"worker":{
            "type":"stdio", "command":child_text(guard.broker_executable())?,
            "args":["--bootstrap",child_text(&self.directory.join("bootstrap.json"))?,
                "--scratch-parent",child_text(&self.directory.join("scratch"))?,
                "--candidate-out",child_text(&self.directory.join("candidate.json"))?]
        }}});
        fs::write(&mcp_path, serde_json::to_vec(&mcp).map_err(|_| INVALID)?)
            .map_err(|_| INVALID)?;
        fs::write(&settings_path, br#"{"disableAllHooks":true}"#).map_err(|_| INVALID)?;
        launch.spec.arguments.extend([
            "--mcp-config".into(),
            child_text(&mcp_path)?,
            "--settings".into(),
            child_text(&settings_path)?,
        ]);
        launch.working_directory = self.directory.join("controller");
        launch.environment = Some(guard.environment(&home, &temporary));
        Ok(())
    }

    /// Called only after the executor confirms that all owned processes and
    /// their output readers have stopped, while the attempt is still reserved.
    pub(super) fn admit_candidate(
        &mut self,
        cancel: &watch::Receiver<bool>,
        lease_loss: Option<&watch::Receiver<bool>>,
    ) -> Result<(), DispatchError> {
        if interrupted(cancel, lease_loss) {
            return Err(DispatchError::Cancelled);
        }
        let bytes = read_regular(&self.directory.join("candidate.json"), MAX_BOOTSTRAP_BYTES)?;
        let OwnerPublication::Candidate { candidate } =
            serde_json::from_slice(&bytes).map_err(|_| INVALID)?;
        let changed = validate_candidate(&self.bootstrap, candidate)
            .map_err(|_| INVALID)?
            .into_changed_files();
        // Compare the complete admitted input again before any import. Previous
        // repair turns are already in this snapshot and therefore remain intact.
        let mut current = self.bootstrap.clone();
        current.files = capture_files(&self.workspace, &current)?;
        if snapshot_digest(&current).map_err(|_| INVALID)?
            != snapshot_digest(&self.bootstrap).map_err(|_| INVALID)?
        {
            return Err(INVALID);
        }
        if interrupted(cancel, lease_loss) {
            return Err(DispatchError::Cancelled);
        }
        for file in changed {
            if interrupted(cancel, lease_loss) {
                return Err(DispatchError::Cancelled);
            }
            let original = self
                .bootstrap
                .files
                .iter()
                .find(|input| input.path == file.path);
            materialize_file(&self.workspace, &file, original, cancel, lease_loss)?;
        }
        if interrupted(cancel, lease_loss) {
            return Err(DispatchError::Cancelled);
        }
        Ok(())
    }

    /// Cleanup is allowed only after the exact fenced completion is confirmed.
    pub(super) fn confirm_completion(&mut self) {
        self.retained = false;
    }
}

impl Drop for BrokerOwner {
    fn drop(&mut self) {
        if self.retained
            && let Some(private) = self.private.take()
        {
            let _ = private.keep();
        }
    }
}

/// The lease receiver is sticky and owned by the heartbeat. Inspect it directly
/// during synchronous import; the async forwarding wrapper cannot poll then.
pub(super) fn interrupted(
    cancel: &watch::Receiver<bool>,
    lease_loss: Option<&watch::Receiver<bool>>,
) -> bool {
    cancellation_requested(cancel) || lease_loss.is_some_and(cancellation_requested)
}

fn capture_files(root: &Path, bootstrap: &Bootstrap) -> Result<Vec<TextFile>, DispatchError> {
    reject_links(root)?;
    let mut capture = Capture {
        root,
        bootstrap,
        files: BTreeMap::new(),
        visited: 0,
        bytes: 0,
    };
    for scope in bootstrap.owned_paths.iter().chain(&bootstrap.read_scopes) {
        capture.visit(scope.base(), scope.is_recursive())?;
    }
    Ok(capture
        .files
        .into_iter()
        .map(|(path, content)| TextFile { path, content })
        .collect())
}

struct Capture<'a> {
    root: &'a Path,
    bootstrap: &'a Bootstrap,
    files: BTreeMap<String, String>,
    visited: usize,
    bytes: usize,
}

impl Capture<'_> {
    fn visit(&mut self, relative: &str, recursive: bool) -> Result<(), DispatchError> {
        self.visited += 1;
        if self.visited > MAX_VISITED_ENTRIES || !is_relative_path(relative) {
            return Err(INVALID);
        }
        if forbidden(self.bootstrap, relative) {
            return Ok(());
        }
        let path = self.root.join(relative);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(INVALID),
            Ok(_) => reject_links(&path)?,
        }
        let metadata = fs::metadata(&path).map_err(|_| INVALID)?;
        if metadata.is_dir() {
            if !recursive {
                return Err(INVALID);
            }
            for entry in fs::read_dir(&path).map_err(|_| INVALID)? {
                let entry = entry.map_err(|_| INVALID)?;
                let name = entry.file_name().into_string().map_err(|_| INVALID)?;
                self.visit(&format!("{relative}/{name}"), true)?;
            }
        } else if metadata.is_file() {
            if !self
                .bootstrap
                .owned_paths
                .iter()
                .chain(&self.bootstrap.read_scopes)
                .any(|scope| scope.matches(relative))
            {
                return Ok(());
            }
            if self.files.contains_key(relative) {
                return Ok(());
            }
            if self.files.len() >= MAX_FILES {
                return Err(INVALID);
            }
            let bytes = read_regular(&path, MAX_FILE_BYTES)?;
            self.bytes = self.bytes.checked_add(bytes.len()).ok_or(INVALID)?;
            if self.bytes > MAX_TOTAL_BYTES {
                return Err(INVALID);
            }
            let content = String::from_utf8(bytes).map_err(|_| INVALID)?;
            self.files.insert(relative.to_owned(), content);
        } else {
            return Err(INVALID);
        }
        Ok(())
    }
}

fn forbidden(bootstrap: &Bootstrap, relative: &str) -> bool {
    let lower = relative.to_ascii_lowercase();
    bootstrap.forbidden_paths.iter().any(|scope| {
        let pattern = scope.as_str().to_ascii_lowercase();
        let base = pattern.strip_suffix("/**").unwrap_or(&pattern);
        lower == base
            || lower
                .strip_prefix(base)
                .is_some_and(|suffix| suffix.starts_with('/'))
    })
}

fn read_regular(path: &Path, maximum: usize) -> Result<Vec<u8>, DispatchError> {
    reject_links(path)?;
    let file = File::open(path).map_err(|_| INVALID)?;
    let metadata = file.metadata().map_err(|_| INVALID)?;
    if !metadata.is_file() || metadata.len() > maximum as u64 {
        return Err(INVALID);
    }
    let mut bytes = Vec::new();
    file.take((maximum + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| INVALID)?;
    if bytes.len() > maximum || bytes.len() as u64 != metadata.len() {
        return Err(INVALID);
    }
    Ok(bytes)
}

fn reject_links(path: &Path) -> Result<(), DispatchError> {
    if !path.is_absolute() {
        return Err(INVALID);
    }
    for ancestor in path.ancestors() {
        let metadata = fs::symlink_metadata(ancestor).map_err(|_| INVALID)?;
        if metadata.file_type().is_symlink() {
            return Err(INVALID);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(INVALID);
            }
        }
    }
    Ok(())
}

fn materialize_file(
    root: &Path,
    file: &TextFile,
    original: Option<&TextFile>,
    cancel: &watch::Receiver<bool>,
    lease_loss: Option<&watch::Receiver<bool>>,
) -> Result<(), DispatchError> {
    let target = root.join(&file.path);
    let parent = target.parent().ok_or(INVALID)?;
    create_parents(root, parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|_| INVALID)?;
    temporary
        .write_all(file.content.as_bytes())
        .map_err(|_| INVALID)?;
    temporary.as_file().sync_all().map_err(|_| INVALID)?;
    publish_staged(temporary, &target, original, cancel, lease_loss)
}

/// The publication fence follows staging, so slow I/O cannot defer interruption
/// checks until after the next independently avoidable workspace mutation.
fn publish_staged(
    temporary: tempfile::NamedTempFile,
    target: &Path,
    original: Option<&TextFile>,
    cancel: &watch::Receiver<bool>,
    lease_loss: Option<&watch::Receiver<bool>>,
) -> Result<(), DispatchError> {
    // Staging/sync is non-preemptible. Fence the next mutation after it returns.
    if interrupted(cancel, lease_loss) {
        return Err(DispatchError::Cancelled);
    }
    if let Some(original) = original {
        // Managed worktrees have one admitted writer. Detect unexpected
        // external edits again at each replacement; this is not OS CAS.
        if read_regular(target, MAX_FILE_BYTES)? != original.content.as_bytes() {
            return Err(INVALID);
        }
    }

    // The input recheck itself performs blocking reads. Fence once more after
    // those reads, immediately before starting the next filesystem mutation.
    if interrupted(cancel, lease_loss) {
        return Err(DispatchError::Cancelled);
    }
    if original.is_some() {
        temporary.persist(target).map_err(|_| INVALID)?;
    } else {
        // A create grant never authorizes replacement, including a file
        // that appeared after the aggregate input comparison.
        temporary.persist_noclobber(target).map_err(|_| INVALID)?;
    }
    Ok(())
}

fn create_parents(root: &Path, parent: &Path) -> Result<(), DispatchError> {
    let relative = parent.strip_prefix(root).map_err(|_| INVALID)?;
    let mut current = root.to_path_buf();
    reject_links(&current)?;
    for part in relative.components() {
        current.push(part);
        match fs::create_dir(&current) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(_) => return Err(INVALID),
        }
        reject_links(&current)?;
        if !current.is_dir() {
            return Err(INVALID);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vibemux_worker_tools::{contract::Operation, workspace::WorkerTools};

    fn owner() -> (TempDir, BrokerOwner) {
        let root = tempfile::tempdir().unwrap();
        #[cfg(windows)]
        vibemux_platform::secure_user_directory(root.path()).unwrap();
        let workspace = root.path().join("work");
        fs::create_dir(&workspace).unwrap();
        fs::create_dir(workspace.join("src")).unwrap();
        fs::write(workspace.join("src/file.txt"), "original").unwrap();
        let private = tempfile::tempdir_in(root.path()).unwrap();
        let directory = fs::canonicalize(private.path()).unwrap();
        let bootstrap = Bootstrap {
            schema_version: SCHEMA_VERSION,
            attempt_id: Uuid::new_v4(),
            grants: WritableGrant {
                read_files: true,
                search_files: true,
                edit_files: true,
                write_files: true,
                run_dev_tests: false,
            },
            owned_paths: vec![PathPattern::new("src/**").unwrap()],
            read_scopes: vec![],
            forbidden_paths: vec![],
            files: vec![TextFile {
                path: "src/file.txt".into(),
                content: "original".into(),
            }],
        };
        let mut tools = WorkerTools::create(root.path(), bootstrap.clone()).unwrap();
        tools
            .execute(Operation::Edit {
                path: "src/file.txt".into(),
                expected_revision: 0,
                content: "changed".into(),
            })
            .unwrap();
        let candidate = tools.candidate().unwrap();
        fs::write(
            directory.join("candidate.json"),
            serde_json::to_vec(&json!({"status":"candidate","candidate":candidate})).unwrap(),
        )
        .unwrap();
        (
            root,
            BrokerOwner {
                private: Some(private),
                retained: true,
                directory,
                workspace: fs::canonicalize(workspace).unwrap(),
                bootstrap,
            },
        )
    }

    #[test]
    fn changed_input_is_rejected_before_import_and_recovery_is_retained() {
        let (_root, mut owner) = owner();
        let retained = owner.directory.clone();
        fs::write(owner.workspace.join("src/file.txt"), "external edit").unwrap();
        let (_signal, cancel) = watch::channel(false);
        assert_eq!(owner.admit_candidate(&cancel, None), Err(INVALID));
        assert_eq!(
            fs::read_to_string(owner.workspace.join("src/file.txt")).unwrap(),
            "external edit"
        );
        drop(owner);
        assert!(retained.join("candidate.json").is_file());
    }

    #[test]
    fn cancellation_before_import_keeps_input_and_publication() {
        let (_root, mut owner) = owner();
        let retained = owner.directory.clone();
        let (_signal, cancel) = watch::channel(true);
        assert_eq!(
            owner.admit_candidate(&cancel, None),
            Err(DispatchError::Cancelled)
        );
        assert_eq!(
            fs::read_to_string(owner.workspace.join("src/file.txt")).unwrap(),
            "original"
        );
        drop(owner);
        assert!(retained.join("candidate.json").is_file());
    }

    #[test]
    fn imported_output_survives_unconfirmed_finish_and_late_cancellation() {
        let (_root, mut owner) = owner();
        let retained = owner.directory.clone();
        let (signal, cancel) = watch::channel(false);
        owner.admit_candidate(&cancel, None).unwrap();
        assert!(retained.join("candidate.json").is_file());
        assert_eq!(
            fs::read_to_string(owner.workspace.join("src/file.txt")).unwrap(),
            "changed"
        );
        signal.send(true).unwrap();
        // Writer failure, unknown response, or cancellation does not confirm cleanup.
        drop(owner);
        assert!(retained.join("candidate.json").is_file());
    }

    #[test]
    fn confirmed_completion_allows_private_publication_cleanup() {
        let (_root, mut owner) = owner();
        let retained = owner.directory.clone();
        let (_signal, cancel) = watch::channel(false);
        owner.admit_candidate(&cancel, None).unwrap();
        owner.confirm_completion();
        drop(owner);
        assert!(!retained.exists());
    }

    #[test]
    fn unknown_or_extended_publication_envelopes_are_refused() {
        for value in [
            json!({"status":"outcome_unknown","code":"worker_tools_transport_failed"}),
            json!({"status":"candidate","candidate":{},"unexpected":true}),
        ] {
            let (_root, mut owner) = owner();
            fs::write(
                owner.directory.join("candidate.json"),
                serde_json::to_vec(&value).unwrap(),
            )
            .unwrap();
            let (_signal, cancel) = watch::channel(false);
            assert_eq!(owner.admit_candidate(&cancel, None), Err(INVALID));
            assert_eq!(
                fs::read_to_string(owner.workspace.join("src/file.txt")).unwrap(),
                "original"
            );
        }
    }
    #[test]
    fn lost_or_closed_lease_refuses_import_without_a_cancel_forwarder() {
        for closed in [false, true] {
            let (_root, mut owner) = owner();
            let (_signal, cancel) = watch::channel(false);
            let (lease, lease_loss) = watch::channel(false);
            if !closed {
                lease.send_replace(true);
            }
            drop(lease);
            assert!(!cancellation_requested(&cancel));
            assert_eq!(
                owner.admit_candidate(&cancel, Some(&lease_loss)),
                Err(DispatchError::Cancelled)
            );
            assert_eq!(
                fs::read_to_string(owner.workspace.join("src/file.txt")).unwrap(),
                "original"
            );
        }
    }
    #[test]
    fn create_import_never_replaces_an_appeared_target() {
        let (_root, owner) = owner();
        let (_signal, cancel) = watch::channel(false);
        let new_file = TextFile {
            path: "src/new.txt".into(),
            content: "broker".into(),
        };
        let target = owner.workspace.join(&new_file.path);
        fs::write(&target, "concurrent creation").unwrap();
        assert_eq!(
            materialize_file(&owner.workspace, &new_file, None, &cancel, None),
            Err(INVALID)
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "concurrent creation");
    }

    #[test]
    fn existing_import_rechecks_the_original_bytes_at_replacement() {
        let (_root, owner) = owner();
        let (_signal, cancel) = watch::channel(false);
        let changed = TextFile {
            path: "src/file.txt".into(),
            content: "broker".into(),
        };
        fs::write(owner.workspace.join(&changed.path), "concurrent edit").unwrap();
        assert_eq!(
            materialize_file(
                &owner.workspace,
                &changed,
                Some(&owner.bootstrap.files[0]),
                &cancel,
                None
            ),
            Err(INVALID)
        );
        assert_eq!(
            fs::read_to_string(owner.workspace.join(&changed.path)).unwrap(),
            "concurrent edit"
        );
    }
    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn prepared_owner_survives_an_unpolled_or_unknown_claim() {
        let (root, existing) = owner();
        let binding = WorkflowTurnBinding {
            workflow_id: Uuid::new_v4(),
            expected_workflow_version: 1,
            lease_id: Uuid::new_v4(),
            lease_generation: 1,
            task_key: vibemux_workflow::SpecIdentifier::new("task").unwrap(),
            contract_id: vibemux_workflow::Sha256Digest::of(b"fixture"),
            purpose: vibemux_store::AttemptPurpose::Implement,
            session_id: Uuid::new_v4(),
        };
        let prepared = BrokerOwner::prepare(
            &existing.workspace,
            BrokerScope {
                scratch_parent: root.path().to_path_buf(),
                owned_paths: existing.bootstrap.owned_paths.clone(),
                read_scopes: vec![],
                forbidden_paths: vec![],
            },
            Uuid::new_v4(),
            existing.bootstrap.grants,
            &binding,
            "codex_fixture_broker_v1",
        )
        .unwrap();
        let retained = prepared.directory.clone();
        // No execution future is polled: admission/claim may have an unknown outcome.
        drop(prepared);
        assert!(retained.join("bootstrap.json").is_file());
        assert!(retained.join("owner.json").is_file());
    }

    #[test]
    fn a_cancel_or_lease_loss_during_staging_prevents_publication() {
        for lease_lost in [false, true] {
            let (_root, owner) = owner();
            let target = owner.workspace.join("src/file.txt");
            let mut staged = tempfile::NamedTempFile::new_in(target.parent().unwrap()).unwrap();
            staged.write_all(b"staged").unwrap();
            staged.as_file().sync_all().unwrap();
            let (signal, cancel) = watch::channel(false);
            let (lease, loss) = watch::channel(false);
            if lease_lost {
                lease.send_replace(true);
            } else {
                signal.send_replace(true);
            }
            assert_eq!(
                publish_staged(
                    staged,
                    &target,
                    Some(&owner.bootstrap.files[0]),
                    &cancel,
                    Some(&loss)
                ),
                Err(DispatchError::Cancelled)
            );
            assert_eq!(fs::read_to_string(target).unwrap(), "original");
        }
    }

    #[test]
    fn an_edit_during_staging_is_not_overwritten_at_publication() {
        let (_root, owner) = owner();
        let target = owner.workspace.join("src/file.txt");
        let mut staged = tempfile::NamedTempFile::new_in(target.parent().unwrap()).unwrap();
        staged.write_all(b"staged").unwrap();
        staged.as_file().sync_all().unwrap();
        fs::write(&target, "edit during staging").unwrap();
        let (_signal, cancel) = watch::channel(false);
        assert_eq!(
            publish_staged(
                staged,
                &target,
                Some(&owner.bootstrap.files[0]),
                &cancel,
                None
            ),
            Err(INVALID)
        );
        assert_eq!(fs::read_to_string(target).unwrap(), "edit during staging");
    }
}
