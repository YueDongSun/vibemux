# DSH session protocol

> **Document role:** the wire contract VibeMux implements when it drives a
> DeepSeek Harness (DSH) SDK runtime. **Scope:** the observed DSH wire shapes,
> the operator configuration, the driver's bounds and failure codes, and the
> terminal surface. **Out of scope:** milestone evidence
> ([PROGRESS.md](../PROGRESS.md)); engineering rules ([AGENTS.md](../AGENTS.md));
> the decision record ([ADR 032](adr/032_dsh_harness_adapter.md)).

## 1. What this binds to

DSH publishes one first-party machine surface for out-of-process clients:
newline-delimited JSON-RPC 2.0 over stdio, defined by
`@deepseek-ai/dsh-sdk-protocol` and served by `@deepseek-ai/dsh-sdk-jsonrpc-server`.

| Fact | Value |
|---|---|
| Client→server requests | `initialize`, `session/prompt`, `shutdown` |
| Server→client notifications | `session.event`, `session.status`, `subagent.started`, `subagent.finished` |
| Wire-stable server identity | `serverInfo.name == "deepseek-harness-sdk-runtime"` |
| `serverInfo.version` | the literal `0.0.1`; treat as opaque |
| Framing | one JSON object per `\n`-terminated line |
| Frame dispatch | `id` + `method` = request; `id` alone = response; `method` alone = notification |
| Session identity | client-chosen; an unknown id lazily creates the agent+session pair |
| Cancellation | **none**; the protocol publishes no cancel or session-close method |
| stdout | protocol frames only; diagnostics belong on stderr |
| Boot command | `dsh --profile sdk` (the profile auto-initializes on first use) |

Version bound: these shapes were read from `0.1.2-rc.1` on the reference host.
The protocol carries no version negotiation and makes no compatibility
promise, so a later release must be re-verified before this adapter is claimed
to work against it.

**Operator-visible side effect.** DSH auto-initializes a templated profile on
first use and rewrites that profile's `cordis.yml` on every boot. Booting
`dsh --profile sdk` for the first time therefore **writes to the operator's DSH
home** (`$DSH_HOME`, by default `%USERPROFILE%\.dsh`). VibeMux never creates
that profile itself and never touches the DSH home; it starts the runtime and
lets DSH provision what it needs. Run `dsh --profile sdk --help` once before
configuring a route so the write happens under the operator's control.

## 2. The launcher pair

On Windows the `dsh` command is an npm shim (`dsh.ps1` / `dsh.cmd`) that runs
`node.exe <npm root>/@deepseek-ai/dsh/lib/bin.js`. ADR 029 forbids route
executables that route argv through a command interpreter, and a `.ps1` or
`.cmd` path is rejected for every protocol.

VibeMux therefore launches the pair directly, with no shell and no shim:

```text
executable  : <node image>            (a native image whose stem is `node`)
argv        : <entry script> --profile <profile>
environment : SYSTEMROOT, WINDIR, TEMP, TMP, plus the configured names
stdin       : the JSON-RPC frames
```

The entry script is verified before anything starts: it must be an absolute
`.js` file whose owning `package.json` declares
`"name": "@deepseek-ai/dsh"` and maps its `dsh` bin entry to exactly that
script. Nothing inside the project root is accepted as either the image or the
script, so a repository cannot ship the code a session runs.

## 3. Operator configuration

`.vibemux/dsh_session.json`, schema 1. Absent by default: an unconfigured
project is not an error, and DSH stays off.

```json
{
  "schema_version": 1,
  "executable": "C:\\Users\\<user>\\AppData\\Roaming\\npm\\node.exe",
  "entry_script": "C:\\Users\\<user>\\AppData\\Roaming\\npm\\node_modules\\@deepseek-ai\\dsh\\lib\\bin.js",
  "profile": "sdk",
  "provider": "deepseek-official",
  "model": "deepseek-v4-flash",
  "environment_names": ["DEEPSEEK_API_KEY"],
  "limits": {
    "frame_bytes": 524288,
    "record_bytes": 8388608,
    "record_count": 8192,
    "initialize_deadline_ms": 60000,
    "turn_deadline_ms": 900000,
    "shutdown_grace_ms": 5000
  }
}
```

