import { useEffect, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  ChevronRight,
  CircleHelp,
  Copy,
  Minus,
  Settings2,
  Square,
  TerminalSquare,
  X,
} from "lucide-react";
import { toast } from "sonner";
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
  const native = isTauri();
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    if (!native) return;
    const appWindow = getCurrentWindow();
    let disposed = false;
    const sync = async () => {
      try {
        const value = await appWindow.isMaximized();
        if (!disposed) setMaximized(value);
      } catch (error) {
        if (!disposed) console.error("读取窗口状态失败", error);
      }
    };
    const unlisten = appWindow
      .onResized(() => void sync())
      .catch((error) => {
        console.error("监听窗口状态失败", error);
        return () => {};
      });
    void sync();
    return () => {
      disposed = true;
      void unlisten.then((stop) => stop()).catch(console.error);
    };
  }, [native]);

  const control = async (action: "minimize" | "toggleMaximize" | "close") => {
    if (!native) return;
    try {
      const appWindow = getCurrentWindow();
      await appWindow[action]();
      if (action === "toggleMaximize")
        setMaximized(await appWindow.isMaximized());
    } catch (error) {
      toast.error("窗口操作失败：" + String(error));
    }
  };

  return (
    <header className="app-titlebar" aria-label="应用标题栏">
      <div className="titlebar-brand titlebar-drag" data-tauri-drag-region>
        <span className="brand-symbol">
          <TerminalSquare aria-hidden="true" />
        </span>
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
      <div className="window-controls" aria-label="窗口控制">
        <Button
          variant="ghost"
          className="window-control"
          disabled={!native}
          aria-label="最小化"
          title="最小化"
          onClick={() => void control("minimize")}
        >
          <Minus />
        </Button>
        <Button
          variant="ghost"
          className="window-control"
          disabled={!native}
          aria-label={maximized ? "还原窗口" : "最大化"}
          title={maximized ? "还原窗口" : "最大化"}
          onClick={() => void control("toggleMaximize")}
        >
          {maximized ? <Copy /> : <Square />}
        </Button>
        <Button
          variant="ghost"
          className="window-control window-close"
          disabled={!native}
          aria-label="关闭窗口（隐藏到托盘）"
          title="关闭窗口（隐藏到托盘）"
          onClick={() => void control("close")}
        >
          <X />
        </Button>
      </div>
    </header>
  );
}
