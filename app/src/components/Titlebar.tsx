import { isTauri } from "@tauri-apps/api/core";
import {
  ChevronRight,
  CircleHelp,
  Settings2,
} from "lucide-react";
import appIcon from "../../src-tauri/icons/128x128.png";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";

export function Titlebar({
  deviceName,
  terminalCount,
  onDiag,
  onSettings,
  settingsEnabled,
}: {
  deviceName?: string;
  terminalCount: number;
  onDiag: () => void;
  onSettings: () => void;
  settingsEnabled: boolean;
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
        <strong>P2PCLI</strong>
        <Badge variant="outline">Beta</Badge>
      </div>
      <div className="titlebar-context titlebar-drag" data-tauri-drag-region>
        <h1>终端工作区</h1>
        <ChevronRight aria-hidden="true" />
        <span>{deviceName ?? "开始连接"}</span>
      </div>
      <div className="titlebar-actions">
        <Badge variant="outline">{terminalCount} 个终端</Badge>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="查看网络诊断"
          title="查看网络诊断"
          onClick={onDiag}
        >
          <CircleHelp />
        </Button>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="设置"
          title="设置"
          disabled={!settingsEnabled}
          onClick={onSettings}
        >
          <Settings2 />
        </Button>
      </div>
    </header>
  );
}
