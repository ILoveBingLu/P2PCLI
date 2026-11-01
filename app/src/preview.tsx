// 开发用：浏览器里假数据渲染界面，看排版。不进正式包。
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import type { Overview, SettingsView, SystemPermission, TerminalSession, Transfer } from "./api";
import type {
  CollaborationArtifact, CollaborationMember, CollaborationProject, CollaborationReply,
  CollaborationSession, CollaborationTask, CollaborationAction, CollaborationDelivery, CollaborationRequest, SessionRef,
} from "./lib/collaboration";

const s = new URLSearchParams(location.search).get("s") ?? "empty";

const ov: Overview = {
  name: "我的 MacBook Pro",
  platform: "macos",
  endpoint_id: "459a2c0a8c093ad76c1b00d7ecfd6dce576f219cdc3c2907cdfc49b3b9ac9ca9",
  code: "246813579",
  password: "ankab7",
  password_fixed: false,
  server: { state: "online" },
  // s=busy：有人正连着本机用终端；s=busy-many：两台同时连着
  sessions:
    s === "busy" || s === "busy-many"
      ? [
          { session: 3, remote: "f9bb5fbc3f", name: "小王的 ThinkPad", platform: "windows", terminals: 1, since: Math.floor(Date.now() / 1000) - 25 * 60 },
          ...(s === "busy-many" ? [{ session: 4, remote: "a1b2c3d4e5", name: "工作室 Mac mini", platform: "macos", terminals: 2, since: Math.floor(Date.now() / 1000) - 3 * 3600 - 10 * 60 }] : []),
        ]
      : [],
  recent: [
    { code: "123456789", endpoint_id: "x", name: "工作室 Mac mini", platform: "macos", last_used: 3 },
    { code: "987654321", endpoint_id: "y", name: "办公室 PC", platform: "windows", last_used: 2 },
  ],
  // 工作室 Mac mini 也在连接历史里，会标「局域网在线」；连客厅那台要密码
  nearby: [
    { endpoint_id: "x", name: "工作室 Mac mini", platform: "macos", online: true },
    { endpoint_id: "c".repeat(64), name: "客厅的 Windows", platform: "windows", online: true },
  ],
  trusted: [{ endpoint_id: "f9bb5fbc3f", name: "工作室 Mac mini", platform: "macos", added_at: 0 }],
  // 后台探测：工作室 Mac mini 在线、配对过，会话是实时的；办公室 PC 离线，只有记住的会话
  devices: [
    {
      endpoint_id: "x", state: "online", paired: true,
      probing: false,
      path: { kind: "ipv6", rtt_ms: 12, remote: "[240e::1]:7788" },
      sessions: [], sessions_live: true, sessions_at: Math.floor(Date.now() / 1000),
    },
    {
      endpoint_id: "y", state: "offline", paired: null, path: null,
      probing: false,
      sessions: [{ id: "d".repeat(32), created_at: Math.floor(Date.now() / 1000) - 26 * 3600, attached: false }],
      sessions_live: false, sessions_at: Math.floor(Date.now() / 1000) - 2 * 3600,
    },
  ],
  paired: [{ endpoint_id: "zz", name: "小王的 ThinkPad", platform: "windows", added_at: 0 }],
  remarks: {},
  inbox_dir: "/Users/me/Downloads/P2PCLI",
};

