// TaskBoard Lite HTTP server: calibration reference implementation
// (CONTRACT.md sections 2 to 6). Parses the launch argv, opens the store,
// listens on a loopback host, prints one listening receipt, and serves the
// JSON API plus three static assets resolved from this module's location.

import { readFile } from "node:fs/promises";
import http from "node:http";
import { createStore } from "./store.mjs";

const server_config = Object.freeze({
  allowed_hosts: new Set(["127.0.0.1", "::1"]),
  max_body_bytes: 8192,
  oversized_drain_timeout_ms: 5000,
  public_dir_url: new URL("../public/", import.meta.url),
  json_content_type: "application/json; charset=utf-8",
  static_routes: new Map([
    ["/", { file_name: "index.html", content_type: "text/html; charset=utf-8" }],
    ["/app.mjs", { file_name: "app.mjs", content_type: "text/javascript; charset=utf-8" }],
    ["/styles.css", { file_name: "styles.css", content_type: "text/css; charset=utf-8" }],
  ]),
  task_path_pattern: /^\/api\/tasks\/([A-Za-z0-9_-]{1,64})$/,
  error_status_by_code: new Map([
    ["invalid_json", 400],
    ["invalid_request", 400],
    ["invalid_title", 400],
    ["not_found", 404],
    ["payload_too_large", 413],
    ["storage_failure", 500],
  ]),
});

class Request_error extends Error {
  constructor(code) {
    super(code);
    this.code = code;
  }
}

// ---------------------------------------------------------------- launch --

function parse_launch_arguments(argv_list) {
  const known_options = new Set(["--host", "--port", "--data"]);
  const options = new Map();
  for (let index = 0; index < argv_list.length; index += 2) {
    const option_name = argv_list[index];
    const option_value = argv_list[index + 1];
    if (!known_options.has(option_name) || option_value === undefined || options.has(option_name)) {
      return { error_code: "invalid_arguments" };
    }
    options.set(option_name, option_value);
  }
  if (options.size !== known_options.size) {
    return { error_code: "invalid_arguments" };
  }
  const host = options.get("--host");
  if (!server_config.allowed_hosts.has(host)) {
    return { error_code: "invalid_host" };
  }
  const port_text = options.get("--port");
  const port = Number(port_text);
  if (!/^\d{1,5}$/.test(port_text) || port > 65535) {
    return { error_code: "invalid_arguments" };
  }
  return { host, port, data_file: options.get("--data") };
}

// Sets the exit code instead of calling process.exit so the stderr line is
// flushed before the process ends (pipe writes can be asynchronous).
function fail_startup(code, exit_code) {
  process.stderr.write(`${JSON.stringify({ error: { code } })}\n`);
  process.exitCode = exit_code;
}

// ------------------------------------------------------------- responses --

function send_json(response, status, value) {
  const body = Buffer.from(JSON.stringify(value), "utf8");
  response.writeHead(status, {
    "content-type": server_config.json_content_type,
    "content-length": body.length,
  });
  response.end(body);
}

function send_error(response, code) {
  send_json(response, server_config.error_status_by_code.get(code) ?? 500, { error: { code } });
}

function send_no_content(response) {
  response.writeHead(204);
  response.end();
}

// ---------------------------------------------------------- request body --

// Reads at most `max_body_bytes`; rejects with payload_too_large as soon as the
// declared or received size exceeds the limit, without buffering the excess.
function read_limited_body(request) {
  return new Promise((resolve, reject) => {
    const declared_length = Number(request.headers["content-length"]);
    if (Number.isFinite(declared_length) && declared_length > server_config.max_body_bytes) {
      reject(new Request_error("payload_too_large"));
      return;
    }
    const chunks = [];
    let received_bytes = 0;
    let settled = false;
    request.on("data", (chunk) => {
      if (settled) {
        return;
      }
      received_bytes += chunk.length;
      if (received_bytes > server_config.max_body_bytes) {
        settled = true;
        chunks.length = 0;
        reject(new Request_error("payload_too_large"));
        return;
      }
      chunks.push(chunk);
    });
    request.on("end", () => {
      if (!settled) {
        settled = true;
        resolve(Buffer.concat(chunks));
      }
    });
    request.on("error", (error) => {
      if (!settled) {
        settled = true;
        reject(error);
      }
    });
  });
}

