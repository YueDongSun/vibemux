//! The state the Tauri app owns: the settings file and the event sink.
//!
//! The daemon credentials never live here. They stay inside
//! `vibemux_client`, which talks to the daemon over the local control
//! channel; the webview only sees the command results.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, RwLock,
    atomic::{AtomicBool, Ordering},
};
use uuid::Uuid;
use vibemux_client::{daemon_control::DaemonCommand, project_folder};

use crate::ask_registry::{AskRegistry, AskSpawner, SharedAskSpawner, validate_submission};
use crate::command_error::CommandError;
use crate::desktop_settings::{
    DesktopSettings, PublicSettings, SettingsPatch, load_settings_at, save_settings_at,
};
use crate::event_sink::EventSink;
use crate::link_effects::{LinkEffects, SystemLinkEffects};
use crate::project_registry::{ProjectRegistry, RegisteredProject, failure};
use crate::project_status::{ProjectServices, SharedProjectServices};
use crate::ui_contract::{AskHarness, AskReceipt, AskTargets, DaemonState, ProjectRow, UiEvent};

struct OperationGuard<'a>(&'a AtomicBool);
struct TaskRunAccess {
    project: Arc<RegisteredProject>,
    paths: vibemuxd::process::DaemonPaths,
    scope: crate::link_bridge::WorkspaceScope,
    generation: Uuid,
}
impl<'a> OperationGuard<'a> {
    fn acquire(flag: &'a AtomicBool) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self(flag))
    }
}
impl Drop for OperationGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

pub struct AppState {
    settings_path: PathBuf,
    settings: RwLock<DesktopSettings>,
    sink: Arc<dyn EventSink>,
    projects: Mutex<ProjectRegistry>,
    project_mutations: Mutex<()>,
    export_job: AtomicBool,
    search_job: AtomicBool,
    terminal_job: AtomicBool,
    link_job: AtomicBool,
    link_effects: Arc<dyn LinkEffects>,
    main_alive: AtomicBool,
    services: Arc<dyn ProjectServices>,
    pub asks: AskRegistry,
    pub probes: crate::probe_registry::ProbeRegistry,
    pub task_windows: crate::task_windows::TaskWindowRegistry,
    pub terminals: crate::terminal_bridge::TerminalBridge,
    main_generation: Uuid,
}

impl AppState {
    #[must_use]
    pub fn new(settings_path: PathBuf, sink: Arc<dyn EventSink>) -> Self {
        Self::with_services(
            settings_path,
            sink,
            Arc::new(SharedProjectServices),
            Arc::new(SharedAskSpawner),
        )
    }

    pub fn with_services(
        settings_path: PathBuf,
        sink: Arc<dyn EventSink>,
        services: Arc<dyn ProjectServices>,
        spawner: Arc<dyn AskSpawner>,
    ) -> Self {
        let settings = load_settings_at(&settings_path);
        let projects = ProjectRegistry::from_settings(&settings);
        let main_generation = Uuid::new_v4();
        Self {
            settings_path,
            settings: RwLock::new(settings),
            sink: Arc::clone(&sink),
            projects: Mutex::new(projects),
            project_mutations: Mutex::new(()),
            export_job: AtomicBool::new(false),
            search_job: AtomicBool::new(false),
            terminal_job: AtomicBool::new(false),
            link_job: AtomicBool::new(false),
            link_effects: Arc::new(SystemLinkEffects::default()),
            main_alive: AtomicBool::new(true),
            services,
            asks: AskRegistry::new(spawner, sink),
            probes: crate::probe_registry::ProbeRegistry::new(Arc::new(
                crate::probe_registry::SharedProbeRunner,
            )),
            task_windows: crate::task_windows::TaskWindowRegistry::default(),
            terminals: crate::terminal_bridge::TerminalBridge::for_main(main_generation),
            main_generation,
        }
    }

    /// Trusted native composition seam; no renderer command selects effects.
    #[must_use]
    pub fn with_link_effects(mut self, effects: Arc<dyn LinkEffects>) -> Self {
        self.link_effects = effects;
        self
    }

    /// Trusted native composition; no webview command selects a probe runner.
    pub fn with_probe_runner(
        mut self,
        runner: Arc<dyn crate::probe_registry::ProbeRunner>,
    ) -> Self {
        self.probes = crate::probe_registry::ProbeRegistry::new(runner);
        self
    }

