import { Bot } from "lucide-react";
import Amp from "@lobehub/icons/es/Amp/components/Color.js";
import ClaudeCode from "@lobehub/icons/es/ClaudeCode/components/Color.js";
import CodeBuddy from "@lobehub/icons/es/CodeBuddy/components/Color.js";
import Codex from "@lobehub/icons/es/Codex/components/Color.js";
import Cursor from "@lobehub/icons/es/Cursor/components/Mono.js";
import GeminiCLI from "@lobehub/icons/es/GeminiCLI/components/Color.js";
import GithubCopilot from "@lobehub/icons/es/GithubCopilot/components/Mono.js";
import Goose from "@lobehub/icons/es/Goose/components/Mono.js";
import Kimi from "@lobehub/icons/es/Kimi/components/Color.js";
import Kiro from "@lobehub/icons/es/Kiro/components/Color.js";
import OpenCode from "@lobehub/icons/es/OpenCode/components/Mono.js";
import Qoder from "@lobehub/icons/es/Qoder/components/Color.js";
import Qwen from "@lobehub/icons/es/Qwen/components/Color.js";
import Trae from "@lobehub/icons/es/Trae/components/Color.js";
import type { AgentInfo, AgentState } from "../api";
import { cn } from "@/lib/utils";

// 直接导入所需的 React 图标，避免加载各品牌的 Avatar、Combine 等组件。
// 新增 CLI 时，在这里按后端 agents.rs 的 kind 添加映射；未收录的保留通用图标。
const ICONS: Readonly<Partial<Record<string, typeof ClaudeCode>>> = {
  amp: Amp,
  claude: ClaudeCode,
  codebuddy: CodeBuddy,
  codex: Codex,
  copilot: GithubCopilot,
  cursor: Cursor,
  gemini: GeminiCLI,
  goose: Goose,
  kimi: Kimi,
  kiro: Kiro,
  opencode: OpenCode,
  qoder: Qoder,
  qwen: Qwen,
  trae: Trae,
};

export const AGENT_STATE_TEXT: Record<AgentState, string> = {
  working: "干活中",
  waiting: "等你确认",
  idle: "空闲",
};

/** AI 的图标；没有收录图标的用通用的机器人图标 */
export function AgentIcon({ kind, className }: { kind: string; className?: string }) {
  const Icon = Object.hasOwn(ICONS, kind) ? ICONS[kind] : undefined;
  if (!Icon) return <Bot className={cn("agent-icon", className)} aria-hidden="true" />;
  return <span className={cn("agent-icon", className)} aria-hidden="true"><Icon /></span>;
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
