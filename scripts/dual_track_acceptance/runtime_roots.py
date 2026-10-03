"""The run's owned output directory, private roots, and process bookkeeping."""

from __future__ import annotations

import csv
import io
import os
import subprocess
import sys
import tempfile
from collections.abc import Iterable
from dataclasses import dataclass
from pathlib import Path

from .runner_config import (
    EVIDENCE_DIR_NAME,
    LOG_DIR_NAME,
    MAX_PRIVATE_TMP_PATH_CHARS,
    PRIVATE_TMP_PREFIX,
    REPORT_FILE_NAME,
    VERSION_PROBE_TIMEOUT_SECONDS,
)


class OutputError(ValueError):
    """The requested output directory cannot be owned by this run."""


@dataclass(frozen=True)
class OutputLayout:
    root: Path
    logs: Path
    evidence: Path
    private_tmp: Path
    report: Path


def _is_within(path: Path, parent: Path) -> bool:
    try:
        path.relative_to(parent)
    except ValueError:
        return False
    return True


def prepare_output(output: Path, repo_root: Path, tmp_parent: Path | None = None) -> OutputLayout:
    """Creates the run's own output tree and private temporary root.

    The directory must be outside the repository (the suites must see a
    clean checkout) and must be new or empty, so nothing in it predates the
    run. The private temporary root is a new, uniquely named directory under
    `tmp_parent` (default: the system temporary directory), created
    exclusively so no other process owns it.
    """
    root = output.resolve()
    if _is_within(root, repo_root.resolve()):
        raise OutputError("the output directory must be outside the repository")
    if root.exists():
        if not root.is_dir():
            raise OutputError("the output path exists and is not a directory")
        if any(root.iterdir()):
            raise OutputError("the output directory is not empty")
    logs = root / LOG_DIR_NAME
    evidence = root / EVIDENCE_DIR_NAME
    for directory in (logs, evidence):
        directory.mkdir(parents=True, exist_ok=False)
    private_tmp = Path(tempfile.mkdtemp(prefix=PRIVATE_TMP_PREFIX, dir=tmp_parent))
    return OutputLayout(
        root=root,
        logs=logs,
        evidence=evidence,
        private_tmp=private_tmp,
        report=root / REPORT_FILE_NAME,
    )


def private_tmp_problems(layout: OutputLayout) -> list[str]:
    """Why the private temporary root cannot host the suites, if it cannot."""
    length = len(str(layout.private_tmp))
    if length > MAX_PRIVATE_TMP_PATH_CHARS:
        return [
            f"the private temporary root is {length} characters long, more than "
            f"{MAX_PRIVATE_TMP_PATH_CHARS}; nested test worktrees would exceed Windows path limits"
        ]
    return []


def private_environment(layout: OutputLayout, base: dict[str, str]) -> dict[str, str]:
    """Temporary files of every child go under the run's private root."""
    environment = dict(base)
    for name in ("TMP", "TEMP", "TMPDIR"):
        environment[name] = str(layout.private_tmp)
    return environment


def retained_entries(private_tmp: Path) -> list[str]:
    """What the suites left in the private temporary root."""
    if not private_tmp.is_dir():
        return []
    return sorted(entry.name for entry in private_tmp.iterdir())


def remove_if_empty(directory: Path) -> bool:
    try:
        directory.rmdir()
    except OSError:
        return False
    return True


ProcessKey = tuple[int, str]


def _windows_processes() -> list[ProcessKey]:
    completed = subprocess.run(
        ["tasklist", "/FO", "CSV", "/NH"],
        capture_output=True,
        stdin=subprocess.DEVNULL,
        timeout=VERSION_PROBE_TIMEOUT_SECONDS,
        check=True,
    )
    text = completed.stdout.decode("utf-8", errors="replace")
    processes: list[ProcessKey] = []
    for row in csv.reader(io.StringIO(text)):
        if len(row) >= 2 and row[1].isdigit():
            processes.append((int(row[1]), row[0]))
    return processes


def _posix_processes() -> list[ProcessKey]:
    completed = subprocess.run(
        ["ps", "-eo", "pid=,comm="],
        capture_output=True,
        stdin=subprocess.DEVNULL,
        timeout=VERSION_PROBE_TIMEOUT_SECONDS,
        check=True,
    )
    processes: list[ProcessKey] = []
    for line in completed.stdout.decode("utf-8", errors="replace").splitlines():
        pid, _, name = line.strip().partition(" ")
        if pid.isdigit():
            processes.append((int(pid), os.path.basename(name.strip())))
    return processes


def owned_process_snapshot(names: Iterable[str]) -> set[ProcessKey] | None:
    """Running processes whose executable is one the suites start, or None
    if the process table cannot be read."""
    wanted = {name.casefold() for name in names}
    try:
        processes = _windows_processes() if sys.platform == "win32" else _posix_processes()
    except (OSError, subprocess.SubprocessError):
        return None
    return {
        (pid, image) for pid, image in processes if image.casefold().removesuffix(".exe") in wanted
    }
