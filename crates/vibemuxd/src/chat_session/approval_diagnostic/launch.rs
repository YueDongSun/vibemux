//! Fixed candidate preparation and explicit contained diagnostic execution.
use super::{DiagnosticLimits, DiagnosticReport, ProbeContext, driver::drive_probe};
use crate::{
    chat_session::transport::NativeTransport,
    harness_dispatch::native_process::{LaunchPlan, NativeProcess, resolve_environment},
};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fmt,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    io::AsyncReadExt,
    sync::watch,
    time::{Instant, timeout},
};
use uuid::Uuid;
use vibemux_harness::{
    AgentKind,
    dispatch::{
        DispatchRoute, NativeProtocol,
        fixture_guard::{FixtureLaunchGuard, build_writable_fixture_launch_spec},
        launch_spec::{LaunchSpec, SYSTEM_ENVIRONMENT_NAMES, interpreter_script_path},
        writable_profile::WritableGrant,
    },
};

const CANDIDATE_VERSION: &str = "2.1.304";
const MAX_IMAGE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_VERSION_BYTES: u64 = 128;
const IMAGE_CHUNK_BYTES: usize = 64 * 1024;
const PROBE_FILE: &str = "approval_diagnostic.txt";
const PROBE_CONTENT: &str = "deny-only diagnostic\n";
const PROVIDER_ENVIRONMENT_NAMES: [&str; 8] = [
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_MODEL",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "CLAUDE_CONFIG_DIR",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum DiagnosticPreparationError {
    #[error("diagnostic_invalid_configuration")]
    InvalidConfiguration,
    #[error("diagnostic_source_changed")]
    SourceChanged,
    #[error("diagnostic_workspace_changed")]
    WorkspaceChanged,
    #[error("diagnostic_version_mismatch")]
    VersionMismatch,
    #[error("diagnostic_environment_unavailable")]
    EnvironmentUnavailable,
    #[error("diagnostic_process_unavailable")]
    ProcessUnavailable,
    #[error("diagnostic_cleanup_unconfirmed")]
    CleanupUnconfirmed,
    #[error("diagnostic_cancelled")]
    Cancelled,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct DiagnosticCandidateSummary {
    pub candidate_version: &'static str,
    pub fixture_only: bool,
    pub maximum_user_inputs: u8,
    pub affirmative_response_available: bool,
    pub policy_sources_qualified: bool,
    pub production_profile_qualified: bool,
}

#[derive(Debug, Serialize)]
pub struct DiagnosticExecution {
    pub candidate: DiagnosticCandidateSummary,
    pub version_verified: bool,
    pub report: DiagnosticReport,
    pub workspace_identity_verified: bool,
    pub probe_artifact_present: Option<bool>,
    pub workspace_retained: bool,
}

struct ImageBinding {
    path: PathBuf,
    identity: same_file::Handle,
    fingerprint: [u8; 32],
}

impl ImageBinding {
    fn pin(path: &Path) -> Result<Self, DiagnosticPreparationError> {
        plain(path, false)?;
        let path = std::fs::canonicalize(path)
            .map_err(|_| DiagnosticPreparationError::InvalidConfiguration)?;
        Ok(Self {
            identity: same_file::Handle::from_path(&path)
                .map_err(|_| DiagnosticPreparationError::InvalidConfiguration)?,
            fingerprint: image_fingerprint(&path)?,
            path,
        })
    }
    fn verify(&self) -> Result<(), DiagnosticPreparationError> {
        plain(&self.path, false)?;
        let identity = same_file::Handle::from_path(&self.path)
            .map_err(|_| DiagnosticPreparationError::SourceChanged)?;
        if identity != self.identity || image_fingerprint(&self.path)? != self.fingerprint {
            return Err(DiagnosticPreparationError::SourceChanged);
        }
        Ok(())
    }
}

/// A fixed, fresh, deny-only candidate. Preparing it starts no executable and
/// reads no credentials. The created empty workspace is owned by this object.
/// Debug and the summary omit source paths, argv, prompt and environment values.
pub struct DiagnosticCandidate {
    executable: ImageBinding,
    trampoline: ImageBinding,
    workspace: PathBuf,
    workspace_identity: Arc<same_file::Handle>,
    fixture: Option<FixtureLaunchGuard>,
    fixture_config: Option<(PathBuf, Vec<u8>)>,
    environment_names: Vec<String>,
    session_id: Uuid,
    user_id: Uuid,
    limits: DiagnosticLimits,
}

impl fmt::Debug for DiagnosticCandidate {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output
            .debug_struct("DiagnosticCandidate")
            .field("summary", &self.summary())
            .finish_non_exhaustive()
    }
}

