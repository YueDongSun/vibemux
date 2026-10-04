// Trusted TaskBoard Lite store-interface suite (CONTRACT.md sections 7 and 8).
//
// "Comparison mode": the same semantics the API suite checks over HTTP are
// checked here directly on `src/store.mjs`, so API and store results can be
// compared to localize a backend defect to the HTTP layer or to the store.
//
// Run by run_verifier.mjs as:
//   node --test --test-reporter=tap store_suite.test.mjs
// with TASKBOARD_CANDIDATE_DIR naming the candidate product directory. Every
// test uses a fresh data file in a fresh directory under os.tmpdir().

import assert from "node:assert/strict";
import { readFile, readdir, writeFile } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";
import { candidate_layout_from_environment, create_temp_dir, remove_temp_dir } from "./candidate_paths.mjs";

const store_suite_config = Object.freeze({
  test_timeout_ms: 60_000,
  data_file_name: "taskboard_store.json",
  concurrent_add_count: 50,
  mixed_seed_count: 20,
  mixed_add_count: 10,
  id_pattern: /^[A-Za-z0-9_-]{1,64}$/,
  forbidden_import_patterns: [
    /(^|\/)server\.mjs$/,
    /(^|\/)public\//,
    /^(node:)?(http|https|net)$/,
  ],
});

const layout = candidate_layout_from_environment();

async function load_create_store() {
  const store_module = await import(pathToFileURL(layout.store_entry).href);
  assert.equal(typeof store_module.createStore, "function", "src/store.mjs must export createStore");
  return store_module.createStore;
}

async function with_data_file(label, body) {
  const directory = await create_temp_dir(`store_${label}`);
  try {
    return await body({ directory, data_file: path.join(directory, store_suite_config.data_file_name) });
  } finally {
    await remove_temp_dir(directory);
  }
}

async function open_store(data_file) {
  const createStore = await load_create_store();
  const store = await createStore(data_file);
  for (const method_name of ["list", "add", "setCompleted", "remove"]) {
    assert.equal(typeof store?.[method_name], "function", `Store.${method_name} must be a function`);
  }
  return store;
}

// Requires a rejected promise (not a synchronous throw) with Error.code.
async function assert_rejects_with_code(invoke, code, context) {
  let outcome;
  try {
    outcome = invoke();
  } catch (error) {
    assert.fail(`${context}: threw synchronously (${error?.code ?? error}); a rejected promise is required`);
  }
  assert.ok(outcome !== null && typeof outcome?.then === "function", `${context}: must return a promise`);
  await assert.rejects(outcome, (error) => {
    assert.ok(error instanceof Error, `${context}: rejection must be an Error instance`);
    assert.equal(error.code, code, `${context}: error code`);
    return true;
  }, context);
}

function assert_task_shape(task, context) {
  assert.ok(task !== null && typeof task === "object" && !Array.isArray(task), `${context}: task must be an object`);
  assert.deepEqual(Object.keys(task).sort(), ["completed", "id", "title"], `${context}: task keys`);
  assert.match(task.id, store_suite_config.id_pattern, `${context}: id pattern`);
  assert.equal(typeof task.title, "string", `${context}: title type`);
  assert.equal(typeof task.completed, "boolean", `${context}: completed type`);
}

