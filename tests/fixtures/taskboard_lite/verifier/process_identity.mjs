// Trusted verifier helper: kills a process only when its start time proves it
// is still the process the verifier saw.
//
// A process id alone is not an identity. Once a process exits, the OS may
// give its id to an unrelated process, and Windows reuses ids quickly. The
// verifier knows two instants for every id it may kill: when the id's owner
// was launched, and when the id was sampled (the process was running then).
// A process that later reuses the id can only start after the original
// exited, so after the sample. A live process whose start time lies between
// the launch and the sample is therefore the original one; anything else is
// left alone and reported.
//
// Start times are read on the clock that stamps them:
//   win32  - system time (Date.now()). Windows PowerShell opens a handle to
//            the process, reads Process.StartTime through it, and kills
//            through the same handle; an open handle keeps the id from being
//            reused between the check and the kill.
//   linux  - boot time (/proc/uptime) against field 22 of /proc/<pid>/stat.
//            The check and the kill are separate calls, but Linux hands out
//            ids sequentially, so an id is not reused within that gap.
//   others - start times are not read; nothing is confirmed or killed.

import { readFile } from "node:fs/promises";
import path from "node:path";
import { spawn_tracked, stderr_text, stdout_text, stop_tracked_process, wait_for_exit } from "./candidate_paths.mjs";

export const process_identity_config = Object.freeze({
  // Allowed difference between the verifier's clock readings and the clock
  // that stamps start times (V8's Date.now() is not the kernel's precise
  // system time; /proc values are truncated to 10 ms). The identity window
  // is shifted earlier by this much, so a process that started within it
  // before the sample is not confirmed.
  clock_tolerance_ms: 100,
  // USER_HZ, the unit of /proc/<pid>/stat start times on mainstream Linux.
  linux_clock_ticks_per_second: 100,
  powershell_timeout_ms: 30_000,
  powershell_kill_wait_ms: 5000,
  windows_filetime_unix_epoch: 116_444_736_000_000_000n,
  windows_filetime_ticks_per_ms: 10_000n,
});

// Returns the current time on the clock that stamps process start times, or
// null where start times are not read. Read it before launching a process and
// before sampling ids, and pass the readings to kill_confirmed_processes.
export async function read_process_clock_ms() {
  if (process.platform === "win32") {
    return Date.now();
  }
  if (process.platform === "linux") {
    try {
      return parse_linux_uptime_ms(await readFile("/proc/uptime", "utf8"));
    } catch {
      return null;
    }
  }
  return null;
}

// candidates: [{ pid, launched_at_ms, sampled_at_ms }], both readings from
// read_process_clock_ms. Kills each candidate whose start time proves its
// identity and leaves every other one running. Returns
// { killed: [pid], unconfirmed: [{ pid, reason }] }; a candidate that has
// already exited is in neither list. Never throws for a single process.
export async function kill_confirmed_processes(candidates) {
  const outcome = { killed: [], unconfirmed: [] };
  if (process.platform !== "win32" && process.platform !== "linux") {
    for (const { pid } of candidates) {
      outcome.unconfirmed.push({ pid, reason: `process start times are not read on ${process.platform}` });
    }
    return outcome;
  }
  const checkable = [];
  for (const candidate of candidates) {
    const window = identity_window(candidate);
    if (window === null) {
      outcome.unconfirmed.push({ pid: candidate.pid, reason: "no process clock reading for this id" });
    } else {
      checkable.push({ pid: candidate.pid, window });
    }
  }
  if (checkable.length === 0) {
    return outcome;
  }
  if (process.platform === "win32") {
    await kill_confirmed_windows(checkable, outcome);
  } else {
    await kill_confirmed_linux(checkable, outcome);
  }
  return outcome;
}

// The start times only the original process can have, shifted earlier by the
// clock tolerance: the upper bound must stay before the real sampling instant,
// and the lower bound must not reject the owner's earliest children.
function identity_window({ pid, launched_at_ms, sampled_at_ms }) {
  if (!Number.isSafeInteger(pid) || pid <= 0 || !Number.isFinite(launched_at_ms) || !Number.isFinite(sampled_at_ms)) {
    return null;
  }
  const tolerance_ms = process_identity_config.clock_tolerance_ms;
  return { not_before_ms: launched_at_ms - tolerance_ms, not_after_ms: sampled_at_ms - tolerance_ms };
}

function describe_outside_window(start_ms, window) {
  if (start_ms < window.not_before_ms) {
    return `started ${Math.round(window.not_before_ms - start_ms)} ms before its owner was launched`;
  }
  return `started ${Math.round(start_ms - window.not_after_ms)} ms after its id was sampled`;
}

// ----------------------------------------------------------------- linux --

function parse_linux_uptime_ms(uptime_text) {
  const match = /^(\d+)\.(\d+)\s/.exec(uptime_text);
  if (match === null) {
    throw new Error(`unexpected /proc/uptime format: ${JSON.stringify(uptime_text.slice(0, 40))}`);
  }
  return Number(match[1]) * 1000 + Number(match[2].padEnd(3, "0").slice(0, 3));
}

function parse_linux_start_ms(stat_text) {
  // Field 2 (comm) may contain spaces and parentheses; fields resume after
  // the last ")". The remainder starts at field 3, so field 22 is index 19.
  const fields = stat_text.slice(stat_text.lastIndexOf(")") + 2).split(" ");
  const start_ticks = Number(fields[19]);
  if (!Number.isSafeInteger(start_ticks)) {
    throw new Error("unexpected /proc/<pid>/stat format");
  }
  return (start_ticks * 1000) / process_identity_config.linux_clock_ticks_per_second;
}

