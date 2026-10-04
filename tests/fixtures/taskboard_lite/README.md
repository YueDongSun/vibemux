# TaskBoard Lite acceptance fixture

TaskBoard Lite is a tiny task-list product used as a harness-acceptance
benchmark for the multi-agent coding orchestrator. Two coding agents implement
it in separate worktrees, and a trusted verifier decides acceptance:

- Track A (backend): `src/server.mjs`, `src/store.mjs`
- Track B (frontend): `public/index.html`, `public/app.mjs`, `public/styles.css`

The fixture is self-contained JavaScript for Node.js 22+ (validated on Node
24) and uses only the standard library: no npm install, no dependencies, and
no network access beyond loopback.

## Layout and ownership

| Path | Role | Owner and trust |
|---|---|---|
| `base/` | Initial commit of the temporary product repository that workers start from | Integrator; workers edit only their track's paths |
| `base/CONTRACT.md` | Frozen product contract (launch, receipt, HTTP, persistence, store, UI hooks) | Integrator; immutable during a run |
| `base/src/*.mjs` | Stubs: the server prints `{"error":{"code":"not_implemented"}}` to stderr and exits 3; `createStore` rejects with `not_implemented` | Track A replaces them |
| `base/public/*` | Stub page, module, and stylesheet | Track B replaces them |
| `base/tests/worker_a/`, `worker_b/`, `worker_store/` | Placeholders for worker-owned development tests | Track A, Track B, store owner |
| `verifier/` | Trusted verifier. Never copied into a worker worktree. | Integrator only |
| `calibration/good/` | Correct reference implementation. Never placed in `base/`. | Integrator only |
| `calibration/mutants/<mutant_id>/` | Labeled defective variants (only the files that differ from `good/`, plus `mutant.json`) | Integrator only |
| `calibration/calibrate.mjs` | Proves the verifier accepts `good/` and rejects every mutant and the base stub | Integrator only |

Verifier files:

| File | Purpose |
|---|---|
| `verifier/run_verifier.mjs` | CLI entry point; runs one suite and writes a result JSON |
| `verifier/api_suite.test.mjs` | `node:test` suite for launch, receipt, HTTP API, static files, persistence, concurrency |
| `verifier/store_suite.test.mjs` | `node:test` suite for the `src/store.mjs` interface (comparison mode, below) |
| `verifier/browser_suite.mjs` | Real headless-browser suite over the Chrome DevTools Protocol |
| `verifier/cdp_client.mjs` | Minimal CDP client over the global `WebSocket` with flattened sessions |
| `verifier/contract_stub_server.mjs` | Verifier-owned reference backend used only for `browser_frontend_only` |
| `verifier/candidate_paths.mjs` | Candidate layout, temp directories, process launch, receipt validation, process cleanup |

"Comparison mode" for the store suite means it checks the same semantics as
the API suite (validation codes, ordering, copies, persistence, corruption,
concurrency) directly on the module interface, so a backend defect can be
localized to the HTTP layer or to the store by comparing the two results. The
store suite imports only `src/store.mjs`; it does not start the server.

## Running the verifier

```text
node verifier/run_verifier.mjs --candidate <product_dir> --suite <api|store|browser|browser_frontend_only>
                               --out <result_file.json> [--browser <absolute msedge.exe or chrome.exe>]
                               [--timeout_ms N]
```

- `api` and `store` run `process.execPath --test --test-reporter=tap <suite>`
  as a child process with `TASKBOARD_CANDIDATE_DIR` set, and parse the TAP.
  `NODE_TEST_CONTEXT` and `NODE_OPTIONS` are removed from the child
  environment so an outer test runner cannot change the reporter.
- `browser` starts the candidate server; `browser_frontend_only` serves the
  candidate `public/` through `contract_stub_server.mjs`. Both run
  in-process and report per-check results.
- Browser discovery without `--browser`: Edge
  (`C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe`), Chrome
  (`C:/Program Files/Google/Chrome/Application/chrome.exe`),
  `/usr/bin/google-chrome`, `/usr/bin/chromium`. None found means `blocked`.
- The default suite timeout is 120 s. Exit codes: 0 passed, 1 failed,
  2 blocked, 3 usage error (no result file is written for usage errors).
- The candidate directory is never written. Each run creates one temp root
  under `os.tmpdir()`; every data file, working directory, browser profile,
  and runner directory lives below it, and it is removed before exit, even
  when a timed-out suite had to be killed.

Result JSON (`schema_version` 1). The fields requested by the integrator are
`suite`, `status` (`passed`, `failed`, or `blocked`), `tests_total`,
`tests_passed`, `tests_failed`, `failed_tests`, `duration_ms`,
`node_version`, `browser` (`{"executable_name","version"}` or `null`),
`command` (child test command, or the browser launch argv for browser suites),
and `blocked_reason`. Additional fields: `tests_skipped` (for example the IPv6
receipt test when `::1` is unavailable), `candidate_dir`, `checks`
(per-check `name`, `passed`, `skipped`, `error`), and `cleanup_errors`.
`blocked` is never reported as passed; a status of `passed` also requires at
least one passing check.

## Browser suite

