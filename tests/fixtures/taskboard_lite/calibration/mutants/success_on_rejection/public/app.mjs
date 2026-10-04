// TaskBoard Lite frontend: calibration reference implementation (CONTRACT.md
// section 9). State is the server-confirmed task list plus the selected
// filter; every render rebuilds the list with text-only DOM APIs.

const app_config = Object.freeze({
  tasks_url: "api/tasks",
  fallback_error_code: "request_failed",
  filter_predicates: {
    all: () => true,
    active: (task) => !task.completed,
    completed: (task) => task.completed,
  },
});

const app_state = { tasks: [], filter: "all" };

const elements = {
  form: document.getElementById("new_task_form"),
  title_input: document.getElementById("new_task_title"),
  task_list: document.getElementById("task_list"),
  total_count: document.getElementById("total_count"),
  completed_count: document.getElementById("completed_count"),
  error_message: document.getElementById("error_message"),
  filter_buttons: [...document.querySelectorAll("button[data-filter]")],
};

// ------------------------------------------------------------------- api --

async function request_json(method, url, body_value) {
  const init = { method, headers: {} };
  if (body_value !== undefined) {
    init.headers["content-type"] = "application/json";
    init.body = JSON.stringify(body_value);
  }
  let response;
  try {
    response = await fetch(url, init);
  } catch {
    return { ok: false, error_code: app_config.fallback_error_code };
  }
  const response_text = await response.text();
  let data = null;
  try {
    data = response_text.length > 0 ? JSON.parse(response_text) : null;
  } catch {
    data = null;
  }
  if (response.ok) {
    return { ok: true, data };
  }
  const error_code = typeof data?.error?.code === "string" ? data.error.code : app_config.fallback_error_code;
  return { ok: false, error_code };
}

function task_url(task_id) {
  return `${app_config.tasks_url}/${encodeURIComponent(task_id)}`;
}

// ---------------------------------------------------------------- render --

function show_error(error_code) {
  elements.error_message.textContent = error_code;
}

function clear_error() {
  elements.error_message.textContent = "";
}

function build_task_item(task) {
  const item = document.createElement("li");
  item.dataset.taskId = task.id;
  item.className = task.completed ? "task_item task_done" : "task_item";

  const toggle = document.createElement("input");
  toggle.type = "checkbox";
  toggle.className = "task_toggle";
  toggle.checked = task.completed;
  toggle.setAttribute("aria-label", task.completed ? "Mark incomplete" : "Mark complete");

  const title = document.createElement("span");
  title.className = "task_title";
  title.textContent = task.title;

  const delete_button = document.createElement("button");
  delete_button.type = "button";
  delete_button.className = "task_delete";
  delete_button.setAttribute("aria-label", "Delete task");
  delete_button.textContent = "Delete";

  item.append(toggle, title, delete_button);
  return item;
}

function render() {
  const is_visible = app_config.filter_predicates[app_state.filter];
  elements.task_list.replaceChildren(...app_state.tasks.filter(is_visible).map(build_task_item));
  elements.total_count.textContent = String(app_state.tasks.length);
  elements.completed_count.textContent = String(app_state.tasks.filter((task) => task.completed).length);
  for (const button of elements.filter_buttons) {
    button.setAttribute("aria-pressed", String(button.dataset.filter === app_state.filter));
  }
}

// ------------------------------------------------------------- mutations --

async function load_tasks() {
  const result = await request_json("GET", app_config.tasks_url);
  if (!result.ok) {
    show_error(result.error_code);
    return;
  }
  app_state.tasks = result.data;
  render();
}

async function add_task(title) {
  const result = await request_json("POST", app_config.tasks_url, { title });
  if (!result.ok) {
    // MUTANT success_on_rejection: the rejected task is still shown locally.
    show_error(result.error_code);
    app_state.tasks = [...app_state.tasks, { id: `local_${Date.now()}`, title, completed: false }];
    render();
    return;
  }
  app_state.tasks = [...app_state.tasks, result.data];
  elements.title_input.value = "";
  clear_error();
  render();
}

async function set_task_completed(task_id, completed) {
  const result = await request_json("PATCH", task_url(task_id), { completed });
  if (!result.ok) {
    show_error(result.error_code);
    render();
    return;
  }
  app_state.tasks = app_state.tasks.map((task) => (task.id === task_id ? result.data : task));
  clear_error();
  render();
}

async function delete_task(task_id) {
  const result = await request_json("DELETE", task_url(task_id));
  if (!result.ok) {
    show_error(result.error_code);
    return;
  }
  app_state.tasks = app_state.tasks.filter((task) => task.id !== task_id);
  clear_error();
  render();
}

// ---------------------------------------------------------------- events --

function bind_events() {
  elements.form.addEventListener("submit", (event) => {
    event.preventDefault();
    void add_task(elements.title_input.value);
  });
  elements.task_list.addEventListener("change", (event) => {
    const toggle = event.target.closest(".task_toggle");
    if (toggle !== null) {
      void set_task_completed(toggle.closest("li").dataset.taskId, toggle.checked);
    }
  });
  elements.task_list.addEventListener("click", (event) => {
    const delete_button = event.target.closest(".task_delete");
    if (delete_button !== null) {
      void delete_task(delete_button.closest("li").dataset.taskId);
    }
  });
  for (const button of elements.filter_buttons) {
    button.addEventListener("click", () => {
      app_state.filter = button.dataset.filter;
      render();
    });
  }
}

bind_events();
void load_tasks();
