import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
const source = readFileSync(
  new URL("../src/tokens.css", import.meta.url),
  "utf8",
);
function palette(block) {
  return Object.fromEntries(
    [...block.matchAll(/--([\w-]+)\s*:\s*(#[\da-f]{6})\s*;/gi)].map((match) => [
      match[1],
      match[2],
    ]),
  );
}
function luminance(hex) {
  const channels = hex
    .slice(1)
    .match(/../g)
    .map((value) => parseInt(value, 16) / 255)
    .map((value) =>
      value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4,
    );
  return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722;
}
function contrast(first, second) {
  const a = luminance(first),
    b = luminance(second);
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}
test("semantic text/background pairs meet WCAG AA in both themes", () => {
  assert.equal(contrast("#000000", "#ffffff"), 21);
  const light = palette(source.match(/:root\s*\{([\s\S]*?)\}/)[1]);
  const dark = {
    ...light,
    ...palette(
      source.match(/:root\[data-theme=['"]dark['"]\]\s*\{([\s\S]*?)\}/)[1],
    ),
  };
  const codex_light={...light,...palette(source.match(/:root\[data-interface=['"]codex['"]\]\s*\{([\s\S]*?)\}/)[1])};
  const codex_dark={...dark,...codex_light,...palette(source.match(/:root\[data-interface=['"]codex['"]\]\[data-theme=['"]dark['"]\]\s*\{([\s\S]*?)\}/)[1])};
  const pairs = [
    ["text", "bg"],
    ["text", "surface"],
    ["text-secondary", "bg"],
    ["text-secondary", "sidebar"],
    ["text-secondary", "hover"],
    ["text-muted", "bg"],
    ["text-muted", "sidebar"],
    ["accent", "accent-soft"],
    ["on-accent", "accent"],
    ["danger", "danger-soft"],
    ["success", "success-soft"],
    ["code-keyword", "code-bg"],
    ["code-string", "code-bg"],
    ["code-number", "code-bg"],
    ["code-comment", "code-bg"],
  ];
  const failures = [];
  for (const [theme, colors] of [
    ["light", light],
    ["dark", dark],
    ["codex light", codex_light],
    ["codex dark", codex_dark],
  ])
    for (const [foreground, background] of pairs) {
      const ratio = contrast(colors[foreground], colors[background]);
      if (ratio < 4.5)
        failures.push(
          `${theme}: ${foreground}/${background} = ${ratio.toFixed(2)}`,
        );
    }
  assert.deepEqual(failures, []);
});
