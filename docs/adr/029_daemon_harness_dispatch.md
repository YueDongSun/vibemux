# ADR 029: Daemon-owned harness request dispatch and output capture

Status: Accepted for pre-alpha implementation. Stages 1 to 6 are implemented
on `feat/harness_dispatch_port` and reach `main` only when that branch
merges. Before the merge, the Stage 2 `windows_job_object` `unsafe` module
still needs reviewer approval on the pull request (AGENTS.md §7.2). Stage 7
recorded live initialize-only probes of the installed Codex, Claude,
OpenCode, Copilot, and Grok CLIs on native Windows
([evidence](../evidence/harness_dispatch_validation.md)). No live submit has
run, because that needs explicit authorization of paid inference. Open
decision 3 is resolved for flag parsing; open decision 4 stays open with
live evidence.

Extends ADR 013 (detection-gated enablement), ADR 015 (single writer), ADR 017
(local control IPC), and ADR 024 (versioned run records). Amends the
core-versus-plugin table in `docs/architecture.md` and AGENTS.md §3.2 for
first-party structured adapters only (see Decision 1). Amends ADR 025's rule
that `vibemux_platform` has exactly one `unsafe` module (see Decision 6).

## Context

Commit `a955484` on `codex/harness_request_dispatch` (base `6498e6a`) built a
standalone harness dispatcher. It sends one text prompt to Codex, Claude Code,
OpenCode, Copilot, or Grok through each tool's structured protocol, captures
the complete stdout JSON records, and only reports `completed` when it sees
correlated terminal protocol evidence and a clean process exit. It was not
merged (PROGRESS.md 2026-09-27 (2)) because:

- a trial merge conflicts in 12 files, including add/add conflicts on
  `crates/vibemux_harness`;
- it puts process, pipe, Job Object (`unsafe` Win32), environment, and
  filesystem I/O inside `vibemux_harness`, which main defines as pure logic;
- main's issue #4 port already owns the harness registry and profiles;
- its later daemon-admission drafts (uncommitted ADR 025 in
  `.vibemux/worktrees/harness_request_dispatch`, staged ADR 027 in
  `.vibemux/developer_worktrees/harness_request_dispatch_latest`) reuse ADR
  numbers already taken on main, target store schemas that collide with main's
  schema 3, and expose admission through a separate A2A loopback listener
  instead of Control IPC.

These branches are the behavior reference for this port. They stay as they
are; this ADR does not adopt their numbering or schema.

### Capability that main lacks

| Capability in `a955484` | Main today |
|---|---|
| Typed request: UUID, harness, text prompt. It cannot choose an executable, environment, shell, or RPC method | No dispatch request type |
| Trusted operator route config (executable, fixed arguments, environment-name allowlist, `enabled`/`allow_execution`, model), SHA-256 pinned, with a denylist of dangerous flags | Probe cache records detection only |
| Protocol session state machines: Codex app-server (`initialize`, `thread/start`, `turn/start`), `codex exec --json`, Claude stream-json with SDK control init, ACP `initialize`/`session/new`/`session/prompt`; RPC/thread/turn/session correlation; approval and permission requests denied | `HarnessProtocol::Pty` label only |
| Conservative per-protocol classification of vendor JSON (started/text/tool/approval/usage/completed/failed/cancelled/opaque) | None |
| Bounded JSONL framing: per-frame ceiling, CRLF, and explicit errors for empty records and invalid UTF-8 | None |
| Ownership of the native process tree: clean environment, `.exe`-only on Windows, Job Object with kill-on-close, POSIX process group; stderr drained and counted, never retained | Plugin supervisor and model peer kill only the direct child |
| Outcome rule: `completed` requires correlated terminal evidence, exit code 0, no forced kill, and no error. EOF without terminal evidence is `unverified` | No harness outcome model |
| Bounded capture of the raw records (16 MiB / 16,384 records) with content-free public summaries | None |
| Initialize-only protocol probe that never sends a prompt | `--version` probe only |

## Decision

### 1. Placement: pure logic in `vibemux_harness`, all effects in `vibemuxd`

- `vibemux_harness` gains a `dispatch` module tree with no I/O, no tokio, and
  no `unsafe`: request and route-config types with validation, a launch-argv
  builder, protocol session state machines, the classifier, an incremental
  byte framer, capture accounting, the attempt phase machine with its
  Task/Run mapping, the outcome decision function, and canonical event drafts.
  New dependencies are only `sha2` and `uuid` (pure computation, both already
  in the workspace).
- `vibemuxd` owns every effect: reading the config file, resolving `HEAD`,
  spawning the vendor process, pipe I/O, deadlines, cancellation, the in-memory
  transcript, and all writer calls. Blocking store and Git work runs in
  `spawn_blocking`. Pipe I/O is async on tokio. Every channel is bounded.
- `vibemux_store` owns the durable dispatch record and its transactions.
  `vibemux_platform` owns the process-tree containment primitive.
- **Architecture amendment:** first-party structured adapters for Codex,
  Claude, OpenCode, Copilot, and Grok run their pure protocol state machine
  inside the daemon. The vendor CLI still runs in a separate OS process. The
  reasons for the plugin boundary (unstable native ABI, crashes in third-party
  code, weak permission isolation) do not apply to pure first-party Rust state
  machines with typed errors and no panics on input. Third-party or optional
  harness adapters still use the out-of-process plugin protocol. Because the
  state machine is host-independent, a later ADR can move an adapter behind
  the plugin protocol without changing it.

### 2. Durable dispatch record (store schema 4)

A forward-only, atomic `MIGRATION_V4` adds `harness_dispatches`. It follows
the `a2a_runs`/`a2a_commands` pattern: a versioned record, compare-and-set
updates, and the state change committed in the same transaction as its event.

