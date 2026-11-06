import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { parse } from "svelte/compiler";
import { create_workspace_autosave } from "../src/services/workspace_autosave.js";
import { create_conversation, uid, load_workspace } from "../src/services/chat_store.js";
import { stream_response as provider_stream } from "../src/services/provider.js";
import { extract_artifact } from "../src/ui_policy.js";
import { persistence_fixture, settle } from "./support/persistence_fixture.mjs";

// Exercise the app's actual turn functions, located through Svelte's parser.
// The setter below provides the same autosave notification as the app effect;
// this is orchestration coverage, not a DOM/browser rendering assertion.
const source = readFileSync(new URL("../src/app.svelte", import.meta.url), "utf8");
const ast = parse(source);
const functions = ["workspace_snapshot", "update_chat", "generate", "regenerate", "edit_message", "stop", "open_artifact"].map((name) => {
  const node = ast.instance.content.body.find((item) => item.type === "FunctionDeclaration" && item.id.name === name);
  assert.ok(node, `Missing app function ${name}`);
  return source.slice(node.start, node.end);
}).join("\n");

function app_fixture(context, stream_response = provider_stream, chat = create_conversation()) {
  let environment;
  // Flush before the fixture restores global storage and releases its lease.
  context.after(() => environment?.autosave.dispose());
  const storage = persistence_fixture(context);
  const session = storage.session();
  environment = {
    settings: session.initial.settings,
    active_id: chat.id,
    controller: null,
    generating: false,
    storage_ready: true,
    replacement: null,
    replacement_notice: null,
    modal: null,
    online: true,
    notifications: [],
    notify(message) { this.notifications.push(message); },
    api_key: "fixture_in_memory_only",
    $state: { snapshot: structuredClone },
    uid, extract_artifact, stream_response,
  };
  let conversations = [chat];
  Object.defineProperty(environment, "conversations", {
    get: () => conversations,
    set: (value) => {
      conversations = value;
      environment.autosave.schedule(actions.workspace_snapshot());
    },
  });
  Object.defineProperty(environment, "active", {
    get: () => conversations.find((item) => item.id === environment.active_id),
  });
  const actions = new Function("environment", `with (environment) {
    ${functions};
    let pending;
    const original_generate = generate;
    generate = (...args) => (pending = original_generate(...args));
    return { generate, workspace_snapshot, regenerate, edit_message, stop, open_artifact, wait: () => pending };
  }`)(environment);
  const errors = [];
  environment.autosave = create_workspace_autosave({
    session,
    snapshot: actions.workspace_snapshot,
    on_error: (error) => errors.push(error),
    on_saved: () => {},
  });
  return { storage, session, environment, actions, chat, errors };
}

test("app saves the submitted turn before inference, checkpoints deltas, and saves completion immediately", async (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const fixture = app_fixture(context, async ({ messages, on_delta }) => {
    assert.equal(messages[0].content, "Submitted prompt");
    assert.equal(load_workspace().conversations[0].messages[0].content, "Submitted prompt");
    for (let i = 0; i < 25; i++) {
      on_delta("x");
      context.mock.timers.tick(100);
    }
    assert.equal(load_workspace().conversations[0].messages[1].content, "x".repeat(20));
    return "x".repeat(25);
  });
  await fixture.actions.generate(fixture.chat, [{ id: "user", role: "user", content: "Submitted prompt" }]);
  const restored = load_workspace();
  assert.equal(restored.conversations[0].messages[1].status, "complete");
  assert.equal(restored.conversations[0].messages[1].content, "x".repeat(25));
  assert.equal(fixture.environment.generating, false);
  assert.equal(fixture.environment.controller, null);
  assert.deepEqual(fixture.errors, []);
  assert.doesNotMatch(fixture.storage.raw(), /fixture_in_memory_only|api_key/);
});

test("app cancellation persists partial text and stopped status without waiting for debounce", async (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const fixture = app_fixture(context, async ({ on_delta }) => {
    on_delta("Partial response");
    throw new DOMException("Cancelled", "AbortError");
  });
  await fixture.actions.generate(fixture.chat, [{ id: "user", role: "user", content: "Stop after a partial reply" }]);
  const reply = load_workspace().conversations[0].messages[1];
  assert.equal(reply.content, "Partial response");
  assert.equal(reply.status, "stopped");
  assert.equal(fixture.environment.generating, false);
});

