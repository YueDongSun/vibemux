use super::*;
use crate::chat_workspace::{
    inspect_owner_workspace,
    tests::{OwnedFixture, fixture_git},
};
use std::collections::{BTreeMap, BTreeSet};
use time::OffsetDateTime;
use uuid::Uuid;
use vibemux_harness::HarnessDetection;
use vibemux_store::{
    ChatExecutionOwnerOpen, ChatExecutionProcessChange, ChatExecutionStart, ChatMutation,
    ChatOwnerChange,
};
use vibemux_types::{Run, RunSpec, Task, TaskSpec, a2a::A2aRunStart, chat::ChatHarness};

fn query(fixture: &OwnedFixture, mode: WorkspaceReadMode) -> WorkspaceReadQuery {
    WorkspaceReadQuery {
        project_id: fixture.owner.project_id,
        task_id: fixture.owner.task_id,
        run_id: fixture.owner.run_id,
        mode,
    }
}

async fn events(fixture: &OwnedFixture) -> serde_json::Value {
    let writer = fixture.writer.clone();
    let events = tokio::task::spawn_blocking(move || writer.with_store(|store| store.events()))
        .await
        .expect("event task")
        .expect("authoritative events");
    serde_json::to_value(events).expect("canonical event snapshot")
}

#[tokio::test]
async fn metadata_and_directory_are_exact_bounded_reads_without_canonical_events() {
    let fixture = OwnedFixture::new().await;
    let before = events(&fixture).await;
    let metadata = read_workspace(
        fixture.writer.clone(),
        &fixture.root,
        query(&fixture, WorkspaceReadMode::Metadata),
    )
    .await;
    let directory = read_workspace(
        fixture.writer.clone(),
        &fixture.root,
        query(&fixture, WorkspaceReadMode::Directory),
    )
    .await;
    let missing_root = fixture.root.join("never_created_by_metadata");
    let metadata_without_filesystem = read_workspace(
        fixture.writer.clone(),
        &missing_root,
        query(&fixture, WorkspaceReadMode::Metadata),
    )
    .await;
    let after = events(&fixture).await;
    let expected_branch = fixture
        .owner
        .workspace
        .as_ref()
        .expect("recorded workspace")
        .branch
        .clone();
    let expected_path = child_path_text(&fixture.worktree()).expect("canonical child path");
    let expected_key = fixture
        .owner
        .resource_key
        .clone()
        .expect("recorded logical key");
    assert!(!missing_root.exists());
    fixture.finish().await;
    let metadata = metadata.expect("metadata");
    assert_eq!(metadata.branch.as_deref(), Some(expected_branch.as_str()));
    assert!(metadata.directory.is_none());
    assert_eq!(
        metadata_without_filesystem.expect("metadata needs no filesystem"),
        metadata
    );
    let directory = directory.expect("managed directory");
    assert_eq!(directory.branch, metadata.branch);
    assert_eq!(
        directory.directory,
        Some(WorkspaceDirectory {
            path: expected_path,
            resource_key: expected_key
        })
    );
    assert_eq!(after, before);
}

#[tokio::test]
async fn foreign_unknown_and_unplanned_runs_do_not_gain_directory_authority() {
    let fixture = OwnedFixture::new().await;
    let mut foreign_project = query(&fixture, WorkspaceReadMode::Directory);
    foreign_project.project_id = ProjectId::new();
    let mut foreign_task = query(&fixture, WorkspaceReadMode::Directory);
    foreign_task.task_id = TaskId::new();
    let mut unknown = query(&fixture, WorkspaceReadMode::Metadata);
    unknown.run_id = RunId::new();
    let foreign_project =
        read_workspace(fixture.writer.clone(), &fixture.root, foreign_project).await;
    let foreign_task = read_workspace(fixture.writer.clone(), &fixture.root, foreign_task).await;
    let unknown = read_workspace(fixture.writer.clone(), &fixture.root, unknown).await;
    let writer = fixture.writer.clone();
    let base_commit = fixture.base.clone();
    let unplanned = tokio::task::spawn_blocking(move || {
        writer.with_store(move |store| {
            store.open_chat_execution_owner(&ChatExecutionOwnerOpen {
                operation_id: Uuid::new_v4(),
                harness: ChatHarness::Codex,
                profile_sha256: "a".repeat(64),
                base_commit,
                admitted_tools: BTreeSet::new(),
                title_ref: None,
                timestamp: OffsetDateTime::now_utc(),
            })
        })
    })
    .await
    .expect("allocation task")
    .expect("unplanned owner")
    .owner;
    let unplanned_query = WorkspaceReadQuery {
        project_id: unplanned.project_id,
        task_id: unplanned.task_id,
        run_id: unplanned.run_id,
        mode: WorkspaceReadMode::Metadata,
    };
    let metadata = read_workspace(
        fixture.writer.clone(),
        &fixture.root,
        unplanned_query.clone(),
    )
    .await;
    let directory = read_workspace(
        fixture.writer.clone(),
        &fixture.root,
        WorkspaceReadQuery {
            mode: WorkspaceReadMode::Directory,
            ..unplanned_query
        },
    )
    .await;
    fixture.finish().await;
    assert_eq!(foreign_project, Err("workspace_scope_mismatch"));
    assert_eq!(foreign_task, Err("workspace_scope_mismatch"));
    assert_eq!(unknown, Err("workspace_run_unavailable"));
    assert!(metadata.expect("unplanned metadata").branch.is_none());
    assert_eq!(directory, Err("workspace_directory_unavailable"));
}

