import { useEffect, useRef } from "react";
import { api, type AgentState, type Overview } from "../api";
import { toast } from "@/components/ui/toast";
import { QUOTA_ALERT, quotaWindowLabel, resetText } from "./Agent";
import { sessionLabels } from "./sessionText";

const focused = () => document.visibilityState === "visible" && document.hasFocus();

/** 窗口在后台发系统通知；在前台弹提示，开着那个会话的标签就带“查看”按钮 */
function notify(title: string, body: string, warning: boolean, show: (() => void) | null) {
  if (!focused()) {
    void api.notify(title, body).catch(() => {});
    return;
  }
  toast.notice(title, { description: body, warning, action: show ? { label: "查看", onPress: show } : undefined });
}

/** 远程会话里的 AI 停下来等你确认、干完活、或者账号额度快用完时提醒你。
 *  用后台探测的结果（连着的设备每 4 秒更新一次），侧边栏不展开也能提醒。
 *  窗口在后台发系统通知；在前台但没在看那个会话就弹提示，可以点过去；正在看就不打扰。
 *  设备刚上线时第一次看到的状态不算变化，打开软件不会弹一堆。
 *  额度按电脑和 AI 算：用到 90%、用满各提醒一次，同一个窗口重置之前不再提醒；正看着那个会话也提醒 */
export function useAgentAlerts(
  ov: Overview | null,
  /** 用户现在是不是正看着这个会话 */
  viewing: (endpointId: string, sessionId: string) => boolean,
  /** 这个会话开着标签的话，返回切过去的办法 */
  showSession: (endpointId: string, sessionId: string) => (() => void) | null,
) {
  const last = useRef(new Map<string, AgentState>());
  const quotaAlerted = useRef(new Set<string>());
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
        const agent = session.agent;
        for (const w of agent.quota?.windows ?? []) {
          const level = w.used_percent >= 100 ? 100 : w.used_percent >= QUOTA_ALERT ? QUOTA_ALERT : 0;
          const quotaKey = `${device.endpoint_id}/${agent.kind}/${w.minutes}/${w.resets_at ?? 0}/${level}`;
          if (!level || quotaAlerted.current.has(quotaKey)) continue;
          quotaAlerted.current.add(quotaKey);
          const label = quotaWindowLabel(w.minutes);
          notify(
            level === 100 ? `${agent.name} ${label}额度用完了` : `${agent.name} ${label}额度用了 ${Math.round(w.used_percent)}%`,
            `${deviceName} · ${resetText(w.resets_at)}`,
            true,
            null,
          );
        }
        const key = `${device.endpoint_id}/${session.id}`;
        seen.add(key);
        const before = last.current.get(key);
        const now = session.agent.state;
        last.current.set(key, now);
        const done = before === "working" && now === "idle";
        if (!before || before === now || (now !== "waiting" && !done)) continue;

        if (focused() && latest.current.viewing(device.endpoint_id, session.id)) continue;
        notify(
          now === "waiting" ? `${agent.name} 在等你确认` : `${agent.name} 干完了`,
          `${deviceName} · ${labels.get(session.id) ?? "会话"}`,
          now === "waiting",
          latest.current.showSession(device.endpoint_id, session.id),
        );
      }
    }
    // 不在了的会话忘掉，下次出现时从头算；额度提醒过的记着，重置以后窗口的重置时间变了自然会再提醒
    for (const key of [...last.current.keys()]) if (!seen.has(key)) last.current.delete(key);
  }, [ov]);
}
