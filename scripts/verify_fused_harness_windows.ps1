#Requires -Version 5.1
<#
.SYNOPSIS
  Native Windows acceptance frame for the VibeMux cloud harness integration.

.DESCRIPTION
  Checks the Windows toolchain, builds the daemon/CLI/probe/trampoline into a
  managed build slot, runs the Windows-only Rust fixtures, and invokes the
  cross-project verification frame for the offline cases. No vendor CLI, model
  call, provider token, or real user state directory is used.

  Every native command checks LASTEXITCODE and raises a terminating error on
  failure. The script prints one JSON summary and exits non-zero if any step
  failed. It does not start a daemon directly; C01 starts and stops its own
  task-owned daemon.

.PARAMETER HubRoot
  Personal Dev Hub checkout. When supplied, C01 runs against the real Hub
  adapter. Otherwise C01 is skipped and recorded as not_run.

.PARAMETER SkipBuild
  Reuse an existing managed slot instead of rebuilding.

.PARAMETER SkipWindowsTests
  Skip the Windows-only `vibemux_platform`/`vibemuxd`/`vibemux_cli` test runs.

.PARAMETER Cleanup
  After the run, plan and apply cleanup for this task's managed slots. Only
  idle, registered, task-owned targets are removed.

.PARAMETER LiveVendor
  Accepted only to make the refusal explicit. Live vendor smoke is not
  implemented here and requires separate authorization and budget.

.EXAMPLE
  .\scripts\verify_fused_harness_windows.ps1 -HubRoot C:\src\personal_dev_hub
#>
[CmdletBinding()]
param(
    [string]$RepoRoot = (Split-Path -Parent $PSScriptRoot),
    [string]$StateRoot = (Join-Path $env:LOCALAPPDATA 'VibeMux\build-storage'),
    [string]$TaskId = 'cloud-harness-windows',
    [string]$HubRoot = '',
    [switch]$SkipBuild,
    [switch]$SkipWindowsTests,
    [switch]$Cleanup,
    [switch]$LiveVendor
)

$ErrorActionPreference = 'Stop'
$steps = New-Object System.Collections.ArrayList

function Add-Step {
    param(
        [string]$Name,
        [string]$Status,
        [int]$DurationMs = 0,
        [string]$Detail = ''
    )
    [void]$steps.Add([pscustomobject]@{
        name        = $Name
        status      = $Status
        duration_ms = $DurationMs
        detail      = $Detail
    })
    Write-Host ("[{0}] {1} {2}" -f $Status, $Name, $Detail)
}

function Invoke-Native {
    param(
        [string]$Name,
        [scriptblock]$Block
    )
    $start = [System.Diagnostics.Stopwatch]::StartNew()
    & $Block
    $code = $LASTEXITCODE
    $start.Stop()
    if ($code -ne 0) {
        Add-Step -Name $Name -Status 'failed' -DurationMs $start.ElapsedMilliseconds -Detail ("exit {0}" -f $code)
        throw ("{0} failed with exit code {1}" -f $Name, $code)
    }
    Add-Step -Name $Name -Status 'passed' -DurationMs $start.ElapsedMilliseconds
}

function Invoke-Python {
    param(
        [string]$PythonExe,
        [string[]]$PythonPrefix,
        [string[]]$Arguments
    )
    & $PythonExe @PythonPrefix @Arguments
}

function Write-Summary {
    param([string]$Status, [string]$ReportPath)
    $summary = [pscustomobject]@{
        schema_version = 1
        status         = $Status
        os             = [System.Environment]::OSVersion.VersionString
        repo_root      = $RepoRoot
        state_root     = $StateRoot
        task_id        = $TaskId
        hub_root       = $(if ($HubRoot) { $HubRoot } else { $null })
        steps          = $steps
        live_vendor    = 'not_run: requires explicit authorization, configuration, and budget'
    }
    $json = $summary | ConvertTo-Json -Depth 6
    if ($ReportPath) {
        $directory = Split-Path -Parent $ReportPath
        if ($directory -and -not (Test-Path -LiteralPath $directory)) {
            New-Item -ItemType Directory -Path $directory | Out-Null
        }
        Set-Content -LiteralPath $ReportPath -Value $json -Encoding UTF8
    }
    Write-Output $json
}

function Resolve-Python {
    $py = Get-Command py -ErrorAction SilentlyContinue
    if ($py) {
        $version = & py -3.12 -c "import sys; print(sys.version_info[:2])" 2>$null
        if ($LASTEXITCODE -eq 0 -and $version -match '\(3, 12\)') {
            return @('py', '-3.12')
        }
    }
    $python = Get-Command python -ErrorAction SilentlyContinue
    if ($python) {
        $version = & python -c "import sys; print(sys.version_info[:2])" 2>$null
        if ($LASTEXITCODE -eq 0 -and $version -match '\(3, 12\)') {
            return @('python')
        }
    }
    throw 'Python 3.12 is required (py -3.12 or python).'
}