function saved_conversation() {
  const chat = create_conversation();
  chat.messages = [
    { id: "u1", role: "user", content: "First question", attachments: [] },
    { id: "a1", role: "assistant", content: "Original answer", status: "complete", feedback: "up" },
    { id: "u2", role: "user", content: "Important follow-up", attachments: [
      { id: "notes", name: "notes.txt", size: 5, type: "text/plain", kind: "text", data: "notes" },
    ] },
    { id: "a2", role: "assistant", content: "```html\n<main>Original artifact</main>\n```", status: "complete" },
  ];
  chat.artifact = extract_artifact(chat.messages[3].content);
  chat.artifact_open = true;
  return chat;
}

async function saved_fixture(context, stream_response = provider_stream) {
  const fixture = app_fixture(context, stream_response, saved_conversation());
  await fixture.environment.autosave.persist(fixture.actions.workspace_snapshot());
  fixture.original = load_workspace().conversations[0];
  return fixture;
}

function assert_original(fixture) {
  assert.deepEqual(load_workspace().conversations[0], fixture.original);
  assert.deepEqual(fixture.environment.active.messages, fixture.chat.messages);
  assert.deepEqual(fixture.environment.active.artifact, fixture.chat.artifact);
  assert.equal(fixture.environment.active.artifact_open, true);
  assert.doesNotMatch(fixture.storage.raw(), /fixture_in_memory_only|api_key/);
}

function assert_settled(fixture) {
  assert.equal(fixture.environment.controller, null);
  assert.equal(fixture.environment.generating, false);
  assert.equal(fixture.environment.replacement, null);
  assert.deepEqual(fixture.errors, []);
}

function event(type, values = {}) {
  return `event: ${type}\ndata: ${JSON.stringify({ type, ...values })}\n\n`;
}
function stream_start(text) {
  return event("message_start", { message: { role: "assistant", content: [], usage: { input_tokens: 4, output_tokens: 0 } } }) +
    event("content_block_start", { index: 0, content_block: { type: "text", text: "" } }) +
    event("content_block_delta", { index: 0, delta: { type: "text_delta", text } });
}
function stream_end(stop_reason = "end_turn") {
  return event("content_block_stop", { index: 0 }) +
    event("message_delta", { delta: { stop_reason }, usage: { output_tokens: 3 } }) + event("message_stop");
}
function stream_response_fixture(text, end = stream_end()) {
  return new Response(stream_start(text) + end, { headers: { "content-type": "text/event-stream" } });
}

for (const failure of ["missing key", "rejected key", "network failure", "truncated stream", "token limit", "empty response"]) {
  test(`regeneration preserves saved transcript and artifact after ${failure}`, async (context) => {
    const fixture = await saved_fixture(context);
    const fetch = context.mock.method(globalThis, "fetch", async () => {
      assert_original(fixture);
      if (failure === "rejected key") return new Response("", { status: 401 });
      if (failure === "network failure") throw new TypeError("Synthetic connection failure");
      if (failure === "truncated stream") return stream_response_fixture("partial", "");
      if (failure === "token limit") return stream_response_fixture("partial", stream_end("max_tokens"));
      if (failure === "empty response") return stream_response_fixture(" ");
      throw new Error("Missing-key validation must not fetch");
    });
    if (failure === "missing key") fixture.environment.api_key = "";
    fixture.actions.regenerate(3);
    await fixture.actions.wait();
    assert_original(fixture);
    assert_settled(fixture);
    assert.equal(fetch.mock.callCount(), failure === "missing key" ? 0 : 1);
    assert.equal(fixture.environment.replacement_notice.error, true);
    assert.match(fixture.environment.replacement_notice.message, /Your original conversation was kept/);
    const expected = {
      "missing key": /Add an Anthropic API key/,
      "rejected key": /API key was rejected/,
      "network failure": /connect|network/i,
      "truncated stream": /ended before|incomplete/i,
      "token limit": /token limit/i,
      "empty response": /empty replacement/i,
    };
    assert.match(fixture.environment.replacement_notice.message, expected[failure]);
  });
}

