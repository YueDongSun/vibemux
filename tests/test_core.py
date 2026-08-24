from pathlib import Path
from uuid import uuid4

import pytest

from vibemux.errors import InvalidStateTransitionError
from vibemux.models import (
    Event,
    Project,
    Run,
    RunCompletionAuthority,
    RunStatus,
    Task,
    TaskStatus,
)
from vibemux.paths import is_within
from vibemux.storage import Storage
from vibemux.terminal import MockTerminalBackend, WezTermBackend
from vibemux.workspace import sanitize_branch


def test_state_transitions_are_explicit() -> None:
    task = Task("demo", uuid4())
    task.transition(TaskStatus.IN_PROGRESS)
    task.transition(TaskStatus.DONE)
    with pytest.raises(InvalidStateTransitionError):
        task.transition(TaskStatus.OPEN)


def test_run_state_transition() -> None:
    run = Run(uuid4(), uuid4(), "mock")
    run.transition(RunStatus.RUNNING)
    run.transition(RunStatus.STOPPED)
    with pytest.raises(InvalidStateTransitionError):
        run.transition(RunStatus.RUNNING)


def test_run_success_requires_explicit_authority() -> None:
    run = Run(uuid4(), uuid4(), "mock")
    run.transition(RunStatus.RUNNING)
    with pytest.raises(InvalidStateTransitionError):
        run.transition(RunStatus.SUCCEEDED)
    run.transition(
        RunStatus.SUCCEEDED,
        completion_authority=RunCompletionAuthority.STRUCTURED_ADAPTER,
    )
    assert run.status == RunStatus.SUCCEEDED


def test_storage_event_order_and_roundtrip(tmp_path: Path) -> None:
    storage = Storage(tmp_path / "db.sqlite3")
    project = Project(str(tmp_path))
    storage.save_project(project)
    task = Task("demo", project.project_id)
    storage.save_task(task, Event("task_created", project.project_id, task.task_id))
    events = storage.list_events(project.project_id)
    stored_task = storage.get_task(task.task_id)
    assert events[0].event_type == "task_created"
    assert events[0].sequence == 1
    assert stored_task is not None
    assert stored_task.title == "demo"
    storage.close()


def test_mock_terminal_never_executes_text(tmp_path: Path) -> None:
    backend = MockTerminalBackend()
    location = backend.open(
        __import__("vibemux.models", fromlist=["TerminalLocation"]).TerminalLocation("mock"),
        ["echo", "ignored"],
        tmp_path,
    )
    assert location.pane_id is not None
    receipt = backend.send_text(location.pane_id, "hello; touch pwned", submit=True)
    assert receipt.byte_count > 0
    assert not (tmp_path / "pwned").exists()


def test_path_boundary_and_branch_safety(tmp_path: Path) -> None:
    root = tmp_path / "worktrees"
    assert is_within(root / "run_a", root)
    assert not is_within(tmp_path / "worktrees-evil", root)
    assert sanitize_branch("vibemux/run 1") == "vibemux/run-1"
    from vibemux.errors import WorktreeError

    with pytest.raises(WorktreeError):
        sanitize_branch("../main")


def test_wezterm_command_contract(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    backend = WezTermBackend("wezterm")
    calls: list[tuple[list[str], str | None]] = []

    class Result:
        returncode = 0
        stdout = "42\n"
        stderr = ""

    def record_call(args: list[str], input_text: str | None = None) -> Result:
        calls.append((args, input_text))
        return Result()

    monkeypatch.setattr(backend, "_run", record_call)
    from vibemux.models import TerminalLocation

    backend.open(
        TerminalLocation("wezterm", workspace_id="project_1"),
        ["python", "-m", "x"],
        tmp_path,
    )
    backend.send_text("42", "& calc.exe", submit=True)
    assert calls[0][0][:3] == ["cli", "spawn", "--cwd"]
    assert calls[0][0][4:7] == ["--new-window", "--workspace", "project_1"]
    assert calls[0][0][-4:] == ["--", "python", "-m", "x"]
    assert calls[1][0][-1] == "42"
    assert calls[1][1] == "& calc.exe"
