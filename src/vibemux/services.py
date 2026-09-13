"""Application services coordinating domain, storage, workspace and terminals."""

from __future__ import annotations

import platform
from dataclasses import dataclass
from pathlib import Path
from urllib.parse import unquote, urlparse
from uuid import UUID

from .command_runner import CommandRunner, SubprocessCommandRunner
from .config import (
    WORKTREE_DIR,
    Config,
    config_dir,
    config_path,
    database_path,
    harness_registry_path,
    require_config,
)
from .errors import (
    HarnessNotDetectedError,
    InvalidRoleError,
    NotInitializedError,
    ReconciliationError,
    TerminalBackendError,
    VibeMuxError,
)
from .harness import (
    DEFAULT_PROFILES,
    HarnessProfile,
    LaunchContext,
    adapter_for,
    profile_for,
)
from .models import (
    Event,
    Project,
    Run,
    RunRole,
    RunStatus,
    Task,
    TaskStatus,
    TerminalLocation,
    utc_now,
)
from .paths import is_within, normalized
from .registry import HarnessRegistry, HarnessState, record_role
from .storage import Storage
from .terminal import MockTerminalBackend, SendReceipt, TerminalBackend, TmuxBackend, WezTermBackend
from .workspace import WorkspaceManager, WorktreeRecord, ensure_clean_with_commit

MOCK_TERMINAL_STATE_FILE = "mock_terminal.json"


@dataclass(frozen=True)
class ReconciliationResult:
    run_id: UUID
    status_before: RunStatus
    status_after: RunStatus
    issues: tuple[str, ...]


def choose_terminal_backend(
    name: str,
    runner: CommandRunner | None = None,
    mock_state_path: Path | None = None,
) -> TerminalBackend:
    if name == "mock":
        return MockTerminalBackend(mock_state_path)
    if name == "wezterm":
        return WezTermBackend(runner=runner)
    if name == "tmux":
        return TmuxBackend(runner=runner)
    if platform.system() == "Windows":
        return WezTermBackend(runner=runner)
    return TmuxBackend(runner=runner)


class ProjectService:
    def __init__(self, repo_root: Path, runner: CommandRunner | None = None):
        self.repo_root = repo_root.resolve()
        self.runner = runner or SubprocessCommandRunner()

    def initialize(self, terminal_backend: str = "auto") -> Config:
        existing = config_path(self.repo_root)
        if existing.exists():
            return Config.read(existing)
        ensure_clean_with_commit(self.repo_root, self.runner)
        project = Project(str(self.repo_root), terminal_backend=terminal_backend)
        config = Config(
            str(project.project_id),
            str(self.repo_root),
            terminal_backend,
            "native",
            str(config_dir(self.repo_root) / WORKTREE_DIR),
        )
        config.write(existing)
        storage = Storage(database_path(self.repo_root))
        storage.save_project(project)
        storage.append_event(
            Event(
                "project_initialized",
                project.project_id,
                payload={"terminal_backend": terminal_backend},
            )
        )
        storage.close()
        return config


class TaskService:
    def __init__(self, repo_root: Path):
        self.repo_root = repo_root.resolve()
        self.config = require_config(self.repo_root)
        self.storage = Storage(database_path(self.repo_root))
        self.project_id = UUID(self.config.project_id)

    def create(self, title: str, description: str = "") -> Task:
        task = Task(title, self.project_id, description)
        self.storage.save_task(
            task, Event("task_created", self.project_id, task.task_id, payload={"title": title})
        )
        return task

    def list(self) -> list[Task]:
        return self.storage.list_tasks(self.project_id)


