import assert from "node:assert/strict";
import { mkdtemp, rm, stat, utimes, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import {
  assert_mutant_inventory,
  expected_mutant_suites,
  snapshot_source_files,
} from "./calibration_integrity.mjs";

const expected_definitions = Object.entries(expected_mutant_suites).map(
  ([mutant_id, expected_failing_suite]) => ({ mutant_id, expected_failing_suite }),
);

test("accepts_the_exact_pinned_mutant_inventory", () => {
  assert.equal(expected_definitions.length, 9);
  assert_mutant_inventory(expected_definitions);
});

test("rejects_an_omitted_mutant", () => {
  assert.throws(() => assert_mutant_inventory(expected_definitions.slice(1)), /omits/);
});

test("rejects_a_relabelled_mutant", () => {
  const relabelled = expected_definitions.map((definition) => ({ ...definition }));
  relabelled[0].expected_failing_suite = "api";
  assert.throws(() => assert_mutant_inventory(relabelled), /expected suite store/);
});

test("rejects_duplicate_and_unknown_mutants", () => {
  assert.throws(
    () => assert_mutant_inventory([...expected_definitions, expected_definitions[0]]),
    /duplicate mutant/,
  );
  assert.throws(
    () => assert_mutant_inventory([...expected_definitions, { mutant_id: "extra", expected_failing_suite: "api" }]),
    /unexpected mutant/,
  );
});

test("source_snapshot_detects_same_length_content_change_with_restored_mtime", async () => {
  const source_root = await mkdtemp(path.join(os.tmpdir(), "vibemux_calibration_sources_"));
  const source_file = path.join(source_root, "fixture.txt");
  const fixed_time = new Date("2001-02-03T04:05:06.000Z");
  try {
    await writeFile(source_file, "alpha", "utf8");
    await utimes(source_file, fixed_time, fixed_time);
    const before_stat = await stat(source_file);
    const before = await snapshot_source_files([{ label: "fixture", root_dir: source_root }]);

    await writeFile(source_file, "bravo", "utf8");
    await utimes(source_file, fixed_time, fixed_time);
    const after_stat = await stat(source_file);
    const after = await snapshot_source_files([{ label: "fixture", root_dir: source_root }]);

    assert.equal(before_stat.size, after_stat.size);
    assert.equal(before_stat.mtimeMs, after_stat.mtimeMs);
    assert.notDeepEqual(before, after);
  } finally {
    await rm(source_root, { recursive: true, force: true });
  }
});
