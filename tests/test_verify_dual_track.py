"""Pure-logic tests of the dual-track acceptance runner (scripts/verify_dual_track.py)."""

from __future__ import annotations

import json
import stat
import sys
from pathlib import Path
from typing import Any

import pytest

REPO_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO_ROOT / "scripts"))

from dual_track_acceptance.assessment import (  # noqa: E402
    MODE_LIVE,
    MODE_OFFLINE,
    STATUS_BLOCKED,
    STATUS_FAIL,
    STATUS_NOT_RUN,
    STATUS_PASS,
    VERDICT_BLOCKED,
    VERDICT_FAIL,
    VERDICT_OFFLINE_PASS,
    VERDICT_PASS,
    RunArtifacts,
    assess_class,
    decide_verdict,
    worst_status,
)
from dual_track_acceptance.check_execution import (  # noqa: E402
    CHECK_BLOCKED,
    CHECK_FAIL,
    CHECK_PASS,
    CommandRecord,
    evaluate_calibration_check,
    file_sha256,
    package_name,
    parse_cargo_test_output,
    parse_pytest_summary,
)
from dual_track_acceptance.evidence_capture import (  # noqa: E402
    FLAGSHIP,
    OPTIMIZER_CYCLE,
    EvidenceFile,
    EvidenceStore,
    candidate_records,
    run_evidence_check,
    session_records,
    verifier_records,
    workflow_summaries,
)
from dual_track_acceptance.live_policy import evaluate_live_gate, policy_problems  # noqa: E402
from dual_track_acceptance.report_validation import validate_report  # noqa: E402
from dual_track_acceptance.runner import private_tmp_retention_reason, validate  # noqa: E402
from dual_track_acceptance.runner_config import (  # noqa: E402
    BENCHMARK_PATH,
    EVIDENCE_DIR_NAME,
    EXIT_FAIL,
    MAX_PRIVATE_TMP_PATH_CHARS,
    PRIVATE_TMP_PREFIX,
)
from dual_track_acceptance.runtime_roots import (  # noqa: E402
    OutputError,
    OutputLayout,
    prepare_output,
    private_tmp_problems,
    remove_private_tmp,
)
from dual_track_acceptance.scenario_catalog import (  # noqa: E402
    LIVE_CLASSES,
    RULES,
    CargoTestRef,
    CatalogError,
    EvidenceRule,
    RuleKind,
    check_catalog,
    load_benchmark,
)

STORE_ID = "path+file:///repo/crates/vibemux_store#0.2.0"
DAEMON_ID = "path+file:///repo/crates/vibemuxd#0.2.0"


def artifact(package_id: str, kind: str, name: str, executable: str) -> str:
    return json.dumps(
        {
            "reason": "compiler-artifact",
            "package_id": package_id,
            "target": {"kind": [kind], "name": name},
            "profile": {"test": True},
            "executable": executable,
        }
    )


def cargo_output() -> list[str]:
    """Two integration tests named `harness_dispatch` in different crates."""
    return [
        artifact(STORE_ID, "test", "harness_dispatch", "C:\\t\\deps\\harness_dispatch-1.exe"),
        artifact(DAEMON_ID, "test", "harness_dispatch", "C:\\t\\deps\\harness_dispatch-2.exe"),
        artifact(DAEMON_ID, "lib", "vibemuxd", "C:\\t\\deps\\vibemuxd-3.exe"),
        "     Running tests\\harness_dispatch.rs (C:/t\\deps\\harness_dispatch-1.exe)",
        "",
        "running 1 test",
        "test records_survive ... ok",
        "",
        "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished",
        "     Running tests\\harness_dispatch.rs (C:/t\\deps\\harness_dispatch-2.exe)",
        "running 2 tests",
        "test an_acp_route_is_probe_only ... ok",
        "test slow_one ... ignored, needs a vendor CLI",
        "test result: ok. 1 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished",
        "     Running unittests src\\lib.rs (C:/t\\deps\\vibemuxd-3.exe)",
        "running 1 test",
        "test process::tests::heals ... FAILED",
        "test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out",
        "   Doc-tests vibemux_store",
        "running 1 test",
        "test crates\\vibemux_store\\src\\lib.rs - open (line 3) ... ok",
        "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out",
    ]


