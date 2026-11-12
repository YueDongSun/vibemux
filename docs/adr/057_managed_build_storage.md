# ADR 057: Managed build slots, low-disk profiles, and split frontend dependencies

Status: Accepted, 2026-11-12.

## Context

The Rust workspace has 24 members and no `default-members` or profile overrides.
A bare `cargo build` therefore built every member, and `vibemux_cli` depended on
`vibemux_frontend`, which unconditionally compiled egui/eframe. `vibemux_web`
unconditionally compiled `ece`, vendored OpenSSL, and `p256` for optional Web
Push. Repeated validation in several worktrees created independent `target`
directories; one local checkout held 55 GiB of `target` output on a 460 GiB
volume.

`AGENTS.md` §7.8, §12.5, and §14.4 already require stable output ownership,
explicit cleanup plans, and scoped validation, but no executable entry point
enforced them.

## Decision

1. Add `scripts/build_storage.py` with `inventory`, `run`, `plan-clean`, and
   `apply-clean`. Builds run in fixed, reusable slots under one state root. A
   build token bounds concurrent heavy builds; an OS advisory slot lease
   prevents cleanup or a second build from touching an active slot. Cleanup
   deletes only an idle, registered, task-owned slot `target`, under the same
   lease, after re-verifying the plan.
2. Set repository-wide low-disk `dev`/`test` profiles: line-tables-only debug
   info, no incremental caches, no debug info for dependencies, and a separate
   `debugging` profile. Debug assertions and overflow checks are unchanged.
3. Add `default-members` for the daily core (`vibemux_cli`, `vibemuxd`,
   `vibemux_probe`, `vibemux_mcp`).
4. Make `vibemux_frontend`'s egui/eframe GUI an opt-in `gui` feature and gate
   the GUI modules, binaries, and GUI-only test target on it. The CLI keeps the
   dashboard/TUI helpers without a windowing stack.
5. Make Web Push an opt-in `push` feature in `vibemux_web`, including its
   routes, handlers, startup service, integration test target, and the
   `/api/v1/health` `push` capability flag. The default web build has no
   `ece`, no `openssl-sys`, and no `p256`.

## Alternatives

- Rely on `default-members` alone: rejected; it does not remove the CLI's
  transitive GUI dependency.
- Make the GUI a separate crate: deferred; a feature split is smaller and keeps
  the existing public module path for `--all-features` builds.
- Use one global `CARGO_TARGET_DIR`: rejected; it serializes unrelated tasks
  and offers no ownership or cleanup record.
- Add filesystem quotas in this change: rejected; they are platform-specific
  and need operator-level configuration. The sampled budget stays a soft guard
  and reports `resource_exhausted`.

## Consequences

- Core builds no longer compile egui/eframe. `--workspace --all-features` and
  `cargo test/clippy --all-features` still cover the GUI and push paths, so CI
  coverage is unchanged.
- The documented GUI command now needs `--features gui`. A push-enabled web
  install needs `--features push`.
- A managed build that exceeds its budget or breaks the reserve floor is killed
  and returns exit code 75. Work in that slot is reproducible, never canonical.
- Cleanup refuses unknown ownership and active leases. `--adopt` is the only
  way to claim a non-empty unregistered slot.
- Measurements and plans are JSON evidence, not canonical VibeMux state.

## Validation

- `scripts/build_storage.py` tests cover exit-code propagation, target-dir
  override refusal, budget exhaustion, reserve refusal, active-lease refusal,
  unknown ownership, path escape, and owned/idle cleanup.
- `cargo tree -p vibemux_cli` contains no egui/eframe/winit/Tauri.
- `cargo tree -p vibemux_web` (default) contains no ece/openssl/p256;
  `--features push` contains them.
- `cargo check`, `cargo test`, and the web contract tests run in managed slots
  with the low-disk profiles on macOS. Windows execution remains a separate
  native acceptance item.
