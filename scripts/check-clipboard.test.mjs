import assert from "node:assert/strict";
import { test } from "node:test";
import { readClipboardText, writeClipboardText } from "../app/src/lib/clipboard.ts";

function setup(t, native, invoke, clipboard) {
  for (const [key, value] of Object.entries({
    isTauri: native,
    window: { __TAURI_INTERNALS__: { invoke } },
    navigator: { clipboard },
  })) {
    const original = Object.getOwnPropertyDescriptor(globalThis, key);
    Object.defineProperty(globalThis, key, { configurable: true, value });
    t.after(() => {
      if (original) Object.defineProperty(globalThis, key, original);
      else delete globalThis[key];
    });
  }
}

test("desktop reads and writes through native IPC even when web clipboard access is denied", async (t) => {
  const calls = [];
  const text = "\u4f60\u597d\nsecond line\t\u{1f680}";
  const denied = () => { throw new DOMException("Permission denied", "NotAllowedError"); };
  setup(t, true, async (command, args) => {
    calls.push([command, args]);
    return command.endsWith("read_text") ? text : undefined;
  }, { readText: denied, writeText: denied });

  assert.equal(await readClipboardText(), text);
  assert.equal(await writeClipboardText(text), undefined);
  assert.deepEqual(calls, [
    ["plugin:clipboard-manager|read_text", {}],
    ["plugin:clipboard-manager|write_text", { text, label: undefined }],
  ]);
});

test("desktop works without a browser clipboard API, including empty text", async (t) => {
  setup(t, true, async () => "", undefined);
  assert.equal(await readClipboardText(), "");
  assert.equal(await writeClipboardText(""), undefined);
});

test("native errors propagate without falling back to the web clipboard", async (t) => {
  const error = new Error("Native clipboard unavailable");
  const unexpected = () => assert.fail("Desktop must not call the web clipboard");
  setup(t, true, async () => { throw error; }, { readText: unexpected, writeText: unexpected });
  await assert.rejects(readClipboardText(), (actual) => actual === error);
  await assert.rejects(writeClipboardText("text"), (actual) => actual === error);
});

test("browser preview uses the browser clipboard, not native IPC", async (t) => {
  let text = "preview";
  setup(t, false, () => assert.fail("Preview must not call native IPC"), {
    async readText() { return text; },
    async writeText(value) { text = value; },
  });
  assert.equal(await readClipboardText(), "preview");
  await writeClipboardText("replacement\ntext");
  assert.equal(await readClipboardText(), "replacement\ntext");
});

test("browser permission errors remain rejected promises for the existing UI error handlers", async (t) => {
  const error = new DOMException("Permission denied", "NotAllowedError");
  const denied = async () => { throw error; };
  setup(t, false, () => assert.fail("Preview must not call native IPC"), {
    readText: denied,
    writeText: denied,
  });
  await assert.rejects(readClipboardText(), (actual) => actual === error);
  await assert.rejects(writeClipboardText("text"), (actual) => actual === error);
});
