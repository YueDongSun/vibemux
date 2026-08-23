from __future__ import annotations

import json
import sqlite3
import sys
from pathlib import Path
from uuid import uuid4

import pytest

from vibemux.agent_host import main as agent_host_main
from vibemux.command_runner import Command, CommandResult, SubprocessCommandRunner
from vibemux.harness import (
    POWERSHELL_SCRIPT_ARGS,
    GenericCommandAdapter,
    HarnessProfile,
    LaunchContext,
    LaunchSpec,
)
from vibemux.models import Project, Run, Task, TerminalLocation
from vibemux.services import ProjectService, RunService, TaskService
from vibemux.storage import CURRENT_SCHEMA_VERSION, Storage
from vibemux.terminal import TmuxBackend
from vibemux.workspace import WorkspaceManager


class RecordingRunner:
    def __init__(self, results: list[CommandResult] | None = None) -> None:
        self.calls: list[Command] = []
        self.results = list(results or [CommandResult(0)])

    def run(self, command: Command) -> CommandResult:
        self.calls.append(command)
        return self.results.pop(0) if self.results else CommandResult(0)


def run_git(runner: SubprocessCommandRunner, repo: Path, *args: str) -> str:
    result = runner.run(Command("git", tuple(args), cwd=repo))
    assert result.returncode == 0, result.stderr
    return result.stdout.strip()


def test_command_runner_keeps_argv_and_stdin_out_of_shell(tmp_path: Path) -> None:
    runner = SubprocessCommandRunner()
    payload = "; touch pwned"
    script = "import json,sys; print(json.dumps([sys.argv[1], sys.stdin.read()]))"
    result = runner.run(
        Command(
            sys.executable,
            ("-c", script, payload),
            cwd=tmp_path,
            input_text=payload,
        )
    )
    assert result.returncode == 0
    assert json.loads(result.stdout) == [payload, payload]
    assert not (tmp_path / "pwned").exists()


def test_agent_host_forwards_validated_argv_without_shell(tmp_path: Path) -> None:
    runner = RecordingRunner([CommandResult(7)])
    exit_code = agent_host_main(
        [
            "--cwd",
            str(tmp_path),
            "--environment-json",
            '{"VIBEMUX_RUN_ID":"run_1"}',
            "--",
            "example_tool",
            "argument with spaces",
            ";not_a_command",
        ],
        runner=runner,
    )
    assert exit_code == 7
    assert runner.calls[0].executable == "example_tool"
    assert runner.calls[0].args == ("argument with spaces", ";not_a_command")
    assert runner.calls[0].environment == {"VIBEMUX_RUN_ID": "run_1"}
    assert runner.calls[0].capture_output is False


def test_launch_spec_wraps_target_in_controlled_agent_host(tmp_path: Path) -> None:
    launch = LaunchSpec(
        "example_tool",
        ("argument with spaces", ";not_a_command"),
        tmp_path,
        {"VIBEMUX_RUN_ID": "run_1"},
    )
    command = launch.as_host_command()
    separator = command.index("--")
    assert command[1:3] == ["-m", "vibemux.agent_host"]
    assert command[separator + 1 :] == [
        "example_tool",
        "argument with spaces",
        ";not_a_command",
    ]


