#Requires -Version 5.1
<#
.SYNOPSIS
Checks, and with -Apply provisions, the local DeepSeek Harness (DSH) prerequisites
VibeMux needs: a Node image, an npm-global `@deepseek-ai/dsh` install at or above the
supported version, the `sdk` profile the VibeMux DSH adapter drives, and the
interactive `tui` profile from an out-of-tree terminal-UI bundle.

.DESCRIPTION
Check mode is the default and performs no mutation. Mutation happens only with
-Apply, and every mutation is one of the documented operator commands:

  node <npm-cli> install -g @deepseek-ai/dsh@<version>   # align the CLI runtime
  node <dsh bin.js> --profile sdk --help                 # initialize the sdk profile
  node <dsh bin.js> plugin --profile tui add <bundle>    # install the tui profile

The script never deletes anything, never writes inside $DSH_HOME by hand, never
touches terminal configuration, and never prints secrets or an environment dump.
It launches tools as `node <javascript entry> <args...>`; it does not route argv
through a command interpreter. The `tui` check is a composition check
(`--dump-default-config`), not an interactive boot: a TUI needs a real terminal.

Exit codes: 0 = every required check passed; 2 = at least one required check did
not pass; 1 = the script could not run (bad usage, unreadable input).

.PARAMETER Apply
Perform the remediation commands for failing checks. Without it the script only
reports.

.PARAMETER Json
Emit one machine-readable JSON object instead of the human report.

.PARAMETER ProjectRoot
Project whose `.vibemux\dsh_session.json` is inspected (and written with
-WriteSessionConfig). Defaults to the current directory.

.PARAMETER NpmPrefix
The npm global prefix. Defaults to `$env:npm_config_prefix`, then the platform
default (`%APPDATA%\npm` on Windows).

