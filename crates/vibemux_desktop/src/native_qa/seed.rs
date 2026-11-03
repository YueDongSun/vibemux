//! Owned synthetic history for native GUI QA; no live provider or private input.
use super::{
    QA_SEED_TIMEOUT_SECONDS, QaError, QaScenario, SeedIds, SeedSummary,
    storage::{DirectoryPin, FilePin},
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};
use uuid::Uuid;
use vibemux_client::daemon_session::DaemonSession;
use vibemux_harness::{AgentKind, HarnessDetection, dispatch::fixture_guard::FixtureLaunchGuard};
use vibemux_types::{
    ChatId,
    chat::{ChatHarness, ChatState, ChatTurnPhase},
    chat_owner::{ChatOwnerPhase, ChatWorkspaceEffect},
};
use vibemuxd::{
    WriterWorker,
    chat_session::{
        api::ChatSessionStatus, execution_api::ChatExecutionStatus, owner_api::ChatOwnerStatus,
    },
    control::{
        ChatExecutionRequest, ChatExecutionResponse, ChatOwnerRequest, ChatOwnerResponse,
        ChatRequest, ChatResponse, ControlClient, ControlError,
    },
    process::DaemonPaths,
};
mod cleanup_scenario;
mod surface_scenario;

const FIXTURE_CONFIG: &[u8] = b"{\"mode\":\"native_links\"}\n";
const SURFACE_FIXTURE_CONFIG: &[u8] = b"{\"mode\":\"native_surfaces\"}\n";
const README: &[u8] = b"# DEMO native GUI project\n\nSynthetic fixture history only. No live provider or personal data.\n";
const GITIGNORE: &[u8] = b".vibemux/\n";
const SAVED_TITLE: &str = "DEMO Saved Codex - 中文";
const ARCHIVED_TITLE: &str = "DEMO Archived Claude - 日本語";
const WORKSPACE_TITLE: &str = "DEMO Workspace Codex - 中文";
const SAVED_PROMPTS: [&str; 2] = [
    "DEMO saved conversation first turn 中文",
    "DEMO saved conversation second turn 日本語",
];
const ARCHIVED_PROMPT: &str = "DEMO archived conversation 中文";
const WORKSPACE_PROMPT: &str = "DEMO workspace read-only conversation 日本語";
const OBSERVATION_INTERVAL: Duration = Duration::from_millis(50);
const MAX_LAYOUT_FILE_BYTES: u64 = 64 * 1024;

fn fixture_config(scenario: QaScenario) -> &'static [u8] {
    match scenario {
        QaScenario::History | QaScenario::CleanupReview => FIXTURE_CONFIG,
        QaScenario::Surfaces => SURFACE_FIXTURE_CONFIG,
    }
}

pub(super) struct QaLayout {
    pub paths: DaemonPaths,
    cleanup: Option<cleanup_scenario::CleanupFixture>,
    _guard: FixtureLaunchGuard,
    directories: Vec<DirectoryPin>,
    files: Vec<FilePin>,
}
impl QaLayout {
    pub fn verify(&self) -> Result<(), QaError> {
        for directory in &self.directories {
            directory.verify()?;
        }
        for file in &self.files {
            file.verify()?;
        }
        if let Some(cleanup) = &self.cleanup {
            cleanup.verify()?;
        }
        Ok(())
    }

    pub fn plugin_startup(&self) -> Result<vibemuxd::plugin_configuration::PluginStartup, QaError> {
        match &self.cleanup {
            Some(cleanup) => cleanup.startup(),
            None => Ok(Default::default()),
        }
    }

    pub async fn wait_for_plugins(&self) -> Result<(), QaError> {
        match &self.cleanup {
            Some(cleanup) => cleanup.wait_ready(&self.paths).await,
            None => Ok(()),
        }
    }
}

/// The parent owns the fresh root and all daemon lifecycle operations.
pub(super) async fn prepare_layout(root: &Path, scenario: QaScenario) -> Result<QaLayout, QaError> {
    let root = root.to_owned();
    tokio::task::spawn_blocking(move || {
        if !cfg!(windows) {
            return Err(QaError::RuntimeUnavailable);
        }
        let root = plain_directory(&root)?;
        let project = root.join("project");
        let vendor = root.join("vendor");
        std::fs::create_dir(&project).map_err(|_| QaError::RootExists)?;
        std::fs::create_dir(&vendor).map_err(|_| QaError::RootExists)?;
        let git = git_executable()?;
        git_command(
            &git,
            &project,
            &["init", "--quiet", "--initial-branch=main"],
        )?;
        create_file(&project.join("README.md"), README)?;
        create_file(&project.join(".gitignore"), GITIGNORE)?;
        git_command(&git, &project, &["add", "README.md", ".gitignore"])?;
        git_command(
            &git,
            &project,
            &["commit", "--quiet", "-m", "DEMO fixture base"],
        )?;
        let source = built_fixture()?;
        for name in ["codex.exe", "claude.exe"] {
            copy_image(&source, &vendor.join(name))?;
        }
        create_file(&vendor.join("chat_fixture.json"), fixture_config(scenario))?;
        if scenario == QaScenario::CleanupReview {
            cleanup_scenario::prepare(&vendor, &source)?;
        }
        let paths = DaemonPaths::from_project_root(&project).map_err(|_| QaError::InvalidRoot)?;
        paths
            .ensure_runtime_dir()
            .map_err(|_| QaError::RuntimeUnavailable)?;
        create_file(
            &paths.state_dir().join("harness_dispatch.json"),
            &route_bytes(&vendor)?,
        )?;
        // An existing descriptor is never permission to start a second writer.
        if std::fs::symlink_metadata(paths.descriptor_path()).is_ok() {
            return Err(QaError::OwnershipChanged);
        }
        seed_detection(&paths)?;
        open_layout(&root, scenario)
    })
    .await
    .map_err(|_| QaError::Io)?
}

