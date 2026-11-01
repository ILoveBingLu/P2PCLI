import assert from "node:assert/strict";
import { test } from "node:test";
import { registerHooks } from "node:module";

// Resolve the same extensionless local TS imports that Vite resolves, without compiling the app.
registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith(".ts") && specifier.startsWith(".") && !/\.[a-z]+$/i.test(specifier)) {
      return nextResolve(new URL(`${specifier}.ts`, context.parentURL).href, context);
    }
    return nextResolve(specifier, context);
  },
});
const {
  activityFromEvent, EventCursor, mergeActivities, mergeRelation, parseRelations,
  relationAvailable, relationKey, relationStorageKey, sessionEndpoint, taskWire, TransferActivityTracker,
} = await import("../app/src/lib/collaborationWires.ts");

const source = { deviceId: "a", deviceName: "Mac" };
const destination = { deviceId: "b", deviceName: "Windows" };
const link = { source, destination };
const scope = { ownerId: "a", coordinatorId: "a", projectId: "p" };
const sessions = [
  { id: "lead", device_id: "a", name: "Lead", available: true, last_seen: 100, terminal: { host_device_id: "a", owner_device_id: "a", terminal_id: "ta" } },
  { id: "worker", device_id: "b", name: "Worker", available: true, last_seen: 100, terminal: { host_device_id: "b", owner_device_id: "a", terminal_id: "tb" } },
];
const members = [{ device_id: "a", role: "admin", revoked: false }, { device_id: "b", role: "worker", revoked: false }];
const overview = { endpoint_id: "a", devices: [{ endpoint_id: "b", state: "online" }] };
const task = {
  id: "t1", project_id: "p", creator_device: "a", creator_session: "lead",
  spec: { title: "Implement feature", inputs: {}, assignee: { device_id: "b", session_id: "worker" } },
  attempts: [{ result: null }], state: "running",
};
const event = (kind, data = task, seq = 1) => ({ seq, kind, entity_id: "t1", data, created_at: 100 });

