//! Explicit live write-mode qualification through ordinary owned dispatches.
//! No slot is made eligible until the workflow layer validates the canaries.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde_json::Value;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use tokio::sync::oneshot;
use uuid::Uuid;
use vibemux_harness::{
    AgentKind,
    dispatch::{
        AttemptOutcome, DispatchError, DispatchLimits, DispatchRequest, NativeProtocol,
        ObservedRecord, ProtocolSession, Sha256Digest,
        capture_budget::CaptureBudget,
        request::DISPATCH_REQUEST_SCHEMA_VERSION,
        writable_profile::{CLAUDE_WRITABLE_PROFILE, WritableGrant},
    },
};
use vibemux_store::HarnessDispatchAdmission;
use vibemux_workflow::slots::EvidenceClass;

use super::{DispatchServiceError, HarnessDispatchService, call_writer, executor::ExecutionPlan};

pub(crate) struct QualificationTurn {
    pub request_id: Uuid,
    pub harness: AgentKind,
    pub prompt: String,
    pub working_directory: PathBuf,
    /// Created by the caller inside the owned worktree. Model writes may only
    /// create its output.txt or edit its edit.txt; proof inputs are immutable.
    pub canary_directory: PathBuf,
    pub base_commit: String,
    pub grant: WritableGrant,
    pub expected_test_command: Option<String>,
}

/// Contains only verified tool names, never tool arguments or output.
pub(crate) struct QualificationTurnEvidence {
    pub completed: bool,
    pub tools_verified: bool,
    pub called_tools: BTreeSet<String>,
}

impl HarnessDispatchService {
    /// Binds a certification to the actual installed native images and the
    /// pinned route. A CLI update invalidates evidence even at the same path.
    pub(crate) async fn qualification_identity(
        &self,
        harness: AgentKind,
    ) -> Result<Sha256Digest, DispatchServiceError> {
        let setup = self.inner.setup()?;
        if harness != AgentKind::Claude
            || self.execution_profile(harness, EvidenceClass::Live) != Some(CLAUDE_WRITABLE_PROFILE)
        {
            return Err(DispatchError::ExecutionDisabled.into());
        }
        let executable = setup.config.executable(harness)?.to_path_buf();
        let trampoline = setup
            .trampoline
            .clone()
            .ok_or(DispatchError::ExecutableUnavailable)?;
        let digest = setup.config.digest();
        tokio::task::spawn_blocking(move || -> Result<Sha256Digest, DispatchError> {
            let vendor = image_digest(&executable)?;
            let launcher = image_digest(&trampoline)?;
            Ok(Sha256Digest::of_fields(
                "vibemux.workflow.live_qualification_route.v1",
                &[
                    digest.as_bytes(),
                    vendor.as_bytes(),
                    launcher.as_bytes(),
                    CLAUDE_WRITABLE_PROFILE.as_bytes(),
                ],
            ))
        })
        .await
        .map_err(|_| DispatchError::Internal)?
        .map_err(Into::into)
    }

