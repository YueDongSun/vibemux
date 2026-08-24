# ADR 015: On-demand daemon and single authoritative writer

Status: Accepted; supersedes [ADR 011](011_no_daemon.md)

## Context

ADR 011 correctly kept the first Python prototype inspectable by running services inside each CLI process. That model cannot safely own long-lived plugin processes, A2A streams, bounded event routing, cancellation, reconciliation, or a strict single-writer database contract. Multiple CLI processes opening SQLite directly would make authoritative transition ownership ambiguous.

VibeMux remains local-first and must not require an unauthenticated network service or a permanently installed system service.

## Decision

The Rust core runs as an on-demand, per-user `vibemuxd` process. It is the only authoritative writer for canonical state after cutover. CLI, UI, and plugins send validated requests through an authenticated local control channel; they do not write state tables directly.

The preferred local transports are Windows named pipes and POSIX Unix-domain sockets. The control API does not require an unauthenticated localhost TCP listener. Daemon startup, lock acquisition, health, graceful drain, shutdown, crash recovery, and stale-instance reconciliation are explicit lifecycle states.

During migration, the Python prototype and Rust daemon must never write the same database concurrently. A migration utility may write only while the daemon is stopped and an exclusive migration lock is held.

## Consequences

- State transitions and their audit events have one process owner.
- Long-lived plugin and A2A tasks gain a supervision and cancellation root.
- CLI startup now includes daemon discovery or on-demand bootstrap.
- Local IPC authentication, version negotiation, upgrade behavior, and support diagnostics require dedicated tests.
- The early no-daemon implementation remains valid historical context but is no longer the target architecture.

## Alternatives

- Keep all services in every CLI process: rejected because it cannot enforce one writer or supervise shared long-lived resources.
- Install an always-running privileged Windows service: rejected for the initial design because it expands installation and privilege complexity beyond current requirements.
- Use unauthenticated localhost TCP: rejected because local TCP broadens exposure and does not provide the desired platform ownership semantics.
- Allow CLI read/write database access alongside the daemon: rejected because it violates the single-writer invariant and complicates migrations and replay.
