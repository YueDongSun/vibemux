"""Regressions for the seven confirmed Python safety and lifecycle defects.

Terminal commands are interpreted by injected runners, never a user's server.
Git operations are confined to repositories created by the ``repo`` fixture.
"""

from __future__ import annotations

import hashlib
import json
import os
import tempfile
from collections.abc import Iterator
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager
from dataclasses import replace
from pathlib import Path
from threading import Event as ThreadEvent
from threading import Lock

import pytest

from vibemux.command_runner import Command, CommandResult, CommandRunner, SubprocessCommandRunner
from vibemux.config import Config, require_config
from vibemux.errors import (
    CommandExecutionError,
    DirtyRepositoryError,
    ReconciliationError,
    ResourceSafetyError,
    StorageError,
    TerminalBackendError,
)
from vibemux.models import RunStatus, Task, TaskStatus, TerminalLocation
from vibemux.services import ProjectService, ReconciliationService, RunService, TaskService
from vibemux.terminal import Pane, SendReceipt, TmuxBackend
from vibemux.workspace import WorkspaceManager, ensure_clean_with_commit


def git(repo: Path, *args: str) -> str:
    result = SubprocessCommandRunner().run(Command("git", args, cwd=repo))
    assert result.returncode == 0, result.stderr
    return result.stdout.strip()


@pytest.fixture
def repo(tmp_path: Path) -> Path:
    path = tmp_path / "repo"
    path.mkdir()
    git(path, "init", "-b", "main")
    git(path, "config", "user.name", "VibeMux Test")
    git(path, "config", "user.email", "test@example.invalid")
    (path / "README.md").write_text("base\n", encoding="utf-8")
    (path / ".gitignore").write_text(".vibemux/\n*.ignored\n", encoding="utf-8")
    git(path, "add", "README.md", ".gitignore")
    git(path, "commit", "-m", "initial")
    return path


@contextmanager
def prepared_service(
    repo: Path, backend: str = "mock", runner: CommandRunner | None = None
) -> Iterator[tuple[RunService, Task]]:
    ProjectService(repo, runner).initialize(backend)
    task_service = TaskService(repo)
    try:
        task = task_service.create("private task title", "private task description")
    finally:
        task_service.storage.close()
    service = RunService(repo, runner)
    try:
        yield service, task
    finally:
        service.storage.close()


def tmux_args(command: Command) -> tuple[str, ...]:
    assert command.executable == "tmux"
    command.validate()
    return command.args[2:] if command.args[:1] == ("-L",) else command.args


class BufferRunner:
    """Model tmux's newest-buffer default and explicit named-buffer operations."""

    def __init__(
        self,
        *,
        interleave: bool = False,
        fail: str | None = None,
        raise_on_failure: bool = False,
        fail_cleanup: bool = False,
    ) -> None:
        self.calls: list[Command] = []
        self.buffers = {"foreign": b"unrelated operator buffer"}
        self.delivered: dict[str, bytes] = {}
        self.paths: list[Path] = []
        self.first_loaded = ThreadEvent()
        self.second_loaded = ThreadEvent()
        self.first_pasted = ThreadEvent()
        self.lock = Lock()
        self.interleave = interleave
        self.fail = fail
        self.raise_on_failure = raise_on_failure
        self.fail_cleanup = fail_cleanup

    def run(self, command: Command) -> CommandResult:
        args = tmux_args(command)
        operation = args[0]
        load_index = -1
        with self.lock:
            self.calls.append(command)
            if operation == "load-buffer":
                self.paths.append(Path(args[-1]))
                load_index = len(self.paths) - 1
            if operation == self.fail or (operation == "delete-buffer" and self.fail_cleanup):
                if self.raise_on_failure:
                    raise CommandExecutionError(f"synthetic {operation} failure")
                return CommandResult(1, stderr=f"synthetic {operation} failure")
            if operation == "load-buffer":
                name = args[args.index("-b") + 1] if "-b" in args else f"buffer{load_index}"
                payload = self.paths[-1].read_bytes()
                if payload:
                    self.buffers[name] = payload
            elif operation == "paste-buffer":
                name = args[args.index("-b") + 1] if "-b" in args else next(reversed(self.buffers))
                if name not in self.buffers:
                    return CommandResult(1, stderr=f"no buffer {name}")
                pane_id = args[args.index("-t") + 1]
                self.delivered[pane_id] = self.buffers[name]
                if "-d" in args:
                    del self.buffers[name]
            elif operation == "delete-buffer":
                # An unqualified cleanup would delete somebody else's newest buffer.
                assert "-b" in args
                self.buffers.pop(args[args.index("-b") + 1], None)
            else:
                assert operation == "send-keys"
        if self.interleave and operation == "load-buffer":
            if load_index == 0:
                self.first_loaded.set()
                assert self.second_loaded.wait(5), "second send did not load"
            else:
                self.second_loaded.set()
                assert self.first_pasted.wait(5), "first send did not paste"
        if self.interleave and operation == "paste-buffer" and pane_id == "%1":
            self.first_pasted.set()
        return CommandResult(0)


