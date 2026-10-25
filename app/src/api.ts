// 和 Rust 端对应的类型与调用
import { invoke } from "@tauri-apps/api/core";

export type PathKind = "ipv6" | "ipv4" | "relay" | "unknown";
export interface PathInfo {
  kind: PathKind;
  rtt_ms: number | null;
  remote: string | null;
}

export interface RemoteInfo {
  endpoint_id: string;
  name: string;
  platform: string;
  user: string;
}

export interface TerminalSession {
  id: string;
  created_at: number;
  attached: boolean;
}

export interface OpenedTerminal { term_id: number; session_id: string }

export interface Connected {
  conn_id: number;
  code: string | null;
  info: RemoteInfo;
  path: PathInfo;
}

export type RejectCode = "need_password" | "bad_password" | "denied" | "timeout" | "locked" | "version";
export interface ConnectFail {
  reject: RejectCode | null;
  message: string;
}

export interface SessionSummary {
  session: number;
  remote: string;
  name: string;
  platform: string;
  terminals: number;
  since: number;
}

export interface RecentDevice {
  code: string;
  endpoint_id: string;
  name: string;
  platform: string;
  last_used: number;
}

export interface TrustedView {
  endpoint_id: string;
  name: string;
  platform: string;
  added_at: number;
}

export type ServerState =
  | { state: "none" }
  | { state: "connecting" }
  | { state: "online" }
  | { state: "offline"; error: string };

export interface Overview {
  name: string;
  platform: string;
  endpoint_id: string;
  code: string | null;
  password: string;
  /** 配对密码固定了，重启也不变 */
  password_fixed: boolean;
  server: ServerState;
  sessions: SessionSummary[];
  recent: RecentDevice[];
  /** 局域网里发现的设备 */
  nearby: NearbyDevice[];
  trusted: TrustedView[];
}

export interface NearbyDevice {
  endpoint_id: string;
  name: string;
  platform: string;
}

export interface IncomingRequest {
  id: number;
  remote: string;
  name: string;
  platform: string;
}

export type HostEvent =
  | { type: "request_cancelled"; id: number }
  | { type: "connected"; session: number; remote: string; name: string; platform: string; trusted: boolean }
  | { type: "disconnected"; session: number }
  | { type: "terminal_opened"; session: number }
  | { type: "terminal_closed"; session: number };

export type TermStatus = { type: "exited"; code: number | null } | { type: "closed"; reason: string | null };

export interface SettingsView {
  proxy: string | null;
  server: string | null;
  device_name: string | null;
  relay: string | null;
  effective_server: string | null;
  effective_relay: string | null;
  default_server: string;
  server_presets: ServerPreset[];
  lan_visible: boolean;
}

export interface ServerPreset {
  address: string;
  label: string;
  note: string;
}

export type NatKind = "none" | "cone" | "symmetric" | "unknown";
export interface Diagnosis {
  rating: "好" | "一般" | "差" | "未知";
  summary: string;
  ipv6_addrs: string[];
  ipv4_public: string | null;
  nat: NatKind;
  nat_source: "stun" | "relay" | "none";
  stun_error: string | null;
  relay: string | null;
  relay_connected: boolean;
  relay_latency_ms: number | null;
  notes: string[];
}

export const api = {
  overview: () => invoke<Overview>("overview"),
  scanNearby: () => invoke<NearbyDevice[]>("scan_nearby"),
  diagnose: () => invoke<Diagnosis>("diagnose"),
  refreshPassword: () => invoke<string>("refresh_password"),
  setPasswordFixed: (fixed: boolean) => invoke("set_password_fixed", { fixed }),
  respond: (id: number, allow: boolean, trust: boolean) => invoke("respond_request", { id, allow, trust }),
  kick: (session: number) => invoke<boolean>("kick", { session }),
  removeTrusted: (endpointId: string) => invoke("remove_trusted", { endpointId }),
  forgetRecent: (code: string) => invoke("forget_recent", { code }),
  connect: (target: string, password: string | null) => invoke<Connected>("connect", { target, password }),
  disconnect: (connId: number) => invoke("disconnect", { connId }),
  pathInfo: (connId: number) => invoke<PathInfo>("path_info", { connId }),
  listTerminals: (connId: number) => invoke<TerminalSession[]>("list_terminals", { connId }),
  endTerminal: (connId: number, sessionId: string) => invoke("end_terminal", { connId, sessionId }),
  getSettings: () => invoke<SettingsView>("get_settings"),
  saveSettings: (server: string | null, deviceName: string | null, relay: string | null, proxy: string | null, lanVisible: boolean) =>
    invoke("save_settings", { server, deviceName, relay, proxy, lanVisible }),
  restart: () => invoke("restart"),
};

export function formatCode(code: string | null | undefined): string {
  if (!code || code.length !== 9) return code ?? "";
  return `${code.slice(0, 3)} - ${code.slice(3, 6)} - ${code.slice(6)}`;
}

export function platformLabel(p: string): string {
  return ({ macos: "macOS", windows: "Windows", linux: "Linux" } as Record<string, string>)[p] ?? p;
}

export function pathLabel(p: PathInfo | null | undefined): string {
  if (!p) return "连接中";
  const name = { ipv6: "IPv6 直连", ipv4: "IPv4 直连", relay: "中继", unknown: "连接中" }[p.kind];
  return p.rtt_ms != null && p.kind !== "unknown" ? `${name} · ${p.rtt_ms} ms` : name;
}
