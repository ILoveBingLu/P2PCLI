import assert from "node:assert/strict";
import { test } from "node:test";
import { canCoordinate, canLink, connectCollaborationSession, endpointKey, linkInputs, matchingSessions, registrationDrafts, registrationDraftText, registrationPrompt, sessionReady, taskDestination, taskSource, taskTeam, teamParticipants, teamTaskSpec } from "../app/src/lib/collaborationLinks.ts";

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

const lead = { id: "lead", device_id: "a", name: "主会话", available: true, last_seen: 100, terminal: null };
const workers = Array.from({ length: 7 }, (_, index) => ({
  ...lead, id: `worker-${index}`, device_id: "b", name: `会话 ${index}`, terminal: null,
}));
const teamMembers = [{ device_id: "a", role: "admin", revoked: false }, { device_id: "b", role: "worker", revoked: false }];
const teamOptions = {
  source, destination: { deviceId: "b", deviceName: "Windows" }, ownerId: "a", coordinatorId: "a",
  goal: "实现登录\n完善测试\n检查文档", acceptance: "全部需求验证通过",
  lead, participants: workers, members: teamMembers, now: 200,
};
test("all sessions creates one lead-owned task with every participant, not five duplicate tasks", () => {
  const spec = teamTaskSpec(teamOptions);
  assert.deepEqual(spec.assignee, { device_id: "a", session_id: "lead" });
  assert.equal(spec.goal, teamOptions.goal);
  assert.equal(spec.title, "实现登录");
  assert.equal(spec.inputs.p2pcli_team.participants.length, 7);
  assert.equal(spec.inputs.p2pcli_team.strategy, "balanced_workload");
  assert.match(spec.inputs.p2pcli_team.instructions, /不把完整主任务重复广播/);
  assert.match(spec.inputs.p2pcli_team.instructions, /负载最低/);
  assert.match(spec.inputs.p2pcli_team.instructions, /p2pcli_parent/);
  assert.match(spec.inputs.p2pcli_team.instructions, /原 request-id/);
  assert.match(spec.inputs.p2pcli_team.instructions, /最终验收交给用户/);
  const task = { creator_device: "a", spec };
  assert.deepEqual(taskSource(task), source);
  assert.deepEqual(taskDestination(task), teamOptions.destination);
  assert.equal(taskTeam(task).participants.length, 7);
});
test("team leads require active dispatcher or admin membership without changing roles", () => {
  for (const role of ["worker", "viewer"]) {
    const members = [{ ...teamMembers[0], role }, teamMembers[1]];
    assert.equal(canCoordinate(lead, members, 200), false);
    assert.throws(() => teamTaskSpec({ ...teamOptions, members }), /调度权限/);
    assert.equal(members[0].role, role);
  }
  assert.equal(canCoordinate(lead, [{ ...teamMembers[0], role: "dispatcher" }], 200), true);
  assert.equal(canCoordinate(lead, [{ ...teamMembers[0], revoked: true }], 200), false);
  assert.equal(canCoordinate({ ...lead, last_seen: 0 }, teamMembers, 200), false);
});
test("team submission rejects unavailable participants and never silently drops them", () => {
  for (const participant of [{ ...workers[0], last_seen: 0 }, { ...workers[0], available: false }]) {
    assert.throws(() => teamTaskSpec({ ...teamOptions, participants: [...workers.slice(1), participant] }), /参与会话已不可用/);
  }
  assert.throws(() => teamTaskSpec({ ...teamOptions, members: [teamMembers[0], { ...teamMembers[1], revoked: true }] }), /参与会话已不可用/);
  assert.throws(() => teamTaskSpec({ ...teamOptions, participants: [] }), /参与会话已不可用/);
  assert.throws(() => teamTaskSpec({ ...teamOptions, participants: [lead] }), /不同于主会话/);
  assert.throws(() => teamTaskSpec({ ...teamOptions, goal: " \n " }), /任务目标/);
});
test("team participants exclude the lead and duplicate registrations of the same terminal", () => {
  const terminal = { host_device_id: "b", owner_device_id: "a", terminal_id: "terminal" };
  const first = { ...workers[0], terminal };
  const duplicate = { ...workers[1], terminal };
  assert.deepEqual(teamParticipants([lead, first, duplicate, workers[2]], lead), [first, workers[2]]);
  assert.deepEqual(teamParticipants([first, duplicate, workers[2]], first), [workers[2]]);
});
test("legacy or malformed destination metadata falls back to the assignee", () => {
  for (const inputs of [null, {}, { p2pcli_link: { version: 1, destination: null } }, { p2pcli_link: { version: 2, destination: teamOptions.destination } }]) {
    assert.equal(taskDestination({ spec: { assignee: { device_id: "b" }, inputs } }).deviceId, "b");
  }
});
test("team metadata is validated before displaying participants", () => {
  for (const inputs of [null, {}, { p2pcli_team: null }, { p2pcli_team: { version: 1, lead: null, participants: [] } },
    { p2pcli_team: { version: 1, lead: { device_id: "a", session_id: "lead" }, participants: [null] } }]) {
    assert.equal(taskTeam({ spec: { inputs } }), null);
  }
});
test("all unregistered terminals get separate scoped prompts, including those beyond five avatars", () => {
  const endpoints = Array.from({ length: 7 }, (_, index) => ({ ...source, terminalId: `terminal-${index}`, sessionName: `CodeBuddy ${index}` }));
  const drafts = registrationDrafts("project", "coordinator", "owner", endpoints);
  assert.equal(drafts.length, 7);
  for (const [index, draft] of drafts.entries()) {
    assert.deepEqual(draft.endpoint, endpoints[index]);
    assert.ok(draft.prompt.includes(`"terminal_id": "terminal-${index}"`));
    assert.equal(draft.prompt.includes(`"terminal_id": "terminal-${(index + 1) % 7}"`), false);
  }
  const text = registrationDraftText(drafts);
  assert.match(text, /不要在一个会话中登记全部身份/);
  for (const endpoint of endpoints) assert.ok(text.includes(endpoint.terminalId));
});
test("onboarding deduplicates exact endpoints and rejects missing terminal identities", () => {
  const drafts = registrationDrafts("p", "c", "o", [source, source, destination]);
  assert.equal(drafts.length, 2);
  assert.equal(registrationDraftText([drafts[0]]), drafts[0].prompt);
  assert.throws(() => registrationDrafts("p", "c", "o", []), /目标 AI 会话/);
  assert.throws(() => registrationDrafts("p", "c", "o", [source, device]), /目标 AI 会话/);
  assert.equal(registrationDrafts("p", "c", "o", [source, { ...source, deviceId: "another-device" }]).length, 2);
});

