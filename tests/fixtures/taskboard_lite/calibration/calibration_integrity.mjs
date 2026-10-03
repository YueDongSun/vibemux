// Pinned calibration inventory and source snapshot rules.

import { readFile, readdir } from "node:fs/promises";
import path from "node:path";

export const expected_mutant_suites = Object.freeze({
  blank_title_accepted: "store",
  corrupt_file_reset: "api",
  filter_broken: "browser",
  lost_concurrent_updates: "store",
  payload_limit_missing: "api",
  success_on_rejection: "browser",
  title_length_off_by_one: "api",
  wrong_delete_status: "api",
  xss_inner_html: "browser",
});

export function assert_mutant_inventory(definitions) {
  if (!Array.isArray(definitions)) {
    throw new Error("mutant definitions must be a list");
  }
  const observed = new Set();
  for (const definition of definitions) {
    const mutant_id = definition?.mutant_id;
    if (typeof mutant_id !== "string" || mutant_id.length === 0) {
      throw new Error("mutant definition lacks a valid mutant_id");
    }
    if (observed.has(mutant_id)) {
      throw new Error(`duplicate mutant ${mutant_id}`);
    }
    if (!Object.hasOwn(expected_mutant_suites, mutant_id)) {
      throw new Error(`unexpected mutant ${mutant_id}`);
    }
    const expected_suite = expected_mutant_suites[mutant_id];
    if (definition.expected_failing_suite !== expected_suite) {
      throw new Error(
        `${mutant_id}: expected suite ${expected_suite}, got ${definition.expected_failing_suite}`,
      );
    }
    observed.add(mutant_id);
  }
  const missing = Object.keys(expected_mutant_suites).filter((mutant_id) => !observed.has(mutant_id));
  if (missing.length > 0) {
    throw new Error(`mutant inventory omits ${missing.join(", ")}`);
  }
}

async function collect_source_files(snapshot, label, root_dir, relative_dir = "") {
  const directory = path.join(root_dir, relative_dir);
  const entries = await readdir(directory, { withFileTypes: true });
  for (const entry of entries) {
    const relative_path = path.join(relative_dir, entry.name);
    const source_path = path.join(root_dir, relative_path);
    if (entry.isDirectory()) {
      await collect_source_files(snapshot, label, root_dir, relative_path);
    } else if (entry.isFile()) {
      const source_bytes = await readFile(source_path);
      const normalized_path = relative_path.split(path.sep).join("/");
      snapshot[`${label}/${normalized_path}`] = source_bytes.toString("base64");
    } else {
      throw new Error(`${label}/${relative_path}: source entry is not a regular file or directory`);
    }
  }
}

export async function snapshot_source_files(source_roots) {
  const snapshot = {};
  for (const { label, root_dir } of source_roots) {
    await collect_source_files(snapshot, label, root_dir);
  }
  return snapshot;
}
