import { useEffect, useState } from "react";
import { Accordion } from "@heroui/react";
import { History, Radar, Trash2 } from "lucide-react";
import { api, formatCode, type NearbyDevice, type Overview } from "../api";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Separator } from "@/components/ui/separator";
import { toast } from "@/components/ui/toast";
import { PlatformIcon } from "./PlatformIcon";
import { DeviceSessions, type SidebarTab } from "./DeviceSessions";
import type { SessionTarget } from "./RemoteSessions";

export function ConnectionSidebar({ ov, busy, open, scanning, tabs, activeKey, sessionTarget, onConnect, onConnectLan, onRefresh, onOpenSession, onSelectTab }: {
  ov: Overview | null;
  busy: boolean;
  open: boolean;
  scanning: boolean;
  tabs: SidebarTab[];
  activeKey: number | null;
  sessionTarget: SessionTarget | null;
  onConnect: (code: string) => void;
  /** 局域网里的设备按公钥连，不经过牵线服务器 */
  onConnectLan: (device: NearbyDevice) => void;
  onRefresh: () => void;
  onOpenSession: (target: SessionTarget, sessionId?: string) => void;
  onSelectTab: (key: number) => void;
}) {
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [expandedSections, setExpandedSections] = useState<Set<string>>(new Set(["nearby", "history"]));
  useEffect(() => {
    if (open && scanning) {
      setExpandedSections((keys) => new Set([...keys, "nearby"]));
      document.getElementById("nearby-title")?.scrollIntoView({ block: "nearest" });
    }
  }, [open, scanning]);
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
  const lanOnline = new Set(nearby.map((d) => d.endpoint_id));

  return (
    <aside id="connection-sidebar" className="connection-sidebar" data-collapsed={open ? undefined : ""} aria-label="连接历史">
      <div className="connection-sidebar-inner">
        <Accordion hideSeparator allowsMultipleExpanded expandedKeys={expandedSections}
          onExpandedChange={(keys) => setExpandedSections(new Set([...keys].map(String)))}>
        <Accordion.Item id="nearby" className="recent-section">
          <Accordion.Heading level={2} id="nearby-title">
            <Accordion.Trigger className="min-w-0 gap-2 rounded-lg px-2 py-2 text-[13px] text-left">
              <Radar className="size-3.5 text-muted-foreground" />
              <span className="flex-1">附近设备</span>
              <Badge variant="secondary">{nearby.length}</Badge>
              <Accordion.Indicator className="size-3.5" />
            </Accordion.Trigger>
          </Accordion.Heading>
          <Accordion.Panel aria-busy={scanning}>
          {scanning && <p className="recent-placeholder" role="status">扫描中…</p>}
          {nearby.length ? (
            <div className="flex flex-col">
              {nearby.map((device) => (
                <Button key={device.endpoint_id} variant="ghost" disabled={busy}
                  className="h-auto w-full min-w-0 justify-start gap-2 rounded-lg px-2 py-2 text-xs font-normal"
                  title={`连接 ${device.name}`} onClick={() => onConnectLan(device)}>
                  <PlatformIcon platform={device.platform} decorative />
                  <span className="min-w-0 flex-1 truncate text-left">{device.name}</span>
                </Button>
              ))}
            </div>
          ) : !scanning && (
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
                return (
                  <Accordion.Item id={device.endpoint_id} key={device.endpoint_id}>
                    <Accordion.Heading>
                      <Accordion.Trigger className="min-w-0 gap-2 rounded-lg px-2 py-2 text-xs">
                        <PlatformIcon platform={device.platform} decorative />
                        <span className="min-w-0 flex-1 truncate">{device.name}</span>
                        {lanOnline.has(device.endpoint_id) && <Badge variant="success" title="在同一个局域网里，连接不经过牵线服务器">局域网</Badge>}
                        <Accordion.Indicator className="size-3.5" />
                      </Accordion.Trigger>
                    </Accordion.Heading>
                    <Accordion.Panel>
                      <div className="mb-2 ml-4 min-w-0 border-l border-border pl-2">
                        <div className="flex items-center justify-between gap-1 pl-2">
                          <span className="font-mono text-[10px] text-muted-foreground">{formatCode(device.code)}</span>
                          <Button variant="ghost" size="icon-sm" className="size-6 min-w-0"
                            aria-label={`移除 ${device.name} 的连接历史`} title="从连接历史移除"
                            onClick={async () => {
                              try {
                                for (const entry of ov?.recent.filter((item) => item.endpoint_id === device.endpoint_id) ?? []) await api.forgetRecent(entry.code);
                                onRefresh();
                              } catch (error) { toast.error(String(error)); }
                            }}><Trash2 className="size-3" /></Button>
                        </div>
                        {open && expandedSections.has("history") && expanded.has(device.endpoint_id) && <DeviceSessions
                          key={target?.connId ?? "disconnected"} target={target} tabs={deviceTabs}
                          activeKey={sessionTarget ? null : activeKey} busy={busy}
                          onConnect={() => lanOnline.has(device.endpoint_id)
                            ? onConnectLan({ endpoint_id: device.endpoint_id, name: device.name, platform: device.platform })
                            : onConnect(device.code)}
                          onOpen={onOpenSession} onSelect={onSelectTab} />}
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
