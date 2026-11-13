# Fused harness cloud validation — local evidence, 2026-11-12

Status: **local frame executed**; the cloud environment was unavailable. This
record contains only runs that actually completed on the listed host. Cases
that need a capability not present in this source revision are `not_run`, not
"passed".

## Environment

| Field | Value |
|---|---|
| Host | macOS 15.6 (Darwin 25.6.0), arm64 |
| Rust | 1.92.0 (pinned toolchain) |
| Node | 26.13.0 |
| Python | 3.12.16 (task venv) |
| VibeMux | `feat/cloud-harness-framework`, base `a06a8c7` |
| Hub | `fix/macos-state-path-ancestor-links`, base `codex/work_cloud_v0_2_0` |
| Runtime mode | fixture + real local daemon + real Hub adapter source |
| Provider calls | none |

## Case results

Command:

```bash
python scripts/verify_fused_harness_cloud.py \
  --hub-root ../personal_dev_hub \
  --vibemuxctl "$VMX_BUILD_ROOT/core/debug/vibemuxctl" \
  --state-root "$VMX_EVIDENCE_ROOT/fused-run" \
  --hub-gates \
  --report "$VMX_EVIDENCE_ROOT/fused_harness_local.json"
```

| Case | Status | Evidence class | Result |
|---|---|---|---|
| C01 | passed | `actual_daemon_hub_adapter` | Real daemon descriptor v10; Hub adapter authenticated `health` + `frontend_tasks`; `check=connected`; additive v10 `capabilities` discovery (`discovery=capabilities`, 47 operations, 11 runtimes); unknown operation → `vibemux_operation_not_allowed`; unknown descriptor version → `vibemux_ipc_version_mismatch`; unknown contract family → `vibemux_contract_family_mismatch`; unknown catalog shape → `vibemux_contract_mismatch` |
| A1 | passed | `source_dependency_graph` | CLI tree has no egui/eframe/winit/Tauri; default web tree has no ece/openssl/p256; `--features push` restores OpenSSL |
| A2 | passed | `local_build_tooling` | `build_storage.py inventory` returned schema 1 with free-space and slot data |
| C11-SLOTS | passed | `managed_build_slot_contention` | Two owned Git worktrees serialized on one `max-active=1` token; both task ids recorded in the slot owner; slot released; owned cleanup removed only `slots/c11/target` |
| C02-PLAN | passed | `planning_contract_fixture` | VibeMux 4 tests + Hub 3 tests: profile digest binding, retained request UUID, deterministic request key, content-free observation accepted by the Hub validator |
| C04-UI | passed | `hub_event_ui` | 3 Hub tests: paged history, exact next cursor, end marker, passive render |
| C07-HUB | passed | `hub_restart_recheck` | A restarted Hub reports not_checked and re-reads the daemon (1 test) |
| C05-CANCEL | passed | `actual_daemon_dispatch_cancellation` | 3 tests: live capture, cancel, and shutdown of a waiting probe |
| C08-HANDOFF | passed | `actual_daemon_context_handoff` | 2 tests: an operator share reaches the other track and survives restarts |
| C10-MCP | passed | `metadata_services_fixture` | VibeMux MCP 3 tests + Hub MCP 15 tests; both services initialize and serve tools |
| C09 | passed | `skill_ref_resolution` | 6 Hub tests: exact content hash, bounded bundle, execution_authority=none, escape/symlink/hardlink/size refusals |
| C11-MERGE | passed | `actual_daemon_two_worker_integration` | Two workers build in isolated worktrees, review, and integrate through the scoped workflow path (2 tests) |
| C14 | passed | `offline_fusion_contracts` | 14 ported contract tests: modes, budgets, scoring, cancellation, append-only evidence; no provider call |
| C06 | passed | `actual_daemon_fixture_worker_verifier` | Real in-process daemon + store + Git worktrees + writable fixture worker: reviewer passes, independent verifier rejects the wrong candidate (`gate_suite_failed`), integration stays empty, checkout clean; good candidate accepted |
| C04-EVENTS | passed | `event_pagination_contracts` | Store paging (31 events, cursor, retention floor), the v10 control read, and the Hub cursor consumer (34 Hub tests) |
| C07-RESTART | passed | `daemon_restart_store_reuse` | Daemon restart re-served health and 48 capabilities and reopened the same store inode; pid changed |
| C13-LINEAGE | passed | `store_migration_tests` | 6 lineage tests: unified schema 15; reports-10 reconciled without a duplicate marker; chat-10/14 upgraded; failed reconciliation rolls back; foreign/future/incomplete refused |
| HUB-GATES | passed | `actual_hub_tests` | `npm run check`, `npm test` (2032 passed, 2 skipped, 0 failed), `npm run verify:artifacts` |
| C02–C15 | not_run | `not_executed` | Reasons are recorded per case in the JSON report |

C01 detail:

