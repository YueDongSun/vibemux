//! Opaque renderer handles bound to the daemon's canonical project identity.
//!
//! Holding a directory handle lets a replaced directory fail closed during the
//! app session. Neither path spelling nor a renderer-provided UUID is authority.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, UNIX_EPOCH},
};

const SUBMISSION_WAIT: Duration = Duration::from_secs(15);

use same_file::Handle;
use uuid::Uuid;
use vibemux_client::project_folder;
use vibemuxd::process::DaemonPaths;

use crate::{
    command_error::CommandError,
    desktop_settings::{ConsentIdentity, DesktopSettings, MAX_PROJECT_FOLDERS},
};

pub struct RegisteredProject {
    pub key: String,
    pub name: String,
    pub folder: PathBuf,
    paths: Option<DaemonPaths>,
    identity: Option<Handle>,
    created_nanos: Option<u128>,
    busy: Arc<AtomicBool>,
    send_gate: Arc<SendGate>,
    revoked: AtomicBool,
    pub confirmed: AtomicBool,
    pub chat_confirmed: AtomicBool,
}

impl RegisteredProject {
    fn new(folder: PathBuf, settings: &DesktopSettings) -> Self {
        let paths = DaemonPaths::from_project_root(&folder).ok();
        let identity = paths
            .as_ref()
            .and_then(|paths| Handle::from_path(paths.project_root()).ok());
        let created_nanos = identity.as_ref().and_then(created_nanos);
        let confirmed = paths.as_ref().is_some_and(|paths| {
            settings
                .ask_confirmed_projects
                .iter()
                .any(|path| path == paths.project_root())
                && settings.ask_consent_identities.iter().any(|receipt| {
                    receipt.project_root == paths.project_root()
                        && Some(receipt.created_nanos) == created_nanos
                })
        });
        let chat_confirmed = paths.as_ref().is_some_and(|paths| {
            settings.chat_consent_identities.iter().any(|receipt| {
                receipt.project_root == paths.project_root()
                    && Some(receipt.created_nanos) == created_nanos
            })
        });
        // A drive root's display_name would reveal its full path. Keep the UI
        // label bounded and use a fixed fallback instead.
        let name = folder
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Project folder".to_owned());
        Self {
            key: Uuid::new_v4().to_string(),
            name: name.chars().filter(|c| !c.is_control()).take(120).collect(),
            folder,
            paths,
            identity,
            created_nanos,
            busy: Arc::new(AtomicBool::new(false)),
            send_gate: Arc::new(SendGate::default()),
            revoked: AtomicBool::new(false),
            confirmed: AtomicBool::new(confirmed),
            chat_confirmed: AtomicBool::new(chat_confirmed),
        }
    }

    pub fn resolve(&self) -> Result<DaemonPaths, CommandError> {
        if self.revoked.load(Ordering::Acquire) {
            return Err(failure(
                "unknown_project",
                "This project registration was forgotten. Choose the folder again.",
            ));
        }
        project_folder::existing_folder(&self.folder).map_err(|_| {
            failure(
                "project_folder_missing",
                "The registered folder is missing. Choose it again or forget it.",
            )
        })?;
        let paths = DaemonPaths::from_project_root(&self.folder).map_err(|_| {
            failure(
                "project_identity_unavailable",
                "The project identity could not be verified.",
            )
        })?;
        let current = Handle::from_path(paths.project_root()).map_err(|_| {
            failure(
                "project_identity_unavailable",
                "The project identity could not be verified.",
            )
        })?;
        if self
            .paths
            .as_ref()
            .is_none_or(|registered| registered.project_root() != paths.project_root())
            || self.identity.as_ref() != Some(&current)
            || self.created_nanos != created_nanos(&current)
        {
            self.confirmed.store(false, Ordering::Release);
            return Err(failure(
                "project_identity_changed",
                "The folder identity changed. Forget this registration and choose the folder again.",
            ));
        }
        Ok(paths)
    }

    pub(crate) fn is_registered(&self) -> bool {
        !self.revoked.load(Ordering::Acquire)
    }

    pub fn consent_identity(&self) -> Option<ConsentIdentity> {
        Some(ConsentIdentity {
            project_root: self.paths.as_ref()?.project_root().to_owned(),
            created_nanos: self.created_nanos?,
        })
    }