test("partial replacement stays out of lifecycle saves and artifact actions; stop preserves the original", async (context) => {
  const fixture = await saved_fixture(context);
  let cancelled = false;
  const partial = "```html\n<main>Unfinished replacement</main>\n```";
  context.mock.method(globalThis, "fetch", async () => new Response(new ReadableStream({
    start(controller) {
      controller.enqueue(new TextEncoder().encode(stream_start(partial)));
    },
    cancel() { cancelled = true; },
  }), { headers: { "content-type": "text/event-stream" } }));
  const page = new EventTarget();
  const document = new EventTarget();
  document.visibilityState = "visible";
  fixture.environment.autosave.attach(page, document);
  fixture.actions.regenerate(3);
  // Let the real fetch/body-reader promise chain deliver the controlled delta.
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(fixture.environment.replacement.messages.at(-1).content, partial);
  assert.equal(fixture.environment.replacement.messages.at(-1).status, "streaming");
  fixture.actions.open_artifact(extract_artifact(partial));
  assert.match(fixture.environment.notifications.at(-1), /Wait for the replacement/);
  document.visibilityState = "hidden";
  document.dispatchEvent(new Event("visibilitychange"));
  assert_original(fixture);
  page.dispatchEvent(new Event("pagehide"));
  assert_original(fixture);
  assert.equal(fixture.session.writable, false);
  page.dispatchEvent(new Event("pageshow"));
  await settle();
  fixture.actions.stop();
  await fixture.actions.wait();
  assert.equal(cancelled, true);
  assert_original(fixture);
  assert_settled(fixture);
  assert.equal(fixture.environment.replacement_notice.error, false);
  assert.match(fixture.environment.replacement_notice.message, /Response stopped/);
});

test("a cancellation just before provider settlement cannot commit a replacement", async (context) => {
  const fixture = await saved_fixture(context, async () => {
    fixture.actions.stop();
    return "Late complete result";
  });
  fixture.actions.regenerate(3);
  await fixture.actions.wait();
  assert_original(fixture);
  assert_settled(fixture);
  assert.match(fixture.environment.replacement_notice.message, /Response stopped/);
});

test("successful latest regeneration commits completed text, usage and its artifact together", async (context) => {
  const fixture = await saved_fixture(context);
  const replacement = "```html\n<main>Finished replacement</main>\n```";
  context.mock.method(globalThis, "fetch", async () => {
    assert_original(fixture);
    // A feedback action while previewing must survive the successful replacement.
    fixture.environment.conversations = fixture.environment.conversations.map((chat) => ({
      ...chat, messages: chat.messages.map((message) => message.id === "a1" ? { ...message, feedback: "down" } : message),
    }));
    return stream_response_fixture(replacement);
  });
  fixture.actions.regenerate(3);
  await fixture.actions.wait();
  const saved = load_workspace().conversations[0];
  assert.deepEqual(saved.messages.slice(0, 3).map((message) => message.content), fixture.original.messages.slice(0, 3).map((message) => message.content));
  assert.equal(saved.messages[1].feedback, "down");
  assert.equal(saved.messages[3].content, replacement);
  assert.equal(saved.messages[3].status, "complete");
  assert.equal(saved.messages[3].usage.output_tokens, 3);
  assert.notEqual(saved.messages[3].id, "a2");
  assert.deepEqual(saved.artifact, extract_artifact(replacement));
  assert.equal(saved.artifact_open, true);
  assert.equal(fixture.environment.replacement_notice, null);
  assert.doesNotMatch(fixture.storage.raw(), /fixture_in_memory_only|api_key/);
  assert_settled(fixture);
});

test("older-turn regeneration requires confirmation and cancelling it leaves all later turns intact", async (context) => {
  const fetch = context.mock.method(globalThis, "fetch", () => { throw new Error("Unconfirmed inference"); });
  const fixture = await saved_fixture(context);
  fixture.actions.regenerate(1);
  assert.equal(fixture.actions.wait(), undefined);
  assert.equal(fixture.environment.controller, null);
  assert.equal(fixture.environment.modal.type, "confirm");
  assert.match(fixture.environment.modal.description, /all later messages/);
  fixture.environment.modal = null; // Existing Cancel/Escape behavior dismisses without calling action.
  assert_original(fixture);
  assert.equal(fetch.mock.callCount(), 0);
});

