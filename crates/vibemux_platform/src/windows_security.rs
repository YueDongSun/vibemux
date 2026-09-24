use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};

use crate::PlatformError;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const WINDOWS_POWERSHELL_RELATIVE_PATH: &str = "System32\\WindowsPowerShell\\v1.0\\powershell.exe";
const SECURE_PATH_ENV: &str = "VIBEMUX_SECURE_PATH";
const MAX_VERIFY_BATCH_PATHS: usize = 16;
// A cold powershell.exe start on a loaded CI runner (2-4 cores, the test
// suite running alongside) can take multiple seconds. The budget is per
// invocation and is a liveness knob, not a security check: the ACL-marker
// (ensure) path serializes its helpers in-process, while separate processes
// may run helpers concurrently (issue #6). Since ADR 025 only the WRITE
// path (`secure_user_directory`) spawns a helper; verification is native.
const HELPER_TIMEOUT: Duration = Duration::from_secs(15);
const HELPER_POLL_INTERVAL: Duration = Duration::from_millis(10);
const MAX_HELPER_OUTPUT_BYTES: usize = 128;

const SECURE_DIRECTORY_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$path = [Environment]::GetEnvironmentVariable('VIBEMUX_SECURE_PATH')
if ([string]::IsNullOrWhiteSpace($path) -or -not [IO.Directory]::Exists($path)) { [Console]::Out.Write('stage_4'); exit 4 }
$current_sid = [Security.Principal.WindowsIdentity]::GetCurrent().User
$system_sid = New-Object Security.Principal.SecurityIdentifier([Security.Principal.WellKnownSidType]::LocalSystemSid, $null)
$admin_sid = New-Object Security.Principal.SecurityIdentifier([Security.Principal.WellKnownSidType]::BuiltinAdministratorsSid, $null)
$acl = New-Object Security.AccessControl.DirectorySecurity
$acl.SetOwner($current_sid)
$acl.SetAccessRuleProtection($true, $false)
$inheritance = [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit'
$propagation = [Security.AccessControl.PropagationFlags]::None
foreach ($sid in @($current_sid, $system_sid, $admin_sid)) {
    $rule = New-Object Security.AccessControl.FileSystemAccessRule($sid, [Security.AccessControl.FileSystemRights]::FullControl, $inheritance, $propagation, [Security.AccessControl.AccessControlType]::Allow)
    [void]$acl.AddAccessRule($rule)
}
$directory = New-Object IO.DirectoryInfo($path)
$directory.SetAccessControl($acl)
$actual = $directory.GetAccessControl()
if (-not $actual.AreAccessRulesProtected) { [Console]::Out.Write('stage_5'); exit 5 }
$allowed = @{}
foreach ($sid in @($current_sid, $system_sid, $admin_sid)) { $allowed[$sid.Value] = $true }
$rules = $actual.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier])
if ($rules.Count -ne 3) { [Console]::Out.Write('stage_6'); exit 6 }
foreach ($rule in $rules) {
    if ($rule.IsInherited -or $rule.AccessControlType -ne [Security.AccessControl.AccessControlType]::Allow -or -not $allowed.ContainsKey($rule.IdentityReference.Value)) { [Console]::Out.Write('stage_7'); exit 7 }
    if (($rule.FileSystemRights -band [Security.AccessControl.FileSystemRights]::FullControl) -ne [Security.AccessControl.FileSystemRights]::FullControl) { [Console]::Out.Write('stage_8'); exit 8 }
}
[Console]::Out.Write('ok')
"#;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowsAclSummary {
    pub restricted: bool,
    pub allow_rule_count: usize,
}

pub fn secure_user_directory(path: &Path) -> Result<WindowsAclSummary, PlatformError> {
    let output = run_helper(SECURE_DIRECTORY_SCRIPT, |command| {
        command.env(SECURE_PATH_ENV, path);
    })?;
    interpret_stage_output(&output)?;
    Ok(WindowsAclSummary {
        restricted: true,
        allow_rule_count: 3,
    })
}

pub fn verify_restricted_path_acl(path: &Path) -> Result<WindowsAclSummary, PlatformError> {
    verify_restricted_path_acls(&[path])
        .map_err(|error| match error {
            PlatformError::AccessControlInvalidAt { stage, .. } => {
                PlatformError::AccessControlInvalid { stage }
            }
            other => other,
        })
        .map(|()| WindowsAclSummary {
            restricted: true,
            allow_rule_count: 3,
        })
}

