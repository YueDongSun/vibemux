"""Harness profiles and safe launch specifications."""

from __future__ import annotations

import shutil
import sys
from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path
from typing import Protocol

from .errors import HarnessConfigurationError, HarnessNotFoundError


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


class HarnessAdapter(Protocol):
    def probe(self, profile: HarnessProfile) -> HarnessCapabilities: ...
    def validate_profile(self, profile: HarnessProfile) -> None: ...
    def build_launch_spec(self, profile: HarnessProfile, context: LaunchContext) -> LaunchSpec: ...


class GenericCommandAdapter:
    def probe(self, profile: HarnessProfile) -> HarnessCapabilities:
        path = shutil.which(profile.command[0]) if profile.command else None
        return HarnessCapabilities(path is not None, False, False, False, path)

    def validate_profile(self, profile: HarnessProfile) -> None:
        if not profile.command or any(not isinstance(part, str) or "\x00" in part for part in profile.command):
            raise HarnessConfigurationError("harness command must be a non-empty safe argv tuple")
        if profile.command[0].lower().endswith((".cmd", ".bat")):
            raise HarnessConfigurationError("batch launchers require an explicit controlled wrapper")

    def build_launch_spec(self, profile: HarnessProfile, context: LaunchContext) -> LaunchSpec:
        self.validate_profile(profile)
        executable = shutil.which(profile.command[0]) or profile.command[0]
        return LaunchSpec(executable, tuple(profile.command[1:]), context.cwd, {"VIBEMUX_RUN_ID": context.run_id})


class MockHarnessAdapter(GenericCommandAdapter):
    def probe(self, profile: HarnessProfile) -> HarnessCapabilities:
        return HarnessCapabilities(True, True, False, False, sys.executable)

    def build_launch_spec(self, profile: HarnessProfile, context: LaunchContext) -> LaunchSpec:
        return LaunchSpec(sys.executable, ("-m", "vibemux.mock_harness"), context.cwd, {"VIBEMUX_RUN_ID": context.run_id})


DEFAULT_PROFILES = {
    "mock": HarnessProfile("mock", ("vibemux-mock",), HarnessProtocol.MOCK),
    "opencode": HarnessProfile("opencode", ("opencode",)),
    "claude": HarnessProfile("claude", ("claude",)),
    "copilot": HarnessProfile("copilot", ("copilot",)),
    "pi": HarnessProfile("pi", ("pi",)),
    "grok": HarnessProfile("grok", ("grok",)),
    "gemini": HarnessProfile("gemini", ("gemini",)),
}


def profile_for(name: str) -> HarnessProfile:
    try:
        return DEFAULT_PROFILES[name]
    except KeyError as exc:
        raise HarnessNotFoundError(name) from exc
