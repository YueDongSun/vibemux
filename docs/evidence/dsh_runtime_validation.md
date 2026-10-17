# DSH runtime validation against `dsh` 0.2.0-rc.2 (ADR 032)

Date: 2026-10-06. Platform: native Windows 11 Pro 10.0.26200, Rust
1.85/MSVC debug builds.

## Scope

The machine's npm-global `dsh` CLI was upgraded from `0.1.2-rc.1` to
`0.2.0-rc.2`. `docs/dsh_session_protocol.md` §1 records that its wire shapes
were read from `0.1.2-rc.1` and that "a later release must be re-verified
before this adapter is claimed to work against it". This report is that
re-verification.

Two probes were run against the real vendor runtime:

- **Part A — wire contract.** One newline-delimited JSON-RPC `initialize`
  request over stdio, then `shutdown`, driven outside VibeMux.
- **Part B — VibeMux adapter.** `vibemux_dsh_tui` in a real ConPTY, and the
  `vibemuxctl dsh` Control v7 path through `vibemuxd`.

**No prompt was sent on any path.** `session/prompt` was never written, so no
model inference ran and no provider spend was incurred. `dsh prompt` was not
run either. Everything below is initialize-only plus a cooperative `shutdown`.

## Environment

| Fact | Value |
|---|---|
| OS | Microsoft Windows NT 10.0.26200.0 (Windows 11 Pro) |
| `node --version` | `v24.18.0` |
| `npm --version` | `11.16.0` |
| `npm prefix -g` | `C:\Users\<user>\AppData\Roaming\npm` |
| `dsh --version` | `0.2.0-rc.2` |
| Resolved `node` image | `C:\nvm4w\nodejs\node.exe` (v24.18.0) |
| Second `node` on PATH | `C:\Program Files\nodejs\node.exe` (v24.19.0) |
| `@deepseek-ai/dsh` entry script | `C:\Users\<user>\AppData\Roaming\npm\node_modules\@deepseek-ai\dsh\lib\bin.js` |
| `DSH_HOME` | `C:\Users\<user>\.dsh` (set in the shell environment) |
| `DEEPSEEK_API_KEY` | **absent** from this shell's environment |

```powershell
node --version           # v24.18.0
npm --version            # 11.16.0
npm prefix -g            # C:\Users\<user>\AppData\Roaming\npm
dsh --version            # 0.2.0-rc.2
where.exe node           # C:\nvm4w\nodejs\node.exe ; C:\Program Files\nodejs\node.exe
where.exe dsh            # ...\npm\dsh ; ...\npm\dsh.cmd
```

The entry script was confirmed to be the package's own bin entry:

```powershell
(Get-Content '...\node_modules\@deepseek-ai\dsh\package.json' -Raw | ConvertFrom-Json) |
  Select-Object name, version, @{n='bin.dsh';e={$_.bin.dsh}}
# name    : @deepseek-ai/dsh
# version : 0.2.0-rc.2
# bin.dsh : lib/bin.js
```

`@deepseek-ai/dsh` is the only package installed under the global
`@deepseek-ai` scope.

### Discrepancy: `<npm prefix>\node.exe` does not exist

`docs/dsh_session_protocol.md` §2 and §3 show the node image as
`<npm prefix>\node.exe` (`C:\Users\<user>\AppData\Roaming\npm\node.exe`). On
this host that file does not exist:

```powershell
Test-Path 'C:\Users\<user>\AppData\Roaming\npm\node.exe'   # False
```

The `dsh.cmd` shim explains why: it prefers `%dp0%\node.exe` and otherwise
falls back to `node` resolved from `PATH`. This machine runs Node through
**nvm4w**, so the real image is `C:\nvm4w\nodejs\node.exe`. The docs' example
path is therefore an artifact of a Node-installer-style global prefix, not a
guarantee. `.vibemux/dsh_session.json` names the resolved image instead.

