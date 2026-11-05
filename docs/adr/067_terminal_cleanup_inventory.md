# ADR 067: Complete terminal subtree observation

- Status: Accepted for the scoped read-only foundation
- Date: 2026-11-03
- Related: [ADR 064](064_task_terminal_observation.md), [proposed ADR 066](066_explicit_worktree_removal.md)

## Context

The existing `terminal:inventory_v1` reports panes whose cwd is exactly the
requested directory object. It deliberately skips nonmatching directories and
some unavailable identities. This serves terminal inspection/focus, but an empty
result cannot prove that a worktree's descendants are unused. Worktree removal
needs a distinct complete-or-error observation; no removal is authorized here.

## Decision

Add the negotiated read-only capability `terminal:cleanup_inventory_v1`. Reuse
`TerminalInventoryRequest` and `TerminalInventory` inside the existing request/
response envelope. The method identity establishes the new semantics; no wire
field or protocol-major change is needed. Keep the existing plugin identity/
version for the compatible default profile. Keep exact-directory inventory and
focus behavior unchanged. An unavailable or unnegotiated cleanup capability
returns an error and never falls back to v1 inventory.

The observer validates the complete `wezterm cli list --format json` output from
its configured instance before filtering. Bound input to 512 KiB and 256 source
panes. Validate duplicate IDs globally, including panes outside the target. Each
live pane needs a complete valid, existing local directory identity; unresolved,
nonabsolute, malformed, remote/UNC or ambiguous cwd prevents a clear result.
Known dead panes may be ignored. Compare canonical real ancestor identities so
the root, descendants, case aliases, symlinks and junctions are handled correctly;
a lexical prefix is insufficient. Return at most four matching pane records and
error above that bound, without truncation or terminal titles/prompts.

Verify the configured backend process/socket instance before and after collection.
Run synchronous filesystem observation behind the existing blocking boundary.
One observer-owned worker slot covers the complete instance/CLI/filesystem job.
Its permit lives in the blocking closure, so request timeout or cancellation
cannot admit another job while that work continues. The observer refuses new
requests during that interval. Shutdown stops admission and waits for actual
worker idleness before acknowledging; an exhausted drain deadline is an error,
with the existing Core supervisor retaining owned-process cleanup.
The daemon additionally binds a result to the active plugin session and verifies
that session after the reply. The plugin advertises the method; Core grants it
only when the manifest/policy permits observation and process execution. Existing
observers/configurations may omit this capability and continue their old work.

Add `cleanup_inventory_enabled` to the validated observer JSON, default false.
Missing/false keeps the existing two-capability Hello; true advertises the third
method and requires its manifest declaration and policy/session grants. A
non-boolean value or unknown config field is rejected. An enabled observer with
an old manifest fails negotiation, without fallback. Rollout requires the new
binary, explicit config opt-in, matching manifest capability and Core grants;
no existing operator config is rewritten automatically.

This observes one explicitly configured supported terminal scope. It does not
enumerate all host terminals, own or terminate panes, reserve a worktree, freeze
external filesystem/cwd changes, or authorize deletion. A removal service must
perform its own canonical/physical checks and obtain a fresh result under its
resource gate. Any matching pane, incomplete observation, capacity error,
session change or unavailable backend blocks cleanup admission.

## Verification and compatibility

The parser candidate has independent source review and isolated Windows checks
for exact/descendant/alias matching, known outside directories and prefix traps,
unknown/remote cwd, malformed data, duplicate IDs and both source/result bounds.
Root integration must also verify capability negotiation, request dispatch,
blocking-boundary behavior and legacy regression tests before claiming support.
No live WezTerm behavior is established by parser tests or protocol construction.

Control v15, store schema 13 and the 31 native commands remain unchanged for this
foundation at its original verification cutoff. The later bounded Windows
service, ledger, recovery and native UI in ADR 066 use Control v16, store schema
14 and native command 32. Their source and local integration evidence is
separate from this observer's qualification; native desktop and live WezTerm
acceptance remain open.
