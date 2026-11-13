import type { DeviceStatus, PathInfo, RemoteInfo, TerminalSession } from "../api";
import { pathLabel } from "../api";
import { t } from "@/i18n";

/** 手机和一台设备之间的连接；一台设备只连一条 */
export interface Conn {
  connId: number;
  info: RemoteInfo;
  code: string | null;
}

/** 手机上开着的一个终端 */
export interface Term {
  key: number;
  connId: number;
  endpointId: string;
  /** 打开以后才知道；新建的终端一开始没有 */
  sessionId?: string;
  termId?: number;
  state: "opening" | "open" | "exited" | "closed";
  note: string | null;
}

/** 设备一行右下角那句状态，tone 决定颜色 */
export type Tone = "ok" | "relay" | "off" | "muted" | "busy";

export function presence(status: DeviceStatus | undefined, connected: boolean, path: PathInfo | undefined): { text: string; tone: Tone } {
  if (connected) return { text: `${t("已连接")} · ${pathLabel(path)}`, tone: path?.kind === "relay" ? "relay" : "ok" };
  if (!status) return { text: t("未知"), tone: "muted" };
  if (status.probing || status.state === "checking") return { text: t("探测中…"), tone: "muted" };
  if (status.state === "online") {
    const p = status.path;
    const rtt = p?.rtt_ms != null && p.kind !== "unknown" ? ` · ${p.rtt_ms} ms` : "";
    return { text: `${t("在线")}${rtt}`, tone: p?.kind === "relay" ? "relay" : "ok" };
  }
  if (status.state === "old_version") return { text: t("对方需要更新 P2PCLI"), tone: "relay" };
  return { text: t("离线"), tone: "off" };
}

/** 会话里有几个 AI 在等你确认 */
export function waitingCount(sessions: TerminalSession[] | null | undefined) {
  return sessions?.filter((s) => s.agent?.state === "waiting").length ?? 0;
}

/** /Users/me/Projects/x → ~/Projects/x，太长只留最后两段 */
export function shortPath(path: string | null | undefined) {
  if (!path) return "";
  const home = path.replace(/^\/(Users|home)\/[^/]+/, "~").replace(/^[A-Za-z]:\\Users\\[^\\]+/, "~");
  const parts = home.split(/[\\/]/).filter(Boolean);
  return parts.length > 3 ? `…/${parts.slice(-2).join("/")}` : home;
}
