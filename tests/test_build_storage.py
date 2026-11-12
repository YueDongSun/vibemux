"""Tests for scripts/build_storage.py managed build storage."""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

from scripts import build_storage


def _layout(tmp_path: Path) -> build_storage.Layout:
    layout = build_storage.Layout.for_root(tmp_path / "state")
    layout.ensure()
    return layout


def _measurements(layout: build_storage.Layout) -> list[dict[str, object]]:
    return [
        json.loads(path.read_text(encoding="utf-8"))
        for path in sorted(layout.measurements.glob("*.json"))
    ]


def test_inventory_reports_unknown_slot_without_claiming_it(tmp_path: Path) -> None:
    layout = _layout(tmp_path)
    unowned = layout.target_dir("stray")
    unowned.mkdir(parents=True)
    (unowned / "artifact").write_bytes(b"abc")

    report = build_storage.inventory(layout, repo_root=tmp_path, deep=False)

    assert report["unknown_slots"] == ["stray"]
    assert report["slots"][0]["registered"] is False
    assert report["slots"][0]["owner"] is None
    assert report["slots"][0]["target"]["logical_bytes"] == 3


def test_run_refuses_non_empty_unowned_slot(tmp_path: Path) -> None:
    layout = _layout(tmp_path)
    target = layout.target_dir("core")
    target.mkdir(parents=True)
    (target / "other-task").write_bytes(b"x")

    with pytest.raises(build_storage.StorageError) as error:
        build_storage.run_managed(
            layout,
            slot="core",
            task_id="task-a",
            command=[sys.executable, "-c", "pass"],
            cwd=tmp_path,
            budget_bytes=1024,
            reserve_bytes=0,
            max_active=1,
            lease_timeout=0.1,
            poll_seconds=0.05,
            allow_incremental=False,
            adopt_unknown=False,
            retained=[],
            note="",
        )

    assert error.value.reason == "slot_unowned"


def test_run_propagates_exit_code_and_records_measurement(tmp_path: Path) -> None:
    layout = _layout(tmp_path)
    code, measurement = build_storage.run_managed(
        layout,
        slot="core",
        task_id="task-a",
        command=[sys.executable, "-c", "raise SystemExit(7)"],
        cwd=tmp_path,
        budget_bytes=64 * 1024**2,
        reserve_bytes=0,
        max_active=1,
        lease_timeout=0.1,
        poll_seconds=0.05,
        allow_incremental=False,
        adopt_unknown=False,
        retained=[],
        note="",
    )

    assert code == 7
    assert measurement["status"] == "failed"
    assert measurement["exit_code"] == 7
    assert measurement["target_dir"] == str(layout.target_dir("core"))
    stored = _measurements(layout)
    assert len(stored) == 1
    assert stored[0]["target_dir"] == str(layout.target_dir("core"))
    assert json.loads(layout.slot_dir("core").joinpath("owner.json").read_text())["tasks"] == [
        "task-a"
    ]


def test_run_rejects_target_dir_override(tmp_path: Path) -> None:
    layout = _layout(tmp_path)
    with pytest.raises(build_storage.StorageError) as error:
        build_storage.run_managed(
            layout,
            slot="core",
            task_id="task-a",
            command=["cargo", "build", "--target-dir", str(tmp_path / "elsewhere")],
            cwd=tmp_path,
            budget_bytes=1024,
            reserve_bytes=0,
            max_active=1,
            lease_timeout=0.1,
            poll_seconds=0.05,
            allow_incremental=False,
            adopt_unknown=False,
            retained=[],
            note="",
        )
    assert error.value.reason == "target_dir_override"


def test_run_accepts_matching_target_dir_override(tmp_path: Path) -> None:
    layout = _layout(tmp_path)
    code, measurement = build_storage.run_managed(
        layout,
        slot="core",
        task_id="task-a",
        command=[
            sys.executable,
            "-c",
            "import os; print(os.environ['CARGO_TARGET_DIR'])",
            "--target-dir",
            str(layout.target_dir("core")),
        ],
        cwd=tmp_path,
        budget_bytes=64 * 1024**2,
        reserve_bytes=0,
        max_active=1,
        lease_timeout=0.1,
        poll_seconds=0.05,
        allow_incremental=False,
        adopt_unknown=False,
        retained=[],
        note="",
    )
    assert code == 0
    assert measurement["status"] == "succeeded"


