// A minimal, content-free Chromium lifecycle probe for NativeProcess tests.
// It never navigates to candidate content and never changes browser sandbox
// flags. stdout is bounded JSONL phase evidence; stderr text is not emitted.

import { spawn } from "node:child_process";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { parseArgs } from "node:util";

import { Cdp_connection, Cdp_session } from "./cdp_client.mjs";

const probe_config = Object.freeze({
  devtools_wait_ms: 20_000,
  devtools_poll_ms: 50,
  browser_close_ms: 5_000,
  stderr_scan_bytes: 16 * 1024,
  stderr_count_cap: 1024 * 1024,
  flags: [
    "--headless=new",
    "--remote-debugging-port=0",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-extensions",
    "--disable-gpu",
    "--disable-background-networking",
    "--disable-component-update",
    "--disable-sync",
    "--no-pings",
    "--proxy-server=http://127.0.0.1:9",
  ],
});
const known_process_types = new Set(["browser", "renderer", "gpu-process", "utility", "zygote"]);

function emit(phase, extra = {}) {
  process.stdout.write(`${JSON.stringify({ phase, ...extra })}\n`);
}

function pause(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function error_class(error) {
  if (error?.code === "ENOENT" || error?.code === "EACCES") return "spawn_error";
  if (Number.isInteger(error?.details?.code)) return "cdp_error";
  const message = String(error?.message ?? "");
  if (message.includes("timed out")) return "timeout";
  if (message.includes("websocket")) return "websocket";
  if (message.includes("access denied") || message.includes("Access is denied")) return "access_denied";
  if (message.includes("aborted")) return "aborted";
  return "other";
}

function stderr_observer(child) {
  const observation = {
    bytes: 0,
    scanned: 0,
    access_denied: false,
    sandbox: false,
    renderer: false,
    crashpad: false,
    win32_access_denied: false,
    process_create: false,
    job_object: false,
  };
  child.stderr.on("data", (chunk) => {
    observation.bytes = Math.min(probe_config.stderr_count_cap, observation.bytes + chunk.length);
    if (observation.scanned >= probe_config.stderr_scan_bytes) return;
    const part = chunk.subarray(0, probe_config.stderr_scan_bytes - observation.scanned);
    observation.scanned += part.length;
    const text = part.toString("utf8").toLowerCase();
    observation.access_denied ||= text.includes("access denied") || text.includes("access is denied");
    observation.sandbox ||= text.includes("sandbox");
    observation.renderer ||= text.includes("renderer");
    observation.crashpad ||= text.includes("crashpad");
    observation.win32_access_denied ||= text.includes("0x80070005") || text.includes("0xc0000022");
    observation.process_create ||= text.includes("createprocess") || text.includes("create process");
    observation.job_object ||= text.includes("jobobject") || text.includes("job object");
  });
  return observation;
}

async function devtools_endpoint(profile, child) {
  const deadline = Date.now() + probe_config.devtools_wait_ms;
  while (Date.now() < deadline) {
    if (child.spawn_error !== null) throw child.spawn_error;
    if (child.exitCode !== null) throw new Error("browser exited before DevTools was ready");
    const text = await readFile(path.join(profile, "DevToolsActivePort"), "utf8").catch(() => null);
    if (text !== null) {
      const [port_text, endpoint_path] = text.trim().split(/\r?\n/);
      const port = Number(port_text);
      if (Number.isInteger(port) && port > 0 && port <= 65535 && endpoint_path?.startsWith("/devtools/browser/")) {
        return `ws://127.0.0.1:${port}${endpoint_path}`;
      }
    }
    await pause(probe_config.devtools_poll_ms);
  }
  throw new Error("DevToolsActivePort timed out");
}

async function wait_exit(child, timeout_ms) {
  if (child.exitCode !== null) return true;
  return Promise.race([
    new Promise((resolve) => child.once("exit", () => resolve(true))),
    pause(timeout_ms).then(() => false),
  ]);
}

async function run() {
  const args = parseArgs({
    args: process.argv.slice(2),
    strict: true,
    options: {
      browser: { type: "string" },
      temp_root: { type: "string" },
      hold_after_attach: { type: "boolean" },
      wait_for_start: { type: "boolean" },
    },
  }).values;
  if (!args.browser || !path.isAbsolute(args.browser) || !args.temp_root || !path.isAbsolute(args.temp_root)) {
    emit("failure", { failed_at: "arguments", error_class: "invalid_arguments" });
    return 2;
  }
  const profile = await mkdtemp(path.join(args.temp_root, "browser_lifecycle_"));
  let browser = null;
  let stderr = null;
  let connection = null;
  let phase = "browser_ready";
  try {
    if (args.wait_for_start) {
      const started = await new Promise((resolve) => {
        process.stdin.once("data", () => resolve(true));
        process.stdin.once("end", () => resolve(false));
        process.stdin.resume();
      });
      if (!started) throw new Error("direct probe start gate closed");
    }
    browser = spawn(args.browser, [
      ...probe_config.flags,
      `--user-data-dir=${profile}`,
      "about:blank",
    ], {
      cwd: args.temp_root,
      env: process.env,
      stdio: ["ignore", "ignore", "pipe"],
      windowsHide: true,
      shell: false,
    });
    browser.spawn_error = null;
    browser.on("error", (error) => {
      browser.spawn_error = error;
    });
    stderr = stderr_observer(browser);
    emit("browser_spawned", { browser_pid: browser.pid ?? null });
    const endpoint = await devtools_endpoint(profile, browser);
    connection = await Cdp_connection.connect(endpoint);
    const version = await connection.send("Browser.getVersion");
    const version_match = String(version.product ?? "").match(/\b(?:Edg|Chrome|Chromium)\/(\d+(?:\.\d+){0,3})\b/);
    emit("browser_ready", {
      browser_family: String(version.product ?? "").includes("Edg/") ? "edge" : "chromium",
      browser_version: version_match?.[1] ?? null,
    });
    phase = "target_create";
    const { targetId } = await connection.send("Target.createTarget", { url: "about:blank" });
    emit("target_created", { target_present: typeof targetId === "string" });
    phase = "target_attach";
    const { sessionId } = await connection.send("Target.attachToTarget", { targetId, flatten: true });
    const session = new Cdp_session(connection, sessionId, targetId);
    emit("target_attached", { session_present: typeof sessionId === "string" });
    if (args.hold_after_attach) {
      const process_info = await connection.send("SystemInfo.getProcessInfo", {}, { timeout_ms: 2000 }).catch(() => null);
      const processes = (process_info?.processInfo ?? [])
        .filter((entry) => Number.isInteger(entry.id) && entry.id > 0)
        .slice(0, 32)
        .map((entry) => ({
          pid: entry.id,
          type: known_process_types.has(entry.type) ? entry.type : "other",
        }));
      emit("process_observation", { processes });
      emit("held_for_cancel");
      await new Promise(() => setInterval(() => undefined, 1000));
    }
    phase = "runtime_enable";
    await session.send("Runtime.enable");
    emit("runtime_enabled");
    phase = "runtime_evaluate";
    const evaluated = await session.send("Runtime.evaluate", { expression: "1 + 1", returnByValue: true });
    emit("runtime_evaluated", { value_is_two: evaluated.result?.value === 2 });
    const process_info = await connection.send("SystemInfo.getProcessInfo", {}, { timeout_ms: 2000 }).catch(() => null);
    const processes = (process_info?.processInfo ?? [])
      .filter((entry) => Number.isInteger(entry.id) && entry.id > 0)
      .slice(0, 32)
      .map((entry) => ({
        pid: entry.id,
        type: known_process_types.has(entry.type) ? entry.type : "other",
      }));
    emit("process_observation", { processes });
    phase = "page_enable";
    await session.send("Page.enable");
    emit("page_enabled");
    return 0;
  } catch (error) {
    emit("failure", {
      failed_at: phase,
      error_class: error_class(error),
      cdp_error_code: Number.isInteger(error?.details?.code) ? error.details.code : null,
      stderr,
    });
    return 1;
  } finally {
    if (connection !== null && !connection.closed) {
      await connection.send("Browser.close", {}, { timeout_ms: 3000 }).catch(() => undefined);
      connection.close();
    }
    let browser_exit_observed = browser === null || await wait_exit(browser, probe_config.browser_close_ms);
    if (browser !== null && !browser_exit_observed) {
      browser.kill();
      browser_exit_observed = await wait_exit(browser, probe_config.browser_close_ms);
    }
    if (browser_exit_observed) await rm(profile, { recursive: true, force: true }).catch(() => undefined);
    emit("cleanup", { browser_exit_observed, stderr });
  }
}

process.exitCode = await run();