class HarnessService:
    """Probes harnesses, persists the detection snapshot, and owns the project default."""

    def __init__(self, repo_root: Path):
        self.repo_root = repo_root.resolve()
        self.config = require_config(self.repo_root)
        self.storage = Storage(database_path(self.repo_root))
        self.project_id = UUID(self.config.project_id)
        self.registry_path = harness_registry_path(self.repo_root)

    def _row(
        self,
        name: str,
        profile: HarnessProfile,
        available: bool,
        path: str | None,
        roles: tuple[str, ...],
    ) -> dict[str, object]:
        return {
            "name": name,
            "command": list(profile.command),
            "protocol": profile.protocol.value,
            "provider": profile.provider,
            "available": available,
            "path": path,
            "roles": list(roles),
            "default": name == self.config.default_harness,
        }

    def refresh(self) -> list[dict[str, object]]:
        previous = HarnessRegistry.read(self.registry_path)
        states: dict[str, HarnessState] = {}
        rows: list[dict[str, object]] = []
        for name, profile in DEFAULT_PROFILES.items():
            capabilities = adapter_for(profile).probe(profile)
            roles = previous.harnesses.get(name, HarnessState(False)).roles
            states[name] = HarnessState(capabilities.available, capabilities.path, roles)
            rows.append(self._row(name, profile, capabilities.available, capabilities.path, roles))
        HarnessRegistry(utc_now().isoformat(), states).write(self.registry_path)
        self.storage.append_event(
            Event(
                "harness_probed",
                self.project_id,
                payload={
                    "detected": sorted(name for name, state in states.items() if state.detected),
                    "missing": sorted(name for name, state in states.items() if not state.detected),
                },
            )
        )
        return rows

    def cached(self) -> list[dict[str, object]]:
        registry = HarnessRegistry.read(self.registry_path)
        rows: list[dict[str, object]] = []
        for name, profile in DEFAULT_PROFILES.items():
            state = registry.harnesses.get(name, HarnessState(False))
            rows.append(self._row(name, profile, state.detected, state.path, state.roles))
        return rows

    def switch(self, harness_name: str) -> Config:
        profile = profile_for(harness_name)
        capabilities = adapter_for(profile).probe(profile)
        if not capabilities.available:
            raise HarnessNotDetectedError(
                f"harness not detected on this machine: {harness_name} (see 'vibemux harnesses')"
            )
        previous = self.config.default_harness
        self.config.default_harness = harness_name
        self.config.write(config_path(self.repo_root))
        self.storage.append_event(
            Event(
                "harness_switched",
                self.project_id,
                payload={"from": previous, "to": harness_name},
            )
        )
        return self.config


