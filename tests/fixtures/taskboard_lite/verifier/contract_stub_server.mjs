// Verifier-owned reference backend for the TaskBoard Lite HTTP contract.
//
// It exists only to test a frontend-only candidate in isolation
// (`--suite browser_frontend_only`): it serves the candidate's `public/`
// directory and implements CONTRACT.md sections 2 to 7 itself. It is written
// independently of calibration/ and never imports candidate code.
//
// CLI: node contract_stub_server.mjs --public_dir <dir> --host <h> --port <p> --data <file>

import { randomUUID } from "node:crypto";
import { open, readFile, rename, rm } from "node:fs/promises";
import http from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";

const stub_config = Object.freeze({
  loopback_hosts: ["127.0.0.1", "::1"],
  body_limit_bytes: 8192,
  title_limit_code_points: 120,
  discard_window_ms: 5000,
  rename_attempts: 10,
  rename_backoff_ms: 25,
  file_format_version: 1,
  assets: {
    "/": ["index.html", "text/html; charset=utf-8"],
    "/app.mjs": ["app.mjs", "text/javascript; charset=utf-8"],
    "/styles.css": ["styles.css", "text/css; charset=utf-8"],
  },
  status_by_code: {
    invalid_json: 400,
    invalid_request: 400,
    invalid_title: 400,
    not_found: 404,
    payload_too_large: 413,
    storage_failure: 500,
    internal_error: 500,
  },
});

function coded_error(code) {
  const error = new Error(code);
  error.code = code;
  return error;
}

// ------------------------------------------------------------ persistence --

function parse_store_file(bytes) {
  let document;
  try {
    document = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
  } catch {
    throw coded_error("corrupt_store");
  }
  const tasks = document?.tasks;
  const valid =
    document?.format_version === stub_config.file_format_version &&
    Array.isArray(tasks) &&
    tasks.every(
      (task) =>
        typeof task?.id === "string" && typeof task.title === "string" && typeof task.completed === "boolean",
    );
  if (!valid) {
    throw coded_error("corrupt_store");
  }
  return tasks.map(({ id, title, completed }) => ({ id, title, completed }));
}

async function load_store_file(data_file) {
  try {
    return parse_store_file(await readFile(data_file));
  } catch (error) {
    if (error.code === "ENOENT") {
      return [];
    }
    throw error.code === "corrupt_store" ? error : coded_error("storage_failure");
  }
}

async function replace_file_atomically(data_file, tasks) {
  const temp_suffix = randomUUID().replaceAll("-", "");
  const temp_path = path.join(path.dirname(data_file), `.${path.basename(data_file)}.${temp_suffix}.partial`);
  try {
    const handle = await open(temp_path, "wx");
    try {
      await handle.writeFile(JSON.stringify({ format_version: stub_config.file_format_version, tasks }));
      await handle.sync();
    } finally {
      await handle.close();
    }
    for (let attempt = 1; ; attempt += 1) {
      try {
        await rename(temp_path, data_file);
        break;
      } catch (error) {
        if (attempt >= stub_config.rename_attempts || !["EPERM", "EACCES", "EBUSY"].includes(error.code)) {
          throw error;
        }
        await new Promise((resolve) => setTimeout(resolve, stub_config.rename_backoff_ms * attempt));
      }
    }
  } catch {
    await rm(temp_path, { force: true });
    throw coded_error("storage_failure");
  }
}

function create_task_state(data_file, initial_tasks) {
  let tasks = initial_tasks;
  let queue = Promise.resolve();
  const commit = (compute) => {
    const job = queue.then(async () => {
      const { next, value } = compute(tasks);
      await replace_file_atomically(data_file, next);
      tasks = next;
      return value;
    });
    queue = job.then(
      () => undefined,
      () => undefined,
    );
    return job;
  };
  const locate = (list, id) => {
    const index = list.findIndex((task) => task.id === id);
    if (index < 0) {
      throw coded_error("not_found");
    }
    return index;
  };
  return {
    list: () => tasks.map((task) => ({ ...task })),
    add: (title) =>
      commit((list) => {
        let id = randomUUID().replaceAll("-", "");
        while (list.some((task) => task.id === id)) {
          id = randomUUID().replaceAll("-", "");
        }
        const task = { id, title, completed: false };
        return { next: [...list, task], value: { ...task } };
      }),
    set_completed: (id, completed) =>
      commit((list) => {
        const index = locate(list, id);
        const task = { ...list[index], completed };
        return { next: list.map((entry, position) => (position === index ? task : entry)), value: { ...task } };
      }),
    remove: (id) =>
      commit((list) => {
        const index = locate(list, id);
        return { next: list.filter((_, position) => position !== index), value: undefined };
      }),
  };
}

// ------------------------------------------------------------------- http --

function write_json(response, status, value) {
  const payload = Buffer.from(JSON.stringify(value));
  response.writeHead(status, { "content-type": "application/json; charset=utf-8", "content-length": payload.length });
  response.end(payload);
}

function collect_body(request) {
  return new Promise((resolve, reject) => {
    if (Number(request.headers["content-length"]) > stub_config.body_limit_bytes) {
      reject(coded_error("payload_too_large"));
      return;
    }
    const parts = [];
    let size = 0;
    const on_data = (part) => {
      size += part.length;
      if (size > stub_config.body_limit_bytes) {
        request.off("data", on_data);
        request.off("end", on_end);
        reject(coded_error("payload_too_large"));
        return;
      }
      parts.push(part);
    };
    const on_end = () => resolve(Buffer.concat(parts));
    request.on("data", on_data);
    request.on("end", on_end);
    request.on("error", reject);
  });
}

