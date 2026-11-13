// Rust 那边交过来的数据（JSON），字段名和桌面前端一样是下划线命名。和 crates/mobile-ffi/src/types.rs 对应

export interface MobileConfig {
  data_dir: string;
  inbox_dir: string | null;
  device_name: string | null;
  server: string | null;
  relay: string | null;
  proxy: string | null;
  app_version: string | null;
  platform: string | null;
}

export interface PathView {
  kind: string; // ipv6 | ipv4 | relay | local | unknown
  rtt_ms: number | null;
}

export interface ApprovalChoice {
  id: number;
  label: string;
}

export interface ApprovalView {
  id: string;
  context: string;
  options: ApprovalChoice[];
}

export interface AgentView {
  kind: string;
  name: string;
  state: string; // working | waiting | idle
  stalled: boolean;
  approval: ApprovalView | null;
}

export interface SessionView {
  id: string;
  created_at: number;
  attached: boolean;
  command: string | null;
  current_path: string | null;
  shell_ready: boolean;
  agent: AgentView | null;
  remark: string | null;
}

/** 对方电脑上装了的一个命令行 AI */
export interface InstalledAgentView {
  kind: string;
  name: string;
}

/** 对方电脑上一个目录里的子目录 */
export interface DirListingView {
  path: string;
  parent: string | null;
  dirs: string[];
  truncated: boolean;
}

export interface DeviceView {
  endpoint_id: string;
  code: string | null;
  name: string;
  remark: string | null;
  platform: string;
  known: boolean;
  presence: string; // checking | online | offline | old_version
  path: PathView | null;
  lan_online: boolean;
  sessions: SessionView[];
  sessions_live: boolean;
}

export interface ServerState {
  state: string; // none | connecting | online | offline
  error?: string;
}

export interface Overview {
  name: string;
  endpoint_id: string;
  server: ServerState;
  devices: DeviceView[];
  nearby: DeviceView[];
  inbox_dir: string;
}

export interface Connected {
  conn_id: number;
  endpoint_id: string;
  name: string;
  platform: string;
  code: string | null;
  path: PathView;
}

export interface TransferView {
  id: string;
  outgoing: boolean;
  peer_name: string;
  label: string;
  files: number;
  total: number;
  done: number;
  state: string; // connecting | waiting | running | done | failed
  error: string | null;
  dir: string | null;
}

export interface FileRequestView {
  id: number;
  name: string;
  platform: string;
  files: number;
  total: number;
  items: string[];
}

export interface PairInfo {
  code: string | null;
  password: string;
  id: string;
  name: string;
}

export interface MobileSettings {
  device_name: string | null;
  server: string | null;
  relay: string | null;
  proxy: string | null;
}

export interface ServerPreset {
  address: string;
  label: string;
  note: string;
}

export interface SettingsView {
  settings: MobileSettings;
  default_device_name: string;
  default_server: string;
  server_presets: ServerPreset[];
  inbox_dir: string;
  endpoint_id: string;
  app_version: string | null;
}

/** 后台变化：type 是 overview_changed、disconnected、transfers、file_request、file_request_cancelled */
export interface AppEvent {
  type: string;
  conn_id?: number;
  reason?: string;
  transfers?: TransferView[];
  request?: FileRequestView;
  id?: number;
}

/** Rust 抛的错误（Error.message 里的 JSON） */
export interface P2pError {
  kind: string; // rejected | disconnected | failed
  reason?: string; // need_password | bad_password | denied | timeout | locked | version | controller_only
  detail: string;
}

export function parseError(e: Error): P2pError {
  try {
    return JSON.parse(e.message) as P2pError;
  } catch (_) {
    return { kind: 'failed', detail: e.message } as P2pError;
  }
}
