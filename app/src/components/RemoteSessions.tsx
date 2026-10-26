import { useEffect, useState } from "react";
import { ArrowLeft, ChevronRight, LoaderCircle, Pencil, Plus, RefreshCw, SquareArrowOutUpRight, SquareTerminal, Trash2, Unplug } from "lucide-react";
import { api, pathLabel, platformLabel, type PathInfo, type RemoteInfo, type TerminalSession, type TerminalSessionSnapshot } from "../api";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { AlertDialog, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { toast } from "@/components/ui/toast";
import { PlatformIcon } from "./PlatformIcon";
import { useContextMenu } from "./ContextMenu";
import { AgentIcon, AgentStateLabel } from "./Agent";
import { RemarkDialog } from "./RemarkDialog";
import { useRemarks } from "./Remarks";
import { relativeTime, sessionTitles } from "./sessionText";

export interface SessionTarget { connId: number; info: RemoteInfo; code: string | null }

export function RemoteSessions({ target, snapshot, openSessions, onRefresh, onChangeSessions, onOpen, onBack, onEnded }: {
  target: SessionTarget;
  /** 和侧边栏、底部栏同一份会话快照，名字和 AI 状态才对得上 */
  snapshot: TerminalSessionSnapshot | undefined;
  /** 本机标签里正开着的会话 */
  openSessions: Set<string>;
  /** 马上重读一次会话 */
  onRefresh: () => void;
  /** 先在本地改掉（比如结束的会话先拿掉），不等下一轮刷新 */
  onChangeSessions: (sessions: TerminalSession[]) => void;
  onOpen: (id?: string) => void;
  onBack: () => void;
  onEnded: (id: string) => void;
}) {
  const error = snapshot?.error ?? null;
  // null 表示第一次还没读到；第一次就读失败时显示空列表和原因
  const sessions = snapshot?.sessions ?? (error ? [] : null);
  const [refreshing, setRefreshing] = useState(false);
  const [ending, setEnding] = useState<TerminalSession | null>(null);
  const [path, setPath] = useState<PathInfo | null>(null);
  const remarks = useRemarks();
  const [remarkTarget, setRemarkTarget] = useState<{ id: string; label: string; remark: string } | null>(null);
  const menu = useContextMenu();

  // 新的一份快照到了，手动刷新就算完成
  useEffect(() => setRefreshing(false), [snapshot]);
  const refresh = () => {
    setRefreshing(true);
    onRefresh();
  };

  useEffect(() => {
    const tick = () => api.pathInfo(target.connId).then(setPath).catch(() => {});
    tick();
    const timer = window.setInterval(tick, 3000);
    return () => window.clearInterval(timer);
  }, [target.connId]);

  const end = async () => {
    const session = ending;
    if (!session || !sessions) return;
    setEnding(null);
    // 先从列表里拿掉，不等对方回话；失败了刷新会把它放回来
    onChangeSessions(sessions.filter((s) => s.id !== session.id));
    try {
      await api.endTerminal(target.connId, session.id);
      onEnded(session.id);
    } catch (e) {
      toast.error(`结束会话失败：${String(e)}`);
    }
    onRefresh();
  };

  // 名字、排序和侧边栏同一套：跑着 AI 的叫 AI 的名字，别的按创建先后叫会话 1、会话 2…
  const titles = sessionTitles(sessions ?? []);
  const remarkFor = (id: string) => remarks.session(target.info.endpoint_id, id);
  const ordered = [...(sessions ?? [])].sort((a, b) => (a.created_at ?? 0) - (b.created_at ?? 0) || a.id.localeCompare(b.id));
  const { info } = target;
  const hasSessions = ordered.length > 0;

  return (
    <section
      className="remote-sessions-page"
      aria-labelledby="remote-sessions-title"
      // 空白处右键：新建、刷新；会话行自己的菜单先接到，不会冒泡到这里
      onContextMenu={(e) => menu.open(e, [
        { id: "new", label: "新建会话", icon: <Plus className="size-4" />, disabled: sessions === null, onAction: () => onOpen() },
        { id: "refresh", label: "刷新会话", icon: <RefreshCw className="size-4" />, onAction: refresh },
      ])}
    >
      {menu.element}
      <RemarkDialog
        open={!!remarkTarget}
        title="会话备注"
        description={remarkTarget?.label ?? ""}
        initialValue={remarkTarget?.remark ?? ""}
        onOpenChange={(next) => !next && setRemarkTarget(null)}
        onSave={async (remark) => {
          if (!remarkTarget) return;
          await remarks.saveSession(target.info.endpoint_id, remarkTarget.id, remark);
          toast.success("会话备注已保存");
        }}
      />
        <header className="remote-device">
          <Button variant="ghost" size="icon-sm" title="返回工作区" aria-label="返回工作区" onClick={onBack}><ArrowLeft /></Button>
          <div className="remote-device-icon">
            <PlatformIcon platform={info.platform} decorative />
          </div>
          <div className="remote-device-summary">
            <h1 id="remote-sessions-title" className="truncate text-base font-semibold">{remarks.device(info.endpoint_id) || info.name}</h1>
            <p className="remote-device-meta">
              <span className="remote-connection-state"><span className="status-dot" data-kind={path?.kind ?? "unknown"} />已连接</span>
              <span>{pathLabel(path)}</span>
              <span className="remote-device-user">{platformLabel(info.platform)} · {info.user}</span>
            </p>
          </div>
          <Button variant="ghost" size="icon-sm" title="刷新会话" aria-label="刷新会话" disabled={refreshing || sessions === null} onClick={refresh}>
            <RefreshCw className={refreshing ? "animate-spin" : undefined} />
          </Button>
        </header>

        <div className={`remote-sessions-body${hasSessions ? " has-sessions" : ""}`}>
          {hasSessions && <div className="remote-sessions-heading">
            <h2>会话 <Badge variant="secondary">{sessions?.length}</Badge></h2>
            <Button size="sm" onClick={() => onOpen()}><Plus data-icon="inline-start" />新建会话</Button>
          </div>}

          {error && hasSessions && <Alert variant="destructive"><AlertDescription>{error}</AlertDescription></Alert>}

          {sessions === null ? (
            <div className="remote-sessions-loading loading-state" role="status"><LoaderCircle className="animate-spin" />正在读取会话…</div>
          ) : !hasSessions ? (
              <Empty className="session-empty">
                <EmptyHeader>
                  <EmptyMedia variant="icon" className="session-empty-icon">{error ? <Unplug /> : <SquareTerminal />}</EmptyMedia>
                  <EmptyTitle role="heading" aria-level={2}>{error ? "暂时无法读取会话" : "开始一个新会话"}</EmptyTitle>
                  <EmptyDescription>{error ? "未能获取这台设备的会话，请重试。" : "这台设备暂无会话，新建一个终端开始使用。"}</EmptyDescription>
                </EmptyHeader>
                {error ? <>
                  <p className="session-empty-error" role="alert">{error}</p>
                  <Button onClick={refresh} disabled={refreshing}><RefreshCw className={refreshing ? "animate-spin" : undefined} data-icon="inline-start" />重新加载</Button>
                </> : <>
                  <Button onClick={() => onOpen()}><Plus data-icon="inline-start" />新建会话</Button>
                  <p className="session-empty-note">关闭标签或断线后，会话仍在后台运行</p>
                </>}
              </Empty>
          ) : (
            <ul className="session-list">
              {ordered.map((s) => {
                const open = openSessions.has(s.id);
                const label = remarkFor(s.id).trim() || titles.get(s.id) || "会话";
                // 刷新失败时列表留着，但旧的 AI 状态不能当成现在的
                const agent = error ? null : s.agent ?? null;
                return (
                  <li key={s.id}>
                    <button
                      type="button"
                      className="session-row"
                      title={open ? "切换到这个会话的标签" : "打开这个会话"}
                      onClick={() => onOpen(s.id)}
                      onContextMenu={(e) => menu.open(e, [
                        { id: "open", label: open ? "切换过去" : "打开", icon: <SquareArrowOutUpRight className="size-4" />, onAction: () => onOpen(s.id) },
                        { id: "remark", label: remarkFor(s.id).trim() ? "修改备注" : "添加备注", icon: <Pencil className="size-4" />, onAction: () => setRemarkTarget({ id: s.id, label, remark: remarkFor(s.id) }) },
                        { id: "end", label: "结束会话", icon: <Trash2 className="size-4" />, danger: true, separated: true, onAction: () => setEnding(s) },
                      ])}
                    >
                      <span className="session-row-icon">{s.agent ? <AgentIcon kind={s.agent.kind} className="size-4" /> : <SquareTerminal />}</span>
                      <span className="block min-w-0 flex-1">
                        <span className="flex min-w-0 items-center gap-2">
                          <span className="truncate text-sm font-medium">{label}</span>
                          {agent && <AgentStateLabel agent={agent} className="shrink-0 text-xs" />}
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
      {ending && (
        <AlertDialog open>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>结束{remarkFor(ending.id) || titles.get(ending.id) || "这个会话"}？</AlertDialogTitle>
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
