"""Loads the evidence the daemon exported during the workflow tests and
checks it against the scenario claims.

The workflow integration tests write `<name>.json` files (Control status,
the full export, and every session inspection of one workflow) when the
runner sets the evidence directory. The checks read only those
daemon-produced records. A check never repairs or infers a missing field:
missing evidence fails the check.
"""

from __future__ import annotations

import json
from collections.abc import Callable, Iterable, Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .check_execution import CHECK_FAIL, CHECK_PASS, CheckOutcome, file_sha256

FLAGSHIP = "cooperate_flagship"
COMPARE_SELECTION = "compare_selection"
COMPARE_BOTH_FAIL = "compare_both_fail"
BOUNDED_REPAIR = "bounded_repair"
SHARE_WALKTHROUGH = "share_walkthrough"
PAUSE_INSPECT = "pause_inspect"
OPTIMIZER_CYCLE = "optimizer_cycle"

WORKFLOW_EVIDENCE = (
    FLAGSHIP,
    COMPARE_SELECTION,
    COMPARE_BOTH_FAIL,
    BOUNDED_REPAIR,
    SHARE_WALKTHROUGH,
    PAUSE_INSPECT,
)
VALUE_EVIDENCE = (OPTIMIZER_CYCLE,)
EXPECTED_EVIDENCE = WORKFLOW_EVIDENCE + VALUE_EVIDENCE

# The only evidence class an offline run may carry.
FIXTURE_EVIDENCE_CLASS = "fixture"


@dataclass(frozen=True)
class EvidenceFile:
    name: str
    path: Path
    sha256: str
    document: Mapping[str, Any]


@dataclass(frozen=True)
class EvidenceStore:
    files: Mapping[str, EvidenceFile]
    errors: Mapping[str, str]
    unexpected: tuple[str, ...]

    def artifacts(self, output_root: Path) -> list[dict[str, Any]]:
        return [
            {
                "name": item.name,
                "path": item.path.relative_to(output_root).as_posix(),
                "sha256": item.sha256,
            }
            for item in sorted(self.files.values(), key=lambda item: item.name)
        ]


def load_evidence(directory: Path) -> EvidenceStore:
    files: dict[str, EvidenceFile] = {}
    errors: dict[str, str] = {}
    present = sorted(path.stem for path in directory.glob("*.json")) if directory.is_dir() else []
    for name in EXPECTED_EVIDENCE:
        path = directory / f"{name}.json"
        if not path.is_file():
            errors[name] = "not written (the test did not reach its evidence point)"
            continue
        try:
            document = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
            errors[name] = f"unreadable: {error}"
            continue
        shape_error = _shape_error(name, document)
        if shape_error is not None:
            errors[name] = shape_error
            continue
        files[name] = EvidenceFile(name, path, file_sha256(path), document)
    unexpected = tuple(name for name in present if name not in EXPECTED_EVIDENCE)
    return EvidenceStore(files, errors, unexpected)


def _shape_error(name: str, document: Any) -> str | None:
    if not isinstance(document, dict) or document.get("schema_version") != 1:
        return "unknown evidence schema"
    if document.get("name") != name:
        return f"names {document.get('name')!r}, not {name!r}"
    required = (
        ("value",) if name in VALUE_EVIDENCE else ("workflow_id", "status", "export", "sessions")
    )
    for key in required:
        if key not in document:
            return f"lacks {key}"
    return None


# ----------------------------------------------------------------- checks --


class CheckFailed(Exception):
    """A claim the evidence does not support."""


def _require(condition: bool, reason: str) -> None:
    if not condition:
        raise CheckFailed(reason)


def _document(store: EvidenceStore, name: str) -> Mapping[str, Any]:
    item = store.files.get(name)
    if item is None:
        raise CheckFailed(f"evidence {name}: {store.errors.get(name, 'missing')}")
    return item.document


