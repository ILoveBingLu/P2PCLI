import assert from "node:assert/strict";
import { test } from "node:test";
import { terminalResizeFrame } from "../app/src/lib/terminalResizeFrame.ts";

function setup(t) {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const frames = new Set();
  const writes = [];
  const renders = new Set();
  let refreshes = 0;
  const copy = { style: {}, removeAttribute() {}, querySelectorAll: () => [] };
  const host = {
    querySelector: () => ({ clientWidth: 800, clientHeight: 600, cloneNode: () => copy, querySelectorAll: () => [] }),
    ownerDocument: {
      createElement: () => {
        const frame = { setAttribute() {}, append() {}, remove: () => frames.delete(frame) };
        return frame;
      },
    },
    append: (frame) => frames.add(frame),
  };
  const term = {
    rows: 24,
    write: (data, parsed) => writes.push(parsed),
    onRender: (fn) => { renders.add(fn); return { dispose: () => renders.delete(fn) }; },
    refresh: () => refreshes++,
  };
  const guard = terminalResizeFrame(term, host);
  t.after(() => guard.dispose());
  return {
    guard, frames, writes,
    refreshes: () => refreshes,
    render: () => [...renders].forEach((fn) => fn()),
    output: () => guard.write(new Uint8Array([27])),
  };
}

test("keeps the previous frame across split clear/redraw output until parsed and rendered", (t) => {
  const s = setup(t);
  s.guard.begin();
  s.output();
  s.writes.shift()();
  t.mock.timers.tick(60);
  s.output();
  t.mock.timers.tick(100);
  assert.equal(s.frames.size, 1, "pending parsing must keep the old frame");
  assert.equal(s.refreshes(), 0);
  s.writes.shift()();
  t.mock.timers.tick(80);
  assert.equal(s.refreshes(), 1);
  assert.equal(s.frames.size, 1, "parsing alone is not rendering");
  s.render();
  assert.equal(s.frames.size, 0);
});

test("no output and continuous output both release within the hard deadline", (t) => {
  const s = setup(t);
  s.guard.begin();
  t.mock.timers.tick(1000);
  assert.equal(s.frames.size, 0);
  s.guard.begin();
  for (let i = 0; i < 20; i++) {
    s.output();
    s.writes.shift()();
    s.guard.begin(); // another resize must not postpone the deadline
    t.mock.timers.tick(50);
  }
  assert.equal(s.frames.size, 0);
});

test("a hidden terminal with no render event releases the frame", (t) => {
  const s = setup(t);
  s.guard.begin();
  s.output();
  s.writes.shift()();
  t.mock.timers.tick(80);
  t.mock.timers.tick(150);
  assert.equal(s.frames.size, 0);
});

test("disposal cancels pending callbacks and removes the snapshot", (t) => {
  const s = setup(t);
  s.guard.begin();
  s.output();
  s.guard.dispose();
  s.writes.shift()();
  t.mock.timers.tick(2000);
  assert.equal(s.frames.size, 0);
  assert.equal(s.refreshes(), 0);
});