def test_the_catalog_covers_the_pinned_benchmark_exactly() -> None:
    benchmark = load_benchmark(REPO_ROOT / BENCHMARK_PATH)
    check_catalog(benchmark, RULES)
    assert len(benchmark.scenarios) == 37
    assert all(scenario.required_for_full_pass for scenario in benchmark.scenarios)
    with pytest.raises(CatalogError):
        check_catalog(benchmark, {key: rule for key, rule in RULES.items() if key[0] != "W03"})


def test_live_classes_can_only_be_live_rules() -> None:
    benchmark = load_benchmark(REPO_ROOT / BENCHMARK_PATH)
    forged = dict(RULES)
    forged[("S01", "live_coding")] = EvidenceRule(RuleKind.TESTS)
    with pytest.raises(CatalogError):
        check_catalog(benchmark, forged)


def test_cargo_output_is_attributed_by_compiler_artifact() -> None:
    run = parse_cargo_test_output(cargo_output())
    assert run.outcome("vibemux_store", "test", "harness_dispatch", "records_survive") == "ok"
    assert run.outcome("vibemuxd", "test", "harness_dispatch", "records_survive") is None
    assert run.outcome("vibemuxd", "test", "harness_dispatch", "an_acp_route_is_probe_only") == "ok"
    assert run.outcome("vibemuxd", "test", "harness_dispatch", "slow_one") == "ignored"
    assert run.outcome("vibemuxd", "lib", "vibemuxd", "process::tests::heals") == "failed"
    assert run.totals() == {"targets": 4, "passed": 3, "failed": 1, "ignored": 1}
    assert run.problems() == []


def test_unreconciled_or_unfinished_targets_are_problems() -> None:
    lines = cargo_output()
    # Drop the store target's summary and forge an extra passing line.
    lines = [
        line for line in lines if "1 passed; 0 failed; 0 ignored" not in line or "ok." not in line
    ]
    lines.insert(11, "test forged ... ok")
    run = parse_cargo_test_output(lines)
    problems = run.problems()
    assert any("no test result line" in problem for problem in problems)
    assert any("passed: 2 test lines but the summary says 1" in problem for problem in problems)


def test_unknown_test_binaries_are_problems() -> None:
    run = parse_cargo_test_output(["     Running tests\\x.rs (C:/t/deps/x-9.exe)"])
    assert any("without a compiler artifact" in problem for problem in run.problems())


@pytest.mark.parametrize(
    ("package_id", "expected"),
    [
        ("path+file:///repo/crates/vibemuxd#0.2.0", "vibemuxd"),
        ("path+file:///repo/crates/dir#vibemux_cli@0.2.0", "vibemux_cli"),
        ("registry+https://github.com/rust-lang/crates.io-index#serde@1.0.0", "serde"),
        ("vibemuxd 0.2.0 (path+file:///repo/crates/vibemuxd)", "vibemuxd"),
    ],
)
def test_package_names_are_read_from_package_ids(package_id: str, expected: str) -> None:
    assert package_name(package_id) == expected


def test_pytest_summary_counts_are_parsed() -> None:
    lines = ["....", "3 failed, 120 passed, 2 skipped, 1 error in 4.20s"]
    assert parse_pytest_summary(lines) == {"failed": 3, "passed": 120, "skipped": 2, "errors": 1}
    assert parse_pytest_summary(["no summary here"]) is None


def calibration_summary(**overrides: Any) -> dict[str, Any]:
    summary: dict[str, Any] = {
        "schema_version": 1,
        "good": {
            "api": "passed",
            "store": "passed",
            "browser": "passed",
            "browser_frontend_only": "passed",
        },
        "mutants": [
            {"mutant_id": "a", "expected_failing_suite": "api", "expectation_met": True},
            {"mutant_id": "s", "expected_failing_suite": "store", "expectation_met": True},
            {
                "mutant_id": "b",
                "expected_failing_suite": "browser",
                "expectation_met": True,
                "frontend_only_expectation_met": True,
            },
        ],
        "base_api_status": "failed",
        "contract_stub_api_status": "passed",
        "sources_unchanged": True,
        "browsers_used": ["msedge 130"],
        "all_expectations_met": True,
    }
    summary.update(overrides)
    return summary


