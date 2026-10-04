# TaskBoard Lite product contract

Contract version: 1 (frozen). This file is immutable for the whole acceptance
run: workers implement it and must not edit it. A trusted verifier that lives
outside the worker-owned paths tests the behavior described here. "MUST" and
"MUST NOT" are normative. Paragraphs starting with "Note:" are not.

## 1. Product, runtime, and ownership

TaskBoard Lite is a single-user task list: a loopback HTTP server with a JSON
API, a file-backed store, and a static browser frontend.

- Runtime: Node.js 22 or newer, standard library only. No dependencies, no
  install scripts, no build step, no network access beyond loopback. All
  JavaScript modules are ES modules (`.mjs`).
- Product layout (paths are relative to the product root):

| Path | Owner |
|---|---|
| `src/server.mjs`, `src/store.mjs`, `tests/worker_a/` | Track A (backend) |
| `tests/worker_store/` | the owner of `src/store.mjs` (Track A unless the integrator assigns a dedicated store worker) |
| `public/index.html`, `public/app.mjs`, `public/styles.css`, `tests/worker_b/` | Track B (frontend) |
| `package.json`, `CONTRACT.md` | integrator; read-only for workers |

Workers MUST NOT create files outside their owned paths and MUST NOT add
dependencies to `package.json`.

## 2. Launch

The server is started with an argv vector (never through a shell):

```text
node src/server.mjs --host <host> --port <port> --data <data_file>
```

- Each of `--host`, `--port`, `--data` appears exactly once, followed by its
  value as the next argv element. Option order is free.
- `--host` MUST be exactly `127.0.0.1` or `::1`. Any other value (including
  `localhost`, `0.0.0.0`, and `::`) is a startup failure with code
  `invalid_host` and exit code 2.
- `--port` is a decimal integer from 0 to 65535. `0` means an OS-assigned port.
- `--data` is the path of the data file (section 7). Its parent directory
  exists. The server never creates directories.
- A missing, duplicated, or unknown option, a missing option value, or an
  invalid port is a startup failure with code `invalid_arguments` and exit
  code 2. When several argument errors apply, any applicable code may be
  reported.
- The server MUST resolve `public/` relative to its own module file
  (`new URL("../public/", import.meta.url)`), never relative to the process
  working directory. The verifier starts the server with an unrelated
  temporary directory as the working directory.
- The server runs until it is killed. Note: on Windows the verifier stops the
  server with `TerminateProcess`, so no signal or exit handler runs.
  Durability therefore cannot depend on shutdown handlers.

## 3. Listening receipt

Once the HTTP server is listening, the process writes exactly one line to
stdout and never writes anything else to stdout (before or after):

```json
{"event":"listening","host":"127.0.0.1","port":49152,"pid":12345}
```

- The line is a JSON object with exactly the keys `event`, `host`, `port`,
  `pid` (any key order) followed by `\n`.
- `event` is `"listening"`; `host` equals the `--host` value exactly; `port` is
  the actual listening port (an integer from 1 to 65535, never 0); `pid` is
  `process.pid` of the server process.
- The line is at most 256 bytes long (excluding the newline).
- The verifier connects only to `host`/`port` from this receipt (IPv6 as
  `http://[::1]:<port>`). It never uses a URL printed anywhere else.
- Diagnostics, if any, go to stderr.

## 4. Startup failures

On a startup failure the server MUST NOT write to stdout, MUST NOT listen, and
MUST exit with a non-zero code. stderr MUST contain one line that is exactly
`{"error":{"code":"<code>"}}`; other stderr lines are allowed.

| Code | Exit code | Cause |
|---|---|---|
| `invalid_arguments` | 2 | argv errors described in section 2 |
| `invalid_host` | 2 | host other than `127.0.0.1` or `::1` |
| `corrupt_store` | non-zero (1 recommended) | the data file exists but is not a valid store (section 7) |
| `storage_failure` | non-zero (1 recommended) | the data file exists but cannot be read |
| `listen_failed` | non-zero (1 recommended) | the socket cannot be bound |

## 5. HTTP API

### 5.1 Task

```json
{"id":"k3J9_x-2","title":"Buy milk","completed":false}
```

- Exactly the keys `id`, `title`, `completed`.
- `id`: a server-assigned string matching `^[A-Za-z0-9_-]{1,64}$`, unique within
  the data file, and stable across restarts.
- `title`: the trimmed title (section 5.4). `completed`: boolean.

### 5.2 Routes

