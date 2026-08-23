"""Application services coordinating domain, storage, workspace and terminals."""

from __future__ import annotations

import platform
from pathlib import Path
from uuid import UUID

from .command_runner import CommandRunner, SubprocessCommandRunner
from .config import WORKTREE_DIR, Config, config_dir, config_path, database_path, require_config
from .errors import NotInitializedError, ReconciliationError
from .harness import LaunchContext, MockHarnessAdapter, profile_for
from .models import Event, Project, Run, RunStatus, Task, TaskStatus, TerminalLocation
from .storage import Storage
from .terminal import MockTerminalBackend, TerminalBackend, TmuxBackend, WezTermBackend
from .workspace import WorkspaceManager, ensure_clean_with_commit


def choose_terminal_backend(
    name: str,
    runner: CommandRunner | None = None,
) -> TerminalBackend:
    if name == "mock":
        return MockTerminalBackend()
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
        return self.backends.setdefault(name, choose_terminal_backend(name, self.runner))

    def spawn(
        self,
        task_id: UUID,
        harness_name: str = "mock",
        *,
        role: str = "worker",
        terminal_backend: str | None = None,
    ) -> Run:
        task = self.storage.get_task(task_id)
        if task is None:
            raise NotInitializedError(f"task not found: {task_id}")
        if task.status == TaskStatus.OPEN:
            task.transition(TaskStatus.IN_PROGRESS)
            self.storage.save_task(task)
        backend_name = terminal_backend or self.config.terminal_backend
        base_commit = ensure_clean_with_commit(self.repo_root, self.runner)
        run = Run(
            task_id,
            self.project_id,
            harness_name,
            role=role,
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
                payload={"harness": harness_name},
            ),
        )
        try:
            worktree = self.workspace.create_worktree(
                run.run_id.hex[:12], f"vibemux/{run.run_id.hex[:12]}", base_commit
            )
            run.worktree, run.branch = str(worktree.path), worktree.branch
            profile = profile_for(harness_name)
            adapter = (
                MockHarnessAdapter()
                if harness_name == "mock"
                else __import__(
                    "vibemux.harness", fromlist=["GenericCommandAdapter"]
                ).GenericCommandAdapter()
            )
            launch = adapter.build_launch_spec(
                profile, LaunchContext(worktree.path, str(run.run_id))
            )
            location = self._backend(backend_name).open(
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
                    },
                ),
            )
            return run
        except Exception as exc:
            run.transition(RunStatus.FAILED)
            run.metadata["error"] = str(exc)
            self.storage.save_run(
                run,
                Event(
                    "run_failed", self.project_id, task_id, run.run_id, payload={"error": str(exc)}
                ),
            )
            raise

    def get(self, run_id: UUID) -> Run:
        run = self.storage.get_run(run_id)
        if run is None:
            raise NotInitializedError(f"run not found: {run_id}")
        return run

    def send(self, run_id: UUID, text: str, submit: bool = False) -> object:
        run = self.get(run_id)
        if not run.terminal or not run.terminal.pane_id:
            raise ReconciliationError("run has no terminal pane")
        receipt = self._backend(run.terminal_backend).send_text(
            run.terminal.pane_id, text, submit=submit
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
        from .workspace import WorktreeRecord

        return self.workspace.diff(WorktreeRecord(Path(run.worktree), run.branch, run.base_commit))

    def trace(self) -> list[Event]:
        return self.storage.list_events(self.project_id)
