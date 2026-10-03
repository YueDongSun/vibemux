"""The pinned scenario matrix and the evidence each execution class needs.

Scenario IDs, descriptions, and required execution classes come from the
pinned benchmark file; this module only says which executed tests,
calibration checks, and exported-evidence checks can satisfy each
(scenario, class) pair. A class with no rule here cannot pass: the
catalog must cover the benchmark exactly or the runner stops.
"""

from __future__ import annotations

import hashlib
import json
from collections.abc import Mapping
from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path
from typing import Any


class RuleKind(Enum):
    """How a (scenario, class) pair is decided."""

    TESTS = "tests"
    REPOSITORY = "repository"
    CALIBRATION = "calibration"
    LIVE = "live"
    NOT_IMPLEMENTED = "not_implemented"
    REPORT_VALIDATION = "report_validation"


@dataclass(frozen=True)
class CargoTestRef:
    """One libtest test: `target_kind` is `lib` or `test`; `name` is the
    full test path libtest prints."""

    package: str
    target_kind: str
    target: str
    name: str

    def label(self) -> str:
        return f"cargo:{self.package}/{self.target_kind}:{self.target}::{self.name}"


@dataclass(frozen=True)
class EvidenceRule:
    kind: RuleKind
    tests: tuple[CargoTestRef, ...] = ()
    evidence_checks: tuple[str, ...] = ()
    calibration_checks: tuple[str, ...] = ()
    requires_windows: bool = False
    # Tests that relate to the class but cannot satisfy it.
    related: tuple[CargoTestRef, ...] = ()
    reason: str = ""


@dataclass(frozen=True)
class ScenarioSpec:
    scenario_id: str
    description: str
    required_for_full_pass: bool
    execution_classes: tuple[str, ...]


@dataclass(frozen=True)
class Benchmark:
    path: Path
    sha256: str
    scenarios: tuple[ScenarioSpec, ...]
    live_policy_template: Mapping[str, Any]
    not_established: tuple[str, ...] = field(default=())


LIVE_CLASSES = frozenset(
    {"live_gateway", "live_coding", "live_native_windows", "live_mixed_harness", "live_optimizer"}
)


class CatalogError(ValueError):
    """The benchmark and the catalog disagree, or the benchmark is invalid."""


def load_benchmark(path: Path) -> Benchmark:
    raw = path.read_bytes()
    try:
        document = json.loads(raw)
    except json.JSONDecodeError as error:
        raise CatalogError(f"benchmark is not JSON: {error}") from error
    if not isinstance(document, dict) or document.get("schema_version") != 1:
        raise CatalogError("benchmark schema_version must be 1")
    entries = document.get("scenarios")
    if not isinstance(entries, list) or not entries:
        raise CatalogError("benchmark has no scenarios")
    scenarios: list[ScenarioSpec] = []
    for entry in entries:
        if not isinstance(entry, dict):
            raise CatalogError("benchmark scenario is not an object")
        classes = entry.get("execution_classes")
        if not isinstance(classes, list) or not all(isinstance(item, str) for item in classes):
            raise CatalogError(f"scenario {entry.get('id')} has invalid execution classes")
        scenarios.append(
            ScenarioSpec(
                scenario_id=str(entry["id"]),
                description=str(entry["description"]),
                required_for_full_pass=bool(entry["required_for_full_pass"]),
                execution_classes=tuple(classes),
            )
        )
    template = document.get("live_policy_template")
    not_established = document.get("not_established", [])
    return Benchmark(
        path=path,
        sha256=hashlib.sha256(raw).hexdigest(),
        scenarios=tuple(scenarios),
        live_policy_template=template if isinstance(template, dict) else {},
        not_established=tuple(str(item) for item in not_established),
    )