def _status(store: EvidenceStore, name: str) -> Mapping[str, Any]:
    status = _document(store, name)["status"]
    if not isinstance(status, dict):
        raise CheckFailed(f"{name}: status is not an object")
    _require(
        status.get("evidence_class") == FIXTURE_EVIDENCE_CLASS,
        f"{name}: evidence class {status.get('evidence_class')!r} is not fixture",
    )
    return status


def receipt_bodies(document: Mapping[str, Any], kind: str) -> list[Mapping[str, Any]]:
    """The exported receipt bodies of one kind, in export order."""
    bodies: list[Mapping[str, Any]] = []
    for item in document.get("export", []):
        if item.get("kind") != "receipt":
            continue
        receipt = item.get("body", {})
        if receipt.get("kind") == kind and isinstance(receipt.get("body"), dict):
            bodies.append(receipt["body"])
    return bodies


def _accepted_tasks(status: Mapping[str, Any], name: str) -> list[Mapping[str, Any]]:
    tasks = status.get("tasks", [])
    _require(bool(tasks), f"{name}: no tasks")
    for task in tasks:
        _require(
            task.get("progress") == "accepted" and isinstance(task.get("accepted_candidate"), str),
            f"{name}: task {task.get('task_key')} is {task.get('progress')}",
        )
    return list(tasks)


def _workers(status: Mapping[str, Any]) -> list[Mapping[str, Any]]:
    return [session for session in status.get("sessions", []) if session.get("role") == "worker"]


def _suites_passed(verification: Mapping[str, Any], suite_ids: Iterable[str]) -> None:
    suites = {suite.get("suite_id"): suite for suite in verification.get("suites", [])}
    for suite_id in suite_ids:
        suite = suites.get(suite_id)
        _require(suite is not None, f"verification lacks suite {suite_id}")
        assert suite is not None
        _require(
            suite.get("status") == "passed"
            and suite.get("tests_failed") == 0
            and isinstance(suite.get("tests_total"), int)
            and suite["tests_total"] > 0
            and suite.get("tests_passed") == suite["tests_total"],
            f"suite {suite_id} is {suite.get('status')} "
            f"({suite.get('tests_passed')}/{suite.get('tests_total')})",
        )
        command = suite.get("command")
        _require(
            isinstance(command, list) and any("run_verifier.mjs" in str(part) for part in command),
            f"suite {suite_id} was not run by the trusted verifier",
        )


def _integration_verification(
    document: Mapping[str, Any], integration: Mapping[str, Any]
) -> Mapping[str, Any]:
    matches = [
        body
        for body in receipt_bodies(document, "verification")
        if body.get("task_key") is None
        and body.get("subject_digest") == integration.get("integration_tree_digest")
    ]
    _require(len(matches) == 1, f"{len(matches)} verifications of the integration tree")
    return matches[0]


def check_flagship_two_sessions(store: EvidenceStore) -> str:
    status = _status(store, FLAGSHIP)
    _require(status.get("mode") == "cooperate", f"mode {status.get('mode')}")
    _require(status.get("phase") == "accepted", f"phase {status.get('phase')}")
    tasks = _accepted_tasks(status, FLAGSHIP)
    workers = _workers(status)
    _require(len(workers) == 2, f"{len(workers)} worker sessions")
    _require(
        len({worker["session_id"] for worker in workers}) == 2, "worker sessions are not distinct"
    )
    _require(len({worker["slot_id"] for worker in workers}) == 2, "workers share a slot")
    _require(
        {worker["task_key"] for worker in workers} == {task["task_key"] for task in tasks},
        "worker sessions do not cover both tracks",
    )
    by_task = {worker["task_key"]: worker for worker in workers}
    document = _document(store, FLAGSHIP)
    for task in tasks:
        candidates = [
            body["candidate"]
            for body in receipt_bodies(document, "candidate")
            if body.get("candidate", {}).get("candidate_digest") == task["accepted_candidate"]
        ]
        _require(bool(candidates), f"no candidate receipt for {task['task_key']}")
        _require(
            all(
                candidate.get("worker_session_id") == by_task[task["task_key"]]["session_id"]
                for candidate in candidates
            ),
            f"{task['task_key']}'s candidate came from another session",
        )
    modes = {inspect.get("interface_mode") for inspect in _document(store, FLAGSHIP)["sessions"]}
    return (
        f"two worker sessions on slots {sorted(by_task[key]['slot_id'] for key in by_task)} "
        f"each produced its track's accepted candidate; interface modes {sorted(modes)}"
    )


