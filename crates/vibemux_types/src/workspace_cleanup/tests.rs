use super::*;

use crate::{ChatId, ProjectId, RunId, TaskId, WorkspaceCleanupId, a2a::RunWorkspace};
use serde_json::{Value, json};
use time::{Duration, OffsetDateTime};

fn identity() -> CleanupObjectIdentity {
    CleanupObjectIdentity::WindowsFileId128V1 {
        volume_serial_number: (1_u64 << 60) + 17,
        file_id: [0x81, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 0xfe],
    }
}

fn planned() -> WorkspaceCleanup {
    let run_id = RunId::new();
    let now = OffsetDateTime::from_unix_timestamp(1_900_000_000).unwrap();
    WorkspaceCleanup {
        schema_version: WORKSPACE_CLEANUP_SCHEMA_VERSION,
        id: WorkspaceCleanupId::new(),
        plan_operation_id: Uuid::new_v4(),
        daemon_incarnation: Uuid::new_v4(),
        version: 1,
        phase: WorkspaceCleanupPhase::Planned,
        binding: WorkspaceCleanupBinding {
            project_id: ProjectId::new(),
            chat_id: ChatId::new(),
            task_id: TaskId::new(),
            run_id,
            owner_version: 1,
            session_generation: 2,
            session_version: 3,
            workspace: RunWorkspace {
                path: format!("C:/private/repo/.vibemux/worktrees/{run_id}"),
                branch: format!("codex/run_{}", run_id.as_uuid().simple()),
                base_commit: "a".repeat(40),
                ownership_token: "private_ownership_token".to_owned(),
            },
            resource_key: "1".repeat(64),
            repository_root: "C:/private/repo".to_owned(),
            common_git_dir: "C:/private/repo/.git".to_owned(),
            repository_identity: identity(),
            common_directory_identity: identity(),
            worktree_identity: identity(),
            head_commit: "b".repeat(40),
            terminal_plugin_id: "terminal_private".to_owned(),
            session_id: "session_private".to_owned(),
            instance_id: "instance_private".to_owned(),
        },
        created_at: now,
        expires_at: now + Duration::seconds(300),
        updated_at: now,
        confirmation_operation_id: None,
        claim_id: None,
        executor: None,
        arm_operation_id: None,
        process_result: None,
        quiescence: None,
        outcome: None,
        error_code: None,
    }
}

fn claimed() -> WorkspaceCleanup {
    let mut record = planned();
    record.phase = WorkspaceCleanupPhase::Claimed;
    record.confirmation_operation_id = Some(Uuid::new_v4());
    record.claim_id = Some(Uuid::new_v4());
    record
}

fn executor(record: &WorkspaceCleanup) -> ExecutorBinding {
    ExecutorBinding {
        executor_nonce: Uuid::new_v4(),
        daemon_incarnation: record.daemon_incarnation,
        process_id: 42,
        observed_start_seconds: 1_900_000_001,
        expected_executable: "C:/private/app/vibemux.exe".to_owned(),
        observed_executable: "C:/private/app/vibemux.exe".to_owned(),
        consumed_executable_identity: identity(),
        observed_path_identity: identity(),
        program_executable: "C:/private/git/bin/git.exe".to_owned(),
        program_identity: identity(),
        command_profile: CleanupCommandProfile::GitWorktreeRemoveV1,
        containment_kind: CleanupContainmentKind::WindowsJobV1,
    }
}

fn joined(executor_nonce: Uuid) -> CleanupQuiescence {
    CleanupQuiescence::JoinedProcess {
        executor_nonce,
        termination_succeeded: true,
        root_reaped: true,
        tree_empty: true,
        stdout_drained: true,
        stderr_drained: true,
    }
}

fn armed() -> WorkspaceCleanup {
    let mut record = claimed();
    record.phase = WorkspaceCleanupPhase::Armed;
    record.executor = Some(executor(&record));
    record.arm_operation_id = Some(Uuid::new_v4());
    record
}