function onboardingHarness() {
  const draft = registrationDrafts("project", "coordinator", "owner", [source])[0];
  const session = { id: "registered", device_id: source.deviceId, terminal: {
    host_device_id: source.deviceId, owner_device_id: "owner", terminal_id: source.terminalId,
  }, available: true, last_seen: 100 };
  const state = { sessions: [], members: [{ device_id: source.deviceId, revoked: false, role: "worker" }] };
  let time = 100_000, sends = 0;
  const controller = new AbortController();
  const io = {
    signal: controller.signal, timeoutMs: 5_000, now: () => time,
    read: async () => state,
    send: async () => { sends++; },
    wait: async () => { time += 1_000; },
  };
  return { draft, session, state, controller, io, sends: () => sends };
}
test("automatic onboarding sends once and waits for matching registration", async () => {
  const h = onboardingHarness();
  let reads = 0;
  h.io.read = async () => {
    if (++reads === 4) h.state.sessions.push(h.session);
    return h.state;
  };
  await connectCollaborationSession(h.draft, h.io);
  assert.equal(h.sends(), 1);
  assert.equal(reads, 4);
});
test("already registered terminals are not sent another onboarding message", async () => {
  const h = onboardingHarness();
  h.state.sessions.push(h.session);
  await connectCollaborationSession(h.draft, h.io);
  assert.equal(h.sends(), 0);
});
test("send receipt, other owner, stale heartbeat and unavailable sessions do not mean connected", async () => {
  for (const change of [
    { terminal: { host_device_id: source.deviceId, owner_device_id: "other", terminal_id: source.terminalId } },
    { last_seen: -1000 }, { available: false },
  ]) {
    const h = onboardingHarness();
    h.state.sessions.push({ ...h.session, ...change });
    await assert.rejects(connectCollaborationSession(h.draft, h.io), /尚未确认登记/);
    assert.equal(h.sends(), 1);
  }
});
test("missing or revoked permission blocks input, including during verification", async () => {
  for (const member of [null, { revoked: true }, { role: "viewer" }]) {
    const h = onboardingHarness();
    h.state.members = member ? [{ ...h.state.members[0], ...member }] : [];
    await assert.rejects(connectCollaborationSession(h.draft, h.io), /权限已失效/);
    assert.equal(h.sends(), 0);
  }
  const h = onboardingHarness();
  h.io.wait = async () => { h.state.members[0].revoked = true; };
  await assert.rejects(connectCollaborationSession(h.draft, h.io), /权限已失效/);
  assert.equal(h.sends(), 1);
});
test("busy, unsupported and ambiguous send errors are never automatically retried", async () => {
  for (const message of ["AI 正在工作", "对方尚不支持自动接入", "会话请求超时"]) {
    const h = onboardingHarness();
    let count = 0;
    h.io.send = async () => { count++; throw new Error(message); };
    await assert.rejects(connectCollaborationSession(h.draft, h.io), new RegExp(message));
    assert.equal(count, 1);
  }
});
test("cancellation prevents input and stops subsequent registration checks", async () => {
  const h = onboardingHarness();
  h.controller.abort();
  await assert.rejects(connectCollaborationSession(h.draft, h.io), { name: "AbortError" });
  assert.equal(h.sends(), 0);
  const pending = onboardingHarness();
  pending.io.wait = async () => pending.controller.abort();
  await assert.rejects(connectCollaborationSession(pending.draft, pending.io), { name: "AbortError" });
  assert.equal(pending.sends(), 1);
});
test("onboarding request identity survives retries but isolates projects and terminals", () => {
  const first = registrationDrafts("p", "c", "o", [source])[0];
  assert.equal(registrationDrafts("p", "c", "o", [source])[0].requestId, first.requestId);
  assert.notEqual(registrationDrafts("other", "c", "o", [source])[0].requestId, first.requestId);
  assert.notEqual(registrationDrafts("p", "c", "other", [source])[0].requestId, first.requestId);
  assert.notEqual(registrationDrafts("p", "c", "o", [destination])[0].requestId, first.requestId);
});
