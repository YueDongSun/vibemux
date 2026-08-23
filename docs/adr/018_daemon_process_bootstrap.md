# ADR 018: On-demand daemon process bootstrap and migration-safe CLI

Status: Accepted for pre-alpha implementation

## Context

The writer worker and authenticated local control transport now exist as library components. M3 still lacks a standalone process owner and a thin client that can start, query, and stop it. Process bootstrap is also a migration boundary: the installed Python CLI currently owns `.vibemux/vibemux.sqlite3`, so a Rust daemon must not open that file before an explicit schema migration and cutover.

PID existence alone cannot prove readiness or identity. Startup can race with another client, and an interrupted process can leave a descriptor, socket, or writer lock. Automatically deleting those artifacts or killing a PID would risk removing a live replacement instance or terminating an unrelated reused PID.

## Decision

- Add an unprivileged foreground `vibemuxd` binary. It derives all runtime paths from one canonical project root, starts `DaemonControlServer`, and waits until authenticated IPC shutdown completes.
- During migration, Rust state uses `.vibemux/vibemux_rust.sqlite3`. Neither the daemon binary nor the lifecycle CLI accepts a public database-path override. The Python `vibemux.sqlite3` file remains untouched.
- Add a separate pre-alpha `vibemuxctl` binary for `daemon start`, `daemon health`, and `daemon stop`. It does not replace the installed Python `vibemux` command before cutover.
- On POSIX, `daemon start` launches `vibemuxd` directly with argv, null stdin/stdout/stderr, and a separate process group. On Windows, it launches the system Windows PowerShell with a fixed embedded companion program, fixed flags, and user paths passed only through environment variables. The companion uses `ProcessStartInfo`/`ShellExecute` with a hidden window, retains the exact child process handle until readiness, and accepts only `release` or `terminate` over stdin. No user text enters PowerShell source or a command string.
- The Windows companion is itself created in a hidden new process group. It prevents the daemon from inheriting a caller's captured stdout pipe, which would otherwise keep `vibemuxctl daemon start | ...` open until daemon shutdown.
- The development-only daemon executable override is an explicit path argument and is never interpreted as a command string.
- Readiness requires a valid runtime descriptor followed by an authenticated versioned health response. A spawned PID or descriptor existence alone is insufficient.
- Concurrent starts converge through the existing exclusive writer lock and health probe. A client that observes an already healthy daemon reports `already_running` rather than spawning another writer.
- A descriptor or writer lock that does not converge to authenticated health within the bounded startup window is reported as `daemon_stale_runtime`. Bootstrap never auto-deletes runtime artifacts, resets the database, or kills a process it did not spawn.
- Startup timeout may terminate and reap only the exact child created by that bootstrap call.
- CLI stdout is bounded structured JSON containing status, process ID when newly spawned, and non-secret health fields. Errors expose stable codes only; database paths, endpoints, bearer tokens, and raw OS error text are omitted.

## Consequences

- The Rust daemon can be exercised at a real process boundary without taking ownership of the Python reference database.
- Packaging must place `vibemuxctl` and `vibemuxd` together or provide an explicit development executable path.
- Windows lifecycle bootstrap depends on the system Windows PowerShell compatibility boundary until a reviewed native safe process-launch API can provide an exact handle allowlist.
- Crash recovery remains fail-closed until a later explicit inspect-and-recover command proves process identity and artifact ownership.
- The daemon is on-demand and user-mode, not an installed privileged service.
- A future cutover must migrate the Python database under an exclusive migration lock and deliberately change the canonical Rust database contract.

## Alternatives

- Reuse `.vibemux/vibemux.sqlite3`: rejected because Python and Rust could become concurrent writers to incompatible schemas.
- Treat a child PID as ready: rejected because the process may fail before binding IPC or may be a reused unrelated PID after a crash.
- Delete stale descriptor/lock files during every start: rejected because absence of health does not prove that the artifacts are unowned.
- Install a Windows service now: rejected because M3 requires an unprivileged on-demand process and does not need system-wide persistence.
- Replace the Python `vibemux` executable immediately: rejected because Rust command parity and database migration are incomplete.
- Use direct `std::process::Command` detachment on Windows: rejected after a real captured-stdout test proved that an unrelated inheritable pipe handle can keep the launching pipeline open for the daemon lifetime.
