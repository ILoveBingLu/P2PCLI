import { useEffect, useState } from "react";
import { LoaderCircle, Monitor, RotateCcw } from "lucide-react";
import { api, type ScreenSettings } from "../api";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { toast } from "@/components/ui/toast";
import { t } from "@/i18n";

export const isMac = navigator.userAgent.includes("Mac");

/** 还缺权限，或者录屏刚允许、要重开才生效 */
export const permissionsPending = (s: ScreenSettings) => !s.capture_permission || !s.control_permission || s.capture_needs_restart;

/**
 * macOS 的录屏、辅助功能两项权限：每项一行，没允许的点“开启”弹系统的授权提示。
 * 开着时每秒看一次，用户在系统设置里打开开关后这里自己变成“已允许”；录屏是这次才允许的，给一个重开按钮
 */
export function MacScreenPermissions({ s, onChange }: { s: ScreenSettings; onChange: (s: ScreenSettings) => void }) {
  const [asked, setAsked] = useState<Record<string, boolean>>({});
  const [restarting, setRestarting] = useState(false);
  const pending = permissionsPending(s);

  useEffect(() => {
    if (!pending) return;
    const timer = window.setInterval(() => void api.screenSettings().then(onChange).catch(() => {}), 1000);
    return () => window.clearInterval(timer);
  }, [pending, onChange]);

  const request = async (capture: boolean) => {
    try {
      onChange(await api.screenRequestPermission(capture));
      setAsked((a) => ({ ...a, [String(capture)]: true }));
    } catch (e) {
      toast.error(String(e));
    }
  };
  const openSettings = (capture: boolean) => void api.screenOpenPermissionSettings(capture).catch((e) => toast.error(String(e)));
  const restart = async () => {
    setRestarting(true);
    try {
      await api.restart();
    } catch (e) {
      setRestarting(false);
      toast.error(String(e));
    }
  };

  return (
    <div className="flex flex-col gap-2">
      {([
        [true, t("录屏与系统录音"), t("让别的设备看到本机画面"), s.capture_permission],
        [false, t("辅助功能"), t("让别的设备操作本机的键盘和鼠标"), s.control_permission],
      ] as const).map(([capture, name, what, granted]) => (
        <div key={name} className="flex flex-wrap items-center gap-x-3 gap-y-2 rounded-md border border-border px-3 py-2.5">
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <strong className="text-sm">{name}</strong>
              <Badge variant={granted ? "success" : "warning"}>{granted ? t("已允许") : t("未允许")}</Badge>
            </div>
            <div className="mt-0.5 text-xs text-muted-foreground">{what}</div>
            {!granted && asked[String(capture)] && (
              <div className="mt-1 text-xs text-muted-foreground">
                {t("在系统的提示里点“打开系统设置”，把 P2PCLI 的开关打开。")}
                <Button variant="link" size="sm" className="h-auto px-1" onClick={() => openSettings(capture)}>
                  {t("没看到提示？直接打开系统设置")}
                </Button>
              </div>
            )}
          </div>
          {!granted && (
            <Button variant="outline" size="sm" onClick={() => void request(capture)}>
              {t("开启")}
            </Button>
          )}
        </div>
      ))}
      {s.capture_needs_restart ? (
        <div className="flex flex-wrap items-center gap-x-3 gap-y-2 rounded-md bg-muted-surface px-3 py-2.5">
          <p className="min-w-0 flex-1 text-xs text-muted-foreground">
            {t("录屏已允许，重新打开 P2PCLI 后生效。正连着的远程终端会断开，后台会话不受影响。")}
          </p>
          <Button size="sm" disabled={restarting} onClick={() => void restart()}>
            {restarting ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : <RotateCcw data-icon="inline-start" />}
            {t("重新打开 P2PCLI")}
          </Button>
        </div>
      ) : !s.capture_permission && (
        <p className="text-xs text-muted-foreground">{t("录屏权限打开后要重新打开 P2PCLI 才生效，到时这里会有按钮。")}</p>
      )}
    </div>
  );
}

/**
 * Mac 上第一次打开时的引导：别的设备要看、要操作本机，得先给这两项权限。
 * 点“以后再说”或“完成”以后不再自动弹，之后在 设置 → 远程控制 里照样能开
 */
export function ScreenPermissionGuide({ initial, onClose }: { initial: ScreenSettings; onClose: () => void }) {
  const [s, setS] = useState(initial);
  const allSet = s.capture_permission && s.control_permission;

  const close = () => {
    void api.screenGuideDone().catch(() => {});
    onClose();
  };

  return (
    <Dialog open onOpenChange={(open) => !open && close()}>
      <DialogContent>
        <DialogHeader>
          <div className="dialog-symbol">
            <Monitor />
          </div>
          <DialogTitle>{t("让别的设备能看、能操作这台 Mac")}</DialogTitle>
          <DialogDescription>
            {t("远程控制要用 macOS 的两项权限。点“开启”后系统会弹出提示，点里面的“打开系统设置”，把 P2PCLI 的开关打开就行。不用远程控制可以跳过，以后在 设置 → 远程控制 里也能开。")}
          </DialogDescription>
        </DialogHeader>
        <MacScreenPermissions s={s} onChange={setS} />
        <DialogFooter>
          {allSet ? (
            <Button onClick={close}>{t("完成")}</Button>
          ) : (
            <Button variant="outline" onClick={close}>{t("以后再说")}</Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
