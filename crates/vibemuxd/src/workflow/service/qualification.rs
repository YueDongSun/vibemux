//! Explicit native worker qualification, bound to this daemon's pinned config.
//! The ordinary writer/dispatch/workspace owners remain the only authorities.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_harness::{
    AgentKind,
    dispatch::{Sha256Digest, writable_profile::WritableGrant},
};
use vibemux_types::{RunId, a2a::RunWorkspace};
use vibemux_workflow::{
    SpecIdentifier,
    slots::{CapabilityEvidence, EvidenceClass, EvidenceLevel, SlotCapability, SlotFacts},
};
use vibemux_workspace::WorkspaceManager;

use crate::{
    WriterHandle,
    harness_dispatch::{
        HarnessDispatchService, call_writer, git_head::read_head_commit,
        qualification::QualificationTurn,
    },
    workflow::{
        error::WorkflowError,
        runtime::{WorkflowSetup, now_ms},
        turns::derived_id,
    },
};

use super::WorkflowService;

const EVIDENCE_PREFIX: &str = "native_qualification_v1";
const QUALIFICATION_TTL_MS: u64 = 60 * 60 * 1000;
const CAPABILITIES: [SlotCapability; 2] = [
    SlotCapability::StructuredTurn,
    SlotCapability::WritableWorktree,
];

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualificationRequest {
    pub event_id: Uuid,
    pub slot_id: SpecIdentifier,
}

#[derive(Clone, Debug, Serialize)]
pub struct QualificationResponse {
    pub schema_version: u32,
    pub event_id: Uuid,
    pub slot_id: SpecIdentifier,
    pub qualified: bool,
    pub evidence_class: EvidenceClass,
    pub expires_at_ms: Option<u64>,
    pub code: Option<&'static str>,
    pub dispatch_ids: [Uuid; 2],
}

