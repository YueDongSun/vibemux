import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { browser_search_paths, resolve_browser_executable } from "./browser_discovery.mjs";

const home_dir = "/Users/verifier fixture";
const chrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const edge = "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge";

test("Windows and Linux keep their existing native browser preference", () => {
  assert.deepEqual(browser_search_paths({ platform: "win32" }), [
    "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
    "C:/Program Files/Google/Chrome/Application/chrome.exe",
  ]);
  assert.deepEqual(browser_search_paths({ platform: "linux" }), ["/usr/bin/google-chrome", "/usr/bin/chromium"]);
});

test("macOS searches executable bundle members in system and user Applications", () => {
  assert.deepEqual(browser_search_paths({ platform: "darwin", home_dir }), [
    chrome,
    edge,
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    `${home_dir}/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`,
    `${home_dir}/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge`,
    `${home_dir}/Applications/Chromium.app/Contents/MacOS/Chromium`,
  ]);
});

test("a relative home directory never makes browser discovery depend on cwd", () => {
  const candidates = browser_search_paths({ platform: "darwin", home_dir: "relative-home" });
  assert.equal(candidates.length, 3);
  assert.ok(candidates.every((candidate) => candidate.startsWith("/Applications/")));
});

test("an explicit override wins and a missing override never selects another browser", async () => {
  const requested = "/operator/browser with spaces";
  for (const present of [true, false]) {
    const probed = [];
    const result = await resolve_browser_executable(requested, {
      platform: "darwin", home_dir,
      file_exists: async (candidate) => { probed.push(candidate); return candidate === requested ? present : true; },
    });
    assert.deepEqual(probed, [requested]);
    assert.deepEqual(result, present ? { executable: requested } : { missing: `browser executable not found: ${requested}` });
  }
});

test("macOS discovery selects the first present native executable without probing Windows paths", async () => {
  const probed = [];
  const result = await resolve_browser_executable(undefined, {
    platform: "darwin", home_dir,
    file_exists: async (candidate) => { probed.push(candidate); return candidate === edge; },
  });
  assert.deepEqual(result, { executable: edge });
  assert.deepEqual(probed, [chrome, edge]);
});

test("per-user macOS installs and the existing Linux fallback remain discoverable", async () => {
  for (const [platform, available] of [
    ["darwin", `${home_dir}/Applications/Chromium.app/Contents/MacOS/Chromium`],
    ["linux", "/usr/bin/chromium"],
  ]) {
    assert.deepEqual(await resolve_browser_executable(undefined, {
      platform, home_dir, file_exists: async (candidate) => candidate === available,
    }), { executable: available });
  }
});

test("missing browser diagnostics name only the current platform's searched paths", async () => {
  const result = await resolve_browser_executable(undefined, {
    platform: "darwin", home_dir, file_exists: async () => false,
  });
  assert.equal(result.missing, `no browser executable found; searched ${browser_search_paths({ platform: "darwin", home_dir }).join(", ")}`);
  assert.ok(!result.missing.includes("C:/"));
  assert.equal((await resolve_browser_executable(undefined, { platform: "unknown" })).missing,
    "no browser search locations for this platform; supply --browser with an absolute executable path");
});

test("the real file probe rejects directories and accepts an explicitly selected file", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "taskboard_browser_discovery_"));
  try {
    const executable = path.join(root, "test browser");
    await writeFile(executable, "fixture only");
    assert.deepEqual(await resolve_browser_executable(executable), { executable });
    assert.deepEqual(await resolve_browser_executable(root), { missing: `browser executable not found: ${root}` });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
