import type { ReactNode } from "react";
import { CircleAlert, Folder, Monitor } from "lucide-react";
import type { PathInfo, RemoteInfo, TerminalSessionSnapshot } from "../api";
import { Tooltip, TooltipTrigger } from "@/components/ui/tooltip";
import { AgentIcon, AgentQuotaLabel, AgentStateLabel, agentStateTip } from "./Agent";
import { ConnectionCard } from "./ConnectionCard";
import { DirPicker } from "./DirPicker";
import { t } from "@/i18n";

function Detail({ tip, children, className = "" }: { tip: string; children: ReactNode; className?: string }) {
  return <Tooltip content={tip}><TooltipTrigger className={`terminal-status-detail ${className}`}>{children}</TooltipTrigger></Tooltip>;
}

export function TerminalStatusbar({ connId, info, sessionId, state, path, snapshot, echoLatency }: {
  connId: number;
  info: RemoteInfo;
  sessionId?: string;
  state: "opening" | "open" | "exited" | "closed";
  path?: PathInfo;
  snapshot?: TerminalSessionSnapshot;
  /** 这个终端最近的按键回显时间，连接详情里显示 */
  echoLatency: () => number | null;
}) {
  const sessions = snapshot?.sessions ?? [];
  const session = sessionId ? sessions.find((entry) => entry.id === sessionId) : undefined;
  const error = snapshot?.error ??
    (sessionId && snapshot?.sessions && !session ? t("会话已结束或暂时不可用") : null);
  const details = session?.details;
  // 断开或刷新失败时，旧的“干活中”不能继续冒充实时状态。
  const agent = state !== "closed" && !error ? session?.agent : null;

  return (
    <footer className="terminal-statusbar" aria-label={t("当前终端状态")}>
      {/* 认出是命令行 AI 就显示它和它的状态；Claude Code 的程序名是版本号，直接显示看不懂 */}
      {agent ? <Detail tip={agentStateTip(agent)} className="terminal-status-agent">
        <AgentIcon kind={agent.kind} className="size-3.5 shrink-0" /><span>{agent.name}</span>
        <AgentStateLabel agent={agent} />
        {agent.quota && <AgentQuotaLabel quota={agent.quota} />}
      </Detail> : details?.command && <Detail tip={t("当前程序")} className="terminal-status-command">
        <span className="truncate">{details.command}</span>
      </Detail>}
      <div className="terminal-status-path">
        {/* 后台会话能点开选目录；临时终端没有会话编号，只显示 */}
        {sessionId && state === "open" && !error
          ? <DirPicker
            connId={connId}
            sessionId={sessionId}
            currentPath={details?.current_path ?? null}
            canChange={!agent && details?.shell_ready === true}
            blockedReason={agent ? t("{{name}} 正在运行，退出后才能切换目录", { name: agent.name }) : t("终端不在空闲提示符，完成当前输入或退出程序后才能切换")}
          />
          : details?.current_path && <Detail tip={details.current_path} className="min-w-0">
            <Folder /><span className="truncate">{details.current_path}</span>
          </Detail>}
      </div>
      {error && state !== "closed" && <Detail tip={error}><CircleAlert aria-label={t("状态更新失败")} /></Detail>}
      {state === "open" && path && (path.kind === "local"
        ? <span className="terminal-status-local" title={t("本机自己的终端，不走网络")}><Monitor aria-hidden="true" />{t("本机")}</span>
        : <ConnectionCard connId={connId} info={info} path={path} echoLatency={echoLatency} />)}
    </footer>
  );
}
