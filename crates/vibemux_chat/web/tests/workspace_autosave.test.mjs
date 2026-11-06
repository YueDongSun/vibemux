import test from "node:test";
import assert from "node:assert/strict";
import { create_workspace_autosave } from "../src/services/workspace_autosave.js";
import { load_workspace, create_conversation } from "../src/services/chat_store.js";
import { persistence_fixture, settle } from "./support/persistence_fixture.mjs";

async function autosave_fixture(context) {
  const storage = persistence_fixture(context);
  const session = storage.session();
  await session.acquire();
  let workspace = session.initial;
  const chat = create_conversation();
  chat.messages = [
    { id: "user", role: "user", content: "Keep my request" },
    { id: "assistant", role: "assistant", content: "", status: "streaming" },
  ];
  workspace.conversations = [chat];
  workspace.active_id = chat.id;
  const errors = [];
  let saved_count = 0;
  const autosave = create_workspace_autosave({
    session,
    snapshot: () => structuredClone(workspace),
    on_error: (error) => errors.push(error),
    on_saved: () => { saved_count++; },
  });
  const page = new EventTarget();
  const document = new EventTarget();
  document.visibilityState = "visible";
  autosave.attach(page, document);
  context.after(() => autosave.dispose());
  await settle();
  return { storage, session, workspace, chat, errors, autosave, page, document, saved_count: () => saved_count };
}

test("immediate persistence saves a submitted prompt before the first streaming token", async (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const fixture = await autosave_fixture(context);
  assert.equal(await fixture.autosave.persist(structuredClone(fixture.workspace)), true);
  assert.equal(load_workspace().conversations[0].messages[0].content, "Keep my request");
  assert.equal(fixture.saved_count(), 1);
});

test("continuous 100 ms deltas checkpoint every second rather than resetting the deadline", async (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const fixture = await autosave_fixture(context);
  for (let i = 0; i < 25; i++) {
    fixture.chat.messages[1].content += "x";
    fixture.autosave.schedule(structuredClone(fixture.workspace));
    context.mock.timers.tick(100);
  }
  assert.equal(fixture.storage.writes.length, 2);
  assert.equal(load_workspace().conversations[0].messages[1].content, "x".repeat(20));
  fixture.page.dispatchEvent(new Event("pagehide"));
  assert.equal(load_workspace().conversations[0].messages[1].content, "x".repeat(25));
  assert.equal(load_workspace().conversations[0].messages[1].status, "stopped");
  assert.equal(fixture.session.writable, false);
});

test("visibility and pagehide capture the newest state before the reactive debounce runs", async (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const fixture = await autosave_fixture(context);
  fixture.chat.messages[1].content = "Not scheduled yet";
  fixture.document.visibilityState = "hidden";
  fixture.document.dispatchEvent(new Event("visibilitychange"));
  assert.equal(load_workspace().conversations[0].messages[1].content, "Not scheduled yet");
  assert.equal(fixture.session.writable, true);
  fixture.chat.messages[1].content += " plus final chunk";
  fixture.page.dispatchEvent(new Event("pagehide"));
  assert.equal(load_workspace().conversations[0].messages[1].content, "Not scheduled yet plus final chunk");
  assert.equal(fixture.session.writable, false);
});

test("restored pages reacquire ownership, but cannot overwrite edits made while hidden", async (context) => {
  const fixture = await autosave_fixture(context);
  await fixture.autosave.persist(structuredClone(fixture.workspace));
  fixture.page.dispatchEvent(new Event("pagehide"));
  await settle();
  fixture.page.dispatchEvent(new Event("pageshow"));
  await settle();
  assert.equal(fixture.session.writable, true);
  fixture.page.dispatchEvent(new Event("pagehide"));
  await settle();
  const other = fixture.storage.session();
  await other.acquire();
  other.initial.settings.theme = "dark";
  other.save(other.initial);
  other.release();
  await settle();
  const saved = fixture.storage.raw();
  fixture.page.dispatchEvent(new Event("pageshow"));
  await settle();
  assert.equal(fixture.session.writable, false);
  assert.equal(fixture.errors.at(-1).code, "storage_conflict");
  assert.equal(fixture.storage.raw(), saved);
});

test("failed checkpoints remain unsaved, report the error, and support an explicit retry", async (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const fixture = await autosave_fixture(context);
  const set = fixture.storage.storage.setItem;
  fixture.storage.storage.setItem = () => { throw new DOMException("Storage full", "QuotaExceededError"); };
  fixture.autosave.schedule(structuredClone(fixture.workspace));
  context.mock.timers.tick(400);
  assert.equal(fixture.saved_count(), 0);
  assert.equal(fixture.errors.at(-1).name, "QuotaExceededError");
  assert.equal(fixture.storage.raw(), null);
  fixture.storage.storage.setItem = set;
  assert.equal(await fixture.autosave.persist(structuredClone(fixture.workspace)), true);
  assert.equal(fixture.saved_count(), 1);
});

test("dispose flushes once, releases ownership, and removes timers and lifecycle callbacks", async (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const fixture = await autosave_fixture(context);
  fixture.autosave.schedule(structuredClone(fixture.workspace));
  fixture.autosave.dispose();
  const count = fixture.storage.writes.length;
  context.mock.timers.tick(2000);
  fixture.page.dispatchEvent(new Event("pagehide"));
  fixture.page.dispatchEvent(new Event("pageshow"));
  fixture.document.visibilityState = "hidden";
  fixture.document.dispatchEvent(new Event("visibilitychange"));
  await settle();
  assert.equal(fixture.storage.writes.length, count);
  assert.equal(count, 1);
  assert.equal(fixture.session.writable, false);
});
