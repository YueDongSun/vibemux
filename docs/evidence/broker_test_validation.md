# Broker version-2 validation, 2026-10-27

This record separates implementation evidence from installed sandbox qualification.
The source is the current task-owned working tree on base `45308fc`; no live slot,
daemon execution profile, remote device or model provider was promoted.

## Contract and ownership regressions

The version-2 broker connects its verified current snapshot to the separate test
executor through an immutable owner policy. Default tests cover grant removal,
stale revisions, suite/input deduplication after no-op edits, busy calls without
queued effects, typed cancellation IDs, strict metadata and existing version-1
behavior.

Two new regressions reproduced loss of edited bytes when an unpolled test future
or idle owned server future was dropped. Both failed with a missing candidate
directory before the fix and passed afterward. Session ownership now retains by
default; only terminal successful publication of the complete latest candidate
permits cleanup. Sink failure, partial/uncertain publication, successful recovery
publication and publisher panic retain the candidate.

Independent review found that decoding through an intermediate JSON value lost
duplicate-key rejection. An actual-CLI test reproduced acceptance of duplicate v1
grants. Both schemas now deserialize directly from the original bytes. The same
regression checks duplicate v2 policy fields; all four CLI tests passed.

A locked workspace entry also reproduced deletion of the owner marker before
failed recursive cleanup. Removing the workspace subtree first preserves that
marker on the observed failure path. The controlled regression passed after the
change; it does not authorize wider deletion or permission repair.

The final default worker suite passed 33 tests. Installed-runtime cases remain
explicitly ignored in that default command and are not included in this count.

## Windows output lifetime

Pinned-source inspection identified the Windows Blocking reader lifetime in Tokio
1.51.6. Two independently owned fixtures deliberately held writer handles open.
Pending and queued-read cases both failed under the old wrapper because runtime
shutdown waited for the writer. Cleanup stopped/reaped the fixed writer before
asserting, preserving a reproducible failure rather than leaving a hung fixture.

The final implementation uses safe, preconnected, inbound-only local named pipes.
It forbids project unsafe code and keeps the original exact two-module allowlist
unchanged. Three explicit controls passed: pending-read cancellation, queued-read
cancellation and normal stdout/stderr plus EOF. Two setup tests passed direction,
instance/preconnection refusal and partial-setup/spawn-failure cleanup. All three
unsafe containment guards passed unchanged. See [ADR 047](../adr/047_windows_process_output.md).

These controls validate ownership and runtime completion while a fixed writer
remains alive. They do not certify the native sandbox or identify every outstanding
handle in a failed MXC run.

## Installed-client evidence

Claude 2.1.304, with native tools removed, strict MCP configuration and a synthetic
local API, invoked the actual broker test tool. Its private owner receipt recorded
completed execution, matching operation binding and test-copy removal; the candidate
contained no test-side mutation. An extra `command` argument was rejected as
`invalid_arguments` with no test receipt or candidate changes.

These observations preceded the final pipe replacement. They prove the exercised
installed-client contract at that point, not a fresh native execution certificate
for the final candidate. No live provider, account credential or paid inference
was used. Controller-context and gateway-only egress exclusion remain unverified.

## Native failures and current limit

The native backpressure control retained an actual failed cleanup receipt: root
exited and the outer Job was empty, capture was incomplete, and workspace removal
failed. Later diagnostics identified Windows sharing violation 32. One old-wrapper
run also remained alive after its result because of the blocking-read lifetime;
only its positively identified task test process was stopped, with fixtures kept.

The IOCP change made the control terminate, but did not satisfy the unchanged
workspace-removal assertion. Bounded stdin and exclusive-file cooperative-owner
experiments also failed normal completion. They were preserved as unqualified
experiments and removed from production; [ADR 048](../adr/048_cooperative_test_cancellation.md)
records that disposition.

An independent startup-only control, without either experimental owner, did not
produce its marker within the declared bound. The same result occurred with the
already-installed app CLI `0.164.0-alpha.11.1` and npm CLI `0.164.0`. A file-marker
comparison also failed with null output, untouched pipes and IOCP output. A direct
contained trampoline with a fixed trusted Node file-marker workload passed.
The safe pipe implementation still failed the MXC startup prerequisite. These
observations localize an unresolved native-execution path; they do not prove its
underlying platform cause or that candidate code began running.

No sandbox mode, managed requirement, global setting, mount, driver, account or
permission was weakened to obtain a pass. Historical failures remain failures.
Production retains the original fixed `node_test_offline_v1` workload profile;
current MXC startup/backpressure qualification remains open.

## Reproduction

Use [the broker guide](../broker_development_tests.md) and
[standalone executor guide](../development_tests.md). Run default contracts first.
For installed checks, supply explicit backend and Node executable paths and require
the startup-only control to produce its marker before running the wider matrix.
The diagnostic trampoline override is test-only and never changes a production
runtime selection.

Whole-workspace CI, non-Windows operation, protected controller startup, complete
runtime disclosure isolation, daemon lease binding and live workflow acceptance
are not established by these focused checks. The full four-direction R&D goal
remains incomplete.