    pub fn probe_request(
        &self,
        label: &str,
        key: &str,
        request: crate::probe_contract::ProbeRequest,
    ) -> Result<crate::probe_contract::ProbeSnapshot, CommandError> {
        use crate::probe_contract::{ProbeRequest, failure};
        if label != "main" || !self.main_alive.load(Ordering::Acquire) {
            return Err(failure("probe_caller_denied"));
        }
        request.validate()?;
        let project = self.project(key)?;
        if !matches!(request, ProbeRequest::Cancel { .. }) {
            project.resolve()?;
        }
        let _job = if matches!(request, ProbeRequest::Start { .. }) {
            Some(project.acquire_job()?)
        } else {
            None
        };
        let result = self.probes.request(Arc::clone(&project), request)?;
        if !self.main_alive.load(Ordering::Acquire)
            || !self
                .project(key)
                .is_ok_and(|current| Arc::ptr_eq(&current, &project))
        {
            return Err(failure("probe_scope_changed"));
        }
        project.resolve()?;
        Ok(result)
    }

    /// The current settings.
    #[must_use]
    pub fn settings(&self) -> DesktopSettings {
        self.settings
            .read()
            .map(|settings| settings.clone())
            .unwrap_or_default()
    }

    #[must_use]
    pub fn public_settings(&self) -> PublicSettings {
        PublicSettings::from(&self.settings())
    }

    fn update_settings(
        &self,
        apply: impl FnOnce(&mut DesktopSettings),
    ) -> Result<(), CommandError> {
        let mut settings = self.settings.write().map_err(|_| state_failed())?;
        let mut updated = settings.clone();
        apply(&mut updated);
        save_settings_at(&self.settings_path, &updated).map_err(|_| {
            failure(
                "settings_write_failed",
                "The settings file could not be saved.",
            )
        })?;
        *settings = updated;
        Ok(())
    }

    fn project(&self, key: &str) -> Result<Arc<RegisteredProject>, CommandError> {
        self.projects.lock().map_err(|_| state_failed())?.get(key)
    }

    pub fn tasks_snapshot(
        &self,
        key: &str,
        query: crate::read_contract::TaskReadQuery,
    ) -> Result<crate::read_contract::TaskReadResult, CommandError> {
        let project = self.project(key)?;
        let paths = project.resolve()?;
        let result = crate::read_services::tasks_snapshot(&paths, query)
            .map_err(|error| CommandError::new(&error.code, error.message));
        project.resolve()?;
        result
    }

    pub fn prepare_task_window(
        &self,
        key: &str,
        task_id: vibemux_types::TaskId,
    ) -> Result<crate::task_windows::TaskWindowReservation, CommandError> {
        let project = self.project(key)?;
        let result =
            self.tasks_snapshot(key, crate::read_contract::TaskReadQuery::Detail { task_id })?;
        let crate::read_contract::TaskReadResult::Detail { task, .. } = result else {
            return Err(crate::task_windows::denied());
        };
        if task.task.task_id != task_id {
            return Err(crate::task_windows::denied());
        }
        project.resolve()?;
        self.task_windows
            .reserve(crate::task_windows::TaskWindowBinding {
                project_key: key.to_owned(),
                project_id: task.task.project_id,
                task_id,
                project,
            })
    }

    pub fn tasks_snapshot_for_window(
        &self,
        label: &str,
        key: &str,
        query: crate::read_contract::TaskReadQuery,
    ) -> Result<crate::read_contract::TaskReadResult, CommandError> {
        if label == "main" {
            return self.tasks_snapshot(key, query);
        }
        let reservation = self.task_windows.authorize(label, key, &query)?;
        let paths = reservation.binding.project.resolve()?;
        let result = crate::read_services::tasks_snapshot(&paths, query)
            .map_err(|error| CommandError::new(&error.code, error.message))?;
        reservation.binding.project.resolve()?;
        self.task_windows.validate_result(&reservation, &result)?;
        Ok(result)
    }

    pub(crate) fn close_main_view(&self) {
        self.main_alive.store(false, Ordering::Release);
        self.probes.begin_shutdown();
        self.terminals
            .close_window(&crate::terminal_bridge::TerminalActor {
                label: "main".into(),
                generation: self.main_generation,
            });
    }

    fn window_generation(
        &self,
        label: &str,
        key: &str,
        task_id: vibemux_types::TaskId,
    ) -> Result<Uuid, CommandError> {
        if label == "main" {
            if !self.main_alive.load(Ordering::Acquire) {
                return Err(crate::link_bridge::failure("link_caller_unavailable"));
            }
            Ok(self.main_generation)
        } else {
            self.task_windows
                .authorize(
                    label,
                    key,
                    &crate::read_contract::TaskReadQuery::Detail { task_id },
                )
                .map(|view| view.generation)
        }
    }

