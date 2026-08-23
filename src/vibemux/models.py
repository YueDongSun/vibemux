"""Platform-neutral domain models and state transitions."""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import UTC, datetime
from enum import StrEnum
from typing import Any
from uuid import UUID, uuid4

from .errors import InvalidStateTransitionError


def utc_now() -> datetime:
    return datetime.now(UTC)


class TaskStatus(StrEnum):
    OPEN = "open"
    IN_PROGRESS = "in_progress"
    BLOCKED = "blocked"
    DONE = "done"
    CANCELLED = "cancelled"


class RunStatus(StrEnum):
    PREPARING = "preparing"
    RUNNING = "running"
    STOPPED = "stopped"
    FAILED = "failed"
    SUCCEEDED = "succeeded"
    STALE = "stale"


class RunCompletionAuthority(StrEnum):
    STRUCTURED_ADAPTER = "structured_adapter"
    VERIFIER = "verifier"
    USER = "user"


TASK_TRANSITIONS: dict[TaskStatus, set[TaskStatus]] = {
    TaskStatus.OPEN: {TaskStatus.IN_PROGRESS, TaskStatus.CANCELLED},
    TaskStatus.IN_PROGRESS: {TaskStatus.BLOCKED, TaskStatus.DONE, TaskStatus.CANCELLED},
    TaskStatus.BLOCKED: {TaskStatus.IN_PROGRESS, TaskStatus.CANCELLED},
    TaskStatus.DONE: set(),
    TaskStatus.CANCELLED: set(),
}
RUN_TRANSITIONS: dict[RunStatus, set[RunStatus]] = {
    RunStatus.PREPARING: {RunStatus.RUNNING, RunStatus.FAILED},
    RunStatus.RUNNING: {RunStatus.STOPPED, RunStatus.FAILED, RunStatus.SUCCEEDED, RunStatus.STALE},
    RunStatus.STALE: {RunStatus.STOPPED, RunStatus.FAILED},
    RunStatus.STOPPED: set(),
    RunStatus.FAILED: set(),
    RunStatus.SUCCEEDED: set(),
}


@dataclass
class TerminalLocation:
    backend: str
    resource_id: str | None = None
    workspace_id: str | None = None
    tab_id: str | None = None
    pane_id: str | None = None
    cwd: str | None = None
    metadata: dict[str, str] = field(default_factory=dict)


@dataclass
class Project:
    root: str
    project_id: UUID = field(default_factory=uuid4)
    created_at: datetime = field(default_factory=utc_now)
    execution_backend: str = "native"
    terminal_backend: str = "auto"


@dataclass
class Task:
    title: str
    project_id: UUID
    description: str = ""
    task_id: UUID = field(default_factory=uuid4)
    status: TaskStatus = TaskStatus.OPEN
    created_at: datetime = field(default_factory=utc_now)
    metadata: dict[str, Any] = field(default_factory=dict)

    def transition(self, target: TaskStatus) -> None:
        if target not in TASK_TRANSITIONS[self.status]:
            raise InvalidStateTransitionError(f"task {self.status} -> {target} is not allowed")
        self.status = target


@dataclass
class Run:
    task_id: UUID
    project_id: UUID
    harness: str
    role: str = "worker"
    protocol: str = "mock"
    execution_backend: str = "native"
    terminal_backend: str = "auto"
    run_id: UUID = field(default_factory=uuid4)
    status: RunStatus = RunStatus.PREPARING
    branch: str | None = None
    worktree: str | None = None
    base_commit: str | None = None
    terminal: TerminalLocation | None = None
    created_at: datetime = field(default_factory=utc_now)
    metadata: dict[str, Any] = field(default_factory=dict)

    def transition(
        self,
        target: RunStatus,
        *,
        completion_authority: RunCompletionAuthority | None = None,
    ) -> None:
        if target not in RUN_TRANSITIONS[self.status]:
            raise InvalidStateTransitionError(f"run {self.status} -> {target} is not allowed")
        if target == RunStatus.SUCCEEDED and completion_authority is None:
            raise InvalidStateTransitionError("run success requires explicit completion authority")
        self.status = target


@dataclass(frozen=True)
class Event:
    event_type: str
    project_id: UUID
    task_id: UUID | None = None
    run_id: UUID | None = None
    payload: dict[str, Any] = field(default_factory=dict)
    event_id: UUID = field(default_factory=uuid4)
    sequence: int | None = None
    timestamp: datetime = field(default_factory=utc_now)
    actor: str = "system"
