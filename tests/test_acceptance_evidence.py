"""Regression tests for acceptance evidence that could otherwise pass empty."""

from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Any

repo_root = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(repo_root / "scripts"))

from dual_track_acceptance.assessment import (  # noqa: E402
    MODE_OFFLINE,
    STATUS_NOT_RUN,
    RunArtifacts,
    assess_class,
)
from dual_track_acceptance.check_execution import (  # noqa: E402
    CHECK_FAIL,
    CHECK_PASS,
    CargoTestRun,
    TargetRun,
    evaluate_calibration_check,
    expected_mutant_suites,
    parse_calibration_output,
)
from dual_track_acceptance.evidence_capture import (  # noqa: E402
    FLAGSHIP,
    EvidenceFile,
    EvidenceStore,
    run_evidence_check,
)
from dual_track_acceptance.scenario_catalog import EvidenceRule, RuleKind  # noqa: E402


def calibration_summary() -> dict[str, Any]:
    return {
        "schema_version": 1,
        "good": {
            "api": "passed",
            "store": "passed",
            "browser": "passed",
            "browser_frontend_only": "passed",
        },
        "mutants": [
            {
                "mutant_id": mutant_id,
                "expected_failing_suite": suite,
                "expectation_met": True,
                "frontend_only_expectation_met": True,
            }
            for mutant_id, suite in expected_mutant_suites.items()
        ],
        "sources_unchanged": True,
        "base_api_status": "failed",
        "contract_stub_api_status": "passed",
        "browsers_used": ["Edge/1"],
        "all_expectations_met": True,
    }


def flagship_integration_store(suite_ids: tuple[str, ...]) -> EvidenceStore:
    task = {
        "task_key": "track_a",
        "progress": "accepted",
        "accepted_candidate": "candidate_a",
        "contract_id": "contract_a",
    }
    integration = {
        "integration_tree_digest": "tree_digest",
        "applied_candidates": ["candidate_a"],
        "conflicts": [],
        "base_commit": "base_commit",
    }
    verification = {
        "task_key": None,
        "subject_digest": "tree_digest",
        "subject_unchanged": True,
        "contract_ids": ["contract_a"],
        "suites": [
            {
                "suite_id": suite_id,
                "status": "passed",
                "tests_failed": 0,
                "tests_total": 1,
                "tests_passed": 1,
                "command": ["node", "run_verifier.mjs", "--suite", suite_id],
            }
            for suite_id in suite_ids
        ],
    }
    document = {
        "schema_version": 1,
        "name": FLAGSHIP,
        "workflow_id": "workflow_a",
        "status": {
            "evidence_class": "fixture",
            "phase": "accepted",
            "tasks": [task],
            "base_commit": "base_commit",
        },
        "export": [
            {"kind": "receipt", "body": {"kind": "integration", "body": integration}},
            {"kind": "receipt", "body": {"kind": "verification", "body": verification}},
        ],
        "sessions": [],
    }
    evidence_file = EvidenceFile(FLAGSHIP, Path(f"{FLAGSHIP}.json"), "0" * 64, document)
    return EvidenceStore({FLAGSHIP: evidence_file}, {}, ())


def test_repository_test_is_not_passed_when_every_test_is_ignored_or_skipped() -> None:
    cargo = CargoTestRun(
        [
            TargetRun(
                package="vibemux",
                target_kind="lib",
                target="vibemux",
                executable="vibemux.exe",
                planned=1,
                outcomes={"ignored_test": "ignored"},
                summary={"passed": 0, "failed": 0, "ignored": 1, "measured": 0, "filtered": 0},
            )
        ],
        unattributed_test_lines=0,
        unknown_executables=[],
        compile_errors=0,
    )
    artifacts = RunArtifacts(
        mode=MODE_OFFLINE,
        is_windows=True,
        cargo=cargo,
        cargo_exit_code=0,
        pytest_summary={"skipped": 1},
        pytest_exit_code=0,
        calibration=None,
        calibration_exit_code=None,
        evidence=EvidenceStore({}, {}, ()),
    )

    result = assess_class("repository_test", EvidenceRule(RuleKind.REPOSITORY), artifacts)

    assert result.status == STATUS_NOT_RUN
    assert "cargo reported no passed tests" in result.reason
    assert "pytest reported no passed tests" in result.reason


def test_calibration_requires_the_exact_mutant_inventory() -> None:
    summary = calibration_summary()
    assert evaluate_calibration_check(summary, "api").status == CHECK_PASS

    omitted = calibration_summary()
    omitted["mutants"] = omitted["mutants"][1:]
    assert evaluate_calibration_check(omitted, "api").status == CHECK_FAIL
    parsed_omitted = parse_calibration_output(json.dumps(omitted))
    assert parsed_omitted.summary is None and "omits expected mutants" in (
        parsed_omitted.error or ""
    )

    relabelled = calibration_summary()
    relabelled["mutants"][0]["expected_failing_suite"] = "api"
    assert evaluate_calibration_check(relabelled, "store").status == CHECK_FAIL
    parsed_relabelled = parse_calibration_output(json.dumps(relabelled))
    assert parsed_relabelled.summary is None and "expected 'store'" in (
        parsed_relabelled.error or ""
    )


def test_integration_evidence_requires_all_pinned_flagship_suites() -> None:
    empty = run_evidence_check(flagship_integration_store(()), "flagship_integration_verified")
    assert empty.status == CHECK_FAIL

    incomplete = run_evidence_check(
        flagship_integration_store(("api", "store")), "flagship_integration_verified"
    )
    assert incomplete.status == CHECK_FAIL

    complete = run_evidence_check(
        flagship_integration_store(("api", "store", "browser")),
        "flagship_integration_verified",
    )
    assert complete.status == CHECK_PASS