function extract_import_specifiers(source_text) {
  const specifiers = [];
  const patterns = [
    /\bimport\s+(?:[^'"]*?\s+from\s+)?["']([^"']+)["']/g,
    /\bexport\s+[^'"]*?\s+from\s+["']([^"']+)["']/g,
    /\bimport\s*\(\s*["']([^"']+)["']\s*\)/g,
    /\brequire\s*\(\s*["']([^"']+)["']\s*\)/g,
  ];
  for (const pattern of patterns) {
    for (const match of source_text.matchAll(pattern)) {
      specifiers.push(match[1]);
    }
  }
  return specifiers;
}

const test_options = { timeout: store_suite_config.test_timeout_ms };

test("module_exports_create_store_without_forbidden_imports", test_options, async () => {
  await load_create_store();
  const source_text = await readFile(layout.store_entry, "utf8");
  for (const specifier of extract_import_specifiers(source_text)) {
    for (const pattern of store_suite_config.forbidden_import_patterns) {
      assert.doesNotMatch(specifier, pattern, `src/store.mjs must not import ${JSON.stringify(specifier)}`);
    }
  }
});

test("missing_file_starts_empty", test_options, async () => {
  await with_data_file("missing", async ({ data_file }) => {
    const store = await open_store(data_file);
    assert.deepEqual(await store.list(), []);
  });
});

test("add_returns_trimmed_task", test_options, async () => {
  await with_data_file("add", async ({ data_file }) => {
    const store = await open_store(data_file);
    const task = await store.add("  Buy milk \t");
    assert_task_shape(task, "added task");
    assert.equal(task.title, "Buy milk");
    assert.equal(task.completed, false);
    assert.deepEqual(await store.list(), [task]);
  });
});

test("add_validation_codes", test_options, async () => {
  await with_data_file("add_validation", async ({ data_file }) => {
    const store = await open_store(data_file);
    for (const value of [5, null, undefined, {}, ["a"], true]) {
      await assert_rejects_with_code(() => store.add(value), "invalid_request", `add(${JSON.stringify(value)})`);
    }
    const invalid_titles = [
      "",
      "   ",
      "\t\n\r ",
      "\u{a0}\u{2003}\u{3000}\u{feff}",
      "c".repeat(121),
      "\u{1F600}".repeat(121),
      ` ${"d".repeat(121)} `,
      `a${"e\u{301}".repeat(60)}`,
    ];
    for (const title of invalid_titles) {
      await assert_rejects_with_code(() => store.add(title), "invalid_title", `add of ${[...title].length} code points`);
    }
    assert.deepEqual(await store.list(), [], "rejected adds must not change state");
  });
});

test("add_accepts_boundaries_and_duplicates", test_options, async () => {
  await with_data_file("add_boundaries", async ({ data_file }) => {
    const store = await open_store(data_file);
    const accepted = [
      ["a".repeat(120), "a".repeat(120)],
      ["\u{1F600}".repeat(120), "\u{1F600}".repeat(120)],
      [`  ${"b".repeat(120)}\n`, "b".repeat(120)],
      ["e\u{301}".repeat(60), "e\u{301}".repeat(60)],
      ["Same", "Same"],
      ["Same", "Same"],
    ];
    const created = [];
    for (const [raw_title, stored_title] of accepted) {
      const task = await store.add(raw_title);
      assert_task_shape(task, "accepted task");
      assert.equal(task.title, stored_title);
      created.push(task);
    }
    const listed = await store.list();
    assert.deepEqual(listed, created, "list must equal added tasks in insertion order");
    assert.equal(new Set(listed.map((task) => task.id)).size, created.length, "ids must be unique");
  });
});

test("returned_values_are_isolated_copies", test_options, async () => {
  await with_data_file("copies", async ({ data_file }) => {
    const store = await open_store(data_file);
    const added = await store.add("original");
    const snapshot = { ...added };
    added.title = "mutated through add result";
    added.completed = true;
    const listed = await store.list();
    listed[0].title = "mutated through list result";
    listed[0].completed = true;
    listed.push({ id: "injected", title: "injected", completed: false });
    listed.reverse();
    const updated = await store.setCompleted(snapshot.id, false);
    updated.title = "mutated through setCompleted result";
    assert.deepEqual(await store.list(), [snapshot], "stored state must not change through returned objects");
  });
});

test("set_completed_updates_and_validates", test_options, async () => {
  await with_data_file("set_completed", async ({ data_file }) => {
    const store = await open_store(data_file);
    const task = await store.add("toggle me");
    const other = await store.add("leave me");
    for (const completed of [true, true, false]) {
      const updated = await store.setCompleted(task.id, completed);
      assert.deepEqual(updated, { ...task, completed });
      assert.deepEqual(await store.list(), [{ ...task, completed }, other]);
    }
    for (const bad_id of [5, null, undefined, "", {}]) {
      await assert_rejects_with_code(() => store.setCompleted(bad_id, true), "invalid_request", `setCompleted(${JSON.stringify(bad_id)}, true)`);
    }
    for (const bad_completed of ["yes", 1, null, undefined, {}]) {
      await assert_rejects_with_code(() => store.setCompleted(task.id, bad_completed), "invalid_request", `setCompleted(id, ${JSON.stringify(bad_completed)})`);
    }
    await assert_rejects_with_code(() => store.setCompleted("does_not_exist", true), "not_found", "setCompleted(unknown id)");
    await assert_rejects_with_code(
      () => store.setCompleted("does_not_exist", "yes"),
      "invalid_request",
      "setCompleted(unknown id, invalid value) validates arguments before lookup",
    );
    assert.deepEqual(await store.list(), [task, other], "rejected updates must not change state");
  });
});

test("remove_deletes_and_validates", test_options, async () => {
  await with_data_file("remove", async ({ data_file }) => {
    const store = await open_store(data_file);
    const first = await store.add("remove me");
    const second = await store.add("keep me");
    assert.equal(await store.remove(first.id), undefined, "remove must resolve undefined");
    assert.deepEqual(await store.list(), [second]);
    await assert_rejects_with_code(() => store.remove(first.id), "not_found", "remove(already removed id)");
    await assert_rejects_with_code(() => store.remove("does_not_exist"), "not_found", "remove(unknown id)");
    for (const bad_id of [5, null, undefined, "", ["a"]]) {
      await assert_rejects_with_code(() => store.remove(bad_id), "invalid_request", `remove(${JSON.stringify(bad_id)})`);
    }
    assert.deepEqual(await store.list(), [second]);
  });
});

test("reopened_store_sees_persisted_state", test_options, async () => {
  await with_data_file("reopen", async ({ data_file }) => {
    const store = await open_store(data_file);
    const created = [];
    for (const title of ["alpha", "beta", "gamma", "delta"]) {
      created.push(await store.add(title));
    }
    await store.setCompleted(created[1].id, true);
    await store.remove(created[2].id);
    const expected = [created[0], { ...created[1], completed: true }, created[3]];
    assert.deepEqual(await store.list(), expected);
    const reopened = await open_store(data_file);
    assert.deepEqual(await reopened.list(), expected, "a reopened store must see every persisted mutation");
    const added = await reopened.add("epsilon");
    assert.ok(!created.some((task) => task.id === added.id), "new id must not reuse an existing id");
    const reopened_again = await open_store(data_file);
    assert.deepEqual(await reopened_again.list(), [...expected, added]);
  });
});

test("corrupt_file_rejected_unchanged", test_options, async () => {
  const variants = [
    ["truncated_json", Buffer.from("{\"tasks\":[{\"id\":\"a\",\"title\":\"x\"")],
    ["not_json", Buffer.from("this is not json\n")],
    ["binary", Buffer.from([0x00, 0xff, 0xfe, 0x80, 0x81, 0x7b])],
    ["empty", Buffer.alloc(0)],
  ];
  for (const [variant_name, corrupt_bytes] of variants) {
    await with_data_file(`corrupt_${variant_name}`, async ({ directory, data_file }) => {
      await writeFile(data_file, corrupt_bytes);
      const createStore = await load_create_store();
      await assert_rejects_with_code(() => createStore(data_file), "corrupt_store", `createStore on ${variant_name}`);
      assert.ok((await readFile(data_file)).equals(corrupt_bytes), `${variant_name}: file bytes must stay unchanged`);
      assert.deepEqual(await readdir(directory), [store_suite_config.data_file_name], `${variant_name}: no other file may be created`);
    });
  }
});

test("concurrent_adds_not_lost", test_options, async () => {
  await with_data_file("concurrent_add", async ({ directory, data_file }) => {
    const store = await open_store(data_file);
    const titles = Array.from({ length: store_suite_config.concurrent_add_count }, (_, index) => `concurrent_${index}`);
    const created = await Promise.all(titles.map((title) => store.add(title)));
    assert.equal(new Set(created.map((task) => task.id)).size, titles.length, "ids must be unique");
    const listed = await store.list();
    assert.equal(listed.length, titles.length, `expected ${titles.length} tasks, found ${listed.length}`);
    assert.deepEqual(listed.map((task) => task.title).sort(), [...titles].sort(), "every title exactly once");
    assert.deepEqual(await (await open_store(data_file)).list(), listed, "a reopened store must see every concurrent add");
    assert.deepEqual(await readdir(directory), [store_suite_config.data_file_name], "no temporary file may remain");
  });
});

test("concurrent_mixed_mutations_consistent", test_options, async () => {
  await with_data_file("concurrent_mixed", async ({ directory, data_file }) => {
    const store = await open_store(data_file);
    const seeds = [];
    for (let index = 0; index < store_suite_config.mixed_seed_count; index += 1) {
      seeds.push(await store.add(`seed_${index}`));
    }
    const is_removed = (index) => index % 4 === 3;
    const is_completed = (index) => index % 2 === 0;
    const operations = [];
    seeds.forEach((task, index) => {
      if (is_removed(index)) {
        operations.push(store.remove(task.id));
      } else if (is_completed(index)) {
        operations.push(store.setCompleted(task.id, true));
      }
    });
    for (let index = 0; index < store_suite_config.mixed_add_count; index += 1) {
      operations.push(store.add(`fresh_${index}`));
    }
    await Promise.all(operations);
    const listed = await store.list();
    const expected_seeds = seeds
      .map((task, index) => ({ task, index }))
      .filter(({ index }) => !is_removed(index))
      .map(({ task, index }) => ({ ...task, completed: is_completed(index) }));
    assert.deepEqual(listed.slice(0, expected_seeds.length), expected_seeds, "surviving seeds keep order and updates");
    const fresh_titles = listed.slice(expected_seeds.length).map((task) => task.title).sort();
    const expected_fresh = Array.from({ length: store_suite_config.mixed_add_count }, (_, index) => `fresh_${index}`).sort();
    assert.deepEqual(fresh_titles, expected_fresh, "every new task appears exactly once after the seeds");
    assert.deepEqual(await (await open_store(data_file)).list(), listed, "a reopened store must match");
    assert.deepEqual(await readdir(directory), [store_suite_config.data_file_name], "no temporary file may remain");
  });
});

test("rejected_mutations_do_not_change_persisted_state", test_options, async () => {
  await with_data_file("rejections", async ({ directory, data_file }) => {
    const store = await open_store(data_file);
    const task = await store.add("stable");
    const persisted_before = await readFile(data_file);
    await assert_rejects_with_code(() => store.add("   "), "invalid_title", "blank add");
    await assert_rejects_with_code(() => store.setCompleted("does_not_exist", true), "not_found", "unknown update");
    await assert_rejects_with_code(() => store.remove("does_not_exist"), "not_found", "unknown remove");
    assert.ok((await readFile(data_file)).equals(persisted_before), "rejected mutations must not rewrite the file");
    assert.deepEqual(await (await open_store(data_file)).list(), [task]);
    assert.deepEqual(await readdir(directory), [store_suite_config.data_file_name], "no temporary file may remain");
  });
});