class InventoryRunner:
    """Run real fixture Git only; emulate terminal inventory and session allocation."""

    def __init__(self) -> None:
        self.calls: list[Command] = []
        self.panes: dict[str, Pane] = {}
        self.wezterm_panes: dict[str, Pane] = {}
        self.sessions: set[str] = set()
        self.next_pane = 40

    def run(self, command: Command) -> CommandResult:
        if command.executable == "git":
            return SubprocessCommandRunner().run(command)
        self.calls.append(command)
        command.validate()
        if command.executable == "tmux":
            args = tmux_args(command)
            if args[0] == "new-session":
                session = args[args.index("-s") + 1]
                if session in self.sessions:
                    return CommandResult(1, stderr="duplicate session")
                self.sessions.add(session)
                self.next_pane += 1
                pane_id = f"%{self.next_pane}"
                self.panes[pane_id] = Pane(
                    pane_id, args[args.index("-c") + 1], "fixture title", session
                )
                return CommandResult(0, f"{pane_id}\n")
            if args[0] == "list-panes":
                template = args[args.index("-F") + 1]
                rows = []
                for pane in self.panes.values():
                    row = template
                    for key, value in {
                        "pane_id": pane.pane_id,
                        "session_name": pane.workspace_id or "",
                        "pane_current_path": pane.cwd,
                        "pane_title": pane.title,
                        "pane_dead": "0" if pane.alive else "1",
                    }.items():
                        row = row.replace("#{" + key + "}", value)
                    rows.append(row)
                return CommandResult(0, "\n".join(rows) + "\n")
            if args[0] == "kill-pane":
                pane = self.panes.pop(args[args.index("-t") + 1])
                self.sessions.discard(pane.workspace_id or "")
                return CommandResult(0)
            assert args[0] in {"load-buffer", "paste-buffer", "delete-buffer", "send-keys"}
            return CommandResult(0)
        assert command.executable == "wezterm", "a real terminal must never be executed"
        args = command.args
        if args[:2] == ("cli", "spawn"):
            self.next_pane += 1
            pane_id = str(self.next_pane)
            self.wezterm_panes[pane_id] = Pane(
                pane_id,
                args[args.index("--cwd") + 1],
                "fixture title",
                args[args.index("--workspace") + 1],
            )
            return CommandResult(0, f"{pane_id}\n")
        if args[:2] == ("cli", "list"):
            return CommandResult(
                0,
                json.dumps(
                    [
                        {
                            "pane_id": pane.pane_id,
                            "cwd": pane.cwd,
                            "title": pane.title,
                            "workspace": pane.workspace_id,
                            "is_dead": not pane.alive,
                        }
                        for pane in self.wezterm_panes.values()
                    ]
                ),
            )
        if args[:2] == ("cli", "kill-pane"):
            del self.wezterm_panes[args[args.index("--pane-id") + 1]]
            return CommandResult(0)
        assert args[:2] == ("cli", "send-text")
        return CommandResult(0)


