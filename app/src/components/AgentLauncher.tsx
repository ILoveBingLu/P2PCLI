import { useEffect, useRef, useState } from "react";
import { Skeleton } from "@heroui/react";
import { LoaderCircle, RefreshCw } from "lucide-react";
import { api, type InstalledAgent } from "../api";
import { AgentIcon } from "./Agent";
import { Button } from "./ui/button";
import { toast } from "./ui/toast";
import { useContextMenu } from "./ContextMenu";
import { t } from "@/i18n";

// 同一条连接的多个终端共享检测请求，被控端还有五分钟缓存。
const scans = new Map<number, Promise<InstalledAgent[]>>();
function scan(connId: number, refresh: boolean) {
  if (refresh || !scans.has(connId)) {
    const pending = api.installedAgents(connId, refresh);
    scans.set(connId, pending);
    void pending.catch(() => { if (scans.get(connId) === pending) scans.delete(connId); });
  }
  return scans.get(connId)!;
}

export function AgentLauncher({ connId, sessionId, active, visible, canLaunch, onLaunched, onFocus }: {
  connId: number; sessionId: string; active: boolean; visible: boolean;
  canLaunch: () => boolean; onLaunched: () => void; onFocus: () => void;
}) {
  const [agents, setAgents] = useState<InstalledAgent[]>([]);
  const [scanning, setScanning] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [launching, setLaunching] = useState<string | null>(null);
  const alive = useRef(true);
  const lock = useRef(false);
  const current = useRef({ visible, canLaunch, onFocus, onLaunched });
  current.current = { visible, canLaunch, onFocus, onLaunched };
  const menu = useContextMenu();
  useEffect(() => {
    alive.current = true;
    void scan(connId, false).then((items) => { if (alive.current) { setAgents(items); setError(null); } })
      .catch((e) => { if (alive.current) setError(String(e)); })
      .finally(() => { if (alive.current) setScanning(false); });
    return () => { alive.current = false; };
  }, [connId]);
  useEffect(() => { if (!visible) menu.close(); }, [visible, menu.close]);

  const refresh = async () => {
    if (scanning) return;
    menu.close();
    setScanning(true);
    try { const items = await scan(connId, true); if (alive.current) { setAgents(items); setError(null); } }
    catch (e) { if (alive.current) { setError(String(e)); toast.error(t("检测 AI 失败：{{e}}", { e: String(e) })); } }
    finally { if (alive.current) setScanning(false); }
  };
  const launch = async (agent: InstalledAgent, allowBypass = false) => {
    if (scanning || lock.current || !current.current.visible || !current.current.canLaunch()) return;
    lock.current = true;
    setLaunching(agent.kind);
    try {
      await api.launchAgent(connId, sessionId, agent.kind, allowBypass);
      if (alive.current) { current.current.onLaunched(); current.current.onFocus(); }
    } catch (e) { if (alive.current) toast.error(t("启动失败：{{e}}", { e: String(e) })); }
    finally { lock.current = false; if (alive.current) setLaunching(null); }
  };
  if (!active || (!visible && !scanning && !error)) return null;
  return <>
    <div className="agent-launcher" aria-label={t("启动已安装的 AI")} aria-busy={scanning}>
      {scanning ? <>
        <div className="agent-launcher-loading" role="status">
          <LoaderCircle className="size-3.5 animate-spin" aria-hidden="true" />
          <span>{t("正在检测 AI")}</span>
        </div>
        <div className="agent-launcher-skeletons" aria-hidden="true">
          <Skeleton animationType="shimmer" className="agent-launcher-skeleton" />
          <Skeleton animationType="shimmer" className="agent-launcher-skeleton agent-launcher-skeleton-short" />
        </div>
      </> : <>
        {error && <Button variant="secondary" size="sm" className="agent-launcher-pill"
          title={t("{{error}}。请确认双方均已更新到支持 AI 检测的版本。", { error })}
          aria-label={t("AI 检测失败，重试")} onPress={() => void refresh()}>
          <RefreshCw className="size-3.5" aria-hidden="true" />
          {t("检测失败 · 重试")}
        </Button>}
        {!error && agents.length === 0 && <div className="agent-launcher-loading" role="status">{t("未检测到 AI")}</div>}
        {visible && agents.map((agent) => (
        <Button key={agent.kind} variant="secondary" size="sm" className="agent-launcher-pill"
          aria-label={t("启动 {{name}}", { name: agent.name })} isDisabled={launching !== null}
          onPress={() => void launch(agent)}
          onContextMenu={agent.kind === "claude" ? (e) => menu.open(e, [
            { id: "normal", label: t("正常启动"), onAction: () => void launch(agent) },
            { id: "allow-bypass", label: t("启动并允许之后切换全部放行"), onAction: () => void launch(agent, true) },
          ]) : (e) => e.preventDefault()}>
          {launching === agent.kind ? <LoaderCircle className="size-4 animate-spin" /> : <AgentIcon kind={agent.kind} className="size-4" />}
          {agent.name}
        </Button>
      ))}</>}
      {!scanning && !error && <Button variant="ghost" size="icon-sm" aria-label={t("刷新已安装的 AI")} isDisabled={launching !== null}
        title={agents.length ? t("刷新已安装的 AI") : t("未检测到 AI，点击重新检测")}
        onPress={() => void refresh()}>
        <RefreshCw className="size-4" />
      </Button>}
    </div>
    {menu.element}
  </>;
}