```text
harness_dispatches(
  request_id TEXT PRIMARY KEY,         -- request UUID; the store belongs to one project
  task_id TEXT NOT NULL UNIQUE, run_id TEXT NOT NULL UNIQUE,
  fingerprint TEXT NOT NULL,           -- see below
  resource_key TEXT NOT NULL,          -- SHA-256 of the canonical working directory; no path stored
  phase TEXT NOT NULL CHECK(phase IN (<the eight phase names>)),
  claim_fence TEXT, version INTEGER NOT NULL CHECK(version > 0),
  record_json TEXT NOT NULL, updated_sequence INTEGER NOT NULL REFERENCES events(sequence),
  CHECK(claim_fence IS NOT NULL for running/cancel_requested,
        claim_fence IS NULL for admitted/recovery_pending))
UNIQUE INDEX harness_dispatches_active_resource ON harness_dispatches(resource_key)
  WHERE phase IN ('admitted','running','cancel_requested','recovery_pending')
```

- The fingerprint is SHA-256 over the project ID, request UUID, harness,
  protocol, prompt SHA-256, and config digest. The store resolves the project
  ID and derives the fingerprint itself (`request_fingerprint`); the writer
  does not supply it. The same request UUID with the same fingerprint returns
  the existing receipt (`duplicate: true`) and never launches again. A
  different fingerprint returns `harness_dispatch_conflict`.
- Admission rechecks what the store can see: the protocol accepts the harness
  (`harness_dispatch_invalid_request`), the prompt is 1 byte to 32 KiB
  (`harness_dispatch_invalid_prompt`), the protocol supports execution
  (`harness_dispatch_execution_disabled`), the harness is detected in the
  persisted registry (`harness_dispatch_not_detected`), and no reserving
  attempt holds the resource key (`harness_dispatch_busy`).
- The claim fence is stored only in its column, never in `record_json` or in
  an event, and its `Debug` form is redacted. A terminal finish keeps it, so a
  retried finish with the same fence and the same report (outcome, error
  code, process summary, capture summary) returns the stored result
  unchanged; a different report returns `harness_dispatch_conflict`. Recovery
  clears it.
- A finish report must be self-consistent before it is applied: `completed`
  needs no error code, exit code 0, and no forced termination; `failed` and
  `unverified` need an error code (`harness_dispatch_invalid_request`
  otherwise). `probed` is never a finish outcome
  (`harness_dispatch_invalid_transition`).
- The record keeps the latest error code. Each event carries only the code of
  its own transition, so acknowledging a `recovery_pending` attempt emits no
  code while the record keeps `harness_dispatch_interrupted`.
- A transition's timestamp is clamped to the record's `updated_at`, so
  timestamps never move backwards when the clock does.
- Every load cross-checks the indexed columns against `record_json`,
  recomputes the fingerprint, and checks that the Task/Run statuses are ones
  the phase allows (`DispatchPhase::allowed_statuses`: the canonical pair of a
  non-terminal phase, or a pair that an edge into a terminal phase produces).
  A `completed` record must also keep its evidence: a completed outcome, a
  capture summary, exit code 0, no forced termination, and no error code. Any
  disagreement fails closed with `store_harness_dispatch_projection_mismatch`.
- The Task and Run belong to the dispatch record. The generic projection
  commit refuses them and any new Run under a dispatch-owned Task
  (`store_harness_dispatch_bound_projection`), checked inside its write
  transaction. A2A refuses to bind a dispatch-owned task.
- Startup recovery runs in one transaction, with each attempt in its own
  savepoint. A row defect (a failed load cross-check, an undecodable row or
  record, or a rejected transition) rolls back only that attempt, which is
  quarantined with its fixed code; the other attempts still recover. The
  result lists the recovered commits and the quarantined request UUIDs with
  codes. Database and I/O failures still fail the whole transaction. A
  quarantined row is left unchanged, so an active one keeps its reservation
  and the project stays `harness_dispatch_busy` until an operator restores a
  backup; there is no in-place repair tool.
- The prompt is never persisted. Events and records store only its SHA-256 and
  byte length (AGENTS.md §11.2).
- Task title is `harness dispatch <harness>`, description is empty,
  `role = "dispatch"`, and `protocol` is the wire protocol label. `base_commit`
  is the project `HEAD`, which the daemon reads from the Git metadata at
  admission (`HEAD`, loose and packed refs, and a linked worktree's common
  directory, with bounded reads and a bounded symbolic-ref depth) without
  running a Git process; unreadable metadata refuses admission with
  `harness_process_invalid_cwd`. It records an observation, not an
  attestation of a clean tree.
- Only `WriterWorker` calls these transactions: `admit_harness_dispatch`,
  `claim_harness_dispatch`, `finish_harness_dispatch`,
  `cancel_harness_dispatch`, `recover_harness_dispatches` (returns the
  attempts it changed and the ones it quarantined), and the read
  `harness_dispatch`. The writer runs recovery before it reports ready, so
  nothing is admitted or claimed first; it starts even when rows are
  quarantined, and keeps the report (recovered request UUIDs, quarantined
  request UUIDs with codes, never prompts or paths) for
  `WriterHandle::harness_dispatch_recovery`. No Control operation exposes that
  report yet.

### 3. Attempt phases and canonical mapping

The pure `DispatchPhase` machine lives in `vibemux_harness`. The store applies
it and rejects any edge not listed here.

