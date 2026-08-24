"""Terminal data-plane contracts and backend adapters."""

from __future__ import annotations

import hashlib
import json
import os
import tempfile
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Protocol
from uuid import uuid4

from .command_runner import Command, CommandResult, CommandRunner, SubprocessCommandRunner
from .errors import (
    CommandExecutionError,
    TerminalBackendError,
    TerminalNotAvailableError,
    TerminalResourceMismatchError,
)
from .models import TerminalLocation

MOCK_TERMINAL_SCHEMA_VERSION = 1


@dataclass(frozen=True)
class Pane:
    pane_id: str
    cwd: str
    title: str = ""
    workspace_id: str | None = None
    alive: bool = True


@dataclass(frozen=True)
class SendReceipt:
    digest: str
    byte_count: int
    line_count: int
    submit: bool


class TerminalBackend(Protocol):
    name: str

    def probe(self) -> bool: ...
    def open(
        self, location: TerminalLocation, command: list[str], cwd: Path
    ) -> TerminalLocation: ...
    def list(self) -> list[Pane]: ...
    def send_text(self, pane_id: str, text: str, *, submit: bool = False) -> SendReceipt: ...
    def stop(self, location: TerminalLocation) -> None: ...
    def activate(self, location: TerminalLocation) -> None: ...


def _receipt(text: str, submit: bool) -> SendReceipt:
    return SendReceipt(
        hashlib.sha256(text.encode("utf-8")).hexdigest(),
        len(text.encode("utf-8")),
        text.count("\n") + (1 if text else 0),
        submit,
    )


class MockTerminalBackend:
    name = "mock"

    def __init__(self, state_path: Path | None = None) -> None:
        self.state_path = state_path
        self.panes: dict[str, Pane] = {}
        self.messages: list[tuple[str, str, bool]] = []

    def probe(self) -> bool:
        return True

    def open(self, location: TerminalLocation, command: list[str], cwd: Path) -> TerminalLocation:
        panes = self._load_panes()
        pane_id = location.pane_id or str(uuid4())
        panes[pane_id] = Pane(
            pane_id,
            str(cwd),
            title=location.metadata.get("title", "vibemux"),
            workspace_id=location.workspace_id,
        )
        self._write_panes(panes)
        return TerminalLocation(
            self.name,
            resource_id=pane_id,
            workspace_id=location.workspace_id,
            pane_id=pane_id,
            cwd=str(cwd),
            metadata=location.metadata,
        )

    def list(self) -> list[Pane]:
        return list(self._load_panes().values())

    def send_text(self, pane_id: str, text: str, *, submit: bool = False) -> SendReceipt:
        panes = self._load_panes()
        if pane_id not in panes:
            raise TerminalResourceMismatchError(pane_id)
        self.messages.append((pane_id, text, submit))
        return _receipt(text, submit)

    def stop(self, location: TerminalLocation) -> None:
        if not location.pane_id:
            return
        panes = self._load_panes()
        if location.pane_id not in panes:
            raise TerminalResourceMismatchError(location.pane_id)
        panes.pop(location.pane_id)
        self._write_panes(panes)

    def activate(self, location: TerminalLocation) -> None:
        if location.pane_id and location.pane_id not in self._load_panes():
            raise TerminalResourceMismatchError(location.pane_id)

    def _load_panes(self) -> dict[str, Pane]:
        if self.state_path is None:
            return dict(self.panes)
        if not self.state_path.exists():
            return {}
        try:
            payload = json.loads(self.state_path.read_text(encoding="utf-8"))
            if payload.get("schema_version") != MOCK_TERMINAL_SCHEMA_VERSION:
                raise ValueError("unsupported schema")
            panes = {
                str(item["pane_id"]): Pane(**item)
                for item in payload.get("panes", [])
                if isinstance(item, dict)
            }
        except (KeyError, OSError, TypeError, ValueError) as exc:
            raise TerminalBackendError("invalid mock terminal inventory") from exc
        self.panes = panes
        return dict(panes)

    def _write_panes(self, panes: dict[str, Pane]) -> None:
        self.panes = dict(panes)
        if self.state_path is None:
            return
        self.state_path.parent.mkdir(parents=True, exist_ok=True)
        temporary_path = self.state_path.with_suffix(self.state_path.suffix + ".tmp")
        temporary_path.write_text(
            json.dumps(
                {
                    "schema_version": MOCK_TERMINAL_SCHEMA_VERSION,
                    "panes": [asdict(pane) for pane in panes.values()],
                },
                ensure_ascii=False,
                indent=2,
            )
            + "\n",
            encoding="utf-8",
        )
        temporary_path.replace(self.state_path)