// After a 413 the rest of the request is discarded (never buffered) for a
// bounded time so the client can read the response before the socket closes.
function discard_remaining_body(request) {
  request.removeAllListeners("data");
  request.on("data", () => undefined);
  request.resume();
  const drain_timer = setTimeout(() => request.socket.destroy(), server_config.oversized_drain_timeout_ms);
  drain_timer.unref();
  request.once("end", () => clearTimeout(drain_timer));
  request.once("close", () => clearTimeout(drain_timer));
}

function parse_json_body(body_bytes) {
  try {
    const text = new TextDecoder("utf-8", { fatal: true }).decode(body_bytes);
    return JSON.parse(text);
  } catch {
    throw new Request_error("invalid_json");
  }
}

function has_exact_shape(value, key_name, value_type) {
  return (
    value !== null &&
    typeof value === "object" &&
    !Array.isArray(value) &&
    Object.keys(value).length === 1 &&
    Object.hasOwn(value, key_name) &&
    typeof value[key_name] === value_type
  );
}

async function read_json_object(request, key_name, value_type) {
  const body_value = parse_json_body(await read_limited_body(request));
  if (!has_exact_shape(body_value, key_name, value_type)) {
    throw new Request_error("invalid_request");
  }
  return body_value[key_name];
}

// ---------------------------------------------------------------- routes --

async function serve_static_asset(response, static_route) {
  let file_bytes;
  try {
    file_bytes = await readFile(new URL(static_route.file_name, server_config.public_dir_url));
  } catch (error) {
    throw new Request_error(error.code === "ENOENT" ? "not_found" : "internal_error");
  }
  response.writeHead(200, { "content-type": static_route.content_type, "content-length": file_bytes.length });
  response.end(file_bytes);
}

async function route_request(store, request, response) {
  const request_path = request.url;
  const method = request.method;
  const static_route = server_config.static_routes.get(request_path);
  if (static_route !== undefined && method === "GET") {
    await serve_static_asset(response, static_route);
    return;
  }
  if (request_path === "/health" && method === "GET") {
    send_json(response, 200, { ok: true });
    return;
  }
  if (request_path === "/api/tasks" && method === "GET") {
    send_json(response, 200, await store.list());
    return;
  }
  if (request_path === "/api/tasks" && method === "POST") {
    const title = await read_json_object(request, "title", "string");
    send_json(response, 201, await store.add(title));
    return;
  }
  const task_path_match = server_config.task_path_pattern.exec(request_path);
  if (task_path_match !== null && method === "PATCH") {
    const completed = await read_json_object(request, "completed", "boolean");
    send_json(response, 200, await store.setCompleted(task_path_match[1], completed));
    return;
  }
  if (task_path_match !== null && method === "DELETE") {
    await store.remove(task_path_match[1]);
    send_no_content(response);
    return;
  }
  throw new Request_error("not_found");
}

function create_request_handler(store) {
  return async (request, response) => {
    try {
      await route_request(store, request, response);
    } catch (error) {
      if (response.headersSent) {
        response.destroy();
        return;
      }
      const code = server_config.error_status_by_code.has(error.code) ? error.code : "internal_error";
      if (code === "payload_too_large") {
        discard_remaining_body(request);
      }
      send_error(response, code);
    }
  };
}

// ------------------------------------------------------------------ main --

async function main() {
  const launch = parse_launch_arguments(process.argv.slice(2));
  if (launch.error_code !== undefined) {
    fail_startup(launch.error_code, 2);
    return;
  }
  let store;
  try {
    store = await createStore(launch.data_file);
  } catch (error) {
    fail_startup(error.code === "corrupt_store" ? "corrupt_store" : "storage_failure", 1);
    return;
  }
  const server = http.createServer(create_request_handler(store));
  server.once("error", () => {
    fail_startup("listen_failed", 1);
    server.close();
  });
  server.listen({ host: launch.host, port: launch.port }, () => {
    const receipt = { event: "listening", host: launch.host, port: server.address().port, pid: process.pid };
    process.stdout.write(`${JSON.stringify(receipt)}\n`);
  });
}

await main();
