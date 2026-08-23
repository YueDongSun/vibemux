from __future__ import annotations

import json
from dataclasses import fields
from pathlib import Path
from typing import Any

from vibemux.models import (
    RUN_TRANSITIONS,
    TASK_TRANSITIONS,
    Event,
    RunCompletionAuthority,
)
from vibemux.terminal import MOCK_TERMINAL_SCHEMA_VERSION, Pane, SendReceipt
from vibemux.workspace import CleanupPlan, WorktreeInspection, WorktreeRecord

FIXTURE_PATH = Path(__file__).parent / "fixtures" / "python_reference" / "python_contract_v1.json"


def field_names(model: type[Any]) -> list[str]:
    return [field.name for field in fields(model)]


def transition_fixture(transitions: dict[Any, set[Any]]) -> dict[str, list[str]]:
    return {
        source.value: sorted(target.value for target in targets)
        for source, targets in transitions.items()
    }


def test_python_reference_contract_fixture_matches_implementation() -> None:
    fixture = json.loads(FIXTURE_PATH.read_text(encoding="utf-8"))
    assert fixture["schema_version"] == 1
    assert fixture["task_transitions"] == transition_fixture(TASK_TRANSITIONS)
    assert fixture["run_transitions"] == transition_fixture(RUN_TRANSITIONS)
    assert fixture["run_completion_authorities"] == sorted(
        authority.value for authority in RunCompletionAuthority
    )
    assert fixture["event_fields"] == field_names(Event)
    assert fixture["worktree_record_fields"] == field_names(WorktreeRecord)
    assert fixture["worktree_inspection_fields"] == field_names(WorktreeInspection)
    assert fixture["cleanup_plan_fields"] == field_names(CleanupPlan)
    assert fixture["terminal_inventory"]["schema_version"] == MOCK_TERMINAL_SCHEMA_VERSION
    assert fixture["terminal_inventory"]["top_level_fields"] == [
        "schema_version",
        "panes",
    ]
    assert fixture["terminal_inventory"]["pane_fields"] == field_names(Pane)
    assert fixture["send_receipt_fields"] == field_names(SendReceipt)