| Field | Required | Notes |
|---|---|---|
| `schema_version` | yes | must be `1` |
| `executable` | yes | absolute native image; file stem must be `node` |
| `entry_script` | yes | absolute `.js` file owned by the `@deepseek-ai/dsh` package |
| `profile` | no | defaults to `sdk`; `[a-z0-9][a-z0-9_-]*`, at most 64 bytes |
| `provider` | yes | `initialize` requires it; same alphabet as `model` |
| `model` | yes | `initialize` requires it |
| `reasoning_effort` | no | adapter-owned identifier when the route supports one |
| `max_tokens` | no | positive, at most 200000 |
| `environment_names` | no | uppercase names; `VIBEMUX_*` is reserved and refused |
| `limits` | no | omitted fields take the defaults below |

Unknown fields are refused, as are a missing provider or model, a `.cmd` /
`.ps1` / `.bat` / `.vbs` launcher, a relative entry script, and either path
resolving inside the project root.

### Limits

| Field | Default | Accepted range |
|---|---|---|
| `frame_bytes` | 524288 | 4096 – 4194304 |
| `record_bytes` | 8388608 | 4096 – 67108864 |
| `record_count` | 8192 | 16 – 65536 |
| `initialize_deadline_ms` | 60000 | 1000 – 300000 |
| `turn_deadline_ms` | 900000 | 1000 – 3600000 |
| `shutdown_grace_ms` | 5000 | 100 – 30000 |

`initialize_deadline_ms` is much larger than a native CLI handshake because a
cold Node boot plus profile resolution dominates startup.

## 4. Session lifecycle

```text
Starting ──initialize──▶ Initializing ──serverInfo ok──▶ Ready
   Ready ──session/prompt──▶ Prompting ──turn/end──▶ Ready
   Ready ──shutdown──▶ Closing ──response or clean exit──▶ Closed
   any ──protocol violation, deadline, or unexpected exit──▶ Failed
```

Rules the driver enforces:

1. **One `initialize`, first.** The server gates its plugin tree on it and
   rejects `session/prompt` with `SDK server is not initialized` otherwise. A
   second `initialize` is silently accepted by the server and would re-target
   future sessions, so VibeMux never sends one.
2. **One turn at a time.** The wire carries no correlation id between a prompt
   and a turn, so a second `session/prompt` while one is running is refused
   with `dsh_not_ready` rather than queued. This is what makes turn
   attribution exact.
3. **Responses must correlate.** A response whose `id` is not the pending
   request, or that carries `error`, or whose `result` is not an object, ends
   the session with a fixed code instead of being retried.
4. **Notifications are filtered by session.** The stream is runtime-wide and
   unfiltered, so `session.event` and `session.status` are matched against the
   session's own id and everything else is counted and dropped.
   `subagent.*` payloads carry `parentSessionId`/`childSessionId` and no
   `sessionId`, so they are never correlated through the session-id path.
5. **Unknown is tolerated, not guessed.** An unrecognized notification method,
   an unrecognized `session.event` type, and an unrecognized
   `turn/end.reason.kind` all leave the session running. The last one resolves
   the turn as `unverified`, which is never success.
6. **stdout is the protocol.** A line that is not JSON ends the session: a
   stray logger in the composed tree would otherwise be indistinguishable from
   protocol traffic.

## 5. Completion authority

`session.status: idle` is rendered as a **diagnostic**. AGENTS.md §2 invariant
5 forbids treating idle state as proof of completion, so it never ends a turn
and never increments a completion counter.

Only a correlated `session.event` whose `event.type` is `turn/end` carries
terminal evidence, through this table:

| `reason.kind` | Outcome |
|---|---|
| `completed` | `completed` |
| `aborted` | `cancelled` |
| `blocked` | `failed` |
| `error` | `failed` |
| `max-tokens` | `failed` |
| `interrupted` | `failed` |
| unrecognized or absent | `unverified` |

