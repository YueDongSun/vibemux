"""Real process-tree timeout coverage for the acceptance command owner."""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import pytest

repo_root = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(repo_root / "scripts"))

from dual_track_acceptance import check_execution, process_execution  # noqa: E402
from dual_track_acceptance.process_execution import run_owned_command  # noqa: E402
from dual_track_acceptance.windows_launch_context import Windows_Launch_Error  # noqa: E402


def test_timeout_terminates_the_owned_parent_and_its_child(tmp_path: Path) -> None:
    child_pid_file = tmp_path / "child_pid"
    child_code = (
        "import os,sys,time; from pathlib import Path; "
        "Path(sys.argv[1]).write_text(str(os.getpid()),encoding='utf-8'); time.sleep(60)"
    )
    parent_code = (
        "import subprocess,sys,time; "
        "subprocess.Popen([sys.executable,'-c',sys.argv[1],sys.argv[2]]); time.sleep(60)"
    )
    with (tmp_path / "command.log").open("wb") as log:
        exit_code, timed_out = run_owned_command(
            [sys.executable, "-c", parent_code, child_code, str(child_pid_file)],
            cwd=tmp_path,
            env=dict(os.environ),
            stdout=log,
            stderr=subprocess.STDOUT,
            timeout_seconds=3,
        )
    assert timed_out and exit_code is None
    assert child_pid_file.is_file(), "the child must start before the timeout is exercised"
    child_pid = int(child_pid_file.read_text(encoding="utf-8"))
    if sys.platform == "win32":
        inventory = subprocess.run(
            ["tasklist", "/FI", f"PID eq {child_pid}", "/FO", "CSV", "/NH"],
            capture_output=True,
            check=True,
            timeout=10,
        )
        assert f'"{child_pid}"' not in inventory.stdout.decode("utf-8", errors="replace")
    else:
        inventory = subprocess.run(
            ["ps", "-o", "stat=", "-p", str(child_pid)],
            capture_output=True,
            check=False,
            timeout=10,
        )
        # A reparented zombie has terminated but may await its init reaper.
        state = inventory.stdout.decode("utf-8").strip()
        assert not state or state.startswith("Z")


def test_normal_command_exit_is_preserved(tmp_path: Path) -> None:
    with (tmp_path / "command.log").open("wb") as log:
        assert run_owned_command(
            [sys.executable, "-c", "raise SystemExit(7)"],
            cwd=tmp_path,
            env=dict(os.environ),
            stdout=log,
            stderr=subprocess.STDOUT,
            timeout_seconds=10,
        ) == (7, False)


def test_launch_refusal_happens_before_child_probe(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.setattr(
        process_execution,
        "require_launch_context",
        lambda: (_ for _ in ()).throw(Windows_Launch_Error("windows_restricted_token", "refused")),
    )
    monkeypatch.setattr(
        process_execution.subprocess,
        "Popen",
        lambda *args, **kwargs: pytest.fail("Popen called"),
    )
    log_path = tmp_path / "launch.log"
    with log_path.open("wb") as log, pytest.raises(Windows_Launch_Error):
        run_owned_command(
            ["tool.exe", "literal bytes"],
            cwd=tmp_path,
            env=dict(os.environ),
            stdout=log,
            stderr=subprocess.STDOUT,
            timeout_seconds=1,
        )


def test_process_creation_preserves_argv_and_windows_noninteractive_flags(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    captured: dict[str, object] = {}

    class Fake_Process:
        pid = 123

        @staticmethod
        def wait(timeout: float) -> int:
            return 9

    def fake_popen(argv: list[str], **kwargs: object) -> Fake_Process:
        captured["argv"] = argv
        captured.update(kwargs)
        return Fake_Process()

    monkeypatch.setattr(process_execution, "require_launch_context", lambda: None)
    monkeypatch.setattr(process_execution.subprocess, "Popen", fake_popen)
    argv = ["tool.exe", "--label", "spaces & quotes ' stay literal"]
    with (tmp_path / "launch.log").open("wb") as log:
        result = run_owned_command(
            argv,
            cwd=tmp_path,
            env={"PATH": "exact"},
            stdout=log,
            stderr=subprocess.STDOUT,
            timeout_seconds=1,
        )
    assert result == (9, False)
    assert captured["argv"] == argv
    assert captured["shell"] is False
    if sys.platform == "win32":
        assert captured["creationflags"] & subprocess.CREATE_NO_WINDOW
        assert captured["creationflags"] & subprocess.CREATE_NEW_PROCESS_GROUP


def test_version_probe_preserves_dll_initialization_failure(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(check_execution, "require_launch_context", lambda: None)
    argv = [r"C:\Program Files\Tool\ruff.exe", "--version"]
    monkeypatch.setattr(
        check_execution.subprocess,
        "run",
        lambda actual_argv, **kwargs: subprocess.CompletedProcess(
            actual_argv, -1073741502, stdout=b"", stderr=b""
        ),
    )
    with pytest.raises(Windows_Launch_Error) as error:
        check_execution.probe_version(argv, {"PATH": "literal"}, 5)
    assert error.value.code == "windows_tool_initialization_failed"
    assert "-1073741502" in str(error.value)
    assert "0xc0000142" in str(error.value)


def test_version_probe_refuses_before_native_call_when_context_is_unknown(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        check_execution,
        "require_launch_context",
        lambda: (_ for _ in ()).throw(
            Windows_Launch_Error("windows_token_context_unknown", "refused")
        ),
    )
    monkeypatch.setattr(
        check_execution.subprocess,
        "run",
        lambda *args, **kwargs: pytest.fail("native probe called"),
    )
    with pytest.raises(Windows_Launch_Error):
        check_execution.probe_version(["tool.exe", "--version"], {}, 5)