def test_calibration_checks_need_the_good_solution_and_every_mutant() -> None:
    for check in ("api", "store", "browser"):
        assert evaluate_calibration_check(calibration_summary(), check).status == CHECK_PASS
    blocked = calibration_summary(
        good={
            "api": "passed",
            "store": "passed",
            "browser": "blocked",
            "browser_frontend_only": "x",
        }
    )
    assert evaluate_calibration_check(blocked, "browser").status == CHECK_BLOCKED
    missed = calibration_summary()
    missed["mutants"][1]["expectation_met"] = False
    assert evaluate_calibration_check(missed, "store").status == CHECK_FAIL
    assert evaluate_calibration_check(missed, "api").status == CHECK_PASS
    changed = calibration_summary(sources_unchanged=False)
    assert evaluate_calibration_check(changed, "api").status == CHECK_FAIL
    stub = calibration_summary(contract_stub_api_status="failed")
    assert evaluate_calibration_check(stub, "api").status == CHECK_FAIL


def artifacts(mode: str = MODE_OFFLINE, *, is_windows: bool = True) -> RunArtifacts:
    return RunArtifacts(
        mode=mode,
        is_windows=is_windows,
        cargo=parse_cargo_test_output(cargo_output()),
        cargo_exit_code=101,
        pytest_summary={"passed": 1},
        pytest_exit_code=0,
        calibration=None,
        calibration_exit_code=None,
        evidence=EvidenceStore({}, {}, ()),
    )


def test_class_statuses_never_turn_missing_evidence_into_a_pass() -> None:
    passing = CargoTestRef("vibemux_store", "test", "harness_dispatch", "records_survive")
    failing = CargoTestRef("vibemuxd", "lib", "vibemuxd", "process::tests::heals")
    ignored = CargoTestRef("vibemuxd", "test", "harness_dispatch", "slow_one")
    absent = CargoTestRef("vibemuxd", "test", "harness_dispatch", "absent")
    run = artifacts()

    def status(rule: EvidenceRule, execution_class: str = "deterministic_test") -> str:
        return assess_class(execution_class, rule, run).status

    assert status(EvidenceRule(RuleKind.TESTS, tests=(passing,))) == STATUS_PASS
    assert status(EvidenceRule(RuleKind.TESTS, tests=(passing, failing))) == STATUS_FAIL
    assert status(EvidenceRule(RuleKind.TESTS, tests=(passing, ignored))) == STATUS_NOT_RUN
    assert status(EvidenceRule(RuleKind.TESTS, tests=(passing, absent))) == STATUS_NOT_RUN
    with_check = EvidenceRule(
        RuleKind.TESTS, tests=(passing,), evidence_checks=("bounded_repair_closed",)
    )
    assert status(with_check) == STATUS_FAIL
    assert status(EvidenceRule(RuleKind.NOT_IMPLEMENTED, reason="no adapter")) == STATUS_NOT_RUN
    assert status(EvidenceRule(RuleKind.LIVE, reason="live"), "live_coding") == STATUS_BLOCKED
    assert status(EvidenceRule(RuleKind.CALIBRATION, calibration_checks=("api",))) == STATUS_NOT_RUN
    assert status(EvidenceRule(RuleKind.REPOSITORY), "repository_test") == STATUS_FAIL
    windows_only = EvidenceRule(RuleKind.TESTS, tests=(passing,), requires_windows=True)
    assert assess_class(
        "native_windows_test", windows_only, artifacts(is_windows=False)
    ).status == (STATUS_BLOCKED)
    live = assess_class("live_coding", EvidenceRule(RuleKind.LIVE), artifacts(MODE_LIVE))
    assert live.status == STATUS_BLOCKED


def scenario(statuses: dict[str, str]) -> dict[str, Any]:
    return {
        "required_for_full_pass": True,
        "classes": [
            {"execution_class": name, "status": status} for name, status in statuses.items()
        ],
    }


