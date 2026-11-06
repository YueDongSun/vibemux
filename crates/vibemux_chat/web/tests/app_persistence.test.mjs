import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { parse } from "svelte/compiler";
import { create_workspace_autosave } from "../src/services/workspace_autosave.js";
import { create_conversation, uid, load_workspace } from "../src/services/chat_store.js";
import { extract_artifact } from "../src/ui_policy.js";
import { persistence_fixture } from "./support/persistence_fixture.mjs";

// Exercise the app's actual turn functions, located through Svelte's parser.
// The setter below provides the same autosave notification as the app effect;
// this is orchestration coverage, not a DOM/browser rendering assertion.
const source = readFileSync(new URL("../src/app.svelte", import.meta.url), "utf8");
const ast = parse(source);
const functions = ["workspace_snapshot", "update_chat", "generate"].map((name) => {
  const node = ast.instance.content.body.find((item) => item.type === "FunctionDeclaration" && item.id.name === name);
  assert.ok(node, `Missing app function ${name}`);
  return source.slice(node.start, node.end);
}).join("\n");

function app_fixture(context, stream_response) {
  const storage = persistence_fixture(context);
  const session = storage.session();
  const chat = create_conversation();
  const environment = {
    settings: session.initial.settings,
    active_id: chat.id,
    controller: null,
    generating: false,
    storage_ready: true,
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
  const actions = new Function("environment", `with (environment) { ${functions}; return { generate, workspace_snapshot }; }`)(environment);
  const errors = [];
  environment.autosave = create_workspace_autosave({
    session,
    snapshot: actions.workspace_snapshot,
    on_error: (error) => errors.push(error),
    on_saved: () => {},
  });
  context.after(() => environment.autosave.dispose());
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
