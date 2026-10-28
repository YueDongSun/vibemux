use tempfile::TempDir;
use uuid::Uuid;
use vibemux_harness::dispatch::writable_profile::WritableGrant;
use vibemux_worker_tools::{contract::*, workspace::WorkerTools};
use vibemux_workflow::PathPattern;

fn bootstrap(grants: WritableGrant) -> Bootstrap {
    Bootstrap {
        schema_version: SCHEMA_VERSION,
        attempt_id: Uuid::new_v4(),
        grants,
        owned_paths: vec![PathPattern::new("src/**").unwrap()],
        read_scopes: vec![PathPattern::new("context/**").unwrap()],
        forbidden_paths: vec![PathPattern::new("src/protected.txt").unwrap()],
        files: vec![
            TextFile {
                path: "src/existing.txt".into(),
                content: "original".into(),
            },
            TextFile {
                path: "context/readme.txt".into(),
                content: "context".into(),
            },
        ],
    }
}

fn parent() -> TempDir {
    let directory = TempDir::new().unwrap();
    #[cfg(windows)]
    vibemux_platform::secure_user_directory(directory.path()).unwrap();
    directory
}

#[test]
fn edit_existing_cannot_create_and_create_cannot_replace() {
    let parent = parent();
    let mut edit = WorkerTools::create(
        parent.path(),
        bootstrap(WritableGrant {
            edit_files: true,
            ..WritableGrant::NONE
        }),
    )
    .unwrap();
    assert!(matches!(
        edit.execute(Operation::Edit {
            path: "src/missing.txt".into(),
            expected_revision: 0,
            content: "new".into()
        }),
        Err(ToolError::Missing)
    ));
    assert!(matches!(
        edit.execute(Operation::Create {
            path: "src/new.txt".into(),
            content: "new".into()
        }),
        Err(ToolError::Denied)
    ));
    edit.execute(Operation::Edit {
        path: "src/existing.txt".into(),
        expected_revision: 0,
        content: "edited".into(),
    })
    .unwrap();
    assert_eq!(edit.candidate().unwrap().changed_files[0].content, "edited");

    let mut create = WorkerTools::create(
        parent.path(),
        bootstrap(WritableGrant {
            write_files: true,
            ..WritableGrant::NONE
        }),
    )
    .unwrap();
    assert!(matches!(
        create.execute(Operation::Create {
            path: "src/existing.txt".into(),
            content: "replaced".into()
        }),
        Err(ToolError::AlreadyExists)
    ));
    assert!(matches!(
        create.execute(Operation::Edit {
            path: "src/existing.txt".into(),
            expected_revision: 0,
            content: "replaced".into()
        }),
        Err(ToolError::Denied)
    ));
    create
        .execute(Operation::Create {
            path: "src/new.txt".into(),
            content: "created".into(),
        })
        .unwrap();
    assert_eq!(
        create.candidate().unwrap().changed_files[0].path,
        "src/new.txt"
    );
}

#[test]
fn read_search_and_mutation_grants_are_independent() {
    let parent = parent();
    for grants in [
        WritableGrant::NONE,
        WritableGrant {
            read_files: true,
            ..WritableGrant::NONE
        },
        WritableGrant {
            search_files: true,
            ..WritableGrant::NONE
        },
    ] {
        let mut tools = WorkerTools::create(parent.path(), bootstrap(grants)).unwrap();
        assert_eq!(
            tools
                .execute(Operation::Read {
                    path: "context/readme.txt".into()
                })
                .is_ok(),
            grants.read_files
        );
        assert_eq!(
            tools
                .execute(Operation::Search { prefix: "".into() })
                .is_ok(),
            grants.search_files
        );
        assert!(matches!(
            tools.execute(Operation::Create {
                path: "src/new.txt".into(),
                content: "x".into()
            }),
            Err(ToolError::Denied)
        ));
        assert!(tools.candidate().unwrap().changed_files.is_empty());
    }
}

