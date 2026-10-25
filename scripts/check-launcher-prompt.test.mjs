import assert from "node:assert/strict";
import { test } from "node:test";
import { emptyShellPrompt, promptGuard } from "../app/src/lib/launcherPrompt.ts";

test("recognizes empty macOS, PowerShell and cmd prompts", () => {
  for (const line of ["PS C:\\work> ", "C:\\work>", "me@mac project % ", "[me@host dir]$ ", "me@host ~/src>", "❯ ", "$ "]) {
    assert.equal(emptyShellPrompt(line), true, line);
  }
});

test("does not mistake drafts, continuation or application input for an empty shell", () => {
  for (const line of ["", "> ", ">> ", "PS C:\\work> claude", "me@mac project % echo $", "Password:", "Do you agree?", "❯ draft", "❯ echo ❯", "1:\\work>"]) {
    assert.equal(emptyShellPrompt(line), false, line);
  }
});

test("local input blocks stale terminal output until a changed empty prompt arrives", () => {
  const guard = promptGuard();
  assert.equal(guard.ready("row1:empty", true), true);
  guard.input("row1:empty");
  assert.equal(guard.ready("row1:empty", true), false);
  assert.equal(guard.ready("row1:empty", true), false);
  assert.equal(guard.ready("row1:draft", false), false);
  assert.equal(guard.ready("row2:empty", true), true);
});

test("a second keystroke blocks the latest screen too", () => {
  const guard = promptGuard();
  guard.input("row1:empty");
  assert.equal(guard.ready("row1:draft", false), false);
  guard.input("row1:draft");
  assert.equal(guard.ready("row1:draft", true), false);
  assert.equal(guard.ready("row2:empty", true), true);
});
