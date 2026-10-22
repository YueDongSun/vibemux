# Installs the built VibeMux command-line tools into one directory so that
# `vibemuxctl` can be run by name. The directory keeps the binaries together on
# purpose: `vibemuxctl daemon start` looks for `vibemuxd` beside itself, and the
# daemon looks for the launch trampoline and decision peer beside itself.
#
# What this does NOT do: it never starts a daemon, never writes operator
# configuration, never touches the machine-wide PATH, and never replaces the
# Python `vibemux` command (a different name). Changing the user PATH needs the
# explicit -add_to_user_path switch.
#
#   .\scripts\install_local_tools.ps1 -build -dry_run
#   .\scripts\install_local_tools.ps1 -build -add_to_user_path
param(
    # Where the tools are copied. Created when missing.
    [string] $install_directory = "",
    # Where the built binaries are read from. Defaults to the release output of
    # this repository (or of CARGO_TARGET_DIR when it is set).
    [string] $source_directory = "",
    # Run the release build of the listed binaries first.
    [switch] $build,
    # Append the install directory to the user PATH (never the machine PATH).
    [switch] $add_to_user_path,
    # Report what would be copied and changed, and change nothing.
    [switch] $dry_run
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# Frequently changed settings: which binaries are installed.
$required_binary_names = @("vibemuxctl", "vibemuxd")
$companion_binary_names = @(
    "vibemux_launch_trampoline",
    "vibemux_probe",
    "vibemux_decision_peer",
    "vibemux_terminal_observer"
)
$build_arguments = @(
    "build", "--release", "--locked",
    "-p", "vibemuxd", "--bin", "vibemuxd", "--bin", "vibemux_launch_trampoline",
    "-p", "vibemux_cli", "--bin", "vibemuxctl",
    "-p", "vibemux_probe", "--bin", "vibemux_probe",
    "-p", "vibemux_decision_peer", "--bin", "vibemux_decision_peer",
    "-p", "vibemux_terminal_observer", "--bin", "vibemux_terminal_observer"
)

function get_normalized_directory([string] $path) {
    return [IO.Path]::GetFullPath($path).TrimEnd("\")
}

$repository_root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
if (-not $install_directory) {
    if (-not $env:LOCALAPPDATA) {
        throw "LOCALAPPDATA is not set; pass -install_directory"
    }
    $install_directory = Join-Path $env:LOCALAPPDATA "VibeMux\bin"
}
if (-not $source_directory) {
    $target_directory = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repository_root "target" }
    $source_directory = Join-Path $target_directory "release"
}
$normalized_install = get_normalized_directory $install_directory
$normalized_source = get_normalized_directory $source_directory
if ($normalized_install -ieq $normalized_source) {
    throw "install_directory and source_directory must differ"
}

if ($build) {
    if ($dry_run) {
        Write-Host "dry run: would run: cargo $($build_arguments -join ' ')"
    } else {
        Push-Location $repository_root
        try {
            & cargo @build_arguments
            if ($LASTEXITCODE -ne 0) {
                throw "release build failed"
            }
        } finally {
            Pop-Location
        }
    }
}

$installed = @()
$skipped = @()
foreach ($name in ($required_binary_names + $companion_binary_names)) {
    $source_path = Join-Path $normalized_source "$name.exe"
    if (-not (Test-Path -LiteralPath $source_path -PathType Leaf)) {
        if ($required_binary_names -contains $name) {
            throw "required binary is missing: $source_path (run with -build, or pass -source_directory)"
        }
        $skipped += $name
        continue
    }
    $installed += $name
}

if ($dry_run) {
    Write-Host "dry run: would copy $($installed -join ', ') from $normalized_source to $normalized_install"
} else {
    New-Item -ItemType Directory -Force -Path $normalized_install | Out-Null
    foreach ($name in $installed) {
        $destination = Join-Path $normalized_install "$name.exe"
        try {
            Copy-Item -LiteralPath (Join-Path $normalized_source "$name.exe") -Destination $destination -Force
        } catch {
            throw "could not replace $destination; stop a running daemon with 'vibemuxctl daemon stop --project-root <root>' and retry: $($_.Exception.Message)"
        }
    }
}

$path_status = "unchanged"
$user_path = [Environment]::GetEnvironmentVariable("Path", "User")
$user_entries = @()
if ($user_path) {
    $user_entries = @($user_path -split ";" | Where-Object { $_ })
}
$already_on_path = $false
foreach ($entry in $user_entries) {
    try {
        if ((get_normalized_directory $entry) -ieq $normalized_install) {
            $already_on_path = $true
        }
    } catch {
        # An entry that is not a valid path cannot match; leave it alone.
    }
}
if ($already_on_path) {
    $path_status = "already_present"
} elseif ($add_to_user_path) {
    if ($dry_run) {
        $path_status = "would_add"
    } else {
        $new_user_path = (@($user_entries) + $normalized_install) -join ";"
        [Environment]::SetEnvironmentVariable("Path", $new_user_path, "User")
        $path_status = "added"
    }
} else {
    $path_status = "not_added"
}

if (-not $dry_run) {
    $installed_ctl = Join-Path $normalized_install "vibemuxctl.exe"
    $version = (& $installed_ctl --version).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw "installed vibemuxctl did not report a version"
    }
} else {
    $version = "not_run"
}

[pscustomobject]@{
    install_directory = $normalized_install
    source_directory = $normalized_source
    dry_run = [bool] $dry_run
    installed = $installed
    skipped_missing_companions = $skipped
    user_path = $path_status
    vibemuxctl_version = $version
} | ConvertTo-Json -Compress

if ($path_status -eq "not_added") {
    Write-Host "vibemuxctl is not on PATH yet. Re-run with -add_to_user_path, or add this directory yourself: $normalized_install"
} elseif ($path_status -eq "added") {
    Write-Host "Added to the user PATH. Open a new terminal for it to take effect."
}