    /// Private qualification entry: its caller creates and verifies the
    /// independent owned worktree. Unlike a workflow turn, admission does not
    /// require the capability that this bounded canary is trying to prove.
    /// The canonical dispatch reservation still provides replay suppression,
    /// cancellation, fenced completion, and joined child ownership.
    pub(crate) async fn run_qualification_turn(
        &self,
        turn: QualificationTurn,
    ) -> Result<QualificationTurnEvidence, DispatchServiceError> {
        let setup = self.inner.setup()?;
        self.inner.ensure_accepting()?;
        if turn.harness != AgentKind::Claude {
            return Err(DispatchError::ExecutionDisabled.into());
        }
        let request = DispatchRequest {
            schema_version: DISPATCH_REQUEST_SCHEMA_VERSION,
            request_id: turn.request_id,
            harness: turn.harness,
            prompt: turn.prompt,
        };
        request.validate()?;
        let detected = self.detected(turn.harness).await?;
        let config = setup.config.config();
        let route = config.execution_route(&request, detected)?;
        if route.protocol != NativeProtocol::ClaudeStreamJson {
            return Err(DispatchError::ExecutionDisabled.into());
        }
        let inner = Arc::clone(&self.inner);
        let launch_route = route.clone();
        let directory = turn.working_directory.clone();
        let grant = turn.grant;
        let prepared = tokio::task::spawn_blocking(move || {
            inner
                .setup()?
                .workflow_launch(&launch_route, grant, EvidenceClass::Live, &directory)
        })
        .await
        .map_err(|_| DispatchError::Internal)??;
        let session = ProtocolSession::for_execution(
            route,
            request.prompt.clone(),
            prepared.working_directory,
        )?;
        let limits = DispatchLimits {
            deadline_ms: config.limits.deadline_ms.min(45_000),
            ..config.limits
        };
        let budget = CaptureBudget::new(route.protocol, &limits)?;
        let admission = HarnessDispatchAdmission {
            request_id: request.request_id,
            harness: request.harness,
            protocol: route.protocol,
            prompt: request.prompt_digest(),
            config_sha256: setup.config.digest(),
            resource_key: prepared.resource_key,
            base_commit: turn.base_commit,
            timestamp: OffsetDateTime::now_utc(),
        };
        let writer = self.inner.writer.clone();
        let commit = call_writer(move || writer.admit_harness_dispatch(admission.clone())).await?;
        // An old admitted/started/terminal attempt is observed, never launched
        // again. A fresh explicit event is required for another qualification.
        if commit.event.is_none() {
            return Ok(QualificationTurnEvidence {
                completed: false,
                tools_verified: false,
                called_tools: BTreeSet::new(),
            });
        }
        let plan = ExecutionPlan {
            launch: prepared.launch,
            session,
            budget,
            limits,
        };
        let (done, finished) = oneshot::channel();
        if !self.launch_workflow_turn(request.request_id, plan, route.protocol, done) {
            let writer = self.inner.writer.clone();
            let request_id = request.request_id;
            call_writer(move || {
                writer.cancel_harness_dispatch(request_id, OffsetDateTime::now_utc())
            })
            .await?;
            return Err(DispatchError::Internal.into());
        }
        let _ = finished.await.map_err(|_| DispatchError::Internal)?;
        let record = self.status(request.request_id).await?;
        let transcript = self
            .transcript(request.request_id, 0, limits.record_count)
            .await?;
        let tools = verify_tools(
            &transcript.records,
            grant,
            turn.expected_test_command.as_deref(),
            Some((&turn.working_directory, &turn.canary_directory)),
        );
        Ok(QualificationTurnEvidence {
            completed: transcript.complete && record.outcome == Some(AttemptOutcome::Completed),
            tools_verified: tools.is_some(),
            called_tools: tools.unwrap_or_default(),
        })
    }
}

fn image_digest(path: &std::path::Path) -> Result<Sha256Digest, DispatchError> {
    let mut file = File::open(path).map_err(|_| DispatchError::ExecutableUnavailable)?;
    let before = file
        .metadata()
        .map_err(|_| DispatchError::ExecutableUnavailable)?;
    if !before.is_file() || before.len() > 512 * 1024 * 1024 {
        return Err(DispatchError::ExecutableUnavailable);
    }
    let mut hash = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| DispatchError::ExecutableUnavailable)?;
        if read == 0 {
            break;
        }
        bytes = bytes.saturating_add(read as u64);
        if bytes > 512 * 1024 * 1024 {
            return Err(DispatchError::ExecutableUnavailable);
        }
        hash.update(&buffer[..read]);
    }
    let after = file
        .metadata()
        .map_err(|_| DispatchError::ExecutableUnavailable)?;
    if bytes != before.len()
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
    {
        return Err(DispatchError::ExecutableUnavailable);
    }
    let hex = hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Sha256Digest::from_hex(&hex).ok_or(DispatchError::Internal)
}