| From | Trigger | To | Task | Run |
|---|---|---|---|---|
| (none) | admit | `admitted` | `open → in_progress` | `preparing` |
| `admitted` | claim (single-use, mints fence) | `running` | unchanged | `preparing → running` |
| `admitted` | cancel | `cancelled` | `→ cancelled` | `preparing → failed` (there is no `preparing → stopped` edge) |
| `admitted` | startup recovery | `failed` (`harness_dispatch_interrupted`) | `→ blocked` | `preparing → failed`; reservation released (nothing was launched) |
| `running` | finish `completed` (fence) | `completed` | `→ done` | `running → succeeded`, authority `StructuredAdapter` |
| `running` | finish `failed` / `unverified` (fence) | same | `→ blocked` | `running → failed` |
| `running` | finish `cancelled` (fence) | `cancelled` | `→ cancelled` | `running → stopped` |
| `running` | cancel | `cancel_requested` | unchanged | unchanged |
| `cancel_requested` | finish, any outcome (fence) | `cancelled` | `→ cancelled` | `running → stopped` |
| `running`, `cancel_requested` | startup recovery | `recovery_pending` | `→ blocked` | `running → stale`; reservation kept |
| `recovery_pending` | cancel (operator acknowledgement) | `cancelled` | `blocked → cancelled` | `stale → stopped`; reservation released |
| terminal | repeat of the same request | unchanged | — | — |

- The claim is committed before the process is spawned. If the daemon crashes
  in `admitted`, no process ran. If it crashes in `running`, a process may
  have run, so the reservation is kept until an operator cancels.
- A finish without the current fence is rejected. Recovery clears fences, so
  a stale executor can never finish a recovered attempt.
- `Succeeded` means only that the requested turn ended with correlated
  structured evidence. It says nothing about quality, review, permission to
  commit, or release readiness.
- Events use the domain name `harness_dispatch_{admitted, started,
  cancel_requested, finished, recovered}`. Payloads are content-free: request
  UUID, harness, protocol, prompt hash and size, config digest, base commit,
  outcome, fixed error code, exit code, forced flag, per-kind record counts,
  stdout/stderr byte counts, and transcript SHA-256.

### 4. Execution in `vibemuxd`

- **Config.** Dispatch is opt-in through the trusted operator file
  `.vibemux/harness_dispatch.json` (schema 1, `snake_case` keys). The daemon
  reads it once at startup in `spawn_blocking`. It must be a regular
  non-symlink file of at most 64 KiB. Its SHA-256 is computed from the same
  bytes that are validated, and that digest is pinned for the daemon's
  lifetime. If the file is missing, dispatch ops return
  `harness_dispatch_unconfigured`. If it is invalid, they return the
  `harness_dispatch_config_*` code of the first failed check (shape, size,
  route, executable, or environment). Neither case blocks daemon startup.
  Executables must be absolute paths, canonicalized, and `.exe` on Windows;
  `.cmd`, `.bat`, and PowerShell shims are rejected rather than shimmed. The
  loader follows a linked executable and checks its canonical target: it must
  be a regular file outside the canonical project root, and its file stem must
  be the harness command name (`codex`, `claude`, `opencode`, `copilot`,
  `grok`; ASCII case-insensitive on Windows), so a cloned repository cannot
  plant the binary or point a route at an interpreter (open decision 2).
  Failures return `harness_dispatch_config_executable_invalid`. The
  file's trust boundary is the same logon SID as the rest of `.vibemux/`; no
  extra ACL is promised. Schema 1 carries **no operator argv**: every argument
  comes from the protocol profile. The installed CLIs expose more than 60
  permission, sandbox, working-directory, network, and config-source flags,
  and both clap and commander accept clustered short flags, so a denylist
  cannot stay complete. A per-protocol allowlist can be added later as an
  additive schema change.
- **Gates, all checked before any state change:** valid request, route
  `enabled` and `allow_execution` on a protocol that permits execution
  (Codex and Claude only; see below), harness `detected` in the persisted
  registry (ADR 013), and no other active reservation.
- **Working directory and concurrency (this slice):** vendor processes run in
  the canonical project root with the read-only protocol profiles from
  `a955484`: Codex `sandbox: read-only` and `approvalPolicy: never`; Claude
  `--bare`, `--tools=`, `--strict-mcp-config`, `--restricted`, and `dontAsk`
  (no built-in tools, no project or user MCP servers or settings files); ACP
  `fs` and `terminal` capabilities false; every approval or permission request
  declined. ACP agents run their own built-in tools and send only the
  permission requests their own configuration asks for. OpenCode documents
  `allow` as the default for most permissions, including `edit` and `bash`,
  and a project `opencode.json` can change it, so declining requests does not
  make an ACP route read-only. **ACP routes are therefore probe-only in this
  slice:** the config rejects `allow_execution: true` on an `acp` route
  (`harness_dispatch_config_route_invalid`), and both the admission gate and
  the launch builder refuse ACP execution (`harness_dispatch_execution_disabled`)
  even for a config that bypassed validation. The ACP execution session model
  stays as tested pure replay logic that the daemon cannot reach. Because the
  reservation key is the working directory, there is **at most one active
  dispatch per project**. These protocol settings are not an OS sandbox.
  Writable dispatch and concurrency both require owned per-Run worktrees and a
  separate ADR.
