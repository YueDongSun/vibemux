// Trusted TaskBoard Lite browser suite (CONTRACT.md section 9).
//
// Drives a real headless Chromium-family browser (Edge or Chrome) through the
// Chrome DevTools Protocol with real input events: Input.insertText and
// Input.dispatchKeyEvent for typing, Input.dispatchMouseEvent at element
// centers for clicks. Every check gets a fresh data file, a fresh server
// process, and a fresh page target; the browser is shared by all checks.
//
// Modes:
//   browser                - the candidate's own server serves its public/
//   browser_frontend_only  - verifier/contract_stub_server.mjs serves the
//                            candidate's public/ against the reference API

import { mkdir, readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { Cdp_connection, Cdp_session } from "./cdp_client.mjs";
import {
  create_temp_dir,
  describe_start_error,
  is_process_alive,
  kill_process_tree,
  remove_temp_dir,
  resolve_candidate_layout,
  spawn_tracked,
  start_listening_server,
  stderr_text,
  stop_tracked_process,
  wait_for_exit,
} from "./candidate_paths.mjs";

export const browser_suite_config = Object.freeze({
  default_suite_timeout_ms: 120_000,
  check_timeout_ms: 30_000,
  devtools_port_timeout_ms: 20_000,
  devtools_port_poll_ms: 50,
  browser_close_timeout_ms: 10_000,
  child_exit_timeout_ms: 5000,
  poll_timeout_ms: 5000,
  poll_interval_ms: 40,
  network_idle_ms: 150,
  settle_ms: 300,
  api_timeout_ms: 5000,
  viewport: { width: 1280, height: 900 },
  data_file_name: "taskboard_browser.json",
  base_flags: [
    "--headless=new",
    "--remote-debugging-port=0",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-extensions",
    "--disable-gpu",
  ],
  // Keeps the browser on loopback: no background services, and every
  // non-loopback request goes to a closed loopback port. Loopback hosts
  // bypass the proxy implicitly.
  network_isolation_flags: [
    "--disable-background-networking",
    "--disable-component-update",
    "--disable-sync",
    "--no-pings",
    "--proxy-server=http://127.0.0.1:9",
  ],
  initial_url: "about:blank",
  unicode_title: "\u{4EFB}\u{52A1} \u{2713} \u{E9}moji \u{1F600}",
  html_title: "<img src=x onerror=\"window.__xss_fired=1\">",
});

const contract_stub_entry = fileURLToPath(new URL("./contract_stub_server.mjs", import.meta.url));

export class Browser_unavailable_error extends Error {
  constructor(message) {
    super(message);
    this.name = "Browser_unavailable_error";
  }
}

class Check_aborted_error extends Error {}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

// ------------------------------------------------------- browser process --

async function read_devtools_endpoint(user_data_dir, tracked) {
  const port_file = path.join(user_data_dir, "DevToolsActivePort");
  const deadline = Date.now() + browser_suite_config.devtools_port_timeout_ms;
  while (Date.now() < deadline) {
    if (tracked.exit_result !== null) {
      throw new Browser_unavailable_error(
        `browser exited before DevTools was ready: ${JSON.stringify(tracked.exit_result)} ${stderr_text(tracked).slice(-400)}`,
      );
    }
    try {
      const [port_line, path_line] = (await readFile(port_file, "utf8")).split(/\r?\n/);
      const port = Number(port_line);
      if (Number.isInteger(port) && port > 0 && path_line?.startsWith("/devtools/browser/")) {
        return `ws://127.0.0.1:${port}${path_line.trim()}`;
      }
    } catch {
      // The file appears once DevTools is listening.
    }
    await delay(browser_suite_config.devtools_port_poll_ms);
  }
  throw new Browser_unavailable_error(`DevToolsActivePort not written within ${browser_suite_config.devtools_port_timeout_ms} ms`);
}

async function launch_browser(browser_executable, temp_root) {
  const user_data_dir = await create_temp_dir("browser_profile", temp_root);
  const args = [
    ...browser_suite_config.base_flags,
    `--user-data-dir=${user_data_dir}`,
    ...browser_suite_config.network_isolation_flags,
    browser_suite_config.initial_url,
  ];
  const browser = {
    user_data_dir,
    command: [browser_executable, ...args],
    tracked: spawn_tracked(browser_executable, args, {}),
    connection: null,
    executable_name: path.basename(browser_executable),
    version: null,
    child_pids: [],
  };
  try {
    const websocket_url = await read_devtools_endpoint(user_data_dir, browser.tracked);
    browser.connection = await Cdp_connection.connect(websocket_url);
    const version_info = await browser.connection.send("Browser.getVersion");
    browser.version = String(version_info.product ?? "").split("/")[1] ?? String(version_info.product);
    return browser;
  } catch (error) {
    await close_browser(browser);
    if (error instanceof Browser_unavailable_error) {
      throw error;
    }
    throw new Browser_unavailable_error(`browser could not be controlled: ${error.message}`);
  }
}

// Records the browser-reported child processes (renderers, GPU, utilities) so
// close_browser can prove they exit.
async function record_child_pids(browser) {
  const info = await browser.connection.send("SystemInfo.getProcessInfo").catch(() => null);
  const pids = (info?.processInfo ?? []).map((entry) => entry.id).filter((pid) => Number.isInteger(pid) && pid !== browser.tracked.pid);
  browser.child_pids = [...new Set([...browser.child_pids, ...pids])];
}

async function wait_for_children_exit(child_pids) {
  const deadline = Date.now() + browser_suite_config.child_exit_timeout_ms;
  let alive_pids = child_pids.filter(is_process_alive);
  while (alive_pids.length > 0 && Date.now() < deadline) {
    await delay(browser_suite_config.poll_interval_ms);
    alive_pids = alive_pids.filter(is_process_alive);
  }
  return alive_pids;
}

// Closes the browser gracefully, escalates to a tree kill, verifies that every
// recorded child process exited, and removes the temporary profile. Returns
// cleanup problems instead of throwing.
async function close_browser(browser) {
  const problems = [];
  if (browser.connection !== null && !browser.connection.closed) {
    await record_child_pids(browser);
    await browser.connection.send("Browser.close", {}, { timeout_ms: 3000 }).catch(() => undefined);
  }
  if ((await wait_for_exit(browser.tracked, browser_suite_config.browser_close_timeout_ms)) === null) {
    try {
      await stop_tracked_process(browser.tracked, { tree: true });
    } catch (error) {
      problems.push(`browser process cleanup failed: ${error.message}`);
    }
  }
  browser.connection?.close();
  const surviving_pids = await wait_for_children_exit(browser.child_pids);
  if (surviving_pids.length > 0) {
    problems.push(`browser child processes survived the browser and were killed: ${surviving_pids.join(", ")}`);
    for (const pid of surviving_pids) {
      await kill_process_tree(pid);
    }
  }
  try {
    await remove_temp_dir(browser.user_data_dir);
  } catch (error) {
    problems.push(`browser profile cleanup failed: ${error.message}`);
  }
  return problems;
}

// ---------------------------------------------------------- page helpers --

function page_resolve_element(locator) {
  let root = document;
  if (locator.task_id !== undefined) {
    root = document.querySelector(`#task_list li[data-task-id="${CSS.escape(locator.task_id)}"]`);
    if (root === null) {
      return null;
    }
  }
  return root.querySelector(locator.selector);
}

function page_center_of(element) {
  if (element === null) {
    return { found: false };
  }
  element.scrollIntoView({ block: "center", inline: "center" });
  const rect = element.getBoundingClientRect();
  const x = rect.left + rect.width / 2;
  const y = rect.top + rect.height / 2;
  const hit = document.elementFromPoint(x, y);
  return {
    found: true,
    x,
    y,
    width: rect.width,
    height: rect.height,
    hit_ok: hit !== null && (hit === element || element.contains(hit)),
    hit_description: hit === null ? "nothing" : `${hit.tagName.toLowerCase()}${hit.id ? `#${hit.id}` : ""}`,
  };
}

function page_focus_report(element) {
  return { is_focused: element !== null && document.activeElement === element, value: element?.value ?? null };
}

function page_read_state() {
  const list = document.getElementById("task_list");
  const is_displayed = (element) =>
    typeof element.checkVisibility === "function" ? element.checkVisibility() : element.getClientRects().length > 0;
  const items = list === null ? [] : [...list.querySelectorAll("li[data-task-id]")];
  const text_of = (element_id) => {
    const element = document.getElementById(element_id);
    return element === null ? null : element.textContent.trim();
  };
  return {
    has_list: list !== null,
    items: items.map((item) => {
      const toggle = item.querySelector("input.task_toggle");
      const title = item.querySelector(".task_title");
      const delete_button = item.querySelector("button.task_delete");
      return {
        id: item.dataset.taskId,
        displayed: is_displayed(item),
        title: title === null ? null : title.textContent,
        title_child_elements: title === null ? null : title.children.length,
        toggle_type: toggle === null ? null : toggle.type,
        checked: toggle === null ? null : toggle.checked,
        toggle_label: toggle === null ? null : toggle.getAttribute("aria-label"),
        delete_label: delete_button === null ? null : delete_button.getAttribute("aria-label"),
      };
    }),
    total: text_of("total_count"),
    completed: text_of("completed_count"),
    error: text_of("error_message"),
    pressed: Object.fromEntries(
      [...document.querySelectorAll("button[data-filter]")].map((button) => [
        button.dataset.filter,
        button.getAttribute("aria-pressed"),
      ]),
    ),
    location: location.href,
    list_images: list === null ? null : list.querySelectorAll("img").length,
    xss_fired: typeof window.__xss_fired !== "undefined",
  };
}

function page_inspect_ui_hooks() {
  const problems = [];
  const expect = (condition, message) => {
    if (!condition) {
      problems.push(message);
    }
  };
  const form = document.getElementById("new_task_form");
  const input = document.getElementById("new_task_title");
  const add_button = document.getElementById("add_task_button");
  const list = document.getElementById("task_list");
  const error_message = document.getElementById("error_message");
  expect(form?.tagName === "FORM", "form#new_task_form is missing");
  expect(input?.tagName === "INPUT", "input#new_task_title is missing");
  if (input !== null) {
    expect(form?.contains(input) === true, "#new_task_title must be inside #new_task_form");
    expect(input.getAttribute("name") === "title", "#new_task_title must have name=\"title\"");
    expect(input.getAttribute("aria-label") === "New task title", "#new_task_title aria-label must be \"New task title\"");
    for (const attribute of ["required", "maxlength", "minlength", "pattern"]) {
      expect(!input.hasAttribute(attribute), `#new_task_title must not have the ${attribute} attribute`);
    }
  }
  expect(add_button?.tagName === "BUTTON", "button#add_task_button is missing");
  if (add_button !== null) {
    expect(add_button.type === "submit", "#add_task_button must be type=submit");
    expect(form?.contains(add_button) === true, "#add_task_button must be inside #new_task_form");
    expect(add_button.textContent.trim() === "Add", "#add_task_button text must be \"Add\"");
  }
  expect(list?.tagName === "UL", "ul#task_list is missing");
  expect(list?.getAttribute("aria-label") === "Tasks", "#task_list aria-label must be \"Tasks\"");
  for (const [filter_name, label] of [["all", "All"], ["active", "Active"], ["completed", "Completed"]]) {
    const button = document.querySelector(`button[data-filter="${filter_name}"]`);
    expect(button !== null, `button[data-filter="${filter_name}"] is missing`);
    if (button !== null) {
      expect(button.textContent.trim() === label, `filter ${filter_name} text must be "${label}"`);
      const expected_pressed = filter_name === "all" ? "true" : "false";
      expect(button.getAttribute("aria-pressed") === expected_pressed, `filter ${filter_name} must start with aria-pressed=${expected_pressed}`);
    }
  }
  for (const count_id of ["total_count", "completed_count"]) {
    const count = document.getElementById(count_id);
    expect(count?.tagName === "SPAN", `span#${count_id} is missing`);
    expect(count?.textContent.trim() === "0", `#${count_id} must show 0 for an empty store`);
  }
  expect(error_message?.tagName === "P", "p#error_message is missing");
  expect(error_message?.getAttribute("role") === "alert", "#error_message must have role=alert");
  expect(error_message?.textContent.trim() === "", "#error_message must start empty");
  return problems;
}

function element_expression(locator) {
  return `(${page_resolve_element.toString()})(${JSON.stringify(locator)})`;
}

function describe_locator(locator) {
  return locator.task_id === undefined ? locator.selector : `${locator.selector} in task ${locator.task_id}`;
}

const locators = Object.freeze({
  title_input: { selector: "#new_task_title" },
  add_button: { selector: "#add_task_button" },
  task_list: { selector: "#task_list" },
  filter: (filter_name) => ({ selector: `button[data-filter="${filter_name}"]` }),
  toggle: (task_id) => ({ selector: "input.task_toggle", task_id }),
  delete_button: (task_id) => ({ selector: "button.task_delete", task_id }),
});

// ------------------------------------------------------------ page driver --

class Page_driver {
  constructor({ session, origin, signal }) {
    this.session = session;
    this.origin = origin;
    this.page_url = `${origin}/`;
    this.signal = signal;
    this.inflight_requests = new Set();
    this.last_network_activity = Date.now();
    this.stop_network_tracking = session.on_event((message) => this.#track_network(message));
  }

  #track_network(message) {
    const request_id = message.params?.requestId;
    if (message.method === "Network.requestWillBeSent") {
      this.inflight_requests.add(request_id);
    } else if (message.method === "Network.loadingFinished" || message.method === "Network.loadingFailed") {
      this.inflight_requests.delete(request_id);
    } else {
      return;
    }
    this.last_network_activity = Date.now();
  }

  ensure_active() {
    if (this.signal.aborted) {
      throw new Check_aborted_error("check aborted by its time limit");
    }
  }

  async evaluate(expression) {
    this.ensure_active();
    const result = await this.session.send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
    if (result.exceptionDetails !== undefined) {
      const description = result.exceptionDetails.exception?.description ?? result.exceptionDetails.text;
      throw new Error(`page evaluation failed: ${description}`);
    }
    return result.result.value;
  }

  read_state() {
    return this.evaluate(`(${page_read_state.toString()})()`);
  }

  async navigate() {
    const load_event = this.session.wait_for_event("Page.loadEventFired", { timeout_ms: browser_suite_config.poll_timeout_ms * 2 });
    const navigation = await this.session.send("Page.navigate", { url: this.page_url });
    if (navigation.errorText) {
      load_event.catch(() => undefined);
      throw new Error(`navigation to ${this.page_url} failed: ${navigation.errorText}`);
    }
    await load_event;
    await this.wait_for_network_idle();
  }

  async wait_for_network_idle() {
    const deadline = Date.now() + browser_suite_config.poll_timeout_ms;
    while (Date.now() < deadline) {
      this.ensure_active();
      const quiet_for = Date.now() - this.last_network_activity;
      if (this.inflight_requests.size === 0 && quiet_for >= browser_suite_config.network_idle_ms) {
        return;
      }
      await delay(browser_suite_config.poll_interval_ms);
    }
    throw new Error(`network did not become idle within ${browser_suite_config.poll_timeout_ms} ms`);
  }

  // Polls the page state until `predicate` holds; bounded by poll_timeout_ms.
  async wait_for_state(predicate, description) {
    const deadline = Date.now() + browser_suite_config.poll_timeout_ms;
    let state = null;
    while (Date.now() < deadline) {
      state = await this.read_state();
      if (predicate(state)) {
        return state;
      }
      await delay(browser_suite_config.poll_interval_ms);
    }
    throw new Error(`timed out waiting for ${description}; last UI state: ${summarize_state(state)}`);
  }

  async settle() {
    await this.wait_for_network_idle();
    await delay(browser_suite_config.settle_ms);
    return this.read_state();
  }

  async click(locator) {
    const point = await this.evaluate(`(${page_center_of.toString()})(${element_expression(locator)})`);
    if (!point.found) {
      throw new Error(`cannot click ${describe_locator(locator)}: element not found`);
    }
    if (point.width <= 0 || point.height <= 0) {
      throw new Error(`cannot click ${describe_locator(locator)}: element has no visible size`);
    }
    if (!point.hit_ok) {
      throw new Error(`cannot click ${describe_locator(locator)}: covered by ${point.hit_description}`);
    }
    const base_event = { x: point.x, y: point.y };
    await this.session.send("Input.dispatchMouseEvent", { ...base_event, type: "mouseMoved", button: "none", buttons: 0 });
    await this.session.send("Input.dispatchMouseEvent", { ...base_event, type: "mousePressed", button: "left", buttons: 1, clickCount: 1 });
    await this.session.send("Input.dispatchMouseEvent", { ...base_event, type: "mouseReleased", button: "left", buttons: 0, clickCount: 1 });
  }

  async press_key({ key, code, key_code, text, modifiers = 0, commands }) {
    const key_down = { type: text === undefined ? "rawKeyDown" : "keyDown", key, code, windowsVirtualKeyCode: key_code, nativeVirtualKeyCode: key_code, modifiers };
    if (text !== undefined) {
      key_down.text = text;
      key_down.unmodifiedText = text;
    }
    if (commands !== undefined) {
      key_down.commands = commands;
    }
    await this.session.send("Input.dispatchKeyEvent", key_down);
    await this.session.send("Input.dispatchKeyEvent", { type: "keyUp", key, code, windowsVirtualKeyCode: key_code, nativeVirtualKeyCode: key_code, modifiers });
  }

  // Focuses the field with a real click, clears it with Ctrl+A and Backspace,
  // then types `text` with Input.insertText.
  async type_text(locator, text) {
    await this.click(locator);
    const focus = await this.evaluate(`(${page_focus_report.toString()})(${element_expression(locator)})`);
    if (!focus.is_focused) {
      throw new Error(`${describe_locator(locator)} did not receive focus from a click`);
    }
    await this.press_key({ key: "a", code: "KeyA", key_code: 65, modifiers: 2, commands: ["selectAll"] });
    await this.press_key({ key: "Backspace", code: "Backspace", key_code: 8 });
    await this.session.send("Input.insertText", { text });
    const typed = await this.evaluate(`(${page_focus_report.toString()})(${element_expression(locator)})`);
    if (typed.value !== text) {
      throw new Error(`typed text not reflected in ${describe_locator(locator)}: ${JSON.stringify(typed.value?.slice(0, 80))}`);
    }
  }

  press_enter() {
    return this.press_key({ key: "Enter", code: "Enter", key_code: 13, text: "\r" });
  }

  async accessible_node(locator) {
    this.ensure_active();
    const handle = await this.session.send("Runtime.evaluate", { expression: element_expression(locator), returnByValue: false });
    const object_id = handle.result?.objectId;
    if (handle.exceptionDetails !== undefined || object_id === undefined) {
      return null;
    }
    try {
      const { node } = await this.session.send("DOM.describeNode", { objectId: object_id });
      const { nodes } = await this.session.send("Accessibility.getPartialAXTree", { objectId: object_id, fetchRelatives: false });
      const ax_node = nodes.find((candidate) => candidate.backendDOMNodeId === node.backendNodeId) ?? nodes[0];
      return { role: ax_node?.role?.value ?? null, name: ax_node?.name?.value ?? null, ignored: ax_node?.ignored ?? null };
    } finally {
      await this.session.send("Runtime.releaseObject", { objectId: object_id }).catch(() => undefined);
    }
  }

  // ------------------------------------------------- trusted API access --

  async api(method, request_path, body_value) {
    const init = { method, signal: AbortSignal.timeout(browser_suite_config.api_timeout_ms) };
    if (body_value !== undefined) {
      init.headers = { "content-type": "application/json" };
      init.body = JSON.stringify(body_value);
    }
    const response = await fetch(`${this.origin}${request_path}`, init);
    const text = await response.text();
    return { status: response.status, data: text.length > 0 ? JSON.parse(text) : null };
  }

  async api_list() {
    const response = await this.api("GET", "/api/tasks");
    if (response.status !== 200 || !Array.isArray(response.data)) {
      throw new Error(`GET /api/tasks returned ${response.status}`);
    }
    return response.data;
  }

  async wait_for_server(predicate, description) {
    const deadline = Date.now() + browser_suite_config.poll_timeout_ms;
    let tasks = null;
    while (Date.now() < deadline) {
      this.ensure_active();
      tasks = await this.api_list();
      if (predicate(tasks)) {
        return tasks;
      }
      await delay(browser_suite_config.poll_interval_ms);
    }
    throw new Error(`timed out waiting for server state: ${description}; last server tasks: ${JSON.stringify(tasks).slice(0, 400)}`);
  }
}

function summarize_state(state) {
  return state === null ? "unavailable" : JSON.stringify(state).slice(0, 700);
}

function displayed_items(state) {
  return state.items.filter((item) => item.displayed);
}

function displayed_titles(state) {
  return displayed_items(state).map((item) => item.title);
}

function require_condition(condition, message, state) {
  if (!condition) {
    throw new Error(state === undefined ? message : `${message}; UI state: ${summarize_state(state)}`);
  }
}

function require_equal(actual, expected, message) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(`${message}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
  }
}

function item_by_id(state, task_id) {
  return state.items.find((item) => item.id === task_id);
}

// --------------------------------------------------------------- checks --

async function add_through_ui(page, title, submit_with) {
  await page.type_text(locators.title_input, title);
  if (submit_with === "enter") {
    await page.press_enter();
  } else {
    await page.click(locators.add_button);
  }
}

async function expect_single_added_item(page, title) {
  const state = await page.wait_for_state(
    (current) => displayed_items(current).length === 1 && displayed_items(current)[0].title === title,
    `one displayed task titled ${JSON.stringify(title)}`,
  );
  const server_tasks = await page.api_list();
  require_equal(server_tasks.map((task) => task.title), [title], "server tasks after the add");
  require_equal(displayed_items(state)[0].id, server_tasks[0].id, "data-task-id must equal the server id");
  require_equal(state.location, page.page_url, "submitting must not navigate the page");
  return state;
}

async function expect_rejected_add(page, title) {
  await add_through_ui(page, title, "click");
  await page.wait_for_state((state) => state.error === "invalid_title", "#error_message to show invalid_title");
  const state = await page.settle();
  require_equal(state.error, "invalid_title", "#error_message after the rejected add");
  require_equal(state.items.length, 0, "a rejected add must not add a list item");
  require_equal(state.total, "0", "#total_count after the rejected add");
  require_equal(await page.api_list(), [], "server tasks after the rejected add");
}

const browser_checks = [
  {
    name: "ui_hooks_present",
    seed: [],
    run: async (page) => {
      const problems = await page.evaluate(`(${page_inspect_ui_hooks.toString()})()`);
      require_condition(problems.length === 0, `UI hook problems: ${problems.join("; ")}`);
    },
  },
  {
    name: "add_via_button",
    seed: [],
    run: async (page) => {
      await add_through_ui(page, "Buy milk", "click");
      await expect_single_added_item(page, "Buy milk");
    },
  },
  {
    name: "add_via_enter_key",
    seed: [],
    run: async (page) => {
      await add_through_ui(page, "Walk the dog", "enter");
      await expect_single_added_item(page, "Walk the dog");
    },
  },
  {
    name: "toggle_complete_and_back",
    seed: [{ title: "Toggle target" }],
    run: async (page, seeded) => {
      const task_id = seeded[0].id;
      await page.wait_for_state(
        (state) => item_by_id(state, task_id)?.checked === false && item_by_id(state, task_id)?.toggle_label === "Mark complete",
        "the seeded task unchecked with aria-label \"Mark complete\"",
      );
      await page.click(locators.toggle(task_id));
      await page.wait_for_state(
        (state) => item_by_id(state, task_id)?.checked === true && item_by_id(state, task_id)?.toggle_label === "Mark incomplete",
        "the task checked with aria-label \"Mark incomplete\"",
      );
      await page.wait_for_server((tasks) => tasks[0]?.completed === true, "task completed on the server");
      await page.click(locators.toggle(task_id));
      await page.wait_for_state(
        (state) => item_by_id(state, task_id)?.checked === false && item_by_id(state, task_id)?.toggle_label === "Mark complete",
        "the task unchecked again with aria-label \"Mark complete\"",
      );
      await page.wait_for_server((tasks) => tasks[0]?.completed === false, "task incomplete on the server");
    },
  },
  {
    name: "delete_task",
    seed: [{ title: "Delete target" }, { title: "Survivor" }],
    run: async (page, seeded) => {
      await page.wait_for_state((state) => displayed_items(state).length === 2, "two seeded tasks");
      await page.click(locators.delete_button(seeded[0].id));
      const state = await page.wait_for_state(
        (current) => item_by_id(current, seeded[0].id) === undefined && displayed_titles(current).join("|") === "Survivor",
        "the deleted task to disappear",
      );
      require_equal(state.total, "1", "#total_count after delete");
      await page.wait_for_server((tasks) => tasks.length === 1 && tasks[0].id === seeded[1].id, "only the survivor on the server");
    },
  },
  {
    name: "filter_all_active_completed",
    seed: [{ title: "Active one" }, { title: "Done one", completed: true }, { title: "Active two" }],
    run: async (page) => {
      const expectations = [
        ["active", ["Active one", "Active two"]],
        ["completed", ["Done one"]],
        ["all", ["Active one", "Done one", "Active two"]],
      ];
      await page.wait_for_state((state) => displayed_items(state).length === 3, "three seeded tasks");
      for (const [filter_name, expected_titles] of expectations) {
        await page.click(locators.filter(filter_name));
        const expected_pressed = { all: "false", active: "false", completed: "false", [filter_name]: "true" };
        await page.wait_for_state(
          (state) =>
            JSON.stringify(displayed_titles(state)) === JSON.stringify(expected_titles) &&
            ["all", "active", "completed"].every((name) => state.pressed[name] === expected_pressed[name]),
          `filter ${filter_name} to display ${JSON.stringify(expected_titles)} with matching aria-pressed`,
        );
      }
    },
  },
  {
    name: "total_and_completed_counts",
    seed: [{ title: "Count A" }, { title: "Count B", completed: true }, { title: "Count C" }],
    run: async (page, seeded) => {
      const wait_counts = (total, completed, context) =>
        page.wait_for_state((state) => state.total === total && state.completed === completed, `counts ${total}/${completed} ${context}`);
      await wait_counts("3", "1", "after load");
      await page.click(locators.toggle(seeded[0].id));
      await wait_counts("3", "2", "after completing a task");
      await page.click(locators.filter("active"));
      await page.wait_for_state((state) => displayed_titles(state).join("|") === "Count C", "active filter showing Count C");
      await wait_counts("3", "2", "while filtered (counts cover all tasks)");
      await page.click(locators.filter("all"));
      await page.wait_for_state((state) => displayed_items(state).length === 3, "all three tasks displayed");
      await page.click(locators.delete_button(seeded[1].id));
      await wait_counts("2", "1", "after deleting a completed task");
    },
  },
  {
    name: "accessible_labels",
    seed: [{ title: "Label open" }, { title: "Label done", completed: true }],
    run: async (page, seeded) => {
      await page.wait_for_state((state) => displayed_items(state).length === 2, "two seeded tasks");
      const expectations = [
        [locators.title_input, "textbox", "New task title"],
        [locators.add_button, "button", "Add"],
        [locators.task_list, "list", "Tasks"],
        [locators.toggle(seeded[0].id), "checkbox", "Mark complete"],
        [locators.toggle(seeded[1].id), "checkbox", "Mark incomplete"],
        [locators.delete_button(seeded[0].id), "button", "Delete task"],
        [locators.delete_button(seeded[1].id), "button", "Delete task"],
      ];
      for (const [locator, role, name] of expectations) {
        const node = await page.accessible_node(locator);
        require_condition(node !== null, `${describe_locator(locator)} is missing`);
        require_equal({ role: node.role, name: node.name }, { role, name }, `accessible role and name of ${describe_locator(locator)}`);
      }
    },
  },
  {
    name: "blank_title_shows_invalid_title",
    seed: [],
    run: async (page) => expect_rejected_add(page, "   "),
  },
  {
    name: "long_title_shows_invalid_title",
    seed: [],
    run: async (page) => expect_rejected_add(page, "y".repeat(121)),
  },
  {
    name: "error_cleared_after_success",
    seed: [],
    run: async (page) => {
      await add_through_ui(page, "   ", "click");
      await page.wait_for_state((state) => state.error === "invalid_title", "#error_message to show invalid_title");
      await add_through_ui(page, "Valid task", "click");
      await page.wait_for_state(
        (state) => displayed_titles(state).join("|") === "Valid task" && state.error === "",
        "the valid task displayed and #error_message cleared",
      );
    },
  },
  {
    name: "stale_task_error_shown",
    seed: [{ title: "Stale" }],
    run: async (page, seeded) => {
      await page.wait_for_state((state) => displayed_items(state).length === 1, "the seeded task");
      const removed = await page.api("DELETE", `/api/tasks/${seeded[0].id}`);
      require_equal(removed.status, 204, "out-of-band DELETE status");
      await page.click(locators.delete_button(seeded[0].id));
      await page.wait_for_state((state) => state.error === "not_found", "#error_message to show not_found");
    },
  },
  {
    name: "unicode_title_renders_exactly",
    seed: [],
    run: async (page) => {
      await add_through_ui(page, browser_suite_config.unicode_title, "click");
      await expect_single_added_item(page, browser_suite_config.unicode_title);
    },
  },
  {
    name: "duplicate_titles_render_twice",
    seed: [],
    run: async (page) => {
      await add_through_ui(page, "Same title", "enter");
      await page.wait_for_state((state) => displayed_items(state).length === 1, "the first duplicate");
      await add_through_ui(page, "Same title", "enter");
      const state = await page.wait_for_state(
        (current) => displayed_titles(current).join("|") === "Same title|Same title",
        "two items titled Same title",
      );
      const ids = displayed_items(state).map((item) => item.id);
      require_condition(ids[0] !== ids[1], `duplicate items must have distinct data-task-id values: ${JSON.stringify(ids)}`);
      const server_tasks = await page.api_list();
      require_equal(server_tasks.map((task) => task.id), ids, "server ids for the duplicates");
    },
  },
  {
    name: "html_title_renders_as_text",
    seed: [],
    run: async (page) => {
      await add_through_ui(page, browser_suite_config.html_title, "click");
      await page.wait_for_state((state) => displayed_items(state).length === 1, "one displayed task");
      const state = await page.settle();
      const item = displayed_items(state)[0];
      require_equal(item.title, browser_suite_config.html_title, ".task_title text must be the literal title");
      require_equal(item.title_child_elements, 0, ".task_title must not contain parsed elements");
      require_equal(state.list_images, 0, "img elements inside #task_list");
      require_equal(state.xss_fired, false, "window.__xss_fired must stay undefined");
    },
  },
  {
    name: "initial_load_renders_existing_tasks",
    seed: [{ title: "Loaded first" }, { title: "Loaded second", completed: true }, { title: "Loaded third" }],
    run: async (page, seeded) => {
      const state = await page.wait_for_state((current) => displayed_items(current).length === 3, "three seeded tasks");
      require_equal(
        displayed_items(state).map((item) => [item.id, item.title, item.checked]),
        seeded.map((task) => [task.id, task.title, task.completed]),
        "displayed tasks in insertion order",
      );
      require_equal([state.total, state.completed], ["3", "1"], "counts after the initial load");
    },
  },
];

export const browser_check_names = Object.freeze(browser_checks.map((check) => check.name));

// --------------------------------------------------------------- runner --

async function start_app_server(mode, layout, workspace) {
  const options = {
    host: "127.0.0.1",
    data_file: path.join(workspace.data_dir, browser_suite_config.data_file_name),
    cwd: workspace.cwd_dir,
  };
  if (mode === "browser_frontend_only") {
    return start_listening_server({ ...options, entry_path: contract_stub_entry, extra_args: ["--public_dir", layout.public_dir] });
  }
  return start_listening_server({ ...options, entry_path: layout.server_entry });
}

async function create_workspace(label, temp_root) {
  const root = await create_temp_dir(`browser_${label}`, temp_root);
  const workspace = { root, data_dir: path.join(root, "data"), cwd_dir: path.join(root, "cwd") };
  await mkdir(workspace.data_dir);
  await mkdir(workspace.cwd_dir);
  return workspace;
}

async function seed_tasks(page, seed_specs) {
  const seeded = [];
  for (const spec of seed_specs) {
    const created = await page.api("POST", "/api/tasks", { title: spec.title });
    if (created.status !== 201) {
      throw new Error(`seeding POST returned ${created.status}`);
    }
    let task = created.data;
    if (spec.completed === true) {
      const patched = await page.api("PATCH", `/api/tasks/${task.id}`, { completed: true });
      if (patched.status !== 200) {
        throw new Error(`seeding PATCH returned ${patched.status}`);
      }
      task = patched.data;
    }
    seeded.push(task);
  }
  return seeded;
}

async function open_page_session(browser) {
  const { targetId } = await browser.connection.send("Target.createTarget", { url: "about:blank" });
  const { sessionId } = await browser.connection.send("Target.attachToTarget", { targetId, flatten: true });
  const session = new Cdp_session(browser.connection, sessionId, targetId);
  for (const domain of ["Page", "Runtime", "DOM", "Network", "Accessibility"]) {
    await session.send(`${domain}.enable`);
  }
  await session.send("Emulation.setDeviceMetricsOverride", {
    width: browser_suite_config.viewport.width,
    height: browser_suite_config.viewport.height,
    deviceScaleFactor: 1,
    mobile: false,
  });
  await session.send("Emulation.setFocusEmulationEnabled", { enabled: true });
  return session;
}

async function run_single_check(browser, check, { mode, layout, time_limit_ms, temp_root }) {
  const started_at = Date.now();
  const controller = new AbortController();
  let workspace = null;
  let server = null;
  let session = null;
  let page = null;
  const body = async () => {
    workspace = await create_workspace(check.name, temp_root);
    try {
      server = await start_app_server(mode, layout, workspace);
    } catch (error) {
      throw new Error(`server start failed: ${describe_start_error(error)}`);
    }
    session = await open_page_session(browser);
    page = new Page_driver({ session, origin: server.origin, signal: controller.signal });
    const seeded = await seed_tasks(page, check.seed);
    await page.navigate();
    await check.run(page, seeded);
  };
  let timer;
  const time_limit = new Promise((_, reject) => {
    timer = setTimeout(() => {
      controller.abort();
      reject(new Error(`check exceeded its ${time_limit_ms} ms time limit`));
    }, time_limit_ms);
  });
  const body_promise = body();
  let error_message = null;
  try {
    await Promise.race([body_promise, time_limit]);
  } catch (error) {
    error_message = error?.message ?? String(error);
  } finally {
    clearTimeout(timer);
    controller.abort();
    await body_promise.catch(() => undefined);
    page?.stop_network_tracking();
    if (session !== null) {
      await browser.connection.send("Target.closeTarget", { targetId: session.target_id }).catch(() => undefined);
    }
    if (server !== null) {
      await stop_tracked_process(server.tracked);
    }
    if (workspace !== null) {
      await remove_temp_dir(workspace.root);
    }
  }
  return { name: check.name, passed: error_message === null, error: error_message, duration_ms: Date.now() - started_at };
}

// In frontend-only mode the reference backend must start; otherwise the
// suite is blocked (a verifier prerequisite), not failed.
async function preflight_contract_stub(layout, temp_root) {
  const workspace = await create_workspace("stub_preflight", temp_root);
  try {
    const server = await start_app_server("browser_frontend_only", layout, workspace);
    await stop_tracked_process(server.tracked);
    return null;
  } catch (error) {
    return `contract stub server failed to start: ${describe_start_error(error)}`;
  } finally {
    await remove_temp_dir(workspace.root);
  }
}

// Runs every browser check and returns per-check results. Never throws for
// candidate defects; a missing prerequisite is reported as blocked_reason.
export async function run_browser_suite({ candidate_dir, mode, browser_executable, timeout_ms, temp_root }) {
  const layout = resolve_candidate_layout(candidate_dir);
  const suite_deadline = Date.now() + (timeout_ms ?? browser_suite_config.default_suite_timeout_ms);
  const result = { checks: [], browser: null, command: [], blocked_reason: null, cleanup_errors: [] };
  if (mode === "browser_frontend_only") {
    result.blocked_reason = await preflight_contract_stub(layout, temp_root);
    if (result.blocked_reason !== null) {
      return result;
    }
  }
  let browser;
  try {
    browser = await launch_browser(browser_executable, temp_root);
  } catch (error) {
    result.blocked_reason = error instanceof Browser_unavailable_error ? error.message : `browser launch failed: ${error.message}`;
    return result;
  }
  result.command = browser.command;
  result.browser = { executable_name: browser.executable_name, version: browser.version };
  try {
    for (const check of browser_checks) {
      if (browser.connection.closed || browser.tracked.exit_result !== null) {
        // A dead browser is an environment failure, not a candidate defect.
        result.blocked_reason = `browser connection lost before check ${check.name}`;
        break;
      }
      const remaining_ms = suite_deadline - Date.now();
      if (remaining_ms <= 0) {
        result.checks.push({ name: check.name, passed: false, error: "suite timeout reached before this check", duration_ms: 0 });
        continue;
      }
      const time_limit_ms = Math.min(browser_suite_config.check_timeout_ms, remaining_ms);
      result.checks.push(await run_single_check(browser, check, { mode, layout, time_limit_ms, temp_root }));
    }
  } finally {
    result.cleanup_errors.push(...(await close_browser(browser)));
  }
  return result;
}
