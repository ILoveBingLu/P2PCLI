import { useCallback, useEffect, useState } from "react";
import { Check, Eye, ImageIcon, LoaderCircle, Monitor } from "lucide-react";
import { Checkbox, Description, ListBox, Select } from "@heroui/react";
import { api, type PrivacyAllow, type PrivacySettings, type ScreenSettings as ScreenSettingsView } from "../api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
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

      {s.privacy_supported && <PrivacySection s={s} onChange={setS} />}

      {isMac && (
        <div className="flex flex-col gap-2">
          <p className="detail-note">{t("macOS 要先在系统设置里允许，别的设备才能看和操作本机：")}</p>
          <MacScreenPermissions s={s} onChange={setS} />
        </div>
      )}
    </div>
  );
}

/** 内置背景主题的名字，顺序和被控端画的一致 */
const presetNames = () => [t("深海"), t("暮色"), t("森林"), t("极光"), t("星空"), t("日落"), t("晨雾"), t("石墨"), t("樱花"), t("海湾")];

/** 隐私屏：谁能开、盖上时本机屏幕上显示什么，可以预览 */
function PrivacySection({ s, onChange }: { s: ScreenSettingsView; onChange: (s: ScreenSettingsView) => void }) {
  const p = s.privacy;
  const [message, setMessage] = useState(p.message);
  const [busy, setBusy] = useState(false);
  const [thumbs, setThumbs] = useState<{ presets: string[]; image: string | null } | null>(null);
  useEffect(() => setMessage(p.message), [p.message]);
  // 小样由被控端按同样的算法画，看到的就是盖上以后的样子；换了图片要重画
  useEffect(() => {
    let alive = true;
    api.screenPrivacyThumbnails().then((x) => alive && setThumbs(x)).catch(() => {});
    return () => { alive = false; };
  }, [s.privacy_has_image]);
  const disabled = !s.enabled || s.view_only;

  const save = async (next: Partial<PrivacySettings>) => {
    setBusy(true);
    try {
      onChange(await api.setScreenPrivacy({ ...p, ...next }));
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };
  const pickImage = async () => {
    try {
      const next = await api.screenPrivacyPickImage();
      if (next) {
        onChange(next);
        setThumbs(await api.screenPrivacyThumbnails());
      }
    } catch (e) {
      toast.error(String(e));
    }
  };
  const preview = async () => {
    try {
      await api.screenPrivacyPreview();
      toast.notice(t("隐私屏盖 5 秒，自己看看效果（预览时不拦键盘鼠标）"));
    } catch (e) {
      toast.error(String(e));
    }
  };

  const names = presetNames();
  const backgrounds: { id: string; label: string; image?: string }[] = [
    { id: "black", label: t("纯黑") },
    ...names.map((label, i) => ({ id: `preset${i}`, label, image: thumbs?.presets[i] })),
    ...(s.privacy_has_image ? [{ id: "image", label: t("我的图片"), image: thumbs?.image ?? undefined }] : []),
  ];
  const current = p.background === "image" && !s.privacy_has_image ? "black" : (p.background || "black");
  const blackout = s.privacy_blackout_supported && !!p.blackout;

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-col gap-1">
        <h3 className="text-sm font-medium">{t("隐私屏")}</h3>
        <p className="text-xs text-muted-foreground">
          {t("对方远程操作时可以盖住本机的屏幕：本机屏幕上只显示下面选的画面，本机的键盘鼠标也被拦住；对方那边照常看到桌面、照常操作。断开、关掉画面、P2PCLI 退出时都会自动撤掉。Windows 上按 Ctrl+Alt+Del 可以锁屏。")}
        </p>
      </div>

      <Select aria-label={t("谁能开隐私屏")} selectedKey={p.allow} isDisabled={busy || disabled}
        onSelectionChange={(key) => key != null && void save({ allow: String(key) as PrivacyAllow })}>
        <Select.Trigger className="max-w-xs"><Select.Value /><Select.Indicator /></Select.Trigger>
        <Select.Popover><ListBox>
          <ListBox.Item id="trusted">{t("只有受信设备能开")}</ListBox.Item>
          <ListBox.Item id="all">{t("能操作本机的设备都能开")}</ListBox.Item>
          <ListBox.Item id="off">{t("不允许开隐私屏")}</ListBox.Item>
        </ListBox></Select.Popover>
      </Select>

      {p.allow !== "off" && s.privacy_blackout_supported && (
        <Checkbox isSelected={blackout} onChange={(on) => void save({ blackout: on })} isDisabled={busy || disabled}>
          <Checkbox.Content className="items-start gap-3 text-sm">
            <Checkbox.Control className="mt-0.5"><Checkbox.Indicator><Check className="size-3" /></Checkbox.Indicator></Checkbox.Control>
            <span className="flex flex-col gap-0.5">
              {t("增强模式：本机屏幕整个变黑")}
              <span className="text-xs text-muted-foreground">
                {t("连指针、通知、锁屏都看不见，看起来像关了屏幕，下面选的画面也不显示。对方那边照常看到桌面。")}
              </span>
            </span>
          </Checkbox.Content>
        </Checkbox>
      )}

      {p.allow !== "off" && <>
        {!blackout && <>
        <div className="grid grid-cols-[repeat(auto-fill,minmax(7.5rem,1fr))] gap-2" role="radiogroup" aria-label={t("隐私屏背景")}>
          {backgrounds.map((b) => (
            <button key={b.id} type="button" role="radio" aria-checked={current === b.id} disabled={busy || disabled}
              title={b.label} onClick={() => void save({ background: b.id })}
              className="group relative aspect-video overflow-hidden rounded-lg border border-white/10 bg-black ring-offset-2 ring-offset-background transition hover:brightness-110 disabled:opacity-50 data-[on]:ring-2 data-[on]:ring-primary"
              data-on={current === b.id ? "" : undefined}>
              {b.image && <img src={b.image} alt="" draggable={false} className="absolute inset-0 size-full object-cover" />}
              {/* 小样上也画个缩小的时钟和那行字，一眼看出盖上后的样子 */}
              <span className="absolute inset-0 flex flex-col items-center justify-center gap-0.5 text-white [text-shadow:0_1px_3px_rgba(0,0,0,.6)]">
                {!p.clock_off && <span className="text-base font-light leading-none tabular-nums">12:30</span>}
                <span className="h-[3px] w-1/2 rounded-full bg-white/60" />
              </span>
              <span className="absolute inset-x-0 bottom-0 bg-gradient-to-t from-black/60 to-transparent px-1.5 pb-1 pt-3 text-left text-[11px] text-white/90">{b.label}</span>
            </button>
          ))}
          <button type="button" disabled={busy || disabled} onClick={() => void pickImage()} title={t("自己选的图片")}
            className="flex aspect-video flex-col items-center justify-center gap-1 rounded-lg border border-dashed text-xs text-muted-foreground transition hover:bg-muted disabled:opacity-50">
            <ImageIcon className="size-4" />{s.privacy_has_image ? t("换一张图片…") : t("选图片…")}
          </button>
        </div>

        <Checkbox isSelected={!p.clock_off} onChange={(on) => void save({ clock_off: !on })} isDisabled={busy || disabled}>
          <Checkbox.Content className="gap-3 text-sm">
            <Checkbox.Control><Checkbox.Indicator><Check className="size-3" /></Checkbox.Indicator></Checkbox.Control>
            {t("显示时钟和日期")}
          </Checkbox.Content>
        </Checkbox>

        <div className="flex flex-col gap-1">
          <Input aria-label={t("隐私屏上的一行字")} value={message} maxLength={200} disabled={busy || disabled}
            placeholder={t("这台电脑正在被「对方设备」远程操作，屏幕已隐藏")}
            onChange={(e) => setMessage(e.target.value)}
            onBlur={() => message.trim() !== p.message && void save({ message })}
            onKeyDown={(e) => { if (e.key === "Enter") (e.target as HTMLInputElement).blur(); }} />
          <span className="text-xs text-muted-foreground">{t("屏幕中间的一行字。留空就写着是哪台设备在操作")}</span>
        </div>
        </>}

        <div>
          <Button variant="secondary" size="sm" disabled={busy || disabled} onPress={() => void preview()}>
            <Eye />{t("预览 5 秒")}
          </Button>
        </div>
      </>}
      {disabled && <p className="text-xs text-muted-foreground">{t("要先允许别的设备查看并操作本机，才能开隐私屏。")}</p>}
    </div>
  );
}
