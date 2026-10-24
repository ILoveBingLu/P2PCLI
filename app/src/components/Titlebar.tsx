import { isTauri } from "@tauri-apps/api/core";
import type { ReactNode } from "react";
import { PanelLeftClose, PanelLeftOpen, Settings, Wifi } from "lucide-react";
import appIcon from "../../src-tauri/icons/128x128.png";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type { Overview } from "../api";
import { LocalDevicePopover } from "./LocalDevicePopover";
import { sidebarShortcut } from "./useSidebar";

export function Titlebar({
  ov,
  onRefresh,
  terminalCount,
  connectionAction,
  onDiag,
  onSettings,
  settingsEnabled,
  sidebarOpen,
  onToggleSidebar,
}: {
  ov: Overview | null;
  onRefresh: () => void;
  terminalCount: number;
  connectionAction: ReactNode;
  onDiag: () => void;
  onSettings: () => void;
  settingsEnabled: boolean;
  sidebarOpen: boolean;
  onToggleSidebar: () => void;
}) {
  const nativeMac = isTauri() && /Macintosh/.test(navigator.userAgent);
  const nativeWindows = isTauri() && /Windows/.test(navigator.userAgent);

  return (
    <header
      className={`app-titlebar${nativeMac ? " titlebar-macos" : ""}${nativeWindows ? " titlebar-windows" : ""}`}
      aria-label="应用标题栏"
    >
      <Button
        variant="ghost"
        size="icon-sm"
        className="titlebar-sidebar-toggle"
        aria-label={sidebarOpen ? "收起侧边栏" : "展开侧边栏"}
        aria-expanded={sidebarOpen}
        aria-controls="connection-sidebar"
        title={`${sidebarOpen ? "收起" : "展开"}侧边栏（${sidebarShortcut}）`}
        onPress={onToggleSidebar}
      >
        {sidebarOpen ? <PanelLeftClose /> : <PanelLeftOpen />}
      </Button>
      <div className="titlebar-brand titlebar-drag" data-tauri-drag-region>
        <img className="brand-symbol" src={appIcon} alt="" draggable={false} />
        <div className="flex items-center gap-1.5">
          <strong>P2PCLI</strong>
          <Badge variant="secondary">Beta</Badge>
        </div>
      </div>
      <div className="titlebar-spacer titlebar-drag" data-tauri-drag-region />
      <div className="titlebar-actions">
        <Badge variant="outline">{terminalCount} 个终端</Badge>
        {connectionAction}
        <LocalDevicePopover ov={ov} onRefresh={onRefresh} />
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="查看网络诊断"
          title="查看网络诊断"
          onClick={onDiag}
        >
          <Wifi />
        </Button>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="设置"
          title="设置"
          disabled={!settingsEnabled}
          onClick={onSettings}
        >
          <Settings />
        </Button>
      </div>
    </header>
  );
}
