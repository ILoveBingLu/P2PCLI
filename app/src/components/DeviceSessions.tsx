import { useState, type MouseEvent } from "react";
import { LoaderCircle, Pencil, SquareArrowOutUpRight, Terminal, Trash2 } from "lucide-react";
import { api, type DeviceStatus, type RemoteInfo, type TerminalSession, type TerminalSessionSnapshot } from "../api";
import { Button } from "@/components/ui/button";
import { toast } from "@/components/ui/toast";
import type { MenuAction } from "./ContextMenu";
import type { SessionTarget } from "./RemoteSessions";
import { AgentIcon, AgentStateLabel, agentStateText } from "./Agent";
import { ownerLabel, sessionTitles, shortSessionId } from "./sessionText";
import { RemarkDialog } from "./RemarkDialog";
import { useRemarks } from "./Remarks";
import { CollaborationAnchor } from "./CollaborationPorts";
import { ClearSessionDialog } from "./ClearSessionDialog";
import { t } from "@/i18n";

export interface SidebarTab {
  key: number;
  connId: number;
  info: RemoteInfo;
  code: string | null;
  sessionId?: string;
  state: "opening" | "open" | "exited" | "closed";
}

export function DeviceSessions({ endpointId, deviceName, target, status, tabs, snapshot, activeKey, busy, onChangeSessions, onConnectOpen, onConnectClear, onOpen, onSelect, onEnded, openMenu }: {
  endpointId: string;
  deviceName: string;
  target?: SessionTarget;
  /** 后台探测的结果：没连上时拿它显示会话列表 */
  status?: DeviceStatus;
  tabs: SidebarTab[];
  /** 与终端底部栏、标签共享的实时快照 */
  snapshot?: TerminalSessionSnapshot;
  activeKey: number | null;
  busy: boolean;
  /** 没连上时点会话：先连上，再打开这个会话；null 是新建 */
  onChangeSessions: (sessions: TerminalSession[]) => void;
  onConnectOpen: (sessionId: string | null) => void;
  onConnectClear: (sessionId: string) => void;
  onOpen: (target: SessionTarget, sessionId?: string) => void;
  onSelect: (key: number) => void;
  onEnded: (sessionId: string) => void;
  openMenu: (e: MouseEvent, actions: MenuAction[]) => void;
}) {
  const [ending, setEnding] = useState<{ id: string; label: string } | null>(null);
  const remarks = useRemarks();
  const [remarkTarget, setRemarkTarget] = useState<{ id: string; label: string; remark: string } | null>(null);
  const connId = target?.connId;
  const sessions = snapshot?.sessions ?? [];
  const loading = !!target && !snapshot;
  const error = snapshot?.error ?? null;
  // 刷新失败时保留会话基本信息，但旧 AI 状态不能再当作实时状态展示。
  const live = snapshot?.sessions != null && !error;
  const sessionRemark = (id: string) => remarks.session(endpointId, id);

  const listed = new Set(sessions.map((session) => session.id));
  const titles = sessionTitles(sessions);
  const rows = [...sessions].sort((a, b) => (a.created_at ?? 0) - (b.created_at ?? 0) || a.id.localeCompare(b.id)).map((session) => ({
    id: session.id,
    sessionId: session.id,
    tab: tabs.find((tab) => tab.sessionId === session.id && (tab.state === "open" || tab.state === "opening")),
    label: sessionRemark(session.id).trim() || titles.get(session.id) || t("会话"),
    remark: sessionRemark(session.id),
    status: session.attached ? t("已连接") : t("后台运行"),
    agent: live ? session.agent ?? null : null,
    // 别的设备开在本机上的会话，标上是谁开的
    from: ownerLabel(session, remarks.device),
  }));
  for (const tab of tabs) {
    if (tab.sessionId && listed.has(tab.sessionId)) continue;
    const label = tab.sessionId ? sessionRemark(tab.sessionId) || t("会话 {{value}}", { value: shortSessionId(tab.sessionId) }) : t("终端 {{key}}", { key: tab.key });
    rows.push({
      id: `tab-${tab.key}`, sessionId: tab.sessionId ?? "", tab, label,
      remark: tab.sessionId ? sessionRemark(tab.sessionId) : "",
      status: { opening: t("打开中"), open: t("已连接"), exited: t("已脱离"), closed: t("已断开") }[tab.state],
      agent: null,
      from: null,
    });
  }

  const openRow = (row: (typeof rows)[number]) =>
    row.tab ? onSelect(row.tab.key) : target && onOpen(target, row.sessionId);

  const end = async () => {
    const session = ending;
    if (!session) return;
    if (connId == null) {
      if (busy || status?.state === "offline") throw new Error(t("设备暂不可连接，请稍后重试"));
      onConnectClear(session.id);
      return;
    }
    await api.endTerminal(connId, session.id);
    onChangeSessions(sessions.filter((s) => s.id !== session.id));
    onEnded(session.id);
  };

  // 会话行的右键菜单；设备本身的菜单在侧边栏那一层
  const onContextMenu = (e: MouseEvent) => {
    const id = (e.target as HTMLElement).closest<HTMLElement>("[data-session-row]")?.dataset.sessionRow;
    const row = rows.find((r) => r.id === id);
    if (!row) return;
    openMenu(e, [
      { id: "open", label: row.tab ? t("切换过去") : t("打开"), icon: <SquareArrowOutUpRight className="size-4" />, disabled: !row.tab && !target, onAction: () => openRow(row) },
      { id: "remark", label: row.remark.trim() ? t("修改备注") : t("添加备注"), icon: <Pencil className="size-4" />, disabled: !row.sessionId, onAction: () => setRemarkTarget({ id: row.sessionId, label: row.label, remark: row.remark }) },
      {
        id: "end", label: t("清除会话"), icon: <Trash2 className="size-4" />, danger: true, separated: true,
        disabled: !row.sessionId || !target || !listed.has(row.sessionId),
        onAction: () => setEnding({ id: row.sessionId, label: row.label }),
      },
    ]);
  };

  const clearDialog = ending && <ClearSessionDialog key={ending.id} label={ending.label}
    needsConnection={!target} onConfirm={end} onCancel={() => setEnding(null)} />;
  const remarkDialog = (
    <RemarkDialog
      open={!!remarkTarget}
      title={t("会话备注")}
      description={remarkTarget?.label ?? ""}
      initialValue={remarkTarget?.remark ?? ""}
      onOpenChange={(next) => !next && setRemarkTarget(null)}
      onSave={async (remark) => {
        if (!remarkTarget || !endpointId) return;
        await remarks.saveSession(endpointId, remarkTarget.id, remark);
        toast.success(t("会话备注已保存"));
      }}
    />
  );

  if (!target) return (
    <>
      {remarkDialog}
      {clearDialog}
      <OfflineSessions
        endpointId={endpointId}
        deviceName={deviceName}
        status={status}
        busy={busy}
        onConnectOpen={onConnectOpen}
        openMenu={openMenu}
        remarkFor={sessionRemark}
        onRemark={(id, label) => setRemarkTarget({ id, label, remark: sessionRemark(id) })}
        onClear={(id, label) => setEnding({ id, label })}
      />
    </>
  );

  return (
    <>
      {remarkDialog}
      {clearDialog}
      <div className="flex min-w-0 flex-col gap-1" onContextMenu={onContextMenu}>
        {rows.map((row) => (
          <CollaborationAnchor key={row.id} endpoint={{ deviceId: endpointId, deviceName, terminalId: row.sessionId, sessionName: row.label }}
            // 协作只对接本机自己开的会话；别的设备开的会话归那台设备
            disabled={!row.sessionId || !live || !listed.has(row.sessionId) || !!row.from}>
          <Button variant={row.tab?.key === activeKey ? "secondary" : "ghost"}
            data-session-row={row.id}
            className="h-8 w-full min-w-0 justify-start gap-2 px-2 text-xs"
            aria-current={row.tab?.key === activeKey ? "true" : undefined}
            title={[row.label, row.from, row.agent && agentStateText(row.agent), (row.agent || row.from) && row.status].filter(Boolean).join(" · ") || undefined}
            onClick={() => openRow(row)}>
            {row.agent ? <AgentIcon kind={row.agent.kind} className="size-3.5 shrink-0" /> : <Terminal className="size-3.5 shrink-0" />}
            <span className="min-w-0 flex-1 truncate text-left">{row.label}</span>
            {row.from && <span className="max-w-[45%] shrink truncate text-[10px] text-muted-foreground">{row.from}</span>}
            {row.agent
              ? <AgentStateLabel agent={row.agent} className="shrink-0 text-[10px]" />
              : row.tab && row.tab.state !== "open"
                ? <span className="shrink-0 text-[10px] text-muted-foreground">{row.status}</span>
                : null}
          </Button>
          </CollaborationAnchor>
        ))}
        {loading && <p className="flex items-center gap-2 px-2 py-1 text-xs text-muted-foreground" role="status"><LoaderCircle className="size-3 animate-spin" />{t("读取会话…")}</p>}
        {error && <p className="px-2 py-1 text-xs text-destructive wrap-break-word" role="alert">{error}</p>}
        {!loading && !error && target && rows.length === 0 && <p className="px-2 py-1 text-xs text-muted-foreground">{t("暂无会话")}</p>}
      </div>
    </>
  );
}

