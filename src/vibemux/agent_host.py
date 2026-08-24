"""Controlled cross-platform harness process host."""

from __future__ import annotations

import argparse
import json
from collections.abc import Sequence
from pathlib import Path

from .command_runner import Command, CommandRunner, SubprocessCommandRunner
from .errors import CommandExecutionError

MAX_ENVIRONMENT_JSON_BYTES = 16 * 1024


def main(argv: Sequence[str] | None = None, *, runner: CommandRunner | None = None) -> int:
    parser = argparse.ArgumentParser(prog="vibemux-agent-host")
    parser.add_argument("--cwd", required=True)
    parser.add_argument("--environment-json", default="{}")
    parser.add_argument("target", nargs=argparse.REMAINDER)
    arguments = parser.parse_args(argv)
    target = list(arguments.target)
    if target and target[0] == "--":
        target.pop(0)
    if not target:
        parser.error("target executable is required")
    if len(arguments.environment_json.encode("utf-8")) > MAX_ENVIRONMENT_JSON_BYTES:
        parser.error("environment JSON exceeds the allowed size")
    try:
        environment = json.loads(arguments.environment_json)
    except json.JSONDecodeError as exc:
        parser.error(f"invalid environment JSON: {exc.msg}")
    if not isinstance(environment, dict) or any(
        not isinstance(key, str) or not isinstance(value, str) for key, value in environment.items()
    ):
        parser.error("environment JSON must be a string map")
    command = Command(
        target[0],
        tuple(target[1:]),
        cwd=Path(arguments.cwd),
        environment=environment,
        # The Python reference inherits the harness environment for compatibility.
        # The Rust plugin permission model will replace this temporary boundary.
        inherit_environment=True,
        capture_output=False,
        timeout_seconds=None,
    )
    try:
        return (runner or SubprocessCommandRunner()).run(command).returncode
    except CommandExecutionError:
        return 126


if __name__ == "__main__":
    raise SystemExit(main())