def test_the_verdict_follows_the_class_statuses() -> None:
    offline_ok = [scenario({"deterministic_test": STATUS_PASS, "live_coding": STATUS_BLOCKED})]
    assert decide_verdict(MODE_OFFLINE, offline_ok, LIVE_CLASSES) == VERDICT_OFFLINE_PASS
    not_run = offline_ok + [scenario({"gateway_fixture": STATUS_NOT_RUN})]
    assert decide_verdict(MODE_OFFLINE, not_run, LIVE_CLASSES) == VERDICT_BLOCKED
    failed = offline_ok + [scenario({"deterministic_fixture": STATUS_FAIL})]
    assert decide_verdict(MODE_OFFLINE, failed, LIVE_CLASSES) == VERDICT_FAIL
    all_pass = [scenario({"deterministic_test": STATUS_PASS, "live_coding": STATUS_PASS})]
    assert decide_verdict(MODE_OFFLINE, all_pass, LIVE_CLASSES) == VERDICT_OFFLINE_PASS
    assert decide_verdict(MODE_LIVE, all_pass, LIVE_CLASSES) == VERDICT_PASS
    assert decide_verdict(MODE_LIVE, offline_ok, LIVE_CLASSES) == VERDICT_BLOCKED
    assert worst_status([STATUS_PASS, STATUS_BLOCKED, STATUS_NOT_RUN]) == STATUS_NOT_RUN


def complete_policy(template: dict[str, Any]) -> dict[str, Any]:
    policy = dict(template)
    policy.update(
        enabled=True,
        configuration_status="CONFIGURED",
        gateway_endpoint="http://127.0.0.1:8317",
        credential_reference="AAG_LOCAL_TOKEN",
        role_routes={role: f"route_{role}" for role in template["role_routes"]},
        known_cost_cap=5,
        currency="USD",
    )
    return policy


def test_the_live_gate_needs_opt_in_and_a_complete_private_policy(tmp_path: Path) -> None:
    template = dict(load_benchmark(REPO_ROOT / BENCHMARK_PATH).live_policy_template)
    assert "enabled is not true" in policy_problems(template, template)
    policy = complete_policy(template)
    assert policy_problems(policy, template) == []
    remote = dict(policy, gateway_endpoint="https://gateway.example.com")
    assert "gateway_endpoint is not a loopback http URL" in policy_problems(remote, template)
    secret = dict(policy, credential_reference="sk-live-0123456789abcdef")
    problems = policy_problems(secret, template)
    assert "credential_reference is not a credential name" in problems
    assert all("sk-live" not in problem for problem in problems)
    switching = dict(policy, global_provider_switching_allowed=True)
    assert "global_provider_switching_allowed is not false" in policy_problems(switching, template)

    path = tmp_path / "policy.json"
    path.write_text(json.dumps(policy), encoding="utf-8")
    assert evaluate_live_gate(path, True, template).open
    closed = evaluate_live_gate(path, False, template)
    assert not closed.open
    assert closed.problems[0].startswith("live mode needs the explicit --live_opt_in flag")
    assert not evaluate_live_gate(None, True, template).open


def test_the_output_directory_must_be_new_and_outside_the_repository(tmp_path: Path) -> None:
    with pytest.raises(OutputError):
        prepare_output(REPO_ROOT / "target_acceptance_output", REPO_ROOT)
    occupied = tmp_path / "occupied"
    occupied.mkdir()
    (occupied / "stale.json").write_text("{}", encoding="utf-8")
    with pytest.raises(OutputError):
        prepare_output(occupied, REPO_ROOT)
    tmp_parent = tmp_path / "system_tmp"
    tmp_parent.mkdir()
    layout = prepare_output(tmp_path / "fresh", REPO_ROOT, tmp_parent)
    assert layout.logs.is_dir() and layout.evidence.is_dir() and layout.private_tmp.is_dir()
    # The suites' temporary root is a fresh run-owned directory outside the
    # (possibly deep) output directory.
    assert layout.private_tmp.parent == tmp_parent
    assert layout.private_tmp.name.startswith(PRIVATE_TMP_PREFIX)
    assert not layout.private_tmp.is_relative_to(layout.root)


def layout_with_private_tmp(private_tmp: Path) -> OutputLayout:
    root = Path("output")
    return OutputLayout(root, root / "logs", root / "evidence", private_tmp, root / "report.json")


def test_a_private_temporary_root_too_long_for_nested_worktrees_blocks_the_run() -> None:
    fitting = Path("t") / ("x" * (MAX_PRIVATE_TMP_PATH_CHARS - 2))
    assert len(str(fitting)) == MAX_PRIVATE_TMP_PATH_CHARS
    assert private_tmp_problems(layout_with_private_tmp(fitting)) == []
    problems = private_tmp_problems(layout_with_private_tmp(fitting / "y"))
    assert len(problems) == 1 and "Windows path limits" in problems[0]


