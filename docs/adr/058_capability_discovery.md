# ADR 058: Additive v10 capability discovery and the operation registry

Status: Accepted, 2026-11-12.

## Context

The Hub must decide which VibeMux operations it may use before issuing them. The
control surface has 46 operations accumulated across v1-v10, each with its own
minimum version, but there was no single machine-readable description of the
surface. Adding a new protocol revision was the obvious carrier, except that
`ControlClient::from_descriptor` refuses any descriptor version the client does
not know: stamping v11 would disconnect every shipped v1-v10 client (CLI, GUI,
MCP reader, web bridge, supervisor clients) even though the server still accepts
old request versions. `ControlDescriptor` is also `deny_unknown_fields`, so an
extra descriptor field breaks older readers the same way.

## Decision

1. Add a read-only `capabilities` operation at the existing **v10** revision.
   The descriptor version and the accepted request range stay unchanged. The
   operation is additive: old clients never call it, and an older v10 daemon
   rejects the unknown variant as an invalid frame.
2. Keep one operation registry: `ControlOperation::ALL` plus
   `wire_name`, `minimum_protocol_version`, `is_mutating`, and `handler_group`.
   The catalog is a wire projection of that registry, not a second hand-written
   table. Exhaustive matches force every new operation to name its access class
   and owning module.
3. The catalog separates four facts that are commonly conflated:
   - `supported_actions`: what this build's adapters can do;
   - `configured`: whether an operator route exists;
   - `launcher_observed`: what the trusted probe cache observed (a missing
     cache degrades to `false`, never to a failed discovery);
   - `enabled_actions`: what the route actually enables.

   `qualified_actions` stays empty: the daemon records no live qualification
   evidence, and fixture coverage is not a live claim.
4. Keep the response content-free and bounded: operation names, versions,
   access classes, schema ids, handler groups, harness/profile/protocol
   metadata, booleans, and fixed action names. No token, endpoint, path,
   prompt, or configuration digest. The encoded catalog must fit the 64 KiB
   frame minus envelope headroom.
5. Hub consumption:
   - legacy reads keep the historical v5 request cap;
   - `capabilities` uses the descriptor version and requires descriptor v10+;
   - the catalog is validated as a closed record (unknown fields, duplicate
     operation names, an unknown contract family, or an action enabled without
     build support are refused);
   - `try_read_vibemux_capabilities` returns `null` only for the precise
     "daemon predates the operation" conditions, so a real outage or a
     contract mismatch still surfaces;
   - the Hub read surface reports `discovery: capabilities | legacy |
     unconfigured` with a reason, and never derives authority from the
     catalog.

## Alternatives

- Bump the descriptor/protocol to v11: rejected; it is a coordinated-upgrade
  cutover for every shipped client and adds no negotiation value here.
- Add optional descriptor fields for a contract family: rejected; older Rust
  readers use `deny_unknown_fields` and would fail to parse.
- Compute the catalog client-side from the descriptor version: rejected for
  the Hub; only the daemon knows its registry, routes, and probe state.
- Expose the catalog through the existing `health` payload: rejected; changing
  an existing payload is a compatibility break, while a new payload variant is
  safe.

## Consequences

- `vibemuxctl capabilities` is a new read-only verb; the CLI maps an old-v10
  daemon's invalid-frame rejection to `control_capabilities_unavailable`.
- The Hub gains `vibemux_capabilities` (MCP) and `capabilities` (read surface)
  without changing the frozen `check_vibemux` shape.
- The descriptor remains v10 until a coordinated cutover is explicitly
  planned. The catalog's `control_contract_id` is the forward-compatible
  identity for that future revision.
- C01 of the cross-project frame now asserts real discovery plus the unknown
  family/shape errors.

## Validation

- `cargo test -p vibemuxd --all-features capabilities` (4 tests) and
  `cargo test -p vibemux_cli --all-features` (70 unit + 12 integration).
- Hub `node --test tests/vibemux.test.mjs tests/mcp.test.mjs` (45 tests) and
  the full Hub gates.
- The cross-project frame's C01 against a real daemon and the real Hub adapter.
