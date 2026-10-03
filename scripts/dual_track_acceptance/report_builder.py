"""Assembles the acceptance report (master prompt section 14 fields first)."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .assessment import MODE_OFFLINE, ScenarioResult
from .check_execution import CalibrationResult, CargoTestRun, CommandRecord
from .evidence_capture import (
    FLAGSHIP,
    OPTIMIZER_CYCLE,
    EvidenceStore,
    candidate_records,
    evidence_classes,
    integration_record,
    json_object,
    review_records,
    session_records,
    verifier_records,
    workflow_summaries,
)
from .live_policy import LiveGate
from .runner_config import EXCLUDED_CLAIMS, REPORT_SCHEMA_VERSION
from .scenario_catalog import Benchmark, EvidenceRule

OFFLINE_EVIDENCE_CLASS = "offline_fixture"

# What no offline run of this implementation establishes, whatever its
# verdict. Kept in step with docs/adr/031_dual_track_workflow.md.
STANDING_LIMITATIONS = (
    "offline evidence only: every worker, reviewer, and compiler turn is a synthetic fixture "
    "process; no model, AAG gateway, or vendor harness was contacted",
    "no daemon AAG gateway client exists: route_generation is null, the compiler route is "
    "unavailable, and the aag_only policy flag is declarative",
    "no native TUI session is owned or attached; sessions are structured-mode only and "
    "native handoff (T02/T03) is not implemented",
    "the optimizer improvement is measured on fixture cases only and implies no model gain",
    "workflow worktrees and materializations are retained by the daemon "
    "(workspace_unsafe_cleanup); the tests' temporary projects, including those worktrees, are "
    "removed with the run's private temporary root",
    "prompt editing of a paused session is not implemented; inspection is read-only",
    "model_requests_remaining is not decremented by live usage because no live usage exists",
    "only template_overrides of an optimizer candidate affect admitted runs",
    "the negative-control verifier receipt of compare mode is not persisted",
    "worker owned paths come from the TaskSpec and are not traced to source requirements",
    "the crash-restart test simulates the crash through the writer, not a killed process",
    "retained content is protected per user (ACL/permissions) only, not against the same user",
    "the gateway side of G02 is a pure contract test, not a gateway fixture",
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


def test_results(
    rules: Mapping[tuple[str, str], EvidenceRule], cargo: CargoTestRun | None
) -> dict[str, dict[str, Any]]:
    """The outcome of every test the catalog cites, as parsed from the log."""
    results: dict[str, dict[str, Any]] = {}
    for rule in rules.values():
        for ref in (*rule.tests, *rule.related):
            outcome = None
            if cargo is not None:
                outcome = cargo.outcome(ref.package, ref.target_kind, ref.target, ref.name)
            results[ref.label()] = {"outcome": outcome or "not_executed"}
    return dict(sorted(results.items()))


def _calibration_view(
    calibration: CalibrationResult | None, stdout_record: Mapping[str, str] | None
) -> dict[str, Any] | None:
    if calibration is None:
        return None
    summary = calibration.summary or {}
    return {
        "error": calibration.error,
        "stdout": dict(stdout_record) if stdout_record else None,
        "good": summary.get("good"),
        "mutants": len(summary.get("mutants", [])),
        "mutants_caught": sum(
            1 for entry in summary.get("mutants", []) if entry.get("expectation_met") is True
        ),
        "base_api_status": summary.get("base_api_status"),
        "contract_stub_api_status": summary.get("contract_stub_api_status"),
        "sources_unchanged": summary.get("sources_unchanged"),
        "browsers_used": summary.get("browsers_used"),
        "all_expectations_met": summary.get("all_expectations_met"),
    }


def _optimization_improved(store: EvidenceStore) -> bool | None:
    item = store.files.get(OPTIMIZER_CYCLE)
    if item is None:
        return None
    report = json_object(json_object(item.document, "value"), "report")
    value = report.get("optimization_improved")
    return value if isinstance(value, bool) else None


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
    flagship = evidence.files.get(FLAGSHIP)
    flagship_status = json_object(flagship.document, "status") if flagship is not None else {}
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
        "workflow_mode": flagship_status.get("mode"),
        "contract_digest": flagship_status.get("start_contract"),
        "policy_digest": flagship_status.get("policy_digest"),
        "route_generation": None,
        "sessions": session_records(evidence),
        "scenarios": [scenario.to_json() for scenario in scenarios],
        "candidate_artifacts": candidate_records(evidence),
        "review_receipts": review_records(evidence),
        "verifier_receipts": verifier_records(evidence),
        "integration_receipt": integration_record(evidence),
        "accounting": {
            "model_calls": 0,
            "known_cost": 0,
            "unknown_usage": False,
            "basis": "no model, gateway, or vendor harness was contacted; every turn was a "
            "synthetic fixture process",
        },
        "optimization_improved": _optimization_improved(evidence),
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
        "test_summary": {
            "totals": cargo.totals() if cargo is not None else None,
            "problems": cargo.problems() if cargo is not None else None,
            "targets": [run.to_json() for run in cargo.targets] if cargo is not None else [],
        },
        "test_results": test_results(rules, cargo),
        "calibration": _calibration_view(calibration, calibration_stdout),
        "evidence_artifacts": evidence.artifacts(output_root),
        "evidence_classes": evidence_classes(evidence),
        "workflows": workflow_summaries(evidence),
        "validation": None,
    }