def test_the_private_temporary_root_is_removed_with_read_only_git_objects(tmp_path: Path) -> None:
    private_tmp = tmp_path / f"{PRIVATE_TMP_PREFIX}run"
    objects = private_tmp / "pytest_basetemp" / "project" / ".git" / "objects" / "ab"
    objects.mkdir(parents=True)
    packed = objects / "cdef"
    packed.write_bytes(b"object")
    packed.chmod(stat.S_IREAD)
    assert remove_private_tmp(private_tmp)
    assert not private_tmp.exists()
    # A directory this run did not create is never removed.
    foreign = tmp_path / "foreign"
    foreign.mkdir()
    assert not remove_private_tmp(foreign)
    assert foreign.is_dir()


def command_record(command_id: str, exit_code: int | None) -> CommandRecord:
    return CommandRecord(command_id, (command_id,), ".", exit_code, 0, "log", "0" * 64, False)


def test_the_private_temporary_root_is_kept_only_after_a_failure_or_a_live_process() -> None:
    succeeded = [command_record("cargo_test", 0), command_record("pytest", 0)]
    assert private_tmp_retention_reason(succeeded, []) is None
    assert private_tmp_retention_reason([], []) is None
    failed = [command_record("cargo_test", 101), command_record("pytest", None)]
    reason = private_tmp_retention_reason(failed, [])
    assert reason is not None and "'cargo_test', 'pytest'" in reason
    outlived = private_tmp_retention_reason(succeeded, ["vibemuxd.exe (pid 7)"])
    assert outlived is not None and "outlived" in outlived


def evidence_store(name: str, document: dict[str, Any]) -> EvidenceStore:
    path = Path(f"{name}.json")
    return EvidenceStore({name: EvidenceFile(name, path, "0" * 64, document)}, {}, ())


def test_null_valued_evidence_fields_fail_checks_and_never_crash_report_views() -> None:
    # A workflow that never finished a run exports `last_run: null`, and an
    # export may carry null bodies; views must not raise on either.
    document: dict[str, Any] = {
        "schema_version": 1,
        "name": FLAGSHIP,
        "workflow_id": "workflow",
        "status": {"evidence_class": "fixture", "last_run": None, "receipts": None},
        "export": [{"kind": "receipt", "body": None}, {"kind": "contract", "body": None}],
        "sessions": [{"session": None, "attempts": None}],
    }
    store = evidence_store(FLAGSHIP, document)
    summary = workflow_summaries(store)[0]
    assert summary["retained_workspaces"] == [] and summary["contract_ids"] == [None]
    assert session_records(store)[0]["attempts"] == 0
    assert candidate_records(store) == [] and verifier_records(store) == []
    outcome = run_evidence_check(store, "flagship_cleanup_retains_unsafe")
    assert outcome.status == CHECK_FAIL


def test_validating_a_report_with_wrong_json_types_reports_it_invalid(tmp_path: Path) -> None:
    report = tmp_path / "dual_track_report.json"
    report.write_text(json.dumps({"scenarios": "not a list", "commands": 7}), encoding="utf-8")
    assert validate(report) == EXIT_FAIL


def test_a_report_validates_its_evidence_through_a_relative_directory(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Evidence paths and the directory they are made relative to must come
    # from one resolved form of the report directory, however it was named.
    evidence_dir = tmp_path / "run" / EVIDENCE_DIR_NAME
    evidence_dir.mkdir(parents=True)
    path = evidence_dir / f"{OPTIMIZER_CYCLE}.json"
    path.write_text(
        json.dumps({"schema_version": 1, "name": OPTIMIZER_CYCLE, "value": {}}), encoding="utf-8"
    )
    recorded = {
        "name": OPTIMIZER_CYCLE,
        "path": f"{EVIDENCE_DIR_NAME}/{OPTIMIZER_CYCLE}.json",
        "sha256": file_sha256(path),
    }
    monkeypatch.chdir(tmp_path)
    benchmark = load_benchmark(REPO_ROOT / BENCHMARK_PATH)
    problems = validate_report(
        {"mode": MODE_OFFLINE, "evidence_artifacts": [recorded]}, Path("run"), benchmark, RULES
    )
    assert "evidence_artifacts do not match the evidence files on disk" not in problems