fn verify_tools(
    records: &[ObservedRecord],
    grant: WritableGrant,
    expected_test_command: Option<&str>,
    write_scope: Option<(&Path, &Path)>,
) -> Option<BTreeSet<String>> {
    let required: BTreeSet<&str> = [
        (grant.read_files, "Read"),
        (grant.search_files, "Glob"),
        (grant.search_files, "Grep"),
        (grant.edit_files, "Edit"),
        (grant.write_files, "Write"),
        (grant.run_dev_tests, "Bash"),
    ]
    .into_iter()
    .filter_map(|(enabled, name)| enabled.then_some(name))
    .collect();
    // Current Claude Code may retain its session-ending tool whenever other
    // tools exist. It adds no filesystem, command, network or delegation grant.
    let mut allowed = required.clone();
    allowed.insert("EndConversation");
    let mut inventory_seen = false;
    let mut called = BTreeSet::new();
    let mut pending = BTreeMap::new();
    let mut seen_ids = BTreeSet::new();
    for record in records {
        let value: Value = serde_json::from_str(record.raw_json()).ok()?;
        if value["type"] == "system" && value["subtype"] == "init" {
            if inventory_seen {
                return None;
            }
            let names: BTreeSet<&str> = value["tools"]
                .as_array()?
                .iter()
                .map(Value::as_str)
                .collect::<Option<_>>()?;
            if !required.is_subset(&names) || !names.is_subset(&allowed) {
                return None;
            }
            inventory_seen = true;
        }
        if value["type"] == "control_request" {
            return None;
        }
        if value["type"] == "user" {
            for result in value["message"]["content"].as_array().into_iter().flatten() {
                if result["type"] != "tool_result" {
                    continue;
                }
                let id = result["tool_use_id"].as_str()?;
                let name = pending.remove(id)?;
                // A tool request is not evidence of execution. Require the
                // correlated native result, with no error, exactly once.
                if !matches!(result.get("is_error"), None | Some(Value::Bool(false)))
                    || !result
                        .get("content")
                        .is_some_and(|content| content.is_string() || content.is_array())
                {
                    return None;
                }
                called.insert(name);
            }
        }
        let mut blocks = Vec::new();
        if value["type"] == "assistant" {
            if value
                .get("parent_tool_use_id")
                .is_some_and(|id| !id.is_null())
            {
                return None;
            }
            blocks.extend(value["message"]["content"].as_array()?.iter());
        }
        if value["type"] == "stream_event" && value["event"]["type"] == "content_block_start" {
            let block = &value["event"]["content_block"];
            // A streaming tool block initially has empty arguments. Its
            // complete assistant message below is the command authority.
            if block["type"] == "tool_use" && !allowed.contains(block["name"].as_str()?) {
                return None;
            }
        }
        for block in blocks {
            if block["type"] != "tool_use" {
                continue;
            }
            let name = block["name"].as_str()?;
            if !allowed.contains(name) {
                return None;
            }
            if name == "Bash" && block["input"]["command"].as_str() != expected_test_command {
                return None;
            }
            if matches!(name, "Write" | "Edit") {
                let (root, canary) = write_scope?;
                if !allowed_canary_write(name, &block["input"], root, canary) {
                    return None;
                }
            }
            if name == "EndConversation" {
                continue;
            }
            let id = block["id"]
                .as_str()
                .filter(|id| !id.is_empty() && id.len() <= 256)?;
            if !seen_ids.insert(id.to_owned()) {
                return None;
            }
            pending.insert(id.to_owned(), name.to_owned());
        }
    }
    (inventory_seen && pending.is_empty()).then_some(called)
}

