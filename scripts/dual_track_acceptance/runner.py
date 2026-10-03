"""Orchestrates one acceptance run and the `--validate` check."""

from __future__ import annotations

import dataclasses
import json
import os
import platform
import re
import subprocess
import sys
from collections.abc import Sequence
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from .assessment import (
    LIVE_NOT_IMPLEMENTED,
    MODE_LIVE,
    MODE_OFFLINE,
    STATUS_FAIL,
    STATUS_PASS,
    VERDICT_BLOCKED,
    VERDICT_FAIL,
    VERDICT_OFFLINE_PASS,
    VERDICT_PASS,
    ClassResult,
    RunArtifacts,
    ScenarioResult,
    assess_scenarios,
    decide_verdict,
)
from .check_execution import (
    CommandRecord,
    file_sha256,
    parse_calibration_output,
    parse_cargo_test_output,
    parse_pytest_summary,
    probe_version,
    read_log_lines,
    run_logged,
)
from .evidence_capture import load_evidence
from .live_policy import LiveGate, evaluate_live_gate
from .report_builder import CleanupInfo, EnvironmentInfo, SourceInfo, build_report
from .report_validation import (
    CALIBRATION_COMMAND,
    CARGO_COMMAND,
    E01_ID,
    PYTEST_COMMAND,
    validate_report,
)
from .runner_config import (
    BENCHMARK_PATH,
    CALIBRATION_SCRIPT,
    CALIBRATION_TIMEOUT_SECONDS,
    CARGO_TEST_ARGS,
    CARGO_TEST_TIMEOUT_SECONDS,
    EVIDENCE_DIR_ENV,
    EXIT_BLOCKED,
    EXIT_FAIL,
    EXIT_PASS,
    EXIT_USAGE,
    MIN_NODE_MAJOR,
    OWNED_PROCESS_NAMES,
    PYTEST_TIMEOUT_SECONDS,
    PYTHON_SOURCE_DIR,
    REPORT_FILE_NAME,
    VERSION_PROBE_TIMEOUT_SECONDS,
)
from .runtime_roots import (
    OutputError,
    OutputLayout,
    owned_process_snapshot,
    prepare_output,
    private_environment,
    private_tmp_problems,
    remove_private_tmp,
    retained_entries,
)
from .scenario_catalog import (
    LIVE_CLASSES,
    RULES,
    Benchmark,
    CatalogError,
    check_catalog,
    load_benchmark,
)

PYTEST_BASETEMP_NAME = "pytest_basetemp"


@dataclass(frozen=True)
class RunRequest:
    mode: str
    output: Path
    config: Path | None
    live_opt_in: bool
    argv: Sequence[str]


def repository_root() -> Path:
    return Path(__file__).resolve().parents[2]


def _now() -> str:
    return datetime.now(UTC).isoformat(timespec="seconds")


