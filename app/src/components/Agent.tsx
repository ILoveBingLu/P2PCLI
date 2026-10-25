import { Bot } from "lucide-react";
import type { AgentInfo, AgentState } from "../api";
import { cn } from "@/lib/utils";

// 各家命令行 AI 的图标来自 Lobe Icons（MIT），见 assets/agents/LICENSE。
// 按文本导入、内嵌渲染：单色图标用的是 currentColor，当图片引用会变成黑色
const files = import.meta.glob<string>("../assets/agents/*.svg", { query: "?raw", import: "default", eager: true });
const ICONS: Record<string, string> = Object.fromEntries(
  Object.entries(files).map(([path, svg]) => [path.slice(path.lastIndexOf("/") + 1, -4), svg]),
);

export const AGENT_STATE_TEXT: Record<AgentState, string> = {
  working: "干活中",
  waiting: "等你确认",
  idle: "空闲",
};

/** AI 的图标；没有收录图标的用通用的机器人图标 */
export function AgentIcon({ kind, className }: { kind: string; className?: string }) {
  const svg = ICONS[kind];
  if (!svg) return <Bot className={cn("agent-icon", className)} aria-hidden="true" />;
  return <span className={cn("agent-icon", className)} aria-hidden="true" dangerouslySetInnerHTML={{ __html: svg }} />;
}

/** 状态点加文字：干活中蓝色带呼吸，等你确认黄色，空闲灰色 */
export function AgentStateLabel({ agent, className }: { agent: AgentInfo; className?: string }) {
  return (
    <span className={cn("agent-state", className)} data-state={agent.state}>
      <span className="agent-state-dot" aria-hidden="true" />
      {AGENT_STATE_TEXT[agent.state]}
    </span>
  );
}

/** 终端标签上的设备名：前面是会话里 AI 的图标，AI 在干活或等你时名字后面带个小点 */
export function AgentTabTitle({ name, agent }: { name: string; agent: AgentInfo | null }) {
  return (
    <>
      {agent && <AgentIcon kind={agent.kind} className="size-3.5 shrink-0" />}
      <span className="truncate" title={agent ? `${agent.name} · ${AGENT_STATE_TEXT[agent.state]}` : undefined}>{name}</span>
      {agent && agent.state !== "idle" && (
        <span className="agent-tab-state" data-state={agent.state} aria-label={AGENT_STATE_TEXT[agent.state]} />
      )}
    </>
  );
}
