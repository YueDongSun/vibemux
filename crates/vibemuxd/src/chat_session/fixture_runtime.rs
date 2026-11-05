//! Exact built-fixture runtime admission. This is not a live vendor profile.
use super::{
    actor::{LaunchCheck, spawn_runtime_with_machine},
    contracts::*,
    execution_sink::ChatExecutionSink,
};
use crate::{
    chat_writer::{ChatApprovalEvidence, ChatWriter},
    harness_dispatch::{git_head, native_process::LaunchPlan},
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, fmt, io::Write, path::PathBuf, sync::Arc};
use vibemux_harness::{
    AgentKind,
    chat_session::{SessionConfig, SessionLimits, SessionMachine, SessionTarget},
    dispatch::{
        DispatchRoute, NativeProtocol,
        fixture_guard::{FixtureLaunchGuard, build_writable_fixture_launch_spec},
        launch_spec::interpreter_script_path,
        writable_profile::WritableGrant,
    },
};
use vibemux_types::{
    ToolName,
    a2a::RunWorkspace,
    chat::ChatState,
    chat_owner::{ChatOwnerPhase, ChatWorkspaceEffect},
};
use vibemux_workspace::WorkspaceManager;

pub const FIXTURE_APPROVAL_FILE: &str = "approved_runtime.txt";
pub const FIXTURE_APPROVAL_CONTENT: &str = "approved fixture\n";
const CONFIG_BYTES: u64 = 16 * 1024;
const TRAMPOLINE_BYTES: u64 = 32 * 1024 * 1024;

pub struct FixtureApprovalSettings {
    pub project_root: PathBuf,
    pub git: PathBuf,
    pub vendor_directory: PathBuf,
    pub trampoline: PathBuf,
    pub runtime_limits: RuntimeLimits,
}
pub(super) struct FixtureApprovalScope {
    manager: WorkspaceManager,
    workspace: RunWorkspace,
    root: PathBuf,
    root_identity: Arc<same_file::Handle>,
    target: PathBuf,
    input: Value,
}
impl fmt::Debug for FixtureApprovalScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FixtureApprovalScope(<owned>)")
    }
}
impl FixtureApprovalScope {
    pub async fn verify(&self, body: &ChatApprovalEvidence) -> Result<String, RuntimeError> {
        if body.tool != ToolName::WriteFiles
            || body.vendor_tool_name != "Write"
            || body.input != self.input
        {
            return Err(RuntimeError::ClaimRefused);
        }
        self.manager
            .inspect(&self.workspace)
            .await
            .map_err(|_| RuntimeError::ClaimRefused)?;
        let root = self.root.clone();
        let target = self.target.clone();
        let expected_identity = self.root_identity.clone();
        let expected_commit = self.workspace.base_commit.clone();
        tokio::task::spawn_blocking(move || {
            plain_directory(&root)?;
            let identity =
                same_file::Handle::from_path(&root).map_err(|_| RuntimeError::ClaimRefused)?;
            if identity != *expected_identity {
                return Err(RuntimeError::ClaimRefused);
            }
            require_absent(&target)?;
            let observed =
                git_head::read_head_commit(&root).map_err(|_| RuntimeError::ClaimRefused)?;
            if observed != expected_commit {
                return Err(RuntimeError::ClaimRefused);
            }
            Ok(observed)
        })
        .await
        .map_err(|_| RuntimeError::ClaimRefused)?
    }
}
pub(super) struct FixtureSpawnCheck {
    guard: FixtureLaunchGuard,
    route: DispatchRoute,
    config: PathBuf,
    config_bytes: Vec<u8>,
    trampoline: PathBuf,
    trampoline_fingerprint: vibemux_harness::dispatch::Sha256Digest,
    working_directory: PathBuf,
}
impl FixtureSpawnCheck {
    pub fn verify(&self, launch: &LaunchPlan) -> Result<(), RuntimeError> {
        let expected = build_writable_fixture_launch_spec(
            &self.route,
            WritableGrant {
                write_files: true,
                ..WritableGrant::NONE
            },
            &self.guard,
        )
        .map_err(|_| RuntimeError::InvalidConfiguration)?;
        let bytes = crate::bounded_file::read_regular_file(&self.config, CONFIG_BYTES)
            .map_err(|_| RuntimeError::InvalidConfiguration)?;
        if expected != launch.spec
            || launch.executable != self.route.executable
            || launch.trampoline != self.trampoline
            || launch.working_directory != self.working_directory
            || trampoline_fingerprint(&self.trampoline)? != self.trampoline_fingerprint
            || bytes != self.config_bytes
        {
            return Err(RuntimeError::InvalidConfiguration);
        }
        Ok(())
    }
}
fn trampoline_fingerprint(
    path: &std::path::Path,
) -> Result<vibemux_harness::dispatch::Sha256Digest, RuntimeError> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| RuntimeError::InvalidConfiguration)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(RuntimeError::InvalidConfiguration);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(RuntimeError::InvalidConfiguration);
        }
    }
    let bytes = crate::bounded_file::read_regular_file(path, TRAMPOLINE_BYTES)
        .map_err(|_| RuntimeError::InvalidConfiguration)?;
    Ok(vibemux_harness::dispatch::Sha256Digest::of(&bytes))
}
fn plain_directory(path: &std::path::Path) -> Result<(), RuntimeError> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| RuntimeError::InvalidConfiguration)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(RuntimeError::InvalidConfiguration);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(RuntimeError::InvalidConfiguration);
        }
    }
    Ok(())
}
fn require_absent(path: &std::path::Path) -> Result<(), RuntimeError> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(RuntimeError::ClaimRefused),
    }
}

