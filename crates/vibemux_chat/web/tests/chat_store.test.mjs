import { save_workspace, reset_workspace } from "./support/store_writer.mjs";
import test from "node:test";
import assert from "node:assert/strict";
import { app_config, default_settings } from "../src/config.js";
import {
  create_conversation,
  create_workspace_session,
  load_workspace,
  export_conversation,
  export_all,
  download_file,
} from "../src/services/chat_store.js";

function install_storage(context, initial = null) {
  const values = new Map(
    initial === null ? [] : [[app_config.storage_key, initial]],
  );
  const writes = [];
  context.mock.method(globalThis, "fetch", () => {
    throw new Error("Network is forbidden in store tests");
  });
  const original = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (key) => values.get(key) ?? null,
      setItem: (key, value) => {
        writes.push(value);
        values.set(key, value);
      },
      removeItem: (key) => values.delete(key),
    },
  });
  context.after(() => {
    if (original) Object.defineProperty(globalThis, "localStorage", original);
    else delete globalThis.localStorage;
  });
  return { values, writes };
}

function workspace_with_messages(messages = []) {
  const conversation = create_conversation(default_settings);
  conversation.messages = messages;
  return {
    conversations: [conversation],
    settings: { ...default_settings },
    active_id: conversation.id,
  };
}

test("new workspace and conversation use independent defaults and browser-local IDs", (context) => {
  const storage = install_storage(context);
  const workspace = load_workspace();
  assert.deepEqual(workspace.conversations, []);
  assert.deepEqual(workspace.settings, default_settings);
  assert.equal(workspace.active_id, null);
  assert.equal(workspace.storage_warning, null);
  const first = create_conversation({
    ...default_settings,
    default_thinking: true,
  });
  const second = create_conversation(default_settings);
  assert.notEqual(first.id, second.id);
  assert.equal(first.title, "New conversation");
  assert.equal(first.model, default_settings.default_model);
  assert.equal(first.thinking, true);
  assert.equal(first.pinned, false);
  assert.equal(first.artifact, null);
  assert.equal(first.artifact_open, false);
  assert.equal(first.created_at, first.updated_at);
  assert.match(first.created_at, /^\d{4}-\d{2}-\d{2}T.*Z$/);
  first.messages.push({ role: "user", content: "local only" });
  assert.deepEqual(second.messages, []);
  workspace.settings.theme = "dark";
  assert.equal(default_settings.theme, "system");
  assert.equal(storage.writes.length, 0);
});

test("reload stops interrupted assistant messages without losing partial text or rewriting storage", (context) => {
  const storage = install_storage(context);
  const workspace = workspace_with_messages([
    {
      id: "user_1",
      role: "user",
      content: "Explain",
      attachments: [],
      status: "complete",
    },
    {
      id: "assistant_1",
      role: "assistant",
      content: "Partial 🌍",
      status: "streaming",
    },
  ]);
  save_workspace(workspace);
  const saved = storage.values.get(app_config.storage_key);
  assert.equal(
    JSON.parse(saved).conversations[0].messages[1].status,
    "streaming",
  );
  const restored = load_workspace();
  assert.equal(restored.conversations[0].messages[1].status, "stopped");
  assert.equal(restored.conversations[0].messages[1].content, "Partial 🌍");
  assert.equal(restored.conversations[0].messages[0].status, "complete");
  assert.equal(storage.values.get(app_config.storage_key), saved);
  assert.equal(storage.writes.length, 1);
});

test("legacy messages receive distinct deterministic IDs without rewriting saved history", (context) => {
  const messages = [
    { role: "user", content: "Repeated question", attachments: [] },
    { role: "assistant", content: "Repeated answer", status: "complete" },
    { role: "user", content: "Repeated question", attachments: [] },
    { role: "assistant", content: "Repeated answer", status: "streaming" },
  ];
  const original = JSON.stringify(workspace_with_messages(messages));
  const storage = install_storage(context, original);
  const restored = load_workspace();
  assert.equal(restored.storage_warning, null);
  const normalized = restored.conversations[0].messages;
  const ids = normalized.map((message) => message.id);
  assert.ok(ids.every((id) => typeof id === "string" && id.length > 0));
  assert.equal(new Set(ids).size, messages.length);
  assert.deepEqual(
    normalized.map(({ id, ...message }) => message),
    messages.map((message) => message.status === "streaming"
      ? { ...message, status: "stopped" }
      : message),
  );
  assert.deepEqual(load_workspace(), restored);
  assert.deepEqual(create_workspace_session().initial, restored);
  assert.equal(storage.values.get(app_config.storage_key), original);
  assert.equal(storage.writes.length, 0);
});

