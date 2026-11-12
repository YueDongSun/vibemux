#!/usr/bin/env python3
"""Managed build storage for VibeMux.

Every heavy Rust build runs through a fixed build slot so that repeated
validation never grows a per-conversation ``target`` copy. The tool keeps
four entry points:

``inventory``
    Report registered slots, their owners, activity leases, directory sizes,
    and the filesystem free space. Unknown directories are reported, never
    claimed.

``run``
    Acquire a slot lease plus one global build token, pin
    ``CARGO_TARGET_DIR`` to the slot, and monitor the owned process tree.
    Touching the slot budget or the reserve floor stops the owned tree and
    returns a non-zero resource-exhausted exit code.

``plan-clean``
    Write a reviewable deletion plan for idle, task-owned, reproducible
    ``target`` directories. Unknown or retained content is excluded.

``apply-clean``
    Re-verify a plan under the same activity leases and delete only the
    confirmed entries.

The interface is intentionally small. A future VibeMux scheduler may consume
the same JSON plans and measurements instead of duplicating this logic.

Python 3.12+, standard library only. Works on Windows and POSIX.
"""

from __future__ import annotations

import argparse
import contextlib
import dataclasses
import datetime as _datetime
import json
import os
import shutil
import signal
import stat
import subprocess
import sys
import time
from collections.abc import Sequence
from pathlib import Path
from typing import Any, Final

SCHEMA_VERSION: Final = 1
EXIT_USAGE: Final = 2
EXIT_RESOURCE_EXHAUSTED: Final = 75

_GIB: Final = 1024**3
DEFAULT_BUDGET_BYTES: Final = 12 * _GIB
DEFAULT_RESERVE_BYTES: Final = 5 * _GIB
DEFAULT_MAX_ACTIVE: Final = 1
DEFAULT_POLL_SECONDS: Final = 2.0
DEFAULT_LEASE_TIMEOUT_SECONDS: Final = 600.0
DEFAULT_RETAINED: Final = ("install", "evidence", "owner.json")

_TARGET_FLAGS: Final = ("--target-dir",)


class StorageError(Exception):
    """A user-facing failure with a stable, machine-readable reason."""

    def __init__(self, reason: str, detail: str) -> None:
        super().__init__(f"{reason}: {detail}")
        self.reason = reason
        self.detail = detail


# --------------------------------------------------------------------------
# Small helpers


def _now() -> str:
    return _datetime.datetime.now(tz=_datetime.UTC).isoformat()


def _default_state_root() -> Path:
    override = os.environ.get("VIBEMUX_BUILD_STATE_ROOT")
    if override:
        return Path(override).expanduser()
    if os.name == "nt":
        base = os.environ.get("LOCALAPPDATA")
        if not base:
            raise StorageError("state_root_unresolved", "LOCALAPPDATA is not set")
        return Path(base) / "VibeMux" / "build-storage"
    if sys.platform == "darwin":
        return Path.home() / "Library" / "Caches" / "vibemux" / "build-storage"
    xdg = os.environ.get("XDG_CACHE_HOME")
    return (Path(xdg) if xdg else Path.home() / ".cache") / "vibemux" / "build-storage"


def _resolve_existing_or_parent(path: Path) -> Path:
    """Resolve symlinks in the longest existing prefix, then append the rest."""
    path = path.expanduser()
    missing: list[str] = []
    current = path
    while not current.exists():
        missing.append(current.name)
        if current.parent == current:
            break
        current = current.parent
    resolved = current.resolve(strict=True)
    for name in reversed(missing):
        resolved = resolved / name
    return resolved


def _is_within(candidate: Path, parent: Path) -> bool:
    try:
        candidate.relative_to(parent)
    except ValueError:
        return False
    return True


def _validate_slot_name(name: str) -> str:
    allowed = set("abcdefghijklmnopqrstuvwxyz0123456789._-")
    if not name or len(name) > 64 or name[0] not in allowed or set(name) - allowed:
        raise StorageError("invalid_slot_name", f"unacceptable slot name: {name!r}")
    if name in {".", ".."} or ".." in name:
        raise StorageError("invalid_slot_name", f"unacceptable slot name: {name!r}")
    return name


