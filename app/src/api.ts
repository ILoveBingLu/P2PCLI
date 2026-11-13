// 和 Rust 端对应的类型与调用
import { Channel, invoke } from "@tauri-apps/api/core";
import { unwrapCollaboration, type CollaborationAction, type CollaborationReply, type CollaborationDelivery } from "./lib/collaboration";
import { t } from "@/i18n";
import { isMobile } from "./lib/platform";
import { pickPhoneFiles, stagePhoneFiles } from "./lib/phoneFiles";

export type PathKind = "ipv6" | "ipv4" | "relay" | "local" | "unknown";
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

/** 对方电脑上一个目录的子目录 */
export interface DirListing {
  path: string;
  /** 已经是根目录时没有 */
  parent: string | null;
  dirs: string[];
  /** 子目录太多只回了一部分 */
  truncated: boolean;
}

/** 远程编辑：目录里的一项 */
export interface FsEntry {
  name: string;
  dir: boolean;
  size: number;
}
export interface EntryListing {
  path: string;
  parent: string | null;
  /** 目录在前、文件在后 */
  entries: FsEntry[];
  truncated: boolean;
}
/** 远程编辑：打开或保存后的文件信息 */
export interface FileMeta {
  path: string;
  /** 文件原始字节的 BLAKE3，保存时核对有没有被别人改过 */
  hash: string;
  size: number;
  /** 文件带 UTF-8 BOM；内容里不带，保存时后端加回去 */
  bom: boolean;
  readonly: boolean;
  mtime: number | null;
}
export interface OpenedFile { meta: FileMeta; text: string }
/** 远程编辑第二阶段：搜索、找文件、看文件变没变、文件操作、Git 状态 */
export type EditRequest =
  | { op: "search"; root: string; query: string; regex: boolean; case_sensitive: boolean; whole_word: boolean }
  | { op: "list_files"; root: string }
  | { op: "stat"; paths: string[] }
  | { op: "create_file"; path: string }
  | { op: "create_dir"; path: string }
  | { op: "rename"; from: string; to: string }
  | { op: "delete"; path: string }
  | { op: "git_status"; root: string }
  // Git 面板，需要对方有 git_v1；paths 相对仓库根目录
  | { op: "git_stage"; root: string; paths: string[] }
  | { op: "git_unstage"; root: string; paths: string[] }
  | { op: "git_discard"; root: string; paths: string[]; untracked: string[] }
  | { op: "git_commit"; root: string; message: string }
  | { op: "git_push"; root: string }
  | { op: "git_pull"; root: string }
  | { op: "git_log"; root: string; limit: number };
/** 列号、长度按 UTF-16 算，列号从 1 开始；preview_col 是匹配在预览里的位置，从 0 开始 */
export interface SearchMatch { line: number; col: number; len: number; preview: string; preview_col: number }
export interface SearchResult { files: { path: string; matches: SearchMatch[] }[]; truncated: boolean }
export interface FileList { root: string; files: string[]; truncated: boolean }
export interface FileStat { size: number; mtime: number | null; dir: boolean }
/** status 是 git status --porcelain 的两个字母，比如 " M"、"A "、"??"；path 相对 top，分隔符是 / */
/** prefix：请求的目录相对仓库根目录的位置（如 "app/src/"），按它把 entries 换算成文件树里的路径 */
/** upstream 是上游分支（如 origin/main），ahead/behind 是比它多几个、少几个提交 */
export interface GitStatus {
  top: string; prefix: string; branch: string | null; entries: { path: string; status: string }[];
  upstream?: string | null; ahead?: number; behind?: number;
}
export interface GitCommit { hash: string; short: string; author: string; time: number; subject: string }
export type EditReply =
  | { kind: "search"; result: SearchResult }
  | { kind: "files"; list: FileList }
  | { kind: "stats"; stats: (FileStat | null)[] }
  | { kind: "done" }
  | { kind: "git"; status: GitStatus | null }
  | { kind: "output"; text: string }
  | { kind: "log"; commits: GitCommit[] };

async function edit<K extends EditReply["kind"]>(connId: number, request: EditRequest, kind: K): Promise<Extract<EditReply, { kind: K }>> {
  const reply = await invoke<EditReply>("edit_op", { connId, request });
  if (reply.kind !== kind) throw new Error(`unexpected reply: ${reply.kind}`);
  return reply as Extract<EditReply, { kind: K }>;
}