def test_budget_exhaustion_stops_owned_tree(tmp_path: Path) -> None:
    layout = _layout(tmp_path)
    script = (
        "import os, pathlib, time\n"
        "target = pathlib.Path(os.environ['CARGO_TARGET_DIR'])\n"
        "target.mkdir(parents=True, exist_ok=True)\n"
        "(target / 'blob').write_bytes(b'x' * (4 * 1024 * 1024))\n"
        "time.sleep(30)\n"
    )
    code, measurement = build_storage.run_managed(
        layout,
        slot="core",
        task_id="task-a",
        command=[sys.executable, "-c", script],
        cwd=tmp_path,
        budget_bytes=1024 * 1024,
        reserve_bytes=0,
        max_active=1,
        lease_timeout=0.1,
        poll_seconds=0.02,
        allow_incremental=False,
        adopt_unknown=False,
        retained=[],
        note="",
    )

    assert code == build_storage.EXIT_RESOURCE_EXHAUSTED
    assert measurement["status"] == "resource_exhausted"
    assert measurement["stop_reason"] == "budget_exceeded"
    assert measurement["after"]["logical_bytes"] >= 4 * 1024 * 1024


def test_active_slot_lease_refuses_concurrent_run(tmp_path: Path) -> None:
    layout = _layout(tmp_path)
    build_storage._ensure_owner(layout, "core", "task-a", adopt_unknown=False, retained=[], note="")
    lease = build_storage._slot_lease(layout, "core")
    assert lease.try_acquire()
    try:
        with pytest.raises(build_storage.StorageError) as error:
            build_storage.run_managed(
                layout,
                slot="core",
                task_id="task-a",
                command=[sys.executable, "-c", "pass"],
                cwd=tmp_path,
                budget_bytes=1024,
                reserve_bytes=0,
                max_active=1,
                lease_timeout=0.05,
                poll_seconds=0.05,
                allow_incremental=False,
                adopt_unknown=False,
                retained=[],
                note="",
            )
    finally:
        lease.release()
    assert error.value.reason == "slot_busy"


def test_reserve_floor_refuses_start(tmp_path: Path) -> None:
    layout = _layout(tmp_path)
    free = build_storage._free_bytes(layout.state_root)
    with pytest.raises(build_storage.StorageError) as error:
        build_storage.run_managed(
            layout,
            slot="core",
            task_id="task-a",
            command=[sys.executable, "-c", "pass"],
            cwd=tmp_path,
            budget_bytes=1024,
            reserve_bytes=free + 1,
            max_active=1,
            lease_timeout=0.1,
            poll_seconds=0.05,
            allow_incremental=False,
            adopt_unknown=False,
            retained=[],
            note="",
        )
    assert error.value.reason == "reserve_breached"


def test_cleanup_plan_and_apply_only_touch_owned_idle_targets(tmp_path: Path) -> None:
    layout = _layout(tmp_path)
    owned = layout.target_dir("owned")
    owned.mkdir(parents=True)
    build_storage._ensure_owner(
        layout, "owned", "task-a", adopt_unknown=False, retained=[], note=""
    )
    (owned / "artifact").write_bytes(b"owned-data")
    unowned = layout.target_dir("stray")
    unowned.mkdir(parents=True)
    (unowned / "artifact").write_bytes(b"stray-data")
    foreign = layout.target_dir("foreign")
    foreign.mkdir(parents=True)
    build_storage._ensure_owner(
        layout, "foreign", "task-b", adopt_unknown=False, retained=[], note=""
    )

    plan = build_storage.plan_clean(layout, task_id="task-a", slots=[])

    assert [entry["slot"] for entry in plan["entries"]] == ["owned"]
    assert {entry["slot"]: entry["reason"] for entry in plan["skipped"]} == {
        "foreign": "not_task_owned",
        "stray": "unknown_ownership",
    }
    report = build_storage.apply_clean(layout, Path(str(plan["plan_path"])), task_id="task-a")
    assert report["results"][0]["status"] == "deleted"
    assert not owned.exists()
    assert unowned.exists()
    assert foreign.exists()


def test_apply_clean_refuses_path_escape(tmp_path: Path) -> None:
    layout = _layout(tmp_path)
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "keep").write_bytes(b"keep")
    plan_path = tmp_path / "plan.json"
    plan_path.write_text(
        json.dumps(
            {
                "schema_version": build_storage.SCHEMA_VERSION,
                "task_id": "task-a",
                "entries": [{"slot": "core", "path": str(outside)}],
            }
        ),
        encoding="utf-8",
    )

    report = build_storage.apply_clean(layout, plan_path, task_id="task-a")

    assert report["results"][0]["status"] == "skipped"
    assert report["results"][0]["reason"] == "path_escaped"
    assert (outside / "keep").exists()


def test_plan_clean_skips_active_lease(tmp_path: Path) -> None:
    layout = _layout(tmp_path)
    target = layout.target_dir("core")
    target.mkdir(parents=True)
    build_storage._ensure_owner(layout, "core", "task-a", adopt_unknown=False, retained=[], note="")
    lease = build_storage._slot_lease(layout, "core")
    assert lease.try_acquire()
    try:
        plan = build_storage.plan_clean(layout, task_id="task-a", slots=[])
    finally:
        lease.release()
    assert plan["entries"] == []
    assert plan["skipped"] == [{"slot": "core", "reason": "active_lease"}]