    fn task_run_access(
        &self,
        label: &str,
        key: &str,
        task_id: vibemux_types::TaskId,
        run_id: vibemux_types::RunId,
    ) -> Result<TaskRunAccess, CommandError> {
        use crate::read_contract::{TaskReadQuery, TaskReadResult};
        let generation = self.window_generation(label, key, task_id)?;
        let project = self.project(key)?;
        let paths = project.resolve()?;
        let detail =
            self.tasks_snapshot_for_window(label, key, TaskReadQuery::Detail { task_id })?;
        let TaskReadResult::Detail { task, .. } = detail else {
            return Err(crate::task_windows::denied());
        };
        if !task.runs.iter().any(|run| run.run_id == run_id) {
            return Err(failure(
                "task_run_unavailable",
                "Select a Run from this task's loaded details.",
            ));
        }
        project.resolve()?;
        Ok(TaskRunAccess {
            project,
            paths,
            generation,
            scope: crate::link_bridge::WorkspaceScope {
                project_id: task.task.project_id,
                task_id,
                run_id,
            },
        })
    }

    fn window_scope_current(&self, label: &str, key: &str, access: &TaskRunAccess) -> bool {
        let matches = if label == "main" {
            access.generation == self.main_generation && self.main_alive.load(Ordering::Acquire)
        } else {
            self.task_windows
                .authorize(
                    label,
                    key,
                    &crate::read_contract::TaskReadQuery::Detail {
                        task_id: access.scope.task_id,
                    },
                )
                .is_ok_and(|view| {
                    view.generation == access.generation
                        && view.binding.project_id == access.scope.project_id
                })
        };
        // The registry guard above is released before filesystem identity I/O.
        matches && access.project.is_registered() && access.project.resolve().is_ok()
    }

    pub fn supervisor_action(
        &self,
        label: &str,
        args: crate::command_arguments::SupervisorArgs,
    ) -> Result<crate::terminal_bridge::TerminalReply, CommandError> {
        use crate::terminal_bridge::{ControlTerminalPort, TerminalActor, TerminalScope};
        let _job = OperationGuard::acquire(&self.terminal_job).ok_or_else(|| {
            failure(
                "desktop_terminal_busy",
                "Another terminal operation is pending.",
            )
        })?;
        let access = self
            .task_run_access(label, &args.project_key, args.task_id, args.run_id)
            .map_err(|error| {
                if error.code == "task_run_unavailable" {
                    failure(
                        "terminal_run_unavailable",
                        "Select a Run from this task's loaded details.",
                    )
                } else {
                    error
                }
            })?;
        let actor = TerminalActor {
            label: label.into(),
            generation: access.generation,
        };
        let scope = TerminalScope {
            project_key: args.project_key,
            project_id: access.scope.project_id,
            task_id: args.task_id,
            run_id: args.run_id,
        };
        let mut port = ControlTerminalPort::connect(&access.paths)?;
        let result = self
            .terminals
            .execute(&actor, &scope, args.request, &mut port, || {
                access.project.is_registered()
            });
        access.project.resolve()?;
        result
    }

    pub fn open_link(
        &self,
        label: &str,
        request: crate::link_contract::OpenLinkRequest,
    ) -> Result<crate::link_contract::OpenLinkReply, CommandError> {
        use crate::{
            link_bridge::{ControlWorkspacePort, failure, worktree_action},
            link_contract::{LinkOutcome, OpenLinkReply, OpenLinkRequest, validate_https},
        };
        let _job =
            OperationGuard::acquire(&self.link_job).ok_or_else(|| failure("link_action_busy"))?;
        if matches!(&request, OpenLinkRequest::Worktree { .. }) {
            request.validate()?;
        }
        match request {
            OpenLinkRequest::Https { url } => {
                if label != "main" {
                    return Err(failure("link_window_forbidden"));
                }
                let url = validate_https(&url)?;
                if !self.main_alive.load(Ordering::Acquire) {
                    return Err(failure("link_caller_unavailable"));
                }
                self.link_effects
                    .open_https(&url)
                    .map_err(|error| failure(&error.code))?;
                if !self.main_alive.load(Ordering::Acquire) {
                    return Err(failure("link_caller_unavailable"));
                }
                Ok(OpenLinkReply::Https {
                    outcome: LinkOutcome::OpenRequested,
                })
            }
            OpenLinkRequest::Worktree {
                project_key,
                task_id,
                run_id,
                action,
            } => {
                let access = self
                    .task_run_access(label, &project_key, task_id, run_id)
                    .map_err(|error| {
                        if error.code == "task_run_unavailable" {
                            failure("workspace_run_unavailable")
                        } else {
                            error
                        }
                    })?;
                let mut port = ControlWorkspacePort::connect(&access.paths)?;
                let result = worktree_action(
                    access.paths.project_root(),
                    &access.scope,
                    action,
                    &mut port,
                    self.link_effects.as_ref(),
                    || self.window_scope_current(label, &project_key, &access),
                );
                access.project.resolve()?;
                result
            }
        }
    }

