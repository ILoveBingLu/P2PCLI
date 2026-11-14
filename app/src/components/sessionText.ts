import type { TerminalSession } from "../api";
import { intlLocale, t } from "@/i18n";

/** 按创建时间给会话编号：会话 1、会话 2…；侧边栏和会话页用同一套名字 */
export function sessionLabels(sessions: TerminalSession[]): Map<string, string> {
  const ordered = [...sessions].sort((a, b) => (a.created_at ?? 0) - (b.created_at ?? 0) || a.id.localeCompare(b.id));
  return new Map(ordered.map((s, i) => [s.id, t("会话 {{value}}", { value: i + 1 })]));
}

/** 侧边栏、标签、状态栏上显示的会话名：跑着 AI 的用 AI 的名字；同一台设备上有两个同样的 AI，后面带上会话编号 */
export function sessionTitles(sessions: TerminalSession[]): Map<string, string> {
  const labels = sessionLabels(sessions);
  const counts = new Map<string, number>();
  for (const s of sessions) if (s.agent) counts.set(s.agent.name, (counts.get(s.agent.name) ?? 0) + 1);
  return new Map(sessions.map((s) => {
    const label = labels.get(s.id) ?? t("会话");
    if (!s.agent) return [s.id, label];
    return [s.id, (counts.get(s.agent.name) ?? 0) > 1 ? `${s.agent.name} · ${label}` : s.agent.name];
  }));
}

/** 会话编号的前 8 位：别的设备开在本机上的会话编号前面带着设备公钥，跳过它 */
export function shortSessionId(id: string): string {
  return id.slice(-32).slice(0, 8);
}

/** 别的设备开在本机上的会话：显示成“来自 xx”。设备名先用备注，没记过名字时说“其他设备” */
export function ownerLabel(session: TerminalSession, deviceRemark: (endpointId: string) => string): string | null {
  const owner = session.owner;
  if (!owner) return null;
  return t("来自 {{name}}", { name: deviceRemark(owner.endpoint_id).trim() || owner.name || t("其他设备") });
}

/** 刚刚、3 分钟前、2 小时前、昨天 19:30、10月20日 */
export function relativeTime(secs: number | null | undefined): string {
  if (!secs) return "";
  const then = new Date(secs * 1000);
  const diff = (Date.now() - then.getTime()) / 1000;
  if (diff < 60) return t("刚刚");
  if (diff < 3600) return t("{{value}} 分钟前", { value: Math.floor(diff / 60) });
  if (diff < 86400 && then.getDate() === new Date().getDate()) return t("{{value}} 小时前", { value: Math.floor(diff / 3600) });
  const hm = then.toLocaleTimeString(intlLocale(), { hour: "2-digit", minute: "2-digit" });
  const yesterday = new Date(Date.now() - 86400000);
  if (then.toDateString() === yesterday.toDateString()) return t("昨天 {{hm}}", { hm });
  return then.toLocaleDateString(intlLocale(), { month: "numeric", day: "numeric" });
}