.PARAMETER DshHome
The Harness home holding `profiles\`. Defaults to `$env:DSH_HOME`, then
`%USERPROFILE%\.dsh`.

.PARAMETER DshVersion
Exact `@deepseek-ai/dsh` version to install with -Apply. Defaults to
-MinDshVersion, so the provisioning is pinned and reproducible.

.PARAMETER MinDshVersion
The lowest `@deepseek-ai/dsh` version accepted as ready.

.PARAMETER TuiBundle
The bundle that provides the `tui` profile. Defaults to the community terminal UI
`@deepseek-harness-tui/dsh-tui`.

.PARAMETER SkipTui
Do not require the interactive `tui` profile. The `sdk` profile — the one the
VibeMux DSH adapter drives — is still required.

.PARAMETER SkipProbes
Never start a child process. The structural checks still run, and the checks that
need a live probe (the Node version and the `tui` composition) report `unknown`.
This is also the mode used by the fixture tests. It cannot be combined with
-Apply, which by definition runs the installer.

.PARAMETER WriteSessionConfig
With -Apply, write `<ProjectRoot>\.vibemux\dsh_session.json` when it is absent.

.PARAMETER Force
Allow -WriteSessionConfig to replace an existing session configuration.

.PARAMETER InstallTimeoutSeconds
Bound for npm/pnpm installs. Defaults to 900.

.PARAMETER ProbeTimeoutSeconds
Bound for every other child process. Defaults to 120.

.EXAMPLE
.\scripts\setup_dsh_prereq.ps1
Report whether this machine can drive the DSH runtime and the interactive TUI.

.EXAMPLE
.\scripts\setup_dsh_prereq.ps1 -Apply -WriteSessionConfig
Install what is missing, then write the VibeMux DSH session configuration.

.EXAMPLE
.\scripts\setup_dsh_prereq.ps1 -Json -SkipTui
Machine-readable DSH runtime readiness without the optional terminal UI.
#>
[CmdletBinding()]
param(
    [switch]$Apply,
    [switch]$Json,
    [string]$ProjectRoot = (Get-Location).Path,
    [string]$NpmPrefix,
    [string]$DshHome,
    [string]$DshVersion,
    [string]$MinDshVersion = '0.2.0-rc.2',
    [string]$TuiBundle = '@deepseek-harness-tui/dsh-tui',
    [switch]$SkipTui,
    [switch]$SkipProbes,
    [switch]$WriteSessionConfig,
    [switch]$Force,
    [int]$InstallTimeoutSeconds = 900,
    [int]$ProbeTimeoutSeconds = 120
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if ($Apply -and $SkipProbes) {
    Write-Error '-Apply runs the installers, so it cannot be combined with -SkipProbes.'
    exit 1
}

$script:Checks = New-Object System.Collections.ArrayList
$script:Notes = New-Object System.Collections.ArrayList
# Windows PowerShell 5.1 has no $IsWindows, and strict mode rejects reading it.
$script:IsWindowsHost = if ($PSVersionTable.PSVersion.Major -ge 6) { [bool]$IsWindows } else { $true }

# Byte limits for every JSON document this script parses, enforced before the
# parse. The package manifest and session configuration limit equals the
# adapter's MAX_SESSION_CONFIG_BYTES (crates/vibemux_dsh/src/config.rs), so a
# document the adapter refuses for size is never reported ready here. Profile
# manifests belong to DSH, which lists bundles in them, so they get more room.
$script:MaxAdapterJsonBytes = 32 * 1024
$script:MaxProfileManifestBytes = 256 * 1024
# Link hops followed while canonicalizing a launcher path before giving up.
$script:MaxLinkHops = 32

function New-Check {
    param(
        [string]$Id,
        [string]$Severity,
        [string]$Status,
        [string]$Detail,
        [string]$Remediation = ''
    )
    $check = [pscustomobject]@{
        id          = $Id
        severity    = $Severity
        status      = $Status
        detail      = $Detail
        remediation = $Remediation
    }
    [void]$script:Checks.Add($check)
    return $check
}

function Add-Note {
    param([string]$Text)
    [void]$script:Notes.Add($Text)
}

function ConvertTo-ArgumentString {
    <#
    Quote one argument for a Windows command line. Arguments that need no quoting
    are returned unchanged; the rest are wrapped and their embedded quotes doubled.
    #>
    param([string]$Value)
    if ($Value -eq '') { return '""' }
    if ($Value -notmatch '[\s"]') { return $Value }
    return '"' + ($Value -replace '"', '\"') + '"'
}

function Invoke-Bounded {
    <#
    Run one child process with bounded output capture and a hard timeout. Returns
    the exit code, captured streams, and whether the timeout fired. A timed-out
    process tree is terminated, because a wedged install must not be left behind.

    The streams are read through pipes rather than redirected to temporary files:
    a file redirect needs a writable temp path in every host that runs this
    script (and is denied outright under some piped parents), while pipes need
    no filesystem at all.
    #>
    param(
        [string]$FilePath,
        [string[]]$Arguments,
        [int]$TimeoutSeconds
    )
    $startInfo = New-Object System.Diagnostics.ProcessStartInfo
    $startInfo.FileName = $FilePath
    $startInfo.Arguments = ($Arguments | ForEach-Object { ConvertTo-ArgumentString $_ }) -join ' '
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $startInfo.RedirectStandardInput = $true
    $startInfo.CreateNoWindow = $true
    try {
        $utf8 = New-Object System.Text.UTF8Encoding($false)
        $startInfo.StandardOutputEncoding = $utf8
        $startInfo.StandardErrorEncoding = $utf8
    }
    catch {
        # Windows PowerShell 5.1 on an older .NET cannot set the stream encoding;
        # the default console encoding is used instead.
    }

    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $startInfo
    $stdoutTask = $null
    $stderrTask = $null
    try {
        [void]$process.Start()
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()
        $process.StandardInput.Close()
        $timedOut = -not $process.WaitForExit($TimeoutSeconds * 1000)
        if ($timedOut) {
            Stop-ProcessTree -ProcessId $process.Id
            [void]$process.WaitForExit(5000)
        }
        else {
            # Drain the asynchronous readers once the process is gone.
            [void]$process.WaitForExit()
        }
        [void]$stdoutTask.Wait(5000)
        [void]$stderrTask.Wait(5000)
        return [pscustomobject]@{
            exit_code = if ($timedOut) { $null } else { $process.ExitCode }
            timed_out = $timedOut
            stdout    = [string]$stdoutTask.Result
            stderr    = [string]$stderrTask.Result
        }
    }
    finally {
        $process.Dispose()
    }
}

function Stop-ProcessTree {
    param([int]$ProcessId)
    if ($script:IsWindowsHost) {
        $taskkill = Join-Path $env:SystemRoot 'System32\taskkill.exe'
        if (Test-Path $taskkill) {
            try {
                $startInfo = New-Object System.Diagnostics.ProcessStartInfo
                $startInfo.FileName = $taskkill
                $startInfo.Arguments = "/PID $ProcessId /T /F"
                $startInfo.UseShellExecute = $false
                $startInfo.CreateNoWindow = $true
                $killer = [System.Diagnostics.Process]::Start($startInfo)
                [void]$killer.WaitForExit(15000)
                $killer.Dispose()
                return
            }
            catch {
                # Fall through to the single-process kill below.
            }
        }
    }
    Stop-Process -Id $ProcessId -Force -ErrorAction SilentlyContinue
}

function Get-SemVerParts {
    param([string]$Version)
    $core = $Version
    $pre = ''
    $dash = $Version.IndexOf('-')
    if ($dash -ge 0) {
        $core = $Version.Substring(0, $dash)
        $pre = $Version.Substring($dash + 1)
    }
    $numbers = @()
    foreach ($piece in $core.Split('.')) {
        $value = 0
        [void][int]::TryParse($piece, [ref]$value)
        $numbers += $value
    }
    while ($numbers.Count -lt 3) { $numbers += 0 }
    return [pscustomobject]@{ numbers = $numbers; prerelease = $pre }
}

function Compare-SemVer {
    <#
    Compare two semantic versions, including prerelease ordering: a prerelease
    sorts below its release, numeric identifiers sort below alphanumeric ones.
    Returns 1, 0, or -1.
    #>
    param([string]$Left, [string]$Right)
    $a = Get-SemVerParts $Left
    $b = Get-SemVerParts $Right
    for ($i = 0; $i -lt 3; $i++) {
        if ($a.numbers[$i] -gt $b.numbers[$i]) { return 1 }
        if ($a.numbers[$i] -lt $b.numbers[$i]) { return -1 }
    }
    if ($a.prerelease -eq $b.prerelease) { return 0 }
    if ($a.prerelease -eq '') { return 1 }
    if ($b.prerelease -eq '') { return -1 }
    $aParts = $a.prerelease.Split('.')
    $bParts = $b.prerelease.Split('.')
    for ($i = 0; $i -lt [Math]::Max($aParts.Count, $bParts.Count); $i++) {
        if ($i -ge $aParts.Count) { return -1 }
        if ($i -ge $bParts.Count) { return 1 }
        $aNum = 0; $bNum = 0
        $aIsNum = [int]::TryParse($aParts[$i], [ref]$aNum)
        $bIsNum = [int]::TryParse($bParts[$i], [ref]$bNum)
        if ($aIsNum -and $bIsNum) {
            if ($aNum -gt $bNum) { return 1 }
            if ($aNum -lt $bNum) { return -1 }
            continue
        }
        if ($aIsNum) { return -1 }
        if ($bIsNum) { return 1 }
        $order = [string]::CompareOrdinal($aParts[$i], $bParts[$i])
        if ($order -ne 0) { return $(if ($order -gt 0) { 1 } else { -1 }) }
    }
    return 0
}

function Read-BoundedJsonObject {
    <#
    Parse one JSON object from a regular file of at most MaxBytes bytes. The size
    is checked before reading, and the read stops one byte past the limit, so a
    file that grows (or a link whose own length is zero) is still refused before
    it is parsed. The text must be strict UTF-8 and the top-level value an
    object. Throws on any failure; callers report a throw as an unreadable
    document.
    #>
    param([string]$Path, [int]$MaxBytes)
    $item = Get-Item -LiteralPath $Path -Force
    if ($item -isnot [System.IO.FileInfo]) { throw 'not a regular file' }
    if ($item.Length -gt $MaxBytes) { throw "larger than $MaxBytes bytes" }
    $buffer = New-Object byte[] ($MaxBytes + 1)
    $total = 0
    $stream = [System.IO.File]::OpenRead($item.FullName)
    try {
        while ($total -lt $buffer.Length) {
            $read = $stream.Read($buffer, $total, $buffer.Length - $total)
            if ($read -le 0) { break }
            $total += $read
        }
    }
    finally {
        $stream.Dispose()
    }
    if ($total -gt $MaxBytes) { throw "larger than $MaxBytes bytes" }
    # A UTF-8 byte-order mark is skipped, as Get-Content does; invalid UTF-8 throws.
    $strictUtf8 = New-Object System.Text.UTF8Encoding($false, $true)
    $reader = New-Object System.IO.StreamReader((New-Object System.IO.MemoryStream($buffer, 0, $total)), $strictUtf8, $true)
    try { $text = $reader.ReadToEnd() } finally { $reader.Dispose() }
    # ConvertFrom-Json unrolls a top-level array, so a one-element array would
    # otherwise pass for an object.
    if (-not $text.TrimStart().StartsWith('{')) { throw 'top-level value is not a JSON object' }
    return ($text | ConvertFrom-Json)
}

function Resolve-ComparablePath {
    <#
    Approximate std::fs::canonicalize, which the adapter applies to the project
    root and to both launcher paths before its containment test: make the path
    absolute, then follow every symbolic-link or junction component, including
    links inside a link's target. When a component is missing or a link cannot be
    followed, the absolute lexical path is returned instead.
    #>
    param([string]$Path)
    $lexical = [System.IO.Path]::GetFullPath($Path)
    $pending = $lexical
    try {
        for ($hop = 0; $hop -le $script:MaxLinkHops; $hop++) {
            $root = [System.IO.Path]::GetPathRoot($pending)
            $components = $pending.Substring($root.Length).Split([char[]]@('\', '/'), [System.StringSplitOptions]::RemoveEmptyEntries)
            $current = $root
            $redirect = $null
            for ($index = 0; $index -lt $components.Count; $index++) {
                $next = [System.IO.Path]::Combine($current, $components[$index])
                $item = Get-Item -LiteralPath $next -Force -ErrorAction SilentlyContinue
                if ($null -eq $item) { return $lexical }
                $linkProperty = $item.PSObject.Properties['LinkType']
                $linkType = if ($null -ne $linkProperty) { [string]$linkProperty.Value } else { '' }
                if ($linkType -eq 'SymbolicLink' -or $linkType -eq 'Junction') {
                    if ($null -ne $item.PSObject.Methods['ResolveLinkTarget']) {
                        # .NET 6+ asks the OS, which also understands stored junction
                        # forms such as `\??\Global\C:\...` that are not plain paths.
                        $final = $item.ResolveLinkTarget($true)
                        $target = if ($null -ne $final) { $final.FullName } else { $null }
                    }
                    else {
                        $target = [string](@($item.Target) | Select-Object -First 1)
                        # Windows PowerShell 5.1 reports a junction's stored name minus
                        # `\??\`; map the object-manager forms back to plain paths.
                        # Junction targets are always absolute, so a relative symlink
                        # target is never rewritten.
                        if ($linkType -eq 'Junction' -and $target -match '^Global\\(.+)$') { $target = $Matches[1] }
                        elseif ($linkType -eq 'Junction' -and $target -match '^UNC\\(.+)$') { $target = '\\' + $Matches[1] }
                    }
                    if (-not $target) { return $lexical }
                    # A relative link target is relative to the directory holding the link.
                    $redirect = [System.IO.Path]::Combine($current, [string]$target)
                    for ($rest = $index + 1; $rest -lt $components.Count; $rest++) {
                        $redirect = [System.IO.Path]::Combine($redirect, $components[$rest])
                    }
                    break
                }
                $current = $next
            }
            if ($null -eq $redirect) { return $current }
            $pending = [System.IO.Path]::GetFullPath($redirect)
        }
    }
    catch {
        # A link this host cannot follow leaves the lexical path, as documented above.
    }
    return $lexical
}

function Test-PathWithin {
    <#
    True when Candidate is Root itself or lies below it. Paths are compared
    component by component, never by string prefix, so `C:\work\project-tools`
    is not inside `C:\work\project`; trailing separators do not matter, and
    components compare case-insensitively on Windows.
    #>
    param([string]$Candidate, [string]$Root)
    $separators = [char[]]@('\', '/')
    $candidateParts = $Candidate.Split($separators, [System.StringSplitOptions]::RemoveEmptyEntries)
    $rootParts = $Root.Split($separators, [System.StringSplitOptions]::RemoveEmptyEntries)
    if ($rootParts.Count -gt $candidateParts.Count) { return $false }
    $comparison = if ($script:IsWindowsHost) { [System.StringComparison]::OrdinalIgnoreCase } else { [System.StringComparison]::Ordinal }
    for ($index = 0; $index -lt $rootParts.Count; $index++) {
        if (-not [string]::Equals($candidateParts[$index], $rootParts[$index], $comparison)) { return $false }
    }
    return $true
}

function Resolve-NodeImage {
    $command = Get-Command node -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($null -eq $command) { return $null }
    $path = $command.Source
    if ([System.IO.Path]::GetFileNameWithoutExtension($path) -ine 'node') { return $null }
    return $path
}

function Resolve-NpmPrefix {
    param([string]$Explicit, [string]$NodeImage)
    if ($Explicit) { return $Explicit }
    foreach ($name in @('npm_config_prefix', 'NPM_CONFIG_PREFIX')) {
        $value = [Environment]::GetEnvironmentVariable($name)
        if ($value) { return $value }
    }
    if ($script:IsWindowsHost) {
        if ($env:APPDATA) { return (Join-Path $env:APPDATA 'npm') }
        return $null
    }
    if ($env:HOME) { return (Join-Path $env:HOME '.npm-global') }
    return $null
}

function Resolve-DshHome {
    param([string]$Explicit)
    if ($Explicit) { return $Explicit }
    if ($env:DSH_HOME) { return $env:DSH_HOME }
    if ($script:IsWindowsHost -and $env:USERPROFILE) { return (Join-Path $env:USERPROFILE '.dsh') }
    if ($env:HOME) { return (Join-Path $env:HOME '.dsh') }
    return $null
}

function Get-DshInstall {
    <#
    Resolve the npm-global DSH package the same way the adapter does: the script
    must be a `.js` file that the installed `@deepseek-ai/dsh` manifest maps in its
    own `bin` map. Anything else is reported as an untrusted install.
    #>
    param([string]$Prefix)
    $packageRoot = Join-Path $Prefix 'node_modules/@deepseek-ai/dsh'
    $manifestPath = Join-Path $packageRoot 'package.json'
    if (-not (Test-Path $manifestPath)) {
        return [pscustomobject]@{ present = $false; trusted = $false; version = $null; entry_script = $null; reason = 'package manifest not found' }
    }
    $manifest = $null
    try { $manifest = Read-BoundedJsonObject -Path $manifestPath -MaxBytes $script:MaxAdapterJsonBytes } catch {
        return [pscustomobject]@{ present = $true; trusted = $false; version = $null; entry_script = $null; reason = "package manifest is not a JSON object of at most $($script:MaxAdapterJsonBytes) bytes" }
    }
    $version = if ($manifest.PSObject.Properties.Name -contains 'version') { [string]$manifest.version } else { $null }
    $packageName = if ($manifest.PSObject.Properties.Name -contains 'name') { [string]$manifest.name } else { '' }
    if ($packageName -ne '@deepseek-ai/dsh') {
        return [pscustomobject]@{ present = $true; trusted = $false; version = $version; entry_script = $null; reason = "package name is '$packageName'" }
    }
    $entryScript = $null
    if ($manifest.PSObject.Properties.Name -contains 'bin') {
        $bin = $manifest.bin
        if ($bin -is [string]) { $entryScript = $bin }
        elseif ($bin.PSObject.Properties.Name -contains 'dsh') { $entryScript = [string]$bin.dsh }
    }
    if (-not $entryScript) {
        return [pscustomobject]@{ present = $true; trusted = $false; version = $version; entry_script = $null; reason = 'manifest declares no dsh bin entry' }
    }
    $resolved = Join-Path $packageRoot $entryScript
    if ([System.IO.Path]::GetExtension($resolved) -ne '.js' -or -not (Test-Path $resolved)) {
        return [pscustomobject]@{ present = $true; trusted = $false; version = $version; entry_script = $null; reason = 'dsh bin entry is not an existing .js file' }
    }
    return [pscustomobject]@{ present = $true; trusted = $true; version = $version; entry_script = $resolved; reason = '' }
}

function Test-TuiBundleSpec {
    param([string]$Spec)
    return $Spec -match '^(@[a-z0-9][a-z0-9._-]*/)?[a-z0-9][a-z0-9._-]*(@[A-Za-z0-9][A-Za-z0-9._+-]*)?$'
}

function Get-ProfileManifest {
    <#
    Read one profile manifest. `present` says the file exists; `valid` says it is
    a readable JSON object within the byte limit. A present but invalid manifest
    is a profile DSH cannot load, so callers must not count it as provisioned.
    #>
    param([string]$DshHomePath, [string]$Profile)
    $path = Join-Path (Join-Path $DshHomePath 'profiles') (Join-Path $Profile 'package.json')
    if (-not (Test-Path $path)) { return [pscustomobject]@{ present = $false; valid = $false; path = $path; bundles = @() } }
    try {
        $manifest = Read-BoundedJsonObject -Path $path -MaxBytes $script:MaxProfileManifestBytes
        $bundles = @()
        $names = $manifest.PSObject.Properties.Name
        if ($names -contains 'dsh' -and $manifest.dsh -and ($manifest.dsh.PSObject.Properties.Name -contains 'profile')) {
            if ($manifest.dsh.profile.PSObject.Properties.Name -contains 'bundles') {
                $bundles = @($manifest.dsh.profile.bundles)
            }
        }
        return [pscustomobject]@{ present = $true; valid = $true; path = $path; bundles = $bundles }
    }
    catch {
        return [pscustomobject]@{ present = $true; valid = $false; path = $path; bundles = @() }
    }
}

# ---------------------------------------------------------------- environment

$nodeImage = Resolve-NodeImage
$npmPrefixResolved = Resolve-NpmPrefix -Explicit $NpmPrefix -NodeImage $nodeImage
$dshHomeResolved = Resolve-DshHome -Explicit $DshHome
$install = if ($npmPrefixResolved) { Get-DshInstall -Prefix $npmPrefixResolved } else {
    [pscustomobject]@{ present = $false; trusted = $false; version = $null; entry_script = $null; reason = 'npm global prefix could not be resolved' }
}
$nodeVersion = $null
# Why a probed node image is unusable; $null when it answered or was not probed.
$nodeProbeFailure = $null
if ($nodeImage -and -not $SkipProbes) {
    try {
        $probe = Invoke-Bounded -FilePath $nodeImage -Arguments @('--version') -TimeoutSeconds $ProbeTimeoutSeconds
        if ($probe.timed_out) { $nodeProbeFailure = "timed out after $ProbeTimeoutSeconds s" }
        elseif ($probe.exit_code -ne 0) { $nodeProbeFailure = "exited with code $($probe.exit_code)" }
        elseif (-not $probe.stdout.Trim()) { $nodeProbeFailure = 'printed no version' }
        else { $nodeVersion = $probe.stdout.Trim() }
    }
    catch {
        $nodeProbeFailure = 'could not be started'
    }
}
# Only a node image that is present and did not fail its probe may run installers
# or be written into a session configuration.
$nodeUsable = [bool]$nodeImage -and -not $nodeProbeFailure

if ($nodeImage -and $nodeProbeFailure) {
    [void](New-Check -Id 'node' -Severity 'required' -Status 'missing' `
            -Detail "$nodeImage did not answer --version ($nodeProbeFailure)" `
            -Remediation 'Repair or reinstall Node.js 22.19+ or 24+ (https://nodejs.org) so `node --version` succeeds, then rerun this script.')
}
elseif ($nodeImage) {
    $nodeDetail = if ($nodeVersion) { "$nodeImage ($nodeVersion)" } else { "$nodeImage (version not probed)" }
    [void](New-Check -Id 'node' -Severity 'required' -Status 'ok' -Detail $nodeDetail)
}
else {
    [void](New-Check -Id 'node' -Severity 'required' -Status 'missing' `
            -Detail 'no native node image on PATH' `
            -Remediation 'Install Node.js 22.19+ or 24+ (https://nodejs.org), then rerun this script.')
}

if ($npmPrefixResolved -and (Test-Path (Join-Path $npmPrefixResolved 'node_modules/npm/bin/npm-cli.js'))) {
    [void](New-Check -Id 'npm' -Severity 'required' -Status 'ok' -Detail $npmPrefixResolved)
}
else {
    [void](New-Check -Id 'npm' -Severity 'required' -Status 'missing' `
            -Detail "npm global prefix could not be resolved from '$npmPrefixResolved'" `
            -Remediation 'Reinstall Node.js with npm, or pass -NpmPrefix <dir>.')
}

if ($SkipTui) {
    Add-Note 'The interactive tui profile is excluded by -SkipTui.'
}
elseif (-not (Test-TuiBundleSpec -Spec $TuiBundle)) {
    [void](New-Check -Id 'pnpm' -Severity 'required' -Status 'unknown' `
            -Detail "-TuiBundle '$TuiBundle' is not a valid npm package spec" `
            -Remediation 'Pass a registry package name such as @scope/name or name@1.2.3.')
}
else {
    $pnpmEntry = if ($npmPrefixResolved) { Join-Path $npmPrefixResolved 'node_modules/pnpm/bin/pnpm.cjs' } else { $null }
    if ($pnpmEntry -and (Test-Path $pnpmEntry)) {
        [void](New-Check -Id 'pnpm' -Severity 'required' -Status 'ok' -Detail $pnpmEntry)
    }
    else {
        $pnpmCommand = Get-Command pnpm -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($pnpmCommand) {
            [void](New-Check -Id 'pnpm' -Severity 'required' -Status 'ok' -Detail $pnpmCommand.Source)
        }
        else {
            [void](New-Check -Id 'pnpm' -Severity 'required' -Status 'missing' `
                    -Detail 'pnpm is not installed; dsh plugin installs profile bundles with it' `
                    -Remediation 'Install pnpm (npm install -g pnpm), then rerun this script.')
        }
    }
}

if ($install.trusted) {
    [void](New-Check -Id 'dsh-install' -Severity 'required' -Status 'ok' `
            -Detail "@deepseek-ai/dsh $($install.version) -> $($install.entry_script)")
}
else {
    [void](New-Check -Id 'dsh-install' -Severity 'required' -Status 'missing' `
            -Detail "no trusted @deepseek-ai/dsh install under '$npmPrefixResolved' ($($install.reason))" `
            -Remediation "Install it globally: npm install -g @deepseek-ai/dsh@$MinDshVersion, or rerun this script with -Apply.")
}

