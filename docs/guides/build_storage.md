# Managed build storage and low-disk profiles

VibeMux is a 24-member Cargo workspace plus three npm trees. An ordinary
validation round used to create another full `target` copy per conversation,
and the CLI pulled the egui/eframe GUI stack through `vibemux_frontend` even
when it only needed the dashboard helpers. This guide describes the managed
storage entry point (`scripts/build_storage.py`), the repository-wide low-disk
profiles, and the dependency boundaries that keep the daily build graph small.

## Quick start

All commands run from the repository root. Use one fixed build slot per task;
do not delete another task's slot.

```powershell
# PowerShell (Windows)
$env:VIBEMUX_BUILD_STATE_ROOT = "$env:LOCALAPPDATA\VibeMux\build-storage"
$env:VIBEMUX_TASK_ID = "cloud-harness-framework"
python scripts\build_storage.py inventory
python scripts\build_storage.py run --slot core -- cargo check -p vibemux_cli -p vibemuxd
```

```bash
# POSIX
export VIBEMUX_BUILD_STATE_ROOT="${XDG_CACHE_HOME:-$HOME/.cache}/vibemux/build-storage"
export VIBEMUX_TASK_ID="cloud-harness-framework"
python scripts/build_storage.py inventory
python scripts/build_storage.py run --slot core -- cargo check -p vibemux_cli -p vibemuxd
```

`run` sets `CARGO_TARGET_DIR` to
`$VIBEMUX_BUILD_STATE_ROOT/slots/<slot>/target`, sets `CARGO_INCREMENTAL=0`
unless `--allow-incremental` is passed, and records a JSON measurement under
`.../measurements/`.

## Commands

```text
build_storage.py inventory [--repo-root DIR] [--deep] [--json]
build_storage.py run [options] -- <command> <args...>
build_storage.py plan-clean --task-id ID [--slot NAME ...] [--json]
build_storage.py apply-clean <plan.json> --task-id ID [--json]
```

Global option: `--state-root DIR` (default `VIBEMUX_BUILD_STATE_ROOT`, then the
OS user cache directory).

### `run`

| Option | Default | Meaning |
|---|---|---|
| `--slot NAME` | `core` | Fixed, reusable output directory |
| `--task-id ID` | `VIBEMUX_TASK_ID` | Registration/cleanup ownership |
| `--budget-bytes N` | 12 GiB | Per-slot size budget (`K`/`M`/`G`/`T` accepted) |
| `--reserve-bytes N` | 5 GiB | Filesystem free space to preserve |
| `--max-active N` | 1 | Concurrent managed builds across slots |
| `--lease-timeout S` | 600 | Wait for a build token |
| `--poll-seconds S` | 2 | Storage sampling period |
| `--allow-incremental` | off | Leave `CARGO_INCREMENTAL` unset for this call |
| `--adopt` | off | Explicitly claim a non-empty unowned slot |
| `--retain PATH` | — | Slot-relative path that cleanup must never delete |

The wrapper validates explicit `--target-dir` arguments. A command that
overrides the managed target is refused before any process starts. The child
inherits the wrapper's `CARGO_TARGET_DIR`; Cargo's own precedence is
`--target-dir` > `CARGO_TARGET_DIR` > `config.toml`, so the check covers the only
higher-priority source.

A build token and a slot lease are both OS advisory locks. They are released
automatically if the owning process dies. Touching the slot budget or the
reserve floor terminates the owned process tree (`SIGTERM` then `SIGKILL` on
POSIX, `taskkill /T /F` on Windows) and returns exit code `75`
(`EX_TEMPFAIL`) with `status: "resource_exhausted"` and a `stop_reason`.

### `inventory`

Reports the state root, filesystem free bytes, every registered slot (owner
task ids, active lease, logical and allocated size), unknown slot directories,
and external targets such as `CARGO_TARGET_DIR` or `<repo>/target`. The
unmanaged repository `target` is not measured unless `--deep` is passed, so the
command stays cheap on a large legacy checkout.

