import type { CollaborationMember, CollaborationSession, CollaborationTask, SessionRef, TaskSpec } from "./collaboration";
import { t } from "@/i18n";

export interface CollaborationEndpoint {
  deviceId: string;
  deviceName: string;
  terminalId?: string;
  sessionName?: string;
  agentKind?: string;
}
export interface CollaborationLink {
  source: CollaborationEndpoint;
  destination: CollaborationEndpoint;
}
export interface CollaborationDraft {
  endpoint: CollaborationEndpoint;
  source?: CollaborationEndpoint;
  prompt: string;
  projectId: string;
  coordinatorId: string;
  ownerId: string;
  requestId: string;
}

export const endpointKey = (endpoint: CollaborationEndpoint) =>
  JSON.stringify([endpoint.deviceId, endpoint.terminalId ?? null]);

export function canLink(source: CollaborationEndpoint, destination: CollaborationEndpoint) {
  return source.deviceId !== destination.deviceId ||
    !!(source.terminalId && destination.terminalId && source.terminalId !== destination.terminalId);
}

export function endpointLabel(endpoint: CollaborationEndpoint) {
  return endpoint.sessionName ? `${endpoint.deviceName} / ${endpoint.sessionName}` : endpoint.deviceName;
}

export function matchingSessions(sessions: CollaborationSession[], endpoint: CollaborationEndpoint, ownerId: string) {
  return sessions.filter((session) => session.device_id === endpoint.deviceId &&
    (!endpoint.terminalId || (session.terminal?.terminal_id === endpoint.terminalId &&
      session.terminal.host_device_id === endpoint.deviceId && session.terminal.owner_device_id === ownerId)));
}

export function sessionReady(session: CollaborationSession, members: CollaborationMember[], now = Date.now() / 1000) {
  return session.available && now - session.last_seen <= 120 &&
    members.some((member) => member.device_id === session.device_id && !member.revoked && member.role !== "viewer");
}

export function linkInputs(source: CollaborationEndpoint, ownerId: string, destination?: CollaborationEndpoint) {
  return { p2pcli_link: { version: 1, owner_device_id: ownerId, source, ...(destination ? { destination } : {}) } };
}

export function canCoordinate(session: CollaborationSession, members: CollaborationMember[], now = Date.now() / 1000) {
  return sessionReady(session, members, now) &&
    members.some((member) => member.device_id === session.device_id && !member.revoked &&
      (member.role === "admin" || member.role === "dispatcher"));
}

export function teamParticipants(sessions: CollaborationSession[], lead: CollaborationSession | null) {
  const seen = new Set<string>();
  const key = (session: CollaborationSession) => JSON.stringify(session.terminal ?
    [session.device_id, session.terminal.host_device_id, session.terminal.owner_device_id, session.terminal.terminal_id] :
    [session.device_id, session.id]);
  if (lead) seen.add(key(lead));
  return sessions.filter((session) => {
    const id = key(session);
    if (seen.has(id)) return false;
    seen.add(id);
    return true;
  });
}

