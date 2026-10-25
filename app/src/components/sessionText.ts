import type { TerminalSession } from "../api";

/** 按创建时间给会话编号：会话 1、会话 2…；侧边栏和会话页用同一套名字 */
export function sessionLabels(sessions: TerminalSession[]): Map<string, string> {
  const ordered = [...sessions].sort((a, b) => (a.created_at ?? 0) - (b.created_at ?? 0) || a.id.localeCompare(b.id));
  return new Map(ordered.map((s, i) => [s.id, `会话 ${i + 1}`]));
}

/** 刚刚、3 分钟前、2 小时前、昨天 19:30、10月20日 */
export function relativeTime(secs: number | null | undefined): string {
  if (!secs) return "";
  const then = new Date(secs * 1000);
  const diff = (Date.now() - then.getTime()) / 1000;
  if (diff < 60) return "刚刚";
  if (diff < 3600) return `${Math.floor(diff / 60)} 分钟前`;
  if (diff < 86400 && then.getDate() === new Date().getDate()) return `${Math.floor(diff / 3600)} 小时前`;
  const hm = then.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" });
  const yesterday = new Date(Date.now() - 86400000);
  if (then.toDateString() === yesterday.toDateString()) return `昨天 ${hm}`;
  return then.toLocaleDateString("zh-CN", { month: "numeric", day: "numeric" });
}