def check_flagship_bundle_delivered(store: EvidenceStore) -> str:
    status = _status(store, FLAGSHIP)
    document = _document(store, FLAGSHIP)
    candidates = {
        task["task_key"]: task["accepted_candidate"] for task in _accepted_tasks(status, FLAGSHIP)
    }
    bundles = receipt_bodies(document, "bundle")
    _require(len(bundles) >= 1, "no context bundle")
    bundle = bundles[0]
    _require(bundle.get("source_task") == "track_a", f"bundle source {bundle.get('source_task')}")
    _require(bundle.get("source", {}).get("kind") == "worker", "bundle source is not a worker")
    _require(
        bundle.get("snapshot_digest") == candidates.get("track_a"),
        "bundle does not cite track A's accepted snapshot",
    )
    audience = bundle.get("audience", [])
    _require(
        any(
            entry.get("kind") == "worker" and entry.get("task_key") == "track_b"
            for entry in audience
        ),
        "track B is not in the bundle audience",
    )
    _require(bool(bundle.get("source_refs")), "bundle has no source references")
    messages = {message["message_id"]: message for message in status.get("messages", [])}
    question = messages.get(bundle.get("request_message_id"))
    _require(question is not None, "the bundle answers no recorded question")
    assert question is not None
    _require(
        question.get("sender") == "worker:track_b"
        and question.get("recipient") == "worker:track_a",
        "the question is not from track B to track A",
    )
    answers = [
        message
        for message in messages.values()
        if message.get("reply_to") == question["message_id"]
    ]
    _require(len(answers) == 1, f"{len(answers)} answers to the question")
    answer = answers[0]
    _require(
        answer.get("sender") == "worker:track_a" and answer.get("recipient") == "worker:track_b",
        "the answer is not from track A to track B",
    )
    for message in (question, answer):
        _require(message.get("state") == "acknowledged", f"message state {message.get('state')}")
    return (
        f"bundle {bundle.get('bundle_id')} from track_a cites {len(bundle['source_refs'])} source "
        "refs at track A's accepted snapshot; question and answer acknowledged"
    )


def check_share_grant_acknowledged(store: EvidenceStore) -> str:
    status = _status(store, SHARE_WALKTHROUGH)
    _require(status.get("phase") == "accepted", f"phase {status.get('phase')}")
    grants = [
        message
        for message in status.get("messages", [])
        if message.get("kind") == "context_grant" and message.get("sender") == "operator"
    ]
    _require(len(grants) == 1, f"{len(grants)} operator grants")
    grant = grants[0]
    _require(
        grant.get("recipient") == "worker:track_b", f"grant recipient {grant.get('recipient')}"
    )
    _require(grant.get("state") == "acknowledged", f"grant state {grant.get('state')}")
    bundles = status.get("receipts", {}).get("bundles", [])
    _require(
        any(bundle.get("source_task") == "track_a" for bundle in bundles), "no bundle from track A"
    )
    return f"operator grant {grant.get('message_id')} acknowledged by worker:track_b"


