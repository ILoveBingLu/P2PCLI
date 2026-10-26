// 和 Rust 端对应的类型与调用
import { Channel, invoke } from "@tauri-apps/api/core";

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
  /** 对方的软件版本、系统、会话管理器；旧版本不告诉 */
  version?: string | null;
  os?: string | null;
  multiplexer?: string | null;
}

/** 连接信息卡片用的统计；字节数、包数从连上开始累计，界面自己算速率 */
export interface ConnStats {
  path: PathInfo;
  /** 本机这头的地址：IPv6 直连一般是公网地址，IPv4 多半是内网地址 */
  local: string | null;
  /** 本机对外的 IPv4 公网地址 */
  public_v4: string | null;
  sent_bytes: number;
  recv_bytes: number;
  sent_packets: number;
  lost_packets: number;
}

export interface TerminalSession {
  id: string;
  created_at: number;
  attached: boolean;
  details?: {
    shell_ready?: boolean;
    window_index: number | null;
    window_name: string | null;
    window_count: number | null;
    command: string | null;
    current_path: string | null;
  } | null;
  /** 会话里跑着的命令行 AI；没认出来、或者对方是旧版本时没有 */
  agent?: AgentInfo | null;
}

export interface InstalledAgent { kind: string; name: string; command: string }

/** 同一条连接共享的会话快照；读取失败后不能再把旧 AI 状态当成实时状态。 */
export interface TerminalSessionSnapshot {
  sessions: TerminalSession[] | null;
  error: string | null;
}

/** AI 在干什么：干活中、等你确认、空闲 */
export type AgentState = "working" | "waiting" | "idle";
export interface AgentApproval {
  id: string;
  context: string;
  options: { id: number; label: string }[];
}
export interface AgentInfo {
  /** claude、codex 这种稳定标识，按它配图标 */
  kind: string;
  name: string;
  state: AgentState;
  /** 干活画面连续多少秒没有变化；达到疑似卡住阈值才提供，不代表进程已经退出 */
  unchanged_for?: number | null;
  /** 那台电脑上这种 AI 账号的额度；读不到就没有 */
  quota?: AgentQuota | null;
  /** 这个会话里的 AI 现在的模型、思考强度、权限模式；目前只有 Claude Code 有 */
  settings?: AgentSettings | null;
  approval?: AgentApproval | null;
}
/** 读不到的项没有 */
export interface AgentSettings {
  /** 显示用的名字，比如 Opus 5.5 */
  model?: string | null;
  /** low、medium、high、xhigh、max、auto */
  effort?: string | null;
  /** 权限模式，各家自己的叫法。Claude Code：default、acceptEdits、plan、auto、bypassPermissions、dontAsk；ZCode：build、edit、yolo、plan */
  mode?: string | null;
  /** 能切到的模型；Claude Code 的别名固定写在界面里，ZCode 按那台电脑上的配置来 */
  models?: { id: string; name: string }[];
  /** 当前模型能用的思考强度，同样只有按配置来的才有 */
  efforts?: string[];
}
export interface AgentQuota {
  windows: QuotaWindow[];
  /** 数据是什么时候记下的（秒） */
  at: number;
}
export interface QuotaWindow {
  /** 窗口多长：5 小时是 300，7 天是 10080 */
  minutes: number;
  used_percent: number;
  /** 什么时候重置（秒） */
  resets_at: number | null;
}

export interface OpenedTerminal { term_id: number; session_id: string }

export interface Connected {
  conn_id: number;
  code: string | null;
  info: RemoteInfo;
  path: PathInfo;
  /** 连接时勾了互相信任并且已经记下 */
  mutual: boolean;
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
  remarks: Record<string, { device: string; sessions: Record<string, string> }>;
  /** 局域网里发现的设备 */
  nearby: NearbyDevice[];
  /** 连接历史里各台设备的在线、线路、延迟、会话列表 */
  devices: DeviceStatus[];
  trusted: TrustedView[];
  /** 同意过一次的设备：能在后台看延迟和自己的会话列表 */
  paired: TrustedView[];
  /** 收到的文件放在哪 */
  inbox_dir: string;
}

export type PresenceState = "checking" | "online" | "offline" | "old_version";