#[tokio::test]
async fn finalized_retained_workspaces_are_presented_without_reopening_the_owner() {
    let mut fixture = OwnedFixture::new().await;
    let old = load_authority(
        fixture.writer.clone(),
        &query(&fixture, WorkspaceReadMode::Directory),
    )
    .await
    .expect("initial authority");
    fixture.change(ChatOwnerChange::Stop).await;
    let before = events(&fixture).await;
    let owner = fixture.owner.clone();
    let path = child_path_text(&fixture.worktree()).expect("path");
    let presented = read_workspace(
        fixture.writer.clone(),
        &fixture.root,
        query(&fixture, WorkspaceReadMode::Directory),
    )
    .await;
    let terminal = inspect_owner_workspace(&fixture.root, &fixture.owner).await;
    let current = load_authority(
        fixture.writer.clone(),
        &query(&fixture, WorkspaceReadMode::Directory),
    )
    .await
    .expect("current finalized authority");
    let same_immutable = old.same_immutable_binding(&current);
    let after = events(&fixture).await;
    fixture.finish().await;
    assert_eq!(owner.phase, ChatOwnerPhase::Finalized);
    assert_eq!(
        presented
            .expect("retained presentation")
            .directory
            .expect("directory")
            .path,
        path
    );
    assert!(
        terminal.is_err(),
        "the existing terminal policy stays Open-only"
    );
    assert!(
        same_immutable,
        "unrelated version, state and timestamps do not change physical binding"
    );
    assert_eq!(after, before);
}

