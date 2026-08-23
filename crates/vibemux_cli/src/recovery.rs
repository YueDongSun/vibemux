//! Read-only stale-runtime inspection and confirmation-bound artifact recovery.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sysinfo::{Pid, ProcessesToUpdate, System};
use vibemuxd::{
    control::{
        ControlArtifactSnapshot, ControlClient, ControlEndpointKind, inspect_control_artifact,
    },
    process::DaemonPaths,
    recovery::{WriterLockSnapshot, inspect_writer_lock},
};

use crate::DaemonCliError;

const INSPECTION_HEALTH_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryStatus {
    NotNeeded,
    Running,
    Blocked,
    Recoverable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RecoveryInspection {
    pub ok: bool,
    pub status: RecoveryStatus,
    pub reason_code: String,
    pub descriptor_present: bool,
    pub descriptor_valid: bool,
    pub writer_lock_present: bool,
    pub writer_lock_valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_kind: Option<ControlEndpointKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process_present: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmation: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RecoveryOutcome {
    pub ok: bool,
    pub status: String,
    pub process_id: u32,
    pub descriptor_removed: bool,
    pub writer_lock_removed: bool,
}

struct RecoveryPlan {
    inspection: RecoveryInspection,
    control: Option<ControlArtifactSnapshot>,
    writer_lock: Option<WriterLockSnapshot>,
}

pub async fn inspect_runtime(paths: &DaemonPaths) -> Result<RecoveryInspection, DaemonCliError> {
    Ok(build_recovery_plan(paths).await?.inspection)
}

pub async fn recover_runtime(
    paths: &DaemonPaths,
    confirmation: &str,
) -> Result<RecoveryOutcome, DaemonCliError> {
    if confirmation.len() != 64 || !confirmation.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(DaemonCliError::RecoveryConfirmationMismatch);
    }
    let plan = build_recovery_plan(paths).await?;
    if plan.inspection.status != RecoveryStatus::Recoverable {
        return Err(DaemonCliError::RecoveryBlocked {
            reason_code: plan.inspection.reason_code,
        });
    }
    let expected =
        plan.inspection
            .confirmation
            .as_deref()
            .ok_or_else(|| DaemonCliError::RecoveryBlocked {
                reason_code: "recovery_confirmation_unavailable".to_string(),
            })?;
    if confirmation != expected {
        return Err(DaemonCliError::RecoveryConfirmationMismatch);
    }
    let process_id = plan
        .inspection
        .process_id
        .ok_or_else(|| DaemonCliError::RecoveryBlocked {
            reason_code: "recovery_owner_pid_unavailable".to_string(),
        })?;
    if process_is_present(process_id) {
        return Err(DaemonCliError::RecoveryBlocked {
            reason_code: "recovery_owner_process_present".to_string(),
        });
    }

    let descriptor_removed = if let Some(control) = plan.control {
        control
            .remove_if_unchanged()
            .map_err(|error| DaemonCliError::RecoveryArtifact {
                code: error.code().to_string(),
            })?;
        true
    } else {
        false
    };
    let writer_lock_removed = if let Some(writer_lock) = plan.writer_lock {
        writer_lock
            .remove_if_unchanged()
            .map_err(|error| DaemonCliError::RecoveryArtifact {
                code: error.code().to_string(),
            })?;
        true
    } else {
        false
    };
    Ok(RecoveryOutcome {
        ok: true,
        status: "recovered".to_string(),
        process_id,
        descriptor_removed,
        writer_lock_removed,
    })
}

async fn build_recovery_plan(paths: &DaemonPaths) -> Result<RecoveryPlan, DaemonCliError> {
    let runtime_present = match std::fs::symlink_metadata(paths.runtime_dir()) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => {
            return Err(DaemonCliError::RecoveryArtifact {
                code: "recovery_artifact_inaccessible".to_string(),
            });
        }
    };
    if !runtime_present {
        return Ok(empty_plan("runtime_artifacts_absent"));
    }
    paths.validate_runtime_dir()?;

    let descriptor_present = path_present(paths.descriptor_path())?;
    let writer_lock_present = path_present(paths.writer_lock_path())?;
    if !descriptor_present && !writer_lock_present {
        return Ok(empty_plan("runtime_artifacts_absent"));
    }

    if descriptor_present {
        if let Ok(client) = ControlClient::from_descriptor(paths.descriptor_path()) {
            if let Ok(health) = client.health_with_deadline(INSPECTION_HEALTH_TIMEOUT).await {
                let control_info = inspect_control_artifact(paths.descriptor_path())
                    .ok()
                    .flatten()
                    .map(|snapshot| snapshot.info());
                let writer_lock_valid = inspect_writer_lock(paths.writer_lock_path())
                    .ok()
                    .flatten()
                    .is_some();
                return Ok(RecoveryPlan {
                    inspection: RecoveryInspection {
                        ok: true,
                        status: RecoveryStatus::Running,
                        reason_code: "daemon_running".to_string(),
                        descriptor_present: true,
                        descriptor_valid: control_info.is_some(),
                        writer_lock_present,
                        writer_lock_valid,
                        protocol_version: control_info.map(|info| info.protocol_version),
                        endpoint_kind: control_info.map(|info| info.endpoint_kind),
                        process_id: Some(health.process_id),
                        process_present: Some(true),
                        confirmation: None,
                    },
                    control: None,
                    writer_lock: None,
                });
            }
        }
    }

    let control = match inspect_control_artifact(paths.descriptor_path()) {
        Ok(control) => control,
        Err(error) => {
            return Ok(blocked_plan(
                descriptor_present,
                false,
                writer_lock_present,
                false,
                error.code(),
            ));
        }
    };
    let writer_lock = match inspect_writer_lock(paths.writer_lock_path()) {
        Ok(writer_lock) => writer_lock,
        Err(error) => {
            return Ok(blocked_plan(
                descriptor_present,
                control.is_some(),
                writer_lock_present,
                false,
                error.code(),
            ));
        }
    };
    let control_info = control.as_ref().map(ControlArtifactSnapshot::info);
    let lock_info = writer_lock.as_ref().map(WriterLockSnapshot::info);
    let process_id = match (control_info, lock_info) {
        (Some(control), Some(lock)) if control.process_id != lock.process_id => {
            return Ok(blocked_plan(
                true,
                true,
                true,
                true,
                "recovery_owner_pid_mismatch",
            ));
        }
        (Some(control), _) => control.process_id,
        (_, Some(lock)) => lock.process_id,
        (None, None) => return Ok(empty_plan("runtime_artifacts_absent")),
    };
    let process_present = process_is_present(process_id);
    if process_present {
        return Ok(RecoveryPlan {
            inspection: RecoveryInspection {
                ok: true,
                status: RecoveryStatus::Blocked,
                reason_code: "recovery_owner_process_present".to_string(),
                descriptor_present,
                descriptor_valid: control.is_some(),
                writer_lock_present,
                writer_lock_valid: writer_lock.is_some(),
                protocol_version: control_info.map(|info| info.protocol_version),
                endpoint_kind: control_info.map(|info| info.endpoint_kind),
                process_id: Some(process_id),
                process_present: Some(true),
                confirmation: None,
            },
            control,
            writer_lock,
        });
    }
    let confirmation = recovery_confirmation(control.as_ref(), writer_lock.as_ref());
    Ok(RecoveryPlan {
        inspection: RecoveryInspection {
            ok: true,
            status: RecoveryStatus::Recoverable,
            reason_code: "recovery_ready".to_string(),
            descriptor_present,
            descriptor_valid: control.is_some(),
            writer_lock_present,
            writer_lock_valid: writer_lock.is_some(),
            protocol_version: control_info.map(|info| info.protocol_version),
            endpoint_kind: control_info.map(|info| info.endpoint_kind),
            process_id: Some(process_id),
            process_present: Some(false),
            confirmation: Some(confirmation),
        },
        control,
        writer_lock,
    })
}