for (const succeed of [false, true]) {
  test(`confirmed older-turn regeneration ${succeed ? "commits only the chosen prefix after completion" : "keeps every original turn on failure"}`, async (context) => {
    const fixture = await saved_fixture(context);
    context.mock.method(globalThis, "fetch", async (url, options) => {
      assert_original(fixture);
      assert.deepEqual(JSON.parse(options.body).messages, [{ role: "user", content: [{ type: "text", text: "First question" }] }]);
      return succeed ? stream_response_fixture("New first answer") : new Response("", { status: 401 });
    });
    fixture.actions.regenerate(1);
    fixture.environment.modal.action();
    await fixture.actions.wait();
    if (succeed) {
      const saved = load_workspace().conversations[0];
      assert.deepEqual(saved.messages.map((message) => message.content), ["First question", "New first answer"]);
      assert.equal(saved.artifact, null);
      assert.equal(saved.artifact_open, false);
    } else assert_original(fixture);
    assert_settled(fixture);
  });
}

for (const succeed of [false, true]) {
  test(`confirmed edit/resend ${succeed ? "commits the edit after completion" : "keeps original user text, attachments and replies on failure"}`, async (context) => {
    const fixture = await saved_fixture(context);
    context.mock.method(globalThis, "fetch", async (url, options) => {
      assert_original(fixture);
      assert.equal(JSON.parse(options.body).messages.at(-1).content.at(-1).text, "Edited follow-up");
      return succeed ? stream_response_fixture("Edited answer") : new Response("", { status: 401 });
    });
    let editor_closed = false;
    fixture.actions.edit_message(2, "Edited follow-up", () => { editor_closed = true; });
    assert.equal(editor_closed, false);
    fixture.environment.modal.action();
    await fixture.actions.wait();
    assert.equal(editor_closed, true);
    if (succeed) {
      const saved = load_workspace().conversations[0];
      assert.equal(saved.messages[2].content, "Edited follow-up");
      assert.deepEqual(saved.messages[2].attachments, fixture.original.messages[2].attachments);
      assert.equal(saved.messages[3].content, "Edited answer");
    } else assert_original(fixture);
    assert_settled(fixture);
  });
}

test("offline regeneration preserves original history without beginning a request", async (context) => {
  const fetch = context.mock.method(globalThis, "fetch", () => { throw new Error("Offline inference"); });
  const fixture = await saved_fixture(context);
  fixture.environment.online = false;
  fixture.actions.regenerate(3);
  await fixture.actions.wait();
  assert_original(fixture);
  assert_settled(fixture);
  assert.equal(fetch.mock.callCount(), 0);
  assert.match(fixture.environment.notifications.at(-1), /offline.*original conversation is unchanged/i);
});


test("ordinary new-send streaming preserves feedback from the visible reply", async (context) => {
  const fixture = app_fixture(context, async ({ on_delta, on_usage }) => {
    on_delta("First ");
    fixture.environment.conversations = fixture.environment.conversations.map((chat) => ({
      ...chat, messages: chat.messages.map((message) => message.role === "assistant" ? { ...message, feedback: "up" } : message),
    }));
    on_usage({ input_tokens: 1, output_tokens: 1 });
    on_delta("second");
    assert.equal(fixture.environment.active.messages.at(-1).feedback, "up");
    return "First second";
  });
  await fixture.actions.generate(fixture.chat, [{ id: "user", role: "user", content: "New prompt" }]);
  assert.equal(load_workspace().conversations[0].messages.at(-1).feedback, "up");
});


test("a preview artifact repeated from user text cannot replace the saved artifact on failure", async (context) => {
  const artifact_source = `<main>\n${"<p>Quoted user HTML</p>\n".repeat(14)}</main>`;
  const preview = `\`\`\`html\n${artifact_source}\n\`\`\``;
  const chat = saved_conversation();
  chat.messages[2].content = `Please improve this HTML: ${artifact_source}`;
  const fixture = app_fixture(context, async ({ on_delta }) => {
    on_delta(preview);
    fixture.actions.open_artifact(extract_artifact(preview));
    assert.deepEqual(fixture.environment.active.artifact, fixture.chat.artifact);
    throw new Error("Synthetic stream failure after preview");
  }, chat);
  await fixture.environment.autosave.persist(fixture.actions.workspace_snapshot());
  fixture.original = load_workspace().conversations[0];
  fixture.actions.regenerate(3);
  await fixture.actions.wait();
  assert_original(fixture);
  assert.match(fixture.environment.notifications.at(-1), /Wait for the replacement/);
});