def check_flagship_receipts_bind_candidates(store: EvidenceStore) -> str:
    status = _status(store, FLAGSHIP)
    document = _document(store, FLAGSHIP)
    tasks = _accepted_tasks(status, FLAGSHIP)
    reviews = receipt_bodies(document, "review")
    verifications = receipt_bodies(document, "verification")
    candidates = receipt_bodies(document, "candidate")
    for task in tasks:
        key = task["task_key"]
        digest = task["accepted_candidate"]
        contract = task["contract_id"]
        candidate = next(
            (body for body in candidates if body["candidate"]["candidate_digest"] == digest), None
        )
        _require(candidate is not None, f"{key}: no candidate receipt")
        assert candidate is not None
        _require(
            candidate.get("admitted") is True
            and not candidate.get("violation_codes")
            and candidate.get("mutated_after_checkpoint") is False
            and candidate["candidate"].get("contract_id") == contract,
            f"{key}: candidate receipt is not an admitted, unmutated candidate of the contract",
        )
        review = next((body for body in reviews if body.get("candidate_digest") == digest), None)
        _require(review is not None, f"{key}: no review of the accepted candidate")
        assert review is not None
        _require(
            review.get("verdict") == "pass"
            and review.get("candidate_unchanged") is True
            and review.get("contract_id") == contract,
            f"{key}: review {review.get('verdict')} does not bind the candidate and contract",
        )
        _require(
            review.get("reviewer_session_id") != candidate["candidate"].get("worker_session_id"),
            f"{key}: the worker reviewed its own candidate",
        )
        verification = next(
            (
                body
                for body in verifications
                if body.get("task_key") == key and body.get("subject_digest") == digest
            ),
            None,
        )
        _require(verification is not None, f"{key}: no verification of the accepted candidate")
        assert verification is not None
        _require(
            verification.get("subject_unchanged") is True
            and contract in verification.get("contract_ids", [])
            and verification.get("verifier_files_digest") == status.get("verifier_digest"),
            f"{key}: verification does not bind the candidate, contract, and pinned verifier",
        )
        _suites_passed(verification, task["required_suites"])
    return (
        f"{len(tasks)} accepted candidates each bound to an independent passing review and a "
        "passing pinned-verifier receipt under the same contract"
    )


def check_flagship_integration_verified(store: EvidenceStore) -> str:
    status = _status(store, FLAGSHIP)
    document = _document(store, FLAGSHIP)
    tasks = _accepted_tasks(status, FLAGSHIP)
    integrations = receipt_bodies(document, "integration")
    _require(len(integrations) == 1, f"{len(integrations)} integration receipts")
    integration = integrations[0]
    _require(
        sorted(integration.get("applied_candidates", []))
        == sorted(task["accepted_candidate"] for task in tasks),
        "integration applied a different candidate set",
    )
    _require(not integration.get("conflicts"), f"conflicts {integration.get('conflicts')}")
    _require(
        integration.get("base_commit") == status.get("base_commit"), "integration base differs"
    )
    verification = _integration_verification(document, integration)
    _require(
        verification.get("subject_unchanged") is True, "integration tree changed under the verifier"
    )
    _require(
        sorted(verification.get("contract_ids", []))
        == sorted(task["contract_id"] for task in tasks),
        "integration verification covers different contracts",
    )
    suites = [suite.get("suite_id") for suite in verification.get("suites", [])]
    _suites_passed(verification, suites)
    return f"isolated integration of {len(tasks)} candidates passed suites {suites}"


def _integration_suite_check(store: EvidenceStore, suite_ids: tuple[str, ...]) -> str:
    status = _status(store, FLAGSHIP)
    document = _document(store, FLAGSHIP)
    integrations = receipt_bodies(document, "integration")
    _require(len(integrations) == 1, f"{len(integrations)} integration receipts")
    verification = _integration_verification(document, integrations[0])
    _suites_passed(verification, suite_ids)
    _require(
        verification.get("verifier_files_digest") == status.get("verifier_digest"),
        "the integration was verified by an unpinned verifier",
    )
    counts = {
        suite["suite_id"]: f"{suite['tests_passed']}/{suite['tests_total']}"
        for suite in verification["suites"]
        if suite.get("suite_id") in suite_ids
    }
    return f"pinned verifier on the integrated tree: {counts}"


