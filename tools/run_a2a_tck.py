"""Run an unmodified, pinned official A2A TCK with an owned local fixture.

Run this script with the Python interpreter from the isolated TCK environment.
Only a synthetic fixture credential is supplied by a2a_tck_auth.py. Raw reports
stay in a unique attempt directory; the receipt contains no credentials or
machine-specific absolute paths. This tool never installs dependencies.
"""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import ipaddress
import json
import os
import re
import subprocess
import sys
import uuid
import xml.etree.ElementTree as element_tree
from datetime import UTC, datetime
from importlib import metadata
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

DEFAULT_TCK_COMMIT = "5996b79f9cefa6fc390980e383e358a66fb9e49e"
MAX_TEST_SECONDS = 420
STARTUP_SECONDS = 15
SHUTDOWN_SECONDS = 8
MAX_LOG_BYTES = 16 * 1024 * 1024
MAX_JUNIT_BYTES = 32 * 1024 * 1024
MAX_READY_BYTES = 4096
TRANSPORTS = ["http_json", "jsonrpc", "grpc"]


class ValidationFailure(Exception):
    """A sanitized failure code suitable for the public receipt."""


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def git_output(root: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(root), *args],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=10,
        check=False,
    )
    if result.returncode:
        raise ValidationFailure("tck_git_verification_failed")
    return result.stdout.strip()


def verify_tck(root: Path, expected: str) -> str:
    actual = git_output(root, "rev-parse", "HEAD")
    if actual != expected:
        raise ValidationFailure("tck_commit_mismatch")
    # A fresh isolated checkout avoids additional tests or import shadowing.
    if git_output(root, "status", "--porcelain", "--untracked-files=normal"):
        raise ValidationFailure("tck_checkout_not_clean")
    if not (root / "tests" / "compatibility" / "conftest.py").is_file():
        raise ValidationFailure("tck_compatibility_tests_missing")
    return actual


def loopback_url(value: Any) -> str:
    if not isinstance(value, str) or len(value) > 2048:
        raise ValidationFailure("fixture_endpoint_invalid")
    try:
        parsed = urlsplit(value)
        allowed = (
            parsed.scheme == "http"
            and parsed.hostname is not None
            and ipaddress.ip_address(parsed.hostname).is_loopback
            and parsed.port is not None
            and parsed.port > 0
            and parsed.path in ("", "/")
            and not parsed.username
            and not parsed.password
            and not parsed.query
            and not parsed.fragment
        )
    except ValueError as error:
        raise ValidationFailure("fixture_endpoint_invalid") from error
    if not allowed:
        raise ValidationFailure("fixture_endpoint_invalid")
    return value.rstrip("/")


def child_environment(plugin_root: Path, tck_root: Path) -> dict[str, str]:
    # Changes apply only to children, never os.environ or system/user defaults.
    omitted = {"no_proxy", "pythonpath", "pytest_addopts", "pytest_plugins"}
    environment = {key: value for key, value in os.environ.items() if key.lower() not in omitted}
    environment.update(
        {
            "PYTHONUTF8": "1",
            "PYTHONNOUSERSITE": "1",
            "PYTHONDONTWRITEBYTECODE": "1",
            "NO_PROXY": "localhost,127.0.0.1,::1",
            "PYTHONPATH": os.pathsep.join((str(plugin_root), str(tck_root))),
        }
    )
    return environment


async def drain_log(reader: asyncio.StreamReader, path: Path) -> bool:
    """Keep draining pipes after the on-disk log limit to avoid pipe deadlock."""
    written = 0
    truncated = False
    with path.open("wb") as target:
        while chunk := await reader.read(16384):
            keep = max(0, min(len(chunk), MAX_LOG_BYTES - written))
            if keep:
                target.write(chunk[:keep])
                written += keep
            truncated |= keep != len(chunk)
    return truncated