impl DiagnosticCandidate {
    /// Prepares the exact installed-version candidate for a separately authorized
    /// diagnostic. This is not production admission or policy qualification.
    pub async fn prepare_native(
        executable: PathBuf,
        workspace_parent: PathBuf,
        environment_names: Vec<String>,
        limits: DiagnosticLimits,
    ) -> Result<Self, DiagnosticPreparationError> {
        if !limits.valid() {
            return Err(DiagnosticPreparationError::InvalidConfiguration);
        }
        let environment_names = environment_allowlist(environment_names)?;
        tokio::task::spawn_blocking(move || {
            let executable = ImageBinding::pin(&executable)?;
            if executable.path.file_name().and_then(|name| name.to_str()) != Some("claude.exe") {
                return Err(DiagnosticPreparationError::InvalidConfiguration);
            }
            Self::prepare(
                executable,
                workspace_parent,
                environment_names,
                limits,
                None,
                None,
            )
        })
        .await
        .map_err(|_| DiagnosticPreparationError::InvalidConfiguration)?
    }

    /// The synthetic image is derived from this build, copied through its opaque
    /// guard, and cannot be replaced by a JSON fixture label or arbitrary path.
    pub async fn prepare_fixture(
        workspace_parent: PathBuf,
        vendor_directory: PathBuf,
        limits: DiagnosticLimits,
    ) -> Result<Self, DiagnosticPreparationError> {
        if !limits.valid() {
            return Err(DiagnosticPreparationError::InvalidConfiguration);
        }
        tokio::task::spawn_blocking(move || {
            let built = build_profile()?.join(format!("vibemux_chat_fixture{}", std::env::consts::EXE_SUFFIX));
            plain(&vendor_directory, true)?;
            plain(&built, false)?;
            let copy = vendor_directory.join(format!("claude{}", std::env::consts::EXE_SUFFIX));
            let source = File::open(&built).map_err(|_| DiagnosticPreparationError::InvalidConfiguration)?;
            let mut destination = std::fs::OpenOptions::new().write(true).create_new(true).open(&copy)
                .map_err(|_| DiagnosticPreparationError::InvalidConfiguration)?;
            let copied = std::io::copy(&mut source.take(MAX_IMAGE_BYTES + 1), &mut destination)
                .map_err(|_| DiagnosticPreparationError::InvalidConfiguration)?;
            if copied > MAX_IMAGE_BYTES {
                return Err(DiagnosticPreparationError::InvalidConfiguration);
            }
            destination.sync_all().map_err(|_| DiagnosticPreparationError::InvalidConfiguration)?;
            let guard = FixtureLaunchGuard::register_worker_copies(&built, &vendor_directory, &[AgentKind::Claude])
                .map_err(|_| DiagnosticPreparationError::InvalidConfiguration)?;
            let executable = ImageBinding::pin(&copy)?;
            let mut candidate = Self::prepare(executable, workspace_parent,
                environment_allowlist(Vec::new())?, limits, Some(guard), None)?;
            let config_path = vendor_directory.join("chat_fixture.json");
            let bytes = serde_json::to_vec(&json!({"mode":"claude_approval", "effect_path":candidate.workspace.join(PROBE_FILE),
                "launch_path":vendor_directory.join("diagnostic_started")}))
                .map_err(|_| DiagnosticPreparationError::InvalidConfiguration)?;
            use std::io::Write;
            let mut config = std::fs::OpenOptions::new().write(true).create_new(true).open(&config_path)
                .map_err(|_| DiagnosticPreparationError::InvalidConfiguration)?;
            config.write_all(&bytes).and_then(|_| config.sync_all())
                .map_err(|_| DiagnosticPreparationError::InvalidConfiguration)?;
            candidate.fixture_config = Some((config_path, bytes));
            Ok(candidate)
        }).await.map_err(|_| DiagnosticPreparationError::InvalidConfiguration)?
    }

