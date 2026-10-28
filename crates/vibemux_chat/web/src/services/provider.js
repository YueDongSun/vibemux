import { app_config, default_settings, models } from "../config.js";

const provider_config = {
  messages_url: "https://api.anthropic.com/v1/messages",
  models_url: "https://api.anthropic.com/v1/models",
  api_version: "2023-06-01",
  max_tokens: 16384,
  connect_timeout_ms: 30000,
  idle_timeout_ms: 60000,
  response_timeout_ms: 600000,
  validation_timeout_ms: 30000,
  max_event_characters: 1048576,
  max_response_characters: 8388608,
  max_request_bytes: 32 * 1024 * 1024,
  max_models_bytes: 1048576,
  image_types: ["image/png", "image/jpeg", "image/gif", "image/webp"],
  text_types: [
    "application/json",
    "application/javascript",
    "application/xml",
    "application/yaml",
  ],
  usage_fields: [
    "input_tokens",
    "output_tokens",
    "cache_creation_input_tokens",
    "cache_read_input_tokens",
  ],
};

class Provider_Error extends Error {
  constructor(code, message, status) {
    super(message);
    this.name = "Provider_Error";
    this.code = code;
    if (status !== undefined) this.status = status;
  }
}

function require_input(condition, message) {
  if (!condition) throw new Provider_Error("invalid_request", message);
}

function require_protocol(condition) {
  if (!condition)
    throw new Provider_Error(
      "invalid_stream",
      "The provider returned an invalid or incomplete response stream.",
    );
}

