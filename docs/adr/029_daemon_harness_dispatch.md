# ADR 029: Daemon-owned harness request dispatch and output capture

Status: Proposed. Stage 1 (the pure `vibemux_harness::dispatch` logic) and
Stage 2 (the `vibemux_platform::ProcessTree` containment primitive) are
implemented on `feat/harness_dispatch_port`; nothing is wired into the daemon,
store, or Control IPC, and nothing in this ADR is implemented on `main` until
`PROGRESS.md` records it. The Stage 2 `unsafe` module needs this ADR accepted
before merge (AGENTS.md §7.2).

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
  request_key TEXT PRIMARY KEY,        -- project_id + request UUID
  task_id TEXT NOT NULL UNIQUE, run_id TEXT NOT NULL UNIQUE,
  fingerprint TEXT NOT NULL,           -- see below
  resource_key TEXT NOT NULL,          -- SHA-256 of the canonical working directory; no path stored
  phase TEXT NOT NULL, claim_fence TEXT, version INTEGER NOT NULL CHECK(version > 0),
  record_json TEXT NOT NULL, updated_sequence INTEGER NOT NULL REFERENCES events(sequence))
UNIQUE INDEX ... ON harness_dispatches(resource_key)
  WHERE phase IN ('admitted','running','cancel_requested','recovery_pending')