#[tokio::test]
async fn cleanup_reservation_and_removed_tombstone_block_directory_but_preserve_branch_metadata() {
    use vibemux_store::{
        WorkspaceCleanupConfirmation, WorkspaceCleanupMutation, WorkspaceCleanupPlan,
        WorkspaceCleanupSettlement,
    };
    use vibemux_types::workspace_cleanup::{CleanupObjectIdentity, WorkspaceCleanupBinding};
    let mut fixture = OwnedFixture::new().await;
    fixture.change(ChatOwnerChange::Stop).await;
    let writer = fixture.writer.clone();
    let chat_id = fixture.owner.chat_id;
    let root = child_path_text(&fixture.root).expect("fixture root");
    let common = child_path_text(&fixture.root.join(".git")).expect("common dir");
    // Synthetic private cleanup observations exercise only the presentation
    // contract. This test performs no cleanup executor or observed-removal check.
    let input = tokio::task::spawn_blocking(move || {
        writer.with_store(move |store| {
            let owner = store.chat_execution_owner_state(chat_id)?.ok_or(
                vibemux_store::StoreError::WorkspaceCleanup("fixture_owner_missing"),
            )?;
            let identity = CleanupObjectIdentity::WindowsFileId128V1 {
                volume_serial_number: 1,
                file_id: [1; 16],
            };
            Ok(WorkspaceCleanupPlan {
                operation_id: Uuid::new_v4(),
                daemon_incarnation: Uuid::new_v4(),
                timestamp: OffsetDateTime::now_utc(),
                binding: WorkspaceCleanupBinding {
                    project_id: owner.owner.project_id,
                    chat_id,
                    task_id: owner.owner.task_id,
                    run_id: owner.owner.run_id,
                    owner_version: owner.owner.version,
                    session_generation: owner.session.generation,
                    session_version: owner.session.version,
                    workspace: owner.owner.workspace.ok_or(
                        vibemux_store::StoreError::WorkspaceCleanup("fixture_workspace_missing"),
                    )?,
                    resource_key: owner.owner.resource_key.ok_or(
                        vibemux_store::StoreError::WorkspaceCleanup("fixture_resource_missing"),
                    )?,
                    repository_root: root,
                    common_git_dir: common,
                    repository_identity: identity.clone(),
                    common_directory_identity: identity.clone(),
                    worktree_identity: identity,
                    head_commit: owner.owner.base_commit,
                    terminal_plugin_id: "synthetic".into(),
                    session_id: "synthetic".into(),
                    instance_id: "synthetic".into(),
                },
            })
        })
    })
    .await
    .expect("task")
    .expect("input");
    let writer = fixture.writer.clone();
    let incarnation = input.daemon_incarnation;
    let preview = tokio::task::spawn_blocking(move || {
        writer.with_store(move |store| store.plan_workspace_cleanup(&input))
    })
    .await
    .expect("task")
    .expect("plan");
    let before_claim = read_workspace(
        fixture.writer.clone(),
        &fixture.root,
        query(&fixture, WorkspaceReadMode::Directory),
    )
    .await;
    let prior_authority = load_authority(
        fixture.writer.clone(),
        &query(&fixture, WorkspaceReadMode::Directory),
    )
    .await
    .expect("pre-claim authority");
    let writer = fixture.writer.clone();
    let mut claimed = tokio::task::spawn_blocking(move || {
        writer.with_store(move |store| {
            store.confirm_workspace_cleanup(&WorkspaceCleanupConfirmation {
                operation_id: Uuid::new_v4(),
                cleanup_id: preview.cleanup.id,
                expected_version: preview.cleanup.version,
                daemon_incarnation: incarnation,
                timestamp: OffsetDateTime::now_utc(),
            })
        })
    })
    .await
    .expect("task")
    .expect("claim");
    let during = read_workspace(
        fixture.writer.clone(),
        &fixture.root,
        query(&fixture, WorkspaceReadMode::Directory),
    )
    .await;
    let late_claim = revalidate_after_inspection(
        fixture.writer.clone(),
        &query(&fixture, WorkspaceReadMode::Directory),
        &prior_authority,
    )
    .await;
    let metadata = read_workspace(
        fixture.writer.clone(),
        &fixture.root,
        query(&fixture, WorkspaceReadMode::Metadata),
    )
    .await;
    let proof = claimed
        .execution_claim
        .take()
        .expect("claim")
        .retire_without_spawn();
    let writer = fixture.writer.clone();
    let record = claimed.cleanup;
    let quiet = tokio::task::spawn_blocking(move || {
        writer.with_store(move |store| {
            Ok(store.record_workspace_cleanup_never_spawned(
                &WorkspaceCleanupMutation {
                    operation_id: Uuid::new_v4(),
                    cleanup_id: record.id,
                    expected_version: record.version,
                    timestamp: OffsetDateTime::now_utc(),
                },
                proof,
            ))
        })
    })
    .await
    .expect("task")
    .expect("writer")
    .expect("retired");
    let writer = fixture.writer.clone();
    let record = quiet.cleanup;
    tokio::task::spawn_blocking(move || {
        writer.with_store(move |store| {
            store.settle_workspace_cleanup(
                &WorkspaceCleanupMutation {
                    operation_id: Uuid::new_v4(),
                    cleanup_id: record.id,
                    expected_version: record.version,
                    timestamp: OffsetDateTime::now_utc(),
                },
                &WorkspaceCleanupSettlement::RemovedObserved,
            )
        })
    })
    .await
    .expect("task")
    .expect("synthetic tombstone");
    let after = read_workspace(
        fixture.writer.clone(),
        &fixture.root,
        query(&fixture, WorkspaceReadMode::Directory),
    )
    .await;
    let after_metadata = read_workspace(
        fixture.writer.clone(),
        &fixture.root,
        query(&fixture, WorkspaceReadMode::Metadata),
    )
    .await;
    let before = events(&fixture).await;
    let still_present = fixture.worktree().exists();
    let after_reads = events(&fixture).await;
    fixture.finish().await;
    assert!(before_claim.is_ok(), "nonreserving preview");
    assert_eq!(during, Err("workspace_directory_unavailable"));
    assert_eq!(late_claim, Err("workspace_directory_unavailable"));
    assert_eq!(after, Err("workspace_directory_unavailable"));
    assert_eq!(
        metadata.expect("branch metadata"),
        after_metadata.expect("retained branch")
    );
    assert!(
        still_present,
        "synthetic verdict did not remove this fixture"
    );
    assert_eq!(before, after_reads);
}