class RunService:
    def __init__(self, repo_root: Path, runner: CommandRunner | None = None):
        self.repo_root = repo_root.resolve()
        self.runner = runner or SubprocessCommandRunner()
        self.config = require_config(self.repo_root)
        self.storage = Storage(database_path(self.repo_root))
        self.project_id = UUID(self.config.project_id)
        self.workspace = WorkspaceManager(
            self.repo_root,
            Path(
                self.config.managed_worktree_root
                or self.repo_root / config_dir(self.repo_root) / WORKTREE_DIR
            ),
            self.runner,
        )
        self.backends: dict[str, TerminalBackend] = {}

    def _backend(self, name: str) -> TerminalBackend:
        return self.backends.setdefault(
            name,
            choose_terminal_backend(
                name,
                self.runner,
                config_dir(self.repo_root) / MOCK_TERMINAL_STATE_FILE,
            ),
        )

    def spawn(
        self,
        task_id: UUID,
        harness_name: str | None = None,
        *,
        role: str = "worker",
        terminal_backend: str | None = None,
    ) -> Run:
        task = self.storage.get_task(task_id)
        if task is None:
            raise NotInitializedError(f"task not found: {task_id}")
        try:
            safe_role = RunRole(role)
        except ValueError as exc:
            raise InvalidRoleError(
                f"unknown run role: {role} (expected one of {', '.join(item.value for item in RunRole)})"
            ) from exc
        if task.status == TaskStatus.OPEN:
            task.transition(TaskStatus.IN_PROGRESS)
            self.storage.save_task(task)
        backend_name = terminal_backend or self.config.terminal_backend
        harness_name = harness_name or self.config.default_harness
        base_commit = ensure_clean_with_commit(self.repo_root, self.runner)
        run = Run(
            task_id,
            self.project_id,
            harness_name,
            role=safe_role,
            terminal_backend=backend_name,
            execution_backend=self.config.execution_backend,
            base_commit=base_commit,
        )
        self.storage.save_run(
            run,
            Event(
                "run_preparing",
                self.project_id,
                task_id,
                run.run_id,
                payload={"harness": harness_name, "role": safe_role.value},
            ),
        )
        worktree: WorktreeRecord | None = None
        location: TerminalLocation | None = None
        backend = self._backend(backend_name)
        try:
            profile = profile_for(harness_name)
            adapter = adapter_for(profile)
            capabilities = adapter.probe(profile)
            if not capabilities.available:
                raise HarnessNotDetectedError(
                    f"harness not detected on this machine: {harness_name} (see 'vibemux harnesses')"
                )
            worktree = self.workspace.create_worktree(
                run.run_id.hex[:12], f"vibemux/{run.run_id.hex[:12]}", base_commit
            )
            run.worktree, run.branch = str(worktree.path), worktree.branch
            launch = adapter.build_launch_spec(
                profile, LaunchContext(worktree.path, str(run.run_id))
            )
            location = backend.open(
                TerminalLocation(
                    backend_name,
                    workspace_id=str(self.project_id),
                    metadata={"run_id": str(run.run_id)},
                ),
                launch.as_host_command(),
                worktree.path,
            )
            run.terminal = location
            run.transition(RunStatus.RUNNING)
            self.storage.save_run(
                run,
                Event(
                    "run_started",
                    self.project_id,
                    task_id,
                    run.run_id,
                    payload={
                        "worktree": run.worktree,
                        "branch": run.branch,
                        "pane_id": location.pane_id,
                        "role": safe_role.value,
                    },
                ),
            )
        except Exception:
            compensation: list[str] = []
            if location is not None:
                try:
                    backend.stop(location)
                    compensation.append("terminal_stopped")
                except VibeMuxError:
                    compensation.append("terminal_preserved")
            if worktree is not None:
                try:
                    plan = self.workspace.plan_cleanup(worktree)
                    if plan.executable:
                        self.workspace.execute_cleanup(plan)
                        compensation.append("worktree_removed")
                    else:
                        compensation.append("worktree_preserved")
                except VibeMuxError:
                    compensation.append("worktree_preserved")
            if run.status in {RunStatus.PREPARING, RunStatus.RUNNING}:
                run.transition(RunStatus.FAILED)
            run.metadata["failure_code"] = "spawn_failed"
            run.metadata["compensation"] = compensation
            self.storage.save_run(
                run,
                Event(
                    "run_failed",
                    self.project_id,
                    task_id,
                    run.run_id,
                    payload={
                        "failure_code": "spawn_failed",
                        "compensation": compensation,
                    },
                ),
            )
            raise
        record_role(
            harness_registry_path(self.repo_root),
            harness_name,
            capabilities.available,
            capabilities.path,
            safe_role.value,
        )
        return run

    def get(self, run_id: UUID) -> Run:
        run = self.storage.get_run(run_id)
        if run is None:
            raise NotInitializedError(f"run not found: {run_id}")
        return run

    def send(self, run_id: UUID, text: str, submit: bool = False) -> SendReceipt:
        run = self.get(run_id)
        self._require_terminal_resource(run)
        location = run.terminal
        if location is None or location.pane_id is None:
            raise ReconciliationError("run has no terminal pane")
        receipt = self._backend(run.terminal_backend).send_text(
            location.pane_id, text, submit=submit
        )
        self.storage.append_event(
            Event(
                "message_sent",
                self.project_id,
                run.task_id,
                run.run_id,
                payload={
                    "sha256": receipt.digest,
                    "bytes": receipt.byte_count,
                    "lines": receipt.line_count,
                    "submit": receipt.submit,
                },
            )
        )
        return receipt

    def stop(self, run_id: UUID) -> Run:
        run = self.get(run_id)
        if run.status not in {RunStatus.RUNNING, RunStatus.STALE}:
            return run
        self._require_terminal_resource(run)
        if run.terminal:
            self._backend(run.terminal_backend).stop(run.terminal)
        if run.status in {RunStatus.RUNNING, RunStatus.STALE}:
            run.transition(RunStatus.STOPPED)
            self.storage.save_run(
                run, Event("run_stopped", self.project_id, run.task_id, run.run_id)
            )
        return run

    def diff(self, run_id: UUID) -> str:
        run = self.get(run_id)
        if not run.worktree or not run.branch:
            return ""
        if not run.base_commit:
            raise ReconciliationError("run has no persisted base commit")
        return self.workspace.diff(WorktreeRecord(Path(run.worktree), run.branch, run.base_commit))

    def trace(self) -> list[Event]:
        return self.storage.list_events(self.project_id)

    def _require_terminal_resource(self, run: Run) -> None:
        issues = self._terminal_resource_issues(run)
        if issues:
            raise ReconciliationError(f"terminal resource mismatch: {','.join(issues)}")

    def _terminal_resource_issues(self, run: Run) -> tuple[str, ...]:
        issues: list[str] = []
        if run.project_id != self.project_id:
            issues.append("project_mismatch")
        location = run.terminal
        if location is None or not location.pane_id:
            return (*issues, "terminal_missing")
        if location.backend != run.terminal_backend:
            issues.append("terminal_backend_mismatch")
        if location.metadata.get("run_id") != str(run.run_id):
            issues.append("terminal_run_mismatch")
        try:
            panes = self._backend(run.terminal_backend).list()
        except TerminalBackendError:
            return (*issues, "terminal_inventory_unavailable")
        pane = next((item for item in panes if item.pane_id == location.pane_id), None)
        if pane is None:
            return (*issues, "terminal_missing")
        if not pane.alive:
            issues.append("terminal_not_alive")
        if location.workspace_id and pane.workspace_id != location.workspace_id:
            issues.append("terminal_workspace_mismatch")
        expected_cwd = run.worktree or location.cwd
        if expected_cwd is None or not _terminal_cwd_matches(pane.cwd, Path(expected_cwd)):
            issues.append("terminal_cwd_mismatch")
        return tuple(issues)