async def terminate_owned(process: asyncio.subprocess.Process) -> None:
    if process.returncode is not None:
        return
    try:
        process.terminate()
    except ProcessLookupError:
        await process.wait()
        return
    try:
        await asyncio.wait_for(process.wait(), SHUTDOWN_SECONDS)
    except TimeoutError:
        try:
            process.kill()
        except ProcessLookupError:
            pass
        await asyncio.wait_for(process.wait(), SHUTDOWN_SECONDS)


async def stop_fixture(process: asyncio.subprocess.Process) -> bool:
    forced = False
    if process.returncode is None:
        try:
            assert process.stdin is not None
            process.stdin.write(b"shutdown\n")
            await asyncio.wait_for(process.stdin.drain(), 2)
            process.stdin.close()
            await asyncio.wait_for(process.wait(), SHUTDOWN_SECONDS)
        except (TimeoutError, BrokenPipeError, ConnectionResetError):
            forced = True
            await terminate_owned(process)
    return forced


def junit_counts(path: Path) -> dict[str, Any]:
    if not path.is_file() or path.stat().st_size > MAX_JUNIT_BYTES:
        raise ValidationFailure("junit_report_missing_or_oversized")
    try:
        root = element_tree.parse(path).getroot()
        cases = list(root.iter("testcase"))
        counts: dict[str, Any] = {
            "tests": len(cases),
            "passed": 0,
            "failures": 0,
            "errors": 0,
            "skipped": 0,
            "xfailed": 0,
        }
        passed_by_binding = {binding: 0 for binding in TRANSPORTS}
        for case in cases:
            if case.find("error") is not None:
                counts["errors"] += 1
            elif case.find("failure") is not None:
                counts["failures"] += 1
            elif (skipped := case.find("skipped")) is not None:
                key = "xfailed" if skipped.get("type") == "pytest.xfail" else "skipped"
                counts[key] += 1
            else:
                counts["passed"] += 1
                identity = case.get("name", "") + " " + case.get("classname", "")
                for binding in TRANSPORTS:
                    if (
                        f"[{binding}]" in identity
                        or f"-{binding}]" in identity
                        or f".{binding}." in identity
                    ):
                        passed_by_binding[binding] += 1
        suites = [root] if root.tag == "testsuite" else list(root.findall("testsuite"))
        expected = {
            "tests": counts["tests"],
            "failures": counts["failures"],
            "errors": counts["errors"],
            "skipped": counts["skipped"] + counts["xfailed"],
        }
        if not suites or any(
            sum(int(suite.get(key, "0")) for suite in suites) != value
            for key, value in expected.items()
        ):
            raise ValidationFailure("junit_counter_mismatch")
    except (element_tree.ParseError, ValueError) as error:
        raise ValidationFailure("junit_report_invalid") from error
    counts["passed_by_binding"] = passed_by_binding
    return counts


def runtime_versions() -> dict[str, str | None]:
    result: dict[str, str | None] = {"python": ".".join(map(str, sys.version_info[:3]))}
    for name in (
        "a2a-tck",
        "pytest",
        "pytest-asyncio",
        "pytest-html",
        "pytest-metadata",
        "httpx",
        "grpcio",
        "protobuf",
        "googleapis-common-protos",
        "jsonschema",
        "gherkin-official",
        "Jinja2",
    ):
        try:
            result[name] = metadata.version(name)
        except metadata.PackageNotFoundError:
            result[name] = None
    return result