#[tokio::test]
async fn dirty_files_and_advanced_owned_head_remain_presentable() {
    let fixture = OwnedFixture::new().await;
    let dirty_file = fixture.worktree().join("workspace_access_owned_dirty.txt");
    std::fs::write(&dirty_file, "owned dirty data\n").expect("fixture-owned data");
    let dirty = read_workspace(
        fixture.writer.clone(),
        &fixture.root,
        query(&fixture, WorkspaceReadMode::Directory),
    )
    .await;
    std::fs::remove_file(&dirty_file).expect("remove exact test-created file");
    let advanced = fixture.advance_head().await;
    let read = read_workspace(
        fixture.writer.clone(),
        &fixture.root,
        query(&fixture, WorkspaceReadMode::Directory),
    )
    .await;
    let base = fixture.base.clone();
    fixture.finish().await;
    assert!(dirty.is_ok());
    assert!(read.is_ok());
    assert_ne!(advanced, base);
}

#[tokio::test]
async fn missing_namespace_and_receipt_refuse_directory_without_recreation_or_metadata_loss() {
    let fixture = OwnedFixture::new().await;
    let managed = fixture
        .worktree()
        .parent()
        .expect("managed parent")
        .to_path_buf();
    let held = fixture.state_dir.join("held_workspace_access_namespace");
    let canonical_root = std::fs::canonicalize(&fixture.root).expect("root");
    let canonical_managed = std::fs::canonicalize(&managed).expect("managed");
    let canonical_state = std::fs::canonicalize(&fixture.state_dir).expect("state");
    assert!(canonical_managed.strip_prefix(&canonical_root).is_ok());
    assert_eq!(canonical_managed.parent(), Some(canonical_state.as_path()));
    assert_eq!(managed.parent(), Some(fixture.state_dir.as_path()));
    assert_eq!(held.parent(), Some(fixture.state_dir.as_path()));
    assert!(!held.exists());
    std::fs::rename(&managed, &held).expect("move only owned fixture namespace");
    let missing = read_workspace(
        fixture.writer.clone(),
        &fixture.root,
        query(&fixture, WorkspaceReadMode::Directory),
    )
    .await;
    let metadata = read_workspace(
        fixture.writer.clone(),
        &fixture.root,
        query(&fixture, WorkspaceReadMode::Metadata),
    )
    .await;
    let recreated = managed.exists();
    if recreated {
        std::fs::remove_dir(&managed).expect("remove only newly created empty test namespace");
    }
    std::fs::rename(&held, &managed).expect("restore exact owned namespace");
    let git_dir = fixture_git(
        &fixture.git,
        &fixture.worktree(),
        &["rev-parse", "--absolute-git-dir"],
    )
    .await;
    let receipt = std::path::PathBuf::from(git_dir.trim()).join("vibemux_owner.json");
    let bytes = std::fs::read(&receipt).expect("owned receipt");
    std::fs::remove_file(&receipt).expect("inject missing owned receipt");
    let no_receipt = read_workspace(
        fixture.writer.clone(),
        &fixture.root,
        query(&fixture, WorkspaceReadMode::Directory),
    )
    .await;
    std::fs::write(&receipt, bytes).expect("restore exact receipt");
    fixture.finish().await;
    assert_eq!(missing, Err("workspace_directory_unavailable"));
    assert!(!recreated);
    assert!(
        metadata
            .expect("recorded metadata survives missing directory")
            .branch
            .is_some()
    );
    assert_eq!(no_receipt, Err("workspace_directory_unavailable"));
}

