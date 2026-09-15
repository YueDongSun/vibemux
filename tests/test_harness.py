import subprocess
from pathlib import Path

import pytest

from vibemux.config import harness_registry_path, require_config
from vibemux.errors import HarnessNotDetectedError, HarnessNotFoundError, InvalidRoleError
from vibemux.harness import (
    DEFAULT_PROFILES,
    GenericCommandAdapter,
    MockHarnessAdapter,
    adapter_for,
    profile_for,
)
from vibemux.models import RunStatus
from vibemux.registry import HarnessRegistry
from vibemux.services import HarnessService, ProjectService, RunService, TaskService

DOMESTIC_PROFILES = {
    "qwen": "alibaba",
    "iflow": "alibaba",
    "trae": "bytedance",
    "codebuddy": "tencent",
    "kimi": "moonshot",
}


def make_repo(tmp_path: Path) -> Path:
    def git(*args: str) -> None:
        subprocess.run(["git", *args], cwd=tmp_path, check=True, capture_output=True, text=True)

    git("init", "-b", "main")
    git("config", "user.name", "VibeMux Test")
    git("config", "user.email", "test@example.invalid")
    (tmp_path / "README.md").write_text("test\n", encoding="utf-8")
    git("add", "README.md")
    git("commit", "-m", "initial")
    return tmp_path


def fake_which(monkeypatch: pytest.MonkeyPatch, found: set[str]) -> None:
    monkeypatch.setattr(
        "vibemux.harness.shutil.which",
        lambda name: f"C:/fake/{name}.exe" if name in found else None,
    )


def test_all_default_profiles_are_safe_argv() -> None:
    adapter = GenericCommandAdapter()
    for profile in DEFAULT_PROFILES.values():
        adapter.validate_profile(profile)


def test_domestic_profiles_registered_with_provider() -> None:
    for name, provider in DOMESTIC_PROFILES.items():
        profile = profile_for(name)
        assert profile.command == (name,)
        assert profile.provider == provider
        assert profile.protocol.value == "pty"


def test_adapter_for_mock_and_generic() -> None:
    assert isinstance(adapter_for(profile_for("mock")), MockHarnessAdapter)
    assert isinstance(adapter_for(profile_for("qwen")), GenericCommandAdapter)