def check_flagship_trusted_api(store: EvidenceStore) -> str:
    return _integration_suite_check(store, ("api", "store"))


def check_flagship_trusted_browser(store: EvidenceStore) -> str:
    detail = _integration_suite_check(store, ("browser",))
    status = _status(store, FLAGSHIP)
    document = _document(store, FLAGSHIP)
    track_b = next(
        task for task in _accepted_tasks(status, FLAGSHIP) if task["task_key"] == "track_b"
    )
    verification = next(
        (
            body
            for body in receipt_bodies(document, "verification")
            if body.get("task_key") == "track_b"
            and body.get("subject_digest") == track_b["accepted_candidate"]
        ),
        None,
    )
    _require(verification is not None, "no verification of track B's candidate")
    assert verification is not None
    _suites_passed(verification, ("browser_frontend_only",))
    return f"{detail}; track B passed browser_frontend_only against the contract stub"


def check_flagship_cleanup_retains_unsafe(store: EvidenceStore) -> str:
    status = _status(store, FLAGSHIP)
    cleanup = status.get("last_run", {}).get("cleanup")
    _require(isinstance(cleanup, list) and bool(cleanup), "no cleanup record")
    assert isinstance(cleanup, list)
    for entry in cleanup:
        _require(
            entry.get("removed") is True or isinstance(entry.get("reason_code"), str),
            f"{entry.get('key')} was neither removed nor retained with a reason",
        )
    retained = [entry for entry in cleanup if entry.get("removed") is not True]
    reasons = sorted({entry["reason_code"] for entry in retained})
    return f"{len(cleanup)} owned workspaces; {len(retained)} retained with reasons {reasons}"


def check_compare_selection_refuses_defect(store: EvidenceStore) -> str:
    status = _status(store, COMPARE_SELECTION)
    document = _document(store, COMPARE_SELECTION)
    _require(status.get("mode") == "compare", f"mode {status.get('mode')}")
    _require(status.get("phase") == "accepted", f"phase {status.get('phase')}")
    selection = status.get("receipts", {}).get("selection") or {}
    winner = selection.get("winner")
    excluded = selection.get("excluded", [])
    _require(isinstance(winner, str), "no winner")
    _require(bool(excluded), "nothing was excluded")
    _require(all(entry.get("reasons") for entry in excluded), "an exclusion has no reason")
    _require(
        winner not in {entry.get("candidate_digest") for entry in excluded}, "winner was excluded"
    )
    reasons = sorted({reason for entry in excluded for reason in entry["reasons"]})
    _require("gate_suite_failed" in reasons, "no candidate was excluded by the trusted suite")
    _require(
        "selection_mutation_artifact" in reasons,
        "the mutation-artifact negative control was not excluded",
    )
    task = _accepted_tasks(status, COMPARE_SELECTION)[0]
    _require(task["accepted_candidate"] == winner, "the accepted candidate is not the winner")
    passing = [
        body
        for body in receipt_bodies(document, "verification")
        if body.get("subject_digest") == winner and body.get("task_key") is not None
    ]
    _require(len(passing) == 1, "the winner has no task verification")
    _suites_passed(passing[0], task["required_suites"])
    return f"winner passed {task['required_suites']}; {len(excluded)} excluded for {reasons}"


def check_compare_both_fail_no_winner(store: EvidenceStore) -> str:
    status = _status(store, COMPARE_BOTH_FAIL)
    document = _document(store, COMPARE_BOTH_FAIL)
    _require(status.get("phase") == "failed", f"phase {status.get('phase')}")
    selection = status.get("receipts", {}).get("selection") or {}
    _require(selection.get("winner") is None, "a winner was selected")
    _require(bool(selection.get("excluded")), "no candidate was excluded")
    _require(not receipt_bodies(document, "integration"), "an integration was produced")
    _require(
        all(task.get("accepted_candidate") is None for task in status.get("tasks", [])),
        "a task accepted a candidate",
    )
    verifications = receipt_bodies(document, "verification")
    _require(bool(verifications), "no verification ran")
    _require(
        all(
            any(suite.get("status") == "failed" for suite in body.get("suites", []))
            for body in verifications
        ),
        "a candidate verification did not fail",
    )
    return f"no winner; {len(selection['excluded'])} exclusions; blocked {status.get('blocked_reason')}"