#[tokio::test]
async fn current_session_recovery_refuses_presentation_after_physical_inspection() {
    let fixture = OwnedFixture::new().await;
    let request = query(&fixture, WorkspaceReadMode::Directory);
    let stored = load_authority(fixture.writer.clone(), &request)
        .await
        .expect("full owner");
    let Authority::Owner(stored) = stored else {
        panic!("owner");
    };
    let owner = stored.owner.clone();
    let generation = stored.session.generation;
    let writer = fixture.writer.clone();
    let started = tokio::task::spawn_blocking(move || {
        writer.with_store(move |store| {
            store.commit_harness_snapshot(
                &BTreeMap::from([(
                    "codex".into(),
                    HarnessDetection {
                        detected: true,
                        path: None,
                        launcher: None,
                        version: Some("fixture".into()),
                    },
                )]),
                "2026-11-02T00:00:00Z",
                "workspace_access_fixture_detection",
            )?;
            store.start_chat_execution_process(&ChatExecutionStart {
                operation_id: Uuid::new_v4(),
                chat_id: owner.chat_id,
                expected_generation: generation,
                expected_owner_version: owner.version,
                profile_sha256: owner.profile_sha256,
                resource_key: owner.resource_key.expect("owned logical key"),
                observed_commit: owner.base_commit,
                expected_vendor_id: None,
                timestamp: OffsetDateTime::now_utc(),
            })
        })
    })
    .await
    .expect("claim task")
    .expect("logical fixture fence; no process launched");
    let fence = started.fence.expect("fresh logical fence");
    let initial = load_authority(fixture.writer.clone(), &request)
        .await
        .expect("pre-inspection authority");
    inspect_presented_owner_workspace(&fixture.root, &fixture.owner)
        .await
        .expect("physical inspection");
    let writer = fixture.writer.clone();
    let chat_id = fixture.owner.chat_id;
    let generation = started.session.generation;
    let recovered = tokio::task::spawn_blocking(move || {
        writer.with_store(move |store| {
            store.change_chat_execution_process(
                &ChatMutation {
                    operation_id: Uuid::new_v4(),
                    chat_id,
                    expected_generation: generation,
                    timestamp: OffsetDateTime::now_utc(),
                },
                fence,
                &ChatExecutionProcessChange::Released {
                    cleanup_verified: false,
                    closed: false,
                },
            )
        })
    })
    .await
    .expect("recovery task")
    .expect("canonical session recovery");
    let refreshed = revalidate_after_inspection(fixture.writer.clone(), &request, &initial).await;
    let refused = read_workspace(fixture.writer.clone(), &fixture.root, request).await;
    let writer = fixture.writer.clone();
    let cleaned = tokio::task::spawn_blocking(move || {
        writer.with_store(move |store| {
            store.change_chat_execution_process(
                &ChatMutation {
                    operation_id: Uuid::new_v4(),
                    chat_id,
                    expected_generation: generation,
                    timestamp: OffsetDateTime::now_utc(),
                },
                fence,
                &ChatExecutionProcessChange::Released {
                    cleanup_verified: true,
                    closed: true,
                },
            )
        })
    })
    .await
    .expect("cleanup task")
    .expect("clear exact logical fence; no process was created");
    fixture.finish().await;
    assert_eq!(recovered.owner.phase, ChatOwnerPhase::Open);
    assert_eq!(recovered.session.state, ChatState::RecoveryRequired);
    assert_eq!(refreshed, Err("workspace_directory_unavailable"));
    assert_eq!(refused, Err("workspace_directory_unavailable"));
    assert_eq!(cleaned.session.state, ChatState::Closed);
}

async fn seed_a2a(
    fixture: &OwnedFixture,
    override_workspace: Option<RunWorkspace>,
    managed: bool,
) -> (WorkspaceReadQuery, RunWorkspace) {
    let task = Task::new(TaskSpec {
        project_id: fixture.owner.project_id,
        title: "Workspace read fixture".into(),
        description: String::new(),
    })
    .expect("task");
    let run = Run::new(RunSpec {
        project_id: task.project_id(),
        task_id: task.task_id(),
        harness: "fixture".into(),
        role: "worker".into(),
        protocol: "a2a".into(),
        base_commit: fixture.base.clone(),
    })
    .expect("run");
    let query = WorkspaceReadQuery {
        project_id: task.project_id(),
        task_id: task.task_id(),
        run_id: run.run_id(),
        mode: WorkspaceReadMode::Directory,
    };
    let workspace = if let Some(workspace) = override_workspace {
        workspace
    } else if managed {
        fixture
            .manager
            .plan(run.run_id())
            .expect("managed A2A plan")
    } else {
        let path = fixture
            .root
            .parent()
            .expect("owned temp root")
            .join("unmanaged")
            .join(run.run_id().to_string());
        std::fs::create_dir_all(&path).expect("ordinary owned legacy directory");
        RunWorkspace {
            path: path.to_str().expect("UTF-8 fixture").into(),
            branch: "legacy/full/recorded_branch".into(),
            base_commit: fixture.base.clone(),
            ownership_token: Uuid::new_v4().simple().to_string(),
        }
    };
    let start = A2aRunStart {
        task,
        run,
        peer_id: "workspace_fixture".into(),
        external_task_id: Uuid::new_v4().to_string(),
        transport: "http_json".into(),
        protocol_version: "1.0".into(),
        workspace: workspace.clone(),
        timestamp: OffsetDateTime::now_utc(),
        idempotency_key: Uuid::new_v4().to_string(),
    };
    let writer = fixture.writer.clone();
    tokio::task::spawn_blocking(move || writer.with_store(move |store| store.start_a2a_run(start)))
        .await
        .expect("A2A task")
        .expect("canonical A2A binding");
    if managed {
        fixture
            .manager
            .create_planned(query.run_id, &workspace)
            .await
            .expect("owned managed A2A effect");
    }
    (query, workspace)
}