fn quiescent_joined() -> WorkspaceCleanup {
    let mut record = armed();
    record.phase = WorkspaceCleanupPhase::Quiescent;
    record.quiescence = Some(joined(record.executor.as_ref().unwrap().executor_nonce));
    record
}

fn quiescent_never_spawned() -> WorkspaceCleanup {
    let mut record = claimed();
    record.phase = WorkspaceCleanupPhase::Quiescent;
    record.quiescence = Some(CleanupQuiescence::NeverSpawned {
        retired_claim_id: record.claim_id.unwrap(),
    });
    record
}

fn successful_result(record: &WorkspaceCleanup) -> CleanupProcessResult {
    CleanupProcessResult {
        executor_nonce: record.executor.as_ref().unwrap().executor_nonce,
        exit_code: Some(0),
        forced_termination: false,
        timed_out: false,
        release_attempted: true,
    }
}

#[test]
fn full_width_identity_round_trip_preserves_high_bits_and_large_volume() {
    let record = planned();
    record.validate().unwrap();
    let encoded = serde_json::to_string(&record).unwrap();
    let decoded: WorkspaceCleanup = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, record);
    decoded.validate().unwrap();
    assert!(encoded.contains("1152921504606846993"));
    assert!(encoded.contains("windows_file_id128_v1"));
    assert_eq!(MAX_WORKSPACE_CLEANUP_METADATA_BYTES, 64 * 1024);
}

#[test]
fn identity_format_is_closed_and_zero_file_ids_are_invalid() {
    let mut value = serde_json::to_value(identity()).unwrap();
    value["format"] = json!("arbitrary_kind");
    assert!(serde_json::from_value::<CleanupObjectIdentity>(value).is_err());

    let mut value = serde_json::to_value(identity()).unwrap();
    value["hash"] = json!("unrecognized");
    assert!(serde_json::from_value::<CleanupObjectIdentity>(value).is_err());

    let zero_id = CleanupObjectIdentity::WindowsFileId128V1 {
        volume_serial_number: u64::MAX,
        file_id: [0; 16],
    };
    assert_eq!(zero_id.validate(), Err(CleanupError::InvalidInput));
    let zero_volume = CleanupObjectIdentity::WindowsFileId128V1 {
        volume_serial_number: 0,
        file_id: [0xff; 16],
    };
    assert!(zero_volume.validate().is_ok());
}

#[test]
fn metadata_rejects_unknown_fields_at_every_object_boundary() {
    for pointer in [
        "",
        "/binding",
        "/binding/workspace",
        "/binding/worktree_identity",
        "/executor",
        "/process_result",
        "/quiescence",
    ] {
        let mut record = quiescent_joined();
        record.process_result = Some(successful_result(&record));
        let mut value = serde_json::to_value(record).unwrap();
        value
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unexpected".to_owned(), Value::Bool(true));
        assert!(
            serde_json::from_value::<WorkspaceCleanup>(value).is_err(),
            "{pointer}"
        );
    }
    let mut value = serde_json::to_value(quiescent_never_spawned()).unwrap();
    value["quiescence"]["unexpected"] = json!(true);
    assert!(serde_json::from_value::<WorkspaceCleanup>(value).is_err());
}