$versionOk = $false
if ($install.trusted -and $install.version) {
    $comparison = Compare-SemVer -Left $install.version -Right $MinDshVersion
    $versionOk = $comparison -ge 0
    $status = if ($versionOk) { 'ok' } else { 'stale' }
    $detail = "@deepseek-ai/dsh $($install.version) (minimum $MinDshVersion)"
    [void](New-Check -Id 'dsh-version' -Severity 'required' -Status $status -Detail $detail `
            -Remediation "Upgrade the CLI: npm install -g @deepseek-ai/dsh@$MinDshVersion, or rerun this script with -Apply.")
}
else {
    [void](New-Check -Id 'dsh-version' -Severity 'required' -Status 'unknown' `
            -Detail 'version cannot be read because no trusted install was found' `
            -Remediation "Install it globally: npm install -g @deepseek-ai/dsh@$MinDshVersion.")
}

$sdkProfile = if ($dshHomeResolved) { Get-ProfileManifest -DshHomePath $dshHomeResolved -Profile 'sdk' } else { $null }
if ($sdkProfile -and $sdkProfile.present -and $sdkProfile.valid) {
    [void](New-Check -Id 'dsh-sdk-profile' -Severity 'required' -Status 'ok' -Detail $sdkProfile.path)
}
elseif ($sdkProfile -and $sdkProfile.present) {
    [void](New-Check -Id 'dsh-sdk-profile' -Severity 'required' -Status 'stale' `
            -Detail "$($sdkProfile.path) is not a JSON object of at most $($script:MaxProfileManifestBytes) bytes" `
            -Remediation 'Inspect or restore that manifest; DSH writes it when it provisions the sdk profile (dsh --profile sdk --help). -Apply runs that command but does not delete or rewrite the file.')
}
else {
    [void](New-Check -Id 'dsh-sdk-profile' -Severity 'required' -Status 'missing' `
            -Detail "no sdk profile under '$(Join-Path $dshHomeResolved 'profiles')'" `
            -Remediation 'Boot it once so DSH provisions the profile: dsh --profile sdk --help (this writes to the DSH home), or rerun this script with -Apply.')
}

if ($SkipTui) {
    [void](New-Check -Id 'dsh-tui-profile' -Severity 'advisory' -Status 'unknown' -Detail 'excluded by -SkipTui')
    [void](New-Check -Id 'dsh-tui-compose' -Severity 'advisory' -Status 'unknown' -Detail 'excluded by -SkipTui')
}
else {
    $tuiProfile = if ($dshHomeResolved) { Get-ProfileManifest -DshHomePath $dshHomeResolved -Profile 'tui' } else { $null }
    $tuiBundleName = ($TuiBundle -split '@')[0..1] -join '@'
    if ($TuiBundle -notmatch '^@') { $tuiBundleName = ($TuiBundle -split '@')[0] }
    $bundlePresent = $tuiProfile -and $tuiProfile.present -and ($tuiProfile.bundles | Where-Object { $_ -eq $tuiBundleName })
    if ($bundlePresent) {
        [void](New-Check -Id 'dsh-tui-profile' -Severity 'required' -Status 'ok' `
                -Detail "$($tuiProfile.path) bundles $($tuiProfile.bundles -join ', ')")
    }
    else {
        $tuiDetail = if ($tuiProfile -and $tuiProfile.present -and -not $tuiProfile.valid) {
            "$($tuiProfile.path) is not a JSON object of at most $($script:MaxProfileManifestBytes) bytes"
        }
        else { "the tui profile is absent or does not bundle $tuiBundleName" }
        [void](New-Check -Id 'dsh-tui-profile' -Severity 'required' -Status 'missing' `
                -Detail $tuiDetail `
                -Remediation "Install it: dsh plugin --profile tui add $TuiBundle, or rerun this script with -Apply.")
    }
    if ($SkipProbes) {
        # -SkipProbes is an explicit request not to run the composition, so the
        # unprobed state is reported without failing a structural readiness check.
        [void](New-Check -Id 'dsh-tui-compose' -Severity 'advisory' -Status 'unknown' `
                -Detail 'composition not probed (-SkipProbes)')
    }
    elseif ($nodeUsable -and $install.trusted -and $bundlePresent) {
        $compose = Invoke-Bounded -FilePath $nodeImage -Arguments @($install.entry_script, '--profile', 'tui', '--dump-default-config') -TimeoutSeconds $ProbeTimeoutSeconds
        if (-not $compose.timed_out -and $compose.exit_code -eq 0) {
            [void](New-Check -Id 'dsh-tui-compose' -Severity 'required' -Status 'ok' `
                    -Detail 'the tui profile composes; run `dsh tui` in a real terminal to use it')
        }
        else {
            $reason = if ($compose.timed_out) { 'timed out' } else { ($compose.stderr.Trim() -split "`n" | Select-Object -First 1) }
            [void](New-Check -Id 'dsh-tui-compose' -Severity 'required' -Status 'missing' `
                    -Detail "the tui profile failed to compose: $reason" `
                    -Remediation "Reinstall the bundle: dsh plugin --profile tui remove $tuiBundleName; dsh plugin --profile tui add $TuiBundle")
        }
    }
    else {
        [void](New-Check -Id 'dsh-tui-compose' -Severity 'required' -Status 'unknown' `
                -Detail 'not attempted because node, the install, or the tui profile is unavailable')
    }
}

$sessionPath = Join-Path (Join-Path $ProjectRoot '.vibemux') 'dsh_session.json'
$sessionPresent = Test-Path $sessionPath
if ($sessionPresent) {
    $sessionOk = $false
    $sessionDetail = "$sessionPath is not valid schema-1 JSON of at most $($script:MaxAdapterJsonBytes) bytes"
    try {
        $session = Read-BoundedJsonObject -Path $sessionPath -MaxBytes $script:MaxAdapterJsonBytes
        $sessionNames = $session.PSObject.Properties.Name
        # Wrap the pipeline so an empty result is an empty array, not $null:
        # under Set-StrictMode a $null.Count read would abort this validation.
        $missingFields = @(
            @('schema_version', 'executable', 'entry_script', 'provider', 'model') |
                Where-Object { $sessionNames -notcontains $_ }
        )
        if ($session.schema_version -eq 1 -and $missingFields.Count -eq 0) {
            $sessionOk = $true
            $profileName = if ($sessionNames -contains 'profile') { [string]$session.profile } else { 'sdk (default)' }
            $sessionDetail = "$sessionPath (profile '$profileName')"
        }
    }
    catch { }
    $status = if ($sessionOk) { 'ok' } else { 'stale' }
    [void](New-Check -Id 'vibemux-session-config' -Severity 'advisory' -Status $status -Detail $sessionDetail `
            -Remediation 'Rewrite it from the template this script prints, or rerun with -Apply -WriteSessionConfig -Force.')
}
else {
    [void](New-Check -Id 'vibemux-session-config' -Severity 'advisory' -Status 'missing' `
            -Detail "no session configuration at $sessionPath" `
            -Remediation 'Rerun with -Apply -WriteSessionConfig, or copy the template this script prints into that path.')
}