| Method and path | Request body | Success |
|---|---|---|
| `GET /health` | none | 200 `{"ok":true}` |
| `GET /api/tasks` | none | 200 array of all tasks in insertion order |
| `POST /api/tasks` | exactly `{"title": <string>}` | 201 the created task (`completed: false`) |
| `PATCH /api/tasks/<id>` | exactly `{"completed": <boolean>}` | 200 the updated task |
| `DELETE /api/tasks/<id>` | none (ignored) | 204 with an empty body |
| `GET /` | none | 200 `public/index.html` |
| `GET /app.mjs` | none | 200 `public/app.mjs` |
| `GET /styles.css` | none | 200 `public/styles.css` |

- Routing compares the raw request-target path exactly: no percent-decoding,
  no dot-segment resolution, no trailing-slash folding, no case folding.
- Every method and path combination not listed above returns 404 `not_found`.
  Examples: `POST /health`, `PUT /api/tasks`, `GET /api/tasks/<id>`,
  `GET /api/tasks/`, `GET /index.html`, `GET /src/store.mjs`.
- Request targets that contain a query string (`?`) are outside this contract.

### 5.3 Responses

- Every response with a body other than a static asset has a `content-type`
  whose media type is `application/json` (a `charset=utf-8` parameter is
  allowed) and a JSON body.
- Every error body is exactly `{"error":{"code":"<code>"}}` with no other keys.
- `204` responses have an empty body.

### 5.4 Request body processing (POST and PATCH)

Rules are applied in this order; the first failing rule determines the
response:

1. Size: a body larger than 8192 bytes returns 413 `payload_too_large`. A body
   of exactly 8192 bytes is accepted. The limit MUST be enforced while reading:
   a request whose `content-length` exceeds 8192 may be rejected before its
   body is read, and a streamed (chunked) body MUST be rejected as soon as more
   than 8192 bytes have arrived, without waiting for the end of the request
   and without buffering the excess. After a 413 the server may stop reading
   and close the connection.
2. Syntax: the body is decoded as UTF-8 and parsed with `JSON.parse`. A parse
   failure (including an empty body) returns 400 `invalid_json`. The request
   `content-type` header is not inspected.
3. Shape: the parsed value MUST be a JSON object (not an array or `null`) with
   exactly the listed key (`title` for POST, `completed` for PATCH) and the
   listed value type. Anything else (non-object body, missing key, extra key,
   wrong value type) returns 400 `invalid_request`.
4. Title rule (POST): the title is trimmed with `String.prototype.trim`. A
   trimmed title that is empty, or longer than 120 Unicode code points
   (counted as `[...trimmed].length`), returns 400 `invalid_title`. Duplicate
   titles are allowed. The stored and returned title is the trimmed one.
5. Lookup (PATCH and DELETE): an id that does not name an existing task
   returns 404 `not_found`.

Example: `PATCH /api/tasks/does_not_exist` with body `{}` returns 400
`invalid_request`; with body `{"completed":true}` it returns 404 `not_found`.

### 5.5 Error codes

| Status | Code | Meaning |
|---|---|---|
| 400 | `invalid_json` | body is not valid JSON |
| 400 | `invalid_request` | valid JSON with an unsupported shape or type |
| 400 | `invalid_title` | `title` is a string that violates the title rule |
| 404 | `not_found` | unknown path, method, or task id |
| 413 | `payload_too_large` | body larger than 8192 bytes |
| 500 | `storage_failure` | the mutation could not be persisted; state is unchanged |
| 500 | `internal_error` | any other unexpected failure |

## 6. Static assets

- `GET /`, `GET /app.mjs`, and `GET /styles.css` return the exact bytes of
  `public/index.html`, `public/app.mjs`, and `public/styles.css` with content
  types `text/html; charset=utf-8`, `text/javascript; charset=utf-8`, and
  `text/css; charset=utf-8`.
- No other file is ever served. Traversal attempts such as
  `/../package.json`, `/%2e%2e/package.json`, `/..%2fsrc%2fstore.mjs`,
  `/..%5cpackage.json`, `/src/store.mjs`, and `/public/../package.json` return
  404 `not_found`.

## 7. Persistence

- The data file is the only state. Its format is implementation-defined UTF-8
  text (JSON is recommended). Only `src/store.mjs` and `src/server.mjs` read or
  write it. Note: the server should persist through `src/store.mjs`.
- A missing data file initializes an empty store. The file need not be created
  before the first mutation.
- An existing data file that cannot be decoded into a valid store is corrupt.
  This includes an empty (0 byte) file, invalid UTF-8, truncated JSON, and
  text that is not JSON. On a corrupt file, server startup fails with
  `corrupt_store` (section 4) and `createStore` rejects with `corrupt_store`.
  The file bytes MUST stay unchanged: no repair, reset, rename, backup, or
  deletion, and no other file is created.
- Every successful mutation MUST be durably written before its HTTP response
  is sent (or before the store promise resolves): write the complete new
  content to a temporary file in the same directory as the data file, then
  rename it over the data file. No temporary file may remain in that
  directory after a mutation completes, whether it succeeded or failed.
