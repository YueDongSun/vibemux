## Scope
- issue/task: M4.2 daemon plugin registry and read-only status slice.
- owned modules: daemon registry/startup/control, supervisor cleanup and mock fixtures, related protocol documentation. Baseline `85697d6`; branch `codex/m4_2_plugin_registry`.

## Implemented
- Explicit bounded startup registration, lifetime restart budgets, capped backoff, terminal quarantine, and negotiated heartbeat grace.
- Authenticated IPC v2 `plugin_status`, with v1 health/shutdown wire compatibility and correct v1 errors.
- Cancellation-safe worker joins and supervised cleanup before releasing the daemon writer.

## Behavior and invariants
- Default startup loads no plugins. Status is read-only and never contacts a child or changes a budget.
- No registry store/writer handle and no Task/Run mutation operation. Unsolicited plugin requests/responses/events quarantine.
- Failed reap confirmation retains PID/session evidence in quarantine. A shutdown acknowledgement is acceptance, not completion.

## Files changed
- `crates/vibemuxd/src/plugin_registry.rs`, `plugin_configuration.rs`, `control.rs`, `lib.rs`, `bin/vibemuxd.rs`, `Cargo.toml`, and `tests/plugin_registry.rs`.
- `crates/vibemux_plugin_supervisor/src/supervisor.rs`, `src/bin/vibemux_mock_plugin.rs`, and `tests/process_supervisor.rs`.
- `crates/vibemux_plugin_protocol/src/negotiation.rs`: expose the negotiated heartbeat interval for safe watchdog validation; no schema change.
- `Cargo.lock`: only existing local crate dependencies and the existing `sysinfo` test dependency; no dependency version changes.
- `PROGRESS.md`, `CHANGELOG.md`, `docs/architecture.md`, `docs/protocol_boundaries.md`, `docs/plugin_registry_protocol.md`, `docs/adr/023_daemon_plugin_registry.md`, and this handoff.

## Validation executed
- command: `cargo test --locked --offline --workspace --all-features`
- result: 126 passed, 0 failed, 0 ignored; includes 9 daemon registry and 13 supervisor real-child integration tests.
- command: `cargo clippy --locked --offline --workspace --all-targets --all-features -- -D warnings`
- result: passed.
- command: `cargo fmt --all -- --check` and `git diff --check`
- result: passed; added-line privacy and local Markdown link checks also passed.
- command: `cargo nextest run --workspace --all-features`, `cargo deny check`, `cargo audit`
- result: attempted; each unavailable with exit 101, subcommand not installed.
- Regression evidence: queued-frame shutdown false timeout and incorrect v1 auth-error version were reproduced before correction; final tests pass.

## Platform coverage
- Windows: native Rust 1.85.0; real child pipes, authenticated named pipe, separate daemon startup, explicit config, exact-child process absence, and existing ACL/lifecycle tests.
- Linux/WSL: not executed for M4.2.
- live terminal backend: not executed; no integration claimed.

## Compatibility / migration
- IPC v2 adds status; v1 health/shutdown wire requests remain accepted. Old v1 binaries reject v2 descriptors and must be upgraded.
- SQLite, plugin Protobuf, and manifest versions are unchanged. No state migration, Python cutover, or public SDK contract.
- Rollback: stop the daemon normally and restart without `--plugin-config`.

## Security impact
- No new Task/Run mutation or direct database access. Status omits paths, argv, environment, prompts, tokens, and raw diagnostics.
- New launch configuration is explicit operator authority, not discovery or permission self-grant. No shell interpolation or inherited environment is added.
- Trusted executables are not an OS sandbox; filesystem/network and descendant-tree isolation remain outside scope.

## Remaining work
- Full M4 release gates remain incomplete: Linux/WSL, nextest/deny/audit, fuzz/soak, plugin-kind parity, operator recovery, SDKs, and vendor/terminal integration.
- Changes are locally reviewable and uncommitted; no publication was requested or performed. Pre-existing `.weave/` remains untouched. The two task-created worker worktrees were backed up and removed without force; only the implementation worktree remains.

## Known risks
- Registry status and budgets are ephemeral to a daemon lifetime; no persisted plugin lifecycle audit or quarantine recovery.
- Drop signals best-effort cleanup but is not a join receipt. Abrupt daemon termination and child descendants are outside the verified guarantee.
- Forced or unconfirmed cleanup reports errors; process absence is claimed only where a real test observed it after cleanup.
