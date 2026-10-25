import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { crash_reason, crash_result_config } from "./crash_result.mjs";

const module_url = pathToFileURL(path.join(path.dirname(fileURLToPath(import.meta.url)), "crash_result.mjs")).href;

// Runs `trigger` in a fresh Node process that installed the crash writer and
// returns the exit status and the result file it left behind.
function crash_in_child(trigger) {
  const directory = mkdtempSync(path.join(os.tmpdir(), "crash_result_test_"));
  const out_file = path.join(directory, "result.json");
  const script = [
    `import { install_crash_result_writer } from ${JSON.stringify(module_url)};`,
    `install_crash_result_writer({ out_file: ${JSON.stringify(out_file)},`,
    "  result_for: (blocked_reason) => ({ status: 'blocked', blocked_reason }) });",
    trigger,
  ].join("\n");
  try {
    const child = spawnSync(process.execPath, ["--input-type=module", "-e", script], { encoding: "utf8", timeout: 20_000 });
    let result = null;
    try {
      result = JSON.parse(readFileSync(out_file, "utf8"));
    } catch {
      // No result file: the assertions below report it.
    }
    return { status: child.status, result };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

test("a reason names the origin, error class, code, and first verifier frame only", () => {
  const error = new TypeError("Cannot read properties of null (reading 'send') at C:\\private\\candidate");
  error.code = "ERR_FIXTURE";
  error.stack = [
    "TypeError: Cannot read properties of null (reading 'send') at C:\\private\\candidate",
    "    at Page_driver.navigate (file:///C:/private/verifier/browser_suite.mjs:412:18)",
    "    at async run_single_check (file:///C:/private/verifier/browser_suite.mjs:956:5)",
  ].join("\n");
  const reason = crash_reason("unhandled_rejection", error);
  assert.equal(reason, "verifier_crash: unhandled_rejection TypeError ERR_FIXTURE at browser_suite.mjs:412");
  assert.ok(!reason.includes("private"));
  assert.ok(!reason.includes(" ("), "the daemon cuts a reason at ' ('");
  assert.ok(reason.length <= crash_result_config.max_reason_chars);
});

test("a thrown non-error value contributes only its type", () => {
  assert.equal(crash_reason("uncaught_exception", "C:\\private\\path"), "verifier_crash: uncaught_exception string");
  assert.equal(crash_reason("unhandled_rejection", undefined), "verifier_crash: unhandled_rejection undefined");
  const odd = new Error("x");
  odd.name = "Not a class name; C:\\private";
  odd.code = "lower case or C:\\private";
  odd.stack = "Error: x\n    at C:\\private\\outside.js:1:1";
  assert.equal(crash_reason("uncaught_exception", odd), "verifier_crash: uncaught_exception Error");
});

test("an unhandled rejection becomes a blocked result and exit code 2", () => {
  const { status, result } = crash_in_child(
    "Promise.reject(Object.assign(new Error(\"open 'C:\\\\private\\\\x'\"), { code: 'ECONNRESET' }));",
  );
  assert.equal(status, crash_result_config.blocked_exit_code);
  assert.equal(result?.status, "blocked");
  assert.match(result.blocked_reason, /^verifier_crash: unhandled_rejection Error ECONNRESET/);
  assert.ok(!result.blocked_reason.includes("private"));
});

test("run_verifier records a blocked result when its final result write fails", () => {
  const directory = mkdtempSync(path.join(os.tmpdir(), "crash_result_verifier_"));
  try {
    // The preload makes every async writeFile reject, as a busy file would;
    // the crash writer's synchronous write is unaffected.
    const preload = path.join(directory, "fail_final_write.mjs");
    writeFileSync(
      preload,
      [
        'import fs from "node:fs";',
        'import { syncBuiltinESMExports } from "node:module";',
        'fs.promises.writeFile = async () => { throw Object.assign(new Error("EBUSY: resource busy"), { code: "EBUSY" }); };',
        "syncBuiltinESMExports();",
      ].join("\n"),
    );
    const out_file = path.join(directory, "result.json");
    const verifier = path.join(path.dirname(fileURLToPath(import.meta.url)), "run_verifier.mjs");
    // A missing --browser blocks the suite at once, so the next step is the final write.
    const child = spawnSync(
      process.execPath,
      [
        "--import", pathToFileURL(preload).href, verifier,
        "--candidate", directory, "--suite", "browser",
        "--browser", path.join(directory, "missing_browser.exe"), "--out", out_file,
      ],
      { encoding: "utf8", timeout: 60_000 },
    );
    assert.equal(child.status, crash_result_config.blocked_exit_code, child.stderr);
    const result = JSON.parse(readFileSync(out_file, "utf8"));
    assert.equal(result.schema_version, 1);
    assert.equal(result.suite, "browser");
    assert.equal(result.status, "blocked");
    assert.equal(result.tests_total, 0);
    assert.equal(typeof result.node_version, "string");
    assert.match(result.blocked_reason, /^verifier_crash: uncaught_exception Error EBUSY/);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("an uncaught exception becomes a blocked result and only the first failure is recorded", () => {
  const { status, result } = crash_in_child(
    "setTimeout(() => { setTimeout(() => { throw new RangeError('second'); }); throw new TypeError('first'); });",
  );
  assert.equal(status, crash_result_config.blocked_exit_code);
  assert.match(result?.blocked_reason ?? "", /^verifier_crash: uncaught_exception TypeError/);
});