/** 保存时文件被别人改过，后端报的错以它开头 */
export const CONFLICT_PREFIX = "conflict:";
/** 对方电脑上找不到某种语言的语言服务时，错误信息以它开头，后面跟语言 */
export const LSP_MISSING_PREFIX = "lsp_missing:";

/** 给代码编辑器窗口的请求：换根目录，和/或打开一个文件跳到某行 */
export interface EditorRequest { root?: string; path?: string; line?: number; col?: number }
export interface EditorBootstrap { endpoint_id: string; device_name: string; requests: EditorRequest[] }
export interface LspOpened { lsp_id: number; server: string; root: string }

/** 对方电脑上正在监听的端口 */
export interface ListeningPort { port: number; process?: string; pid?: number }
/** 一个正在进行的端口转发：对方的 remote_port 接到了本机的 local_port */
export interface ForwardView { id: number; conn_id: number; remote_port: number; local_port: number; connections: number }
export type LspEventView = { kind: "message"; message: string } | { kind: "closed"; reason: string | null };

export interface OpenedTerminal { term_id: number; session_id: string }

export interface Connected {
  conn_id: number;
  code: string | null;
  info: RemoteInfo;
  path: PathInfo;
  /** 连接时勾了互相信任并且已经记下 */
  mutual: boolean;
}

