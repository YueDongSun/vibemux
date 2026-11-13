#!/usr/bin/env python3
"""Cross-project verification entry point for the VibeMux + Personal Dev Hub harness.

This script is the executable frame for the cloud integration cases. It runs
only task-owned fixtures, a loopback/Unix-socket daemon, the local Hub adapter
source, and offline checks. It never reads a provider token, never starts a
model call, and never modifies either checkout.

Cases implemented in this source revision:

- ``C01``: a real VibeMux daemon publishes its descriptor; the real Hub
  ``local/adapters/vibemux.mjs`` reads it, performs an authenticated
  ``health``/``frontend_tasks`` exchange, completes the additive v10
  ``capabilities`` discovery, rejects an unknown descriptor version, an unknown
  operation, an unknown contract family, and an unknown catalog shape with
  stable Hub error codes.
- ``A1``: the CLI build graph excludes egui/eframe/Tauri; the default web
  build excludes the push-only ece/openssl/p256 stack.
- ``A2``: ``scripts/build_storage.py inventory`` reports a valid state.
- ``HUB-GATES`` (opt-in): the Hub's ``check``, ``test``, and
  ``verify:artifacts`` gates pass on this host.

Every other cloud case is emitted as ``not_run`` with the exact missing
capability. A case is never promoted to ``passed`` because a file exists.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any, Final

SCHEMA_VERSION: Final = 1
HUB_ADAPTER_REL: Final = "local/adapters/vibemux.mjs"
CASE_IDS: Final = (
    "C01",
    "A1",
    "A2",
    "C11-SLOTS",
    "C11-MERGE",
    "C10-MCP",
    "C08-HANDOFF",
    "C05-CANCEL",
    "C13-LINEAGE",
    "C04-EVENTS",
    "C07-HUB",
    "C04-UI",
    "C02-PLAN",
    "C07-RESTART",
    "HUB-GATES",
    "C02",
    "C03",
    "C04",
    "C05",
    "C06",
    "C07",
    "C08",
    "C09",
    "C10",
    "C11",
    "C12",
    "C13",
    "C14",
    "C15",
)

NOT_RUN_REASONS: Final[dict[str, str]] = {
    "C02": "Request identity, the pinned profile catalog, and the fixture-class planning compiler and submission (daemon plan_request through the normal gates) are implemented and Hub-boundary-tested (C02-PLAN); the live model-compiler admission (BoundReceiptV2) and a live Hub-to-daemon run remain not implemented.",
    "C03": "Lost-submit replay is implemented in fixture class (plan_request replay plus the digest-bound planning_observation query; Hub adapter boundary pinned by tests) under C02-PLAN; a live Hub consumer replay against a real daemon is not executed.",
    "C05": "Daemon dispatch cancellation is covered by C05-CANCEL; Hub generation-scoped cancellation is not implemented.",
    "C08": "Cross-track operator context sharing is covered by C08-HANDOFF; a second harness adapter taking over with its own native identity is not implemented.",
    "C10": "Both metadata services are covered by C10-MCP; a VibeMux worker calling Hub tools through an admitted broker is not implemented.",
    "C11": "Both halves are now covered by C11-SLOTS (build-slot contention) and C11-MERGE (two-worker candidate integration); this entry records no further C11 gap.",
    "C12": "Requires an AAG checkout and its real server source for v2 receipt interoperability.",
    "C13": "Known layouts are recognized and forward-merged into unified schema 15 (reports-10 reconciled, chat 10-14 upgraded); a production upgrade of a live historical database and the chat service code that would use the new tables remain outside this case.",
    "C15": "Requires the shared native host bridge and the two chat layouts.",
}

NODE_PROBE: Final = r"""
import { pathToFileURL } from 'node:url';