@pytest.mark.parametrize("after_planning", [False, True])
def test_cleanup_preserves_ignored_files(repo: Path, after_planning: bool) -> None:
    manager = WorkspaceManager(repo, repo / ".vibemux" / "worktrees")
    record = manager.create_worktree("run_cleanup", "vibemux/run_cleanup")
    plan = manager.plan_cleanup(record)
    assert plan.executable
    ignored = record.path / "private.ignored"
    ignored.write_text("must be preserved\n", encoding="utf-8")
    if not after_planning:
        plan = manager.plan_cleanup(record)
        assert plan.inspection.dirty
        assert plan.inspection.issues == ("worktree_dirty",)
        assert not plan.executable
    with pytest.raises(ResourceSafetyError):
        manager.execute_cleanup(plan)
    assert ignored.read_text(encoding="utf-8") == "must be preserved\n"
    assert record.path.is_dir()
    assert git(record.path, "branch", "--show-current") == record.branch
    assert git(repo, "branch", "--show-current") == "main"
    assert git(repo, "status", "--porcelain") == ""


@pytest.mark.parametrize("submit", [False, True])
def test_tmux_interleaved_sends_use_distinct_owned_buffers(submit: bool) -> None:
    runner = BufferRunner(interleave=True)
    first = TmuxBackend(socket_name="vibemux_test", runner=runner)
    second = TmuxBackend(socket_name="vibemux_test", runner=runner)
    texts = {"%1": "first α\n; not a shell command\n", "%2": "second 'quoted'\r\nmessage"}
    with ThreadPoolExecutor(max_workers=2) as executor:
        a = executor.submit(first.send_text, "%1", texts["%1"], submit=submit)
        assert runner.first_loaded.wait(5), "first send did not load"
        b = executor.submit(second.send_text, "%2", texts["%2"], submit=submit)
        receipts = {"%1": a.result(timeout=10), "%2": b.result(timeout=10)}
    assert runner.delivered == {pane: text.encode("utf-8") for pane, text in texts.items()}
    assert runner.buffers == {"foreign": b"unrelated operator buffer"}
    loads = [tmux_args(call) for call in runner.calls if tmux_args(call)[0] == "load-buffer"]
    assert all("-b" in args for args in loads)
    names = [args[args.index("-b") + 1] for args in loads]
    assert len(set(names)) == 2
    for pane, text in texts.items():
        assert receipts[pane] == SendReceipt(
            hashlib.sha256(text.encode("utf-8")).hexdigest(),
            len(text.encode("utf-8")),
            text.count("\n") + 1,
            submit,
        )
    assert all(not path.exists() for path in runner.paths)
    assert all(
        text not in argument
        for call in runner.calls
        for argument in call.args
        for text in texts.values()
    )
    enters = [tmux_args(call) for call in runner.calls if tmux_args(call)[0] == "send-keys"]
    assert len(enters) == (2 if submit else 0)


@pytest.mark.parametrize("text", ["", "multiline\nUnicode 界\n' ; $()\r\n"])
@pytest.mark.parametrize("submit", [False, True])
def test_tmux_send_keeps_exact_payload_and_receipt(text: str, submit: bool) -> None:
    runner = BufferRunner()
    backend = TmuxBackend(socket_name="vibemux_test", runner=runner)
    receipt = backend.send_text("%1", text, submit=submit)
    assert runner.delivered == ({"%1": text.encode("utf-8")} if text else {})
    assert receipt == SendReceipt(
        hashlib.sha256(text.encode("utf-8")).hexdigest(),
        len(text.encode("utf-8")),
        text.count("\n") + (1 if text else 0),
        submit,
    )
    assert runner.buffers == {"foreign": b"unrelated operator buffer"}
    assert all(not path.exists() for path in runner.paths)
    enters = [tmux_args(call) for call in runner.calls if tmux_args(call)[0] == "send-keys"]
    assert enters == ([("send-keys", "-t", "%1", "Enter")] if submit else [])
    if not text:
        assert runner.paths == []
        assert all(tmux_args(call)[0] == "send-keys" for call in runner.calls)


