"""E01: an acceptance report is valid only if it follows from its artifacts.

Validation never trusts a status written in the report. It re-hashes every
referenced log and evidence file, re-parses the logs, re-runs the evidence
checks, re-derives every class status and the verdict, and compares them
with what the report claims.
"""

from __future__ import annotations

from collections.abc import Mapping
from pathlib import Path
from typing import Any

from .assessment import (
    LIVE_NOT_IMPLEMENTED,
    MODE_LIVE,
    MODE_OFFLINE,
    STATUS_BLOCKED,
    STATUS_FAIL,
    STATUS_PASS,
    STATUSES,
    VERDICT_BLOCKED,
    VERDICT_PASS,
    VERDICTS,
    RunArtifacts,
    assess_scenarios,
    decide_verdict,
    worst_status,
)
from .check_execution import (
    CalibrationResult,
    file_sha256,
    parse_calibration_output,
    parse_cargo_test_output,
    parse_pytest_summary,
    read_log_lines,
)
from .evidence_capture import EVIDENCE_CHECKS, FIXTURE_EVIDENCE_CLASS, load_evidence
from .runner_config import EVIDENCE_DIR_NAME, EXCLUDED_CLAIMS
from .scenario_catalog import LIVE_CLASSES, Benchmark, EvidenceRule

E01_ID = "E01"
CARGO_COMMAND = "cargo_test"
PYTEST_COMMAND = "pytest"
CALIBRATION_COMMAND = "calibration"

# Fields the master prompt's report template requires.
TEMPLATE_FIELDS = (
    "schema_version",
    "verdict",
    "evidence_class",
    "source",
    "environment",
    "workflow_mode",
    "contract_digest",
    "policy_digest",
    "route_generation",
    "sessions",
    "scenarios",
    "candidate_artifacts",
    "review_receipts",
    "verifier_receipts",
    "integration_receipt",
    "accounting",
    "optimization_improved",
    "cleanup",
    "limitations",
)


def _verified_file(
    report_dir: Path, relative: Any, expected_sha256: Any, label: str, problems: list[str]
) -> Path | None:
    if not isinstance(relative, str) or not isinstance(expected_sha256, str):
        problems.append(f"{label}: no file reference")
        return None
    path = (report_dir / relative).resolve()
    try:
        path.relative_to(report_dir.resolve())
    except ValueError:
        problems.append(f"{label}: {relative} is outside the report directory")
        return None
    if not path.is_file():
        problems.append(f"{label}: {relative} is missing")
        return None
    if file_sha256(path) != expected_sha256:
        problems.append(f"{label}: {relative} does not match its recorded digest")
        return None
    return path


def load_run_artifacts(
    report: Mapping[str, Any], report_dir: Path, problems: list[str]
) -> RunArtifacts:
    """Rebuilds what the run executed from the files the report cites."""
    commands = {command.get("command_id"): command for command in report.get("commands", [])}
    logs: dict[str, Path | None] = {}
    for command_id, command in commands.items():
        logs[str(command_id)] = _verified_file(
            report_dir,
            command.get("log_path"),
            command.get("log_sha256"),
            f"log {command_id}",
            problems,
        )
    cargo_log = logs.get(CARGO_COMMAND)
    cargo = parse_cargo_test_output(read_log_lines(cargo_log)) if cargo_log is not None else None
    pytest_log = logs.get(PYTEST_COMMAND)
    pytest_summary = parse_pytest_summary(read_log_lines(pytest_log)) if pytest_log else None
    calibration: CalibrationResult | None = None
    view = report.get("calibration")
    if isinstance(view, dict) and isinstance(view.get("stdout"), dict):
        stdout = _verified_file(
            report_dir,
            view["stdout"].get("path"),
            view["stdout"].get("sha256"),
            "calibration stdout",
            problems,
        )
        if stdout is not None:
            calibration = parse_calibration_output(stdout.read_text(encoding="utf-8"))
    evidence = load_evidence(report_dir / EVIDENCE_DIR_NAME)
    recorded = {item.get("name"): item for item in report.get("evidence_artifacts", [])}
    actual = {item["name"]: item for item in evidence.artifacts(report_dir.resolve())}
    if recorded != actual:
        problems.append("evidence_artifacts do not match the evidence files on disk")
    if evidence.unexpected:
        problems.append(f"unexpected evidence files {list(evidence.unexpected)}")
    live_gate = report.get("live_gate") or {}
    return RunArtifacts(
        mode=str(report.get("mode")),
        is_windows=str(report.get("environment", {}).get("os", "")).startswith("Windows"),
        cargo=cargo,
        cargo_exit_code=commands.get(CARGO_COMMAND, {}).get("exit_code"),
        pytest_summary=pytest_summary,
        pytest_exit_code=commands.get(PYTEST_COMMAND, {}).get("exit_code"),
        calibration=calibration,
        calibration_exit_code=commands.get(CALIBRATION_COMMAND, {}).get("exit_code"),
        evidence=evidence,
        live_gate_reason=LIVE_NOT_IMPLEMENTED if live_gate.get("gate_open") else None,
    )


