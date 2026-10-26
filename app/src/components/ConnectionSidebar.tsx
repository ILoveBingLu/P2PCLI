import { useEffect, useState, type MouseEvent } from "react";
import { Accordion } from "@heroui/react";
import { ArrowUpRight, Copy, FileUp, FolderUp, History, House, LoaderCircle, Monitor, Pencil, Plus, Radar, RefreshCw, Trash2 } from "lucide-react";
import { api, pathLabel, type DeviceStatus, type NearbyDevice, type Overview, type PathInfo, type TerminalSession, type TerminalSessionSnapshot } from "../api";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Tooltip, TooltipTrigger } from "@/components/ui/tooltip";
import { toast } from "@/components/ui/toast";
import { PlatformIcon } from "./PlatformIcon";
import { DeviceSessions, type SidebarTab } from "./DeviceSessions";
import { RemarkDialog } from "./RemarkDialog";
import { useRemarks } from "./Remarks";
import { useContextMenu, type MenuAction } from "./ContextMenu";
import type { SessionTarget } from "./RemoteSessions";

const EXPANDED_DEVICES_KEY = "p2pcli.sidebar.expanded-devices";
/** 本机那一行第一次出现时默认展开，之后听用户的 */
const LOCAL_SEEDED_KEY = "p2pcli.sidebar.local-seeded";

