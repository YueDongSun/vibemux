import assert from "node:assert/strict";
import test from "node:test";
import { summarize_context } from "../src/codex_context_policy.js";

test("empty conversations have zero counts and no invented metadata", () => {
  const expected = {
    message_count: 0,
    input_tokens: 0,
    output_tokens: 0,
    attachments: [],
    artifact: null,
    model: null,
    thinking: false,
    created_at: null,
  };

  assert.deepEqual(summarize_context(), expected);
  assert.deepEqual(summarize_context(null), expected);
  assert.deepEqual(summarize_context({}), expected);
  assert.deepEqual(summarize_context({ messages: [] }), expected);
});

test("message counts and usage totals come from recorded conversation data", () => {
  const conversation = {
    id: "conversation_1",
    model: "claude-opus-4-6",
    thinking: true,
    created_at: "2026-10-25T09:00:00.000Z",
    messages: [
      { role: "user", content: "Review the implementation." },
      {
        role: "assistant",
        content: "Review complete.",
        usage: { input_tokens: 1200, output_tokens: 345 },
      },
      {
        role: "assistant",
        content: "Additional context.",
        usage: { input_tokens: 800, output_tokens: 55 },
      },
    ],
  };

  const summary = summarize_context(conversation);

  assert.equal(summary.message_count, 3);
  assert.equal(summary.input_tokens, 2000);
  assert.equal(summary.output_tokens, 400);
  assert.equal(summary.model, "claude-opus-4-6");
  assert.equal(summary.thinking, true);
  assert.equal(summary.created_at, "2026-10-25T09:00:00.000Z");
});

test("attachments are deduplicated across messages and retain their file metadata", () => {
  const source_file = {
    id: "file_1",
    name: "architecture.md",
    size: 2048,
    type: "text/markdown",
  };
  const conversation = {
    attachments: [source_file, { id: "file_1", name: "duplicate.md" }],
    messages: [
      {
        role: "user",
        attachments: [
          source_file,
          { id: "file_2", name: "diagram.png", size: 4096, type: "image/png" },
        ],
      },
      {
        role: "assistant",
        attachments: [{ id: "file_2", name: "diagram.png" }],
      },
    ],
  };

  const summary = summarize_context(conversation);

  assert.deepEqual(summary.attachments, [
    source_file,
    { id: "file_2", name: "diagram.png", size: 4096, type: "image/png" },
  ]);
});

test("artifact is preserved and unrelated conversation content is excluded", () => {
  const artifact = {
    id: "artifact_1",
    name: "review.md",
    content: "Review output",
  };
  const conversation = {
    title: "Private conversation title",
    api_key: "must-not-be-copied",
    artifact,
    messages: [{ role: "user", content: "Private prompt" }],
  };

  const summary = summarize_context(conversation);

  assert.equal(summary.artifact, artifact);
  assert.equal(Object.hasOwn(summary, "api_key"), false);
  assert.equal(Object.hasOwn(summary, "title"), false);
  assert.deepEqual(Object.keys(summary), [
    "message_count",
    "input_tokens",
    "output_tokens",
    "attachments",
    "artifact",
    "model",
    "thinking",
    "created_at",
  ]);
});
