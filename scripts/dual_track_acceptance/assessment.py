"""Derives every (scenario, execution class) status from executed artifacts.

Pure logic: the inputs are parsed test logs, the calibration summary, and
the exported evidence; the same function runs when the report is written
and when `--validate` re-derives it from the files on disk, so a report
whose statuses do not follow from its artifacts fails validation.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from typing import Any

from .check_execution import (
    CHECK_BLOCKED,
    CHECK_FAIL,
    OUTCOME_FAILED,
    OUTCOME_IGNORED,
    OUTCOME_OK,
    CalibrationResult,
    CargoTestRun,
    evaluate_calibration_check,
)
from .evidence_capture import (
    EVIDENCE_CHECKS,
    EvidenceStore,
    evidence_reference,
    run_evidence_check,
)
from .scenario_catalog import Benchmark, CargoTestRef, EvidenceRule, RuleKind

STATUS_PASS = "PASS"
STATUS_FAIL = "FAIL"
STATUS_BLOCKED = "BLOCKED"
STATUS_NOT_RUN = "NOT_RUN"
STATUSES = (STATUS_PASS, STATUS_FAIL, STATUS_BLOCKED, STATUS_NOT_RUN)
# The worst status of a scenario's classes is the scenario's status.
STATUS_PRECEDENCE = (STATUS_FAIL, STATUS_NOT_RUN, STATUS_BLOCKED, STATUS_PASS)

VERDICT_PASS = "PASS"
VERDICT_OFFLINE_PASS = "OFFLINE_PASS"
VERDICT_FAIL = "FAIL"
VERDICT_BLOCKED = "BLOCKED"
VERDICT_NOT_RUN = "NOT_RUN"
VERDICTS = (VERDICT_PASS, VERDICT_OFFLINE_PASS, VERDICT_FAIL, VERDICT_BLOCKED, VERDICT_NOT_RUN)

MODE_OFFLINE = "offline"
MODE_LIVE = "live"

LIVE_NOT_IMPLEMENTED = (
    "the daemon has no AAG gateway client and no owned native TUI sessions, so no live turn can run"
)


@dataclass(frozen=True)
class RunArtifacts:
    """What one acceptance run executed, as parsed from its own files."""

    mode: str
    is_windows: bool
    cargo: CargoTestRun | None
    cargo_exit_code: int | None
    pytest_summary: Mapping[str, int] | None
    pytest_exit_code: int | None
    calibration: CalibrationResult | None
    calibration_exit_code: int | None
    evidence: EvidenceStore
    live_gate_reason: str | None = None


@dataclass
class ClassResult:
    execution_class: str
    status: str
    reason: str
    evidence: list[str] = field(default_factory=list)
    related: list[str] = field(default_factory=list)

    def to_json(self) -> dict[str, Any]:
        return {
            "execution_class": self.execution_class,
            "status": self.status,
            "reason": self.reason,
            "evidence": list(self.evidence),
            "related": list(self.related),
        }


def worst_status(statuses: Sequence[str]) -> str:
    for status in STATUS_PRECEDENCE:
        if status in statuses:
            return status
    return STATUS_NOT_RUN


def _test_outcome(cargo: CargoTestRun | None, ref: CargoTestRef) -> tuple[str | None, list[str]]:
    """The test's outcome and the problems of the target that ran it."""
    if cargo is None:
        return None, []
    runs = cargo.find(ref.package, ref.target_kind, ref.target)
    problems = [problem for run in runs for problem in run.problems()]
    return cargo.outcome(ref.package, ref.target_kind, ref.target, ref.name), problems


def _assess_tests(
    tests: Sequence[CargoTestRef], cargo: CargoTestRun | None
) -> tuple[str, list[str], list[str]]:
    failed: list[str] = []
    missing: list[str] = []
    incomplete: list[str] = []
    evidence: list[str] = []
    for ref in tests:
        outcome, problems = _test_outcome(cargo, ref)
        if problems:
            incomplete.append(f"{ref.label()} ({problems[0]})")
        if outcome == OUTCOME_OK:
            evidence.append(ref.label())
        elif outcome == OUTCOME_FAILED:
            failed.append(ref.label())
        elif outcome == OUTCOME_IGNORED:
            missing.append(f"{ref.label()} (ignored)")
        else:
            missing.append(f"{ref.label()} (not executed)")
    reasons: list[str] = []
    if failed:
        reasons.append(f"failed: {failed}")
    if incomplete:
        reasons.append(f"incomplete targets: {incomplete}")
    if missing:
        reasons.append(f"no result: {missing}")
    if failed or incomplete:
        return STATUS_FAIL, reasons, evidence
    if missing:
        return STATUS_NOT_RUN, reasons, evidence
    return STATUS_PASS, [f"{len(tests)} tests passed"], evidence