- **Executor.** One owned tokio task per attempt, held in the dispatch
  service's `JoinSet`, with a `watch` cancellation signal. An admission
  starts a task only for an `admitted` record not already running, and the
  claim is single-use, so a repeated or concurrent submit never launches
  twice. Order of work: claim; spawn the launch trampoline (Decision 6) with
  a cleared environment holding only the allowlisted names; contain it; send
  the go byte; queue the initial message or prompt; then loop.
  - A separate task owns the vendor's stdin and writes queued lines in order
    from a bounded queue of 16 lines. A vendor that leaves that many replies
    unread fails the attempt with `harness_process_pipe_failed`, and one that
    never drains its stdin cannot stall the stdout read, the cancel, or the
    deadline.
  - Each stdout chunk goes to the pure framer. Each record is accounted by
    the capture budget, appended directly to the attempt's in-memory
    transcript (Decision 5), then classified and passed to the session state
    machine, whose replies are queued for stdin.
  - After a terminal record, stdin closes and the vendor has the shutdown
    grace to exit; otherwise the tree is killed (open decision 4). The
    request deadline kills the tree and fails the attempt with
    `harness_dispatch_deadline_exceeded`.
  - When the loop ends, the process is ended (Decision 6), the pure
    `decide_outcome` is applied, then a fenced finish. Every writer call runs
    in `spawn_blocking`; a call that meets a saturated writer queue or a late
    response is retried up to 3 times with bounded backoff (50, 200, and
    800 ms). After that, the attempt is left for startup recovery; it is
    never reported as success.
  - The vendor path, its working directory, the protocol's working-directory
    field, and the reservation key use the canonical path without the Windows
    verbatim prefix (`\\?\`, with `\\?\UNC\` becoming `\\`), which vendor
    CLIs do not expect.
- **Cancellation.** A cancel op commits `cancel_requested`, then signals the
  executor; a cancel before the claim ends the attempt without launching. The
  executor sends the protocol cancel (`turn/interrupt`, `session/cancel`, or
  Claude `interrupt`) and reads trailing records for up to the shutdown grace.
  If the vendor does not confirm within the grace, or the protocol has no
  cancel message (`codex exec`), the contained tree is force-killed.
- **Shutdown.** The Control `shutdown` op stops admission and signals every
  in-flight attempt and probe before it replies, so a connection waiting on a
  probe ends before the server drains its connections. The attempt tasks
  cancel as above and commit their own finishes. After the drain, the daemon
  waits for the tasks up to the grace period plus 15 s;
  tasks still running then are aborted, which kills their trees, and startup
  recovery settles any attempt whose finish was not committed. Only then do
  plugin cleanup and writer shutdown run (existing order).
- **Probes** run in their own task, one at a time
  (`harness_dispatch_busy` otherwise), with the same launch, exchange, and
  teardown; their records are counted and discarded, and they write no
  canonical state. A probe's deadline is the request deadline capped at
  60 s, because the Control connection waits for its report.
- **Default limits** (validated ranges live in the config module): native
  frame 128 KiB; capture 16 MiB and 16,384 records per attempt; request
  deadline 600 s (1 s to 3,600 s); shutdown grace 2 s (0.1 s to 10 s);
  environment allowlist of at most 24 names; a model name of at most
  256 bytes; at most 16 routes.

### 5. Output capture

- The raw vendor JSON records of an attempt are kept byte-exact, including
  unknown fields, in a bounded in-memory transcript owned by the dispatch
  service. A record is appended when the capture budget accepts it, before
  the session interprets it, so the transcript and the capture summary cover
  the same records. Up to 4 finished transcripts (64 MiB total) are retained;
  the oldest is evicted first. Transcripts are lost on daemon restart. The output
  op then returns `harness_dispatch_output_unavailable`, while status and
  aggregates stay durable.
- Raw content never enters events, logs, `tracing` fields, health, probe, or
  error text. Stderr is counted and discarded.
- Durable transcript artifacts under `.vibemux/` are deferred to a later ADR.
  They would need their own retention and privacy decision.

### 6. Process-tree containment (`vibemux_platform`)

`vibemux_platform::ProcessTree` is one safe API with three steps:

1. `ProcessTree::prepare_command(&mut std::process::Command)` before spawn.
   Tokio callers pass `Command::as_std_mut()`.
2. `ProcessTree::contain(process_id)` right after spawn and before any input
   is written.
3. `terminate()`, or dropping the tree, kills every remaining member.

The API takes the child's id because tokio's `Child` exposes only a raw
Windows handle, which safe code cannot borrow. The caller must still hold
the un-reaped child, so the id cannot be reused. `contain` refuses the
daemon's own id on both platforms. If `contain` fails, the child keeps
running uncontained, and the executor must kill and reap it before reporting
the failure. Errors are `PlatformError::ProcessTree { operation, os_code }`
with a fixed step name (`platform_process_tree_failed`). They carry no path,
command line, or environment. The daemon maps them to
`harness_process_containment_failed`.

- **Windows** (`process_tree/windows_job_object.rs`, the only new `unsafe`
  module):
  - an unnamed Job Object with a non-inheritable handle and only
    `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, so a daemon crash also kills the
    tree;
  - breakaway is never permitted, so a member's `CreateProcess` with
    `CREATE_BREAKAWAY_FROM_JOB` fails with access denied. Processes that
    system services start on a member's behalf (WMI, Task Scheduler, COM
    servers) are outside the job, and containment does not cover them;
  - the child is opened by id with `PROCESS_SET_QUOTA | PROCESS_TERMINATE`
    and assigned; descendants created later join the job automatically;
  - every `terminate` call runs `TerminateJobObject` with exit code 1, so a
    repeated call also kills members created while an earlier call ran.
  - Members killed only by closing the handle report exit code 0, so an exit
    status is never evidence of success. The outcome rules already require a
    correlated terminal record.
  - Assignment happens after `CreateProcess`, so it is not atomic. The
    vendor CLI is therefore never the process that `contain` assigns; the
    launch trampoline below is (open decision 5).
  - A read-only `active_process_count()` exposes the job's accounting. The
    Stage 4 tests use it to show that a descendant the vendor starts before
    reading input is a member; the executor itself runs no leak scan.
