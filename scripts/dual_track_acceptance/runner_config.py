"""Defaults and tunable constants of the acceptance runner."""

from __future__ import annotations

from pathlib import Path
from typing import Final

REPORT_SCHEMA_VERSION: Final = 1
REPORT_FILE_NAME: Final = "dual_track_report.json"

# Set for the cargo tests so selected workflow tests write the daemon's
# exported evidence (crates/vibemuxd/tests/workflow_support/mod.rs).
EVIDENCE_DIR_ENV: Final = "VIBEMUX_ACCEPTANCE_EVIDENCE_DIR"

# Inputs, relative to the repository root.
BENCHMARK_PATH: Final = Path("tests/fixtures/dual_track_benchmark/dual_track_benchmark_v1.json")
CALIBRATION_SCRIPT: Final = Path("tests/fixtures/taskboard_lite/calibration/calibrate.mjs")
PYTHON_SOURCE_DIR: Final = Path("src")
PYTHON_TEST_DIR: Final = Path("tests")

# Output layout below the owned output directory.
LOG_DIR_NAME: Final = "logs"
EVIDENCE_DIR_NAME: Final = "evidence"

# The suites' temporary root is a run-owned directory created under the
# system temporary directory rather than below the output directory: the
# tests nest Git repositories and worktrees inside it, and Git for Windows
# (core.longpaths off) fails `git worktree add` once those paths approach
# MAX_PATH. A longer root blocks the run instead of failing tests obscurely.
PRIVATE_TMP_PREFIX: Final = "vibemux_dt_"
MAX_PRIVATE_TMP_PATH_CHARS: Final = 64

CARGO_TEST_ARGS: Final = (
    "test",
    "--workspace",
    "--all-features",
    "--no-fail-fast",
    "--message-format=json",
)
CARGO_TEST_TIMEOUT_SECONDS: Final = 5400
CALIBRATION_TIMEOUT_SECONDS: Final = 1800
PYTEST_TIMEOUT_SECONDS: Final = 1200
VERSION_PROBE_TIMEOUT_SECONDS: Final = 60
MIN_NODE_MAJOR: Final = 22

# Executables the tests start; any still running after the suites end was
# not joined.
OWNED_PROCESS_NAMES: Final = (
    "vibemux_worker_fixture",
    "vibemux_native_fixture",
    "vibemux_launch_trampoline",
    "vibemux_model_fixture",
    "vibemux_mock_plugin",
    "vibemux_registry_mock",
    "vibemux_hang_fixture",
    "vibemux_exit_fixture",
    "vibemux_recovery_fixture",
    "vibemuxd",
)

# Exit codes of `verify_dual_track.py`.
EXIT_PASS: Final = 0
EXIT_FAIL: Final = 1
EXIT_BLOCKED: Final = 2
EXIT_USAGE: Final = 3

# Claims no report of this runner supports, whatever its verdict.
EXCLUDED_CLAIMS: Final = (
    "full official A2A ITK conformance",
    "remote or TLS production deployment",
    "compatibility with every listed harness",
    "arbitrary model parity",
    "hostile same-user process isolation",
    "statistically significant model superiority",
    "N_qubit scientific validation",
)
