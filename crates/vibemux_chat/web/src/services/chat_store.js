import { app_config, default_settings, models } from "../config.js";
import { valid_interface } from "../interface_policy.js";

const store_config = {
  themes: ["system", "light", "dark"],
  font_sizes: ["small", "default", "large"],
  densities: ["comfortable", "compact"],
  message_statuses: ["complete", "streaming", "stopped", "error"],
  image_types: ["image/png", "image/jpeg", "image/gif", "image/webp"],
  text_types: [
    "application/json",
    "application/javascript",
    "application/xml",
    "application/yaml",
  ],
  extension_types: {
    png: "image/png",
    jpg: "image/jpeg",
    jpeg: "image/jpeg",
    gif: "image/gif",
    webp: "image/webp",
    pdf: "application/pdf",
    txt: "text/plain",
    md: "text/markdown",
    csv: "text/csv",
    json: "application/json",
    js: "application/javascript",
    ts: "text/plain",
    jsx: "text/plain",
    tsx: "text/plain",
    sql: "text/plain",
    html: "text/html",
    css: "text/css",
    xml: "application/xml",
    yaml: "application/yaml",
    yml: "application/yaml",
    py: "text/plain",
    rs: "text/plain",
    log: "text/plain",
  },
  usage_fields: [
    "input_tokens",
    "output_tokens",
    "cache_creation_input_tokens",
    "cache_read_input_tokens",
  ],
  corrupt_warning:
    "Saved conversation data is invalid and has been preserved. Export a backup from browser storage and explicitly reset local data before saving again.",
};

export function uid() {
  return globalThis.crypto.randomUUID();
}

function require_shape(condition, description) {
  if (!condition) throw new TypeError(`Invalid ${description}.`);
}

