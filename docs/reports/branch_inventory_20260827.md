# Branch Inventory and Language Decision

Observed on 2026-08-27. This is a source/ref inventory, not a deployment report.

## Current Checkout and Delivery Location

The original checkout remains on `main` at `99d1f8e`, the Python bootstrap fix.
Its pre-existing untracked `target/` was preserved. The cached `origin/main`
was 27 commits ahead, and a fresh remote query confirmed `6498e6a`.

This implementation uses an isolated worktree on
`codex/harness_request_dispatch`, based exactly on `6498e6a`. Its changes are
uncommitted. No pull, stash, reset, clean, merge, commit, or push was performed.

## Branch Chain

All the following historical local branches are already ancestors of
`6498e6aa711f751d9f9e50e37034d61c17d4b341`. They are milestone references, not
unmerged competing implementations:

- `main` — `99d1f8e`: Python prototype and persistence fixes.
- `python_source_reference` — `9a27423`: Rust migration baseline.
- `python_reference_p0` — `4b76455`: frozen Python behavior contracts.
- `rust_core_refactor` — `10a6b11`: canonical Rust types and events.
- `integration_core` — `98f70a3`: integrated Rust and Python reference history.
- `rust_store_daemon` — `f782d1c`: atomic Rust SQLite store.
- `a2a_basic_share` — `7d1cf37`: bounded A2A information exchange.
- `probe_unified_frontend` — `df845c7`: probes, frontend shell, artifact ignores.
- `daemon_writer_worker` — `6a2c1ea`: dedicated single-writer worker.
- `daemon_local_ipc` — `423fb57`: authenticated local control transport.
- `daemon_process_cli` — `0267644`: on-demand daemon lifecycle.
- `daemon_stale_recovery` — `896ee18`: explicit stale-runtime recovery.
- `daemon_windows_security` — `dd013c6`: protected Windows control metadata.
- `plugin_protocol_core` — `68a1af8`: Protobuf v1 plugin contracts.
- `plugin_supervisor_core` — `fce05cf`: bounded plugin process supervisor.

The public remote merged the supervisor through `6498e6a`. The new dispatch
branch starts there; no reconstruction of these older branches is necessary.

## Independently Observed Remotes

A fresh read of each configured remote returned:

- Public `origin/main`: `6498e6aa711f751d9f9e50e37034d61c17d4b341`.
- Secondary `linux/main`: `99d1f8ebcf9e25248eb2af1eb6d64b530e51f848`.
- Both remotes' `plugin_supervisor_core`: `fce05cffe628ce65c49620a06f6bd1ef7eaadb89`.
- Both remotes' `probe_unified_frontend`: `df845c745ef246f468b4248edc48ff7e05b7d968`.
- Both remotes' `python_reference_p0`: `4b764558d2d6d12f145ae3b36228fdb8e5c1c2a4`.
- Both remotes' `rust_core_refactor`: `10a6b1184ed40f02c73a672b2e5ce6c85ba11e30`.
- Both remotes' `rust_store_daemon`: `f782d1cdb680cdc991655716dd05c18ed80b37e2`.

The two `main` refs differ; the listed feature refs agree. Remote tracking refs
alone were not treated as live remote evidence. Private addresses, SSH identity
paths, usernames, and transport configuration are intentionally omitted.

## Language Foundation

The earlier Rust migration request and ADR 013 agree: retain Rust 2024 for the
long-lived core, Tokio for bounded asynchronous I/O, Protobuf for the private
plugin boundary, and the existing single-writer SQLite architecture. Keep
Python as a reference and tooling surface.

Rust is selected for ownership, cancellation, Windows integration, and reuse of
the existing implementation. This decision is not a measured Rust-versus-Go
throughput result. Start with trace-size, queue-pressure, spawn, and cancellation
measurements before adding pools or changing the language again.

The toolchain is pinned to Rust 1.85 by the repository and was used for local
checks. No global toolchain, PATH, model default, or remote configuration was
changed.

## Publication and Cleanup

Review this additive slice, its validation evidence, and local-only configuration
before authorizing a commit or PR. Do not delete milestone branches merely
because they are merged. Any main fast-forward, secondary-remote update, branch
cleanup, or publication is a separate explicit action.

Implementation boundaries and usage are recorded in
[ADR 040](../adr/040_native_harness_request_dispatch.md) and
[the harness core guide](../guides/harness_core_guide.md).