export function teamTaskSpec({
  source, destination, ownerId, coordinatorId, goal, acceptance, lead, participants, members, now = Date.now() / 1000,
}: {
  source: CollaborationEndpoint; destination: CollaborationEndpoint; ownerId: string; coordinatorId: string; goal: string; acceptance: string;
  lead: CollaborationSession; participants: CollaborationSession[]; members: CollaborationMember[]; now?: number;
}): TaskSpec {
  if (!goal.trim()) throw new Error(t("请填写任务目标"));
  if (!canCoordinate(lead, members, now)) throw new Error(t("主会话不可用或没有调度权限，请重新选择"));
  if (!participants.length || participants.some((session) => !sessionReady(session, members, now))) {
    throw new Error(t("参与会话已不可用，请重新选择"));
  }
  const workers = teamParticipants(participants, lead);
  if (!workers.length) throw new Error(t("至少需要一个不同于主会话的参与会话"));
  const ref = (session: CollaborationSession) => ({ device_id: session.device_id, session_id: session.id });
  return {
    title: goal.trim().split("\n")[0].slice(0, 120), goal: goal.trim(), acceptance: acceptance.trim(),
    assignee: ref(lead),
    inputs: {
      ...linkInputs(source, ownerId, destination),
      p2pcli_team: {
        version: 1, coordinator_id: coordinatorId, lead: ref(lead),
        participants: workers.map((session) => ({ ...ref(session), name: session.name })),
        strategy: "balanced_workload",
        instructions: [
          t("你是此主任务的主会话。先确认并领取分配给自己的主任务，再拆分、调度和汇总，不把完整主任务重复广播给每个会话。"),
          t("使用本机已有的 p2pcli collab 会话凭据；协调设备作用域必须与登记凭据一致。不要输出、复制或传送 token，不自动提权。"),
          t("核对 task create/list/show/progress/result/cancel/stop/accept 和 session list 的 --help。所有调用使用参数数组和 JSON 文件，不将任务文本或会话名称拼接为 shell 命令。"),
          t("将 goal 和 acceptance 拆成有唯一 requirement_id 的独立需求，记录依赖、验收条件、预计工作量、工作目录和文件所有权；有依赖的需求按顺序执行，共享目录的冲突写入必须串行。"),
          t("每次分配前读取 session list 和 task list，限定在 participants 名单内且仍在线、可用、获授权的会话。按当前未完成任务的预计工作量优先分配给负载最低者，工作量相近时轮流分配；不得向未选中的会话扩大范围。"),
          t("每个需求通过 task create --file 创建一个子任务，assignee 指定对应参与会话。inputs.p2pcli_parent 必须包含 version:1、task_id:当前主任务ID、requirement_id 和 estimated_work。保留必要输入资料，不复制 p2pcli_team，避免递归拆分。"),
          t("将拆分计划、子任务ID和分配情况写入主任务 progress。创建前查询相同父任务ID和 requirement_id，重试使用原 request-id 和原参数；queued 或 committed_unverified 不是执行成功，不重复派发。"),
          t("持续查询主任务和子任务状态并维持心跳。参与者需自行领取子任务；离线时报告 blocked，不假定超时已经停止。重新分配前先取消旧任务并确认执行已停止。"),
          t("主任务收到取消请求时停止新增分配，取消属于本主任务的子任务，并确认它们已停止后才确认主任务停止。父子关系是 inputs 元数据，系统不会自动级联取消。"),
          t("汇总每个需求的真实结果、验证证据与已接收产物，核对依赖及全部验收条件。子任务失败、未结束或产物未收到时不得宣称主任务完成；在主任务 result 中列出子任务ID、完成项、剩余项和验证结果，最终验收交给用户。"),
        ].join("\n"),
      },
    },
  };
}

export function taskDestination(task: CollaborationTask): CollaborationEndpoint {
  const inputs = task.spec.inputs;
  if (inputs && typeof inputs === "object" && "p2pcli_link" in inputs) {
    const link = inputs.p2pcli_link;
    if (link && typeof link === "object" && "version" in link && link.version === 1 && "destination" in link) {
      const destination = link.destination;
      if (destination && typeof destination === "object" &&
        "deviceId" in destination && typeof destination.deviceId === "string" &&
        "deviceName" in destination && typeof destination.deviceName === "string") {
        return { deviceId: destination.deviceId, deviceName: destination.deviceName };
      }
    }
  }
  return { deviceId: task.spec.assignee.device_id, deviceName: task.spec.assignee.device_id };
}

export function taskTeam(task: CollaborationTask): { lead: SessionRef; participants: SessionRef[] } | null {
  const inputs = task.spec.inputs;
  if (!inputs || typeof inputs !== "object" || !("p2pcli_team" in inputs)) return null;
  const team = inputs.p2pcli_team;
  const isRef = (value: unknown): value is SessionRef => !!value && typeof value === "object" &&
    "device_id" in value && typeof value.device_id === "string" &&
    "session_id" in value && typeof value.session_id === "string";
  if (!team || typeof team !== "object" || !("version" in team) || team.version !== 1 ||
    !("lead" in team) || !isRef(team.lead) ||
    !("participants" in team) || !Array.isArray(team.participants) || !team.participants.every(isRef)) return null;
  return { lead: team.lead, participants: team.participants };
}

export function taskSource(task: CollaborationTask): CollaborationEndpoint {
  const inputs = task.spec.inputs;
  if (inputs && typeof inputs === "object" && "p2pcli_link" in inputs) {
    const link = inputs.p2pcli_link;
    if (link && typeof link === "object" && "version" in link && link.version === 1 && "source" in link) {
      const source = link.source;
      if (source && typeof source === "object" && "deviceId" in source && typeof source.deviceId === "string" &&
        "deviceName" in source && typeof source.deviceName === "string") {
        return {
          deviceId: source.deviceId, deviceName: source.deviceName,
          terminalId: "terminalId" in source && typeof source.terminalId === "string" ? source.terminalId : undefined,
          sessionName: "sessionName" in source && typeof source.sessionName === "string" ? source.sessionName : undefined,
        };
      }
    }
  }
  return { deviceId: task.creator_device, deviceName: task.creator_device };
}

