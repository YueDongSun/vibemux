# Security

VibeMux worktrees provide concurrent-edit isolation only; they are not security sandboxes. Core security boundaries include `shell=False` with structured argv, stdin-based message delivery, `commonpath` path validation, fail-closed symlink/reparse handling, pane-ownership validation, append-only events, and dry-run cleanup. Do not publicly disclose credentials or exploit payloads when reporting an issue; contact the maintainer first.

Workspace Git operations contain ordinary filter descendants with the existing
process-tree primitive and fail closed when its bundled helper is unavailable.
The owner terminates remaining descendants even after successful Git exit.
POSIX process groups do not contain code that deliberately escapes the group;
see [ADR 029](docs/adr/029_daemon_harness_dispatch.md#workspace-git-process-ownership-2026-11-06).

DSH uses the bundled launch gate before vendor execution and always terminates
remaining contained descendants before reporting completed cleanup. Failed
containment refuses startup; failed cleanup retains daemon ownership for retry
and refuses session replacement. Blocked pipe writes have deadlines and cannot
leave a partial frame reusable. These changes address leaking ordinary children,
not deliberate process-group escape or hostile same-user interference; see
[ADR 032](docs/adr/032_dsh_harness_adapter.md#cleanup-correction-2026-11-06).

On macOS, an `EPERM` cleanup result is accepted only after a non-reaping check
and kernel group enumeration prove the owned, exited leader is the sole member.
Permission/query failures, additional members and incomplete observations remain
errors. Local control sockets use a bounded path with a randomized `/tmp`
fallback when the system temporary directory is too long; binding never removes
an existing entry, and descriptor ownership and bearer checks still apply.

Desktop settings use exclusively created, unpredictable sibling staging files,
so a pre-existing temporary file or symlink is never truncated or removed by a
save. Only the staging file created by that save is cleaned up after failure.
Reads and writes are limited to 64 KiB, and non-regular files are refused. This
protects against pre-existing staging collisions and simultaneous instances;
it is not a defense against another process with the same user's privileges
actively replacing the settings directory or files during an operation.
Separate instances may replace each other's complete settings snapshots.

Browser-local history has an exclusive writer lease plus a stored-record check
on every save/reset. This prevents cooperating tabs from overwriting stale
history; older tabs must be reloaded after an upgrade. It does not isolate
history from other same-origin scripts. API keys remain memory-only, and no
browser record acquires daemon or canonical-state authority. See [ADR 056](docs/adr/056_browser_local_chat.md).

Chat regeneration and edit/resend keep replacement previews outside saved
history until a complete response succeeds. Failure or cancellation preserves
the original transcript and artifact. Monitor callbacks capture authentication
generation and token; sign-out or rotation invalidates pending requests before
asynchronous cache clearing. Obsolete responses, errors and finalizers cannot
change a new session. This is client-state integrity, not a change to server
authorization or bearer-token storage.

Frontend build dependencies use pinned, reviewed security overrides for
deepmerge and Workbox's glob chain. These prevent returned-config prototype
poisoning and remove the affected glob/minimatch versions; audit thresholds
remain unchanged. The deepmerge replacement is a third-party interim fork with
an explicit provenance review and regression tests, not an upstream fix.
See [the dependency review and removal criteria](docs/frontend_dependencies.md).