#[test]
fn nil_identifiers_are_rejected_by_typed_deserialization_or_validation() {
    for field in ["id", "plan_operation_id", "daemon_incarnation"] {
        let mut value = serde_json::to_value(planned()).unwrap();
        value[field] = json!(Uuid::nil());
        if let Ok(record) = serde_json::from_value::<WorkspaceCleanup>(value) {
            assert_eq!(
                record.validate(),
                Err(CleanupError::InvalidInput),
                "{field}"
            );
        }
    }
    for field in ["project_id", "chat_id", "task_id", "run_id"] {
        let mut value = serde_json::to_value(planned()).unwrap();
        value["binding"][field] = json!(Uuid::nil());
        assert!(
            serde_json::from_value::<WorkspaceCleanup>(value).is_err(),
            "{field}"
        );
    }
    for field in ["confirmation_operation_id", "claim_id", "arm_operation_id"] {
        let mut value = serde_json::to_value(armed()).unwrap();
        value[field] = json!(Uuid::nil());
        let record: WorkspaceCleanup = serde_json::from_value(value).unwrap();
        assert_eq!(
            record.validate(),
            Err(CleanupError::InvalidInput),
            "{field}"
        );
    }
    for field in ["executor_nonce", "daemon_incarnation"] {
        let mut value = serde_json::to_value(armed()).unwrap();
        value["executor"][field] = json!(Uuid::nil());
        let record: WorkspaceCleanup = serde_json::from_value(value).unwrap();
        assert_eq!(
            record.validate(),
            Err(CleanupError::InvalidInput),
            "{field}"
        );
    }
}

#[test]
fn scope_binding_requires_matching_branch_and_physical_run_slot() {
    let record = planned();
    let mut foreign = record.clone();
    foreign.binding.run_id = RunId::new();
    assert_eq!(foreign.validate(), Err(CleanupError::InvalidInput));
    let mut foreign = record.clone();
    foreign.binding.workspace.branch = "codex/run_foreign".to_owned();
    assert_eq!(foreign.validate(), Err(CleanupError::InvalidInput));
    for path in [
        format!("C:/repo/{}/suffix", record.binding.run_id),
        format!("C:/repo/../{}", record.binding.run_id),
        format!("C:/repo/./{}", record.binding.run_id),
        format!("relative/{}", record.binding.run_id),
    ] {
        let mut foreign = record.clone();
        foreign.binding.workspace.path = path;
        assert_eq!(foreign.validate(), Err(CleanupError::InvalidInput));
    }
    let mut windows = record.clone();
    windows.binding.workspace.path = windows.binding.workspace.path.replace('/', "\\");
    assert!(windows.validate().is_ok());
}

#[test]
fn versions_schema_times_and_private_string_limits_are_enforced() {
    let record = planned();
    for field in [
        "version",
        "owner_version",
        "session_generation",
        "session_version",
    ] {
        for value in [0, i64::MAX as u64 + 1] {
            let mut invalid = record.clone();
            match field {
                "version" => invalid.version = value,
                "owner_version" => invalid.binding.owner_version = value,
                "session_generation" => invalid.binding.session_generation = value,
                "session_version" => invalid.binding.session_version = value,
                _ => unreachable!(),
            }
            assert_eq!(
                invalid.validate(),
                Err(CleanupError::InvalidInput),
                "{field}"
            );
        }
    }
    let mut invalid = record.clone();
    invalid.schema_version += 1;
    assert!(invalid.validate().is_err());
    for expires_at in [
        record.created_at,
        record.created_at + Duration::seconds(301),
    ] {
        let mut invalid = record.clone();
        invalid.expires_at = expires_at;
        assert!(invalid.validate().is_err());
    }
    let mut invalid = record.clone();
    invalid.updated_at = invalid.created_at - Duration::seconds(1);
    assert!(invalid.validate().is_err());
    for field in ["repository_root", "common_git_dir"] {
        for value in [
            "relative".to_owned(),
            "C:/line\nbreak".to_owned(),
            format!("C:/{}", "a".repeat(4094)),
        ] {
            let mut invalid = record.clone();
            match field {
                "repository_root" => invalid.binding.repository_root = value,
                "common_git_dir" => invalid.binding.common_git_dir = value,
                _ => unreachable!(),
            }
            assert!(invalid.validate().is_err(), "{field}");
        }
    }
    for field in ["terminal_plugin_id", "session_id", "instance_id"] {
        for value in [
            "".to_owned(),
            " ".to_owned(),
            "bad\u{7f}".to_owned(),
            "a".repeat(129),
        ] {
            let mut invalid = record.clone();
            match field {
                "terminal_plugin_id" => invalid.binding.terminal_plugin_id = value,
                "session_id" => invalid.binding.session_id = value,
                "instance_id" => invalid.binding.instance_id = value,
                _ => unreachable!(),
            }
            assert!(invalid.validate().is_err(), "{field}");
        }
    }
    for value in ["a".repeat(63), "g".repeat(64)] {
        let mut invalid = record.clone();
        invalid.binding.resource_key = value;
        assert!(invalid.validate().is_err());
    }
    for value in ["a".repeat(39), "g".repeat(40)] {
        let mut invalid = record.clone();
        invalid.binding.head_commit = value;
        assert!(invalid.validate().is_err());
    }
    let mut maximal = record;
    maximal.version = i64::MAX as u64;
    maximal.binding.owner_version = i64::MAX as u64;
    maximal.binding.session_generation = i64::MAX as u64;
    maximal.binding.session_version = i64::MAX as u64;
    maximal.binding.repository_root = format!("C:/{}", "a".repeat(4093));
    maximal.binding.common_git_dir = format!("C:/{}", "b".repeat(4093));
    maximal.binding.terminal_plugin_id = "x".repeat(128);
    maximal.binding.session_id = "y".repeat(128);
    maximal.binding.instance_id = "z".repeat(128);
    maximal.binding.head_commit = "F".repeat(64);
    assert!(maximal.validate().is_ok());
}

