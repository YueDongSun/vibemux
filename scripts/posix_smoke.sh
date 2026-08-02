#!/usr/bin/env bash
set -euo pipefail
python3 scripts/smoke_test.py
echo "PASS: POSIX-compatible mock smoke"

