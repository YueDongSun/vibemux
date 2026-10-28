import test from "node:test";
import assert from "node:assert/strict";
import { models } from "../src/config.js";
import { stream_response, validate_api_key } from "../src/services/provider.js";

const fixture_key = "fixture_key_not_a_real_credential";
const encoder = new TextEncoder();

function request(overrides = {}) {
  return {
    messages: [{ role: "user", content: "Hello" }],
    model: models[0].id,
    thinking: false,
    api_key: fixture_key,
    ...overrides,
  };
}

function event(type, values = {}, newline = "\n") {
  return `event: ${type}${newline}data: ${JSON.stringify({ type, ...values })}${newline}${newline}`;
}

function complete_stream(text = "Hello 🌍", stop_reason = "end_turn") {
  return (
    event("message_start", {
      message: {
        role: "assistant",
        content: [],
        usage: {
          input_tokens: 23,
          output_tokens: 0,
          cache_read_input_tokens: 5,
        },
      },
    }) +
    event("content_block_start", {
      index: 0,
      content_block: { type: "text", text: "" },
    }) +
    event("content_block_delta", {
      index: 0,
      delta: { type: "text_delta", text },
    }) +
    event("content_block_stop", { index: 0 }) +
    event("message_delta", {
      delta: { stop_reason },
      usage: { output_tokens: 7 },
    }) +
    event("message_stop")
  );
}

function stream_fixture(chunks, close = true) {
  let controller;
  let cancelled = false;
  const body = new ReadableStream({
    start(stream_controller) {
      controller = stream_controller;
      for (const chunk of chunks) stream_controller.enqueue(chunk);
      if (close) stream_controller.close();
    },
    cancel() {
      cancelled = true;
    },
  });
  return {
    response: new Response(body, {
      headers: { "content-type": "text/event-stream; charset=utf-8" },
    }),
    enqueue: (text) => controller.enqueue(encoder.encode(text)),
    was_cancelled: () => cancelled,
    body,
  };
}

test.beforeEach((context) => {
  context.mock.method(globalThis, "fetch", () => {
    throw new Error("Uncontrolled network access is forbidden");
  });
});

test("byte-at-a-time UTF-8 SSE produces the full text and cumulative usage without exposing thinking", async (context) => {
  const thinking =
    event("content_block_start", {
      index: 1,
      content_block: { type: "thinking", thinking: "" },
    }) +
    event("content_block_delta", {
      index: 1,
      delta: { type: "thinking_delta", thinking: "fixture private thinking" },
    }) +
    event("content_block_stop", { index: 1 });
  const source = complete_stream("Café 中文 🌍\nsecond line").replace(
    "event: content_block_start",
    `${thinking}event: content_block_start`,
  );
  const bytes = encoder.encode(source);
  const fixture = stream_fixture(
    Array.from(bytes, (byte) => Uint8Array.of(byte)),
  );
  context.mock.method(globalThis, "fetch", async () => fixture.response);
  const deltas = [];
  const usage = [];
  const text = await stream_response(
    request({
      on_delta: (delta) => deltas.push(delta),
      on_usage: (value) => usage.push(value),
    }),
  );
  assert.equal(text, "Café 中文 🌍\nsecond line");
  assert.equal(deltas.join(""), text);
  assert.doesNotMatch(text, /private thinking/);
  assert.equal(usage[0].input_tokens, 23);
  assert.deepEqual(usage.at(-1), {
    input_tokens: 23,
    output_tokens: 7,
    cache_read_input_tokens: 5,
  });
  assert.equal(usage[0].output_tokens, 0);
  assert.equal(fixture.body.locked, false);
});

