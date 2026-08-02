"""Git repository and worktree lifecycle with fail-closed cleanup."""

from __future__ import annotations

import re
import subprocess
from dataclasses import dataclass
from pathlib import Path

from .errors import DirtyRepositoryError, GitRepositoryError, ResourceSafetyError, WorktreeError
from .paths import ensure_managed_worktree


def git_run(repo_root: Path, args: list[str], *, check: bool = True) -> subprocess.CompletedProcess[str]:
    try:
        result = subprocess.run(["git", *args], cwd=repo_root, text=True, capture_output=True, check=False)
    except OSError as exc:
        raise GitRepositoryError("git is not available") from exc
    if check and result.returncode != 0:
        raise GitRepositoryError(result.stderr.strip() or "git command failed")
    return result


def ensure_repository(repo_root: Path) -> None:
    if git_run(repo_root, ["rev-parse", "--show-toplevel"], check=False).returncode != 0:
        raise GitRepositoryError(f"not a git repository: {repo_root}")


def ensure_clean_with_commit(repo_root: Path) -> str:
    ensure_repository(repo_root)
    status = git_run(repo_root, ["status", "--porcelain"]).stdout.strip()
    if status:
        raise DirtyRepositoryError("repository must be clean")
    commit = git_run(repo_root, ["rev-parse", "HEAD"], check=False)
    if commit.returncode != 0:
        raise GitRepositoryError("repository has no initial commit")
    return commit.stdout.strip()


def sanitize_branch(value: str) -> str:
    if ".." in value or "\\" in value:
        raise WorktreeError("invalid branch name")
    candidate = re.sub(r"[^A-Za-z0-9._/-]+", "-", value).strip("./-")
    if not candidate or candidate.startswith("-") or ".." in candidate:
        raise WorktreeError("invalid branch name")
    return candidate[:120]


@dataclass(frozen=True)
class WorktreeRecord:
    path: Path
    branch: str
    base_commit: str


class WorkspaceManager:
    def __init__(self, repo_root: Path, managed_root: Path):
        self.repo_root = repo_root.resolve()
        self.managed_root = managed_root.resolve()
        self.managed_root.mkdir(parents=True, exist_ok=True)

    def create_worktree(self, run_id: str, branch: str, base_commit: str | None = None) -> WorktreeRecord:
        base = base_commit or ensure_clean_with_commit(self.repo_root)
        safe_branch = sanitize_branch(branch)
        path = self.managed_root / run_id
        if path.exists():
            raise WorktreeError(f"worktree already exists: {path}")
        result = git_run(self.repo_root, ["worktree", "add", "-b", safe_branch, str(path), base], check=False)
        if result.returncode != 0:
            raise WorktreeError(result.stderr.strip() or "git worktree add failed")
        return WorktreeRecord(path, safe_branch, base)

    def diff(self, record: WorktreeRecord) -> str:
        return git_run(record.path, ["diff", "--no-ext-diff", "--binary"]).stdout

    def cleanup(self, record: WorktreeRecord, *, dry_run: bool = True) -> None:
        ensure_managed_worktree(record.path, self.managed_root, self.repo_root)
        status = git_run(record.path, ["status", "--porcelain"]).stdout.strip()
        if status:
            raise ResourceSafetyError("refusing to remove dirty worktree")
        if dry_run:
            return
        result = git_run(self.repo_root, ["worktree", "remove", str(record.path)], check=False)
        if result.returncode != 0:
            raise WorktreeError(result.stderr.strip() or "git worktree remove failed")