pub(super) fn open_layout(root: &Path, scenario: QaScenario) -> Result<QaLayout, QaError> {
    if !cfg!(windows) {
        return Err(QaError::RuntimeUnavailable);
    }
    let root = plain_directory(root)?;
    let project = plain_directory(&root.join("project"))?;
    let vendor = plain_directory(&root.join("vendor"))?;
    let source = built_fixture()?;
    let paths = DaemonPaths::from_project_root(&project).map_err(|_| QaError::InvalidRoot)?;
    let directories = [
        project.clone(),
        vendor.clone(),
        project.join(".git"),
        paths.state_dir().to_owned(),
        source
            .parent()
            .ok_or(QaError::FixtureUnavailable)?
            .to_owned(),
    ]
    .iter()
    .map(|path| DirectoryPin::open(path))
    .collect::<Result<Vec<_>, _>>()?;
    let files = [
        source.clone(),
        source.with_file_name("vibemux_launch_trampoline.exe"),
        vendor.join("codex.exe"),
        vendor.join("claude.exe"),
        vendor.join("chat_fixture.json"),
        project.join("README.md"),
        project.join(".gitignore"),
        paths.state_dir().join("harness_dispatch.json"),
    ]
    .iter()
    .map(|path| FilePin::open(path))
    .collect::<Result<Vec<_>, _>>()?;
    verify_file(&project.join("README.md"), README)?;
    verify_file(&project.join(".gitignore"), GITIGNORE)?;
    plain_directory(&project.join(".git"))?;
    verify_file(&vendor.join("chat_fixture.json"), fixture_config(scenario))?;
    let paths = DaemonPaths::from_project_root(&project).map_err(|_| QaError::InvalidRoot)?;
    // A clean daemon shutdown may remove its owned runtime leaf.
    // Validate an existing leaf; the lifecycle owner recreates an absent one.
    match std::fs::symlink_metadata(paths.runtime_dir()) {
        Ok(_) => paths
            .validate_runtime_dir()
            .map_err(|_| QaError::RuntimeUnavailable)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(QaError::RuntimeUnavailable),
    }
    verify_file(
        &paths.state_dir().join("harness_dispatch.json"),
        &route_bytes(&vendor)?,
    )?;
    let guard = FixtureLaunchGuard::register_worker_copies(
        &source,
        &vendor,
        &[AgentKind::Codex, AgentKind::Claude],
    )
    .map_err(|_| QaError::FixtureUnavailable)?;
    let cleanup = match scenario {
        QaScenario::History | QaScenario::Surfaces => None,
        QaScenario::CleanupReview => {
            Some(cleanup_scenario::CleanupFixture::open(&vendor, &source)?)
        }
    };
    let layout = QaLayout {
        paths,
        cleanup,
        _guard: guard,
        directories,
        files,
    };
    layout.verify()?;
    Ok(layout)
}

fn route_bytes(vendor: &Path) -> Result<Vec<u8>, QaError> {
    let mut bytes = serde_json::to_vec_pretty(&json!({"schema_version":1,"routes":[
        {"harness":"codex","protocol":"codex_app_server","executable":vendor.join("codex.exe"),"enabled":true,"allow_execution":true,"environment_names":[]},
        {"harness":"claude","protocol":"claude_stream_json","executable":vendor.join("claude.exe"),"enabled":true,"allow_execution":true,"environment_names":[]}
    ]})).map_err(|_| QaError::Io)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn built_fixture() -> Result<PathBuf, QaError> {
    let image = std::env::current_exe().map_err(|_| QaError::FixtureUnavailable)?;
    let directory = image.parent().ok_or(QaError::FixtureUnavailable)?;
    let profile = if directory.file_name() == Some(std::ffi::OsStr::new("deps")) {
        directory.parent().ok_or(QaError::FixtureUnavailable)?
    } else {
        directory
    };
    let source = profile.join("vibemux_chat_fixture.exe");
    let metadata = std::fs::symlink_metadata(&source).map_err(|_| QaError::FixtureUnavailable)?;
    if !metadata.is_file() || is_link(&metadata) || metadata.len() == 0 {
        return Err(QaError::FixtureUnavailable);
    }
    Ok(source)
}

fn plain_directory(path: &Path) -> Result<PathBuf, QaError> {
    if !path.is_absolute() {
        return Err(QaError::InvalidRoot);
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|_| QaError::InvalidRoot)?;
    if !metadata.is_dir() || is_link(&metadata) {
        return Err(QaError::InvalidRoot);
    }
    std::fs::canonicalize(path).map_err(|_| QaError::InvalidRoot)
}
fn is_link(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}
fn create_file(path: &Path, bytes: &[u8]) -> Result<(), QaError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| QaError::Io)?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| QaError::Io)
}
fn copy_image(source: &Path, destination: &Path) -> Result<(), QaError> {
    let mut source = File::open(source).map_err(|_| QaError::FixtureUnavailable)?;
    let mut destination = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|_| QaError::FixtureUnavailable)?;
    std::io::copy(&mut source, &mut destination).map_err(|_| QaError::FixtureUnavailable)?;
    destination
        .sync_all()
        .map_err(|_| QaError::FixtureUnavailable)
}
fn verify_file(path: &Path, expected: &[u8]) -> Result<(), QaError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| QaError::OwnershipChanged)?;
    if !metadata.is_file() || is_link(&metadata) || metadata.len() > MAX_LAYOUT_FILE_BYTES {
        return Err(QaError::OwnershipChanged);
    }
    let mut actual = Vec::new();
    File::open(path)
        .map_err(|_| QaError::OwnershipChanged)?
        .take(MAX_LAYOUT_FILE_BYTES + 1)
        .read_to_end(&mut actual)
        .map_err(|_| QaError::OwnershipChanged)?;
    if actual == expected {
        Ok(())
    } else {
        Err(QaError::OwnershipChanged)
    }
}

