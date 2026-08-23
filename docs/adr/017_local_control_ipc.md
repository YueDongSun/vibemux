# ADR 017: Versioned authenticated local control IPC

Status: Accepted for pre-alpha implementation

## Context

The daemon writer core now owns the only Rust SQLite writer, but CLI and frontend clients still need a local control path. An unauthenticated localhost TCP server would broaden exposure and conflict with the accepted daemon architecture. Raw stdio would couple daemon lifetime to one parent process and would not support independent clients.

## Decision

- Windows uses a per-instance named pipe; POSIX uses a Unix-domain socket.
- There is no TCP fallback.
- Frames are four-byte big-endian length-prefixed JSON with a fixed maximum size and explicit protocol version.
- Every request carries a unique request ID and a random 256-bit bearer token. Authentication is checked before dispatch and token text is redacted from Debug/error output.
- The daemon publishes an ignored runtime descriptor containing the endpoint, protocol version, and token only after the listener is bound. POSIX creates the descriptor with mode `0600`; Windows relies on the per-user runtime directory plus the unguessable endpoint/token in this pre-alpha slice.
- Initial operations are `health` and `shutdown`. State mutation is not exposed until authorization and compatibility tests cover it.
- IPC handlers call the blocking writer through an owned blocking boundary and never hold an async mutex across await.
- Malformed, oversized, unauthenticated, and version-mismatched frames return structured errors or close the connection without crashing the daemon.

## Consequences

- Independent local clients can query and stop the daemon without direct database access.
- Descriptor/token lifecycle becomes security-sensitive and requires cleanup, redaction, and stale-artifact diagnostics.
- Windows same-user ACL hardening remains a release gate; this slice does not claim hostile same-user isolation.
- Protocol v1 is private pre-alpha and may change before the Rust CLI is public.

## Alternatives

- Local unauthenticated TCP: rejected because it creates an unnecessary network attack surface.
- Stdio-only control: rejected because it prevents independent on-demand clients.
- Direct CLI SQLite access: rejected because it violates the single-writer invariant.
- Public named pipe without token: rejected because endpoint discovery alone must not authorize control.
- Native Windows ACL APIs through unsafe code: deferred until a dedicated reviewed platform module and ADR satisfy unsafe-code gates.