export function ConnectionSidebar({ ov, liveSessions, busy, open, scanning, nearbyReveal, onScanNearby, tabs, paths, activeKey, sessionTarget, onConnect, onConnectLan, onRefresh, onOpenSession, onShowSessions, onSelectTab, onSessionEnded, onSessionsChanged, onSessionsLocalChange, onPendingOpen }: {
  ov: Overview | null;
  liveSessions: Record<number, TerminalSessionSnapshot>;
  busy: boolean;
  open: boolean;
  scanning: boolean;
  nearbyReveal: number;
  onScanNearby: () => void;
  tabs: SidebarTab[];
  paths: Record<number, PathInfo>;
  activeKey: number | null;
  sessionTarget: SessionTarget | null;
  onConnect: (code: string) => void;
  /** 局域网里的设备按公钥连，不经过牵线服务器 */
  onConnectLan: (device: NearbyDevice) => void;
  onRefresh: () => void;
  onOpenSession: (target: SessionTarget, sessionId?: string) => void;
  onShowSessions: (target: SessionTarget) => void;
  onSelectTab: (key: number) => void;
  /** 侧边栏里结束了某台设备的会话 */
  onSessionEnded: (endpointId: string, sessionId: string) => void;
  /** 要所有会话列表重读一遍 */
  onSessionsChanged: () => void;
  onSessionsLocalChange: (connId: number, sessions: TerminalSession[]) => void;
  /** 没连上的设备上点了会话：连上以后直接打开它（null 是新建） */
  onPendingOpen: (endpointId: string, sessionId: string | null) => void;
}) {
  const menu = useContextMenu();
  const [expanded, setExpanded] = useState<Set<string>>(() => {
    try {
      const saved: unknown = JSON.parse(localStorage.getItem(EXPANDED_DEVICES_KEY) ?? "[]");
      return new Set(Array.isArray(saved) ? saved.filter((id): id is string => typeof id === "string") : []);
    } catch {
      return new Set();
    }
  });
  useEffect(() => {
    try {
      // Use stable device IDs, so renaming a device or changing its code keeps its state.
      localStorage.setItem(EXPANDED_DEVICES_KEY, JSON.stringify([...expanded]));
    } catch {
      // If storage is unavailable, expansion still works for this session.
    }
  }, [expanded]);
  const [expandedSections, setExpandedSections] = useState<Set<string>>(new Set(["local", "nearby", "history"]));
  // 本机自己：常驻的一条连接，会话和终端都和远程设备一样用
  const local = ov?.local ?? null;
  const localId = local?.info.endpoint_id ?? null;
  const localTarget: SessionTarget | null = local ? { connId: local.conn_id, info: local.info, code: null } : null;
  useEffect(() => {
    if (!localId) return;
    try {
      if (localStorage.getItem(LOCAL_SEEDED_KEY)) return;
      localStorage.setItem(LOCAL_SEEDED_KEY, "1");
    } catch {
      // 读不了存储就每次都展开，也不碍事
    }
    setExpanded((keys) => new Set([...keys, localId]));
  }, [localId]);
  const remarks = useRemarks();
  const [remarkTarget, setRemarkTarget] = useState<{ endpointId: string; name: string; remark: string } | null>(null);
  useEffect(() => {
    if (nearbyReveal > 0) {
      setExpandedSections((keys) => new Set([...keys, "nearby"]));
      document.getElementById("nearby-title")?.scrollIntoView({ block: "nearest" });
    }
  }, [nearbyReveal]);
  const activeDevice = sessionTarget?.info.endpoint_id ?? tabs.find((tab) => tab.key === activeKey)?.info.endpoint_id;
  useEffect(() => {
    if (activeDevice) {
      setExpandedSections((keys) => new Set([...keys, activeDevice === localId ? "local" : "history"]));
    }
  }, [activeDevice, localId]);
  // The store preserves device order; duplicate codes use the most recently used entry.
  const seen = new Set<string>();
  const devices = (ov?.recent ?? []).filter((device) => {
    if (seen.has(device.endpoint_id)) return false;
    seen.add(device.endpoint_id);
    return true;
  });
  const deviceRemark = (device: { endpoint_id: string }) => remarks.device(device.endpoint_id);
  const deviceLabel = (device: { endpoint_id: string; name: string }) => deviceRemark(device) || device.name;
  const nearby = ov?.nearby ?? [];
  // 正连着的设备：开着的标签，或者正在看它的会话页
  const connected = new Set(tabs.filter((t) => t.state === "open" || t.state === "opening").map((t) => t.info.endpoint_id));
  if (sessionTarget) connected.add(sessionTarget.info.endpoint_id);
  const deviceIcon = (endpointId: string, platform: string) => (
    <span className="device-icon">
      <PlatformIcon platform={platform} decorative />
      {connected.has(endpointId) && <><span className="connected-dot" /><span className="sr-only">（已连接）</span></>}
    </span>
  );
  const lanOnline = new Set(nearby.filter((d) => d.online).map((d) => d.endpoint_id));
  const statuses = new Map((ov?.devices ?? []).map((d) => [d.endpoint_id, d]));

  // 正连着就拿到连接，没连着就按局域网或设备码去连
  const targetFor = (endpointId: string) =>
    sessionTarget?.info.endpoint_id === endpointId
      ? sessionTarget
      : tabs.find((tab) => tab.info.endpoint_id === endpointId && tab.state !== "closed");
  const connectDevice = (device: { endpoint_id: string; name: string; platform: string; code: string }) =>
    lanOnline.has(device.endpoint_id)
      ? onConnectLan({ endpoint_id: device.endpoint_id, name: device.name, platform: device.platform, online: true })
      : onConnect(device.code);
  const connectOpen = (device: { endpoint_id: string; name: string; platform: string; code: string }, sessionId: string | null) => {
    onPendingOpen(device.endpoint_id, sessionId);
    connectDevice(device);
  };
  const forget = async (endpointId: string) => {
    try {
      for (const entry of ov?.recent.filter((item) => item.endpoint_id === endpointId) ?? []) await api.forgetRecent(entry.code);
      setExpanded((keys) => new Set([...keys].filter((id) => id !== endpointId)));
      onRefresh();
    } catch (error) { toast.error(String(error)); }
  };
  const copyCode = (code: string) =>
    navigator.clipboard.writeText(code).then(() => toast.success("设备码已复制"), () => toast.error("复制失败"));
  /** 选好文件就在后台发，进度在左下角 */
  const sendTo = async (endpointId: string, folders: boolean) => {
    try {
      const picked = await api.pickPaths(folders);
      if (picked.length) await api.sendFiles(endpointId, picked);
    } catch (error) { toast.error(`发不了：${String(error)}`); }
  };
  const sendActions = (endpointId: string, disabled: boolean): MenuAction[] => [
    { id: "send-files", label: "发送文件…", icon: <FileUp className="size-4" />, disabled, separated: true, onAction: () => void sendTo(endpointId, false) },
    { id: "send-folder", label: "发送文件夹…", icon: <FolderUp className="size-4" />, disabled, onAction: () => void sendTo(endpointId, true) },
  ];

  // 设备和附近设备的右键菜单；会话行的菜单在 DeviceSessions 里，先接到就不会冒泡到这里
  const onContextMenu = (e: MouseEvent) => {
    const el = (e.target as HTMLElement).closest<HTMLElement>("[data-menu]");
    if (!el) return;
    const [kind, endpointId] = (el.dataset.menu ?? "").split(":");
    let actions: MenuAction[] = [];
    if (kind === "device") {
      const device = devices.find((d) => d.endpoint_id === endpointId);
      if (!device) return;
      const target = targetFor(endpointId);
      const offline = statuses.get(endpointId)?.state === "offline";
      actions = target
        ? [
          { id: "new", label: "新建会话", icon: <Plus className="size-4" />, onAction: () => onOpenSession(target) },
          { id: "refresh", label: "刷新会话", icon: <RefreshCw className="size-4" />, onAction: onSessionsChanged },
        ]
        : [
          { id: "new", label: "新建会话", icon: <Plus className="size-4" />, disabled: busy || offline, onAction: () => connectOpen(device, null) },
          { id: "connect", label: "连接", icon: <ArrowUpRight className="size-4" />, disabled: busy || offline, onAction: () => connectDevice(device) },
          { id: "refresh", label: "刷新状态", icon: <RefreshCw className="size-4" />, onAction: () => void api.refreshDevices() },
        ];
      actions = [
        ...actions,
        { id: "remark", label: deviceRemark(device).trim() ? "修改备注" : "添加备注", icon: <Pencil className="size-4" />, onAction: () => setRemarkTarget({ endpointId, name: device.name, remark: deviceRemark(device) }) },
        ...sendActions(endpointId, !target && offline),
        { id: "copy", label: "复制设备码", icon: <Copy className="size-4" />, onAction: () => void copyCode(device.code) },
        { id: "forget", label: "从连接历史移除", icon: <Trash2 className="size-4" />, danger: true, separated: true, onAction: () => void forget(endpointId) },
      ];
    } else if (kind === "local") {
      if (!localTarget) return;
      actions = [
        { id: "new", label: "新建会话", icon: <Plus className="size-4" />, onAction: () => onOpenSession(localTarget) },
        { id: "refresh", label: "刷新会话", icon: <RefreshCw className="size-4" />, onAction: onSessionsChanged },
      ];
    } else if (kind === "nearby") {
      const device = nearby.find((d) => d.endpoint_id === endpointId);
      if (!device) return;
      const target = targetFor(endpointId);
      actions = target
        ? [{ id: "new", label: "新建会话", icon: <Plus className="size-4" />, onAction: () => onOpenSession(target) }]
        : [{ id: "connect", label: "连接", icon: <ArrowUpRight className="size-4" />, disabled: busy || !device.online, onAction: () => onConnectLan(device) }];
      actions.push({
          id: "remark",
          label: deviceRemark(device) ? "修改备注" : "添加备注",
          icon: <Pencil className="size-4" />,
          onAction: () => setRemarkTarget({ endpointId, name: device.name, remark: deviceRemark(device) }),
      });
      actions = [...actions, ...sendActions(endpointId, !target && !device.online)];
    }
    if (actions.length) menu.open(e, actions);
  };

  return (
    <aside id="connection-sidebar" className="connection-sidebar" data-collapsed={open ? undefined : ""} aria-label="连接历史" onContextMenu={onContextMenu}>
      {menu.element}
      <RemarkDialog
        open={!!remarkTarget}
        title="设备备注"
        description={remarkTarget?.name ?? ""}
        initialValue={remarkTarget?.remark ?? ""}
        onOpenChange={(next) => !next && setRemarkTarget(null)}
        onSave={async (remark) => {
          if (!remarkTarget) return;
          await remarks.saveDevice(remarkTarget.endpointId, remark);
          onRefresh();
          toast.success("设备备注已保存");
        }}
      />
      <div className="connection-sidebar-inner">
        <Accordion allowsMultipleExpanded expandedKeys={expandedSections}
          onExpandedChange={(keys) => setExpandedSections(new Set([...keys].map(String)))}>
        {local && localTarget && localId && (
          <Accordion.Item id="local" className="recent-section">
            <Accordion.Heading level={2} id="local-title">
              <Accordion.Trigger className="min-w-0 gap-2 rounded-lg px-2 py-2 text-[13px] text-left">
                <Monitor className="size-3.5 text-muted-foreground" />
                <span className="flex-1">本机</span>
                <Accordion.Indicator className="size-3.5" />
              </Accordion.Trigger>
            </Accordion.Heading>
            <Accordion.Panel>
              <Accordion allowsMultipleExpanded expandedKeys={expanded}
                onExpandedChange={(keys) => setExpanded(new Set([...keys].map(String)))}>
                <Accordion.Item id={localId} className="py-1">
                  <Accordion.Heading className="flex items-stretch" data-menu={`local:${localId}`}>
                    <Button slot={null} variant="ghost"
                      className="h-auto min-w-0 flex-1 justify-start gap-2 rounded-lg px-2 py-2 text-xs"
                      aria-label={`查看本机的会话 ${local.info.name}`}
                      title="在本机开终端、查看后台会话"
                      onPress={() => onShowSessions(localTarget)}>
                      {deviceIcon(localId, local.info.platform)}
                      <span className="min-w-0 flex-1 truncate text-left">{local.info.name}</span>
                      <span className="presence-label" data-kind="local">本机</span>
                    </Button>
                    <Accordion.Trigger className="w-8 min-w-0 flex-none justify-center rounded-lg p-2"
                      aria-label={`${expanded.has(localId) ? "收起" : "展开"}本机的会话`}>
                      <Accordion.Indicator className="size-3.5" />
                    </Accordion.Trigger>
                  </Accordion.Heading>
                  <Accordion.Panel>
                    <div className="pb-2">
                      <div className="ml-4 min-w-0 border-l border-border pl-2">
                        {open && expandedSections.has("local") && expanded.has(localId) && <DeviceSessions
                          key="local" target={localTarget}
                          tabs={tabs.filter((tab) => tab.info.endpoint_id === localId)}
                          endpointId={localId}
                          snapshot={liveSessions[local.conn_id]}
                          activeKey={sessionTarget ? null : activeKey} busy={busy}
                          onChangeSessions={(sessions) => onSessionsLocalChange(local.conn_id, sessions)}
                          onConnectOpen={() => onOpenSession(localTarget)}
                          onOpen={onOpenSession} onSelect={onSelectTab}
                          onEnded={(id) => onSessionEnded(localId, id)}
                          openMenu={menu.open} />}
                      </div>
                    </div>
                  </Accordion.Panel>
                </Accordion.Item>
              </Accordion>
            </Accordion.Panel>
          </Accordion.Item>
        )}
        <Accordion.Item id="nearby" className="recent-section">
          <Accordion.Heading level={2} id="nearby-title" className="grid grid-cols-[max-content_1.5rem_minmax(0,1fr)] items-center gap-2">
            <Accordion.Trigger className="col-span-3 col-start-1 row-start-1 grid grid-cols-subgrid min-w-0 gap-2 rounded-lg px-2 py-2 text-[13px] text-left">
              <span className="flex items-center gap-2"><Radar className="size-3.5 text-muted-foreground" />附近设备</span>
              <span aria-hidden="true" />
              <span className="flex items-center justify-end gap-2">
                <Badge variant="secondary">{nearby.length}</Badge>
                <Accordion.Indicator className="size-3.5" />
              </span>
            </Accordion.Trigger>
            <Button slot={null} variant="ghost" size="icon-sm"
              className="col-start-2 row-start-1 z-10 size-6 min-w-0 text-muted-foreground"
              title="刷新附近设备" aria-label="刷新附近设备" disabled={scanning}
              onPress={onScanNearby}>
              <RefreshCw className={scanning ? "size-3 animate-spin" : "size-3"} />
            </Button>
          </Accordion.Heading>
          <Accordion.Panel aria-busy={scanning}>
          {nearby.length ? (
            <div className="flex flex-col">
              {nearby.map((device) => (
                <div key={device.endpoint_id} data-menu={`nearby:${device.endpoint_id}`}>
                <Button variant="ghost" disabled={busy || !device.online}
                  className="h-auto w-full min-w-0 justify-start gap-2 rounded-lg px-2 py-2 text-xs font-normal"
                  aria-label={`${deviceLabel(device)}${connected.has(device.endpoint_id) ? "（已连接）" : device.online ? "" : "（离线）"}`}
                  title={device.online ? `连接 ${deviceLabel(device)}` : `${deviceLabel(device)} · 离线`} onClick={() => onConnectLan(device)}>
                  {deviceIcon(device.endpoint_id, device.platform)}
                  <span className="min-w-0 flex-1 truncate text-left">{deviceLabel(device)}</span>
                </Button>
                </div>
              ))}
            </div>
          ) : (
            <p className="recent-placeholder">
              暂无附近设备
            </p>
          )}
          </Accordion.Panel>
        </Accordion.Item>
        <Accordion.Item id="history" className="recent-section">
          <Accordion.Heading level={2} id="recent-title">
            <Accordion.Trigger className="min-w-0 gap-2 rounded-lg px-2 py-2 text-[13px] text-left">
              <History className="size-3.5 text-muted-foreground" />
              <span className="flex-1">连接历史</span>
              <Badge variant="secondary">{devices.length}</Badge>
              <Accordion.Indicator className="size-3.5" />
            </Accordion.Trigger>
          </Accordion.Heading>
          <Accordion.Panel>
          {devices.length ? (
            <Accordion allowsMultipleExpanded expandedKeys={expanded}
              onExpandedChange={(keys) => setExpanded(new Set([...keys].map(String)))}>
              {devices.map((device) => {
                const label = deviceLabel(device);
                const deviceTabs = tabs.filter((tab) => tab.info.endpoint_id === device.endpoint_id);
                const target = sessionTarget?.info.endpoint_id === device.endpoint_id ? sessionTarget : deviceTabs.find((tab) => tab.state !== "closed");
                // A device can have multiple connections: match the visible terminal first.
                const pathTarget = deviceTabs.find((tab) => tab.key === activeKey && tab.state !== "closed") ?? target;
                const status = statuses.get(device.endpoint_id);
                return (
                  <Accordion.Item id={device.endpoint_id} key={device.endpoint_id} className="py-1">
                    <Accordion.Heading className="flex items-stretch" data-menu={`device:${device.endpoint_id}`}>
                      <Button slot={null} variant="ghost"
                        className="h-auto min-w-0 flex-1 justify-start gap-2 rounded-lg px-2 py-2 text-xs"
                        aria-label={`${target ? "查看会话" : "连接"} ${label}`}
                        disabled={!target && busy}
                        onPress={() => target ? onShowSessions(target) : connectDevice(device)}
                        data-offline={!target && status?.state === "offline" ? "" : undefined}>
                        {deviceIcon(device.endpoint_id, device.platform)}
                        <span className="min-w-0 flex-1 truncate text-left" title={deviceRemark(device).trim() ? `${label} · ${device.name}` : undefined}>{label}</span>
                        {lanOnline.has(device.endpoint_id) && (
                          <span className="lan-mark" title="在同一个局域网里，连接不经过牵线服务器">
                            <House aria-hidden="true" /><span className="sr-only">局域网</span>
                          </span>
                        )}
                        <PresenceLabel status={status} connected={!!pathTarget} path={pathTarget ? paths[pathTarget.connId] : undefined} />
                      </Button>
                      <Accordion.Trigger className="w-8 min-w-0 flex-none justify-center rounded-lg p-2"
                        aria-label={`${expanded.has(device.endpoint_id) ? "收起" : "展开"} ${label}的会话`}>
                        <Accordion.Indicator className="size-3.5" />
                      </Accordion.Trigger>
                    </Accordion.Heading>
                    <Accordion.Panel>
                      {/* Padding participates in the panel's measured height; bottom margins can be clipped. */}
                      <div className="pb-2">
                        <div className="ml-4 min-w-0 border-l border-border pl-2">
                          {open && expandedSections.has("history") && expanded.has(device.endpoint_id) && <DeviceSessions
                            key={target?.connId ?? "disconnected"} target={target} status={status} tabs={deviceTabs}
                            endpointId={device.endpoint_id}
                            snapshot={target ? liveSessions[target.connId] : undefined}
                            activeKey={sessionTarget ? null : activeKey} busy={busy}
                            onChangeSessions={(sessions) => target && onSessionsLocalChange(target.connId, sessions)}
                            onConnectOpen={(id) => connectOpen(device, id)}
                            onOpen={onOpenSession} onSelect={onSelectTab}
                            onEnded={(id) => onSessionEnded(device.endpoint_id, id)}
                            openMenu={menu.open} />}
                        </div>
                      </div>
                    </Accordion.Panel>
                  </Accordion.Item>
                );
              })}
            </Accordion>
          ) : <p className="recent-placeholder">暂无连接历史</p>}
          </Accordion.Panel>
        </Accordion.Item>
        </Accordion>
      </div>
    </aside>
  );
}