async function read_single_field(request, field, expected_type) {
  const bytes = await collect_body(request);
  let value;
  try {
    value = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
  } catch {
    throw coded_error("invalid_json");
  }
  const keys = value !== null && typeof value === "object" && !Array.isArray(value) ? Object.keys(value) : null;
  if (keys === null || keys.length !== 1 || keys[0] !== field || typeof value[field] !== expected_type) {
    throw coded_error("invalid_request");
  }
  return value[field];
}

function normalize_title(raw_title) {
  const trimmed = raw_title.trim();
  const length = [...trimmed].length;
  if (length < 1 || length > stub_config.title_limit_code_points) {
    throw coded_error("invalid_title");
  }
  return trimmed;
}

function create_handler(state, public_dir) {
  const handle = async (request, response) => {
    const target = request.url;
    const asset = Object.hasOwn(stub_config.assets, target) ? stub_config.assets[target] : undefined;
    if (request.method === "GET" && asset !== undefined) {
      let bytes;
      try {
        bytes = await readFile(path.join(public_dir, asset[0]));
      } catch {
        throw coded_error("not_found");
      }
      response.writeHead(200, { "content-type": asset[1], "content-length": bytes.length });
      response.end(bytes);
      return;
    }
    if (request.method === "GET" && target === "/health") {
      write_json(response, 200, { ok: true });
      return;
    }
    if (target === "/api/tasks" && request.method === "GET") {
      write_json(response, 200, state.list());
      return;
    }
    if (target === "/api/tasks" && request.method === "POST") {
      const title = normalize_title(await read_single_field(request, "title", "string"));
      write_json(response, 201, await state.add(title));
      return;
    }
    const id_match = /^\/api\/tasks\/([A-Za-z0-9_-]{1,64})$/.exec(target);
    if (id_match !== null && request.method === "PATCH") {
      const completed = await read_single_field(request, "completed", "boolean");
      write_json(response, 200, await state.set_completed(id_match[1], completed));
      return;
    }
    if (id_match !== null && request.method === "DELETE") {
      await state.remove(id_match[1]);
      response.writeHead(204);
      response.end();
      return;
    }
    throw coded_error("not_found");
  };
  return (request, response) => {
    handle(request, response).catch((error) => {
      if (response.headersSent) {
        response.destroy();
        return;
      }
      const code = Object.hasOwn(stub_config.status_by_code, error.code) ? error.code : "internal_error";
      if (code === "payload_too_large") {
        request.removeAllListeners("data");
        request.on("data", () => undefined);
        request.resume();
        setTimeout(() => request.socket.destroy(), stub_config.discard_window_ms).unref();
      }
      write_json(response, stub_config.status_by_code[code], { error: { code } });
    });
  };
}

// ------------------------------------------------------------------- main --

function parse_contract_argv(argv) {
  const values = {};
  for (let index = 0; index < argv.length; index += 2) {
    const name = argv[index];
    if (!["--host", "--port", "--data"].includes(name) || argv[index + 1] === undefined || name in values) {
      return { failure: "invalid_arguments" };
    }
    values[name] = argv[index + 1];
  }
  if (Object.keys(values).length !== 3) {
    return { failure: "invalid_arguments" };
  }
  if (!stub_config.loopback_hosts.includes(values["--host"])) {
    return { failure: "invalid_host" };
  }
  if (!/^\d+$/.test(values["--port"]) || Number(values["--port"]) > 65535) {
    return { failure: "invalid_arguments" };
  }
  return { host: values["--host"], port: Number(values["--port"]), data_file: values["--data"] };
}

function report_failure(code, exit_code) {
  process.stderr.write(`${JSON.stringify({ error: { code } })}\n`);
  process.exitCode = exit_code;
}

// Runs the contract server with an argv vector (without the entry path) and
// an explicit public directory.
export async function run_contract_stub_server({ argv, public_dir }) {
  const launch = parse_contract_argv(argv);
  if (launch.failure !== undefined) {
    report_failure(launch.failure, 2);
    return;
  }
  let initial_tasks;
  try {
    initial_tasks = await load_store_file(launch.data_file);
  } catch (error) {
    report_failure(error.code, 1);
    return;
  }
  const server = http.createServer(create_handler(create_task_state(launch.data_file, initial_tasks), public_dir));
  server.once("error", () => {
    report_failure("listen_failed", 1);
    server.close();
  });
  server.listen(launch.port, launch.host, () => {
    const line = JSON.stringify({ event: "listening", host: launch.host, port: server.address().port, pid: process.pid });
    process.stdout.write(`${line}\n`);
  });
}

function split_public_dir_option(argv) {
  const index = argv.indexOf("--public_dir");
  if (index === -1 || argv[index + 1] === undefined) {
    return null;
  }
  return { public_dir: path.resolve(argv[index + 1]), argv: argv.toSpliced(index, 2) };
}

const is_cli_entry = process.argv[1] !== undefined && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (is_cli_entry) {
  const split = split_public_dir_option(process.argv.slice(2));
  if (split === null) {
    report_failure("invalid_arguments", 2);
  } else {
    await run_contract_stub_server(split);
  }
}
