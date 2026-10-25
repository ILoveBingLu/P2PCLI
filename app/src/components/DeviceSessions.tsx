import { useEffect, useRef, useState, type MouseEvent } from "react";
import { LoaderCircle, SquareArrowOutUpRight, Terminal, Trash2 } from "lucide-react";
import { api, type DeviceStatus, type RemoteInfo, type TerminalSession } from "../api";
import { Button } from "@/components/ui/button";
import { AlertDialog, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog";
import { toast } from "@/components/ui/toast";
import type { MenuAction } from "./ContextMenu";
import type { SessionTarget } from "./RemoteSessions";
import { AGENT_STATE_TEXT, AgentIcon, AgentStateLabel } from "./Agent";
import { relativeTime, sessionLabels, sessionTitles } from "./sessionText";

export interface SidebarTab {
  key: number;
  connId: number;
  info: RemoteInfo;
  code: string | null;
  sessionId?: string;
  state: "opening" | "open" | "exited" | "closed";
}

export function DeviceSessions({ target, status, tabs, sessionsRev, activeKey, busy, onConnectOpen, onOpen, onSelect, onEnded, openMenu }: {
  target?: SessionTarget;
  /** 后台探测的结果：没连上时拿它显示会话列表 */
  status?: DeviceStatus;
  tabs: SidebarTab[];
  /** 会话增减时变，变了就在后台重读 */
  sessionsRev: number;
  activeKey: number | null;
  busy: boolean;
  /** 没连上时点会话：先连上，再打开这个会话；null 是新建 */
  onConnectOpen: (sessionId: string | null) => void;
  onOpen: (target: SessionTarget, sessionId?: string) => void;
  onSelect: (key: number) => void;
  onEnded: (sessionId: string) => void;
  openMenu: (e: MouseEvent, actions: MenuAction[]) => void;
}) {
  const [sessions, setSessions] = useState<TerminalSession[]>([]);
  const [loading, setLoading] = useState(!!target);
  const [error, setError] = useState<string | null>(null);
  const [ending, setEnding] = useState<{ id: string; label: string } | null>(null);
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
        if (alive) setLoading(false);
      }
    };
    void load();
    const timer = window.setInterval(() => void load(), 5000);
    return () => { alive = false; window.clearInterval(timer); };
  }, [connId, tabRevision, sessionsRev]);

  const listed = new Set(sessions.map((session) => session.id));
  const labels = sessionLabels(sessions);
  const titles = sessionTitles(sessions);
  const rows = [...sessions].sort((a, b) => (a.created_at ?? 0) - (b.created_at ?? 0) || a.id.localeCompare(b.id)).map((session) => ({
    id: session.id,
    sessionId: session.id,
    tab: tabs.find((tab) => tab.sessionId === session.id && (tab.state === "open" || tab.state === "opening")),
    label: titles.get(session.id) ?? "会话",
    number: labels.get(session.id) ?? "会话",
    status: session.attached ? "已连接" : "后台运行",
    agent: session.agent ?? null,
  }));
  for (const tab of tabs) {
    if (tab.sessionId && listed.has(tab.sessionId)) continue;
    const label = tab.sessionId ? `会话 ${tab.sessionId.slice(0, 8)}` : `终端 ${tab.key}`;
    rows.push({
      id: `tab-${tab.key}`, sessionId: tab.sessionId ?? "", tab, label, number: label,
      status: { opening: "打开中", open: "已连接", exited: "已脱离", closed: "已断开" }[tab.state],
      agent: null,
    });
  }

  const openRow = (row: (typeof rows)[number]) =>
    row.tab ? onSelect(row.tab.key) : target && onOpen(target, row.sessionId);

  const end = async () => {
    const session = ending;
    if (!session || connId == null) return;
    setEnding(null);
    // 先从列表里拿掉，失败了下次刷新会放回来
    setSessions((list) => list.filter((s) => s.id !== session.id));
    try {
      await api.endTerminal(connId, session.id);
      onEnded(session.id);
    } catch (e) {
      toast.error(`结束会话失败：${String(e)}`);
    }
  };

  // 会话行的右键菜单；设备本身的菜单在侧边栏那一层
  const onContextMenu = (e: MouseEvent) => {
    const id = (e.target as HTMLElement).closest<HTMLElement>("[data-session-row]")?.dataset.sessionRow;
    const row = rows.find((r) => r.id === id);
    if (!row) return;
    openMenu(e, [
      { id: "open", label: row.tab ? "切换过去" : "打开", icon: <SquareArrowOutUpRight className="size-4" />, disabled: !row.tab && !target, onAction: () => openRow(row) },
      {
        id: "end", label: "结束会话", icon: <Trash2 className="size-4" />, danger: true, separated: true,
        disabled: !row.sessionId || !target || !listed.has(row.sessionId),
        onAction: () => setEnding({ id: row.sessionId, label: row.label }),
      },
    ]);
  };

  if (!target) return (
    <OfflineSessions status={status} busy={busy} onConnectOpen={onConnectOpen} openMenu={openMenu} />
  );

  return (
    <div className="flex min-w-0 flex-col gap-1" onContextMenu={onContextMenu}>
      {rows.map((row) => (
        <Button key={row.id} variant={row.tab?.key === activeKey ? "secondary" : "ghost"}
          data-session-row={row.id}
          className="h-8 w-full min-w-0 justify-start gap-2 px-2 text-xs"
          aria-current={row.tab?.key === activeKey ? "true" : undefined}
          title={row.agent ? `${row.number} · ${row.agent.name} ${AGENT_STATE_TEXT[row.agent.state]} · ${row.status}` : undefined}
          onClick={() => openRow(row)}>
          {row.agent ? <AgentIcon kind={row.agent.kind} className="size-3.5 shrink-0" /> : <Terminal className="size-3.5 shrink-0" />}
          <span className="min-w-0 flex-1 truncate text-left">{row.label}</span>
          {row.agent
            ? <AgentStateLabel agent={row.agent} className="shrink-0 text-[10px]" />
            : <span className="shrink-0 text-[10px] text-muted-foreground">{row.status}</span>}
        </Button>
      ))}
      {loading && <p className="flex items-center gap-2 px-2 py-1 text-xs text-muted-foreground" role="status"><LoaderCircle className="size-3 animate-spin" />读取会话…</p>}
      {error && <p className="px-2 py-1 text-xs text-destructive break-words" role="alert">{error}</p>}
      {!loading && !error && target && rows.length === 0 && <p className="px-2 py-1 text-xs text-muted-foreground">暂无会话，右键设备可以新建</p>}
      {ending && (
        <AlertDialog open>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>结束{ending.label}？</AlertDialogTitle>
              <AlertDialogDescription>会话里正在跑的程序会被结束，没保存的内容可能丢失。只是想暂时离开的话，关掉终端标签就行。</AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <Button variant="outline" onClick={() => setEnding(null)}>取消</Button>
              <Button variant="destructive" onClick={end}>结束会话</Button>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      )}
    </div>
  );
}

