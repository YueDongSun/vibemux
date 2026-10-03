"""Runs the trusted local checks and parses what they actually executed.

Every command's combined output goes to a log file under the owned output
directory; the report references each log by path and digest. Parsers only
count results that libtest, pytest, or the calibration script printed, and
they mark any count that does not reconcile.
"""

from __future__ import annotations

import hashlib
import json
import re
import subprocess
import time
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

# ------------------------------------------------------------- commands --


@dataclass(frozen=True)
class CommandRecord:
    command_id: str
    argv: tuple[str, ...]
    cwd: str
    exit_code: int | None
    duration_ms: int
    log_path: str
    log_sha256: str
    timed_out: bool

    def to_json(self) -> dict[str, Any]:
        return {
            "command_id": self.command_id,
            "argv": list(self.argv),
            "cwd": self.cwd,
            "exit_code": self.exit_code,
            "duration_ms": self.duration_ms,
            "log_path": self.log_path,
            "log_sha256": self.log_sha256,
            "timed_out": self.timed_out,
        }


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def run_logged(
    command_id: str,
    argv: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
    log_path: Path,
    output_root: Path,
    timeout_seconds: float,
    stdout_path: Path | None = None,
) -> CommandRecord:
    """Runs one command to completion, stdin closed, output to `log_path`.

    With `stdout_path`, stdout goes there and stderr to the log; otherwise
    both go to the log in the order the process wrote them. A timeout kills
    the process (and only it) and is recorded, never retried. The record
    names the log relative to `output_root`.
    """
    log_path.parent.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    timed_out = False
    exit_code: int | None
    with log_path.open("wb") as log:
        stdout_handle = stdout_path.open("wb") if stdout_path is not None else None
        try:
            process = subprocess.Popen(
                list(argv),
                cwd=cwd,
                env=dict(env),
                stdin=subprocess.DEVNULL,
                stdout=stdout_handle if stdout_handle is not None else log,
                stderr=log if stdout_handle is not None else subprocess.STDOUT,
            )
            try:
                exit_code = process.wait(timeout=timeout_seconds)
            except subprocess.TimeoutExpired:
                timed_out = True
                process.kill()
                process.wait()
                exit_code = None
        finally:
            if stdout_handle is not None:
                stdout_handle.close()
    return CommandRecord(
        command_id=command_id,
        argv=tuple(argv),
        cwd=str(cwd),
        exit_code=exit_code,
        duration_ms=int((time.monotonic() - started) * 1000),
        log_path=log_path.relative_to(output_root).as_posix(),
        log_sha256=file_sha256(log_path),
        timed_out=timed_out,
    )