test("edited conversation context and image/PDF/text attachments map to real Anthropic content blocks", async (context) => {
  const messages = [
    { role: "user", content: "Original question" },
    { role: "assistant", content: "Earlier answer" },
    {
      role: "user",
      content: "Edited question",
      attachments: [
        {
          id: "image_1",
          name: "pixel.png",
          size: 3,
          type: "image/png",
          kind: "image",
          data: "YWJj",
        },
        {
          id: "pdf_1",
          name: "report.pdf",
          size: 3,
          type: "application/pdf",
          kind: "pdf",
          data: "YWJj",
        },
        {
          id: "text_1",
          name: "notes.md",
          size: 5,
          type: "text/markdown",
          kind: "text",
          data: "notes",
        },
      ],
    },
  ];
  const original = structuredClone(messages);
  context.mock.method(globalThis, "fetch", async (url, options) => {
    assert.equal(url, "https://api.anthropic.com/v1/messages");
    assert.equal(options.method, "POST");
    assert.equal(options.headers["x-api-key"], fixture_key);
    assert.equal(options.headers["anthropic-version"], "2023-06-01");
    assert.equal(
      options.headers["anthropic-dangerous-direct-browser-access"],
      "true",
    );
    assert.equal(options.redirect, "error");
    assert.equal(options.credentials, "omit");
    const payload = JSON.parse(options.body);
    assert.equal(payload.stream, true);
    assert.equal(payload.model, models[0].id);
    assert.deepEqual(payload.thinking, { type: "adaptive" });
    assert.ok(payload.max_tokens > 1024);
    assert.doesNotMatch(options.body, /fixture_key|api_key/);
    assert.deepEqual(payload.messages, [
      { role: "user", content: [{ type: "text", text: "Original question" }] },
      {
        role: "assistant",
        content: [{ type: "text", text: "Earlier answer" }],
      },
      {
        role: "user",
        content: [
          {
            type: "image",
            source: { type: "base64", media_type: "image/png", data: "YWJj" },
          },
          {
            type: "document",
            title: "report.pdf",
            source: {
              type: "base64",
              media_type: "application/pdf",
              data: "YWJj",
            },
          },
          {
            type: "document",
            title: "notes.md",
            source: { type: "text", media_type: "text/plain", data: "notes" },
          },
          { type: "text", text: "Edited question" },
        ],
      },
    ]);
    return stream_fixture([encoder.encode(complete_stream())]).response;
  });
  assert.equal(
    await stream_response(request({ messages, thinking: true })),
    "Hello 🌍",
  );
  assert.deepEqual(messages, original);
});

test("Haiku thinking selection falls back to a normal request without retrying inference", async (context) => {
  const calls = [];
  context.mock.method(globalThis, "fetch", async (url, options) => {
    calls.push(url);
    assert.equal(JSON.parse(options.body).thinking, undefined);
    return stream_fixture([encoder.encode(complete_stream())]).response;
  });
  await stream_response(request({ model: "claude-haiku-4-5", thinking: true }));
  assert.equal(calls.length, 1);
});

test("Models API validation uses GET and makes no inference request", async (context) => {
  const calls = [];
  context.mock.method(globalThis, "fetch", async (url, options) => {
    calls.push(url);
    assert.equal(url, "https://api.anthropic.com/v1/models");
    assert.equal(options.method, "GET");
    assert.equal(options.body, undefined);
    assert.equal(options.headers["x-api-key"], fixture_key);
    return Response.json({
      data: [
        {
          id: models[0].id,
          display_name: "Sonnet 4.6",
          created_at: "2026-02-17T00:00:00Z",
        },
      ],
      has_more: false,
    });
  });
  const result = await validate_api_key(fixture_key);
  assert.equal(result.valid, true);
  assert.match(result.message, /no inference/i);
  assert.equal(result.models[0].id, models[0].id);
  assert.equal(calls.length, 1);
});

test("CRLF splits, comments, multiline data, unknown events, and final unterminated lines are supported", async (context) => {
  const source =
    ": keepalive\r\n\r\n" +
    complete_stream("Across chunks")
      .replaceAll("\n", "\r\n")
      .replace(
        "event: content_block_start",
        `${event("future_event", { value: "ignored" }, "\r\n")}event: content_block_start`,
      )
      .replace(
        'data: {"type":"message_stop"}\r\n\r\n',
        'data: {"type":\r\ndata: "message_stop"}',
      );
  const fixture = stream_fixture(
    Array.from(encoder.encode(source), (byte) => Uint8Array.of(byte)),
  );
  context.mock.method(globalThis, "fetch", async () => fixture.response);
  assert.equal(await stream_response(request()), "Across chunks");
});

test("terminal message_stop completes and cancels the body without waiting for HTTP EOF", async (context) => {
  const fixture = stream_fixture(
    [encoder.encode(complete_stream("Done"))],
    false,
  );
  context.mock.method(globalThis, "fetch", async () => fixture.response);
  assert.equal(await stream_response(request()), "Done");
  assert.equal(fixture.was_cancelled(), true);
  assert.equal(fixture.body.locked, false);
});