- **POSIX** (`process_tree/posix_process_group.rs`, safe code): `std`'s
  `process_group(0)` makes the child lead a new group. `contain` verifies,
  through `rustix`'s safe `getpgid` and `getpgrp`, that the child leads its
  own group, and refuses a child spawned without preparation. It also
  refuses id 1, because signalling group 1 is `kill(-1)` and reaches every
  process the user may signal, and it refuses the daemon's own group.
  `terminate` sends `SIGKILL` to the group with `kill_process_group`;
  `ESRCH` means the group is already empty. After the first success it sends
  nothing more, so a later call cannot reach a reused id.
  - A member can leave the group with `setsid` or `setpgid`.
  - A daemon crash leaves the group running, because POSIX has no
    kill-on-close. Its stdin closes, and startup recovery marks the attempt
    `recovery_pending` without killing anything.
  - The group id stays reserved only while a member, or the un-reaped leader,
    exists. The executor therefore terminates (or drops the tree) before
    reaping the leader; the rustdoc on `terminate` and `Drop` states this.

**Launch trampoline (Stage 4).** The executor never spawns a vendor CLI
directly. It spawns `vibemux_launch_trampoline`, a first-party binary built
from the `vibemuxd` package, with the vendor executable and argv as its own
arguments and with the route's environment and working directory. The
executor contains the trampoline, then writes one go byte to its stdin.
Only after reading that byte does the trampoline spawn the vendor CLI. The
vendor is therefore a member from its first instruction, as is everything it
starts.
- The go byte is read from an unbuffered stdin handle, exactly one byte, so
  no protocol byte the executor writes afterwards is consumed.
- The vendor inherits the trampoline's stdin, stdout, and stderr. The
  trampoline waits for it and exits with its exit code. If the vendor could
  not be spawned or ended without a code, the trampoline exits nonzero. The
  attempt is then not clean, and so is never `Completed`.
- Anything other than the go byte, including end of file, makes the
  trampoline exit without spawning.
- Its own exit codes are fixed: 120 for a missing or relative vendor path,
  121 when not released, 122 when the vendor cannot be spawned, and 123 when
  the vendor ended without a code. It never searches `PATH`.
- The process is ended in one order: unless forced, the trampoline may exit
  on its own until the grace deadline; then the tree is terminated, and only
  then is the trampoline reaped (on POSIX its exit is observed without
  reaping, so the group id stays reserved). Stderr is drained and counted.
- The trampoline logic is safe code in `vibemux_platform`. The binary is a
  thin entry point, needs no tokio, and is used on every platform, so Linux
  CI exercises the same launch path. On POSIX the group already exists
  before exec, so there the trampoline adds uniformity, not safety.
- The daemon resolves the trampoline once at startup, as a regular file next
  to its own executable. If it is missing, dispatch ops return
  `harness_process_executable_unavailable`, and nothing is spawned.

`windows-sys` gains the `Win32_System_JobObjects` feature, and `rustix`
(`process`, `std`) becomes a Unix-only dependency. This amends ADR 025's
"exactly one `unsafe` module" rule to add `windows_job_object` as the second
reviewed module; `tests/unsafe_containment.rs` pins both.

Tests re-execute the test binary as a contained parent that spawns a
grandchild sharing its stdout. End of file on that pipe proves both exited,
on both platforms:
- both die on `terminate` and on drop;
- on Windows, both die when the owning process is killed, and a member's
  breakaway attempt fails with access denied;
- a negative control shows the grandchild outliving its killed parent until
  the tree is terminated;
- ids 0, 1, `u32::MAX`, and the test's own id are refused; on POSIX, so are
  a child outside its own group and a member naming its own group's leader.

A test that would otherwise be handed an unexpectedly contained tree leaks
it with `mem::forget` instead of dropping it, so a regression cannot signal
the test's own process or group, or everything through group 1.

Each of these mutations fails at least one test: on Windows, removing
kill-on-close, turning `terminate` into a no-op, permitting breakaway, or
removing the own-id check; on Linux, a no-op group kill, removing the
own-group check, or removing the init check (run under `setsid`, so that the
own-group check does not also catch id 1).

### 7. Control IPC v5 (Stage 5)

`CONTROL_PROTOCOL_VERSION` goes from 4 to 5. The six new operations require
version 5: the client refuses them against a v4 descriptor and the server
refuses them in a v4 request, both with `control_unsupported_version`. All
v1–v4 operations keep their minimum versions, and the daemon accepts v1–v5
requests. Arguments are compact JSON strings in `argument`, matching
`frontend_tasks`. Unknown fields, a missing argument, any argument to
`harness_dispatch_catalog`, or a lookup argument over 1 KiB return
`control_invalid_request`. The server side lives in
`crates/vibemuxd/src/control_harness_dispatch.rs`. The `Debug` form of a
Control request prints only the argument's byte count.

| Operation | Argument | Result (content-free unless noted) |
|---|---|---|
| `harness_dispatch_catalog` | none | Routes `{harness, protocol, enabled, allow_execution, probe_supported}`; no paths, model, or environment names |
| `harness_dispatch_probe` | `{harness}` | Initialize-only report `{harness, protocol, outcome, error_code, process, capture}`. Sends no prompt and writes no canonical state |
| `harness_dispatch_submit` | `{schema_version, request_id, harness, prompt}` | Receipt `{request_id, task_id, run_id, harness, protocol, phase, duplicate, sequence}`, returned right after the durable admit; `sequence` is the event that last changed the attempt |
| `harness_dispatch_status` | `{request_id}` | Allowlisted record `{request_id, task_id, run_id, harness, protocol, phase, version, outcome, error_code, process, capture, live_capture, prompt_bytes, base_commit, created_at, updated_at}` |
| `harness_dispatch_output` | `{request_id, cursor: {after_sequence, cursor_offset}}` | **Raw record fragments** `{fragments: [{sequence, kind, total_bytes, offset, fragment}], next, complete}`, the only result that carries vendor content |
| `harness_dispatch_cancel` | `{request_id}` | The status record after the cancel. Idempotent while in flight; cancelling a completed or failed attempt returns `harness_dispatch_terminal` |

