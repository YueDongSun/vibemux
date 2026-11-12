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
| C01 | passed | `actual_daemon_hub_adapter` | Real daemon descriptor v10; Hub adapter authenticated `health` + `frontend_tasks`; `check=connected`; unknown operation → `vibemux_operation_not_allowed`; unknown descriptor version → `vibemux_ipc_version_mismatch` |
| A1 | passed | `source_dependency_graph` | CLI tree has no egui/eframe/winit/Tauri; default web tree has no ece/openssl/p256; `--features push` restores OpenSSL |
| A2 | passed | `local_build_tooling` | `build_storage.py inventory` returned schema 1 with free-space and slot data |
| HUB-GATES | passed | `actual_hub_tests` | `npm run check`, `npm test` (2026 passed, 2 skipped, 0 failed), `npm run verify:artifacts` |
| C02–C15 | not_run | `not_executed` | Reasons are recorded per case in the JSON report |

C01 detail:

```text
descriptor_version=10
health.healthy=true
check.status=connected
frontend_tasks.tasks=[] (fresh fixture project)
unknown_operation_error=vibemux_operation_not_allowed
unknown_version_error=vibemux_ipc_version_mismatch
```

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

After the fix: `npm run check` passed, `npm test` reported **2026 passed, 0
failed, 2 skipped**, and `npm run verify:artifacts` passed.

## Additional VibeMux gates

Executed on the same host and managed slot:

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | passed |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | passed (49 s) |
| `cargo test --locked --workspace --all-features` | **1702 passed, 0 failed, 2 ignored, 151 targets** (638.9 s cold, 363.6 s warm) |
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