def check_bounded_repair_closed(store: EvidenceStore) -> str:
    status = _status(store, BOUNDED_REPAIR)
    document = _document(store, BOUNDED_REPAIR)
    _require(status.get("phase") == "accepted", f"phase {status.get('phase')}")
    task = _accepted_tasks(status, BOUNDED_REPAIR)[0]
    _require(
        task.get("repairs_used") == 1 and task["repairs_used"] <= task.get("max_repairs", 0),
        f"repairs used {task.get('repairs_used')} of {task.get('max_repairs')}",
    )
    _require(task.get("contract_version") == 1, "the repair changed the contract version")
    candidates = [body["candidate"] for body in receipt_bodies(document, "candidate")]
    _require(len(candidates) == 2, f"{len(candidates)} candidates")
    verifications = {
        body.get("subject_digest"): body
        for body in receipt_bodies(document, "verification")
        if body.get("task_key") is not None
    }
    first = verifications.get(candidates[0]["candidate_digest"])
    _require(first is not None, "the defective candidate was not verified")
    assert first is not None
    _require(
        any(suite.get("status") == "failed" for suite in first.get("suites", [])),
        "the injected defect was not caught",
    )
    _require(
        task["accepted_candidate"] == candidates[1]["candidate_digest"],
        "the accepted candidate is not the repair",
    )
    second = verifications.get(candidates[1]["candidate_digest"])
    _require(second is not None, "the repair was not verified")
    assert second is not None
    _suites_passed(second, task["required_suites"])
    return "the injected defect failed the trusted suite; one repair passed it"


def check_optimizer_cycle_bounded(store: EvidenceStore) -> str:
    value = _document(store, OPTIMIZER_CYCLE)["value"]
    report = value.get("report", {})
    rejections = [outcome.get("rejection") for outcome in report.get("outcomes", [])]
    _require("hard_gate_failure" in rejections, "no harmful candidate was rejected")
    _require("no_improvement" in rejections, "no no-gain candidate was rejected")
    _require(report.get("optimization_improved") is True, "no improvement was promoted")
    _require(
        report.get("promoted_version") == 2, f"promoted version {report.get('promoted_version')}"
    )
    _require(report.get("holdout_consultations") == 1, "the holdout was consulted more than once")
    spent = report.get("model_requests_spent")
    limit = report.get("max_model_requests")
    _require(
        isinstance(spent, int) and isinstance(limit, int) and 0 < spent <= limit,
        f"spent {spent} of {limit} model requests",
    )
    _require(value.get("replay", {}).get("duplicate") is True, "a replayed cycle ran again")
    restored = value.get("restored", {})
    _require(
        restored.get("version") == 1 and restored.get("state") == "active",
        "rollback did not restore version 1",
    )
    return (
        f"rejected {sorted(item for item in rejections if item)}; promoted version 2 within "
        f"{spent}/{limit} fixture requests; replay deduplicated; rolled back to version 1"
    )


