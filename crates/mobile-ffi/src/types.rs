//! 交给手机界面的数据：都是扁平的记录和枚举，UniFFI 照着生成 Kotlin data class 和 Swift struct。
//! 从 core 的类型转过来，只留手机要用的字段；core 的类型本身不沾 UniFFI。

use p2pcli_core::agents::{AgentInfo, AgentState as CoreAgentState};
use p2pcli_core::files::{Direction, Transfer, TransferState as CoreTransferState};
use p2pcli_core::host::FileRequest;
use p2pcli_core::lan::NearbyDevice;
use p2pcli_core::path::{PathInfo, PathKind as CorePathKind};
use p2pcli_core::presence::PresenceState;
use p2pcli_core::proto::{RejectCode, TerminalSession};
use p2pcli_core::rendezvous::RdvStatus;

/// 牵线服务器的状态：没连上时只能连同一个局域网里的设备
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum ServerState {
    /// 没配牵线服务器
    None,
    Connecting,
    Online,
    Offline { error: String },
}

impl From<RdvStatus> for ServerState {
    fn from(s: RdvStatus) -> Self {
        match s {
            RdvStatus::Connecting => Self::Connecting,
            RdvStatus::Online => Self::Online,
            RdvStatus::Offline { error } => Self::Offline { error },
        }
    }
}

/// 走的是哪条路：直连（IPv6、IPv4）还是中继
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PathKind {
    Ipv6,
    Ipv4,
    Relay,
    Local,
    Unknown,
}