/** 没连上时：显示后台探测拿到的、或者上次记住的会话，点一下就连上并打开 */
function OfflineSessions({ endpointId, deviceName, status, busy, onConnectOpen, openMenu, remarkFor, onRemark, onClear }: {
  endpointId: string;
  deviceName: string;
  status?: DeviceStatus;
  busy: boolean;
  onConnectOpen: (sessionId: string | null) => void;
  openMenu: (e: MouseEvent, actions: MenuAction[]) => void;
  remarkFor: (id: string) => string;
  onRemark: (id: string, label: string) => void;
  onClear: (id: string, label: string) => void;
}) {
  const sessions = [...(status?.sessions ?? [])].sort((a, b) => (a.created_at ?? 0) - (b.created_at ?? 0) || a.id.localeCompare(b.id));
  const titles = sessionTitles(sessions);
  const stale = !status?.sessions_live;
  const offline = status?.state === "offline";
  return (
    <div className="flex min-w-0 flex-col gap-1" data-stale={stale ? "" : undefined}
      onContextMenu={(e) => {
        const id = (e.target as HTMLElement).closest<HTMLElement>("[data-offline-session]")?.dataset.offlineSession;
        if (!id) return;
        openMenu(e, [
          { id: "open", label: t("连接并打开"), icon: <SquareArrowOutUpRight className="size-4" />, disabled: busy || offline, onAction: () => onConnectOpen(id) },
          { id: "remark", label: remarkFor(id).trim() ? t("修改备注") : t("添加备注"), icon: <Pencil className="size-4" />, onAction: () => onRemark(id, titles.get(id) ?? t("会话")) },
          { id: "end", label: t("清除会话"), icon: <Trash2 className="size-4" />, danger: true, separated: true,
            disabled: busy || offline, onAction: () => onClear(id, remarkFor(id).trim() || titles.get(id) || t("会话")) },
        ]);
      }}>
      {sessions.length === 0 && <p className="px-2 py-1 text-xs text-muted-foreground">{t("暂无会话")}</p>}
      {sessions.map((s) => (
        <div key={s.id} data-offline-session={s.id}>
        <CollaborationAnchor endpoint={{ deviceId: endpointId, deviceName, terminalId: s.id, sessionName: remarkFor(s.id).trim() || titles.get(s.id) || t("会话") }}
          disabled={offline || stale}>
        <Button variant="ghost"
          className="offline-session h-8 w-full min-w-0 justify-start gap-2 px-2 text-xs"
          disabled={busy || offline}
          title={offline ? t("设备离线") : t("连接并打开这个会话")}
          onClick={() => onConnectOpen(s.id)}>
          {s.agent ? <AgentIcon kind={s.agent.kind} className="size-3.5 shrink-0" /> : <Terminal className="size-3.5 shrink-0" />}
          <span className="min-w-0 flex-1 truncate text-left">{remarkFor(s.id).trim() || titles.get(s.id) || t("会话")}</span>
          {s.agent && !stale && !offline && <AgentStateLabel agent={s.agent} className="shrink-0 text-[10px]" />}
        </Button>
        </CollaborationAnchor>
        </div>
      ))}
    </div>
  );
}
