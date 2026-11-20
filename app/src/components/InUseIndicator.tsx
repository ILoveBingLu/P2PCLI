import { Popover } from "@heroui/react";
import { MonitorX, Unplug } from "lucide-react";
import type { ScreenViewer, SessionSummary } from "../api";
import { Button } from "@/components/ui/button";
import { PlatformIcon } from "./PlatformIcon";
import { t } from "@/i18n";

/** 连了多久：since 是对方连上时的秒数 */
function connectedFor(since: number) {
  const minutes = Math.floor((Date.now() / 1000 - since) / 60);
  if (minutes < 1) return t("刚连上");
  if (minutes < 60) return t("连了 {{minutes}} 分钟", { minutes });
  const hours = Math.floor(minutes / 60);
  return minutes % 60 ? t("连了 {{hours}} 小时 {{value}} 分钟", { hours, value: minutes % 60 }) : t("连了 {{hours}} 小时", { hours });
}

/** 别人正连着本机用终端或看本机画面时，标题栏右边亮一个琥珀色的提示，点开看是谁、断开谁。没人连就不显示 */
export function InUseIndicator({ sessions, screens = [], onKick, onStopScreen }: {
  sessions: SessionSummary[];
  /** 正在看本机画面的 */
  screens?: ScreenViewer[];
  onKick: (session: SessionSummary) => void;
  onStopScreen?: (viewer: ScreenViewer) => void;
}) {
  if (!sessions.length) return null;
  const names = sessions.map((s) => s.name).join("、");
  const watching = screens.length > 0;
  const label = watching
    ? screens.length === 1 ? t("{{value}} 在看本机画面", { value: screens[0].name }) : t("{{length}} 台设备在看本机画面", { length: screens.length })
    : sessions.length === 1 ? t("{{value}} 在用本机", { value: sessions[0].name }) : t("{{length}} 台设备在用本机", { length: sessions.length });
  return (
    <Popover>
      <Button variant="ghost" size="sm" className="titlebar-in-use" data-screen={watching ? "" : undefined} aria-label={t("本机终端正在被远程使用：{{names}}", { names })} title={t("{{names}} 正在用本机的终端", { names })}>
        <span className="titlebar-in-use-dot" aria-hidden="true" />
        <span className="truncate">{label}</span>
      </Button>
      <Popover.Content placement="bottom" offset={8} className="w-80 max-w-[calc(100vw-24px)]">
        <Popover.Arrow />
        <Popover.Dialog aria-label={t("正在用本机的设备")} className="flex flex-col gap-3">
          {watching && (
            <>
              <Popover.Heading>{t("本机画面正在被查看")}</Popover.Heading>
              <ul className="in-use-list">
                {screens.map((v) => (
                  <li key={v.id} className="in-use-item">
                    <span className="titlebar-in-use-dot" aria-hidden="true" />
                    <div className="min-w-0 flex-1">
                      <div className="truncate font-medium">{v.name}</div>
                      <div className="in-use-meta">
                        {v.control ? t("查看并操作") : t("只看")} · {connectedFor(v.since)}
                      </div>
                    </div>
                    <Button variant="outline" size="sm" onPress={() => onStopScreen?.(v)}>
                      <MonitorX data-icon="inline-start" />
                      {t("停止")}
                    </Button>
                  </li>
                ))}
              </ul>
            </>
          )}
          <Popover.Heading>{t("本机终端正在被远程使用")}</Popover.Heading>
          <ul className="in-use-list">
            {sessions.map((s) => (
              <li key={s.session} className="in-use-item">
                <PlatformIcon platform={s.platform} />
                <div className="min-w-0 flex-1">
                  <div className="truncate font-medium">{s.name}</div>
                  <div className="in-use-meta">
                    {s.terminals} {t("个终端 ·")} {connectedFor(s.since)}
                  </div>
                </div>
                <Button variant="outline" size="sm" onPress={() => onKick(s)}>
                  <Unplug data-icon="inline-start" />
                  {t("断开")}
                </Button>
              </li>
            ))}
          </ul>
        </Popover.Dialog>
      </Popover.Content>
    </Popover>
  );
}
