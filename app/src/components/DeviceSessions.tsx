import { useEffect, useRef, useState } from "react";
import { ArrowUpRight, LoaderCircle, Plus, RefreshCw, Terminal } from "lucide-react";
import { api, type RemoteInfo, type TerminalSession } from "../api";
import { Button } from "@/components/ui/button";
import type { SessionTarget } from "./RemoteSessions";
import { sessionLabels } from "./sessionText";

export interface SidebarTab {
  key: number;
  connId: number;
  info: RemoteInfo;
  code: string | null;
  sessionId?: string;
  state: "opening" | "open" | "exited" | "closed";
}

export function DeviceSessions({ target, tabs, sessionsRev, activeKey, busy, onConnect, onOpen, onSelect }: {
  target?: SessionTarget;
  tabs: SidebarTab[];
  sessionsRev: number;
  activeKey: number | null;
  busy: boolean;
  onConnect: () => void;
  onOpen: (target: SessionTarget, sessionId?: string) => void;
  onSelect: (key: number) => void;
}) {
  const [sessions, setSessions] = useState<TerminalSession[]>([]);
  const [loading, setLoading] = useState(!!target);
  const [error, setError] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);
  const [refreshing, setRefreshing] = useState(false);
  // 只有第一次读的时候显示“读取会话…”，之后在后台刷新，列表不闪
  const loaded = useRef(false);
  const tabRevision = tabs.map((tab) => `${tab.key}:${tab.sessionId}:${tab.state}`).join("|");
  const connId = target?.connId;

  useEffect(() => {
    if (connId == null) return;
    let alive = true, pending = false;
    const load = async () => {
      if (pending) return;
      pending = true;
      if (!loaded.current) setLoading(true);
      try {
        const value = await api.listTerminals(connId);
        if (alive) { setSessions(value); setError(null); loaded.current = true; }
      } catch (e) {
        if (alive) setError(String(e));
      } finally {
        pending = false;
        if (alive) { setLoading(false); setRefreshing(false); }
      }
    };
    void load();
    const timer = window.setInterval(() => void load(), 5000);
    return () => { alive = false; window.clearInterval(timer); };
  }, [connId, tabRevision, revision, sessionsRev]);

  const listed = new Set(sessions.map((session) => session.id));
  const labels = sessionLabels(sessions);
  const rows = [...sessions].sort((a, b) => (a.created_at ?? 0) - (b.created_at ?? 0) || a.id.localeCompare(b.id)).map((session) => ({
    id: session.id,
    sessionId: session.id,
    tab: tabs.find((tab) => tab.sessionId === session.id && (tab.state === "open" || tab.state === "opening")),
    label: labels.get(session.id) ?? "会话",
    status: session.attached ? "已连接" : "后台运行",
  }));
  for (const tab of tabs) {
    if (tab.sessionId && listed.has(tab.sessionId)) continue;
    rows.push({
      id: `tab-${tab.key}`, sessionId: tab.sessionId ?? "", tab,
      label: tab.sessionId ? `会话 ${tab.sessionId.slice(0, 8)}` : `终端 ${tab.key}`,
      status: { opening: "打开中", open: "已连接", exited: "已脱离", closed: "已断开" }[tab.state],
    });
  }

  return (
    <div className="flex min-w-0 flex-col gap-1">
      {rows.map((row) => (
        <Button key={row.id} variant={row.tab?.key === activeKey ? "secondary" : "ghost"}
          className="h-8 w-full min-w-0 justify-start gap-2 px-2 text-xs"
          aria-current={row.tab?.key === activeKey ? "true" : undefined}
          onClick={() => row.tab ? onSelect(row.tab.key) : target && onOpen(target, row.sessionId)}>
          <Terminal className="size-3.5 shrink-0" />
          <span className="min-w-0 flex-1 truncate text-left">{row.label}</span>
          <span className="shrink-0 text-[10px] text-muted-foreground">{row.status}</span>
        </Button>
      ))}
      {loading && <p className="flex items-center gap-2 px-2 py-1 text-xs text-muted-foreground" role="status"><LoaderCircle className="size-3 animate-spin" />读取会话…</p>}
      {error && <p className="px-2 py-1 text-xs text-destructive break-words" role="alert">{error}</p>}
      {!loading && !error && target && rows.length === 0 && <p className="px-2 py-1 text-xs text-muted-foreground">暂无会话</p>}
      {target ? (
        <div className="flex items-center gap-1">
          <Button variant="ghost" className="h-8 min-w-0 flex-1 justify-start px-2 text-xs" disabled={loading || !!error} onClick={() => onOpen(target)}><Plus className="size-3.5" />新建会话</Button>
          <Button variant="ghost" size="icon-sm" title="刷新会话" aria-label="刷新会话" disabled={loading || refreshing} onClick={() => { setRefreshing(true); setRevision((value) => value + 1); }}><RefreshCw className={refreshing ? "size-3.5 animate-spin" : "size-3.5"} /></Button>
        </div>
      ) : (
        <Button variant="ghost" className="h-8 w-full justify-start px-2 text-xs" disabled={busy} onClick={onConnect}>
          {busy ? <LoaderCircle className="size-3.5 animate-spin" /> : <ArrowUpRight className="size-3.5" />}连接以查看会话
        </Button>
      )}
    </div>
  );
}