def probe_version(
    argv: Sequence[str], env: Mapping[str, str], timeout_seconds: float
) -> str | None:
    """The first output line of a `--version` probe, or None if it fails."""
    try:
        completed = subprocess.run(
            list(argv),
            env=dict(env),
            stdin=subprocess.DEVNULL,
            capture_output=True,
            timeout=timeout_seconds,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if completed.returncode != 0:
        return None
    text = (completed.stdout or completed.stderr).decode("utf-8", errors="replace").strip()
    return text.splitlines()[0] if text else None


# ----------------------------------------------------------- cargo test --

_RUNNING = re.compile(r"^\s*Running (?P<source>.+?) \((?P<executable>.+)\)\s*$")
_DOC_TESTS = re.compile(r"^\s*Doc-tests (?P<package>\S+)\s*$")
_RUNNING_COUNT = re.compile(r"^running (?P<count>\d+) tests?\s*$")
_TEST_LINE = re.compile(r"^test (?P<name>.+?) \.\.\. (?P<outcome>ok|FAILED|ignored(?:, .*)?)\s*$")
_RESULT = re.compile(
    r"^test result: (?P<state>ok|FAILED)\. (?P<passed>\d+) passed; (?P<failed>\d+) failed; "
    r"(?P<ignored>\d+) ignored; (?P<measured>\d+) measured; (?P<filtered>\d+) filtered out"
)

OUTCOME_OK = "ok"
OUTCOME_FAILED = "failed"
OUTCOME_IGNORED = "ignored"


@dataclass(frozen=True)
class ArtifactTarget:
    package: str
    target_kind: str
    target: str


@dataclass
class TargetRun:
    package: str
    target_kind: str
    target: str
    executable: str | None
    planned: int | None = None
    outcomes: dict[str, str] = field(default_factory=dict)
    duplicate_names: list[str] = field(default_factory=list)
    summary: dict[str, int] | None = None

    def key(self) -> str:
        return f"{self.package}/{self.target_kind}:{self.target}"

    def counts(self) -> dict[str, int]:
        values = list(self.outcomes.values())
        return {
            "passed": values.count(OUTCOME_OK),
            "failed": values.count(OUTCOME_FAILED),
            "ignored": values.count(OUTCOME_IGNORED),
        }

    def problems(self) -> list[str]:
        """Why this target's counted results cannot be trusted as complete."""
        problems: list[str] = []
        if self.summary is None:
            problems.append("no test result line (the binary did not finish)")
            return problems
        counts = self.counts()
        for name in ("passed", "failed", "ignored"):
            if counts[name] != self.summary[name]:
                problems.append(
                    f"{name}: {counts[name]} test lines but the summary says {self.summary[name]}"
                )
        if self.planned is not None and self.planned != len(self.outcomes):
            problems.append(f"{self.planned} planned but {len(self.outcomes)} reported")
        if self.summary["filtered"] != 0:
            problems.append(f"{self.summary['filtered']} tests filtered out")
        if self.duplicate_names:
            problems.append(f"duplicate results for {sorted(set(self.duplicate_names))}")
        return problems

    def to_json(self) -> dict[str, Any]:
        return {
            "target": self.key(),
            "executable": self.executable,
            "planned": self.planned,
            "counted": self.counts(),
            "summary": self.summary,
            "problems": self.problems(),
        }


@dataclass
class CargoTestRun:
    targets: list[TargetRun]
    unattributed_test_lines: int
    unknown_executables: list[str]
    compile_errors: int

    def find(self, package: str, target_kind: str, target: str) -> list[TargetRun]:
        return [
            run
            for run in self.targets
            if run.package == package and run.target_kind == target_kind and run.target == target
        ]

    def outcome(self, package: str, target_kind: str, target: str, name: str) -> str | None:
        outcomes = [
            run.outcomes[name]
            for run in self.find(package, target_kind, target)
            if name in run.outcomes
        ]
        if len(outcomes) != 1:
            return None
        return outcomes[0]

    def totals(self) -> dict[str, int]:
        totals = {"targets": len(self.targets), "passed": 0, "failed": 0, "ignored": 0}
        for run in self.targets:
            for name, value in run.counts().items():
                totals[name] += value
        return totals

    def problems(self) -> list[str]:
        problems = [f"{run.key()}: {problem}" for run in self.targets for problem in run.problems()]
        if self.unattributed_test_lines:
            problems.append(f"{self.unattributed_test_lines} test lines outside any target")
        if self.unknown_executables:
            problems.append(
                f"test binaries without a compiler artifact: {self.unknown_executables}"
            )
        if self.compile_errors:
            problems.append(f"{self.compile_errors} compiler errors")
        return problems


def _normalize_executable(path: str) -> str:
    return path.replace("\\", "/").casefold()


def package_name(package_id: str) -> str:
    """The package name of a cargo package ID (`path+file:///…/name#1.0`
    or `path+file:///…/dir#name@1.0`)."""
    fragment = package_id.rsplit("#", 1)[-1] if "#" in package_id else ""
    if "@" in fragment:
        return fragment.split("@", 1)[0]
    if " " in package_id:
        # The older `name version (source)` form.
        return package_id.split(" ", 1)[0]
    location = package_id.split("#", 1)[0].rstrip("/")
    return location.rsplit("/", 1)[-1]


def _target_kind(kinds: Iterable[str]) -> str:
    kinds = list(kinds)
    if "test" in kinds:
        return "test"
    if "bin" in kinds:
        return "bin"
    if "bench" in kinds:
        return "bench"
    if "example" in kinds:
        return "example"
    return "lib"


def _artifact(message: Mapping[str, Any]) -> tuple[str, ArtifactTarget] | None:
    if message.get("reason") != "compiler-artifact":
        return None
    executable = message.get("executable")
    target = message.get("target")
    profile = message.get("profile")
    if not isinstance(executable, str) or not isinstance(target, dict):
        return None
    if not isinstance(profile, dict) or not profile.get("test"):
        return None
    return _normalize_executable(executable), ArtifactTarget(
        package=package_name(str(message.get("package_id", ""))),
        target_kind=_target_kind(target.get("kind", [])),
        target=str(target.get("name", "")),
    )


def parse_cargo_test_output(lines: Iterable[str]) -> CargoTestRun:
    """Parses `cargo test --message-format=json` output with stderr merged.

    Compiler-artifact messages map every test executable to its package and
    target, so two integration tests with one file name in different crates
    stay distinct. Libtest lines are attributed to the most recent
    `Running`/`Doc-tests` header.
    """
    artifacts: dict[str, ArtifactTarget] = {}
    targets: list[TargetRun] = []
    current: TargetRun | None = None
    unattributed = 0
    unknown: list[str] = []
    compile_errors = 0
    for raw in lines:
        line = raw.rstrip("\r\n")
        if line.startswith("{"):
            try:
                message = json.loads(line)
            except json.JSONDecodeError:
                message = None
            if isinstance(message, dict):
                artifact = _artifact(message)
                if artifact is not None:
                    artifacts[artifact[0]] = artifact[1]
                if message.get("reason") == "compiler-message":
                    inner = message.get("message")
                    if isinstance(inner, dict) and inner.get("level") == "error":
                        compile_errors += 1
                continue
        running = _RUNNING.match(line)
        if running is not None:
            executable = running.group("executable")
            known = artifacts.get(_normalize_executable(executable))
            if known is None:
                unknown.append(executable)
                current = TargetRun("unknown", "unknown", executable, executable)
            else:
                current = TargetRun(known.package, known.target_kind, known.target, executable)
            targets.append(current)
            continue
        doc_tests = _DOC_TESTS.match(line)
        if doc_tests is not None:
            package = doc_tests.group("package")
            current = TargetRun(package, "doctest", package, None)
            targets.append(current)
            continue
        if current is None:
            if _TEST_LINE.match(line):
                unattributed += 1
            continue
        planned = _RUNNING_COUNT.match(line)
        if planned is not None:
            current.planned = int(planned.group("count"))
            continue
        test_line = _TEST_LINE.match(line)
        if test_line is not None:
            name = test_line.group("name")
            outcome_text = test_line.group("outcome")
            if outcome_text == "ok":
                outcome = OUTCOME_OK
            elif outcome_text == "FAILED":
                outcome = OUTCOME_FAILED
            else:
                outcome = OUTCOME_IGNORED
            if name in current.outcomes:
                current.duplicate_names.append(name)
            current.outcomes[name] = outcome
            continue
        result = _RESULT.match(line)
        if result is not None and current.summary is None:
            current.summary = {
                name: int(result.group(name))
                for name in ("passed", "failed", "ignored", "measured", "filtered")
            }
    return CargoTestRun(targets, unattributed, unknown, compile_errors)


def read_log_lines(path: Path) -> list[str]:
    return path.read_text(encoding="utf-8", errors="replace").splitlines()


# ---------------------------------------------------------- calibration --


@dataclass(frozen=True)
class CalibrationResult:
    summary: dict[str, Any] | None
    error: str | None


def parse_calibration_output(text: str) -> CalibrationResult:
    try:
        document = json.loads(text)
    except json.JSONDecodeError as error:
        return CalibrationResult(None, f"calibration output is not JSON: {error}")
    if not isinstance(document, dict) or document.get("schema_version") != 1:
        return CalibrationResult(None, "calibration output has an unknown schema")
    for key in ("good", "mutants", "sources_unchanged", "browsers_used", "all_expectations_met"):
        if key not in document:
            return CalibrationResult(None, f"calibration output lacks {key}")
    inventory_error = _mutant_inventory_error(document["mutants"])
    if inventory_error is not None:
        return CalibrationResult(None, inventory_error)
    return CalibrationResult(document, None)


CHECK_PASS = "PASS"
CHECK_FAIL = "FAIL"
CHECK_BLOCKED = "BLOCKED"

EXPECTED_MUTANT_SUITES: Mapping[str, str] = {
    "blank_title_accepted": "store",
    "corrupt_file_reset": "api",
    "filter_broken": "browser",
    "lost_concurrent_updates": "store",
    "payload_limit_missing": "api",
    "success_on_rejection": "browser",
    "title_length_off_by_one": "api",
    "wrong_delete_status": "api",
    "xss_inner_html": "browser",
}


@dataclass(frozen=True)
class CheckOutcome:
    status: str
    reason: str


def _mutant_inventory_error(mutants: Any) -> str | None:
    if not isinstance(mutants, list):
        return "calibration mutants must be a list"
    observed: set[str] = set()
    for entry in mutants:
        if not isinstance(entry, Mapping):
            return "calibration mutant entry is not an object"
        mutant_id = entry.get("mutant_id")
        suite = entry.get("expected_failing_suite")
        if not isinstance(mutant_id, str) or not mutant_id:
            return "calibration mutant lacks a valid mutant_id"
        if mutant_id in observed:
            return f"duplicate calibration mutant {mutant_id}"
        expected_suite = EXPECTED_MUTANT_SUITES.get(mutant_id)
        if expected_suite is None:
            return f"unexpected calibration mutant {mutant_id}"
        if suite != expected_suite:
            return f"calibration mutant {mutant_id} targets {suite!r}; expected {expected_suite!r}"
        observed.add(mutant_id)
    missing = sorted(set(EXPECTED_MUTANT_SUITES) - observed)
    if missing:
        return f"calibration omits expected mutants {missing}"
    return None


def _mutants_met(summary: Mapping[str, Any], suite: str, frontend_only: bool) -> tuple[int, int]:
    entries = [
        entry for entry in summary["mutants"] if entry.get("expected_failing_suite") == suite
    ]
    met = 0
    for entry in entries:
        ok = entry.get("expectation_met") is True
        if frontend_only:
            ok = ok and entry.get("frontend_only_expectation_met") is True
        met += int(ok)
    return met, len(entries)


def evaluate_calibration_check(summary: Mapping[str, Any], check: str) -> CheckOutcome:
    """One suite's calibration verdict: the good solution passes, every
    mutant aimed at the suite fails it, and the fixture sources are
    unchanged."""
    good = summary.get("good", {})
    if summary.get("sources_unchanged") is not True:
        return CheckOutcome(CHECK_FAIL, "calibration changed the fixture sources")
    inventory_error = _mutant_inventory_error(summary.get("mutants"))
    if inventory_error is not None:
        return CheckOutcome(CHECK_FAIL, inventory_error)
    suites: tuple[str, ...]
    if check == "api":
        suites = ("api",)
    elif check == "store":
        suites = ("store",)
    elif check == "browser":
        suites = ("browser", "browser_frontend_only")
    else:
        return CheckOutcome(CHECK_FAIL, f"unknown calibration check {check}")
    for suite in suites:
        status = good.get(suite)
        if status == "blocked":
            return CheckOutcome(CHECK_BLOCKED, f"the good solution's {suite} suite was blocked")
        if status != "passed":
            return CheckOutcome(CHECK_FAIL, f"the good solution's {suite} suite is {status}")
    met, total = _mutants_met(summary, suites[0], frontend_only=check == "browser")
    if total == 0:
        return CheckOutcome(CHECK_FAIL, f"no {suites[0]} mutants were run")
    if met != total:
        return CheckOutcome(
            CHECK_FAIL, f"{total - met} of {total} {suites[0]} mutants were not caught"
        )
    if check == "api":
        if summary.get("base_api_status") != "failed":
            return CheckOutcome(CHECK_FAIL, "the unimplemented base did not fail api")
        if summary.get("contract_stub_api_status") != "passed":
            return CheckOutcome(CHECK_FAIL, "the contract stub server did not pass api")
    if check == "browser" and not summary.get("browsers_used"):
        return CheckOutcome(CHECK_FAIL, "no browser version was recorded")
    extra = f"; browsers {summary['browsers_used']}" if check == "browser" else ""
    return CheckOutcome(
        CHECK_PASS, f"good solution passed {', '.join(suites)}; {met}/{total} mutants caught{extra}"
    )


# ---------------------------------------------------------------- pytest --

_PYTEST_SUMMARY = re.compile(r"^=*\s*(?P<body>.*\bin [\d.]+s.*?)\s*=*$")
_PYTEST_COUNT = re.compile(
    r"(?P<count>\d+) (?P<kind>passed|failed|errors?|skipped|xfailed|xpassed)"
)


def parse_pytest_summary(lines: Iterable[str]) -> dict[str, int] | None:
    """The counts on pytest's final summary line, or None if absent."""
    summary: dict[str, int] | None = None
    for line in lines:
        match = _PYTEST_SUMMARY.match(line.strip())
        if match is None:
            continue
        counts: dict[str, int] = {}
        for count in _PYTEST_COUNT.finditer(match.group("body")):
            kind = count.group("kind")
            kind = "errors" if kind.startswith("error") else kind
            counts[kind] = counts.get(kind, 0) + int(count.group("count"))
        if counts:
            summary = counts
    return summary