    pub fn console_section(
        &self,
        key: &str,
        query: crate::read_contract::ConsoleQuery,
    ) -> Result<crate::read_contract::ConsoleSnapshot, CommandError> {
        let project = self.project(key)?;
        let paths = project.resolve()?;
        let result = crate::read_services::console_section(&paths, query)
            .map_err(|error| CommandError::new(&error.code, error.message));
        project.resolve()?;
        result
    }

    /// Called only with a native picker result, or by Rust service tests.
    /// No command takes a path from the renderer.
    pub fn register_project(&self, folder: &Path) -> Result<String, CommandError> {
        let _mutation = self.project_mutations.lock().map_err(|_| state_failed())?;
        let settings = self.settings();
        let (project, existed) = {
            let mut projects = self.projects.lock().map_err(|_| state_failed())?;
            let before = projects.entries().len();
            let project = projects.register(folder, &settings)?;
            let existed = projects.entries().len() == before;
            (project, existed)
        };
        if !existed
            && let Err(error) = self
                .update_settings(|settings| settings.project_folders.push(project.folder.clone()))
        {
            self.projects
                .lock()
                .map_err(|_| state_failed())?
                .forget(&project.key);
            return Err(error);
        }
        self.sink.emit(UiEvent::ProjectsChanged);
        Ok(project.key.clone())
    }

    pub fn pick_project(&self) -> Result<Option<ProjectRow>, CommandError> {
        if !project_folder::native_picker_available() {
            return Err(failure(
                "folder_dialog_unsupported",
                "A native folder picker is unavailable on this platform.",
            ));
        }
        let Some(folder) = project_folder::pick_folder_native(None) else {
            return Ok(None);
        };
        let key = self.register_project(&folder)?;
        self.select_project(&key)?;
        let project = self.project(&key)?;
        self.project_row(&project, true).map(Some)
    }

    pub fn select_project(&self, key: &str) -> Result<(), CommandError> {
        self.projects
            .lock()
            .map_err(|_| state_failed())?
            .select(key)?;
        self.sink.emit(UiEvent::ProjectsChanged);
        Ok(())
    }

    pub fn projects_list(&self) -> Result<Vec<ProjectRow>, CommandError> {
        let (entries, selected) = {
            let registry = self.projects.lock().map_err(|_| state_failed())?;
            (registry.entries(), registry.selected().map(str::to_owned))
        };
        // At most16 project reads; every task is joined within this request.
        // No registry lock crosses the authenticated health calls.
        std::thread::scope(|scope| {
            let jobs = entries
                .iter()
                .map(|entry| {
                    let active = selected.as_deref() == Some(&entry.key);
                    scope.spawn(move || self.project_row(entry, active))
                })
                .collect::<Vec<_>>();
            jobs.into_iter()
                .map(|job| job.join().map_err(|_| state_failed())?)
                .collect()
        })
    }

    fn project_row(
        &self,
        project: &RegisteredProject,
        active: bool,
    ) -> Result<ProjectRow, CommandError> {
        let (daemon, code, missing) = match project.resolve() {
            Ok(paths) => {
                let (daemon, code) = self.services.status(&paths);
                (daemon, code, false)
            }
            Err(error) => (DaemonState::Unreachable, Some(error.code), true),
        };
        Ok(ProjectRow {
            project_key: project.key.clone(),
            name: project.name.clone(),
            daemon,
            code,
            missing,
            active,
            ask_confirmed: !missing && project.confirmed.load(Ordering::Acquire),
            chat_confirmed: !missing && project.chat_confirmed.load(Ordering::Acquire),
            ask_ids: self.asks.ids(&project.key),
        })
    }