#[test]
fn phase_field_matrix_requires_claims_receipts_and_correlated_completion() {
    let planned = planned();
    let claimed = claimed();
    let mut held = claimed.clone();
    held.executor = Some(executor(&held));
    let armed = armed();
    let joined = quiescent_joined();
    let never_spawned = quiescent_never_spawned();
    for record in [&planned, &claimed, &held, &armed, &joined, &never_spawned] {
        assert!(record.validate().is_ok(), "{record:?}");
    }
    for mut record in [
        planned.clone(),
        claimed.clone(),
        armed.clone(),
        joined.clone(),
    ] {
        record.outcome = Some(WorkspaceCleanupOutcome::RemovedObserved);
        assert!(record.validate().is_err());
    }
    let mut invalid = planned.clone();
    invalid.claim_id = Some(Uuid::new_v4());
    assert!(invalid.validate().is_err());
    let mut invalid = planned;
    invalid.executor = Some(executor(&invalid));
    assert!(invalid.validate().is_err());
    let mut invalid = claimed.clone();
    invalid.arm_operation_id = Some(Uuid::new_v4());
    assert!(invalid.validate().is_err());
    let mut invalid = claimed;
    invalid.process_result = Some(CleanupProcessResult {
        executor_nonce: Uuid::new_v4(),
        exit_code: Some(0),
        forced_termination: false,
        timed_out: false,
        release_attempted: true,
    });
    assert!(invalid.validate().is_err());
    let mut invalid = armed;
    invalid.executor = None;
    assert!(invalid.validate().is_err());
    let mut invalid = joined;
    invalid.quiescence = None;
    assert!(invalid.validate().is_err());
}

