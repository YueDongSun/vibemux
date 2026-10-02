// Trusted TaskBoard Lite verifier CLI.
//
//   node run_verifier.mjs --candidate <dir> --suite <api|store|browser|browser_frontend_only>
//                         --out <result_file.json> [--browser <absolute browser path>] [--timeout_ms N]
//
// Exit codes: 0 passed, 1 failed, 2 blocked (a prerequisite is missing or the
// verifier itself failed), 3 usage error (no result file is written).
// The candidate directory is never modified; all scratch data lives in
// temporary directories under os.tmpdir() that are removed before exit.

import { writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { run_browser_suite } from "./browser_suite.mjs";
import {
  create_temp_dir,
  is_directory,
  is_file,
  kill_process_tree,
  remove_temp_dir,
  spawn_tracked,
  stdout_text,
  stderr_text,
  wait_for_exit,
} from "./candidate_paths.mjs";

const verifier_config = Object.freeze({
  schema_version: 1,
  suites: ["api", "store", "browser", "browser_frontend_only"],
  node_suite_files: { api: "api_suite.test.mjs", store: "store_suite.test.mjs" },
  default_timeout_ms: 120_000,
  runner_capture_max_bytes: 16 * 1024 * 1024,
  failure_message_max_chars: 600,
  browser_search_paths: [
    "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
    "C:/Program Files/Google/Chrome/Application/chrome.exe",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
  ],
  // Environment variables that would change how the child test runner behaves.
  stripped_environment_keys: ["NODE_TEST_CONTEXT", "NODE_OPTIONS"],
  exit_codes: { passed: 0, failed: 1, blocked: 2, usage: 3 },
});

const verifier_dir = path.dirname(fileURLToPath(import.meta.url));

const usage_text = [
  "usage: node run_verifier.mjs --candidate <dir> --suite <api|store|browser|browser_frontend_only>",
  "                             --out <result_file.json> [--browser <absolute path>] [--timeout_ms N]",
].join("\n");

class Usage_error extends Error {}

// ------------------------------------------------------------- arguments --

async function parse_cli_arguments(argv) {
  let parsed;
  try {
    parsed = parseArgs({
      args: argv,
      strict: true,
      allowPositionals: false,
      options: {
        candidate: { type: "string" },
        suite: { type: "string" },
        out: { type: "string" },
        browser: { type: "string" },
        timeout_ms: { type: "string" },
      },
    });
  } catch (error) {
    throw new Usage_error(error.message);
  }
  const { candidate, suite, out, browser, timeout_ms } = parsed.values;
  if (candidate === undefined || suite === undefined || out === undefined) {
    throw new Usage_error("--candidate, --suite, and --out are required");
  }
  if (!verifier_config.suites.includes(suite)) {
    throw new Usage_error(`--suite must be one of ${verifier_config.suites.join(", ")}`);
  }
  const candidate_dir = path.resolve(candidate);
  if (!(await is_directory(candidate_dir))) {
    throw new Usage_error(`--candidate is not a directory: ${candidate_dir}`);
  }
  const out_file = path.resolve(out);
  if (!(await is_directory(path.dirname(out_file)))) {
    throw new Usage_error(`the directory of --out does not exist: ${path.dirname(out_file)}`);
  }
  if (browser !== undefined && !path.isAbsolute(browser)) {
    throw new Usage_error("--browser must be an absolute path");
  }
  let suite_timeout_ms = verifier_config.default_timeout_ms;
  if (timeout_ms !== undefined) {
    suite_timeout_ms = Number(timeout_ms);
    if (!/^\d+$/.test(timeout_ms) || suite_timeout_ms <= 0) {
      throw new Usage_error("--timeout_ms must be a positive integer");
    }
  }
  return { candidate_dir, suite, out_file, browser, suite_timeout_ms };
}

// ------------------------------------------------------------------- tap --

function unescape_tap_name(raw_name) {
  return raw_name.replace(/\\(.)/g, (_, character) => (character === "n" ? "\n" : character));
}

function extract_yaml_error(yaml_lines) {
  const error_index = yaml_lines.findIndex((line) => /^ {2}error: /.test(line));
  if (error_index === -1) {
    return null;
  }
  const inline_value = yaml_lines[error_index].replace(/^ {2}error: /, "");
  let message = inline_value;
  if (/^\|[-+]?$/.test(inline_value)) {
    const block = [];
    for (const line of yaml_lines.slice(error_index + 1)) {
      if (!/^ {4}/.test(line) && line.trim().length > 0) {
        break;
      }
      block.push(line.slice(4));
    }
    message = block.join("\n").trim();
  } else {
    message = inline_value.replace(/^'(.*)'$/, "$1").replace(/''/g, "'");
  }
  return message.slice(0, verifier_config.failure_message_max_chars);
}

// Parses top-level test points from Node's TAP reporter output.
function parse_tap_output(tap_text) {
  const checks = [];
  let current_check = null;
  let yaml_lines = null;
  for (const line of tap_text.split(/\r?\n/)) {
    const point = /^(ok|not ok) \d+ - (.*)$/.exec(line);
    if (point !== null) {
      let raw_name = point[2];
      const directive = / # (SKIP|TODO)\b.*$/i.exec(raw_name);
      if (directive !== null) {
        raw_name = raw_name.slice(0, directive.index);
      }
      current_check = {
        name: unescape_tap_name(raw_name),
        passed: point[1] === "ok" && directive === null,
        skipped: directive !== null,
        error: null,
      };
      checks.push(current_check);
      yaml_lines = null;
      continue;
    }
    if (current_check !== null && line === "  ---") {
      yaml_lines = [];
    } else if (yaml_lines !== null && line === "  ...") {
      if (!current_check.passed && !current_check.skipped) {
        current_check.error = extract_yaml_error(yaml_lines);
      }
      yaml_lines = null;
    } else if (yaml_lines !== null) {
      yaml_lines.push(line);
    }
  }
  return checks;
}

// ---------------------------------------------------------------- suites --

async function run_node_test_suite(suite, candidate_dir, suite_timeout_ms, run_root) {
  const suite_file = path.join(verifier_dir, verifier_config.node_suite_files[suite]);
  const command = [process.execPath, "--test", "--test-reporter=tap", suite_file];
  const environment = { ...process.env, TASKBOARD_CANDIDATE_DIR: candidate_dir, TASKBOARD_VERIFIER_TEMP_ROOT: run_root };
  for (const key of verifier_config.stripped_environment_keys) {
    delete environment[key];
  }
  let tracked = null;
  try {
    tracked = spawn_tracked(command[0], command.slice(1), {
      cwd: run_root,
      env: environment,
      detached: true,
      capture_max_bytes: verifier_config.runner_capture_max_bytes,
    });
    let exit_result = await wait_for_exit(tracked, suite_timeout_ms);
    const timed_out = exit_result === null;
    if (timed_out) {
      await kill_process_tree(tracked.pid);
      exit_result = await wait_for_exit(tracked, 10_000);
    }
    const checks = parse_tap_output(stdout_text(tracked));
    const extra_failures = [];
    if (timed_out) {
      extra_failures.push(`suite_timeout_after_${suite_timeout_ms}_ms`);
    } else if (checks.length === 0) {
      extra_failures.push(`no_tests_reported (runner exit ${exit_result?.code}; ${stderr_text(tracked).slice(-300)})`);
    } else if (exit_result?.code !== 0 && checks.every((check) => check.passed || check.skipped)) {
      extra_failures.push(`test_runner_exit_${exit_result?.code}`);
    }
    return { checks, extra_failures, command, browser: null, blocked_reason: null, cleanup_errors: [] };
  } finally {
    if (tracked !== null && tracked.exit_result === null) {
      await kill_process_tree(tracked.pid);
    }
  }
}

async function resolve_browser_executable(requested_browser) {
  if (requested_browser !== undefined) {
    return (await is_file(requested_browser)) ? { executable: requested_browser } : { missing: `browser executable not found: ${requested_browser}` };
  }
  for (const candidate_path of verifier_config.browser_search_paths) {
    if (await is_file(candidate_path)) {
      return { executable: candidate_path };
    }
  }
  return { missing: `no browser executable found; searched ${verifier_config.browser_search_paths.join(", ")}` };
}

async function run_browser_mode(suite, candidate_dir, requested_browser, suite_timeout_ms, run_root) {
  const resolved = await resolve_browser_executable(requested_browser);
  if (resolved.missing !== undefined) {
    return { checks: [], extra_failures: [], command: [], browser: null, blocked_reason: resolved.missing, cleanup_errors: [] };
  }
  const outcome = await run_browser_suite({
    candidate_dir,
    mode: suite,
    browser_executable: resolved.executable,
    timeout_ms: suite_timeout_ms,
    temp_root: run_root,
  });
  return { ...outcome, extra_failures: [] };
}

// ---------------------------------------------------------------- result --

function build_result({ suite, candidate_dir, outcome, duration_ms }) {
  const checks = outcome.checks;
  const failed_names = [
    ...checks.filter((check) => !check.passed && !check.skipped).map((check) => check.name),
    ...outcome.extra_failures,
  ];
  const tests_passed = checks.filter((check) => check.passed).length;
  const tests_skipped = checks.filter((check) => check.skipped).length;
  let status = "passed";
  if (outcome.blocked_reason !== null) {
    status = "blocked";
  } else if (failed_names.length > 0 || tests_passed === 0) {
    status = "failed";
  }
  return {
    schema_version: verifier_config.schema_version,
    suite,
    status,
    tests_total: checks.length,
    tests_passed,
    tests_failed: failed_names.length,
    tests_skipped,
    failed_tests: failed_names,
    duration_ms,
    node_version: process.version,
    browser: outcome.browser,
    command: outcome.command,
    blocked_reason: outcome.blocked_reason,
    candidate_dir,
    checks: checks.map((check) => ({ name: check.name, passed: check.passed, skipped: check.skipped === true, error: check.error })),
    cleanup_errors: outcome.cleanup_errors,
  };
}

// Every scratch directory of this run (including those of a child suite that
// is killed on timeout) lives under one run root that is removed here.
async function run_suite(options) {
  const run_root = await create_temp_dir(`run_${options.suite}`);
  let outcome;
  try {
    if (options.suite === "api" || options.suite === "store") {
      outcome = await run_node_test_suite(options.suite, options.candidate_dir, options.suite_timeout_ms, run_root);
    } else {
      outcome = await run_browser_mode(options.suite, options.candidate_dir, options.browser, options.suite_timeout_ms, run_root);
    }
  } finally {
    try {
      await remove_temp_dir(run_root);
    } catch (error) {
      if (outcome !== undefined) {
        outcome.cleanup_errors.push(`temp root cleanup failed: ${error.message}`);
      }
    }
  }
  return outcome;
}

async function main() {
  let options;
  try {
    options = await parse_cli_arguments(process.argv.slice(2));
  } catch (error) {
    if (error instanceof Usage_error) {
      process.stderr.write(`run_verifier: ${error.message}\n${usage_text}\n`);
      return verifier_config.exit_codes.usage;
    }
    throw error;
  }
  const started_at = Date.now();
  let outcome;
  try {
    outcome = await run_suite(options);
  } catch (error) {
    outcome = {
      checks: [],
      extra_failures: [],
      command: [],
      browser: null,
      blocked_reason: `verifier_error: ${error?.stack ?? error}`,
      cleanup_errors: [],
    };
  }
  const result = build_result({
    suite: options.suite,
    candidate_dir: options.candidate_dir,
    outcome,
    duration_ms: Date.now() - started_at,
  });
  await writeFile(options.out_file, `${JSON.stringify(result, null, 2)}\n`, "utf8");
  process.stdout.write(
    `${result.suite}: ${result.status} (${result.tests_passed} passed, ${result.tests_failed} failed, ` +
      `${result.tests_skipped} skipped; ${result.duration_ms} ms)\n`,
  );
  return verifier_config.exit_codes[result.status];
}

process.exitCode = await main();
