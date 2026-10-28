# Closed worker-tool component

`vibemux_worker_tools` supplies a bounded text-editing component and an explicit
stdio MCP entry point. It is a building block for qualified workflow execution.
Explicit Windows fixture profiles now connect the actual file broker to daemon
attempt ownership, candidate collection, review and verification
([ADR 051](adr/051_workflow_file_broker.md)). Native writable slots remain
unqualified. [ADR 044](adr/044_closed_worker_tools.md) records the component boundary.

An explicit [installed-controller qualification](adr/052_installed_controller_qualification.md)
now exercises Claude 2.1.304 through that daemon path using synthetic inference,
actual broker calls, independent fixture roles and trusted verification. It remains
Fixture evidence and does not enable a Live slot.

A separate [offline development-test command](development_tests.md) now executes
owner-admitted copies. The explicit [version-2 broker bootstrap](broker_development_tests.md)
adds owner-selected test dispatch; version 1 retains the four-tool contract.

The trusted owner supplies a snapshot of relative text filenames and contents,
an attempt UUID, independent tool grants, and the existing workflow path-pattern
vocabulary. Only exact paths and trailing `/**` directory patterns are supported.
All content goes into a fresh private workspace. The source checkout, repository
configuration and native agent homes are never adopted by this component.

| Tool | Required grant | Effect |
| --- | --- | --- |
| `read_file` | `read_files` | Bounded text and current revision from an admitted read/owned scope |
| `search_paths` | `search_files` | Names, sizes and revisions under a relative prefix; no contents |
| `create_file` | `write_files` | Create a missing file in an owned scope; refuse existing targets |
| `edit_file` | `edit_files` | Replace an existing file at the expected revision; refuse missing/stale targets |

Client roots, extra arguments, unknown tools, alternate streams, traversal,
reserved device names, protected configuration components and case-colliding
names cannot expand authority. The client cannot add grants or change the
attempt. Version 1 refuses `run_dev_tests: true`; version 2 requires its explicit
owner test policy and both read/execution grants. No shell tool is substituted.

Files are limited to 64 KiB, 256 files and 4 MiB total. A session admits at most
256 operations, 1024 request IDs and 2048 messages. JSONL frames and replies are
bounded at 512 KiB. The transport has a five-minute session deadline and a
five-second output deadline; the owner still needs process-level cancellation
and cleanup for a stalled OS operation. The component advertises MCP 2025-06-18,
requires initialization, and rejects duplicate request IDs instead of replaying
mutations. A lost reply is not permission to issue another mutation.

## Explicit component invocation

Create an owner-controlled private directory using the platform's existing
private-directory setup. On Windows, both the scratch parent and result parent
must pass the three-principal private ACL check; the bootstrap file must also
pass the private-file check. Do not change a shared directory's ACL to make it
qualify. Use a new dedicated directory and keep the bootstrap outside the worker's
fresh workspace.

The [example bootstrap](../examples/worker_tools/bootstrap.json) is a non-live
read-only fixture. Supply absolute paths:

```text
cargo build --locked --offline -p vibemux_worker_tools
target/debug/vibemux_worker_tools --bootstrap <private-bootstrap.json> --scratch-parent <private-directory> --candidate-out <new-private-result.json>
```

The process consumes MCP JSONL on stdin and replies only on stdout. Its fixed
error codes go to stderr. The result filename must be absent; an existing result
causes refusal before a session starts. The caller owns process lifetime and must
check terminal exit and the complete result together.

On clean EOF, the owner result has `status: candidate`, original attempt/base
snapshot identity, bounded operation receipts and changed text files. It is a
candidate, including when every requested mutation was refused; it is not task
success or approval to promote changes. Before materializing into an owned
worktree, the owner verifies the exact input/attempt, scopes and grants. Workflow
acceptance still requires independent candidate, review and verifier gates.

On transport or filesystem uncertainty the result is `outcome_unknown` when it
can be stored, and the private workspace is retained. The recovery record names
that exact directory and attempt. If result storage itself fails, the process
fails and retains the directory with its original owner marker. Reconcile it
through the owning operation; do not restart the same effect automatically.
These private recovery locations are absent from model-visible tool replies.

The library provides `WorkerTools`, `candidate`, `recovery_record` and the
owner-output finalizer. It has no database access and does not read producer
private state. The owner may consume the candidate without parsing terminal text.

## Verification and current limits

```text
cargo test --locked --offline -p vibemux_worker_tools
cargo clippy --locked --offline -p vibemux_worker_tools --all-targets -- -D warnings
cargo fmt -p vibemux_worker_tools -- --check
```

Checks cover independent grants, create/edit exclusion, stale and ABA revisions,
path/scope and resource refusal, Windows private ACLs, owner-attributable recovery,
failed output storage, bounded escaped text, protocol initialization, duplicate
IDs and the actual executable's clean/broken transport behavior.

Installed Claude 2.1.304 was also exercised against a synthetic local API with
native tools removed. Its create-only and edit-only MCP calls produced the correct
candidate; edit-on-missing and create-on-existing returned errors with no changed
files. This is installed-client component evidence. It does not establish OS
sandboxing, provider attribution, malicious same-user resistance or a live
development workflow. No daemon/config schema migration is performed by adding
this crate. Existing read-only MCP and dispatch configurations remain compatible.

Daemon-owned binding, per-launch fixture identity, cancellation and candidate
transfer are now exercised by the explicit file-only fixture profiles in ADR 051.
The remaining native gates include protected controller configuration and OS
isolation, live provider qualification, and qualified offline command execution
on a separate snapshot. Test-side mutations must never become broker candidate
files. See [workflow validation](evidence/workflow_file_broker_validation.md).
