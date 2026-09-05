"""Persisted harness detection snapshot and per-harness role usage."""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from pathlib import Path

from .errors import ConfigurationError


@dataclass
class HarnessState:
    detected: bool
    path: str | None = None
    roles: tuple[str, ...] = ()


@dataclass
class HarnessRegistry:
    checked_at: str = ""
    harnesses: dict[str, HarnessState] = field(default_factory=dict)

    def write(self, path: Path) -> None:
        payload = {
            "checked_at": self.checked_at,
            "harnesses": {
                name: {"detected": state.detected, "path": state.path, "roles": list(state.roles)}
                for name, state in sorted(self.harnesses.items())
            },
        }
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    @classmethod
    def read(cls, path: Path) -> HarnessRegistry:
        if not path.exists():
            return cls()
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
            states = {
                str(name): HarnessState(
                    detected=bool(item["detected"]),
                    path=str(item["path"]) if item.get("path") else None,
                    roles=tuple(str(role) for role in item.get("roles", [])),
                )
                for name, item in data["harnesses"].items()
            }
        except (OSError, ValueError, TypeError, KeyError, AttributeError) as exc:
            raise ConfigurationError(f"invalid harness registry: {path}") from exc
        return cls(checked_at=str(data.get("checked_at", "")), harnesses=states)


def record_role(
    path: Path, harness: str, detected: bool, executable_path: str | None, role: str
) -> None:
    registry = HarnessRegistry.read(path)
    state = registry.harnesses.get(harness, HarnessState(detected, executable_path))
    if role not in state.roles:
        state.roles = (*state.roles, role)
    registry.harnesses[harness] = state
    registry.write(path)