fn git_executable() -> Result<PathBuf, QaError> {
    std::env::var_os("PATH")
        .and_then(|path| {
            std::env::split_paths(&path)
                .filter(|directory| directory.is_absolute())
                .map(|directory| directory.join("git.exe"))
                .find(|executable| executable.is_file())
        })
        .ok_or(QaError::RuntimeUnavailable)
}
fn git_command(executable: &Path, root: &Path, arguments: &[&str]) -> Result<(), QaError> {
    let mut command = Command::new(executable);
    command
        .current_dir(root)
        .args([
            "-c",
            "user.name=Desktop Owner Fixture",
            "-c",
            "user.email=fixture@invalid.test",
            "-c",
            "core.hooksPath=NUL",
            "-c",
            "commit.gpgSign=false",
        ])
        .args(arguments)
        .env_clear()
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "NUL")
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    for key in ["SystemRoot", "WINDIR", "TEMP", "TMP", "PATH"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    let output = command.output().map_err(|_| QaError::Io)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(QaError::Io)
    }
}
fn seed_detection(paths: &DaemonPaths) -> Result<(), QaError> {
    let writer = WriterWorker::start(paths.database_path()).map_err(|_| QaError::SeedFailed)?;
    let result = (|| {
        writer
            .handle()
            .map_err(|_| QaError::SeedFailed)?
            .commit_harness_snapshot(
                BTreeMap::from(["codex", "claude"].map(|name| {
                    (
                        name.into(),
                        HarnessDetection {
                            detected: true,
                            path: None,
                            launcher: None,
                            version: Some("synthetic".into()),
                        },
                    )
                })),
                "2026-11-02T00:00:00Z".into(),
                "native_gui_qa_detection".into(),
            )
            .map_err(|_| QaError::SeedFailed)?;
        Ok(())
    })();
    let shutdown = writer.shutdown().map_err(|_| QaError::SeedFailed);
    result.and(shutdown)
}
async fn connect(paths: &DaemonPaths) -> Result<ControlClient, QaError> {
    let paths = paths.clone();
    tokio::task::spawn_blocking(move || {
        DaemonSession::default()
            .connect(&paths)
            .map(|(client, _)| client)
            .map_err(|_| QaError::RuntimeUnavailable)
    })
    .await
    .map_err(|_| QaError::RuntimeUnavailable)?
}
fn uncertain(error: &ControlError) -> bool {
    matches!(
        error.code(),
        "writer_response_timeout"
            | "chat_acknowledgement_uncertain"
            | "control_deadline_exceeded"
            | "control_endpoint_unavailable"
            | "control_invalid_frame"
    )
}