    fn prepare(
        executable: ImageBinding,
        workspace_parent: PathBuf,
        environment_names: Vec<String>,
        limits: DiagnosticLimits,
        fixture: Option<FixtureLaunchGuard>,
        fixture_config: Option<(PathBuf, Vec<u8>)>,
    ) -> Result<Self, DiagnosticPreparationError> {
        plain(&workspace_parent, true)?;
        let parent = std::fs::canonicalize(workspace_parent)
            .map_err(|_| DiagnosticPreparationError::InvalidConfiguration)?;
        let trampoline = ImageBinding::pin(&build_profile()?.join(format!(
            "vibemux_launch_trampoline{}",
            std::env::consts::EXE_SUFFIX
        )))?;
        let workspace = parent.join(format!("approval_probe_{}", Uuid::new_v4().simple()));
        std::fs::create_dir(&workspace)
            .map_err(|_| DiagnosticPreparationError::InvalidConfiguration)?;
        let workspace_identity = same_file::Handle::from_path(&workspace)
            .map_err(|_| DiagnosticPreparationError::InvalidConfiguration)?;
        Ok(Self {
            executable,
            trampoline,
            workspace,
            workspace_identity: Arc::new(workspace_identity),
            fixture,
            fixture_config,
            environment_names,
            session_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            limits,
        })
    }

    pub fn summary(&self) -> DiagnosticCandidateSummary {
        DiagnosticCandidateSummary {
            candidate_version: CANDIDATE_VERSION,
            fixture_only: self.fixture.is_some(),
            maximum_user_inputs: 1,
            affirmative_response_available: false,
            policy_sources_qualified: false,
            production_profile_qualified: false,
        }
    }

    /// Explicit access for reviewing the disposable target; never included in
    /// the default summary, Debug or execution report.
    pub fn workspace_path(&self) -> &Path {
        &self.workspace
    }

    pub fn launch_spec(&self) -> LaunchSpec {
        let settings = json!({"permissions":{"ask":["Write"]}}).to_string();
        LaunchSpec {
            executable: self.executable.path.clone(),
            environment_names: self.environment_names.clone(),
            arguments: vec![
                "-p".into(),
                "--input-format".into(),
                "stream-json".into(),
                "--output-format".into(),
                "stream-json".into(),
                "--verbose".into(),
                "--include-partial-messages".into(),
                "--permission-mode".into(),
                "manual".into(),
                "--permission-prompts".into(),
                "host".into(),
                "--tools=Write".into(),
                "--strict-mcp-config".into(),
                "--restricted".into(),
                "--safe-mode".into(),
                "--no-chrome".into(),
                "--disable-slash-commands".into(),
                "--no-session-persistence".into(),
                "--replay-user-messages".into(),
                "--session-id".into(),
                self.session_id.to_string(),
                "--max-turns".into(),
                "1".into(),
                "--settings".into(),
                settings,
            ],
        }
    }

    fn verify(&self) -> Result<(), DiagnosticPreparationError> {
        self.executable.verify()?;
        self.trampoline.verify()?;
        plain(&self.workspace, true)?;
        let identity = same_file::Handle::from_path(&self.workspace)
            .map_err(|_| DiagnosticPreparationError::WorkspaceChanged)?;
        if identity != *self.workspace_identity
            || std::fs::read_dir(&self.workspace)
                .map_err(|_| DiagnosticPreparationError::WorkspaceChanged)?
                .next()
                .is_some()
        {
            return Err(DiagnosticPreparationError::WorkspaceChanged);
        }
        if let Some(guard) = &self.fixture {
            let route = DispatchRoute {
                harness: AgentKind::Claude,
                protocol: NativeProtocol::ClaudeStreamJson,
                executable: self.executable.path.clone(),
                entry_script: None,
                profile: None,
                provider: None,
                environment_names: Vec::new(),
                enabled: true,
                allow_execution: true,
                model: None,
            };
            build_writable_fixture_launch_spec(&route, WritableGrant::NONE, guard)
                .map_err(|_| DiagnosticPreparationError::SourceChanged)?;
        }
        if let Some((path, bytes)) = &self.fixture_config {
            plain(path, false)?;
            if std::fs::metadata(path)
                .map_err(|_| DiagnosticPreparationError::SourceChanged)?
                .len()
                != bytes.len() as u64
                || std::fs::read(path).map_err(|_| DiagnosticPreparationError::SourceChanged)?
                    != *bytes
            {
                return Err(DiagnosticPreparationError::SourceChanged);
            }
        }
        Ok(())
    }