/// Verify that every given path carries exactly the restricted effective
/// ACL (three Allow rules for the current user, `LOCAL_SYSTEM`, and the
/// built-in administrators; the current user holds FullControl). Since ADR
/// 025 the rule set is evaluated natively through read-only Win32 calls
/// (~ms, no helper process); failure reporting keeps the Stage 1
/// `(stage, 1-based index)` contract and carries no path, SID, or ACL
/// text (ADR 020). Verifying an empty or oversized batch fails closed
/// instead of reading as success.
pub fn verify_restricted_path_acls(paths: &[&Path]) -> Result<(), PlatformError> {
    if paths.is_empty() || paths.len() > MAX_VERIFY_BATCH_PATHS {
        return Err(PlatformError::HelperRejected);
    }
    crate::windows_acl_native::verify_paths(paths)
        .map_err(|(stage, index)| PlatformError::AccessControlInvalidAt { stage, index })
}

struct HelperOutput {
    success: bool,
    exit_code: Option<i32>,
    stdout: Vec<u8>,
}

/// Serialize helper invocations within one process. This is not a
/// correctness lock - the script never mutates shared state - it bounds
/// powershell.exe fan-out from parallel callers (test threads, concurrent
/// first-touch bootstraps), the CI-load class behind the issue #6 flake.
/// Since ADR 025 only `secure_user_directory` (the write path) spawns a
/// helper. vibemuxd holds `control_acl_init_lock` across its helper use,
/// so the order is always control_acl_init_lock -> helper_slot, never
/// reversed.
fn helper_slot() -> std::sync::MutexGuard<'static, ()> {
    static SLOT: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SLOT.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn run_helper(
    script: &str,
    configure: impl FnOnce(&mut Command),
) -> Result<HelperOutput, PlatformError> {
    use std::os::windows::process::CommandExt;

    let _slot = helper_slot();
    let powershell = system_powershell()?;
    let mut utf16 = Vec::with_capacity(script.len() * 2);
    for unit in script.encode_utf16() {
        utf16.extend_from_slice(&unit.to_le_bytes());
    }
    let mut command = Command::new(powershell);
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
        ])
        .arg(STANDARD.encode(utf16));
    configure(&mut command);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|_| PlatformError::HelperUnavailable)?;
    let deadline = Instant::now() + HELPER_TIMEOUT;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|_| PlatformError::HelperRejected)?
        {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(PlatformError::HelperTimeout);
        }
        thread::sleep(HELPER_POLL_INTERVAL);
    };
    let mut output = Vec::new();
    child
        .stdout
        .take()
        .ok_or(PlatformError::HelperRejected)?
        .take((MAX_HELPER_OUTPUT_BYTES + 1) as u64)
        .read_to_end(&mut output)
        .map_err(|_| PlatformError::HelperRejected)?;
    Ok(HelperOutput {
        success: status.success(),
        exit_code: status.code(),
        stdout: output,
    })
}

fn interpret_stage_output(output: &HelperOutput) -> Result<(), PlatformError> {
    if output.success && output.stdout == b"ok" {
        Ok(())
    } else if let Some(stage) = parse_stage(&output.stdout) {
        Err(PlatformError::AccessControlInvalid { stage })
    } else {
        Err(PlatformError::AccessControlInvalid {
            stage: output.exit_code.map_or(0, |code| code as u32),
        })
    }
}

fn parse_stage(output: &[u8]) -> Option<u32> {
    std::str::from_utf8(output)
        .ok()?
        .strip_prefix("stage_")?
        .parse()
        .ok()
}

fn system_powershell() -> Result<PathBuf, PlatformError> {
    let system_root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .ok_or(PlatformError::HelperUnavailable)?;
    let powershell = system_root.join(WINDOWS_POWERSHELL_RELATIVE_PATH);
    if powershell.is_file() {
        Ok(powershell)
    } else {
        Err(PlatformError::HelperUnavailable)
    }
}

/// Shared ACL-loosening recipe for fail-closed tests (own unit tests and,
/// behind the crate's `test_helpers` feature, other workspace crates'
/// integration tests). Not part of the production surface: it exists so
/// every loosened-ACL test uses the same reviewed icacls invocation
/// instead of re-spelling it (code-review V9 finding on issue #8).
#[cfg(any(test, feature = "test_helpers"))]
pub mod tests {
    use std::path::Path;

    /// Loosen `path` by granting `sid_and_rights` (icacls grant syntax,
    /// e.g. `*S-1-1-0:F` for Everyone on a file or
    /// `*S-1-5-32-545:(OI)(CI)F` for BUILTIN\Users on a directory; SID
    /// forms stay locale-independent). The extra ACE breaks the
    /// exactly-three-allow-rules invariant the verifiers enforce.
    /// `icacls.exe` is part of Windows system32, so there is no
    /// availability fallback: a missing icacls fails the test loudly.
    pub fn loosen_with_icacls(path: &Path, sid_and_rights: &str) {
        let status = std::process::Command::new("icacls")
            .arg(path)
            .args(["/grant", sid_and_rights])
            .status()
            .expect("icacls");
        assert!(status.success(), "icacls must succeed");
    }

