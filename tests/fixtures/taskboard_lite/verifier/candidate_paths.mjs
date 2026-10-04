// Trusted verifier helpers: resolve a candidate product directory, own temp
// directories, and own the lifecycle of every child process the verifier
// starts (candidate servers, the contract stub server, test runners).
//
// Every process started here is tracked so callers can stop it and await its
// exit in a finally block. Nothing is ever written into the candidate.

import { spawn } from "node:child_process";
import { mkdtemp, rm, stat } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

export const process_config = Object.freeze({
  temp_prefix: "taskboard_lite_",
  startup_timeout_ms: 10_000,
  stop_timeout_ms: 5_000,
  tree_kill_timeout_ms: 10_000,
  receipt_max_bytes: 256,
  captured_output_max_bytes: 64 * 1024,
  allowed_hosts: Object.freeze(["127.0.0.1", "::1"]),
  temp_remove_retries: 10,
  temp_remove_retry_delay_ms: 200,
});

export class Server_start_error extends Error {
  constructor(message, details) {
    super(message);
    this.name = "Server_start_error";
    this.details = details;
  }
}

// ------------------------------------------------------------- candidate --

export function resolve_candidate_layout(candidate_dir) {
  const root = path.resolve(candidate_dir);
  return Object.freeze({
    candidate_dir: root,
    server_entry: path.join(root, "src", "server.mjs"),
    store_entry: path.join(root, "src", "store.mjs"),
    public_dir: path.join(root, "public"),
    package_json: path.join(root, "package.json"),
  });
}

export async function is_directory(directory_path) {
  try {
    return (await stat(directory_path)).isDirectory();
  } catch {
    return false;
  }
}

export async function is_file(file_path) {
  try {
    return (await stat(file_path)).isFile();
  } catch {
    return false;
  }
}

// Reads TASKBOARD_CANDIDATE_DIR for node:test suites running as a child.
export function candidate_layout_from_environment() {
  const candidate_dir = process.env.TASKBOARD_CANDIDATE_DIR;
  if (typeof candidate_dir !== "string" || candidate_dir.length === 0 || !path.isAbsolute(candidate_dir)) {
    throw new Error("TASKBOARD_CANDIDATE_DIR must be set to an absolute candidate directory");
  }
  return resolve_candidate_layout(candidate_dir);
}

// ------------------------------------------------------------- temp dirs --

// run_verifier.mjs creates one temp root per run and hands it to child test
// suites through TASKBOARD_VERIFIER_TEMP_ROOT, so directories left by a
// suite that is killed on timeout are still removed with that root.
export function default_temp_root() {
  const configured_root = process.env.TASKBOARD_VERIFIER_TEMP_ROOT;
  return typeof configured_root === "string" && path.isAbsolute(configured_root) ? configured_root : os.tmpdir();
}

export async function create_temp_dir(label, temp_root = default_temp_root()) {
  return mkdtemp(path.join(temp_root, `${process_config.temp_prefix}${label}_`));
}

export async function remove_temp_dir(directory_path) {
  if (directory_path === undefined || directory_path === null) {
    return;
  }
  await rm(directory_path, {
    recursive: true,
    force: true,
    maxRetries: process_config.temp_remove_retries,
    retryDelay: process_config.temp_remove_retry_delay_ms,
  });
}

// ------------------------------------------------------------- processes --

function append_capped(current_buffer, chunk, max_bytes) {
  if (current_buffer.length >= max_bytes) {
    return current_buffer;
  }
  const room = max_bytes - current_buffer.length;
  return Buffer.concat([current_buffer, chunk.subarray(0, room)]);
}

