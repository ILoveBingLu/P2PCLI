import type { ReactNode } from "react";
import { CircleAlert, Folder } from "lucide-react";
import { pathLabel, type PathInfo, type TerminalSessionSnapshot } from "../api";
import { Tooltip, TooltipTrigger } from "@/components/ui/tooltip";
import { AGENT_STATE_TEXT, AgentIcon, AgentQuotaLabel, AgentStateLabel } from "./Agent";

function Detail({ tip, children, className = "" }: { tip: string; children: ReactNode; className?: string }) {
  return <Tooltip content={tip}><TooltipTrigger className={`terminal-status-detail ${className}`}>{children}</TooltipTrigger></Tooltip>;
}

export function TerminalStatusbar({ sessionId, state, path, snapshot }: {
  sessionId?: string;
  state: "opening" | "open" | "exited" | "closed";
  path?: PathInfo;
  snapshot?: TerminalSessionSnapshot;
}) {
  const sessions = snapshot?.sessions ?? [];
  const session = sessionId ? sessions.find((entry) => entry.id === sessionId) : undefined;
  const error = snapshot?.error ??
    (sessionId && snapshot?.sessions && !session ? "会话已结束或暂时不可用" : null);
  const details = session?.details;
  // 断开或刷新失败时，旧的“干活中”不能继续冒充实时状态。
  const agent = state !== "closed" && !error ? session?.agent : null;

  return (
    <footer className="terminal-statusbar" aria-label="当前终端状态">
      {/* 认出是命令行 AI 就显示它和它的状态；Claude Code 的程序名是版本号，直接显示看不懂 */}
      {agent ? <Detail tip={`${agent.name} · ${AGENT_STATE_TEXT[agent.state]}`} className="terminal-status-agent">
        <AgentIcon kind={agent.kind} className="size-3.5 shrink-0" /><span>{agent.name}</span>
        <AgentStateLabel agent={agent} />
        {agent.quota && <AgentQuotaLabel quota={agent.quota} />}
      </Detail> : details?.command && <Detail tip="当前程序" className="terminal-status-command">
        <span className="truncate">{details.command}</span>
      </Detail>}
      <div className="terminal-status-path">
        {details?.current_path && <Detail tip={details.current_path} className="min-w-0">
          <Folder /><span className="truncate">{details.current_path}</span>
        </Detail>}
      </div>
      {error && state !== "closed" && <Detail tip={error}><CircleAlert aria-label="状态更新失败" /></Detail>}
      {state === "open" && path && <Detail tip={`连接方式：${pathLabel(path)}`}>
        <span>{pathLabel(path)}</span>
      </Detail>}
    </footer>
  );
}