The browser is launched with
`--headless=new --remote-debugging-port=0 --user-data-dir=<temp> --no-first-run --no-default-browser-check --disable-extensions --disable-gpu`
plus network-isolation flags (`--disable-background-networking`,
`--disable-component-update`, `--disable-sync`, `--no-pings`,
`--proxy-server=http://127.0.0.1:9`) so that every non-loopback request goes
to a closed loopback port; loopback bypasses the proxy implicitly. The suite
reads `DevToolsActivePort`, connects with the global `WebSocket`, creates a
page target per check, attaches with flattened sessions, and drives the page
with real input: `Input.insertText`, `Input.dispatchKeyEvent` (Ctrl+A,
Backspace, Enter with key code 13), and `Input.dispatchMouseEvent` at element
centers after `scrollIntoView`, with a hit test that rejects covered elements.
Accessible names and roles come from `Accessibility.getPartialAXTree`. Every
check uses a fresh data file and a fresh server, seeds state through the API,
and polls the UI every 40 ms with a 5 s bound.

Checks: `ui_hooks_present`, `add_via_button`, `add_via_enter_key`,
`toggle_complete_and_back`, `delete_task`, `filter_all_active_completed`,
`total_and_completed_counts`, `accessible_labels`,
`blank_title_shows_invalid_title`, `long_title_shows_invalid_title`,
`error_cleared_after_success`, `stale_task_error_shown`,
`unicode_title_renders_exactly`, `duplicate_titles_render_twice`,
`html_title_renders_as_text`, `initial_load_renders_existing_tasks`.

Cleanup: the browser is closed with `Browser.close`, awaited, and tree-killed
if it does not exit; the child process ids reported by
`SystemInfo.getProcessInfo` must exit (survivors are killed and reported in
`cleanup_errors`); the profile directory is removed.

## Calibration

```text
node calibration/calibrate.mjs [--browser <absolute path>] [--suite_timeout_ms N]
```

Each candidate is a temporary copy: `good/` as is, each mutant overlaid on a
copy of `good/`, and `base/`. The summary on stdout contains the requested
fields `good` (status per suite), `mutants` (`mutant_id`,
`expected_failing_suite`, `observed_status`, `expectation_met`),
`base_api_status`, and `all_expectations_met`, plus these stricter additions:

- `observed_failed_tests` per mutant, to show it failed for the intended reason;
- `frontend_only_observed_status` / `frontend_only_expectation_met` for
  browser mutants: they must also fail `browser_frontend_only`;
- `contract_stub_api_status`: the reference backend behind
  `browser_frontend_only` must itself pass the API suite;
- `sources_unchanged`: sizes and modification times of `good/`, `mutants/`,
  and `base/` are identical before and after;
- `browsers_used` and `duration_ms`.

The exit code is 0 only when every expectation holds.

| Mutant | Defect | Expected failing suite |
|---|---|---|
| `title_length_off_by_one` | store accepts 121 code points | `api` |
| `blank_title_accepted` | store accepts a blank trimmed title | `store` |
| `corrupt_file_reset` | corrupt data file silently reset to empty | `api` |
| `lost_concurrent_updates` | unserialized read-modify-write of the data file | `store` |
| `wrong_delete_status` | DELETE returns 200 | `api` |
| `payload_limit_missing` | no 8192-byte limit, no 413 | `api` |
| `xss_inner_html` | titles rendered with `innerHTML` | `browser` |
| `filter_broken` | Completed filter shows all tasks | `browser` |
| `success_on_rejection` | rejected add still shown in the list | `browser` |

Mutant files are full copies of the corresponding `good/` file with one
change marked by a `MUTANT <mutant_id>` comment. `calibrate.mjs` rejects a
mutant whose overlay file is missing from, or identical to, `good/`. When a
`good/` file changes, update the matching mutant files and rerun calibration.

## Contract decisions beyond the original brief

`base/CONTRACT.md` resolves these points explicitly, and the verifier tests them:

- argv errors other than the host use `invalid_arguments` (exit 2); `localhost`,
  `0.0.0.0`, and `::` are rejected hosts;
- the receipt has exactly four keys, at most 256 bytes, and nothing else is
  ever written to stdout; startup failures print a JSON error line on stderr;
- routing compares the raw request path; every unlisted method/path pair
  (for example `POST /health`, `GET /api/tasks/<id>`) is 404 `not_found`;
- body rules are ordered size, JSON syntax, shape, title, lookup, so
  `PATCH /api/tasks/does_not_exist` with `{}` is 400 `invalid_request`;
- an empty body is `invalid_json`; the request `content-type` is not inspected;
- exactly 8192 bytes is accepted; 413 must arrive before a chunked body
  ends; a declared `content-length` above the limit may be rejected early;
- error bodies are exactly `{"error":{"code":...}}`; ids match
  `^[A-Za-z0-9_-]{1,64}$`;
- an empty existing data file, invalid UTF-8, or non-JSON is corrupt;
  no temporary file may remain in the data directory after a mutation;
- store errors are rejected promises (never synchronous throws), arguments
  are validated before lookup, and every returned object is a copy;
- the title input must not carry `required`, `maxlength`, `minlength`, or
  `pattern`, so server validation is observable; filtered-out tasks may be
  removed or hidden; `#error_message` shows `request_failed` when no error
  code is available and is cleared by the next successful mutation.

## Limitations

- Persistence durability is checked by hard-killing the server
  (`TerminateProcess` on Windows) right after a response and restarting it;
  power-loss durability is not testable here.
- The 413 streaming checks pace 4 KB chunks with 2 ms pauses so the response
  can be read before an abrupt server-side close.
- IPv6 receipt coverage is skipped (and reported in `tests_skipped`) when
  `::1` cannot be bound.
- On Windows, a `rename` over a file held by another process (for example an
  antivirus scan) can fail transiently; the reference implementations retry,
  and the contract recommends that workers do too.