fn empty_plan(reason_code: &str) -> RecoveryPlan {
    RecoveryPlan {
        inspection: RecoveryInspection {
            ok: true,
            status: RecoveryStatus::NotNeeded,
            reason_code: reason_code.to_string(),
            descriptor_present: false,
            descriptor_valid: false,
            writer_lock_present: false,
            writer_lock_valid: false,
            protocol_version: None,
            endpoint_kind: None,
            process_id: None,
            process_present: None,
            confirmation: None,
        },
        control: None,
        writer_lock: None,
    }
}

fn blocked_plan(
    descriptor_present: bool,
    descriptor_valid: bool,
    writer_lock_present: bool,
    writer_lock_valid: bool,
    reason_code: &str,
) -> RecoveryPlan {
    RecoveryPlan {
        inspection: RecoveryInspection {
            ok: true,
            status: RecoveryStatus::Blocked,
            reason_code: reason_code.to_string(),
            descriptor_present,
            descriptor_valid,
            writer_lock_present,
            writer_lock_valid,
            protocol_version: None,
            endpoint_kind: None,
            process_id: None,
            process_present: None,
            confirmation: None,
        },
        control: None,
        writer_lock: None,
    }
}

fn recovery_confirmation(
    control: Option<&ControlArtifactSnapshot>,
    writer_lock: Option<&WriterLockSnapshot>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"vibemux-recovery-plan-v1\0");
    match control {
        Some(control) => {
            hasher.update([1]);
            hasher.update(control.binding_digest());
        }
        None => hasher.update([0]),
    }
    match writer_lock {
        Some(writer_lock) => {
            hasher.update([1]);
            hasher.update(writer_lock.binding_digest());
        }
        None => hasher.update([0]),
    }
    let digest: [u8; 32] = hasher.finalize().into();
    hex(&digest)
}