def test_empty_tmux_submit_failure_preserves_other_buffers() -> None:
    runner = BufferRunner(fail="send-keys")
    backend = TmuxBackend(socket_name="vibemux_test", runner=runner)
    with pytest.raises(TerminalBackendError, match="synthetic send-keys failure"):
        backend.send_text("%1", "", submit=True)
    assert runner.paths == []
    assert runner.buffers == {"foreign": b"unrelated operator buffer"}
    assert len(runner.calls) == 1
    assert tmux_args(runner.calls[0]) == ("send-keys", "-t", "%1", "Enter")


@pytest.mark.parametrize("operation", ["load-buffer", "paste-buffer", "send-keys"])
def test_tmux_send_failure_cleans_only_its_buffer(operation: str) -> None:
    runner = BufferRunner(fail=operation)
    backend = TmuxBackend(socket_name="vibemux_test", runner=runner)
    with pytest.raises(TerminalBackendError):
        backend.send_text("%1", "private input", submit=True)
    assert runner.buffers == {"foreign": b"unrelated operator buffer"}
    assert runner.paths and all(not path.exists() for path in runner.paths)
    buffer_commands = [
        tmux_args(call)
        for call in runner.calls
        if tmux_args(call)[0] in {"load-buffer", "paste-buffer", "delete-buffer"}
    ]
    assert all("-b" in args for args in buffer_commands)
    assert len({args[args.index("-b") + 1] for args in buffer_commands}) == 1


def test_tmux_cleanup_error_does_not_mask_paste_error_or_leak_tempfile() -> None:
    runner = BufferRunner(fail="paste-buffer", raise_on_failure=True, fail_cleanup=True)
    backend = TmuxBackend(socket_name="vibemux_test", runner=runner)
    with pytest.raises(TerminalBackendError, match="tmux") as caught:
        backend.send_text("%1", "private input")
    assert str(caught.value.__cause__) == "synthetic paste-buffer failure"
    assert runner.paths and all(not path.exists() for path in runner.paths)
    cleanup = [tmux_args(call) for call in runner.calls if tmux_args(call)[0] == "delete-buffer"]
    assert len(cleanup) == 1 and "-b" in cleanup[0]
    assert cleanup[0][cleanup[0].index("-b") + 1] != "foreign"
    assert runner.buffers["foreign"] == b"unrelated operator buffer"


@pytest.mark.parametrize(("system", "actual_backend"), [("Windows", "wezterm"), ("Linux", "tmux")])
@pytest.mark.parametrize("explicit_auto", [False, True])
def test_auto_run_persists_actual_backend_and_round_trips(
    repo: Path,
    monkeypatch: pytest.MonkeyPatch,
    system: str,
    actual_backend: str,
    explicit_auto: bool,
) -> None:
    runner = InventoryRunner()
    monkeypatch.setattr("vibemux.services.platform.system", lambda: system)
    configured = "mock" if explicit_auto else "auto"
    with prepared_service(repo, configured, runner) as (service, task):
        run = service.spawn(
            task.task_id, "mock", terminal_backend="auto" if explicit_auto else None
        )
        assert run.terminal_backend == actual_backend
        assert run.terminal is not None and run.terminal.backend == actual_backend
        assert service.get(run.run_id).terminal_backend == actual_backend
        assert require_config(repo).terminal_backend == configured
        # A subsequent process must use the stored backend, not resolve auto again.
        monkeypatch.setattr(
            "vibemux.services.platform.system",
            lambda: "Linux" if system == "Windows" else "Windows",
        )
        reopened = RunService(repo, runner)
        try:
            assert reopened.send(run.run_id, "fixture input", submit=True).submit
            result = ReconciliationService(reopened).reconcile(run.run_id)
            assert result.status_after == RunStatus.RUNNING and not result.issues
            assert reopened.stop(run.run_id).status == RunStatus.STOPPED
        finally:
            reopened.storage.close()


