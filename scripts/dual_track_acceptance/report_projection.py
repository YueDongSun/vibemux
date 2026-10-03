"""Report fields derived from command logs and daemon-exported evidence."""

from __future__ import annotations

from collections.abc import Mapping
from pathlib import Path
from typing import Any

from .check_execution import CalibrationResult, CargoTestRun
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
from .scenario_catalog import EvidenceRule


def test_results(
    rules: Mapping[tuple[str, str], EvidenceRule], cargo: CargoTestRun | None
) -> dict[str, dict[str, Any]]:
    """The outcome of every catalog test, read from its attributed log."""
    results: dict[str, dict[str, Any]] = {}
    for rule in rules.values():
        for ref in (*rule.tests, *rule.related):
            outcome = (
                cargo.outcome(ref.package, ref.target_kind, ref.target, ref.name) if cargo else None
            )
            results[ref.label()] = {"outcome": outcome or "not_executed"}
    return dict(sorted(results.items()))


def calibration_view(
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


def artifact_report_fields(
    evidence: EvidenceStore,
    output_root: Path,
    rules: Mapping[tuple[str, str], EvidenceRule],
    cargo: CargoTestRun | None,
    calibration: CalibrationResult | None,
    calibration_stdout: Mapping[str, str] | None,
) -> dict[str, Any]:
    """One projection shared by report construction and independent validation."""
    flagship = evidence.files.get(FLAGSHIP)
    status = json_object(flagship.document, "status") if flagship else {}
    optimizer = evidence.files.get(OPTIMIZER_CYCLE)
    optimizer_report = (
        json_object(json_object(optimizer.document, "value"), "report") if optimizer else {}
    )
    improved = optimizer_report.get("optimization_improved")
    return {
        "workflow_mode": status.get("mode"),
        "contract_digest": status.get("start_contract"),
        "policy_digest": status.get("policy_digest"),
        "sessions": session_records(evidence),
        "candidate_artifacts": candidate_records(evidence),
        "review_receipts": review_records(evidence),
        "verifier_receipts": verifier_records(evidence),
        "integration_receipt": integration_record(evidence),
        "optimization_improved": improved if isinstance(improved, bool) else None,
        "evidence_artifacts": evidence.artifacts(output_root),
        "evidence_classes": evidence_classes(evidence),
        "workflows": workflow_summaries(evidence),
        "test_summary": {
            "totals": cargo.totals() if cargo else None,
            "problems": cargo.problems() if cargo else None,
            "targets": [run.to_json() for run in cargo.targets] if cargo else [],
        },
        "test_results": test_results(rules, cargo),
        "calibration": calibration_view(calibration, calibration_stdout),
    }
