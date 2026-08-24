"""Offline cross-platform smoke checks for the package."""

from __future__ import annotations

import subprocess
import sys
import tempfile
from pathlib import Path


def run(command: list[str], cwd: Path) -> str:
    result = subprocess.run(command, cwd=cwd, text=True, capture_output=True, check=False)
    if result.returncode:
        raise RuntimeError(result.stderr or result.stdout)
    return result.stdout.strip()


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="vibemux_smoke_") as temp:
        repo = Path(temp)
        run(["git", "init", "-b", "main"], repo)
        run(["git", "config", "user.name", "VibeMux Smoke"], repo)
        run(["git", "config", "user.email", "smoke@example.invalid"], repo)
        (repo / "README.md").write_text("smoke\n", encoding="utf-8")
        run(["git", "add", "README.md"], repo)
        run(["git", "commit", "-m", "initial"], repo)
        root = Path(__file__).resolve().parents[1]
        env = dict(__import__("os").environ, PYTHONPATH=str(root / "src"))
        cli = [sys.executable, "-m", "vibemux.cli"]
        subprocess.run([*cli, "init", "--terminal-backend", "mock"], cwd=repo, env=env, check=True)
        task_result = subprocess.run(
            [*cli, "task", "smoke"], cwd=repo, env=env, text=True, capture_output=True, check=True
        )
        task = task_result.stdout.strip()
        spawn_result = subprocess.run(
            [*cli, "spawn", task, "--terminal-backend", "mock"],
            cwd=repo,
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )
        if spawn_result.returncode:
            raise RuntimeError(spawn_result.stderr or spawn_result.stdout)
        run_id = spawn_result.stdout.strip()
        subprocess.run([*cli, "send", run_id, "PING", "--submit"], cwd=repo, env=env, check=True)
        subprocess.run([*cli, "stop", run_id], cwd=repo, env=env, check=True)
    print("PASS: mock workflow")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
