# Changelog

## Unreleased

- Fix stacked dashboard clipping the tenth agent (Kimi) at common terminal sizes: the agent table now reserves 13 rows and slot overflow announces itself with a `+N more slots` marker instead of truncating silently. Light-theme aggregate health text now uses the WCAG-audited dark variants (previously terminal green/yellow on white, ~1.71:1).
- Fix writer queue telemetry: enqueue rejection no longer leaves phantom depth behind, so `queue_depth`/`queue_high_watermark`/`queue_saturated` track the real bounded channel.
- Fix `vibemux status` and run listing for projects with pre-release runs whose persisted role is not one of `worker|reviewer|orchestrator`: unknown legacy role strings now read back as `worker` instead of raising.
- Fix Linux daemon startup for deeply nested project roots: the unix control socket now binds in the system temp directory under a short project-keyed per-instance name instead of inside the project runtime dir, staying within the 107-byte `sun_path` limit (previously any root nested deeper than ~50 characters failed with `EndpointUnavailable` at bind). The socket is owner-only (0600); endpoint and auth token remain anchored in the runtime-dir descriptor, and clients are unchanged.
- Fix worktree inventory on older Git: drop the Git 2.37-only `-z` flag from `git worktree list --porcelain` so run ownership inspection works with the Git 2.34 shipped by Ubuntu 22.04/WSL2 (previously `error: unknown switch 'z'`). Records whose path or branch contains a newline are now rejected fail-closed.
- Extend the probe and unified frontend to the Chinese CLI agents (qwen, iflow, trae, codebuddy, kimi): launcher and version probes join the report and the dashboard now renders ten agent rows with ten reserved native-TUI slots.
- Fix control-plane head-of-line blocking: control connections are now handled concurrently (per-connection task with JoinSet drain on graceful shutdown, pipe instance ceiling raised to 16), so an idle or slow client can no longer stall health checks and other control traffic; covered by a dedicated regression test.
- Add writer scheduling observability: `WriterHealth` and daemon health now report `queue_depth`, `queue_high_watermark`, and `queue_saturated`; a saturated queue no longer blinds health (the snapshot answers without a worker round trip), and `vibemuxctl daemon health` exposes the metrics.
- Add dashboard themes: `classic` (8-color default), `high-contrast` (light variants for bright environments and low-vision users), and `mono` (no foreground colors; bold/dim only) selectable via `--theme` or `VIBEMUX_FRONTEND_THEME`; all themes keep real probe data unclipped at common terminal sizes.
- Fix unified frontend agent-table truncation: split the combined launcher/auth/inference cell into per-state columns (no more clipped `I:not_r` on verified agents), raised the side-by-side layout threshold to 160 columns so the table keeps full width on common terminals, and added a real-data rendering regression test.
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
- Add built-in Chinese CLI agent harnesses: qwen (Qwen Code), iflow (iFlow CLI), trae (TRAE CLI), codebuddy (CodeBuddy Code), and kimi (Kimi Code CLI).
- Add `vibemux harnesses` (registry + availability) and `vibemux switch` (project default harness with `harness_switched` event); `spawn` falls back to the project default harness.
- Persist probes: `vibemux harnesses` writes the detection snapshot to `.vibemux/harnesses.json` (`--cached` reads it, `harness_probed` event recorded); `switch`/`spawn` gate on detection, rejecting undetected harnesses before any worktree is created.
- Persist run roles: `spawn --role worker|reviewer|orchestrator` stores the role on the Run and in events, shows it in `status`, and accumulates it into the harness snapshot (ADR 013).

## 0.1.0a0

- Establish the Windows-first Python package, domain models, SQLite event log, Git worktree, and terminal/harness adapters.
- Add the CLI, cross-platform smoke, documentation, ADRs, and GitHub Actions matrix.