@dataclasses.dataclass(frozen=True)
class Layout:
    state_root: Path
    slots: Path
    measurements: Path
    plans: Path
    locks: Path

    @classmethod
    def for_root(cls, root: Path) -> Layout:
        root = _resolve_existing_or_parent(root)
        return cls(
            state_root=root,
            slots=root / "slots",
            measurements=root / "measurements",
            plans=root / "plans",
            locks=root / "locks",
        )

    def ensure(self) -> None:
        for directory in (self.slots, self.measurements, self.plans, self.locks):
            directory.mkdir(parents=True, exist_ok=True)

    def slot_dir(self, slot: str) -> Path:
        return self.slots / _validate_slot_name(slot)

    def target_dir(self, slot: str) -> Path:
        return self.slot_dir(slot) / "target"


# --------------------------------------------------------------------------
# Activity leases


class FileLease:
    """An OS advisory lock released automatically when the process dies."""

    def __init__(self, path: Path) -> None:
        self.path = path
        self._handle: Any | None = None

    @property
    def held(self) -> bool:
        return self._handle is not None

    def try_acquire(self) -> bool:
        if self._handle is not None:
            return True
        self.path.parent.mkdir(parents=True, exist_ok=True)
        handle = open(self.path, "a+b")
        try:
            if os.name == "nt":  # pragma: no cover - exercised on Windows CI
                import msvcrt

                handle.seek(0)
                msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)  # type: ignore[attr-defined]
            else:
                import fcntl

                fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            handle.close()
            return False
        self._handle = handle
        return True

    def release(self) -> None:
        handle = self._handle
        if handle is None:
            return
        self._handle = None
        try:
            if os.name == "nt":  # pragma: no cover - exercised on Windows CI
                import msvcrt

                handle.seek(0)
                msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)  # type: ignore[attr-defined]
            else:
                import fcntl

                fcntl.flock(handle.fileno(), fcntl.LOCK_UN)
        finally:
            handle.close()

    def __enter__(self) -> FileLease:
        if not self.try_acquire():
            raise StorageError("lease_busy", f"lease already held: {self.path}")
        return self

    def __exit__(self, *_exc: object) -> None:
        self.release()


def _slot_lease(layout: Layout, slot: str) -> FileLease:
    return FileLease(layout.slot_dir(slot) / ".lease.lock")


def _build_token(layout: Layout, index: int) -> FileLease:
    return FileLease(layout.locks / f"build-token-{index}.lock")


def _acquire_build_token(layout: Layout, max_active: int, timeout_seconds: float) -> FileLease:
    deadline = time.monotonic() + timeout_seconds
    while True:
        for index in range(max_active):
            token = _build_token(layout, index)
            if token.try_acquire():
                return token
        if time.monotonic() >= deadline:
            raise StorageError(
                "build_slots_busy",
                f"no build token became free within {timeout_seconds:.0f}s "
                f"(max_active={max_active})",
            )
        time.sleep(min(0.25, max(0.0, deadline - time.monotonic())))


# --------------------------------------------------------------------------
# Ownership manifests


@dataclasses.dataclass
class SlotOwner:
    slot: str
    tasks: list[str]
    created_at: str
    retained: list[str]
    note: str

    def to_json(self) -> dict[str, Any]:
        return {
            "schema_version": SCHEMA_VERSION,
            "slot": self.slot,
            "tasks": list(self.tasks),
            "created_at": self.created_at,
            "retained": list(self.retained),
            "note": self.note,
        }

    @classmethod
    def from_json(cls, value: object) -> SlotOwner:
        if not isinstance(value, dict) or value.get("schema_version") != SCHEMA_VERSION:
            raise StorageError("owner_manifest_invalid", "unsupported owner manifest")
        slot = value.get("slot")
        tasks = value.get("tasks")
        created_at = value.get("created_at")
        retained = value.get("retained", [])
        note = value.get("note", "")
        if (
            not isinstance(slot, str)
            or not isinstance(tasks, list)
            or not all(isinstance(item, str) for item in tasks)
            or not isinstance(created_at, str)
            or not isinstance(retained, list)
            or not all(isinstance(item, str) for item in retained)
            or not isinstance(note, str)
        ):
            raise StorageError("owner_manifest_invalid", "malformed owner manifest")
        return cls(
            slot=slot,
            tasks=list(tasks),
            created_at=created_at,
            retained=list(retained),
            note=note,
        )


def _owner_path(layout: Layout, slot: str) -> Path:
    return layout.slot_dir(slot) / "owner.json"