def test_windows_cmd_uses_controlled_powershell_companion(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    command = tmp_path / "tool.cmd"
    companion = tmp_path / "tool.ps1"
    command.write_text("untrusted shim data\n", encoding="utf-8")
    companion.write_text("param([string] $Value)\n", encoding="utf-8")
    monkeypatch.setattr("vibemux.harness.platform.system", lambda: "Windows")
    monkeypatch.setattr(
        "vibemux.harness.shutil.which",
        lambda name: "powershell.exe" if name == "powershell" else str(command),
    )
    launch = GenericCommandAdapter().build_launch_spec(
        HarnessProfile("tool", (str(command), ";not_a_command")),
        LaunchContext(tmp_path, "run_1"),
    )
    assert launch.executable == "powershell.exe"
    assert launch.args == (*POWERSHELL_SCRIPT_ARGS, str(companion), ";not_a_command")


def test_missing_harness_probe_reports_unavailable(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("vibemux.harness.platform.system", lambda: "Windows")
    monkeypatch.setattr("vibemux.harness.os.get_exec_path", lambda: [])
    monkeypatch.setattr("vibemux.harness.shutil.which", lambda _name: None)
    capabilities = GenericCommandAdapter().probe(HarnessProfile("missing", ("missing",)))
    assert capabilities.available is False
    assert capabilities.path is None


def test_tmux_launch_keeps_target_as_multiple_arguments(tmp_path: Path) -> None:
    runner = RecordingRunner([CommandResult(0, "%42\n")])
    backend = TmuxBackend("tmux", "vibemux_test", runner)
    location = backend.open(
        TerminalLocation("tmux", workspace_id="project_1"),
        ["python", "-m", "vibemux.agent_host", "--", "tool", "argument with spaces"],
        tmp_path,
    )
    command = runner.calls[0]
    separator = command.args.index("--")
    assert command.args[separator + 1 :] == (
        "python",
        "-m",
        "vibemux.agent_host",
        "--",
        "tool",
        "argument with spaces",
    )
    assert location.pane_id == "%42"


def test_storage_migrates_and_round_trips_base_commit(tmp_path: Path) -> None:
    database = tmp_path / "legacy.sqlite3"
    connection = sqlite3.connect(database)
    connection.execute(
        """
        CREATE TABLE runs (
            run_id TEXT PRIMARY KEY, task_id TEXT NOT NULL, project_id TEXT NOT NULL,
            harness TEXT NOT NULL, role TEXT NOT NULL, protocol TEXT NOT NULL,
            execution_backend TEXT NOT NULL, terminal_backend TEXT NOT NULL,
            status TEXT NOT NULL, branch TEXT, worktree TEXT, terminal TEXT,
            created_at TEXT NOT NULL, metadata TEXT NOT NULL
        )
        """
    )
    connection.commit()
    connection.close()

    storage = Storage(database)
    columns = {str(row["name"]) for row in storage.connection.execute("PRAGMA table_info(runs)")}
    versions = [
        int(row["version"])
        for row in storage.connection.execute(
            "SELECT version FROM schema_migrations ORDER BY version"
        )
    ]
    assert "base_commit" in columns
    assert versions == list(range(1, CURRENT_SCHEMA_VERSION + 1))

    project = Project(str(tmp_path))
    task = Task("migration", project.project_id)
    run = Run(task.task_id, project.project_id, "mock", base_commit="a" * 40)
    storage.save_project(project)
    storage.save_task(task)
    storage.save_run(run)
    stored_run = storage.get_run(run.run_id)
    assert stored_run is not None
    assert stored_run.base_commit == "a" * 40
    storage.close()


def test_real_git_diff_covers_committed_staged_unstaged_and_untracked(tmp_path: Path) -> None:
    runner = SubprocessCommandRunner()
    repo = tmp_path / "repo"
    repo.mkdir()
    run_git(runner, repo, "init", "-b", "main")
    run_git(runner, repo, "config", "user.name", "VibeMux Test")
    run_git(runner, repo, "config", "user.email", "test@example.invalid")
    (repo / "tracked.txt").write_text("base\n", encoding="utf-8")
    run_git(runner, repo, "add", "tracked.txt")
    run_git(runner, repo, "commit", "-m", "initial")

    manager = WorkspaceManager(repo, repo / ".vibemux" / "worktrees", runner)
    record = manager.create_worktree("run_1", "vibemux/run_1")
    (record.path / "committed.txt").write_text("committed\n", encoding="utf-8")
    run_git(runner, record.path, "add", "committed.txt")
    run_git(runner, record.path, "commit", "-m", "agent commit")
    (record.path / "tracked.txt").write_text("unstaged\n", encoding="utf-8")
    (record.path / "staged.txt").write_text("staged\n", encoding="utf-8")
    run_git(runner, record.path, "add", "staged.txt")
    (record.path / "untracked file.txt").write_text("untracked\n", encoding="utf-8")

    patch = manager.diff(record)
    assert record.base_commit == run_git(runner, repo, "rev-parse", "HEAD")
    assert "committed.txt" in patch
    assert "tracked.txt" in patch
    assert "staged.txt" in patch
    assert "untracked file.txt" in patch


def test_spawn_persists_the_real_base_commit(tmp_path: Path) -> None:
    runner = SubprocessCommandRunner()
    repo = tmp_path / "service_repo"
    repo.mkdir()
    run_git(runner, repo, "init", "-b", "main")
    run_git(runner, repo, "config", "user.name", "VibeMux Test")
    run_git(runner, repo, "config", "user.email", "test@example.invalid")
    (repo / "README.md").write_text("base\n", encoding="utf-8")
    run_git(runner, repo, "add", "README.md")
    run_git(runner, repo, "commit", "-m", "initial")
    expected_base = run_git(runner, repo, "rev-parse", "HEAD")

    ProjectService(repo, runner).initialize("mock")
    task_service = TaskService(repo)
    task = task_service.create("base commit")
    task_service.storage.close()
    run_service = RunService(repo, runner)
    run = run_service.spawn(task.task_id, "mock", terminal_backend="mock")
    stored_run = run_service.storage.get_run(run.run_id)
    assert run.base_commit == expected_base
    assert stored_run is not None
    assert stored_run.base_commit == expected_base
    run_service.storage.close()


def test_legacy_run_preserves_missing_base_commit(tmp_path: Path) -> None:
    storage = Storage(tmp_path / "state.sqlite3")
    project = Project(str(tmp_path))
    task = Task("legacy", project.project_id)
    run = Run(task.task_id, project.project_id, "mock", run_id=uuid4())
    storage.save_project(project)
    storage.save_task(task)
    storage.save_run(run)
    stored_run = storage.get_run(run.run_id)
    assert stored_run is not None
    assert stored_run.base_commit is None
    storage.close()