def test_refresh_persists_snapshot_and_event(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    make_repo(tmp_path)
    ProjectService(tmp_path).initialize("mock")
    fake_which(monkeypatch, {"qwen"})

    rows = HarnessService(tmp_path).refresh()

    by_name = {str(row["name"]): row for row in rows}
    assert by_name["mock"]["available"] is True
    assert by_name["mock"]["default"] is True
    assert by_name["qwen"]["available"] is True
    assert by_name["iflow"]["available"] is False
    registry = HarnessRegistry.read(harness_registry_path(tmp_path))
    assert registry.checked_at != ""
    assert registry.harnesses["qwen"].detected is True
    assert registry.harnesses["qwen"].path == "C:/fake/qwen.exe"
    assert registry.harnesses["iflow"].detected is False
    cached = {str(row["name"]): row["available"] for row in HarnessService(tmp_path).cached()}
    assert cached["qwen"] is True
    service = HarnessService(tmp_path)
    probed = [
        event
        for event in service.storage.list_events(service.project_id)
        if event.event_type == "harness_probed"
    ]
    assert (
        probed
        and "qwen" in probed[-1].payload["detected"]
        and "iflow" in probed[-1].payload["missing"]
    )


def test_switch_rejects_undetected_harness(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    make_repo(tmp_path)
    ProjectService(tmp_path).initialize("mock")
    fake_which(monkeypatch, set())

    service = HarnessService(tmp_path)
    with pytest.raises(HarnessNotDetectedError):
        service.switch("qwen")

    assert require_config(tmp_path).default_harness == "mock"
    assert not [
        event
        for event in service.storage.list_events(service.project_id)
        if event.event_type == "harness_switched"
    ]
    with pytest.raises(HarnessNotFoundError):
        HarnessService(tmp_path).switch("does-not-exist")


def test_switch_persists_default_and_logs_event(tmp_path: Path) -> None:
    make_repo(tmp_path)
    ProjectService(tmp_path).initialize("mock")
    service = HarnessService(tmp_path)
    service.switch("mock")

    assert require_config(tmp_path).default_harness == "mock"
    events = service.storage.list_events(service.project_id)
    switched = [event for event in events if event.event_type == "harness_switched"]
    assert switched and switched[-1].payload == {"from": "mock", "to": "mock"}
    assert any(
        row["name"] == "mock" and row["default"] for row in HarnessService(tmp_path).cached()
    )


def test_spawn_rejects_undetected_harness_before_worktree(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    make_repo(tmp_path)
    ProjectService(tmp_path).initialize("mock")
    fake_which(monkeypatch, set())
    task = TaskService(tmp_path).create("undetected harness")

    run_service = RunService(tmp_path)
    with pytest.raises(HarnessNotDetectedError):
        run_service.spawn(task.task_id, "qwen", terminal_backend="mock")

    # Detection gates before any state change: no run row, no FAILED record,
    # the task stays OPEN, and no worktree is created.
    runs = run_service.storage.list_runs(run_service.project_id)
    assert runs == []
    reloaded = run_service.storage.get_task(task.task_id)
    assert reloaded is not None
    assert reloaded.status.value == "open"
    assert not list((tmp_path / ".vibemux" / "worktrees").glob("*"))


def test_spawn_survives_role_recording_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import vibemux.services
    from vibemux.errors import ConfigurationError

    make_repo(tmp_path)
    ProjectService(tmp_path).initialize("mock")
    task = TaskService(tmp_path).create("role recording outage")

    def broken_record_role(*args: object, **kwargs: object) -> None:
        raise ConfigurationError("registry payload corrupt")

    monkeypatch.setattr(vibemux.services, "record_role", broken_record_role)
    run_service = RunService(tmp_path)
    run = run_service.spawn(task.task_id, "mock", role="reviewer", terminal_backend="mock")

    # The spawn itself is healthy; the bookkeeping failure is audited instead
    # of tearing down the live run, worktree, and pane.
    assert run.status == RunStatus.RUNNING
    assert run.role.value == "reviewer"
    events = run_service.storage.list_events(run_service.project_id)
    audit = [event for event in events if event.event_type == "harness_role_record_failed"]
    assert len(audit) == 1
    assert audit[0].payload["harness"] == "mock"
    assert "corrupt" in audit[0].payload["error"]


def test_spawn_records_role_in_registry_and_run(tmp_path: Path) -> None:
    make_repo(tmp_path)
    ProjectService(tmp_path).initialize("mock")
    task = TaskService(tmp_path).create("role persistence")

    run_service = RunService(tmp_path)
    run = run_service.spawn(task.task_id, "mock", role="reviewer", terminal_backend="mock")

    assert run.role.value == "reviewer"
    assert run_service.get(run.run_id).role.value == "reviewer"
    registry = HarnessRegistry.read(harness_registry_path(tmp_path))
    assert "reviewer" in registry.harnesses["mock"].roles
    HarnessService(tmp_path).refresh()
    registry = HarnessRegistry.read(harness_registry_path(tmp_path))
    assert "reviewer" in registry.harnesses["mock"].roles
    mock_rows = [row for row in HarnessService(tmp_path).cached() if row["name"] == "mock"]
    assert mock_rows and "reviewer" in mock_rows[0]["roles"]


def test_spawn_rejects_unknown_role(tmp_path: Path) -> None:
    make_repo(tmp_path)
    ProjectService(tmp_path).initialize("mock")
    task = TaskService(tmp_path).create("bad role")

    with pytest.raises(InvalidRoleError):
        RunService(tmp_path).spawn(task.task_id, "mock", role="bogus", terminal_backend="mock")


def test_registry_rejects_corrupt_payloads(tmp_path: Path) -> None:
    from vibemux.errors import ConfigurationError
    from vibemux.registry import HarnessRegistry

    snapshot = tmp_path / "harnesses.json"
    for corrupt in ["not json", "{", '["unexpected"]', '{"harnesses": {"qwen": 7}}']:
        snapshot.write_text(corrupt, encoding="utf-8")
        with pytest.raises(ConfigurationError):
            HarnessRegistry.read(snapshot)

    snapshot.write_text('{"harnesses": {"qwen": {"detected": true}}}', encoding="utf-8")
    registry = HarnessRegistry.read(snapshot)
    assert registry.harnesses["qwen"].detected is True
    assert registry.harnesses["qwen"].roles == ()


def test_cached_snapshot_reports_nothing_before_first_refresh(tmp_path: Path) -> None:
    make_repo(tmp_path)
    ProjectService(tmp_path).initialize("mock")

    rows = HarnessService(tmp_path).cached()

    assert rows, "cached view still lists every registered harness"
    assert all(row["available"] is False for row in rows)
    assert all(row["roles"] == [] for row in rows)
    assert not tmp_path.joinpath(".vibemux", "harnesses.json").exists()


def test_spawn_falls_back_to_project_default_harness(tmp_path: Path) -> None:
    make_repo(tmp_path)
    ProjectService(tmp_path).initialize("mock")
    HarnessService(tmp_path).switch("mock")
    task = TaskService(tmp_path).create("default harness")

    run = RunService(tmp_path).spawn(task.task_id, terminal_backend="mock")

    assert run.harness == "mock"
    assert run.status.value == "running"
