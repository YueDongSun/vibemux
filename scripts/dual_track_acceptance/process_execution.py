"""Owns an acceptance command's process tree through completion or timeout."""

from __future__ import annotations

import os
import signal
import subprocess
import sys
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import IO

process_termination_timeout_seconds = 15
windows_taskkill = Path(os.environ.get("SystemRoot", r"C:\Windows")) / "System32" / "taskkill.exe"


def terminate_owned_tree(process: subprocess.Popen[bytes]) -> None:
    """Terminates only the process tree this invocation created.

    Failure propagates so the caller retains its runtime files for diagnosis.
    Windows taskkill targets the live owned parent; POSIX descendants stay in
    the new session's process group even if the parent exits during timeout.
    """
    if sys.platform == "win32":
        terminated = subprocess.run(
            [str(windows_taskkill), "/PID", str(process.pid), "/T", "/F"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=process_termination_timeout_seconds,
            check=False,
        )
        if terminated.returncode != 0:
            raise OSError("owned process tree termination could not be confirmed")
    else:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    process.wait(timeout=process_termination_timeout_seconds)


def run_owned_command(
    argv: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
    stdout: IO[bytes],
    stderr: IO[bytes] | int,
    timeout_seconds: float,
) -> tuple[int | None, bool]:
    """Returns the exit status and timeout flag after owned-tree termination."""
    process = subprocess.Popen(
        list(argv),
        cwd=cwd,
        env=dict(env),
        stdin=subprocess.DEVNULL,
        stdout=stdout,
        stderr=stderr,
        shell=False,
        start_new_session=sys.platform != "win32",
        creationflags=subprocess.CREATE_NEW_PROCESS_GROUP if sys.platform == "win32" else 0,
    )
    try:
        return process.wait(timeout=timeout_seconds), False
    except subprocess.TimeoutExpired:
        terminate_owned_tree(process)
        return None, True
    except BaseException:
        terminate_owned_tree(process)
        raise