#[test]
fn stale_and_aba_revisions_do_not_mutate_and_shorter_edits_truncate() {
    let parent = parent();
    let input = bootstrap(WritableGrant {
        edit_files: true,
        read_files: true,
        ..WritableGrant::NONE
    });
    let attempt = input.attempt_id;
    let mut tools = WorkerTools::create(parent.path(), input).unwrap();
    let base = tools.candidate().unwrap().base_snapshot;
    tools
        .execute(Operation::Edit {
            path: "src/existing.txt".into(),
            expected_revision: 0,
            content: "B".into(),
        })
        .unwrap();
    tools
        .execute(Operation::Edit {
            path: "src/existing.txt".into(),
            expected_revision: 1,
            content: "original".into(),
        })
        .unwrap();
    assert!(matches!(
        tools.execute(Operation::Edit {
            path: "src/existing.txt".into(),
            expected_revision: 0,
            content: "stale".into()
        }),
        Err(ToolError::StaleRevision)
    ));
    let OperationResult::Read { content, file } = tools
        .execute(Operation::Read {
            path: "src/existing.txt".into(),
        })
        .unwrap()
    else {
        panic!("read result")
    };
    assert_eq!(content, "original");
    assert_eq!(file.revision, 2);
    tools
        .execute(Operation::Edit {
            path: "src/existing.txt".into(),
            expected_revision: 2,
            content: "X".into(),
        })
        .unwrap();
    let candidate = tools.candidate().unwrap();
    assert_eq!(candidate.attempt_id, attempt);
    assert_eq!(candidate.base_snapshot, base);
    assert_eq!(candidate.changed_files[0].content, "X");
    assert_eq!(candidate.operations[2].status, "refused");
}

#[test]
fn paths_scopes_and_reserved_names_fail_without_candidate_changes() {
    let parent = parent();
    let mut tools = WorkerTools::create(
        parent.path(),
        bootstrap(WritableGrant {
            write_files: true,
            edit_files: true,
            read_files: true,
            ..WritableGrant::NONE
        }),
    )
    .unwrap();
    for path in [
        "../escape",
        "/absolute",
        "C:/drive",
        "a\\b",
        "a:b",
        "src/CON.txt",
        "src/Lpt1.log",
        "src/trailing.",
        "src/.GIT/config",
        "src/.claude/settings.json",
        "src/.vibemux_owner.json",
    ] {
        assert!(
            tools
                .execute(Operation::Create {
                    path: path.into(),
                    content: "bad".into()
                })
                .is_err(),
            "{path}"
        );
    }
    for path in ["other/new.txt", "context/new.txt", "src/PROTECTED.txt"] {
        assert!(
            tools
                .execute(Operation::Create {
                    path: path.into(),
                    content: "bad".into()
                })
                .is_err(),
            "{path}"
        );
    }
    assert!(tools.candidate().unwrap().changed_files.is_empty());
    assert!(matches!(
        tools.execute(Operation::Create {
            path: "src/Existing.txt".into(),
            content: "alias".into()
        }),
        Err(ToolError::InvalidPath)
    ));
}

#[test]
fn bootstrap_rejects_aliases_file_directory_conflicts_and_unqualified_commands() {
    let parent = parent();
    for path in [
        "src/Existing.txt",
        "src/existing.txt/child",
        "src/existing.txt",
    ] {
        let mut input = bootstrap(WritableGrant::NONE);
        input.files.push(TextFile {
            path: path.into(),
            content: "x".into(),
        });
        assert!(WorkerTools::create(parent.path(), input).is_err());
    }
    assert!(matches!(
        WorkerTools::create(
            parent.path(),
            bootstrap(WritableGrant {
                run_dev_tests: true,
                ..WritableGrant::NONE
            })
        ),
        Err(ToolError::ExecutionUnsupported)
    ));
    assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 0);
}

#[test]
fn resource_limits_refuse_before_creating_or_overwriting() {
    let parent = parent();
    let mut tools = WorkerTools::create(
        parent.path(),
        bootstrap(WritableGrant {
            write_files: true,
            ..WritableGrant::NONE
        }),
    )
    .unwrap();
    for content in ["x".repeat(MAX_FILE_BYTES + 1), "bad\0text".into()] {
        assert!(matches!(
            tools.execute(Operation::Create {
                path: "src/new.txt".into(),
                content
            }),
            Err(ToolError::ContentLimit)
        ));
    }
    for _ in 2..MAX_OPERATIONS {
        assert!(matches!(
            tools.execute(Operation::Read {
                path: "src/existing.txt".into()
            }),
            Err(ToolError::Denied)
        ));
    }
    assert!(matches!(
        tools.execute(Operation::Create {
            path: "src/new.txt".into(),
            content: "late".into()
        }),
        Err(ToolError::ResourceLimit)
    ));
    assert!(tools.candidate().unwrap().changed_files.is_empty());
}

