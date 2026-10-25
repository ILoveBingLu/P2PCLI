import { Popover } from "@heroui/react";
import { Unplug } from "lucide-react";
import type { SessionSummary } from "../api";
import { Button } from "@/components/ui/button";
import { PlatformIcon } from "./PlatformIcon";

/** 连了多久：since 是对方连上时的秒数 */
function connectedFor(since: number) {
  const minutes = Math.floor((Date.now() / 1000 - since) / 60);
  if (minutes < 1) return "刚连上";
  if (minutes < 60) return `连了 ${minutes} 分钟`;
  const hours = Math.floor(minutes / 60);
  return minutes % 60 ? `连了 ${hours} 小时 ${minutes % 60} 分钟` : `连了 ${hours} 小时`;
}

/** 别人正连着本机用终端时，标题栏右边亮一个琥珀色的提示，点开看是谁、断开谁。没人连就不显示 */
export function InUseIndicator({ sessions, onKick }: {
  sessions: SessionSummary[];
  onKick: (session: SessionSummary) => void;
}) {
  if (!sessions.length) return null;
  const names = sessions.map((s) => s.name).join("、");
  const label = sessions.length === 1 ? `${sessions[0].name} 在用本机` : `${sessions.length} 台设备在用本机`;
  return (
    <Popover>
      <Button variant="ghost" size="sm" className="titlebar-in-use" aria-label={`本机终端正在被远程使用：${names}`} title={`${names} 正在用本机的终端`}>
        <span className="titlebar-in-use-dot" aria-hidden="true" />
        <span className="truncate">{label}</span>
      </Button>
      <Popover.Content placement="bottom" offset={8} className="w-80 max-w-[calc(100vw-24px)]">
        <Popover.Arrow />
        <Popover.Dialog aria-label="正在用本机的设备" className="flex flex-col gap-3">
          <Popover.Heading>本机终端正在被远程使用</Popover.Heading>
          <ul className="in-use-list">
            {sessions.map((s) => (
              <li key={s.session} className="in-use-item">
                <PlatformIcon platform={s.platform} />
                <div className="min-w-0 flex-1">
                  <div className="truncate font-medium">{s.name}</div>
                  <div className="in-use-meta">
                    {s.terminals} 个终端 · {connectedFor(s.since)}
                  </div>
                </div>
                <Button variant="outline" size="sm" onPress={() => onKick(s)}>
                  <Unplug data-icon="inline-start" />
                  断开
                </Button>
              </li>
            ))}
          </ul>
        </Popover.Dialog>
      </Popover.Content>
    </Popover>
  );
}
