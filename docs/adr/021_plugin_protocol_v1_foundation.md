# ADR 021: Plugin protocol v1 wire and handshake foundation

Status: Accepted for pre-alpha implementation

## Context

ADR 014 selects generated Protobuf over child stdin/stdout, but implementation requires exact framing, validation, negotiation, manifest, and lifecycle rules before any supervisor may execute an external plugin. A generated message type alone is not a trust boundary: an attacker can advertise unsupported versions, omit identities, request undeclared permissions, or send oversized/malformed frames.

## Decision

- Check in one canonical `vibemux.plugin.v1` Protobuf schema and generate Rust bindings with the pinned Prost toolchain plus vendored `protoc` on Windows/Linux.
- The stream uses a four-byte big-endian frame length followed by one encoded `Envelope`. The decoder rejects zero or oversized lengths before payload allocation. The pre-alpha default maximum is 1 MiB and remains configurable through a validated codec configuration.
- Every envelope carries protocol major/minor, message ID, optional correlation/causation IDs, and exactly one body. Project-controlled IDs use a bounded ASCII grammar; arbitrary stdout text is never parsed as protocol.
- Protocol major `1` is exact. A peer minor newer than the core-supported minor fails negotiation until additive compatibility policy explicitly permits it.
- The initial body set is `Hello`, `CoreHello`, `Ready`, `Request`, `Response`, `Event`, `Heartbeat`, `Cancel`, `Drain`, `Shutdown`, and structured `ProtocolError`.
- Plugin manifests are TOML with schema version, plugin identity/version, kind, argv entry point, capabilities, requested permissions, supported platforms, and protocol range. The entry point is a non-empty argv vector; no shell command string exists.
- Capabilities and permissions are bounded, deduplicated ASCII identifiers. Core grants must be a subset of both manifest declarations and `Hello` requests; omitted permissions are denied.
- A core-side lifecycle machine accepts only `Hello -> CoreHello sent -> Ready -> Active -> Drain -> Shutdown -> Closed`. Out-of-phase or duplicate handshake messages return stable errors without mutating canonical state.
- Generic request/event payload bytes remain opaque to this foundation and are bounded by the frame. Method-specific schemas and authoritative Task/Run mutations are not introduced here.

## Consequences

- Framing, wire compatibility, manifest validation, and negotiation can be tested independently from child-process supervision.
- Rust and future Python SDKs have one checked-in schema and deterministic compatibility fixtures.
- Protobuf unknown fields remain forward-compatible, but version negotiation still fails closed until compatibility rules accept a newer minor.
- Supervisor work must add bounded queues, process ownership, deadlines, heartbeat, cancellation, drain, stderr limits, and crash policy around this foundation.

## Alternatives

- Protobuf varint stream framing: rejected for the first implementation because a fixed four-byte prefix makes pre-allocation bounds and cross-language fixtures simpler.
- Newline JSON as canonical wire format: rejected by ADR 014.
- Accept arbitrary capability/permission strings: rejected because unbounded or ambiguous identifiers weaken policy checks and diagnostics.
- Let the plugin self-grant requested permissions: rejected because the core is the sole authority for the granted subset.
- Spawn a mock process before the codec/state machine is validated: rejected because process tests would conflate transport defects with supervisor defects.
