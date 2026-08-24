"""Single shell-free command execution boundary."""

from __future__ import annotations

import os
import re
import subprocess
from dataclasses import dataclass, field
from pathlib import Path
from typing import Protocol

from .errors import CommandExecutionError

DEFAULT_COMMAND_TIMEOUT_SECONDS = 30.0
ENVIRONMENT_KEY_PATTERN = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")


@dataclass(frozen=True)
class Command:
    executable: str
    args: tuple[str, ...] = ()
    cwd: Path | None = None
    input_text: str | None = None
    environment: dict[str, str] = field(default_factory=dict)
    inherit_environment: bool = True
    capture_output: bool = True
    timeout_seconds: float | None = DEFAULT_COMMAND_TIMEOUT_SECONDS

    def validate(self) -> None:
        values = (self.executable, *self.args)
        if not self.executable or any(
            not isinstance(value, str) or "\x00" in value for value in values
        ):
            raise CommandExecutionError("command must be a non-empty argv without null bytes")
        if self.input_text is not None and (
            not isinstance(self.input_text, str) or "\x00" in self.input_text
        ):
            raise CommandExecutionError("command stdin contains a null byte")
        if self.timeout_seconds is not None and self.timeout_seconds <= 0:
            raise CommandExecutionError("command timeout must be positive")
        for key, value in self.environment.items():
            if (
                not isinstance(key, str)
                or not isinstance(value, str)
                or not ENVIRONMENT_KEY_PATTERN.fullmatch(key)
                or "\x00" in value
            ):
                raise CommandExecutionError("command environment contains an invalid entry")


@dataclass(frozen=True)
class CommandResult:
    returncode: int
    stdout: str = ""
    stderr: str = ""


class CommandRunner(Protocol):
    def run(self, command: Command) -> CommandResult: ...


class SubprocessCommandRunner:
    """Run validated argv with shell disabled and bounded execution time."""

    def run(self, command: Command) -> CommandResult:
        command.validate()
        environment: dict[str, str] | None = None
        if command.environment or not command.inherit_environment:
            environment = dict(os.environ) if command.inherit_environment else {}
            environment.update(command.environment)
        try:
            result = subprocess.run(
                [command.executable, *command.args],
                cwd=command.cwd,
                input=command.input_text,
                env=environment,
                text=True,
                capture_output=command.capture_output,
                timeout=command.timeout_seconds,
                check=False,
                shell=False,
            )
        except subprocess.TimeoutExpired as exc:
            raise CommandExecutionError("command exceeded its deadline") from exc
        except OSError as exc:
            raise CommandExecutionError("command could not be started") from exc
        return CommandResult(result.returncode, result.stdout or "", result.stderr or "")
