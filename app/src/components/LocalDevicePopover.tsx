import { useState } from "react";
import { Popover } from "@heroui/react";
import {
  Check,
  Copy,
  Eye,
  EyeOff,
  Laptop,
  LoaderCircle,
  Lock,
  LockOpen,
  Monitor,
  RefreshCw,
} from "lucide-react";
import { api, formatCode, type Overview } from "../api";
import { writeClipboardText } from "../lib/clipboard";
import { Button } from "@/components/ui/button";
import { toast } from "@/components/ui/toast";
import { Tooltip, TooltipTrigger } from "@/components/ui/tooltip";
import { PlatformIcon } from "./PlatformIcon";
import { t } from "@/i18n";

function ServerStatus({ ov }: { ov: Overview | null }) {
  const state = ov?.server.state;
  const label = {
    none: t("未配置服务器"),
    connecting: t("连接中"),
    online: t("在线"),
    offline: t("离线"),
  };
  const status = (
    <span
      className="server-status"
      data-state={state ?? "loading"}
    >
      <span className="status-dot" />
      {state ? label[state] : t("加载中")}
    </span>
  );
  if (ov?.server.state !== "offline" || !ov.server.error) return status;

  return (
    <Tooltip content={ov.server.error}>
      <TooltipTrigger role="group" aria-label={t("服务器离线原因")}>{status}</TooltipTrigger>
    </Tooltip>
  );
}

export function LocalDevicePopover({
  ov,
  onRefresh,
  open,
  onOpenChange,
}: {
  ov: Overview | null;
  onRefresh: () => void;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const [refreshing, setRefreshing] = useState(false);
  const [locking, setLocking] = useState(false);
  const fixed = !!ov?.password_fixed;
  const [showPassword, setShowPassword] = useState(false);
  const [copied, setCopied] = useState<"code" | "password" | null>(null);

  const copy = async (text: string, id: "code" | "password") => {
    try {
      await writeClipboardText(text);
      setCopied(id);
      toast.success(id === "code" ? t("设备码已复制") : t("配对密码已复制"));
      window.setTimeout(
        () => setCopied((value) => (value === id ? null : value)),
        1800,
      );
    } catch {
      toast.error(t("复制失败，请手动选择并复制"));
    }
  };

  const refreshPassword = async () => {
    setRefreshing(true);
    try {
      await api.refreshPassword();
      onRefresh();
      toast.success(t("配对密码已更新"));
    } catch (e) {
      toast.error(t("更新失败：{{e}}", { e: String(e) }));
    } finally {
      setRefreshing(false);
    }
  };

  const toggleFixed = async () => {
    setLocking(true);
    try {
      await api.setPasswordFixed(!fixed);
      onRefresh();
      toast.success(fixed ? t("已取消固定，下次打开软件会换新密码") : t("配对密码已固定，重启也不变"));
    } catch (e) {
      toast.error(t("设置失败：{{e}}", { e: String(e) }));
    } finally {
      setLocking(false);
    }
  };

  return (
    <Popover
      isOpen={open}
      onOpenChange={(nextOpen) => {
        onOpenChange(nextOpen);
        if (!nextOpen) {
          setShowPassword(false);
          setCopied(null);
        }
      }}
    >
      <Button variant="ghost" size="icon-sm"
        aria-label={t("本机信息")} title={t("查看本机设备码和配对密码")}>
        <Monitor data-icon="inline-start" />
      </Button>
      <Popover.Content
        placement="bottom"
        offset={8}
        className="w-80 max-w-[calc(100vw-24px)]"
      >
        <Popover.Arrow />
        <Popover.Dialog aria-label={t("本机信息")} className="flex flex-col gap-4">
          <div className="flex items-center justify-between gap-3">
            <Popover.Heading>{t("本机信息")}</Popover.Heading>
            <ServerStatus ov={ov} />
          </div>
          <div className="flex flex-col gap-3">
            <div>
              <div className="credential-label">{t("设备码")}</div>
              <div className="credential-row">
                {ov?.code ? (
                  <span className="device-code">{formatCode(ov.code)}</span>
                ) : (
                  <span className="credential-placeholder">
                    {ov?.server.state === "none"
                      ? t("等待配置服务器")
                      : t("正在获取设备码…")}
                  </span>
                )}
                <Button
                  variant="ghost"
                  size="icon-sm"
                  disabled={!ov?.code}
                  aria-label={t("复制设备码")}
                  title={t("复制设备码")}
                  onClick={() => ov?.code && copy(ov.code, "code")}
                >
                  {copied === "code" ? <Check /> : <Copy />}
                </Button>
              </div>
            </div>
            <div>
              <div className="credential-label">{t("配对密码")}</div>
              <div className="credential-row">
                <span className="device-password">
                  {ov ? (showPassword ? ov.password : "••••••") : "—"}
                </span>
                <div className="flex shrink-0 items-center">
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    disabled={!ov}
                    aria-label={showPassword ? t("隐藏配对密码") : t("显示配对密码")}
                    title={showPassword ? t("隐藏配对密码") : t("显示配对密码")}
                    onClick={() => setShowPassword(!showPassword)}
                  >
                    {showPassword ? <EyeOff /> : <Eye />}
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    disabled={!ov}
                    aria-label={t("复制配对密码")}
                    title={t("复制配对密码")}
                    onClick={() => ov && copy(ov.password, "password")}
                  >
                    {copied === "password" ? <Check /> : <Copy />}
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    disabled={!ov || locking}
                    aria-label={fixed ? t("取消固定配对密码") : t("固定配对密码")}
                    aria-pressed={fixed}
                    title={fixed ? t("已固定，重启也不变；点击取消固定") : t("固定密码，重启也不变，对方以后用同一个密码就能连")}
                    onClick={toggleFixed}
                  >
                    {fixed ? <Lock className="text-accent" /> : <LockOpen />}
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    disabled={!ov || refreshing || fixed}
                    aria-label={t("刷新配对密码")}
                    title={fixed ? t("密码已固定，先取消固定才能换") : t("刷新密码，旧密码立即失效")}
                    onClick={refreshPassword}
                  >
                    {refreshing ? (
                      <LoaderCircle className="animate-spin" />
                    ) : (
                      <RefreshCw />
                    )}
                  </Button>
                </div>
              </div>
            </div>
          </div>
          <div className="device-caption">
            <Laptop />
            <span className="truncate">{ov?.name ?? t("正在读取本机信息")}</span>
            {ov && <PlatformIcon platform={ov.platform} />}
          </div>
        </Popover.Dialog>
      </Popover.Content>
    </Popover>
  );
}
