# ADR 019: Explicit stale-runtime inspection and recovery

Status: Accepted for pre-alpha implementation

## Context

An abrupt daemon exit intentionally leaves `control.json`, a POSIX socket path, and/or the SQLite writer lock. Automatic deletion is unsafe: a temporarily unreachable daemon may still own SQLite, a PID can be reused, and another instance can replace an artifact between inspection and cleanup. Normal `daemon start` therefore fails closed, but operators need a bounded way to diagnose and recover genuinely stale local state.

The descriptor contains an authentication token and endpoint path. Recovery diagnostics must not expose either. Recovery must also preserve the Rust and Python databases byte-for-byte.

## Decision

- Add `vibemuxctl daemon inspect` as a read-only operation. It first attempts authenticated health, then inspects bounded descriptor and writer-lock snapshots without exposing raw contents.
- Inspection reports only allowlisted metadata: artifact presence/validity, protocol version, endpoint kind, owner PID, owner-process presence, recoverability, and stable reason code. It never emits a token, endpoint, project path, database path, raw lock nonce, or OS error text.
- Process liveness uses a pinned cross-platform process-enumeration library. If the recorded PID is present, even if it may be an unrelated reused PID, recovery refuses. No recovery path kills a PID.
- Descriptor and lock PIDs must agree when both artifacts are present. Missing, malformed, symlinked, oversized, mismatched, or inaccessible artifacts fail closed.
- A recoverable inspection emits a domain-separated SHA-256 confirmation derived from the exact bounded artifact bytes. The value is an authorization checksum, not a secret.
- Add `vibemuxctl daemon recover --confirmation <value>`. It recomputes the plan, requires an exact confirmation match, refreshes process liveness, and removes only artifact bytes that still match the inspected snapshots.
- POSIX socket removal is bound to the unchanged descriptor owner. Windows named pipes have no filesystem endpoint to remove.
- Recovery never deletes, truncates, migrates, opens, or rewrites either SQLite database. Partial cleanup returns a stable error and requires a fresh inspection before retry.
- Recovery remains an explicit user command; normal start/health/stop never invoke it automatically.

## Consequences

- A crashed local daemon can be recovered without weakening the single-writer invariant or relying on PID-based termination.
- Conservative false negatives are possible when a PID has been reused; the operator must wait until that PID is absent rather than override the guard.
- The inspection confirmation becomes invalid whenever any protected artifact changes, closing the inspect-to-recover race for ordinary local failures.
- Filesystem compare-then-delete cannot claim atomicity against a process under the same trusted SID; Windows cross-user runtime ACL hardening and stronger native handle-based deletion remain separate concerns.

## Alternatives

- Delete artifacts automatically during start: rejected because an unreachable live writer could still own SQLite.
- Add `--force` to ignore process/artifact checks: rejected because it defeats the authoritative-writer contract.
- Kill the recorded PID: rejected because PID reuse can target an unrelated process and health failure does not authorize termination.
- Expose the descriptor token as confirmation: rejected because authentication secrets must never enter CLI output or user-visible recovery logs.
- Delete the Rust database and recreate it: rejected because stale runtime metadata is not evidence of database corruption.
