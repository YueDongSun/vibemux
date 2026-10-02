// TaskBoard Lite store: calibration reference implementation (CONTRACT.md
// sections 7 and 8). Tasks live in memory; every mutation is serialized,
// written to a temporary file in the data directory, synced, and renamed over
// the data file before the in-memory state changes and the promise resolves.

import { randomBytes } from "node:crypto";
import { open, readFile, rename, rm } from "node:fs/promises";

const store_config = Object.freeze({
  max_title_code_points: 120,
  data_schema_version: 1,
  id_byte_length: 12,
  rename_retry_limit: 10,
  rename_retry_delay_ms: 25,
  retryable_rename_codes: new Set(["EPERM", "EACCES", "EBUSY"]),
  id_pattern: /^[A-Za-z0-9_-]{1,64}$/,
});

let temp_file_counter = 0;

function create_store_error(code, message, cause) {
  const error = new Error(message, cause === undefined ? undefined : { cause });
  error.code = code;
  return error;
}

function copy_task(task) {
  return { id: task.id, title: task.title, completed: task.completed };
}

function validate_title(title) {
  if (typeof title !== "string") {
    throw create_store_error("invalid_request", "title must be a string");
  }
  const trimmed_title = title.trim();
  const code_point_count = [...trimmed_title].length;
  if (code_point_count === 0 || code_point_count > store_config.max_title_code_points) {
    throw create_store_error("invalid_title", "title must have 1 to 120 code points after trimming");
  }
  return trimmed_title;
}

function validate_task_id(task_id) {
  if (typeof task_id !== "string" || task_id.length === 0) {
    throw create_store_error("invalid_request", "id must be a non-empty string");
  }
}

function is_valid_stored_task(task) {
  return (
    task !== null &&
    typeof task === "object" &&
    !Array.isArray(task) &&
    Object.keys(task).length === 3 &&
    typeof task.id === "string" &&
    store_config.id_pattern.test(task.id) &&
    typeof task.title === "string" &&
    typeof task.completed === "boolean"
  );
}

function decode_snapshot(file_bytes) {
  let document;
  try {
    const text = new TextDecoder("utf-8", { fatal: true }).decode(file_bytes);
    document = JSON.parse(text);
  } catch (error) {
    throw create_store_error("corrupt_store", "data file is not valid UTF-8 JSON", error);
  }
  const is_valid_document =
    document !== null &&
    typeof document === "object" &&
    document.schema_version === store_config.data_schema_version &&
    Array.isArray(document.tasks) &&
    document.tasks.every(is_valid_stored_task) &&
    new Set(document.tasks.map((task) => task.id)).size === document.tasks.length;
  if (!is_valid_document) {
    throw create_store_error("corrupt_store", "data file does not contain a valid task list");
  }
  return document.tasks.map(copy_task);
}

async function read_initial_tasks(data_file) {
  let file_bytes;
  try {
    file_bytes = await readFile(data_file);
  } catch (error) {
    if (error.code === "ENOENT") {
      return [];
    }
    throw create_store_error("storage_failure", "data file cannot be read", error);
  }
  try {
    return decode_snapshot(file_bytes);
  } catch {
    // MUTANT corrupt_file_reset: a corrupt file is silently replaced by an empty store.
    await write_snapshot(data_file, []);
    return [];
  }
}

function wait_milliseconds(delay_ms) {
  return new Promise((resolve) => setTimeout(resolve, delay_ms));
}

async function rename_with_retry(source_path, target_path) {
  for (let attempt = 1; ; attempt += 1) {
    try {
      await rename(source_path, target_path);
      return;
    } catch (error) {
      const can_retry =
        store_config.retryable_rename_codes.has(error.code) && attempt < store_config.rename_retry_limit;
      if (!can_retry) {
        throw error;
      }
      await wait_milliseconds(store_config.rename_retry_delay_ms * attempt);
    }
  }
}

async function write_snapshot(data_file, tasks) {
  temp_file_counter += 1;
  const temp_file = `${data_file}.${process.pid}.${temp_file_counter}.tmp`;
  const content = `${JSON.stringify({ schema_version: store_config.data_schema_version, tasks })}\n`;
  try {
    const file_handle = await open(temp_file, "wx");
    try {
      await file_handle.writeFile(content, "utf8");
      await file_handle.sync();
    } finally {
      await file_handle.close();
    }
    await rename_with_retry(temp_file, data_file);
  } catch (error) {
    await rm(temp_file, { force: true });
    throw create_store_error("storage_failure", "data file could not be written", error);
  }
}

function create_unique_id(tasks) {
  for (;;) {
    const candidate_id = randomBytes(store_config.id_byte_length).toString("base64url");
    if (!tasks.some((task) => task.id === candidate_id)) {
      return candidate_id;
    }
  }
}

function find_task_index(tasks, task_id) {
  const task_index = tasks.findIndex((task) => task.id === task_id);
  if (task_index === -1) {
    throw create_store_error("not_found", "task does not exist");
  }
  return task_index;
}

class Task_store {
  #data_file;
  #tasks;
  #mutation_tail = Promise.resolve();

  constructor(data_file, tasks) {
    this.#data_file = data_file;
    this.#tasks = tasks;
  }

  // Runs mutations one at a time; `build_next_tasks` returns the next task
  // list plus the value to resolve with, and state changes only after the
  // snapshot is durable.
  #serialize_mutation(build_next_tasks) {
    const mutation = this.#mutation_tail.then(async () => {
      const { next_tasks, result } = build_next_tasks(this.#tasks);
      await write_snapshot(this.#data_file, next_tasks);
      this.#tasks = next_tasks;
      return result;
    });
    this.#mutation_tail = mutation.catch(() => undefined);
    return mutation;
  }

  async list() {
    return this.#tasks.map(copy_task);
  }

  async add(title) {
    const trimmed_title = validate_title(title);
    return this.#serialize_mutation((tasks) => {
      const task = { id: create_unique_id(tasks), title: trimmed_title, completed: false };
      return { next_tasks: [...tasks, task], result: copy_task(task) };
    });
  }

  async setCompleted(id, completed) {
    validate_task_id(id);
    if (typeof completed !== "boolean") {
      throw create_store_error("invalid_request", "completed must be a boolean");
    }
    return this.#serialize_mutation((tasks) => {
      const task_index = find_task_index(tasks, id);
      const updated_task = { ...tasks[task_index], completed };
      const next_tasks = tasks.with(task_index, updated_task);
      return { next_tasks, result: copy_task(updated_task) };
    });
  }

  async remove(id) {
    validate_task_id(id);
    return this.#serialize_mutation((tasks) => {
      const task_index = find_task_index(tasks, id);
      return { next_tasks: tasks.toSpliced(task_index, 1), result: undefined };
    });
  }
}

export async function createStore(dataFile) {
  if (typeof dataFile !== "string" || dataFile.length === 0) {
    throw create_store_error("invalid_request", "data file path must be a non-empty string");
  }
  const tasks = await read_initial_tasks(dataFile);
  return new Task_store(dataFile, tasks);
}