def _reference_problems(report: Mapping[str, Any], reference: str) -> str | None:
    kind, _, rest = reference.partition(":")
    if kind == "cargo":
        outcome = report.get("test_results", {}).get(reference, {}).get("outcome")
        return None if outcome == "ok" else f"{reference} is {outcome}"
    if kind == "evidence":
        name, _, digest = rest.partition(":sha256:")
        for item in report.get("evidence_artifacts", []):
            if item.get("name") == name and item.get("sha256") == digest:
                return None
        return f"{reference} names no recorded evidence file"
    if kind == "command":
        for command in report.get("commands", []):
            if command.get("command_id") == rest and command.get("exit_code") == 0:
                return None
        return f"{reference} names no successful command"
    if kind == "calibration":
        view = report.get("calibration")
        return (
            None if isinstance(view, dict) and view.get("stdout") else f"{reference} has no output"
        )
    if kind == "check":
        return None if rest in EVIDENCE_CHECKS else f"{reference} is not a known check"
    if kind == "validation":
        return None
    return f"{reference} has an unknown kind"


def structural_problems(
    report: Mapping[str, Any], benchmark: Benchmark, *, final: bool
) -> list[str]:
    problems: list[str] = []
    for key in TEMPLATE_FIELDS:
        if key not in report:
            problems.append(f"missing field {key}")
    if report.get("schema_version") != 1:
        problems.append("schema_version is not 1")
    if final and report.get("verdict") not in VERDICTS:
        problems.append(f"verdict {report.get('verdict')} is not allowed")
    mode = report.get("mode")
    if mode not in (MODE_OFFLINE, MODE_LIVE):
        problems.append(f"unknown mode {mode}")
    if report.get("inputs", {}).get("benchmark_sha256") != benchmark.sha256:
        problems.append("the report was produced against a different benchmark")
    scenarios = report.get("scenarios", [])
    expected = [(item.scenario_id, list(item.execution_classes)) for item in benchmark.scenarios]
    actual = [
        (
            scenario.get("id"),
            [entry.get("execution_class") for entry in scenario.get("classes", [])],
        )
        for scenario in scenarios
    ]
    if actual != expected:
        problems.append("scenarios or execution classes differ from the pinned benchmark")
    for scenario in scenarios:
        statuses = [entry.get("status") for entry in scenario.get("classes", [])]
        for entry in scenario.get("classes", []):
            label = f"{scenario.get('id')}/{entry.get('execution_class')}"
            if entry.get("status") not in STATUSES:
                problems.append(f"{label}: status {entry.get('status')} is not allowed")
            if not final and scenario.get("id") == E01_ID:
                continue
            if entry.get("status") == STATUS_PASS:
                if not entry.get("evidence"):
                    problems.append(f"{label}: PASS without evidence")
                for reference in entry.get("evidence", []):
                    problem = _reference_problems(report, reference)
                    if problem is not None:
                        problems.append(f"{label}: {problem}")
                if mode == MODE_OFFLINE and entry.get("execution_class") in LIVE_CLASSES:
                    problems.append(f"{label}: a live class passed in offline mode")
            if not entry.get("reason"):
                problems.append(f"{label}: no reason")
        if final and scenario.get("status") != worst_status(statuses):
            problems.append(f"{scenario.get('id')}: status does not follow from its classes")
    if mode == MODE_OFFLINE:
        if report.get("verdict") == VERDICT_PASS:
            problems.append("an offline run claims full PASS")
        if report.get("accounting", {}).get("model_calls") != 0:
            problems.append("an offline run reports model calls")
        for field in ("sessions", "candidate_artifacts", "review_receipts", "verifier_receipts"):
            for record in report.get(field, []):
                if record.get("evidence_class") != FIXTURE_EVIDENCE_CLASS:
                    problems.append(f"{field}: a non-fixture record in an offline run")
        if report.get("evidence_classes") not in ([], [FIXTURE_EVIDENCE_CLASS]):
            problems.append(f"offline evidence classes {report.get('evidence_classes')}")
    if list(report.get("excluded_claims", [])) != list(EXCLUDED_CLAIMS):
        problems.append("excluded claims are missing or altered")
    if not report.get("limitations"):
        problems.append("no limitations are recorded")
    return problems