try {
    if (-not $IsWindows -and $env:OS -ne 'Windows_NT') {
        throw 'This acceptance script runs on native Windows only.'
    }
    if (-not (Test-Path -LiteralPath $RepoRoot)) {
        throw "Repository root not found: $RepoRoot"
    }
    $RepoRoot = (Resolve-Path -LiteralPath $RepoRoot).Path
    $python = Resolve-Python
    $pythonExe = $python[0]
    $pythonPrefix = @()
    if ($python.Count -gt 1) {
        $pythonPrefix = $python[1..($python.Count - 1)]
    }

    foreach ($tool in @('cargo', 'rustc', 'node')) {
        $found = Get-Command $tool -ErrorAction SilentlyContinue
        if (-not $found) {
            throw "Required tool not found on PATH: $tool"
        }
    }
    $rustVersion = (& rustc --version) -join ' '
    $nodeVersion = (& node --version) -join ' '
    Add-Step -Name 'toolchain' -Status 'passed' -Detail ("{0}; node {1}" -f $rustVersion, $nodeVersion)

    $buildStorage = Join-Path $RepoRoot 'scripts\build_storage.py'
    if (-not (Test-Path -LiteralPath $buildStorage)) {
        throw "build_storage.py not found at $buildStorage"
    }

    if (-not $SkipBuild) {
        Invoke-Native -Name 'managed-build' -Block {
            Invoke-Python -PythonExe $pythonExe -PythonPrefix $pythonPrefix -Arguments @(
                $buildStorage,
                '--state-root', $StateRoot,
                'run', '--slot', 'core', '--task-id', $TaskId,
                '--budget-bytes', '20G', '--reserve-bytes', '8G',
                '--',
                'cargo', 'build', '--locked',
                '-p', 'vibemuxd', '--bin', 'vibemuxd', '--bin', 'vibemux_launch_trampoline',
                '-p', 'vibemux_cli', '--bin', 'vibemuxctl',
                '-p', 'vibemux_probe', '--bin', 'vibemux_probe'
            )
        }
    } else {
        Add-Step -Name 'managed-build' -Status 'skipped' -Detail 'SkipBuild'
    }

    if (-not $SkipWindowsTests) {
        $windowsPackages = @('vibemux_platform', 'vibemuxd', 'vibemux_cli')
        foreach ($package in $windowsPackages) {
            Invoke-Native -Name ("test-" + $package) -Block {
                Invoke-Python -PythonExe $pythonExe -PythonPrefix $pythonPrefix -Arguments @(
                    $buildStorage,
                    '--state-root', $StateRoot,
                    'run', '--slot', 'core', '--task-id', $TaskId,
                    '--budget-bytes', '20G', '--reserve-bytes', '8G',
                    '--',
                    'cargo', 'test', '--locked', '-p', $package, '--all-features'
                )
            }
        }
    } else {
        Add-Step -Name 'windows-tests' -Status 'skipped' -Detail 'SkipWindowsTests'
    }

    $vibemuxctl = Join-Path $StateRoot 'slots\core\target\debug\vibemuxctl.exe'
    $cases = 'A1,A2'
    if ($HubRoot) {
        if (-not (Test-Path -LiteralPath $HubRoot)) {
            throw "Hub root not found: $HubRoot"
        }
        $cases = 'C01,A1,A2'
    }
    Invoke-Native -Name 'cross-project-frame' -Block {
        $frameArguments = @(
            (Join-Path $RepoRoot 'scripts\verify_fused_harness_cloud.py'),
            '--cases', $cases,
            '--state-root', (Join-Path $StateRoot 'verify-windows')
        )
        if ($HubRoot) {
            $frameArguments += @('--hub-root', $HubRoot)
        }
        if (Test-Path -LiteralPath $vibemuxctl) {
            $frameArguments += @('--vibemuxctl', $vibemuxctl)
        }
        Invoke-Python -PythonExe $pythonExe -PythonPrefix $pythonPrefix -Arguments $frameArguments
    }

    if ($LiveVendor) {
        Add-Step -Name 'live-vendor' -Status 'not_run' -Detail 'requires explicit authorization, configured routes, and a bounded budget'
    }

    if ($Cleanup) {
        Invoke-Native -Name 'cleanup-plan' -Block {
            Invoke-Python -PythonExe $pythonExe -PythonPrefix $pythonPrefix -Arguments @(
                $buildStorage, '--state-root', $StateRoot, 'plan-clean', '--task-id', $TaskId
            )
        }
        Add-Step -Name 'cleanup-apply' -Status 'skipped' -Detail 'Run apply-clean with the printed plan path after reviewing it.'
    }

    Write-Summary -Status 'passed' -ReportPath (Join-Path $StateRoot 'windows-acceptance.json')
    exit 0
} catch {
    Add-Step -Name 'fatal' -Status 'failed' -Detail $_.Exception.Message
    Write-Summary -Status 'failed' -ReportPath (Join-Path $StateRoot 'windows-acceptance.json')
    exit 1
}
