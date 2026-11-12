# Cloud harness integration guide

This guide describes the executable cross-project frame joining VibeMux and
Personal Dev Hub (the private "personal toolbox"). The cloud environment was
unavailable during the 2026-11-12 implementation round, so the frame runs
locally against a real daemon, a real Hub checkout, and task-owned fixtures.
Capabilities that are not implemented stay `not_run` in the report.

## What runs today

| Case | Requires | What it proves |
|---|---|---|
| `C01` | Built `vibemuxctl`/`vibemuxd`, Hub checkout, Node | The Hub's real descriptor reader and read-only IPC client authenticate to a real daemon, complete the additive v10 `capabilities` discovery, and reject unknown versions, operations, contract families, and catalog shapes |
| `A1` | Rust toolchain | The CLI graph is GUI-free; Web Push is opt-in |
| `A2` | Python 3.12 | Managed build-storage inventory is valid |
| `C11-SLOTS` | Python 3.12, Git | Two owned worktrees compete for one managed build slot: serialization, ownership, release, and owned cleanup |
| `C13-LINEAGE` | Rust toolchain | Known store layouts are recognized and preserved: current v9, PR #52 reports schema 10, PR #49 chat schema 10 and 14, a foreign Python store, and future/incomplete markers |
| `HUB-GATES` | Hub checkout, npm | The Hub's own check/test/artifact gates pass |

All other ids (`C02`–`C15`) are emitted with a precise missing-capability
reason. The report is the source of truth, not this table.

## Run

```bash
# 1. Build the daemon + CLI into a fixed slot.
export VIBEMUX_BUILD_STATE_ROOT="${XDG_CACHE_HOME:-$HOME/.cache}/vibemux/build-storage"
export VIBEMUX_TASK_ID="cloud-harness-framework"
python scripts/build_storage.py run --slot core -- \
  cargo build --locked -p vibemuxd --bin vibemuxd \
                 -p vibemux_cli --bin vibemuxctl

# 2. Run the frame.
python scripts/verify_fused_harness_cloud.py \
  --hub-root /path/to/personal_dev_hub \
  --vibemuxctl "$VIBEMUX_BUILD_STATE_ROOT/slots/core/target/debug/vibemuxctl" \
  --state-root "$PWD/.vmx-cloud-evidence/run" \
  --hub-gates \
  --report "$PWD/.vmx-cloud-evidence/fused_harness_local.json"
```

Exit code is `0` when no selected case failed; `blocked` and `not_run` do not
fail the run. Select cases with `--cases C01,A1,A2`.

## Isolation guarantees

- The script creates a fixture Git project under the task-owned state root and
  starts the daemon with that project root only.
- The Hub adapter is imported by absolute path from the supplied checkout. No
  Hub file is modified.
- No provider token is read or forwarded. No model call is started.
- The daemon is stopped in a `finally` block; the state root is removed unless
  `--state-root` or `--keep` is used.
- The report records `evidence_class` separately for actual daemon/Hub runs,
  source-graph checks, local tooling, and Hub tests.

## Report shape

```json
{
  "schema_version": 1,
  "environment": {
    "os": "darwin",
    "runtime_mode": "fixture",
    "rust": "cargo 1.92.0",
    "node": "v26.13.0",
    "vibemux_root": "...",
    "hub_root": "..."
  },
  "cases": [
    {
      "id": "C01",
      "status": "passed",
      "evidence_class": "actual_daemon_hub_adapter",
      "duration_ms": 3450,
      "detail": "descriptor_version=10 ...",
      "artifact_refs": [".../c01-hub-probe.json"]
    }
  ],
  "resources": {
    "state_root": "...",
    "before_bytes": 0,
    "after_bytes": 366260,
    "peak_bytes": null,
    "filesystem_free_before": 181304131584,
    "filesystem_free_after": 181300355072
  }
}
```

Allowed statuses: `passed`, `failed`, `blocked`, `not_run`. `peak_bytes` stays
`null` unless a measurement source provides it.

## Capability discovery

The daemon serves a read-only `capabilities` catalog at Control v10 (additive;
the descriptor version is unchanged). It lists every operation with its stable
name, minimum version, read/write access, schema ids, and handler group, plus
one row per harness runtime with build-supported actions, operator
configuration, probe observation, and enabled actions. The Hub adapter validates
the catalog as a closed record and exposes `discovery: capabilities | legacy |
unconfigured`; an older daemon falls back to the legacy read path. See
[ADR 058](../adr/058_capability_discovery.md).

## What the cloud/Hub work must add

| Case | Missing implementation |
|---|---|
| C02 | Hub goal submission and owner planning profile selection (capability discovery is complete; see C01) |
| C03 | Hub original-operation lookup after an uncertain submit |
| C04 | Hub event pagination and reconnect cursor |
| C05 | Hub generation-scoped cancellation |
| C06 | Writable fixture worker + independent verifier |
| C07 | Hub restart recovery over daemon records |
| C08 | Second fixture harness adapter |
| C09 | Hub `skill_refs` resolver |
| C10 | Hub read-only MCP tool + worker tool admission |
| C11 | Worker/candidate merge half (the managed-slot contention half is C11-SLOTS) |
| C12 | AAG real server source for v2 receipt interoperability |
| C13 | Forward merge of the recognized layouts (reports schema 10, chat schema 10-14) into one current schema; recognition and preservation are covered by C13-LINEAGE |
| C14 | Fusion offline benchmark contracts |
| C15 | Shared native host bridge and chat layouts |

Each addition should keep the same report schema and `evidence_class`
separation. Do not turn a fixture case into `actual_daemon_hub_fixture_worker`
without running that exact chain.

## Windows

`scripts/verify_fused_harness_windows.ps1` checks the native prerequisites,
builds in a managed slot, runs the Windows-only fixtures, and invokes this
Python frame for `A1`/`A2` (and `C01` when `-HubRoot` is supplied). Native
acceptance remains a separate result; WSL/Linux success does not replace it.

Live vendor smoke is not part of the default flow. It requires explicit
authorization, a limited budget, configured routes, and the operator's own
credentials.