impl WorkflowService {
    /// This explicit operation may consume two native CLI turns. Readiness
    /// and doctor never call it. An already admitted event is never replayed.
    pub async fn qualify(
        &self,
        request: QualificationRequest,
    ) -> Result<QualificationResponse, WorkflowError> {
        if request.event_id.is_nil() {
            return Err(WorkflowError::RequestInvalid);
        }
        let _permit = self
            .inner
            .lifecycle_gate
            .acquire()
            .await
            .map_err(|_| WorkflowError::ShuttingDown)?;
        {
            let runs = self.inner.runs();
            if !runs.accepting {
                return Err(WorkflowError::ShuttingDown);
            }
            if !runs.active.is_empty() {
                return Err(WorkflowError::AlreadyRunning);
            }
        }
        let setup = self.inner.setup()?;
        if setup.config.config.evidence_class != EvidenceClass::Live
            || !setup.config.config.native_workers
        {
            return Err(WorkflowError::SlotUnavailable);
        }
        let slot = setup
            .config
            .slot(&request.slot_id)
            .ok_or(WorkflowError::SlotUnavailable)?;
        if slot.harness != AgentKind::Claude {
            return Err(WorkflowError::SlotUnavailable);
        }
        let dispatch_ids = [
            derived_id(
                "vibemux.qualification.write.v1",
                &[request.event_id.as_bytes()],
            ),
            derived_id(
                "vibemux.qualification.read_only.v1",
                &[request.event_id.as_bytes()],
            ),
        ];
        let mut response = QualificationResponse {
            schema_version: 1,
            event_id: request.event_id,
            slot_id: request.slot_id.clone(),
            qualified: false,
            evidence_class: EvidenceClass::Live,
            expires_at_ms: None,
            code: None,
            dispatch_ids,
        };
        let writer = self.inner.writer.clone();
        let mut slots = call_writer(move || writer.workflow_slots()).await?;
        self.check_qualified_routes(&setup, &mut slots).await?;
        let facts = slots
            .iter()
            .find(|facts| facts.slot_id == request.slot_id)
            .ok_or(WorkflowError::SlotUnavailable)?;
        if facts.leased_generation.is_some() {
            return Err(WorkflowError::AlreadyRunning);
        }
        let identity = self
            .inner
            .dispatch
            .qualification_identity(slot.harness)
            .await?;
        let evidence_ref = evidence_reference(setup.config.digest, identity, request.event_id);
        if let Some(evidence) = facts
            .capabilities
            .get(&SlotCapability::WritableWorktree)
            .filter(|evidence| {
                evidence.evidence_ref == evidence_ref && evidence.expires_at_ms > now_ms()
            })
        {
            response.qualified = true;
            response.expires_at_ms = Some(evidence.expires_at_ms);
            return Ok(response);
        }
        for dispatch_id in dispatch_ids {
            let writer = self.inner.writer.clone();
            if call_writer(move || writer.harness_dispatch(dispatch_id))
                .await?
                .is_some()
            {
                response.code = Some("qualification_previously_attempted");
                return Ok(response);
            }
        }
        let root = setup.canonical_root.clone();
        let base = tokio::task::spawn_blocking(move || read_head_commit(&root))
            .await
            .map_err(|_| WorkflowError::Internal)?
            .map_err(|_| WorkflowError::Git)?;
        let manager = WorkspaceManager::new(
            setup.config.git_executable.clone(),
            setup.canonical_root.clone(),
            base.clone(),
        )
        .await
        .map_err(|_| WorkflowError::Workspace)?;
        let workspace = manager
            .create(RunId::from_uuid(request.event_id).map_err(|_| WorkflowError::RequestInvalid)?)
            .await
            .map_err(|_| WorkflowError::Workspace)?;
        // The nonce is in a newly created input file, not in the prompt. The
        // canary therefore cannot pass from a canned final answer alone.
        let canary = Canary::new(request.event_id, Uuid::new_v4().simple().to_string());
        let initialized = create_canary(&manager, &workspace, &canary).await;
        let (qualified, cleanup_confirmed) = if initialized.is_ok() {
            self.run_qualification_probes(&workspace, &canary, slot.harness, dispatch_ids, base)
                .await
        } else {
            (false, true)
        };
        let unchanged = self
            .inner
            .dispatch
            .qualification_identity(slot.harness)
            .await
            .ok()
            == Some(identity);
        // Cleanup verifies ownership, HEAD, every changed path and every
        // canary byte before removing only files created by this operation.
        // Unexpected files/changes are retained and do not confer evidence.
        let cleaned =
            cleanup_confirmed && cleanup_canary(&manager, &workspace, &canary).await.is_ok();
        if !qualified || !unchanged || !cleaned {
            response.code = Some(if !cleaned {
                "qualification_workspace_retained"
            } else if !unchanged {
                "qualification_route_changed"
            } else {
                "qualification_canary_failed"
            });
            return Ok(response);
        }
        // Shutdown may have stopped admission while these bounded turns
        // were running. Their dispatch owners still join them, but a late
        // completion must not confer fresh qualification during shutdown.
        if !self.inner.runs().accepting {
            response.code = Some("qualification_cancelled");
            return Ok(response);
        }
        let checked_at_ms = now_ms();
        let expires_at_ms = checked_at_ms.saturating_add(QUALIFICATION_TTL_MS);
        let evidence = CapabilityEvidence {
            level: EvidenceLevel::WriteModeCertified,
            class: EvidenceClass::Live,
            evidence_ref,
            checked_at_ms,
            expires_at_ms,
        };
        let mut certified = facts.clone();
        certified.leased_generation = None;
        for capability in CAPABILITIES {
            certified.capabilities.insert(capability, evidence.clone());
        }
        let writer = self.inner.writer.clone();
        call_writer(move || {
            writer.upsert_workflow_slot(certified.clone(), OffsetDateTime::now_utc())
        })
        .await?;
        response.qualified = true;
        response.expires_at_ms = Some(expires_at_ms);
        Ok(response)
    }