    /// Launches a vendor and can consume model resources. The caller must first
    /// obtain the separate scoped diagnostic budget and review surviving policy
    /// sources. A report is observational and never certifies those sources or
    /// authorizes production approvals. No service/config/renderer calls this API.
    /// This value is consumed: a lost result cannot be retried with the same plan.
    pub async fn run(
        self,
        cancellation: watch::Receiver<bool>,
    ) -> Result<DiagnosticExecution, DiagnosticPreparationError> {
        if *cancellation.borrow() || cancellation.has_changed().is_err() {
            return Err(DiagnosticPreparationError::Cancelled);
        }
        let candidate = tokio::task::spawn_blocking(move || {
            self.verify()?;
            Ok(self)
        })
        .await
        .map_err(|_| DiagnosticPreparationError::SourceChanged)??;
        let environment = resolve_environment(&candidate.environment_names)
            .map_err(|_| DiagnosticPreparationError::EnvironmentUnavailable)?;
        let native = candidate.fixture.is_none();
        if native {
            candidate
                .verify_version(environment.clone(), cancellation.clone())
                .await?;
        }
        let candidate = tokio::task::spawn_blocking(move || {
            candidate.verify()?;
            Ok(candidate)
        })
        .await
        .map_err(|_| DiagnosticPreparationError::SourceChanged)??;
        if *cancellation.borrow() || cancellation.has_changed().is_err() {
            return Err(DiagnosticPreparationError::Cancelled);
        }
        let working_directory = interpreter_script_path(
            candidate
                .workspace
                .to_str()
                .ok_or(DiagnosticPreparationError::InvalidConfiguration)?,
        );
        let plan = LaunchPlan {
            trampoline: candidate.trampoline.path.clone(),
            executable: candidate.executable.path.clone(),
            spec: candidate.launch_spec(),
            working_directory: candidate.workspace.clone(),
        };
        let process = NativeProcess::spawn(&plan, environment)
            .await
            .map_err(|_| DiagnosticPreparationError::ProcessUnavailable)?;
        let prompt = format!(
            "Use Write exactly once to create {} containing {:?}. Do not use other tools or paths. If permission is denied, stop; do not retry.",
            PROBE_FILE, PROBE_CONTENT
        );
        let report = drive_probe(
            NativeTransport(process),
            ProbeContext {
                session_id: candidate.session_id,
                user_id: candidate.user_id,
                working_directory,
                prompt,
                limits: candidate.limits,
            },
            cancellation,
        )
        .await;
        tokio::task::spawn_blocking(move || {
            let identity = plain(&candidate.workspace, true).is_ok()
                && same_file::Handle::from_path(&candidate.workspace)
                    .is_ok_and(|handle| handle == *candidate.workspace_identity);
            let artifact = if identity {
                match std::fs::symlink_metadata(candidate.workspace.join(PROBE_FILE)) {
                    Ok(_) => Some(true),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(false),
                    _ => None,
                }
            } else {
                None
            };
            let removed = identity
                && report.cleanup.cleanup_verified
                && std::fs::remove_dir(&candidate.workspace).is_ok();
            Ok(DiagnosticExecution {
                candidate: candidate.summary(),
                version_verified: native,
                report,
                workspace_identity_verified: identity,
                probe_artifact_present: artifact,
                workspace_retained: !removed,
            })
        })
        .await
        .map_err(|_| DiagnosticPreparationError::WorkspaceChanged)?
    }

    /// Removes only this still-empty prepared workspace. Unknown files are kept.
    pub async fn discard(self) -> Result<(), DiagnosticPreparationError> {
        tokio::task::spawn_blocking(move || {
            self.verify()?;
            std::fs::remove_dir(&self.workspace)
                .map_err(|_| DiagnosticPreparationError::WorkspaceChanged)
        })
        .await
        .map_err(|_| DiagnosticPreparationError::WorkspaceChanged)?
    }

