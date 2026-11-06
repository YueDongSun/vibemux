import assert from "node:assert/strict";
import test from "node:test";
import { Page_driver } from "./browser_suite.mjs";

function keyboard_fixture() {
  const calls = [];
  const session = {
    on_event: () => () => {},
    async send(method, params) { calls.push({ method, params }); },
  };
  return {
    calls,
    page: new Page_driver({ session, origin: "http://127.0.0.1:8420", signal: new AbortController().signal }),
  };
}

test("select-all and Backspace remain real paired key events without OS-native key numbers", async () => {
  const { calls, page } = keyboard_fixture();
  await page.press_key({ key: "a", code: "KeyA", key_code: 65, modifiers: 2, commands: ["selectAll"] });
  await page.press_key({ key: "Backspace", code: "Backspace", key_code: 8 });
  assert.deepEqual(calls.map(({ method }) => method), Array(4).fill("Input.dispatchKeyEvent"));
  assert.deepEqual(calls.map(({ params: event }) => [event.type, event.key, event.code, event.windowsVirtualKeyCode, event.modifiers]), [
    ["rawKeyDown", "a", "KeyA", 65, 2],
    ["keyUp", "a", "KeyA", 65, 2],
    ["rawKeyDown", "Backspace", "Backspace", 8, 0],
    ["keyUp", "Backspace", "Backspace", 8, 0],
  ]);
  assert.deepEqual(calls[0].params.commands, ["selectAll"]);
  assert.ok(!Object.hasOwn(calls[1].params, "commands"));
  for (const { params: event } of calls) {
    assert.ok(!Object.hasOwn(event, "nativeVirtualKeyCode"), "Windows virtual keys must not be sent as macOS/Linux native keys");
    assert.ok(!Object.hasOwn(event, "text"), "editing keys must not insert characters");
  }
});

test("Enter retains the text and DOM identity needed for real form submission", async () => {
  const { calls, page } = keyboard_fixture();
  await page.press_enter();
  assert.equal(calls.length, 2);
  assert.equal(calls[0].method, "Input.dispatchKeyEvent");
  assert.equal(calls[1].method, "Input.dispatchKeyEvent");
  assert.deepEqual(calls[0].params, {
    type: "keyDown", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13,
    modifiers: 0, text: "\r", unmodifiedText: "\r",
  });
  assert.deepEqual(calls[1].params, {
    type: "keyUp", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13, modifiers: 0,
  });
});

test("failed key dispatch propagates instead of fabricating successful input", async () => {
  const { page } = keyboard_fixture();
  const failure = new Error("fixture renderer unavailable");
  page.session.send = async () => { throw failure; };
  await assert.rejects(page.press_enter(), (error) => error === failure);
});