/// Test callers prepare a canonical owner/worktree and fresh process claim.
/// This function never admits a prompt and cannot resume an old native root.
pub async fn spawn_fixture_approval_runtime(
    writer: ChatWriter,
    binding: RuntimeBinding,
    settings: FixtureApprovalSettings,
    protocol: SessionConfig,
) -> Result<RuntimeOwner, RuntimeError> {
    spawn_fixture_approval_runtime_with_sink(writer, binding, settings, protocol, |sink| sink).await
}
/// Test-only dependency injection for observing callback/receipt races. The
/// executable and fixed effect remain guarded independently of this sink.
pub async fn spawn_fixture_approval_runtime_with_sink<F>(
    writer: ChatWriter,
    binding: RuntimeBinding,
    settings: FixtureApprovalSettings,
    protocol: SessionConfig,
    wrap: F,
) -> Result<RuntimeOwner, RuntimeError>
where
    F: FnOnce(Arc<dyn ChatRuntimeSink>) -> Arc<dyn ChatRuntimeSink>,
{
    spawn_fixture_approval_runtime_owned(writer, binding, settings, protocol, wrap)
        .await
        .map(|(owner, _)| owner)
}

/// Service fixtures retain the actual sink used by this owner for live reads.
pub(crate) async fn spawn_fixture_approval_runtime_owned<F>(
    writer: ChatWriter,
    binding: RuntimeBinding,
    settings: FixtureApprovalSettings,
    mut protocol: SessionConfig,
    wrap: F,
) -> Result<(RuntimeOwner, Arc<ChatExecutionSink>), RuntimeError>
where
    F: FnOnce(Arc<dyn ChatRuntimeSink>) -> Arc<dyn ChatRuntimeSink>,
{
    if !matches!(protocol.target, SessionTarget::Claude { resume: false, .. }) {
        return Err(RuntimeError::InvalidConfiguration);
    }
    let stored = writer
        .execution_owner(binding.chat_id)
        .await
        .map_err(|_| RuntimeError::WriterUnavailable)?
        .ok_or(RuntimeError::ClaimRefused)?;
    if stored.owner.phase != ChatOwnerPhase::Open
        || stored.owner.workspace_effect != Some(ChatWorkspaceEffect::Settled)
        || stored.owner.workspace_verified_at.is_none()
        || stored.owner.admitted_tools != BTreeSet::from([ToolName::WriteFiles])
        || stored.session.state != ChatState::Opening
        || stored.session.generation != binding.generation
        || binding.generation != 1
        || stored.session.vendor_session_id.is_some()
        || stored.session.expected_vendor_id != protocol.target.expected_id()
    {
        return Err(RuntimeError::ClaimRefused);
    }
    let workspace = stored.owner.workspace.ok_or(RuntimeError::ClaimRefused)?;
    let manager = WorkspaceManager::new(
        settings.git,
        settings.project_root,
        stored.owner.base_commit,
    )
    .await
    .map_err(|_| RuntimeError::ClaimRefused)?;
    manager
        .inspect(&workspace)
        .await
        .map_err(|_| RuntimeError::ClaimRefused)?;
    let root =
        std::fs::canonicalize(&workspace.path).map_err(|_| RuntimeError::InvalidConfiguration)?;
    let working_directory =
        interpreter_script_path(root.to_str().ok_or(RuntimeError::InvalidConfiguration)?);
    protocol.working_directory = working_directory.clone();
    let target = PathBuf::from(&working_directory).join(FIXTURE_APPROVAL_FILE);
    let input = json!({"file_path":target,"content":FIXTURE_APPROVAL_CONTENT});
    let vendor = settings.vendor_directory;
    let supplied_trampoline = settings.trampoline;
    let setup_root = root.clone();
    let setup_target = target.clone();
    let expected_input = input.clone();
    let setup = tokio::task::spawn_blocking(move || {
        plain_directory(&setup_root)?;
        require_absent(&setup_target)?;
        let image = std::env::current_exe().map_err(|_| RuntimeError::InvalidConfiguration)?;
        let parent = image.parent().ok_or(RuntimeError::InvalidConfiguration)?;
        let profile = if parent.file_name().is_some_and(|s| s == "deps") {
            parent.parent().ok_or(RuntimeError::InvalidConfiguration)?
        } else {
            parent
        };
        let built = profile.join(format!(
            "vibemux_chat_fixture{}",
            std::env::consts::EXE_SUFFIX
        ));
        let expected_trampoline = std::fs::canonicalize(profile.join(format!(
            "vibemux_launch_trampoline{}",
            std::env::consts::EXE_SUFFIX
        )))
        .map_err(|_| RuntimeError::InvalidConfiguration)?;
        let fingerprint = trampoline_fingerprint(&supplied_trampoline)?;
        if std::fs::canonicalize(&supplied_trampoline)
            .map_err(|_| RuntimeError::InvalidConfiguration)?
            != expected_trampoline
            || trampoline_fingerprint(&expected_trampoline)? != fingerprint
        {
            return Err(RuntimeError::InvalidConfiguration);
        }
        let guard =
            FixtureLaunchGuard::register_worker_copies(&built, &vendor, &[AgentKind::Claude])
                .map_err(|_| RuntimeError::InvalidConfiguration)?;
        let route = DispatchRoute {
            harness: AgentKind::Claude,
            protocol: NativeProtocol::ClaudeStreamJson,
            executable: std::fs::canonicalize(
                vendor.join(format!("claude{}", std::env::consts::EXE_SUFFIX)),
            )
            .map_err(|_| RuntimeError::InvalidConfiguration)?,
            entry_script: None,
            profile: None,
            provider: None,
            environment_names: Vec::new(),
            enabled: true,
            allow_execution: true,
            model: None,
        };
        let spec = build_writable_fixture_launch_spec(
            &route,
            WritableGrant {
                write_files: true,
                ..WritableGrant::NONE
            },
            &guard,
        )
        .map_err(|_| RuntimeError::InvalidConfiguration)?;
        let config = vendor.join("chat_fixture.json");
        let config_bytes = serde_json::to_vec(
            &json!({"mode":"claude_runtime_approval","effect_path":setup_target}),
        )
        .map_err(|_| RuntimeError::InvalidConfiguration)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&config)
            .map_err(|_| RuntimeError::InvalidConfiguration)?;
        file.write_all(&config_bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| RuntimeError::InvalidConfiguration)?;
        let scope = FixtureApprovalScope {
            manager,
            workspace,
            root: setup_root.clone(),
            root_identity: Arc::new(
                same_file::Handle::from_path(&setup_root)
                    .map_err(|_| RuntimeError::InvalidConfiguration)?,
            ),
            target: setup_target,
            input: expected_input,
        };
        Ok::<_, RuntimeError>((
            spec,
            FixtureSpawnCheck {
                guard,
                route,
                config,
                config_bytes,
                trampoline: expected_trampoline,
                trampoline_fingerprint: fingerprint,
                working_directory: setup_root.clone(),
            },
            Arc::new(scope),
        ))
    })
    .await
    .map_err(|_| RuntimeError::InvalidConfiguration)??;
    let (spec, check, scope) = setup;
    let trampoline = check.trampoline.clone();
    let machine =
        SessionMachine::new_fixture_file_approval(protocol.clone(), SessionLimits::default())?;
    let plan = RuntimePlan {
        binding: binding.clone(),
        launch: LaunchPlan {
            executable: spec.executable.clone(),
            spec,
            trampoline,
            working_directory: root,
        },
        protocol,
        protocol_limits: SessionLimits::default(),
        limits: settings.runtime_limits,
    };
    let sink = Arc::new(ChatExecutionSink::new_fixture(writer, binding, scope));
    let owner = spawn_runtime_with_machine(
        plan,
        wrap(sink.clone()),
        machine,
        LaunchCheck::Fixture(Box::new(check)),
    )?;
    Ok((owner, sink))
}