$apiKeyPresent = [bool]$env:DEEPSEEK_API_KEY
[void](New-Check -Id 'deepseek-api-key' -Severity 'advisory' `
        -Status $(if ($apiKeyPresent) { 'ok' } else { 'missing' }) `
        -Detail $(if ($apiKeyPresent) { 'DEEPSEEK_API_KEY is present in this shell (value never read)' } else { 'DEEPSEEK_API_KEY is not set in this shell' }) `
        -Remediation 'Set DEEPSEEK_API_KEY before running a real turn; the composition checks above do not need it.')

# --------------------------------------------------------------------- apply

$applied = New-Object System.Collections.ArrayList

function Get-CheckStatus {
    param([string]$Id)
    $check = $script:Checks | Where-Object { $_.id -eq $Id } | Select-Object -First 1
    if ($null -eq $check) { return 'unknown' }
    return $check.status
}

function Set-CheckResult {
    param([string]$Id, [string]$Status, [string]$Detail)
    $check = $script:Checks | Where-Object { $_.id -eq $Id } | Select-Object -First 1
    if ($null -ne $check) {
        $check.status = $Status
        $check.detail = $Detail
    }
}

if ($Apply) {
    if ($nodeUsable -and $npmPrefixResolved) {
        $npmCli = Join-Path $npmPrefixResolved 'node_modules/npm/bin/npm-cli.js'
        if ((Get-CheckStatus 'dsh-install') -ne 'ok' -or (Get-CheckStatus 'dsh-version') -ne 'ok') {
            if (Test-Path $npmCli) {
                $target = if ($DshVersion) { $DshVersion } else { $MinDshVersion }
                $result = Invoke-Bounded -FilePath $nodeImage -Arguments @($npmCli, 'install', '-g', "@deepseek-ai/dsh@$target") -TimeoutSeconds $InstallTimeoutSeconds
                $ok = -not $result.timed_out -and $result.exit_code -eq 0
                [void]$applied.Add("npm install -g @deepseek-ai/dsh@$target -> $(if ($ok) { 'ok' } else { 'failed' })")
                $install = Get-DshInstall -Prefix $npmPrefixResolved
                if ($ok -and $install.trusted) {
                    Set-CheckResult -Id 'dsh-install' -Status 'ok' -Detail "@deepseek-ai/dsh $($install.version) -> $($install.entry_script)"
                    $versionOk = (Compare-SemVer -Left $install.version -Right $MinDshVersion) -ge 0
                    Set-CheckResult -Id 'dsh-version' -Status $(if ($versionOk) { 'ok' } else { 'stale' }) -Detail "@deepseek-ai/dsh $($install.version) (minimum $MinDshVersion)"
                }
            }
            else {
                [void]$applied.Add('npm install skipped: no npm-cli.js under the resolved prefix')
            }
        }
        if ((Get-CheckStatus 'dsh-sdk-profile') -ne 'ok' -and $install.trusted) {
            $result = Invoke-Bounded -FilePath $nodeImage -Arguments @($install.entry_script, '--profile', 'sdk', '--help') -TimeoutSeconds $ProbeTimeoutSeconds
            $ok = -not $result.timed_out -and $result.exit_code -eq 0
            [void]$applied.Add("dsh --profile sdk --help -> $(if ($ok) { 'ok' } else { 'failed' })")
            if ($ok -and $dshHomeResolved) {
                $sdkProfile = Get-ProfileManifest -DshHomePath $dshHomeResolved -Profile 'sdk'
                if ($sdkProfile.present -and $sdkProfile.valid) {
                    Set-CheckResult -Id 'dsh-sdk-profile' -Status 'ok' -Detail $sdkProfile.path
                }
            }
        }
        if (-not $SkipTui -and $install.trusted -and (Get-CheckStatus 'dsh-tui-profile') -ne 'ok') {
            $result = Invoke-Bounded -FilePath $nodeImage -Arguments @($install.entry_script, 'plugin', '--profile', 'tui', 'add', $TuiBundle) -TimeoutSeconds $InstallTimeoutSeconds
            $ok = -not $result.timed_out -and $result.exit_code -eq 0
            [void]$applied.Add("dsh plugin --profile tui add $TuiBundle -> $(if ($ok) { 'ok' } else { 'failed' })")
            if ($ok -and $dshHomeResolved) {
                $tuiProfile = Get-ProfileManifest -DshHomePath $dshHomeResolved -Profile 'tui'
                $tuiBundleName = if ($TuiBundle -match '^@') { ($TuiBundle -split '@')[0..1] -join '@' } else { ($TuiBundle -split '@')[0] }
                if ($tuiProfile.present -and ($tuiProfile.bundles | Where-Object { $_ -eq $tuiBundleName })) {
                    Set-CheckResult -Id 'dsh-tui-profile' -Status 'ok' -Detail "$($tuiProfile.path) bundles $($tuiProfile.bundles -join ', ')"
                    $compose = Invoke-Bounded -FilePath $nodeImage -Arguments @($install.entry_script, '--profile', 'tui', '--dump-default-config') -TimeoutSeconds $ProbeTimeoutSeconds
                    if (-not $compose.timed_out -and $compose.exit_code -eq 0) {
                        Set-CheckResult -Id 'dsh-tui-compose' -Status 'ok' -Detail 'the tui profile composes; run `dsh tui` in a real terminal to use it'
                    }
                }
            }
        }
    }
    else {
        [void]$applied.Add('no remediation attempted: no runnable node image, or the npm prefix is unresolved')
    }

    if ($WriteSessionConfig -and $install.trusted) {
        # Mirror the adapter: it canonicalizes the project root and both launcher
        # paths, then refuses a launcher at or below the root.
        # Wrap the pipeline: an empty result must be an empty array, not $null,
        # because Set-StrictMode rejects a $null.Count read.
        $insideProject = @()
        if ($nodeUsable) {
            $projectComparable = Resolve-ComparablePath -Path $ProjectRoot
            $insideProject = @(
                @($nodeImage, $install.entry_script) | Where-Object {
                    Test-PathWithin -Candidate (Resolve-ComparablePath -Path $_) -Root $projectComparable
                }
            )
        }
        if ($sessionPresent -and -not $Force) {
            [void]$applied.Add("session config left unchanged (already present; pass -Force to replace): $sessionPath")
        }
        elseif (-not $nodeUsable) {
            [void]$applied.Add('session config skipped: no runnable node image')
        }
        elseif ($insideProject.Count -gt 0) {
            # The adapter refuses either path inside the project root, so writing
            # such a file would only produce a configuration it rejects.
            [void]$applied.Add('session config skipped: the node image or the entry script resolves inside the project root, which the adapter refuses')
            Set-CheckResult -Id 'vibemux-session-config' -Status 'missing' -Detail 'not written: a launcher path resolves inside the project root'
        }
        else {
            $sessionDirectory = Split-Path $sessionPath -Parent
            if (-not (Test-Path $sessionDirectory)) { [void](New-Item -ItemType Directory -Path $sessionDirectory -Force) }
            $document = [ordered]@{
                schema_version    = 1
                executable        = $nodeImage
                entry_script      = $install.entry_script
                profile           = 'sdk'
                provider          = 'deepseek-official'
                model             = 'deepseek-v4-flash'
                environment_names = @('DEEPSEEK_API_KEY')
            }
            # Write UTF-8 without a BOM: Windows PowerShell 5.1's -Encoding UTF8
            # emits one, and a byte-order mark makes the document invalid JSON.
            # The name must not be $json: parameter names are case-insensitive,
            # so that would assign this string to the [switch]$Json parameter.
            $documentJson = $document | ConvertTo-Json -Depth 5
            [System.IO.File]::WriteAllText($sessionPath, $documentJson, (New-Object System.Text.UTF8Encoding($false)))
            [void]$applied.Add("wrote $sessionPath")
            Set-CheckResult -Id 'vibemux-session-config' -Status 'ok' -Detail "$sessionPath (profile 'sdk')"
        }
    }
}

# -------------------------------------------------------------------- report

# -WriteSessionConfig may have created the file during the apply pass, so the
# reported presence must be re-read rather than remembered.
$sessionPresent = Test-Path $sessionPath

$required = $script:Checks | Where-Object { $_.severity -eq 'required' }
$failedRequired = @($required | Where-Object { $_.status -ne 'ok' })
$ok = $failedRequired.Count -eq 0

$sessionTemplate = [ordered]@{
    schema_version    = 1
    executable        = $(if ($nodeImage) { $nodeImage } else { '<absolute path to a native node image>' })
    entry_script      = $(if ($install.trusted) { $install.entry_script } else { '<absolute path to @deepseek-ai/dsh/lib/bin.js>' })
    profile           = 'sdk'
    provider          = 'deepseek-official'
    model             = 'deepseek-v4-flash'
    environment_names = @('DEEPSEEK_API_KEY')
}

if ($Json) {
    $payload = [ordered]@{
        schema_version = 1
        ok             = $ok
        mode           = $(if ($Apply) { 'apply' } else { 'check' })
        npm_prefix     = $npmPrefixResolved
        dsh_home       = $dshHomeResolved
        project_root   = $ProjectRoot
        node           = [ordered]@{ image = $nodeImage; version = $nodeVersion }
        dsh            = [ordered]@{
            package     = '@deepseek-ai/dsh'
            version     = $install.version
            min_version = $MinDshVersion
            entry_script = $install.entry_script
        }
        checks           = @($script:Checks)
        applied          = @($applied)
        notes            = @($script:Notes)
        session_config   = [ordered]@{ path = $sessionPath; present = $sessionPresent; template = $sessionTemplate }
    }
    $payload | ConvertTo-Json -Depth 6
}
else {
    Write-Host 'VibeMux DSH prerequisite'
    Write-Host ("  mode        : {0}" -f $(if ($Apply) { 'apply' } else { 'check (no changes; add -Apply to provision)' }))
    Write-Host ("  project     : {0}" -f $ProjectRoot)
    Write-Host ("  npm prefix  : {0}" -f $npmPrefixResolved)
    Write-Host ("  DSH home    : {0}" -f $dshHomeResolved)
    Write-Host ''
    foreach ($check in $script:Checks) {
        $marker = switch ($check.status) {
            'ok' { '[ ok ]' }
            'missing' { '[MISS]' }
            'stale' { '[STALE]' }
            default { '[ ?  ]' }
        }
        $tag = if ($check.severity -eq 'required') { 'required' } else { 'advisory' }
        Write-Host ("{0} {1,-24} {2,-8} {3}" -f $marker, $check.id, $tag, $check.detail)
        if ($check.status -ne 'ok' -and $check.remediation) {
            Write-Host ("        -> {0}" -f $check.remediation)
        }
    }
    if ($applied.Count -gt 0) {
        Write-Host ''
        Write-Host 'Applied:'
        foreach ($line in $applied) { Write-Host ("  - {0}" -f $line) }
    }
    if ($script:Notes.Count -gt 0) {
        Write-Host ''
        foreach ($note in $script:Notes) { Write-Host ("note: {0}" -f $note) }
    }
    if (-not $sessionPresent) {
        Write-Host ''
        Write-Host ('.vibemux\dsh_session.json template (fields not filled in are resolved above):')
        Write-Host ($sessionTemplate | ConvertTo-Json -Depth 5)
    }
    Write-Host ''
    if ($ok) {
        Write-Host 'READY: the DSH runtime and the requested surfaces are available.'
        if (-not $SkipTui) {
            Write-Host 'Launch the interactive terminal UI from a real terminal with: dsh tui'
        }
    }
    else {
        Write-Host ("NOT READY: {0} required check(s) did not pass." -f $failedRequired.Count)
    }
}

if ($ok) { exit 0 }
exit 2