    pub fn forget_project(&self, key: &str) -> Result<(), CommandError> {
        let _mutation = self.project_mutations.lock().map_err(|_| state_failed())?;
        let project = self.project(key)?;
        let _job = project.acquire_job()?;
        if self.asks.has_active(key) {
            return Err(failure(
                "ask_busy",
                "An active or unresolved Ask still owns this project.",
            ));
        }
        if self.probes.has_unsettled(key) {
            return Err(crate::probe_contract::failure("probe_busy"));
        }
        let receipt = project.consent_identity();
        self.update_settings(|settings| {
            settings
                .project_folders
                .retain(|folder| folder != &project.folder);
            if let Some(receipt) = receipt {
                settings
                    .ask_confirmed_projects
                    .retain(|folder| folder != &receipt.project_root);
                settings
                    .ask_consent_identities
                    .retain(|stored| stored.project_root != receipt.project_root);
                settings
                    .chat_consent_identities
                    .retain(|stored| stored.project_root != receipt.project_root);
            }
        })?;
        self.asks.forget_display(key)?;
        self.projects
            .lock()
            .map_err(|_| state_failed())?
            .forget(key);
        self.terminals.forget_project(key);
        self.sink.emit(UiEvent::ProjectsChanged);
        Ok(())
    }

    pub fn daemon_action(&self, key: &str, command: DaemonCommand) -> Result<(), CommandError> {
        let project = self.project(key)?;
        let _job = project.acquire_job()?;
        let paths = project.resolve()?;
        self.services.lifecycle(&paths, command)?;
        self.sink.emit(UiEvent::ProjectsChanged);
        Ok(())
    }

    pub fn ask_targets(&self, key: &str) -> Result<AskTargets, CommandError> {
        let project = self.project(key)?;
        Ok(self.services.targets(&project.resolve()?))
    }

    pub fn ask_setup(&self, key: &str) -> Result<(), CommandError> {
        let project = self.project(key)?;
        let _job = project.acquire_job()?;
        if self.asks.has_active(key) {
            return Err(failure(
                "ask_busy",
                "Cancel and reconcile the project's active Ask before setup.",
            ));
        }
        self.services.setup(&project.resolve()?)?;
        self.sink.emit(UiEvent::ProjectsChanged);
        Ok(())
    }

    pub fn ask_confirm(&self, key: &str) -> Result<(), CommandError> {
        let project = self.project(key)?;
        let _job = project.acquire_job()?;
        let paths = project.resolve()?;
        let receipt = project.consent_identity();
        self.update_settings(|settings| {
            settings
                .ask_confirmed_projects
                .retain(|folder| folder != paths.project_root());
            settings
                .ask_confirmed_projects
                .push(paths.project_root().to_owned());
            settings
                .ask_consent_identities
                .retain(|stored| stored.project_root != paths.project_root());
            if let Some(receipt) = receipt {
                settings.ask_consent_identities.push(receipt);
            }
        })?;
        project.confirmed.store(true, Ordering::Release);
        self.sink.emit(UiEvent::ProjectsChanged);
        Ok(())
    }

    pub fn ask_submit(
        &self,
        key: &str,
        harness: AskHarness,
        prompt: String,
        submission_key: Uuid,
    ) -> Result<AskReceipt, CommandError> {
        validate_submission(harness, &prompt, submission_key)?;
        let project = self.project(key)?;
        let _submission = project.acquire_submission()?;
        let paths = project.resolve()?;
        if let Some(receipt) = self.asks.replay(key, harness, &prompt, submission_key)? {
            return Ok(receipt);
        }
        let _job = project.acquire_job()?;
        if !project.confirmed.load(Ordering::Acquire) {
            return Err(failure(
                "ask_consent_required",
                "Confirm this project's inference consent before sending.",
            ));
        }
        if self.asks.has_active(key) {
            return Err(failure(
                "ask_busy",
                "This project already has an active or unresolved Ask.",
            ));
        }
        let targets = self.services.targets(&paths);
        if !targets
            .targets
            .iter()
            .any(|target| target.harness == harness && target.available)
        {
            return Err(failure(
                "harness_not_available",
                "The selected harness has no available, detected one-turn route. Refresh targets for the reason.",
            ));
        }
        project.resolve()?;
        self.asks.submit(project, harness, prompt, submission_key)
    }

    pub fn chat_confirm(&self, key: &str) -> Result<(), CommandError> {
        let project = self.project(key)?;
        let _job = project.acquire_job()?;
        let paths = project.resolve()?;
        let receipt = project.consent_identity().ok_or_else(|| {
            failure(
                "project_identity_unavailable",
                "Choose the project again before confirming chat storage.",
            )
        })?;
        self.update_settings(|settings| {
            settings
                .chat_consent_identities
                .retain(|stored| stored.project_root != paths.project_root());
            settings.chat_consent_identities.push(receipt);
        })?;
        project.chat_confirmed.store(true, Ordering::Release);
        self.sink.emit(UiEvent::ProjectsChanged);
        Ok(())
    }