// Wraps a spawned child: captures bounded stdout/stderr and exposes one exit
// promise that settles for both normal exits and spawn errors.
export function track_child_process(child, capture_max_bytes = process_config.captured_output_max_bytes) {
  const tracked = {
    child,
    pid: child.pid,
    stdout_bytes: Buffer.alloc(0),
    stderr_bytes: Buffer.alloc(0),
    stdout_total_bytes: 0,
    exit_result: null,
    stdout_listeners: new Set(),
    exit_promise: null,
  };
  tracked.exit_promise = new Promise((resolve) => {
    child.once("exit", (code, signal) => {
      tracked.exit_result ??= { code, signal, spawn_error: null };
      resolve(tracked.exit_result);
    });
    child.once("error", (error) => {
      tracked.exit_result ??= { code: null, signal: null, spawn_error: error.message };
      resolve(tracked.exit_result);
    });
  });
  child.stdout?.on("data", (chunk) => {
    tracked.stdout_total_bytes += chunk.length;
    tracked.stdout_bytes = append_capped(tracked.stdout_bytes, chunk, capture_max_bytes);
    for (const listener of tracked.stdout_listeners) {
      listener();
    }
  });
  child.stderr?.on("data", (chunk) => {
    tracked.stderr_bytes = append_capped(tracked.stderr_bytes, chunk, capture_max_bytes);
  });
  return tracked;
}

export function spawn_tracked(
  executable,
  args,
  { cwd, env = process.env, detached = false, capture_max_bytes = process_config.captured_output_max_bytes } = {},
) {
  const child = spawn(executable, args, {
    cwd,
    env,
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: true,
    detached: detached && process.platform !== "win32",
  });
  return track_child_process(child, capture_max_bytes);
}

export function stderr_text(tracked) {
  return tracked.stderr_bytes.toString("utf8");
}

export function stdout_text(tracked) {
  return tracked.stdout_bytes.toString("utf8");
}

// Returns the parsed `{"error":{"code":...}}` objects found on stderr lines.
export function stderr_error_codes(tracked) {
  const codes = [];
  for (const line of stderr_text(tracked).split(/\r?\n/)) {
    try {
      const value = JSON.parse(line);
      if (typeof value?.error?.code === "string") {
        codes.push(value.error.code);
      }
    } catch {
      // Non-JSON diagnostic lines are allowed on stderr.
    }
  }
  return codes;
}

function wait_with_timeout(promise, timeout_ms) {
  let timer;
  const timeout = new Promise((resolve) => {
    timer = setTimeout(() => resolve(null), timeout_ms);
  });
  return Promise.race([promise, timeout]).finally(() => clearTimeout(timer));
}

export async function wait_for_exit(tracked, timeout_ms) {
  return wait_with_timeout(tracked.exit_promise, timeout_ms);
}

// Kills the whole process tree (needed on Windows, where killing a parent
// does not kill its children).
export async function kill_process_tree(pid) {
  if (!Number.isInteger(pid)) {
    return;
  }
  if (process.platform === "win32") {
    const taskkill_path = path.join(process.env.SystemRoot ?? "C:\\Windows", "System32", "taskkill.exe");
    const killer = spawn_tracked(taskkill_path, ["/PID", String(pid), "/T", "/F"]);
    await wait_for_exit(killer, process_config.tree_kill_timeout_ms);
    return;
  }
  for (const target of [-pid, pid]) {
    try {
      process.kill(target, "SIGKILL");
    } catch {
      // The group or process is already gone.
    }
  }
}

export function is_process_alive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error.code === "EPERM";
  }
}

// Stops a tracked process and awaits its exit; escalates to a tree kill.
export async function stop_tracked_process(tracked, { tree = false } = {}) {
  if (tracked === null || tracked === undefined || tracked.exit_result !== null) {
    return;
  }
  if (tree) {
    await kill_process_tree(tracked.pid);
  } else {
    tracked.child.kill();
  }
  if ((await wait_for_exit(tracked, process_config.stop_timeout_ms)) !== null) {
    return;
  }
  await kill_process_tree(tracked.pid);
  if ((await wait_for_exit(tracked, process_config.stop_timeout_ms)) === null) {
    throw new Error(`process ${tracked.pid} did not exit after a tree kill`);
  }
}

// ------------------------------------------------------- receipt parsing --

export function format_origin(host, port) {
  return host.includes(":") ? `http://[${host}]:${port}` : `http://${host}:${port}`;
}