export interface DeviceStatus {
  endpoint_id: string;
  state: PresenceState;
  /** 正在建立探测连接或读取探测结果 */
  probing: boolean;
  path: PathInfo | null;
  /** 对方认不认我；还没探测到是 null */
  paired: boolean | null;
  /** 刚从对方拿到的，或者上次记住的 */
  sessions: TerminalSession[];
  sessions_live: boolean;
  sessions_at: number | null;
}

export interface NearbyDevice {
  endpoint_id: string;
  name: string;
  platform: string;
  online: boolean;
}

export interface IncomingRequest {
  id: number;
  remote: string;
  name: string;
  platform: string;
  /** 本机连过的设备反过来要连本机：没要配对密码，只靠主人点同意 */
  known: boolean;
}

/** 没配对过的设备要发文件过来 */
export interface FileRequest {
  id: number;
  remote: string;
  name: string;
  platform: string;
  files: number;
  total: number;
  /** 前几项的名字 */
  items: string[];
}

export type TransferState = "connecting" | "waiting" | "running" | "done" | "failed";
export interface Transfer {
  id: string;
  direction: "send" | "receive";
  peer: string;
  peer_name: string;
  label: string;
  files: number;
  total: number;
  done: number;
  state: TransferState;
  error: string | null;
  /** 存在接收方哪个目录、第一层每一项的路径，都是接收方电脑上的路径 */
  dir: string | null;
  paths: string[];
  started_at: number;
  finished_at: number | null;
}

/** 终端里用的命令行工具 p2pcli */
export interface CliStatus {
  path: string | null;
  installed: boolean;
  on_path: boolean;
  /** 开发版没带 */
  bundled: boolean;
}

/** 一个 AI 的使用说明现在什么情况 */
export type AiHintState = "missing" | "current" | "outdated" | "foreign";

/** 给本机的 AI 装的 p2pcli 使用说明：检测到的每个位置 */
export interface AiHintRow {
  /** 读这个位置的 AI */
  ais: string[];
  /** SKILL.md 的完整路径，操作时用它指定是哪一行 */
  path: string;
  state: AiHintState;
}

export interface AiHintStatus {
  /** 启动时自动安装和更新 */
  auto: boolean;
  rows: AiHintRow[];
}

export type HostEvent =
  | { type: "request_cancelled"; id: number }
  | { type: "files_received"; from: string; from_name: string; files: number; total: number; dir: string; paths: string[] }
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
  /** 本机作为被控端时，各 AI 的自定义启动命令 */
  agent_commands: Record<string, string>;
  /** 收到的文件放在哪 */
  inbox_dir: string;
}

export type SystemPermissionStatus = "granted" | "denied" | "unknown";
export interface SystemPermission {
  id: string;
  name: string;
  description: string;
  status: SystemPermissionStatus;
  can_request: boolean;
  settings_url: string | null;
}

/** 发布页上比本机新的版本 */
export interface UpdateView {
  version: string;
  current_version: string;
  notes: string | null;
}

