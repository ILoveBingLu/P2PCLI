import type { CollaborationMember, CollaborationSession, CollaborationTask } from "./collaboration";

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

export function linkInputs(source: CollaborationEndpoint, ownerId: string) {
  return { p2pcli_link: { version: 1, owner_device_id: ownerId, source } };
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

// The receiving AI runs this itself: no credentials or terminal input are sent automatically.
export function registrationPrompt(projectId: string, coordinatorId: string, ownerId: string, endpoint: CollaborationEndpoint) {
  const scope = ["p2pcli", "--json", "collab", "--coordinator", coordinatorId, "--project", projectId];
  const terminal = endpoint.terminalId ? {
    host_device_id: endpoint.deviceId, owner_device_id: ownerId, terminal_id: endpoint.terminalId,
  } : null;
  return [
    "请将当前 AI 会话接入这次 P2PCLI 协作。只做登记与心跳，不执行尚未领取的任务。",
    `协调设备：${coordinatorId}\n项目：${projectId}\n本会话所在设备：${endpoint.deviceId}`,
    "先用 p2pcli collab session register --help 核对本机支持的参数；若版本不支持，报告原因，不要假装已接入。",
    terminal ? `将以下终端引用写入本机临时 JSON 文件，登记时通过 --terminal-file 指向它：\n${JSON.stringify(terminal, null, 2)}` : "",
    `登记命令参数数组（用子进程参数数组调用，不把设备名或会话名拼接为 shell 命令；替换本机路径占位符）：\n${JSON.stringify([
      ...scope, "session", "register", "--name", endpoint.sessionName ?? endpoint.deviceName, "--mode", "manual-pull",
      ...(terminal ? ["--terminal-file", "<上述本机临时文件路径>"] : []),
    ])}`,
    "凭据只保存在本机，不要输出或传送 token。若当前会话已有相同项目和终端引用的凭据，复用它，不要重复登记。",
    `使用登记返回的凭据文件回报可用，命令参数：\n${JSON.stringify([...scope, "--session-file", "<本机凭据文件路径>", "session", "heartbeat", "--available", "true"])}`,
    `随后查询分配给自己的任务，命令参数：\n${JSON.stringify([...scope, "--session-file", "<本机凭据文件路径>", "task", "list"])}\n明确领取后再执行，并通过 task progress / task result 回报。`,
    "任务等待期间维持心跳；需要权限时停下请求用户确认。不自动扩大权限，不迁移其他会话的上下文。",
  ].filter(Boolean).join("\n\n");
}