    pub fn chat_pick_files(
        &self,
        key: &str,
    ) -> Result<Option<Vec<vibemux_client::text_attachment::TextAttachment>>, CommandError> {
        let project = self.project(key)?;
        let _job = project.acquire_job()?;
        let paths = project.resolve()?;
        let selected =
            vibemux_client::text_attachment::pick_text_attachments(Some(paths.project_root()))
                .map_err(|error| CommandError::new(error.code(), error.message()))?;
        project.resolve()?;
        Ok(selected)
    }

    pub fn chat_search_request(
        &self,
        key: &str,
        request: vibemux_client::chat_search::SearchRequest,
    ) -> Result<vibemux_client::chat_search::SearchPage, CommandError> {
        request
            .validate()
            .map_err(|e| failure(e.code(), e.message()))?;
        let _search = OperationGuard::acquire(&self.search_job).ok_or_else(|| {
            failure(
                "chat_search_in_progress",
                "The previous history search is still finishing.",
            )
        })?;
        let project = self.project(key)?;
        let paths = project.resolve()?;
        let result = crate::chat_bridge::search_request(&paths, &request);
        project.resolve()?;
        result
    }

    pub fn chat_export_request(
        &self,
        key: &str,
        request: vibemux_client::chat_export::ExportRequest,
    ) -> Result<vibemux_client::chat_export::ExportReceipt, CommandError> {
        // Export owns its dialog/file operation, independently of daemon jobs.
        // Conversation cancellation stays available while the dialog is open.
        let _export = OperationGuard::acquire(&self.export_job).ok_or_else(|| {
            failure(
                "chat_export_in_progress",
                "Finish the current export before starting another.",
            )
        })?;
        let project = self.project(key)?;
        let paths = project.resolve()?;
        crate::chat_export::request(
            &paths,
            &request,
            vibemux_client::chat_export::pick_export_destination,
            || project.resolve().map(|_| ()),
        )
    }

    pub fn chat_request(
        &self,
        key: &str,
        request: vibemuxd::control::ChatRequest,
    ) -> Result<vibemuxd::control::ChatResponse, CommandError> {
        use vibemuxd::control::ChatRequest;
        let project = self.project(key)?;
        let mutating = matches!(
            &request,
            ChatRequest::Open { .. }
                | ChatRequest::Send { .. }
                | ChatRequest::Cancel { .. }
                | ChatRequest::Rename { .. }
                | ChatRequest::Close { .. }
                | ChatRequest::Archive { .. }
                | ChatRequest::Resume { .. }
                | ChatRequest::Purge { .. }
        );
        let _job = if mutating {
            Some(project.acquire_job()?)
        } else {
            None
        };
        let paths = project.resolve()?;
        if matches!(
            &request,
            ChatRequest::Open { .. } | ChatRequest::Send { .. } | ChatRequest::Resume { .. }
        ) && !project.chat_confirmed.load(Ordering::Acquire)
        {
            return Err(failure(
                "chat_consent_required",
                "Confirm inference and private conversation storage for this project before starting a chat.",
            ));
        }
        let result = crate::chat_bridge::request(&paths, &request);
        project.resolve()?;
        result
    }

    pub fn chat_owner_request(
        &self,
        key: &str,
        request: vibemuxd::control::ChatOwnerRequest,
    ) -> Result<vibemuxd::control::ChatOwnerResponse, CommandError> {
        use vibemuxd::control::ChatOwnerRequest;
        let project = self.project(key)?;
        let _job = match &request {
            ChatOwnerRequest::Open { .. }
            | ChatOwnerRequest::Reconcile { .. }
            | ChatOwnerRequest::Stop { .. } => Some(project.acquire_job()?),
            ChatOwnerRequest::List { .. }
            | ChatOwnerRequest::LookupOpen { .. }
            | ChatOwnerRequest::Status { .. }
            | ChatOwnerRequest::Title { .. } => None,
        };
        let paths = project.resolve()?;
        // Each Open carries explicit workspace and private-storage consent.
        // Read-only chat or Ask consent cannot authorize workspace creation.
        let result = crate::chat_bridge::owner_request(&paths, &request);
        project.resolve()?;
        result
    }

    pub fn chat_execution_request(
        &self,
        key: &str,
        request: vibemuxd::control::ChatExecutionRequest,
    ) -> Result<vibemuxd::control::ChatExecutionResponse, CommandError> {
        request.validate().map_err(|_| {
            failure(
                "invalid_arguments",
                "Check the conversation request and required consent.",
            )
        })?;
        let project = self.project(key)?;
        let _job = if request.is_mutation() {
            Some(project.acquire_job()?)
        } else {
            None
        };
        let paths = project.resolve()?;
        if request.requires_execution_consent() && !project.chat_confirmed.load(Ordering::Acquire) {
            return Err(failure(
                "chat_consent_required",
                "Confirm inference and private conversation storage for this project before starting or sending.",
            ));
        }
        let result = crate::chat_bridge::execution_request(&paths, &request);
        project.resolve()?;
        result
    }