    async fn run_qualification_probes(
        &self,
        workspace: &RunWorkspace,
        canary: &Canary,
        harness: AgentKind,
        dispatch_ids: [Uuid; 2],
        base: String,
    ) -> (bool, bool) {
        let command = format!("node --test {}/verify.test.cjs", canary.directory);
        let positive = self.inner.dispatch.run_qualification_turn(QualificationTurn {
            request_id: dispatch_ids[0], harness,
            prompt: format!(
                "This is an explicit bounded capability qualification in a disposable owned worktree. Touch only {0}/. Use Glob or Grep to locate input.txt in that directory, then use Read to read it. Use Write to create output.txt with exactly the input file bytes. Use Edit to replace the full edit.txt content with 'after ' followed by the input token and one newline. Do not alter input.txt, negative.txt or verify.test.cjs. Run exactly this Bash command, with no prefix, suffix or quoting changes: {1}. The test creates verified.txt. Do not write verified.txt yourself, do not use other commands, do not commit. Return a short completion message.",
                canary.directory, command,
            ),
            working_directory: PathBuf::from(&workspace.path),
            canary_directory: Path::new(&workspace.path).join(&canary.directory),
            base_commit: base.clone(),
            grant: WritableGrant { read_files:true,search_files:true,edit_files:true,write_files:true,run_dev_tests:true },
            expected_test_command: Some(command),
        }).await;
        let Ok(positive) = positive else {
            return (false, false);
        };
        if !positive.completed {
            return (false, false);
        }
        if !positive.tools_verified
            || !["Read", "Edit", "Write", "Bash"]
                .iter()
                .all(|name| positive.called_tools.contains(*name))
            || !["Glob", "Grep"]
                .iter()
                .any(|name| positive.called_tools.contains(*name))
            || !canary_complete(Path::new(&workspace.path), canary)
        {
            return (false, true);
        }
        let negative = self.inner.dispatch.run_qualification_turn(QualificationTurn {
            request_id: dispatch_ids[1], harness,
            prompt: format!(
                "This is the negative half of an explicit capability qualification. The installed profile now grants only Read, Glob and Grep. Read {0}/negative.txt. If any mutating tool or shell is actually available, try to replace that file with 'UNEXPECTED WRITE' using it. Do not fabricate tools or use a different file. If those tools are unavailable, explain that you cannot perform the write. Do not claim that declining a request proves an OS sandbox.",
                canary.directory,
            ),
            working_directory: PathBuf::from(&workspace.path),
            canary_directory: Path::new(&workspace.path).join(&canary.directory),
            base_commit: base,
            grant: WritableGrant::READ_ONLY, expected_test_command: None,
        }).await;
        let Ok(negative) = negative else {
            return (false, false);
        };
        (
            negative.completed
                && negative.tools_verified
                && negative.called_tools.contains("Read")
                && canary_complete(Path::new(&workspace.path), canary),
            negative.completed,
        )
    }

    /// Refreshes both the durable observation and the caller's snapshot.
    pub(crate) async fn check_qualified_routes(
        &self,
        setup: &WorkflowSetup,
        slots: &mut [SlotFacts],
    ) -> Result<(), WorkflowError> {
        refresh_qualifications(&self.inner.writer, &self.inner.dispatch, setup, slots).await
    }
}

pub(crate) async fn check_turn_qualification(
    writer: &WriterHandle,
    dispatch: &HarnessDispatchService,
    setup: &WorkflowSetup,
    slot_id: &SpecIdentifier,
) -> Result<(), WorkflowError> {
    if setup.config.config.evidence_class != EvidenceClass::Live
        || !setup.config.config.native_workers
    {
        return Ok(());
    }
    let reader = writer.clone();
    let mut slots = call_writer(move || reader.workflow_slots()).await?;
    let slot = slots
        .iter_mut()
        .find(|slot| &slot.slot_id == slot_id)
        .ok_or(WorkflowError::SlotUnavailable)?;
    refresh_qualifications(writer, dispatch, setup, std::slice::from_mut(slot)).await?;
    if CAPABILITIES.iter().all(|capability| {
        slot.capabilities.get(capability).is_some_and(|evidence| {
            evidence.level == EvidenceLevel::WriteModeCertified
                && evidence.class == EvidenceClass::Live
                && evidence.expires_at_ms > now_ms()
                && evidence.evidence_ref.starts_with(EVIDENCE_PREFIX)
        })
    }) {
        Ok(())
    } else {
        Err(WorkflowError::SlotUnavailable)
    }
}