async def run_attempt(args: argparse.Namespace, attempt: Path, receipt: dict[str, Any]) -> None:
    fixture_process = None
    pytest_process = None
    drains: dict[str, asyncio.Task[bool]] = {}
    plugin_root = Path(__file__).resolve().parent
    try:
        tck_root = args.tck_root.resolve(strict=True)
        fixture = args.fixture.resolve(strict=True)
        if not tck_root.is_dir() or not fixture.is_file():
            raise ValidationFailure("input_path_invalid")
        if attempt.is_relative_to(tck_root):
            raise ValidationFailure("output_must_be_outside_tck_checkout")
        receipt["tck_commit"] = verify_tck(tck_root, args.tck_commit)
        receipt["fixture_sha256"] = file_sha256(fixture)
        receipt["auth_adapter_sha256"] = file_sha256(plugin_root / "a2a_tck_auth.py")
        environment = child_environment(plugin_root, tck_root)
        creationflags = getattr(subprocess, "CREATE_NO_WINDOW", 0)
        fixture_process = await asyncio.create_subprocess_exec(
            str(fixture),
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            env=environment,
            creationflags=creationflags,
            limit=MAX_READY_BYTES,
        )
        assert fixture_process.stdout is not None and fixture_process.stderr is not None
        drains["fixture_stderr"] = asyncio.create_task(
            drain_log(fixture_process.stderr, attempt / "fixture_stderr.log")
        )
        ready_line = await asyncio.wait_for(fixture_process.stdout.readline(), STARTUP_SECONDS)
        if not ready_line or len(ready_line) > MAX_READY_BYTES:
            raise ValidationFailure("fixture_ready_message_invalid")
        try:
            ready = json.loads(ready_line)
        except (json.JSONDecodeError, UnicodeDecodeError) as error:
            raise ValidationFailure("fixture_ready_message_invalid") from error
        if (
            not isinstance(ready, dict)
            or set(ready) != {"fixture", "http_url", "grpc_url"}
            or ready["fixture"] != "vibemux_task_tck_v1"
        ):
            raise ValidationFailure("fixture_ready_message_invalid")
        http_url = loopback_url(ready["http_url"])
        grpc_url = loopback_url(ready["grpc_url"])
        receipt["fixture_endpoints"] = {"http_url": http_url, "grpc_url": grpc_url}
        drains["fixture_stdout"] = asyncio.create_task(
            drain_log(fixture_process.stdout, attempt / "fixture_stdout.log")
        )
        environment["VIBEMUX_TCK_GRPC_URL"] = grpc_url
        reports = attempt / "reports"
        reports.mkdir()
        # The same official pytest entrypoint as run_tck.py, with explicit unique
        # report destinations. No shared reports/ directory or test edits.
        command = [
            sys.executable,
            "-m",
            "pytest",
            "tests/compatibility/",
            f"--sut-host={http_url}",
            "--transport=" + ",".join(TRANSPORTS),
            "--tb=short",
            "-q",
            "-p",
            "a2a_tck_auth",
            f"--compatibility-report={reports / 'compatibility'}",
            f"--html={reports / 'tck_report.html'}",
            "--self-contained-html",
            f"--junitxml={reports / 'junitreport.xml'}",
        ]
        if args.level != "all":
            command.extend(["-m", args.level])
        pytest_process = await asyncio.create_subprocess_exec(
            *command,
            cwd=tck_root,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.STDOUT,
            env=environment,
            creationflags=creationflags,
        )
        assert pytest_process.stdout is not None
        drains["pytest_stdout"] = asyncio.create_task(
            drain_log(pytest_process.stdout, attempt / "pytest.log")
        )
        try:
            receipt["pytest_exit_code"] = await asyncio.wait_for(
                pytest_process.wait(), args.timeout_seconds
            )
        except TimeoutError as error:
            receipt["timed_out"] = True
            raise ValidationFailure("pytest_timeout") from error
        receipt["junit"] = junit_counts(reports / "junitreport.xml")
    except ValidationFailure as error:
        receipt["failure_code"] = str(error)
    except TimeoutError:
        receipt["failure_code"] = "fixture_startup_timeout"
    except (OSError, ValueError, subprocess.SubprocessError):
        receipt["failure_code"] = "validation_infrastructure_error"
    finally:
        if pytest_process is not None:
            try:
                await terminate_owned(pytest_process)
            except (TimeoutError, OSError):
                receipt["failure_code"] = receipt["failure_code"] or "pytest_cleanup_failed"
            receipt["pytest_exit_code"] = pytest_process.returncode
        if fixture_process is not None:
            receipt["fixture_exited_before_shutdown"] = fixture_process.returncode is not None
            if receipt["fixture_exited_before_shutdown"]:
                receipt["failure_code"] = (
                    receipt["failure_code"] or "fixture_exited_before_shutdown"
                )
            try:
                receipt["forced_fixture_cleanup"] = await stop_fixture(fixture_process)
            except (TimeoutError, OSError):
                receipt["forced_fixture_cleanup"] = True
                receipt["failure_code"] = receipt["failure_code"] or "fixture_cleanup_failed"
            receipt["fixture_exit_code"] = fixture_process.returncode
        for name, task in drains.items():
            try:
                receipt["logs_truncated"][name] = await asyncio.wait_for(task, SHUTDOWN_SECONDS)
            except (TimeoutError, OSError):
                task.cancel()
                await asyncio.gather(task, return_exceptions=True)
                receipt["failure_code"] = receipt["failure_code"] or "log_collection_incomplete"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tck-root", type=Path, required=True)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--tck-commit",
        default=DEFAULT_TCK_COMMIT,
        help="Exact official TCK Git commit, never a moving branch",
    )
    parser.add_argument("--level", choices=("all", "must", "should", "may"), default="all")
    parser.add_argument("--timeout-seconds", type=int, default=MAX_TEST_SECONDS)
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9a-f]{40}", args.tck_commit):
        parser.error("--tck-commit must be a full lowercase Git SHA")
    if not 1 <= args.timeout_seconds <= MAX_TEST_SECONDS:
        parser.error("--timeout-seconds must be between 1 and 420")
    if args.output.resolve().is_relative_to(args.tck_root.resolve()):
        parser.error("--output must be outside the official TCK checkout")
    stamp = datetime.now(UTC).strftime("%Y%m%d_%H%M%S_%f")
    attempt = args.output.resolve() / f"attempt_{stamp}_{uuid.uuid4().hex[:8]}"
    attempt.mkdir(parents=True, exist_ok=False)
    receipt: dict[str, Any] = {
        "schema_version": 1,
        "started_at": datetime.now(UTC).isoformat(),
        "attempt": attempt.name,
        "classification": "official_tck_with_synthetic_fixture_auth",
        "expected_tck_commit": args.tck_commit,
        "tck_commit": None,
        "level": args.level,
        "transports": TRANSPORTS,
        "timeout_seconds": args.timeout_seconds,
        "runtime_versions": runtime_versions(),
        "pytest_exit_code": None,
        "fixture_exit_code": None,
        "junit": None,
        "timed_out": False,
        "forced_fixture_cleanup": False,
        "fixture_exited_before_shutdown": False,
        "failure_code": None,
        "logs_truncated": {},
        "passed": False,
    }
    try:
        asyncio.run(run_attempt(args, attempt, receipt))
    except (KeyboardInterrupt, asyncio.CancelledError):
        receipt["failure_code"] = "validation_interrupted"
    finally:
        counts = receipt["junit"]
        receipt["passed"] = bool(
            receipt["failure_code"] is None
            and receipt["pytest_exit_code"] == 0
            and receipt["fixture_exit_code"] == 0
            and not receipt["forced_fixture_cleanup"]
            and counts
            and counts["passed"] > 0
            and counts["failures"] == 0
            and counts["errors"] == 0
            and all(counts["passed_by_binding"].values())
        )
        if not receipt["passed"] and receipt["failure_code"] is None:
            receipt["failure_code"] = "conformance_or_fixture_failed"
        receipt["completed_at"] = datetime.now(UTC).isoformat()
        artifact_names = {
            "pytest_log": "pytest.log",
            "fixture_stdout": "fixture_stdout.log",
            "fixture_stderr": "fixture_stderr.log",
            "junit": "reports/junitreport.xml",
            "compatibility_json": "reports/compatibility.json",
            "compatibility_html": "reports/compatibility.html",
            "pytest_html": "reports/tck_report.html",
        }
        receipt["artifacts"] = {
            key: value for key, value in artifact_names.items() if (attempt / value).is_file()
        }
        (attempt / "receipt.json").write_text(
            json.dumps(receipt, indent=2) + "\n", encoding="utf-8"
        )
        print(json.dumps(receipt, indent=2))
    return 0 if receipt["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
