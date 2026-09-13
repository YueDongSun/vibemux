"""Harness profiles and safe launch specifications."""

from __future__ import annotations

import json
import os
import platform
import shutil
import sys
from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path
from typing import Protocol

from .errors import HarnessConfigurationError, HarnessNotFoundError

AGENT_HOST_MODULE = "vibemux.agent_host"
WINDOWS_DIRECT_SUFFIXES = (".exe", ".com")
WINDOWS_SCRIPT_SUFFIXES = (".cmd", ".bat", ".ps1")
POWERSHELL_SCRIPT_ARGS = (
    "-NoLogo",
    "-NoProfile",
    "-NonInteractive",
    "-ExecutionPolicy",
    "Bypass",
    "-File",
)


class HarnessProtocol(StrEnum):
    PTY = "pty"
    ACP = "acp"
    RPC = "rpc"
    STREAM_JSON = "stream_json"
    MOCK = "mock"


@dataclass(frozen=True)
class HarnessProfile:
    name: str
    command: tuple[str, ...]
    protocol: HarnessProtocol = HarnessProtocol.PTY
    provider: str | None = None
    model: str | None = None


@dataclass(frozen=True)
class HarnessCapabilities:
    available: bool
    structured_events: bool
    supports_approval: bool
    supports_resume: bool
    path: str | None = None


@dataclass(frozen=True)
class LaunchContext:
    cwd: Path
    run_id: str


@dataclass(frozen=True)
class LaunchSpec:
    executable: str
    args: tuple[str, ...]
    cwd: Path
    environment: dict[str, str]

    def as_command(self) -> list[str]:
        return [self.executable, *self.args]

    def as_host_command(self) -> list[str]:
        return [
            sys.executable,
            "-m",
            AGENT_HOST_MODULE,
            "--cwd",
            str(self.cwd),
            "--environment-json",
            json.dumps(self.environment, ensure_ascii=True, separators=(",", ":")),
            "--",
            self.executable,
            *self.args,
        ]


class HarnessAdapter(Protocol):
    def probe(self, profile: HarnessProfile) -> HarnessCapabilities: ...
    def validate_profile(self, profile: HarnessProfile) -> None: ...
    def build_launch_spec(self, profile: HarnessProfile, context: LaunchContext) -> LaunchSpec: ...


class GenericCommandAdapter:
    def probe(self, profile: HarnessProfile) -> HarnessCapabilities:
        try:
            self.validate_profile(profile)
            resolved = self._locate_command(profile.command[0])
            if resolved is None:
                return HarnessCapabilities(False, False, False, False, None)
            executable, args = self._resolve_command(profile, resolved)
        except HarnessConfigurationError:
            return HarnessCapabilities(False, False, False, False, None)
        path = (
            args[len(POWERSHELL_SCRIPT_ARGS)] if self._is_powershell_wrapper(args) else executable
        )
        return HarnessCapabilities(True, False, False, False, path)

    def validate_profile(self, profile: HarnessProfile) -> None:
        if not profile.command or any(
            not isinstance(part, str) or "\x00" in part for part in profile.command
        ):
            raise HarnessConfigurationError("harness command must be a non-empty safe argv tuple")

    def build_launch_spec(self, profile: HarnessProfile, context: LaunchContext) -> LaunchSpec:
        self.validate_profile(profile)
        executable, args = self._resolve_command(profile)
        return LaunchSpec(executable, args, context.cwd, {"VIBEMUX_RUN_ID": context.run_id})

    def _resolve_command(
        self,
        profile: HarnessProfile,
        resolved: str | None = None,
    ) -> tuple[str, tuple[str, ...]]:
        resolved = resolved or self._locate_command(profile.command[0]) or profile.command[0]
        suffix = Path(resolved).suffix.lower()
        if platform.system() != "Windows" or suffix not in WINDOWS_SCRIPT_SUFFIXES:
            return resolved, tuple(profile.command[1:])
        script = Path(resolved) if suffix == ".ps1" else Path(resolved).with_suffix(".ps1")
        powershell = shutil.which("powershell") or shutil.which("pwsh")
        if not script.is_file() or powershell is None:
            raise HarnessConfigurationError(
                "Windows script launcher has no controlled PowerShell wrapper"
            )
        return powershell, (*POWERSHELL_SCRIPT_ARGS, str(script), *profile.command[1:])

    @classmethod
    def _locate_command(cls, command: str) -> str | None:
        direct = (
            cls._find_direct_windows_executable(command) if platform.system() == "Windows" else None
        )
        return direct or shutil.which(command)

    @staticmethod
    def _find_direct_windows_executable(command: str) -> str | None:
        command_path = Path(command)
        if command_path.suffix.lower() in WINDOWS_DIRECT_SUFFIXES:
            return str(command_path) if command_path.is_file() else None
        if command_path.parent != Path("."):
            return None
        for directory in os.get_exec_path():
            for suffix in WINDOWS_DIRECT_SUFFIXES:
                candidate = Path(directory) / f"{command}{suffix}"
                if candidate.is_file():
                    return str(candidate)
        return None

    @staticmethod
    def _is_powershell_wrapper(args: tuple[str, ...]) -> bool:
        return args[: len(POWERSHELL_SCRIPT_ARGS)] == POWERSHELL_SCRIPT_ARGS


class MockHarnessAdapter(GenericCommandAdapter):
    def probe(self, profile: HarnessProfile) -> HarnessCapabilities:
        return HarnessCapabilities(True, True, False, False, sys.executable)

    def build_launch_spec(self, profile: HarnessProfile, context: LaunchContext) -> LaunchSpec:
        return LaunchSpec(
            sys.executable,
            ("-m", "vibemux.mock_harness"),
            context.cwd,
            {"VIBEMUX_RUN_ID": context.run_id},
        )


DEFAULT_PROFILES = {
    "mock": HarnessProfile("mock", ("vibemux-mock",), HarnessProtocol.MOCK),
    "opencode": HarnessProfile("opencode", ("opencode",)),
    "claude": HarnessProfile("claude", ("claude",)),
    "copilot": HarnessProfile("copilot", ("copilot",)),
    "pi": HarnessProfile("pi", ("pi",)),
    "grok": HarnessProfile("grok", ("grok",)),
    "gemini": HarnessProfile("gemini", ("gemini",)),
    # Chinese CLI agents: executable name and vendor only; interactive protocol is always PTY.
    "qwen": HarnessProfile("qwen", ("qwen",), provider="alibaba"),
    "iflow": HarnessProfile("iflow", ("iflow",), provider="alibaba"),
    "trae": HarnessProfile("trae", ("trae",), provider="bytedance"),
    "codebuddy": HarnessProfile("codebuddy", ("codebuddy",), provider="tencent"),
    "kimi": HarnessProfile("kimi", ("kimi",), provider="moonshot"),
}


def profile_for(name: str) -> HarnessProfile:
    try:
        return DEFAULT_PROFILES[name]
    except KeyError as exc:
        raise HarnessNotFoundError(name) from exc


def adapter_for(profile: HarnessProfile) -> HarnessAdapter:
    if profile.protocol is HarnessProtocol.MOCK:
        return MockHarnessAdapter()
    return GenericCommandAdapter()
