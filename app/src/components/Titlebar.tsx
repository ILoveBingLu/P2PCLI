import { isTauri } from "@tauri-apps/api/core";
import type { ReactNode } from "react";
import { ArrowLeft, Menu, PanelLeftClose, PanelLeftOpen, Settings, Wifi } from "lucide-react";
import logo from "@/assets/logo.svg";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import type { Overview, SessionSummary } from "../api";
import { InUseIndicator } from "./InUseIndicator";
import { LocalDevicePopover } from "./LocalDevicePopover";
import { PhonePairPopover } from "./PhonePairPopover";
import { sidebarShortcut } from "./useSidebar";
import { t } from "@/i18n";
import { isMobile } from "@/lib/platform";

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
  onKick,
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
  /** 断开一个正连着本机的设备 */
  onKick: (session: SessionSummary) => void;
}) {
  // 手机：一行顶栏，左边菜单（设备抽屉）或返回，右边新建连接、网络诊断、设置。
  // 手机不能被别人连，没有“谁在用本机”“本机信息”；终端数看标签就知道
  if (isMobile) {
    return (
      <header className="app-titlebar mobile-topbar" aria-label={t("应用标题栏")}>
        <Button
          variant="ghost"
          size="icon"
          aria-label={onBack ? t("返回工作区") : sidebarOpen ? t("收起侧边栏") : t("展开侧边栏")}
          aria-expanded={onBack ? undefined : sidebarOpen}
          aria-controls={onBack ? undefined : "connection-sidebar"}
          onPress={onBack ?? onToggleSidebar}
        >
          {onBack ? <ArrowLeft /> : <Menu />}
        </Button>
        <div className="mobile-topbar-brand">
          <img className="brand-symbol" src={logo} alt="" draggable={false} />
          <strong>P2PCLI</strong>
        </div>
        <div className="titlebar-actions">
          {connectionAction}
          <Button variant="ghost" size="icon" aria-label={t("查看网络诊断")} onClick={onDiag}>
            <Wifi />
          </Button>
          <Button variant="ghost" size="icon" aria-label={t("设置")} disabled={!settingsEnabled} onClick={onSettings}>
            <Settings />
          </Button>
        </div>
      </header>
    );
  }

  const nativeMac = isTauri() && /Macintosh/.test(navigator.userAgent);
  const nativeWindows = isTauri() && /Windows/.test(navigator.userAgent);

  return (
    <header
      className={`app-titlebar${nativeMac ? " titlebar-macos" : ""}${nativeWindows ? " titlebar-windows" : ""}`}
      aria-label={t("应用标题栏")}
    >
      <div className="titlebar-brand titlebar-drag" data-tauri-drag-region>
        <img className="brand-symbol" src={logo} alt="" draggable={false} />
        <div className="flex items-center gap-1.5">
          <strong>P2PCLI</strong>
          <Badge className="border border-[#b78c3f]/50 bg-[#17130c] text-[#e7c477]">Beta</Badge>
        </div>
      </div>
      <Button
        variant="ghost"
        size="icon-sm"
        aria-label={t("设置")}
        title={t("设置")}
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
        aria-label={onBack ? t("返回工作区") : sidebarOpen ? t("收起侧边栏") : t("展开侧边栏")}
        aria-expanded={onBack ? undefined : sidebarOpen}
        aria-controls={onBack ? undefined : "connection-sidebar"}
        title={onBack ? t("返回工作区") : t("{{value}}侧边栏（{{sidebarShortcut}}）", { value: sidebarOpen ? t("收起") : t("展开"), sidebarShortcut })}
        onPress={onBack ?? onToggleSidebar}
      >
        {onBack ? <><ArrowLeft data-icon="inline-start" /><span>{t("返回")}</span></> : sidebarOpen ? <PanelLeftClose /> : <PanelLeftOpen />}
      </Button>
      <div className="titlebar-spacer titlebar-drag" data-tauri-drag-region />
      <div className="titlebar-actions">
        <InUseIndicator sessions={ov?.sessions ?? []} onKick={onKick} />
        <Badge variant="outline">{terminalCount} {t("个终端")}</Badge>
        {connectionAction}
        <PhonePairPopover ov={ov} />
        <LocalDevicePopover ov={ov} onRefresh={onRefresh} open={localDeviceOpen} onOpenChange={onLocalDeviceOpenChange} />
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label={t("查看网络诊断")}
          title={t("查看网络诊断")}
          onClick={onDiag}
        >
          <Wifi />
        </Button>
      </div>
    </header>
  );
}