### `plan-clean` / `apply-clean`

A plan is written to `.../plans/<timestamp>-clean.json`. It contains only slots
that are:

- registered with an `owner.json` that lists the requested `--task-id`;
- idle (the slot lease can be taken);
- non-empty and marked reproducible (`target` is not in `retained`);
- still present.

Unknown ownership, another task's slot, and active slots are listed under
`skipped` with a stable reason. `apply-clean` re-verifies every path under the
same lease, refuses symlinks and any path that is not the slot's managed
`target`, deletes with `shutil.rmtree`, and reports freed logical/allocated
bytes plus the filesystem free-space delta.

Only `slots/<name>/target` is ever a deletion candidate. `owner.json`, retained
slot-relative paths (`install`, `evidence` by default), source, Git metadata,
lockfiles, `.vibemux` state, and measurement/plan evidence are not deleted by
this tool.

## Low-disk profiles

`Cargo.toml` defines:

```toml
[profile.dev]
debug = "line-tables-only"
incremental = false

[profile.dev.package."*"]
debug = false

[profile.test]
debug = "line-tables-only"
incremental = false

[profile.test.package."*"]
debug = false

[profile.debugging]
inherits = "dev"
debug = 2
incremental = true
```

Debug assertions, overflow checks, and panic behavior are unchanged. Use
`--profile debugging` for variable-level debugging. Environment variables still
win: `CARGO_INCREMENTAL=1` re-enables incremental compilation for one managed
call. Set `CARGO_INCREMENTAL=0` explicitly when using a debugging profile
through the wrapper, or pass `--allow-incremental` and set it yourself.

## Build recipes

A bare `cargo build` builds the `default-members`: `vibemux_cli`, `vibemuxd`,
`vibemux_probe`, and `vibemux_mcp`. The other surfaces are explicit:

```bash
# Core (daemon, CLI, probe, MCP). No egui/eframe, no Tauri, no OpenSSL.
cargo build

# TUI, dump, dashboard, DSH surface. No egui/eframe.
cargo build -p vibemux_frontend \
  --bin vibemux_frontend_tui --bin vibemux_frontend_dump \
  --bin vibemux_dashboard --bin vibemux_dsh_tui

# egui/eframe GUI (needs the `gui` feature).
cargo build -p vibemux_frontend --features gui --bin vibemux_frontend

# Web monitor. Web Push and its ece/openssl-sys/p256 stack are opt-in.
(cd crates/vibemux_web/web && npm ci && npm run build)
cargo build -p vibemux_web
cargo build -p vibemux_web --features push   # only for a push-enabled install

# Tauri desktop shell (needs the UI bundle).
(cd crates/vibemux_desktop/ui && npm ci && npm run build)
cargo build -p vibemux_desktop

# Full workspace (after every frontend dist exists).
cargo build --workspace --all-features
```

`cargo test --workspace --all-features` and
`cargo clippy --workspace --all-targets --all-features` still cover every
feature, including `gui` and `push`, so CI keeps full coverage.

## Measuring a round

```bash
python scripts/build_storage.py run --slot core --json -- \
  cargo test --locked -p vibemux_harness -p vibemux_harness_native --all-features
python scripts/build_storage.py inventory --deep --json
python scripts/build_storage.py plan-clean --task-id "$VIBEMUX_TASK_ID" --json
```

The measurement records wall time, exit code, before/after/peak slot size,
filesystem free bytes before/after/delta, budget, reserve, and the resolved
target directory. `allocated_bytes` is `null` when the platform cannot report
`st_blocks`; it is never guessed.

## Limits

- The budget is a soft, sampled protection, not a filesystem quota. For a hard
  limit use the platform quota mechanism.
- A slot is concurrency isolation, not a security boundary.
- `--adopt` is the only way to claim a non-empty, unregistered slot. Unknown
  ownership is preserved and reported.
- The script does not delete worktrees, installed programs, Cargo caches, npm
  caches, or another task's output. Those follow the cleanup rules in
  [AGENTS.md §12.5](../../AGENTS.md).
