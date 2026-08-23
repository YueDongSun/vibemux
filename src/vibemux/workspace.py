"""Git repository and worktree lifecycle with fail-closed cleanup."""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

from .command_runner import Command, CommandResult, CommandRunner, SubprocessCommandRunner
from .errors import (
    CommandExecutionError,
    DirtyRepositoryError,
    GitRepositoryError,
    ResourceSafetyError,
    WorktreeError,
)
from .paths import ensure_managed_worktree

GIT_COMMAND_TIMEOUT_SECONDS = 120.0


def git_run(
    repo_root: Path,
    args: list[str],
    *,
    runner: CommandRunner | None = None,
    check: bool = True,
) -> CommandResult:
    try:
        result = (runner or SubprocessCommandRunner()).run(
            Command("git", tuple(args), cwd=repo_root, timeout_seconds=GIT_COMMAND_TIMEOUT_SECONDS)
        )
    except CommandExecutionError as exc:
        raise GitRepositoryError("git is not available") from exc
    if check and result.returncode != 0:
        raise GitRepositoryError(result.stderr.strip() or "git command failed")
    return result


def ensure_repository(repo_root: Path, runner: CommandRunner | None = None) -> None:
    if (
        git_run(repo_root, ["rev-parse", "--show-toplevel"], runner=runner, check=False).returncode
        != 0
    ):
        raise GitRepositoryError(f"not a git repository: {repo_root}")


def ensure_clean_with_commit(repo_root: Path, runner: CommandRunner | None = None) -> str:
    ensure_repository(repo_root, runner)
    status_lines = git_run(repo_root, ["status", "--porcelain"], runner=runner).stdout.splitlines()
    relevant = [
        line for line in status_lines if not line[3:].replace("\\", "/").startswith(".vibemux/")
    ]
    if relevant:
        raise DirtyRepositoryError("repository must be clean")
    commit = git_run(repo_root, ["rev-parse", "HEAD"], runner=runner, check=False)
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
    def __init__(
        self,
        repo_root: Path,
        managed_root: Path,
        runner: CommandRunner | None = None,
    ):
        self.repo_root = repo_root.resolve()
        self.managed_root = managed_root.resolve()
        self.runner = runner or SubprocessCommandRunner()
        self.managed_root.mkdir(parents=True, exist_ok=True)

    def create_worktree(
        self, run_id: str, branch: str, base_commit: str | None = None
    ) -> WorktreeRecord:
        base = base_commit or ensure_clean_with_commit(self.repo_root, self.runner)
        safe_branch = sanitize_branch(branch)
        path = self.managed_root / run_id
        if path.exists():
            raise WorktreeError(f"worktree already exists: {path}")
        result = git_run(
            self.repo_root,
            ["worktree", "add", "-b", safe_branch, str(path), base],
            runner=self.runner,
            check=False,
        )
        if result.returncode != 0:
            raise WorktreeError(result.stderr.strip() or "git worktree add failed")
        return WorktreeRecord(path, safe_branch, base)

    def diff(self, record: WorktreeRecord) -> str:
        ensure_managed_worktree(record.path, self.managed_root, self.repo_root)
        base_check = git_run(
            record.path,
            ["cat-file", "-e", f"{record.base_commit}^{{commit}}"],
            runner=self.runner,
            check=False,
        )
        if base_check.returncode != 0:
            raise WorktreeError("base commit is unavailable")
        tracked = git_run(
            record.path,
            ["diff", "--no-ext-diff", "--binary", record.base_commit, "--"],
            runner=self.runner,
        ).stdout
        untracked_output = git_run(
            record.path,
            ["ls-files", "--others", "--exclude-standard", "-z"],
            runner=self.runner,
        ).stdout
        patches = [tracked]
        for relative_path in filter(None, untracked_output.split("\x00")):
            untracked = git_run(
                record.path,
                ["diff", "--no-index", "--binary", "--", "/dev/null", relative_path],
                runner=self.runner,
                check=False,
            )
            if untracked.returncode not in {0, 1}:
                raise WorktreeError(untracked.stderr.strip() or "untracked diff failed")
            patches.append(untracked.stdout)
        return "".join(patches)

    def cleanup(self, record: WorktreeRecord, *, dry_run: bool = True) -> None:
        ensure_managed_worktree(record.path, self.managed_root, self.repo_root)
        status = git_run(record.path, ["status", "--porcelain"], runner=self.runner).stdout.strip()
        if status:
            raise ResourceSafetyError("refusing to remove dirty worktree")
        if dry_run:
            return
        result = git_run(
            self.repo_root,
            ["worktree", "remove", str(record.path)],
            runner=self.runner,
            check=False,
        )
        if result.returncode != 0:
            raise WorktreeError(result.stderr.strip() or "git worktree remove failed")
