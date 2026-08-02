$ErrorActionPreference = "Stop"
python --version
git --version
if (Get-Command wezterm -ErrorAction SilentlyContinue) { wezterm --version } else { Write-Host "WezTerm unavailable: contract-only mode" }
python scripts/smoke_test.py
Write-Host "PASS: Windows-compatible mock smoke"