def _assess_evidence(
    checks: Sequence[str], store: EvidenceStore
) -> tuple[str, list[str], list[str]]:
    reasons: list[str] = []
    evidence: list[str] = []
    status = STATUS_PASS
    for check in checks:
        outcome = run_evidence_check(store, check)
        names = EVIDENCE_CHECKS[check][0] if check in EVIDENCE_CHECKS else ()
        if outcome.status == CHECK_FAIL:
            status = STATUS_FAIL
            reasons.append(f"{check} failed: {outcome.reason}")
        else:
            reasons.append(f"{check}: {outcome.reason}")
            evidence.append(f"check:{check}")
            evidence.extend(evidence_reference(store, name) for name in names)
    return status, reasons, evidence


def _assess_calibration(
    checks: Sequence[str], calibration: CalibrationResult | None, exit_code: int | None
) -> tuple[str, list[str], list[str]]:
    if calibration is None:
        return STATUS_NOT_RUN, ["calibration did not run"], []
    if calibration.summary is None:
        return STATUS_FAIL, [f"calibration output unusable: {calibration.error}"], []
    statuses: list[str] = []
    reasons: list[str] = []
    evidence: list[str] = []
    for check in checks:
        outcome = evaluate_calibration_check(calibration.summary, check)
        reasons.append(f"calibration {check}: {outcome.reason}")
        if outcome.status == CHECK_FAIL:
            statuses.append(STATUS_FAIL)
        elif outcome.status == CHECK_BLOCKED:
            statuses.append(STATUS_BLOCKED)
        else:
            statuses.append(STATUS_PASS)
            evidence.append(f"calibration:{check}")
    if exit_code not in (0, 1):
        statuses.append(STATUS_FAIL)
        reasons.append(f"calibration exited {exit_code}")
    return worst_status(statuses), reasons, evidence


def _assess_repository(artifacts: RunArtifacts) -> ClassResult:
    reasons: list[str] = []
    status = STATUS_PASS
    cargo = artifacts.cargo
    if cargo is None:
        return ClassResult("repository_test", STATUS_NOT_RUN, "cargo test did not run")
    totals = cargo.totals()
    problems = cargo.problems()
    failed_targets = [run.key() for run in cargo.targets if run.counts()["failed"]]
    if artifacts.cargo_exit_code != 0 or problems or failed_targets:
        status = STATUS_FAIL
        reasons.append(
            f"cargo test exited {artifacts.cargo_exit_code}; failing targets {failed_targets}; "
            f"problems {problems[:5]}"
        )
    reasons.append(
        f"cargo: {totals['passed']} passed, {totals['failed']} failed, {totals['ignored']} ignored "
        f"in {totals['targets']} targets"
    )
    summary = artifacts.pytest_summary
    if summary is None or artifacts.pytest_exit_code is None:
        status = STATUS_FAIL if status == STATUS_FAIL else STATUS_NOT_RUN
        reasons.append("pytest did not report a summary")
    else:
        bad = summary.get("failed", 0) + summary.get("errors", 0)
        if artifacts.pytest_exit_code != 0 or bad:
            status = STATUS_FAIL
        reasons.append(f"pytest exited {artifacts.pytest_exit_code}: {dict(summary)}")
    evidence = ["command:cargo_test", "command:pytest"] if status == STATUS_PASS else []
    return ClassResult("repository_test", status, "; ".join(reasons), evidence)