/** 设备行右边的在线状态：在线显示延迟（直连绿色、中继黄色），离线、旧版本各有提示 */
function PresenceLabel({ status, connected, path }: { status?: DeviceStatus; connected: boolean; path?: PathInfo }) {
  if (!connected && status && (status.probing || status.state === "checking")) {
    return (
      <Tooltip content="正在与设备握手并探测延迟">
        <TooltipTrigger<"span"> render={(props) => <span {...props} />}
          role="status" aria-label="正在探测" className="presence-label" data-kind="checking">
          <LoaderCircle className="presence-spinner animate-spin" aria-hidden="true" />
          <span>探测中…</span>
        </TooltipTrigger>
      </Tooltip>
    );
  }
  if (connected || status?.state === "online") {
    const p = connected ? path : status?.path;
    const label = connected ? "已连接" : "在线";
    const tip = `${label} · ${connected ? "终端连接" : "后台探测"} · ${pathLabel(p)}${p?.remote ? ` · ${p.remote}` : ""}`;
    return (
      <Tooltip content={tip}>
        <TooltipTrigger<"span"> render={(props) => <span {...props} />}
          role="presentation" tabIndex={-1} className="presence-label" data-kind={p?.kind ?? "unknown"}>
          {p?.rtt_ms != null && p.kind !== "unknown" ? `${p.rtt_ms} ms` : label}
        </TooltipTrigger>
      </Tooltip>
    );
  }
  if (!status) return null;
  if (status.state === "offline") return <span className="presence-label" data-kind="off">离线</span>;
  if (status.state === "old_version") return (
    <Tooltip content="对方的 P2PCLI 版本旧，升级后能显示延迟和会话">
      <TooltipTrigger<"span"> render={(props) => <span {...props} />}
        role="presentation" tabIndex={-1} className="presence-label">需更新</TooltipTrigger>
    </Tooltip>
  );
  return null;
}
