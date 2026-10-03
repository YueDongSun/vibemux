// Fast trusted suite for the offline workflow tests (ADR 031): every .mjs
// or .js file under src/ and public/ of the subject must parse.
//
//   node syntax_suite.mjs --candidate <dir> --suite <id> --out <result_file.json>
//
// Exit codes follow the verifier contract: 0 passed, 1 failed, 2 blocked
// (no source file to check). It proves syntax only, never behavior, so a
// test that needs a behavioral verdict uses the TaskBoard suites instead.

import { spawnSync } from "node:child_process";
import { readdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { parseArgs } from "node:util";

const suite_config = Object.freeze({
  schema_version: 1,
  source_roots: ["src", "public"],
  source_pattern: /\.(mjs|js)$/,
  exit_codes: { passed: 0, failed: 1, blocked: 2 },
});

const { values } = parseArgs({
  options: {
    candidate: { type: "string" },
    suite: { type: "string" },
    out: { type: "string" },
  },
});

function collect_sources(directory, found) {
  let entries;
  try {
    entries = readdirSync(directory, { withFileTypes: true });
  } catch {
    return;
  }
  for (const entry of entries) {
    const full_path = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      collect_sources(full_path, found);
    } else if (entry.isFile() && suite_config.source_pattern.test(entry.name)) {
      found.push(full_path);
    }
  }
}

const started_ms = Date.now();
const sources = [];
for (const root of suite_config.source_roots) {
  collect_sources(path.join(values.candidate, root), sources);
}
sources.sort();
const failed_tests = [];
for (const source of sources) {
  const result = spawnSync(process.execPath, ["--check", source], { stdio: "ignore" });
  if (result.status !== 0) {
    failed_tests.push(path.relative(values.candidate, source).split(path.sep).join("/"));
  }
}
const status = sources.length === 0 ? "blocked" : failed_tests.length > 0 ? "failed" : "passed";
writeFileSync(
  values.out,
  JSON.stringify({
    schema_version: suite_config.schema_version,
    suite: values.suite,
    status,
    tests_total: sources.length,
    tests_passed: sources.length - failed_tests.length,
    tests_failed: failed_tests.length,
    failed_tests,
    duration_ms: Date.now() - started_ms,
    node_version: process.version,
    blocked_reason: sources.length === 0 ? "no_source_files" : null,
  }),
);
process.exit(suite_config.exit_codes[status]);