impl From<CorePathKind> for PathKind {
    fn from(k: CorePathKind) -> Self {
        match k {
            CorePathKind::Ipv6 => Self::Ipv6,
            CorePathKind::Ipv4 => Self::Ipv4,
            CorePathKind::Relay => Self::Relay,
            CorePathKind::Local => Self::Local,
            CorePathKind::Unknown => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PathView {
    pub kind: PathKind,
    /// 往返延迟；还没测到是空
    pub rtt_ms: Option<u64>,
}

impl From<PathInfo> for PathView {
    fn from(p: PathInfo) -> Self {
        Self { kind: p.kind.into(), rtt_ms: p.rtt_ms }
    }
}

/// 连接历史里一台设备在不在线（后台探测的结果）
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Presence {
    /// 还没探测，或者正在探测
    Checking,
    Online,
    Offline,
    /// 对方在线，但版本太旧，探测不了
    OldVersion,
}

impl From<PresenceState> for Presence {
    fn from(s: PresenceState) -> Self {
        match s {
            PresenceState::Checking => Self::Checking,
            PresenceState::Online => Self::Online,
            PresenceState::Offline => Self::Offline,
            PresenceState::OldVersion => Self::OldVersion,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentState {
    Working,
    /// 停下来等你：批准操作、选选项
    Waiting,
    Idle,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ApprovalChoice {
    pub id: u64,
    pub label: String,
}

/// AI 弹出的确认框：选项直接在手机上点，不用到终端里按方向键
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ApprovalView {
    pub id: String,
    pub context: String,
    pub options: Vec<ApprovalChoice>,
}

/// 会话里认出来的命令行 AI
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct AgentView {
    /// claude、codex 这种稳定标识，按它配图标
    pub kind: String,
    pub name: String,
    pub state: AgentState,
    /// 干活画面很久没变化，可能卡住了
    pub stalled: bool,
    pub approval: Option<ApprovalView>,
}

impl From<AgentInfo> for AgentView {
    fn from(a: AgentInfo) -> Self {
        let state = match a.state {
            CoreAgentState::Working => AgentState::Working,
            CoreAgentState::Waiting => AgentState::Waiting,
            CoreAgentState::Idle => AgentState::Idle,
        };
        Self {
            stalled: state == AgentState::Working && a.unchanged_for.is_some(),
            kind: a.kind,
            name: a.name,
            state,
            approval: a.approval.map(|ap| ApprovalView {
                id: ap.id,
                context: ap.context,
                options: ap.options.into_iter().map(|c| ApprovalChoice { id: c.id as u64, label: c.label }).collect(),
            }),
        }
    }
}

/// 对方电脑上的一个后台会话（tmux），手机关掉终端它也接着跑
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SessionView {
    pub id: String,
    /// 创建时间（秒）
    pub created_at: u64,
    /// 有别的终端正连着它
    pub attached: bool,
    /// 正在跑的命令，比如 zsh、vim
    pub command: Option<String>,
    /// 当前目录
    pub current_path: Option<String>,
    /// 停在空闲的命令提示符
    pub shell_ready: bool,
    pub agent: Option<AgentView>,
    /// 用户给这个会话起的备注
    pub remark: Option<String>,
}

impl SessionView {
    pub(crate) fn new(s: TerminalSession, remark: Option<String>) -> Self {
        let details = s.details.unwrap_or_else(|| p2pcli_core::proto::TerminalDetails {
            shell_ready: false,
            window_index: None,
            window_name: None,
            window_count: None,
            command: None,
            current_path: None,
        });
        Self {
            id: s.id,
            created_at: s.created_at,
            attached: s.attached,
            command: details.command,
            current_path: details.current_path,
            shell_ready: details.shell_ready,
            agent: s.agent.map(Into::into),
            remark: remark.filter(|r| !r.trim().is_empty()),
        }
    }
}

/// 设备列表里的一台设备：连接历史、后台探测、局域网发现、备注合在一起，界面直接显示
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct DeviceView {
    pub endpoint_id: String,
    /// 设备码；只在局域网里发现、没连过的设备没有
    pub code: Option<String>,
    pub name: String,
    /// 用户起的备注，显示时优先用它
    pub remark: Option<String>,
    pub platform: String,
    /// 连过（在连接历史里）
    pub known: bool,
    pub presence: Presence,
    pub path: Option<PathView>,
    /// 在同一个局域网里在线：连它不经过牵线服务器
    pub lan_online: bool,
    /// 后台探测拿到的会话列表；sessions_live 为假时是上次记住的
    pub sessions: Vec<SessionView>,
    pub sessions_live: bool,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Overview {
    /// 本机（手机）的名字
    pub name: String,
    pub endpoint_id: String,
    pub server: ServerState,
    /// 连接历史，最近用过的在前
    pub devices: Vec<DeviceView>,
    /// 局域网里发现、还没连过的设备
    pub nearby: Vec<DeviceView>,
    /// 收到的文件放在哪
    pub inbox_dir: String,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct NearbyView {
    pub endpoint_id: String,
    pub name: String,
    pub platform: String,
    pub online: bool,
}

impl From<NearbyDevice> for NearbyView {
    fn from(d: NearbyDevice) -> Self {
        Self { endpoint_id: d.endpoint_id, name: d.name, platform: d.platform, online: d.online }
    }
}

/// 连上一台设备
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Connected {
    /// 之后开终端、读会话都用这个编号
    pub conn_id: u64,
    pub endpoint_id: String,
    pub name: String,
    pub platform: String,
    /// 按设备码连的才有
    pub code: Option<String>,
    pub path: PathView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TransferState {
    Connecting,
    /// 等对方同意
    Waiting,
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TransferView {
    pub id: String,
    /// 真是发出去，假是收进来
    pub outgoing: bool,
    pub peer_name: String,
    /// 第一项的名字，多项时带“等 N 项”
    pub label: String,
    pub files: u64,
    pub total: u64,
    pub done: u64,
    pub state: TransferState,
    pub error: Option<String>,
    /// 收进来的放在哪（本机路径）
    pub dir: Option<String>,
}

impl From<Transfer> for TransferView {
    fn from(t: Transfer) -> Self {
        Self {
            id: t.id,
            outgoing: t.direction == Direction::Send,
            peer_name: t.peer_name,
            label: t.label,
            files: t.files as u64,
            total: t.total,
            done: t.done,
            state: match t.state {
                CoreTransferState::Connecting => TransferState::Connecting,
                CoreTransferState::Waiting => TransferState::Waiting,
                CoreTransferState::Running => TransferState::Running,
                CoreTransferState::Done => TransferState::Done,
                CoreTransferState::Failed => TransferState::Failed,
            },
            error: t.error,
            dir: t.dir,
        }
    }
}

/// 没配对过的设备要发文件给手机，问收不收
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FileRequestView {
    pub id: u64,
    pub name: String,
    pub platform: String,
    pub files: u64,
    pub total: u64,
    /// 前几项的名字
    pub items: Vec<String>,
}

impl From<FileRequest> for FileRequestView {
    fn from(r: FileRequest) -> Self {
        Self { id: r.id, name: r.name, platform: r.platform, files: r.files as u64, total: r.total, items: r.items }
    }
}

/// 连不上的原因：要密码、密码不对时界面弹出填密码，别的提示一下就行
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RejectReason {
    NeedPassword,
    BadPassword,
    Denied,
    Timeout,
    Locked,
    Version,
    ControllerOnly,
}

impl From<RejectCode> for RejectReason {
    fn from(c: RejectCode) -> Self {
        match c {
            RejectCode::NeedPassword => Self::NeedPassword,
            RejectCode::BadPassword => Self::BadPassword,
            RejectCode::Denied => Self::Denied,
            RejectCode::Timeout => Self::Timeout,
            RejectCode::Locked => Self::Locked,
            RejectCode::Version => Self::Version,
            RejectCode::ControllerOnly => Self::ControllerOnly,
        }
    }
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum P2pError {
    /// 对方拒绝了：detail 是给人看的说明。不叫 message：Kotlin 的异常自带 message，会撞名
    #[error("{detail}")]
    Rejected { reason: RejectReason, detail: String },
    /// 连接已断开（编号对不上）
    #[error("连接已断开")]
    Disconnected,
    #[error("{detail}")]
    Failed { detail: String },
}

impl P2pError {
    pub(crate) fn failed(e: impl std::fmt::Display) -> Self {
        Self::Failed { detail: e.to_string() }
    }
    pub(crate) fn from_anyhow(e: anyhow::Error) -> Self {
        Self::Failed { detail: format!("{e:#}") }
    }
}
