import test from "node:test";
import assert from "node:assert/strict";
import { app_config, default_settings } from "../src/config.js";
import {
  create_conversation,
  load_workspace,
  save_workspace,
  reset_workspace,
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
  install_storage(context);
  context.mock.method(localStorage, "getItem", () => {
    throw new DOMException("Blocked", "SecurityError");
  });
  assert.match(load_workspace().storage_warning, /could not be read/i);
  assert.throws(() => save_workspace(workspace_with_messages()), /Blocked/);
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
