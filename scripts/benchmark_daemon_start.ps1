param(
    [Parameter(Mandatory = $true)]
    [string] $project_root,
    [int] $sample_count = 20,
    [string] $vibemuxctl_path = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if ($sample_count -lt 2) {
    throw "sample_count must be at least 2"
}
$resolved_project_root = (Resolve-Path -LiteralPath $project_root).Path
if (-not $vibemuxctl_path) {
    $repository_root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
    $vibemuxctl_path = Join-Path $repository_root "target\release\vibemuxctl.exe"
}
$resolved_vibemuxctl = (Resolve-Path -LiteralPath $vibemuxctl_path).Path
$descriptor_path = Join-Path $resolved_project_root ".vibemux\control.json"
if (Test-Path -LiteralPath $descriptor_path) {
    throw "refusing to benchmark while a daemon descriptor exists"
}

$warmup = & $resolved_vibemuxctl daemon start --project-root $resolved_project_root |
    ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or $warmup.status -ne "started") {
    throw "daemon warmup start failed"
}
& $resolved_vibemuxctl daemon stop --project-root $resolved_project_root | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "daemon warmup stop failed"
}

$start_samples_ms = @()
$stop_samples_ms = @()
1..$sample_count | ForEach-Object {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $start_result = & $resolved_vibemuxctl daemon start --project-root $resolved_project_root |
        ConvertFrom-Json
    $timer.Stop()
    if ($LASTEXITCODE -ne 0 -or $start_result.status -ne "started") {
        throw "measured daemon start failed"
    }
    $start_samples_ms += $timer.Elapsed.TotalMilliseconds

    $timer.Restart()
    $stop_result = & $resolved_vibemuxctl daemon stop --project-root $resolved_project_root |
        ConvertFrom-Json
    $timer.Stop()
    if ($LASTEXITCODE -ne 0 -or $stop_result.status -ne "stopped") {
        throw "measured daemon stop failed"
    }
    $stop_samples_ms += $timer.Elapsed.TotalMilliseconds
}

$sorted_start_ms = @($start_samples_ms | Sort-Object)
$sorted_stop_ms = @($stop_samples_ms | Sort-Object)
$p50_index = [Math]::Ceiling($sample_count * 0.50) - 1
$p95_index = [Math]::Ceiling($sample_count * 0.95) - 1
[pscustomobject]@{
    workload = "release_existing_database_start_to_authenticated_health"
    samples = $sample_count
    warmups = 1
    start_p50_ms = [Math]::Round($sorted_start_ms[$p50_index], 2)
    start_p95_ms = [Math]::Round($sorted_start_ms[$p95_index], 2)
    start_mean_ms = [Math]::Round(($start_samples_ms | Measure-Object -Average).Average, 2)
    start_min_ms = [Math]::Round($sorted_start_ms[0], 2)
    start_max_ms = [Math]::Round($sorted_start_ms[-1], 2)
    stop_p95_ms = [Math]::Round($sorted_stop_ms[$p95_index], 2)
    all_status_checks_passed = $true
} | ConvertTo-Json -Compress
