// TaskBoard Lite verifier calibration.
//
//   node calibrate.mjs [--browser <absolute browser path>] [--suite_timeout_ms N]
//
// Proves that the trusted verifier accepts correct code and rejects defects:
//   - good/ must pass api, store, browser, and browser_frontend_only;
//   - every mutant (its files overlaid on a temporary copy of good/) must fail
//     its expected suite; browser mutants must also fail browser_frontend_only;
//   - base/ must fail api (failed, not blocked);
//   - the verifier-owned contract stub server must pass api (it is the
//     reference backend behind browser_frontend_only);
//   - good/, base/, and mutants/ must be unchanged afterwards.
// Prints one JSON summary on stdout (progress goes to stderr) and exits 0 only
// when every expectation holds. All candidates are temporary copies under
// os.tmpdir(); nothing in calibration/ or base/ is modified in place.

import { cp, mkdir, readdir, readFile, stat, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { parseArgs } from "node:util";
import {
  create_temp_dir,
  kill_process_tree,
  remove_temp_dir,
  spawn_tracked,
  stderr_text,
  wait_for_exit,
} from "../verifier/candidate_paths.mjs";

const calibration_config = Object.freeze({
  schema_version: 1,
  good_suites: ["api", "store", "browser", "browser_frontend_only"],
  mutant_suites: ["api", "store", "browser"],
  mutant_definition_file: "mutant.json",
  run_timeout_ms: 300_000,
  exit_codes: { all_met: 0, not_met: 1, usage: 3 },
});

const calibration_dir = path.dirname(fileURLToPath(import.meta.url));
const fixture_root = path.dirname(calibration_dir);
const paths = Object.freeze({
  good_dir: path.join(calibration_dir, "good"),
  mutants_dir: path.join(calibration_dir, "mutants"),
  base_dir: path.join(fixture_root, "base"),
  verifier_entry: path.join(fixture_root, "verifier", "run_verifier.mjs"),
  stub_entry: path.join(fixture_root, "verifier", "contract_stub_server.mjs"),
});

function log_progress(message) {
  process.stderr.write(`[calibrate] ${message}\n`);
}

// --------------------------------------------------------------- sources --

async function list_files(root_dir, relative_dir = "") {
  const files = [];
  for (const entry of await readdir(path.join(root_dir, relative_dir), { withFileTypes: true })) {
    const relative_path = path.join(relative_dir, entry.name);
    if (entry.isDirectory()) {
      files.push(...(await list_files(root_dir, relative_path)));
    } else {
      files.push(relative_path);
    }
  }
  return files.sort();
}

// Size and modification time of every source file; used to prove that
// calibration never modifies its inputs in place.
async function snapshot_sources() {
  const snapshot = {};
  for (const [label, root_dir] of [["good", paths.good_dir], ["mutants", paths.mutants_dir], ["base", paths.base_dir]]) {
    for (const relative_path of await list_files(root_dir)) {
      const info = await stat(path.join(root_dir, relative_path));
      snapshot[`${label}/${relative_path}`] = `${info.size}:${info.mtimeMs}`;
    }
  }
  return snapshot;
}

async function load_mutant_definitions() {
  const definitions = [];
  const entries = (await readdir(paths.mutants_dir, { withFileTypes: true })).filter((entry) => entry.isDirectory());
  for (const entry of entries.sort((left, right) => left.name.localeCompare(right.name))) {
    const mutant_dir = path.join(paths.mutants_dir, entry.name);
    const definition = JSON.parse(await readFile(path.join(mutant_dir, calibration_config.mutant_definition_file), "utf8"));
    const keys = Object.keys(definition).sort().join(",");
    if (keys !== "defect,expected_failing_suite,mutant_id" || definition.mutant_id !== entry.name) {
      throw new Error(`${entry.name}: mutant.json must have exactly mutant_id (equal to the directory), defect, expected_failing_suite`);
    }
    if (!calibration_config.mutant_suites.includes(definition.expected_failing_suite)) {
      throw new Error(`${entry.name}: expected_failing_suite must be one of ${calibration_config.mutant_suites.join(", ")}`);
    }
    const overlay_files = (await list_files(mutant_dir)).filter((file) => file !== calibration_config.mutant_definition_file);
    if (overlay_files.length === 0) {
      throw new Error(`${entry.name}: a mutant must contain at least one overlay file`);
    }
    for (const relative_path of overlay_files) {
      const good_bytes = await readFile(path.join(paths.good_dir, relative_path)).catch(() => null);
      if (good_bytes === null) {
        throw new Error(`${entry.name}: overlay ${relative_path} does not exist in good/`);
      }
      if (good_bytes.equals(await readFile(path.join(mutant_dir, relative_path)))) {
        throw new Error(`${entry.name}: overlay ${relative_path} is identical to good/`);
      }
    }
    definitions.push({ ...definition, mutant_dir, overlay_files });
  }
  return definitions;
}

// ------------------------------------------------------------ candidates --

async function prepare_good_copy(workspace_dir, label) {
  const candidate_dir = path.join(workspace_dir, "candidates", label);
  await cp(paths.good_dir, candidate_dir, { recursive: true });
  return candidate_dir;
}

async function prepare_mutant_candidate(workspace_dir, mutant) {
  const candidate_dir = await prepare_good_copy(workspace_dir, `mutant_${mutant.mutant_id}`);
  for (const relative_path of mutant.overlay_files) {
    await cp(path.join(mutant.mutant_dir, relative_path), path.join(candidate_dir, relative_path));
  }
  return candidate_dir;
}

async function prepare_base_candidate(workspace_dir) {
  const candidate_dir = path.join(workspace_dir, "candidates", "base");
  await cp(paths.base_dir, candidate_dir, { recursive: true });
  return candidate_dir;
}

// A good/ copy whose src/server.mjs runs the verifier-owned contract stub,
// so the API suite can check the reference backend itself.
async function prepare_stub_candidate(workspace_dir) {
  const candidate_dir = await prepare_good_copy(workspace_dir, "contract_stub");
  const wrapper_source = [
    "// Calibration wrapper: runs the verifier-owned contract stub server as the candidate server.",
    "import { fileURLToPath } from \"node:url\";",
    `import { run_contract_stub_server } from ${JSON.stringify(pathToFileURL(paths.stub_entry).href)};`,
    "",
    "await run_contract_stub_server({",
    "  argv: process.argv.slice(2),",
    "  public_dir: fileURLToPath(new URL(\"../public/\", import.meta.url)),",
    "});",
    "",
  ].join("\n");
  await writeFile(path.join(candidate_dir, "src", "server.mjs"), wrapper_source, "utf8");
  return candidate_dir;
}

// ------------------------------------------------------------- verifier --

async function run_verifier_once({ workspace_dir, candidate_dir, suite, label, options }) {
  const out_file = path.join(workspace_dir, "results", `${label}.json`);
  const args = [paths.verifier_entry, "--candidate", candidate_dir, "--suite", suite, "--out", out_file];
  if (options.browser !== undefined) {
    args.push("--browser", options.browser);
  }
  if (options.suite_timeout_ms !== undefined) {
    args.push("--timeout_ms", String(options.suite_timeout_ms));
  }
  const started_at = Date.now();
  const tracked = spawn_tracked(process.execPath, args, { cwd: workspace_dir });
  let exit_result = await wait_for_exit(tracked, calibration_config.run_timeout_ms);
  if (exit_result === null) {
    await kill_process_tree(tracked.pid);
    exit_result = await wait_for_exit(tracked, 10_000);
  }
  let result = null;
  try {
    result = JSON.parse(await readFile(out_file, "utf8"));
  } catch {
    result = null;
  }
  const outcome = {
    status: result?.status ?? "error",
    failed_tests: result?.failed_tests ?? [],
    blocked_reason: result?.blocked_reason ?? null,
    browser: result?.browser ?? null,
    exit_code: exit_result?.code ?? null,
    duration_ms: Date.now() - started_at,
  };
  if (result === null) {
    outcome.blocked_reason = `no result file; verifier stderr: ${stderr_text(tracked).slice(-400)}`;
  }
  log_progress(`${label} ${suite}: ${outcome.status} in ${outcome.duration_ms} ms`);
  return outcome;
}

// ------------------------------------------------------------------ main --

async function parse_cli_arguments(argv) {
  const { values } = parseArgs({
    args: argv,
    strict: true,
    options: { browser: { type: "string" }, suite_timeout_ms: { type: "string" } },
  });
  if (values.browser !== undefined && !path.isAbsolute(values.browser)) {
    throw new Error("--browser must be an absolute path");
  }
  if (values.suite_timeout_ms !== undefined && !/^[1-9]\d*$/.test(values.suite_timeout_ms)) {
    throw new Error("--suite_timeout_ms must be a positive integer");
  }
  return {
    browser: values.browser,
    suite_timeout_ms: values.suite_timeout_ms === undefined ? undefined : Number(values.suite_timeout_ms),
  };
}

async function run_calibration(options, workspace_dir) {
  await mkdir(path.join(workspace_dir, "results"), { recursive: true });
  const sources_before = await snapshot_sources();
  const mutants = await load_mutant_definitions();
  const run = (candidate_dir, suite, label) => run_verifier_once({ workspace_dir, candidate_dir, suite, label, options });

  const good_dir = await prepare_good_copy(workspace_dir, "good");
  const good = {};
  const browser_versions = new Set();
  for (const suite of calibration_config.good_suites) {
    const outcome = await run(good_dir, suite, `good_${suite}`);
    good[suite] = outcome.status;
    if (outcome.browser !== null) {
      browser_versions.add(`${outcome.browser.executable_name} ${outcome.browser.version}`);
    }
  }

  const mutant_results = [];
  for (const mutant of mutants) {
    const candidate_dir = await prepare_mutant_candidate(workspace_dir, mutant);
    const outcome = await run(candidate_dir, mutant.expected_failing_suite, `mutant_${mutant.mutant_id}`);
    const entry = {
      mutant_id: mutant.mutant_id,
      expected_failing_suite: mutant.expected_failing_suite,
      observed_status: outcome.status,
      expectation_met: outcome.status === "failed",
      observed_failed_tests: outcome.failed_tests,
    };
    if (mutant.expected_failing_suite === "browser") {
      const frontend_only = await run(candidate_dir, "browser_frontend_only", `mutant_${mutant.mutant_id}_frontend_only`);
      entry.frontend_only_observed_status = frontend_only.status;
      entry.frontend_only_expectation_met = frontend_only.status === "failed";
    }
    mutant_results.push(entry);
  }

  const base_api = await run(await prepare_base_candidate(workspace_dir), "api", "base");
  const stub_api = await run(await prepare_stub_candidate(workspace_dir), "api", "contract_stub");
  const sources_after = await snapshot_sources();
  const sources_unchanged = JSON.stringify(sources_before) === JSON.stringify(sources_after);

  const all_expectations_met =
    calibration_config.good_suites.every((suite) => good[suite] === "passed") &&
    mutant_results.every((entry) => entry.expectation_met && entry.frontend_only_expectation_met !== false) &&
    base_api.status === "failed" &&
    stub_api.status === "passed" &&
    sources_unchanged;
  return {
    schema_version: calibration_config.schema_version,
    good,
    mutants: mutant_results,
    base_api_status: base_api.status,
    contract_stub_api_status: stub_api.status,
    sources_unchanged,
    browsers_used: [...browser_versions],
    all_expectations_met,
  };
}

async function main() {
  let options;
  try {
    options = await parse_cli_arguments(process.argv.slice(2));
  } catch (error) {
    process.stderr.write(`calibrate: ${error.message}\nusage: node calibrate.mjs [--browser <absolute path>] [--suite_timeout_ms N]\n`);
    return calibration_config.exit_codes.usage;
  }
  const started_at = Date.now();
  const workspace_dir = await create_temp_dir("calibration");
  try {
    const summary = await run_calibration(options, workspace_dir);
    summary.duration_ms = Date.now() - started_at;
    process.stdout.write(`${JSON.stringify(summary, null, 2)}\n`);
    return summary.all_expectations_met ? calibration_config.exit_codes.all_met : calibration_config.exit_codes.not_met;
  } finally {
    await remove_temp_dir(workspace_dir);
  }
}

process.exitCode = await main();
