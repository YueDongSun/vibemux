"""The live-mode gate: explicit opt-in plus a complete private policy.

The policy file is private operator configuration. The runner reads it only
to validate it and records only its digest and the names of the checks it
failed, never its values. Passing the gate does not make a live class pass:
it only allows a live run to be attempted.
"""

from __future__ import annotations

import hashlib
import json
import re
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

ROLE_ROUTES = ("supervisor", "compiler", "worker_a", "worker_b", "reviewer", "optimizer")
LOOPBACK_HOSTS = frozenset({"127.0.0.1", "::1", "localhost"})
# A credential reference names a secret (for example an environment
# variable); it is never the secret itself.
CREDENTIAL_REFERENCE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]{0,63}$")
POSITIVE_LIMITS = (
    "max_parallel_worker_slots",
    "max_repairs_per_task",
    "max_optimizer_candidates",
    "max_elapsed_seconds",
    "max_total_model_requests",
)
REQUIRED_FALSE = (
    "global_provider_switching_allowed",
    "publish_allowed",
    "original_checkout_writes_allowed",
)


@dataclass(frozen=True)
class LiveGate:
    opted_in: bool
    policy_sha256: str | None
    problems: tuple[str, ...]

    @property
    def open(self) -> bool:
        return self.opted_in and self.policy_sha256 is not None and not self.problems

    def to_json(self) -> dict[str, Any]:
        return {
            "opted_in": self.opted_in,
            "policy_sha256": self.policy_sha256,
            "problems": list(self.problems),
            "gate_open": self.open,
        }


def policy_problems(policy: Any, template: Mapping[str, Any]) -> list[str]:
    """Why a live policy cannot authorize a run (names only, no values)."""
    if not isinstance(policy, dict):
        return ["policy is not a JSON object"]
    problems = [f"missing {key}" for key in template if key not in policy]
    problems.extend(f"unknown key {key}" for key in policy if key not in template)
    if policy.get("enabled") is not True:
        problems.append("enabled is not true")
    if policy.get("configuration_status") == template.get("configuration_status"):
        problems.append("configuration_status still requires operator configuration")
    endpoint = policy.get("gateway_endpoint")
    if not isinstance(endpoint, str):
        problems.append("gateway_endpoint is not set")
    else:
        parts = urlsplit(endpoint)
        if parts.scheme != "http" or parts.hostname not in LOOPBACK_HOSTS:
            problems.append("gateway_endpoint is not a loopback http URL")
    reference = policy.get("credential_reference")
    if not isinstance(reference, str) or not CREDENTIAL_REFERENCE.match(reference):
        problems.append("credential_reference is not a credential name")
    routes = policy.get("role_routes")
    if not isinstance(routes, dict):
        problems.append("role_routes is not an object")
    else:
        for role in ROLE_ROUTES:
            route = routes.get(role)
            if not isinstance(route, str) or not route.strip():
                problems.append(f"role_routes.{role} is not configured")
    if policy.get("per_session_config_isolation") is not True:
        problems.append("per_session_config_isolation is not true")
    for key in REQUIRED_FALSE:
        if policy.get(key) is not False:
            problems.append(f"{key} is not false")
    if not isinstance(policy.get("content_store_opt_in"), bool):
        problems.append("content_store_opt_in is not a boolean")
    for key in POSITIVE_LIMITS:
        value = policy.get(key)
        if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
            problems.append(f"{key} is not a positive integer")
    cap = policy.get("known_cost_cap")
    if not isinstance(cap, (int, float)) or isinstance(cap, bool) or cap <= 0:
        problems.append("known_cost_cap is not an explicit positive budget")
    if not isinstance(policy.get("currency"), str) or not policy.get("currency"):
        problems.append("currency is not set")
    return problems


def evaluate_live_gate(
    config_path: Path | None, opted_in: bool, template: Mapping[str, Any]
) -> LiveGate:
    if config_path is None:
        return LiveGate(opted_in, None, ("no --config policy was given",))
    try:
        raw = config_path.read_bytes()
    except OSError as error:
        return LiveGate(opted_in, None, (f"policy is unreadable: {type(error).__name__}",))
    digest = hashlib.sha256(raw).hexdigest()
    try:
        policy = json.loads(raw)
    except (UnicodeDecodeError, json.JSONDecodeError):
        return LiveGate(opted_in, digest, ("policy is not JSON",))
    problems = policy_problems(policy, template)
    if not opted_in:
        problems.insert(0, "live mode needs the explicit --live_opt_in flag")
    return LiveGate(opted_in, digest, tuple(problems))