- **Status** omits the prompt hash, the fingerprint, the config digest, and
  the project ID. `live_capture` `{record_count, record_bytes}` is present
  only while the attempt runs in this daemon; `capture` is the durable final
  summary. An unknown request UUID returns `harness_dispatch_not_found` from
  status, output, and cancel.
- **Output pages.** Records are numbered from 1 in capture order. The cursor
  names the next byte to send: `after_sequence` is the last record fully
  delivered, and `cursor_offset` is a byte offset into the record after it.
  A nonzero offset must fall inside that record on a UTF-8 boundary;
  otherwise the op returns `harness_dispatch_invalid_request`. A page holds
  at most 24 KiB of raw record bytes and 256 fragments. The daemon sizes it
  by its exact encoded length, JSON escaping included, so it stays 2 KiB
  under the 64 KiB frame. A record longer than a page is split on UTF-8
  character boundaries. `next` is the cursor for the following call.
  `complete` is true only when the attempt has finished and `next` is past
  the last record; an empty page with `complete: false` means the reader has
  caught up with a running attempt. A known attempt whose transcript is gone
  (daemon restart or eviction) returns `harness_dispatch_output_unavailable`.
  `vibemuxd::harness_dispatch::OutputReassembler` checks that each page
  starts at the previous cursor and that `next` matches, and yields whole,
  byte-exact records.
- **Frame budget.** Prompts are limited to 32 KiB. A submit carries the
  prompt JSON-escaped twice (in the argument string, then in the frame), so
  ordinary text up to the limit fits. A prompt dense in quotes, backslashes,
  or control characters can exceed the 64 KiB frame; the client then refuses
  it with `control_frame_too_large` before sending anything. The daemon
  checks every dispatch response against the frame budget and returns
  `control_frame_too_large` instead of an oversized frame.
- **Deadlines.** Operations use the 10 s Control deadline, except that the
  client waits up to 90 s for a probe: the 60 s probe cap, the 10 s maximum
  shutdown grace, and a 20 s margin.
- Error codes use the `harness_` namespace, so they round-trip as
  `ControlError::Remote{code}`. The closed set is the
  `vibemux_harness::dispatch::DispatchError` enum; nothing else is sent:
  - `harness_dispatch_{unconfigured, config_invalid, config_too_large,
    config_route_invalid, config_executable_invalid,
    config_environment_invalid, route_unavailable, execution_disabled,
    not_detected, probe_unsupported, invalid_request, invalid_prompt, busy,
    conflict, not_found, output_unavailable, terminal, invalid_transition,
    stale_fence, interrupted, cancelled, deadline_exceeded, internal}`;
  - `harness_protocol_*` for framing, JSON-RPC, correlation, and terminal
    evidence failures (for example `harness_protocol_eof_without_terminal`);
  - `harness_capture_too_large`;
  - `harness_process_*` for executable, working-directory, environment,
    spawn, containment, pipe, and exit failures.
- **CLI.** `vibemuxctl dispatch` (`crates/vibemux_cli/src/dispatch_commands.rs`)
  is a thin client that needs a running daemon and never starts one. Each
  command prints one JSON object; errors print `{ok: false, error_code,
  error}`.

  ```text
  vibemuxctl dispatch catalog [--project-root <path>]
  vibemuxctl dispatch probe <harness> [--project-root <path>]
  vibemuxctl dispatch submit <harness> --prompt-file <path|-> [--request-id <uuid>] [--project-root <path>]
  vibemuxctl dispatch status <request_id> [--project-root <path>]
  vibemuxctl dispatch output <request_id> [--after-sequence <n>] [--project-root <path>]
  vibemuxctl dispatch cancel <request_id> [--project-root <path>]
  ```

  - A harness is named by its command (`opencode`) or wire name
    (`open_code`).
  - `submit` reads the prompt from the file, or from stdin for `-`, never
    from argv or a shell. It reads at most 32 KiB plus one byte and refuses a
    blank, non-UTF-8, or oversized prompt with
    `harness_dispatch_invalid_prompt` before contacting the daemon; an
    unreadable source returns `cli_prompt_unavailable`. Without
    `--request-id` it generates a UUID v4; submitting again with the same id
    is an idempotent retry.
  - `output` reads pages until `complete` or caught up (at most 4,096 pages)
    and prints whole records `{sequence, kind, raw_json}` with
    `next_after_sequence` for a later `--after-sequence`.
  - A dispatch command against a pre-v5 daemon returns
    `control_unsupported_version`, not a stale-runtime error.

## Consequences

- Main gains a real, canonical path from prompt to structured vendor turn to
  audited Task/Run outcome. The single writer, detection gating, and
  content-free public JSON stay intact.
- Conservative recovery trades availability for safety. A crash during
  `running` blocks the project's dispatch reservation until an operator
  cancels, and a quarantined defective row blocks it until a backup is
  restored.
- With one dispatch per project, this slice does not yet provide concurrent
  multi-harness execution.
- Prompts execute only through the Codex and Claude routes. OpenCode, Copilot,
  and Grok are reachable for initialize-only probes until a verified ACP deny
  posture exists.
- `unsafe` surface grows by one narrow Windows platform module (Job Object
  create, assign, terminate, and query). POSIX containment is safe code. The
  module needs deep review, and its tests run on Windows and Linux.
- Every dispatch runs one extra first-party process, the launch trampoline,
  which must be installed next to `vibemuxd`.
- Vendor protocol churn is caught by the pure fixture-replay tests. Live
  vendor behavior is only verified against installed CLIs, and paid model
  inference runs only with explicit authorization.