function is_record(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function clean_key(key) {
  if (typeof key !== "string" || !key.trim()) {
    throw new Provider_Error(
      "missing_api_key",
      "Add an Anthropic API key in Settings before sending a message. The key is kept in memory only.",
    );
  }
  const trimmed = key.trim();
  require_input(
    trimmed.length <= 2048 && !/\s/.test(trimmed),
    "The API key has an invalid format. Enter the key again in Settings.",
  );
  return trimmed;
}

function request_headers(api_key) {
  return {
    "content-type": "application/json",
    "x-api-key": api_key,
    "anthropic-version": provider_config.api_version,
    "anthropic-dangerous-direct-browser-access": "true",
  };
}

function http_error(status) {
  const messages = {
    400: "The provider rejected the request. Check the selected model, message size, and attachments.",
    401: "The API key was rejected. Enter a valid Anthropic API key in Settings.",
    403: "This API key does not have permission to access the requested resource.",
    404: "The selected model or API resource is not available to this API key.",
    413: "The provider rejected the request because it is too large. Remove some attachments or messages.",
    429: "The provider rate limit or credit limit was reached. Check your account and retry later.",
    529: "The provider is overloaded. Retry later.",
  };
  return new Provider_Error(
    "provider_http_error",
    messages[status] ||
      (status >= 500
        ? "The provider is temporarily unavailable. Retry later."
        : "The provider could not accept the request."),
    status,
  );
}

function cancel_body(body) {
  try {
    body?.cancel().catch(() => {});
  } catch {
    /* Cleanup cannot replace the request outcome. */
  }
}

function create_request_scope(signal, timeout_ms) {
  const controller = new AbortController();
  let failure;
  let reader;
  let connect_timer;
  let idle_timer;
  let total_timer;
  let reject_abort;
  const aborted = new Promise((resolve, reject) => {
    reject_abort = reject;
  });
  aborted.catch(() => {});
  const stop = (error) => {
    if (controller.signal.aborted) return;
    failure = error;
    controller.abort();
    reject_abort(error);
    if (reader) {
      try {
        reader.cancel().catch(() => {});
      } catch {
        /* The abort still rejects the pending operation. */
      }
    }
  };
  const timeout = () =>
    stop(
      new Provider_Error(
        "timeout",
        "The provider request timed out. Retry when the connection is stable.",
      ),
    );
  const on_abort = () =>
    stop(new DOMException("The provider request was cancelled.", "AbortError"));
  if (signal) {
    require_input(
      typeof signal.aborted === "boolean" &&
        typeof signal.addEventListener === "function" &&
        typeof signal.removeEventListener === "function",
      "Invalid cancellation signal.",
    );
    signal.addEventListener("abort", on_abort, { once: true });
    if (signal.aborted) on_abort();
  }
  if (!controller.signal.aborted) {
    connect_timer = setTimeout(timeout, provider_config.connect_timeout_ms);
    total_timer = setTimeout(timeout, timeout_ms);
  }
  const check = () => {
    if (failure) throw failure;
  };
  const activity = () => {
    clearTimeout(idle_timer);
    if (!failure)
      idle_timer = setTimeout(timeout, provider_config.idle_timeout_ms);
  };
  return {
    signal: controller.signal,
    check,
    activity,
    set_reader(value) {
      reader = value;
    },
    connected() {
      clearTimeout(connect_timer);
      activity();
    },
    async wait(operation) {
      return Promise.race([aborted, operation]);
    },
    safe_error(error) {
      if (failure) return failure;
      if (error instanceof Provider_Error) return error;
      return new Provider_Error(
        "connection_error",
        "The provider connection failed or the response was interrupted. Check your network and browser access, then retry.",
      );
    },
    close() {
      clearTimeout(connect_timer);
      clearTimeout(idle_timer);
      clearTimeout(total_timer);
      signal?.removeEventListener("abort", on_abort);
      if (reader) {
        try {
          reader.cancel().catch(() => {});
        } catch {
          /* The stream may already be closed. */
        }
        try {
          reader.releaseLock();
        } catch {
          /* A cancelled read may still be settling. */
        }
      }
    },
  };
}

async function fetch_response(url, options, scope) {
  scope.check();
  const pending = fetch(url, {
    ...options,
    signal: scope.signal,
    credentials: "omit",
    redirect: "error",
    cache: "no-store",
    referrerPolicy: "no-referrer",
  });
  pending.then(
    (response) => {
      if (scope.signal.aborted) cancel_body(response.body);
    },
    () => {},
  );
  const response = await scope.wait(pending);
  scope.check();
  scope.connected();
  if (!response.ok) {
    cancel_body(response.body);
    throw http_error(response.status);
  }
  return response;
}

function attachment_block(attachment) {
  require_input(
    is_record(attachment) &&
      typeof attachment.name === "string" &&
      typeof attachment.type === "string" &&
      typeof attachment.data === "string",
    "An attachment is invalid. Remove it and choose the file again.",
  );
  const { type, kind, data, size, name } = attachment;
  require_input(
    Number.isSafeInteger(size) &&
      size >= 0 &&
      size <= app_config.max_attachment_bytes,
    "Attachments must be 10 MiB (10 MB) or smaller.",
  );
  if (kind === "text") {
    require_input(
      (type.startsWith("text/") || provider_config.text_types.includes(type)) &&
        new TextEncoder().encode(data).byteLength <=
          app_config.max_attachment_bytes,
      "The text attachment is invalid or too large.",
    );
    return {
      type: "document",
      title: name,
      source: { type: "text", media_type: "text/plain", data },
    };
  }
  require_input(
    (kind === "image" && provider_config.image_types.includes(type)) ||
      (kind === "pdf" && type === "application/pdf"),
    "Unsupported attachment type. Choose a PNG, JPEG, GIF, WebP, PDF, or text file.",
  );
  require_input(
    data.length > 0 &&
      data.length <= Math.ceil(app_config.max_attachment_bytes / 3) * 4 &&
      data.length % 4 === 0 &&
      /^[A-Za-z0-9+/]+={0,2}$/.test(data),
    "An attachment has invalid base64 data. Choose the file again.",
  );
  const padding = data.endsWith("==") ? 2 : data.endsWith("=") ? 1 : 0;
  require_input(
    (data.length / 4) * 3 - padding === size,
    "An attachment has an invalid byte length. Choose the file again.",
  );
  const source = { type: "base64", media_type: type, data };
  return kind === "image"
    ? { type: "image", source }
    : { type: "document", title: name, source };
}

function request_body(messages, model, thinking) {
  require_input(
    models.some((entry) => entry.id === model),
    "Choose a supported model in Settings.",
  );
  require_input(
    typeof thinking === "boolean",
    "The thinking setting must be a boolean.",
  );
  require_input(
    Array.isArray(messages) && messages.length > 0,
    "Enter a message before sending.",
  );
  const content_messages = messages.map((message) => {
    require_input(
      is_record(message) &&
        ["user", "assistant"].includes(message.role) &&
        typeof message.content === "string",
      "Conversation context must contain user/assistant roles and plain text content.",
    );
    const attachments =
      message.attachments === undefined ? [] : message.attachments;
    require_input(
      Array.isArray(attachments) &&
        attachments.length <= app_config.max_attachments,
      `A message can contain at most ${app_config.max_attachments} attachments.`,
    );
    require_input(
      message.role === "user" || attachments.length === 0,
      "Only user messages can contain attachments.",
    );
    require_input(
      message.role !== "user" ||
        message.content.length <= app_config.max_characters,
      `Messages must be ${app_config.max_characters} characters or shorter.`,
    );
    const content = attachments.map(attachment_block);
    if (message.content.trim())
      content.push({ type: "text", text: message.content });
    require_input(
      content.length > 0,
      "Empty conversation messages cannot be sent.",
    );
    return { role: message.role, content };
  });
  require_input(
    content_messages[0].role === "user" &&
      content_messages.at(-1).role === "user",
    "The request context must begin and end with a user message. Trim the previous assistant response when regenerating.",
  );
  const payload = {
    model,
    max_tokens: provider_config.max_tokens,
    stream: true,
    messages: content_messages,
  };
  if (thinking && !model.includes("haiku"))
    payload.thinking = { type: "adaptive" };
  const body = JSON.stringify(payload);
  require_input(
    new TextEncoder().encode(body).byteLength <=
      provider_config.max_request_bytes,
    "The conversation and attachments exceed the provider request limit. Remove attachments or start a new conversation.",
  );
  return body;
}

function create_sse_parser(on_event) {
  let buffer = "";
  let event_name = "";
  let data_lines = [];
  let frame_size = 0;
  const dispatch = async () => {
    const data = data_lines.join("\n");
    const name = event_name;
    event_name = "";
    data_lines = [];
    frame_size = 0;
    if (!data) return false;
    let value;
    try {
      value = JSON.parse(data);
    } catch {
      require_protocol(false);
    }
    require_protocol(
      is_record(value) &&
        typeof value.type === "string" &&
        (!name || name === value.type),
    );
    return on_event(value);
  };
  const line_received = async (line) => {
    frame_size += line.length + 1;
    require_protocol(frame_size <= provider_config.max_event_characters);
    if (line === "") return dispatch();
    if (line.startsWith(":")) return false;
    const separator = line.indexOf(":");
    const field = separator < 0 ? line : line.slice(0, separator);
    let value = separator < 0 ? "" : line.slice(separator + 1);
    if (value.startsWith(" ")) value = value.slice(1);
    if (field === "event") event_name = value;
    else if (field === "data") data_lines.push(value);
    return false;
  };
  return async (text, eof = false) => {
    buffer += text;
    while (true) {
      const position = buffer.search(/[\r\n]/);
      if (
        position < 0 ||
        (!eof && buffer[position] === "\r" && position === buffer.length - 1)
      )
        break;
      const line = buffer.slice(0, position);
      const delimiter_length =
        buffer[position] === "\r" && buffer[position + 1] === "\n" ? 2 : 1;
      buffer = buffer.slice(position + delimiter_length);
      if (await line_received(line)) return true;
    }
    require_protocol(
      buffer.length + frame_size <= provider_config.max_event_characters,
    );
    if (eof) {
      if (buffer && (await line_received(buffer))) return true;
      buffer = "";
      return dispatch();
    }
    return false;
  };
}

function create_message_consumer(scope, on_delta, on_usage) {
  let started = false;
  let stop_reason = null;
  let text = "";
  let usage = {};
  const blocks = new Map();
  const update_usage = (value) => {
    if (value === undefined) return;
    require_protocol(is_record(value));
    const updated = { ...usage };
    for (const key of provider_config.usage_fields) {
      if (!Object.hasOwn(value, key)) continue;
      require_protocol(Number.isSafeInteger(value[key]) && value[key] >= 0);
      updated[key] = value[key];
    }
    usage = updated;
    scope.check();
    on_usage({ ...usage });
    scope.check();
  };
  const append_text = (delta) => {
    require_protocol(typeof delta === "string");
    if (!delta) return;
    require_protocol(
      text.length + delta.length <= provider_config.max_response_characters,
    );
    scope.check();
    text += delta;
    on_delta(delta, text);
    scope.check();
  };
  const consume = (value) => {
    scope.check();
    const result = dispatch_event(value);
    scope.check();
    return result;
  };
  const dispatch_event = (value) => {
    scope.check();
    if (value.type === "ping") return false;
    if (value.type === "error") {
      throw new Provider_Error(
        "provider_stream_error",
        "The provider reported a stream error. The response is incomplete; retry later.",
      );
    }
    if (value.type === "message_start") {
      require_protocol(
        !started &&
          is_record(value.message) &&
          value.message.role === "assistant" &&
          Array.isArray(value.message.content) &&
          value.message.content.length === 0,
      );
      started = true;
      update_usage(value.message.usage);
    } else if (value.type === "content_block_start") {
      require_protocol(
        started &&
          !stop_reason &&
          Number.isSafeInteger(value.index) &&
          value.index >= 0 &&
          !blocks.has(value.index) &&
          is_record(value.content_block) &&
          typeof value.content_block.type === "string",
      );
      const block = { type: value.content_block.type, open: true };
      blocks.set(value.index, block);
      require_protocol(
        block.type !== "tool_use" && block.type !== "server_tool_use",
      );
      if (block.type === "text") append_text(value.content_block.text);
    } else if (value.type === "content_block_delta") {
      const block = blocks.get(value.index);
      require_protocol(
        started && !stop_reason && block?.open && is_record(value.delta),
      );
      if (value.delta.type === "text_delta") {
        require_protocol(block.type === "text");
        append_text(value.delta.text);
      }
    } else if (value.type === "content_block_stop") {
      const block = blocks.get(value.index);
      require_protocol(started && !stop_reason && block?.open);
      block.open = false;
    } else if (value.type === "message_delta") {
      require_protocol(
        started &&
          !stop_reason &&
          is_record(value.delta) &&
          [...blocks.values()].every((block) => !block.open),
      );
      if (
        value.delta.stop_reason !== null &&
        value.delta.stop_reason !== undefined
      ) {
        require_protocol(typeof value.delta.stop_reason === "string");
        stop_reason = value.delta.stop_reason;
      }
      update_usage(value.usage);
    } else if (value.type === "message_stop") {
      require_protocol(
        started &&
          stop_reason &&
          [...blocks.values()].every((block) => !block.open),
      );
      if (stop_reason === "max_tokens")
        throw new Provider_Error(
          "response_truncated",
          "The response reached the output token limit and is incomplete. Ask for a shorter response or continue in another turn.",
        );
      if (stop_reason === "refusal")
        throw new Provider_Error(
          "provider_refusal",
          "The provider declined this request. Revise the message before trying again.",
        );
      require_protocol(["end_turn", "stop_sequence"].includes(stop_reason));
      return true;
    }
    return false;
  };
  return { consume, text: () => text };
}

async function read_message_stream(response, scope, on_delta, on_usage) {
  if (
    !/^text\/event-stream(?:\s*;|$)/i.test(
      response.headers.get("content-type") || "",
    ) ||
    !response.body
  ) {
    cancel_body(response.body);
    throw new Provider_Error(
      "invalid_stream",
      "The provider did not return a streaming response. Check browser access and retry.",
    );
  }
  const reader = response.body.getReader();
  scope.set_reader(reader);
  const decoder = new TextDecoder("utf-8", { fatal: true });
  const consumer = create_message_consumer(scope, on_delta, on_usage);
  const parse = create_sse_parser(consumer.consume);
  while (true) {
    const { done, value } = await scope.wait(reader.read());
    scope.check();
    if (done) {
      if (await parse(decoder.decode(), true)) return consumer.text();
      throw new Provider_Error(
        "stream_truncated",
        "The provider stream ended before completion. The response is incomplete; retry when the connection is stable.",
      );
    }
    if (value.byteLength > 0) scope.activity();
    if (await parse(decoder.decode(value, { stream: true })))
      return consumer.text();
  }
}

/** Calls on_delta(delta, accumulated_text) and on_usage(cumulative_usage); returns completed text. */
export async function stream_response({
  messages,
  model = default_settings.default_model,
  thinking = false,
  api_key,
  signal,
  on_delta = () => {},
  on_usage = () => {},
}) {
  const key = clean_key(api_key);
  require_input(
    typeof on_delta === "function" && typeof on_usage === "function",
    "Stream callbacks must be functions.",
  );
  const body = request_body(messages, model, thinking);
  const scope = create_request_scope(
    signal,
    provider_config.response_timeout_ms,
  );
  try {
    const response = await fetch_response(
      provider_config.messages_url,
      { method: "POST", headers: request_headers(key), body },
      scope,
    );
    return await read_message_stream(response, scope, on_delta, on_usage);
  } catch (error) {
    throw scope.safe_error(error);
  } finally {
    scope.close();
  }
}

async function read_models(response, scope) {
  require_protocol(response.body);
  const reader = response.body.getReader();
  scope.set_reader(reader);
  const decoder = new TextDecoder("utf-8", { fatal: true });
  let text = "";
  let byte_count = 0;
  while (true) {
    const { done, value } = await scope.wait(reader.read());
    scope.check();
    if (done) {
      text += decoder.decode();
      break;
    }
    byte_count += value.byteLength;
    require_protocol(byte_count <= provider_config.max_models_bytes);
    if (value.byteLength > 0) scope.activity();
    text += decoder.decode(value, { stream: true });
  }
  let value;
  try {
    value = JSON.parse(text);
  } catch {
    require_protocol(false);
  }
  require_protocol(is_record(value) && Array.isArray(value.data));
  return value.data.map((model) => {
    require_protocol(
      is_record(model) &&
        typeof model.id === "string" &&
        /^claude-[a-z0-9-]+$/.test(model.id),
    );
    const cleaned = { id: model.id };
    for (const key of ["display_name", "created_at"]) {
      if (!Object.hasOwn(model, key)) continue;
      require_protocol(typeof model[key] === "string");
      cleaned[key] = model[key];
    }
    return cleaned;
  });
}

/** Validates credentials with the non-inference Models API; failures throw credential-safe errors. */
export async function validate_api_key(key) {
  const api_key = clean_key(key);
  const scope = create_request_scope(
    undefined,
    provider_config.validation_timeout_ms,
  );
  try {
    const response = await fetch_response(
      provider_config.models_url,
      { method: "GET", headers: request_headers(api_key) },
      scope,
    );
    const available_models = await read_models(response, scope);
    return {
      valid: true,
      message:
        "API key verified through the Models API. No inference was requested.",
      models: available_models,
    };
  } catch (error) {
    throw scope.safe_error(error);
  } finally {
    scope.close();
  }
}