const adapter = await import(pathToFileURL(process.env.VMX_HUB_ADAPTER).href);
const descriptor_path = process.env.VMX_PROBE_DESCRIPTOR;
const connection = { descriptor_path, project_root: process.env.VMX_PROBE_PROJECT_ROOT };
const out = { ok: true };
try {
  const descriptor = await adapter.read_descriptor(descriptor_path);
  out.descriptor_version = descriptor.version;
  out.health = await adapter.ipc_request(connection, 'health');
  const listing = await adapter.ipc_request(connection, 'frontend_tasks', JSON.stringify({ after: null, limit: 32 }));
  out.listing = {
    project_id: listing.project_id ?? null,
    task_count: Array.isArray(listing.tasks) ? listing.tasks.length : null,
    next_cursor: listing.next_cursor ?? null,
  };
  const check = await adapter.check_vibemux(connection);
  out.check = { status: check.status, projects: check.projects.length, tasks: check.tasks.length };
  const catalog = await adapter.try_read_vibemux_capabilities(connection);
  out.discovery = catalog ? 'capabilities' : 'legacy';
  if (catalog) {
    out.catalog = {
      protocol_version: catalog.protocol_version,
      operation_count: catalog.operations.length,
      capability_access: catalog.operations.find((operation) => operation.name === 'capabilities')?.access ?? null,
      runtime_count: catalog.runtimes.length,
    };
  }
  const sample = {
    schema_version: 1,
    control_contract_id: 'vibemux.control.v10',
    protocol_version: 10,
    operations: [{ name: 'capabilities', minimum_version: 10, access: 'read',
      request_schema: 'control.request.v1', response_schema: 'control.response.v1', handler: 'control' }],
    runtimes: [],
  };
  try {
    adapter.validate_capability_catalog({ ...sample, control_contract_id: 'other.control.v10' });
    out.unknown_family_error = 'none';
  } catch (error) {
    out.unknown_family_error = error.code ?? String(error);
  }
  try {
    adapter.validate_capability_catalog({ ...sample, extra: true });
    out.unknown_shape_error = 'none';
  } catch (error) {
    out.unknown_shape_error = error.code ?? String(error);
  }
  try {
    await adapter.ipc_request(connection, 'not_a_real_operation');
    out.unknown_operation_error = 'none';
  } catch (error) {
    out.unknown_operation_error = error.code ?? String(error);
  }
  try {
    await adapter.read_descriptor(process.env.VMX_PROBE_BAD_DESCRIPTOR);
    out.unknown_version_error = 'none';
  } catch (error) {
    out.unknown_version_error = error.code ?? String(error);
  }
} catch (error) {
  out.ok = false;
  out.error = error.code ?? String(error);
}
console.log(JSON.stringify(out));
"""


class VerificationError(Exception):
    pass


def _now() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def _run(
    command: list[str],
    *,
    cwd: Path,
    env: dict[str, str] | None = None,
    timeout: float,
    check: bool = False,
) -> subprocess.CompletedProcess[str]:
    process_env = dict(os.environ)
    if env:
        process_env.update(env)
    result = subprocess.run(
        command,
        cwd=cwd,
        env=process_env,
        capture_output=True,
        text=True,
        timeout=timeout,
        check=False,
    )
    if check and result.returncode != 0:
        raise VerificationError(
            f"{' '.join(command)} exited {result.returncode}: "
            f"{(result.stderr or result.stdout).strip()[-400:]}"
        )
    return result


def _directory_bytes(path: Path) -> int | None:
    if not path.exists():
        return 0
    total = 0
    try:
        for root, _dirs, files in os.walk(path, followlinks=False):
            for name in files:
                entry = Path(root) / name
                try:
                    info = entry.lstat()
                except OSError:
                    return None
                if not stat.S_ISLNK(info.st_mode):
                    total += info.st_size
    except OSError:
        return None
    return total


def _free_bytes(path: Path) -> int | None:
    try:
        return shutil.disk_usage(path).free
    except OSError:
        return None


def _case(
    case_id: str,
    status: str,
    *,
    evidence_class: str,
    duration_ms: int = 0,
    detail: str = "",
    artifact_refs: list[str] | None = None,
) -> dict[str, Any]:
    return {
        "id": case_id,
        "status": status,
        "evidence_class": evidence_class,
        "duration_ms": duration_ms,
        "detail": detail,
        "artifact_refs": artifact_refs or [],
    }


def _tail(result: subprocess.CompletedProcess[str], limit: int = 400) -> str:
    text = (result.stderr or result.stdout).strip()
    return text[-limit:]


def _init_fixture_repo(path: Path) -> None:
    path.mkdir(parents=True, exist_ok=True)
    readme = path / "README.md"
    readme.write_text("# VibeMux cloud fixture project\n", encoding="utf-8")
    _run(["git", "init", "-q"], cwd=path, timeout=30, check=True)
    _run(["git", "config", "user.email", "fixture@example.invalid"], cwd=path, timeout=10)
    _run(["git", "config", "user.name", "VibeMux Fixture"], cwd=path, timeout=10)
    _run(["git", "add", "README.md"], cwd=path, timeout=10, check=True)
    _run(["git", "commit", "-qm", "fixture baseline"], cwd=path, timeout=30, check=True)


def _wait_for_descriptor(path: Path, timeout: float) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.is_file():
            return
        time.sleep(0.1)
    raise VerificationError(f"daemon descriptor was not published at {path}")


def _run_c01(
    *,
    vibemuxctl: Path,
    hub_root: Path,
    run_root: Path,
    timeout: float,
) -> dict[str, Any]:
    started = time.monotonic()
    fixture = run_root / "fixture-project"
    _init_fixture_repo(fixture)
    descriptor = fixture / ".vibemux" / "control.json"
    probe_path = run_root / "hub_probe.mjs"
    probe_path.write_text(NODE_PROBE, encoding="utf-8")
    adapter = hub_root / HUB_ADAPTER_REL
    if not adapter.is_file():
        return _case(
            "C01",
            "blocked",
            evidence_class="actual_daemon_hub_adapter",
            detail=f"Hub adapter missing at {adapter}",
            duration_ms=int((time.monotonic() - started) * 1000),
        )
    daemon_started = False
    try:
        start = _run(
            [str(vibemuxctl), "daemon", "start", "--project-root", str(fixture)],
            cwd=fixture,
            timeout=timeout,
        )
        # Stop in the finally block even if start reported a failure after
        # publishing a process; the fixture project is task-owned.
        daemon_started = True
        if start.returncode != 0:
            raise VerificationError(f"daemon start failed: {_tail(start)}")
        _wait_for_descriptor(descriptor, timeout)
        bad_descriptor = run_root / "unsupported-descriptor.json"
        payload = json.loads(descriptor.read_text(encoding="utf-8"))
        payload["version"] = 99
        bad_descriptor.write_text(json.dumps(payload), encoding="utf-8")
        probe = _run(
            ["node", str(probe_path)],
            cwd=hub_root,
            env={
                "VMX_HUB_ADAPTER": str(adapter),
                "VMX_PROBE_DESCRIPTOR": str(descriptor),
                "VMX_PROBE_PROJECT_ROOT": str(fixture),
                "VMX_PROBE_BAD_DESCRIPTOR": str(bad_descriptor),
            },
            timeout=timeout,
        )
        if probe.returncode != 0:
            raise VerificationError(f"Hub probe failed: {_tail(probe)}")
        try:
            result = json.loads(probe.stdout.strip().splitlines()[-1])
        except (IndexError, json.JSONDecodeError) as error:
            raise VerificationError(f"Hub probe emitted no JSON: {error}") from error
        failures: list[str] = []
        if result.get("ok") is not True:
            failures.append(f"hub_error={result.get('error')}")
        if result.get("health", {}).get("healthy") is not True:
            failures.append("health_not_healthy")
        if not isinstance(result.get("listing", {}).get("task_count"), int):
            failures.append("task_listing_missing")
        if result.get("check", {}).get("status") != "connected":
            failures.append("check_not_connected")
        if result.get("unknown_operation_error") != "vibemux_operation_not_allowed":
            failures.append(f"unknown_operation={result.get('unknown_operation_error')}")
        if result.get("unknown_version_error") != "vibemux_ipc_version_mismatch":
            failures.append(f"unknown_version={result.get('unknown_version_error')}")
        if result.get("discovery") != "capabilities":
            failures.append(f"discovery={result.get('discovery')}")
        catalog = result.get("catalog", {})
        if catalog.get("protocol_version") != result.get("descriptor_version"):
            failures.append(f"catalog_version={catalog.get('protocol_version')}")
        if catalog.get("capability_access") != "read":
            failures.append(f"capability_access={catalog.get('capability_access')}")
        if (
            not isinstance(catalog.get("operation_count"), int)
            or catalog.get("operation_count", 0) < 1
        ):
            failures.append("catalog_operations_missing")
        if not isinstance(catalog.get("runtime_count"), int):
            failures.append("catalog_runtimes_missing")
        if result.get("unknown_family_error") != "vibemux_contract_family_mismatch":
            failures.append(f"unknown_family={result.get('unknown_family_error')}")
        if result.get("unknown_shape_error") != "vibemux_contract_mismatch":
            failures.append(f"unknown_shape={result.get('unknown_shape_error')}")
        detail = (
            f"descriptor_version={result.get('descriptor_version')} "
            f"project_id={result.get('listing', {}).get('project_id')} "
            f"tasks={result.get('listing', {}).get('task_count')} "
            f"check={result.get('check', {}).get('status')} "
            f"discovery={result.get('discovery')} "
            f"operations={catalog.get('operation_count')} "
            f"runtimes={catalog.get('runtime_count')}"
        )
        artifact = run_root / "c01-hub-probe.json"
        artifact.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
        return _case(
            "C01",
            "failed" if failures else "passed",
            evidence_class="actual_daemon_hub_adapter",
            duration_ms=int((time.monotonic() - started) * 1000),
            detail="; ".join(failures) or detail,
            artifact_refs=[str(artifact)],
        )
    except (VerificationError, OSError, subprocess.TimeoutExpired) as error:
        return _case(
            "C01",
            "failed",
            evidence_class="actual_daemon_hub_adapter",
            duration_ms=int((time.monotonic() - started) * 1000),
            detail=str(error),
        )
    finally:
        if daemon_started:
            _run(
                [str(vibemuxctl), "daemon", "stop", "--project-root", str(fixture)],
                cwd=fixture,
                timeout=timeout,
            )


def _run_a1(*, vibemux_root: Path, timeout: float) -> dict[str, Any]:
    started = time.monotonic()
    checks = [
        (
            ["cargo", "tree", "--locked", "-p", "vibemux_cli", "-e", "normal"],
            ("egui", "eframe", "winit", "tauri"),
            "cli_gui_dependency",
        ),
        (
            ["cargo", "tree", "--locked", "-p", "vibemux_web", "-e", "normal"],
            ("ece", "openssl", "p256"),
            "web_default_push_dependency",
        ),
    ]
    failures: list[str] = []
    try:
        for command, forbidden, label in checks:
            tree = _run(command, cwd=vibemux_root, timeout=timeout)
            if tree.returncode != 0:
                failures.append(f"{label}: tree failed ({_tail(tree, 200)})")
                continue
            lowered = tree.stdout.lower()
            hits = [name for name in forbidden if name in lowered]
            if hits:
                failures.append(f"{label}: {','.join(hits)}")
        push_tree = _run(
            [
                "cargo",
                "tree",
                "--locked",
                "-p",
                "vibemux_web",
                "-e",
                "normal",
                "--features",
                "push",
            ],
            cwd=vibemux_root,
            timeout=timeout,
        )
        if push_tree.returncode != 0 or "openssl" not in push_tree.stdout.lower():
            failures.append("web_push_feature_missing_openssl")
        return _case(
            "A1",
            "failed" if failures else "passed",
            evidence_class="source_dependency_graph",
            duration_ms=int((time.monotonic() - started) * 1000),
            detail="; ".join(failures) or "CLI GUI-free; web push opt-in",
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        return _case(
            "A1",
            "failed",
            evidence_class="source_dependency_graph",
            duration_ms=int((time.monotonic() - started) * 1000),
            detail=str(error),
        )


def _run_a2(*, vibemux_root: Path, state_root: Path, timeout: float) -> dict[str, Any]:
    started = time.monotonic()
    script = vibemux_root / "scripts" / "build_storage.py"
    try:
        result = _run(
            [sys.executable, str(script), "--state-root", str(state_root), "--json", "inventory"],
            cwd=vibemux_root,
            timeout=timeout,
        )
        if result.returncode != 0:
            raise VerificationError(_tail(result))
        report = json.loads(result.stdout)
        if report.get("schema_version") != 1 or "filesystem_free_bytes" not in report:
            raise VerificationError("inventory schema mismatch")
        return _case(
            "A2",
            "passed",
            evidence_class="local_build_tooling",
            duration_ms=int((time.monotonic() - started) * 1000),
            detail=f"free={report['filesystem_free_bytes']} slots={len(report['slots'])}",
        )
    except (VerificationError, OSError, json.JSONDecodeError, subprocess.TimeoutExpired) as error:
        return _case(
            "A2",
            "failed",
            evidence_class="local_build_tooling",
            duration_ms=int((time.monotonic() - started) * 1000),
            detail=str(error),
        )


def _run_hub_gates(*, hub_root: Path, timeout: float) -> dict[str, Any]:
    started = time.monotonic()
    results: list[str] = []
    failed = False
    for command in (
        ["npm", "run", "check"],
        ["npm", "test"],
        ["npm", "run", "verify:artifacts"],
    ):
        try:
            result = _run(command, cwd=hub_root, timeout=timeout)
        except (OSError, subprocess.TimeoutExpired) as error:
            results.append(f"{' '.join(command)}: {error}")
            failed = True
            continue
        results.append(f"{' '.join(command)}: exit {result.returncode}")
        if result.returncode != 0:
            failed = True
    return _case(
        "HUB-GATES",
        "failed" if failed else "passed",
        evidence_class="actual_hub_tests",
        duration_ms=int((time.monotonic() - started) * 1000),
        detail="; ".join(results),
    )


def _run_c11_slots(*, vibemux_root: Path, state_root: Path, timeout: float) -> dict[str, Any]:
    """Two owned Git worktrees compete for one managed build slot.

    This covers the slot/ownership half of C11: two independent worktrees,
    one registered task per worktree, a single active-build token, observable
    serialization, clear ownership, slot release, and owned cleanup. The
    worker-candidate merge half is not claimed by this case.
    """
    started = time.monotonic()
    build_storage = vibemux_root / "scripts" / "build_storage.py"
    root = state_root / "c11"
    base = root / "base"
    worktree_a = root / "worktree-a"
    worktree_b = root / "worktree-b"

    def launch(worktree: Path, task_id: str) -> subprocess.Popen[str]:
        script = (
            "import pathlib, time\n"
            "start = time.time()\n"
            "pathlib.Path('managed-build-started').write_text(str(start))\n"
            "time.sleep(1.2)\n"
            "pathlib.Path('managed-build-done').write_text(str(time.time()))\n"
        )
        return subprocess.Popen(
            [
                sys.executable,
                str(build_storage),
                "--state-root",
                str(state_root),
                "run",
                "--slot",
                "c11",
                "--task-id",
                task_id,
                "--max-active",
                "1",
                "--lease-timeout",
                str(max(30.0, timeout)),
                "--poll-seconds",
                "0.1",
                "--",
                sys.executable,
                "-c",
                script,
            ],
            cwd=worktree,
            env=dict(os.environ),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )

    try:
        _init_fixture_repo(base)
        _run(
            ["git", "worktree", "add", "-q", "-b", "c11-a", str(worktree_a)],
            cwd=base,
            timeout=timeout,
            check=True,
        )
        _run(
            ["git", "worktree", "add", "-q", "-b", "c11-b", str(worktree_b)],
            cwd=base,
            timeout=timeout,
            check=True,
        )
        first = launch(worktree_a, "c11-worktree-a")
        second = launch(worktree_b, "c11-worktree-b")
        out_a, err_a = first.communicate(timeout=timeout)
        out_b, err_b = second.communicate(timeout=timeout)
        failures: list[str] = []
        if first.returncode != 0:
            failures.append(f"worktree_a_exit={first.returncode}:{err_a.strip()[-160:]}")
        if second.returncode != 0:
            failures.append(f"worktree_b_exit={second.returncode}:{err_b.strip()[-160:]}")
        intervals: dict[str, tuple[float, float]] = {}
        for label, worktree in (("a", worktree_a), ("b", worktree_b)):
            started_file = worktree / "managed-build-started"
            done_file = worktree / "managed-build-done"
            if not started_file.is_file() or not done_file.is_file():
                failures.append(f"worktree_{label}_marker_missing")
                continue
            intervals[label] = (
                float(started_file.read_text().strip()),
                float(done_file.read_text().strip()),
            )
        if len(intervals) == 2:
            overlap = min(intervals["a"][1], intervals["b"][1]) - max(
                intervals["a"][0], intervals["b"][0]
            )
            # A shared max-active=1 token must serialize the children.
            if overlap > 0.25:
                failures.append(f"build_overlap={overlap:.2f}s")
        inventory = _run(
            [
                sys.executable,
                str(build_storage),
                "--state-root",
                str(state_root),
                "--json",
                "inventory",
            ],
            cwd=vibemux_root,
            timeout=timeout,
        )
        slot: dict[str, Any] | None = None
        if inventory.returncode == 0:
            report = json.loads(inventory.stdout)
            slot = next(
                (item for item in report.get("slots", []) if item.get("slot") == "c11"),
                None,
            )
        if slot is None:
            failures.append("slot_inventory_missing")
        else:
            if slot.get("active") is not False:
                failures.append("slot_still_active")
            owner = slot.get("owner") or {}
            tasks = owner.get("tasks", [])
            for task_id in ("c11-worktree-a", "c11-worktree-b"):
                if task_id not in tasks:
                    failures.append(f"owner_missing:{task_id}")
        plan = _run(
            [
                sys.executable,
                str(build_storage),
                "--state-root",
                str(state_root),
                "plan-clean",
                "--task-id",
                "c11-worktree-a",
                "--slot",
                "c11",
            ],
            cwd=vibemux_root,
            timeout=timeout,
        )
        plan_path = ""
        if plan.returncode == 0:
            for line in plan.stdout.splitlines():
                if line.startswith("plan: "):
                    plan_path = line[len("plan: ") :].strip()
        if not plan_path:
            failures.append("cleanup_plan_missing")
        else:
            applied = _run(
                [
                    sys.executable,
                    str(build_storage),
                    "--state-root",
                    str(state_root),
                    "apply-clean",
                    plan_path,
                    "--task-id",
                    "c11-worktree-a",
                ],
                cwd=vibemux_root,
                timeout=timeout,
            )
            if applied.returncode != 0:
                failures.append(f"cleanup_failed:{_tail(applied, 160)}")
        target = state_root / "slots" / "c11" / "target"
        if target.exists():
            failures.append("slot_target_not_removed")
        return _case(
            "C11-SLOTS",
            "failed" if failures else "passed",
            evidence_class="managed_build_slot_contention",
            duration_ms=int((time.monotonic() - started) * 1000),
            detail="; ".join(failures)
            or (
                "two worktrees serialized on one max-active=1 token; "
                "ownership recorded; slot released and cleaned"
            ),
            artifact_refs=[],
        )
    except (OSError, subprocess.TimeoutExpired, json.JSONDecodeError) as error:
        return _case(
            "C11-SLOTS",
            "failed",
            evidence_class="managed_build_slot_contention",
            duration_ms=int((time.monotonic() - started) * 1000),
            detail=str(error),
        )


def _run_c13_lineage(
    *, vibemux_root: Path, build_state_root: Path, timeout: float
) -> dict[str, Any]:
    """Recognize and preserve the known historical store layouts.

    The case runs the store's lineage tests in a managed slot: a current
    database, PR #52's reports schema 10, PR #49's chat schema 10 and 14, a
    foreign Python store, a future marker, and an incomplete current layout.
    """
    started = time.monotonic()
    build_storage = vibemux_root / "scripts" / "build_storage.py"
    try:
        result = _run(
            [
                sys.executable,
                str(build_storage),
                "--state-root",
                str(build_state_root),
                "run",
                "--slot",
                "verify",
                "--task-id",
                "c13-lineage",
                "--budget-bytes",
                "8G",
                "--reserve-bytes",
                "4G",
                "--",
                "cargo",
                "test",
                "--locked",
                "-p",
                "vibemux_store",
                "--all-features",
                "--lib",
                "lineage",
            ],
            cwd=vibemux_root,
            timeout=timeout,
        )
        if result.returncode != 0:
            raise VerificationError(_tail(result, 600))
        passed = 0
        for line in result.stdout.splitlines():
            if line.startswith("test result: ok."):
                for token in line.split():
                    if token == "passed;":
                        passed = int(line.split()[line.split().index("passed;") - 1])
                        break
        if passed < 5:
            raise VerificationError(f"expected at least five lineage tests, saw {passed}")
        return _case(
            "C13-LINEAGE",
            "passed",
            evidence_class="store_migration_tests",
            duration_ms=int((time.monotonic() - started) * 1000),
            detail=(
                f"{passed} lineage tests passed: current v15, reports-10 "
                "reconciled, chat-10/14 upgraded, foreign/future/incomplete refused"
            ),
        )
    except (VerificationError, OSError, subprocess.TimeoutExpired) as error:
        return _case(
            "C13-LINEAGE",
            "failed",
            evidence_class="store_migration_tests",
            duration_ms=int((time.monotonic() - started) * 1000),
            detail=str(error),
        )


def _run_c04_events(
    *, vibemux_root: Path, build_state_root: Path, hub_root: Path, timeout: float
) -> dict[str, Any]:
    """Paged event reads, retention-floor resync, and the Hub cursor consumer."""
    started = time.monotonic()
    build_storage = vibemux_root / "scripts" / "build_storage.py"

    def cargo_test(package: str, filter_text: str) -> subprocess.CompletedProcess[str]:
        return _run(
            [
                sys.executable,
                str(build_storage),
                "--state-root",
                str(build_state_root),
                "run",
                "--slot",
                "verify",
                "--task-id",
                "c04-events",
                "--budget-bytes",
                "8G",
                "--reserve-bytes",
                "4G",
                "--",
                "cargo",
                "test",
                "--locked",
                "-p",
                package,
                "--all-features",
                "--lib",
                filter_text,
            ],
            cwd=vibemux_root,
            timeout=timeout,
        )

    try:
        store = cargo_test("vibemux_store", "frontend")
        if store.returncode != 0:
            raise VerificationError(f"store event tests failed: {_tail(store, 500)}")
        control = cargo_test("vibemuxd", "frontend_events")
        if control.returncode != 0:
            raise VerificationError(f"control event test failed: {_tail(control, 500)}")
        hub = _run(
            ["node", "--test", "tests/vibemux.test.mjs"],
            cwd=hub_root,
            timeout=timeout,
        )
        if hub.returncode != 0:
            raise VerificationError(f"Hub event tests failed: {_tail(hub, 500)}")
        store_passed = _cargo_passed(store.stdout)
        control_passed = _cargo_passed(control.stdout)
        hub_passed = 0
        for line in hub.stdout.splitlines():
            if line.startswith("ℹ pass "):
                hub_passed = int(line.rsplit(" ", 1)[-1])
        return _case(
            "C04-EVENTS",
            "passed",
            evidence_class="event_pagination_contracts",
            duration_ms=int((time.monotonic() - started) * 1000),
            detail=(
                f"store={store_passed} control={control_passed} hub={hub_passed}; "
                "cursor pages, retention-floor resync, and the Hub consumer verified"
            ),
        )
    except (VerificationError, OSError, subprocess.TimeoutExpired) as error:
        return _case(
            "C04-EVENTS",
            "failed",
            evidence_class="event_pagination_contracts",
            duration_ms=int((time.monotonic() - started) * 1000),
            detail=str(error),
        )


def _cargo_passed(stdout: str) -> int:
    for line in stdout.splitlines():
        if line.startswith("test result: ok."):
            fields = line.split()
            if "passed;" in fields:
                return int(fields[fields.index("passed;") - 1])
    return 0


def _cargo_sum_passed(stdout: str) -> int:
    """Total passed across every test-target summary in one cargo run."""
    total = 0
    for line in stdout.splitlines():
        if line.startswith("test result: ok."):
            fields = line.split()
            if "passed;" in fields:
                total += int(fields[fields.index("passed;") - 1])
    return total


def _run_c07_restart(*, vibemuxctl: Path, run_root: Path, timeout: float) -> dict[str, Any]:
    """Stop and restart the daemon over the same project store.

    Asserts the restarted daemon serves health and capability discovery, and
    that it reopens the same store file instead of creating a new one. Hub
    restart recovery of suspended tasks is not claimed by this case.
    """
    started = time.monotonic()
    fixture = run_root / "c07"
    descriptor = fixture / ".vibemux" / "control.json"
    store = fixture / ".vibemux" / "vibemux_rust.sqlite3"
    running = False

    def start() -> None:
        nonlocal running
        result = _run(
            [str(vibemuxctl), "daemon", "start", "--project-root", str(fixture)],
            cwd=fixture,
            timeout=timeout,
        )
        running = True
        if result.returncode != 0:
            raise VerificationError(f"daemon start failed: {_tail(result)}")
        _wait_for_descriptor(descriptor, timeout)

    def stop() -> None:
        nonlocal running
        result = _run(
            [str(vibemuxctl), "daemon", "stop", "--project-root", str(fixture)],
            cwd=fixture,
            timeout=timeout,
        )
        if result.returncode != 0:
            raise VerificationError(f"daemon stop failed: {_tail(result)}")
        running = False

    def health() -> int:
        result = _run(
            [str(vibemuxctl), "--json", "daemon", "health", "--project-root", str(fixture)],
            cwd=fixture,
            timeout=timeout,
        )
        if result.returncode != 0:
            raise VerificationError(f"daemon health failed: {_tail(result)}")
        try:
            value = json.loads(result.stdout)
        except json.JSONDecodeError as error:
            raise VerificationError(f"health emitted no JSON: {error}") from error
        if value.get("ok") is not True:
            raise VerificationError("health is not ok")
        return int(value.get("process_id", 0))

    def capabilities() -> int:
        result = _run(
            [str(vibemuxctl), "--json", "capabilities", "--project-root", str(fixture)],
            cwd=fixture,
            timeout=timeout,
        )
        if result.returncode != 0:
            raise VerificationError(f"capabilities failed: {_tail(result)}")
        try:
            value = json.loads(result.stdout)
        except json.JSONDecodeError as error:
            raise VerificationError(f"capabilities emitted no JSON: {error}") from error
        operations = value.get("capabilities", {}).get("operations", [])
        if not operations:
            raise VerificationError("capability catalog is empty")
        return len(operations)

    try:
        _init_fixture_repo(fixture)
        start()
        first_pid = health()
        first_operations = capabilities()
        if not store.is_file():
            raise VerificationError(f"store file missing at {store}")
        first_inode = store.stat().st_ino
        first_size = store.stat().st_size
        stop()
        start()
        second_pid = health()
        second_operations = capabilities()
        second_inode = store.stat().st_ino
        second_size = store.stat().st_size
        if first_pid == second_pid:
            raise VerificationError("restart reused the same process id")
        if first_inode != second_inode:
            raise VerificationError("restart replaced the store file instead of reopening it")
        if second_size < first_size:
            raise VerificationError("restart shrank the store")
        if first_operations != second_operations:
            raise VerificationError("capability catalog changed across restart")
        return _case(
            "C07-RESTART",
            "passed",
            evidence_class="daemon_restart_store_reuse",
            duration_ms=int((time.monotonic() - started) * 1000),
            detail=(
                f"health and {second_operations} capabilities after restart; "
                f"same store inode; pid {first_pid}->{second_pid}"
            ),
        )
    except (VerificationError, OSError, subprocess.TimeoutExpired) as error:
        return _case(
            "C07-RESTART",
            "failed",
            evidence_class="daemon_restart_store_reuse",
            duration_ms=int((time.monotonic() - started) * 1000),
            detail=str(error),
        )
    finally:
        if running:
            _run(
                [str(vibemuxctl), "daemon", "stop", "--project-root", str(fixture)],
                cwd=fixture,
                timeout=timeout,
            )


def _run_c06(*, vibemux_root: Path, build_state_root: Path, timeout: float) -> dict[str, Any]:
    """A wrong fixture candidate is rejected by the independent verifier.

    The test starts a real in-process daemon, a real store, real Git
    worktrees, a writable fixture worker, and the trusted TaskBoard suite.
    The reviewer passes the candidate on purpose, so the rejection is
    attributable to the verifier; a positive control accepts a good
    candidate through the same path.
    """
    started = time.monotonic()
    build_storage = vibemux_root / "scripts" / "build_storage.py"
    try:
        result = _run(
            [
                sys.executable,
                str(build_storage),
                "--state-root",
                str(build_state_root),
                "run",
                "--slot",
                "verify",
                "--task-id",
                "c06-verifier",
                "--budget-bytes",
                "12G",
                "--reserve-bytes",
                "4G",
                "--",
                "cargo",
                "test",
                "--locked",
                "-p",
                "vibemuxd",
                "--features",
                "test_helpers",
                "--test",
                "workflow_gates",
                "c06_",
            ],
            cwd=vibemux_root,
            timeout=timeout,
        )
        if result.returncode != 0:
            raise VerificationError(_tail(result, 700))
        passed = _cargo_passed(result.stdout)
        if passed < 2:
            raise VerificationError(f"expected two C06 tests, saw {passed}")
        return _case(
            "C06",
            "passed",
            evidence_class="actual_daemon_fixture_worker_verifier",
            duration_ms=int((time.monotonic() - started) * 1000),
            detail=(
                f"{passed} tests: wrong candidate rejected by the independent verifier; "
                "good candidate accepted by the same path; checkout clean"
            ),
        )
    except (VerificationError, OSError, subprocess.TimeoutExpired) as error:
        return _case(
            "C06",
            "failed",
            evidence_class="actual_daemon_fixture_worker_verifier",
            duration_ms=int((time.monotonic() - started) * 1000),
            detail=str(error),
        )


def _run_c14_fusion(
    *, vibemux_root: Path, build_state_root: Path, timeout: float
) -> dict[str, Any]:
    """The ported Fusion benchmark contracts, offline and deterministic.

    The example's tests cover the four modes' contracts, budgets, scoring,
    cancellation, and append-only evidence without any provider call.
    """
    started = time.monotonic()
    build_storage = vibemux_root / "scripts" / "build_storage.py"
    try:
        result = _run(
            [
                sys.executable,
                str(build_storage),
                "--state-root",
                str(build_state_root),
                "run",
                "--slot",
                "verify",
                "--task-id",
                "c14-fusion",
                "--budget-bytes",
                "8G",
                "--reserve-bytes",
                "4G",
                "--",
                "cargo",
                "test",
                "--locked",
                "-p",
                "vibemux_harness_native",
                "--example",
                "fusion_benchmark",
            ],
            cwd=vibemux_root,
            timeout=timeout,
        )
        if result.returncode != 0:
            raise VerificationError(_tail(result, 700))
        passed = _cargo_passed(result.stdout)
        if passed < 14:
            raise VerificationError(f"expected fourteen Fusion contract tests, saw {passed}")
        return _case(
            "C14",
            "passed",
            evidence_class="offline_fusion_contracts",
            duration_ms=int((time.monotonic() - started) * 1000),
            detail=(
                f"{passed} offline contract tests: modes, budgets, scoring, "
                "cancellation, and append-only evidence; no provider call"
            ),
        )
    except (VerificationError, OSError, subprocess.TimeoutExpired) as error:
        return _case(
            "C14",
            "failed",
            evidence_class="offline_fusion_contracts",
            duration_ms=int((time.monotonic() - started) * 1000),
            detail=str(error),
        )


def _run_c11_merge(*, vibemux_root: Path, build_state_root: Path, timeout: float) -> dict[str, Any]:
    """Two owned workers build, review, and integrate through the workflow path."""
    started = time.monotonic()
    build_storage = vibemux_root / "scripts" / "build_storage.py"
    try:
        result = _run(
            [
                sys.executable,
                str(build_storage),
                "--state-root",
                str(build_state_root),
                "run",
                "--slot",
                "verify",
                "--task-id",
                "c11-merge",
                "--budget-bytes",
                "12G",
                "--reserve-bytes",
                "4G",
                "--",
                "cargo",
                "test",
                "--locked",
                "-p",
                "vibemuxd",
                "--features",
                "test_helpers",
                "--test",
                "workflow_cooperate",
            ],
            cwd=vibemux_root,
            timeout=timeout,
        )
        if result.returncode != 0:
            raise VerificationError(_tail(result, 700))
        passed = _cargo_passed(result.stdout)
        if passed < 2:
            raise VerificationError(f"expected two two-worker tests, saw {passed}")
        return _case(
            "C11-MERGE",
            "passed",
            evidence_class="actual_daemon_two_worker_integration",
            duration_ms=int((time.monotonic() - started) * 1000),
            detail=(
                f"{passed} tests: two workers build in isolated worktrees, review, "
                "and integrate through the scoped workflow path"
            ),
        )
    except (VerificationError, OSError, subprocess.TimeoutExpired) as error:
        return _case(
            "C11-MERGE",
            "failed",
            evidence_class="actual_daemon_two_worker_integration",
            duration_ms=int((time.monotonic() - started) * 1000),
            detail=str(error),
        )


def _run_c09_skills(*, hub_root: Path, timeout: float) -> dict[str, Any]:
    """The Hub resolves a declared skill ref into a bounded context bundle.

    The resolver refuses escapes, symlinks, hardlinks, non-UTF-8, and
    oversized content; the bundle carries the exact content hash and the
    constant `execution_authority: none`, so skill text never grants tools or
    permissions.
    """
    started = time.monotonic()
    try:
        result = _run(
            ["node", "--test", "tests/skill_refs.test.mjs"],
            cwd=hub_root,
            timeout=timeout,
        )
        if result.returncode != 0:
            raise VerificationError(_tail(result, 500))
        passed = 0
        for line in result.stdout.splitlines():
            if line.startswith("ℹ pass "):
                passed = int(line.rsplit(" ", 1)[-1])
        if passed < 6:
            raise VerificationError(f"expected six skill-ref tests, saw {passed}")
        return _case(
            "C09",
            "passed",
            evidence_class="skill_ref_resolution",
            duration_ms=int((time.monotonic() - started) * 1000),
            detail=(
                f"{passed} tests: escape/symlink/hardlink/size refusals, exact "
                "content hash, bounded bundle, execution_authority=none"
            ),
        )
    except (VerificationError, OSError, subprocess.TimeoutExpired) as error:
        return _case(
            "C09",
            "failed",
            evidence_class="skill_ref_resolution",
            duration_ms=int((time.monotonic() - started) * 1000),
            detail=str(error),
        )


def _run_cargo_case(
    *,
    id: str,
    vibemux_root: Path,
    build_state_root: Path,
    timeout: float,
    package: str,
    test_target: str,
    minimum_passed: int,
    evidence_class: str,
    detail_prefix: str,
) -> dict[str, Any]:
    started = time.monotonic()
    build_storage = vibemux_root / "scripts" / "build_storage.py"
    try:
        result = _run(
            [
                sys.executable,
                str(build_storage),
                "--state-root",
                str(build_state_root),
                "run",
                "--slot",
                "verify",
                "--task-id",
                id.lower(),
                "--budget-bytes",
                "12G",
                "--reserve-bytes",
                "4G",
                "--",
                "cargo",
                "test",
                "--locked",
                "-p",
                package,
                "--features",
                "test_helpers",
                "--test",
                test_target,
            ],
            cwd=vibemux_root,
            timeout=timeout,
        )
        if result.returncode != 0:
            raise VerificationError(_tail(result, 700))
        passed = _cargo_passed(result.stdout)
        if passed < minimum_passed:
            raise VerificationError(f"expected at least {minimum_passed} tests, saw {passed}")
        return _case(
            id,
            "passed",
            evidence_class=evidence_class,
            duration_ms=int((time.monotonic() - started) * 1000),
            detail=f"{passed} tests: {detail_prefix}",
        )
    except (VerificationError, OSError, subprocess.TimeoutExpired) as error:
        return _case(
            id,
            "failed",
            evidence_class=evidence_class,
            duration_ms=int((time.monotonic() - started) * 1000),
            detail=str(error),
        )


def _run_c05_cancel(
    *, vibemux_root: Path, build_state_root: Path, timeout: float
) -> dict[str, Any]:
    return _run_cargo_case(
        id="C05-CANCEL",
        vibemux_root=vibemux_root,
        build_state_root=build_state_root,
        timeout=timeout,
        package="vibemuxd",
        test_target="control_harness_dispatch",
        minimum_passed=2,
        evidence_class="actual_daemon_dispatch_cancellation",
        detail_prefix="live capture, cancel, and shutdown of a waiting probe",
    )


def _run_c08_handoff(
    *, vibemux_root: Path, build_state_root: Path, timeout: float
) -> dict[str, Any]:
    return _run_cargo_case(
        id="C08-HANDOFF",
        vibemux_root=vibemux_root,
        build_state_root=build_state_root,
        timeout=timeout,
        package="vibemuxd",
        test_target="workflow_share",
        minimum_passed=2,
        evidence_class="actual_daemon_context_handoff",
        detail_prefix="operator share reaches the other track and survives restarts",
    )


def _run_c10_mcp(
    *, vibemux_root: Path, build_state_root: Path, hub_root: Path, timeout: float
) -> dict[str, Any]:
    started = time.monotonic()
    build_storage = vibemux_root / "scripts" / "build_storage.py"
    try:
        rust = _run(
            [
                sys.executable,
                str(build_storage),
                "--state-root",
                str(build_state_root),
                "run",
                "--slot",
                "verify",
                "--task-id",
                "c10-mcp",
                "--budget-bytes",
                "12G",
                "--reserve-bytes",
                "4G",
                "--",
                "cargo",
                "test",
                "--locked",
                "-p",
                "vibemux_mcp",
                "--features",
                "test_helpers",
                "--test",
                "ipc",
            ],
            cwd=vibemux_root,
            timeout=timeout,
        )
        if rust.returncode != 0:
            raise VerificationError(f"VibeMux MCP tests failed: {_tail(rust, 500)}")
        hub = _run(["node", "--test", "tests/mcp.test.mjs"], cwd=hub_root, timeout=timeout)
        if hub.returncode != 0:
            raise VerificationError(f"Hub MCP tests failed: {_tail(hub, 500)}")
        rust_passed = _cargo_passed(rust.stdout)
        hub_passed = 0
        for line in hub.stdout.splitlines():
            if line.startswith("ℹ pass "):
                hub_passed = int(line.rsplit(" ", 1)[-1])
        if rust_passed < 3 or hub_passed < 1:
            raise VerificationError(
                f"expected VibeMux MCP tests and Hub MCP tests, saw {rust_passed}/{hub_passed}"
            )
        return _case(
            "C10-MCP",
            "passed",
            evidence_class="metadata_services_fixture",
            duration_ms=int((time.monotonic() - started) * 1000),
            detail=(
                f"VibeMux MCP={rust_passed} Hub MCP={hub_passed}; both read-only "
                "metadata services initialize and serve tools; a worker calling "
                "Hub tools is not claimed"
            ),
        )
    except (VerificationError, OSError, subprocess.TimeoutExpired) as error:
        return _case(
            "C10-MCP",
            "failed",
            evidence_class="metadata_services_fixture",
            duration_ms=int((time.monotonic() - started) * 1000),
            detail=str(error),
        )


def _run_node_case(
    *,
    id: str,
    hub_root: Path,
    timeout: float,
    arguments: list[str],
    minimum_passed: int,
    evidence_class: str,
    detail_prefix: str,
) -> dict[str, Any]:
    started = time.monotonic()
    try:
        result = _run(["node", *arguments], cwd=hub_root, timeout=timeout)
        if result.returncode != 0:
            raise VerificationError(_tail(result, 500))
        passed = 0
        for line in result.stdout.splitlines():
            if line.startswith("ℹ pass "):
                passed = int(line.rsplit(" ", 1)[-1])
        if passed < minimum_passed:
            raise VerificationError(f"expected at least {minimum_passed} tests, saw {passed}")
        return _case(
            id,
            "passed",
            evidence_class=evidence_class,
            duration_ms=int((time.monotonic() - started) * 1000),
            detail=f"{passed} tests: {detail_prefix}",
        )
    except (VerificationError, OSError, subprocess.TimeoutExpired) as error:
        return _case(
            id,
            "failed",
            evidence_class=evidence_class,
            duration_ms=int((time.monotonic() - started) * 1000),
            detail=str(error),
        )


def _run_c04_ui(*, hub_root: Path, timeout: float) -> dict[str, Any]:
    return _run_node_case(
        id="C04-UI",
        hub_root=hub_root,
        timeout=timeout,
        arguments=["--test", "tests/workspaces_ui.test.mjs"],
        minimum_passed=3,
        evidence_class="hub_event_ui",
        detail_prefix="paged history, exact next cursor, end marker, passive render",
    )


def _run_c07_hub(*, hub_root: Path, timeout: float) -> dict[str, Any]:
    return _run_node_case(
        id="C07-HUB",
        hub_root=hub_root,
        timeout=timeout,
        arguments=["--test", "--test-name-pattern", "restarted Hub", "tests/vibemux.test.mjs"],
        minimum_passed=1,
        evidence_class="hub_restart_recheck",
        detail_prefix="a restarted Hub reports not_checked and re-reads the daemon",
    )


def _run_c02_plan(
    *, vibemux_root: Path, build_state_root: Path, hub_root: Path, timeout: float
) -> dict[str, Any]:
    """Planning request identity and the Hub-validated observation contract."""
    started = time.monotonic()
    build_storage = vibemux_root / "scripts" / "build_storage.py"
    try:
        rust = _run(
            [
                sys.executable,
                str(build_storage),
                "--state-root",
                str(build_state_root),
                "run",
                "--slot",
                "verify",
                "--task-id",
                "c02-plan",
                "--budget-bytes",
                "12G",
                "--reserve-bytes",
                "4G",
                "--",
                "cargo",
                "test",
                "--locked",
                "-p",
                "vibemux_workflow",
                "--all-features",
                "planning_contract",
            ],
            cwd=vibemux_root,
            timeout=timeout,
        )
        if rust.returncode != 0:
            raise VerificationError(f"planning contract tests failed: {_tail(rust, 500)}")
        daemon = _run(
            [
                sys.executable,
                str(build_storage),
                "--state-root",
                str(build_state_root),
                "run",
                "--slot",
                "verify",
                "--task-id",
                "c02-plan",
                "--budget-bytes",
                "12G",
                "--reserve-bytes",
                "4G",
                "--",
                "cargo",
                "test",
                "--locked",
                "-p",
                "vibemuxd",
                "--all-features",
                "--test",
                "workflow_planning_profiles",
                "--test",
                "workflow_plan",
            ],
            cwd=vibemux_root,
            timeout=timeout,
        )
        if daemon.returncode != 0:
            raise VerificationError(
                f"planning daemon tests failed: {_tail(daemon, 500)}"
            )
        hub = _run(
            ["node", "--test", "tests/planning_contract.test.mjs"], cwd=hub_root, timeout=timeout
        )
        if hub.returncode != 0:
            raise VerificationError(f"Hub planning contract failed: {_tail(hub, 500)}")
        hub_adapter = _run(
            ["node", "--test", "tests/rd_planning.test.mjs"], cwd=hub_root, timeout=timeout
        )
        if hub_adapter.returncode != 0:
            raise VerificationError(
                f"Hub planning adapter tests failed: {_tail(hub_adapter, 500)}"
            )
        rust_passed = _cargo_passed(rust.stdout)
        daemon_passed = _cargo_sum_passed(daemon.stdout)

        def _node_passed(stdout: str) -> int:
            for line in stdout.splitlines():
                if line.startswith("ℹ pass "):
                    return int(line.rsplit(" ", 1)[-1])
            return 0

        hub_passed = _node_passed(hub.stdout)
        adapter_passed = _node_passed(hub_adapter.stdout)
        if (
            rust_passed < 5
            or daemon_passed < 3
            or hub_passed < 3
            or adapter_passed < 12
        ):
            raise VerificationError(
                "expected planning tests, saw "
                f"{rust_passed}/{daemon_passed}/{hub_passed}/{adapter_passed}"
            )
        return _case(
            "C02-PLAN",
            "passed",
            evidence_class="planning_contract_fixture",
            duration_ms=int((time.monotonic() - started) * 1000),
            detail=(
                f"VibeMux={rust_passed} daemon={daemon_passed} Hub={hub_passed} "
                f"adapter={adapter_passed}: profile digest, retained request UUID, "
                "content-free observation, pinned catalog, fixture compile through "
                "the normal gates, lost-submit replay, and the Hub CLI boundary; "
                "live model admission and a live Hub run are not claimed"
            ),
        )
    except (VerificationError, OSError, subprocess.TimeoutExpired) as error:
        return _case(
            "C02-PLAN",
            "failed",
            evidence_class="planning_contract_fixture",
            duration_ms=int((time.monotonic() - started) * 1000),
            detail=str(error),
        )


def _parse_args(argv: list[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="verify_fused_harness_cloud.py",
        description=(
            "Run the locally executable cloud-harness verification frame. "
            "Only task-owned fixtures, a loopback daemon, the Hub adapter source, "
            "and offline checks are used; no provider token or model call is read."
        ),
    )
    parser.add_argument(
        "--vibemux-root",
        type=Path,
        default=Path(__file__).resolve().parent.parent,
        help="VibeMux checkout (default: this script's repository)",
    )
    parser.add_argument("--hub-root", type=Path, help="Personal Dev Hub checkout")
    parser.add_argument(
        "--vibemuxctl",
        type=Path,
        help="vibemuxctl binary (default: <vibemux-root>/target/debug/vibemuxctl)",
    )
    parser.add_argument(
        "--state-root",
        type=Path,
        help="task-owned runtime root (default: a new temporary directory)",
    )
    parser.add_argument(
        "--build-state-root",
        type=Path,
        help="managed build root used by cases that compile (default: the frame state root)",
    )
    parser.add_argument(
        "--cases",
        default=",".join(CASE_IDS),
        help="comma-separated case ids (default: all known ids)",
    )
    parser.add_argument("--report", type=Path, help="write the JSON report to this path")
    parser.add_argument("--timeout-seconds", type=float, default=180.0)
    parser.add_argument(
        "--hub-gates", action="store_true", help="run the Hub check/test/artifact gates"
    )
    parser.add_argument("--keep", action="store_true", help="keep the task-owned runtime root")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    arguments = _parse_args(argv)
    vibemux_root = arguments.vibemux_root.resolve()
    hub_root = arguments.hub_root.resolve() if arguments.hub_root else None
    vibemuxctl = (
        arguments.vibemuxctl.resolve()
        if arguments.vibemuxctl
        else vibemux_root / "target" / "debug" / "vibemuxctl"
    )
    created_root = arguments.state_root is None
    run_root = (
        Path(tempfile.mkdtemp(prefix="vmx-cloud-verify-")).resolve()
        if created_root
        else arguments.state_root.resolve()
    )
    run_root.mkdir(parents=True, exist_ok=True)
    build_state_root = (
        arguments.build_state_root.resolve()
        if arguments.build_state_root
        else run_root / "build-storage"
    )
    selected = [item.strip() for item in arguments.cases.split(",") if item.strip()]
    unknown = [item for item in selected if item not in CASE_IDS]
    if unknown:
        print(f"unknown case ids: {', '.join(unknown)}", file=sys.stderr)
        return 2

    free_before = _free_bytes(run_root)
    bytes_before = _directory_bytes(run_root)
    bytes_after = bytes_before
    free_after = free_before
    cases: list[dict[str, Any]] = []
    try:
        if "C01" in selected:
            if hub_root is None:
                cases.append(
                    _case(
                        "C01",
                        "blocked",
                        evidence_class="actual_daemon_hub_adapter",
                        detail="--hub-root is required for a real Hub adapter run",
                    )
                )
            elif not vibemuxctl.is_file():
                cases.append(
                    _case(
                        "C01",
                        "blocked",
                        evidence_class="actual_daemon_hub_adapter",
                        detail=f"vibemuxctl not built at {vibemuxctl}",
                    )
                )
            else:
                cases.append(
                    _run_c01(
                        vibemuxctl=vibemuxctl,
                        hub_root=hub_root,
                        run_root=run_root,
                        timeout=arguments.timeout_seconds,
                    )
                )
        if "A1" in selected:
            cases.append(_run_a1(vibemux_root=vibemux_root, timeout=arguments.timeout_seconds))
        if "A2" in selected:
            cases.append(
                _run_a2(
                    vibemux_root=vibemux_root,
                    state_root=run_root / "build-storage",
                    timeout=arguments.timeout_seconds,
                )
            )
        if "C11-SLOTS" in selected:
            cases.append(
                _run_c11_slots(
                    vibemux_root=vibemux_root,
                    state_root=run_root / "build-storage",
                    timeout=arguments.timeout_seconds,
                )
            )
        if "C13-LINEAGE" in selected:
            cases.append(
                _run_c13_lineage(
                    vibemux_root=vibemux_root,
                    build_state_root=build_state_root,
                    timeout=arguments.timeout_seconds,
                )
            )
        if "C04-EVENTS" in selected:
            if hub_root is None:
                cases.append(
                    _case(
                        "C04-EVENTS",
                        "blocked",
                        evidence_class="event_pagination_contracts",
                        detail="--hub-root is required for the Hub cursor consumer",
                    )
                )
            else:
                cases.append(
                    _run_c04_events(
                        vibemux_root=vibemux_root,
                        build_state_root=build_state_root,
                        hub_root=hub_root,
                        timeout=arguments.timeout_seconds,
                    )
                )
        if "C02-PLAN" in selected:
            if hub_root is None:
                cases.append(
                    _case(
                        "C02-PLAN",
                        "blocked",
                        evidence_class="planning_contract_fixture",
                        detail="--hub-root is required for the Hub planning validator",
                    )
                )
            else:
                cases.append(
                    _run_c02_plan(
                        vibemux_root=vibemux_root,
                        build_state_root=build_state_root,
                        hub_root=hub_root,
                        timeout=arguments.timeout_seconds,
                    )
                )
        if "C04-UI" in selected:
            if hub_root is None:
                cases.append(
                    _case(
                        "C04-UI",
                        "blocked",
                        evidence_class="hub_event_ui",
                        detail="--hub-root is required for the Workspaces view",
                    )
                )
            else:
                cases.append(_run_c04_ui(hub_root=hub_root, timeout=arguments.timeout_seconds))
        if "C07-HUB" in selected:
            if hub_root is None:
                cases.append(
                    _case(
                        "C07-HUB",
                        "blocked",
                        evidence_class="hub_restart_recheck",
                        detail="--hub-root is required for the Hub restart test",
                    )
                )
            else:
                cases.append(_run_c07_hub(hub_root=hub_root, timeout=arguments.timeout_seconds))
        if "C05-CANCEL" in selected:
            cases.append(
                _run_c05_cancel(
                    vibemux_root=vibemux_root,
                    build_state_root=build_state_root,
                    timeout=arguments.timeout_seconds,
                )
            )
        if "C08-HANDOFF" in selected:
            cases.append(
                _run_c08_handoff(
                    vibemux_root=vibemux_root,
                    build_state_root=build_state_root,
                    timeout=arguments.timeout_seconds,
                )
            )
        if "C10-MCP" in selected:
            if hub_root is None:
                cases.append(
                    _case(
                        "C10-MCP",
                        "blocked",
                        evidence_class="metadata_services_fixture",
                        detail="--hub-root is required for the Hub MCP service",
                    )
                )
            else:
                cases.append(
                    _run_c10_mcp(
                        vibemux_root=vibemux_root,
                        build_state_root=build_state_root,
                        hub_root=hub_root,
                        timeout=arguments.timeout_seconds,
                    )
                )
        if "C09" in selected:
            if hub_root is None:
                cases.append(
                    _case(
                        "C09",
                        "blocked",
                        evidence_class="skill_ref_resolution",
                        detail="--hub-root is required for the skill resolver",
                    )
                )
            else:
                cases.append(
                    _run_c09_skills(
                        hub_root=hub_root,
                        timeout=arguments.timeout_seconds,
                    )
                )
        if "C11-MERGE" in selected:
            cases.append(
                _run_c11_merge(
                    vibemux_root=vibemux_root,
                    build_state_root=build_state_root,
                    timeout=arguments.timeout_seconds,
                )
            )
        if "C14" in selected:
            cases.append(
                _run_c14_fusion(
                    vibemux_root=vibemux_root,
                    build_state_root=build_state_root,
                    timeout=arguments.timeout_seconds,
                )
            )
        if "C06" in selected:
            cases.append(
                _run_c06(
                    vibemux_root=vibemux_root,
                    build_state_root=build_state_root,
                    timeout=arguments.timeout_seconds,
                )
            )
        if "C07-RESTART" in selected:
            if not vibemuxctl.is_file():
                cases.append(
                    _case(
                        "C07-RESTART",
                        "blocked",
                        evidence_class="daemon_restart_store_reuse",
                        detail=f"vibemuxctl not built at {vibemuxctl}",
                    )
                )
            else:
                cases.append(
                    _run_c07_restart(
                        vibemuxctl=vibemuxctl,
                        run_root=run_root,
                        timeout=arguments.timeout_seconds,
                    )
                )
        if "HUB-GATES" in selected:
            if not arguments.hub_gates:
                cases.append(
                    _case(
                        "HUB-GATES",
                        "not_run",
                        evidence_class="actual_hub_tests",
                        detail="pass --hub-gates to execute the Hub checks",
                    )
                )
            elif hub_root is None:
                cases.append(
                    _case(
                        "HUB-GATES",
                        "blocked",
                        evidence_class="actual_hub_tests",
                        detail="--hub-root is required",
                    )
                )
            else:
                cases.append(_run_hub_gates(hub_root=hub_root, timeout=arguments.timeout_seconds))
        for case_id in selected:
            if case_id in NOT_RUN_REASONS:
                cases.append(
                    _case(
                        case_id,
                        "not_run",
                        evidence_class="not_executed",
                        detail=NOT_RUN_REASONS[case_id],
                    )
                )
        bytes_after = _directory_bytes(run_root)
        free_after = _free_bytes(run_root)
    finally:
        if created_root and not arguments.keep:
            shutil.rmtree(run_root, ignore_errors=True)

    report = {
        "schema_version": SCHEMA_VERSION,
        "generated_at": _now(),
        "environment": {
            "os": sys.platform,
            "runtime_mode": "fixture",
            "rust": _version(["cargo", "--version"]),
            "node": _version(["node", "--version"]),
            "vibemux_root": str(vibemux_root),
            "hub_root": str(hub_root) if hub_root else None,
        },
        "cases": cases,
        "resources": {
            "state_root": str(run_root),
            "before_bytes": bytes_before,
            "after_bytes": bytes_after,
            "peak_bytes": None,
            "filesystem_free_before": free_before,
            "filesystem_free_after": free_after,
        },
    }
    text = json.dumps(report, indent=2) + "\n"
    if arguments.report:
        arguments.report.parent.mkdir(parents=True, exist_ok=True)
        arguments.report.write_text(text, encoding="utf-8")
    print(text)
    return 1 if any(case["status"] == "failed" for case in cases) else 0


def _version(command: list[str]) -> str | None:
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=10, check=False)
    except (OSError, subprocess.TimeoutExpired):
        return None
    return result.stdout.strip() or None


if __name__ == "__main__":
    raise SystemExit(main())
