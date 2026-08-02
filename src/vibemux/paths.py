"""Fail-closed cross-platform path safety helpers."""

from __future__ import annotations

import os
from pathlib import Path

from .errors import ResourceSafetyError


def normalized(path: Path) -> Path:
    return Path(os.path.normcase(os.path.abspath(os.fspath(path))))


def is_within(path: Path, root: Path, *, resolve: bool = False) -> bool:
    candidate = path.resolve() if resolve else normalized(path)
    boundary = root.resolve() if resolve else normalized(root)
    try:
        common = os.path.commonpath([os.fspath(candidate), os.fspath(boundary)])
    except ValueError:
        return False
    return common == os.fspath(boundary)


def ensure_within(path: Path, root: Path, *, resolve: bool = False) -> Path:
    if not is_within(path, root, resolve=resolve):
        raise ResourceSafetyError(f"path escapes managed root: {path}")
    return path


def ensure_managed_worktree(path: Path, managed_root: Path, repo_root: Path) -> Path:
    if not is_within(path, managed_root, resolve=False):
        raise ResourceSafetyError("worktree is outside managed root")
    if normalized(path) == normalized(repo_root):
        raise ResourceSafetyError("main repository cannot be cleaned")
    if path.exists() and path.is_symlink():
        raise ResourceSafetyError("symlink/reparse worktree is refused")
    try:
        ensure_within(path, managed_root, resolve=True)
    except FileNotFoundError:
        ensure_within(path.parent, managed_root, resolve=True)
    return path
