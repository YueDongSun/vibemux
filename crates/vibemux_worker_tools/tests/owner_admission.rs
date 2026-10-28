use tempfile::TempDir;
use uuid::Uuid;
use vibemux_harness::dispatch::writable_profile::WritableGrant;
use vibemux_worker_tools::{
    contract::*,
    owner_admission::{snapshot_digest, validate_candidate},
    workspace::WorkerTools,
};
use vibemux_workflow::{PathPattern, Sha256Digest};

fn grants() -> WritableGrant {
    WritableGrant {
        read_files: true,
        search_files: true,
        edit_files: true,
        write_files: true,
        run_dev_tests: false,
    }
}

fn input(grants: WritableGrant) -> Bootstrap {
    Bootstrap {
        schema_version: SCHEMA_VERSION,
        attempt_id: Uuid::new_v4(),
        grants,
        owned_paths: vec![PathPattern::new("src/**").unwrap()],
        read_scopes: vec![PathPattern::new("context/**").unwrap()],
        forbidden_paths: vec![PathPattern::new("src/secret/**").unwrap()],
        files: vec![
            file("src/existing.txt", "original"),
            file("context/readme.txt", "reference"),
        ],
    }
}

fn file(path: &str, content: &str) -> TextFile {
    TextFile {
        path: path.to_string(),
        content: content.to_string(),
    }
}

fn private_parent() -> TempDir {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(windows)]
    vibemux_platform::secure_user_directory(directory.path()).unwrap();
    directory
}

// Adversarial tests construct purported publications. Positive export tests
// below obtain their candidates from the real WorkerTools implementation.
fn purported(input: &Bootstrap, changed_files: Vec<TextFile>) -> Candidate {
    Candidate {
        schema_version: SCHEMA_VERSION,
        attempt_id: input.attempt_id,
        base_snapshot: snapshot_digest(input).unwrap(),
        operations: changed_files
            .iter()
            .enumerate()
            .map(|(index, file)| OperationReceipt {
                sequence: index as u64 + 1,
                operation: if input.files.iter().any(|old| old.path == file.path) {
                    "edit_file"
                } else {
                    "create_file"
                }
                .to_string(),
                status: "succeeded".to_string(),
                code: None,
            })
            .collect(),
        changed_files,
    }
}

fn refusal(input: &Bootstrap, operation: &str, code: &str) -> Candidate {
    let mut candidate = purported(input, Vec::new());
    candidate.operations.push(OperationReceipt {
        sequence: 1,
        operation: operation.to_string(),
        status: "refused".to_string(),
        code: Some(code.to_string()),
    });
    candidate
}

#[test]
fn actual_create_and_edit_exports_keep_their_independent_grants() {
    let parent = private_parent();
    let create_input = input(WritableGrant {
        write_files: true,
        ..WritableGrant::NONE
    });
    let mut create = WorkerTools::create(parent.path(), create_input.clone()).unwrap();
    assert!(matches!(
        create.execute(Operation::Edit {
            path: "src/existing.txt".into(),
            expected_revision: 0,
            content: "not allowed".into(),
        }),
        Err(ToolError::Denied)
    ));
    create
        .execute(Operation::Create {
            path: "src/new.txt".into(),
            content: "created".into(),
        })
        .unwrap();
    let changes = validate_candidate(&create_input, create.candidate().unwrap())
        .unwrap()
        .into_changed_files();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].path, "src/new.txt");
    assert_eq!(changes[0].content, "created");

    let edit_input = input(WritableGrant {
        edit_files: true,
        ..WritableGrant::NONE
    });
    let mut edit = WorkerTools::create(parent.path(), edit_input.clone()).unwrap();
    assert!(matches!(
        edit.execute(Operation::Create {
            path: "src/new.txt".into(),
            content: "not allowed".into(),
        }),
        Err(ToolError::Denied)
    ));
    edit.execute(Operation::Edit {
        path: "src/existing.txt".into(),
        expected_revision: 0,
        content: "edited".into(),
    })
    .unwrap();
    let changes = validate_candidate(&edit_input, edit.candidate().unwrap())
        .unwrap()
        .into_changed_files();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].path, "src/existing.txt");
    assert_eq!(changes[0].content, "edited");
}