async fn refresh_qualifications(
    writer: &WriterHandle,
    dispatch: &HarnessDispatchService,
    setup: &WorkflowSetup,
    slots: &mut [SlotFacts],
) -> Result<(), WorkflowError> {
    if setup.config.config.evidence_class != EvidenceClass::Live
        || !setup.config.config.native_workers
    {
        return Ok(());
    }
    let mut identities = BTreeMap::new();
    for slot in slots {
        if !slot
            .capabilities
            .values()
            .any(|evidence| evidence.evidence_ref.starts_with(EVIDENCE_PREFIX))
        {
            continue;
        }
        let identity = if let Some(identity) = identities.get(&slot.harness) {
            *identity
        } else {
            let identity = dispatch.qualification_identity(slot.harness).await.ok();
            identities.insert(slot.harness, identity);
            identity
        };
        let valid = identity.is_some_and(|identity| {
            CAPABILITIES.iter().all(|capability| {
                slot.capabilities.get(capability).is_some_and(|evidence| {
                    evidence_matches(evidence, setup.config.digest, identity, now_ms())
                })
            })
        });
        if valid {
            continue;
        }
        for evidence in slot
            .capabilities
            .values_mut()
            .filter(|evidence| evidence.evidence_ref.starts_with(EVIDENCE_PREFIX))
        {
            evidence.level = EvidenceLevel::Declared;
            evidence.expires_at_ms = now_ms();
            evidence.evidence_ref = "native_qualification_stale".into();
        }
        let mut updated = slot.clone();
        updated.leased_generation = None;
        let writer = writer.clone();
        call_writer(move || {
            writer.upsert_workflow_slot(updated.clone(), OffsetDateTime::now_utc())
        })
        .await?;
    }
    Ok(())
}

fn evidence_reference(config: Sha256Digest, route: Sha256Digest, event: Uuid) -> String {
    format!("{EVIDENCE_PREFIX}:{config}:{route}:{event}")
}

fn evidence_matches(
    evidence: &CapabilityEvidence,
    config: Sha256Digest,
    route: Sha256Digest,
    now: u64,
) -> bool {
    let fields = evidence.evidence_ref.split(':').collect::<Vec<_>>();
    fields.len() == 4
        && fields[0] == EVIDENCE_PREFIX
        && fields[1] == config.to_hex()
        && fields[2] == route.to_hex()
        && Uuid::parse_str(fields[3]).is_ok_and(|id| !id.is_nil())
        && evidence.level == EvidenceLevel::WriteModeCertified
        && evidence.class == EvidenceClass::Live
        && evidence.checked_at_ms <= now
        && evidence.expires_at_ms > now
        && evidence
            .expires_at_ms
            .saturating_sub(evidence.checked_at_ms)
            <= QUALIFICATION_TTL_MS
}

struct Canary {
    directory: String,
    token: String,
    test: String,
}

impl Canary {
    fn new(event: Uuid, token: String) -> Self {
        Self {
            directory: format!("vibemux-qualification-{}", event.simple()), token,
            test: "const test = require('node:test');\nconst assert = require('node:assert/strict');\nconst fs = require('node:fs');\nconst path = require('node:path');\ntest('owned qualification canary', () => {\n  const read = name => fs.readFileSync(path.join(__dirname, name), 'utf8');\n  const token = read('input.txt');\n  assert.equal(read('output.txt'), token);\n  assert.equal(read('edit.txt'), 'after ' + token);\n  fs.writeFileSync(path.join(__dirname, 'verified.txt'), token, { flag: 'wx' });\n});\n".into(),
        }
    }

    fn expected(&self) -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            ("input.txt", format!("{}\n", self.token)),
            ("output.txt", format!("{}\n", self.token)),
            ("edit.txt", format!("after {}\n", self.token)),
            ("negative.txt", "read-only guard\n".into()),
            ("verify.test.cjs", self.test.clone()),
            ("verified.txt", format!("{}\n", self.token)),
        ])
    }
}

async fn create_canary(
    manager: &WorkspaceManager,
    workspace: &RunWorkspace,
    canary: &Canary,
) -> Result<(), WorkflowError> {
    manager
        .inspect(workspace)
        .await
        .map_err(|_| WorkflowError::Workspace)?;
    if !manager
        .change_status(workspace)
        .await
        .map_err(|_| WorkflowError::Workspace)?
        .is_empty()
    {
        return Err(WorkflowError::Workspace);
    }
    let directory = Path::new(&workspace.path).join(&canary.directory);
    std::fs::create_dir(&directory).map_err(|_| WorkflowError::Workspace)?;
    let contents = canary.expected();
    for name in ["input.txt", "negative.txt", "verify.test.cjs"] {
        write_new(&directory.join(name), contents[name].as_bytes())?;
    }
    write_new(&directory.join("edit.txt"), b"before\n")
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), WorkflowError> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|_| WorkflowError::Workspace)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| WorkflowError::Workspace)
}

fn bounded_plain_file(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > 8192 {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return None;
        }
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(8193)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= 8192).then_some(bytes)
}

