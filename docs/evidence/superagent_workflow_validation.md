# Superagent workflow enhancement validation

Date: 2026-11-09. Baseline: `adac4e42bc38cd481cb7541db6d59629c89eaf7a`.
Implementation branch: `codex/dsh-superagent-enhancements`.
Host: native macOS, Apple Silicon, Rust 1.92.0; offline fixture execution.
Upstream reference: `deepseek-ai/dsh-superagent-prototype` at
`8c32a9dc5d72c033816c6a8110b37f40cf94fba4`.

## Scope and boundaries

- Operator intake reuses existing workflow preparation, policy narrowing and
  immutable contract admission. It does not launch work.
- Follow-ups are authenticated operator context, fenced to a current known
  cooperative worker session/contract and delivered at a turn boundary.
- Schema-2 worker claims are stored atomically with content-free events;
  human questions fence progress and acceptance. Ready still needs review,
  trusted verification and integration.
- Report text stays in the existing opt-in content store. Content reads
  verify per-workflow retention permission/ownership and both digests.
- Control v11, store schema 10 and `worker_v2` are explicit private pre-alpha
  versions. `worker_v1` rendered bytes remain unchanged.

## Automated checks

Commands and actual outcomes are recorded below. The full workspace and baseline comparison failures are retained as evidence.

| Command | Outcome |
|---|---|
| `cargo check -p vibemux_workflow -p vibemux_store -p vibemuxd -p vibemux_cli --all-features --locked` | Passed before regression testing |
| `TMPDIR=/private/tmp cargo test -p vibemuxd --test workflow_superagent --all-features --locked` | Five new end-to-end scenarios passed in the final applicable regression command (initial three-test increment also passed) |
| `cargo fmt --all -- --check` | Passed |
| `TMPDIR=/private/tmp cargo test -p vibemux_workflow -p vibemux_store -p vibemux_cli -p vibemuxd --all-features --locked` | Failed at two legacy browser acceptance cases: the verifier searches Windows/Linux executable locations, not macOS Chrome |
| Same command with `-- --skip failed_answer_delivery_does_not_block_later_repair --skip two_tracks_build_taskboard_with_a_scoped_bundle_and_isolated_integration` | Passed: 644 tests, zero failures, two platform-dependent cases explicitly filtered (not passing) |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | Passed after building the existing web and desktop bundles |
| `python3 scripts/check_public_artifacts.py` | Passed: 696 publishable text files, zero findings |
| `TMPDIR=/private/tmp cargo test --workspace --all-features --locked` | Failed at three unchanged MCP lifecycle tests; see baseline comparison below |
| `TMPDIR=/private/tmp CARGO_TARGET_DIR=<main-worktree>/target cargo test -p vibemux_mcp --test lifecycle --all-features --locked` in the clean baseline worktree | Same three failures reproduced, 19 passed |
| `npm ci --no-audit --no-fund` and `npm run build` in `crates/vibemux_web/web` and `crates/vibemux_desktop/ui` | Both passed; generated bundles and dependencies are ignored, source/lockfiles unchanged. Host Node 26.13.0 differs from the frontend's pinned Node 22.22.0 |
| `cargo nextest run --workspace --all-features` | Unavailable: `cargo-nextest` is not installed (exit 101) |
| `cargo deny check` | Unavailable: `cargo-deny` is not installed (exit 101) |
| `cargo audit` | Unavailable: `cargo-audit` is not installed (exit 101) |

Tests cover strict intake, duplicate/conflicting admission, independent
verification despite ready claims, strict/stale/unknown-session follow-ups,
same-event concurrent retry, conflicting body refusal, literal shell-like
text in a worker prompt, restart delivery and acknowledgement, report
attribution/fences, audit/projection tampering refusal, migration/history preservation and rollback, default
privacy, opt-in content, purge, identical-content cross-workflow permission,
and missing-report restart refusal.

## Diagnosed intermediate failures

- The first CLI regression run inherited macOS's long temporary-directory
  path. Control socket names exceeded the native Unix socket limit, causing
  two recovery tests to fail with `EndpointUnavailable` before new behavior
  ran. Subsequent daemon/CLI checks use `TMPDIR=/private/tmp`. This is a
  test-host condition, not a Windows/Linux release verification.
- The historical schema-6 ledger fixture built from a current database
  removed later schema markers but retained the new reports table. Its
  downgrade setup now removes this test-owned schema-10 table as well, so
  the forward-migration test represents the historical schema accurately.
- Initial IPC assertions expected a generic remote error for malformed
  envelopes; the existing client maps `control_invalid_request` to typed
  `ControlError::InvalidRequest`. Assertions now require that existing typed
  refusal. No behavioral or safety assertion was removed.

## Native browser acceptance limitation

The two legacy `workflow_cooperate` acceptance tests above require a real
Chromium browser. The checked-in verifier searches fixed Windows and Linux
locations; the host's macOS Chrome location is absent from that list. Both
tests reported a blocked `browser_frontend_only` receipt. No test assertion,
browser substitute, search path, verifier or production feature was changed
to pass them. The applicable regression command filters only those two
explicit names. A filter is not Windows/Linux release evidence.

## Baseline MCP failures

The full workspace command failed at these unchanged `vibemux_mcp` lifecycle
tests, with 19 other tests in that target passing:

- `faults_and_partial_startup_reap_the_exact_owned_child`:
  `reader_cleanup_failed` during owned-child shutdown.
- `idle_worker_exit_unloads_capability_without_a_new_read`:
  session cleanup returned an error.
- `buffered_requests_after_a_slow_error_do_not_starve_expired_backoff`:
  the expected bounded reply was absent.

The exact same three failures reproduced in the separate, clean worktree at
baseline `adac4e42bc38cd481cb7541db6d59629c89eaf7a`. Its MCP fixture uses an
`unused_fixture_descriptor` and does not exercise the new workflow operations.
MCP and platform source diffs against baseline are empty. The native cleanup
cause remains an independent issue; no MCP implementation or timing assertion
was changed, skipped or retried blindly to hide it. Full workspace test
success is not claimed.

## Platform and release coverage

Windows native, Linux/WSL, live DSH/vendor turns, live terminal backends and
hosted CI were not exercised by this change. No source account, watcher,
model generation or external reply was invoked. These are offline Rust
integration results and do not advance live vendor or Windows release gates.
The unavailable nextest/deny/audit commands remain required CI/release checks.
