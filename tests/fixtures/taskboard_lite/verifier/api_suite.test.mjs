// Trusted TaskBoard Lite API and persistence suite (CONTRACT.md sections 2-7).
//
// Run by run_verifier.mjs as:
//   node --test --test-reporter=tap api_suite.test.mjs
// with TASKBOARD_CANDIDATE_DIR naming the candidate product directory. Every
// test uses a fresh temporary workspace under os.tmpdir() with separate data
// and working directories, and stops every server it starts.

import assert from "node:assert/strict";
import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import http from "node:http";
import net from "node:net";
import path from "node:path";
import test from "node:test";
import {
  candidate_layout_from_environment,
  create_temp_dir,
  describe_start_error,
  remove_temp_dir,
  spawn_tracked,
  start_listening_server,
  stderr_error_codes,
  stop_tracked_process,
  stdout_text,
  wait_for_exit,
} from "./candidate_paths.mjs";

const api_config = Object.freeze({
  test_timeout_ms: 60_000,
  request_timeout_ms: 10_000,
  startup_failure_timeout_ms: 10_000,
  data_file_name: "taskboard_data.json",
  body_limit_bytes: 8192,
  concurrent_create_count: 40,
  mixed_seed_count: 20,
  mixed_create_count: 10,
  stream_chunk_bytes: 4096,
  stream_pause_ms: 2,
  stream_cap_bytes: 2 * 1024 * 1024,
  declared_length_bytes: 65_536,
  declared_sent_bytes: 9000,
  early_response_timeout_ms: 5000,
  stdout_quiet_wait_ms: 300,
  id_pattern: /^[A-Za-z0-9_-]{1,64}$/,
});

const layout = candidate_layout_from_environment();

// ------------------------------------------------------------ workspaces --

async function with_workspace(label, body) {
  const root = await create_temp_dir(`api_${label}`);
  const workspace = {
    root,
    data_dir: path.join(root, "data"),
    cwd_dir: path.join(root, "cwd"),
    servers: [],
  };
  workspace.data_file = path.join(workspace.data_dir, api_config.data_file_name);
  workspace.start = async ({ host = "127.0.0.1" } = {}) => {
    try {
      const server = await start_listening_server({
        entry_path: layout.server_entry,
        host,
        data_file: workspace.data_file,
        cwd: workspace.cwd_dir,
      });
      workspace.servers.push(server);
      return server;
    } catch (error) {
      assert.fail(describe_start_error(error));
    }
  };
  workspace.restart = async (server) => {
    await stop_tracked_process(server.tracked);
    return workspace.start({ host: server.receipt.host });
  };
  try {
    await mkdir(workspace.data_dir);
    await mkdir(workspace.cwd_dir);
    return await body(workspace);
  } finally {
    for (const server of workspace.servers) {
      await stop_tracked_process(server.tracked);
    }
    await remove_temp_dir(root);
  }
}

// Runs the server with raw argv that must fail at startup; returns the exit
// result and captured output. A server that keeps running is stopped.
async function run_expected_startup_failure(workspace, launch_args) {
  const tracked = spawn_tracked(process.execPath, [layout.server_entry, ...launch_args], { cwd: workspace.cwd_dir });
  try {
    const exit_result = await wait_for_exit(tracked, api_config.startup_failure_timeout_ms);
    assert.notEqual(exit_result, null, `server kept running for ${launch_args.join(" ")}; stdout=${stdout_text(tracked)}`);
    assert.equal(exit_result.spawn_error, null, `spawn failed: ${exit_result.spawn_error}`);
    return { exit_result, error_codes: stderr_error_codes(tracked), stdout: stdout_text(tracked) };
  } finally {
    await stop_tracked_process(tracked);
  }
}

// ------------------------------------------------------------------ http --

function http_exchange(server, { method = "GET", request_path, headers = {}, body }) {
  return new Promise((resolve, reject) => {
    const request_headers = { ...headers };
    if (body !== undefined) {
      request_headers["content-length"] = Buffer.byteLength(body);
    }
    const request = http.request(
      {
        host: server.receipt.host,
        port: server.receipt.port,
        method,
        path: request_path,
        headers: request_headers,
        agent: false,
        timeout: api_config.request_timeout_ms,
      },
      (response) => {
        const chunks = [];
        response.on("data", (chunk) => chunks.push(chunk));
        response.on("end", () => {
          const body_bytes = Buffer.concat(chunks);
          resolve({ status: response.statusCode, headers: response.headers, body_bytes, body_text: body_bytes.toString("utf8") });
        });
        response.on("error", reject);
      },
    );
    request.on("timeout", () => request.destroy(new Error(`request timed out: ${method} ${request_path}`)));
    request.on("error", reject);
    request.end(body);
  });
}

