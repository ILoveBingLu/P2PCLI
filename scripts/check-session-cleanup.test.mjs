import assert from "node:assert/strict";
import { test } from "node:test";
import { withoutClearedSessions } from "../app/src/lib/sessionCleanup.ts";

const overview = {
  name: "Local",
  devices: [
    { endpoint_id: "a", sessions: [{ id: "same-id" }, { id: "keep" }], sessions_live: true },
    { endpoint_id: "b", sessions: [{ id: "same-id" }], sessions_live: false },
    { endpoint_id: "c" },
  ],
};

test("clearing a session only removes it from its owning device", () => {
  const result = withoutClearedSessions(overview, new Map([["a", new Set(["same-id"])]]));
  assert.deepEqual(result.devices[0].sessions, [{ id: "keep" }]);
  assert.equal(result.devices[0].sessions_live, true);
  assert.equal(result.devices[1], overview.devices[1]);
  assert.equal(result.devices[2], overview.devices[2]);
  assert.equal(result.name, "Local");
  assert.equal(overview.devices[0].sessions.length, 2);
});

test("late cached snapshots cannot resurrect confirmed cleared sessions", () => {
  const cleared = new Map([["a", new Set(["same-id"])]]);
  const updated = { ...overview, devices: [
    { ...overview.devices[0], sessions: [...overview.devices[0].sessions, { id: "new" }] },
    overview.devices[1],
  ] };
  assert.deepEqual(withoutClearedSessions(updated, cleared).devices[0].sessions, [{ id: "keep" }, { id: "new" }]);
});

test("cancelled or failed requests do not filter any sessions", () => {
  assert.equal(withoutClearedSessions(overview, new Map()), overview);
});

test("missing cached lists and repeated clears are safe", () => {
  const cleared = new Map([["a", new Set(["same-id"])], ["c", new Set(["missing"])]]);
  const result = withoutClearedSessions(overview, cleared);
  assert.deepEqual(withoutClearedSessions(result, cleared), result);
  assert.equal(result.devices[2].sessions, undefined);
});
