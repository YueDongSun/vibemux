import assert from "node:assert/strict";
import test from "node:test";
import { preflight_browser_page } from "./browser_preflight.mjs";

function fixture_connection(failing_method = null, evaluation = { result: { value: 2 } }) {
  const calls = [];
  return {
    calls,
    async send(method, params, options) {
      calls.push({ method, params, options });
      if (method === failing_method) throw new Error("infrastructure unavailable");
      if (method === "Target.createTarget") return { targetId: "blank_target" };
      if (method === "Target.attachToTarget") return { sessionId: "blank_session" };
      if (method === "Runtime.evaluate") return evaluation;
      return {};
    },
  };
}

test("a usable blank renderer is checked and its target is closed", async () => {
  const connection = fixture_connection();
  assert.equal(await preflight_browser_page(connection, { timeout_ms: 1000 }), null);
  assert.deepEqual(connection.calls[0].params, { url: "about:blank" });
  assert.equal(connection.calls.at(-1).method, "Target.closeTarget");
  for (const call of connection.calls.slice(0, -1)) {
    assert.ok(call.options.timeout_ms > 0 && call.options.timeout_ms <= 1000);
  }
  assert.equal(connection.calls.find((call) => call.method === "Runtime.evaluate").options.session_id, "blank_session");
});

test("renderer domain failures are blocked before candidate checks and close the target", async () => {
  for (const domain of ["Page", "Runtime", "DOM", "Network", "Accessibility"]) {
    const connection = fixture_connection(`${domain}.enable`);
    assert.equal(await preflight_browser_page(connection), `browser_page_unavailable:${domain.toLowerCase()}_enable`);
    assert.equal(connection.calls.at(-1).method, "Target.closeTarget");
    assert.ok(!connection.calls.some((call) => call.method === "Runtime.evaluate"));
  }
});

test("attachment and evaluation uncertainty never admit browser checks", async () => {
  const connection = fixture_connection("Target.attachToTarget");
  assert.equal(await preflight_browser_page(connection), "browser_page_unavailable:target_attach");
  assert.equal(connection.calls.at(-1).method, "Target.closeTarget");
  for (const evaluated of [{ result: { value: 3 } }, { exceptionDetails: {}, result: { value: 2 } }, {}]) {
    assert.equal(await preflight_browser_page(fixture_connection(null, evaluated)), "browser_page_unavailable:runtime_evaluate");
  }
});

test("emulation prerequisites cannot be mistaken for candidate assertions", async () => {
  for (const [method, phase] of [
    ["Emulation.setDeviceMetricsOverride", "emulation_metrics"],
    ["Emulation.setFocusEmulationEnabled", "emulation_focus"],
  ]) {
    const connection = fixture_connection(method);
    assert.equal(await preflight_browser_page(connection), `browser_page_unavailable:${phase}`);
    assert.equal(connection.calls.at(-1).method, "Target.closeTarget");
  }
});

test("an exhausted prerequisite deadline launches no target", async () => {
  const connection = fixture_connection();
  assert.equal(await preflight_browser_page(connection, { timeout_ms: 0 }), "browser_page_unavailable:target_create");
  assert.deepEqual(connection.calls, []);
});
