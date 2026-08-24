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
const HELPER_TIMEOUT: Duration = Duration::from_secs(5);
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

const VERIFY_PATH_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$stage = 10
try {
    $path = [Environment]::GetEnvironmentVariable('VIBEMUX_SECURE_PATH')
    if ([string]::IsNullOrWhiteSpace($path) -or -not [IO.File]::Exists($path) -and -not [IO.Directory]::Exists($path)) { [Console]::Out.Write('stage_4'); exit 4 }
    $stage = 11
    $current_sid = [Security.Principal.WindowsIdentity]::GetCurrent().User
    $system_sid = New-Object Security.Principal.SecurityIdentifier([Security.Principal.WellKnownSidType]::LocalSystemSid, $null)
    $admin_sid = New-Object Security.Principal.SecurityIdentifier([Security.Principal.WellKnownSidType]::BuiltinAdministratorsSid, $null)
    $allowed = @{}
    foreach ($sid in @($current_sid, $system_sid, $admin_sid)) { $allowed[$sid.Value] = $true }
    $stage = 12
    if ([IO.Directory]::Exists($path)) {
        $actual = (New-Object IO.DirectoryInfo($path)).GetAccessControl()
    } else {
        $actual = (New-Object IO.FileInfo($path)).GetAccessControl()
    }
    $stage = 13
    $rules = $actual.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier])
    if ($rules.Count -ne 3) { [Console]::Out.Write('stage_7'); exit 7 }
    $current_full_control = $false
    $stage = 14
    foreach ($rule in $rules) {
        if ($rule.AccessControlType -ne [Security.AccessControl.AccessControlType]::Allow -or -not $allowed.ContainsKey($rule.IdentityReference.Value)) { [Console]::Out.Write('stage_5'); exit 5 }
        if ($rule.IdentityReference.Value -eq $current_sid.Value -and ($rule.FileSystemRights -band [Security.AccessControl.FileSystemRights]::FullControl) -eq [Security.AccessControl.FileSystemRights]::FullControl) { $current_full_control = $true }
    }
    if (-not $current_full_control) { [Console]::Out.Write('stage_6'); exit 6 }
    [Console]::Out.Write('ok')
} catch {
    [Console]::Out.Write('stage_' + $stage)
    exit 9
}
"#;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowsAclSummary {
    pub restricted: bool,
    pub allow_rule_count: usize,
}

pub fn secure_user_directory(path: &Path) -> Result<WindowsAclSummary, PlatformError> {
    run_fixed_script(SECURE_DIRECTORY_SCRIPT, path)?;
    Ok(WindowsAclSummary {
        restricted: true,
        allow_rule_count: 3,
    })
}

pub fn verify_restricted_path_acl(path: &Path) -> Result<WindowsAclSummary, PlatformError> {
    run_fixed_script(VERIFY_PATH_SCRIPT, path)?;
    Ok(WindowsAclSummary {
        restricted: true,
        allow_rule_count: 3,
    })
}

fn run_fixed_script(script: &str, path: &Path) -> Result<(), PlatformError> {
    use std::os::windows::process::CommandExt;

    let powershell = system_powershell()?;
    let mut utf16 = Vec::with_capacity(script.len() * 2);
    for unit in script.encode_utf16() {
        utf16.extend_from_slice(&unit.to_le_bytes());
    }
    let mut child = Command::new(powershell)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
        ])
        .arg(STANDARD.encode(utf16))
        .env(SECURE_PATH_ENV, path)
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
    if status.success() && output == b"ok" {
        Ok(())
    } else if let Some(stage) = parse_stage(&output) {
        Err(PlatformError::AccessControlInvalid { stage })
    } else {
        Err(PlatformError::AccessControlInvalid {
            stage: status.code().map_or(0, |code| code as u32),
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