```text
descriptor_version=10
health.healthy=true
check.status=connected
discovery=capabilities
catalog.control_contract_id=vibemux.control.v10
catalog.operations=48 (capabilities: minimum_version=10, access=read)
catalog.runtimes=11
frontend_tasks.tasks=[] (fresh fixture project)
unknown_operation_error=vibemux_operation_not_allowed
unknown_version_error=vibemux_ipc_version_mismatch
unknown_family_error=vibemux_contract_family_mismatch
unknown_shape_error=vibemux_contract_mismatch
```

The real `vibemuxctl capabilities --project-root <fixture>` output was also
captured: contract `vibemux.control.v10`, 47 operations, 11 runtimes, and an
unconfigured Claude row with `supported_actions=[probe,submit]`,
`enabled_actions=[]`, `qualified_actions=[]`, and
`unavailable_reason=route_not_configured`.

The daemon was stopped in the case's `finally` block; no `vibemuxd` process
remained.

## Hub baseline repair

The Hub's `npm test` failed on this macOS host before any Hub change: 33 local
tests failed with `rd_session_store_path_unsafe` or
`device_state_path_unsafe`, plus two integration export tests that compared a
canonicalized directory with the raw temporary root.

The fix (Hub commit `0d24222`):

- canonicalize the existing prefix of a configured state root, verify the
  canonical prefix is made of real directories, and keep checking the managed
  path and every component below it with `lstat`;
- still reject a configured root or managed child that is itself a link,
  including a Windows junction;
- compare the integration export directory with the canonical project root.

After the fix: `npm run check` passed, `npm test` reported **2032 passed, 0
failed, 2 skipped** (the count includes the new capability-discovery cases),
and `npm run verify:artifacts` passed.

## Capability discovery increment
The additive v10 `capabilities` operation and the Hub consumption landed after
the first evidence capture (ADR 058). The C01 row and detail above are from the
re-run against a real daemon and the real Hub adapter. The Hub suite grew from
2026 to 2032 passing tests; the VibeMux side added 4 daemon tests, the CLI verb
and its tests, and the C01 assertions.

## Store lineage increment

The store now classifies a database before writing to it (`crates/vibemux_store/src/lineage.rs`).
`MAX(schema_migrations.version)` alone is not enough: PR #52 and PR #49 both
used version 10 for different DDL, and the Python reference store shares the
`schema_migrations` table with a different `events` shape. The recognizer
combines the migration history with the table, column, and partial-index set
and reports:

| Layout | Result |
|---|---|
| Current v9 | opened; reopen is idempotent (no second migration) |
| Rust v1-v8 | forward migration through v9, then a post-migration object check |
| PR #52 reports schema 10 | `store_known_lineage`, database left unchanged |
| PR #49 chat schema 10-14 | `store_known_lineage`, database left unchanged |
| Python VibeMux store | `store_unsupported_database`, database left unchanged |
| Marker newer than every lineage | `store_newer_schema` |
| Current marker with missing objects | `store_schema_incomplete`, listing the missing objects |

The forward merge of the recognized layouts into one current schema is not
implemented; that is the remaining part of C13.


## Additional VibeMux gates

Executed on the same host and managed slot:

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | passed |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | passed (49 s) |
| `cargo test --locked --workspace --all-features` | **1724 passed, 0 failed, 2 ignored, 151 targets** (final run after schema 15 and the planning contract; 373.9 s warm) |
| `cargo deny check` | advisories ok, bans ok, licenses ok, sources ok |
| `python -m pytest` | passed (2 skipped groups) |
| `ruff check scripts tests`, `ruff format --check` | passed |
| `mypy scripts/build_storage.py scripts/verify_fused_harness_cloud.py` | passed |
| `python scripts/check_public_artifacts.py` | passed, 0 findings |

The managed core slot held 14.1 GB after the full scope (including Tauri);
the comparison slots were cleaned after measurement.

## What this evidence does and does not establish

- It establishes that the current VibeMux daemon descriptor and read-only
  operations are consumable by the Hub's real adapter code on POSIX.
- It does **not** establish goal submission, planning profiles, worker
  execution, follow-up, dual worker, migration lineage, AAG interoperability,
  Fusion benchmarks, or the shared native UI. Those are `not_run` with
  explicit missing capabilities.
- It does not run on Windows. See
  [scripts/verify_fused_harness_windows.ps1](../../scripts/verify_fused_harness_windows.ps1)
  and the [cloud integration guide](../guides/cloud_harness_integration.md).
- The Hub is private; this public repository records only the gate result and
  the public contract exercised, not Hub source or private configuration.

## Reproduce

```bash
# Build the daemon and CLI into a managed slot.
python scripts/build_storage.py run --slot core -- \
  cargo build --locked -p vibemuxd --bin vibemuxd \
                 -p vibemux_cli --bin vibemuxctl

# Run C01/A1/A2 and the Hub gates.
python scripts/verify_fused_harness_cloud.py \
  --hub-root <hub-checkout> \
  --vibemuxctl <managed-slot>/debug/vibemuxctl \
  --hub-gates \
  --report <evidence>/fused_harness_local.json
```