EVIDENCE_CHECKS: Mapping[str, tuple[tuple[str, ...], Callable[[EvidenceStore], str]]] = {
    "flagship_two_sessions": ((FLAGSHIP,), check_flagship_two_sessions),
    "flagship_bundle_delivered": ((FLAGSHIP,), check_flagship_bundle_delivered),
    "share_grant_acknowledged": ((SHARE_WALKTHROUGH,), check_share_grant_acknowledged),
    "flagship_receipts_bind_candidates": ((FLAGSHIP,), check_flagship_receipts_bind_candidates),
    "flagship_integration_verified": ((FLAGSHIP,), check_flagship_integration_verified),
    "flagship_trusted_api": ((FLAGSHIP,), check_flagship_trusted_api),
    "flagship_trusted_browser": ((FLAGSHIP,), check_flagship_trusted_browser),
    "flagship_cleanup_retains_unsafe": ((FLAGSHIP,), check_flagship_cleanup_retains_unsafe),
    "compare_selection_refuses_defect": (
        (COMPARE_SELECTION,),
        check_compare_selection_refuses_defect,
    ),
    "compare_both_fail_no_winner": ((COMPARE_BOTH_FAIL,), check_compare_both_fail_no_winner),
    "bounded_repair_closed": ((BOUNDED_REPAIR,), check_bounded_repair_closed),
    "optimizer_cycle_bounded": ((OPTIMIZER_CYCLE,), check_optimizer_cycle_bounded),
}


def run_evidence_check(store: EvidenceStore, check: str) -> CheckOutcome:
    entry = EVIDENCE_CHECKS.get(check)
    if entry is None:
        return CheckOutcome(CHECK_FAIL, f"unknown evidence check {check}")
    try:
        return CheckOutcome(CHECK_PASS, entry[1](store))
    except CheckFailed as error:
        return CheckOutcome(CHECK_FAIL, str(error))
    except (KeyError, TypeError, AttributeError, StopIteration) as error:
        return CheckOutcome(CHECK_FAIL, f"malformed evidence: {type(error).__name__}: {error}")


def evidence_reference(store: EvidenceStore, name: str) -> str:
    item = store.files.get(name)
    if item is None:
        return f"evidence:{name}:missing"
    return f"evidence:{name}:sha256:{item.sha256}"


# ---------------------------------------------------------- report views --


def _receipt_entries(store: EvidenceStore, kind: str) -> list[tuple[str, Mapping[str, Any]]]:
    entries: list[tuple[str, Mapping[str, Any]]] = []
    for name in WORKFLOW_EVIDENCE:
        item = store.files.get(name)
        if item is not None:
            entries.extend((name, body) for body in receipt_bodies(item.document, kind))
    return entries


def session_records(store: EvidenceStore) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    for name in WORKFLOW_EVIDENCE:
        item = store.files.get(name)
        if item is None:
            continue
        for inspect in item.document.get("sessions", []):
            session = inspect.get("session", {})
            records.append(
                {
                    "source_evidence": name,
                    "evidence_class": FIXTURE_EVIDENCE_CLASS,
                    "session_id": session.get("session_id"),
                    "role": session.get("role"),
                    "task_key": session.get("task_key"),
                    "slot_id": session.get("slot_id"),
                    "harness": session.get("harness"),
                    "route": session.get("route"),
                    "interface_mode": inspect.get("interface_mode"),
                    "native_tui_attachable": inspect.get("native_tui_attachable"),
                    "attempts": len(inspect.get("attempts", [])),
                    "rendered_prompt_sha256": [
                        attempt.get("rendered_prompt_sha256")
                        for attempt in inspect.get("attempts", [])
                    ],
                }
            )
    return records


def candidate_records(store: EvidenceStore) -> list[dict[str, Any]]:
    return [
        {
            "source_evidence": name,
            "evidence_class": FIXTURE_EVIDENCE_CLASS,
            "receipt_id": body.get("receipt_id"),
            "candidate_digest": body.get("candidate", {}).get("candidate_digest"),
            "task_key": body.get("candidate", {}).get("task_key"),
            "contract_id": body.get("candidate", {}).get("contract_id"),
            "origin": body.get("candidate", {}).get("origin", {}).get("kind"),
            "worker_route": body.get("candidate", {}).get("worker_route"),
            "admitted": body.get("admitted"),
            "violation_codes": body.get("violation_codes"),
        }
        for name, body in _receipt_entries(store, "candidate")
    ]