function send_json(server, method, request_path, value) {
  return send_raw_json(server, method, request_path, JSON.stringify(value));
}

function send_raw_json(server, method, request_path, body_text) {
  return http_exchange(server, {
    method,
    request_path,
    headers: { "content-type": "application/json" },
    body: body_text,
  });
}

function describe_response(response) {
  return `status=${response.status} content-type=${response.headers["content-type"]} body=${response.body_text.slice(0, 300)}`;
}

function assert_json_content_type(response, context) {
  const media_type = String(response.headers["content-type"] ?? "").split(";")[0].trim().toLowerCase();
  assert.equal(media_type, "application/json", `${context}: expected application/json; ${describe_response(response)}`);
}

function parse_json_body(response, context) {
  assert_json_content_type(response, context);
  try {
    return JSON.parse(response.body_text);
  } catch {
    assert.fail(`${context}: body is not JSON; ${describe_response(response)}`);
  }
}

function assert_error_response(response, status, code, context) {
  assert.equal(response.status, status, `${context}: expected ${status} ${code}; ${describe_response(response)}`);
  assert.deepEqual(parse_json_body(response, context), { error: { code } }, `${context}: error body`);
}

function assert_task_shape(task, context) {
  assert.ok(task !== null && typeof task === "object" && !Array.isArray(task), `${context}: task must be an object`);
  assert.deepEqual(Object.keys(task).sort(), ["completed", "id", "title"], `${context}: task keys`);
  assert.equal(typeof task.id, "string", `${context}: id type`);
  assert.match(task.id, api_config.id_pattern, `${context}: id pattern`);
  assert.equal(typeof task.title, "string", `${context}: title type`);
  assert.equal(typeof task.completed, "boolean", `${context}: completed type`);
}

async function create_task(server, title) {
  const response = await send_json(server, "POST", "/api/tasks", { title });
  assert.equal(response.status, 201, `POST ${JSON.stringify(title)}: ${describe_response(response)}`);
  const task = parse_json_body(response, "POST /api/tasks");
  assert_task_shape(task, "created task");
  return task;
}

async function list_tasks(server) {
  const response = await http_exchange(server, { request_path: "/api/tasks" });
  assert.equal(response.status, 200, `GET /api/tasks: ${describe_response(response)}`);
  const tasks = parse_json_body(response, "GET /api/tasks");
  assert.ok(Array.isArray(tasks), "GET /api/tasks must return an array");
  for (const task of tasks) {
    assert_task_shape(task, "listed task");
  }
  return tasks;
}

