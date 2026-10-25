import { useState } from "react";
import { Popover } from "@heroui/react";
import {
  Check,
  Copy,
  Eye,
  EyeOff,
  Laptop,
  LoaderCircle,
  Monitor,
  RefreshCw,
} from "lucide-react";
import { api, formatCode, type Overview } from "../api";
import { Button } from "@/components/ui/button";
import { toast } from "@/components/ui/toast";
import { Tooltip, TooltipTrigger } from "@/components/ui/tooltip";
import { PlatformIcon } from "./PlatformIcon";

function ServerStatus({ ov }: { ov: Overview | null }) {
  const state = ov?.server.state;
  const label = {
    none: "未配置服务器",
    connecting: "连接中",
    online: "在线",
    offline: "离线",
  };
  const status = (
    <span
      className="server-status"
      data-state={state ?? "loading"}
    >
      <span className="status-dot" />
      {state ? label[state] : "加载中"}
    </span>
  );
  if (ov?.server.state !== "offline" || !ov.server.error) return status;

  return (
    <Tooltip content={ov.server.error}>
      <TooltipTrigger role="group" aria-label="服务器离线原因">{status}</TooltipTrigger>
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
  const [showPassword, setShowPassword] = useState(false);
  const [copied, setCopied] = useState<string | null>(null);

  const copy = async (text: string, what: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(what);
      toast.success(`${what}已复制`);
      window.setTimeout(
        () => setCopied((value) => (value === what ? null : value)),
        1800,
      );
    } catch {
      toast.error("复制失败，请手动选择并复制");
    }
  };

  const refreshPassword = async () => {
    setRefreshing(true);
    try {
      await api.refreshPassword();
      onRefresh();
      toast.success("临时密码已更新");
    } catch (e) {
      toast.error(`更新失败：${String(e)}`);
    } finally {
      setRefreshing(false);
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
        aria-label="本机设备" title="查看本机设备码和临时密码">
        <Monitor data-icon="inline-start" />
      </Button>
      <Popover.Content
        placement="bottom end"
        offset={8}
        className="w-80 max-w-[calc(100vw-24px)]"
      >
        <Popover.Arrow />
        <Popover.Dialog aria-label="本机设备信息" className="flex flex-col gap-4">
          <div className="flex items-center justify-between gap-3">
            <Popover.Heading>本机设备</Popover.Heading>
            <ServerStatus ov={ov} />
          </div>
          <div className="flex flex-col gap-3">
            <div>
              <div className="credential-label">设备码</div>
              <div className="credential-row">
                {ov?.code ? (
                  <span className="device-code">{formatCode(ov.code)}</span>
                ) : (
                  <span className="credential-placeholder">
                    {ov?.server.state === "none"
                      ? "等待配置服务器"
                      : "正在获取设备码…"}
                  </span>
                )}
                <Button
                  variant="ghost"
                  size="icon-sm"
                  disabled={!ov?.code}
                  aria-label="复制设备码"
                  title="复制设备码"
                  onClick={() => ov?.code && copy(ov.code, "设备码")}
                >
                  {copied === "设备码" ? <Check /> : <Copy />}
                </Button>
              </div>
            </div>
            <div>
              <div className="credential-label">临时密码</div>
              <div className="credential-row">
                <span className="device-password">
                  {ov ? (showPassword ? ov.password : "••••••") : "—"}
                </span>
                <div className="flex shrink-0 items-center">
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    disabled={!ov}
                    aria-label={showPassword ? "隐藏临时密码" : "显示临时密码"}
                    title={showPassword ? "隐藏临时密码" : "显示临时密码"}
                    onClick={() => setShowPassword(!showPassword)}
                  >
                    {showPassword ? <EyeOff /> : <Eye />}
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    disabled={!ov}
                    aria-label="复制临时密码"
                    title="复制临时密码"
                    onClick={() => ov && copy(ov.password, "临时密码")}
                  >
                    {copied === "临时密码" ? <Check /> : <Copy />}
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    disabled={!ov || refreshing}
                    aria-label="刷新临时密码"
                    title="刷新密码，旧密码立即失效"
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
            <span className="truncate">{ov?.name ?? "正在读取本机信息"}</span>
            {ov && <PlatformIcon platform={ov.platform} />}
          </div>
        </Popover.Dialog>
      </Popover.Content>
    </Popover>
  );
}