- If persisting fails, the mutation is not applied (memory and file stay
  consistent) and the server returns 500 `storage_failure`.
- After a restart (including a hard kill) with the same data file, all tasks,
  their order, ids, titles, and completion states are preserved.
- Concurrent requests within one process MUST NOT lose, duplicate, or corrupt
  tasks: mutations are serialized. Multiple processes writing one data file
  are out of scope.
- Note: on Windows, `rename` over an existing file can fail transiently with
  `EPERM`, `EACCES`, or `EBUSY` while another process (for example an
  antivirus scanner) holds the file. A bounded retry is appropriate.

## 8. Store module (`src/store.mjs`)

`src/store.mjs` MUST NOT import `src/server.mjs`, anything under `public/`, or
`node:http`, `node:https`, `node:net` (with or without the `node:` prefix).

```js
export async function createStore(dataFile) // -> Promise<Store>
```

| Method | Result | Rejections (`Error` instances with a string `.code`) |
|---|---|---|
| `list()` | `Promise<Task[]>` in insertion order | none |
| `add(title)` | `Promise<Task>` with `completed: false` | non-string `title`: `invalid_request`; trimmed empty or more than 120 code points: `invalid_title` |
| `setCompleted(id, completed)` | `Promise<Task>` (updated) | `id` not a non-empty string or `completed` not a boolean: `invalid_request`; unknown id: `not_found` |
| `remove(id)` | `Promise<void>` (resolves `undefined`) | `id` not a non-empty string: `invalid_request`; unknown id: `not_found` |

- Errors are delivered as rejected promises, never as synchronous throws.
- Argument validation precedes lookup: `setCompleted("does_not_exist", "yes")`
  rejects with `invalid_request`.
- Title and id rules are the same as in section 5. Duplicate titles are
  allowed.
- Every returned array and Task object is a copy: mutating it never changes
  stored state.
- A store promise resolves only after the mutation is durable (section 7). A
  store opened later on the same file sees all persisted data.
- `createStore` on a corrupt existing file rejects with `corrupt_store` and
  does not modify the file. On a missing file it resolves to an empty store.
- Mutations issued concurrently from one process without awaiting each other
  are serialized; none is lost.

## 9. Frontend UI hooks

The frontend is `public/index.html` loading `public/app.mjs` as a module
(`<script type="module" src="app.mjs">`) and `public/styles.css`. It uses
same-origin relative API URLs only (for example `fetch("api/tasks")` or
`fetch("/api/tasks")`) and loads no external resources.

Required elements (ids, classes, and attributes are frozen):

- `<form id="new_task_form">` containing
  `<input id="new_task_title" name="title" aria-label="New task title">` and
  `<button type="submit" id="add_task_button">Add</button>`. Pressing Enter in
  the input submits the form. Submitting MUST NOT navigate the page.
- The title input MUST NOT carry `required`, `maxlength`, `minlength`, or
  `pattern`, and the frontend MUST submit the input value unmodified as the
  `title` of `POST api/tasks` (validation is the server's job).
- `<ul id="task_list" aria-label="Tasks">`. Each displayed task is
  `<li data-task-id="<id>">` containing:
  - `<input type="checkbox" class="task_toggle">`, checked exactly when the
    task is completed, with `aria-label` `Mark complete` when the task is
    incomplete and `Mark incomplete` when it is complete;
  - `<span class="task_title">` whose text content is exactly the title,
    rendered as text (never through `innerHTML` or any other HTML parsing of
    user data);
  - `<button class="task_delete" aria-label="Delete task">`.
- Filter buttons `<button data-filter="all">All</button>`,
  `<button data-filter="active">Active</button>`, and
  `<button data-filter="completed">Completed</button>`. The selected filter
  has `aria-pressed="true"` and the others `aria-pressed="false"`. The
  initial filter is `all`. Tasks outside the selected filter MUST NOT be
  displayed (remove them from `#task_list` or hide them so they are not
  rendered). Displayed tasks keep insertion order.
- `<span id="total_count">` and `<span id="completed_count">` show the number
  of all tasks and of completed tasks, independent of the selected filter.
- `<p id="error_message" role="alert">` is empty initially and after every
  successful mutation. After a rejected mutation its text content is exactly
  the `error.code` from the server response (for example `invalid_title` or
  `not_found`); if no error code is available it shows `request_failed`.

Behavior:

- On load, the page fetches `GET api/tasks` and displays all tasks.
- A task appears in the list only after the server confirmed it (201). A
  rejected add MUST NOT add a list item.
- After each mutation response, the list, the checkboxes, and the counts
  reflect the server-confirmed state.

## 10. Out of scope

Authentication, HTTPS, CORS, HEAD requests, query strings, compression,
caching headers, multiple processes sharing one data file, data migration, and
graceful shutdown.
