import test from "node:test";
import assert from "node:assert/strict";
import { resolve_interface, interface_url } from "../src/interface_policy.js";

test("explicit interface URLs override the saved presentation with a safe fallback", () => {
  assert.equal(resolve_interface("?view=codex", "claude"), "codex");
  assert.equal(resolve_interface("?view=claude", "codex"), "claude");
  assert.equal(resolve_interface("?view=unknown", "codex"), "codex");
  assert.equal(resolve_interface("", "codex"), "codex");
  assert.equal(resolve_interface("", "unknown"), "claude");
});
test("switching presentation preserves unrelated query parameters and the anchor", () => {
  assert.equal(
    interface_url("http://127.0.0.1:5182/?filter=one#latest", "codex"),
    "/?filter=one&view=codex#latest",
  );
  assert.throws(
    () => interface_url("http://127.0.0.1:5182/", "unknown"),
    /interface/i,
  );
});
