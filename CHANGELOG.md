# Changelog

## Unreleased

- Add local stateful A2A HTTP+JSON/JSONRPC/gRPC, atomic schema-2 binding/verification commands, owned Run workspaces, and an explicit supervisor workflow using separate model peers. Add official TCK tooling and bidirectional Python/Go SDK interoperability fixtures; remote/TLS and full ITK remain outside verified scope.

- Add M4.2 daemon-owned plugin registry: explicit bounded startup configuration, lifetime restart budgets/backoff/quarantine, read-only IPC v2 status with v1 health/shutdown compatibility, and joined plugin cleanup before writer shutdown. No Task/Run mutation or SQLite/plugin-wire migration.

- Persist Run base commit and fix committed/staged/unstaged/untracked diff semantics.
- Introduce an injected shell-free `CommandRunner` for Git, terminal, and harness host code.
- Add SQLite forward migration and a controlled Windows PowerShell companion launcher.
- Add immutable cleanup plan, spawn compensation, persistent mock inventory, and resource reconciliation.
- Require structured-adapter, verifier, or explicit user authority for a Run to enter `succeeded`.
- Freeze Python domain/event/cleanup/terminal contract fixtures for reuse in Rust parity tests.
- Add Rust `vibemux_store`: SQLite migration, WAL, atomic state/event, idempotency, and replay foundation.
- Add Rust `vibemux_a2a`: official A2A v1 SDK Agent Card discovery and local HTTP+JSON basic information-share.
- Add Rust `vibemux_probe` and `vibemux_frontend`: fixed agent/gateway/A2A read-only probe, unified Ratatui dashboard, and five native-TUI reserved slots.
- Add Rust `vibemuxd::WriterWorker`: exclusive lifecycle lock, dedicated SQLite owner, bounded queue, backpressure, health, and graceful shutdown core.
- Add Rust authenticated local control transport: Windows named pipe, POSIX UDS, versioned bounded framing, OS-random token, health/shutdown, and owner-safe runtime cleanup.
- Add standalone `vibemuxd` and pre-alpha `vibemuxctl`: on-demand start/health/stop, migration-isolated database, stale fail-closed, and cross-platform process lifecycle validation.
- Add explicit `daemon inspect/recover`: PID/liveness blocking, snapshot-bound confirmation, unchanged-artifact cleanup, and recovery without database modification.
- Add Windows protected per-user control runtime, remote-pipe rejection, dual-generation writer locks, and legacy health/recovery compatibility.
- Add M4.0 plugin protocol foundation: checked-in Protobuf v1, bounded framing, validated manifest, policy negotiation, session lifecycle, and contract fixtures.
- Add M4.1 process supervisor foundation: clean argv spawn, real stdio handshake, bounded queues/stderr, deadline/heartbeat/cancel/shutdown, and crash containment.

## 0.1.0a0

- Establish the Windows-first Python package, domain models, SQLite event log, Git worktree, and terminal/harness adapters.
- Add the CLI, cross-platform smoke, documentation, ADRs, and GitHub Actions matrix.