def _read_owner(layout: Layout, slot: str) -> SlotOwner | None:
    path = _owner_path(layout, slot)
    if not path.is_file():
        return None
    try:
        return SlotOwner.from_json(json.loads(path.read_text(encoding="utf-8")))
    except (OSError, json.JSONDecodeError) as error:
        raise StorageError("owner_manifest_unreadable", str(error)) from error


def _write_owner(layout: Layout, owner: SlotOwner) -> None:
    path = _owner_path(layout, owner.slot)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(".json.tmp")
    temporary.write_text(json.dumps(owner.to_json(), indent=2) + "\n", encoding="utf-8")
    os.replace(temporary, path)


def _ensure_owner(
    layout: Layout,
    slot: str,
    task_id: str,
    *,
    adopt_unknown: bool,
    retained: Sequence[str],
    note: str,
) -> SlotOwner:
    """Register the slot for ``task_id`` or refuse unknown ownership.

    A non-empty slot directory without an ``owner.json`` may belong to another
    task. It is never claimed implicitly; ``--adopt`` is the explicit opt-in.
    """

    if not task_id:
        raise StorageError("task_id_required", "pass --task-id or VIBEMUX_TASK_ID")
    layout.slot_dir(slot).mkdir(parents=True, exist_ok=True)
    owner = _read_owner(layout, slot)
    if owner is None:
        target = layout.target_dir(slot)
        non_empty = target.is_dir() and any(target.iterdir())
        if non_empty and not adopt_unknown:
            raise StorageError(
                "slot_unowned",
                f"{target} already contains output without owner.json; "
                "rerun with --adopt only if this task owns it",
            )
        owner = SlotOwner(
            slot=slot,
            tasks=[task_id],
            created_at=_now(),
            retained=list(retained) or list(DEFAULT_RETAINED),
            note=note,
        )
        _write_owner(layout, owner)
        return owner
    if task_id not in owner.tasks:
        owner.tasks.append(task_id)
        _write_owner(layout, owner)
    return owner


# --------------------------------------------------------------------------
# Storage observation


@dataclasses.dataclass(frozen=True)
class DirectorySize:
    logical_bytes: int
    allocated_bytes: int | None

    def to_json(self) -> dict[str, int | None]:
        return {
            "logical_bytes": self.logical_bytes,
            "allocated_bytes": self.allocated_bytes,
        }


def _directory_size(path: Path) -> DirectorySize:
    logical = 0
    allocated = 0
    allocated_known = True
    if not path.exists():
        return DirectorySize(0, 0 if allocated_known else None)
    for root, dirs, files in os.walk(path, followlinks=False):
        for name in [*dirs, *files]:
            entry = Path(root) / name
            try:
                info = entry.lstat()
            except OSError:
                allocated_known = False
                continue
            if stat.S_ISLNK(info.st_mode):
                continue
            logical += info.st_size
            blocks = getattr(info, "st_blocks", None)
            if blocks is None:
                allocated_known = False
            else:
                allocated += blocks * 512
    return DirectorySize(logical, allocated if allocated_known else None)


def _free_bytes(path: Path) -> int:
    return shutil.disk_usage(path).free


def _slot_report(layout: Layout, slot: str) -> dict[str, Any]:
    owner = _read_owner(layout, slot)
    lease = _slot_lease(layout, slot)
    active = not lease.try_acquire()
    if not active:
        lease.release()
    target = layout.target_dir(slot)
    size = _directory_size(target)
    return {
        "slot": slot,
        "path": str(target),
        "owner": owner.to_json() if owner else None,
        "registered": owner is not None,
        "active": active,
        "target": size.to_json(),
    }


def _known_slots(layout: Layout) -> list[str]:
    if not layout.slots.is_dir():
        return []
    return sorted(
        entry.name
        for entry in layout.slots.iterdir()
        if entry.is_dir() and not entry.name.startswith(".")
    )


# --------------------------------------------------------------------------
# Inventory


def inventory(layout: Layout, *, repo_root: Path, deep: bool) -> dict[str, Any]:
    layout.ensure()
    slots = [_slot_report(layout, slot) for slot in _known_slots(layout)]
    unknown = [entry for entry in slots if not entry["registered"]]
    external: dict[str, Any] = {}
    env_target = os.environ.get("CARGO_TARGET_DIR")
    if env_target:
        external["CARGO_TARGET_DIR"] = env_target
    default_target = repo_root / "target"
    if default_target.exists():
        size = _directory_size(default_target) if deep else None
        external["repo_target"] = {
            "path": str(default_target),
            "measured": size.to_json() if size else None,
            "note": None if deep else "pass --deep to measure",
        }
    return {
        "schema_version": SCHEMA_VERSION,
        "state_root": str(layout.state_root),
        "filesystem_free_bytes": _free_bytes(layout.state_root),
        "slots": slots,
        "unknown_slots": [entry["slot"] for entry in unknown],
        "external_targets": external,
    }