Note that the DSH `sdk` profile defaults `DSH_MAX_TOKENS_AS_SUCCESS` to `true`,
so a deployment may fold a token-limited turn into `completed` before it
reaches VibeMux. VibeMux's own table stays conservative either way.

## 6. Cancellation and teardown

The protocol publishes no cancel and no session-close method, and the runtime's
own documentation says a client abandons a turn by closing the process. So:

- **Cancel** marks the in-flight turn `cancelled` and terminates the contained
  process tree.
- **Close** writes `shutdown`, waits `shutdown_grace_ms`, and then terminates
  and reaps. A runtime that was asked to close and ignored the request is
  `dsh_driver_shutdown_timeout`; a runtime that had already failed is torn down
  without a spurious error.

The runtime is contained with `vibemux_platform::ProcessTree` from its first
instruction, so descendants cannot outlive the session. Containment governs
lifetime only: it is **not** a sandbox, and the runtime's own profile decides
what it may read and write.

## 7. Failure codes

| Code | Meaning |
|---|---|
| `dsh_driver_unconfigured` | the configuration file is absent or unreadable |
| `dsh_driver_config_invalid` | the document is malformed or out of range |
| `dsh_driver_executable_untrusted` | not a canonical `node` image outside the project root |
| `dsh_driver_entry_script_untrusted` | not a DSH-owned `.js` script outside the project root |
| `dsh_driver_spawn_failed` | the process could not be started |
| `dsh_driver_stdin_unavailable` | the child exposes no stdin/stdout pipe |
| `dsh_driver_write_failed` | writing a frame failed |
| `dsh_driver_transport_closed` | stdout reached EOF before the session finished |
| `dsh_driver_frame_too_large` | a record exceeded `frame_bytes` or the record buffer bound |
| `dsh_driver_invalid_utf8` | a record was not valid UTF-8 |
| `dsh_driver_initialize_timeout` | the handshake exceeded `initialize_deadline_ms` |
| `dsh_driver_turn_timeout` | a turn exceeded `turn_deadline_ms` |
| `dsh_driver_shutdown_timeout` | the runtime ignored a cooperative `shutdown` |
| `dsh_driver_invalid_state` | the session refused the operation in its current phase |

Session-level codes carried by the state machine are listed in
`vibemux_harness::dsh::DshError` (all `dsh_*`).

## 8. Bounded records

Runtime records are kept byte-exact in a monotonically sequenced ring.

- A page returns at most 512 records and 384 KiB.
- A cursor ahead of the head is `dsh_invalid_cursor` rather than silently
  clamped.
- When a cursor predates the retained window, the page sets `truncated`, so a
  consumer reports the gap instead of believing it saw everything.
- A single record larger than `record_bytes` is refused outright; a record is
  never truncated in place.
- Otherwise the oldest records are evicted, and `records_dropped` counts them.

Raw runtime content leaves the driver only through the poll operation. Session
summaries carry a prompt's **size and SHA-256**, never its text, and never a
path.

## 9. Surfaces

Three surfaces reach the same substrate.

### Terminal UI

`vibemux_dsh_tui` drives a session through `vibemux_dsh` directly, so an
operator needs no daemon.

```powershell
cargo run -p vibemux_frontend --bin vibemux_dsh_tui -- --project-root .
```

| Flag | Meaning |
|---|---|
| `--project-root <path>` | project whose `.vibemux/dsh_session.json` is loaded (default: the current directory) |
| `--config <path>` | override the configuration path |
| `--theme <name>` | start with any `tui::Theme` (`VIBEMUX_FRONTEND_THEME` applies otherwise) |
| `--print-config` | print the content-free configuration summary and exit |
| `--json` | with `--print-config`, print JSON |