- The frontend Send action stays unavailable (ADR 028). A one-shot dispatch is
  not a continuous coordinator-chat adapter.

## Alternatives

- **Plugin-hosted executor**, the shape of `a955484`: the daemon supervises a
  first-party `vibemux_harness_plugin` process that does the vendor I/O, and
  the core replays the transcript. It matches the current core/plugin table,
  but it adds a second process hop, duplicate validation, and wire encoding of
  every record. Kept as the route for third-party adapters.
- **Cherry-pick or merge `a955484`:** rejected. It breaks the pure-crate rule,
  has 12 conflicting files, and duplicates main's registry.
- **A2A loopback listener for admission** (codex drafts): rejected for this
  slice. Control IPC is already authenticated, per-user, and single-owner. A2A
  exposure of dispatch can follow as a separate mapping.
- **Durable per-record observation events:** rejected. Up to 16,384 events per
  attempt adds no audit value over content-free aggregates plus a transcript
  hash.
- **In-memory leases only:** rejected. Leases lost on restart would allow a
  second launch while an orphaned process may still run.

## Compatibility and rollback

- Store schema 3 → 4 is additive, atomic, and forward-only, with
  historical-fixture migration tests. A schema-3 binary cannot open a schema-4
  store. To downgrade, restore a backup taken while the daemon was stopped.
- Control v5 adds operations and changes none. The daemon still accepts
  v1–v4 requests, but a pre-v5 client refuses a daemon descriptor that
  advertises v5 (`control_unsupported_version`, as at the v3 to v4 bump), so
  `vibemuxctl` and `vibemuxd` are upgraded together. A v5 client against a
  v4 daemon keeps every older operation and receives
  `control_unsupported_version` for the dispatch operations.
- Plugin wire v1.0 and the Python reference are unchanged.
- To roll back: delete or rename `.vibemux/harness_dispatch.json` and restart
  the daemon. With no config, every dispatch op returns
  `harness_dispatch_unconfigured` and nothing is spawned.

## Implementation plan (stages 1 to 6 done on the branch, Stage 7 partial)

Branch `feat/harness_dispatch_port`, rebased onto `main` after PR #9 lands.
PR #9 carries Control v4 and ADR 028, which this work depends on. Each stage
is one reviewable commit with its own tests.

| Stage | Owned files | Content | Gate |
|---|---|---|---|
| 1 | `crates/vibemux_harness/src/dispatch/{mod,error_code,digest,request,route_config,launch_spec,protocol_session,observation,json_line_framer,capture_budget,attempt,outcome,events}.rs`, `tests/dispatch_*.rs`, `tests/fixtures/dispatch/*.jsonl` | Port the pure logic from `a955484`; add the attempt machine and `decide_outcome` | Unit and fixture-replay tests for every protocol; exhaustive transition table; framer split, CRLF, oversize, and UTF-8 cases |
| 2 | `crates/vibemux_platform/src/process_tree{,/windows_job_object,/posix_process_group}.rs`, `lib.rs` export and error variant, `tests/{process_tree,unsafe_containment}.rs`, `windows-sys` feature and Unix `rustix` dependency | Containment primitive | A descendant is killed on terminate and on handle drop (Windows and POSIX) and, on Windows, when the owner dies; breakaway is refused on Windows; the own id, init, and the own group are refused; deep review of `unsafe` |
| 3 | `crates/vibemux_store/src/harness_dispatch{,/dispatch_rows}.rs`, `tests/harness_dispatch.rs`, `lib.rs` migration hook, error variants and projection guard, `a2a.rs` binding guard, `uuid` dependency; `request_fingerprint` in `vibemux_harness` | Schema 4 and transactions | Idempotent admit; conflict; prompt bounds; unique reservation; single-use claim; redacted fence; cancel before and after claim; recovery with per-attempt quarantine; a retried finish must repeat its report; failures carry a code; monotonic timestamps; `Succeeded` only with authority; atomic migration and reopen; corrupted rows and changed terminal evidence fail closed |
| 4 | `crates/vibemux_platform/src/process_tree/launch_trampoline.rs`, `leader_exited` in `process_tree{,/posix_process_group}.rs` (+ tests), `executable_names_harness` in `vibemux_harness`, `crates/vibemuxd/src/bin/{vibemux_launch_trampoline,vibemux_native_fixture}.rs` (the fixture only with `test_helpers`), `crates/vibemuxd/src/harness_dispatch/{mod,config_loader,git_head,native_process,executor,transcript_store}.rs`, writer arms and startup recovery in `lib.rs`, service start and join in `control.rs`, `tests/{launch_trampoline,harness_dispatch}.rs` | Trampoline, service, executor, capture, recovery at writer start | Done: the trampoline passes the bytes after the go byte through intact, propagates the exit code, starts nothing unreleased, and contains a descendant the vendor starts before reading input. Fixture-binary integration tests, ported from `a955484`'s `process_dispatch.rs`: each execution protocol completes with a byte-exact transcript; probes for ACP, app-server, and Claude; admission gates and ACP probe-only; duplicate, conflict, and concurrent repeats; busy; confirmed and forced cancel; deadline; protocol violations; stderr counted only; a vendor that never reads its prompt; a vendor-started descendant ends with the attempt; shutdown during a run; restart to `recovery_pending`; the executable trust rules; and no prompt, vendor output, stderr, or path in the canonical database |
| 5 | `crates/vibemuxd/src/control_harness_dispatch.rs`, `control.rs` op table, payloads, version, and shutdown signal, `crates/vibemuxd/src/harness_dispatch/{mod,output_page,transcript_store}.rs` (output pages, live capture, probe cap, `begin_shutdown`), the fixture's `hang_initialize` mode, `tests/control_harness_dispatch.rs`, `crates/vibemux_cli/src/{dispatch_commands,lib,main}.rs`, `uuid` dependency in `vibemux_cli` | Control v5 and `vibemuxctl dispatch` | Done: v5 ops refused at v4 by client and server, v1–v5 accepted; malformed and oversized arguments refused; codes round-trip; over real Control IPC with the fixture binary, a submit near the prompt limit completes and its multi-page output reassembles byte-exact (SHA-256 of every record matches); duplicate submit; live capture and cancel of a running attempt; terminal cancel; a Control shutdown ends a waiting probe promptly (the test fails without the shutdown signal); the largest page, status, and ordinary prompt fit the frame; CLI parsing, prompt bounds, and error codes |
| 6 | `docs/architecture.md`, `docs/protocol_boundaries.md`, `docs/platform_support.md`, `CLAUDE.md`, `README.md`, `CHANGELOG.md`, `PROGRESS.md`, the AGENTS.md §3.2 exception, ADR 025's status line, this ADR → Accepted | Documentation matches the code | Done: relative links and anchors in the edited documents resolve; `cargo fmt --check` and `clippy -D warnings` are clean; `cargo test --workspace --all-features -- --test-threads=1` on Windows passes 488 tests with 2 ignored; Python pytest (63 tests), Ruff lint and format, mypy, and the mock smoke pass |
| 7 | `docs/evidence/harness_dispatch_validation.md`, the status lines of this ADR and of the documents Stage 6 edited, `PROGRESS.md` | Live native Windows: `dispatch probe` against the installed CLIs (initialize only, no inference). A real `submit` only with explicit authorization of paid inference | Partial: initialize-only probes of the installed Codex, Claude, OpenCode, Copilot, and Grok CLIs are recorded in the [evidence](../evidence/harness_dispatch_validation.md). With the default grace, four are `probed` and Grok is `failed` by a forced kill after a clean but slow exit; with a 5 s grace, all five are `probed`. `submit` was not run and stays unverified until paid inference is authorized |