/// Runs only after the parent starts the guarded daemon. All admission is Control.
pub(super) async fn seed_history(
    paths: &DaemonPaths,
    scenario: QaScenario,
) -> Result<SeedIds, QaError> {
    tokio::time::timeout(Duration::from_secs(QA_SEED_TIMEOUT_SECONDS), async {
        let client = connect(paths).await?;
        let mut saved = saved_open(&client, ChatHarness::Codex, SAVED_TITLE).await?;
        saved = saved_ready(&client, saved).await?;
        for prompt in SAVED_PROMPTS {
            saved = saved_send(&client, saved, prompt).await?;
        }
        let saved_chat = saved.chat_id;
        saved_lifecycle(&client, &saved, false).await?;
        let archived = saved_open(&client, ChatHarness::Claude, ARCHIVED_TITLE).await?;
        let archived = saved_ready(&client, archived).await?;
        let archived = saved_send(&client, archived, ARCHIVED_PROMPT).await?;
        let archived = saved_lifecycle(&client, &archived, false).await?;
        saved_lifecycle(&client, &archived, true).await?;
        let owner = workspace_open(&client).await?;
        let runtime = workspace_start(&client, &owner).await?;
        let runtime = workspace_send(&client, runtime).await?;
        workspace_close(&client, &runtime).await?;
        if scenario == QaScenario::CleanupReview {
            workspace_finalize(&client, owner.chat_id, owner.run_id).await?;
        }
        let ids = SeedIds {
            saved_chat,
            archived_chat: archived.chat_id,
            workspace_chat: owner.chat_id,
            workspace_run: owner.run_id,
        };
        check_with_client(&client, &ids, scenario, paths.project_root())
            .await
            .map_err(|_| QaError::SeedFailed)?;
        Ok(ids)
    })
    .await
    .map_err(|_| QaError::CleanupUnconfirmed)?
}
async fn saved_status(
    client: &ControlClient,
    chat_id: ChatId,
) -> Result<ChatSessionStatus, QaError> {
    match client
        .chat(&ChatRequest::Status { chat_id })
        .await
        .map_err(|_| QaError::SeedFailed)?
    {
        ChatResponse::Status {
            session: Some(session),
        } if session.chat_id == chat_id => Ok(session),
        _ => Err(QaError::SeedFailed),
    }
}
async fn saved_open(
    client: &ControlClient,
    harness: ChatHarness,
    title: &str,
) -> Result<ChatSessionStatus, QaError> {
    let operation_id = Uuid::new_v4();
    match client
        .chat(&ChatRequest::Open {
            operation_id,
            harness,
            title: Some(title.into()),
            consent_private_storage: true,
        })
        .await
    {
        Ok(ChatResponse::Session { session, .. }) if session.harness == harness => Ok(session),
        Err(error) if uncertain(&error) => loop {
            match client.chat(&ChatRequest::LookupOpen { operation_id }).await {
                Ok(ChatResponse::Status {
                    session: Some(session),
                }) if session.harness == harness => return Ok(session),
                Ok(ChatResponse::Status { session: None }) => {}
                Err(error) if uncertain(&error) => {}
                _ => return Err(QaError::SeedFailed),
            }
            tokio::time::sleep(OBSERVATION_INTERVAL).await;
        },
        _ => Err(QaError::SeedFailed),
    }
}
async fn saved_ready(
    client: &ControlClient,
    mut session: ChatSessionStatus,
) -> Result<ChatSessionStatus, QaError> {
    let chat_id = session.chat_id;
    let generation = session.generation;
    loop {
        if session.chat_id != chat_id || session.generation != generation {
            return Err(QaError::SeedFailed);
        }
        if session.state == ChatState::Ready && session.active_request_id.is_none() {
            return Ok(session);
        }
        if session.state != ChatState::Opening {
            return Err(QaError::SeedFailed);
        }
        tokio::time::sleep(OBSERVATION_INTERVAL).await;
        session = saved_status(client, chat_id).await?;
    }
}
async fn saved_send(
    client: &ControlClient,
    session: ChatSessionStatus,
    prompt: &str,
) -> Result<ChatSessionStatus, QaError> {
    let chat_id = session.chat_id;
    let generation = session.generation;
    let request_id = Uuid::new_v4();
    match client
        .chat(&ChatRequest::Send {
            chat_id,
            generation,
            request_id,
            prompt: prompt.into(),
        })
        .await
    {
        Ok(ChatResponse::Turn { session, turn, .. })
            if session.chat_id == chat_id
                && turn.chat_id == chat_id
                && turn.request_id == request_id
                && turn.generation == generation => {}
        Err(error) if uncertain(&error) => {}
        _ => return Err(QaError::SeedFailed),
    }
    loop {
        match client
            .chat(&ChatRequest::Submission {
                chat_id,
                generation,
                request_id,
            })
            .await
        {
            Ok(ChatResponse::Submission { session, turn })
                if session.chat_id == chat_id && session.generation == generation =>
            {
                if let Some(turn) = turn {
                    if turn.chat_id != chat_id
                        || turn.request_id != request_id
                        || turn.generation != generation
                    {
                        return Err(QaError::SeedFailed);
                    }
                    if turn.phase == ChatTurnPhase::Completed
                        && turn.correlated_terminal
                        && turn.output_complete
                        && session.state == ChatState::Ready
                        && session.active_request_id.is_none()
                    {
                        return Ok(session);
                    }
                    if !matches!(
                        turn.phase,
                        ChatTurnPhase::Admitted | ChatTurnPhase::Running | ChatTurnPhase::Completed
                    ) {
                        return Err(QaError::SeedFailed);
                    }
                }
            }
            Err(error) if uncertain(&error) => {}
            _ => return Err(QaError::SeedFailed),
        }
        tokio::time::sleep(OBSERVATION_INTERVAL).await;
    }
}
async fn saved_lifecycle(
    client: &ControlClient,
    session: &ChatSessionStatus,
    archive: bool,
) -> Result<ChatSessionStatus, QaError> {
    let chat_id = session.chat_id;
    let generation = session.generation;
    let operation_id = Uuid::new_v4();
    let expected = if archive {
        ChatState::Archived
    } else {
        ChatState::Closed
    };
    let request = if archive {
        ChatRequest::Archive {
            operation_id,
            chat_id,
            generation,
        }
    } else {
        ChatRequest::Close {
            operation_id,
            chat_id,
            generation,
        }
    };
    match client.chat(&request).await {
        Ok(ChatResponse::Session { session, .. })
            if session.chat_id == chat_id && session.generation == generation => {}
        Err(error) if uncertain(&error) => {}
        _ => return Err(QaError::SeedFailed),
    }
    loop {
        let current = saved_status(client, chat_id).await?;
        if current.generation != generation {
            return Err(QaError::SeedFailed);
        }
        if current.state == expected && current.active_request_id.is_none() {
            return Ok(current);
        }
        tokio::time::sleep(OBSERVATION_INTERVAL).await;
    }
}
async fn workspace_status(
    client: &ControlClient,
    chat_id: ChatId,
) -> Result<ChatExecutionStatus, QaError> {
    match client
        .chat_execution(&ChatExecutionRequest::Status { chat_id })
        .await
        .map_err(|_| QaError::SeedFailed)?
    {
        ChatExecutionResponse::Status {
            runtime: Some(runtime),
        } if runtime.owner.chat_id == chat_id && runtime.session.chat_id == chat_id => Ok(runtime),
        _ => Err(QaError::SeedFailed),
    }
}
async fn workspace_open(client: &ControlClient) -> Result<ChatOwnerStatus, QaError> {
    let operation_id = Uuid::new_v4();
    let mut owner = match client
        .chat_owner(&ChatOwnerRequest::Open {
            operation_id,
            harness: ChatHarness::Codex,
            title: Some(WORKSPACE_TITLE.into()),
            consent_private_storage: true,
            consent_workspace: true,
        })
        .await
    {
        Ok(ChatOwnerResponse::Owner { owner, .. }) if owner.harness == ChatHarness::Codex => owner,
        Err(error) if uncertain(&error) => loop {
            match client
                .chat_owner(&ChatOwnerRequest::LookupOpen { operation_id })
                .await
            {
                Ok(ChatOwnerResponse::Status { owner: Some(owner) })
                    if owner.harness == ChatHarness::Codex =>
                {
                    break owner;
                }
                Ok(ChatOwnerResponse::Status { owner: None }) => {}
                Err(error) if uncertain(&error) => {}
                _ => return Err(QaError::SeedFailed),
            }
            tokio::time::sleep(OBSERVATION_INTERVAL).await;
        },
        _ => return Err(QaError::SeedFailed),
    };
    let chat_id = owner.chat_id;
    let run_id = owner.run_id;
    let reconcile_id = Uuid::new_v4();
    let mut reconciled = false;
    loop {
        if owner.chat_id != chat_id || owner.run_id != run_id || !owner.admitted_tools.is_empty() {
            return Err(QaError::SeedFailed);
        }
        if owner.phase == ChatOwnerPhase::Open && !owner.provisioning_active {
            return Ok(owner);
        }
        if owner.phase != ChatOwnerPhase::Provisioning {
            return Err(QaError::SeedFailed);
        }
        if !owner.provisioning_active && !reconciled {
            reconciled = true;
            match client
                .chat_owner(&ChatOwnerRequest::Reconcile {
                    operation_id: reconcile_id,
                    chat_id,
                    expected_version: owner.version,
                })
                .await
            {
                Ok(ChatOwnerResponse::Owner { owner: next, .. })
                    if next.chat_id == chat_id && next.run_id == run_id =>
                {
                    owner = next
                }
                Err(error) if uncertain(&error) => {}
                _ => return Err(QaError::SeedFailed),
            }
        }
        tokio::time::sleep(OBSERVATION_INTERVAL).await;
        match client
            .chat_owner(&ChatOwnerRequest::Status { chat_id })
            .await
        {
            Ok(ChatOwnerResponse::Status { owner: Some(next) })
                if next.chat_id == chat_id && next.run_id == run_id =>
            {
                owner = next
            }
            Err(error) if uncertain(&error) => {}
            _ => return Err(QaError::SeedFailed),
        }
    }
}
async fn workspace_start(
    client: &ControlClient,
    owner: &ChatOwnerStatus,
) -> Result<ChatExecutionStatus, QaError> {
    let initial = workspace_status(client, owner.chat_id).await?;
    if initial.process_claimed || initial.owner.run_id != owner.run_id {
        return Err(QaError::SeedFailed);
    }
    let chat_id = owner.chat_id;
    let generation = initial.session.generation;
    let owner_version = owner.version;
    let operation_id = Uuid::new_v4();
    match client
        .chat_execution(&ChatExecutionRequest::Start {
            operation_id,
            chat_id,
            generation,
            owner_version,
            consent_execution: true,
        })
        .await
    {
        Ok(ChatExecutionResponse::Process { runtime, .. })
            if runtime.owner.chat_id == chat_id
                && runtime.owner.run_id == owner.run_id
                && runtime.session.generation == generation => {}
        Err(error) if uncertain(&error) => loop {
            match client
                .chat_execution(&ChatExecutionRequest::LookupStart {
                    operation_id,
                    chat_id,
                    generation,
                    owner_version,
                })
                .await
            {
                Ok(ChatExecutionResponse::StartReceipt {
                    runtime: Some(runtime),
                    sequence: Some(_),
                }) if runtime.owner.chat_id == chat_id
                    && runtime.owner.run_id == owner.run_id
                    && runtime.session.generation == generation =>
                {
                    break;
                }
                Ok(ChatExecutionResponse::StartReceipt {
                    runtime: None,
                    sequence: None,
                }) => {}
                Err(error) if uncertain(&error) => {}
                _ => return Err(QaError::SeedFailed),
            }
            tokio::time::sleep(OBSERVATION_INTERVAL).await;
        },
        _ => return Err(QaError::SeedFailed),
    }
    loop {
        let runtime = workspace_status(client, chat_id).await?;
        if runtime.owner.run_id != owner.run_id
            || runtime.session.generation != generation
            || !runtime.owner.admitted_tools.is_empty()
        {
            return Err(QaError::SeedFailed);
        }
        if runtime.process_claimed
            && runtime.session.state == ChatState::Ready
            && runtime.session.active_request_id.is_none()
        {
            return Ok(runtime);
        }
        if runtime.session.state != ChatState::Opening {
            return Err(QaError::SeedFailed);
        }
        tokio::time::sleep(OBSERVATION_INTERVAL).await;
    }
}
async fn workspace_send(
    client: &ControlClient,
    runtime: ChatExecutionStatus,
) -> Result<ChatExecutionStatus, QaError> {
    let chat_id = runtime.owner.chat_id;
    let generation = runtime.session.generation;
    let run_id = runtime.owner.run_id;
    let request_id = Uuid::new_v4();
    match client
        .chat_execution(&ChatExecutionRequest::Send {
            chat_id,
            generation,
            owner_version: runtime.owner.version,
            request_id,
            prompt: WORKSPACE_PROMPT.into(),
        })
        .await
    {
        Ok(ChatExecutionResponse::Attempt {
            runtime, attempt, ..
        }) if runtime.owner.chat_id == chat_id
            && runtime.owner.run_id == run_id
            && attempt.chat_id == chat_id
            && attempt.request_id == request_id
            && attempt.generation == generation => {}
        Err(error) if uncertain(&error) => {}
        _ => return Err(QaError::SeedFailed),
    }
    loop {
        match client
            .chat_execution(&ChatExecutionRequest::Submission {
                chat_id,
                generation,
                request_id,
            })
            .await
        {
            Ok(ChatExecutionResponse::Submission { runtime, attempt })
                if runtime.owner.chat_id == chat_id
                    && runtime.owner.run_id == run_id
                    && runtime.session.generation == generation =>
            {
                if let Some(attempt) = attempt {
                    if attempt.chat_id != chat_id
                        || attempt.request_id != request_id
                        || attempt.generation != generation
                        || attempt.run_id != run_id
                    {
                        return Err(QaError::SeedFailed);
                    }
                    if attempt.phase == ChatTurnPhase::Completed
                        && attempt.correlated_terminal
                        && attempt.output_complete
                        && runtime.session.state == ChatState::Ready
                        && runtime.session.active_request_id.is_none()
                    {
                        return Ok(runtime);
                    }
                    if !matches!(
                        attempt.phase,
                        ChatTurnPhase::Admitted | ChatTurnPhase::Running | ChatTurnPhase::Completed
                    ) {
                        return Err(QaError::SeedFailed);
                    }
                }
            }
            Err(error) if uncertain(&error) => {}
            _ => return Err(QaError::SeedFailed),
        }
        tokio::time::sleep(OBSERVATION_INTERVAL).await;
    }
}
async fn workspace_close(
    client: &ControlClient,
    runtime: &ChatExecutionStatus,
) -> Result<(), QaError> {
    let chat_id = runtime.owner.chat_id;
    let generation = runtime.session.generation;
    match client
        .chat_execution(&ChatExecutionRequest::Close {
            chat_id,
            generation,
        })
        .await
    {
        Ok(ChatExecutionResponse::Status {
            runtime: Some(closed),
        }) if closed.owner.chat_id == chat_id && closed.owner.run_id == runtime.owner.run_id => {}
        Err(error) if uncertain(&error) => {}
        _ => return Err(QaError::CleanupUnconfirmed),
    }
    loop {
        let current = workspace_status(client, chat_id).await?;
        if current.owner.run_id != runtime.owner.run_id || current.session.generation != generation
        {
            return Err(QaError::CleanupUnconfirmed);
        }
        if !current.process_claimed
            && current.session.state == ChatState::Closed
            && current.session.active_request_id.is_none()
            && current.owner.phase == ChatOwnerPhase::Open
        {
            return Ok(());
        }
        tokio::time::sleep(OBSERVATION_INTERVAL).await;
    }
}

