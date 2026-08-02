"""Project configuration with safe, non-secret defaults."""

from __future__ import annotations

import json
from dataclasses import asdict, dataclass
from pathlib import Path

from .errors import ConfigurationError, NotInitializedError

CONFIG_DIR = ".vibemux"
CONFIG_FILE = "config.json"
DATABASE_FILE = "vibemux.sqlite3"
WORKTREE_DIR = "worktrees"


@dataclass
class Config:
    project_id: str
    repo_root: str
    terminal_backend: str = "auto"
    execution_backend: str = "native"
    managed_worktree_root: str | None = None
    schema_version: int = 1

    def write(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(asdict(self), ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

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


def require_config(repo_root: Path) -> Config:
    path = config_path(repo_root)
    if not path.exists():
        raise NotInitializedError(f"VibeMux is not initialized in {repo_root}")
    return Config.read(path)