@pytest.mark.parametrize(("system", "actual_backend"), [("Windows", "wezterm"), ("Linux", "tmux")])
def test_legacy_auto_runs_use_recorded_placement_without_a_read_time_write(
    repo: Path,
    monkeypatch: pytest.MonkeyPatch,
    system: str,
    actual_backend: str,
) -> None:
    runner = InventoryRunner()
    monkeypatch.setattr("vibemux.services.platform.system", lambda: system)
    with prepared_service(repo, "auto", runner) as (service, task):
        run = service.spawn(task.task_id, "mock")
        with service.storage.connection:
            service.storage.connection.execute(
                "UPDATE runs SET terminal_backend='auto' WHERE run_id=?", (str(run.run_id),)
            )
        monkeypatch.setattr(
            "vibemux.services.platform.system",
            lambda: "Linux" if system == "Windows" else "Windows",
        )
        reopened = RunService(repo, runner)
        try:
            restored = reopened.get(run.run_id)
            assert restored.terminal_backend == actual_backend
            raw = reopened.storage.connection.execute(
                "SELECT terminal_backend FROM runs WHERE run_id=?", (str(run.run_id),)
            ).fetchone()
            assert raw[0] == "auto"
            assert reopened.send(run.run_id, "legacy input", submit=True).submit
            assert not ReconciliationService(reopened).reconcile(run.run_id).issues
            assert reopened.stop(run.run_id).status == RunStatus.STOPPED
        finally:
            reopened.storage.close()


@pytest.mark.parametrize("stored_backend", ["auto", "tmux"])
def test_legacy_backend_resolution_does_not_repair_unrecognized_placement(
    repo: Path, stored_backend: str
) -> None:
    with prepared_service(repo) as (service, task):
        run = service.spawn(task.task_id, "mock")
        assert run.terminal is not None
        run.terminal_backend = stored_backend
        run.terminal = replace(run.terminal, backend="unrecognized")
        service.storage.save_run(run)
        restored = service.get(run.run_id)
        assert restored.terminal_backend == stored_backend
        assert restored.terminal is not None and restored.terminal.backend == "unrecognized"