    pub fn chat_approval_request(
        &self,
        key: &str,
        request: vibemuxd::control::ChatApprovalRequest,
    ) -> Result<vibemuxd::control::ChatApprovalResponse, CommandError> {
        request.validate().map_err(|_| {
            failure(
                "invalid_arguments",
                "Check the approval identity, version and decision.",
            )
        })?;
        let project = self.project(key)?;
        // The runtime owns decision serialization. A project-exclusive job
        // would prevent cancellation while its acknowledgement is pending.
        let paths = project.resolve()?;
        let result = crate::chat_bridge::approval_request(&paths, &request);
        project.resolve()?;
        result
    }

    /// Where the settings file lives.
    #[must_use]
    pub fn settings_path(&self) -> &Path {
        &self.settings_path
    }

    /// Applies a patch and writes the file. The write lock is held across
    /// the whole transaction — reading the current settings, applying the
    /// patch, writing the file, and publishing the new value — so
    /// overlapping calls serialize instead of losing updates or racing on
    /// the temporary file. The in-memory copy changes only when the write
    /// succeeds.
    pub fn apply_settings(&self, patch: &SettingsPatch) -> io::Result<()> {
        let mut settings = self
            .settings
            .write()
            .map_err(|_| io::Error::other("the settings lock is poisoned"))?;
        let mut updated = settings.clone();
        updated.apply(patch);
        save_settings_at(&self.settings_path, &updated)?;
        *settings = updated;
        Ok(())
    }

    /// The sink the services emit their events to.
    #[must_use]
    pub fn sink(&self) -> &Arc<dyn EventSink> {
        &self.sink
    }

    pub fn workspace_cleanup_request(
        &self,
        label: &str,
        request: crate::workspace_cleanup_bridge::NativeCleanupRequest,
    ) -> Result<crate::workspace_cleanup_bridge::NativeCleanupReply, CommandError> {
        use crate::workspace_cleanup_bridge::{self, ControlCleanupPort, NativeCleanupRequest};
        if label != "main" {
            return Err(workspace_cleanup_bridge::failure(
                "workspace_cleanup_window_forbidden",
            ));
        }
        if !self.main_alive.load(Ordering::Acquire) {
            return Err(workspace_cleanup_bridge::failure(
                "workspace_cleanup_caller_unavailable",
            ));
        }
        let target = request.target();
        target.validate()?;
        // Read-only recovery does not compete with a native folder handoff.
        // Mutating cleanup admission and presentation use the same app gate;
        // the daemon reservation/gate then covers the admitted effect lifetime.
        let _job = if matches!(&request, NativeCleanupRequest::Status { .. }) {
            None
        } else {
            Some(OperationGuard::acquire(&self.link_job).ok_or_else(|| {
                workspace_cleanup_bridge::failure("workspace_cleanup_native_busy")
            })?)
        };
        let access = self.task_run_access(
            label,
            &target.registration_key,
            target.task_id,
            target.run_id,
        )?;
        let scope = vibemuxd::control::WorkspaceCleanupScope {
            project_id: access.scope.project_id,
            task_id: target.task_id,
            run_id: target.run_id,
        };
        let mut port = ControlCleanupPort::connect(&access.paths)?;
        workspace_cleanup_bridge::perform(request, scope, &mut port, || {
            self.window_scope_current(label, &target.registration_key, &access)
        })
    }
}

impl Drop for AppState {
    fn drop(&mut self) {
        self.probes.shutdown();
        self.asks.shutdown();
    }
}

