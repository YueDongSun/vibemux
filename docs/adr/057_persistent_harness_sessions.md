# ADR 057: Daemon-owned persistent Claude and Codex conversations

Status: **Accepted for the read-only persistent-session scope**, 2026-11-01.
The original proposal text below is preserved as the decision history. The
accepted implementation scope, the allocated versions, and the compatibility
boundary are recorded in
[Accepted implementation scope](#accepted-implementation-scope-and-allocated-versions-2026-11-01).

Writable chats, worktree ownership, tool execution and approval decisions remain
proposed. Native navigation, settings and bilingual draft handling passed a
bounded Windows check. Claude 2.1.304 and Codex 0.166.0 passed bounded live
follow-up and restart-resume checks. Full S5 qualification remains open.

Extends [ADR 029](029_daemon_harness_dispatch.md),
[ADR 032](032_dsh_harness_adapter.md), and
[ADR 043](043_tauri_desktop_app.md). Retains
[ADR 036](036_runtime_execution_authority.md)'s execution-authority gates and
[ADR 033](033_mobile_access_channels.md)'s read-only monitor boundary.

## Accepted implementation scope and allocated versions (2026-11-01)

The proposal preserved below is accepted for a **read-only persistent-session
scope**. The daemon backend and the desktop chat surface are implemented and
locally tested. Bounded native navigation, settings and draft checks passed. Saved-history interaction and persistent-vendor inference remain unverified. The accepted
scope is exactly the following, and nothing beyond it.

### Allocated versions

| Item | Value | Note |
|---|---|---|
| Local control protocol | **Control v11** | Adds the persistent-chat operation. |
| SQLite store schema | **10** | Adds the chat metadata tables. Forward-only. |
| Chat metadata schema | **1** (`CHAT_SCHEMA_VERSION`) | The chat payload schema version, distinct from the store schema. |

The earlier text that called these "historical reservations, not allocations"
is superseded by this table. The implementation inspected for this decision
was the current parent source, not the `617bb1f` baseline described below.

### What is implemented

- **A daemon-owned chat service.** One bounded command owner serializes session
  open, send, cancel, rename, close, archive, resume, purge, and private
  live-text reads. A chat is a separate session aggregate with its own states
  (`opening`, `ready`, `busy`, `suspended`, `recovery_required`, `closed`,
  `archived`); it never becomes a completed Task because one turn finished.
- **A fixed chat request contract.** The control surface is a single
  authenticated chat operation carrying a closed request enum tagged by
  `operation`: `list`, `open`, `lookup_open`, `status`, `submission`, `send`,
  `cancel`, `turns`, `title`, `content`, `live`, `rename`, `close`, `archive`,
  `resume`, `purge`. This is one Control operation with a fixed request enum,
  not the many separately named operations this proposal sketched. Unknown
  operations and unknown fields are rejected.
- **Durable metadata plus protected private bodies.** Session, turn, and record
  metadata live in the single writer. Private text — titles, prompts, assistant
  output — is stored as immutable private bodies referenced by an opaque blob
  id, a SHA-256 digest, and a byte count. A reference is never a path, and the
  writer publishes a private body before the reference that names it commits.
- **Separate identity-bound chat consent.** Opening a chat requires an explicit
  `consent_private_storage` grant on the request; the daemon refuses an open
  without it. This is a different grant from the one-shot Ask consent.
  Project registration is an identity boundary rather than a consent of its
  own: it establishes which directory the app is bound to, and it carries no
  separate consent grant.
- **Read-only startup, list, and status.** Daemon startup creates no private
  storage and launches no vendor process. Listing and status read committed
  metadata only. Only explicit private reads — `title`, `content`, `live` —
  return text.
- **Exact Claude and Codex resume.** A Claude chat resumes by the exact stored
  vendor session id (`--resume <session_id>`), never by `--continue` or
  latest-session selection; a new Claude chat takes `--session-id <session_id>`
  plus `--replay-user-messages` and drops `--no-session-persistence`, while the
  one-shot profile keeps that flag. A Codex chat resumes the exact stored thread
  id through the app-server protocol. A resume refuses when the operator route
  profile or workspace resource key changed since the chat was opened.
- **A canonical Task/Run per turn.** Each admitted turn carries its own
  `task_id` and `run_id` and records the Git HEAD observed at admission. The
  chat is not a single lifetime Task.
- **Idempotent replay across restart and purge.** Session operations are bound
  to an operation id, sends to a request id, and the writer consults the durable
  binding first. A repeated send after a daemon restart, or after the chat's
  private content was purged, returns the original receipt instead of creating
  a second effect, and a purge never recreates text it removed.
- **Close reserves before it stops.** A close records its operation before the
  process owner is stopped, so a close that is interrupted cannot leave a
  session that no operation id can address. This repair was covered by the
  independent scoped source review.
- **Namespace collision checks.** Chat operation ids and one-shot Ask request
  ids are checked against each other in both directions, so an identifier from
  one namespace cannot be mistaken for an identifier in the other.
- **Bounded private reads.** Stored records are read through a cursor that
  names the record after `after_sequence` plus a byte offset inside that
  record; a completed record advances the sequence and resets the offset. Live
  reads additionally bind a replacement revision. Text is delivered in chunks
  of at most 8,192 bytes and a chat response is bounded before it leaves the
  daemon.
- **Renderer handoff.** A successful complete private Assistant body replaces
  only its matching turn/attempt's live assistant snapshot. Ordinary tool and
  notice records cannot end the stream, and a stale same-ID Live response cannot
  duplicate the loaded assistant. Existing private-context/purge resets and all
  wire/store versions are unchanged. See the
  [handoff evidence](../evidence/native_harness_m1.md#live-assistant-handoff-repair).

### What stays future and unaccepted

Writable persistent chats are **not** part of this acceptance:

- no chat-owned worktree or branch, and no writable profile;
- no tool-call execution and no in-chat approval contract, so no
  `allow_once`/`deny` decision surface and no 30-minute approval expiry;
- no `chat_decide` or `chat_export` operation;
- no concurrent writable chats sharing or reusing a workspace;
- no live certification of a persistent session against an installed Claude or
  Codex version.

The written worktree, tool, and approval design remains in the sections below
as a proposal. It needs its own accepted ADR before any implementation, and it
continues to depend on the ADR 036 live qualification gate.

### Compatibility boundary

- The chat operation requires Control v11. A client that predates it is
  refused with the established unsupported-version behavior, so the daemon and
  its clients upgrade together.
- Store schema 10 is forward-only. An older daemon binary does not understand a
  migrated project database, so the daemon and the desktop app upgrade
  together.
- The one-shot Ask path is unchanged: its Control minimum, its one-shot
  harness profiles, and its `--no-session-persistence` flag all stay as they
  were.
- Existing chat metadata and records keep their meanings. Older records are
  readable after the migration.
- The desktop's webview command list grew from 23 to 25 entries. The two
  additions are the chat consent and chat request commands. `tasks_snapshot`
  and `console_section` were previously stubs and now take the strict
  `{ project_key, query }` shape; the remaining existing commands keep their
  names and arguments.

### Verification status of this scope

The later [ADR 070](070_private_command_tool_records.md) adds inert completed
Codex command observations to existing private Tool records and the native
transcript presentation. It changes no profile, permission, approval decision,
Control operation or store schema. Writable/tool-effect qualification remains
separate from displaying a vendor observation.

The accepted desktop scope also includes bounded text/code imports through
`chat_pick_files`, bringing the fixed command list to 26. The renderer receives
basename/content snapshots from explicit native selection. Send encodes them
as `VIBEMUX_TEXT_ATTACHMENTS_V1` within the existing private prompt contract.
Control v11 and store schema 10 continue to carry an ordinary text prompt.
Limits, history parsing and privacy are specified in the
[native conversation guide](../native_harness_conversations.md#native-text-attachments).
Image and PDF inputs remain outside this implementation.

The subsequent ADR 058 desktop integration adds `chat_owner_request` as the
27th fixed command and a Workspaces view for lifetime owners. It uses the
existing Control v12 owner contract with separate workspace/title-storage
consent. Read-only chat turns retain the mapping defined in this ADR.

ADR 058's schema-12 increment adds a separate owner-backed attempt/record ledger
and private-writer facade. Its historical attempt outcomes are independent of
the lifetime Run's later state. This ADR's per-turn Task/Run contract remains
the read-only mapping. Owner runtime/control integration is the next increment.

The pre-attachment UI passed 53 tests, typechecking and a production build.
Native Windows fixtures verified repeated turns, exact resume, reopened private
history, authenticated Control paging, purge replay, and shutdown ownership.
Independent source reviews passed after the recorded repairs. The current
attachment candidate and subsequent live qualification are recorded below.

A bounded interaction check on the rebuilt Windows app verified navigation, theme changes and restoration, Settings focus return, the project picker, and bilingual draft retention across Chats and Independent Ask. Saved-history interaction with a running vendor, Chinese IME composition, DPI variants and Windows 10 remain unverified.

The exact commands, counts, original failures and corrections are retained in the [acceptance record](../evidence/native_harness_m1.md#2026-11-01-continuation-evidence). Live persistent-vendor qualification, Linux/WSL, hosted CI and complete dependency audits remain open.

The later attachment candidate passed 64 UI tests, typechecking and a production
build. Its native reader passed four tests including a Windows junction-ancestor
regression, and the real Tauri invoke contract passed two tests.

Six separately authorized live turns were attempted on 2026-11-01. Codex
0.166.0 completed two context-linked turns and a third after daemon/process
restart. A matching status notification arriving before the resume response
first exposed an ordering defect. The regression-tested repair preserves the
authoritative response's identity/profile checks. All three Codex turns had
completed canonical outcomes and complete retained output.

Claude Code 2.1.304 failed the initial temporary route, which omitted the host's
existing authentication/provider environment references. Corrected temporary
routes retained those references by name. Their initialization frames then
failed the plugin check while cwd, empty tools, empty MCP servers and `dontAsk`
matched. The plugin guard remains enforced. Claude follow-up/resume, live
cancellation, Windows 10 and native saved-history interaction remain unverified.

[ADR 058](058_writable_chat_ownership.md) records the lifetime writable-owner
mapping. Its schema-11 and Control-v12 provisioning foundation is implemented.
The separate attempt and approval contracts remain prerequisites for S6. The
[native guide](../native_harness_conversations.md#lifetime-owner-workspace-foundation)
documents the owner API, recovery and coordinated client upgrade.

### Claude customization isolation follow-up

The follow-up Claude profile `chat_claude_read_only_v2` adds the installed
2.1.304 CLI's `--safe-mode` flag. This disables user customizations, installed
plugins and hooks according to that executable's help, while retaining its
supported authentication methods. The effective initialization checks for
cwd, tools, MCP and permission mode remain enforced. The installed CLI reports
three bundled plugins even in safe mode. Nonempty plugin metadata is accepted
only for CLI `2.1.304`, with distinct names from `cc-plugin-agents-md`,
`cc-plugin-telemetry` and `cc-plugin-plugin-authoring`, and an exact corresponding
`name@builtin` source. Unknown plugins, other sources, duplicates, excess entries
and a different version are refused. Plugin paths supply no authority. Tool and
MCP lists must still be empty, and tool requests still receive denial.

The extra four-turn allowance used one diagnostic attempt and three successful
v2 turns. The successful conversation retained its synthetic code across a
follow-up and daemon/process restart. All three turns had matching canonical
IDs, completed outcomes, correlated terminals and complete retained output.
This is bounded native Windows evidence for the pinned version/profile.

The changed profile has a distinct fingerprint. Existing v1 Claude records stay
readable, and a v1 session cannot silently resume under v2. Create a new Claude
chat for the new profile. Codex retains its existing profile binding and the
one-turn Ask launch profile retains its existing arguments.

Default route setup now records supported Claude authentication/provider/model
environment names that are already present in the desktop process. Values
remain in that environment and are resolved by the native launch owner. Codex
receives its existing home-directory references. Existing operator route files
are preserved, so an older file may require an explicit environment-name update.

## Proposal history: source baseline and the decision gap

The following sections retain the original proposal context and record subsequent implementation decisions. It
describes intended behavior, not shipped behavior: where it discusses
mechanisms that the accepted scope implements differently — the operation
names, the version numbers, and the worktree, tool, and approval design — the
accepted scope above governs. The source baseline inspected for the original
proposal is `617bb1f`:

- `vibemux_client::ask::worker::spawn_ask` drives one dispatch through
  `DaemonSession` and authenticated `ControlClient` calls.
- `vibemux_harness::dispatch::ProtocolSession` owns one prompt and ends in
  `Phase::Finished`. Codex initializes, starts an ephemeral read-only thread,
  then starts one turn. Claude receives one user message under a tool-free,
  restricted profile with `--no-session-persistence`; `--bare` is intentionally
  absent under ADR 029's 2026-10-23 amendment.
- `vibemuxd::harness_dispatch::executor::execute` ends the contained process
  before `decide_outcome` establishes an attempt outcome. A successful
  correlated vendor terminal and a clean process exit are both required by
  the existing one-shot contract.
- The writer persists `harness_dispatches` and its canonical Task/Run pair.
  `TranscriptStore` retains raw output only in bounded daemon memory.
- `DshSessionService`, `vibemux_dsh::DshDriver`, and the pure DSH session
  machine demonstrate explicit long-lived ownership and repeated turns. They
  have no persisted canonical chat mapping and do not implement Claude/Codex
  persistence. Their in-memory records and one-session limit are separate
  contracts. This proposal does not copy their mutex-across-driver-await
  implementation into a new service.
- The current source constants are `CONTROL_PROTOCOL_VERSION =
  CONTROL_PROTOCOL_V10` and `STORE_SCHEMA_VERSION = 9`. Ask operations retain
  their Control v5 minimum.

[Desktop design section 6](../desktop_app_design.md#6-phase-2--harness-chat-shape)
previously proposed one lifetime Task/Run, Control v11, and store schema 10.
This proposal refines that shape: a chat is a separate session aggregate and
each admitted turn has its own canonical Task/Run. The cited version numbers
are historical reservations, not allocations. The implementation owner must
inspect the then-current protocol, migrations, and pending changes and allocate
new versions together when an accepted implementation starts. M1 changes none
of them. The later acceptance allocated Control v11 and store schema 10; see
[Accepted implementation scope](#accepted-implementation-scope-and-allocated-versions-2026-11-01).

## Proposed ownership and identity

The daemon owns an opaque `ChatId`, an immutable project identity, a harness
profile binding, and optional worktree ownership. Each user send has a distinct
`turn_id` and idempotent `request_id`; the single writer allocates the turn's
canonical `TaskId` and `RunId` and links all four IDs atomically. The browser
conversation ID and renderer selection never supply any of these authorities.

A chat has session states such as `opening`, `ready`, `busy`, `suspended`,
`recovery_required`, `closed`, and `archived`. It never becomes `Succeeded`
because one turn completed. Each turn independently records admission,
submission, execution, cancellation, terminal evidence, and uncertainty.
The complete transition table, including canonical Task/Run mappings, is a
required implementation artifact; it must not alter old dispatch records.

One turn at a time may own a chat. The writer atomically reserves the turn,
resource, remaining allowance, and outbox effect before the process owner sends
vendor input. It binds a submission key to project, chat, profile, generation,
request ID, and prompt fingerprint. An exact duplicate returns the existing
receipt; changed reuse conflicts. A duplicate lookup uses the original immutable binding
before checking admission for a new generation; resuming a session cannot make
an old key launchable again. An unknown submitted effect is retained and
cannot be sent again automatically. A client UUID is correlation, not consent.

Initial proposed limits are one active turn per chat and at most three live
vendor processes per project, with a bounded admission queue that refuses when
full. Read-only chats still share the existing project resource reservation;
opening a chat must not bypass an active Ask or `recovery_pending` reservation.
Concurrent writable chats need independent owned worktrees and explicit
resource-policy integration. Limits are validated daemon configuration, never
renderer arguments. Allocation and refusal must be tested atomically.

## Proposed versioned operations

These names describe a future fixed Control contract, not current commands.
Each operation carries the future schema version and bounded typed fields.
Every mutation also binds an operation ID to its arguments and expected
generation: exact duplicates return the retained receipt and changed reuse
conflicts, including close, archive, resume, decisions and purge.
Opaque chat handles resolve the project, profile, and vendor mapping inside
Rust. No operation accepts arbitrary paths, executable names, argv,
environments, RPC methods, or renderer-supplied vendor resume IDs.

The accepted implementation instead uses **one** Control operation carrying a
fixed request enum whose `operation` tag selects the effect. The table below
records the proposal's intent, not the shipped wire shape. The differences
that matter to a client are:

- the shipped names are the short enum tags listed in the accepted scope, not
  the `chat_*` operation names below;
- a private read is split into an explicit `title` read and a `content` read
  with a cursor, and a separate `live` read for the in-flight replacement
  snapshot; there is no combined `chat_poll` and no server-side wait;
- `chat_status` additionally reports whether a title exists without returning
  it, and a `submission` read returns the admitted turn for a request id;
- the original Chat request set has no `chat_decide` or `chat_export` tag.
  The later native export command in [ADR 060](060_native_conversation_export.md)
  composes the existing authenticated private reads without a new Control tag.

See [Accepted implementation scope](#accepted-implementation-scope-and-allocated-versions-2026-11-01)
for the shipped request set.

| Operation | Contract |
|---|---|
| `chat_open` | Explicit consent and profile admission; reserve ChatId and binding; idempotent open receipt. Opening alone sends no prompt. |
| `chat_send` | Validate bounded text and submission key, serialize with other turns, atomically admit one canonical Task/Run and reserve one send effect. Return admission, not inference completion. |
| `chat_status` | Content-free session/generation, current and recent turn IDs, canonical state/outcome, revision and transcript availability. |
| `chat_poll` | Explicit private content read by cursor; bounded page, earliest retained sequence, next cursor and gap/completeness markers. Optional wait is capped at 1 second. |
| `chat_cancel_turn` | Exact active turn and generation; request cancellation and report canonical result or unresolved state. Never cancel another turn through a stale selection. |
| `chat_close` | Stop accepting turns, reconcile active work under a deadline, close and reap the owned process. Preserve canonical metadata and permitted transcript. |
| `chat_archive` | Explicit quiescent transition; refuse unsettled effects. Preserve records and worktree. Does not delete files or imply turn success. |
| `chat_resume` | Explicit request for one known ChatId; revalidate profile, project/worktree, vendor mapping, terminal predecessor ownership and current generation before launch. |
| `chat_decide` | A separately gated future approval contract described below; unavailable for the initial read-only session profile. |
| `chat_export` / `chat_purge` | Separate explicit private-content operations with bounded output and retention semantics. Never remove canonical audit events or an owned worktree. |

A content-free bounded list/read operation is also required so reconnecting
clients can discover authorized ChatIds without scanning private storage.
Status reads and renderer startup never acknowledge recovery, open a session,
resume, or replay a turn. Clients older than the new operation minimum fail
with the established unsupported-version behavior. Old operations and records
keep their present meanings.

## Vendor adapters and lifetime

### Codex

Use the existing Rust protocol boundary. The proposed persistent adapter
initializes a contained app-server connection, binds the exact returned
`thread.id`, and maps each `turn/start` response and notification to its
expected `turn.id`. On process restart, `thread/resume` must request that exact
stored ID and validate the returned identity and effective profile. A resume
response is not proof that an uncertain prior turn never executed.

Official documentation distinguishes thread, turn, and item; documents
`thread/start`, `thread/resume`, `turn/start`, and `turn/interrupt`; and reports
`completed`, `failed`, or `interrupted` in `turn/completed`. These are the
upstream protocol facts used here, not VibeMux qualification evidence.
[Codex App Server](https://learn.chatgpt.com/docs/app-server)

Do not use Codex exec as a surrogate persistent protocol. Changing the current
`ephemeral: true` flag is insufficient: the new adapter must reset its per-turn
state after a validated terminal, maintain connection ownership, drain events
between turns, and fence stale notifications. Model/provider/sandbox overrides
remain operator-owned and are revalidated rather than copied from UI history.

### Claude Code

Use a dedicated stream-json session owner with stdin open across turns. Bind
the exact vendor `session_id` from validated root-session evidence; exclude
nested/subagent results. Resume a stopped process only with that stored ID
and compatible project/profile. Never use bare `--continue`, latest-session
selection, display names, or current-directory coincidence as session identity.

The official SDK documentation distinguishes selecting the most recent session
from resuming an exact ID and recommends exact-ID handling when several
sessions coexist. Streaming input is a persistent input mode. The CLI documents
`--resume` and states that `--no-session-persistence` prevents later resume.
These observations support a Rust adapter design; they do not require adding
an SDK dependency.
[Session handling](https://code.claude.com/docs/en/agent-sdk/sessions),
[streaming input](https://code.claude.com/docs/en/agent-sdk/streaming-vs-single-mode),
[CLI reference](https://code.claude.com/docs/en/cli-reference)

A new persistent profile deliberately chooses vendor persistence and must be
qualified separately. The current tool-free one-shot profile remains intact.
Removing its persistence flag or keeping its process open alone is not a
session implementation. Exact root-message/turn correlation, interruption,
error result handling and repeated messages are qualification requirements.

### Turn completion and process shutdown

The persistent driver separates a successful turn from process lifetime.
A correlated terminal record may finish its turn while a healthy process
returns to `ready`; normal turn completion must not invoke the one-shot
`NativeProcess::finish` path. This requires a distinct execution mode and
outcome reducer, with fixtures that prove the next turn can execute.
The existing one-shot `decide_outcome` rule remains unchanged.

EOF, broken stdin, uncorrelated terminals, a nonzero exit, forced termination,
or a lost process during an active turn produce interrupted/failed/unverified
outcomes according to evidence. An already committed completed turn is not
rewritten merely because the idle process later dies. Its session becomes
suspended or recovery-required, with a separate process-lifecycle event.
A cancellation request is not cancellation proof. At its deadline the owner
ends only its contained tree and records whether the active turn was reconciled.

Idle eviction is proposed at ten minutes: stop the exact owner, drain and reap,
then retain a resumable mapping if validated vendor persistence exists. No
future send may race an unresolved close or automatically reuse an unknown
process. Every worker has a cancellation path, bounded queue, supervisor and
join; registry locks are released before I/O, awaits and joins.

## Fencing, recovery and replay

Store a monotonically increasing generation and an opaque owner fence per chat.
A process generation can claim only a quiescent predecessor with verified
terminal cleanup. All input, approval, record append and terminal commit paths
check the fence. A second daemon/process cannot resume the same mapping while
the first is alive or of unknown ownership. Reused operating-system PIDs do not
prove identity. The daemon's single-writer lock remains authoritative.

At restart, admitted-but-unsent work and submitted-or-uncertain work are
separate durable states. The former may be cancelled without vendor input;
the latter enters `recovery_required` and keeps reservations. Query exact vendor
identity where the installed protocol supports it, compare the last committed
turn/checkpoint, and expose any gap. If absence or delivery cannot be proved,
require an explicit new-send action with a new request ID; never replay the
old text automatically. Resume does not refund spent allowances or reset
attempt budgets.

Record order uses daemon sequences. Persist checkpoints only after the writer
commits the corresponding record range. Duplicate records are rejected by
chat/generation/turn/sequence binding; skipped or evicted ranges are reported.
Bounded poll pages cannot claim complete delivery while a gap remains. A crash
mid-block preserves committed prefixes and marks the missing tail; old messages
are never silently presented as a complete recovered transcript.

## Four separate persistence policies

| Layer | Owner and lifetime |
|---|---|
| Process | Daemon supervisor and contained vendor tree; ends on close, idle eviction, failure or daemon shutdown. |
| Vendor session | Exact Codex thread or Claude session in vendor-owned storage under a qualified persistent profile; may outlive the process. |
| Canonical metadata | Single daemon writer: ChatId, generation, profile/worktree binding, turn/request/Task/Run links, receipts, audit and retention tombstones. |
| Private transcript | Single daemon writer, with explicit content policy and access checks; never browser history or ordinary event payloads. |

Proposed private tables are `chat_sessions`, `chat_turns`, `chat_records`, and
`chat_approvals`/outbox metadata in the existing writer-owned store. Schema
migration is forward-only, atomic, backed up with matching private content,
and tested with historical fixtures. No plugin, renderer, or second SQLite
connection writes them. Existing ACL/runtime protections apply; transcript
storage requires verified current-user permissions and link/reparse-point
refusal. Logs and audit events retain only IDs, counts, fixed codes and
required integrity metadata. Titles, tool arguments/results, prompts, and
replies are private content, including when generated by a model.

The accepted scope implements the session, turn, and record metadata and the
protected private bodies. The approval and outbox metadata remain future work
with the writable profile.

Proposed bounds retain the desktop design's 256 KiB display-record and 32 MiB
per-chat limits, but distinguish display truncation from protocol integrity:
never truncate a wire frame, approval, or tool-call JSON and then execute it.
Reject oversized semantic inputs before allocation/execution; display-only
text truncation has an explicit flag and original byte count. A full chat
refuses new turns instead of dropping idempotency or uncertain-ownership rows.
Proposed defaults are 128 chats and 1 GiB of private content per project,
with archived content eligible for expiry after 30 days. These bounds are
validated configuration and require acceptance in S0. Active, open, and
uncertain chats are never silently expired; capacity refusal precedes admission.
Content expiry does not remove canonical metadata or submission tombstones.
The durable deduplication budget is finite (proposed 65,536 submissions per
project); at exhaustion new submissions refuse until a separately defined,
explicit retention migration preserves the supported replay contract.

Export is an explicit private-content action with scope/size checks. Purge
removes retained local content and private titles while preserving content-free
canonical receipts and deletion audit. It cannot claim to erase vendor-side
session files, provider retention, backups, or already exported copies.
Vendor deletion requires its own supported explicit operation and evidence.
Worktree removal remains a distinct checked cleanup plan.

## Project, worktree and profile binding

A chat binds the canonical project identity from `DaemonPaths`, the operator
route/profile revision and vendor version, and, for writing, an independently
owned worktree and branch. Alias resolution and identity changes fail closed.
Neither frontend path hashing nor a vendor's returned cwd overrides that
binding. Existing read-only profile names do not imply filesystem isolation.

Writable mapping is an explicit unresolved design gate. The initial persistent
read-only profile can keep the per-turn canonical Task/Run mapping above.
A later writable profile cannot silently reuse one chat worktree across those
Run IDs: the repository requires an independent worktree/branch per writable
Run, while vendor resume needs a stable workspace binding. Before implementing
writable sessions, a separate accepted ADR must choose either a lifetime
writable owner Run with correlated turn attempts, or an explicit successor
Run/worktree plus context handoff. The latter must not be presented as a
transparent resume of the same workspace. Neither alternative is enabled or
accepted by this proposal. This gate precedes S6.

Writable chats additionally remain subject to ADR 036: explicit effective grants,
exact worktree ownership, per-action approvals, installed-version live
qualification and independent acceptance. Their resource reservations must
prevent two writable Runs from sharing one worktree. Repository instruction
files, model text, browser attachments, a successful handshake or a consent
UUID cannot authorize a broader profile.

## Future scoped approvals

The initial persistent profile declines unsupported requests. A later writable
profile may expose only an allowlisted approval kind with exact action details,
request identity, ChatId, turn, generation, profile and digest binding. The
writer reserves a single decision slot before presenting private details.
Only authenticated local control clients can answer `allow_once` or `deny`.
No allow-for-chat/session decision is proposed.

A proposed 30-minute ceiling expires to denial, capped by the turn deadline.
Duplicate or changed decisions, stale generations, completed/cancelled turns,
unknown kinds, and process restarts refuse. An affirmative response is an
owned outbox effect: reserve, claim once, send, reconcile; unknown delivery
cannot be replayed. Audit contains the challenge/decision IDs and outcome,
not command text or content. Restart invalidates outstanding approvals before
any new owner can run. Vendor approval messages are data to validate, not
commands for the renderer to execute.

Official Codex approval requests carry thread/turn/item correlation; the Claude
SDK exposes a permission decision boundary to the application. These are
upstream surfaces to qualify against installed versions, not proof of complete
write denial or permission coverage.
[Codex approvals](https://learn.chatgpt.com/docs/app-server#approvals),
[Claude approvals](https://code.claude.com/docs/en/agent-sdk/user-input)

Qualification must exercise denial of every supported edit and command,
malformed/unknown requests, timeouts, stale decisions, network/MCP requests,
and repository attempts to widen grants. Until this evidence exists the UI
shows the exact limited profile and writable controls remain disabled.

## Explicit context changes

Browser imports are an explicit bounded text import with provenance, preview
and consent. Importing history does not resume a vendor session or transfer
execution authority. Attachments require a separate input/artifact contract;
a browser filename is never a filesystem path. Model/profile changes require
an explicit compatible transition or a new chat with a visible handoff record.
Cross-harness handoff creates a new destination session from operator-approved
bounded context; it never claims shared vendor memory. Prior canonical turns
and provenance are retained on retry, edit/resend, fork, or import.

## Qualification evidence and staged acceptance

The following checks ran on 2026-10-31 on native Windows 11 Pro Education,
version `10.0.26300`, build `26300`, x64. They are **version/help/schema evidence
only**, not native desktop interaction, runtime initialization or inference.

| Installed component | Observation |
|---|---|
| Native Codex CLI `0.166.0` | `--version`, `app-server --help`, and `app-server generate-json-schema --help` exited 0. Schema generation into a temporary directory exited 0. Stable schemas require `threadId` for resume, `input`/`threadId` for turn start, `threadId`/`turn` for completion, and `threadId`/`turnId` for interrupt. Approval schema carries item/thread/turn correlation. The temporary schema directory was removed after inspection. |
| Native Claude Code `2.1.304` | `--version` and `--help` exited 0; help lists stream-json input/output, exact resume/session-ID flags, restricted permissions and disabled persistence. No authenticated runtime or permission behavior was exercised. |
| Agent SDK | Documentation read; no SDK installed, executed or added as a dependency. |

Official pages above were fetched on 2026-10-31. Live documentation can describe
features beyond these installed versions. Pin the tested executable version,
profile and protocol fixtures in each qualification record; upgrades require
requalification of affected paths. No account/credential data was inspected.

| Stage | Deliverable | Decisive acceptance |
|---|---|---|
| S0 | Accepted ADR, current version allocation, privacy/retention configuration, closed operation schemas | Explicit review of transitions, resource accounting, compatibility and full threat model; all numeric versions rechecked. |
| S1 | Pure reusable Codex and Claude session machines | Two correlated follow-up turns on the same exact vendor identity; no process-exit completion inference; reject wrong IDs, nested results, malformed frames and stale terminals; bound split UTF-8/JSON and record replay. |
| S2 | Writer-owned chat/turn mapping, generation fences and idempotent outbox | Concurrent duplicate sends launch once; changed reuse conflicts; crash at reserve/claim/write/receipt boundaries never replays uncertain work; historical schema migration and reopen tests. |
| S3 | Owned persistent driver and Control client/CLI | Temporary-daemon integration with contained fixtures: repeated turns, busy refusal, cancel/completion race, hung input/output, bounded close, idle eviction, two concurrent chats and stale-owner fencing. Existing one-shot Ask behavior stays unchanged. |
| S4 | Private storage and desktop session UI | Reload and daemon restart expose exact retained records and gaps; resume binds exact session; retention/full-budget/export/purge and stale approval tests; no raw content in status/events/logs. |
| S5 | Authorized native read-only live qualification | One bounded two-turn context check and exact resume after process restart for each pinned Claude/Codex version; correlate canonical and vendor IDs; verify cancellation and output availability. Windows 10 and 11 evidence recorded separately. |
| S6 | Separately authorized writable profile | Owned worktrees; allow-once/deny/expiry/restart invalidation; unknown permission refusal; file/command/network/MCP negative controls; parallel chat isolation and independent review. No writable label before qualification. |

Failure in a stage blocks dependent stages and remains recorded. A fixture,
kept-open process, browser array, removal of flags, prompt-history replay or
standalone `vibemux_harness_native` run cannot satisfy S5 or S6.

The accepted read-only scope advances S0 through part of S4: the versions are
allocated, the session/turn/record mapping and generation fences exist, the
owned driver runs repeated turns against contained fixtures, and private
bodies are stored behind explicit reads. The authenticated Control integration
now passes, including reassembly of escaped-Unicode private history and
preservation of the original canonical turn across a duplicate send and a
purge replay. S5 remains partial. The later bounded live checks above verified
follow-up and restart-resume for both pinned vendors. Live cancellation,
Windows 10 and native saved-history interaction remain open. The original
one-turn smoke failure remains recorded separately. S6 stays out of scope. The
desktop session UI of S4 is implemented and locally tested, while its Native saved-history interaction against a running vendor remains unverified.

## Alternatives and consequences

- Replaying browser history into independent Asks is retained only as an
  explicitly labeled import/context mechanism, not persistent vendor memory.
- One lifetime Task with per-turn Runs was considered. Existing dispatch owns
  one Task/Run pair per attempt; separate ChatId links preserve that boundary
  without giving a completed turn authority to finish the whole conversation.
- Replacing the Rust protocol layer with an SDK adds a second runtime owner
  without solving canonical mapping, writer ownership or uncertain effects.
  Retain protocol code and use official SDK documentation as evidence.
- Reusing DSH's service wholesale would inherit a different correlation,
  persistence and permission contract. Reuse its established separation of
  pure state machine, driver and daemon ownership; do not substitute its profile.
- Exposing an HTTP bridge or renderer RPC proxy would broaden the local control
  boundary. Keep fixed typed commands and authenticated Control transport.

The design adds explicit durable session metadata and a private-content policy,
which require migrations and lifecycle tests beyond M1. The accepted read-only
scope above adds that durable session metadata and private-content policy for
a read-only Claude or Codex chat, and the desktop chat surface is implemented
and locally tested. The writable profile, the tool and approval contract,
image/PDF inputs, and the remaining live qualification stages remain open.
Native saved-history interaction against a running vendor remains unverified.