#[test]
fn changed_workspace_is_retained_with_owner_identity_and_cannot_export() {
    let parent = parent();
    let input = bootstrap(WritableGrant {
        edit_files: true,
        ..WritableGrant::NONE
    });
    let attempt = input.attempt_id;
    let mut tools = WorkerTools::create(parent.path(), input).unwrap();
    let root = std::fs::read_dir(parent.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(root.join("src/existing.txt"), "unexpected external bytes").unwrap();
    assert!(matches!(
        tools.execute(Operation::Edit {
            path: "src/existing.txt".into(),
            expected_revision: 0,
            content: "replacement".into()
        }),
        Err(ToolError::WorkspaceChanged)
    ));
    assert!(matches!(tools.candidate(), Err(ToolError::Quarantined)));
    assert!(matches!(
        tools.execute(Operation::Edit {
            path: "src/existing.txt".into(),
            expected_revision: 0,
            content: "retry".into()
        }),
        Err(ToolError::Quarantined)
    ));
    let recovery = tools.recovery_record().unwrap();
    assert_eq!(recovery.attempt_id, attempt);
    assert_eq!(recovery.retained_directory, root.canonicalize().unwrap());
    assert_eq!(recovery.operations[0].status, "outcome_unknown");
    drop(tools);
    assert!(root.exists());
    let marker: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(".vibemux_owner.json")).unwrap()).unwrap();
    assert_eq!(marker["attempt_id"], attempt.to_string());
}

#[cfg(windows)]
#[test]
fn every_imported_object_inherits_the_verified_private_directory() {
    let parent = parent();
    let tools = WorkerTools::create(parent.path(), bootstrap(WritableGrant::NONE)).unwrap();
    let root = std::fs::read_dir(parent.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    for path in [
        &root,
        &root.join("src/existing.txt"),
        &root.join(".vibemux_owner.json"),
    ] {
        assert!(
            vibemux_platform::verify_restricted_path_acl(path)
                .unwrap()
                .restricted
        );
    }
    drop(tools);
    assert!(!root.exists());
}

#[test]
fn strict_operations_reject_permission_updates_and_unknown_fields() {
    for value in [
        serde_json::json!({"operation":"create","path":"src/new","content":"x","grants":{}}),
        serde_json::json!({"operation":"run","command":"anything"}),
        serde_json::json!({"operation":"edit","path":"src/existing.txt","content":"x","expected_revision":-1}),
    ] {
        assert!(serde_json::from_value::<Operation>(value).is_err());
    }
}

#[test]
fn owner_output_failure_retains_the_only_complete_candidate() {
    let parent = parent();
    let input = bootstrap(WritableGrant {
        edit_files: true,
        ..WritableGrant::NONE
    });
    let attempt = input.attempt_id;
    let mut tools = WorkerTools::create(parent.path(), input).unwrap();
    tools
        .execute(Operation::Edit {
            path: "src/existing.txt".into(),
            expected_revision: 0,
            content: "complete candidate".into(),
        })
        .unwrap();
    let result = vibemux_worker_tools::owner_output::finalize(&mut tools, true, |_bytes| {
        Err(std::io::Error::other("fixture sink failed after write"))
    });
    assert_eq!(result, Err("worker_tools_candidate_outcome_unknown"));
    let recovery = tools.recovery_record().unwrap();
    assert_eq!(recovery.attempt_id, attempt);
    assert!(matches!(tools.candidate(), Err(ToolError::Quarantined)));
    drop(tools);
    assert_eq!(
        std::fs::read_to_string(recovery.retained_directory.join("src/existing.txt")).unwrap(),
        "complete candidate"
    );
    assert_eq!(recovery.operations[0].status, "succeeded");
}
