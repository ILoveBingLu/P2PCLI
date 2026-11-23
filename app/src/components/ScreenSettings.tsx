import { useCallback, useEffect, useState } from "react";
import { Check, LoaderCircle, Monitor } from "lucide-react";
import { Checkbox, Description } from "@heroui/react";
import { api, type ScreenSettings as ScreenSettingsView } from "../api";
import { MacScreenPermissions, isMac } from "./ScreenPermissions";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { toast } from "@/components/ui/toast";
import { t } from "@/i18n";

/** 远程画面：别的设备能不能看、能不能操作本机；macOS 上还要系统给录屏和辅助功能权限 */
export function ScreenSettings() {
  const [s, setS] = useState<ScreenSettingsView | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(() => api.screenSettings().then(setS).catch((e) => toast.error(String(e))), []);
  useEffect(() => {
    void load();
    // 从系统设置里授权回来时刷新一下
    const onFocus = () => void load();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [load]);

  const save = async (enabled: boolean, viewOnly: boolean) => {
    setBusy(true);
    try {
      setS(await api.setScreenSettings(enabled, viewOnly));
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  if (!s) return <div className="loading-state" role="status"><LoaderCircle className="animate-spin" /></div>;
  if (!s.supported) {
    return (
      <Empty>
        <EmptyHeader>
          <EmptyMedia variant="icon"><Monitor /></EmptyMedia>
          <EmptyTitle>{t("这台电脑暂不支持被查看画面")}</EmptyTitle>
          <EmptyDescription>{t("目前支持 macOS 和 Windows。这台电脑照样能查看别的电脑的画面。")}</EmptyDescription>
        </EmptyHeader>
      </Empty>
    );
  }

  return (
    <div className="flex flex-col gap-5">
      <Alert>
        <AlertDescription>
          {t("点左侧的设备，在右边设备页点“远程控制”（或者在设备上右键选“远程控制”），就能在单独的窗口里查看和操作对方的屏幕。抓屏、编码、解码都用两边电脑的显卡，几乎不占 CPU。画面只走直连（IPv6、IPv4 打洞、局域网），连接落到中继上时画面暂停，终端照常能用。")}
        </AlertDescription>
      </Alert>

      <Checkbox isSelected={s.enabled} onChange={(on) => void save(on, s.view_only)} isDisabled={busy}>
        <Checkbox.Content className="gap-3 text-sm">
          <Checkbox.Control><Checkbox.Indicator><Check className="size-3" /></Checkbox.Indicator></Checkbox.Control>
          {t("允许别的设备查看本机画面")}
        </Checkbox.Content>
        <Description className="pl-7 text-xs text-muted-foreground">
          {t("受信设备直接打开，别的设备要你在弹窗里点允许。有人在看时标题栏右边会一直亮着，随时可以停止。")}
        </Description>
      </Checkbox>

      <Checkbox isSelected={!s.view_only} onChange={(on) => void save(s.enabled, !on)} isDisabled={busy || !s.enabled}>
        <Checkbox.Content className="gap-3 text-sm">
          <Checkbox.Control><Checkbox.Indicator><Check className="size-3" /></Checkbox.Indicator></Checkbox.Control>
          {t("允许对方操作键盘和鼠标")}
        </Checkbox.Content>
        <Description className="pl-7 text-xs text-muted-foreground">
          {t("关掉以后别的设备只能看，不能点也不能打字。")}
        </Description>
      </Checkbox>

      {isMac && (
        <div className="flex flex-col gap-2">
          <p className="detail-note">{t("macOS 要先在系统设置里允许，别的设备才能看和操作本机：")}</p>
          <MacScreenPermissions s={s} onChange={setS} />
        </div>
      )}
    </div>
  );
}