test("aborting inside a delta prevents every subsequent delta, even in the same chunk", async (context) => {
  const controller = new AbortController();
  const source = complete_stream("first").replace(
    "event: content_block_stop",
    `${event("content_block_delta", { index: 0, delta: { type: "text_delta", text: "must not arrive" } })}event: content_block_stop`,
  );
  const fixture = stream_fixture([encoder.encode(source)], false);
  const deltas = [];
  context.mock.method(globalThis, "fetch", async () => fixture.response);
  await assert.rejects(
    stream_response(
      request({
        signal: controller.signal,
        on_delta: (delta) => {
          deltas.push(delta);
          controller.abort(new Error(fixture_key));
        },
      }),
    ),
    (error) =>
      error.name === "AbortError" && !String(error).includes(fixture_key),
  );
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(deltas, ["first"]);
  assert.equal(fixture.was_cancelled(), true);
  assert.equal(fixture.body.locked, false);
});

test("cancellation from a synchronous callback stops delivery and releases the reader", async (context) => {
  const controller = new AbortController();
  const fixture = stream_fixture([encoder.encode(complete_stream())], false);
  context.mock.method(globalThis, "fetch", async () => fixture.response);
  await assert.rejects(
    stream_response(
      request({
        signal: controller.signal,
        on_delta: () => controller.abort(),
      }),
    ),
    { name: "AbortError" },
  );
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(fixture.was_cancelled(), true);
});

test("abort interrupts a pending body read and releases the reader", async (context) => {
  const controller = new AbortController();
  const fixture = stream_fixture([], false);
  let fetch_signal;
  context.mock.method(globalThis, "fetch", async (url, options) => {
    fetch_signal = options.signal;
    return fixture.response;
  });
  const pending = stream_response(request({ signal: controller.signal }));
  const rejected = assert.rejects(pending, { name: "AbortError" });
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(fixture.body.locked, true);
  controller.abort();
  await rejected;
  assert.equal(fetch_signal.aborted, true);
  assert.equal(fixture.was_cancelled(), true);
  assert.equal(fixture.body.locked, false);
});

test("an already aborted signal starts no fetch and an ignored fetch abort still settles promptly", async (context) => {
  const cancelled = new AbortController();
  cancelled.abort();
  const calls = [];
  let resolve_fetch;
  context.mock.method(globalThis, "fetch", (url) => {
    calls.push(url);
    return new Promise((resolve) => {
      resolve_fetch = resolve;
    });
  });
  await assert.rejects(stream_response(request({ signal: cancelled.signal })), {
    name: "AbortError",
  });
  assert.equal(calls.length, 0);
  const controller = new AbortController();
  const pending = stream_response(request({ signal: controller.signal }));
  const rejected = assert.rejects(pending, { name: "AbortError" });
  controller.abort();
  await rejected;
  const late = stream_fixture([], false);
  resolve_fetch(late.response);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(late.was_cancelled(), true);
});

for (const [description, source, code] of [
  [
    "EOF after partial text",
    complete_stream("partial").split("event: content_block_stop")[0],
    "stream_truncated",
  ],
  [
    "EOF after stop reason but before message_stop",
    complete_stream().split("event: message_stop")[0],
    "stream_truncated",
  ],
  [
    "token-limited completion",
    complete_stream("partial", "max_tokens"),
    "response_truncated",
  ],
  ["refused completion", complete_stream("", "refusal"), "provider_refusal"],
  ["message_stop without start", event("message_stop"), "invalid_stream"],
  [
    "message_stop with open block",
    complete_stream().replace(event("content_block_stop", { index: 0 }), ""),
    "invalid_stream",
  ],
  [
    "malformed JSON",
    `event: error\ndata: {${fixture_key}\n\n`,
    "invalid_stream",
  ],
  [
    "provider error event",
    event("error", {
      error: { type: "authentication_error", message: fixture_key },
    }),
    "provider_stream_error",
  ],
  ["non-Anthropic DONE sentinel", "data: [DONE]\n\n", "invalid_stream"],
]) {
  test(`${description} rejects rather than reporting completed inference`, async (context) => {
    const fixture = stream_fixture([encoder.encode(source)]);
    context.mock.method(globalThis, "fetch", async () => fixture.response);
    await assert.rejects(stream_response(request()), (error) => {
      assert.equal(error.code, code);
      assert.doesNotMatch(
        `${error.stack}${JSON.stringify(error)}`,
        new RegExp(fixture_key),
      );
      return true;
    });
    assert.equal(fixture.body.locked, false);
  });
}

