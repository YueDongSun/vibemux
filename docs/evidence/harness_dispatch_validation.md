# Harness dispatch live validation (ADR 029 Stage 7)

Date: 2026-09-27. Platform: native Windows 11 Pro 10.0.26200, Rust 1.85/MSVC
debug builds.

## Scope

Stage 7 of [ADR 029](../adr/029_daemon_harness_dispatch.md) runs
`vibemuxctl dispatch probe` against the vendor CLIs installed on the
development host. A probe sends only the protocol's initialize handshake. No
prompt was sent and no model inference ran. `dispatch submit` was not run: a
real submit sends a prompt that may use paid inference, and that needs the
user's explicit authorization. Everything this stage did not exercise is
listed under [Not verified](#not-verified).

Code under test: `feat/harness_dispatch_port` at `4302c13` (Stage 6). Stage 7
changes documentation only. The debug `vibemuxd`, `vibemux_launch_trampoline`,
`vibemuxctl`, and `vibemux_probe` binaries were built from that commit, with
the trampoline next to the daemon.

## Installed vendor CLIs

| Harness | `--version` | Detected launcher | Route protocol | Route executable |
|---|---|---|---|---|
| Codex | `codex-cli 0.157.1` | `power_shell_companion` | `codex_app_server` | native `codex.exe` inside the npm platform package |
| Claude | `2.1.283 (Claude Code)` | `direct_executable` | `claude_stream_json` | native installer image |
| OpenCode | `1.18.32` | `power_shell_companion` | `acp` | native `opencode.exe` inside the npm platform package |
| Copilot | `GitHub Copilot CLI 1.0.87-0.` | `direct_executable` | `acp` | native installer image |
| Grok | `grok 1.0.41 (4220f3b224a6) [stable]` | `direct_executable` | `acp` | native installer image |

Qwen, iFlow, TRAE, CodeBuddy, and Kimi are not installed on this host.
`vibemuxctl harnesses` reports them unavailable, and no route names them.

Codex and OpenCode are detected through their npm PowerShell companions. A
route rejects `.cmd`, `.bat`, and PowerShell shims (ADR 029 Decision 4), so
these two routes name the native image inside the npm platform package. All
five executables are regular files, none is a symbolic link or other reparse
point, each lies outside the project root, and each file stem is the harness
command name. The config loader accepted all five routes.

## Setup

- A scratch project directory outside any Git repository. A probe needs no
  Git `HEAD`.
- `vibemux_probe --write-cache` and `vibemuxctl harnesses` persisted
  detection, because dispatch admission requires a detected harness.
- `.vibemux/harness_dispatch.json`, schema 1, with one route per installed
  harness. Every route is `enabled: true` and `allow_execution: false`, so the
  daemon refuses any submit. Each route forwards `USERPROFILE`, `APPDATA`,
  and `LOCALAPPDATA`. The Claude route also forwards `DISABLE_AUTOUPDATER`
  and the OpenCode route `OPENCODE_DISABLE_AUTOUPDATE`. Both variables were
  set in the daemon's environment so that a probe cannot start a vendor
  update.
- Run 1 used the default limits. Run 2 added
  `"limits": {"shutdown_grace_ms": 5000}`. The daemon reads the config once at
  startup, so it was restarted between the runs.

The commands, shown for PowerShell from the build output directory (the runs
used equivalent Git Bash commands):

```powershell
.\vibemux_probe.exe --write-cache --project-root <project>
.\vibemuxctl.exe daemon start --project-root <project>
.\vibemuxctl.exe harnesses --project-root <project>
.\vibemuxctl.exe dispatch catalog --project-root <project>
.\vibemuxctl.exe dispatch probe codex --project-root <project>
.\vibemuxctl.exe dispatch probe claude --project-root <project>
.\vibemuxctl.exe dispatch probe opencode --project-root <project>
.\vibemuxctl.exe dispatch probe copilot --project-root <project>
.\vibemuxctl.exe dispatch probe grok --project-root <project>
.\vibemuxctl.exe daemon stop --project-root <project>
```

`dispatch catalog` listed the five routes with `probe_supported: true` and
`allow_execution: false`.

## Results

Each probe prints a content-free report. The tables show its fields. Wall
time is the `vibemuxctl` wall-clock time, including the Control round trip.

Run 1, default limits (`shutdown_grace_ms` 2,000):

| Harness | Outcome | Error code | Exit code | Forced | Records (bytes) | Stderr bytes | Wall |
|---|---|---|---|---|---|---|---|
| Codex | `probed` | none | 0 | no | 3 (766) | 345 | 525 ms |
| Claude | `probed` | none | 0 | no | 2 (29,864) | 0 | 937 ms |
| OpenCode | `probed` | none | 0 | no | 1 (444) | 0 | 1,249 ms |
| Copilot | `probed` | none | 0 | no | 1 (583) | 0 | 3,371 ms |
| Grok | `failed` | `harness_process_failed` | 1 | yes | 2 (3,763) | 0 | 2,521 ms |

Two more Grok probes against the same daemon gave the same report (2,372 and
2,369 ms).

Run 2, `shutdown_grace_ms` 5,000:

| Harness | Outcome | Error code | Exit code | Forced | Records (bytes) | Stderr bytes | Wall |
|---|---|---|---|---|---|---|---|
| Codex | `probed` | none | 0 | no | 3 (766) | 345 | 355 ms |
| Claude | `probed` | none | 0 | no | 2 (29,864) | 0 | 989 ms |
| OpenCode | `probed` | none | 0 | no | 1 (444) | 0 | 1,347 ms |
| Copilot | `probed` | none | 0 | no | 1 (583) | 0 | 681 ms |
| Grok | `probed` | none | 0 | no | 2 (3,763) | 0 | 2,571 ms |

- Every record was classified `opaque`: an initialize exchange carries no
  text, tool, usage, or turn terminal record. The probe discards its records
  after the report, which carries only counts, byte totals, and the
  transcript digest. Stderr is counted, never captured.
- Record counts and byte totals were the same in both runs.
- After the 12 daemon probes, the scratch store had no `harness_dispatches`
  row. Its event log held only the harness registry seed and the detection
  refresh from `vibemuxctl harnesses`, so the probes wrote no canonical state.
- After each run, no vendor process launched with a probe argv was still
  running.

## Grok forced termination (open decision 4)

The Grok failure in run 1 is a clean but slow exit, not a crash.

- It reproduced in 3 of 3 probes, with the same record count and bytes.
- A temporary script outside the daemon replayed the same ACP initialize
  message to `grok agent stdio`, with the same cleared environment. It printed
  only record structure and timing, and closed stdin once the initialize
  result arrived. In three replays, Grok answered after 0.25 to 0.30 s with a
  result carrying `protocolVersion`, `agentCapabilities`, `authMethods`, and
  `_meta`, and exited with code 0 2.12 to 2.14 s after stdin closed. The two
  replays that kept reading also saw one vendor notification
  (`_x.ai/mcp/servers_updated`) right after the result, which matches the two
  records the daemon counted, and saw stdout close together with the exit.
- The default grace is 2,000 ms. When the grace expires after a terminal
  record, the executor kills the process tree. On Windows,
  `TerminateJobObject` sets exit code 1. Under open decision 4's conservative
  rule, the attempt is then `failed` with `harness_process_failed`, although
  the vendor would have exited cleanly about 120 ms later.
- With `limits.shutdown_grace_ms` at 5,000, the same probe returns `probed`
  with exit code 0 and no forced termination.

This is the first live case for decision 4: the conservative rule reports a
clean but slow vendor shutdown as a failure. Operators can raise
`limits.shutdown_grace_ms` (100 to 10,000 ms). That value applies to every
route and also bounds the wait after a cancel. The options still open are a
larger default, a per-route grace, or a narrower rule. This stage changes no
code.

## Claude flag combination (open decision 3)

Claude Code 2.1.283 accepted the route's full argv: `--bare -p
--input-format stream-json --output-format stream-json --verbose
--include-partial-messages --permission-mode dontAsk --tools=
--strict-mcp-config --restricted --no-session-persistence`. The probe
reported `probed`, which requires a `control_response` with subtype
`success` for the `vibemux_initialize` request. In both runs the process
exited on its own with code 0. So `--strict-mcp-config` and `--restricted`
combine with `--bare` stream-json. Their effect during a turn (no MCP
servers, restricted mode, no tools) needs a submit and was not exercised.

## Not verified

- `dispatch submit` against any vendor. It was not run, because it needs
  explicit authorization of paid inference. So these are unverified: Codex
  and Claude execution turns, their turn terminal and usage records,
  `dispatch output` with real vendor content, and `dispatch cancel` against a
  live vendor.
- Whether the forwarded environment names let each vendor authenticate
  during a real turn. An initialize handshake does not authenticate: Grok's
  result only lists its auth methods, and the probe cache reports
  `authentication_state: not_run` for every harness.
- `codex_exec`. It has no initialize handshake and cannot be probed, so no
  route used it.
- ACP execution and ACP permission postures. ACP routes are probe-only by
  design (ADR 029 Decision 4).
- Live vendors on Linux or WSL. Only native Windows was exercised.
- Vendor versions other than those listed above.