async fn workspace_finalize(
    client: &ControlClient,
    chat_id: ChatId,
    run_id: vibemux_types::RunId,
) -> Result<(), QaError> {
    let current = workspace_status(client, chat_id).await?;
    if current.owner.run_id != run_id
        || current.owner.phase != ChatOwnerPhase::Open
        || current.session.state != ChatState::Closed
        || current.process_claimed
    {
        return Err(QaError::SeedFailed);
    }
    let task_id = current.owner.task_id;
    let generation = current.session.generation;
    match client
        .chat_owner(&ChatOwnerRequest::Stop {
            operation_id: Uuid::new_v4(),
            chat_id,
            expected_version: current.owner.version,
        })
        .await
    {
        Ok(ChatOwnerResponse::Owner { owner, .. })
            if owner.chat_id == chat_id && owner.run_id == run_id => {}
        Err(error) if uncertain(&error) => {}
        _ => return Err(QaError::SeedFailed),
    }
    // Observe the one accepted request; uncertainty never sends another Stop.
    loop {
        let current = workspace_status(client, chat_id).await?;
        if current.owner.run_id != run_id
            || current.owner.task_id != task_id
            || current.session.generation != generation
        {
            return Err(QaError::OwnershipChanged);
        }
        if current.owner.phase == ChatOwnerPhase::Finalized
            && current.session.state == ChatState::Closed
            && !current.process_claimed
            && current.session.active_request_id.is_none()
        {
            return Ok(());
        }
        tokio::time::sleep(OBSERVATION_INTERVAL).await;
    }
}