for (const operation of ["stream", "validate"]) {
  test(`${operation} auth and network failures never expose credentials or raw provider text`, async (context) => {
    context.mock.method(
      globalThis,
      "fetch",
      async () =>
        new Response(`${fixture_key} private provider body`, { status: 401 }),
    );
    const invoke = () =>
      operation === "stream"
        ? stream_response(request())
        : validate_api_key(fixture_key);
    await assert.rejects(invoke(), (error) => {
      assert.equal(error.status, 401);
      assert.match(error.message, /key.*rejected/i);
      assert.doesNotMatch(
        `${error.stack}${JSON.stringify(error)}`,
        /fixture_key|private provider body/,
      );
      return true;
    });
    context.mock.method(globalThis, "fetch", async () => {
      throw new Error(`headers x-api-key: ${fixture_key}`);
    });
    await assert.rejects(invoke(), (error) => {
      assert.equal(error.code, "connection_error");
      assert.doesNotMatch(
        `${error.stack}${JSON.stringify(error)}`,
        /fixture_key|x-api-key/,
      );
      return true;
    });
  });
}

test("missing credentials and invalid inputs fail locally with guided errors, never demo inference", async (context) => {
  let calls = 0;
  context.mock.method(globalThis, "fetch", async () => {
    calls++;
    throw new Error("No request expected");
  });
  await assert.rejects(
    stream_response(request({ api_key: "" })),
    /API key in Settings/i,
  );
  await assert.rejects(validate_api_key("   "), /API key in Settings/i);
  for (const overrides of [
    { messages: [] },
    { messages: [{ role: "system", content: "bad" }] },
    { messages: [{ role: "user", content: "" }] },
    { model: "unknown" },
    {
      messages: [
        { role: "user", content: "Malformed attachments", attachments: null },
      ],
    },

    {
      messages: [
        { role: "user", content: "start" },
        { role: "assistant", content: "prefill" },
      ],
    },
    {
      messages: [
        {
          role: "user",
          content: "",
          attachments: [
            {
              name: "bad.png",
              type: "image/png",
              kind: "image",
              size: 3,
              data: "bad base64",
            },
          ],
        },
      ],
    },
  ])
    await assert.rejects(
      stream_response(request(overrides)),
      (error) => error.code === "invalid_request",
    );
  assert.equal(calls, 0);
});

test("invalid UTF-8 and wrong response media types reject without returning mangled text", async (context) => {
  const bad_utf8 = stream_fixture([Uint8Array.of(0xff)]);
  context.mock.method(globalThis, "fetch", async () => bad_utf8.response);
  await assert.rejects(
    stream_response(request()),
    /connection failed|invalid/i,
  );
  context.mock.method(globalThis, "fetch", async () =>
    Response.json({ content: "not SSE" }),
  );
  await assert.rejects(
    stream_response(request()),
    (error) => error.code === "invalid_stream",
  );
});

test("connection deadline bounds a fetch that never resolves", async (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  context.mock.method(globalThis, "fetch", () => new Promise(() => {}));
  const rejected = assert.rejects(
    stream_response(request()),
    (error) => error.code === "timeout",
  );
  context.mock.timers.tick(30000);
  await rejected;
});

test("idle deadline cancels a stalled stream with no future callbacks", async (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const fixture = stream_fixture([], false);
  context.mock.method(globalThis, "fetch", async () => fixture.response);
  const deltas = [];
  const rejected = assert.rejects(
    stream_response(request({ on_delta: (delta) => deltas.push(delta) })),
    (error) => error.code === "timeout",
  );
  await new Promise((resolve) => setImmediate(resolve));
  context.mock.timers.tick(60000);
  await rejected;
  assert.equal(fixture.was_cancelled(), true);
  assert.equal(fixture.body.locked, false);
  assert.deepEqual(deltas, []);
});

test("total deadline remains bounded even when heartbeat data prevents idle timeout", async (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const fixture = stream_fixture([], false);
  context.mock.method(globalThis, "fetch", async () => fixture.response);
  const rejected = assert.rejects(
    stream_response(request()),
    (error) => error.code === "timeout",
  );
  await new Promise((resolve) => setImmediate(resolve));
  for (let index = 0; index < 11; index++) {
    context.mock.timers.tick(50000);
    fixture.enqueue(": heartbeat\n\n");
    await new Promise((resolve) => setImmediate(resolve));
  }
  context.mock.timers.tick(50000);
  await rejected;
  assert.equal(fixture.was_cancelled(), true);
});