export type UpdateProgress =
  | { event: "downloading"; downloaded: number; total: number | null }
  | { event: "installing" };

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
  showLogs: () => invoke<void>("show_logs"),
  refreshPassword: () => invoke<string>("refresh_password"),
  setPasswordFixed: (fixed: boolean) => invoke("set_password_fixed", { fixed }),
  removePaired: (endpointId: string) => invoke("remove_paired", { endpointId }),
  refreshDevices: () => invoke("refresh_devices"),
  /** 发一条系统通知 */
  notify: (title: string, body: string) => invoke("notify", { title, body }),
  checkUpdate: () => invoke<UpdateView | null>("check_update"),
  /** 下载、安装并重启；成功时软件会直接重启，不会返回 */
  installUpdate: (onProgress: (p: UpdateProgress) => void) => {
    const progress = new Channel<UpdateProgress>();
    progress.onmessage = onProgress;
    return invoke("install_update", { progress });
  },
  respond: (id: number, allow: boolean, trust: boolean) => invoke("respond_request", { id, allow, trust }),
  respondFiles: (id: number, accept: boolean) => invoke("respond_files", { id, accept }),
  /** 在后台发文件，返回传输编号；target 是设备公钥、设备码或名字 */
  sendFiles: (target: string, paths: string[]) => invoke<string>("send_files", { target, paths }),
  transfers: () => invoke<Transfer[]>("transfers"),
  /** 系统选择框，没选返回空 */
  pickPaths: (folders: boolean) => invoke<string[]>("pick_paths", { folders }),
  revealPath: (path: string) => invoke("reveal_path", { path }),
  openFolder: (path: string) => invoke("open_folder", { path }),
  cliStatus: () => invoke<CliStatus>("cli_status"),
  installCli: () => invoke<CliStatus>("install_cli"),
  /** 在默认浏览器里打开本项目的 GitHub 页面，别的链接后端不开 */
  openUrl: (url: string) => invoke<void>("open_url", { url }),
  aiHintStatus: () => invoke<AiHintStatus>("ai_hint_status"),
  setAiHintAuto: (enabled: boolean) => invoke<AiHintStatus>("set_ai_hint_auto", { enabled }),
  /** action 是 install 或 remove；path 不写就是全部 */
  aiHintApply: (action: "install" | "remove", path?: string) => invoke<AiHintStatus>("ai_hint_apply", { action, path }),
  termInput: (termId: number, data: string) => invoke("term_input", { termId, data, binary: false }),
  kick: (session: number) => invoke<boolean>("kick", { session }),
  removeTrusted: (endpointId: string) => invoke("remove_trusted", { endpointId }),
  forgetRecent: (code: string) => invoke("forget_recent", { code }),
  setDeviceRemark: (endpointId: string, remark: string) => invoke<string>("set_device_remark", { endpointId, remark }),
  setSessionRemark: (endpointId: string, sessionId: string, remark: string) =>
    invoke<string>("set_session_remark", { endpointId, sessionId, remark }),
  connect: (target: string, password: string | null, mutual = false) => invoke<Connected>("connect", { target, password, mutual }),
  disconnect: (connId: number) => invoke("disconnect", { connId }),
  pathInfo: (connId: number) => invoke<PathInfo>("path_info", { connId }),
  listTerminals: (connId: number) => invoke<TerminalSession[]>("list_terminals", { connId }),
  connectionStats: (connId: number) => invoke<ConnStats>("connection_stats", { connId }),
  installedAgents: (connId: number, refresh = false) => invoke<InstalledAgent[]>("installed_agents", { connId, refresh }),
  localInstalledAgents: (refresh = false) => invoke<InstalledAgent[]>("local_installed_agents", { refresh }),
  launchAgent: (connId: number, sessionId: string, kind: string, allowBypass = false) =>
    invoke<void>("launch_agent", { connId, sessionId, kind, allowBypass }),
  chooseAgentOption: (connId: number, sessionId: string, approvalId: string, optionId: number) =>
    invoke<void>("choose_agent_option", { connId, sessionId, approvalId, optionId }),
  endTerminal: (connId: number, sessionId: string) => invoke("end_terminal", { connId, sessionId }),
  getSettings: () => invoke<SettingsView>("get_settings"),
  systemPermissions: () => invoke<SystemPermission[]>("system_permissions"),
  requestSystemPermission: (permission: string) => invoke<SystemPermission[]>("request_system_permission", { permission }),
  openSystemSettings: (permission: string) => invoke<void>("open_system_settings", { permission }),
  saveSettings: (server: string | null, deviceName: string | null, relay: string | null, proxy: string | null, lanVisible: boolean, agentCommands: Record<string, string>) =>
    invoke("save_settings", { server, deviceName, relay, proxy, lanVisible, agentCommands }),
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

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(1)} ${units[unit]}`;
}

/** 把路径写成对方终端能直接用的样子：普通字符原样，带空格、中文这些就加单引号（PowerShell 和 sh 都认） */
export function shellQuote(path: string, platform: string): string {
  if (/^[\w@%+=:,./~-]+$/.test(path) && !(platform === "windows" && path.includes("~"))) return path;
  return platform === "windows" ? `'${path.replace(/'/g, "''")}'` : `'${path.replace(/'/g, `'\\''`)}'`;
}
