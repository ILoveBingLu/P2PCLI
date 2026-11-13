import { ArrowRight, KeyRound, LoaderCircle, Monitor, Radar, ScanLine } from "lucide-react";
import logo from "@/assets/logo.svg";
import type { Overview } from "../api";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyTitle,
} from "@/components/ui/empty";
import { t } from "@/i18n";
import { isMobile } from "@/lib/platform";

export function Welcome({
  ov,
  onSettings,
  onConnect,
  onLocalDevice,
  onScanNearby,
  onScanCode,
  scanning,
}: {
  ov: Overview | null;
  onSettings: () => void;
  onConnect: () => void;
  onLocalDevice: () => void;
  onScanNearby: () => void;
  /** 手机：扫桌面端的二维码连接 */
  onScanCode: () => void;
  scanning: boolean;
}) {
  return (
    <div className="welcome-scroll">
      <Empty className="welcome">
        <img
          src={logo}
          className="size-14 shrink-0 object-contain"
          alt="P2PCLI"
          draggable={false}
        />
        <EmptyHeader>
          <EmptyTitle>
            <h2>{t("连接远程设备")}</h2>
          </EmptyTitle>
          <EmptyDescription>
            {isMobile
              ? t("在电脑上点标题栏的手机图标，用手机扫它显示的二维码；也可以输入设备码和配对密码。")
              : t("输入设备码和配对密码，开始远程终端会话。")}
          </EmptyDescription>
        </EmptyHeader>
        <EmptyContent>
          {isMobile && (
            <div className="welcome-mobile-actions">
              <Button size="lg" onClick={onScanCode}>
                <ScanLine data-icon="inline-start" />{t("扫码连接")}
              </Button>
              <Button size="lg" variant="outline" onClick={onConnect}>
                <KeyRound data-icon="inline-start" />{t("输入设备码")}
              </Button>
              <Button size="lg" variant="ghost" className="text-muted-foreground" onClick={onScanNearby} disabled={scanning}>
                {scanning ? <LoaderCircle data-icon="inline-start" className="animate-spin" /> : <Radar data-icon="inline-start" />}
                {t("附近设备")}
              </Button>
            </div>
          )}
          {!isMobile && <div className="flex flex-wrap items-center justify-center gap-2">
            <Button variant="ghost" className="text-muted-foreground" onClick={onLocalDevice}>
              <Monitor data-icon="inline-start" />{t("本机信息")}
            </Button>
            <Button onClick={onConnect}>
              {t("开始连接")}
              <ArrowRight data-icon="inline-end" />
            </Button>
            <Button variant="ghost" className="text-muted-foreground" onClick={onScanNearby} disabled={scanning}>
              {scanning ? <LoaderCircle data-icon="inline-start" className="animate-spin" /> : <Radar data-icon="inline-start" />}
              {t("附近设备")}
            </Button>
          </div>}
          {ov?.server.state === "none" && (
            <Alert>
              <AlertTitle>{t("先配置牵线服务器")}</AlertTitle>
              <AlertDescription>
                {t("配置后即可获取设备码。")}
                <Button variant="link" size="sm" onClick={onSettings}>
                  {t("打开设置")}
                  <ArrowRight data-icon="inline-end" />
                </Button>
              </AlertDescription>
            </Alert>
          )}
        </EmptyContent>
      </Empty>
    </div>
  );
}
