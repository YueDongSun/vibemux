import test from "node:test";
import assert from "node:assert/strict";
import { default_settings } from "../src/config.js";
import {
  create_conversation,
  save_workspace,
  load_workspace,
  export_conversation,
} from "../src/services/chat_store.js";
import {
  group_conversations,
  extract_artifact,
  stopped_snapshot,
} from "../src/ui_policy.js";

test("a UI conversation saves UTC timestamps and restores local feedback", (context) => {
  const values = new Map();
  const original = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (key) => values.get(key) ?? null,
      setItem: (key, value) => values.set(key, value),
    },
  });
  context.after(() => {
    if (original) Object.defineProperty(globalThis, "localStorage", original);
    else delete globalThis.localStorage;
  });
  const chat = create_conversation(default_settings);
  assert.match(chat.created_at, /^\d{4}-\d{2}-\d{2}T.*Z$/);
  chat.messages = [
    {
      id: "user_1",
      role: "user",
      content: "A clear question",
      created_at: "2026-10-25T08:00:00.000Z",
      attachments: [],
    },
    {
      id: "reply_1",
      role: "assistant",
      content: "A useful answer",
      created_at: "2026-10-25T08:00:01.000Z",
      status: "complete",
      feedback: "up",
    },
  ];
  save_workspace({
    conversations: [chat],
    settings: default_settings,
    active_id: chat.id,
  });
  const restored = load_workspace();
  assert.equal(restored.storage_warning, null);
  assert.equal(restored.conversations[0].messages[1].feedback, "up");
  assert.equal(
    restored.conversations[0].messages[0].created_at,
    "2026-10-25T08:00:00.000Z",
  );
});

test("artifact extraction keeps literal backticks and longer Markdown fences intact", () => {
  const source = '<script>\nconst literal = "```";\n</script>';
  assert.equal(
    extract_artifact("```html\n" + source + "\n```").content,
    source,
  );
  const nested = "<main>\n```\nA literal fence\n```\n</main>";
  assert.equal(
    extract_artifact("````html\n" + nested + "\n````").content,
    nested,
  );
});

test("undo snapshots stop an interrupted response and preserve the partial text", () => {
  const chat = {
    id: "chat_1",
    messages: [
      {
        id: "reply_1",
        role: "assistant",
        status: "streaming",
        content: "A partial answer",
      },
    ],
  };
  const restored = stopped_snapshot(chat);
  assert.equal(restored.messages[0].status, "stopped");
  assert.equal(restored.messages[0].content, "A partial answer");
  assert.equal(chat.messages[0].status, "streaming");
});

test("history groups respect local day boundaries and keep pinning separate", () => {
  const now = new Date(2026, 9, 25, 12);
  const groups = group_conversations(
    [
      {
        id: "p",
        pinned: true,
        updated_at: new Date(2026, 9, 20).toISOString(),
      },
      {
        id: "t",
        pinned: false,
        updated_at: new Date(2026, 9, 25, 0, 1).toISOString(),
      },
      {
        id: "y",
        pinned: false,
        updated_at: new Date(2026, 9, 24, 23, 59).toISOString(),
      },
      {
        id: "o",
        pinned: false,
        updated_at: new Date(2026, 9, 23, 23, 59).toISOString(),
      },
    ],
    now,
  );
  assert.deepEqual(
    groups.map(([name, items]) => [name, items[0].id]),
    [
      ["Pinned", "p"],
      ["Today", "t"],
      ["Yesterday", "y"],
      ["Older", "o"],
    ],
  );
});

test("duplicated UI-shaped user messages remain valid through export and reload", (context) => {
  const values = new Map();
  const original = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (key) => values.get(key) ?? null,
      setItem: (key, value) => values.set(key, value),
    },
  });
  context.after(() => {
    if (original) Object.defineProperty(globalThis, "localStorage", original);
    else delete globalThis.localStorage;
  });
  const source = create_conversation(default_settings);
  source.messages = [
    {
      id: "user_without_status",
      role: "user",
      content: "A real UI-shaped message",
      attachments: [],
    },
    { id: "reply", role: "assistant", content: "A reply", status: "complete" },
  ];
  const copy = stopped_snapshot(source);
  copy.id = "duplicate";
  copy.title = "Copy";
  copy.messages = copy.messages.map((message, index) => ({
    ...message,
    id: "copy_" + index,
  }));
  save_workspace({
    conversations: [source, copy],
    settings: default_settings,
    active_id: copy.id,
  });
  const exported = JSON.parse(export_conversation(copy, "json"));
  assert.equal(Object.hasOwn(exported.messages[0], "status"), false);
  const restored = load_workspace();
  assert.equal(restored.storage_warning, null);
  assert.equal(restored.conversations.length, 2);
  assert.equal(restored.active_id, "duplicate");
  assert.equal(
    restored.conversations[1].messages[0].content,
    "A real UI-shaped message",
  );
});
