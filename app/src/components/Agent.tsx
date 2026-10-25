import { Bot } from "lucide-react";
import type { AgentInfo, AgentQuota, AgentState } from "../api";
import { cn } from "@/lib/utils";

// 各家命令行 AI 的图标取自 Lobe Icons（MIT）的静态 SVG，放在 assets/agents，见那里的 LICENSE。
// 按图片显示，不内嵌、不引 @lobehub/icons：那个包会带进整套 Lobe UI，而且新版本常常刚发布、过不了 pnpm 的发布时间检查。
// 新增 CLI 时，按后端 agents.rs 的 kind 放一个同名 SVG；没收录的显示通用图标
const files = import.meta.glob<string>("../assets/agents/*.svg", { query: "?url", import: "default", eager: true });
const ICONS: Readonly<Record<string, string>> = Object.fromEntries(
  Object.entries(files).map(([path, url]) => [path.slice(path.lastIndexOf("/") + 1, -4), url]),
);

export const AGENT_STATE_TEXT: Record<AgentState, string> = {
  working: "干活中",
  waiting: "等你确认",
  idle: "空闲",
};

/** AI 的图标；没有收录图标的用通用的机器人图标 */
export function AgentIcon({ kind, className }: { kind: string; className?: string }) {
  const url = Object.hasOwn(ICONS, kind) ? ICONS[kind] : undefined;
  if (!url) return <Bot className={cn("agent-icon", className)} aria-hidden="true" />;
  return <img className={cn("agent-icon", className)} src={url} alt="" aria-hidden="true" draggable={false} />;
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

/** 额度用到这么多变黄、变红 */
const QUOTA_WARN = 75;
export const QUOTA_ALERT = 90;

/** 300 分钟叫“5 小时”，10080 叫“7 天” */
export function quotaWindowLabel(minutes: number) {
  if (minutes <= 0) return "额度";
  if (minutes % 1440 === 0) return `${minutes / 1440} 天`;
  if (minutes % 60 === 0) return `${minutes / 60} 小时`;
  return `${minutes} 分钟`;
}

/** 今天 18:30、明天 07:00、10月27日 07:00 */
export function resetText(secs: number | null | undefined) {
  if (!secs) return "重置时间不详";
  const at = new Date(secs * 1000);
  const hm = at.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" });
  const today = new Date();
  const tomorrow = new Date(Date.now() + 86400000);
  if (at.toDateString() === today.toDateString()) return `今天 ${hm} 重置`;
  if (at.toDateString() === tomorrow.toDateString()) return `明天 ${hm} 重置`;
  return `${at.getMonth() + 1}月${at.getDate()}日 ${hm} 重置`;
}

/** 额度：“5 小时 35% · 7 天 75%”，用到 75% 变黄、90% 变红，悬停看重置时间 */
export function AgentQuotaLabel({ quota }: { quota: AgentQuota }) {
  const ago = Math.max(0, Math.round((Date.now() / 1000 - quota.at) / 60));
  const tip = [
    ...quota.windows.map((w) => `${quotaWindowLabel(w.minutes)}额度已用 ${Math.round(w.used_percent)}%，${resetText(w.resets_at)}`),
    ago < 1 ? "刚刚读到的" : `${ago} 分钟前的数据`,
  ].join("\n");
  return (
    <span className="agent-quota" title={tip}>
      {quota.windows.map((w) => (
        <span key={w.minutes} data-level={w.used_percent >= QUOTA_ALERT ? "high" : w.used_percent >= QUOTA_WARN ? "warn" : undefined}>
          {quotaWindowLabel(w.minutes)} {Math.round(w.used_percent)}%
        </span>
      ))}
    </span>
  );
}