Keys: **Enter** sends the prompt, **Esc** cancels the running turn (or clears
the input), **Ctrl-C** / **Ctrl-Q** quit, **Up**/**Down**/**PageUp**/**PageDown**
scroll the transcript. Every printable key goes to the input, so no shortcut
can steal a character.

### Control IPC v7

Six operations. Every argument is a strict JSON document; a malformed one is
`control_invalid_request`, and a service refusal passes its `dsh_session_*` or
`dsh_driver_*` code through.

| Operation | Argument | Answer |
|---|---|---|
| `dsh_session_open` | `{"session_id": "<id>"}` (optional) | status |
| `dsh_session_prompt` | `{"prompt": "<text>"}` | status |
| `dsh_session_poll` | `{"cursor": <n>}` | page + status |
| `dsh_session_status` | none | status |
| `dsh_session_cancel` | none | status |
| `dsh_session_close` | none | status |

Only `dsh_session_poll` returns runtime content. Every status view is
allowlisted and carries no path, no digest, and no prompt text. A pre-v7 client
refuses a v7 descriptor, so `vibemuxctl` and `vibemuxd` upgrade together.

A poll page and its `status` come from one driver snapshot. The runtime reader
appends each record and advances the session under the lock that the poll
reads, so a page's status never counts a record the page cannot reach. When a
page has `more: false`, its `next` equals `status.records`. A consumer that has
read every page up to that one therefore holds the `TurnEnd` record of every
turn that status counts as settled. A page with `more: true` can still report a
settled turn whose `TurnEnd` is on a later page. That happens, for example,
when the page was trimmed to fit the control frame. A consumer therefore stops
at the first page that both reports the turn settled and has `more: false`.

Service codes: `dsh_session_unconfigured` (no configuration; DSH is off),
`dsh_session_config_invalid` (malformed or untrusted), `dsh_session_busy` (a
session is already live), `dsh_session_not_found` (configured, but no live
session), `dsh_session_refused` (the session refused the request, e.g. a
handshake timeout), `dsh_session_prompt_invalid`, `dsh_session_record_too_large`
(one record cannot fit the control frame), `dsh_session_settings_invalid`.

### CLI

```powershell
vibemuxctl dsh open   [--session-id <id>] [--project-root <path>]
vibemuxctl dsh prompt --prompt-file <path|-> [--project-root <path>]
vibemuxctl dsh poll   [--after-sequence <n>] [--project-root <path>]
vibemuxctl dsh status [--project-root <path>]
vibemuxctl dsh cancel [--project-root <path>]
vibemuxctl dsh close  [--project-root <path>]
```

A prompt comes from a file or stdin, never from argv, so it stays out of
process listings and shell history. `dsh prompt`, `status`, `cancel`, and
`close` print a content-free status projection; `dsh poll` prints byte-exact
records and the cursor to resume from. It follows pages until one has nothing
more, and the session status it prints comes from that last page.

The terminal surface writes no canonical state. The daemon path also writes no
canonical state today: a DSH session has no persisted record, so a daemon
restart loses it and its records (ADR 032 open decision 1).

## 10. Not implemented

- **Canonical session record.** A session is not persisted; nothing about it
  reaches Task/Run state or the event log.
- **Resume.** A session ends with its process; DSH's own durable session logs
  are not read.
- **Multi-session.** One session per project, enforced by `dsh_session_busy`.
- **Images.** `SdkEncodedImageBlock` is not sent; prompts are text-only.
- **Approval bridging.** The runtime's approval events are shown, never
  answered.
- **`sdk-minimal`.** Its composition is `danger-full-access` by default and it
  ships no credential store, so it is not a supported route here.
- **Live validation.** No prompt has been sent to a real DSH runtime. The
  adapter is verified against the wire shapes read from
  `@deepseek-ai/dsh-sdk-protocol` 0.1.2-rc.1 and against
  `vibemux_dsh_fixture`.

## 11. References

- Decision record: [ADR 032](adr/032_dsh_harness_adapter.md).
- Harness registration and dispatch: [ADR 029](adr/029_daemon_harness_dispatch.md).
- Architecture: [docs/architecture.md](architecture.md).
- DSH SDK protocol (vendor): `@deepseek-ai/dsh-sdk-protocol` package README and
  `lib/types/types.d.ts`.
- DSH session vocabulary (vendor): `@deepseek-ai/dsh-session` package README and
  `lib/types/known-event-types.js`.