test("relationship identities are bidirectional and terminal-specific", () => {
  assert.equal(relationKey(link), relationKey({ source: destination, destination: source }));
  assert.notEqual(relationKey(link), relationKey({ source, destination: { ...destination, terminalId: "one" } }));
  for (const change of [{ ownerId: "other" }, { coordinatorId: "other" }, { projectId: "other" }]) {
    assert.notEqual(relationStorageKey(scope), relationStorageKey({ ...scope, ...change }));
  }
});
test("local records survive round trips and reject corrupt or mismatched identities", () => {
  const records = mergeRelation([], link, 100);
  assert.deepEqual(parseRelations(JSON.stringify({ version: 1, relations: records })), records);
  for (const raw of [null, "not-json", "[]", '{"version":2,"relations":[]}',
    JSON.stringify({ version: 1, relations: [{ ...records[0], id: "spoofed" }] }),
    JSON.stringify({ version: 1, relations: [{ ...records[0], source: null }] })]) {
    assert.deepEqual(parseRelations(raw), []);
  }
});
test("history imports do not resurrect removed records; a new confirmed connection can", () => {
  const records = mergeRelation([], link, 100);
  assert.equal(mergeRelation(records, link, 200), records);
  const hidden = records.map((item) => ({ ...item, hidden: true }));
  assert.equal(mergeRelation(hidden, link, 200, true), hidden);
  assert.equal(mergeRelation(hidden, link, 200)[0].hidden, false);
  assert.deepEqual(mergeRelation([], { source, destination: source }, 100), []);
});
test("availability requires live devices, valid membership and fresh terminal registrations", () => {
  assert.equal(relationAvailable(link, overview, sessions, members, 110000), true);
  assert.equal(relationAvailable(link, { ...overview, devices: [{ endpoint_id: "b", state: "offline" }] }, sessions, members, 110000), false);
  assert.equal(relationAvailable(link, overview, sessions, [{ ...members[0] }, { ...members[1], revoked: true }], 110000), false);
  assert.equal(relationAvailable(link, overview, sessions, members, 221000), false);
  const specific = { source, destination: { ...destination, terminalId: "wrong" } };
  assert.equal(relationAvailable(specific, overview, sessions, members, 110000), false);
});
test("terminal mapping never targets a different owner", () => {
  assert.equal(sessionEndpoint({ device_id: "b", session_id: "worker" }, sessions, "a").terminalId, "tb");
  assert.equal(sessionEndpoint({ device_id: "b", session_id: "worker" }, sessions, "other").terminalId, undefined);
});
test("task delivery and result return follow opposite actual assignee routes", () => {
  const outgoing = activityFromEvent(event("task_claimed"), sessions, "a", 1000);
  const incoming = activityFromEvent(event("task_result"), sessions, "a", 1000);
  assert.equal(outgoing.source.deviceId, "a");
  assert.equal(outgoing.destination.deviceId, "b");
  assert.deepEqual(incoming.source, outgoing.destination);
  assert.deepEqual(incoming.destination, outgoing.source);
  assert.equal(outgoing.kind, "task");
  assert.equal(incoming.kind, "result");
});
test("team tasks point at their lead, never fake a broadcast to every participant", () => {
  const team = { ...task, creator_session: null, spec: {
    ...task.spec, assignee: { device_id: "a", session_id: "lead" },
    inputs: { p2pcli_link: { version: 1, source, destination }, p2pcli_team: { participants: sessions } },
  } };
  assert.equal(taskWire(team, sessions, "a").destination.deviceId, "a");
  assert.equal(activityFromEvent(event("task_claimed", team), sessions, "a", 0), null);
});
test("heartbeats, queued work, local acceptance and message consumption never animate", () => {
  for (const kind of ["session_heartbeat", "task_created", "task_assigned", "task_progress", "task_requeued", "task_accepted", "message_accepted", "message_consumed"]) {
    assert.equal(activityFromEvent(event(kind), sessions, "a", 0), null);
  }
});
test("message pulses require delivery confirmation and do not expose message bodies", () => {
  const data = { from: { device_id: "b", session_id: "worker" }, to: { device_id: "a", session_id: "lead" }, body: "private prompt" };
  const pulse = activityFromEvent(event("message_delivered", data), sessions, "a", 0);
  assert.equal(pulse.kind, "message");
  assert.equal(pulse.source.terminalId, "tb");
  assert.equal(pulse.destination.terminalId, "ta");
  assert.equal(JSON.stringify(pulse).includes("private prompt"), false);
  for (const invalid of [null, {}, { from: null, to: data.to }]) {
    assert.equal(activityFromEvent(event("message_delivered", invalid), sessions, "a", 0), null);
  }
});
test("failed results are static errors, not success pulses", () => {
  const pulse = activityFromEvent(event("task_result", { ...task, attempts: [{ result: { failed: true } }] }), sessions, "a", 0);
  assert.equal(pulse.failed, true);
});
test("event baseline drains pages silently, deduplicates replies, and advances past invisible events", () => {
  const cursor = new EventCursor();
  assert.deepEqual(cursor.accept([event("task_claimed", task, 1)], 1), []);
  assert.deepEqual(cursor.accept([], 1000), []);
  assert.deepEqual(cursor.accept([], 1000), []);
  const fresh = event("task_claimed", task, 1001);
  assert.deepEqual(cursor.accept([fresh, fresh], 1001), [fresh]);
  assert.deepEqual(cursor.accept([fresh], 1001), []);
  assert.deepEqual(cursor.accept([fresh], 1), []);
});
test("activity bursts coalesce by kind and direction, expire and remain bounded", () => {
  const a = activityFromEvent(event("task_claimed"), sessions, "a", 1000);
  const b = { ...a, id: "new", label: "Latest task" };
  const c = activityFromEvent(event("task_result"), sessions, "a", 1000);
  assert.deepEqual(mergeActivities([a], [b, c], 1100), [b, c]);
  assert.deepEqual(mergeActivities([a], [], 4001), []);
  const many = Array.from({ length: 100 }, (_, i) => ({ ...a, source: { ...source, deviceId: `device-${i}` } }));
  assert.equal(mergeActivities([], many, 1100).length, 32);
});

const file = { id: "file", peer: "b", peer_name: "Windows", label: "report.pdf", direction: "send", state: "running", done: 0, total: 100 };
test("file pulses need actual byte increases, stop on stalls, and never replay initial progress", () => {
  const tracker = new TransferActivityTracker();
  assert.deepEqual(tracker.update([{ ...file, done: 10 }], source, 1000), []);
  const moving = tracker.update([{ ...file, done: 40 }], source, 1500);
  assert.equal(moving[0].label, "发送 report.pdf · 40%");
  assert.equal(moving[0].source.deviceId, "a");
  assert.equal(tracker.update([{ ...file, done: 40 }], source, 4000).length, 1);
  assert.deepEqual(tracker.update([{ ...file, done: 40 }], source, 4501), []);
});
test("file direction is reversed for receiving and completion stops movement immediately", () => {
  const tracker = new TransferActivityTracker();
  tracker.update([{ ...file, direction: "receive" }], source, 1000);
  const moving = tracker.update([{ ...file, direction: "receive", done: 70 }], source, 2000);
  assert.equal(moving[0].source.deviceId, "b");
  assert.equal(moving[0].destination.deviceId, "a");
  assert.deepEqual(tracker.update([{ ...file, done: 100, state: "done" }], source, 2100), []);
});
test("waiting and failures never animate; new failures get a short-lived error marker", () => {
  const tracker = new TransferActivityTracker();
  assert.deepEqual(tracker.update([{ ...file, state: "connecting" }], source, 1000), []);
  assert.deepEqual(tracker.update([{ ...file, state: "waiting", done: 5 }], source, 2000), []);
  const failed = tracker.update([{ ...file, state: "failed" }], source, 3000);
  assert.equal(failed[0].failed, true);
  assert.deepEqual(tracker.update([{ ...file, state: "failed" }], source, 7001), []);
  assert.deepEqual(new TransferActivityTracker().update([{ ...file, state: "failed" }], source, 1000), []);
});