    async fn verify_version(
        &self,
        environment: Vec<(String, std::ffi::OsString)>,
        mut cancellation: watch::Receiver<bool>,
    ) -> Result<(), DiagnosticPreparationError> {
        if *cancellation.borrow() || cancellation.has_changed().is_err() {
            return Err(DiagnosticPreparationError::Cancelled);
        }
        let plan = LaunchPlan {
            trampoline: self.trampoline.path.clone(),
            executable: self.executable.path.clone(),
            working_directory: self.workspace.clone(),
            spec: LaunchSpec {
                executable: self.executable.path.clone(),
                arguments: vec!["--version".into()],
                environment_names: self.environment_names.clone(),
            },
        };
        let mut process = NativeProcess::spawn(&plan, environment)
            .await
            .map_err(|_| DiagnosticPreparationError::ProcessUnavailable)?;
        process.close_stdin();
        let mut bytes = Vec::new();
        let read = {
            let mut version_output = (&mut process.stdout).take(MAX_VERSION_BYTES + 1);
            tokio::select! {
                read=timeout(self.limits.initialize,version_output.read_to_end(&mut bytes))=>Some(read),
                _=cancellation.changed()=>None,
            }
        };
        let cleanup = process
            .finish_session(
                !matches!(read, Some(Ok(Ok(_)))),
                Instant::now() + self.limits.shutdown,
            )
            .await;
        if !cleanup.cleanup_verified {
            return Err(DiagnosticPreparationError::CleanupUnconfirmed);
        }
        if read.is_none() {
            return Err(DiagnosticPreparationError::Cancelled);
        }
        if !matches!(read, Some(Ok(Ok(_))))
            || bytes.len() as u64 > MAX_VERSION_BYTES
            || cleanup.exit.exit_code != Some(0)
            || std::str::from_utf8(&bytes).ok().map(str::trim) != Some("2.1.304 (Claude Code)")
        {
            return Err(DiagnosticPreparationError::VersionMismatch);
        }
        Ok(())
    }
}

fn build_profile() -> Result<PathBuf, DiagnosticPreparationError> {
    let image =
        std::env::current_exe().map_err(|_| DiagnosticPreparationError::InvalidConfiguration)?;
    let parent = image
        .parent()
        .ok_or(DiagnosticPreparationError::InvalidConfiguration)?;
    Ok(if parent.file_name().is_some_and(|name| name == "deps") {
        parent
            .parent()
            .ok_or(DiagnosticPreparationError::InvalidConfiguration)?
    } else {
        parent
    }
    .to_owned())
}

fn environment_allowlist(names: Vec<String>) -> Result<Vec<String>, DiagnosticPreparationError> {
    let mut result = SYSTEM_ENVIRONMENT_NAMES
        .iter()
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>();
    for name in names {
        if !SYSTEM_ENVIRONMENT_NAMES.contains(&name.as_str())
            && !PROVIDER_ENVIRONMENT_NAMES.contains(&name.as_str())
        {
            return Err(DiagnosticPreparationError::InvalidConfiguration);
        }
        if !result.contains(&name) {
            result.push(name);
        }
    }
    Ok(result)
}

fn plain(path: &Path, directory: bool) -> Result<(), DiagnosticPreparationError> {
    if !path.is_absolute() {
        return Err(DiagnosticPreparationError::InvalidConfiguration);
    }
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|_| DiagnosticPreparationError::InvalidConfiguration)?;
    if metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return Err(DiagnosticPreparationError::InvalidConfiguration);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(DiagnosticPreparationError::InvalidConfiguration);
        }
    }
    Ok(())
}

fn image_fingerprint(path: &Path) -> Result<[u8; 32], DiagnosticPreparationError> {
    let mut file =
        File::open(path).map_err(|_| DiagnosticPreparationError::InvalidConfiguration)?;
    let size = file
        .metadata()
        .map_err(|_| DiagnosticPreparationError::InvalidConfiguration)?
        .len();
    if size == 0 || size > MAX_IMAGE_BYTES {
        return Err(DiagnosticPreparationError::InvalidConfiguration);
    }
    let mut hash = Sha256::new();
    let mut total = 0;
    let mut chunk = [0u8; IMAGE_CHUNK_BYTES];
    loop {
        let read = file
            .read(&mut chunk)
            .map_err(|_| DiagnosticPreparationError::SourceChanged)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > MAX_IMAGE_BYTES {
            return Err(DiagnosticPreparationError::SourceChanged);
        }
        hash.update(&chunk[..read]);
    }
    if total != size {
        return Err(DiagnosticPreparationError::SourceChanged);
    }
    Ok(hash.finalize().into())
}
