import { isTauri } from "@tauri-apps/api/core";
import type { ReactNode } from "react";
import { ArrowLeft, PanelLeftClose, PanelLeftOpen, Settings, Wifi } from "lucide-react";
import appIcon from "../../src-tauri/icons/128x128.png";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import type { Overview } from "../api";
import { LocalDevicePopover } from "./LocalDevicePopover";
import { sidebarShortcut } from "./useSidebar";

export function Titlebar({
  ov,
  onRefresh,
  localDeviceOpen,
  onLocalDeviceOpenChange,
  terminalCount,
  connectionAction,
  onDiag,
  onSettings,
  settingsEnabled,
  sidebarOpen,
  onBack,
  onToggleSidebar,
}: {
  ov: Overview | null;
  onRefresh: () => void;
  localDeviceOpen: boolean;
  onLocalDeviceOpenChange: (open: boolean) => void;
  terminalCount: number;
  connectionAction: ReactNode;
  onDiag: () => void;
  onSettings: () => void;
  settingsEnabled: boolean;
  sidebarOpen: boolean;
  onBack?: () => void;
  onToggleSidebar: () => void;
}) {
  const nativeMac = isTauri() && /Macintosh/.test(navigator.userAgent);
  const nativeWindows = isTauri() && /Windows/.test(navigator.userAgent);

  return (
    <header
      className={`app-titlebar${nativeMac ? " titlebar-macos" : ""}${nativeWindows ? " titlebar-windows" : ""}`}
      aria-label="应用标题栏"
    >
      <div className="titlebar-brand titlebar-drag" data-tauri-drag-region>
        <img className="brand-symbol" src={appIcon} alt="" draggable={false} />
        <div className="flex items-center gap-1.5">
          <strong>P2PCLI</strong>
          <Badge className="border border-[#b78c3f]/50 bg-[#17130c] text-[#e7c477]">Beta</Badge>
        </div>
      </div>
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
      <Separator orientation="vertical" className="mx-2 h-4 shrink-0 self-center" />
      <Button
        variant={onBack ? "secondary" : "ghost"}
        size={onBack ? "sm" : "icon-sm"}
        className={onBack ? "titlebar-sidebar-toggle gap-1.5 rounded-full px-2.5" : "titlebar-sidebar-toggle"}
        aria-label={onBack ? "返回工作区" : sidebarOpen ? "收起侧边栏" : "展开侧边栏"}
        aria-expanded={onBack ? undefined : sidebarOpen}
        aria-controls={onBack ? undefined : "connection-sidebar"}
        title={onBack ? "返回工作区" : `${sidebarOpen ? "收起" : "展开"}侧边栏（${sidebarShortcut}）`}
        onPress={onBack ?? onToggleSidebar}
      >
        {onBack ? <><ArrowLeft data-icon="inline-start" /><span>返回</span></> : sidebarOpen ? <PanelLeftClose /> : <PanelLeftOpen />}
      </Button>
      <div className="titlebar-spacer titlebar-drag" data-tauri-drag-region />
      <div className="titlebar-actions">
        <Badge variant="outline">{terminalCount} 个终端</Badge>
        {connectionAction}
        <LocalDevicePopover ov={ov} onRefresh={onRefresh} open={localDeviceOpen} onOpenChange={onLocalDeviceOpenChange} />
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="查看网络诊断"
          title="查看网络诊断"
          onClick={onDiag}
        >
          <Wifi />
        </Button>
      </div>
    </header>
  );
}