#[test]
fn actual_read_only_exports_and_refused_operations_are_accepted() {
    let parent = private_parent();
    for grants in [WritableGrant::READ_ONLY, WritableGrant::NONE] {
        let input = input(grants);
        let mut tools = WorkerTools::create(parent.path(), input.clone()).unwrap();
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
        assert!(
            tools
                .execute(Operation::Create {
                    path: "src/new.txt".into(),
                    content: "denied".into()
                })
                .is_err()
        );
        assert!(
            validate_candidate(&input, tools.candidate().unwrap())
                .unwrap()
                .into_changed_files()
                .is_empty()
        );
    }
}

#[test]
fn no_op_and_restore_edits_remain_valid_exported_changes() {
    let parent = private_parent();
    let input = input(grants());
    let mut tools = WorkerTools::create(parent.path(), input.clone()).unwrap();
    for (revision, content) in [(0, "original"), (1, "temporary"), (2, "original")] {
        tools
            .execute(Operation::Edit {
                path: "src/existing.txt".into(),
                expected_revision: revision,
                content: content.into(),
            })
            .unwrap();
        let changes = validate_candidate(&input, tools.candidate().unwrap())
            .unwrap()
            .into_changed_files();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].content, content);
    }
}

#[test]
fn snapshot_identity_preserves_wire_bytes_and_ignores_input_order() {
    let parent = private_parent();
    let mut input = input(grants());
    let expected = Sha256Digest::of_fields(
        "vibemux.worker_tools.snapshot.v1",
        &[br#"[{"path":"context/readme.txt","content":"reference"},{"path":"src/existing.txt","content":"original"}]"#],
    );
    assert_eq!(snapshot_digest(&input).unwrap(), expected);
    input.files.reverse();
    assert_eq!(snapshot_digest(&input).unwrap(), expected);
    let mut tools = WorkerTools::create(parent.path(), input.clone()).unwrap();
    assert_eq!(tools.candidate().unwrap().base_snapshot, expected);
}

#[test]
fn stale_schema_attempt_and_input_are_refused() {
    let input = input(grants());
    let mut candidate = purported(&input, Vec::new());
    candidate.schema_version += 1;
    assert!(validate_candidate(&input, candidate).is_err());

    let mut candidate = purported(&input, Vec::new());
    candidate.attempt_id = Uuid::new_v4();
    assert!(validate_candidate(&input, candidate).is_err());

    let candidate = purported(&input, Vec::new());
    let mut changed_input = input.clone();
    changed_input.files[0].content = "later input".into();
    assert!(validate_candidate(&changed_input, candidate).is_err());

    let mut candidate = purported(&input, Vec::new());
    candidate.base_snapshot = Sha256Digest::of(b"unrelated input");
    assert!(validate_candidate(&input, candidate).is_err());
}

#[test]
fn invalid_inputs_and_unqualified_test_grants_fail_before_admission() {
    let valid = input(grants());
    for invalid in [
        Bootstrap {
            attempt_id: Uuid::nil(),
            ..valid.clone()
        },
        Bootstrap {
            schema_version: 2,
            ..valid.clone()
        },
        Bootstrap {
            files: vec![file("src/a.txt", "a"), file("src/A.txt", "b")],
            ..valid.clone()
        },
        Bootstrap {
            files: vec![file("src/file.txt", "\0")],
            ..valid.clone()
        },
    ] {
        assert!(snapshot_digest(&invalid).is_err());
        assert!(validate_candidate(&invalid, purported(&valid, Vec::new())).is_err());
    }
    let tests = Bootstrap {
        grants: WritableGrant {
            run_dev_tests: true,
            ..grants()
        },
        ..valid.clone()
    };
    assert_eq!(
        snapshot_digest(&tests),
        Err(ToolError::ExecutionUnsupported)
    );
    assert!(matches!(
        validate_candidate(&tests, purported(&valid, Vec::new())),
        Err(ToolError::ExecutionUnsupported)
    ));
}

#[test]
fn changed_paths_cannot_escape_owned_scope_or_protected_names() {
    let input = input(grants());
    for path in [
        "../escape.txt",
        "C:/escape.txt",
        "src\\escape.txt",
        "src//bad.txt",
        "src/CON.txt",
        "src/file.",
        "src/.git/config",
        "src/.vibemux_owner.json",
        "src/secret/data.txt",
        "src/SECRET/data.txt",
        "context/readme.txt",
        "outside/new.txt",
    ] {
        assert!(
            validate_candidate(&input, purported(&input, vec![file(path, "bad")])).is_err(),
            "{path}"
        );
    }
}

#[test]
fn duplicate_case_and_prefix_collisions_are_refused_with_unchanged_input() {
    let input = input(grants());
    for paths in [
        vec!["src/new.txt", "src/new.txt"],
        vec!["src/new.txt", "src/NEW.txt"],
        vec!["src/tree", "src/tree/leaf.txt"],
        vec!["src/tree/a.txt", "src/TREE/b.txt"],
        vec!["src/Existing.txt"],
        vec!["src/existing.txt/child.txt"],
        vec!["context"],
    ] {
        let files = paths.iter().map(|path| file(path, "bad")).collect();
        assert!(
            validate_candidate(&input, purported(&input, files)).is_err(),
            "{paths:?}"
        );
    }
}

#[test]
fn file_existence_cannot_be_relabelled_to_bypass_create_or_edit_grants() {
    let edit_only = input(WritableGrant {
        edit_files: true,
        ..WritableGrant::NONE
    });
    let mut forged = purported(&edit_only, vec![file("src/new.txt", "new")]);
    forged.operations[0].operation = "edit_file".into();
    assert!(matches!(
        validate_candidate(&edit_only, forged),
        Err(ToolError::Denied)
    ));

    let create_only = input(WritableGrant {
        write_files: true,
        ..WritableGrant::NONE
    });
    let mut forged = purported(&create_only, vec![file("src/existing.txt", "replacement")]);
    forged.operations[0].operation = "create_file".into();
    assert!(matches!(
        validate_candidate(&create_only, forged),
        Err(ToolError::Denied)
    ));

    let read_only = input(WritableGrant::READ_ONLY);
    let mut forged = purported(&read_only, vec![file("src/existing.txt", "replacement")]);
    forged.operations[0].operation = "read_file".into();
    assert!(matches!(
        validate_candidate(&read_only, forged),
        Err(ToolError::Denied)
    ));
}

#[test]
fn combined_snapshot_limits_count_unchanged_files_and_content() {
    let mut full = input(grants());
    full.files = (0..MAX_FILES)
        .map(|index| file(&format!("src/{index}.txt"), "x"))
        .collect();
    assert!(matches!(
        validate_candidate(&full, purported(&full, vec![file("src/new.txt", "x")])),
        Err(ToolError::ResourceLimit)
    ));
    full.files = (0..MAX_TOTAL_BYTES / MAX_FILE_BYTES)
        .map(|index| file(&format!("src/{index}.txt"), &"x".repeat(MAX_FILE_BYTES)))
        .collect();
    assert!(matches!(
        validate_candidate(&full, purported(&full, vec![file("src/new.txt", "x")])),
        Err(ToolError::ResourceLimit)
    ));
    // Shrinking one existing file legitimately frees room for a new file.
    let candidate = purported(&full, vec![file("src/0.txt", ""), file("src/new.txt", "x")]);
    assert_eq!(
        validate_candidate(&full, candidate)
            .unwrap()
            .into_changed_files()
            .len(),
        2
    );
}

#[test]
fn changed_content_and_operation_counts_remain_bounded() {
    let input = input(grants());
    for content in ["\0".to_string(), "x".repeat(MAX_FILE_BYTES + 1)] {
        assert!(matches!(
            validate_candidate(
                &input,
                purported(&input, vec![file("src/new.txt", &content)])
            ),
            Err(ToolError::ContentLimit)
        ));
    }
    let candidate = purported(
        &input,
        (0..=MAX_FILES)
            .map(|index| file(&format!("src/{index}.txt"), "x"))
            .collect(),
    );
    assert!(matches!(
        validate_candidate(&input, candidate),
        Err(ToolError::ResourceLimit)
    ));

    let mut candidate = purported(&input, Vec::new());
    candidate.operations = (1..=MAX_OPERATIONS + 1)
        .map(|sequence| OperationReceipt {
            sequence: sequence as u64,
            operation: "read_file".into(),
            status: "succeeded".into(),
            code: None,
        })
        .collect();
    assert!(matches!(
        validate_candidate(&input, candidate),
        Err(ToolError::ResourceLimit)
    ));
}

#[test]
fn malformed_or_unknown_receipt_shapes_are_refused() {
    let input = input(grants());
    for (operation, status, code) in [
        ("unknown_tool", "succeeded", None),
        ("read_file", "unknown_status", None),
        ("read_file", "succeeded", Some("worker_tools_missing")),
        ("read_file", "refused", None),
        ("read_file", "refused", Some("unknown_code")),
        (
            "read_file",
            "refused",
            Some("worker_tools_mutation_unknown"),
        ),
        (
            "read_file",
            "refused",
            Some("worker_tools_workspace_changed"),
        ),
        ("read_file", "refused", Some("worker_tools_denied")),
        (
            "read_file",
            "outcome_unknown",
            Some("worker_tools_mutation_unknown"),
        ),
    ] {
        let mut candidate = purported(&input, Vec::new());
        candidate.operations.push(OperationReceipt {
            sequence: 1,
            operation: operation.into(),
            status: status.into(),
            code: code.map(str::to_string),
        });
        assert!(
            validate_candidate(&input, candidate).is_err(),
            "{operation}/{status}/{code:?}"
        );
    }
    for sequences in [vec![0], vec![2], vec![1, 1], vec![1, 3]] {
        let mut candidate = purported(&input, Vec::new());
        candidate.operations = sequences
            .iter()
            .map(|sequence| OperationReceipt {
                sequence: *sequence,
                operation: "read_file".into(),
                status: "succeeded".into(),
                code: None,
            })
            .collect();
        assert!(
            validate_candidate(&input, candidate).is_err(),
            "{sequences:?}"
        );
    }
    assert!(
        validate_candidate(
            &input,
            refusal(&input, "read_file", ToolError::Missing.code())
        )
        .is_ok()
    );
}

#[test]
fn receipt_grants_and_minimum_mutation_counts_cannot_contradict_changes() {
    for (operation, grants) in [
        (
            "read_file",
            WritableGrant {
                read_files: false,
                ..grants()
            },
        ),
        (
            "search_paths",
            WritableGrant {
                search_files: false,
                ..grants()
            },
        ),
        (
            "create_file",
            WritableGrant {
                write_files: false,
                ..grants()
            },
        ),
        (
            "edit_file",
            WritableGrant {
                edit_files: false,
                ..grants()
            },
        ),
    ] {
        let input = input(grants);
        let mut candidate = purported(&input, Vec::new());
        candidate.operations.push(OperationReceipt {
            sequence: 1,
            operation: operation.into(),
            status: "succeeded".into(),
            code: None,
        });
        assert!(matches!(
            validate_candidate(&input, candidate),
            Err(ToolError::Denied)
        ));
        assert!(
            validate_candidate(&input, refusal(&input, operation, ToolError::Denied.code()))
                .is_ok()
        );
        assert!(
            validate_candidate(
                &input,
                refusal(&input, operation, ToolError::Missing.code())
            )
            .is_err()
        );
    }

    let input = input(grants());
    let mut candidate = purported(
        &input,
        vec![file("src/new.txt", "x"), file("src/other.txt", "y")],
    );
    candidate.operations.pop();
    assert!(validate_candidate(&input, candidate).is_err());
    let mut candidate = purported(&input, vec![file("src/existing.txt", "original")]);
    candidate.operations.clear();
    assert!(validate_candidate(&input, candidate).is_err());
}
