# ADR 073: Optional process-external supervisor bridge

Status: Proposed; implemented on this feature branch for an opt-in, locally authenticated integration.
Production model execution and native Windows qualification are separate gates.

## Context

A separate responsive operator interface needs bounded task/workflow reads,
an intake receipt, and context-only follow-up without becoming a second
scheduler. Main already has authoritative daemon workflows and content-free
read projections. Open PR #52 adds intake, follow-up and worker report verbs,
while open PR #49 uses some of the same protocol version numbers for different
operations. Inferring operation support from a version number would conceal
that compatibility conflict.

## Decision

Add `integrations/superagent_bridge`, a standalone Node application using only
built-in modules. Its `/api/v1` API translates an allowlist of operations into
the configured, installed `vibemuxctl` binary. It uses neither SQLite nor the
daemon's internal mutable objects. No canonical schema, Control version,
scheduling policy, compiler, model client, or existing monitor authorization
changes.

Project aliases resolve to fixed, private server configuration. Requests cannot
select an executable, project root, provider credential or argument string.
Task/detail/status are read projections. Attention explicitly represents
historical worker report claims, not a second owner of unresolved task state.
Task event metadata supports bounded activity inspection; it is not advertised
as complete workflow replay.

Capability discovery reads actual CLI help verbs. A capability describes the
CLI advertisement and bridge configuration; it does not establish daemon
reachability, protocol compatibility, or live gateway qualification. Main and
#49 remain usable for their advertised reads and refuse unsupported writes.
#52 writes/reports become available only with matching installed commands.
No open branch is merged as part of this decision.

Imported intake preserves a caller-owned event ID and the existing exact
request/policy. It prepares only. Optional private goal presets replace
`request_text` only and must leave compilation to a configured daemon compiler;
precompiled TaskSpecs cannot be reused against changed goal text. Follow-up
requires the caller's exact workflow version, task, worker session and contract
identity. It queues bounded context and cannot widen contracts or resume work.
The daemon remains responsible for admission, idempotency, execution and
acceptance. No local draft or mutation ledger is added.

## Failure ownership

An acknowledged response preserves the daemon's duplicate/prepare/context-only
receipt. A possibly submitted command without a matching receipt returns
`outcome: unknown`, including timeout or malformed output. Neither the adapter
nor its application proxy retries a mutation automatically. Clients retain the
original event ID and draft across a disconnect or project selection change.
Explicit pause/cancel use their existing daemon operations.

Subprocesses use `shell:false`, fixed argv and an allowlisted environment.
Private JSON files are uniquely created, bounded and removed by exact path.
Output, upload time, command time and concurrent requests are bounded. The
bridge owns its short-lived CLI processes; canonical daemon/worker lifetime
does not follow an HTTP request. An unconfirmed child exit retains its ownership
slot and reports incomplete cleanup. This adapter is not a general subprocess
launcher or a replacement for daemon process containment.

## Authentication and threat model

The service binds to loopback. All requests require a dedicated bearer;
credentials remain in the trusted application server's environment. Host and
Origin checks, no CORS, strict envelope/query validation and fixed project roots
address browser cross-origin requests and caller-supplied path/command injection.
CLI diagnostics are reduced to public codes and outcomes; task detail omits
absolute worktree paths. Report text requires explicit configuration and query
opt-in in addition to daemon retention/ownership gates.

One bearer authorizes the complete configured alias set. The proxy must
authenticate its own users; this bridge is not a multi-tenant authorization
system. A malicious process running as the same OS user, or a replaced trusted
CLI, is outside its isolation boundary. Unix private mode bits are checked in
tests; Windows relies on the operator's protected user temp directory and still
needs native qualification. No browser may receive this bearer.

## Compatibility, rollback and evidence

Removing the bridge requires no canonical migration or rollback. In-flight
mutations still belong to the daemon and may need state inspection before an
operator replays the same event. The bridge cannot bypass the current live AAG
attestation refusal, and its fixture tests do not qualify a model backend.

Contract tests run actual Node child processes and loopback HTTP, using
synthetic examples of the verified main and #52 DTOs. They cover incompatible
capability combinations, prepared/duplicate receipts, exact follow-ups, unknown
outcomes, limits, cleanup and auth failures. Native Windows, a real installed
CLI/daemon, and production model execution remain separately unverified.
Executed results are recorded in [PROGRESS.md](../../PROGRESS.md).

The complete HTTP/configuration contract and operational limits live in the
[integration guide](../../integrations/superagent_bridge/README.md).