#[test]
fn executor_fields_are_audit_only_but_must_be_well_formed_and_correlated() {
    let record = armed();
    for field in [
        "process_id",
        "observed_start_seconds",
        "expected_executable",
        "observed_executable",
        "program_executable",
        "consumed_executable_identity",
        "program_identity",
        "daemon_incarnation",
    ] {
        let mut invalid = record.clone();
        let executor = invalid.executor.as_mut().unwrap();
        match field {
            "process_id" => executor.process_id = 0,
            "observed_start_seconds" => executor.observed_start_seconds = 0,
            "expected_executable" => executor.expected_executable = "relative".to_owned(),
            "observed_executable" => executor.observed_executable = "C:/bad\0path".to_owned(),
            "program_executable" => executor.program_executable = "C:/".repeat(1400),
            "consumed_executable_identity" => {
                executor.consumed_executable_identity = CleanupObjectIdentity::WindowsFileId128V1 {
                    volume_serial_number: 1,
                    file_id: [2; 16],
                }
            }
            "program_identity" => {
                executor.program_identity = CleanupObjectIdentity::WindowsFileId128V1 {
                    volume_serial_number: 0,
                    file_id: [0; 16],
                }
            }
            "daemon_incarnation" => executor.daemon_incarnation = Uuid::new_v4(),
            _ => unreachable!(),
        }
        assert!(invalid.validate().is_err(), "{field}");
    }
    let mut value = serde_json::to_value(record).unwrap();
    value["executor"]["command_profile"] = json!("arbitrary_command");
    assert!(serde_json::from_value::<WorkspaceCleanup>(value).is_err());
}

#[test]
fn negative_quiescence_is_reserved_unknown_and_never_terminal() {
    for field in [
        "termination_succeeded",
        "root_reaped",
        "tree_empty",
        "stdout_drained",
        "stderr_drained",
    ] {
        let record = quiescent_joined();
        let mut value = serde_json::to_value(record).unwrap();
        value["quiescence"][field] = json!(false);
        let mut negative: WorkspaceCleanup = serde_json::from_value(value).unwrap();
        assert!(!negative.quiescence.as_ref().unwrap().is_positive());
        assert!(negative.validate().is_err(), "{field}");
        negative.phase = WorkspaceCleanupPhase::Unknown;
        negative.error_code = Some("quiescence_unverified".to_owned());
        assert!(negative.validate().is_ok(), "{field}");
        assert!(negative.phase.holds_resource());
        negative.phase = WorkspaceCleanupPhase::Removed;
        negative.outcome = Some(WorkspaceCleanupOutcome::RemovedObserved);
        negative.error_code = None;
        assert!(negative.validate().is_err(), "{field}");
        negative.phase = WorkspaceCleanupPhase::Refused;
        negative.outcome = Some(WorkspaceCleanupOutcome::Refused);
        negative.error_code = Some("cleanup_refused".to_owned());
        assert!(negative.validate().is_err(), "{field}");
    }
}

#[test]
fn removed_executed_requires_successful_release_and_full_completion() {
    let mut record = quiescent_joined();
    record.phase = WorkspaceCleanupPhase::Removed;
    record.outcome = Some(WorkspaceCleanupOutcome::RemovedExecuted);
    assert!(record.validate().is_err());
    record.process_result = Some(successful_result(&record));
    assert!(record.validate().is_ok());
    for field in [
        "exit_code",
        "no_exit_code",
        "forced_termination",
        "timed_out",
        "release_attempted",
        "executor_nonce",
        "arm_operation_id",
        "quiescence",
    ] {
        let mut invalid = record.clone();
        match field {
            "exit_code" => invalid.process_result.as_mut().unwrap().exit_code = Some(1),
            "no_exit_code" => invalid.process_result.as_mut().unwrap().exit_code = None,
            "forced_termination" => {
                invalid.process_result.as_mut().unwrap().forced_termination = true
            }
            "timed_out" => invalid.process_result.as_mut().unwrap().timed_out = true,
            "release_attempted" => {
                invalid.process_result.as_mut().unwrap().release_attempted = false
            }
            "executor_nonce" => {
                invalid.process_result.as_mut().unwrap().executor_nonce = Uuid::new_v4()
            }
            "arm_operation_id" => invalid.arm_operation_id = None,
            "quiescence" => invalid.quiescence = None,
            _ => unreachable!(),
        }
        assert!(invalid.validate().is_err(), "{field}");
    }
    let mut observed = quiescent_never_spawned();
    observed.phase = WorkspaceCleanupPhase::Removed;
    observed.outcome = Some(WorkspaceCleanupOutcome::RemovedObserved);
    assert!(observed.validate().is_ok());
    observed.quiescence = None;
    assert!(observed.validate().is_err());
}

