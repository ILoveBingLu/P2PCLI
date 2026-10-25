import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowLeft, ChevronRight, LoaderCircle, Plus, RefreshCw, SquareArrowOutUpRight, SquareTerminal, Trash2 } from "lucide-react";
import { api, pathLabel, platformLabel, type PathInfo, type RemoteInfo, type TerminalSession } from "../api";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { AlertDialog, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { toast } from "@/components/ui/toast";
import { PlatformIcon } from "./PlatformIcon";
import { useContextMenu } from "./ContextMenu";
import { relativeTime, sessionLabels } from "./sessionText";

export interface SessionTarget { connId: number; info: RemoteInfo; code: string | null }

export function RemoteSessions({ target, openSessions, sessionsRev, onOpen, onBack, onEnded }: {
  target: SessionTarget;
  /** 别处（比如侧边栏）结束了会话时变，变了就在后台重读 */
  sessionsRev: number;
  /** 本机标签里正开着的会话 */
  openSessions: Set<string>;
  onOpen: (id?: string) => void;
  onBack: () => void;
  onEnded: (id: string) => void;
}) {
  // null 表示第一次还没读到；之后的刷新都在后台进行，列表不闪
  const [sessions, setSessions] = useState<TerminalSession[] | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [ending, setEnding] = useState<TerminalSession | null>(null);
  const [path, setPath] = useState<PathInfo | null>(null);
  const pending = useRef(false);
  const menu = useContextMenu();

  const load = useCallback(async (manual = false) => {
    if (pending.current) return;
    pending.current = true;
    if (manual) setRefreshing(true);
    try {
      setSessions(await api.listTerminals(target.connId));
      setError(null);
    } catch (e) {
      setError(String(e));
      setSessions((list) => list ?? []);
    } finally {
      pending.current = false;
      setRefreshing(false);
    }
  }, [target.connId]);

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(), 5000);
    return () => window.clearInterval(timer);
  }, [load, sessionsRev]);

  useEffect(() => {
    const tick = () => api.pathInfo(target.connId).then(setPath).catch(() => {});
    tick();
    const timer = window.setInterval(tick, 3000);
    return () => window.clearInterval(timer);
  }, [target.connId]);

  const end = async () => {
    const session = ending;
    if (!session) return;
    setEnding(null);
    // 先从列表里拿掉，不等对方回话；失败了后台刷新会把它放回来
    setSessions((list) => list?.filter((s) => s.id !== session.id) ?? list);
    try {
      await api.endTerminal(target.connId, session.id);
      onEnded(session.id);
    } catch (e) {
      toast.error(`结束会话失败：${String(e)}`);
    }
    void load();
  };

  const labels = sessionLabels(sessions ?? []);
  const ordered = [...(sessions ?? [])].sort((a, b) => (labels.get(a.id) ?? "").localeCompare(labels.get(b.id) ?? "", "zh-CN", { numeric: true }));
  const { info } = target;

  return (
    <section
      className="settings-page"
      aria-labelledby="remote-sessions-title"
      // 空白处右键：新建、刷新；会话行自己的菜单先接到，不会冒泡到这里
      onContextMenu={(e) => menu.open(e, [
        { id: "new", label: "新建会话", icon: <Plus className="size-4" />, disabled: sessions === null, onAction: () => onOpen() },
        { id: "refresh", label: "刷新会话", icon: <RefreshCw className="size-4" />, onAction: () => void load(true) },
      ])}
    >
      {menu.element}
      <div className="settings-page-inner">
        <header className="remote-device">
          <Button variant="ghost" size="icon-sm" title="返回工作区" aria-label="返回工作区" onClick={onBack}><ArrowLeft /></Button>
          <div className="remote-device-icon">
            <PlatformIcon platform={info.platform} decorative />
            <span className="connected-dot" aria-hidden="true" />
          </div>
          <div className="min-w-0 flex-1">
            <h1 id="remote-sessions-title" className="truncate text-base font-semibold">{info.name}</h1>
            <p className="remote-device-meta">
              <span className="status-dot" data-kind={path?.kind ?? "unknown"} />
              <span>已连接 · {pathLabel(path)}</span>
              <span className="text-muted-foreground">{platformLabel(info.platform)} · 用户 {info.user}</span>
            </p>
          </div>
          <Button onClick={() => onOpen()} disabled={sessions === null}><Plus data-icon="inline-start" />新建会话</Button>
        </header>

        <div className="flex flex-col gap-3">
          <div className="section-title mb-0">
            <h2>
              会话
              {sessions && sessions.length > 0 && <Badge variant="secondary">{sessions.length}</Badge>}
            </h2>
            <Button variant="ghost" size="icon-sm" title="刷新会话" aria-label="刷新会话" disabled={refreshing} onClick={() => void load(true)}>
              <RefreshCw className={refreshing ? "animate-spin" : undefined} />
            </Button>
          </div>
          <p className="-mt-1 text-xs text-muted-foreground">关闭标签或断线后，会话照样在这台设备上运行。点一下打开，右键可以结束。</p>

          {error && <Alert variant="destructive"><AlertDescription>{error}</AlertDescription></Alert>}

          {sessions === null ? (
            <div className="loading-state" role="status"><LoaderCircle className="animate-spin" />正在读取会话…</div>
          ) : ordered.length === 0 ? (
            !error && (
              <Empty className="session-empty">
                <EmptyHeader>
                  <EmptyMedia variant="icon"><SquareTerminal /></EmptyMedia>
                  <EmptyTitle>这台设备上还没有会话</EmptyTitle>
                  <EmptyDescription>新建一个会话，关掉标签或断线后它会继续在后台运行。</EmptyDescription>
                </EmptyHeader>
                <Button onClick={() => onOpen()}><Plus data-icon="inline-start" />新建会话</Button>
              </Empty>
            )
          ) : (
            <ul className="session-list">
              {ordered.map((s) => {
                const open = openSessions.has(s.id);
                const label = labels.get(s.id) ?? "会话";
                return (
                  <li key={s.id}>
                    <button
                      type="button"
                      className="session-row"
                      title={open ? "切换到这个会话的标签" : "打开这个会话"}
                      onClick={() => onOpen(s.id)}
                      onContextMenu={(e) => menu.open(e, [
                        { id: "open", label: open ? "切换过去" : "打开", icon: <SquareArrowOutUpRight className="size-4" />, onAction: () => onOpen(s.id) },
                        { id: "end", label: "结束会话", icon: <Trash2 className="size-4" />, danger: true, separated: true, onAction: () => setEnding(s) },
                      ])}
                    >
                      <span className="session-row-icon"><SquareTerminal /></span>
                      <span className="block min-w-0 flex-1">
                        <span className="flex min-w-0 items-center gap-2">
                          <span className="truncate text-sm font-medium">{label}</span>
                          {open
                            ? <Badge variant="success">已在标签中打开</Badge>
                            : s.attached
                              ? <Badge variant="warning">其他地方正在用</Badge>
                              : <Badge variant="secondary">后台运行</Badge>}
                        </span>
                        <span className="mt-0.5 block truncate text-xs text-muted-foreground">
                          <span className="font-mono">{s.id.slice(0, 8)}</span>
                          {s.created_at ? ` · ${relativeTime(s.created_at)}创建` : ""}
                        </span>
                      </span>
                      <ChevronRight className="session-row-chevron" aria-hidden="true" />
                    </button>
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      </div>
      {ending && (
        <AlertDialog open>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>结束{labels.get(ending.id) ?? "这个会话"}？</AlertDialogTitle>
              <AlertDialogDescription>会话里正在跑的程序会被结束，没保存的内容可能丢失。只是想暂时离开的话，关掉终端标签就行。</AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <Button variant="outline" onClick={() => setEnding(null)}>取消</Button>
              <Button variant="destructive" onClick={end}>结束会话</Button>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      )}
    </section>
  );
}
