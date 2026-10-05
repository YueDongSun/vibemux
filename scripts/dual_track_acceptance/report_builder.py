"""Assembles an acceptance report from its artifact projections."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .assessment import MODE_OFFLINE, ScenarioResult
from .check_execution import CalibrationResult, CargoTestRun, CommandRecord
from .evidence_capture import EvidenceStore
from .live_policy import LiveGate
from .report_projection import artifact_report_fields
from .runner_config import EXCLUDED_CLAIMS, REPORT_SCHEMA_VERSION
from .scenario_catalog import Benchmark, EvidenceRule

OFFLINE_EVIDENCE_CLASS = "offline_fixture"

# What no offline run of this implementation establishes, whatever its
# verdict. Kept in step with docs/adr/031_dual_track_workflow.md.
STANDING_LIMITATIONS = (
    "offline evidence only: worker and reviewer turns use copied synthetic adapters; compiler "
    "and gateway fixtures use an owned loopback HTTP server and synthetic responses. No live "
    "model, external AAG service, or vendor CLI was contacted",
    "AAG supports authorized fixture calls; live calls are refused until request-scoped upstream "
    "attestation is available, and vendor route certification is not established",
    "native controller input, interruption, and context handoff have adapter-fixture coverage; "
    "there is no production-owned native TUI session or attach/resume surface",
    "the optimizer improvement is measured on fixture cases only and implies no model gain",
    "workflow worktrees and materializations are retained by the daemon "
    "(workspace_unsafe_cleanup); the tests' temporary projects, including those worktrees, are "
    "removed with the run's private temporary root",
    "prompt editing of a paused session is not implemented; inspection is read-only",
    "model-call budgets are charged at reservation, including compiler calls; offline fixture "
    "traffic provides no observed live token usage",
    "unsupported optimizer parameters are rejected; operational candidates currently change "
    "template_overrides only",
    "negative-control verifier receipts are persisted but remain non-promotable",
    "owned paths require source-requirement or exact operator-policy provenance at admission; "
    "the provenance map is not separately persisted",
    "the crash-restart test simulates the crash through the writer, not a killed process",
    "retained content is protected per user (ACL/permissions) only, not against the same user",
    "G02 gateway refusals are checked with a local fixture; no live route certification is implied",
    "active checkpoint joins remain blocked until their required evidence and policy are admitted",
)


@dataclass(frozen=True)
class SourceInfo:
    vibemux_commit: str | None
    tree_dirty: bool | None


@dataclass(frozen=True)
class EnvironmentInfo:
    os: str
    tool_versions: Mapping[str, str | None]
    runtime_roots_isolated: bool


@dataclass(frozen=True)
class CleanupInfo:
    joined_owned_processes: bool | None
    unjoined_processes: Sequence[str]
    # What the suites left in the private temporary root, and what of it is
    # still on disk after the runner's cleanup (every test project and its
    # worktrees live below that root).
    suite_leftovers: Sequence[str]
    retained_entries: Sequence[str]
    private_tmp_removed: bool
    private_tmp_retained_reason: str | None
    # Where retained entries can be found; the root is outside the output
    # directory (see runner_config.PRIVATE_TMP_PREFIX).
    private_tmp_root: str


def build_report(
    *,
    mode: str,
    verdict: str,
    benchmark: Benchmark,
    repository_relative_benchmark: str,
    runner_argv: Sequence[str],
    source: SourceInfo,
    environment: EnvironmentInfo,
    scenarios: Sequence[ScenarioResult],
    commands: Sequence[CommandRecord],
    cargo: CargoTestRun | None,
    calibration: CalibrationResult | None,
    calibration_stdout: Mapping[str, str] | None,
    evidence: EvidenceStore,
    output_root: Path,
    rules: Mapping[tuple[str, str], EvidenceRule],
    live_gate: LiveGate | None,
    cleanup: CleanupInfo,
    extra_limitations: Sequence[str],
    started_at: str,
    finished_at: str,
) -> dict[str, Any]:
    limitations = list(STANDING_LIMITATIONS)
    limitations.extend(extra_limitations)
    for name, error in sorted(evidence.errors.items()):
        limitations.append(f"evidence {name} is unavailable: {error}")
    return {
        "schema_version": REPORT_SCHEMA_VERSION,
        "verdict": verdict,
        "evidence_class": OFFLINE_EVIDENCE_CLASS if mode == MODE_OFFLINE else "live_attempt",
        "source": {
            "vibemux_commit": source.vibemux_commit,
            "vibemux_tree_dirty": source.tree_dirty,
            "gateway_commit": None,
        },
        "environment": {
            "os": environment.os,
            "tool_versions": dict(environment.tool_versions),
            "runtime_roots_isolated": environment.runtime_roots_isolated,
        },
        "route_generation": None,
        "scenarios": [scenario.to_json() for scenario in scenarios],
        "accounting": {
            # Schema 1 keeps model_calls as the live/paid inference count.
            "model_calls": 0,
            "known_cost": 0,
            "unknown_usage": False,
            "fixture_http_requests": None,
            "fixture_usage_is_synthetic": True,
            "basis": "live inference calls and paid usage: 0. Owned local AAG fixture HTTP "
            "requests and synthetic responses may occur; they are excluded from billing counts, "
            "and the fixture HTTP request total is unknown because it is not aggregated",
        },
        "cleanup": {
            "joined_owned_processes": cleanup.joined_owned_processes,
            "unjoined_processes": list(cleanup.unjoined_processes),
            "retained_worktrees": list(cleanup.retained_entries),
            "suite_leftovers": list(cleanup.suite_leftovers),
            "private_tmp_removed": cleanup.private_tmp_removed,
            "private_tmp_retained_reason": cleanup.private_tmp_retained_reason,
            "private_tmp_root": cleanup.private_tmp_root,
        },
        "limitations": limitations,
        "mode": mode,
        "started_at": started_at,
        "finished_at": finished_at,
        "runner": {"argv": list(runner_argv)},
        "inputs": {
            "benchmark_path": repository_relative_benchmark,
            "benchmark_sha256": benchmark.sha256,
        },
        "excluded_claims": list(EXCLUDED_CLAIMS),
        "live_gate": live_gate.to_json() if live_gate is not None else None,
        "commands": [record.to_json() for record in commands],
        **artifact_report_fields(
            evidence, output_root, rules, cargo, calibration, calibration_stdout
        ),
        "validation": None,
    }
