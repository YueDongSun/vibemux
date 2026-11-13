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
| `C07-RESTART` | Built `vibemuxctl` | Daemon stop/restart reopens the same store and re-serves health and capability discovery |
| `C02-PLAN` | Rust toolchain, Node, Hub checkout | Planning request identity: profile digest, retained request UUID, deterministic request key, and a content-free observation the Hub validator accepts |
| `C04-UI` | Node, Hub checkout | Workspaces paged event history with the exact next cursor, end marker, and passive render |
| `C07-HUB` | Node, Hub checkout | A restarted Hub reports `not_checked` and re-reads the daemon instead of trusting a cache |
| `C05-CANCEL` | Rust toolchain | A running dispatch reports live capture and cancels; a waiting probe is cancelled at shutdown |
| `C08-HANDOFF` | Rust toolchain | An operator context share reaches the other track and survives restarts |
| `C10-MCP` | Rust toolchain, Node, Hub checkout | Both read-only metadata services initialize and serve tools (worker-to-Hub tool calls are not claimed) |
| `C09` | Node, Hub checkout | Resolves a declared skill ref into a bounded context bundle with an exact content hash and `execution_authority: none`; escapes, links, non-UTF-8, and oversized content are refused |
| `C11-MERGE` | Rust toolchain | Two owned workers build in isolated worktrees, review, and integrate through the scoped workflow path |
| `C14` | Rust toolchain | Fusion Single/Cascade/Critique contracts run offline with deterministic fixtures; no provider call |
| `C06` | Rust toolchain | A real in-process daemon, store, Git worktrees and writable fixture worker: the independent trusted verifier rejects a wrong candidate while the reviewer passes it, and accepts the good candidate through the same path |
| `C04-EVENTS` | Rust toolchain, Node, Hub checkout | Paged task events with a sequence cursor, retention-floor resynchronization, and the Hub consumer |
| `C13-LINEAGE` | Rust toolchain | Unified schema 15: PR #52 reports-10 is reconciled in place, PR #49 chat-10/14 is upgraded, current v15 reopens idempotently, and foreign/future/incomplete layouts are refused without modification |
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
| C02 | Daemon planning-profile compiler and end-to-end Hub goal submission (the request/observation contract is covered by C02-PLAN) |
| C03 | Lost-submit replay through a real Hub consumer (the retained-UUID request key is covered by C02-PLAN) |
| C05 | Hub generation-scoped cancellation (daemon dispatch cancellation is covered by C05-CANCEL) |
| C06 | Out-of-process fixture worker on a stock daemon (the in-process fixture path is covered by C06) |
| C08 | A second harness adapter taking over with its own native identity (cross-track sharing is covered by C08-HANDOFF) |
| C09 | Planning-path consumption of resolved skills (the resolver and bundle are covered by C09) |
| C10 | A VibeMux worker calling Hub tools through an admitted broker (both metadata services are covered by C10-MCP) |
| C12 | AAG real server source for v2 receipt interoperability |
| C13 | Forward merge of the recognized layouts (reports schema 10, chat schema 10-14) into one current schema; recognition and preservation are covered by C13-LINEAGE |
| C14 | Live Fusion measurement (offline contracts are covered by C14) |
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