test("legacy ID normalization reserves supplied IDs even after missing messages", (context) => {
  const supplied_ids = [
    undefined,
    "provided_message",
    undefined,
    "legacy_message_0",
    "legacy_message_1",
    "legacy_message_3",
    undefined,
  ];
  const messages = supplied_ids.map((id, index) => ({
    ...(id === undefined ? {} : { id }),
    role: index % 2 === 0 ? "user" : "assistant",
    content: `Message ${index}`,
  }));
  const original = JSON.stringify(workspace_with_messages(messages));
  const storage = install_storage(context, original);
  const restored = load_workspace();
  assert.equal(restored.storage_warning, null);
  const normalized = restored.conversations[0].messages;
  const ids = normalized.map((message) => message.id);
  assert.ok(ids.every((id) => typeof id === "string" && id.length > 0));
  assert.equal(new Set(ids).size, messages.length);
  supplied_ids.forEach((id, index) => {
    if (id !== undefined) assert.equal(ids[index], id);
    else assert.equal(supplied_ids.includes(ids[index]), false);
  });
  assert.deepEqual(
    normalized.map((message) => message.content),
    messages.map((message) => message.content),
  );
  assert.deepEqual(load_workspace(), restored);
  assert.equal(storage.values.get(app_config.storage_key), original);
  assert.equal(storage.writes.length, 0);
});

test("normalized legacy IDs survive export, save and reload without changing", (context) => {
  const workspace = workspace_with_messages([
    { role: "user", content: "First question" },
    { id: "provided_reply", role: "assistant", content: "First reply" },
    { role: "user", content: "Follow-up" },
    { role: "assistant", content: "Second reply" },
  ]);
  const original = JSON.stringify(workspace);
  const storage = install_storage(context, original);
  const restored = load_workspace();
  const ids = restored.conversations[0].messages.map((message) => message.id);
  assert.ok(ids.every((id) => typeof id === "string" && id.length > 0));
  assert.equal(new Set(ids).size, 4);
  const exported = JSON.parse(export_conversation(restored.conversations[0]));
  assert.deepEqual(exported.messages.map((message) => message.id), ids);
  assert.deepEqual(
    JSON.parse(export_conversation(workspace.conversations[0])),
    exported,
  );
  assert.equal(JSON.stringify(workspace), original);
  assert.equal(storage.writes.length, 0);

  save_workspace(restored);
  const saved = storage.values.get(app_config.storage_key);
  assert.deepEqual(
    JSON.parse(saved).conversations[0].messages.map((message) => message.id),
    ids,
  );
  assert.deepEqual(load_workspace(), restored);
  assert.deepEqual(create_workspace_session().initial, restored);
  save_workspace(load_workspace());
  assert.equal(storage.values.get(app_config.storage_key), saved);
});

test("corrupt storage remains byte-for-byte intact and blocks saving until explicit reset", (context) => {
  const original = "{broken JSON with valuable data";
  const storage = install_storage(context, original);
  const workspace = load_workspace();
  assert.match(workspace.storage_warning, /preserved.*reset/i);
  assert.deepEqual(workspace.conversations, []);
  assert.throws(() => save_workspace(workspace), /reset/i);
  assert.equal(storage.values.get(app_config.storage_key), original);
  assert.equal(storage.writes.length, 0);
  const reset = reset_workspace();
  assert.equal(reset.storage_warning, null);
  save_workspace(reset);
  assert.deepEqual(
    JSON.parse(storage.values.get(app_config.storage_key)).conversations,
    [],
  );
});

for (const [description, change] of [
  [
    "settings type",
    (workspace) => {
      workspace.settings = [];
    },
  ],
  [
    "theme",
    (workspace) => {
      workspace.settings.theme = "neon";
    },
  ],
  [
    "boolean setting",
    (workspace) => {
      workspace.settings.auto_scroll = "yes";
    },
  ],
  [
    "default model",
    (workspace) => {
      workspace.settings.default_model = "unknown";
    },
  ],
  [
    "conversation collection",
    (workspace) => {
      workspace.conversations = {};
    },
  ],
  [
    "missing active conversation",
    (workspace) => {
      workspace.active_id = "missing";
    },
  ],
  [
    "duplicate conversation ID",
    (workspace) => {
      workspace.conversations.push(workspace.conversations[0]);
    },
  ],
  [
    "duplicate provided message ID",
    (workspace) => {
      workspace.conversations[0].messages = [
        { role: "user", content: "Legacy question" },
        { id: "legacy_message_0", role: "assistant", content: "First reply" },
        { id: "legacy_message_0", role: "user", content: "Duplicate ID" },
      ];
    },
  ],
  ...["", null, 0].map((id) => [
    `provided message ID ${JSON.stringify(id)}`,
    (workspace) => {
      workspace.conversations[0].messages = [
        { id, role: "user", content: "Invalid ID is not a missing ID" },
      ];
    },
  ]),
  [
    "invalid timestamp",
    (workspace) => {
      workspace.conversations[0].created_at = "yesterday";
    },
  ],
  [
    "invalid role",
    (workspace) => {
      workspace.conversations[0].messages.push({
        role: "system",
        content: "bad",
      });
    },
  ],
  [
    "invalid content",
    (workspace) => {
      workspace.conversations[0].messages.push({ role: "user", content: {} });
    },
  ],
  [
    "invalid status",
    (workspace) => {
      workspace.conversations[0].messages.push({
        role: "assistant",
        content: "",
        status: "pretend_success",
      });
    },
  ],
  [
    "invalid usage",
    (workspace) => {
      workspace.conversations[0].messages.push({
        role: "assistant",
        content: "",
        usage: { output_tokens: -1 },
      });
    },
  ],
  [
    "invalid attachments",
    (workspace) => {
      workspace.conversations[0].messages.push({
        role: "user",
        content: "",
        attachments: [{}],
      });
    },
  ],
]) {
  test(`invalid stored ${description} is preserved rather than partially discarded`, (context) => {
    const workspace = workspace_with_messages();
    change(workspace);
    const original = JSON.stringify(workspace);
    const storage = install_storage(context, original);
    assert.match(load_workspace().storage_warning, /preserved.*reset/i);
    assert.throws(() => save_workspace(workspace_with_messages()), /reset/i);
    assert.equal(storage.values.get(app_config.storage_key), original);
    assert.equal(storage.writes.length, 0);
  });
}