// Credentials stay with the receiving AI; the desktop only submits and verifies onboarding.
export function registrationDrafts(projectId: string, coordinatorId: string, ownerId: string, endpoints: CollaborationEndpoint[]): CollaborationDraft[] {
  if (!endpoints.length || endpoints.some((endpoint) => !endpoint.terminalId)) throw new Error(t("请先选择目标 AI 会话"));
  const seen = new Set<string>();
  return endpoints.filter((endpoint) => {
    const key = endpointKey(endpoint);
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  }).map((endpoint) => ({
    endpoint, prompt: registrationPrompt(projectId, coordinatorId, ownerId, endpoint),
    projectId, coordinatorId, ownerId,
    requestId: JSON.stringify(["onboard-v1", coordinatorId, projectId, ownerId, endpoint.terminalId]),
  }));
}

export async function connectCollaborationSession(draft: CollaborationDraft, io: {
  read: () => Promise<{ sessions: CollaborationSession[]; members: CollaborationMember[] }>;
  send: () => Promise<void>;
  wait: () => Promise<void>;
  now?: () => number;
  signal: AbortSignal;
  timeoutMs?: number;
}): Promise<void> {
  const now = io.now ?? Date.now;
  const deadline = now() + (io.timeoutMs ?? 120_000);
  const registered = async () => {
    io.signal.throwIfAborted();
    const { sessions, members } = await io.read();
    io.signal.throwIfAborted();
    if (!members.some((member) => member.device_id === draft.endpoint.deviceId && !member.revoked && member.role !== "viewer")) {
      throw new Error(t("目标设备的协作权限已失效，未继续接入"));
    }
    return matchingSessions(sessions, draft.endpoint, draft.ownerId).some((session) => sessionReady(session, members, now() / 1000));
  };
  if (await registered()) return;
  io.signal.throwIfAborted();
  await io.send();
  // A send receipt is not a registration receipt. Never retry terminal input in this loop.
  while (now() < deadline) {
    if (await registered()) return;
    await io.wait();
  }
  io.signal.throwIfAborted();
  throw new Error(t("接入请求已发送，但尚未确认登记。请查看会话中的授权或错误，不要重复发送。"));
}

export function registrationDraftText(drafts: CollaborationDraft[]): string {
  if (drafts.length === 1) return drafts[0].prompt;
  return [
    t("以下为各会话独立的接入请求。只将对应段落交给该会话，不要在一个会话中登记全部身份。"),
    ...drafts.map(({ endpoint, prompt }) => t("## {{value}}（终端 {{terminalId}}）\n\n{{prompt}}", { value: endpointLabel(endpoint), terminalId: endpoint.terminalId, prompt })),
  ].join("\n\n");
}

export function registrationPrompt(projectId: string, coordinatorId: string, ownerId: string, endpoint: CollaborationEndpoint) {
  const scope = ["p2pcli", "--json", "collab", "--coordinator", coordinatorId, "--project", projectId];
  const terminal = endpoint.terminalId ? {
    host_device_id: endpoint.deviceId, owner_device_id: ownerId, terminal_id: endpoint.terminalId,
  } : null;
  return [
    t("请将当前 AI 会话接入这次 P2PCLI 协作。只做登记与心跳，不执行尚未领取的任务。"),
    t("协调设备：{{coordinatorId}}\n项目：{{projectId}}\n本会话所在设备：{{deviceId}}", { coordinatorId, projectId, deviceId: endpoint.deviceId }),
    t("先用 p2pcli collab session register --help 核对本机支持的参数；若版本不支持，报告原因，不要假装已接入。"),
    terminal ? t("将以下终端引用写入本机临时 JSON 文件，登记时通过 --terminal-file 指向它：\n{{value}}", { value: JSON.stringify(terminal, null, 2) }) : "",
    t("登记命令参数数组（用子进程参数数组调用，不把设备名或会话名拼接为 shell 命令；替换本机路径占位符）：\n{{value}}", { value: JSON.stringify([
      ...scope, "session", "register", "--name", endpoint.sessionName ?? endpoint.deviceName, "--mode", "manual-pull",
      ...(terminal ? ["--terminal-file", t("<上述本机临时文件路径>")] : []),
    ]) }),
    t("凭据只保存在本机，不要输出或传送 token。若当前会话已有相同项目和终端引用的凭据，复用它，不要重复登记。"),
    t("使用登记返回的凭据文件回报可用，命令参数：\n{{value}}", { value: JSON.stringify([...scope, "--session-file", t("<本机凭据文件路径>"), "session", "heartbeat", "--available", "true"]) }),
    t("随后查询分配给自己的任务，命令参数：\n{{value}}\n明确领取后再执行，并通过 task progress / task result 回报。", { value: JSON.stringify([...scope, "--session-file", t("<本机凭据文件路径>"), "task", "list"]) }),
    t("任务等待期间维持心跳；需要权限时停下请求用户确认。不自动扩大权限，不迁移其他会话的上下文。"),
  ].filter(Boolean).join("\n\n");
}