fn process_is_present(process_id: u32) -> bool {
    let pid = Pid::from_u32(process_id);
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    system.process(pid).is_some()
}

fn path_present(path: &Path) -> Result<bool, DaemonCliError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(DaemonCliError::RecoveryArtifact {
            code: "recovery_artifact_inaccessible".to_string(),
        }),
    }
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use vibemuxd::control::DaemonControlServer;

    use super::*;

    const ABSENT_PROCESS_ID: u32 = u32::MAX;

    #[test]
    fn current_process_is_present() {
        assert!(process_is_present(std::process::id()));
    }

    #[test]
    fn confirmation_is_stable_and_fixed_length() {
        let first = recovery_confirmation(None, None);
        let second = recovery_confirmation(None, None);
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_daemon_is_never_recoverable() {
        let temp = tempfile::tempdir().expect("temp project");
        let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
        paths.ensure_runtime_dir().expect("runtime directory");
        let server = DaemonControlServer::start(paths.database_path(), paths.runtime_dir())
            .await
            .expect("control server");

        let inspection = inspect_runtime(&paths).await.expect("inspect live daemon");
        assert_eq!(inspection.status, RecoveryStatus::Running);
        assert!(inspection.confirmation.is_none());
        assert!(matches!(
            recover_runtime(&paths, &"0".repeat(64)).await,
            Err(DaemonCliError::RecoveryBlocked { .. })
        ));
        assert!(paths.descriptor_path().exists());
        assert!(paths.writer_lock_path().exists());
        server.shutdown().await.expect("shutdown server");
    }

    #[tokio::test]
    async fn lock_only_recovery_requires_matching_confirmation_and_preserves_databases() {
        let temp = tempfile::tempdir().expect("temp project");
        let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
        paths.ensure_runtime_dir().expect("runtime directory");
        let lock_contents = format!("{ABSENT_PROCESS_ID}-123456789-7");
        std::fs::write(paths.writer_lock_path(), &lock_contents).expect("stale writer lock");
        std::fs::write(paths.database_path(), b"rust-database-sentinel")
            .expect("Rust database sentinel");
        std::fs::write(paths.python_database_path(), b"python-database-sentinel")
            .expect("Python database sentinel");

        let inspection = inspect_runtime(&paths).await.expect("inspect stale lock");
        assert_eq!(inspection.status, RecoveryStatus::Recoverable);
        let confirmation = inspection.confirmation.expect("recovery confirmation");
        assert_eq!(
            recover_runtime(&paths, &"0".repeat(64))
                .await
                .expect_err("wrong confirmation must fail"),
            DaemonCliError::RecoveryConfirmationMismatch
        );
        assert_eq!(
            std::fs::read_to_string(paths.writer_lock_path()).expect("unchanged lock"),
            lock_contents
        );

        let outcome = recover_runtime(&paths, &confirmation)
            .await
            .expect("recover stale lock");
        assert!(!outcome.descriptor_removed);
        assert!(outcome.writer_lock_removed);
        assert!(!paths.writer_lock_path().exists());
        assert_eq!(
            std::fs::read(paths.database_path()).expect("Rust database after recovery"),
            b"rust-database-sentinel"
        );
        assert_eq!(
            std::fs::read(paths.python_database_path()).expect("Python database after recovery"),
            b"python-database-sentinel"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn descriptor_only_recovery_and_pid_mismatch_fail_closed() {
        let temp = tempfile::tempdir().expect("temp project");
        let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
        paths.ensure_runtime_dir().expect("runtime directory");
        let server = DaemonControlServer::start(paths.database_path(), paths.runtime_dir())
            .await
            .expect("control server");
        let descriptor_bytes = std::fs::read(paths.descriptor_path()).expect("live descriptor");
        server.shutdown().await.expect("shutdown server");

        let mut descriptor: serde_json::Value =
            serde_json::from_slice(&descriptor_bytes).expect("decode descriptor");
        descriptor["process_id"] = json!(ABSENT_PROCESS_ID);
        std::fs::write(
            paths.descriptor_path(),
            serde_json::to_vec(&descriptor).expect("encode stale descriptor"),
        )
        .expect("stale descriptor");
        let inspection = inspect_runtime(&paths)
            .await
            .expect("inspect descriptor only");
        assert_eq!(inspection.status, RecoveryStatus::Recoverable);
        let encoded_inspection =
            serde_json::to_string(&inspection).expect("encode redacted inspection");
        let project_path = temp.path().to_string_lossy();
        assert!(!encoded_inspection.contains(project_path.as_ref()));
        assert!(!encoded_inspection.contains("vibemux_rust.sqlite3"));
        assert!(!encoded_inspection.contains("\"token\""));
        assert!(!encoded_inspection.contains("vibemux_control_"));
        assert!(!encoded_inspection.contains(r"\\.\pipe\vibemux_"));
        let confirmation = inspection.confirmation.expect("descriptor confirmation");
        let outcome = recover_runtime(&paths, &confirmation)
            .await
            .expect("recover descriptor only");
        assert!(outcome.descriptor_removed);
        assert!(!outcome.writer_lock_removed);
        assert!(!paths.descriptor_path().exists());

        std::fs::write(
            paths.descriptor_path(),
            serde_json::to_vec(&descriptor).expect("encode mismatch descriptor"),
        )
        .expect("mismatch descriptor");
        std::fs::write(
            paths.writer_lock_path(),
            format!("{}-123456790-8", ABSENT_PROCESS_ID - 1),
        )
        .expect("mismatch lock");
        let inspection = inspect_runtime(&paths).await.expect("inspect PID mismatch");
        assert_eq!(inspection.status, RecoveryStatus::Blocked);
        assert_eq!(inspection.reason_code, "recovery_owner_pid_mismatch");
        assert!(inspection.confirmation.is_none());
        assert!(paths.descriptor_path().exists());
        assert!(paths.writer_lock_path().exists());
    }

    #[tokio::test]
    async fn changed_artifact_invalidates_inspection_confirmation() {
        let temp = tempfile::tempdir().expect("temp project");
        let paths = DaemonPaths::from_project_root(temp.path()).expect("daemon paths");
        paths.ensure_runtime_dir().expect("runtime directory");
        std::fs::write(
            paths.writer_lock_path(),
            format!("{ABSENT_PROCESS_ID}-123456789-7"),
        )
        .expect("stale writer lock");
        let inspection = inspect_runtime(&paths).await.expect("initial inspection");
        let confirmation = inspection.confirmation.expect("initial confirmation");
        std::fs::write(
            paths.writer_lock_path(),
            format!("{ABSENT_PROCESS_ID}-123456790-8"),
        )
        .expect("replacement writer lock");

        assert_eq!(
            recover_runtime(&paths, &confirmation)
                .await
                .expect_err("changed artifact must fail"),
            DaemonCliError::RecoveryConfirmationMismatch
        );
        assert!(paths.writer_lock_path().exists());
    }
}