    /// A job lease, not a registry lock. No lock is held while doing I/O.
    pub fn acquire_job(&self) -> Result<ProjectJob, CommandError> {
        self.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| {
                failure(
                    "daemon_job_busy",
                    "Another project operation is still running.",
                )
            })?;
        let job = ProjectJob(Arc::clone(&self.busy));
        if self.revoked.load(Ordering::Acquire) {
            drop(job);
            return Err(failure(
                "unknown_project",
                "This project registration was forgotten. Choose the folder again.",
            ));
        }
        Ok(job)
    }

    /// Serialize concurrent send preflight, then let a duplicate read the first
    /// receipt. The condition lock is released before any I/O or worker launch.
    pub fn acquire_submission(&self) -> Result<SubmissionLease, CommandError> {
        let busy = self.send_gate.busy.lock().map_err(|_| {
            failure(
                "ask_registry_unavailable",
                "Submission state is unavailable.",
            )
        })?;
        let (mut busy, _) = self
            .send_gate
            .ready
            .wait_timeout_while(busy, SUBMISSION_WAIT, |busy| *busy)
            .map_err(|_| {
                failure(
                    "ask_registry_unavailable",
                    "Submission state is unavailable.",
                )
            })?;
        if *busy {
            return Err(failure(
                "ask_submission_busy",
                "A submission is still being checked. Retry with the same submission key.",
            ));
        }
        *busy = true;
        Ok(SubmissionLease(Arc::clone(&self.send_gate)))
    }
}

#[derive(Default)]
struct SendGate {
    busy: Mutex<bool>,
    ready: Condvar,
}

pub struct SubmissionLease(Arc<SendGate>);
impl Drop for SubmissionLease {
    fn drop(&mut self) {
        if let Ok(mut busy) = self.0.busy.lock() {
            *busy = false;
            self.0.ready.notify_one();
        }
    }
}

fn created_nanos(handle: &Handle) -> Option<u128> {
    handle
        .as_file()
        .metadata()
        .ok()?
        .created()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|value| value.as_nanos())
}

pub struct ProjectJob(Arc<AtomicBool>);

impl Drop for ProjectJob {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[derive(Default)]
pub struct ProjectRegistry {
    entries: Vec<Arc<RegisteredProject>>,
    selected: Option<String>,
}

impl ProjectRegistry {
    pub fn from_settings(settings: &DesktopSettings) -> Self {
        let mut registry = Self::default();
        for folder in settings.project_folders.iter().take(MAX_PROJECT_FOLDERS) {
            let candidate = RegisteredProject::new(folder.clone(), settings);
            if !registry
                .entries
                .iter()
                .any(|entry| same_registration(entry, &candidate))
            {
                registry.entries.push(Arc::new(candidate));
            }
        }
        registry
    }

    pub fn register(
        &mut self,
        folder: &Path,
        settings: &DesktopSettings,
    ) -> Result<Arc<RegisteredProject>, CommandError> {
        if !folder.is_absolute() {
            return Err(failure(
                "project_folder_invalid",
                "Choose an existing project folder.",
            ));
        }
        let candidate = RegisteredProject::new(folder.to_owned(), settings);
        candidate.resolve()?;
        if let Some(existing) = self
            .entries
            .iter()
            .find(|entry| same_registration(entry, &candidate))
        {
            existing.resolve()?;
            return Ok(Arc::clone(existing));
        }
        if self.entries.len() >= MAX_PROJECT_FOLDERS {
            return Err(failure(
                "project_limit",
                "Forget an inactive project before adding another.",
            ));
        }
        let candidate = Arc::new(candidate);
        self.entries.push(Arc::clone(&candidate));
        Ok(candidate)
    }

    pub fn get(&self, key: &str) -> Result<Arc<RegisteredProject>, CommandError> {
        self.entries
            .iter()
            .find(|entry| entry.key == key)
            .cloned()
            .ok_or_else(|| failure("unknown_project", "Choose a registered project."))
    }

    pub fn entries(&self) -> Vec<Arc<RegisteredProject>> {
        self.entries.clone()
    }

    pub fn select(&mut self, key: &str) -> Result<(), CommandError> {
        self.get(key)?.resolve()?;
        self.selected = Some(key.to_owned());
        Ok(())
    }

    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub fn forget(&mut self, key: &str) {
        if let Some(entry) = self.entries.iter().find(|entry| entry.key == key) {
            entry.revoked.store(true, Ordering::Release);
            entry.confirmed.store(false, Ordering::Release);
        }
        self.entries.retain(|entry| entry.key != key);
        if self.selected.as_deref() == Some(key) {
            self.selected = None;
        }
    }
}

fn same_registration(left: &RegisteredProject, right: &RegisteredProject) -> bool {
    match (&left.paths, &right.paths, &left.identity, &right.identity) {
        (Some(left_paths), Some(right_paths), Some(left_id), Some(right_id)) => {
            left_paths.project_root() == right_paths.project_root() && left_id == right_id
        }
        _ => left.folder == right.folder,
    }
}

pub(crate) fn failure(code: &str, message: &str) -> CommandError {
    CommandError::new(code, message)
}
