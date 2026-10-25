import { useEffect, useRef } from "react";
import { api, type AgentState, type Overview } from "../api";
import { toast } from "@/components/ui/toast";
import { sessionLabels } from "./sessionText";

/** 远程会话里的 AI 停下来等你确认、或者干完活时提醒你。
 *  用后台探测的结果（连着的设备每 4 秒更新一次），侧边栏不展开也能提醒。
 *  窗口在后台发系统通知；在前台但没在看那个会话就弹提示，可以点过去；正在看就不打扰。
 *  设备刚上线时第一次看到的状态不算变化，打开软件不会弹一堆 */
export function useAgentAlerts(
  ov: Overview | null,
  /** 用户现在是不是正看着这个会话 */
  viewing: (endpointId: string, sessionId: string) => boolean,
  /** 这个会话开着标签的话，返回切过去的办法 */
  showSession: (endpointId: string, sessionId: string) => (() => void) | null,
) {
  const last = useRef(new Map<string, AgentState>());
  const latest = useRef({ viewing, showSession });
  latest.current = { viewing, showSession };

  useEffect(() => {
    if (!ov) return;
    const seen = new Set<string>();
    for (const device of ov.devices) {
      if (device.state !== "online" || !device.sessions_live) continue;
      const labels = sessionLabels(device.sessions);
      const deviceName = ov.recent.find((r) => r.endpoint_id === device.endpoint_id)?.name ?? "远程设备";
      for (const session of device.sessions) {
        if (!session.agent) continue;
        const key = `${device.endpoint_id}/${session.id}`;
        seen.add(key);
        const before = last.current.get(key);
        const now = session.agent.state;
        last.current.set(key, now);
        const done = before === "working" && now === "idle";
        if (!before || before === now || (now !== "waiting" && !done)) continue;

        const focused = document.visibilityState === "visible" && document.hasFocus();
        if (focused && latest.current.viewing(device.endpoint_id, session.id)) continue;
        const title = now === "waiting" ? `${session.agent.name} 在等你确认` : `${session.agent.name} 干完了`;
        const body = `${deviceName} · ${labels.get(session.id) ?? "会话"}`;
        if (!focused) {
          void api.notify(title, body).catch(() => {});
          continue;
        }
        const show = latest.current.showSession(device.endpoint_id, session.id);
        toast.notice(title, {
          description: body,
          warning: now === "waiting",
          action: show ? { label: "查看", onPress: show } : undefined,
        });
      }
    }
    // 不在了的会话忘掉，下次出现时从头算
    for (const key of [...last.current.keys()]) if (!seen.has(key)) last.current.delete(key);
  }, [ov]);
}