pub(super) async fn check_history(
    paths: &DaemonPaths,
    ids: &SeedIds,
    scenario: QaScenario,
) -> Result<SeedSummary, QaError> {
    tokio::time::timeout(Duration::from_secs(QA_SEED_TIMEOUT_SECONDS), async {
        let client = connect(paths).await?;
        check_with_client(&client, ids, scenario, paths.project_root()).await
    })
    .await
    .map_err(|_| QaError::CheckFailed)?
}
async fn check_with_client(
    client: &ControlClient,
    ids: &SeedIds,
    scenario: QaScenario,
    project_root: &Path,
) -> Result<SeedSummary, QaError> {
    let saved = saved_status(client, ids.saved_chat)
        .await
        .map_err(|_| QaError::CheckFailed)?;
    let archived = saved_status(client, ids.archived_chat)
        .await
        .map_err(|_| QaError::CheckFailed)?;
    let workspace = workspace_status(client, ids.workspace_chat)
        .await
        .map_err(|_| QaError::CheckFailed)?;
    let summary = validate_summary(ids, &saved, &archived, &workspace, scenario)?;
    for (chat_id, expected) in [
        (ids.saved_chat, SAVED_TITLE),
        (ids.archived_chat, ARCHIVED_TITLE),
    ] {
        match client
            .chat(&ChatRequest::Title { chat_id })
            .await
            .map_err(|_| QaError::CheckFailed)?
        {
            ChatResponse::Title {
                chat_id: actual,
                title: Some(title),
            } if actual == chat_id && title == expected => {}
            _ => return Err(QaError::CheckFailed),
        }
    }
    match client
        .chat_owner(&ChatOwnerRequest::Title {
            chat_id: ids.workspace_chat,
        })
        .await
        .map_err(|_| QaError::CheckFailed)?
    {
        ChatOwnerResponse::Title {
            chat_id,
            title: Some(title),
        } if chat_id == ids.workspace_chat && title == WORKSPACE_TITLE => {}
        _ => return Err(QaError::CheckFailed),
    }
    if scenario == QaScenario::Surfaces {
        surface_scenario::check(client, ids, &saved, &archived, &workspace, project_root).await?;
    }
    Ok(summary)
}
fn validate_summary(
    ids: &SeedIds,
    saved: &ChatSessionStatus,
    archived: &ChatSessionStatus,
    workspace: &ChatExecutionStatus,
    scenario: QaScenario,
) -> Result<SeedSummary, QaError> {
    let phase = match scenario {
        QaScenario::History | QaScenario::Surfaces => ChatOwnerPhase::Open,
        QaScenario::CleanupReview => ChatOwnerPhase::Finalized,
    };
    let (saved_records, archived_records, workspace_records) = match scenario {
        QaScenario::History | QaScenario::CleanupReview => (4, 2, 2),
        QaScenario::Surfaces => (8, 2, 4),
    };
    if ids.saved_chat == ids.archived_chat
        || ids.saved_chat == ids.workspace_chat
        || ids.archived_chat == ids.workspace_chat
        || saved.chat_id != ids.saved_chat
        || archived.chat_id != ids.archived_chat
        || workspace.owner.chat_id != ids.workspace_chat
        || workspace.session.chat_id != ids.workspace_chat
        || workspace.owner.run_id != ids.workspace_run
        || saved.project_id != archived.project_id
        || saved.project_id != workspace.session.project_id
        || saved.project_id != workspace.owner.project_id
        || saved.harness != ChatHarness::Codex
        || archived.harness != ChatHarness::Claude
        || workspace.session.harness != ChatHarness::Codex
        || workspace.owner.harness != ChatHarness::Codex
        || saved.state != ChatState::Closed
        || archived.state != ChatState::Archived
        || workspace.session.state != ChatState::Closed
        || workspace.owner.phase != phase
        || workspace.owner.schema_version != 1
        || !workspace.owner.has_title
        || !workspace.owner.admitted_tools.is_empty()
        || workspace.owner.provisioning_active
        || workspace.process_claimed
        || !workspace.owner.workspace_planned
        || workspace.owner.workspace_branch.is_none()
        || workspace.owner.workspace_verified_at.is_none()
        || workspace.owner.workspace_effect != Some(ChatWorkspaceEffect::Settled)
        || workspace.owner.error_code.is_some()
        || [saved, archived, &workspace.session].iter().any(|session| {
            session.schema_version != 1
                || session.generation == 0
                || session.generation > i64::MAX as u64
                || session.version == 0
                || session.version > i64::MAX as u64
                || session.active_request_id.is_some()
                || session.content_purged
                || !session.has_title
                || session.last_record_sequence == 0
        })
        || saved.record_count != saved_records
        || archived.record_count != archived_records
        || workspace.session.record_count != workspace_records
    {
        return Err(QaError::CheckFailed);
    }
    Ok(SeedSummary {
        saved_records: saved.record_count,
        archived_records: archived.record_count,
        workspace_records: workspace.session.record_count,
        process_claims_present: workspace.process_claimed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_routes_exclude_provider_model_environment_and_writable_profiles() {
        let value: serde_json::Value =
            serde_json::from_slice(&route_bytes(Path::new("C:/DEMO/vendor")).expect("routes"))
                .expect("JSON");
        let routes = value["routes"].as_array().expect("routes");
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0]["protocol"], "codex_app_server");
        assert_eq!(routes[1]["protocol"], "claude_stream_json");
        for route in routes {
            assert_eq!(route["allow_execution"], true);
            assert_eq!(route["environment_names"], json!([]));
            for key in ["provider", "model", "profile", "entry_script"] {
                assert!(route.get(key).is_none());
            }
        }
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(FIXTURE_CONFIG).expect("config"),
            json!({"mode":"native_links"})
        );
    }

    #[test]
    fn scenario_configs_preserve_legacy_bytes_and_select_surfaces_in_both_layout_paths() {
        assert_eq!(
            fixture_config(QaScenario::History),
            b"{\"mode\":\"native_links\"}\n"
        );
        assert_eq!(
            fixture_config(QaScenario::CleanupReview),
            b"{\"mode\":\"native_links\"}\n"
        );
        assert_eq!(
            fixture_config(QaScenario::Surfaces),
            b"{\"mode\":\"native_surfaces\"}\n"
        );
        let temp = tempfile::tempdir().expect("fixture");
        let config = temp.path().join("chat_fixture.json");
        create_file(&config, fixture_config(QaScenario::Surfaces)).expect("surface config");
        verify_file(&config, fixture_config(QaScenario::Surfaces))
            .expect("matching surface config");
        assert_eq!(
            verify_file(&config, fixture_config(QaScenario::History)),
            Err(QaError::OwnershipChanged)
        );
        std::fs::write(&config, fixture_config(QaScenario::History)).expect("legacy bytes");
        assert_eq!(
            verify_file(&config, fixture_config(QaScenario::Surfaces)),
            Err(QaError::OwnershipChanged)
        );
    }

    #[test]
    fn image_copy_is_exclusive_and_verified_config_cannot_change_mode() {
        let temp = tempfile::tempdir().expect("fixture");
        let source = temp.path().join("source.exe");
        let target = temp.path().join("codex.exe");
        std::fs::write(&source, b"synthetic image bytes").expect("source");
        copy_image(&source, &target).expect("exclusive copy");
        assert_eq!(
            std::fs::read(&target).expect("copied image"),
            b"synthetic image bytes"
        );
        std::fs::write(&source, b"changed source bytes").expect("changed source");
        assert_eq!(
            copy_image(&source, &target),
            Err(QaError::FixtureUnavailable)
        );
        assert_eq!(
            std::fs::read(&target).expect("preserved image"),
            b"synthetic image bytes"
        );
        let config = temp.path().join("chat_fixture.json");
        create_file(&config, FIXTURE_CONFIG).expect("fixed config");
        verify_file(&config, FIXTURE_CONFIG).expect("matching config");
        std::fs::write(&config, b"{\"mode\":\"claude_approval\"}\n").expect("changed mode");
        assert_eq!(
            verify_file(&config, FIXTURE_CONFIG),
            Err(QaError::OwnershipChanged)
        );
    }

    #[cfg(windows)]
    #[test]
    fn guard_binds_both_exclusive_copies_and_rejects_a_changed_image() {
        let temp = tempfile::tempdir().expect("fixture");
        let source = temp.path().join("vibemux_chat_fixture.exe");
        let vendor = temp.path().join("vendor");
        std::fs::create_dir(&vendor).expect("vendor");
        std::fs::write(&source, b"synthetic guard image").expect("source");
        for name in ["codex.exe", "claude.exe"] {
            copy_image(&source, &vendor.join(name)).expect("copy");
        }
        FixtureLaunchGuard::register_worker_copies(
            &source,
            &vendor,
            &[AgentKind::Codex, AgentKind::Claude],
        )
        .expect("both exact aliases");
        std::fs::write(vendor.join("claude.exe"), b"changed image").expect("changed copy");
        assert!(
            FixtureLaunchGuard::register_worker_copies(
                &source,
                &vendor,
                &[AgentKind::Codex, AgentKind::Claude]
            )
            .is_err()
        );
    }
    fn summary_fixture() -> (
        SeedIds,
        ChatSessionStatus,
        ChatSessionStatus,
        ChatExecutionStatus,
    ) {
        use vibemux_types::{ProjectId, RunId, TaskId};
        let ids = SeedIds {
            saved_chat: ChatId::new(),
            archived_chat: ChatId::new(),
            workspace_chat: ChatId::new(),
            workspace_run: RunId::new(),
        };
        let project_id = ProjectId::new();
        let session = |chat_id, harness, state, count| {
            serde_json::from_value::<ChatSessionStatus>(json!({
            "schema_version":1,"chat_id":chat_id,"project_id":project_id,"harness":harness,"state":state,
            "generation":1,"version":5,"active_request_id":null,"record_count":count,"last_record_sequence":30,
            "content_purged":false,"has_title":true,"created_at":"2026-11-02T00:00:00Z","updated_at":"2026-11-02T00:00:00Z"
        })).expect("session fixture")
        };
        let saved = session(ids.saved_chat, "codex", "closed", 4);
        let archived = session(ids.archived_chat, "claude", "archived", 2);
        let owner = serde_json::from_value::<ChatOwnerStatus>(json!({
            "schema_version":1,"chat_id":ids.workspace_chat,"project_id":project_id,"task_id":TaskId::new(),"run_id":ids.workspace_run,
            "harness":"codex","phase":"open","version":4,"run_status":"stopped","admitted_tools":[],"has_title":true,
            "workspace_planned":true,"workspace_branch":"codex/demo_fixture","workspace_effect":"settled",
            "workspace_verified_at":"2026-11-02T00:00:00Z","provisioning_active":false,"error_code":null,
            "created_at":"2026-11-02T00:00:00Z","updated_at":"2026-11-02T00:00:00Z"
        })).expect("owner fixture");
        let workspace = ChatExecutionStatus {
            owner,
            session: session(ids.workspace_chat, "codex", "closed", 2),
            process_claimed: false,
        };
        (ids, saved, archived, workspace)
    }

    #[test]
    fn baseline_check_reports_actual_counts_and_refuses_claims_identity_or_state_changes() {
        let (ids, saved, archived, workspace) = summary_fixture();
        assert_eq!(
            validate_summary(&ids, &saved, &archived, &workspace, QaScenario::History),
            Ok(SeedSummary {
                saved_records: 4,
                archived_records: 2,
                workspace_records: 2,
                process_claims_present: false,
            })
        );
        for fault in ["claim", "run", "records", "state", "workspace"] {
            let (mut ids, mut saved, mut archived, mut workspace) = summary_fixture();
            match fault {
                "claim" => workspace.process_claimed = true,
                "run" => ids.workspace_run = vibemux_types::RunId::new(),
                "records" => saved.record_count = 5,
                "state" => archived.state = ChatState::Closed,
                _ => workspace.owner.workspace_verified_at = None,
            }
            assert_eq!(
                validate_summary(&ids, &saved, &archived, &workspace, QaScenario::History),
                Err(QaError::CheckFailed),
                "{fault}"
            );
        }
    }

    #[test]
    fn cleanup_baseline_requires_finalized_while_history_requires_open() {
        let (ids, saved, archived, mut workspace) = summary_fixture();
        assert_eq!(
            validate_summary(
                &ids,
                &saved,
                &archived,
                &workspace,
                QaScenario::CleanupReview
            ),
            Err(QaError::CheckFailed)
        );
        workspace.owner.phase = ChatOwnerPhase::Finalized;
        assert!(
            validate_summary(
                &ids,
                &saved,
                &archived,
                &workspace,
                QaScenario::CleanupReview
            )
            .is_ok()
        );
        assert_eq!(
            validate_summary(&ids, &saved, &archived, &workspace, QaScenario::History),
            Err(QaError::CheckFailed)
        );
        workspace.process_claimed = true;
        assert_eq!(
            validate_summary(
                &ids,
                &saved,
                &archived,
                &workspace,
                QaScenario::CleanupReview
            ),
            Err(QaError::CheckFailed)
        );
    }

    #[test]
    fn surface_baseline_requires_eight_two_four_closed_records_and_open_settled_owner() {
        let (ids, mut saved, archived, mut workspace) = summary_fixture();
        saved.record_count = 8;
        workspace.session.record_count = 4;
        assert_eq!(
            validate_summary(&ids, &saved, &archived, &workspace, QaScenario::Surfaces),
            Ok(SeedSummary {
                saved_records: 8,
                archived_records: 2,
                workspace_records: 4,
                process_claims_present: false,
            })
        );
        assert_eq!(
            validate_summary(&ids, &saved, &archived, &workspace, QaScenario::History),
            Err(QaError::CheckFailed)
        );
        for fault in [
            "saved_count",
            "archived_count",
            "workspace_count",
            "phase",
            "claim",
            "active",
            "grants",
            "settled",
            "identity",
        ] {
            let mut saved = saved.clone();
            let mut archived = archived.clone();
            let mut workspace = workspace.clone();
            match fault {
                "saved_count" => saved.record_count = 4,
                "archived_count" => archived.record_count = 4,
                "workspace_count" => workspace.session.record_count = 2,
                "phase" => workspace.owner.phase = ChatOwnerPhase::Finalized,
                "claim" => workspace.process_claimed = true,
                "active" => saved.active_request_id = Some(Uuid::new_v4()),
                "grants" => {
                    workspace.owner.admitted_tools = vec![vibemux_types::ToolName::WriteFiles]
                }
                "settled" => workspace.owner.workspace_effect = None,
                _ => workspace.session.project_id = vibemux_types::ProjectId::new(),
            }
            assert_eq!(
                validate_summary(&ids, &saved, &archived, &workspace, QaScenario::Surfaces),
                Err(QaError::CheckFailed),
                "{fault}"
            );
        }
    }
}
