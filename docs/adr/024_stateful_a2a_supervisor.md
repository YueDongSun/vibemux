# ADR 024: Stateful local A2A gateway and supervisor workflow

Status: Accepted for the explicitly requested local pre-alpha implementation

## Context

M7.0 proves only information sharing. The next requested acceptance requires a real supervisor delegating to independent model peers, obtaining artifacts and review evidence, and closing a task only after verification. M7 transport work and a bounded subset of M8 must therefore proceed together without claiming full workspace/terminal parity or remote readiness.

## Decision

- Keep official A2A v1 types and REST/JSONRPC/gRPC mappings inside `vibemux_a2a`. Use the official SDK RequestHandler and protocol serializers with bounded local adapters where SDK convenience clients lack memory bounds. Do not use the SDK default detached execution manager as core authority.
- Backends expose VibeMux-owned request/task/artifact contracts. Bearer credentials bind to explicit subjects before dispatch; task access is checked atomically with backend operations. Use numeric loopback origins, explicit origin allowlists, no redirects/proxy inheritance, bounded bodies/streams/concurrency and owned shutdown. Non-loopback deployment remains disabled pending its own TLS/auth/SSRF acceptance.
- The daemon keeps the one authoritative SQLite writer. Add a forward schema migration and bounded writer commands for internal Task/Run plus external A2A binding, workspace ownership, immutable events, versions, cancellation and artifact references. Requests cannot directly declare canonical completion.
- Remote Completed records an observation only. Canonical success requires an independent reviewer Run, matching artifact and review hashes, current versions, and a local verifier receipt. Rejected verification leaves the failed runs auditable and opens new repair runs; no history rewriting, auto-commit, or merge.
- Every writable role Run uses a distinct Git worktree/branch pinned to the selected base commit. Workspace creation, inventory checks and artifact writes use owned paths and structured argv. Cleanup is plan-first and refuses dirty or mismatched resources.
- Model providers are optional out-of-process A2A peers. The user has authorized using existing CC Switch providers for local acceptance. Only explicitly selected provider configuration is read, using read-only SQLite; the CC Switch database, defaults and credentials are never modified. Requests carry only synthetic acceptance task data, never implicit repository contents or credential material.
- Initial supervisor acceptance uses a bounded structured-data artifact task. Planner, worker and reviewer invoke actual configured models; the local verifier checks the result against the task contract. Model text is data and is never executed as shell or generated code. Deterministic fixtures remain separately labeled protocol/failure tests.
- Official TCK uses a separate fixture backend implementing the official test scenarios. It cannot call real providers. Any test-only authentication injection preserves the production auth middleware and official assertions. Record pinned upstream commits, commands and failures rather than calling ordinary tests official conformance.

## Compatibility and migration

Existing information-share and daemon/plugin status contracts remain available. The canonical Task/Run state graphs do not change. New A2A-bound entities reject legacy snapshot mutations that would bypass the binding/verifier gates. SQLite migration is forward-only and must preserve schema-v1 rows/events. Public plugin wire schemas are unchanged. Exact final schema/CLI/transport versions and example contracts are documented with the implementation evidence.

## Threat model and limits

No remote payload supplies executable paths, shell commands, credentials, writer handles, or permission grants. Credentials and full model prompts stay out of events and diagnostic logs; artifacts are explicit bounded outputs. A provider HTTP success is not a workflow success. A peer's terminal state does not satisfy the verifier gate. Git worktrees and environment filtering are not OS sandboxes. Native local tests do not establish remote/TLS, production, live-terminal, or arbitrary-code execution safety.