class ReconciliationService:
    def __init__(self, run_service: RunService):
        self.run_service = run_service

    def reconcile(self, run_id: UUID) -> ReconciliationResult:
        run = self.run_service.get(run_id)
        status_before = run.status
        issues = list(self.run_service._terminal_resource_issues(run))
        if not run.worktree or not run.branch or not run.base_commit:
            issues.append("worktree_record_incomplete")
        else:
            try:
                inspection = self.run_service.workspace.inspect(
                    WorktreeRecord(Path(run.worktree), run.branch, run.base_commit)
                )
                issues.extend(issue for issue in inspection.issues if issue != "worktree_dirty")
            except VibeMuxError:
                issues.append("worktree_unsafe")
        unique_issues = tuple(dict.fromkeys(issues))
        if unique_issues and run.status == RunStatus.RUNNING:
            run.transition(RunStatus.STALE)
            self.run_service.storage.save_run(
                run,
                Event(
                    "run_stale",
                    self.run_service.project_id,
                    run.task_id,
                    run.run_id,
                    payload={"issues": list(unique_issues)},
                ),
            )
        return ReconciliationResult(run.run_id, status_before, run.status, unique_issues)


def _terminal_cwd_matches(inventory_cwd: str, expected_cwd: Path) -> bool:
    if not inventory_cwd:
        return False
    parsed = urlparse(inventory_cwd)
    candidate_text = inventory_cwd
    if parsed.scheme == "file":
        path_text = unquote(parsed.path)
        if platform.system() == "Windows" and len(path_text) >= 3 and path_text[0] == "/":
            path_text = path_text[1:]
        candidate_text = f"//{parsed.netloc}{path_text}" if parsed.netloc else path_text
    try:
        candidate = Path(candidate_text)
        return normalized(candidate) == normalized(expected_cwd) or is_within(
            candidate, expected_cwd
        )
    except (OSError, ValueError):
        return False