/// Qualification evidence must not be self-authored. A successful Write to the
/// proof marker or test script would make a later test success meaningless.
/// Check the actual native path argument as well as the final canary bytes.
/// This is post-execution evidence validation, not an OS filesystem sandbox.
fn allowed_canary_write(name: &str, input: &Value, root: &Path, canary: &Path) -> bool {
    let expected_name = match name {
        "Write" => "output.txt",
        "Edit" => "edit.txt",
        _ => return false,
    };
    let Some(raw) = input["file_path"]
        .as_str()
        .filter(|path| !path.is_empty() && path.len() <= 8192 && !path.contains('\0'))
    else {
        return false;
    };
    if root.canonicalize().ok().as_deref() != Some(root)
        || canary.parent() != Some(root)
        || canary.canonicalize().ok().as_deref() != Some(canary)
        || !std::fs::symlink_metadata(canary)
            .ok()
            .is_some_and(|metadata| metadata.is_dir())
    {
        return false;
    }
    let path = Path::new(raw);
    let target = if path.is_absolute() {
        path.to_path_buf()
    } else {
        // A Windows drive-relative path resolves outside the explicit cwd
        // contract. Ordinary relative paths are resolved only against root.
        if path
            .components()
            .any(|part| matches!(part, std::path::Component::Prefix(_)))
        {
            return false;
        }
        root.join(path)
    };
    std::fs::symlink_metadata(&target)
        .ok()
        .is_some_and(|metadata| metadata.is_file())
        && target.canonicalize().ok().as_deref() == Some(canary.join(expected_name).as_path())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn records(values: &[Value]) -> Vec<ObservedRecord> {
        values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                ObservedRecord::parse(
                    NativeProtocol::ClaudeStreamJson,
                    index as u64 + 1,
                    value.to_string(),
                )
                .unwrap()
                .0
            })
            .collect()
    }

    #[test]
    fn negative_probe_requires_actual_restricted_tool_inventory() {
        let init = json!({"type":"system","subtype":"init","tools":["Read","Glob","Grep"]});
        assert!(
            verify_tools(
                &records(std::slice::from_ref(&init)),
                WritableGrant::READ_ONLY,
                None,
                None
            )
            .is_some()
        );
        assert!(verify_tools(&records(&[]), WritableGrant::READ_ONLY, None, None).is_none());
        let bad = json!({"type":"system","subtype":"init","tools":["Read","Glob","Grep","Write"]});
        assert!(verify_tools(&records(&[bad]), WritableGrant::READ_ONLY, None, None).is_none());
        let write = json!({"type":"assistant","message":{"content":[{"type":"tool_use","name":"Write","input":{"file_path":"x","content":"y"}}]}});
        assert!(
            verify_tools(
                &records(&[init, write]),
                WritableGrant::READ_ONLY,
                None,
                None
            )
            .is_none()
        );
    }

    #[test]
    fn certification_never_counts_an_arbitrary_shell_command_as_a_dev_test() {
        let grant = WritableGrant {
            run_dev_tests: true,
            ..WritableGrant::READ_ONLY
        };
        let init = json!({"type":"system","subtype":"init","tools":["Read","Glob","Grep","Bash"]});
        let shell = |command: &str| json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"call-1","name":"Bash","input":{"command":command}}]}});
        let success = json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"call-1","is_error":false,"content":"test passed"}]}});
        assert!(
            verify_tools(
                &records(&[init.clone(), shell("node --test canary.test.cjs"), success]),
                grant,
                Some("node --test canary.test.cjs"),
                None,
            )
            .is_some()
        );
        assert!(
            verify_tools(
                &records(&[init, shell("node --test canary.test.cjs && echo escaped")]),
                grant,
                Some("node --test canary.test.cjs"),
                None,
            )
            .is_none()
        );
    }

    #[test]
    fn failed_missing_or_uncorrelated_tool_results_never_certify_execution() {
        let grant = WritableGrant {
            run_dev_tests: true,
            ..WritableGrant::READ_ONLY
        };
        let init = json!({"type":"system","subtype":"init","tools":["Read","Glob","Grep","Bash"]});
        let call = json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"test-1","name":"Bash","input":{"command":"node --test canary.test.cjs"}}]}});
        let result = |id: &str, error: bool| json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":id,"is_error":error,"content":"test output"}]}});
        let verify = |values: &[Value]| {
            verify_tools(
                &records(values),
                grant,
                Some("node --test canary.test.cjs"),
                None,
            )
        };
        assert!(verify(&[init.clone(), call.clone()]).is_none());
        assert!(verify(&[init.clone(), call.clone(), result("test-1", true)]).is_none());
        assert!(verify(&[init.clone(), call.clone(), result("other-call", false)]).is_none());
        assert!(
            verify(&[
                init.clone(),
                call.clone(),
                result("test-1", false),
                result("test-1", false)
            ])
            .is_none()
        );
        assert!(verify(&[init, call, result("test-1", false)]).is_some());
    }

    fn write_records(name: &str, target: &str) -> Vec<ObservedRecord> {
        records(&[
            json!({"type":"system","subtype":"init","tools":["Write","Edit"]}),
            json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"write-1","name":name,"input":{"file_path":target,"content":"synthetic","old_string":"before","new_string":"after"}}]}}),
            json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"write-1","is_error":false,"content":"updated"}]}}),
        ])
    }

    #[test]
    fn canary_write_grants_are_exact_even_with_successful_tool_results() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let canary = root.join("canary");
        std::fs::create_dir(&canary).unwrap();
        for name in [
            "output.txt",
            "edit.txt",
            "input.txt",
            "negative.txt",
            "verify.test.cjs",
            "verified.txt",
        ] {
            std::fs::write(canary.join(name), "synthetic").unwrap();
        }
        let grant = WritableGrant {
            write_files: true,
            edit_files: true,
            ..WritableGrant::NONE
        };
        let verify = |name: &str, target: &str| {
            verify_tools(
                &write_records(name, target),
                grant,
                None,
                Some((&root, &canary)),
            )
        };
        assert!(verify("Write", "canary/output.txt").is_some());
        assert!(verify("Write", "./canary/./output.txt").is_some());
        assert!(verify("Edit", "canary/edit.txt").is_some());
        assert!(verify("Write", canary.join("output.txt").to_str().unwrap()).is_some());
        assert!(verify("Edit", canary.join("edit.txt").to_str().unwrap()).is_some());
        assert!(verify("Write", "canary/edit.txt").is_none());
        assert!(verify("Edit", "canary/output.txt").is_none());
        for protected in [
            "input.txt",
            "negative.txt",
            "verify.test.cjs",
            "verified.txt",
        ] {
            for tool in ["Write", "Edit"] {
                assert!(verify(tool, &format!("canary/{protected}")).is_none());
                assert!(verify(tool, canary.join(protected).to_str().unwrap()).is_none());
            }
        }
        assert!(
            verify_tools(
                &write_records("Write", "canary/output.txt"),
                grant,
                None,
                None
            )
            .is_none()
        );
    }

    #[test]
    fn canary_write_targets_reject_escape_prefix_traps_missing_and_symlinked_files() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let canary = root.join("canary");
        let other = root.join("canary-other");
        std::fs::create_dir(&canary).unwrap();
        std::fs::create_dir(&other).unwrap();
        std::fs::write(root.join("output.txt"), "external").unwrap();
        std::fs::write(other.join("output.txt"), "external").unwrap();
        let allowed = |target: &str| {
            allowed_canary_write("Write", &json!({"file_path":target}), &root, &canary)
        };
        assert!(
            !allowed("canary/output.txt"),
            "a missing result file is not successful write evidence"
        );
        assert!(!allowed("canary/../output.txt"));
        assert!(!allowed("canary-other/output.txt"));
        assert!(!allowed(other.join("output.txt").to_str().unwrap()));
        assert!(!allowed("canary/output.txt\0ignored"));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("output.txt"), canary.join("output.txt")).unwrap();
            assert!(!allowed("canary/output.txt"));
            std::fs::remove_file(canary.join("output.txt")).unwrap();
            std::os::unix::fs::symlink(&other, root.join("redirected")).unwrap();
            assert!(!allowed("redirected/output.txt"));
        }
        #[cfg(windows)]
        {
            assert!(!allowed(r"C:canary\output.txt"));
            assert!(!allowed(r"Z:\outside\output.txt"));
        }
    }
}