# --------------------------------------------------------------------------
# Managed run


def _find_explicit_target_dir(command: Sequence[str]) -> str | None:
    for index, argument in enumerate(command):
        for flag in _TARGET_FLAGS:
            if argument == flag:
                if index + 1 < len(command):
                    return command[index + 1]
                raise StorageError("target_dir_flag_incomplete", f"{flag} needs a value")
            prefix = f"{flag}="
            if argument.startswith(prefix):
                return argument[len(prefix) :]
    return None


def _validate_command_target(command: Sequence[str], slot_target: Path) -> None:
    explicit = _find_explicit_target_dir(command)
    if explicit is None:
        return
    resolved = _resolve_existing_or_parent(Path(explicit))
    if resolved != slot_target:
        raise StorageError(
            "target_dir_override",
            f"command overrides the managed target with {resolved}; expected {slot_target}",
        )


def _terminate_tree(process: subprocess.Popen[bytes], grace_seconds: float = 10.0) -> None:
    if process.poll() is not None:
        return
    if os.name == "nt":  # pragma: no cover - exercised on Windows CI
        subprocess.run(
            ["taskkill", "/PID", str(process.pid), "/T", "/F"],
            check=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    else:
        with contextlib.suppress(ProcessLookupError):
            os.killpg(process.pid, signal.SIGTERM)
    try:
        process.wait(timeout=grace_seconds)
        return
    except subprocess.TimeoutExpired:
        pass
    if os.name == "nt":  # pragma: no cover - exercised on Windows CI
        subprocess.run(
            ["taskkill", "/PID", str(process.pid), "/T", "/F"],
            check=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    else:
        with contextlib.suppress(ProcessLookupError):
            os.killpg(process.pid, signal.SIGKILL)
    with contextlib.suppress(subprocess.TimeoutExpired):
        process.wait(timeout=grace_seconds)


def _write_measurement(layout: Layout, measurement: dict[str, Any]) -> Path:
    layout.measurements.mkdir(parents=True, exist_ok=True)
    stamp = _datetime.datetime.now(tz=_datetime.UTC).strftime("%Y%m%dT%H%M%S%fZ")
    path = layout.measurements / f"{stamp}-{measurement['slot']}.json"
    temporary = path.with_suffix(".json.tmp")
    temporary.write_text(json.dumps(measurement, indent=2) + "\n", encoding="utf-8")
    os.replace(temporary, path)
    return path


def run_managed(
    layout: Layout,
    *,
    slot: str,
    task_id: str,
    command: Sequence[str],
    cwd: Path,
    budget_bytes: int,
    reserve_bytes: int,
    max_active: int,
    lease_timeout: float,
    poll_seconds: float,
    allow_incremental: bool,
    adopt_unknown: bool,
    retained: Sequence[str],
    note: str,
) -> tuple[int, dict[str, Any]]:
    if not command:
        raise StorageError("command_required", "pass the command after --")
    layout.ensure()
    slot_target = layout.target_dir(slot)
    _validate_command_target(command, slot_target)
    owner = _ensure_owner(
        layout,
        slot,
        task_id,
        adopt_unknown=adopt_unknown,
        retained=retained,
        note=note,
    )
    before_size = _directory_size(slot_target)
    free_before = _free_bytes(layout.state_root)
    if free_before < reserve_bytes:
        raise StorageError(
            "reserve_breached",
            f"{free_before} bytes free is below the {reserve_bytes}-byte reserve",
        )
    existing = before_size.allocated_bytes
    if existing is not None and existing > budget_bytes:
        raise StorageError(
            "budget_already_exceeded",
            f"slot already holds {existing} bytes, above the {budget_bytes}-byte budget",
        )

    started_monotonic = time.monotonic()
    started_at = _now()
    token = _acquire_build_token(layout, max_active, lease_timeout)
    lease = _slot_lease(layout, slot)
    if not lease.try_acquire():
        token.release()
        raise StorageError(
            "slot_busy",
            f"slot {slot!r} is held by another build; wait or use another slot",
        )
    process: subprocess.Popen[bytes] | None = None
    status = "not_started"
    exit_code = EXIT_RESOURCE_EXHAUSTED
    peak = before_size
    stop_reason: str | None = None
    metadata_path = layout.slot_dir(slot) / "lease.json"
    try:
        environment = dict(os.environ)
        environment["CARGO_TARGET_DIR"] = str(slot_target)
        if not allow_incremental:
            environment.setdefault("CARGO_INCREMENTAL", "0")
        metadata_path.write_text(
            json.dumps(
                {
                    "schema_version": SCHEMA_VERSION,
                    "pid": os.getpid(),
                    "task_id": task_id,
                    "slot": slot,
                    "target_dir": str(slot_target),
                    "command": list(command),
                    "started_at": started_at,
                },
                indent=2,
            )
            + "\n",
            encoding="utf-8",
        )
        if os.name == "nt":  # pragma: no cover - exercised on Windows CI
            process = subprocess.Popen(
                list(command),
                cwd=cwd,
                env=environment,
                stdin=subprocess.DEVNULL,
                creationflags=subprocess.CREATE_NEW_PROCESS_GROUP,  # type: ignore[attr-defined]
            )
        else:
            process = subprocess.Popen(
                list(command),
                cwd=cwd,
                env=environment,
                stdin=subprocess.DEVNULL,
                start_new_session=True,
            )
        status = "running"
        while True:
            code = process.poll()
            current = _directory_size(slot_target)
            if current.logical_bytes > peak.logical_bytes or (current.allocated_bytes or 0) > (
                peak.allocated_bytes or 0
            ):
                peak = current
            free_now = _free_bytes(layout.state_root)
            allocated = current.allocated_bytes
            if allocated is not None and allocated > budget_bytes:
                stop_reason = "budget_exceeded"
            elif free_now < reserve_bytes:
                stop_reason = "reserve_breached"
            if stop_reason is not None:
                _terminate_tree(process)
                process.wait()
                status = "resource_exhausted"
                exit_code = EXIT_RESOURCE_EXHAUSTED
                break
            if code is not None:
                exit_code = code if code >= 0 else 128 - code
                status = "succeeded" if code == 0 else "failed"
                break
            time.sleep(max(0.05, poll_seconds))
    finally:
        if process is not None and process.poll() is None:
            _terminate_tree(process)
            status = "resource_exhausted"
            exit_code = EXIT_RESOURCE_EXHAUSTED
        after_size = _directory_size(slot_target)
        free_after = _free_bytes(layout.state_root)
        duration_ms = int((time.monotonic() - started_monotonic) * 1000)
        measurement = {
            "schema_version": SCHEMA_VERSION,
            "slot": slot,
            "tasks": owner.tasks,
            "task_id": task_id,
            "command": list(command),
            "cwd": str(cwd),
            "target_dir": str(slot_target),
            "started_at": started_at,
            "ended_at": _now(),
            "duration_ms": duration_ms,
            "status": status,
            "stop_reason": stop_reason,
            "exit_code": exit_code,
            "budget_bytes": budget_bytes,
            "reserve_bytes": reserve_bytes,
            "before": before_size.to_json(),
            "after": after_size.to_json(),
            "peak": peak.to_json(),
            "filesystem_free_before": free_before,
            "filesystem_free_after": free_after,
            "filesystem_free_delta": free_after - free_before,
        }
        measurement_path = _write_measurement(layout, measurement)
        measurement["measurement_path"] = str(measurement_path)
        with contextlib.suppress(OSError):
            metadata_path.unlink()
        lease.release()
        token.release()
    return exit_code, measurement


# --------------------------------------------------------------------------
# Cleanup


def plan_clean(layout: Layout, *, task_id: str, slots: Sequence[str] | None) -> dict[str, Any]:
    layout.ensure()
    entries: list[dict[str, Any]] = []
    skipped: list[dict[str, str]] = []
    candidates = [_validate_slot_name(slot) for slot in slots] if slots else _known_slots(layout)
    for slot in candidates:
        owner = _read_owner(layout, slot)
        if owner is None:
            skipped.append({"slot": slot, "reason": "unknown_ownership"})
            continue
        if task_id not in owner.tasks:
            skipped.append({"slot": slot, "reason": "not_task_owned"})
            continue
        target = layout.target_dir(slot)
        if not target.is_dir():
            skipped.append({"slot": slot, "reason": "no_target"})
            continue
        lease = _slot_lease(layout, slot)
        if not lease.try_acquire():
            skipped.append({"slot": slot, "reason": "active_lease"})
            continue
        lease.release()
        retained = {entry for entry in owner.retained if entry}
        if "target" in retained:
            skipped.append({"slot": slot, "reason": "target_retained"})
            continue
        size = _directory_size(target)
        entries.append(
            {
                "slot": slot,
                "path": str(target),
                "logical_bytes": size.logical_bytes,
                "allocated_bytes": size.allocated_bytes,
                "reason": "idle task-owned reproducible target",
            }
        )
    plan = {
        "schema_version": SCHEMA_VERSION,
        "created_at": _now(),
        "state_root": str(layout.state_root),
        "task_id": task_id,
        "entries": entries,
        "skipped": skipped,
        "reclaimable_logical_bytes": sum(entry["logical_bytes"] for entry in entries),
        "reclaimable_allocated_bytes": (
            sum(entry["allocated_bytes"] for entry in entries)
            if all(entry["allocated_bytes"] is not None for entry in entries)
            else None
        ),
    }
    layout.plans.mkdir(parents=True, exist_ok=True)
    stamp = _datetime.datetime.now(tz=_datetime.UTC).strftime("%Y%m%dT%H%M%S%fZ")
    path = layout.plans / f"{stamp}-clean.json"
    temporary = path.with_suffix(".json.tmp")
    temporary.write_text(json.dumps(plan, indent=2) + "\n", encoding="utf-8")
    os.replace(temporary, path)
    plan["plan_path"] = str(path)
    return plan


def _verify_plan_entry(layout: Layout, entry: object) -> tuple[str, Path]:
    if not isinstance(entry, dict):
        raise StorageError("plan_invalid", "entry is not an object")
    slot = entry.get("slot")
    raw_path = entry.get("path")
    if not isinstance(slot, str) or not isinstance(raw_path, str):
        raise StorageError("plan_invalid", "entry lacks slot or path")
    _validate_slot_name(slot)
    slot_dir = layout.slot_dir(slot)
    expected = layout.target_dir(slot)
    candidate = Path(raw_path)
    if candidate.is_symlink():
        raise StorageError("path_escaped", f"{candidate} is a symlink")
    resolved = _resolve_existing_or_parent(candidate)
    expected_resolved = _resolve_existing_or_parent(expected)
    if resolved != expected_resolved:
        raise StorageError("path_escaped", f"{resolved} is not the managed target of slot {slot}")
    if not _is_within(resolved, layout.slots.resolve()):
        raise StorageError("path_escaped", f"{resolved} is outside {layout.slots}")
    if not resolved.is_dir():
        raise StorageError("path_missing", f"{resolved} does not exist")
    if slot_dir.is_symlink():
        raise StorageError("path_escaped", f"{slot_dir} is a symlink")
    return slot, resolved


def apply_clean(layout: Layout, plan_path: Path, *, task_id: str) -> dict[str, Any]:
    try:
        plan = json.loads(plan_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise StorageError("plan_unreadable", str(error)) from error
    if not isinstance(plan, dict) or plan.get("schema_version") != SCHEMA_VERSION:
        raise StorageError("plan_invalid", "unsupported plan schema")
    if plan.get("task_id") != task_id:
        raise StorageError(
            "plan_task_mismatch",
            f"plan is for {plan.get('task_id')!r}, not {task_id!r}",
        )
    entries = plan.get("entries")
    if not isinstance(entries, list):
        raise StorageError("plan_invalid", "plan has no entries list")
    free_before = _free_bytes(layout.state_root)
    results: list[dict[str, Any]] = []
    for entry in entries:
        result: dict[str, Any] = {}
        slot: str | None = None
        lease: FileLease | None = None
        try:
            slot, path = _verify_plan_entry(layout, entry)
            result = {"slot": slot, "path": str(path)}
            owner = _read_owner(layout, slot)
            if owner is None:
                raise StorageError("unknown_ownership", f"{slot} has no owner.json")
            if task_id not in owner.tasks:
                raise StorageError("not_task_owned", f"{slot} is not owned by {task_id}")
            lease = _slot_lease(layout, slot)
            if not lease.try_acquire():
                raise StorageError("active_lease", f"{slot} is in active use")
            size = _directory_size(path)
            shutil.rmtree(path)
            result.update(
                {
                    "status": "deleted",
                    "logical_bytes": size.logical_bytes,
                    "allocated_bytes": size.allocated_bytes,
                }
            )
        except StorageError as error:
            result.update({"status": "skipped", "reason": error.reason})
            result.setdefault("slot", slot)
        except OSError as error:
            result.update({"status": "failed", "reason": "delete_failed", "detail": str(error)})
            result.setdefault("slot", slot)
        finally:
            if lease is not None:
                lease.release()
        results.append(result)
    free_after = _free_bytes(layout.state_root)
    return {
        "schema_version": SCHEMA_VERSION,
        "applied_at": _now(),
        "plan_path": str(plan_path),
        "task_id": task_id,
        "results": results,
        "filesystem_free_before": free_before,
        "filesystem_free_after": free_after,
        "filesystem_free_delta": free_after - free_before,
    }


# --------------------------------------------------------------------------
# CLI


def _parse_bytes(value: str) -> int:
    normalized = value.strip().lower().replace("_", "")
    multipliers = {"k": 1024, "m": 1024**2, "g": 1024**3, "t": 1024**4}
    if normalized and normalized[-1] in multipliers:
        return int(float(normalized[:-1]) * multipliers[normalized[-1]])
    return int(normalized)


def _byte_notice(measurement: dict[str, Any]) -> str:
    if measurement["status"] == "resource_exhausted":
        return f"build stopped: {measurement['stop_reason']}"
    return (
        f"build {measurement['status']} (exit {measurement['exit_code']}, "
        f"{measurement['duration_ms']} ms)"
    )


def _build_parser(state_root: Path) -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="build_storage.py",
        description="Managed build slots, storage measurement, and owned cleanup.",
    )
    parser.add_argument(
        "--state-root",
        type=Path,
        default=state_root,
        help="build-storage root (default: VIBEMUX_BUILD_STATE_ROOT or the user cache)",
    )
    parser.add_argument("--json", action="store_true", help="print machine-readable JSON")
    subparsers = parser.add_subparsers(dest="verb", required=True)

    inventory_parser = subparsers.add_parser("inventory", help="report slots and free space")
    inventory_parser.add_argument(
        "--repo-root", type=Path, default=Path.cwd(), help="repository root for target reporting"
    )
    inventory_parser.add_argument(
        "--deep", action="store_true", help="measure the unmanaged repository target"
    )

    run_parser = subparsers.add_parser("run", help="run a command inside a managed slot")
    run_parser.add_argument("--slot", default="core", help="slot name (default: core)")
    run_parser.add_argument(
        "--task-id",
        default=os.environ.get("VIBEMUX_TASK_ID", ""),
        help="task identifier that owns the slot (default: VIBEMUX_TASK_ID)",
    )
    run_parser.add_argument(
        "--budget-bytes",
        type=_parse_bytes,
        default=_parse_bytes(
            os.environ.get("VIBEMUX_BUILD_BUDGET_BYTES", str(DEFAULT_BUDGET_BYTES))
        ),
        help="per-slot byte budget (accepts K/M/G/T suffixes)",
    )
    run_parser.add_argument(
        "--reserve-bytes",
        type=_parse_bytes,
        default=_parse_bytes(
            os.environ.get("VIBEMUX_BUILD_RESERVE_BYTES", str(DEFAULT_RESERVE_BYTES))
        ),
        help="minimum filesystem free space to preserve",
    )
    run_parser.add_argument(
        "--max-active",
        type=int,
        default=int(os.environ.get("VIBEMUX_BUILD_MAX_ACTIVE", DEFAULT_MAX_ACTIVE)),
        help="concurrent managed builds (default: 1)",
    )
    run_parser.add_argument(
        "--lease-timeout",
        type=float,
        default=DEFAULT_LEASE_TIMEOUT_SECONDS,
        help="seconds to wait for a build token",
    )
    run_parser.add_argument(
        "--poll-seconds", type=float, default=DEFAULT_POLL_SECONDS, help="storage sampling period"
    )
    run_parser.add_argument(
        "--allow-incremental",
        action="store_true",
        help="leave CARGO_INCREMENTAL unset for this managed call",
    )
    run_parser.add_argument(
        "--adopt",
        action="store_true",
        help="claim a non-empty slot that has no owner.json",
    )
    run_parser.add_argument(
        "--retain",
        action="append",
        default=[],
        help="slot-relative path that cleanup must preserve (repeatable)",
    )
    run_parser.add_argument("--note", default="", help="free-form slot note")
    run_parser.add_argument("--cwd", type=Path, default=Path.cwd())
    run_parser.add_argument(
        "command",
        nargs=argparse.REMAINDER,
        help="command to run after --",
    )

    plan_parser = subparsers.add_parser("plan-clean", help="plan owned idle target removal")
    plan_parser.add_argument(
        "--task-id",
        default=os.environ.get("VIBEMUX_TASK_ID", ""),
        help="task identifier that owns the slots",
    )
    plan_parser.add_argument("--slot", action="append", default=[], help="limit to a slot")

    apply_parser = subparsers.add_parser("apply-clean", help="apply a reviewed cleanup plan")
    apply_parser.add_argument("plan", type=Path)
    apply_parser.add_argument(
        "--task-id",
        default=os.environ.get("VIBEMUX_TASK_ID", ""),
        help="task identifier that owns the slots",
    )
    return parser


def _strip_remainder(command: Sequence[str]) -> list[str]:
    arguments = list(command)
    if arguments and arguments[0] == "--":
        arguments = arguments[1:]
    return arguments


def main(argv: Sequence[str] | None = None) -> int:
    state_root = _default_state_root()
    parser = _build_parser(state_root)
    arguments = parser.parse_args(argv)
    layout = Layout.for_root(arguments.state_root)
    try:
        if arguments.verb == "inventory":
            report = inventory(layout, repo_root=arguments.repo_root, deep=arguments.deep)
            if arguments.json:
                print(json.dumps(report, indent=2))
            else:
                print(f"state root: {report['state_root']}")
                print(f"free: {report['filesystem_free_bytes']} bytes")
                for slot in report["slots"]:
                    owner = slot["owner"]["tasks"] if slot["owner"] else "UNOWNED"
                    print(
                        f"  {slot['slot']}: active={slot['active']} owner={owner} "
                        f"logical={slot['target']['logical_bytes']} bytes"
                    )
                if report["unknown_slots"]:
                    print(f"unknown: {', '.join(report['unknown_slots'])}")
            return 0
        if arguments.verb == "run":
            command = _strip_remainder(arguments.command)
            code, measurement = run_managed(
                layout,
                slot=arguments.slot,
                task_id=arguments.task_id,
                command=command,
                cwd=arguments.cwd,
                budget_bytes=arguments.budget_bytes,
                reserve_bytes=arguments.reserve_bytes,
                max_active=arguments.max_active,
                lease_timeout=arguments.lease_timeout,
                poll_seconds=arguments.poll_seconds,
                allow_incremental=arguments.allow_incremental,
                adopt_unknown=arguments.adopt,
                retained=arguments.retain,
                note=arguments.note,
            )
            if arguments.json:
                print(json.dumps(measurement, indent=2))
            else:
                print(_byte_notice(measurement), file=sys.stderr)
            return code
        if arguments.verb == "plan-clean":
            if not arguments.task_id:
                raise StorageError("task_id_required", "pass --task-id or VIBEMUX_TASK_ID")
            plan = plan_clean(layout, task_id=arguments.task_id, slots=arguments.slot)
            if arguments.json:
                print(json.dumps(plan, indent=2))
            else:
                print(f"plan: {plan['plan_path']}")
                for entry in plan["entries"]:
                    print(f"  delete {entry['path']} ({entry['logical_bytes']} bytes)")
                for skipped in plan["skipped"]:
                    print(f"  skip {skipped['slot']}: {skipped['reason']}")
            return 0
        if arguments.verb == "apply-clean":
            if not arguments.task_id:
                raise StorageError("task_id_required", "pass --task-id or VIBEMUX_TASK_ID")
            report = apply_clean(layout, arguments.plan, task_id=arguments.task_id)
            if arguments.json:
                print(json.dumps(report, indent=2))
            else:
                for result in report["results"]:
                    print(f"  {result.get('slot')}: {result['status']} {result.get('reason', '')}")
                print(f"free delta: {report['filesystem_free_delta']} bytes")
            return 0
    except StorageError as error:
        print(f"build_storage: {error.reason}: {error.detail}", file=sys.stderr)
        return EXIT_USAGE
    return EXIT_USAGE


if __name__ == "__main__":
    raise SystemExit(main())