def reconciliation_problems(
    report: Mapping[str, Any],
    report_dir: Path,
    benchmark: Benchmark,
    rules: Mapping[tuple[str, str], EvidenceRule],
) -> list[str]:
    """Re-derives every non-E01 class from the artifacts and compares."""
    problems: list[str] = []
    artifacts = load_run_artifacts(report, report_dir, problems)
    derived = {
        (scenario.scenario_id, result.execution_class): result
        for scenario in assess_scenarios(benchmark, rules, artifacts)
        for result in scenario.classes
    }
    for scenario in report.get("scenarios", []):
        if scenario.get("id") == E01_ID:
            continue
        for entry in scenario.get("classes", []):
            key = (str(scenario.get("id")), str(entry.get("execution_class")))
            result = derived.get(key)
            if result is None:
                continue
            if entry.get("status") != result.status:
                problems.append(
                    f"{key[0]}/{key[1]}: report says {entry.get('status')}, artifacts give "
                    f"{result.status}"
                )
            elif sorted(entry.get("evidence", [])) != sorted(result.evidence):
                problems.append(f"{key[0]}/{key[1]}: evidence differs from the artifacts")
    return problems


def validate_report(
    report: Mapping[str, Any],
    report_dir: Path,
    benchmark: Benchmark,
    rules: Mapping[tuple[str, str], EvidenceRule],
    *,
    final: bool = True,
) -> list[str]:
    """All problems of a report. A draft (`final=False`) skips E01 and the
    verdict, which depend on this very result."""
    problems = structural_problems(report, benchmark, final=final)
    if report.get("blocked_reason"):
        # Nothing ran: every class must say so, and nothing reconciles.
        if report.get("commands"):
            problems.append("a blocked run records executed commands")
        for scenario in report.get("scenarios", []):
            for entry in scenario.get("classes", []):
                if entry.get("status") != STATUS_BLOCKED:
                    problems.append(f"{scenario.get('id')}: a blocked run has a non-BLOCKED class")
        if report.get("verdict") != VERDICT_BLOCKED:
            problems.append("a blocked run has a verdict other than BLOCKED")
        return problems
    problems.extend(reconciliation_problems(report, report_dir, benchmark, rules))
    if not final:
        return problems
    e01 = next((item for item in report.get("scenarios", []) if item.get("id") == E01_ID), None)
    e01_status = e01.get("status") if e01 is not None else None
    if e01_status == STATUS_PASS and problems:
        problems.append("E01 passed although validation found problems")
    if e01_status == STATUS_FAIL and not problems:
        problems.append("E01 failed although validation found no problems")
    verdict = decide_verdict(str(report.get("mode")), report.get("scenarios", []), LIVE_CLASSES)
    if report.get("verdict") != verdict:
        problems.append(f"verdict {report.get('verdict')} does not follow (expected {verdict})")
    return problems
