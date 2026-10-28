# ADR 044: Closed worker tools over an owned snapshot

Status: Accepted for component implementation and qualification (2026-10-27).
Daemon admission and live workflow integration remain pending.

## Context

The installed Claude 2.1.304 profile cannot establish ADR 036's independent
EditFiles/WriteFiles grants: its bare palette omits Write, while Edit can create
a missing file. A fixed stdio MCP tool works with all native tools removed.
Offline native MXC controls separately pass, but managed online MXC permits host
loopback and therefore cannot establish a gateway-only network boundary.

## Decision

Introduce a separate `vibemux_worker_tools` component. Keep the existing read-only
MCP frontend and native writable profile unchanged. The component takes only a
bounded admitted text snapshot and immutable operation/path grants. A fresh
private directory receives new regular files from name/content records; it never
adopts a source checkout, imports links, or loads repository configuration.
The supplied parent must already pass the private-directory check; the new Windows
child is then secured and verified before any admitted byte is imported.

The first component exposes bounded read, path search, create-new, and edit-existing
operations. Grants and path scopes reuse the existing workflow vocabulary. Creation
uses exclusive creation; editing opens an existing file without creation and
checks a caller-supplied revision before mutation. Content is validated before
writing. Any uncertain filesystem mutation quarantines the candidate and prevents
further effects/export. Reads and searches are independent; removing a grant
removes its tool and still refuses a forged call to that tool.
An owner-only recovery record and private marker bind retained effects to the
attempt, base snapshot and operation receipts. Failed owner-output publication
retains those effects; successful output is staged and synced before replacement
of this invocation's reserved empty result file.

The broker owns one serialized mutable workspace. Untrusted executable code never
runs there. A later qualified test operation must materialize a separate disposable
snapshot, use a fixed admitted runtime/argv inside an offline OS boundary, and
discard every test-side mutation. Until that executor exists, RunDevTests is
unsupported and cannot be advertised or silently mapped to a native shell.

The owner receives a candidate bound to its original snapshot, attempt and
operation sequence. This component does not apply it to a repository, commit it,
write canonical state, or certify execution. The daemon retains its independent
Run worktree, single writer, durable admission, review, verifier and integration
gates. No serialized file or tool reply grants daemon authority by itself.

The initial stdio interface is for explicit component use and qualification.
It accepts no client-provided roots, executable, environment, network target or
permission update. Initialization and tools have closed versioned schemas and
bounded framing. Lost replies do not authorize automatic mutation replay.
Owner output is separate from model-visible tool output.

## Alternatives and security limits

Native Claude tools are rejected for this operation contract by installed evidence.
The official filesystem MCP server combines creation and overwrite in write_file;
its dynamic client roots also cannot confer the daemon's admission authority.
The official Rust SDK supplies broad MCP lifecycle machinery but would introduce
a new dependency/version boundary. This small closed protocol can use the existing
bounded JSONL framing and serde/Tokio dependencies; it must be tested against the
installed client rather than claiming complete MCP support.

Fresh snapshot construction, one owner, and the absence of link/rename/process
operations prevent links from arising through the interface. This is a construction
invariant, not a hostile same-user filesystem security claim. The trusted OS account
and parent process remain trusted. A future file-import, shared-workspace or test
execution feature must re-evaluate that invariant. The model controller's native
tools/configuration exclusions and sandbox effects remain separate qualification
requirements. Component checks cannot promote live slots.

## Evidence and sources

- [Installed profile observations](../evidence/claude_profile_validation.md).
- [Official filesystem server](https://github.com/modelcontextprotocol/servers/tree/main/src/filesystem)
  and [Rust SDK](https://github.com/modelcontextprotocol/rust-sdk), inspected through
  GitHub MCP before this implementation.
- [CaMeL](https://arxiv.org/abs/2503.18813), queried through arXiv: capability checks
  outside model interpretation motivate this boundary; its benchmark is not
  evidence that this implementation is secure.
- Rust 1.92 Windows OpenOptions maps existing-only opens to OPEN_EXISTING and
  create_new to CREATE_NEW. File contents are checked and changed through the
  same handle; new unsafe Windows APIs are unnecessary for this component.

Acceptance requires independent grant-removal cases, stale/ABA revisions, path
and scope rejection, bounded input/output, ambiguous mutation handling, snapshot
export provenance, strict protocol lifecycle and an installed-client fixture.
The full workflow still requires daemon-owned admission, sandboxed tests and
correlated AAG transport evidence.