def _git(repo_root: Path, *args: str) -> str | None:
    try:
        completed = subprocess.run(
            ["git", *args],
            cwd=repo_root,
            capture_output=True,
            stdin=subprocess.DEVNULL,
            timeout=VERSION_PROBE_TIMEOUT_SECONDS,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if completed.returncode != 0:
        return None
    return completed.stdout.decode("utf-8", errors="replace").strip()


def _source(repo_root: Path) -> SourceInfo:
    head = _git(repo_root, "rev-parse", "HEAD")
    status = _git(repo_root, "status", "--porcelain")
    return SourceInfo(head, None if status is None else bool(status))


def _tool_versions(environment: dict[str, str]) -> dict[str, str | None]:
    probes = {
        "rustc": ["rustc", "--version"],
        "cargo": ["cargo", "--version"],
        "node": ["node", "--version"],
        "git": ["git", "--version"],
        "python": [sys.executable, "--version"],
        "pytest": [sys.executable, "-m", "pytest", "--version"],
    }
    return {
        name: probe_version(argv, environment, VERSION_PROBE_TIMEOUT_SECONDS)
        for name, argv in probes.items()
    }


def prerequisite_problems(versions: dict[str, str | None]) -> list[str]:
    problems = [f"{name} is unavailable" for name, version in versions.items() if version is None]
    node = versions.get("node")
    if node is not None:
        match = re.match(r"v?(\d+)", node)
        if match is None or int(match.group(1)) < MIN_NODE_MAJOR:
            problems.append(f"node {node} is older than {MIN_NODE_MAJOR}")
    return problems


def _blocked_scenarios(benchmark: Benchmark, reason: str) -> list[ScenarioResult]:
    return [
        ScenarioResult(
            scenario.scenario_id,
            scenario.description,
            scenario.required_for_full_pass,
            [ClassResult(name, "BLOCKED", reason) for name in scenario.execution_classes],
        )
        for scenario in benchmark.scenarios
    ]


def _write_json(path: Path, document: Any) -> None:
    path.write_text(json.dumps(document, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def _exit_code(verdict: str) -> int:
    if verdict in (VERDICT_PASS, VERDICT_OFFLINE_PASS):
        return EXIT_PASS
    if verdict == VERDICT_FAIL:
        return EXIT_FAIL
    return EXIT_BLOCKED


def _print_summary(report: dict[str, Any], layout: OutputLayout) -> None:
    for scenario in report["scenarios"]:
        classes = ", ".join(
            f"{entry['execution_class']}={entry['status']}" for entry in scenario["classes"]
        )
        print(f"{scenario['id']:<4} {scenario['status']:<8} {classes}")
    print(f"verdict: {report['verdict']} (evidence class {report['evidence_class']})")
    print(f"report: {layout.report}")


def _run_suites(
    repo_root: Path, layout: OutputLayout, environment: dict[str, str]
) -> tuple[list[CommandRecord], Path]:
    cargo = run_logged(
        CARGO_COMMAND,
        ["cargo", *CARGO_TEST_ARGS],
        cwd=repo_root,
        env=environment,
        log_path=layout.logs / "cargo_test.log",
        output_root=layout.root,
        timeout_seconds=CARGO_TEST_TIMEOUT_SECONDS,
    )
    # pytest's base temporary directory is removed with the private root.
    basetemp = layout.private_tmp / PYTEST_BASETEMP_NAME
    pytest = run_logged(
        PYTEST_COMMAND,
        [sys.executable, "-m", "pytest", "-p", "no:cacheprovider", "--basetemp", str(basetemp)],
        cwd=repo_root,
        env=environment,
        log_path=layout.logs / "pytest.log",
        output_root=layout.root,
        timeout_seconds=PYTEST_TIMEOUT_SECONDS,
    )
    calibration_stdout = layout.logs / "calibration_summary.json"
    calibration = run_logged(
        CALIBRATION_COMMAND,
        ["node", str(repo_root / CALIBRATION_SCRIPT)],
        cwd=repo_root,
        env=environment,
        log_path=layout.logs / "calibration.log",
        output_root=layout.root,
        timeout_seconds=CALIBRATION_TIMEOUT_SECONDS,
        stdout_path=calibration_stdout,
    )
    return [cargo, pytest, calibration], calibration_stdout


def private_tmp_retention_reason(
    commands: Sequence[CommandRecord], unjoined: Sequence[str]
) -> str | None:
    """Why the run's private temporary root must outlive the run, if it must.

    A clean run removes the whole root. It is kept while an owned process may
    still use it, and after a failed suite so its test projects and worktrees
    can be inspected.
    """
    if unjoined:
        return f"owned processes outlived the suites: {list(unjoined)}"
    failed = [record.command_id for record in commands if record.exit_code != 0]
    if failed:
        return f"kept for diagnosis because {failed} did not succeed"
    return None


def _decide_e01(report: dict[str, Any], layout: OutputLayout, benchmark: Benchmark) -> list[str]:
    """Validates the draft report and records E01 and the verdict."""
    problems = validate_report(report, layout.root, benchmark, RULES, final=False)
    for scenario in report["scenarios"]:
        if scenario["id"] != E01_ID:
            continue
        entry = scenario["classes"][0]
        if problems:
            entry.update(
                status=STATUS_FAIL,
                reason=f"{len(problems)} validation problems: {problems[:10]}",
                evidence=[],
            )
        else:
            entry.update(
                status=STATUS_PASS,
                reason="every class status, evidence reference, log digest, and evidence digest "
                "was re-derived from the run's own artifacts",
                evidence=["validation:report"],
            )
        scenario["status"] = entry["status"]
        scenario["evidence"] = sorted(entry["evidence"])
        scenario["reason"] = f"{entry['execution_class']}: {entry['status']}: {entry['reason']}"
    report["verdict"] = decide_verdict(report["mode"], report["scenarios"], LIVE_CLASSES)
    return problems


def run(request: RunRequest) -> int:
    started_at = _now()
    repo_root = repository_root()
    try:
        benchmark = load_benchmark(repo_root / BENCHMARK_PATH)
        check_catalog(benchmark, RULES)
    except (OSError, CatalogError) as error:
        print(f"verify_dual_track: {error}", file=sys.stderr)
        return EXIT_USAGE
    live_gate: LiveGate | None = None
    if request.mode == MODE_LIVE:
        live_gate = evaluate_live_gate(
            request.config, request.live_opt_in, benchmark.live_policy_template
        )
    try:
        layout = prepare_output(request.output, repo_root)
    except (OutputError, OSError) as error:
        print(f"verify_dual_track: {error}", file=sys.stderr)
        return EXIT_USAGE

    environment = private_environment(layout, dict(os.environ))
    environment[EVIDENCE_DIR_ENV] = str(layout.evidence)
    environment["PYTHONPATH"] = str(repo_root / PYTHON_SOURCE_DIR)
    versions = _tool_versions(environment)
    source = _source(repo_root)
    env_info = EnvironmentInfo(
        os=f"{platform.system()} {platform.release()} ({platform.version()})",
        tool_versions=versions,
        runtime_roots_isolated=True,
    )
    extra_limitations: list[str] = []
    if source.tree_dirty:
        extra_limitations.append("the source tree had uncommitted changes when the run started")

    blocked_reason: str | None = None
    problems = prerequisite_problems(versions) + private_tmp_problems(layout)
    if problems:
        blocked_reason = f"prerequisites missing: {problems}"
    elif live_gate is not None and not live_gate.open:
        blocked_reason = f"the live gate is closed: {list(live_gate.problems)}"

    commands: list[CommandRecord] = []
    calibration_stdout: Path | None = None
    joined: bool | None = None
    unjoined: list[str] = []
    if blocked_reason is None:
        before = owned_process_snapshot(OWNED_PROCESS_NAMES)
        commands, calibration_stdout = _run_suites(repo_root, layout, environment)
        after = owned_process_snapshot(OWNED_PROCESS_NAMES)
        if before is not None and after is not None:
            unjoined = sorted(f"{name} (pid {pid})" for pid, name in after - before)
            joined = not unjoined

    by_id = {record.command_id: record for record in commands}
    cargo = (
        parse_cargo_test_output(read_log_lines(layout.logs / "cargo_test.log"))
        if CARGO_COMMAND in by_id
        else None
    )
    pytest_summary = (
        parse_pytest_summary(read_log_lines(layout.logs / "pytest.log"))
        if PYTEST_COMMAND in by_id
        else None
    )
    calibration = (
        parse_calibration_output(calibration_stdout.read_text(encoding="utf-8"))
        if calibration_stdout is not None
        else None
    )
    evidence = load_evidence(layout.evidence)
    artifacts = RunArtifacts(
        mode=request.mode,
        is_windows=sys.platform == "win32",
        cargo=cargo,
        cargo_exit_code=by_id[CARGO_COMMAND].exit_code if CARGO_COMMAND in by_id else None,
        pytest_summary=pytest_summary,
        pytest_exit_code=by_id[PYTEST_COMMAND].exit_code if PYTEST_COMMAND in by_id else None,
        calibration=calibration,
        calibration_exit_code=(
            by_id[CALIBRATION_COMMAND].exit_code if CALIBRATION_COMMAND in by_id else None
        ),
        evidence=evidence,
        live_gate_reason=None,
    )
    if blocked_reason is not None:
        scenarios = _blocked_scenarios(benchmark, blocked_reason)
        extra_limitations.append(blocked_reason)
    else:
        if live_gate is not None:
            artifacts = dataclasses.replace(artifacts, live_gate_reason=LIVE_NOT_IMPLEMENTED)
        scenarios = assess_scenarios(benchmark, RULES, artifacts)

    suite_leftovers = retained_entries(layout.private_tmp)
    retained_reason = private_tmp_retention_reason(commands, unjoined)
    private_tmp_removed = retained_reason is None and remove_private_tmp(layout.private_tmp)
    if retained_reason is None and not private_tmp_removed:
        retained_reason = "the runner could not remove it"
    calibration_record = (
        {
            "path": calibration_stdout.relative_to(layout.root).as_posix(),
            "sha256": file_sha256(calibration_stdout),
        }
        if calibration_stdout is not None
        else None
    )
    report = build_report(
        mode=request.mode,
        verdict=VERDICT_BLOCKED,
        benchmark=benchmark,
        repository_relative_benchmark=BENCHMARK_PATH.as_posix(),
        runner_argv=request.argv,
        source=source,
        environment=env_info,
        scenarios=scenarios,
        commands=commands,
        cargo=cargo,
        calibration=calibration,
        calibration_stdout=calibration_record,
        evidence=evidence,
        output_root=layout.root,
        rules=RULES,
        live_gate=live_gate,
        cleanup=CleanupInfo(
            joined_owned_processes=joined,
            unjoined_processes=unjoined,
            suite_leftovers=suite_leftovers,
            retained_entries=retained_entries(layout.private_tmp),
            private_tmp_removed=private_tmp_removed,
            private_tmp_retained_reason=retained_reason,
            private_tmp_root=str(layout.private_tmp),
        ),
        extra_limitations=extra_limitations,
        started_at=started_at,
        finished_at=_now(),
    )
    report["blocked_reason"] = blocked_reason
    if blocked_reason is None:
        _decide_e01(report, layout, benchmark)
    final_problems = validate_report(report, layout.root, benchmark, RULES)
    report["validation"] = {"problems": final_problems}
    if blocked_reason is None and final_problems and report["verdict"] != VERDICT_FAIL:
        report["verdict"] = VERDICT_FAIL
    _write_json(layout.report, report)
    _print_summary(report, layout)
    if final_problems:
        print(f"validation problems: {final_problems}", file=sys.stderr)
    return _exit_code(report["verdict"])


def validate(report_path: Path) -> int:
    """Re-derives a written report from its artifacts (`--validate`)."""
    repo_root = repository_root()
    try:
        benchmark = load_benchmark(repo_root / BENCHMARK_PATH)
        check_catalog(benchmark, RULES)
        report = json.loads(report_path.read_text(encoding="utf-8"))
    except (OSError, CatalogError, json.JSONDecodeError) as error:
        print(f"verify_dual_track: {error}", file=sys.stderr)
        return EXIT_USAGE
    if not isinstance(report, dict):
        print("verify_dual_track: the report is not a JSON object", file=sys.stderr)
        return EXIT_USAGE
    try:
        problems = validate_report(report, report_path.parent, benchmark, RULES)
    except (AttributeError, KeyError, TypeError) as error:
        # A report whose fields have the wrong JSON types is invalid, not
        # a reason to stop validating.
        problems = [f"the report does not have the expected shape: {type(error).__name__}"]
    recorded = (report.get("validation") or {}).get("problems")
    if recorded != problems:
        problems.append("the recorded validation result differs from this validation")
    for problem in problems:
        print(f"problem: {problem}")
    print(f"{report_path.name}: {'valid' if not problems else 'INVALID'} ({report.get('verdict')})")
    return EXIT_PASS if not problems else EXIT_FAIL


__all__ = ["MODE_LIVE", "MODE_OFFLINE", "REPORT_FILE_NAME", "RunRequest", "run", "validate"]