def test_auto_selection_is_resolved_for_each_run(
    repo: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runner = InventoryRunner()
    with prepared_service(repo, "auto", runner) as (service, task):
        monkeypatch.setattr("vibemux.services.platform.system", lambda: "Linux")
        first = service.spawn(task.task_id, "mock")
        monkeypatch.setattr("vibemux.services.platform.system", lambda: "Windows")
        second = service.spawn(task.task_id, "mock")
        assert [first.terminal_backend, second.terminal_backend] == ["tmux", "wezterm"]
        assert first.terminal is not None and first.terminal.backend == "tmux"
        assert second.terminal is not None and second.terminal.backend == "wezterm"
        assert require_config(repo).terminal_backend == "auto"


@pytest.mark.parametrize("alive", [False, True])
@pytest.mark.parametrize("title", ["fixture title", "title\twith a tab"])
def test_tmux_inventory_reports_actual_session_name(
    tmp_path: Path, alive: bool, title: str
) -> None:
    runner = InventoryRunner()
    pane = Pane("%42", str(tmp_path), title, "project_session", alive)
    runner.panes[pane.pane_id] = pane
    assert TmuxBackend(socket_name="vibemux_test", runner=runner).list() == [pane]
    assert "#{session_name}" in runner.calls[-1].args[-1]


@pytest.mark.parametrize("unknown_session_exists", [False, True])
def test_tmux_runs_allocate_separate_sessions_without_adopting_unknown_session(
    repo: Path, unknown_session_exists: bool
) -> None:
    runner = InventoryRunner()
    with prepared_service(repo, "tmux", runner) as (service, task):
        foreign_session = str(service.project_id)
        foreign_sessions = {foreign_session} if unknown_session_exists else set()
        runner.sessions.update(foreign_sessions)
        first = service.spawn(task.task_id, "mock")
        second = service.spawn(task.task_id, "mock")
        assert first.terminal is not None and second.terminal is not None
        sessions = {first.terminal.workspace_id, second.terminal.workspace_id}
        assert len(sessions) == 2 and foreign_session not in sessions
        assert runner.sessions == sessions | foreign_sessions
        for run in (first, second):
            assert run.terminal is not None and run.terminal.pane_id is not None
            assert runner.panes[run.terminal.pane_id].workspace_id == run.terminal.workspace_id
            assert not ReconciliationService(service).reconcile(run.run_id).issues
            assert service.stop(run.run_id).status == RunStatus.STOPPED
        assert runner.sessions == foreign_sessions
        assert sum(tmux_args(call)[0] == "new-session" for call in runner.calls) == 2
        assert not any(
            tmux_args(call)[0] in {"attach-session", "has-session", "kill-session"}
            for call in runner.calls
        )


def test_tmux_session_name_collision_fails_closed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from uuid import UUID

    runner = InventoryRunner()
    backend = TmuxBackend(socket_name="vibemux_test", runner=runner)
    monkeypatch.setattr("vibemux.terminal.uuid4", lambda: UUID(int=1))
    location = TerminalLocation("tmux", workspace_id="project")
    first = backend.open(location, ["fixture", "argument with spaces"], tmp_path)
    inventory = dict(runner.panes)
    with pytest.raises(TerminalBackendError, match="duplicate session"):
        backend.open(location, ["fixture", "argument with spaces"], tmp_path)
    assert runner.panes == inventory
    assert runner.sessions == {first.workspace_id}
    assert all(tmux_args(call)[0] == "new-session" for call in runner.calls)


def test_tmux_session_mismatch_still_blocks_send_and_stop(repo: Path) -> None:
    runner = InventoryRunner()
    with prepared_service(repo, "tmux", runner) as (service, task):
        run = service.spawn(task.task_id, "mock")
        assert run.terminal is not None and run.terminal.pane_id is not None
        pane_id = run.terminal.pane_id
        runner.panes[pane_id] = replace(runner.panes[pane_id], workspace_id="foreign_session")
        calls_before = len(runner.calls)
        with pytest.raises(ReconciliationError, match="terminal_workspace_mismatch"):
            service.send(run.run_id, "must not reach another session")
        with pytest.raises(ReconciliationError, match="terminal_workspace_mismatch"):
            service.stop(run.run_id)
        assert all(tmux_args(call)[0] == "list-panes" for call in runner.calls[calls_before:])
        result = ReconciliationService(service).reconcile(run.run_id)
        assert result.status_after == RunStatus.STALE
        assert result.issues == ("terminal_workspace_mismatch",)
        assert pane_id in runner.panes


def test_config_write_publishes_complete_exclusive_same_directory_stage(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "config.json"
    before = Config("project", str(tmp_path), default_harness="mock")
    after = replace(before, terminal_backend="tmux")
    before.write(path)
    foreign = path.with_suffix(".json.tmp")
    foreign.write_text("unrelated staging file", encoding="utf-8")
    original_open, original_replace = os.open, os.replace
    opened: dict[Path, int] = {}
    published: list[Path] = []

    def record_open(file: str, flags: int, mode: int = 0o777) -> int:
        opened[Path(file)] = flags
        return original_open(file, flags, mode)

    def observe_replace(source: str | os.PathLike[str], target: str | os.PathLike[str]) -> None:
        staged = Path(source)
        assert staged != path and staged.parent == path.parent
        assert Path(target) == path
        assert opened[staged] & os.O_EXCL and opened[staged] & os.O_CREAT
        assert Config.read(path) == before
        assert Config.read(staged) == after
        published.append(staged)
        original_replace(source, target)

    monkeypatch.setattr(os, "open", record_open)
    monkeypatch.setattr(os, "replace", observe_replace)
    after.write(path)
    assert len(published) == 1
    assert Config.read(path) == after
    assert not published[0].exists()
    assert foreign.read_text(encoding="utf-8") == "unrelated staging file"


def test_config_publication_releases_ownership_of_the_staging_name(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "config.json"
    config = Config("project", str(tmp_path))
    replace_file = os.replace
    reused_names: list[Path] = []

    def replace_then_reuse(source: str | os.PathLike[str], target: str | os.PathLike[str]) -> None:
        replace_file(source, target)
        reused = Path(source)
        reused.write_text("unrelated replacement", encoding="utf-8")
        reused_names.append(reused)

    monkeypatch.setattr(os, "replace", replace_then_reuse)
    config.write(path)
    assert Config.read(path) == config
    assert len(reused_names) == 1
    assert reused_names[0].read_text(encoding="utf-8") == "unrelated replacement"


@pytest.mark.parametrize("failure_at", ["flush", "replace"])
def test_config_write_failure_preserves_previous_complete_file(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, failure_at: str
) -> None:
    path = tmp_path / "config.json"
    before = Config("project", str(tmp_path))
    before.write(path)
    previous_bytes = path.read_bytes()
    after = replace(before, default_harness="other")

    def fail_flush(fd: int) -> None:
        assert Config.read(path) == before
        raise OSError("synthetic flush failure")

    def fail_replace(source: str | os.PathLike[str], target: str | os.PathLike[str]) -> None:
        assert Config.read(Path(source)) == after
        assert Config.read(path) == before
        raise OSError("synthetic replace failure")

    if failure_at == "flush":
        monkeypatch.setattr(os, "fsync", fail_flush)
    else:
        monkeypatch.setattr(os, "replace", fail_replace)
    with pytest.raises(OSError, match=f"synthetic {failure_at} failure"):
        after.write(path)
    assert path.read_bytes() == previous_bytes
    assert list(tmp_path.iterdir()) == [path]


def test_interleaved_config_writes_own_their_staging_files(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "config.json"
    before = Config("project", str(tmp_path))
    first = replace(before, terminal_backend="tmux")
    second = replace(before, terminal_backend="wezterm")
    before.write(path)
    original_replace = os.replace
    stages: list[Path] = []

    def interleave_replace(source: str | os.PathLike[str], target: str | os.PathLike[str]) -> None:
        staged = Path(source)
        stages.append(staged)
        assert Config.read(path) == before
        if len(stages) == 1:
            second.write(path)
            assert Config.read(path) == second
            assert Config.read(staged) == first
        original_replace(source, target)

    monkeypatch.setattr(os, "replace", interleave_replace)
    first.write(path)
    assert len(stages) == 2 and len(set(stages)) == 2
    assert all(not staged.exists() for staged in stages)
    assert Config.read(path) == first
    assert list(tmp_path.iterdir()) == [path]


def test_config_partial_staging_write_preserves_previous_file(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "config.json"
    before = Config("project", str(tmp_path))
    before.write(path)
    original_temporary_file = tempfile.NamedTemporaryFile

    @contextmanager
    def failing_temporary_file(
        *, mode: str, encoding: str, dir: Path, prefix: str, suffix: str, delete: bool
    ) -> Iterator[object]:
        with original_temporary_file(
            mode=mode, encoding=encoding, dir=dir, prefix=prefix, suffix=suffix, delete=delete
        ) as handle:

            class PartialWriter:
                name = handle.name

                def write(self, payload: str) -> int:
                    handle.write(payload[:8])
                    handle.flush()
                    assert Config.read(path) == before
                    raise OSError("synthetic partial write failure")

            yield PartialWriter()

    monkeypatch.setattr(tempfile, "NamedTemporaryFile", failing_temporary_file)
    with pytest.raises(OSError, match="synthetic partial write failure"):
        replace(before, default_harness="other").write(path)
    assert Config.read(path) == before
    assert list(tmp_path.iterdir()) == [path]


def test_dirty_spawn_keeps_open_task_and_audit_unchanged(repo: Path) -> None:
    with prepared_service(repo) as (service, task):
        before = service.trace()
        (repo / "README.md").write_text("uncommitted private content\n", encoding="utf-8")
        with pytest.raises(DirtyRepositoryError, match="repository must be clean"):
            service.spawn(task.task_id, "mock")
        assert service.storage.get_task(task.task_id) == task
        assert service.trace() == before
        assert service.storage.list_runs(service.project_id) == []
        assert not list(service.workspace.managed_root.iterdir())
        assert require_config(repo).terminal_backend == "mock"


def test_spawn_task_transition_is_audited_once_without_task_content(repo: Path) -> None:
    with prepared_service(repo) as (service, task):
        first = service.spawn(task.task_id, "mock")
        second = service.spawn(task.task_id, "mock")
        assert first.status == second.status == RunStatus.RUNNING
        reloaded = service.storage.get_task(task.task_id)
        assert reloaded is not None and reloaded.status == TaskStatus.IN_PROGRESS
        events = service.trace()
        started = [event for event in events if event.event_type == "task_started"]
        assert len(started) == 1
        event = started[0]
        assert event.task_id == task.task_id and event.project_id == service.project_id
        assert event.payload == {"from": "open", "to": "in_progress"}
        assert task.title not in json.dumps(event.payload)
        assert task.description not in json.dumps(event.payload)
        assert event.sequence is not None
        preparing = [item for item in events if item.event_type == "run_preparing"]
        assert all(
            item.sequence is not None and item.sequence > event.sequence for item in preparing
        )


def test_overlapping_spawns_audit_one_transition_and_preserve_fresh_task_data(
    repo: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    with prepared_service(repo) as (service, task):
        entered = False
        other_runs = []

        def interleaved_validation(root: Path, runner: CommandRunner | None = None) -> str:
            nonlocal entered
            base = ensure_clean_with_commit(root, runner)
            if not entered:
                entered = True
                other = RunService(repo)
                try:
                    fresh = other.storage.get_task(task.task_id)
                    assert fresh is not None
                    fresh.title = "updated while validating"
                    fresh.metadata = {"concurrent": "preserved"}
                    other.storage.save_task(fresh)
                    other_runs.append(other.spawn(task.task_id, "mock"))
                finally:
                    other.storage.close()
            return base

        monkeypatch.setattr("vibemux.services.ensure_clean_with_commit", interleaved_validation)
        first = service.spawn(task.task_id, "mock")
        assert first.status == RunStatus.RUNNING
        assert len(other_runs) == 1 and other_runs[0].status == RunStatus.RUNNING
        started = [event for event in service.trace() if event.event_type == "task_started"]
        assert len(started) == 1
        fresh = service.storage.get_task(task.task_id)
        assert fresh is not None and fresh.status == TaskStatus.IN_PROGRESS
        assert fresh.title == "updated while validating"
        assert fresh.metadata == {"concurrent": "preserved"}


def test_spawn_task_event_insertion_failure_rolls_back_transition(repo: Path) -> None:
    with prepared_service(repo) as (service, task):
        before = service.trace()
        service.storage.connection.execute(
            """
            CREATE TEMP TRIGGER fail_task_started BEFORE INSERT ON events
            WHEN NEW.event_type = 'task_started'
            BEGIN SELECT RAISE(ABORT, 'synthetic task audit failure'); END
            """
        )
        with pytest.raises(StorageError, match="synthetic task audit failure"):
            service.spawn(task.task_id, "mock")
        assert service.storage.get_task(task.task_id) == task
        assert service.trace() == before
        assert service.storage.list_runs(service.project_id) == []
        assert not list(service.workspace.managed_root.iterdir())
        assert not service.storage.connection.in_transaction