def check_catalog(benchmark: Benchmark, rules: Mapping[tuple[str, str], EvidenceRule]) -> None:
    """Refuses a catalog that does not cover exactly the benchmark's pairs."""
    expected = {
        (scenario.scenario_id, execution_class)
        for scenario in benchmark.scenarios
        for execution_class in scenario.execution_classes
    }
    ids = [scenario.scenario_id for scenario in benchmark.scenarios]
    if len(ids) != len(set(ids)):
        raise CatalogError("benchmark scenario IDs are not unique")
    missing = sorted(expected - rules.keys())
    extra = sorted(rules.keys() - expected)
    if missing or extra:
        raise CatalogError(f"catalog mismatch: missing {missing}, unexpected {extra}")
    for (scenario_id, execution_class), rule in rules.items():
        is_live = execution_class in LIVE_CLASSES
        if is_live != (rule.kind is RuleKind.LIVE):
            raise CatalogError(f"{scenario_id}/{execution_class}: live classes need live rules")


# ------------------------------------------------------------- test refs --


def _unit(package: str, name: str) -> CargoTestRef:
    return CargoTestRef(package, "lib", package, name)


def _workflow(module: str, name: str) -> CargoTestRef:
    return _unit("vibemux_workflow", f"{module}::tests::{name}")


def _daemon_unit(module: str, name: str) -> CargoTestRef:
    return _unit("vibemuxd", f"{module}::tests::{name}")


def _integration(package: str, target: str, name: str) -> CargoTestRef:
    return CargoTestRef(package, "test", target, name)


def _daemon(target: str, name: str) -> CargoTestRef:
    return _integration("vibemuxd", target, name)


def _store(name: str) -> CargoTestRef:
    return _integration("vibemux_store", "workflow", name)


FLAGSHIP = _daemon(
    "workflow_cooperate", "two_tracks_build_taskboard_with_a_scoped_bundle_and_isolated_integration"
)
COMPARE_SELECTS = _daemon(
    "workflow_compare", "the_trusted_suite_selects_the_correct_candidate_and_refuses_the_defect"
)
COMPARE_NO_WINNER = _daemon("workflow_compare", "two_defective_candidates_select_no_winner")
SHARE_RESTARTS = _daemon(
    "workflow_share", "an_operator_share_reaches_the_other_track_and_survives_restarts"
)
SHARE_PURGED = _daemon("workflow_share", "purged_bundle_content_is_never_forged_into_a_later_turn")
PAUSE_SEQUENTIAL = _daemon(
    "workflow_control",
    "a_paused_session_shows_its_exact_next_prompt_and_one_slot_runs_sequentially",
)
CANCELLED_TURN = _daemon("workflow_control", "a_cancelled_turn_is_stopped_and_nothing_is_promoted")
RESTART_NO_REPEAT = _daemon(
    "workflow_control", "a_restart_stops_the_workflow_and_the_resumed_run_repeats_no_completed_turn"
)
NO_FREE_SLOT = _daemon(
    "workflow_admission", "with_no_free_slot_a_workflow_blocks_until_one_is_released"
)
OPTIMIZER_PROMOTES = _daemon(
    "workflow_optimizer", "only_a_verified_improvement_is_promoted_and_it_can_be_rolled_back"
)
OPTIMIZER_BOUNDS = _daemon("workflow_optimizer", "the_budget_and_the_suite_shape_bound_a_cycle")
BOUNDED_REPAIR = _daemon("workflow_gates", "one_bounded_repair_closes_an_injected_defect")
EXHAUSTED_REPAIRS = _daemon("workflow_gates", "exhausted_repairs_fail_the_task")
OUT_OF_SCOPE = _daemon("workflow_gates", "an_out_of_scope_file_is_refused_and_repaired")
PROTECTED_PATHS = _daemon("workflow_gates", "protected_paths_fail_the_run")


def _tests(
    *tests: CargoTestRef, checks: tuple[str, ...] = (), windows: bool = False
) -> EvidenceRule:
    return EvidenceRule(
        RuleKind.TESTS, tests=tests, evidence_checks=checks, requires_windows=windows
    )


def _live(reason: str) -> EvidenceRule:
    return EvidenceRule(RuleKind.LIVE, reason=reason)


def _not_implemented(reason: str, *related: CargoTestRef) -> EvidenceRule:
    return EvidenceRule(RuleKind.NOT_IMPLEMENTED, related=related, reason=reason)


LIVE_GATEWAY = "needs real AAG requests over configured role routes"
LIVE_CODING = "needs real writable harness turns through configured AAG routes"
LIVE_NATIVE = "needs two owned native TUI sessions on Windows"

