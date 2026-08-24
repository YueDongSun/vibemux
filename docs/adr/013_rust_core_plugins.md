# ADR 013: Rust core and out-of-process plugins

Status: Accepted

## Context

The Python prototype validated the Windows-first CLI, canonical Task/Run/Event concepts, Git worktree isolation, terminal adapters, and an append-only SQLite event log. It also exposed lifecycle and ownership gaps that should not become public protocol contracts. VibeMux now needs long-lived routing, bounded concurrency, cancellation, plugin supervision, reconciliation, and A2A interoperability without coupling the canonical model to any terminal or harness vendor.

An in-process Rust or C dynamic-library plugin ABI would couple plugins to compiler, allocator, dependency, and process-failure details. Keeping vendor integrations inside the core would also expand the trusted state boundary and make independent compatibility testing difficult.

## Decision

The authoritative orchestration and A2A path will be implemented as a Rust 2024 workspace. Canonical IDs, state machines, event sequencing, persistence policy, resource ownership, reconciliation, plugin supervision, A2A mapping, and the stable local control API remain in the core.

Vendor-specific and optional capabilities run as separately versioned processes. Initial plugin kinds are harness, terminal, sandbox, integration, UI, reporter, and optional policy extension. Plugins communicate through a versioned protocol and never open or mutate the core SQLite database.

The Python implementation remains in the repository as a behavior reference until Rust parity fixtures and a state migration path exist. It may later support plugin SDKs, compatibility tools, fixtures, migration utilities, and optional integrations. It must not remain a concurrent authoritative writer after Rust cutover.

No public in-process native plugin ABI will be provided in the first stable plugin API.

## Consequences

- The repository migrates incrementally and remains runnable at every accepted milestone.
- Rust domain crates cannot depend on Git, terminal, database, network, vendor SDK, or platform implementations.
- Plugin failures can degrade a capability but cannot directly corrupt canonical state or crash the core process.
- Cross-language plugins remain possible through the wire protocol.
- Packaging and development become more complex because the project must build and test a Rust core plus external plugins.
- Python core changes are limited to correctness, safety, parity fixtures, and migration support.

## Alternatives

- Continue growing the Python CLI as the authoritative core: rejected because it would duplicate the future state writer and freeze known lifecycle defects into public interfaces.
- Use Go for the core: viable, but rejected for this architecture because Rust provides stronger ownership and resource-lifecycle constraints for Windows process control, bounded streams, and long-lived plugin supervision.
- Split the authoritative core between Go A2A services and Rust platform services: rejected because two cores would create ambiguous ownership and state boundaries.
- Use in-process native plugins: rejected because ABI instability and shared-process failure undermine compatibility and isolation.
- Use WASM for every plugin: deferred; deterministic policy or transformation extensions may fit WASM later, but OS-facing terminal and harness plugins require capabilities outside that boundary.