async function kill_confirmed_linux(checkable, outcome) {
  for (const { pid, window } of checkable) {
    let start_ms;
    try {
      start_ms = parse_linux_start_ms(await readFile(`/proc/${pid}/stat`, "utf8"));
    } catch (error) {
      if (error.code !== "ENOENT") {
        outcome.unconfirmed.push({ pid, reason: `start time unavailable: ${error.message}` });
      }
      continue;
    }
    if (start_ms < window.not_before_ms || start_ms > window.not_after_ms) {
      outcome.unconfirmed.push({ pid, reason: describe_outside_window(start_ms, window) });
      continue;
    }
    try {
      process.kill(pid, "SIGKILL");
      outcome.killed.push(pid);
    } catch (error) {
      if (error.code !== "ESRCH") {
        outcome.unconfirmed.push({ pid, reason: `kill failed: ${error.message}` });
      }
    }
  }
}

// --------------------------------------------------------------- windows --

function filetime_from_ms(clock_ms) {
  const config = process_identity_config;
  return BigInt(Math.floor(clock_ms)) * config.windows_filetime_ticks_per_ms + config.windows_filetime_unix_epoch;
}

function ms_from_filetime(filetime) {
  const config = process_identity_config;
  return Number((filetime - config.windows_filetime_unix_epoch) / config.windows_filetime_ticks_per_ms);
}

// One line per id: "<pid> killed|unconfirmed|gone|error [detail]". Every value
// in the script is an integer produced by this module.
function windows_kill_script(checkable) {
  const ids = checkable.map(({ pid }) => String(pid)).join(",");
  const not_before = checkable.map(({ window }) => filetime_from_ms(window.not_before_ms).toString()).join(",");
  const not_after = checkable.map(({ window }) => filetime_from_ms(window.not_after_ms).toString()).join(",");
  return `
$ErrorActionPreference = 'Stop'
$ids = @(${ids}); $not_before = @(${not_before}); $not_after = @(${not_after})
for ($i = 0; $i -lt $ids.Count; $i++) {
  $id = $ids[$i]
  try { $p = [System.Diagnostics.Process]::GetProcessById($id) } catch { Write-Output "$id gone"; continue }
  try {
    $null = $p.Handle
    $start = $p.StartTime.ToFileTimeUtc()
    if ($start -lt $not_before[$i] -or $start -gt $not_after[$i]) { Write-Output "$id unconfirmed $start"; continue }
    try { $p.Kill() } catch [System.InvalidOperationException] { Write-Output "$id gone"; continue }
    if ($p.WaitForExit(${process_identity_config.powershell_kill_wait_ms})) { Write-Output "$id killed $start" }
    else { Write-Output "$id error kill did not finish within ${process_identity_config.powershell_kill_wait_ms} ms" }
  } catch {
    Write-Output ("$id error " + ($_.Exception.Message -replace '\\s+', ' '))
  } finally {
    $p.Dispose()
  }
}
`;
}

async function kill_confirmed_windows(checkable, outcome) {
  const powershell_path = path.join(process.env.SystemRoot ?? "C:\\Windows", "System32", "WindowsPowerShell", "v1.0", "powershell.exe");
  const encoded_script = Buffer.from(windows_kill_script(checkable), "utf16le").toString("base64");
  const tracked = spawn_tracked(powershell_path, ["-NoProfile", "-NonInteractive", "-EncodedCommand", encoded_script]);
  let failure = null;
  try {
    const exit_result = await wait_for_exit(tracked, process_identity_config.powershell_timeout_ms);
    if (exit_result === null) {
      failure = `identity check timed out after ${process_identity_config.powershell_timeout_ms} ms`;
    } else if (exit_result.code !== 0) {
      failure = `identity check exited with ${JSON.stringify(exit_result)} ${stderr_text(tracked).slice(-300)}`;
    }
  } finally {
    if (tracked.exit_result === null) {
      await stop_tracked_process(tracked, { tree: true }).catch(() => undefined);
    }
  }
  const verdicts = parse_windows_verdicts(stdout_text(tracked));
  for (const { pid, window } of checkable) {
    const verdict = verdicts.get(pid);
    if (verdict === undefined) {
      outcome.unconfirmed.push({ pid, reason: failure ?? "identity check gave no verdict" });
    } else if (verdict.kind === "killed") {
      outcome.killed.push(pid);
    } else if (verdict.kind === "unconfirmed") {
      outcome.unconfirmed.push({ pid, reason: describe_outside_window(ms_from_filetime(BigInt(verdict.detail)), window) });
    } else if (verdict.kind === "error") {
      outcome.unconfirmed.push({ pid, reason: `identity check failed: ${verdict.detail}` });
    }
  }
}

function parse_windows_verdicts(output_text) {
  const verdicts = new Map();
  for (const line of output_text.split(/\r?\n/)) {
    const match = /^(\d+) (killed|gone|unconfirmed|error)(?: (.*))?$/.exec(line.trim());
    if (match === null) {
      continue;
    }
    const [, pid_text, kind, detail = ""] = match;
    if (kind === "unconfirmed" && !/^\d+$/.test(detail)) {
      continue;
    }
    verdicts.set(Number(pid_text), { kind, detail });
  }
  return verdicts;
}