test("quota failure throws and does not replace the last saved workspace", (context) => {
  const workspace = workspace_with_messages();
  const original = JSON.stringify(workspace);
  const storage = install_storage(context, original);
  const quota_error = new DOMException("Storage is full", "QuotaExceededError");
  context.mock.method(localStorage, "setItem", () => {
    throw quota_error;
  });
  assert.throws(
    () => save_workspace(workspace),
    (error) => error === quota_error,
  );
  assert.equal(storage.values.get(app_config.storage_key), original);
});

test("blocked browser storage surfaces a warning and never reports a successful save", (context) => {
  const storage = install_storage(context);
  context.mock.method(localStorage, "getItem", () => {
    throw new DOMException("Blocked", "SecurityError");
  });
  assert.match(load_workspace().storage_warning, /could not be read/i);
  assert.throws(() => save_workspace(workspace_with_messages()), /could not be read/);
  assert.equal(storage.writes.length, 0);
  assert.equal(storage.values.size, 0);
});

test("persistence and exports whitelist chat data, excluding API keys and transient state", (context) => {
  const storage = install_storage(context);
  const workspace = workspace_with_messages([
    {
      role: "assistant",
      content: "A useful answer",
      usage: { input_tokens: 12, output_tokens: 4, api_key: "fixture_secret" },
      api_key: "fixture_secret",
    },
  ]);
  workspace.api_key = "fixture_secret";
  workspace.settings.api_key = "fixture_secret";
  workspace.conversations[0].api_key = "fixture_secret";
  workspace.conversations[0].artifact = {
    title: "Snippet",
    language: "js",
    content: "const value = 1;",
    api_key: "fixture_secret",
  };
  save_workspace(workspace);
  const stored = storage.values.get(app_config.storage_key);
  assert.doesNotMatch(stored, /fixture_secret|api_key|storage_warning/);
  const conversation = workspace.conversations[0];
  const json = export_conversation(conversation, "json");
  assert.equal(JSON.parse(json).messages[0].content, "A useful answer");
  assert.doesNotMatch(json, /fixture_secret|api_key/);
  assert.equal(JSON.parse(export_all([conversation], "json")).length, 1);
  const markdown = export_conversation(conversation, "markdown");
  assert.match(markdown, /# New conversation/);
  assert.match(markdown, /## Assistant/);
  assert.match(markdown, /A useful answer/);
  assert.match(markdown, /const value = 1/);
  assert.doesNotMatch(markdown, /fixture_secret|api_key/);
  assert.match(export_all([conversation, conversation], "md"), /\n---\n/);
  assert.throws(() => export_conversation(conversation, "html"), /format/i);
});

test("download creates a real Blob and releases the object URL and anchor", async (context) => {
  const events = [];
  let downloaded_blob;
  context.mock.method(URL, "createObjectURL", (blob) => {
    downloaded_blob = blob;
    return "blob:fixture";
  });
  context.mock.method(URL, "revokeObjectURL", (url) =>
    events.push(["revoke", url]),
  );
  const anchor = {
    click: () => events.push(["click"]),
    remove: () => events.push(["remove"]),
  };
  const original = Object.getOwnPropertyDescriptor(globalThis, "document");
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: {
      createElement: (tag) => {
        assert.equal(tag, "a");
        return anchor;
      },
      body: {
        append: (element) => {
          assert.equal(element, anchor);
          events.push(["append"]);
        },
      },
    },
  });
  context.after(() => {
    if (original) Object.defineProperty(globalThis, "document", original);
    else delete globalThis.document;
  });
  download_file("conversation.json", '{"text":"🌍"}', "application/json");
  assert.equal(anchor.download, "conversation.json");
  assert.equal(anchor.href, "blob:fixture");
  assert.equal(downloaded_blob.type, "application/json");
  assert.equal(await downloaded_blob.text(), '{"text":"🌍"}');
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.deepEqual(events, [
    ["append"],
    ["click"],
    ["remove"],
    ["revoke", "blob:fixture"],
  ]);
});