if (s === "unconfigured") {
  ov.server = { state: "none" };
  ov.code = null;
  ov.recent = [];
}
// s=lan-empty：局域网里没发现设备
if (s === "unconfigured" || s === "lan-empty") ov.nearby = [];
if (s === "offline") ov.server = { state: "offline", error: "无法连接牵线服务器" };
const previewSettings: SettingsView = {
  server: null, device_name: null, relay: null, proxy: null,
  effective_relay: null, effective_server: "wss://p2pcli.aiqji.cn",
  default_server: "wss://p2pcli.aiqji.cn",
  lan_visible: true,
  agent_commands: {},
  inbox_dir: "/Users/me/Downloads/P2PCLI",
  server_presets: [
    { address: "wss://p2pcli.aiqji.cn", label: "国内服务器", note: "国内节点，在国内用连得最快" },
    { address: "wss://p2pcli.yscopy.cn", label: "Cloudflare", note: "海外节点，适合在国外用，国内访问有时不稳" },
  ],
};
const previewPermissions: SystemPermission[] = [{
  id: "local_network",
  name: "局域网访问",
  description: "用于发现同一局域网里的 P2PCLI 设备，并尝试局域网直连。",
  status: s === "permissions-off" ? "denied" : "granted",
  can_request: true,
  settings_url: "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork",
}];
// s=custom-server：设置里填过自定义牵线服务器
if (s === "custom-server") previewSettings.server = "b840307fdbdff2aa164308932575e8f7396b7fd539bd7cfd1927b2456b3fc8a8@rdv.example.com:7788";
let termSeq = 0;
let scanSeq = 0;
let pathSamples = 0;
// s=latency: probe and terminal deliberately disagree; connected views must share updates.
if (s === "latency") ov.devices[0].path!.rtt_ms = 99;
let statusStartedAt = 0;
const fakeTraffic = { sent: 120_000, recv: 2_400_000, packets: 4000, lost: 3 };
const now = Math.floor(Date.now() / 1000);
const collabDevice = "459a2c0a8c093ad76c1b00d7ecfd6dce576f219cdc3c2907cdfc49b3b9ac9ca9";
const collabProject: CollaborationProject = { id: "project-p2pcli", name: "P2PCLI 跨设备验证", coordinator_id: collabDevice, created_at: now - 86_400 };
const collabMembers: CollaborationMember[] = [
  { device_id: collabDevice, role: "admin", revoked: false },
  { device_id: "windows-device-01", role: "worker", revoked: false },
  { device_id: "studio-device-02", role: "dispatcher", revoked: false },
];
const collabSessions: CollaborationSession[] = [
  { id: "session-windows-cli", project_id: collabProject.id, device_id: "windows-device-01", name: "Windows 验证 · Codex", mode: "manual_pull", terminal: null, cwd: "C:\\work\\P2PCLI", available: true, last_seen: now - 8 },
  { id: "session-studio-claude", project_id: collabProject.id, device_id: "studio-device-02", name: "工作室 Mac · Claude", mode: "manual_pull", terminal: null, cwd: "/Users/demo/Projects/P2PCLI", available: true, last_seen: now - 12 },
  { id: "session-studio-old", project_id: collabProject.id, device_id: "studio-device-02", name: "旧验证会话", mode: "manual_pull", terminal: null, cwd: "/Users/demo/Projects/P2PCLI", available: false, last_seen: now - 3600 },
];
const collabTasks: CollaborationTask[] = [
  {
    id: "task-transfer-smoke", project_id: collabProject.id, creator_device: collabDevice, creator_session: null,
    spec: { title: "验证 Windows 文件传输", goal: "在 Windows 上验证最新传输改动，覆盖断线重试和大文件校验。", acceptance: "回传测试命令、结果和日志文件；失败项需要明确说明。", inputs: { commit: "731d8ab", scope: ["send", "receive", "retry"] }, assignee: { device_id: "windows-device-01", session_id: "session-windows-cli" } },
    state: "running", version: 4, attempts: [{ id: "attempt-win-1", assignee: { device_id: "windows-device-01", session_id: "session-windows-cli" }, started_at: now - 820, last_seen: now - 24, stopped: false, result: null }],
    progress: "已完成基础发送测试，正在验证断线重试。", created_at: now - 1200, updated_at: now - 24,
  },
  {
    id: "task-ui-review", project_id: collabProject.id, creator_device: collabDevice, creator_session: null,
    spec: { title: "检查协作界面交接", goal: "检查任务详情、成员权限和离线状态在桌面界面中的表现。", acceptance: "给出截图或逐项检查结果。", inputs: { viewport: "desktop", cases: ["offline", "version_conflict"] }, assignee: { device_id: "studio-device-02", session_id: "session-studio-claude" } },
    state: "result_ready", version: 7, attempts: [{ id: "attempt-ui-1", assignee: { device_id: "studio-device-02", session_id: "session-studio-claude" }, started_at: now - 4200, last_seen: now - 900, stopped: true, result: { summary: "桌面布局和离线提示已检查。", completed: ["任务详情可滚动", "离线状态会暂停写操作"], remaining: [], verification: ["浏览器预览：view=collab"], artifact_ids: ["artifact-ui-report"], failed: false } }],
    progress: "结果已提交，等待协调方验收。", created_at: now - 4600, updated_at: now - 900,
  },
];
const collabArtifacts: CollaborationArtifact[] = [{ id: "artifact-ui-report", task_id: "task-ui-review", attempt_id: "attempt-ui-1", provider_device: "studio-device-02", name: "collaboration-review.md", size: 18_420, hash: "sha256:preview-collaboration-review", received: true, local_path: "/Users/me/Downloads/P2PCLI/collaboration-review.md" }];
const collabClone = <T,>(value: T): T => structuredClone(value);
collabTasks.push({
  ...collabClone(collabTasks[0]), id: "task-protocol-review", state: "queued", version: 1, attempts: [], progress: null, updated_at: now - 45,
  spec: { ...collabClone(collabTasks[0].spec), title: "复核会话隔离和权限", goal: "核对成员撤销后，新请求和旧会话都无法继续访问。", inputs: { branch: "main" } },
});
interface PreviewCollabProject {
  project: CollaborationProject; members: CollaborationMember[]; sessions: CollaborationSession[];
  tasks: CollaborationTask[]; artifacts: CollaborationArtifact[];
}
const collabCoordinators = new Map<string, PreviewCollabProject[]>();
const previewDeliveries: CollaborationDelivery[] = [];
const collabReceipts = new Map<string, { request: string; reply: CollaborationReply }>();
let previewConflict = s === "collab-conflict";
let previewLostReply = s === "collab-lost-reply";
function collabReply(response: CollaborationReply["response"]): CollaborationReply { return collabClone({ response }); }
function collabReject(code: string, message: string): CollaborationReply { return { error: { code, message } }; }
function collabAction(action: CollaborationAction, projectId: string | null, target: string | null): CollaborationReply {
  const key = target ?? "local";
  if (!collabCoordinators.has(key)) {
    const data = collabClone({ project: collabProject, members: collabMembers, sessions: collabSessions, tasks: collabTasks, artifacts: collabArtifacts });
    data.project.coordinator_id = target ?? collabDevice;
    if (s === "collab-viewer") data.members[0].role = "viewer";
    if (s === "collab-artifact-pending") data.artifacts[0].received = false;
    collabCoordinators.set(key, s === "collab-empty" ? [] : [data]);
  }
  const projects = collabCoordinators.get(key)!;
  if (action.type === "projects_list") return collabReply({ type: "projects", projects: projects.map((data) => data.project) });
  if (action.type === "project_create") {
    if (target) return collabReject("forbidden", "项目只能在协调设备本机创建");
    if (!action.name.trim()) return collabReject("invalid_request", "项目名称不能为空");
    const created = { ...collabProject, id: crypto.randomUUID().replaceAll("-", ""), name: action.name, created_at: Math.floor(Date.now() / 1000) };
    projects.push({ project: created, members: [{ device_id: collabDevice, role: "admin", revoked: false }], sessions: [], tasks: [], artifacts: [] });
    return collabReply({ type: "project", project: created });
  }
  const data = projects.find((entry) => entry.project.id === projectId);
  if (!data) return collabReject("not_found", "项目不存在");
  const { members, sessions, tasks, artifacts } = data;
  if (action.type === "sessions_list" && s !== "collab-stale") {
    for (const session of sessions) if (session.available) session.last_seen = Math.floor(Date.now() / 1000) - 8;
  }
  const own = members.find((member) => member.device_id === collabDevice && !member.revoked);
  if (!own) return collabReject("forbidden", "当前设备无权访问项目");
  const canDispatch = own.role === "admin" || own.role === "dispatcher";
  const validAssignee = (ref: SessionRef) => sessions.some((session) => session.id === ref.session_id && session.device_id === ref.device_id && session.available) && members.some((member) => member.device_id === ref.device_id && !member.revoked && member.role !== "viewer");
  if (action.type === "project_get") return collabReply({ type: "project", project: data.project });
  if (action.type === "tasks_list") return collabReply({ type: "tasks", tasks });
  if (action.type === "sessions_list") return collabReply({ type: "sessions", sessions: sessions.filter((session) => members.some((member) => member.device_id === session.device_id && !member.revoked)) });
  if (action.type === "members_list") return collabReply({ type: "members", members });
  if (action.type === "artifacts_list") return collabReply({ type: "artifacts", artifacts: artifacts.filter((artifact) => artifact.task_id === action.task_id) });
  if (action.type === "member_set") {
    if (own.role !== "admin") return collabReject("forbidden", "需要管理员角色");
    const member = members.find((item) => item.device_id === action.device_id);
    if (member?.role === "admin" && !member.revoked && (action.revoked || action.role !== "admin") && members.filter((item) => item.role === "admin" && !item.revoked).length <= 1) return collabReject("invalid_state", "不能撤销最后一位管理员");
    if (member) Object.assign(member, { role: action.role, revoked: action.revoked });
    else members.push({ device_id: action.device_id, role: action.role, revoked: action.revoked });
    if (action.revoked) sessions.filter((session) => session.device_id === action.device_id).forEach((session) => { session.available = false; });
    return collabReply({ type: "ack" });
  }
  if (action.type === "task_create") {
    if (!canDispatch) return collabReject("forbidden", "需要调度者角色");
    if (!validAssignee(action.spec.assignee)) return collabReject("session_unavailable", "执行会话不可用");
    if (!action.spec.title.trim() || !action.spec.goal.trim()) return collabReject("invalid_request", "标题和目标不能为空");
    const task: CollaborationTask = { id: crypto.randomUUID().replaceAll("-", ""), project_id: data.project.id, creator_device: collabDevice, creator_session: null, spec: collabClone(action.spec), state: "queued", version: 1, attempts: [], progress: null, created_at: Math.floor(Date.now() / 1000), updated_at: Math.floor(Date.now() / 1000) };
    tasks.push(task);
    return collabReply({ type: "task", task });
  }
  if (action.type === "task_get") {
    const task = tasks.find((item) => item.id === action.task_id);
    return task ? collabReply({ type: "task", task: collabClone(task) }) : { error: { code: "not_found", message: "任务不存在" } };
  }
  if (action.type === "task_assign" || action.type === "task_accept" || action.type === "task_cancel" || action.type === "task_requeue") {
    const taskId = action.task_id;
    if (!canDispatch) return collabReject("forbidden", "需要调度者角色");
    const task = tasks.find((item) => item.id === taskId);
    if (!task) return { error: { code: "not_found", message: "任务不存在" } };
    if (previewConflict) { previewConflict = false; task.version++; task.progress = "另一会话刚刚更新了任务。"; }
    if (task.version !== action.expected_version) return { error: { code: "version_conflict", message: "任务版本已变化" } };
    if (action.type === "task_assign") {
      if (task.state !== "queued") return collabReject("invalid_state", "只有待领取任务可以调整分配");
      if (!validAssignee(action.assignee)) return collabReject("session_unavailable", "执行会话不可用");
      task.spec.assignee = collabClone(action.assignee);
    }
    if (action.type === "task_accept") {
      if (task.state !== "result_ready") return collabReject("invalid_state", "任务还未提交结果");
      if (task.attempts.at(-1)?.result?.artifact_ids.some((id) => !artifacts.some((artifact) => artifact.id === id && artifact.received && artifact.local_path))) return collabReject("artifact_pending", "产物尚未全部接收");
      task.state = "completed";
    }
    if (action.type === "task_cancel") {
      if (!["queued", "running", "blocked"].includes(task.state)) return collabReject("invalid_state", "当前状态不能取消");
      task.state = task.state === "queued" ? "cancelled" : "cancel_requested";
    }
    if (action.type === "task_requeue") {
      if (!["result_ready", "failed", "cancelled"].includes(task.state) || (task.attempts.length > 0 && !task.attempts.at(-1)?.stopped)) return collabReject("invalid_state", "上一执行尚未停止或当前状态不能重新排队");
      task.state = "queued"; task.progress = null;
    }
    task.version += 1; task.updated_at = Math.floor(Date.now() / 1000);
    return collabReply({ type: "task", task: collabClone(task) });
  }
  return collabReject("unsupported", "预览不支持此操作");
}
async function previewCollabRequest(request: CollaborationRequest, target: string | null): Promise<CollaborationReply> {
  await new Promise((resolve) => setTimeout(resolve, s === "collab-loading" ? 5000 : 180));
  if (s === "collab-offline" || target === "offline") throw new Error("协调设备离线");
  if (s === "collab-error") return collabReject("forbidden", "此设备尚未获授权");
  const key = JSON.stringify([target, request.request_id]);
  const previous = collabReceipts.get(key);
  const serialized = JSON.stringify(request);
  if (previous) return previous.request === serialized ? collabClone(previous.reply) : collabReject("idempotency_conflict", "相同请求编号包含不同内容");
  if ((s === "collab-queued" || target === "queued") && request.action.type === "task_create") {
    const reply = collabReply({ type: "queued", request_id: request.request_id, message: "请求已保存在本机，等待协调设备恢复连接。" });
    previewDeliveries.unshift({ id: key, request_id: request.request_id, project_id: request.project_id, target, operation: request.action.type, state: "pending", attempts: 1, updated_at: Math.floor(Date.now() / 1000), last_error: "协调设备暂不可达" });
    collabReceipts.set(key, { request: serialized, reply });
    return reply;
  }
  if (s === "collab-unverified" && request.action.type === "task_create") {
    const reply = collabReply({ type: "committed_unverified", request_id: request.request_id, message: "协调设备暂时不可达，本机保留了这次请求的提交记录。" });
    collabReceipts.set(key, { request: serialized, reply });
    return reply;
  }
  const reply = collabAction(request.action, request.project_id, target);
  collabReceipts.set(key, { request: serialized, reply });
  if (previewLostReply && request.action.type === "task_create" && reply.response?.type === "task") {
    previewLostReply = false;
    throw new Error("模拟提交成功后响应丢失");
  }
  return reply;
}
// Approval fixtures exercise display/submission without sending keys to a real AI.
const approvalKind = ["codex", "gemini", "opencode"].find((kind) => s === `approval-${kind}`) ?? "claude";
const approvalNames: Record<string, string> = { claude: "Claude Code", codex: "Codex", gemini: "Gemini CLI", opencode: "OpenCode" };
const approvalLabels: Record<string, string[]> = {
  claude: ["Yes", "Yes, and switch to accept edits (auto-approve file edits and common file commands) for this session", "No"],
  codex: ["Yes, proceed (y)", "Yes, and don't ask again for commands that start with `echo` (p)", "No, and tell Codex what to do differently (esc)"],
  gemini: ["Allow once", "Allow for this session", "No, suggest changes (esc)"],
  opencode: ["Allow once", "Allow always", "Reject"],
};
const previewApproval = {
  id: "preview-approval-1",
  context: approvalKind === "claude" ? "Create file · hello.txt\n+ hi\nDo you want to create hello.txt?" : "Run command · ~/Projects/demo\n$ echo hi\nAllow execution?",
  options: approvalLabels[approvalKind].map((label, id) => ({ id, label })),
};
const terminalSessions: TerminalSession[] = s === "sessions" ? [
  { id: "a".repeat(32), created_at: now - 3 * 3600, attached: false },
  { id: "b".repeat(32), created_at: now - 40 * 60, attached: true },
  { id: "c".repeat(32), created_at: now - 90, attached: false },
] : s === "agents" || s === "stalled" ? [
  // s=agents：会话里跑着几种命令行 AI，最后一个是普通终端；s=stalled：忙碌画面八小时没变化
  { id: "a".repeat(32), created_at: now - 3 * 3600, attached: false, agent: { kind: "claude", name: "Claude Code", state: "working",
    unchanged_for: s === "stalled" ? 8 * 3600 : undefined,
    quota: { at: now - 120, windows: [{ minutes: 300, used_percent: 35, resets_at: now + 2 * 3600 }, { minutes: 10080, used_percent: 78, resets_at: now + 2 * 86400 }] },
    settings: { model: "Opus 5.5", effort: "xhigh", mode: "auto" } } },
  // Codex 的 5 小时额度快用完，打开就会提醒一次
  { id: "b".repeat(32), created_at: now - 40 * 60, attached: true, agent: { kind: "codex", name: "Codex", state: "waiting",
    quota: { at: now - 30, windows: [{ minutes: 300, used_percent: 92, resets_at: now + 3600 }, { minutes: 10080, used_percent: 41, resets_at: now + 5 * 86400 }] } } },
  { id: "c".repeat(32), created_at: now - 20 * 60, attached: false, agent: { kind: "gemini", name: "Gemini CLI", state: "idle" } },
  { id: "e".repeat(32), created_at: now - 10 * 60, attached: false, agent: { kind: "zcode", name: "ZCode", state: "idle",
    settings: { model: "bigmodel/GLM-5.3-Flash", effort: "max", mode: "build", efforts: ["low", "high", "max"],
      models: [{ id: "bigmodel/GLM-5.3", name: "GLM-5.3" }, { id: "bigmodel/GLM-5.3-Flash", name: "GLM-5.3-Flash" }, { id: "zai/glm-5.2", name: "GLM-5.2" }] } } },
  { id: "d".repeat(32), created_at: now - 90, attached: false },
] : s.startsWith("collab") ? [
  { id: "a".repeat(32), created_at: now - 3600, attached: false, agent: { kind: "claude", name: "Claude Code", state: "idle" } },
  { id: "b".repeat(32), created_at: now - 1800, attached: false, agent: { kind: "codex", name: "Codex", state: "idle" } },
] : s.startsWith("opencode") ? [
  { id: "e".repeat(32), created_at: now - 1200, attached: false, agent: {
    kind: "opencode", name: "OpenCode",
    state: s === "opencode-working" ? "working" : s === "opencode-waiting" ? "waiting" : "idle",
  } },
] : s.startsWith("approval-") ? [
  { id: "f".repeat(32), created_at: now - 60, attached: false, agent: {
    kind: approvalKind, name: approvalNames[approvalKind], state: "waiting",
    approval: s === "approval-unsupported" ? undefined : previewApproval,
  } },
] : s.startsWith("launcher") ? [
  { id: "f".repeat(32), created_at: now - 60, attached: false, details: {
    window_index: 0, window_name: "zsh", window_count: 1, command: "zsh", current_path: "/Users/demo/Projects/demo", shell_ready: true,
  } },
] : [];
// 工作室 Mac mini 的后台探测结果用同一份会话，连上前后编号对得上
ov.devices[0].sessions = terminalSessions.map((session) => ({ ...session }));
if (s.startsWith("collab")) {
  ov.local = { conn_id: 0, info: { endpoint_id: collabDevice, name: ov.name, platform: "macos", user: "me" } };
  ov.recent = [
    { code: "123456789", endpoint_id: "studio-device-02", name: "工作室 Mac mini", platform: "macos", last_used: now },
    { code: "987654321", endpoint_id: "windows-device-01", name: "办公室 Windows", platform: "windows", last_used: now },
  ];
  ov.nearby = [];
  ov.devices = ov.recent.map((device, index) => ({
    endpoint_id: device.endpoint_id, state: s === "collab-device-offline" && index === 1 ? "offline" : "online",
    paired: true, probing: false, path: { kind: "ipv6", rtt_ms: index ? 28 : 12, remote: null },
    sessions: [terminalSessions[index]], sessions_live: s !== "collab-device-offline" || index !== 1, sessions_at: now,
  }));
  collabSessions[0].terminal = { host_device_id: "windows-device-01", owner_device_id: collabDevice, terminal_id: "b".repeat(32) };
  collabSessions[1].terminal = { host_device_id: "studio-device-02", owner_device_id: collabDevice, terminal_id: "a".repeat(32) };
  collabSessions.push({ id: "session-local", project_id: collabProject.id, device_id: collabDevice, name: "本机 Claude Code", mode: "manual_pull",
    terminal: { host_device_id: collabDevice, owner_device_id: collabDevice, terminal_id: "a".repeat(32) },
    cwd: "/Users/me/Projects/P2PCLI", available: true, last_seen: now - 5 });
  if (s === "collab-unregistered" || s === "collab-empty") collabSessions.length = 0;
  if (s === "collab-stale") for (const session of collabSessions) session.last_seen = now - 3600;
  if (s === "collab-multiple") collabSessions.push({ ...collabSessions[0], id: "second-windows-session", name: "Windows 复核", terminal: null });
  if (s === "collab-team" || s === "collab-team-no-lead") {
    for (let index = 1; index <= 6; index++) {
      const terminalId = `team-terminal-${index}`;
      ov.devices[1].sessions.push({
        ...terminalSessions[1], id: terminalId,
        agent: { ...terminalSessions[1].agent!, kind: index % 2 ? "claude" : "codex", name: `执行会话 ${index + 1}` },
      });
      collabSessions.push({
        ...collabSessions[0], id: `team-session-${index}`, name: `执行会话 ${index + 1}`,
        terminal: { host_device_id: "windows-device-01", owner_device_id: collabDevice, terminal_id: terminalId },
      });
    }
    if (s === "collab-team-no-lead") collabSessions.splice(collabSessions.findIndex((session) => session.id === "session-local"), 1);
  }
  if (s.startsWith("collab-onboarding")) {
    collabSessions.length = 0;
    collabMembers.splice(collabMembers.findIndex((member) => member.device_id === "studio-device-02"), 1);
    ov.devices[0].sessions = Array.from({ length: s === "collab-onboarding-many" ? 7 : 2 }, (_, index) => ({
      ...terminalSessions[0], id: `onboarding-studio-${index + 1}`,
      agent: { ...terminalSessions[0].agent!, kind: "codebuddy", name: "CodeBuddy",
        state: s === "collab-onboarding-busy" && index === 1 ? "working" : "idle" },
    }));
    terminalSessions.push(...ov.devices[0].sessions);
  }
}
// s=agents：Claude Code 过一会儿停下来等确认，再过一会儿干完，看提醒
if (s === "agents") {
  const claude = terminalSessions[0];
  const step = (state: "waiting" | "idle", at: number) => window.setTimeout(() => {
    claude.agent = { ...claude.agent!, state };
    ov.devices[0].sessions = terminalSessions.map((session) => ({ ...session }));
    void emit("server-status", {});
  }, at);
  step("waiting", 4000);
  step("idle", 9000);
}
const attachments = new Map<number, { sessionId: string; status: any; output: any }>();
const shellPrompt = "me@preview ~/Projects/demo % ";
const shellDrafts = new Map<number, string>();
function previewWrite(output: any, text: string) { output.onmessage(new TextEncoder().encode(text).buffer); }
function fakeLauncherInput(termId: number, data: string) {
  if (!s.startsWith("launcher")) return;
  const attachment = attachments.get(termId);
  const session = terminalSessions.find((x) => x.id === attachment?.sessionId);
  if (!attachment || !session?.details) return;
  if (data.includes("\x04") && session.agent) {
    session.agent = undefined;
    session.details.command = "zsh";
    session.details.shell_ready = true;
    previewWrite(attachment.output, "\r\n" + shellPrompt);
  } else if (!session.agent) {
    let draft = shellDrafts.get(termId) ?? "";
    for (const ch of data) {
      if (ch === "\x03" || ch === "\r") { draft = ""; previewWrite(attachment.output, "\r\n" + shellPrompt); }
      else if (ch === "\x7f") { if (draft) { draft = draft.slice(0, -1); previewWrite(attachment.output, "\b \b"); } }
      else if (ch >= " ") { draft += ch; previewWrite(attachment.output, ch); }
    }
    shellDrafts.set(termId, draft);
    session.details.shell_ready = !draft;
  }
}

