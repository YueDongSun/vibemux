import test from "node:test";
import assert from "node:assert/strict";
import { app_config, default_settings } from "../src/config.js";
import {
  create_conversation,
  load_workspace,
  save_workspace,
} from "../src/services/chat_store.js";
function storage(context, initial) {
  const values = new Map(
      initial === undefined ? [] : [[app_config.storage_key, initial]],
    ),
    writes = [];
  const original = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (key) => values.get(key) ?? null,
      setItem: (key, value) => {
        values.set(key, value);
        writes.push(value);
      },
    },
  });
  context.after(() => {
    if (original) Object.defineProperty(globalThis, "localStorage", original);
    else delete globalThis.localStorage;
  });
  return { values, writes };
}
function fixture() {
  const chat = create_conversation(default_settings);
  chat.messages = [
    { id: "user", role: "user", content: "Keep this conversation" },
  ];
  chat.artifact = {
    title: "Keep this artifact",
    language: "html",
    content: "<h1>Hello</h1>",
  };
  return {
    conversations: [chat],
    settings: { ...default_settings },
    active_id: chat.id,
  };
}
test("legacy settings gain Claude presentation without losing history or rewriting storage", (context) => {
  const value = fixture();
  delete value.settings.interface_style;
  const raw = JSON.stringify(value);
  const saved = storage(context, raw);
  const loaded = load_workspace();
  assert.equal(loaded.settings.interface_style, "claude");
  assert.equal(loaded.active_id, value.active_id);
  assert.equal(loaded.conversations[0].artifact.content, "<h1>Hello</h1>");
  assert.equal(
    loaded.conversations[0].messages[0].content,
    "Keep this conversation",
  );
  assert.equal(saved.writes.length, 0);
  save_workspace(loaded);
  assert.equal(saved.writes.length, 1);
});
test("Codex presentation persists through save and reload", (context) => {
  storage(context);
  const value = fixture();
  value.settings.interface_style = "codex";
  save_workspace(value);
  assert.equal(load_workspace().settings.interface_style, "codex");
});
test("invalid persisted presentation preserves the corrupt-data guard", (context) => {
  const value = fixture();
  value.settings.interface_style = "unknown";
  const raw = JSON.stringify(value);
  const saved = storage(context, raw);
  assert.match(load_workspace().storage_warning, /preserved/i);
  assert.throws(() => save_workspace(value), /reset/i);
  assert.equal(saved.values.get(app_config.storage_key), raw);
  assert.equal(saved.writes.length, 0);
});
