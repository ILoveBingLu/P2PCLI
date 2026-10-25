import assert from "node:assert/strict";
import { setImmediate } from "node:timers/promises";
import { test } from "node:test";
import { pollConnections } from "../app/src/lib/pollConnections.ts";

test("a stalled device does not delay other devices or overlap its own requests", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const slow = Promise.withResolvers();
  const calls = [];
  const values = [];
  const stop = pollConnections([1, 2, 2], (id) => {
    calls.push(id);
    return id === 1 ? slow.promise : Promise.resolve("ready");
  }, (id, value) => values.push([id, value]), assert.fail);
  t.after(stop);
  await setImmediate();
  for (let i = 0; i < 3; i++) {
    t.mock.timers.tick(3000);
    await setImmediate();
  }
  assert.deepEqual(calls, [1, 2, 2, 2, 2]);
  assert.equal(values.length, 4);
  stop();
  slow.resolve("late");
  await setImmediate();
  t.mock.timers.tick(30000);
  await setImmediate();
  assert.equal(values.length, 4, "late responses after stopping must be ignored");
  assert.equal(calls.length, 5, "stopping must cancel scheduled refreshes");
});

test("failed queries retry independently and can recover", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const failure = new Error("offline");
  const errors = [];
  const values = [];
  let attempts = 0;
  const stop = pollConnections([7], async () => {
    if (++attempts === 1) throw failure;
    return "recovered";
  }, (id, value) => values.push([id, value]), (id, error) => errors.push([id, error]));
  t.after(stop);
  await setImmediate();
  assert.deepEqual(errors, [[7, failure]]);
  t.mock.timers.tick(3000);
  await setImmediate();
  assert.deepEqual(values, [[7, "recovered"]]);
});

test("stopping ignores pending failures and does not schedule a retry", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const pending = Promise.withResolvers();
  let calls = 0;
  const stop = pollConnections([1], () => { calls++; return pending.promise; }, assert.fail, assert.fail);
  stop();
  pending.reject(new Error("late failure"));
  await setImmediate();
  t.mock.timers.tick(30000);
  await setImmediate();
  assert.equal(calls, 1);
});