// s=agents：Claude Code、ZCode 收到 /model、/effort 和 Shift+Tab 时像真的一样改设置；Shift+Tab 的顺序照本机真实的
const fakeModels: Record<string, string> = { opus: "Opus 5.5", sonnet: "Sonnet 5.5", haiku: "Haiku 5.5", fable: "Fable 5.1" };
const fakeModeCycle: Record<string, string[]> = { claude: ["default", "acceptEdits", "plan", "auto"], zcode: ["build", "edit", "yolo", "plan"] };
function fakeAgentInput(termId: number, data: string) {
  const session = terminalSessions.find((x) => x.id === attachments.get(termId)?.sessionId);
  const agent = session?.agent;
  if (!agent?.settings || !fakeModeCycle[agent.kind]) return;
  const settings = { ...agent.settings };
  const model = /\/model (\S+?)(?:\x1b|$)/.exec(data)?.[1];
  const effort = /\/effort (\w+)/.exec(data)?.[1];
  if (model && agent.kind === "claude" && fakeModels[model]) settings.model = fakeModels[model];
  if (model && agent.kind === "zcode" && settings.models?.some((m) => m.id === model)) settings.model = model;
  if (effort) settings.effort = effort;
  const cycle = fakeModeCycle[agent.kind];
  for (const _ of data.match(/\x1b\[Z/g) ?? []) settings.mode = cycle[(cycle.indexOf(settings.mode ?? "") + 1) % cycle.length];
  session!.agent = { ...agent, settings };
  ov.devices[0].sessions = terminalSessions.map((x) => ({ ...x }));
}

// 传文件：s=transfers 一开始就有几条在传、传完、失败的；发文件时模拟进度
const transfers: Transfer[] = [];
const transferBase = { files: 1, error: null, dir: null, paths: [] as string[], started_at: now, finished_at: null as number | null };
if (s === "transfers") {
  transfers.push(
    { ...transferBase, id: "t1", direction: "send", peer: "x", peer_name: "工作室 Mac mini", label: "设计稿 等 3 项", files: 12, total: 420_000_000, done: 168_000_000, state: "running" },
    { ...transferBase, id: "t2", direction: "receive", peer: "y", peer_name: "办公室 PC", label: "报告.pdf", total: 2_400_000, done: 2_400_000, state: "done", finished_at: now + 3600, paths: ["/Users/me/Downloads/P2PCLI/办公室 PC/报告.pdf"] },
    { ...transferBase, id: "t3", direction: "send", peer: "y", peer_name: "办公室 PC", label: "素材.zip", total: 80_000_000, done: 0, state: "failed", finished_at: now + 3600, error: "30 秒内没连上对方，对方可能不在线" },
  );
}
const emitTransfers = () => void emit("transfers", transfers.map((t) => ({ ...t })));
function fakeSend(target: string, paths: string[]) {
  const id = `t${Date.now()}`;
  const name = paths[0]?.split("/").pop() ?? "文件";
  const peer = ov.recent.find((d) => d.endpoint_id === target)?.name ?? "设备";
  const t: Transfer = { ...transferBase, id, direction: "send", peer: target, peer_name: peer, label: paths.length > 1 ? `${name} 等 ${paths.length} 项` : name, total: 30_000_000, done: 0, state: "connecting", started_at: Math.floor(Date.now() / 1000) };
  transfers.unshift(t);
  let step = 0;
  const timer = setInterval(() => {
    step += 1;
    if (step === 1) t.state = "running";
    else t.done = Math.min(t.total, t.done + 6_000_000);
    if (t.done >= t.total) {
      t.state = "done";
      t.finished_at = Math.floor(Date.now() / 1000);
      t.paths = paths.map((p) => `/Users/me/Downloads/P2PCLI/我的 MacBook Pro/${p.split("/").pop()}`);
      clearInterval(timer);
    }
    emitTransfers();
  }, 500);
  emitTransfers();
  return id;
}

const sample = [
  "\x1b[1;32mme@studio\x1b[0m \x1b[1;34m~\x1b[0m % claude\r\n",
  "\x1b[38;5;208m╭──────────────────────────────────────────────╮\x1b[0m\r\n",
  "\x1b[38;5;208m│\x1b[0m \x1b[1m✻ Welcome to Claude Code!\x1b[0m                     \x1b[38;5;208m│\x1b[0m\r\n",
  "\x1b[38;5;208m│\x1b[0m   /help for help, /status for your setup     \x1b[38;5;208m│\x1b[0m\r\n",
  "\x1b[38;5;208m│\x1b[0m   cwd: ~/Projects/demo                       \x1b[38;5;208m│\x1b[0m\r\n",
  "\x1b[38;5;208m╰──────────────────────────────────────────────╯\x1b[0m\r\n\r\n",
  "\x1b[2m> \x1b[0m帮我看看这个仓库最近改了什么\r\n\r\n",
  "\x1b[1m⏺\x1b[0m 我先看一下最近的提交记录。\r\n",
  "\x1b[2m  ⎿  a1b2c3d feat: 初始版本\x1b[0m\r\n",
].join("");

mockWindows("main");
mockIPC(
  async (cmd, args: any) => {
    switch (cmd) {
      case "overview":
        // 跟真实接口一样每次给新对象：s=agents 会改设备状态、断开会改 sessions，同一个对象界面不会重画
        return { ...ov, devices: ov.devices.map((d) => ({ ...d })) };
      case "collab_request":
        return previewCollabRequest(args.request, args.target ?? null);
      case "collab_deliveries":
        return collabClone(previewDeliveries);
      case "notify":
        console.info("系统通知", args.title, args.body);
        return null;
      case "scan_nearby":
        return new Promise((resolve, reject) => setTimeout(() => {
          if (s === "lan-scan-error") return reject("局域网扫描启动失败");
          if (s === "lan-refresh") {
            scanSeq += 1;
            ov.nearby = ov.nearby.map((device, index) => ({ ...device, online: index !== 1 || scanSeq % 2 === 0 }));
            if (scanSeq === 1) ov.nearby.push({ endpoint_id: "d".repeat(64), name: "新发现的 Mac", platform: "macos", online: true });
          }
          resolve(ov.nearby);
        }, 3000));
      case "connect": {
        if (s === "error") return Promise.reject({ reject: "bad_password", message: "配对密码不正确，请向对方获取最新密码。" });
        const lan = ov.nearby.find((d) => d.endpoint_id === args.target);
        const collabRemote = s.startsWith("collab") ? ov.recent.find((device) => device.code === args.target || device.endpoint_id === args.target) : null;
        if (lan && lan.endpoint_id !== "x" && !args.password)
          return Promise.reject({ reject: "need_password", message: "需要输入配对密码" });
        return new Promise((r) =>
          setTimeout(
            () =>
              r({
                conn_id: collabRemote?.endpoint_id === "windows-device-01" ? 2 : 1,
                code: lan ? null : "123456789",
                info: { endpoint_id: collabRemote?.endpoint_id ?? lan?.endpoint_id ?? "x", name: collabRemote?.name ?? lan?.name ?? "工作室 Mac mini", platform: collabRemote?.platform ?? lan?.platform ?? "macos", user: "me",
                  // s=old-peer：对方是不交换版本的旧版本
                  ...(s === "old-peer" ? {} : { version: "0.1.4", os: "macOS 26.1 Tahoe · arm64", multiplexer: "tmux 3.7c" }) },
                path: { kind: s === "relay" ? "relay" : "ipv6", rtt_ms: s === "relay" ? 180 : 12, remote: "[240e::1]:7788" },
              }),
            600,
          ),
        );
      }
      case "list_terminals": {
        if (s.startsWith("collab")) {
          const remote = Number(args.connId) === 2 ? ov.devices[1] : Number(args.connId) === 1 ? ov.devices[0] : null;
          if (remote) return collabClone(remote.sessions);
          const id = (Number(args.connId) === 2 ? "b" : "a").repeat(32);
          return collabClone(terminalSessions.filter((session) => session.id === id));
        }
        if (s === "sessions-error") return Promise.reject("无法连接到会话服务，请检查被控端运行状态。");
        // s=status-live: change metadata, fail briefly, then recover without a reload.
        const elapsed = statusStartedAt ? Date.now() - statusStartedAt : 0;
        if (s === "status-live" && elapsed >= 12000 && elapsed < 18000)
          return Promise.reject("模拟状态查询暂时失败");
        const changed = s === "status-live" && elapsed >= 6000;
        // 模拟网络往返，看得出刷新时列表有没有闪
        return new Promise((r) => setTimeout(() => r(terminalSessions.map((session) => ({
          ...session,
          details: session.details ?? (changed
            ? { window_index: 1, window_name: "编辑器", window_count: 2, command: "vim", current_path: "/Users/demo/Projects/updated" }
            : { window_index: 0, window_name: "工作终端", window_count: 1, command: "zsh", current_path: "/Users/demo/Projects/P2PCLI" }),
        }))), 300));
      }
      case "installed_agents":
        if (s === "launcher-loading") await new Promise((resolve) => setTimeout(resolve, 8000));
        if (s === "launcher-error") throw new Error("模拟检测失败");
        return s === "launcher-empty" ? [] : [
          { kind: "claude", name: "Claude Code", command: "claude" },
          { kind: "codex", name: "Codex", command: "codex" },
          { kind: "opencode", name: "OpenCode", command: "opencode" },
        ];
      case "local_installed_agents":
        return [
          { kind: "claude", name: "Claude Code", command: "claude" },
          { kind: "codex", name: "Codex", command: "codex" },
          { kind: "opencode", name: "OpenCode", command: "opencode" },
        ];
      case "launch_agent": {
        const session = terminalSessions.find((x) => x.id === args.sessionId);
        if (!session?.details?.shell_ready) throw new Error("终端不在空闲提示符");
        session.agent = { kind: args.kind, name: { claude: "Claude Code", codex: "Codex", opencode: "OpenCode" }[args.kind as string] ?? args.kind, state: "idle" };
        session.details.command = args.kind;
        session.details.shell_ready = false;
        console.info("Launcher preview", args.kind, { allowBypass: args.allowBypass });
        for (const attachment of attachments.values()) {
          if (attachment.sessionId === session.id) previewWrite(attachment.output, args.kind + "\r\n\r\n" + session.agent.name + "\r\n> ");
        }
        return null;
      }
      case "choose_agent_option": {
        const session = terminalSessions.find((x) => x.id === args.sessionId);
        const approval = session?.agent?.approval;
        if (!approval || approval.id !== args.approvalId || !approval.options.some((option) => option.id === args.optionId)) throw new Error("确认内容已变化或已处理");
        if (s === "approval-stale") {
          session!.agent = { ...session!.agent!, approval: undefined, state: "working" };
          throw new Error("确认内容已变化或已处理，请查看最新选项");
        }
        console.info("Approval preview choice", approvalKind, args.optionId, approval.options[args.optionId].label);
        await new Promise((resolve) => setTimeout(resolve, 1500));
        session!.agent = { ...session!.agent!, approval: undefined, state: "working" };
        for (const attachment of attachments.values()) {
          if (attachment.sessionId === session!.id) previewWrite(attachment.output, "\r\n已选择：" + approval.options[args.optionId].label + "\r\n");
        }
        return null;
      }
      case "send_agent_message": {
        if (s === "collab-onboarding-old") throw new Error("对方尚不支持自动接入，请更新对方 P2PCLI");
        const terminal = terminalSessions.find((item) => item.id === args.sessionId);
        if (!terminal?.agent || terminal.agent.kind !== args.kind) throw new Error("AI 已退出");
        if (terminal.agent.state !== "idle") throw new Error("AI 正在工作或等待授权，请处理后重新连接协作");
        const [version, coordinatorId, projectId, ownerId, terminalId] = JSON.parse(args.requestId);
        if (version !== "onboard-v1" || terminalId !== args.sessionId) throw new Error("接入请求格式不合法");
        const data = collabCoordinators.get(coordinatorId === collabDevice ? "local" : coordinatorId)?.find((item) => item.project.id === projectId);
        if (!data) throw new Error("项目不存在");
        const receipt = `onboarding:${args.connId}:${args.requestId}`;
        if (collabReceipts.has(receipt)) return null;
        collabReceipts.set(receipt, { request: args.text, reply: { response: { type: "ack" } } });
        console.info("Onboarding preview submitted", args.sessionId, args.requestId);
        if (s !== "collab-onboarding-timeout") window.setTimeout(() => {
          data.sessions.push({
            id: `registered-${args.sessionId}`, project_id: projectId, device_id: "studio-device-02",
            name: terminal.agent!.name + " · " + args.sessionId, mode: "manual_pull",
            terminal: { host_device_id: "studio-device-02", owner_device_id: ownerId, terminal_id: terminalId },
            cwd: null, available: true, last_seen: Math.floor(Date.now() / 1000),
          });
        }, 1500);
        return null;
      }
      case "end_terminal": {
        await new Promise((r) => setTimeout(r, 500));
        if (s === "collab-clear-error") throw new Error("模拟清除失败：设备连接已断开");
        const index = terminalSessions.findIndex((session) => session.id === args.sessionId);
        if (index >= 0) terminalSessions.splice(index, 1);
        const endpointId = s.startsWith("collab")
          ? Number(args.connId) === 2 ? "windows-device-01" : Number(args.connId) === 0 ? collabDevice : "studio-device-02"
          : "x";
        for (const device of ov.devices) {
          if (device.endpoint_id === endpointId) device.sessions = device.sessions?.filter((session) => session.id !== args.sessionId);
        }
        for (const [id, attachment] of attachments) {
          if (attachment.sessionId === args.sessionId) { attachment.status.onmessage({ type: "exited", code: 0 }); attachments.delete(id); }
        }
        return null;
      }
      case "open_terminal": {
        let session = terminalSessions.find((session) => session.id === args.sessionId) ??
          ov.devices.flatMap((device) => device.sessions).find((session) => session.id === args.sessionId);
        if (!session && args.sessionId) return Promise.reject("会话已结束");
        if (!session) { session = { id: crypto.randomUUID().replaceAll("-", ""), created_at: Math.floor(Date.now() / 1000), attached: true }; terminalSessions.push(session); }
        session.attached = true;
        const termId = ++termSeq;
        if (!statusStartedAt) statusStartedAt = Date.now();
        attachments.set(termId, { sessionId: session.id, status: args.status, output: args.output });
        ((window as any).__outs ??= []).push(args.output);
        // tmux attach 使用外层备用屏幕，即使内层 pane 停在普通 Shell。
        const screen = s.startsWith("approval-") ? `\x1b[?1049h${previewApproval.context.replaceAll("\n", "\r\n")}\r\n\r\n${previewApproval.options.map((option) => `${option.id === 0 ? "❯" : " "} ${option.id + 1}. ${option.label}`).join("\r\n")}\r\nEnter to confirm · Esc to cancel\r\n`
          : s.startsWith("launcher") ? "\x1b[?1049h" + shellPrompt : session.agent?.kind === "opencode"
          ? "\x1b[1;36mOpenCode\x1b[0m\r\n\r\nBuild · 模型由当前供应商配置提供\r\n\r\n这是界面预览；控制按键记录在浏览器控制台。\r\n"
          : sample;
        setTimeout(() => args.output.onmessage(new TextEncoder().encode(screen).buffer), 100);
        // Reproduce disconnected overlays alongside another mounted terminal tab.
        if (s === "disconnected" && termId === 1) {
          setTimeout(() => args.status.onmessage({ type: "closed", reason: "网络中断，连接超时" }), 1500);
        }
        return { term_id: termId, session_id: session.id };
      }
      case "term_close": {
        const attachment = attachments.get(args.termId);
        attachments.delete(args.termId);
        const session = terminalSessions.find((s) => s.id === attachment?.sessionId);
        if (session) session.attached = [...attachments.values()].some((a) => a.sessionId === session.id);
        return null;
      }
      case "plugin:app|version":
        return "0.1.0";
      // s=update：发布页上有新版本；s=update-fail：下载到一半失败
      case "check_update":
        return s === "update" || s === "update-fail"
          ? { version: "0.1.1", current_version: "0.1.0", notes: "局域网发现更稳定\n修复连接面板偶尔不弹出" }
          : null;
      case "install_update": {
        // 模拟下载 30 MB，大约 12 秒下完，然后进入安装（真软件这时会重启，这里停在安装）
        const total = 30 * 1024 * 1024;
        let index = 0;
        let downloaded = 0;
        const send = (message: unknown) =>
          (window as any).__TAURI_INTERNALS__.runCallback(args.progress.id, { index: index++, message });
        return new Promise((_, reject) => {
          const timer = window.setInterval(() => {
            downloaded = Math.min(total, downloaded + 160 * 1024 * (3 + Math.random() * 4));
            send({ event: "downloading", downloaded, total });
            if (s === "update-fail" && downloaded > total * 0.4) {
              window.clearInterval(timer);
              reject("下载更新失败：网络连接中断，稍后再试");
            } else if (downloaded >= total) {
              window.clearInterval(timer);
              send({ event: "installing" });
            }
          }, 250);
        });
      }
      case "set_password_fixed":
        ov.password_fixed = !!args.fixed;
        return null;
      case "refresh_password":
        ov.password = ov.password === "ankab7" ? "h9km2p" : "ankab7";
        return ov.password;
      case "forget_recent":
        ov.recent = ov.recent.filter((device) => device.code !== args.code);
        return null;
      case "set_device_remark":
        ov.remarks[args.endpointId] ??= { device: "", sessions: {} };
        ov.remarks[args.endpointId].device = String(args.remark ?? "").trim();
        if (!ov.remarks[args.endpointId].device && !Object.keys(ov.remarks[args.endpointId].sessions).length)
          delete ov.remarks[args.endpointId];
        return ov.remarks[args.endpointId]?.device ?? "";
      case "set_session_remark": {
        ov.remarks[args.endpointId] ??= { device: "", sessions: {} };
        const remark = String(args.remark ?? "").trim();
        if (remark) ov.remarks[args.endpointId].sessions[args.sessionId] = remark;
        else delete ov.remarks[args.endpointId].sessions[args.sessionId];
        if (!ov.remarks[args.endpointId].device && !Object.keys(ov.remarks[args.endpointId].sessions).length)
          delete ov.remarks[args.endpointId];
        return remark;
      }
      case "refresh_devices":
        return null;
      case "remove_paired":
        ov.paired = ov.paired.filter((device) => device.endpoint_id !== args.endpointId);
        return null;
      case "remove_trusted":
        ov.trusted = ov.trusted.filter((device) => device.endpoint_id !== args.endpointId);
        return null;
      case "kick":
        ov.sessions = ov.sessions.filter((session) => session.session !== args.session);
        return true;
      case "connection_stats": {
        // 计数从“连上”开始累计，每次多一点；s=relay 走中继、偶尔丢包
        fakeTraffic.sent += 900 + Math.random() * 3000;
        fakeTraffic.recv += 4000 + Math.random() * 40000;
        fakeTraffic.packets += 30;
        if (s === "relay" && Math.random() < 0.3) fakeTraffic.lost += 1;
        const relay = s === "relay";
        return {
          path: { kind: relay ? "relay" : "ipv6", rtt_ms: relay ? 180 : 12, remote: relay ? "https://relay.example.com./" : "[240e::1]:7788" },
          local: relay ? null : "240e:abc::5",
          public_v4: "203.0.113.25",
          sent_bytes: Math.round(fakeTraffic.sent),
          recv_bytes: Math.round(fakeTraffic.recv),
          sent_packets: fakeTraffic.packets,
          lost_packets: fakeTraffic.lost,
        };
      }
      case "path_info":
        if (s === "latency") return { kind: "ipv6", rtt_ms: ++pathSamples === 1 ? 20 : 35, remote: "[240e::1]:7788" };
        return { kind: s === "relay" ? "relay" : "ipv6", rtt_ms: s === "relay" ? 180 : 12, remote: "[240e::1]:7788" };
      case "get_settings":
        return { ...previewSettings };
      case "system_permissions":
      case "request_system_permission":
        return previewPermissions;
      case "open_system_settings":
        return null;
      case "save_settings":
        previewSettings.server = args.server?.trim() || null;
        previewSettings.device_name = args.deviceName?.trim() || null;
        previewSettings.relay = args.relay?.trim() || null;
        previewSettings.proxy = args.proxy?.trim() || null;
        previewSettings.lan_visible = args.lanVisible ?? true;
        previewSettings.agent_commands = args.agentCommands ?? {};
        ov.name = previewSettings.device_name ?? "我的 MacBook Pro";
        return null;
      case "restart":
        return null;
      case "transfers":
        return transfers.map((t) => ({ ...t }));
      case "pick_paths":
        return args.folders ? ["/Users/me/Projects/demo"] : ["/Users/me/Desktop/截图 1.png", "/Users/me/Desktop/notes.md"];
      case "send_files":
        return fakeSend(args.target, args.paths);
      case "respond_files":
        return null;
      case "cli_status":
        return s === "cli-missing"
          ? { path: "/Users/me/.local/bin/p2pcli", installed: false, on_path: false, bundled: true }
          : { path: "/Users/me/.local/bin/p2pcli", installed: true, on_path: true, bundled: true };
      case "install_cli":
        return { path: "/Users/me/.local/bin/p2pcli", installed: true, on_path: true, bundled: true };
      case "term_input":
        ((window as any).__inputs ??= []).push(args.data);
        if (s.startsWith("opencode")) console.info("OpenCode preview input", JSON.stringify(args.data));
        fakeAgentInput(args.termId, args.data);
        fakeLauncherInput(args.termId, args.data);
        return null;
      case "selftest_config":
        return null;
      case "diagnose":
        if (s === "error") return Promise.reject("网络检测暂时不可用，请稍后重试。");
        return new Promise((r) =>
          setTimeout(
            () =>
              r({
                rating: "差",
                summary: "没有 IPv6，IPv4 是对称 NAT：对方网络好时还能直连，否则走中继，延迟会高一些",
                ipv6_addrs: [],
                ipv4_public: "203.0.113.25",
                nat: "symmetric",
                nat_source: "relay",
                stun_error: "小米 STUN 服务器连不上，可能是路由器或代理拦了 UDP 3478",
                relay: "https://relay.example.com/",
                relay_connected: true,
                relay_latency_ms: 38,
                notes: [
                  "没有公网 IPv6，只能走 IPv4 打洞或中继",
                  "IPv4 是对称 NAT（经中继测得）：每个目标外网端口都不同；对方是锥形 NAT 时多半还能打通，对方也是对称 NAT 就只能走中继",
                  "STUN 检测没成功：小米 STUN 服务器连不上，可能是路由器或代理拦了 UDP 3478",
                  "中继已连上：https://relay.example.com/，延迟 38 毫秒",
                  "牵线服务器在线",
                ],
              }),
            800,
          ),
        );
      default:
        return null;
    }
  },
  { shouldMockEvents: true },
);

await import("./main");
// 测拖文件这类系统事件：页面里调 window.__emit("tauri://drag-drop", { paths, position })
(window as any).__emit = emit;
if (s === "file-request") {
  setTimeout(() => emit("file-request", { id: 11, remote: "f9bb5fbc3f0a1b2c3d4e5f60718293a4", name: "客厅的 Windows", platform: "windows", files: 3, total: 12_582_912, items: ["照片", "说明.txt"] }), 300);
}
if (s === "request") {
  setTimeout(() => emit("host-request", { id: 9, remote: "f9bb5fbc3f0a1b2c3d4e5f60718293a4", name: "小王的 ThinkPad", platform: "windows" }), 300);
}
