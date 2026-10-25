import { useEffect, useRef, useState } from "react";
import { LoaderCircle, RefreshCw } from "lucide-react";
import { api, type InstalledAgent } from "../api";
import { AgentIcon } from "./Agent";
import { Button } from "./ui/button";
import { toast } from "./ui/toast";
import { useContextMenu } from "./ContextMenu";

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

export function AgentLauncher({ connId, sessionId, visible, canLaunch, onLaunched, onFocus }: {
  connId: number; sessionId: string; visible: boolean;
  canLaunch: () => boolean; onLaunched: () => void; onFocus: () => void;
}) {
  const [agents, setAgents] = useState<InstalledAgent[]>([]);
  const [scanning, setScanning] = useState(true);
  const [error, setError] = useState(false);
  const [launching, setLaunching] = useState<string | null>(null);
  const alive = useRef(true);
  const lock = useRef(false);
  const current = useRef({ visible, canLaunch, onFocus, onLaunched });
  current.current = { visible, canLaunch, onFocus, onLaunched };
  const menu = useContextMenu();
  useEffect(() => {
    alive.current = true;
    void scan(connId, false).then((items) => { if (alive.current) { setAgents(items); setError(false); } })
      .catch(() => { if (alive.current) setError(true); })
      .finally(() => { if (alive.current) setScanning(false); });
    return () => { alive.current = false; };
  }, [connId]);
  useEffect(() => { if (!visible) menu.close(); }, [visible, menu.close]);

  const refresh = async () => {
    if (scanning) return;
    setScanning(true);
    try { const items = await scan(connId, true); if (alive.current) { setAgents(items); setError(false); } }
    catch (e) { if (alive.current) { setError(true); toast.error(`检测 AI 失败：${String(e)}`); } }
    finally { if (alive.current) setScanning(false); }
  };
  const launch = async (agent: InstalledAgent, allowBypass = false) => {
    if (lock.current || !current.current.visible || !current.current.canLaunch()) return;
    lock.current = true;
    setLaunching(agent.kind);
    try {
      await api.launchAgent(connId, sessionId, agent.kind, allowBypass);
      if (alive.current) { current.current.onLaunched(); current.current.onFocus(); }
    } catch (e) { if (alive.current) toast.error(`启动失败：${String(e)}`); }
    finally { lock.current = false; if (alive.current) setLaunching(null); }
  };
  if (!visible) return null;
  return <>
    <div className="agent-launcher" aria-label="启动已安装的 AI">
      {agents.map((agent) => (
        <Button key={agent.kind} variant="secondary" size="sm" className="agent-launcher-pill"
          aria-label={`启动 ${agent.name}`} isDisabled={launching !== null}
          onPress={() => void launch(agent)}
          onContextMenu={agent.kind === "claude" ? (e) => menu.open(e, [
            { id: "normal", label: "正常启动", onAction: () => void launch(agent) },
            { id: "allow-bypass", label: "启动并允许之后切换全部放行", onAction: () => void launch(agent, true) },
          ]) : (e) => e.preventDefault()}>
          {launching === agent.kind ? <LoaderCircle className="size-4 animate-spin" /> : <AgentIcon kind={agent.kind} className="size-4" />}
          {agent.name}
        </Button>
      ))}
      <Button variant="ghost" size="icon-sm" aria-label="刷新已安装的 AI" isDisabled={scanning || launching !== null}
        title={scanning ? "正在检测这台设备的 AI" : error ? "检测失败，点击重试（双方需支持启动器）" : agents.length ? "刷新已安装的 AI" : "未检测到 AI，点击重新检测"}
        onPress={() => void refresh()}>
        <RefreshCw className={scanning ? "size-4 animate-spin" : "size-4"} />
      </Button>
    </div>
    {menu.element}
  </>;
}