def assess_class(execution_class: str, rule: EvidenceRule, artifacts: RunArtifacts) -> ClassResult:
    """One class's status. E01 (report validation) is decided by the caller."""
    if rule.kind is RuleKind.LIVE:
        if artifacts.mode != MODE_LIVE:
            reason = f"live mode was not requested; {rule.reason}"
        else:
            reason = f"{artifacts.live_gate_reason or LIVE_NOT_IMPLEMENTED}; {rule.reason}"
        return ClassResult(execution_class, STATUS_BLOCKED, reason)
    if rule.kind is RuleKind.NOT_IMPLEMENTED:
        related = []
        for ref in rule.related:
            outcome, _ = _test_outcome(artifacts.cargo, ref)
            related.append(f"{ref.label()} = {outcome or 'not executed'}")
        return ClassResult(execution_class, STATUS_NOT_RUN, rule.reason, related=related)
    if rule.kind is RuleKind.REPOSITORY:
        return _assess_repository(artifacts)
    if rule.kind is RuleKind.REPORT_VALIDATION:
        return ClassResult(execution_class, STATUS_NOT_RUN, "decided by report validation")
    if rule.requires_windows and not artifacts.is_windows:
        return ClassResult(execution_class, STATUS_BLOCKED, "needs native Windows")
    statuses: list[str] = []
    reasons: list[str] = []
    evidence: list[str] = []
    if rule.kind is RuleKind.CALIBRATION:
        status, why, refs = _assess_calibration(
            rule.calibration_checks, artifacts.calibration, artifacts.calibration_exit_code
        )
        statuses.append(status)
        reasons.extend(why)
        evidence.extend(refs)
    if rule.tests:
        status, why, refs = _assess_tests(rule.tests, artifacts.cargo)
        statuses.append(status)
        reasons.extend(why)
        evidence.extend(refs)
    if rule.evidence_checks:
        status, why, refs = _assess_evidence(rule.evidence_checks, artifacts.evidence)
        statuses.append(status)
        reasons.extend(why)
        evidence.extend(refs)
    status = worst_status(statuses)
    return ClassResult(
        execution_class, status, "; ".join(reasons), evidence if status == STATUS_PASS else []
    )


@dataclass
class ScenarioResult:
    scenario_id: str
    description: str
    required_for_full_pass: bool
    classes: list[ClassResult]

    @property
    def status(self) -> str:
        return worst_status([result.status for result in self.classes])

    def to_json(self) -> dict[str, Any]:
        return {
            "id": self.scenario_id,
            "description": self.description,
            "required_for_full_pass": self.required_for_full_pass,
            "required_execution_classes": [result.execution_class for result in self.classes],
            "status": self.status,
            "classes": [result.to_json() for result in self.classes],
            "evidence": sorted({ref for result in self.classes for ref in result.evidence}),
            "reason": " | ".join(
                f"{result.execution_class}: {result.status}: {result.reason}"
                for result in self.classes
            ),
        }


def assess_scenarios(
    benchmark: Benchmark,
    rules: Mapping[tuple[str, str], EvidenceRule],
    artifacts: RunArtifacts,
) -> list[ScenarioResult]:
    return [
        ScenarioResult(
            scenario.scenario_id,
            scenario.description,
            scenario.required_for_full_pass,
            [
                assess_class(
                    execution_class, rules[(scenario.scenario_id, execution_class)], artifacts
                )
                for execution_class in scenario.execution_classes
            ],
        )
        for scenario in benchmark.scenarios
    ]


def decide_verdict(
    mode: str, scenarios: Sequence[Mapping[str, Any]], live_classes: frozenset[str]
) -> str:
    """The run verdict from serialized scenario records.

    FAIL on any failure; otherwise OFFLINE_PASS when an offline run passed
    every non-live class of every required scenario, PASS only when a live
    run passed every class, and BLOCKED for anything missing.
    """
    classes = [
        (entry["execution_class"], entry["status"])
        for scenario in scenarios
        if scenario.get("required_for_full_pass", True)
        for entry in scenario["classes"]
    ]
    if any(status == STATUS_FAIL for _, status in classes):
        return VERDICT_FAIL
    if mode == MODE_LIVE:
        return (
            VERDICT_PASS if all(status == STATUS_PASS for _, status in classes) else VERDICT_BLOCKED
        )
    offline = [status for execution_class, status in classes if execution_class not in live_classes]
    if offline and all(status == STATUS_PASS for status in offline):
        return VERDICT_OFFLINE_PASS
    return VERDICT_BLOCKED