/** 没连上时：显示后台探测拿到的、或者上次记住的会话，点一下就连上并打开 */
function OfflineSessions({ status, busy, onConnectOpen, openMenu }: {
  status?: DeviceStatus;
  busy: boolean;
  onConnectOpen: (sessionId: string | null) => void;
  openMenu: (e: MouseEvent, actions: MenuAction[]) => void;
}) {
  const sessions = [...(status?.sessions ?? [])].sort((a, b) => (a.created_at ?? 0) - (b.created_at ?? 0) || a.id.localeCompare(b.id));
  const titles = sessionTitles(sessions);
  const stale = !status?.sessions_live;
  const offline = status?.state === "offline";
  const hint = sessions.length
    ? stale && status?.sessions_at ? `上次看到 · ${relativeTime(status.sessions_at)}` : null
    : status?.paired === false || status?.state === "old_version"
      ? "连接一次以后，这里会显示会话"
      : offline ? "离线，没有记住的会话" : status?.state === "online" ? "暂无会话" : null;
  return (
    <div className="flex min-w-0 flex-col gap-1" data-stale={stale ? "" : undefined}
      onContextMenu={(e) => {
        const id = (e.target as HTMLElement).closest<HTMLElement>("[data-offline-session]")?.dataset.offlineSession;
        if (!id) return;
        openMenu(e, [
          { id: "open", label: "连接并打开", icon: <SquareArrowOutUpRight className="size-4" />, disabled: busy || offline, onAction: () => onConnectOpen(id) },
        ]);
      }}>
      {hint && <p className="px-2 pt-0.5 text-[10px] text-muted-foreground">{hint}</p>}
      {sessions.map((s) => (
        <Button key={s.id} variant="ghost" data-offline-session={s.id}
          className="offline-session h-8 w-full min-w-0 justify-start gap-2 px-2 text-xs"
          disabled={busy || offline}
          title={offline ? "设备离线" : "连接并打开这个会话"}
          onClick={() => onConnectOpen(s.id)}>
          {s.agent ? <AgentIcon kind={s.agent.kind} className="size-3.5 shrink-0" /> : <Terminal className="size-3.5 shrink-0" />}
          <span className="min-w-0 flex-1 truncate text-left">{titles.get(s.id) ?? "会话"}</span>
          {s.agent && !stale && !offline
            ? <AgentStateLabel agent={s.agent} className="shrink-0 text-[10px]" />
            : <span className="shrink-0 text-[10px] text-muted-foreground">{s.attached ? "在用" : "后台运行"}</span>}
        </Button>
      ))}
    </div>
  );
}
