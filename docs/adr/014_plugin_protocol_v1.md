# ADR 014: Plugin protocol v1

Status: Accepted for implementation; not yet a public compatibility promise

## Context

Out-of-process plugins need a cross-language contract that preserves core authority, contains malformed input, supports long-lived streams, and makes overload and shutdown behavior explicit. Plain terminal output cannot provide structured completion, ownership, cancellation, or compatibility semantics. A public Rust ABI would not be stable enough for an open plugin ecosystem.

## Decision

Plugin protocol v1 uses length-delimited, generated Protobuf envelopes over child-process stdin and stdout. Stderr is reserved for diagnostics. JSON may be offered only as a development codec and does not define canonical wire compatibility.

Every session follows this lifecycle:

```text
spawn
  -> Hello
  -> CoreHello
  -> capability and permission negotiation
  -> Ready
  -> request, response, event, and stream traffic
  -> Drain
  -> Shutdown
  -> exit
```

Every envelope carries an explicit protocol version and message identity. Applicable messages also carry request, correlation, causation, deadline, cancellation, and idempotency fields. Frames and queues are bounded. Heartbeats, overload behavior, graceful drain, incompatibility errors, duplicate handling, and shutdown deadlines are part of the contract.

Plugin manifests declare kind, entry point, API version, capabilities, supported platforms, and requested permissions. The core validates and records the granted subset; an omitted permission is denied. Plugins never allocate canonical event sequence numbers or write the core database.

Major versions represent breaking wire changes. Additive minor-version fields require deterministic defaults and compatibility fixtures. No public compatibility window is promised before the milestone in `PROGRESS.md` explicitly declares it.

## Consequences

- Rust and Python plugins can share generated contract tests.
- Stdout protocol purity and bounded decoding can be fuzzed independently of plugin behavior.
- The supervisor must own process lifetime, cancellation, restart budget, heartbeat, and output limits.
- Protobuf code generation and historical fixtures become part of release maintenance.
- Malformed frames terminate or quarantine one plugin session without changing canonical task success.

## Alternatives

- Newline-delimited JSON: rejected as the primary codec because framing, size enforcement, and schema evolution are weaker for long-lived binary-safe streams.
- gRPC for every child plugin: rejected for v1 because it adds local server lifecycle and transport surface without improving the initial stdio trust boundary.
- Rust dynamic libraries: rejected by ADR 013.
- Ad hoc terminal parsing: rejected because PTY text cannot be an authoritative structured lifecycle signal.
