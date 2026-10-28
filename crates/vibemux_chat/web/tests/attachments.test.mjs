import test from "node:test";
import assert from "node:assert/strict";
import { app_config } from "../src/config.js";
import { prepare_attachment } from "../src/services/chat_store.js";

function install_reader(context, failure = null) {
  const calls = [];
  class Fixture_File_Reader {
    result = null;
    readAsText(file) {
      calls.push("text");
      this.read(file, false);
    }
    readAsDataURL(file) {
      calls.push("data_url");
      this.read(file, true);
    }
    async read(file, binary) {
      if (failure === "throw")
        throw new Error("Fixture synchronous read failure");
      const bytes = new Uint8Array(await file.arrayBuffer());
      this.onprogress?.({
        lengthComputable: true,
        loaded: 1,
        total: bytes.length,
      });
      this.onprogress?.({
        lengthComputable: true,
        loaded: bytes.length,
        total: bytes.length,
      });
      if (failure === "error") {
        this.onerror?.();
        return;
      }
      if (failure === "abort") {
        this.onabort?.();
        return;
      }
      this.result =
        failure === "invalid"
          ? null
          : binary
            ? `data:${file.type || "application/octet-stream"};base64,${Buffer.from(bytes).toString("base64")}`
            : new TextDecoder().decode(bytes);
      this.onload?.();
    }
  }
  const original = Object.getOwnPropertyDescriptor(globalThis, "FileReader");
  Object.defineProperty(globalThis, "FileReader", {
    configurable: true,
    value: Fixture_File_Reader,
  });
  context.after(() => {
    if (original) Object.defineProperty(globalThis, "FileReader", original);
    else delete globalThis.FileReader;
  });
  return calls;
}

test("images and PDFs are read as base64 with native FileReader progress", async (context) => {
  const calls = install_reader(context);
  for (const [type, name, kind] of [
    ["image/png", "image.png", "image"],
    ["application/pdf", "notes.pdf", "pdf"],
  ]) {
    const progress = [];
    const attachment = await prepare_attachment(
      new File(["abc"], name, { type }),
      (percent) => progress.push(percent),
    );
    assert.equal(attachment.type, type);
    assert.equal(attachment.name, name);
    assert.equal(attachment.size, 3);
    assert.equal(attachment.kind, kind);
    assert.equal(attachment.data, "YWJj");
    assert.ok(attachment.id.length > 0);
    assert.equal(progress[0], 0);
    assert.ok(progress.some((percent) => percent > 0 && percent < 100));
    assert.equal(progress.at(-1), 100);
  }
  assert.deepEqual(calls, ["data_url", "data_url"]);
});

test("UTF-8 text and untyped known extensions are decoded without discarding characters", async (context) => {
  const calls = install_reader(context);
  const file = new File(["Hello 🌍\n<not HTML markup>"], "notes.md");
  const attachment = await prepare_attachment(file);
  assert.equal(attachment.type, "text/markdown");
  assert.equal(attachment.kind, "text");
  assert.equal(attachment.data, "Hello 🌍\n<not HTML markup>");
  assert.equal(attachment.size, file.size);
  assert.deepEqual(calls, ["text"]);
});

test("oversized and unsupported files fail before reading and exactly 10 MiB is accepted", async (context) => {
  const calls = install_reader(context);
  await assert.rejects(
    prepare_attachment(
      new File(
        [new Uint8Array(app_config.max_attachment_bytes + 1)],
        "big.txt",
        { type: "text/plain" },
      ),
    ),
    /10.*MB|10.*MiB/i,
  );
  await assert.rejects(
    prepare_attachment(
      new File(["binary"], "program.exe", { type: "application/octet-stream" }),
    ),
    /unsupported/i,
  );
  await assert.rejects(
    prepare_attachment(
      new File(["svg"], "drawing.svg", { type: "image/svg+xml" }),
    ),
    /unsupported/i,
  );
  assert.deepEqual(calls, []);
  const attachment = await prepare_attachment(
    new File([new Uint8Array(app_config.max_attachment_bytes)], "limit.txt", {
      type: "text/plain",
    }),
  );
  assert.equal(attachment.size, 10485760);
  assert.deepEqual(calls, ["text"]);
});

for (const failure of ["error", "abort", "invalid"]) {
  test(`FileReader ${failure} rejects rather than returning a pretend attachment`, async (context) => {
    install_reader(context, failure);
    await assert.rejects(
      prepare_attachment(
        new File(["hello"], "notes.txt", { type: "text/plain" }),
      ),
      /read|cancel/i,
    );
  });
}