fn state_failed() -> CommandError {
    failure(
        "desktop_state_unavailable",
        "The desktop state is unavailable.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop_settings::{Appearance, Density};
    use crate::event_sink::RecordingEventSink;
    use std::sync::atomic::AtomicUsize;

    struct HttpsEffects {
        calls: AtomicUsize,
        close: Mutex<Option<std::sync::Weak<AppState>>>,
        fail: bool,
    }
    impl LinkEffects for HttpsEffects {
        fn open_https(&self, _: &url::Url) -> Result<(), CommandError> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            if let Some(state) = self
                .close
                .lock()
                .expect("close slot")
                .as_ref()
                .and_then(std::sync::Weak::upgrade)
            {
                state.close_main_view();
            }
            if self.fail {
                Err(CommandError::new(
                    "private/url?payload",
                    "private native error",
                ))
            } else {
                Ok(())
            }
        }
        fn open_directory(&self, _: &Path) -> Result<(), CommandError> {
            panic!("HTTPS test cannot open a directory")
        }
        fn copy_text(&self, _: &str) -> Result<(), CommandError> {
            panic!("HTTPS test cannot copy text")
        }
    }

    #[test]
    fn https_requires_live_main_and_sanitizes_effect_failures_without_replay() {
        use crate::link_contract::OpenLinkRequest;
        let directory = tempfile::tempdir().expect("settings root");
        let effects = Arc::new(HttpsEffects {
            calls: AtomicUsize::new(0),
            close: Mutex::new(None),
            fail: true,
        });
        let state = AppState::new(
            directory.path().join("settings.json"),
            Arc::new(RecordingEventSink::default()),
        )
        .with_link_effects(effects.clone());
        let request = || OpenLinkRequest::Https {
            url: "https://example.org/?private=one".into(),
        };
        assert_eq!(
            state
                .open_link("task_forged", request())
                .expect_err("main only")
                .code,
            "link_window_forbidden"
        );
        assert_eq!(effects.calls.load(Ordering::Acquire), 0);
        let error = state
            .open_link("main", request())
            .expect_err("failed effect");
        assert_eq!(error.code, "workspace_access_failed");
        assert!(!error.message.contains("private"));
        state.close_main_view();
        assert_eq!(
            state
                .open_link("main", request())
                .expect_err("closed main")
                .code,
            "link_caller_unavailable"
        );
        assert_eq!(effects.calls.load(Ordering::Acquire), 1);
    }

    #[test]
    fn main_closure_during_a_handoff_suppresses_its_acknowledgement() {
        use crate::link_contract::OpenLinkRequest;
        let directory = tempfile::tempdir().expect("settings root");
        let effects = Arc::new(HttpsEffects {
            calls: AtomicUsize::new(0),
            close: Mutex::new(None),
            fail: false,
        });
        let state = Arc::new(
            AppState::new(
                directory.path().join("settings.json"),
                Arc::new(RecordingEventSink::default()),
            )
            .with_link_effects(effects.clone()),
        );
        *effects.close.lock().expect("close slot") = Some(Arc::downgrade(&state));
        assert_eq!(
            state
                .open_link(
                    "main",
                    OpenLinkRequest::Https {
                        url: "https://example.org".into()
                    }
                )
                .expect_err("caller closed after accepted handoff")
                .code,
            "link_caller_unavailable"
        );
        assert_eq!(effects.calls.load(Ordering::Acquire), 1);
    }

    #[test]
    fn the_settings_file_is_read_at_construction() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("desktop_settings.json");
        let settings = DesktopSettings {
            appearance: Appearance::Dark,
            density: Density::Compact,
            ..DesktopSettings::default()
        };
        save_settings_at(&path, &settings).expect("saved");

        let state = AppState::new(path, Arc::new(RecordingEventSink::default()));
        assert_eq!(state.settings().appearance, Appearance::Dark);
        assert_eq!(state.settings().density, Density::Compact);
    }

    #[test]
    fn a_patch_is_written_to_the_file() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("desktop_settings.json");
        let state = AppState::new(path.clone(), Arc::new(RecordingEventSink::default()));
        let patch = SettingsPatch {
            appearance: Some(Appearance::Dark),
            ..SettingsPatch::default()
        };
        state.apply_settings(&patch).expect("applied");
        assert_eq!(state.settings().appearance, Appearance::Dark);
        let stored = load_settings_at(&path);
        assert_eq!(stored.appearance, Appearance::Dark);
    }

    #[test]
    fn overlapping_patches_are_both_applied() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("desktop_settings.json");
        let state = Arc::new(AppState::new(
            path.clone(),
            Arc::new(RecordingEventSink::default()),
        ));

        let appearance_state = Arc::clone(&state);
        let appearance = std::thread::spawn(move || {
            appearance_state.apply_settings(&SettingsPatch {
                appearance: Some(Appearance::Dark),
                ..SettingsPatch::default()
            })
        });
        let density_state = Arc::clone(&state);
        let density = std::thread::spawn(move || {
            density_state.apply_settings(&SettingsPatch {
                density: Some(Density::Compact),
                ..SettingsPatch::default()
            })
        });
        appearance.join().expect("thread").expect("applied");
        density.join().expect("thread").expect("applied");

        let stored = load_settings_at(&path);
        assert_eq!(stored.appearance, Appearance::Dark);
        assert_eq!(stored.density, Density::Compact);
    }
}