    /// Remove an ACE previously added by [`loosen_with_icacls`] so shared
    /// artifacts return to their secured state for sibling tests.
    pub fn restore_with_icacls(path: &Path, sid: &str) {
        let status = std::process::Command::new("icacls")
            .arg(path)
            .arg("/remove")
            .arg(sid)
            .status()
            .expect("icacls restore");
        assert!(status.success(), "icacls restore must succeed");
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;
    use tests::loosen_with_icacls;

    #[test]
    fn protected_directory_and_inherited_file_pass_verification() {
        let temp = tempfile::tempdir().expect("temp directory");
        let protected = temp.path().join("protected_runtime");
        std::fs::create_dir(&protected).expect("protected directory");
        let summary = secure_user_directory(&protected).expect("secure directory");
        assert!(summary.restricted);
        assert_eq!(summary.allow_rule_count, 3);
        let descriptor = protected.join("control.json");
        std::fs::write(&descriptor, b"test").expect("inherited descriptor");
        assert_eq!(
            verify_restricted_path_acl(&descriptor).expect("verify inherited descriptor"),
            WindowsAclSummary {
                restricted: true,
                allow_rule_count: 3,
            }
        );
    }

    #[test]
    fn batched_verification_accepts_secured_directory_and_inherited_files() {
        let temp = tempfile::tempdir().expect("temp directory");
        let protected = temp.path().join("protected_runtime");
        std::fs::create_dir(&protected).expect("protected directory");
        secure_user_directory(&protected).expect("secure directory");
        let first = protected.join("control.json");
        std::fs::write(&first, b"descriptor").expect("first artifact");
        let second = protected.join("writer.lock");
        std::fs::write(&second, b"lock").expect("second artifact");
        verify_restricted_path_acls(&[&protected, &first, &second]).expect("batched verification");
    }

    #[test]
    fn batched_verification_reports_failing_index_and_stage() {
        let temp = tempfile::tempdir().expect("temp directory");
        let protected = temp.path().join("protected_runtime");
        std::fs::create_dir(&protected).expect("protected directory");
        secure_user_directory(&protected).expect("secure directory");
        let first = protected.join("control.json");
        std::fs::write(&first, b"descriptor").expect("first artifact");
        let second = protected.join("writer.lock");
        std::fs::write(&second, b"lock").expect("second artifact");
        // Broaden one artifact with an extra ACE for BUILTIN\Users: the
        // effective rule count stops being exactly three, so the batch must
        // name path index 3 at stage 7.
        loosen_with_icacls(&second, "*S-1-5-32-545:F");
        assert_eq!(
            verify_restricted_path_acls(&[&protected, &first, &second]),
            Err(PlatformError::AccessControlInvalidAt { stage: 7, index: 3 })
        );
    }

    #[test]
    fn batched_verification_reports_a_missing_path_index() {
        let temp = tempfile::tempdir().expect("temp directory");
        let protected = temp.path().join("protected_runtime");
        std::fs::create_dir(&protected).expect("protected directory");
        secure_user_directory(&protected).expect("secure directory");
        let first = protected.join("control.json");
        std::fs::write(&first, b"descriptor").expect("first artifact");
        let missing = protected.join("missing_artifact");
        assert_eq!(
            verify_restricted_path_acls(&[&protected, &first, &missing]),
            Err(PlatformError::AccessControlInvalidAt { stage: 4, index: 3 })
        );
    }

    #[test]
    fn batched_verification_rejects_empty_and_oversized_batches() {
        assert_eq!(
            verify_restricted_path_acls(&[]),
            Err(PlatformError::HelperRejected)
        );
        let placeholders: Vec<PathBuf> = (0..=MAX_VERIFY_BATCH_PATHS)
            .map(|_| PathBuf::from("unused"))
            .collect();
        let references: Vec<&Path> = placeholders.iter().map(|path| path.as_path()).collect();
        assert_eq!(
            verify_restricted_path_acls(&references),
            Err(PlatformError::HelperRejected)
        );
    }

    #[test]
    fn single_path_verification_keeps_its_stage_only_error_shape() {
        let temp = tempfile::tempdir().expect("temp directory");
        let protected = temp.path().join("protected_runtime");
        std::fs::create_dir(&protected).expect("protected directory");
        secure_user_directory(&protected).expect("secure directory");
        let broadened = protected.join("writer.lock");
        std::fs::write(&broadened, b"lock").expect("artifact");
        loosen_with_icacls(&broadened, "*S-1-5-32-545:F");
        assert_eq!(
            verify_restricted_path_acl(&broadened),
            Err(PlatformError::AccessControlInvalid { stage: 7 })
        );
    }
}