#[tokio::test]
async fn managed_a2a_is_presentable_while_unmanaged_and_other_run_slots_are_refused() {
    let fixture = OwnedFixture::new().await;
    let (managed_query, managed) = seed_a2a(&fixture, None, true).await;
    let (legacy_query, legacy) = seed_a2a(&fixture, None, false).await;
    let borrowed = fixture
        .owner
        .workspace
        .clone()
        .expect("another managed Run slot");
    let (borrowed_query, _) = seed_a2a(&fixture, Some(borrowed), false).await;
    let before = events(&fixture).await;
    let managed_view = read_workspace(fixture.writer.clone(), &fixture.root, managed_query).await;
    let legacy_view =
        read_workspace(fixture.writer.clone(), &fixture.root, legacy_query.clone()).await;
    let legacy_metadata = read_workspace(
        fixture.writer.clone(),
        &fixture.root,
        WorkspaceReadQuery {
            mode: WorkspaceReadMode::Metadata,
            ..legacy_query
        },
    )
    .await;
    let wrong_slot = read_workspace(fixture.writer.clone(), &fixture.root, borrowed_query).await;
    let after = events(&fixture).await;
    let cleanup = fixture.manager.cleanup(&managed).await;
    if let Err(error) = cleanup {
        drop(fixture);
        panic!("owned secondary cleanup refused: {error}");
    }
    fixture.finish().await;
    assert!(managed_view.expect("managed A2A").directory.is_some());
    assert_eq!(legacy_view, Err("workspace_directory_unavailable"));
    assert_eq!(
        legacy_metadata.expect("legacy metadata").branch,
        Some(legacy.branch)
    );
    assert_eq!(wrong_slot, Err("workspace_directory_unavailable"));
    assert_eq!(after, before);
}

#[test]
fn closed_query_bounds_and_debug_keep_private_values_out_of_diagnostics() {
    let query = WorkspaceReadQuery {
        project_id: ProjectId::new(),
        task_id: TaskId::new(),
        run_id: RunId::new(),
        mode: WorkspaceReadMode::Metadata,
    };
    let mut encoded = serde_json::to_value(&query).expect("query");
    encoded["path"] = serde_json::json!("private");
    assert!(serde_json::from_value::<WorkspaceReadQuery>(encoded).is_err());
    let mut encoded = serde_json::to_value(&query).expect("query");
    encoded["run_id"] = serde_json::json!(Uuid::nil());
    assert!(serde_json::from_value::<WorkspaceReadQuery>(encoded).is_err());
    let directory = WorkspaceDirectory {
        path: "/PRIVATE_DIRECTORY".into(),
        resource_key: "a".repeat(64),
    };
    let mut view = WorkspaceReadView {
        project_id: query.project_id,
        task_id: query.task_id,
        run_id: query.run_id,
        branch: Some("PRIVATE_BRANCH".into()),
        directory: Some(directory.clone()),
    };
    assert!(validate_view(&view).is_ok());
    for debug in [format!("{directory:?}"), format!("{view:?}")] {
        assert!(!debug.contains("PRIVATE_DIRECTORY"));
        assert!(!debug.contains("PRIVATE_BRANCH"));
        assert!(!debug.contains(&"a".repeat(64)));
    }
    view.branch = Some("x".repeat(MAX_WORKSPACE_BRANCH_BYTES + 1));
    assert_eq!(validate_view(&view), Err("workspace_limit"));
    view.branch = Some("branch\nforged".into());
    assert_eq!(validate_view(&view), Err("workspace_limit"));
    view.branch = Some("branch".into());
    view.directory.as_mut().expect("directory").path = "x".repeat(MAX_WORKSPACE_PATH_BYTES + 1);
    assert_eq!(validate_view(&view), Err("workspace_limit"));
}
