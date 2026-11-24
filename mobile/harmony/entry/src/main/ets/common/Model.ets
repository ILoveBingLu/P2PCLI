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

/** 对方电脑的一块屏幕 */
export interface ScreenDisplay {
  id: string;
  name: string;
  width: number;
  height: number;
  primary: boolean;
}

/** 对方同意看画面以后的情况 */
export interface ScreenStartedView {
  displays: ScreenDisplay[];
  display: string;
  /** 能不能操作：对方只给看、或者对方电脑没给辅助功能权限时为假 */
  control: boolean;
  control_note: string | null;
  /** 能直接输入文字（中文、表情）；对方是老版本时为假 */
  text_input: boolean;
}

/** 远程画面的状态变化，type 是 config、first_frame、cursor、cursor_shape、paused、resumed、switched、stats、closed、unsupported */
export interface ScreenEvent {
  type: string;
  width?: number;
  height?: number;
  x?: number;
  y?: number;
  visible?: boolean;
  id?: number;
  hot_x?: number;
  hot_y?: number;
  scale?: number;
  /** 指针图案：RGBA（不预乘透明度），base64 */
  rgba?: string;
  relay?: boolean;
  display?: string;
  bitrate?: number;
  fps?: number;
  reason?: string | null;
}

/** 发给对方的键鼠事件，坐标是相对屏幕的 0~1 */
export interface ScreenInput {
  k: string;
  x?: number;
  y?: number;
  button?: string;
  down?: boolean;
  dx?: number;
  dy?: number;
  code?: string;
}

export function parseError(e: Error): P2pError {
  try {
    return JSON.parse(e.message) as P2pError;
  } catch (_) {
    return { kind: 'failed', detail: e.message } as P2pError;
  }
}
