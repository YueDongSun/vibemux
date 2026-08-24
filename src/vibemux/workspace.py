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


@dataclass(frozen=True)
class WorktreeInspection:
    registered: bool
    exists: bool
    branch_matches: bool
    head: str | None
    dirty: bool
    issues: tuple[str, ...]


@dataclass(frozen=True)
class CleanupPlan:
    record: WorktreeRecord
    inspection: WorktreeInspection

    @property
    def executable(self) -> bool:
        return not self.inspection.issues


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

    def inspect(self, record: WorktreeRecord) -> WorktreeInspection:
        ensure_managed_worktree(record.path, self.managed_root, self.repo_root)
        inventory = self._worktree_inventory()
        entry = inventory.get(str(record.path.resolve()))
        exists = record.path.exists()
        registered = entry is not None
        expected_branch = f"refs/heads/{record.branch}"
        branch_matches = entry is not None and entry[1] == expected_branch
        dirty = False
        issues: list[str] = []
        if not exists:
            issues.append("worktree_missing")
        if not registered:
            issues.append("worktree_unregistered")
        if registered and not branch_matches:
            issues.append("worktree_branch_mismatch")
        if exists and registered:
            dirty = bool(
                git_run(
                    record.path,
                    ["status", "--porcelain"],
                    runner=self.runner,
                ).stdout.strip()
            )
            if dirty:
                issues.append("worktree_dirty")
        return WorktreeInspection(
            registered,
            exists,
            branch_matches,
            entry[0] if entry else None,
            dirty,
            tuple(issues),
        )

    def plan_cleanup(self, record: WorktreeRecord) -> CleanupPlan:
        return CleanupPlan(record, self.inspect(record))

    def execute_cleanup(self, plan: CleanupPlan) -> None:
        if not plan.executable:
            raise ResourceSafetyError(
                f"cleanup plan is not executable: {','.join(plan.inspection.issues)}"
            )
        refreshed = self.plan_cleanup(plan.record)
        if refreshed != plan:
            raise ResourceSafetyError("cleanup plan changed before execution")
        result = git_run(
            self.repo_root,
            ["worktree", "remove", str(plan.record.path)],
            runner=self.runner,
            check=False,
        )
        if result.returncode != 0:
            raise WorktreeError(result.stderr.strip() or "git worktree remove failed")

    def cleanup(self, record: WorktreeRecord, *, dry_run: bool = True) -> CleanupPlan:
        plan = self.plan_cleanup(record)
        if not dry_run:
            self.execute_cleanup(plan)
        return plan

    def _worktree_inventory(self) -> dict[str, tuple[str | None, str | None]]:
        output = git_run(
            self.repo_root,
            ["worktree", "list", "--porcelain", "-z"],
            runner=self.runner,
        ).stdout
        inventory: dict[str, tuple[str | None, str | None]] = {}
        current_path: str | None = None
        current_head: str | None = None
        current_branch: str | None = None
        for field in output.split("\x00"):
            if not field:
                if current_path is not None:
                    inventory[str(Path(current_path).resolve())] = (
                        current_head,
                        current_branch,
                    )
                current_path = current_head = current_branch = None
            elif field.startswith("worktree "):
                current_path = field.removeprefix("worktree ")
            elif field.startswith("HEAD "):
                current_head = field.removeprefix("HEAD ")
            elif field.startswith("branch "):
                current_branch = field.removeprefix("branch ")
        if current_path is not None:
            inventory[str(Path(current_path).resolve())] = (current_head, current_branch)
        return inventory
