"""Real process-tree timeout coverage for the acceptance command owner."""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

repo_root = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(repo_root / "scripts"))

from dual_track_acceptance.process_execution import run_owned_command  # noqa: E402


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
