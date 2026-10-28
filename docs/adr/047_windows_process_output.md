# ADR 047: Owned overlapped Windows process output through safe APIs

Status: Accepted for targeted implementation and qualification (2026-10-27).
Decision disposition: auto-approved within the requested implementation scope.

## Evidence and decision

Native cancellation exposed incomplete output, failed cleanup and a runtime that
remained alive after its recorded process tree had exited. Tokio 1.51.6 wraps
ordinary Windows child output in Blocking<ArcFile>. Dropping an async read does
not stop a started blocking closure, and runtime shutdown waits for it. Controlled
fixtures reproduced both pending-read and queued-read failures while a separately
owned writer stayed alive; both fixtures stopped/reaped that writer before failing.

Use Tokio's safe named-pipe constructors in a Windows-only platform factory.
Each stdout/stderr pair has a random local name, inbound-only byte mode, first
instance, one-instance limit and remote-client rejection. The same owner opens a
write-only client and finishes connecting both pairs before spawning the trusted
unreleased trampoline. There is no accept loop, reconnect or caller-provided name.
An earlier connection makes setup fail; it is never adopted as the owner's peer.

Inbound direction permits reads only at the server and writes only at the client.
The default pipe DACL's Everyone/anonymous read access therefore does not expose
output through another client. The creator owner, administrators and System remain
trusted; this is not hostile-same-user isolation. UUID naming reduces collisions,
not authorization. The connections carry only the existing stdout/stderr streams,
without a TCP service or expanded workload filesystem/network permissions.

Rust Command retains configured writer handles across spawn. The factory resets
stdout/stderr to null immediately after the spawn attempt on both success and
error, closing those parent copies before return. Readers remain on Tokio's IOCP
pipe type; no blocking File-reader wrapper or detached read thread is introduced.
Partial setup errors drop their resources before a process is created. The caller
continues to own Job containment, trampoline release, deadlines and reaping.

The project uses safe APIs only. Its existing two-module unsafe allowlist remains
unchanged, and process_stdio explicitly forbids unsafe code. The Windows target
reuses already-pinned Tokio and UUID dependencies; no dependency version changes.
Validation names Rust 1.92.0, Tokio 1.51.6 and resolved Mio 1.2.2, without relying
on an undocumented standard-library pipe-handle conversion for memory safety.

## Alternatives and limits

A fresh raw-handle transfer was investigated and its ownership assumptions were
reviewed, but it would require another project unsafe boundary tied to Rust's
internal pipe implementation. The safe constructor approach preserves the existing
unsafe boundary and provides explicit direction, connection and writer ownership.

CancelIoEx requests cancellation but does not prove completion; it can race a
queued blocking closure. CancelSynchronousIo cannot safely target an inferred
reusable Tokio worker thread. Longer timeouts, shutdown_background, TerminateThread
and unbounded disk capture do not satisfy the ownership contract.

Required controls cover normal bytes/EOF with Command kept alive, direction and
connection-race refusal, partial setup/spawn failure cleanup, and runtime shutdown
while a controlled writer remains alive. The original MXC backpressure gate remains
separate and unchanged. Its failed receipts and current startup-control failures
are not converted into passes by this transport change. No sandbox qualification,
resource-quota guarantee or universal driver deadline is inferred.

## Primary sources

- [Tokio 1.51.6 Windows process streams](https://github.com/tokio-rs/tokio/blob/tokio-1.51.6/tokio/src/process/windows.rs).
- [Tokio safe named-pipe construction](https://github.com/tokio-rs/tokio/blob/tokio-1.51.6/tokio/src/net/windows/named_pipe.rs).
- [Windows pipe direction](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-open-modes).
- [Windows pipe security](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights).
- [Rust 1.92 process handle ownership](https://github.com/rust-lang/rust/blob/1.92.0/library/std/src/sys/process/windows.rs).
