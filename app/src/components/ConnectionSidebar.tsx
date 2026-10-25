import { useEffect, useRef, useState, type MouseEvent } from "react";
import { Accordion } from "@heroui/react";
import { ArrowUpRight, Copy, History, House, Plus, Radar, RefreshCw, Trash2 } from "lucide-react";
import { api, formatCode, pathLabel, type DeviceStatus, type NearbyDevice, type Overview } from "../api";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Separator } from "@/components/ui/separator";
import { toast } from "@/components/ui/toast";
import { PlatformIcon } from "./PlatformIcon";
import { DeviceSessions, type SidebarTab } from "./DeviceSessions";
import { useContextMenu, type MenuAction } from "./ContextMenu";
import type { SessionTarget } from "./RemoteSessions";

export function ConnectionSidebar({ ov, sessionsRev, busy, open, scanning, nearbyReveal, onScanNearby, tabs, activeKey, sessionTarget, onConnect, onConnectLan, onRefresh, onOpenSession, onSelectTab, onSessionEnded, onSessionsChanged, onPendingOpen }: {
  ov: Overview | null;
  /** 会话被结束时变，设备下面的会话列表跟着刷新 */
  sessionsRev: number;
  busy: boolean;
  open: boolean;
  scanning: boolean;
  nearbyReveal: number;
  onScanNearby: () => void;
  tabs: SidebarTab[];
  activeKey: number | null;
  sessionTarget: SessionTarget | null;
  onConnect: (code: string) => void;
  /** 局域网里的设备按公钥连，不经过牵线服务器 */
  onConnectLan: (device: NearbyDevice) => void;
  onRefresh: () => void;
  onOpenSession: (target: SessionTarget, sessionId?: string) => void;
  onSelectTab: (key: number) => void;
  /** 侧边栏里结束了某台设备的会话 */
  onSessionEnded: (endpointId: string, sessionId: string) => void;
  /** 要所有会话列表重读一遍 */
  onSessionsChanged: () => void;
  /** 没连上的设备上点了会话：连上以后直接打开它（null 是新建） */
  onPendingOpen: (endpointId: string, sessionId: string | null) => void;
}) {
  const menu = useContextMenu();
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [expandedSections, setExpandedSections] = useState<Set<string>>(new Set(["nearby", "history"]));
  useEffect(() => {
    if (nearbyReveal > 0) {
      setExpandedSections((keys) => new Set([...keys, "nearby"]));
      document.getElementById("nearby-title")?.scrollIntoView({ block: "nearest" });
    }
  }, [nearbyReveal]);
  const activeDevice = sessionTarget?.info.endpoint_id ?? tabs.find((tab) => tab.key === activeKey)?.info.endpoint_id;
  useEffect(() => {
    if (activeDevice) {
      setExpanded((keys) => new Set([...keys, activeDevice]));
      setExpandedSections((keys) => new Set([...keys, "history"]));
    }
  }, [activeDevice]);
  // A device may have historical codes; show it once, using its most recent code.
  const seen = new Set<string>();
  const devices = (ov?.recent ?? []).filter((device) => {
    if (seen.has(device.endpoint_id)) return false;
    seen.add(device.endpoint_id);
    return true;
  });
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

  // 第一次拿到状态时，把有会话的设备展开，打开软件就能看到之前的会话
  const autoExpanded = useRef(false);
  useEffect(() => {
    if (autoExpanded.current || !ov?.devices.length) return;
    autoExpanded.current = true;
    const withSessions = ov.devices.filter((d) => d.sessions.length > 0).map((d) => d.endpoint_id);
    if (withSessions.length) setExpanded((keys) => new Set([...keys, ...withSessions]));
  }, [ov?.devices]);

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
      onRefresh();
    } catch (error) { toast.error(String(error)); }
  };
  const copyCode = (code: string) =>
    navigator.clipboard.writeText(code).then(() => toast.success("设备码已复制"), () => toast.error("复制失败"));

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
        { id: "copy", label: "复制设备码", icon: <Copy className="size-4" />, onAction: () => void copyCode(device.code) },
        { id: "forget", label: "从连接历史移除", icon: <Trash2 className="size-4" />, danger: true, separated: true, onAction: () => void forget(endpointId) },
      ];
    } else if (kind === "nearby") {
      const device = nearby.find((d) => d.endpoint_id === endpointId);
      if (!device) return;
      const target = targetFor(endpointId);
      actions = target
        ? [{ id: "new", label: "新建会话", icon: <Plus className="size-4" />, onAction: () => onOpenSession(target) }]
        : [{ id: "connect", label: "连接", icon: <ArrowUpRight className="size-4" />, disabled: busy || !device.online, onAction: () => onConnectLan(device) }];
    }
    if (actions.length) menu.open(e, actions);
  };

  return (
    <aside id="connection-sidebar" className="connection-sidebar" data-collapsed={open ? undefined : ""} aria-label="连接历史" onContextMenu={onContextMenu}>
      {menu.element}
      <div className="connection-sidebar-inner">
        <Accordion hideSeparator allowsMultipleExpanded expandedKeys={expandedSections}
          onExpandedChange={(keys) => setExpandedSections(new Set([...keys].map(String)))}>
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
                <Button key={device.endpoint_id} variant="ghost" disabled={busy || !device.online}
                  data-menu={`nearby:${device.endpoint_id}`}
                  className="h-auto w-full min-w-0 justify-start gap-2 rounded-lg px-2 py-2 text-xs font-normal"
                  aria-label={`${device.name}${connected.has(device.endpoint_id) ? "（已连接）" : device.online ? "" : "（离线）"}`}
                  title={device.online ? `连接 ${device.name}` : `${device.name} · 离线`} onClick={() => onConnectLan(device)}>
                  {deviceIcon(device.endpoint_id, device.platform)}
                  <span className="min-w-0 flex-1 truncate text-left">{device.name}</span>
                </Button>
              ))}
            </div>
          ) : (
            <p className="recent-placeholder">
              暂无附近设备
            </p>
          )}
          </Accordion.Panel>
        </Accordion.Item>
        <Separator className="mt-1 mb-0 h-[0.7px] opacity-50" />
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
            <Accordion hideSeparator allowsMultipleExpanded expandedKeys={expanded}
              onExpandedChange={(keys) => setExpanded(new Set([...keys].map(String)))}>
              {devices.map((device) => {
                const deviceTabs = tabs.filter((tab) => tab.info.endpoint_id === device.endpoint_id);
                const target = sessionTarget?.info.endpoint_id === device.endpoint_id ? sessionTarget : deviceTabs.find((tab) => tab.state !== "closed");
                const status = statuses.get(device.endpoint_id);
                return (
                  <Accordion.Item id={device.endpoint_id} key={device.endpoint_id}>
                    <Accordion.Heading>
                      <Accordion.Trigger className="min-w-0 gap-2 rounded-lg px-2 py-2 text-xs" data-menu={`device:${device.endpoint_id}`}
                        data-offline={!target && status?.state === "offline" ? "" : undefined}>
                        {deviceIcon(device.endpoint_id, device.platform)}
                        <span className="min-w-0 flex-1 truncate">{device.name}</span>
                        {lanOnline.has(device.endpoint_id) && (
                          <span className="lan-mark" title="在同一个局域网里，连接不经过牵线服务器">
                            <House aria-hidden="true" /><span className="sr-only">局域网</span>
                          </span>
                        )}
                        <PresenceLabel status={status} />
                        <Accordion.Indicator className="size-3.5" />
                      </Accordion.Trigger>
                    </Accordion.Heading>
                    <Accordion.Panel>
                      <div className="mb-2 ml-4 min-w-0 border-l border-border pl-2">
                        <div className="flex h-6 items-center pl-2" data-menu={`device:${device.endpoint_id}`}>
                          <span className="font-mono text-[10px] text-muted-foreground">{formatCode(device.code)}</span>
                        </div>
                        {open && expandedSections.has("history") && expanded.has(device.endpoint_id) && <DeviceSessions
                          key={target?.connId ?? "disconnected"} target={target} status={status} tabs={deviceTabs} sessionsRev={sessionsRev}
                          activeKey={sessionTarget ? null : activeKey} busy={busy}
                          onConnect={() => connectDevice(device)}
                          onConnectOpen={(id) => connectOpen(device, id)}
                          onOpen={onOpenSession} onSelect={onSelectTab}
                          onEnded={(id) => onSessionEnded(device.endpoint_id, id)}
                          openMenu={menu.open} />}
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
function PresenceLabel({ status }: { status?: DeviceStatus }) {
  if (!status) return null;
  if (status.state === "online") {
    const p = status.path;
    const tip = `在线 · ${pathLabel(p)}${p?.remote ? ` · ${p.remote}` : ""}`;
    return <span className="presence-label" data-kind={p?.kind ?? "unknown"} title={tip}>{p?.rtt_ms != null ? `${p.rtt_ms} ms` : "在线"}</span>;
  }
  if (status.state === "offline") return <span className="presence-label" data-kind="off" title="离线">离线</span>;
  if (status.state === "old_version") return <span className="presence-label" title="对方的 P2PCLI 版本旧，升级后能显示延迟和会话">需更新</span>;
  return null;
}