function is_record(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function valid_model(value) {
  return models.some((model) => model.id === value);
}

function valid_timestamp(value) {
  return (
    (Number.isFinite(value) && value >= 0) ||
    (typeof value === "string" &&
      /^\d{4}-\d{2}-\d{2}T.*Z$/.test(value) &&
      Number.isFinite(Date.parse(value)))
  );
}

function clean_settings(settings) {
  require_shape(is_record(settings), "settings");
  const cleaned = { ...default_settings };
  for (const key of Object.keys(default_settings)) {
    if (Object.hasOwn(settings, key)) cleaned[key] = settings[key];
  }
  require_shape(store_config.themes.includes(cleaned.theme), "settings theme");
  require_shape(
    valid_interface(cleaned.interface_style),
    "settings interface style",
  );
  require_shape(
    store_config.font_sizes.includes(cleaned.font_size),
    "settings font size",
  );
  require_shape(
    store_config.densities.includes(cleaned.density),
    "settings density",
  );
  require_shape(valid_model(cleaned.default_model), "default model");
  for (const key of ["enter_to_send", "auto_scroll", "default_thinking"]) {
    require_shape(typeof cleaned[key] === "boolean", `settings ${key}`);
  }
  return cleaned;
}

export function create_conversation(settings = default_settings) {
  const cleaned = clean_settings(settings);
  const now = new Date().toISOString();
  return {
    id: uid(),
    title: "New conversation",
    created_at: now,
    updated_at: now,
    pinned: false,
    messages: [],
    model: cleaned.default_model,
    thinking: cleaned.default_thinking,
    artifact: null,
    artifact_open: false,
  };
}

function clean_usage(usage) {
  require_shape(is_record(usage), "message usage");
  const cleaned = {};
  for (const key of store_config.usage_fields) {
    if (!Object.hasOwn(usage, key)) continue;
    require_shape(
      Number.isSafeInteger(usage[key]) && usage[key] >= 0,
      `usage ${key}`,
    );
    cleaned[key] = usage[key];
  }
  return cleaned;
}

function attachment_kind(type) {
  if (store_config.image_types.includes(type)) return "image";
  if (type === "application/pdf") return "pdf";
  if (type.startsWith("text/") || store_config.text_types.includes(type))
    return "text";
  throw new TypeError(
    "Unsupported attachment type. Choose a PNG, JPEG, GIF, WebP, PDF, or text file.",
  );
}

function clean_attachment(attachment) {
  require_shape(is_record(attachment), "attachment");
  require_shape(
    typeof attachment.id === "string" && attachment.id.length > 0,
    "attachment id",
  );
  require_shape(
    typeof attachment.name === "string" && attachment.name.length > 0,
    "attachment name",
  );
  require_shape(
    typeof attachment.type === "string" && typeof attachment.data === "string",
    "attachment data",
  );
  require_shape(
    Number.isSafeInteger(attachment.size) &&
      attachment.size >= 0 &&
      attachment.size <= app_config.max_attachment_bytes,
    "attachment size",
  );
  require_shape(
    attachment.kind === attachment_kind(attachment.type),
    "attachment kind",
  );
  if (attachment.kind === "text") {
    require_shape(
      new TextEncoder().encode(attachment.data).byteLength <=
        app_config.max_attachment_bytes,
      "text attachment size",
    );
  } else {
    require_shape(
      attachment.data.length > 0 &&
        attachment.data.length <=
          Math.ceil(app_config.max_attachment_bytes / 3) * 4 &&
        attachment.data.length % 4 === 0 &&
        /^[A-Za-z0-9+/]+={0,2}$/.test(attachment.data),
      "base64 attachment",
    );
    const padding = attachment.data.endsWith("==")
      ? 2
      : attachment.data.endsWith("=")
        ? 1
        : 0;
    require_shape(
      (attachment.data.length / 4) * 3 - padding === attachment.size,
      "attachment byte length",
    );
  }
  return {
    id: attachment.id,
    name: attachment.name,
    size: attachment.size,
    type: attachment.type,
    kind: attachment.kind,
    data: attachment.data,
  };
}

function clean_message(message, recover_interrupted) {
  require_shape(is_record(message), "message");
  require_shape(["user", "assistant"].includes(message.role), "message role");
  require_shape(typeof message.content === "string", "message content");
  const cleaned = { role: message.role, content: message.content };
  for (const key of ["id", "error"]) {
    if (!Object.hasOwn(message, key)) continue;
    require_shape(
      typeof message[key] === "string" &&
        (key !== "id" || message[key].length > 0),
      `message ${key}`,
    );
    cleaned[key] = message[key];
  }
  for (const key of ["created_at", "updated_at", "edited_at"]) {
    if (!Object.hasOwn(message, key)) continue;
    require_shape(valid_timestamp(message[key]), `message ${key}`);
    cleaned[key] = message[key];
  }
  if (Object.hasOwn(message, "status")) {
    require_shape(
      store_config.message_statuses.includes(message.status),
      "message status",
    );
    cleaned.status =
      recover_interrupted && message.status === "streaming"
        ? "stopped"
        : message.status;
  }
  if (Object.hasOwn(message, "model")) {
    require_shape(valid_model(message.model), "message model");
    cleaned.model = message.model;
  }
  if (Object.hasOwn(message, "thinking")) {
    require_shape(typeof message.thinking === "boolean", "message thinking");
    cleaned.thinking = message.thinking;
  }
  if (Object.hasOwn(message, "feedback")) {
    require_shape(
      [null, "up", "down"].includes(message.feedback),
      "message feedback",
    );
    cleaned.feedback = message.feedback;
  }
  if (Object.hasOwn(message, "usage"))
    cleaned.usage = clean_usage(message.usage);
  if (Object.hasOwn(message, "attachments")) {
    require_shape(
      Array.isArray(message.attachments) &&
        message.attachments.length <= app_config.max_attachments,
      "message attachments",
    );
    cleaned.attachments = message.attachments.map(clean_attachment);
  }
  return cleaned;
}

function clean_artifact(artifact) {
  if (artifact === null) return null;
  require_shape(
    is_record(artifact) && typeof artifact.content === "string",
    "conversation artifact",
  );
  const cleaned = { content: artifact.content };
  for (const key of ["id", "title", "language", "type"]) {
    if (!Object.hasOwn(artifact, key)) continue;
    require_shape(typeof artifact[key] === "string", `artifact ${key}`);
    cleaned[key] = artifact[key];
  }
  return cleaned;
}

function clean_conversation(conversation, recover_interrupted = false) {
  require_shape(is_record(conversation), "conversation");
  require_shape(
    typeof conversation.id === "string" && conversation.id.length > 0,
    "conversation id",
  );
  require_shape(typeof conversation.title === "string", "conversation title");
  require_shape(
    valid_timestamp(conversation.created_at) &&
      valid_timestamp(conversation.updated_at),
    "conversation timestamps",
  );
  require_shape(
    typeof conversation.pinned === "boolean" &&
      typeof conversation.thinking === "boolean",
    "conversation flags",
  );
  require_shape(
    typeof conversation.artifact_open === "boolean",
    "artifact open flag",
  );
  require_shape(valid_model(conversation.model), "conversation model");
  require_shape(Array.isArray(conversation.messages), "conversation messages");
  const messages = conversation.messages.map((message) =>
    clean_message(message, recover_interrupted),
  );
  const message_ids = messages
    .filter((message) => message.id !== undefined)
    .map((message) => message.id);
  require_shape(
    new Set(message_ids).size === message_ids.length,
    "duplicate message id",
  );
  return {
    id: conversation.id,
    title: conversation.title,
    created_at: conversation.created_at,
    updated_at: conversation.updated_at,
    pinned: conversation.pinned,
    messages,
    model: conversation.model,
    thinking: conversation.thinking,
    artifact: clean_artifact(conversation.artifact),
    artifact_open: conversation.artifact_open,
  };
}

function clean_workspace(workspace, recover_interrupted = false) {
  require_shape(
    is_record(workspace) && Array.isArray(workspace.conversations),
    "workspace",
  );
  const conversations = workspace.conversations.map((conversation) =>
    clean_conversation(conversation, recover_interrupted),
  );
  const conversation_ids = new Set(
    conversations.map((conversation) => conversation.id),
  );
  require_shape(
    conversation_ids.size === conversations.length,
    "duplicate conversation id",
  );
  require_shape(
    workspace.active_id === null || conversation_ids.has(workspace.active_id),
    "active conversation id",
  );
  return {
    conversations,
    settings: clean_settings(workspace.settings),
    active_id: workspace.active_id,
  };
}

function empty_workspace(storage_warning = null) {
  return {
    conversations: [],
    settings: { ...default_settings },
    active_id: null,
    storage_warning,
  };
}

/** Returns browser-local data only; storage_warning blocks autosave until an explicit reset. */
export function load_workspace() {
  let raw;
  try {
    raw = globalThis.localStorage.getItem(app_config.storage_key);
  } catch {
    return empty_workspace(
      "Browser storage could not be read. Local data has not been changed; enable storage before saving.",
    );
  }
  if (raw === null) return empty_workspace();
  try {
    return { ...clean_workspace(JSON.parse(raw), true), storage_warning: null };
  } catch {
    return empty_workspace(store_config.corrupt_warning);
  }
}

export function save_workspace(workspace) {
  const storage = globalThis.localStorage;
  const raw = storage.getItem(app_config.storage_key);
  if (raw !== null) {
    try {
      clean_workspace(JSON.parse(raw));
    } catch {
      throw new Error(store_config.corrupt_warning);
    }
  }
  const content = JSON.stringify(clean_workspace(workspace));
  storage.setItem(app_config.storage_key, content);
}

/** Destructive: callers must obtain explicit user confirmation before invoking this. */
export function reset_workspace() {
  globalThis.localStorage.removeItem(app_config.storage_key);
  return empty_workspace();
}

function render_markdown(conversation) {
  const sections = [`# ${conversation.title}`, `Model: ${conversation.model}`];
  for (const message of conversation.messages) {
    sections.push(
      `## ${message.role === "user" ? "User" : "Assistant"}`,
      message.content,
    );
    for (const attachment of message.attachments ?? []) {
      sections.push(
        `Attachment: ${attachment.name} (${attachment.type}, ${attachment.size} bytes)`,
      );
      if (attachment.kind === "text") sections.push(attachment.data);
    }
  }
  if (conversation.artifact) {
    sections.push(
      `## ${conversation.artifact.title || "Artifact"}`,
      conversation.artifact.content,
    );
  }
  return `${sections.join("\n\n")}\n`;
}

function check_export_format(format) {
  require_shape(
    ["json", "markdown", "md"].includes(format),
    "export format (choose json or markdown)",
  );
}

export function export_conversation(conversation, format = "json") {
  check_export_format(format);
  const cleaned = clean_conversation(conversation);
  return format === "json"
    ? JSON.stringify(cleaned, null, 2)
    : render_markdown(cleaned);
}

export function export_all(list, format = "json") {
  check_export_format(format);
  require_shape(Array.isArray(list), "conversation list");
  const cleaned = list.map((conversation) => clean_conversation(conversation));
  return format === "json"
    ? JSON.stringify(cleaned, null, 2)
    : cleaned.map(render_markdown).join("\n---\n\n");
}

export function download_file(name, content, type = "text/plain") {
  require_shape(
    typeof name === "string" && name.length > 0,
    "download filename",
  );
  require_shape(
    typeof content === "string" && typeof type === "string",
    "download content",
  );
  const blob = new Blob([content], { type });
  const anchor = document.createElement("a");
  const object_url = URL.createObjectURL(blob);
  try {
    anchor.href = object_url;
    anchor.download = name;
    document.body.append(anchor);
    anchor.click();
  } finally {
    anchor.remove();
    setTimeout(() => URL.revokeObjectURL(object_url), 0);
  }
}

/**
 * @param {File} file
 * @param {(percent: number) => void} [on_progress]
 * @returns {Promise<{id: string, name: string, size: number, type: string, kind: 'image'|'pdf'|'text', data: string}>}
 */
export async function prepare_attachment(file, on_progress = () => {}) {
  require_shape(
    file && typeof file.name === "string" && typeof file.type === "string",
    "attachment file",
  );
  require_shape(
    Number.isSafeInteger(file.size) && file.size >= 0,
    "attachment size",
  );
  if (file.size > app_config.max_attachment_bytes)
    throw new Error("Attachments must be 10 MiB (10 MB) or smaller.");
  require_shape(
    typeof on_progress === "function",
    "attachment progress callback",
  );
  const extension = file.name.split(".").at(-1).toLowerCase();
  const type =
    file.type.toLowerCase() || store_config.extension_types[extension] || "";
  const kind = attachment_kind(type);
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    let settled = false;
    let last_progress = -1;
    const cleanup = () => {
      reader.onload = null;
      reader.onerror = null;
      reader.onabort = null;
      reader.onprogress = null;
    };
    const fail = (error) => {
      if (settled) return;
      settled = true;
      cleanup();
      reject(error);
    };
    const report_progress = (percent) => {
      if (settled || percent <= last_progress) return;
      last_progress = percent;
      try {
        on_progress(percent);
      } catch {
        fail(new Error("Attachment progress callback failed."));
      }
    };
    reader.onprogress = (event) => {
      if (event.lengthComputable && event.total > 0)
        report_progress(
          Math.min(99, Math.floor((event.loaded / event.total) * 100)),
        );
    };
    reader.onerror = () =>
      fail(
        new Error("The attachment could not be read. Choose the file again."),
      );
    reader.onabort = () =>
      fail(new DOMException("Attachment reading was cancelled.", "AbortError"));
    reader.onload = () => {
      if (settled) return;
      try {
        require_shape(
          typeof reader.result === "string",
          "attachment read result",
        );
        const result = reader.result;
        const base64_marker = result.indexOf(";base64,");
        if (kind !== "text")
          require_shape(
            result.startsWith("data:") && base64_marker >= 0,
            "attachment read encoding",
          );
        const data =
          kind === "text"
            ? result
            : result.slice(base64_marker + ";base64,".length);
        const attachment = clean_attachment({
          id: uid(),
          name: file.name,
          size: file.size,
          type,
          kind,
          data,
        });
        report_progress(100);
        if (settled) return;
        settled = true;
        cleanup();
        resolve(attachment);
      } catch {
        fail(
          new Error("The attachment could not be read as a supported file."),
        );
      }
    };
    report_progress(0);
    if (settled) return;
    try {
      if (kind === "text") reader.readAsText(file, "UTF-8");
      else reader.readAsDataURL(file);
    } catch {
      fail(
        new Error("The attachment could not be read. Choose the file again."),
      );
    }
  });
}
