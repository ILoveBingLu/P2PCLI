import assert from "node:assert/strict";
import { test } from "node:test";
import { canLink, endpointKey, linkInputs, matchingSessions, registrationPrompt, sessionReady, taskSource } from "../app/src/lib/collaborationLinks.ts";

const device = { deviceId: "a", deviceName: "Mac" };
const source = { ...device, terminalId: "first", sessionName: "Codex" };
const destination = { ...device, terminalId: "second" };
test("links allow cross-device and different same-device sessions, not self-links", () => {
  assert.equal(canLink(source, destination), true);
  assert.equal(canLink(source, source), false);
  assert.equal(canLink(device, source), false);
  assert.equal(canLink(device, { deviceId: "b" }), true);
  assert.notEqual(endpointKey(device), endpointKey(source));
});
test("terminal selection respects host and owner, never silently picks a different session", () => {
  const session = { device_id: "a", terminal: { terminal_id: "first", host_device_id: "a", owner_device_id: "owner" } };
  const sessions = [session, { ...session, terminal: null }, { ...session, terminal: { ...session.terminal, owner_device_id: "other" } }];
  assert.deepEqual(matchingSessions(sessions, source, "owner"), [session]);
  assert.deepEqual(matchingSessions(sessions, destination, "owner"), []);
  assert.equal(matchingSessions(sessions, device, "owner").length, 3);
});
test("availability requires fresh heartbeat and active executing membership", () => {
  const session = { device_id: "a", available: true, last_seen: 100 };
  const members = [{ device_id: "a", role: "worker", revoked: false }];
  assert.equal(sessionReady(session, members, 200), true);
  assert.equal(sessionReady(session, members, 221), false);
  assert.equal(sessionReady({ ...session, available: false }, members, 200), false);
  assert.equal(sessionReady(session, [{ ...members[0], revoked: true }], 200), false);
  assert.equal(sessionReady(session, [{ ...members[0], role: "viewer" }], 200), false);
});
test("source metadata round-trips and old or malformed task inputs fall back safely", () => {
  const task = { creator_device: "owner", spec: { inputs: linkInputs(source, "owner") } };
  assert.deepEqual(taskSource(task), source);
  for (const inputs of [null, [], "text", {}, { p2pcli_link: { version: 1, source: null } }]) {
    assert.deepEqual(taskSource({ ...task, spec: { inputs } }), { deviceId: "owner", deviceName: "owner" });
  }
});
test("onboarding prompt scopes the terminal and preserves credential ownership", () => {
  const prompt = registrationPrompt("project", "coordinator", "owner", source);
  assert.match(prompt, /"--coordinator","coordinator","--project","project"/);
  assert.match(prompt, /"owner_device_id": "owner"/);
  assert.match(prompt, /--terminal-file/);
  assert.match(prompt, /不执行尚未领取的任务/);
  assert.match(prompt, /不要输出或传送 token/);
  assert.match(prompt, /"task","list"/);
});
test("onboarding keeps shell metacharacters inside structured arguments", () => {
  const name = "agent'; $(touch /tmp/not-a-command) `whoami`";
  const prompt = registrationPrompt("project", "coordinator", "owner", { ...source, sessionName: name });
  const registration = prompt.split("\n").find((line) => line.startsWith('["p2pcli"') && line.includes('"register"'));
  const args = JSON.parse(registration);
  assert.equal(args[args.indexOf("--name") + 1], name);
  assert.match(prompt, /不把设备名或会话名拼接为 shell 命令/);
});