// Validates the listening receipt line against CONTRACT.md section 3.
export function validate_receipt_line(line_bytes, { host, pid }) {
  if (line_bytes.length > process_config.receipt_max_bytes) {
    throw new Error(`receipt line is ${line_bytes.length} bytes, limit is ${process_config.receipt_max_bytes}`);
  }
  let receipt;
  try {
    receipt = JSON.parse(line_bytes.toString("utf8"));
  } catch {
    throw new Error(`receipt line is not JSON: ${JSON.stringify(line_bytes.toString("utf8"))}`);
  }
  const is_object = receipt !== null && typeof receipt === "object" && !Array.isArray(receipt);
  const keys = is_object ? Object.keys(receipt).sort().join(",") : "";
  if (keys !== "event,host,pid,port") {
    throw new Error(`receipt keys must be exactly event, host, port, pid; got ${JSON.stringify(receipt)}`);
  }
  if (receipt.event !== "listening") {
    throw new Error(`receipt event must be "listening"; got ${JSON.stringify(receipt.event)}`);
  }
  if (receipt.host !== host) {
    throw new Error(`receipt host ${JSON.stringify(receipt.host)} differs from requested ${JSON.stringify(host)}`);
  }
  if (!Number.isInteger(receipt.port) || receipt.port < 1 || receipt.port > 65535) {
    throw new Error(`receipt port must be an integer from 1 to 65535; got ${JSON.stringify(receipt.port)}`);
  }
  if (receipt.pid !== pid) {
    throw new Error(`receipt pid ${JSON.stringify(receipt.pid)} differs from spawned pid ${pid}`);
  }
  return receipt;
}

function first_stdout_line(tracked) {
  const newline_index = tracked.stdout_bytes.indexOf(0x0a);
  if (newline_index === -1) {
    return null;
  }
  let line_end = newline_index;
  if (line_end > 0 && tracked.stdout_bytes[line_end - 1] === 0x0d) {
    line_end -= 1;
  }
  return tracked.stdout_bytes.subarray(0, line_end);
}

// Resolves with the first stdout line, or null when the process exits or the
// timeout elapses before a complete line arrives.
function wait_for_first_line(tracked, timeout_ms) {
  return new Promise((resolve) => {
    let settled = false;
    const finish = (value) => {
      if (settled) {
        return;
      }
      settled = true;
      clearTimeout(timer);
      tracked.stdout_listeners.delete(check_line);
      resolve(value);
    };
    const check_line = () => {
      const line = first_stdout_line(tracked);
      if (line !== null) {
        finish({ line });
      } else if (tracked.stdout_bytes.length > process_config.receipt_max_bytes * 4) {
        finish({ line: tracked.stdout_bytes });
      }
    };
    const timer = setTimeout(() => finish({ timed_out: true }), timeout_ms);
    tracked.stdout_listeners.add(check_line);
    tracked.exit_promise.then(() => {
      // Give already-buffered stdout data a chance to arrive before deciding.
      setImmediate(() => {
        check_line();
        finish({ exited: true });
      });
    });
    check_line();
  });
}

// Starts a node entry point with the contract argv, waits for a valid
// listening receipt, and returns a handle. The caller owns stopping it; on
// any failure the process is stopped before the error is thrown.
export async function start_listening_server({
  entry_path,
  host,
  port = "0",
  data_file,
  cwd,
  extra_args = [],
  startup_timeout_ms = process_config.startup_timeout_ms,
}) {
  const args = [entry_path, ...extra_args, "--host", host, "--port", String(port), "--data", data_file];
  const tracked = spawn_tracked(process.execPath, args, { cwd });
  try {
    const outcome = await wait_for_first_line(tracked, startup_timeout_ms);
    if (outcome.line === undefined) {
      const reason = outcome.timed_out ? `no receipt within ${startup_timeout_ms} ms` : "process exited without a receipt";
      throw new Server_start_error(`server did not start: ${reason}`, {
        exit_result: tracked.exit_result,
        stderr_error_codes: stderr_error_codes(tracked),
        stderr_tail: stderr_text(tracked).slice(-600),
      });
    }
    const receipt = validate_receipt_line(outcome.line, { host, pid: tracked.pid });
    return { tracked, receipt, origin: format_origin(receipt.host, receipt.port) };
  } catch (error) {
    await stop_tracked_process(tracked);
    if (error instanceof Server_start_error) {
      throw error;
    }
    throw new Server_start_error(`invalid listening receipt: ${error.message}`, {
      stderr_tail: stderr_text(tracked).slice(-600),
    });
  }
}

export function describe_start_error(error) {
  if (!(error instanceof Server_start_error)) {
    return error?.stack ?? String(error);
  }
  return `${error.message} ${JSON.stringify(error.details)}`;
}