class WezTermBackend:
    name = "wezterm"

    def __init__(
        self,
        executable: str = "wezterm",
        runner: CommandRunner | None = None,
    ) -> None:
        self.executable = executable
        self.runner = runner or SubprocessCommandRunner()

    def _run(self, args: list[str], *, input_text: str | None = None) -> CommandResult:
        try:
            return self.runner.run(Command(self.executable, tuple(args), input_text=input_text))
        except CommandExecutionError as exc:
            raise TerminalNotAvailableError(self.executable) from exc

    def probe(self) -> bool:
        return self._run(["--version"]).returncode == 0

    def open(self, location: TerminalLocation, command: list[str], cwd: Path) -> TerminalLocation:
        args = ["cli", "spawn", "--cwd", os.fspath(cwd)]
        if location.workspace_id:
            args.extend(["--new-window", "--workspace", location.workspace_id])
        result = self._run([*args, "--", *command])
        if result.returncode != 0:
            raise TerminalBackendError(result.stderr.strip() or "wezterm spawn failed")
        pane_id = result.stdout.strip().splitlines()[-1] if result.stdout.strip() else None
        if not pane_id:
            raise TerminalBackendError("wezterm returned no pane id")
        return TerminalLocation(
            self.name,
            resource_id=pane_id,
            pane_id=pane_id,
            cwd=str(cwd),
            workspace_id=location.workspace_id,
            metadata=location.metadata,
        )

    def list(self) -> list[Pane]:
        result = self._run(["cli", "list", "--format", "json"])
        if result.returncode != 0:
            raise TerminalBackendError(result.stderr.strip() or "wezterm list failed")
        try:
            payload = json.loads(result.stdout or "[]")
        except json.JSONDecodeError as exc:
            raise TerminalBackendError("malformed wezterm JSON") from exc
        return [
            Pane(
                str(item.get("pane_id")),
                item.get("cwd", ""),
                item.get("title", ""),
                item.get("workspace"),
                item.get("is_dead") is not True,
            )
            for item in payload
        ]

    def send_text(self, pane_id: str, text: str, *, submit: bool = False) -> SendReceipt:
        result = self._run(["cli", "send-text", "--pane-id", pane_id], input_text=text)
        if result.returncode != 0:
            raise TerminalBackendError(result.stderr.strip() or "wezterm send failed")
        if submit:
            result = self._run(["cli", "send-text", "--pane-id", pane_id], input_text="\r")
            if result.returncode != 0:
                raise TerminalBackendError(result.stderr.strip() or "wezterm submit failed")
        return _receipt(text, submit)

    def stop(self, location: TerminalLocation) -> None:
        if not location.pane_id:
            return
        if not any(p.pane_id == location.pane_id for p in self.list()):
            return
        result = self._run(["cli", "kill-pane", "--pane-id", location.pane_id])
        if result.returncode != 0:
            raise TerminalBackendError(result.stderr.strip() or "wezterm kill failed")

    def activate(self, location: TerminalLocation) -> None:
        if location.pane_id:
            result = self._run(["cli", "activate-pane", "--pane-id", location.pane_id])
            if result.returncode != 0:
                raise TerminalBackendError(result.stderr.strip() or "wezterm activate failed")


class TmuxBackend:
    name = "tmux"

    def __init__(
        self,
        executable: str = "tmux",
        socket_name: str | None = None,
        runner: CommandRunner | None = None,
    ) -> None:
        self.executable = executable
        self.socket_name = socket_name
        self.runner = runner or SubprocessCommandRunner()

    def _base(self) -> list[str]:
        return ["-L", self.socket_name] if self.socket_name else []

    def _run(self, args: list[str], *, input_text: str | None = None) -> CommandResult:
        try:
            return self.runner.run(
                Command(self.executable, tuple([*self._base(), *args]), input_text=input_text)
            )
        except CommandExecutionError as exc:
            raise TerminalNotAvailableError(self.executable) from exc

    def probe(self) -> bool:
        try:
            return self._run(["-V"]).returncode == 0
        except TerminalNotAvailableError:
            return False

    def open(self, location: TerminalLocation, command: list[str], cwd: Path) -> TerminalLocation:
        session = location.workspace_id or f"vibemux_{uuid4().hex[:8]}"
        result = self._run(
            [
                "new-session",
                "-d",
                "-P",
                "-F",
                "#{pane_id}",
                "-s",
                session,
                "-c",
                os.fspath(cwd),
                "--",
                *command,
            ]
        )
        if result.returncode != 0:
            raise TerminalBackendError(result.stderr.strip() or "tmux new-session failed")
        pane_id = result.stdout.strip().splitlines()[-1] if result.stdout.strip() else None
        if not pane_id:
            raise TerminalBackendError("tmux returned no pane id")
        return TerminalLocation(
            self.name,
            resource_id=pane_id,
            workspace_id=session,
            pane_id=pane_id,
            cwd=str(cwd),
            metadata=location.metadata,
        )

    def list(self) -> list[Pane]:
        result = self._run(
            [
                "list-panes",
                "-a",
                "-F",
                "#{pane_id}\t#{pane_current_path}\t#{pane_title}\t#{pane_dead}",
            ]
        )
        if result.returncode != 0:
            if "no server running" in result.stderr.lower():
                return []
            raise TerminalBackendError(result.stderr.strip() or "tmux list failed")
        panes = []
        for line in result.stdout.splitlines():
            fields = line.split("\t", 3)
            if len(fields) == 4:
                panes.append(
                    Pane(
                        fields[0],
                        fields[1],
                        fields[2],
                        fields[0].split(":", 1)[0],
                        fields[3] != "1",
                    )
                )
        return panes

    def send_text(self, pane_id: str, text: str, *, submit: bool = False) -> SendReceipt:
        with tempfile.NamedTemporaryFile(prefix="vibemux_", suffix=".txt", delete=False) as handle:
            handle.write(text.encode("utf-8"))
            path = handle.name
        try:
            result = self._run(["load-buffer", path])
            if result.returncode != 0:
                raise TerminalBackendError(result.stderr.strip() or "tmux load-buffer failed")
            result = self._run(["paste-buffer", "-t", pane_id, "-d"])
            if result.returncode != 0:
                raise TerminalBackendError(result.stderr.strip() or "tmux paste-buffer failed")
            if submit:
                result = self._run(["send-keys", "-t", pane_id, "Enter"])
                if result.returncode != 0:
                    raise TerminalBackendError(result.stderr.strip() or "tmux submit failed")
        finally:
            Path(path).unlink(missing_ok=True)
        return _receipt(text, submit)

    def stop(self, location: TerminalLocation) -> None:
        if location.pane_id:
            self._run(["kill-pane", "-t", location.pane_id])

    def activate(self, location: TerminalLocation) -> None:
        if location.workspace_id:
            self._run(["switch-client", "-t", location.workspace_id])