function padded_json(json_text, total_bytes) {
  const padding = total_bytes - Buffer.byteLength(json_text);
  assert.ok(padding >= 0, "padding must not be negative");
  return json_text + " ".repeat(padding);
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

async function assert_only_data_file(workspace, context) {
  const entries = (await readdir(workspace.data_dir)).sort();
  assert.deepEqual(entries, [api_config.data_file_name], `${context}: data directory must contain only the data file`);
}

async function is_ipv6_loopback_available() {
  return new Promise((resolve) => {
    const probe = net.createServer();
    probe.once("error", () => resolve(false));
    probe.listen({ host: "::1", port: 0 }, () => probe.close(() => resolve(true)));
  });
}

const test_options = { timeout: api_config.test_timeout_ms };

// ---------------------------------------------------- launch and receipt --

test("receipt_valid_and_stdout_quiet", test_options, async () => {
  await with_workspace("receipt", async (workspace) => {
    const server = await workspace.start();
    assert.equal(server.receipt.host, "127.0.0.1");
    await http_exchange(server, { request_path: "/health" });
    await create_task(server, "quiet stdout");
    await list_tasks(server);
    await delay(api_config.stdout_quiet_wait_ms);
    const lines = stdout_text(server.tracked).split("\n").filter((line) => line.trim().length > 0);
    assert.equal(lines.length, 1, `stdout must contain only the receipt line; got ${JSON.stringify(lines)}`);
    assert.equal(server.tracked.stdout_total_bytes, Buffer.byteLength(stdout_text(server.tracked)));
  });
});

test("ipv6_loopback_host_accepted", test_options, async (t) => {
  if (!(await is_ipv6_loopback_available())) {
    t.skip("IPv6 loopback ::1 is not available on this machine");
    return;
  }
  await with_workspace("ipv6", async (workspace) => {
    const server = await workspace.start({ host: "::1" });
    assert.equal(server.receipt.host, "::1");
    const response = await http_exchange(server, { request_path: "/health" });
    assert.equal(response.status, 200, describe_response(response));
    assert.deepEqual(parse_json_body(response, "GET /health over ::1"), { ok: true });
  });
});

test("invalid_host_rejected", test_options, async () => {
  await with_workspace("invalid_host", async (workspace) => {
    for (const host of ["0.0.0.0", "localhost", "::"]) {
      const outcome = await run_expected_startup_failure(workspace, [
        "--host", host, "--port", "0", "--data", workspace.data_file,
      ]);
      assert.equal(outcome.exit_result.code, 2, `host ${host}: exit code`);
      assert.ok(outcome.error_codes.includes("invalid_host"), `host ${host}: stderr codes ${JSON.stringify(outcome.error_codes)}`);
      assert.equal(outcome.stdout, "", `host ${host}: stdout must be empty`);
    }
    assert.deepEqual(await readdir(workspace.data_dir), [], "no data file may be created");
  });
});

test("missing_data_argument_rejected", test_options, async () => {
  await with_workspace("missing_data", async (workspace) => {
    const outcome = await run_expected_startup_failure(workspace, ["--host", "127.0.0.1", "--port", "0"]);
    assert.equal(outcome.exit_result.code, 2, "exit code");
    assert.ok(outcome.error_codes.includes("invalid_arguments"), `stderr codes ${JSON.stringify(outcome.error_codes)}`);
    assert.equal(outcome.stdout, "", "stdout must be empty");
  });
});

// ---------------------------------------------------------- static files --

test("static_assets_served_from_module_location", test_options, async () => {
  await with_workspace("static", async (workspace) => {
    const server = await workspace.start();
    const expected_assets = [
      ["/", "index.html", "text/html;charset=utf-8"],
      ["/app.mjs", "app.mjs", "text/javascript;charset=utf-8"],
      ["/styles.css", "styles.css", "text/css;charset=utf-8"],
    ];
    for (const [request_path, file_name, content_type] of expected_assets) {
      const file_bytes = await readFile(path.join(layout.public_dir, file_name));
      const response = await http_exchange(server, { request_path });
      assert.equal(response.status, 200, `GET ${request_path}: ${describe_response(response)}`);
      const actual_type = String(response.headers["content-type"] ?? "").toLowerCase().replace(/\s+/g, "");
      assert.equal(actual_type, content_type, `GET ${request_path}: content-type`);
      assert.ok(response.body_bytes.equals(file_bytes), `GET ${request_path}: body must equal public/${file_name}`);
    }
  });
});

test("traversal_and_unlisted_paths_return_not_found", test_options, async () => {
  await with_workspace("traversal", async (workspace) => {
    const server = await workspace.start();
    const blocked_paths = [
      "/../package.json",
      "/%2e%2e/package.json",
      "/%2E%2E/package.json",
      "/..%2fsrc%2fstore.mjs",
      "/..%5cpackage.json",
      "/%2e%2e%2fpackage.json",
      "/src/store.mjs",
      "/src/server.mjs",
      "/public/../package.json",
      "/app.mjs/../../package.json",
      "/package.json",
      "/CONTRACT.md",
      "/index.html",
      "/public/index.html",
      "/public/app.mjs",
      "/api/unknown",
      "/healthz",
    ];
    for (const request_path of blocked_paths) {
      const response = await http_exchange(server, { request_path });
      assert_error_response(response, 404, "not_found", `GET ${request_path}`);
    }
  });
});

test("unlisted_methods_return_not_found", test_options, async () => {
  await with_workspace("methods", async (workspace) => {
    const server = await workspace.start();
    const task = await create_task(server, "method probe");
    const requests = [
      ["POST", "/health", "{}"],
      ["PUT", "/api/tasks", JSON.stringify({ title: "put" })],
      ["DELETE", "/api/tasks", undefined],
      ["PATCH", "/api/tasks", JSON.stringify({ completed: true })],
      ["GET", `/api/tasks/${task.id}`, undefined],
      ["GET", "/api/tasks/", undefined],
      ["POST", `/api/tasks/${task.id}`, JSON.stringify({ title: "nested" })],
      ["POST", "/", JSON.stringify({ title: "root" })],
    ];
    for (const [method, request_path, body] of requests) {
      const response = await http_exchange(server, { method, request_path, body });
      assert_error_response(response, 404, "not_found", `${method} ${request_path}`);
    }
    assert.deepEqual(await list_tasks(server), [task], "unlisted requests must not change state");
  });
});

// ----------------------------------------------------------------- tasks --

test("health_returns_ok", test_options, async () => {
  await with_workspace("health", async (workspace) => {
    const server = await workspace.start();
    const response = await http_exchange(server, { request_path: "/health" });
    assert.equal(response.status, 200, describe_response(response));
    assert.deepEqual(parse_json_body(response, "GET /health"), { ok: true });
  });
});

test("missing_data_file_starts_empty", test_options, async () => {
  await with_workspace("missing_file", async (workspace) => {
    const server = await workspace.start();
    assert.deepEqual(await list_tasks(server), []);
  });
});

test("create_task_returns_trimmed_task", test_options, async () => {
  await with_workspace("create", async (workspace) => {
    const server = await workspace.start();
    const task = await create_task(server, "  Buy milk \t");
    assert.equal(task.title, "Buy milk");
    assert.equal(task.completed, false);
    assert.deepEqual(await list_tasks(server), [task]);
  });
});

test("list_preserves_insertion_order_and_unique_ids", test_options, async () => {
  await with_workspace("order", async (workspace) => {
    const server = await workspace.start();
    const titles = ["first", "second", "Same", "Same", "fifth"];
    const created = [];
    for (const title of titles) {
      created.push(await create_task(server, title));
    }
    const listed = await list_tasks(server);
    assert.deepEqual(listed, created, "list must equal created tasks in insertion order");
    assert.equal(new Set(listed.map((task) => task.id)).size, titles.length, "ids must be unique");
  });
});

test("title_rule_boundaries", test_options, async () => {
  await with_workspace("titles", async (workspace) => {
    const server = await workspace.start();
    const accepted = [
      ["a".repeat(120), "a".repeat(120)],
      ["\u{1F600}".repeat(120), "\u{1F600}".repeat(120)],
      [`  ${"b".repeat(120)}\t\n`, "b".repeat(120)],
      ["e\u{301}".repeat(60), "e\u{301}".repeat(60)],
      ["\u{a0}x\u{2003}", "x"],
    ];
    for (const [raw_title, stored_title] of accepted) {
      const task = await create_task(server, raw_title);
      assert.equal(task.title, stored_title, `stored title for ${JSON.stringify(raw_title.slice(0, 12))}`);
    }
    const rejected = [
      "",
      "   ",
      "\t\n\r ",
      "\u{a0}\u{2003}\u{3000}\u{feff}",
      "c".repeat(121),
      "\u{1F600}".repeat(121),
      ` ${"d".repeat(121)} `,
      `a${"e\u{301}".repeat(60)}`,
    ];
    for (const raw_title of rejected) {
      const response = await send_json(server, "POST", "/api/tasks", { title: raw_title });
      assert_error_response(response, 400, "invalid_title", `POST title of ${[...raw_title].length} code points`);
    }
    assert.equal((await list_tasks(server)).length, accepted.length, "only accepted titles may be stored");
  });
});

test("post_rejects_malformed_json", test_options, async () => {
  await with_workspace("malformed_post", async (workspace) => {
    const server = await workspace.start();
    for (const body_text of ["{", "{\"title\":", "", "not json", "{'title':'x'}", "{\"title\":\"x\"} trailing"]) {
      const response = await send_raw_json(server, "POST", "/api/tasks", body_text);
      assert_error_response(response, 400, "invalid_json", `POST body ${JSON.stringify(body_text)}`);
    }
    assert.deepEqual(await list_tasks(server), []);
  });
});

test("post_rejects_invalid_shapes", test_options, async () => {
  await with_workspace("shape_post", async (workspace) => {
    const server = await workspace.start();
    const bodies = [
      "[]",
      "[\"title\"]",
      "\"title\"",
      "null",
      "42",
      "true",
      "{}",
      "{\"title\":\"a\",\"completed\":false}",
      "{\"title\":5}",
      "{\"title\":null}",
      "{\"title\":true}",
      "{\"title\":[\"a\"]}",
      "{\"title\":{\"text\":\"a\"}}",
      "{\"Title\":\"a\"}",
      "{\"__proto__\":{\"x\":1},\"title\":\"a\"}",
    ];
    for (const body_text of bodies) {
      const response = await send_raw_json(server, "POST", "/api/tasks", body_text);
      assert_error_response(response, 400, "invalid_request", `POST body ${body_text}`);
    }
    assert.deepEqual(await list_tasks(server), []);
  });
});

test("patch_updates_completed", test_options, async () => {
  await with_workspace("patch", async (workspace) => {
    const server = await workspace.start();
    const task = await create_task(server, "toggle me");
    const other = await create_task(server, "leave me");
    for (const completed of [true, true, false]) {
      const response = await send_json(server, "PATCH", `/api/tasks/${task.id}`, { completed });
      assert.equal(response.status, 200, `PATCH completed=${completed}: ${describe_response(response)}`);
      assert.deepEqual(parse_json_body(response, "PATCH"), { ...task, completed });
      assert.deepEqual(await list_tasks(server), [{ ...task, completed }, other]);
    }
  });
});

test("patch_rejects_invalid_requests", test_options, async () => {
  await with_workspace("patch_invalid", async (workspace) => {
    const server = await workspace.start();
    const task = await create_task(server, "stable");
    const task_path = `/api/tasks/${task.id}`;
    const invalid_shapes = [
      "{}",
      "{\"completed\":\"true\"}",
      "{\"completed\":1}",
      "{\"completed\":null}",
      "{\"completed\":true,\"title\":\"x\"}",
      "[]",
      "true",
      "null",
    ];
    for (const body_text of invalid_shapes) {
      const response = await send_raw_json(server, "PATCH", task_path, body_text);
      assert_error_response(response, 400, "invalid_request", `PATCH body ${body_text}`);
    }
    for (const body_text of ["{", ""]) {
      const response = await send_raw_json(server, "PATCH", task_path, body_text);
      assert_error_response(response, 400, "invalid_json", `PATCH body ${JSON.stringify(body_text)}`);
    }
    const unknown_valid = await send_json(server, "PATCH", "/api/tasks/does_not_exist", { completed: true });
    assert_error_response(unknown_valid, 404, "not_found", "PATCH unknown id");
    const unknown_invalid = await send_raw_json(server, "PATCH", "/api/tasks/does_not_exist", "{}");
    assert_error_response(unknown_invalid, 400, "invalid_request", "PATCH unknown id with invalid body (validation precedes lookup)");
    assert.deepEqual(await list_tasks(server), [task], "rejected PATCH requests must not change state");
  });
});

test("delete_returns_204_and_removes", test_options, async () => {
  await with_workspace("delete", async (workspace) => {
    const server = await workspace.start();
    const first = await create_task(server, "remove me");
    const second = await create_task(server, "keep me");
    const response = await http_exchange(server, { method: "DELETE", request_path: `/api/tasks/${first.id}` });
    assert.equal(response.status, 204, `DELETE: ${describe_response(response)}`);
    assert.equal(response.body_bytes.length, 0, "204 must have an empty body");
    assert.deepEqual(await list_tasks(server), [second]);
    const repeated = await http_exchange(server, { method: "DELETE", request_path: `/api/tasks/${first.id}` });
    assert_error_response(repeated, 404, "not_found", "DELETE already deleted id");
    const unknown = await http_exchange(server, { method: "DELETE", request_path: "/api/tasks/does_not_exist" });
    assert_error_response(unknown, 404, "not_found", "DELETE unknown id");
  });
});

// --------------------------------------------------------- payload limit --

test("payload_limit_boundaries", test_options, async () => {
  await with_workspace("payload", async (workspace) => {
    const server = await workspace.start();
    const exact_body = padded_json(JSON.stringify({ title: "exact" }), api_config.body_limit_bytes);
    const exact_response = await send_raw_json(server, "POST", "/api/tasks", exact_body);
    assert.equal(exact_response.status, 201, `8192-byte body must be accepted: ${describe_response(exact_response)}`);
    const exact_task = parse_json_body(exact_response, "POST 8192 bytes");
    const oversized_post = padded_json(JSON.stringify({ title: "over" }), api_config.body_limit_bytes + 1);
    assert_error_response(await send_raw_json(server, "POST", "/api/tasks", oversized_post), 413, "payload_too_large", "POST 8193 bytes");
    const large_title_post = JSON.stringify({ title: "y".repeat(9000) });
    assert_error_response(await send_raw_json(server, "POST", "/api/tasks", large_title_post), 413, "payload_too_large", "POST 9000-byte title");
    const oversized_patch = padded_json(JSON.stringify({ completed: true }), api_config.body_limit_bytes + 1);
    assert_error_response(
      await send_raw_json(server, "PATCH", `/api/tasks/${exact_task.id}`, oversized_patch),
      413,
      "payload_too_large",
      "PATCH 8193 bytes",
    );
    assert.deepEqual(await list_tasks(server), [exact_task], "rejected oversized requests must not change state");
  });
});

// Sends a request whose body never completes on its own; resolves with the
// response status/body if the server answers before the request ends.
function exchange_streamed_body(server, { declared_length }) {
  return new Promise((resolve, reject) => {
    const outcome = { response: null, bytes_written: 0, ended_request: false };
    const headers = { "content-type": "application/json" };
    if (declared_length !== undefined) {
      headers["content-length"] = declared_length;
    }
    const request = http.request({
      host: server.receipt.host,
      port: server.receipt.port,
      method: "POST",
      path: "/api/tasks",
      headers,
      agent: false,
    });
    let finished = false;
    const finish = (error) => {
      if (finished) {
        return;
      }
      finished = true;
      clearTimeout(overall_timer);
      request.destroy();
      if (error !== undefined && outcome.response === null) {
        reject(error);
      } else {
        resolve(outcome);
      }
    };
    // A declared oversized length must be answered after at most 9000 bytes;
    // a chunked stream may need time to push up to stream_cap_bytes first.
    const response_limit_ms =
      declared_length === undefined ? api_config.request_timeout_ms * 2 : api_config.early_response_timeout_ms;
    const overall_timer = setTimeout(
      () => finish(new Error(`no response to the streamed request within ${response_limit_ms} ms`)),
      response_limit_ms,
    );
    request.on("response", (response) => {
      const chunks = [];
      outcome.response = { status: response.statusCode, headers: response.headers, ended_request_first: outcome.ended_request };
      response.on("data", (chunk) => chunks.push(chunk));
      response.on("end", () => {
        outcome.response.body_text = Buffer.concat(chunks).toString("utf8");
        finish();
      });
      response.on("error", () => finish());
    });
    request.on("error", (error) => finish(error));
    const write_loop = async () => {
      const opening = Buffer.from("{\"title\":\"");
      request.write(opening);
      outcome.bytes_written += opening.length;
      const chunk = Buffer.alloc(api_config.stream_chunk_bytes, "x");
      const write_limit = declared_length === undefined ? api_config.stream_cap_bytes : api_config.declared_sent_bytes;
      while (outcome.response === null && !request.destroyed && outcome.bytes_written < write_limit) {
        const slice = chunk.subarray(0, Math.min(chunk.length, write_limit - outcome.bytes_written));
        request.write(slice);
        outcome.bytes_written += slice.length;
        await delay(api_config.stream_pause_ms);
      }
      if (declared_length === undefined && outcome.response === null && !request.destroyed) {
        outcome.ended_request = true;
        request.end("\"}");
      }
    };
    write_loop().catch((error) => finish(error));
  });
}

test("payload_limit_enforced_while_reading", test_options, async () => {
  await with_workspace("payload_stream", async (workspace) => {
    const server = await workspace.start();
    const declared = await exchange_streamed_body(server, { declared_length: api_config.declared_length_bytes });
    assert.notEqual(declared.response, null, "declared-length request got no response");
    assert.equal(declared.response.status, 413, `declared ${api_config.declared_length_bytes} bytes but sent ${declared.bytes_written}: status`);
    assert.deepEqual(JSON.parse(declared.response.body_text), { error: { code: "payload_too_large" } });

    const streamed = await exchange_streamed_body(server, { declared_length: undefined });
    assert.notEqual(streamed.response, null, "chunked request got no response");
    assert.equal(
      streamed.response.ended_request_first,
      false,
      `server answered only after the whole ${streamed.bytes_written}-byte chunked body was sent (limit not enforced while reading)`,
    );
    assert.equal(streamed.response.status, 413, "chunked oversized body status");
    assert.deepEqual(JSON.parse(streamed.response.body_text), { error: { code: "payload_too_large" } });

    assert.deepEqual(await list_tasks(server), [], "oversized requests must not create tasks");
  });
});

// ----------------------------------------------------------- persistence --

test("restart_preserves_tasks_order_and_ids", test_options, async () => {
  await with_workspace("restart", async (workspace) => {
    const first_server = await workspace.start();
    const created = [];
    for (const title of ["alpha", "beta", "gamma", "delta"]) {
      created.push(await create_task(first_server, title));
    }
    const patched = await send_json(first_server, "PATCH", `/api/tasks/${created[1].id}`, { completed: true });
    assert.equal(patched.status, 200, describe_response(patched));
    const deleted = await http_exchange(first_server, { method: "DELETE", request_path: `/api/tasks/${created[2].id}` });
    assert.equal(deleted.status, 204, describe_response(deleted));
    const before_restart = await list_tasks(first_server);
    assert.deepEqual(before_restart, [created[0], { ...created[1], completed: true }, created[3]]);

    const second_server = await workspace.restart(first_server);
    assert.deepEqual(await list_tasks(second_server), before_restart, "restart must preserve tasks, order, ids");
    const added = await create_task(second_server, "epsilon");
    assert.ok(!created.some((task) => task.id === added.id), "new id must not reuse an existing id");
    assert.deepEqual(await list_tasks(second_server), [...before_restart, added]);
  });
});

test("mutations_durable_before_response", test_options, async () => {
  await with_workspace("durable", async (workspace) => {
    let server = await workspace.start();
    const task = await create_task(server, "durable");
    server = await workspace.restart(server);
    assert.deepEqual(await list_tasks(server), [task], "POST must be durable before its response");

    const patched = await send_json(server, "PATCH", `/api/tasks/${task.id}`, { completed: true });
    assert.equal(patched.status, 200, describe_response(patched));
    server = await workspace.restart(server);
    assert.deepEqual(await list_tasks(server), [{ ...task, completed: true }], "PATCH must be durable before its response");

    const deleted = await http_exchange(server, { method: "DELETE", request_path: `/api/tasks/${task.id}` });
    assert.equal(deleted.status, 204, describe_response(deleted));
    server = await workspace.restart(server);
    assert.deepEqual(await list_tasks(server), [], "DELETE must be durable before its response");
  });
});

test("corrupt_data_file_fails_startup_unchanged", test_options, async () => {
  const variants = [
    ["truncated_json", Buffer.from("{\"tasks\":[{\"id\":\"a\",\"title\":\"x\"")],
    ["not_json", Buffer.from("this is not json\n")],
    ["binary", Buffer.from([0x00, 0xff, 0xfe, 0x80, 0x81, 0x7b])],
    ["empty", Buffer.alloc(0)],
  ];
  for (const [variant_name, corrupt_bytes] of variants) {
    await with_workspace(`corrupt_${variant_name}`, async (workspace) => {
      await writeFile(workspace.data_file, corrupt_bytes);
      const outcome = await run_expected_startup_failure(workspace, [
        "--host", "127.0.0.1", "--port", "0", "--data", workspace.data_file,
      ]);
      assert.notEqual(outcome.exit_result.code, 0, `${variant_name}: exit code must be non-zero`);
      assert.equal(outcome.stdout, "", `${variant_name}: no listening receipt may be printed`);
      assert.ok(outcome.error_codes.includes("corrupt_store"), `${variant_name}: stderr codes ${JSON.stringify(outcome.error_codes)}`);
      assert.ok((await readFile(workspace.data_file)).equals(corrupt_bytes), `${variant_name}: file bytes must stay unchanged`);
      await assert_only_data_file(workspace, variant_name);
    });
  }
});

test("concurrent_creates_not_lost", test_options, async () => {
  await with_workspace("concurrent_create", async (workspace) => {
    let server = await workspace.start();
    const titles = Array.from({ length: api_config.concurrent_create_count }, (_, index) => `concurrent_${index}`);
    const responses = await Promise.all(titles.map((title) => send_json(server, "POST", "/api/tasks", { title })));
    for (const response of responses) {
      assert.equal(response.status, 201, describe_response(response));
    }
    const created = responses.map((response) => JSON.parse(response.body_text));
    assert.equal(new Set(created.map((task) => task.id)).size, titles.length, "created ids must be unique");
    const listed = await list_tasks(server);
    assert.equal(listed.length, titles.length, `expected ${titles.length} tasks, found ${listed.length}`);
    assert.deepEqual(listed.map((task) => task.title).sort(), [...titles].sort(), "every title exactly once");
    assert.deepEqual(new Set(listed.map((task) => task.id)), new Set(created.map((task) => task.id)));
    await assert_only_data_file(workspace, "after concurrent creates");
    server = await workspace.restart(server);
    assert.deepEqual(await list_tasks(server), listed, "restart must preserve concurrent creates");
  });
});

test("concurrent_mixed_mutations_consistent", test_options, async () => {
  await with_workspace("concurrent_mixed", async (workspace) => {
    let server = await workspace.start();
    const seeds = [];
    for (let index = 0; index < api_config.mixed_seed_count; index += 1) {
      seeds.push(await create_task(server, `seed_${index}`));
    }
    const is_deleted = (index) => index % 4 === 3;
    const is_patched = (index) => index % 2 === 0;
    const operations = [];
    seeds.forEach((task, index) => {
      if (is_deleted(index)) {
        operations.push(http_exchange(server, { method: "DELETE", request_path: `/api/tasks/${task.id}` }).then((r) => [204, r]));
      } else if (is_patched(index)) {
        operations.push(send_json(server, "PATCH", `/api/tasks/${task.id}`, { completed: true }).then((r) => [200, r]));
      }
    });
    for (let index = 0; index < api_config.mixed_create_count; index += 1) {
      operations.push(send_json(server, "POST", "/api/tasks", { title: `fresh_${index}` }).then((r) => [201, r]));
    }
    for (const [expected_status, response] of await Promise.all(operations)) {
      assert.equal(response.status, expected_status, describe_response(response));
    }
    const listed = await list_tasks(server);
    const expected_seeds = seeds
      .map((task, index) => ({ task, index }))
      .filter(({ index }) => !is_deleted(index))
      .map(({ task, index }) => ({ ...task, completed: is_patched(index) }));
    assert.deepEqual(listed.slice(0, expected_seeds.length), expected_seeds, "surviving seeds keep order and updates");
    const fresh_titles = listed.slice(expected_seeds.length).map((task) => task.title).sort();
    const expected_fresh = Array.from({ length: api_config.mixed_create_count }, (_, index) => `fresh_${index}`).sort();
    assert.deepEqual(fresh_titles, expected_fresh, "every new task appears exactly once after the seeds");
    await assert_only_data_file(workspace, "after mixed mutations");
    server = await workspace.restart(server);
    assert.deepEqual(await list_tasks(server), listed, "restart must preserve mixed mutations");
  });
});

test("no_leftover_temporary_files", test_options, async () => {
  await with_workspace("temp_files", async (workspace) => {
    const server = await workspace.start();
    const tasks = [];
    for (const title of ["one", "two", "three"]) {
      tasks.push(await create_task(server, title));
    }
    await send_json(server, "PATCH", `/api/tasks/${tasks[0].id}`, { completed: true });
    await http_exchange(server, { method: "DELETE", request_path: `/api/tasks/${tasks[1].id}` });
    await send_json(server, "POST", "/api/tasks", { title: "   " });
    await send_json(server, "PATCH", "/api/tasks/does_not_exist", { completed: true });
    await delay(api_config.stdout_quiet_wait_ms);
    await assert_only_data_file(workspace, "after mutations");
  });
});