```

- The fingerprint is SHA-256 over the project ID, request UUID, harness,
  protocol, prompt SHA-256, and config digest. The same request UUID with the
  same fingerprint returns the existing receipt (`duplicate: true`) and never
  launches again. A different fingerprint returns `harness_dispatch_conflict`.
- The prompt is never persisted. Events and records store only its SHA-256 and
  byte length (AGENTS.md §11.2).
- Task title is `harness dispatch <harness>`, description is empty,
  `role = "dispatch"`, and `protocol` is the wire protocol label. `base_commit`
  is the project `HEAD`, which the daemon resolves with the shell-free Git
  runner at admission. It records an observation, not an attestation of a
  clean tree.
- Only `WriterWorker` calls these transactions: `admit`, `claim`, `finish`,
  `request_cancel`, `recover`, and a read by request.

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
  service's `JoinSet`, with a `watch` cancellation signal. Order of work:
  claim, spawn, attach containment, send the initial message or prompt, then
  loop. Each stdout chunk goes to the pure framer. Each record is classified
  and passed to the session state machine; its replies go to stdin. Records
  flow to the capture over a bounded channel (capacity 16). When the loop ends,
  the pure `decide_outcome` is applied, then a fenced finish. A finish that
  hits writer backpressure is retried up to 3 times with bounded backoff. After
  that, the attempt is left for startup recovery; it is never reported as
  success.
- **Cancellation.** A cancel op commits `cancel_requested`, then signals the
  executor. The executor sends the protocol cancel (`turn/interrupt`,
  `session/cancel`, or Claude `interrupt`), drains trailing records within the
  shutdown grace, then force-kills the contained tree.
- **Shutdown.** Control shutdown stops admission, cancels every in-flight
  attempt, waits for the grace period, force-kills, commits the finishes, and
  joins the executors. Only then do plugin cleanup and writer shutdown run
  (existing order).
- **Default limits** (validated ranges live in the config module): native
  frame 128 KiB; capture 16 MiB and 16,384 records per attempt; request
  deadline 600 s (1 s to 3,600 s); shutdown grace 2 s (0.1 s to 10 s);
  environment allowlist of at most 24 names; a model name of at most
  256 bytes; at most 16 routes.

### 5. Output capture

- The raw vendor JSON records of an attempt are kept byte-exact, including
  unknown fields, in a bounded in-memory transcript owned by the dispatch
  service. Up to 4 finished transcripts (64 MiB total) are retained; the
  oldest is evicted first. Transcripts are lost on daemon restart. The output
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
  - Assignment happens after `CreateProcess`, so it is not atomic. Nothing
    is written to stdin until `contain` succeeds, which covers descendants
    started in response to input. A descendant the vendor CLI starts on its
    own during that window, before reading any input, escapes the job. This
    residual risk is open decision 5.
  - A read-only `active_process_count()` exposes the job's accounting for the
    Stage 4 leak scan.
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

### 7. Control IPC v5

`CONTROL_PROTOCOL_VERSION` goes from 4 to 5. The new operations require
version 5. All v1–v4 operations stay accepted with their existing minimum
versions. Arguments are JSON strings in `argument`, matching the style of
`frontend_tasks`.

| Operation | Argument | Result (content-free unless noted) |
|---|---|---|
| `harness_dispatch_catalog` | none | Routes: harness, protocol, `enabled`, `allow_execution`, `probe_supported`; no paths, model, or environment names |
| `harness_dispatch_probe` | `{harness}` | Initialize-only handshake: status, codes, counts. Sends no prompt and writes no canonical state |
| `harness_dispatch_submit` | `{schema_version, request_id, harness, prompt}` | Admission receipt `{request_id, task_id, run_id, phase, duplicate, sequence}`, returned right after the durable admit |
| `harness_dispatch_status` | `{request_id}` | Phase, outcome, fixed codes, aggregates, and live counters while running |
| `harness_dispatch_output` | `{request_id, after_sequence, cursor_offset}` | **Raw record fragments** `{sequence, kind, total_bytes, offset, fragment}`, at most 24 KiB of raw bytes and 256 items per page, split on UTF-8 boundaries, plus the next cursor and `complete` |
| `harness_dispatch_cancel` | `{request_id}` | New phase. Idempotent; cancelling a completed or failed attempt returns `harness_dispatch_terminal` |

- Prompts are limited to 32 KiB so a submit fits the 64 KiB control frame.
  The daemon also checks the encoded size of every output page against the
  frame limit before sending it.
- Error codes use the `harness_` namespace, so they round-trip as
  `ControlError::Remote{code}` today. The closed set is the
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
- `vibemuxctl dispatch {catalog|probe|submit|status|output|cancel}` are thin
  clients. `submit` reads the prompt from `--prompt-file` or stdin; the prompt
  is data and never goes through argv or a shell.

## Consequences

- Main gains a real, canonical path from prompt to structured vendor turn to
  audited Task/Run outcome. The single writer, detection gating, and
  content-free public JSON stay intact.
- Conservative recovery trades availability for safety. A crash during
  `running` blocks the project's dispatch reservation until an operator
  cancels.
- With one dispatch per project, this slice does not yet provide concurrent
  multi-harness execution.
- Prompts execute only through the Codex and Claude routes. OpenCode, Copilot,
  and Grok are reachable for initialize-only probes until a verified ACP deny
  posture exists.
- `unsafe` surface grows by one narrow Windows platform module (Job Object
  create, assign, terminate, and query). POSIX containment is safe code. The
  module needs deep review, and its tests run on Windows and Linux.
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
- Control v5 is additive. Older clients keep working. New clients probing an
  older daemon receive the existing `control_version_unsupported` error.
- Plugin wire v1.0 and the Python reference are unchanged.
- To roll back: delete or rename `.vibemux/harness_dispatch.json` and restart
  the daemon. With no config, every dispatch op returns
  `harness_dispatch_unconfigured` and nothing is spawned.

## Implementation plan (proposed; stages 1 and 2 done on the branch)

Branch `feat/harness_dispatch_port`, rebased onto `main` after PR #9 lands.
PR #9 carries Control v4 and ADR 028, which this work depends on. Each stage
is one reviewable commit with its own tests.

| Stage | Owned files | Content | Gate |
|---|---|---|---|
| 1 | `crates/vibemux_harness/src/dispatch/{mod,error_code,digest,request,route_config,launch_spec,protocol_session,observation,json_line_framer,capture_budget,attempt,outcome,events}.rs`, `tests/dispatch_*.rs`, `tests/fixtures/dispatch/*.jsonl` | Port the pure logic from `a955484`; add the attempt machine and `decide_outcome` | Unit and fixture-replay tests for every protocol; exhaustive transition table; framer split, CRLF, oversize, and UTF-8 cases |
| 2 | `crates/vibemux_platform/src/process_tree{,/windows_job_object,/posix_process_group}.rs`, `lib.rs` export and error variant, `tests/{process_tree,unsafe_containment}.rs`, `windows-sys` feature and Unix `rustix` dependency | Containment primitive | A descendant is killed on terminate and on handle drop (Windows and POSIX) and, on Windows, when the owner dies; breakaway is refused on Windows; the own id, init, and the own group are refused; deep review of `unsafe` |
| 3 | `crates/vibemux_store/src/harness_dispatch.rs` (+ tests), `lib.rs` migration hook | Schema 4 and transactions | Idempotent admit; conflict; unique reservation; single-use claim; fence; cancel before and after claim; recovery; `Succeeded` only with authority; atomic migration and reopen |
| 4 | `crates/vibemuxd/src/harness_dispatch/{mod,config_loader,native_process,executor,transcript_store}.rs`, writer arms in `lib.rs`, `src/bin/vibemux_native_fixture.rs` (`test_helpers`) | Service, executor, capture, recovery at writer start | Per-protocol integration tests with the fixture binary, ported from `a955484`'s `process_dispatch.rs` (19 scenarios); shutdown during a run; restart to `recovery_pending`; leak scan |
| 5 | `crates/vibemuxd/src/control_harness_dispatch.rs`, `control.rs` op table and version, `crates/vibemux_cli/src/*` | Control v5 and `vibemuxctl dispatch` | Version gating (v5 ops refused at v4; v1–v4 accepted); code round-trip; output page reassembles byte-exact within the frame budget |
| 6 | `docs/architecture.md`, `docs/protocol_boundaries.md`, `CLAUDE.md`, `README.md`, `CHANGELOG.md`, `PROGRESS.md`, this ADR → Accepted | Documentation matches the code | Link check; `cargo fmt`, `clippy -D warnings`, `cargo test --workspace --all-features` (serial on Windows, issue #6); Python regression |
| 7 | `docs/evidence/harness_dispatch_validation.md` | Live native Windows: `dispatch probe` against the installed CLIs (initialize only, no inference). A real `submit` only with explicit authorization of paid inference | Otherwise recorded as unverified |

### Open decisions before Stage 4

Stage 1 executes nothing, so these do not block it. Each open item needs a
recorded decision before the executor can launch a vendor process:

1. **ACP execution posture — resolved: probe-only in this slice.**
   OpenCode's permissive defaults (Decision 4) mean an ACP prompt can edit
   files and run shell commands without a request, and Copilot and Grok
   defaults are unverified. ACP routes accept probes only (Decision 4, stage 1
   amendment). Enabling ACP execution needs a protocol-owned, per-vendor deny
   posture verified live (for OpenCode, an injected permission config) and a
   new recorded decision.
2. **Executable trust.** `.vibemux/harness_dispatch.json` lives in the project
   tree, so it can arrive with a cloned repository and name any absolute
   `.exe`. The loader must bind the executable to something the repository
   cannot choose. For example, it could require the probe cache's resolved
   path for that harness, reject executables under the project root, or read
   routes from a per-user location.
3. **Claude flags.** `--strict-mcp-config` and `--restricted` are verified
   only by `--help`; the Stage 7 initialize probe confirms they combine with
   `--bare` stream-json.
4. **Forced exit after a terminal.** A correlated terminal followed by a
   forced kill (for example, an app-server that does not exit within the
   grace period after stdin closes) is `failed` with `harness_process_failed`.
   This is conservative and may need a narrower rule once Stage 4 observes
   real shutdown behavior.
5. **Windows startup window.** Job assignment follows `CreateProcess`
   (Decision 6), so a descendant that a vendor CLI starts before `contain`
   returns, without waiting for input, escapes the job. The options are:
   - accept the window and let the Stage 4 leak scan report it;
   - launch through a first-party trampoline that is contained first and
     starts the vendor CLI only after it reads a go byte;
   - create the child with `CREATE_SUSPENDED` and resume its main thread after
     assignment. `std` cannot resume the thread, so this needs more `unsafe`
     in the Job Object module.

Out of scope: writable or owned-worktree dispatch, more than one dispatch per
project, multi-turn or resume, executing approvals, the frontend Send action,
A2A exposure, durable transcript artifacts, streaming subscriptions over
Control IPC, replay-only direct JSON protocols (`opencode_json`,
`copilot_json`, `grok_stream_json`), the CLI/workspace/frontend refactors in
`a955484`, and any change to the codex branches or their worktrees.
