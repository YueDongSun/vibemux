import test from "node:test";
import assert from "node:assert/strict";
import { create_workspace_session, create_conversation, load_workspace, export_all } from "../src/services/chat_store.js";
import { persistence_fixture, settle } from "./support/persistence_fixture.mjs";

function add_chat(workspace, title) {
  const chat = create_conversation();
  chat.title = title;
  workspace.conversations.push(chat);
  workspace.active_id = chat.id;
  return chat;
}

test("simultaneous tabs have one writer; stale retry cannot erase the winner's chat", async (context) => {
  const fixture = persistence_fixture(context);
  const first = fixture.session();
  const second = fixture.session();
  add_chat(first.initial, "First tab's work");
  add_chat(second.initial, "Second tab's unsaved work");
  const results = await Promise.allSettled([first.acquire(), second.acquire()]);
  assert.equal(results[0].status, "fulfilled");
  assert.equal(results[1].reason.code, "storage_busy");
  first.save(first.initial);
  const saved = fixture.raw();
  assert.throws(() => second.save(second.initial), { code: "storage_busy" });
  first.release();
  await settle();
  await assert.rejects(second.acquire(), { code: "storage_conflict" });
  assert.throws(() => second.save(second.initial), { code: "storage_conflict" });
  assert.equal(fixture.raw(), saved);
  assert.equal(JSON.parse(export_all(second.initial.conversations))[0].title, "Second tab's unsaved work");
});

test("retry acquires a released writer when saved history has not changed", async (context) => {
  const fixture = persistence_fixture(context);
  const first = fixture.session();
  const second = fixture.session();
  await first.acquire();
  await assert.rejects(second.acquire(), { code: "storage_busy" });
  first.release();
  await settle();
  await second.acquire();
  add_chat(second.initial, "Safe retry");
  second.save(second.initial);
  second.initial.settings.theme = "dark";
  second.save(second.initial);
  assert.equal(load_workspace().conversations[0].title, "Safe retry");
  assert.equal(load_workspace().settings.theme, "dark");
});

test("external record changes fence both save and explicit reset", async (context) => {
  const fixture = persistence_fixture(context);
  const session = fixture.session();
  await session.acquire();
  fixture.replace('{a different corrupt but valuable record');
  assert.throws(() => session.save(session.initial), { code: "storage_conflict" });
  assert.throws(() => session.reset(), { code: "storage_conflict" });
  assert.equal(fixture.raw(), '{a different corrupt but valuable record');
});

test("corrupt baseline is retained until the owning tab explicitly resets it", async (context) => {
  const fixture = persistence_fixture(context, '{valuable broken record');
  const session = fixture.session();
  assert.match(session.initial.storage_warning, /preserved/);
  assert.throws(() => session.reset(), { code: "storage_not_owned" });
  await session.acquire();
  assert.throws(() => session.save(session.initial), /preserved.*reset/i);
  assert.equal(fixture.raw(), '{valuable broken record');
  session.save(session.reset());
  assert.deepEqual(load_workspace().conversations, []);
});

test("legacy records acquire without rewriting and saves still exclude secrets", async (context) => {
  const seed = persistence_fixture(context);
  const old = seed.session().initial;
  const chat = add_chat(old, "Legacy");
  chat.messages = [{ role: "assistant", content: "Partial", status: "streaming" }];
  delete old.settings.interface_style;
  seed.replace(JSON.stringify(old));
  const raw = seed.raw();
  const session = seed.session();
  await session.acquire();
  assert.equal(seed.raw(), raw);
  assert.equal(session.initial.conversations[0].messages[0].status, "stopped");
  session.initial.api_key = "fixture_secret";
  session.initial.settings.api_key = "fixture_secret";
  session.save(session.initial);
  assert.doesNotMatch(seed.raw(), /fixture_secret|api_key/);
  assert.equal(JSON.parse(seed.raw()).settings.interface_style, "claude");
});

test("quota failure leaves the saved token intact and can be retried", async (context) => {
  const fixture = persistence_fixture(context);
  const session = fixture.session();
  await session.acquire();
  session.save(session.initial);
  const previous = fixture.raw();
  const set = fixture.storage.setItem;
  fixture.storage.setItem = () => { throw new DOMException("Storage full", "QuotaExceededError"); };
  add_chat(session.initial, "Unsaved");
  assert.throws(() => session.save(session.initial), { name: "QuotaExceededError" });
  assert.equal(fixture.raw(), previous);
  fixture.storage.setItem = set;
  session.save(session.initial);
  assert.equal(load_workspace().conversations[0].title, "Unsaved");
});

test("unavailable locks and unreadable storage fail without a fallback write", async (context) => {
  const fixture = persistence_fixture(context);
  const unsupported = create_workspace_session({ locks: null });
  await assert.rejects(unsupported.acquire(), { code: "storage_lock_unavailable" });
  assert.throws(() => unsupported.save(unsupported.initial), { code: "storage_lock_unavailable" });
  fixture.storage.getItem = () => { throw new DOMException("Blocked", "SecurityError"); };
  const blocked = fixture.session();
  assert.match(blocked.initial.storage_warning, /could not be read/);
  await assert.rejects(blocked.acquire(), /could not be read/);
  assert.throws(() => blocked.save(blocked.initial), /could not be read/);
  assert.equal(fixture.writes.length, 0);
});

test("page release before an asynchronous grant cannot leave a writer behind", async (context) => {
  const fixture = persistence_fixture(context);
  const session = fixture.session();
  const pending = session.acquire();
  session.release();
  await assert.rejects(pending, { code: "storage_released" });
  await settle();
  assert.equal(session.writable, false);
  const next = fixture.session();
  await next.acquire();
  next.save(next.initial);
  assert.equal(fixture.writes.length, 1);
});