#[test]
fn release_attempts_require_an_arm_in_every_phase() {
    use WorkspaceCleanupPhase::*;

    let mut accepted = Vec::new();
    for phase in [
        Quiescent, Unknown, Refused, Removed, Planned, Claimed, Armed,
    ] {
        let mut record = quiescent_joined();
        record.phase = phase;
        record.process_result = Some(successful_result(&record));
        match phase {
            Unknown => record.error_code = Some("completion_unverified".to_owned()),
            Refused => {
                record.error_code = Some("cleanup_refused".to_owned());
                record.outcome = Some(WorkspaceCleanupOutcome::Refused);
            }
            Removed => record.outcome = Some(WorkspaceCleanupOutcome::RemovedObserved),
            Planned | Claimed | Armed => record.quiescence = None,
            Quiescent => {}
        }
        if matches!(phase, Quiescent | Unknown | Refused | Removed) {
            assert!(record.validate().is_ok(), "armed {phase:?}");
        }
        record.arm_operation_id = None;
        if record.validate().is_ok() {
            accepted.push(phase);
        }
    }
    assert!(
        accepted.is_empty(),
        "unarmed release attempts accepted: {accepted:?}"
    );
}

#[test]
fn no_go_joined_processes_preserve_unattempted_results_with_or_without_arm() {
    let mut record = quiescent_joined();
    record.arm_operation_id = None;
    assert!(record.validate().is_ok());

    let mut result = successful_result(&record);
    result.release_attempted = false;
    record.process_result = Some(result);
    assert!(record.validate().is_ok());

    record.arm_operation_id = Some(Uuid::new_v4());
    assert!(record.validate().is_ok());
    record.phase = WorkspaceCleanupPhase::Refused;
    record.outcome = Some(WorkspaceCleanupOutcome::Refused);
    record.error_code = Some("callback_denied".to_owned());
    assert!(record.validate().is_ok());
}

#[test]
fn unknown_allows_positive_evidence_without_releasing_the_resource() {
    let mut record = quiescent_joined();
    record.phase = WorkspaceCleanupPhase::Unknown;
    record.error_code = Some("postcheck_failed".to_owned());
    record.process_result = Some(successful_result(&record));
    assert!(record.validate().is_ok());
    assert!(record.phase.holds_resource());
    assert!(!record.phase.is_terminal());
    for code in [
        "",
        "UPPER",
        "has-dash",
        "has space",
        "é",
        "a".repeat(97).as_str(),
    ] {
        let mut invalid = record.clone();
        invalid.error_code = Some(code.to_owned());
        assert!(invalid.validate().is_err());
    }
    record.error_code = Some("x".repeat(96));
    assert!(record.validate().is_ok());
    record.error_code = None;
    assert!(record.validate().is_err());
}

#[test]
fn never_spawned_is_only_retired_claim_shape_and_grants_no_authority() {
    let record = quiescent_never_spawned();
    assert!(record.validate().is_ok());
    let mut invalid = record.clone();
    invalid.quiescence = Some(CleanupQuiescence::NeverSpawned {
        retired_claim_id: Uuid::nil(),
    });
    assert!(invalid.validate().is_err());
    let mut invalid = record.clone();
    invalid.quiescence = Some(CleanupQuiescence::NeverSpawned {
        retired_claim_id: Uuid::new_v4(),
    });
    assert!(invalid.validate().is_err());
    let mut invalid = record.clone();
    invalid.executor = Some(executor(&invalid));
    assert!(invalid.validate().is_err());
    let mut invalid = record.clone();
    invalid.executor = Some(executor(&invalid));
    invalid.arm_operation_id = Some(Uuid::new_v4());
    assert!(invalid.validate().is_err());
    let mut no_claim = record;
    no_claim.claim_id = None;
    no_claim.confirmation_operation_id = None;
    assert!(no_claim.validate().is_err());
}