RULES: Mapping[tuple[str, str], EvidenceRule] = {
    ("B01", "repository_test"): EvidenceRule(RuleKind.REPOSITORY),
    ("B02", "native_windows_test"): _tests(
        _daemon_unit("process", "windows_control_runtime_is_hashed_and_acl_restricted"),
        _daemon_unit("process", "acl_marker_publication_replaces_stale_markers_without_debris"),
        _daemon_unit("process", "ensure_runtime_dir_heals_a_stale_acl_marker"),
        _daemon_unit("process", "concurrent_first_touch_serializes_marker_publication"),
        _daemon_unit("process", "isolated_local_app_data_relocates_the_whole_control_surface"),
        _daemon("daemon_process", "trusted_start_fails_closed_when_the_runtime_acl_is_loosened"),
        _integration(
            "vibemux_platform", "process_tree", "terminate_kills_the_child_and_its_descendant"
        ),
        _integration(
            "vibemux_platform",
            "process_tree",
            "a_descendant_outlives_its_killed_parent_until_the_tree_is_terminated",
        ),
        _integration(
            "vibemux_cli",
            "bootstrap_process",
            "startup_timeout_terminates_only_the_spawned_fixture",
        ),
        windows=True,
    ),
    ("P01", "deterministic_test"): _tests(
        _workflow("renderer", "identical_inputs_render_identical_bytes"),
        _workflow("renderer", "any_input_change_changes_the_bytes"),
        _workflow("renderer", "turn_sections_extend_the_shared_contract_text"),
        _workflow("canonical_json", "canonical_bytes_round_trip_and_are_stable"),
        _workflow("contract", "identity_is_stable_and_sensitive_to_every_input"),
        _store("workflow_admission_is_idempotent_and_conflicting_content_is_refused"),
        FLAGSHIP,
    ),
    ("P02", "deterministic_test"): _tests(
        _workflow("compiler_validation", "dropped_prohibition_is_rejected"),
        _workflow("compiler_validation", "weakened_prohibition_is_rejected"),
        _workflow("compiler_validation", "changed_number_is_rejected_both_ways"),
        _workflow("compiler_validation", "invented_scope_is_rejected"),
        _workflow("compiler_validation", "exclusions_cannot_hide_obligations_or_prohibitions"),
        _workflow(
            "compiler_validation",
            "a_binding_clause_may_be_assigned_to_another_task_only_if_it_covers_it",
        ),
        _workflow("compiler_validation", "numbers_may_come_from_verified_contract_excerpts_only"),
        _workflow("compiler_validation", "normative_assumptions_are_rejected"),
        _daemon(
            "workflow_admission", "a_contract_that_drops_a_prohibition_or_invents_scope_is_rejected"
        ),
    ),
    ("P03", "deterministic_test"): _tests(
        _workflow("compiler_validation", "unicode_literals_must_stay_literals_not_instructions"),
        _workflow("compiler_validation", "transformed_or_dropped_literals_are_rejected"),
        _workflow("renderer", "literals_cannot_escape_their_fence_or_forge_sections"),
        _workflow("renderer", "mentions_fail_closed_unless_the_harness_is_certified_not_to_expand"),
        _workflow("renderer", "email_like_text_is_not_a_mention_but_a_leading_mention_is"),
        _workflow("source_request", "prohibitions_are_found_outside_literals_only"),
        _workflow("source_request", "literal_tokens_cover_quotes_paths_and_mentions"),
        _workflow(
            "source_request", "clauses_split_on_sentence_ends_but_not_inside_literals_or_file_names"
        ),
        _workflow("canonical_json", "unicode_and_markup_are_preserved_and_controls_escaped"),
        _workflow("templates", "every_template_is_resolvable_and_contains_no_mentions"),
    ),
    ("P03", "adapter_fixture"): _tests(
        _daemon(
            "workflow_literals",
            "adversarial_literals_reach_the_worker_verbatim_and_change_no_policy",
        ),
        _daemon("workflow_literals", "a_mention_a_harness_could_expand_is_refused_before_any_turn"),
    ),
    ("P04", "deterministic_test"): _tests(
        _workflow("contract", "semantic_edits_require_a_new_generation"),
        _workflow("contract", "scope_and_requirement_set_changes_are_reported"),
        _store("contract_generations_never_rebind_admitted_attempts"),
        _daemon("workflow_admission", "a_changed_request_cannot_overwrite_an_admitted_one"),
    ),
    ("G01", "gateway_fixture"): _not_implemented(
        "the daemon has no AAG gateway client, so no fixture gateway can be driven through it; "
        "only the pure route contract is unit-tested",
        _workflow("gateway", "a_global_pin_blocks_strict_routing_and_changes_the_generation"),
        _workflow("gateway", "correlation_header_is_content_free"),
        _workflow("gateway", "configs_are_loopback_only_and_reference_credentials_by_name"),
    ),
    ("G01", "live_gateway"): _live(LIVE_GATEWAY),
    ("G02", "adapter_fixture"): _tests(
        _daemon("workflow_admission", "a_slot_that_cannot_execute_is_refused_before_admission"),
        _daemon("harness_dispatch", "an_acp_route_is_probe_only"),
        _daemon("harness_dispatch", "admission_gates_refuse_before_any_state_change"),
        _workflow("slots", "probe_only_and_detected_only_slots_are_not_coding_workers"),
        _workflow(
            "gateway",
            "unsupported_protocols_unbound_roles_and_missing_aliases_are_refused_up_front",
        ),
    ),
    ("G03", "gateway_fixture"): _not_implemented(
        "no daemon gateway client exists to fail over or stream through a fixture gateway; "
        "only the pure retry and stream-stitching contract is unit-tested",
        _workflow("gateway", "failures_after_output_or_tool_calls_are_never_retried_automatically"),
        _workflow("gateway", "broken_streams_are_not_stitched"),
        _workflow("gateway", "usage_is_parsed_when_reported_and_unknown_otherwise"),
    ),
    ("G04", "live_gateway"): _live(LIVE_GATEWAY),
    ("S01", "deterministic_fixture"): _tests(FLAGSHIP, checks=("flagship_two_sessions",)),
    ("S01", "live_coding"): _live(LIVE_CODING),
    ("S02", "deterministic_fixture"): _tests(
        PAUSE_SEQUENTIAL,
        NO_FREE_SLOT,
        _workflow("slots", "zero_one_and_two_slots_plan_blocked_sequential_and_parallel"),
    ),
    ("S03", "deterministic_fixture"): _tests(
        NO_FREE_SLOT,
        _workflow("leases", "missed_heartbeat_quarantines_and_never_releases"),
        _workflow("leases", "stale_generations_are_fenced_everywhere"),
        _workflow("leases", "revocation_requires_a_settled_attempt"),
        _store("leases_reserve_one_slot_and_one_worktree_under_increasing_generations"),
        _store("a_missed_heartbeat_quarantines_and_keeps_the_worktree_reserved"),
        _store("restart_quarantines_running_attempts_and_releases_idle_leases"),
    ),
    ("S04", "deterministic_fixture"): _tests(
        RESTART_NO_REPEAT,
        _store("workflow_admission_is_idempotent_and_conflicting_content_is_refused"),
        _store("candidates_are_fenced_and_bound_to_their_settled_attempt"),
        _daemon("harness_dispatch", "concurrent_repeats_of_one_request_launch_one_attempt"),
    ),
    ("T01", "live_native_windows"): _live(LIVE_NATIVE),
    ("T02", "adapter_fixture"): _not_implemented(
        "native sessions are not owned, so human input ownership and unknown-delivery handling "
        "are not implemented; only structured-mode interruption is tested",
        _daemon("harness_dispatch", "claude_confirms_an_interrupt_without_a_kill"),
        _daemon("harness_dispatch", "codex_app_server_confirms_an_interrupt_without_a_kill"),
        CANCELLED_TURN,
    ),
    ("T02", "live_native_windows"): _live(LIVE_NATIVE),
    ("T03", "adapter_fixture"): _not_implemented(
        "structured/native handoff and context-transfer resume are not implemented; "
        "native attach is refused",
        FLAGSHIP,
    ),
    ("T03", "live_native_windows"): _live(LIVE_NATIVE),
    ("T04", "live_mixed_harness"): _live(
        "needs two distinct live harness kinds performing writable turns"
    ),
    ("C01", "deterministic_fixture"): _tests(
        FLAGSHIP,
        SHARE_RESTARTS,
        checks=("flagship_bundle_delivered", "share_grant_acknowledged"),
    ),
    ("C01", "live_coding"): _live(LIVE_CODING),
    ("C02", "deterministic_test"): _tests(
        _workflow("messages", "forged_senders_and_spoofed_grants_are_refused"),
        _workflow("messages", "audience_contract_and_source_freshness_are_enforced"),
        _workflow("messages", "delivery_states_are_idempotent_and_recipient_bound"),
        _workflow("messages", "inbox_cursor_deduplicates_and_refuses_gaps"),
        _workflow("messages", "fan_out_ttl_size_and_loop_depth_are_bounded"),
        _store("messages_are_sequenced_deduplicated_and_survive_reopen"),
        SHARE_RESTARTS,
    ),
    ("C03", "deterministic_test"): _tests(
        _workflow("context_bundle", "redaction_covers_common_secret_shapes_without_eating_code"),
        _workflow("context_bundle", "bundles_carry_source_refs_attribution_and_redaction"),
        _store("content_index_records_and_tombstones_blobs_without_content"),
        _daemon_unit("workflow::state_files", "outbox_bodies_are_rehashed_and_deleted"),
        SHARE_RESTARTS,
        SHARE_PURGED,
    ),
    ("C03", "native_windows_test"): _tests(
        _daemon_unit(
            "workflow::state_protection",
            "the_state_root_and_its_files_are_restricted_and_a_loosened_root_is_narrowed",
        ),
        _daemon_unit(
            "workflow::state_protection", "a_file_in_place_of_the_state_root_fails_closed"
        ),
        SHARE_RESTARTS,
        windows=True,
    ),
    ("C04", "deterministic_fixture"): _tests(
        SHARE_RESTARTS, SHARE_PURGED, checks=("share_grant_acknowledged",)
    ),
    ("W01", "deterministic_test"): _tests(
        _workflow("snapshot", "out_of_scope_protected_and_case_alias_changes_fail"),
        _workflow("snapshot", "traversal_paths_are_invalid"),
        _workflow("snapshot", "symlinks_special_files_modes_and_binary_content_are_rejected"),
        _store("a_protected_path_violation_fails_the_task_and_is_kept_as_evidence"),
        OUT_OF_SCOPE,
        PROTECTED_PATHS,
    ),
    ("W02", "deterministic_test"): _tests(
        _workflow("snapshot", "untracked_files_and_deletions_inside_owned_paths_are_collected"),
        _workflow("snapshot", "mutation_after_collection_is_detected"),
        _workflow("gates", "modified_verifiers_mutated_subjects_and_mutation_artifacts_fail"),
        _daemon_unit(
            "workflow::collector", "status_entries_map_untracked_ignored_and_deleted_paths"
        ),
        _daemon_unit("workflow::collector", "blobs_are_content_addressed_and_rehashed"),
        OUT_OF_SCOPE,
    ),
    ("W03", "deterministic_test"): _tests(
        _integration(
            "vibemux_workspace",
            "workspace_safety",
            "owner_receipt_mismatch_missing_and_oversized_never_authorize_mutation",
        ),
        _integration(
            "vibemux_workspace",
            "workspace_safety",
            "artifacts_are_immutable_bounded_and_ignored_artifacts_block_cleanup",
        ),
        _integration(
            "vibemux_workspace",
            "workspace_safety",
            "forged_owner_token_cannot_authorize_existing_git_worktree",
        ),
        _integration(
            "vibemux_workspace",
            "workspace_safety",
            "managed_directory_symlink_or_junction_is_refused_without_touching_target",
        ),
        _integration(
            "vibemux_cli",
            "bootstrap_process",
            "startup_timeout_terminates_only_the_spawned_fixture",
        ),
        FLAGSHIP,
        checks=("flagship_cleanup_retains_unsafe",),
    ),
    ("V01", "deterministic_fixture"): _tests(
        _workflow("gates", "a_protocol_completed_but_failing_candidate_cannot_pass"),
        BOUNDED_REPAIR,
        COMPARE_NO_WINNER,
        checks=("compare_both_fail_no_winner",),
    ),
    ("V02", "deterministic_fixture"): _tests(
        _workflow("gates", "receipts_must_bind_the_same_candidate_and_contract"),
        _workflow("gates", "reviewers_must_be_independent_and_must_not_mutate"),
        _store("the_gate_needs_an_independent_review_and_a_passing_verifier_on_the_same_candidate"),
        FLAGSHIP,
        checks=("flagship_receipts_bind_candidates",),
    ),
    ("V02", "live_coding"): _live(LIVE_CODING),
    ("V03", "deterministic_fixture"): _tests(
        FLAGSHIP,
        _store("workflow_acceptance_requires_integration_and_a_cancel_blocks_late_results"),
        checks=("flagship_integration_verified",),
    ),
    ("V03", "live_coding"): _live(LIVE_CODING),
    ("V04", "deterministic_fixture"): _tests(
        _workflow("selection", "the_defective_candidate_is_refused_even_when_cheaper"),
        _workflow("selection", "both_failing_selects_no_winner"),
        COMPARE_SELECTS,
        COMPARE_NO_WINNER,
        checks=("compare_selection_refuses_defect", "compare_both_fail_no_winner"),
    ),
    ("V04", "live_coding"): _live(LIVE_CODING),
    ("V05", "deterministic_fixture"): _tests(
        _workflow("gates", "cancelled_workflows_are_never_accepted_by_late_results"),
        _store("workflow_acceptance_requires_integration_and_a_cancel_blocks_late_results"),
        CANCELLED_TURN,
    ),
    ("A01", "live_coding"): _live(LIVE_CODING),
    ("A01", "trusted_api_test"): EvidenceRule(
        RuleKind.CALIBRATION,
        calibration_checks=("api", "store"),
        tests=(FLAGSHIP,),
        evidence_checks=("flagship_trusted_api",),
    ),
    ("A02", "live_coding"): _live(LIVE_CODING),
    ("A02", "trusted_browser_test"): EvidenceRule(
        RuleKind.CALIBRATION,
        calibration_checks=("browser",),
        tests=(FLAGSHIP,),
        evidence_checks=("flagship_trusted_browser",),
    ),
    ("A03", "deterministic_fixture"): _tests(
        BOUNDED_REPAIR, EXHAUSTED_REPAIRS, checks=("bounded_repair_closed",)
    ),
    ("O01", "deterministic_test"): _tests(
        _workflow("optimizer", "only_allowlisted_fields_and_optimizable_blocks_can_change"),
        _workflow(
            "templates", "only_optimizable_blocks_accept_overrides_and_the_digest_tracks_them"
        ),
        _daemon_unit("workflow::evaluate", "candidate_files_are_bounded_and_named"),
        OPTIMIZER_PROMOTES,
    ),
    ("O02", "deterministic_fixture"): _tests(
        OPTIMIZER_PROMOTES,
        OPTIMIZER_BOUNDS,
        _workflow("optimizer", "evaluation_must_cover_exactly_the_fixed_dataset"),
        _workflow(
            "optimizer", "a_cycle_is_budget_bounded_consults_holdout_once_and_can_find_nothing"
        ),
        _daemon_unit(
            "workflow::evaluate", "suites_need_disjoint_defined_splits_and_bounded_limits"
        ),
        _store("policy_versions_promote_for_future_admissions_and_roll_back"),
    ),
    ("O03", "deterministic_fixture"): _tests(
        OPTIMIZER_PROMOTES,
        OPTIMIZER_BOUNDS,
        _workflow("optimizer", "harmful_and_no_gain_candidates_are_rejected"),
        _workflow("optimizer", "a_holdout_regression_blocks_promotion"),
        _workflow("optimizer", "promotion_is_versioned_and_rollbackable"),
        checks=("optimizer_cycle_bounded",),
    ),
    ("O03", "live_optimizer"): _live("needs a bounded optimizer cycle over live model routes"),
    ("E01", "evidence_validation"): EvidenceRule(RuleKind.REPORT_VALIDATION),
}