### Operator configuration used

`.vibemux/dsh_session.json` (schema 1, gitignored local runtime state):

```json
{
  "schema_version": 1,
  "executable": "C:\\nvm4w\\nodejs\\node.exe",
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

Both paths are outside `I:\VibeMux`, as the trust rule requires. The
documented side effect was observed and is expected: booting `--profile sdk`
rewrote `C:\Users\<user>\.dsh\profiles\sdk\cordis.yml` (timestamp moved to the
probe window). No repository file was modified by this validation.

## Part A — wire-contract probe

Driver script: `%TEMP%\dsh_probe_initialize.js` (spawns
`node <entry_script> --profile sdk` with stdin/stdout pipes, writes one
`initialize` frame, reads framed lines until the matching `id`, then writes
`shutdown`). Bounds: 90 s initialize deadline, 4 MiB stdout cap.

Frame 1 written to the runtime's stdin (raw single line):

```json
{"jsonrpc":"2.0","id":"vibemux-verify-init-1","method":"initialize","params":{"cwd":"I:\\VibeMux","provider":"deepseek-official","model":"deepseek-v4-flash"}}
```

The `params` set (`cwd`, `provider`, `model`) is exactly what
`DshSessionMachine::start` emits (`crates/vibemux_harness/src/dsh/session.rs`).

Response received verbatim (single line, 128 bytes):

```json
{"jsonrpc":"2.0","id":"vibemux-verify-init-1","result":{"serverInfo":{"name":"deepseek-harness-sdk-runtime","version":"0.0.1"}}}
```

Frame 2 written to stdin:

```json
{"jsonrpc":"2.0","id":"vibemux-verify-shutdown-2","method":"shutdown"}
```

Shutdown response received verbatim:

```json
{"jsonrpc":"2.0","id":"vibemux-verify-shutdown-2","result":{}}
```

Runtime outcome: exit code `0` after 1435–2241 ms, no signal, **0 bytes on
stderr**, exactly 2 stdout frames totalling 192 bytes.

### Comparison against `docs/dsh_session_protocol.md` §1

| Documented fact | Observed against 0.2.0-rc.2 | Difference |
|---|---|---|
| Client→server requests `initialize`, `session/prompt`, `shutdown` | `initialize` and `shutdown` accepted and answered | none observed |
| `serverInfo.name == "deepseek-harness-sdk-runtime"` | `"deepseek-harness-sdk-runtime"` | **none** |
| `serverInfo.version` is the literal `0.0.1` | `"0.0.1"` | **none** |
| Framing: one JSON object per `\n`-terminated line | Exactly one frame per line; no partial or extra output | none observed |
| Frame dispatch: `id`+`method` = request, `id` alone = response | Response carried `jsonrpc`, `id`, `result` only | none observed |
| `stdout` is protocol frames only | 192 stdout bytes, all protocol; stderr empty | none observed |
| Boot command `dsh --profile sdk` | `node <entry_script> --profile sdk` booted and served `initialize` | none observed |
| `shutdown` takes no params → `{}` | Empty result object returned | none observed |

**No difference was found.** The `InitializeResult` shape is
`{jsonrpc, id, result:{serverInfo:{name, version}}}` — top-level keys
`["jsonrpc","id","result"]`, result keys `["serverInfo"]`.

### Repetitions and controls

Five successful initialize-only runs were made. All produced **byte-identical
192-byte stdout**:

| # | Environment | Spawn posture | Result |
|---|---|---|---|
| 1 | inherited (full parent env) | plain pipes | `initialize` ok, exit 0 |
| 2 | adapter allowlist (`SYSTEMROOT`,`WINDIR`,`TEMP`,`TMP`) | plain pipes | `initialize` ok, exit 0 |
| 3 | adapter allowlist | `CREATE_NEW_PROCESS_GROUP`-style `detached` + `stderr: ignore` + `windowsHide` | `initialize` ok, exit 0 |
| 4 | adapter allowlist + adapter spawn | child `cwd` = `\\?\I:\VibeMux` | `initialize` ok, exit 0 |
| 5 | adapter allowlist + adapter spawn | `initialize.cwd` = `\\?\I:\VibeMux` | `initialize` ok, exit 0 |

Run 2 shows the handshake succeeds with **only** the four names the adapter
forwards and **without** `DEEPSEEK_API_KEY`, `USERPROFILE`, `DSH_HOME`, or
`PATH` in the child environment. Runs 3–5 were produced inside a real ConPTY
as well, so the environment allowlist, the creation flags, discarding stderr,
and a pseudo-console parent are all individually insufficient to break the
handshake.

## Part B — VibeMux adapter probe

### Build

```powershell
cargo build -p vibemux_frontend --bin vibemux_dsh_tui
```

The **first** invocation failed, exit code 1, from the `protoc` build scripts
of `vibemux_plugin_protocol` and `a2a-pb`:

```text
Error: Custom { kind: Other, error: "protoc failed: C:\\Users\\<user>\\AppData\\Local\\Temp\\prost-buildyUb52B\\prost-descriptor-set: Permission denied\r\n" }
```

An immediate retry with no source change succeeded (`Finished dev profile ...
in 46.61s`). The same intermittent failure then recurred once on the
`vibemuxd`/`vibemuxctl` build and again succeeded on retry. It is an
environment-level flake in this host's `protoc`
(`K:\annaconda\Library\bin\protoc.exe`, libprotoc 29.3) writing prost's
descriptor set into `%TEMP%`; direct `protoc --descriptor_set_out` runs into
both `%TEMP%` and a fresh `prost-build*` directory succeeded, so no persistent
ACL or sandbox block exists. It is unrelated to DSH and to the 0.2.0-rc.2
upgrade.

### Configuration surface

```powershell
target\debug\vibemux_dsh_tui.exe --project-root . --print-config --json
```

Exit code `0`:

```json
{
  "profile": "sdk",
  "provider": "deepseek-official",
  "model": "deepseek-v4-flash",
  "has_reasoning_effort": false,
  "max_tokens": null,
  "environment_name_count": 1,
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

The projection is content-free: it carries no path, no digest, and no prompt
text, exactly as documented.

### Real PTY session

`%TEMP%\dsh_tui_pty_probe.js` loads
`C:\Users\<user>\.dsh\profiles\node_modules\node-pty` (v1.2.0-beta.15, win32-x64
prebuilds present), spawns
`target\debug\vibemux_dsh_tui.exe --project-root I:\VibeMux` with
`cols=120 rows=32`, and captures output for 25 s before sending Ctrl-Q
(`\x11`). Nothing was installed and no repository file changed.

```text
[+    16 ms] pty spawned, pid=0
[+  1163 ms] pty exit: exitCode=1 signal=undefined
raw bytes captured : 184
Ctrl-Q sent        : no
exit code          : 1
```

Rendered 120x32 screen (ANSI stripped):

```text
vibemux dsh: dsh_driver_transport_closed (provider deepseek-official model deepseek-v4-flash)
```

Raw capture:

```text
[?9001h[?1004h[?25l[2J[m[H vibemux dsh: [1C ]0;I:\VibeMux\target\debug\vibemux_dsh_tui.exe [?25h dsh_driver_transport_closed (provider deepseek-official model deepseek-v4-flash)
```

- **Error code: `dsh_driver_transport_closed`** — the code
  `docs/dsh_session_protocol.md` §7 defines as "stdout reached EOF before the
  session finished". This is a session-level failure, so the process exits
  `1` through `usage_or_failure`.
- **There is no rendered status line.** The `DeepSeek Harness session` panel
  never drew: the surface failed during the initialize handshake, before the
  first frame. No `phase ... provider ... model ...` line, no `server ...`
  line, and no `records ...` line exist for this run. No `dsh_session_*` code
  appeared; that vocabulary belongs to the Control v7 path below.
- `Ctrl-Q` was never needed, because the process had already exited.

### Control v7 path (fallback, as authorized)

`vibemuxctl daemon start` could not auto-spawn the daemon:

```text
{"error":"daemon process could not be spawned","error_code":"daemon_spawn_failed","ok":false}   # exit 4
```

Running `target\debug\vibemuxd.exe --project-root I:\VibeMux` directly works,
so the failure is in `vibemuxctl`'s spawn step rather than in the daemon
binary; it was not root-caused. Against the directly started daemon:

```text
daemon health   {"healthy":true,"ok":true,"process_id":151324,...,"store_schema_version":5}
dsh open        {"error":"daemon control operation failed: dsh_session_refused","error_code":"dsh_session_refused","ok":false}     # exit 4
dsh status      {"error":"daemon control operation failed: dsh_session_not_found","error_code":"dsh_session_not_found","ok":false} # exit 4
dsh close       {"error":"daemon control operation failed: dsh_session_not_found","error_code":"dsh_session_not_found","ok":false} # exit 4
daemon stop     {"ok":true,"process_id":151324,"status":"stopped"}                                                                  # exit 0
```

`dsh_session_refused` is the documented service code for a session that
refused the request. The daemon path drives the **same** `vibemux_dsh` driver
as the TUI, so the two independent surfaces fail the same way. The daemon was
started by this validation and stopped by it.

`dsh_session_poll` was not run: with no live session it would only return
`dsh_session_not_found`, and it is the one operation that prints runtime
content.

## Root cause (observed)

The runtime process **is** created, and the entry script is **never
executed**.

1. `std::fs::canonicalize` returns a Windows **verbatim** path. Measured with
   a `rustc`-built probe, not inferred:

   ```text
   canonicalize(I:\VibeMux) = \\?\I:\VibeMux
   ```

2. `vibemux_dsh::config` canonicalizes both the executable and the entry
   script (`canonical_outside_root` → `std::fs::canonicalize`), and
   `build_launch_spec` passes the canonical entry script as the interpreter's
   first argument: `vec![script, "--profile", profile]`.

3. Node cannot run a verbatim script path. With either installed image:

   ```powershell
   node 'C:\...\Temp\dsh_shim\hello.js'      # prints "SCRIPT RAN", exit 0
   node '\\?\C:\...\Temp\dsh_shim\hello.js'  # exit 1, nothing printed
   ```

   Against the real entry script, stderr shows:

   ```text
   Error: EISDIR: illegal operation on a directory, lstat 'C:'
       at Object.realpathSync (node:fs:2745:25)
       at toRealPath (node:internal/modules/helpers:63:13)
       at Module._findPath (node:internal/modules/cjs/loader:803:22)
       at resolveMainPath (node:internal/modules/run_main:35:21)
       at Module.executeUserEntryPoint [as runMain:146:20]
   ```

   Both `C:\nvm4w\nodejs\node.exe` (v24.18.0) and
   `C:\Program Files\nodejs\node.exe` (v24.19.0) fail identically. Node's CJS
   loader mis-parses the verbatim path and tries to `lstat("C:")`, a
   directory, and exits `1` before loading any script.

4. The driver sets `.stderr(Stdio::null())`, so that error is discarded. The
   child's stdout closes with no frames, the reader task reports EOF, and the
   failure surfaces as `dsh_driver_transport_closed`. On the daemon path the
   same closed transport surfaces as `dsh_session_refused`.

This chain was confirmed end to end without touching the repository, by
installing a **valid** entry script under the adapter's own trust rule: a
`package.json` declaring `"name": "@deepseek-ai/dsh"` with
`"bin": {"dsh": "lib/bin.js"}` in `%TEMP%\dsh_shim`, whose `lib/bin.js` records
its launch context and re-executes the real runtime with the runtime's stderr
redirected to a file. `--print-config` accepted it (exit 0), which proves the
ownership check passed. Under **both** surfaces its trace file was never
created — the script never ran — while the identical shim invoked with a plain
path ran immediately and successfully re-executed the real `dsh`. This is what
makes the failure a path-encoding bug rather than a protocol or environment
problem.

**This is a VibeMux-side defect and is not caused by the 0.2.0-rc.2 upgrade.**
Nothing in the observed failure depends on DSH's version: the adapter never
launches the runtime on this host, so no DSH code runs at all. It would fail
identically against `0.1.2-rc.1`. The `docs/dsh_session_protocol.md` §10 note
("Live validation. No prompt has been sent to a real DSH runtime. The adapter
is verified against the wire shapes ... and against `vibemux_dsh_fixture`")
is consistent with this: an initialize-only probe through the adapter was
never run against a real runtime, so this path was never exercised.

## Verdict

**No — the VibeMux DSH adapter does not work against `dsh` 0.2.0-rc.2 on this
host.** Both of its surfaces fail before the handshake completes: the terminal
surface with `dsh_driver_transport_closed` (exit 1) and the Control v7 path
with `dsh_session_refused` (exit 4). Neither reached a `Ready` phase, so no
status line was ever rendered.

**The DSH wire contract itself is intact and unchanged.** `initialize`
answers with `serverInfo.name == "deepseek-harness-sdk-runtime"` and
`serverInfo.version == "0.0.1"`, exactly as `docs/dsh_session_protocol.md` §1
documents, and `shutdown` returns `{}` and a clean exit 0. Five independent
initialize-only runs, including ones reproducing the adapter's environment
allowlist, creation flags, discarded stderr, ConPTY parent, and verbatim
`cwd`, all succeeded with byte-identical output.

**The upgrade is not the cause.** The adapter passes a `\\?\` verbatim entry
script to Node, and Node cannot load a script from that form; the resulting
error is hidden by `Stdio::null()`. That defect is version-independent, so
"does the adapter still work after the upgrade" is really "the adapter has
never worked against a real runtime on this host, and still does not".

The registration, configuration, trust, and projection layers are unaffected:
`--print-config --json` returns the documented content-free summary with exit
code 0, and the launcher-pair ownership rule accepted a legitimate entry
script outside the project root.

## Not verified

- **Any turn.** No `session/prompt` was sent on any path (hard constraint:
  paid inference was not authorized). Therefore unverified against
  0.2.0-rc.2: turn execution, `session.event` and `session.status` handling,
  `turn/end.reason.kind` mapping, subagent notifications, record buffering,
  `dsh poll` paging, and every completion-authority rule in
  `docs/dsh_session_protocol.md` §5.
- **Whether 0.2.0-rc.2 changed any *notification* shape.** Only the
  request/response surface and `serverInfo` were observed; notifications
  cannot appear without a turn.
- **The `Ready`-phase terminal UI.** The status line, transcript, input box,
  hint line, the Ctrl-Q quit path, Esc cancellation, and scrolling were never
  reached on any run, so their behaviour against 0.2.0-rc.2 is untested.
- **`dsh cancel` and a cooperative close of a live session.** No session ever
  reached `Ready`, so `shutdown_grace_ms` and forced termination were never
  exercised against the real runtime.
- **The fix.** No repository file was modified, so no claim is made about the
  correct repair (candidate directions include passing a non-verbatim path, or
  not canonicalizing the argument that reaches Node's argv). This report is
  evidence, not a patch.
- **The driver's exact creation flags.** libuv's `detached` adds
  `DETACHED_PROCESS` alongside `CREATE_NEW_PROCESS_GROUP`, so the Node harness
  reproduced a close but not bit-identical flag set. This became moot once the
  entry-script probe proved the script never executes.
- **`protoc` flake.** It failed 2 of the 3 `cargo build` invocations made
  during this validation and succeeded on retry each time with no source
  change; the exact cause was not determined.
- **`vibemuxctl daemon start`.** Fails with `daemon_spawn_failed` while
  `vibemuxd.exe --project-root ...` runs correctly; not root-caused.
- **Linux/WSL.** Only native Windows was exercised.
- **Version independence of the root cause** is inferred from the mechanism
  (Node's module loader rejecting a verbatim path), not measured against
  `0.1.2-rc.1`, which is no longer installed.

## Addendum (2026-10-06, later the same day) — repaired and re-probed

The verdict above records the state during the validation. The defect it
identified was then repaired, and the adapter was re-probed live.

**Repair.** `crates/vibemux_harness/src/dispatch/launch_spec.rs` gained
`interpreter_script_path`, applied to the launcher-pair script argument:
`\\?\C:\x` → `C:\x`, `\\?\UNC\s\share\x` → `\\s\share\x`, and a verbatim path
with no classic equivalent (a volume GUID path) is left unchanged so it fails
loudly rather than silently pointing somewhere else. The admitted canonical path
remains the trust anchor used by `canonical_outside_root`; only the string
handed to Node's loader changes. The executable is untouched, because
`CreateProcess` accepted the verbatim image (the child was created in every
failed run).

**Tests.** `cargo test -p vibemux_harness -p vibemux_dsh --all-features`: all
suites green, including three new unit tests for the transformation and one
Windows-gated integration test asserting that a route whose `entry_script` is
`\\?\C:\tools\dsh-package\lib\bin.js` produces the argv
`["C:\\tools\\dsh-package\\lib\\bin.js", "--profile", "sdk"]` with no `\\?\`
prefix anywhere in argv. `cargo clippy -p vibemux_harness -p vibemux_dsh
--all-targets --all-features -- -D warnings` clean.

**Live re-probe.** `cargo build -p vibemux_frontend --bin vibemux_dsh_tui`, then
the same node-pty harness (120x32, `--project-root I:\VibeMux`), against the
same real `dsh` 0.2.0-rc.2 runtime. Before the repair this exited `1` after
~1.16 s with `dsh_driver_transport_closed` and never drew a status line. After
the repair the surface reached the Ready phase and rendered:

```text
┌ DeepSeek Harness session ──────────────────────────────┐
│ phase ready   provider deepseek-official   model deepseek-v4-flash
│ server 0.0.1  turns 0 (ok 0/failed 0/cancelled 0/unverified 0)
└────────────────────────────────────────────────────────┘
┌ runtime records ───────────────────────────────────────┐
│ vibemux dsh: starting the DeepSeek Harness SDK runtime
│ vibemux dsh: profile sdk provider deepseek-official model deepseek-v4-flash
│ 1 handshake ok: deepseek-harness-sdk-runtime 0.0.1
└────────────────────────────────────────────────────────┘
```

`Ctrl-Q` then quit with exit code `0` after 21.1 s. This is the first live DSH
session this adapter has completed.

**Still not verified after the repair.** Any turn (no `session/prompt` was sent
before or after), the Control v7 daemon path (`vibemuxctl dsh open`) re-run
against the repaired driver, `Esc` cancellation against a live turn, and
Linux/WSL.

## Working-tree note

This validation modified no tracked repository file. Its only repository-tree
write is this new file plus the gitignored local runtime state
`.vibemux/dsh_session.json`. During the session, `git status --short` also
showed changes that this validation did not make and did not touch:
`M README.md`, `M CONTRIBUTING.md`, `?? scripts/setup_dsh_prereq.ps1`,
`?? tests/test_dsh_prereq.py`, `?? ui_check/`, `?? tmpnex29_6h`,
`?? tmpvr_p8ysl` — concurrent work in the shared workspace by another agent.
Three `node.exe` processes running `dsh ... --profile tui` were already
running before this validation started and were left alone.