#[test]
fn joined_witness_and_process_result_must_match_the_recorded_executor() {
    let mut record = quiescent_joined();
    record.quiescence = Some(joined(Uuid::new_v4()));
    assert!(record.validate().is_err());
    let mut record = quiescent_joined();
    record.quiescence = Some(joined(Uuid::nil()));
    assert!(record.validate().is_err());
    let mut record = quiescent_joined();
    record.executor = None;
    record.arm_operation_id = None;
    assert!(record.validate().is_err());
    let mut record = quiescent_joined();
    record.process_result = Some(successful_result(&record));
    record.process_result.as_mut().unwrap().executor_nonce = Uuid::new_v4();
    assert!(record.validate().is_err());
}

#[test]
fn phase_transitions_are_explicit_and_terminals_have_no_fresh_replays() {
    use WorkspaceCleanupPhase::*;
    let phases = [
        Planned, Claimed, Armed, Quiescent, Removed, Refused, Unknown,
    ];
    let expected = [
        (Planned, Claimed),
        (Claimed, Claimed),
        (Claimed, Armed),
        (Claimed, Quiescent),
        (Claimed, Unknown),
        (Armed, Quiescent),
        (Armed, Unknown),
        (Quiescent, Removed),
        (Quiescent, Refused),
        (Quiescent, Unknown),
        (Unknown, Unknown),
        (Unknown, Quiescent),
    ];
    for from in phases {
        for to in phases {
            assert_eq!(
                from.can_transition_to(to),
                expected.contains(&(from, to)),
                "{from:?} -> {to:?}"
            );
            assert_eq!(
                from.validate_transition_to(to).is_ok(),
                expected.contains(&(from, to))
            );
        }
        assert_eq!(
            from.holds_resource(),
            matches!(from, Claimed | Armed | Quiescent | Unknown)
        );
        assert_eq!(from.is_terminal(), matches!(from, Removed | Refused));
    }
    assert_eq!(
        CleanupError::InvalidInput.code(),
        "workspace_cleanup_invalid_input"
    );
    assert_eq!(
        CleanupError::InvalidTransition.code(),
        "workspace_cleanup_invalid_transition"
    );
}

#[test]
fn refused_requires_positive_quiescence_and_stable_error() {
    let mut record = quiescent_never_spawned();
    record.phase = WorkspaceCleanupPhase::Refused;
    record.outcome = Some(WorkspaceCleanupOutcome::Refused);
    record.error_code = Some("workspace_changed".to_owned());
    assert!(record.validate().is_ok());
    record.error_code = None;
    assert!(record.validate().is_err());
    record.error_code = Some("workspace_changed".to_owned());
    record.quiescence = None;
    assert!(record.validate().is_err());
}

#[test]
fn debug_never_prints_private_paths_tokens_identities_results_or_errors() {
    let mut record = quiescent_joined();
    record.phase = WorkspaceCleanupPhase::Unknown;
    record.error_code = Some("private_error_code".to_owned());
    record.process_result = Some(successful_result(&record));
    for output in [
        format!("{record:?}"),
        format!("{:?}", record.binding),
        format!("{:?}", record.executor.as_ref().unwrap()),
        format!("{:?}", record.binding.worktree_identity),
        format!("{:?}", record.process_result.as_ref().unwrap()),
        format!("{:?}", record.quiescence.as_ref().unwrap()),
    ] {
        for secret in [
            "C:/private",
            "private_ownership_token",
            "private_error_code",
            "terminal_private",
            "session_private",
            "instance_private",
            "1152921504606846993",
            "release_attempted",
            "root_reaped",
            "exit_code",
        ] {
            assert!(!output.contains(secret), "{secret}: {output}");
        }
    }
}
