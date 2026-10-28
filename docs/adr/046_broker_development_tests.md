# ADR 046: Broker-owned development-test dispatch

Status: Accepted for component implementation and qualification (2026-10-27).
Decision disposition: auto-approved within the requested implementation scope.
Daemon admission and live runtime qualification remain separate gates.

## Decision

Add an explicit bootstrap version 2 containing the version-1 worker snapshot and
an immutable test policy: owner-selected Node/MXC executables, named suites,
explicit entry lists, limits and a finite run budget. Version 1 keeps its existing
four-tool contract and refuses RunDevTests. Version 2 advertises `run_dev_tests`
only when both read and execution grants are present. A call supplies only a
suite ID and expected workspace revision.

The broker reads and verifies its current admitted files, records a fresh owner
operation ID and input binding, then uses ADR 045's separate disposable executor.
An owner scope that permits reading a file also permits its admitted test copy;
forbidden paths still win. Tests never execute in or copy mutations back to the
editable broker. The broker revision changes only through successful file writes.

The same suite and input-snapshot identity can execute only once within this
owner session, including after cancellation, failed preparation or a lost reply.
Changing a JSON-RPC ID or making a no-op/ABA edit cannot bypass that restriction.
A deliberate rerun needs renewed owner admission. Cross-process/session replay
protection still belongs to the integrating daemon, not a new component database.

One active slot covers snapshot admission, preparation, execution, cleanup and
private receipt publication. Other file/test calls receive a busy error and are
never queued. Ping and immutable catalog requests remain available. A bounded
serialized output queue runs concurrently with the active operation and input;
an unconsumed reply cannot abandon cancellation or process finalization.

Matching MCP cancellation latches the stop request and suppresses the pending
result while owner finalization continues. Unknown, completed, malformed or
wrong-type IDs have no effect. Enqueuing a completed response is the commit point
after which cancellation is late. Clean stdio EOF cancels and joins any active
test; a malformed trailing frame or other transport failure also cancels/joins
and retains the editable workspace for recovery.

Private admitted/completed journal entries use the existing protected `.vibemux`
namespace. Owner output includes test receipts separately from the ordinary
candidate. An interrupted operation guard retains the editable snapshot. The
disposable executor retains its own envelope when its outcome is unknown. The
original owner markers permit attribution without automatic retry or reuse.

The model result is an explicit allowlist projection, with no owner recovery
locator field. Captured text is labeled untrusted and may contain diagnostics
about the execution environment; it is not a safe public log. Its entire encoded
JSONL frame is checked before any byte is written. There is one text representation,
not duplicated structured/text payloads. Assertions remain unverified regardless
of process exit or claimed test output.

## Alternatives and limits

The existing closed protocol is retained and split into pure message admission
and owned effects. A broad SDK, task API, scheduler, HTTP transport or polling
service is unnecessary for this single-operation stdio component. Earlier official
SDK/filesystem-server GitHub MCP research is recorded in ADR 044. A fresh GitKraken
GitHub read required an unconfigured provider connection; no credentials or account
configuration were changed. The official MCP specification supplies the current
cancellation and transport requirements.

The session deadline closes admission and requests cancellation; it does not
pretend to interrupt synchronous filesystem cleanup. Blocking phases are owned
and joined on normal finalization. Aborting the owner is an unknown outcome,
not a clean completion receipt. Runtime/configuration protection, all readable
environment disclosure, disk-growth limits, process escape qualification,
daemon lease binding and independent assertion verification remain open.

## Sources and checks

- [MCP cancellation](https://modelcontextprotocol.io/specification/2025-06-18/basic/utilities/cancellation).
- [MCP stdio transport](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports#stdio).
- [MCP base protocol](https://modelcontextprotocol.io/specification/2025-06-18/basic).
- ADR 044's capability-check research and ADR 045's installed offline controls.

Acceptance covers old-schema compatibility, grant removal, stale/ABA input
binding, duplicate transport and semantic replay refusal, busy calls without
deferred effects, cancellation ID typing/result suppression, EOF/partial input,
output backpressure, escaped-frame bounds, owner recovery and an installed-client
call through the actual broker executable. Component checks do not promote a slot.
