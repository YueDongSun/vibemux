"""Project configuration with safe, non-secret defaults."""

from __future__ import annotations

import json
import os
import tempfile
from dataclasses import asdict, dataclass
from pathlib import Path

from .errors import ConfigurationError, NotInitializedError

CONFIG_DIR = ".vibemux"
CONFIG_FILE = "config.json"
DATABASE_FILE = "vibemux.sqlite3"
HARNESS_REGISTRY_FILE = "harnesses.json"
WORKTREE_DIR = "worktrees"


@dataclass
class Config:
    project_id: str
    repo_root: str
    terminal_backend: str = "auto"
    execution_backend: str = "native"
    managed_worktree_root: str | None = None
    default_harness: str = "mock"
    schema_version: int = 1

    def write(self, path: Path) -> None:
        payload = json.dumps(asdict(self), ensure_ascii=False, indent=2) + "\n"
        path.parent.mkdir(parents=True, exist_ok=True)
        staged_path: Path | None = None
        try:
            # Exclusive, same-directory staging keeps concurrent readers and writers
            # away from a partial live file and from one another's temporary files.
            with tempfile.NamedTemporaryFile(
                mode="w",
                encoding="utf-8",
                dir=path.parent,
                prefix=f".{path.name}.",
                suffix=".tmp",
                delete=False,
            ) as handle:
                staged_path = Path(handle.name)
                handle.write(payload)
                handle.flush()
                os.fsync(handle.fileno())
            staged_path.replace(path)
            staged_path = None
        finally:
            if staged_path is not None:
                staged_path.unlink(missing_ok=True)

    @classmethod
    def read(cls, path: Path) -> Config:
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
            return cls(**data)
        except (OSError, ValueError, TypeError) as exc:
            raise ConfigurationError(f"invalid config: {path}") from exc


def config_dir(repo_root: Path) -> Path:
    return repo_root / CONFIG_DIR


def config_path(repo_root: Path) -> Path:
    return config_dir(repo_root) / CONFIG_FILE


def database_path(repo_root: Path) -> Path:
    return config_dir(repo_root) / DATABASE_FILE


def harness_registry_path(repo_root: Path) -> Path:
    return config_dir(repo_root) / HARNESS_REGISTRY_FILE


def require_config(repo_root: Path) -> Config:
    path = config_path(repo_root)
    if not path.exists():
        raise NotInitializedError(f"VibeMux is not initialized in {repo_root}")
    return Config.read(path)