def review_records(store: EvidenceStore) -> list[dict[str, Any]]:
    return [
        {
            "source_evidence": name,
            "evidence_class": FIXTURE_EVIDENCE_CLASS,
            "receipt_id": body.get("receipt_id"),
            "candidate_digest": body.get("candidate_digest"),
            "contract_id": body.get("contract_id"),
            "verdict": body.get("verdict"),
            "candidate_unchanged": body.get("candidate_unchanged"),
            "reviewer_route": body.get("reviewer_route"),
            "reviewer_session_id": body.get("reviewer_session_id"),
        }
        for name, body in _receipt_entries(store, "review")
    ]


def verifier_records(store: EvidenceStore) -> list[dict[str, Any]]:
    return [
        {
            "source_evidence": name,
            "evidence_class": FIXTURE_EVIDENCE_CLASS,
            "receipt_id": body.get("receipt_id"),
            "task_key": body.get("task_key"),
            "subject_digest": body.get("subject_digest"),
            "subject_unchanged": body.get("subject_unchanged"),
            "verifier_files_digest": body.get("verifier_files_digest"),
            "tool_versions": body.get("tool_versions"),
            "suites": [
                {
                    "suite_id": suite.get("suite_id"),
                    "status": suite.get("status"),
                    "tests_passed": suite.get("tests_passed"),
                    "tests_total": suite.get("tests_total"),
                    "command": suite.get("command"),
                    "output_sha256": suite.get("output_sha256"),
                }
                for suite in body.get("suites", [])
            ],
        }
        for name, body in _receipt_entries(store, "verification")
    ]


def integration_record(store: EvidenceStore) -> dict[str, Any] | None:
    item = store.files.get(FLAGSHIP)
    if item is None:
        return None
    integrations = receipt_bodies(item.document, "integration")
    if len(integrations) != 1:
        return None
    integration = integrations[0]
    verification_ids = [
        body.get("receipt_id")
        for body in receipt_bodies(item.document, "verification")
        if body.get("task_key") is None
        and body.get("subject_digest") == integration.get("integration_tree_digest")
    ]
    return {
        "source_evidence": FLAGSHIP,
        "evidence_class": FIXTURE_EVIDENCE_CLASS,
        "receipt_id": integration.get("receipt_id"),
        "base_commit": integration.get("base_commit"),
        "applied_candidates": integration.get("applied_candidates"),
        "conflicts": integration.get("conflicts"),
        "integration_tree_digest": integration.get("integration_tree_digest"),
        "patch_sha256": integration.get("patch_sha256"),
        "verification_receipt_ids": verification_ids,
    }


def workflow_summaries(store: EvidenceStore) -> list[dict[str, Any]]:
    summaries: list[dict[str, Any]] = []
    for name in WORKFLOW_EVIDENCE:
        item = store.files.get(name)
        if item is None:
            continue
        status = item.document.get("status", {})
        summaries.append(
            {
                "source_evidence": name,
                "workflow_id": item.document.get("workflow_id"),
                "mode": status.get("mode"),
                "phase": status.get("phase"),
                "evidence_class": status.get("evidence_class"),
                "start_contract": status.get("start_contract"),
                "policy_digest": status.get("policy_digest"),
                "verifier_digest": status.get("verifier_digest"),
                "base_commit": status.get("base_commit"),
                "contract_ids": [
                    entry.get("body", {}).get("contract_id")
                    for entry in item.document.get("export", [])
                    if entry.get("kind") == "contract"
                ],
                "retained_workspaces": [
                    entry.get("key")
                    for entry in status.get("last_run", {}).get("cleanup", [])
                    if entry.get("removed") is not True
                ],
            }
        )
    return summaries


def evidence_classes(store: EvidenceStore) -> list[str]:
    """Every distinct evidence class the exported statuses declare."""
    classes = {
        str(item.document.get("status", {}).get("evidence_class"))
        for name, item in store.files.items()
        if name in WORKFLOW_EVIDENCE
    }
    return sorted(classes)