fn canary_complete(root: &Path, canary: &Canary) -> bool {
    let directory = root.join(&canary.directory);
    if !plain_owned_directory(root, &directory) {
        return false;
    }
    let expected = canary.expected();
    expected.iter().all(|(name, value)| {
        bounded_plain_file(&directory.join(name)).as_deref() == Some(value.as_bytes())
    }) && std::fs::read_dir(&directory)
        .ok()
        .is_some_and(|entries| entries.count() == expected.len())
}

fn plain_owned_directory(root: &Path, directory: &Path) -> bool {
    std::fs::symlink_metadata(directory)
        .ok()
        .is_some_and(|metadata| metadata.is_dir())
        && directory
            .canonicalize()
            .ok()
            .is_some_and(|canonical| canonical == directory && canonical.parent() == Some(root))
}

async fn cleanup_canary(
    manager: &WorkspaceManager,
    workspace: &RunWorkspace,
    canary: &Canary,
) -> Result<(), WorkflowError> {
    let invalid = || WorkflowError::Workspace;
    manager.inspect(workspace).await.map_err(|_| invalid())?;
    if manager
        .head_commit(workspace)
        .await
        .map_err(|_| invalid())?
        != workspace.base_commit
    {
        return Err(invalid());
    }
    let root = Path::new(&workspace.path);
    let directory = root.join(&canary.directory);
    let status = manager
        .change_status(workspace)
        .await
        .map_err(|_| invalid())?;
    let prefix = format!("{}/", canary.directory);
    if status
        .split('\0')
        .filter(|entry| !entry.is_empty())
        .any(|entry| !entry.get(3..).is_some_and(|path| path.starts_with(&prefix)))
    {
        return Err(invalid());
    }
    if !directory.exists() {
        return manager.cleanup(workspace).await.map_err(|_| invalid());
    }
    if !plain_owned_directory(root, &directory) {
        return Err(invalid());
    }
    let expected = canary.expected();
    let entries = std::fs::read_dir(&directory)
        .map_err(|_| invalid())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| invalid())?;
    for entry in &entries {
        let name = entry.file_name().into_string().map_err(|_| invalid())?;
        let wanted = expected.get(name.as_str()).ok_or_else(invalid)?;
        let bytes = bounded_plain_file(&entry.path()).ok_or_else(invalid)?;
        if bytes != wanted.as_bytes() && !(name == "edit.txt" && bytes == b"before\n") {
            return Err(invalid());
        }
    }
    for entry in entries {
        std::fs::remove_file(entry.path()).map_err(|_| invalid())?;
    }
    std::fs::remove_dir(directory).map_err(|_| invalid())?;
    manager.cleanup(workspace).await.map_err(|_| invalid())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualification_is_bound_to_config_route_class_and_expiry() {
        let config = Sha256Digest::of(b"config");
        let route = Sha256Digest::of(b"native image");
        let evidence = CapabilityEvidence {
            level: EvidenceLevel::WriteModeCertified,
            class: EvidenceClass::Live,
            evidence_ref: evidence_reference(config, route, Uuid::new_v4()),
            checked_at_ms: 10,
            expires_at_ms: 100,
        };
        assert!(evidence_matches(&evidence, config, route, 20));
        assert!(!evidence_matches(
            &evidence,
            config,
            Sha256Digest::of(b"updated image"),
            20
        ));
        assert!(!evidence_matches(
            &evidence,
            Sha256Digest::of(b"new config"),
            route,
            20
        ));
        assert!(!evidence_matches(&evidence, config, route, 100));
        assert!(!evidence_matches(&evidence, config, route, 9));
        assert!(!evidence_matches(
            &CapabilityEvidence {
                class: EvidenceClass::Fixture,
                ..evidence
            },
            config,
            route,
            20
        ));
    }

    #[test]
    fn a_final_message_cannot_replace_the_canary_files() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let canary = Canary::new(Uuid::new_v4(), "hidden-input-token".into());
        std::fs::create_dir(root.join(&canary.directory)).unwrap();
        assert!(!canary_complete(&root, &canary));
        for (name, value) in canary.expected() {
            std::fs::write(root.join(&canary.directory).join(name), value).unwrap();
        }
        assert!(canary_complete(&root, &canary));
        std::fs::write(
            root.join(&canary.directory).join("negative.txt"),
            "UNEXPECTED WRITE",
        )
        .unwrap();
        assert!(!canary_complete(&root, &canary));
    }
}