### Open decisions before Stage 4

Items 1, 2, and 5 are resolved and implemented. Stage 7 resolved item 3 for
flag parsing. Item 4 stays open with the conservative behavior described,
now with live evidence.
Since Stage 5, `harness_dispatch_submit` and `harness_dispatch_probe` are the
only Control operations that launch a vendor process, and only through a
route in a valid `.vibemux/harness_dispatch.json`.

1. **ACP execution posture — resolved: probe-only in this slice.**
   OpenCode's permissive defaults (Decision 4) mean an ACP prompt can edit
   files and run shell commands without a request, and Copilot and Grok
   defaults are unverified. ACP routes accept probes only (Decision 4, stage 1
   amendment). Enabling ACP execution needs a protocol-owned, per-vendor deny
   posture verified live (for OpenCode, an injected permission config) and a
   new recorded decision.
2. **Executable trust — resolved: outside the root and named for the
   harness.** `.vibemux/harness_dispatch.json` lives in the project tree, so
   it can arrive with a cloned repository and name any absolute `.exe`. The
   config stays in `.vibemux/`, and the loader binds each executable to
   properties the repository cannot choose: the canonical target must lie
   outside the canonical project root, and its file stem must be the
   harness command name (Decision 4). A route can therefore name only a
   binary installed outside the checkout under that harness's own name, not
   one the checkout ships and not an interpreter such as `cmd.exe` or
   `powershell.exe`. Rejected alternatives: requiring the probe cache's
   resolved path, which ties dispatch to a cache refresh and to `PATH`
   order; and a per-user route file, which adds a second configuration
   location and precedence rules.
3. **Claude flags — resolved for parsing (Stage 7).** `--strict-mcp-config`
   and `--restricted` were first verified only by `--help`. The Stage 7
   initialize probe of Claude Code 2.1.283 accepted the full route argv,
   including `--bare` stream-json, got a successful initialize response, and
   exited with code 0 on its own. Their effect during a turn stays
   unverified until a live submit runs.
4. **Forced exit after a terminal.** A correlated terminal followed by a
   forced kill (for example, an app-server that does not exit within the
   grace period after stdin closes) is `failed` with `harness_process_failed`.
   This is conservative and may need a narrower rule once Stage 4 observes
   real shutdown behavior. Stage 7 observed it live: Grok 1.0.41 answers an
   ACP initialize and exits with code 0 about 2.1 s after stdin closes, just
   past the 2 s default grace, so its probe is `failed`; with
   `limits.shutdown_grace_ms` at 5,000 it is `probed`. A larger default, a
   per-route grace, or a narrower rule is still undecided. The grace also
   bounds the wait after a cancel.
5. **Windows startup window — resolved: launch trampoline.** Job
   assignment follows `CreateProcess` (Decision 6), so a descendant that a
   vendor CLI starts before `contain` returns, without waiting for input,
   would escape the job. Vendor CLIs are launched through the first-party
   trampoline (Decision 6), which is contained before it starts them.
   Rejected alternatives:
   - accepting the window and reporting escapes through the leak scan, which
     detects an escape only after the fact;
   - `CREATE_SUSPENDED` with a resume after assignment. `std` cannot resume
     the main thread, so this needs more `unsafe` in the Job Object module.

Out of scope: writable or owned-worktree dispatch, more than one dispatch per
project, multi-turn or resume, executing approvals, the frontend Send action,
A2A exposure, durable transcript artifacts, streaming subscriptions over
Control IPC, replay-only direct JSON protocols (`opencode_json`,
`copilot_json`, `grok_stream_json`), the CLI/workspace/frontend refactors in
`a955484`, and any change to the codex branches or their worktrees.