export type RejectCode = "need_password" | "bad_password" | "denied" | "timeout" | "locked" | "version" | "controller_only";
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
  /** 本机自己：常驻的一条连接，侧边栏的“本机”用它开本地终端、读会话 */
  local?: { conn_id: number; info: RemoteInfo } | null;
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
  /** 并发分段时用了几条流；0 是一条流（对方是旧版本，或者关了并发） */
  lanes?: number;
  /** 并发分段传输的诊断：往返延迟、丢包、时间花在读盘还是等网络上，查慢的时候看 */
  diag?: string;
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
  collabDeliveries: () => invoke<CollaborationDelivery[]>("collab_deliveries"),
  collabRequest: async (target: string | null, projectId: string | null, action: CollaborationAction, requestId = crypto.randomUUID().replaceAll("-", "")) =>
    unwrapCollaboration(await invoke<CollaborationReply>("collab_request", {
      target,
      request: { request_id: requestId, project_id: projectId, action },
    })),
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
  /** 系统选择框，没选返回空。手机上选的文件先存进缓存目录，返回的是存好的路径；手机不能选文件夹 */
  pickPaths: async (folders: boolean): Promise<string[]> => {
    if (!isMobile) return invoke<string[]>("pick_paths", { folders });
    if (folders) throw new Error(t("手机上暂不支持这个功能"));
    return stagePhoneFiles(await pickPhoneFiles());
  },
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
  sendAgentMessage: (connId: number, sessionId: string, kind: string, requestId: string, text: string) =>
    invoke<void>("send_agent_message", { connId, sessionId, kind, requestId, text }),
  endTerminal: (connId: number, sessionId: string) => invoke("end_terminal", { connId, sessionId }),
  /** 列出对方电脑上某个目录的子目录；path 空着是用户目录 */
  listDir: (connId: number, path: string) => invoke<DirListing>("list_dir", { connId, path }),
  /** 会话在空闲提示符时切到这个目录 */
  changeDir: (connId: number, sessionId: string, path: string) => invoke<void>("change_dir", { connId, sessionId, path }),
  /** 远程编辑：列出目录里的子目录和文件 */
  listEntries: (connId: number, path: string) => invoke<EntryListing>("list_entries", { connId, path }),
  /** head 为真时读 Git 最新提交里的版本（对比用） */
  readFile: (connId: number, path: string, head = false) => invoke<OpenedFile>("read_file", { connId, path, head }),
  searchFiles: async (connId: number, root: string, query: string, options: { regex: boolean; caseSensitive: boolean; wholeWord: boolean }) =>
    (await edit(connId, { op: "search", root, query, regex: options.regex, case_sensitive: options.caseSensitive, whole_word: options.wholeWord }, "search")).result,
  listFiles: async (connId: number, root: string) => (await edit(connId, { op: "list_files", root }, "files")).list,
  statFiles: async (connId: number, paths: string[]) => (await edit(connId, { op: "stat", paths }, "stats")).stats,
  createFile: (connId: number, path: string) => edit(connId, { op: "create_file", path }, "done"),
  createDir: (connId: number, path: string) => edit(connId, { op: "create_dir", path }, "done"),
  renamePath: (connId: number, from: string, to: string) => edit(connId, { op: "rename", from, to }, "done"),
  /** 移到废纸篓/回收站 */
  deletePath: (connId: number, path: string) => edit(connId, { op: "delete", path }, "done"),
  gitStatus: async (connId: number, root: string) => (await edit(connId, { op: "git_status", root }, "git")).status,
  gitStage: (connId: number, root: string, paths: string[]) => edit(connId, { op: "git_stage", root, paths }, "done"),
  gitUnstage: (connId: number, root: string, paths: string[]) => edit(connId, { op: "git_unstage", root, paths }, "done"),
  /** paths 换回暂存区的版本，untracked 移到废纸篓 */
  gitDiscard: (connId: number, root: string, paths: string[], untracked: string[]) => edit(connId, { op: "git_discard", root, paths, untracked }, "done"),
  gitCommit: async (connId: number, root: string, message: string) => (await edit(connId, { op: "git_commit", root, message }, "output")).text,
  gitPush: async (connId: number, root: string) => (await edit(connId, { op: "git_push", root }, "output")).text,
  gitPull: async (connId: number, root: string) => (await edit(connId, { op: "git_pull", root }, "output")).text,
  gitLog: async (connId: number, root: string, limit = 20) => (await edit(connId, { op: "git_log", root, limit }, "log")).commits,
  /** 在对方电脑上启动语言服务（补全、跳转、报错）；它发来的消息和结束都从 onEvent 送来 */
  lspStart: (connId: number, language: string, file: string, root: string, onEvent: (event: LspEventView) => void) => {
    const messages = new Channel<LspEventView>();
    messages.onmessage = onEvent;
    return invoke<LspOpened>("lsp_start", { connId, language, file, root, messages });
  },
  /** 按顺序发一批消息 */
  lspSend: (lspId: number, messages: string[]) => invoke<void>("lsp_send", { lspId, messages }),
  lspStop: (lspId: number) => invoke<void>("lsp_stop", { lspId }),
  /** 打开（或叫到前面）某台设备的代码编辑器窗口；initialRoot 只在新开窗口时用 */
  openEditorWindow: (endpointId: string, deviceName: string, initialRoot: string, request?: EditorRequest) =>
    invoke<void>("open_editor_window", { endpointId, deviceName, initialRoot, request: request ?? null }),
  /** 编辑器窗口用：自己是哪台设备的，取走排队的请求 */
  editorTakeRequests: () => invoke<EditorBootstrap>("editor_take_requests"),
  /** 这台设备现在用哪条连接，断开时为空 */
  editorConnection: (endpointId: string) => invoke<number | null>("editor_connection", { endpointId }),
  listPorts: (connId: number) => invoke<ListeningPort[]>("list_ports", { connId }),
  /** 同一台设备的同一个端口已经在转发时返回已有的那个 */
  forwardPort: (connId: number, remotePort: number, localPort?: number) => invoke<ForwardView>("forward_port", { connId, remotePort, localPort: localPort ?? null }),
  stopForward: (id: number) => invoke<void>("stop_forward", { id }),
  portForwards: () => invoke<ForwardView[]>("port_forwards"),
  /** 在浏览器里打开转发过来的端口，path 是 / 开头的路径 */
  openForward: (id: number, path = "/", https = false) => invoke<void>("open_forward", { id, path, https }),
  /** 本机自己的服务，不用转发 */
  openLocalhost: (port: number, path = "/", https = false) => invoke<void>("open_localhost", { port, path, https }),
  /** expected 是打开时的哈希；对不上会报 conflict:，force 直接覆盖 */
  writeFile: (connId: number, path: string, text: string, bom: boolean, expected: string | null, force = false) =>
    invoke<FileMeta>("write_file", { connId, path, text, bom, expected, force }),
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
  return ({ macos: "macOS", windows: "Windows", linux: "Linux", android: "Android", ios: "iOS", harmonyos: "HarmonyOS" } as Record<string, string>)[p] ?? p;
}

export function pathLabel(p: PathInfo | null | undefined): string {
  if (!p) return t("连接中");
  const name = { ipv6: t("IPv6 直连"), ipv4: t("IPv4 直连"), relay: t("中继"), local: t("本机"), unknown: t("连接中") }[p.kind];
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
