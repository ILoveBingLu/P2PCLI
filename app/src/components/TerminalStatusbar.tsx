import { useEffect, useState, type ReactNode } from "react";
import { CircleAlert, Folder, Layers, SquareTerminal } from "lucide-react";
import { api, pathLabel, type PathInfo, type RemoteInfo, type TerminalSession } from "../api";
import { Tooltip, TooltipTrigger } from "@/components/ui/tooltip";
import { AGENT_STATE_TEXT, AgentIcon, AgentStateLabel } from "./Agent";
import { sessionLabels } from "./sessionText";

function Detail({ tip, children, className = "" }: { tip: string; children: ReactNode; className?: string }) {
  return <Tooltip content={tip}><TooltipTrigger className={`terminal-status-detail ${className}`}>{children}</TooltipTrigger></Tooltip>;
}

export function TerminalStatusbar({ connId, sessionId, info, state, path, visible }: {
  connId: number;
  sessionId?: string;
  info: RemoteInfo;
  state: "opening" | "open" | "exited" | "closed";
  path?: PathInfo;
  visible: boolean;
}) {
  const [snapshot, setSnapshot] = useState<{ session: TerminalSession; label: string } | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!visible || !sessionId || state === "opening" || state === "closed") return;
    let disposed = false;
    let timer: number | undefined;
    const refresh = async () => {
      try {
        const sessions = await api.listTerminals(connId);
        if (disposed) return;
        const session = sessions.find((entry) => entry.id === sessionId);
        setSnapshot(session ? { session, label: sessionLabels(sessions).get(sessionId) ?? "会话" } : null);
        setError(session ? null : "会话已结束或暂时不可用");
      } catch {
        if (!disposed) setError("状态暂时无法更新，显示上次读取的信息");
      } finally {
        // 完成后再调度，慢请求不叠加；切换页面或断开后停止。
        if (!disposed) timer = window.setTimeout(() => void refresh(), 3000);
      }
    };
    void refresh();
    return () => { disposed = true; window.clearTimeout(timer); };
  }, [connId, sessionId, state, visible]);

  const details = snapshot?.session.details;
  const agent = snapshot?.session.agent;
  const stateText = { opening: "打开中", open: "已连接", exited: "已脱离", closed: "已断开" }[state];
  const windowText = details?.window_name
    ? `${details.window_index == null ? "" : `${details.window_index}: `}${details.window_name}`
    : details?.window_index != null ? `窗口 ${details.window_index}` : null;
  const created = snapshot?.session.created_at
    ? new Date(snapshot.session.created_at * 1000).toLocaleString("zh-CN") : null;

  return (
    <footer className="terminal-statusbar" aria-label="当前终端状态">
      <Detail tip={`${info.name} · ${info.user}`}>
        <span className="status-dot" data-kind={state === "open" ? path?.kind ?? "unknown" : "off"} />
        <span>{stateText}</span>
      </Detail>
      <Detail tip={created ? `创建于 ${created}` : "后台会话"}>
        <SquareTerminal /><span>{snapshot?.label ?? "会话"}</span>
      </Detail>
      {windowText && <Detail tip={details?.window_count ? `当前窗口 · 共 ${details.window_count} 个窗口` : "当前窗口"} className="terminal-status-window">
        <Layers /><span className="truncate">{windowText}</span>
      </Detail>}
      {/* 认出是命令行 AI 就显示它和它的状态；Claude Code 的程序名是版本号，直接显示看不懂 */}
      {agent ? <Detail tip={`${agent.name} · ${AGENT_STATE_TEXT[agent.state]}`} className="terminal-status-command">
        <AgentIcon kind={agent.kind} /><span className="truncate">{agent.name}</span>
        <AgentStateLabel agent={agent} />
      </Detail> : details?.command && details.command !== details.window_name && <Detail tip="当前程序" className="terminal-status-command">
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
