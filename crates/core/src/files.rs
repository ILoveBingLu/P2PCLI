//! 传文件：设备之间直接推文件，走单独的协议号，和终端、在线探测分开。
//!
//! 发送方一次推一批（一个或几个文件、文件夹）：先报清单，接收方认识这台设备就直接收，
//! 不认识的弹窗问主人。然后按顺序发每个文件的字节，每个文件发完附上 BLAKE3 哈希，接收方核对。
//! 收到的先放在收件目录的 .partial 里，整批核对完才挪到正式位置；
//! 断了再发同一批，已经收到的部分不用重传。

use std::collections::{HashMap, HashSet};
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::{Endpoint, EndpointAddr, EndpointId};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::time::timeout;
use tracing::{debug, info, warn};

mod lanes;

use crate::ALPN_FILE;
use crate::client::RemoteHost;
use crate::host::{APPROVE_TIMEOUT, FileRequest, Host, HostEvent, clean_name};
use crate::identity::DeviceInfo;
use crate::presence::is_unknown_protocol;
use crate::proto::{AnyFrame, Msg, read_frame_as, read_json, write_data, write_json};
use crate::store::{RecentStore, now_secs, write_private};

/// 每个数据帧多大
const CHUNK: usize = 256 * 1024;
/// 一批最多多少个文件
const MAX_ENTRIES: usize = 100_000;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// 收件记录留多少条
const INBOX_KEEP: usize = 200;
/// 传输记录留多少条（界面和命令行看进度用，不存盘）
const TRANSFERS_KEEP: usize = 50;
/// 没传完的半截文件留多久，过了就清掉
const PARTIAL_KEEP: Duration = Duration::from_secs(7 * 24 * 3600);
const PARTIAL_DIR: &str = ".partial";

/// 清单里的一个文件
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileEntry {
    /// 相对路径，用 / 分隔，第一段是发送方选的文件或文件夹名
    pub path: String,
    pub size: u64,
    /// 修改时间（秒）：和路径、大小一起认是不是同一个文件，决定能不能续传
    #[serde(default)]
    pub modified: u64,
    /// 可执行文件，Mac 和 Linux 之间保留这个权限
    #[serde(default)]
    pub exec: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum FileMsg {
    /// 发送方开场：我是谁，要发哪些文件。lanes 是发送方最多想开几条并发流（旧版本没有这一项）
    Offer {
        id: String,
        name: String,
        platform: String,
        entries: Vec<FileEntry>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lanes: Option<u32>,
    },
    /// 接收方同意：每个文件已经有多少字节，从那里接着发
    Accept { offsets: Vec<u64> },
    /// 接收方同意并发分段收：用几条流、每段多大、每个文件里已经收好的段（段编号）
    LaneAccept { lanes: u32, segment: u64, done: Vec<Vec<u32>> },
    /// 并发分段：这条单向流属于哪次传输（流上的第一条消息）
    Lane { id: String },
    /// 一段数据：第 file 个文件从 offset 开始 len 字节，后面跟着这么多字节的数据帧
    Seg { file: u32, offset: u64, len: u64 },
    /// 这一段发完了，附上这一段的哈希
    SegEnd { hash: String },
    /// 所有段都发完了
    Finish,
    Reject { message: String },
    /// 第 index 个文件发完了，附上整个文件的哈希
    Done { index: u32, hash: String },
    /// 整批收好了：放在哪个目录，第一层每一项存成了什么路径
    Saved { dir: String, paths: Vec<String> },
    Error { message: String },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Send,
    Receive,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferState {
    Connecting,
    /// 等对方同意
    Waiting,
    Running,
    Done,
    Failed,
}

/// 一批文件的传输进度
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Transfer {
    pub id: String,
    pub direction: Direction,
    /// 对方的公钥
    pub peer: String,
    pub peer_name: String,
    /// 显示用：第一项的名字，多项时加“等 N 项”
    pub label: String,
    pub files: usize,
    pub total: u64,
    pub done: u64,
    pub state: TransferState,
    pub error: Option<String>,
    /// 存在接收方的哪个目录，第一层每一项的完整路径（都是接收方那台电脑上的路径）
    pub dir: Option<String>,
    pub paths: Vec<String>,
    pub started_at: u64,
    pub finished_at: Option<u64>,
    /// 并发分段时用了几条流；0 是一条流（对方是旧版本，或者本机关了并发）
    #[serde(default)]
    pub lanes: u32,
}

impl Transfer {
    pub fn finished(&self) -> bool {
        matches!(self.state, TransferState::Done | TransferState::Failed)
    }
}

/// 收件记录：收到过哪些文件、放在哪
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InboxItem {
    pub id: String,
    pub from: String,
    pub from_name: String,
    pub at: u64,
    pub dir: String,
    pub paths: Vec<String>,
    pub files: usize,
    pub total: u64,
}

/// 默认的收件目录：下载文件夹里的 P2PCLI，按发送设备分文件夹
pub fn default_inbox_dir() -> PathBuf {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    home.join("Downloads").join("P2PCLI")
}

pub struct Files {
    host: Arc<Host>,
    recent: Arc<RecentStore>,
    inbox_root: PathBuf,
    inbox_path: PathBuf,
    inbox: Mutex<Vec<InboxItem>>,
    transfers: Mutex<Vec<Transfer>>,
    /// 发文件用的连接，留着下次复用
    conns: Mutex<HashMap<EndpointId, Connection>>,
    /// 正在收的半截文件：同一个文件不能两批同时收
    receiving: Mutex<HashSet<String>>,
    /// 并发分段最多开几条流；0 是关掉，只用一条流
    lanes_max: u32,
    /// 正在并发分段收的传输，各条车道按编号找到它
    lane_sessions: Mutex<HashMap<String, Arc<lanes::LaneSession>>>,
}

/// 默认最多开几条并发流
pub const DEFAULT_LANES: u32 = 8;

impl Files {
    pub fn new(host: Arc<Host>, recent: Arc<RecentStore>, data_dir: &Path, inbox_root: PathBuf, lanes_max: u32) -> Arc<Self> {
        let inbox_path = data_dir.join("inbox.json");
        let inbox = std::fs::read(&inbox_path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let linked = host.clone();
        let files = Arc::new(Self {
            host,
            recent,
            inbox_root,
            inbox_path,
            inbox: Mutex::new(inbox),
            transfers: Default::default(),
            conns: Default::default(),
            receiving: Default::default(),
            lanes_max,
            lane_sessions: Default::default(),
        });
        // 被连方收“推到指定目录”、发“被拉走的文件”时要用到这里
        linked.attach_files(&files);
        files
    }

    pub fn inbox_root(&self) -> &Path {
        &self.inbox_root
    }

    /// 最近的传输，新的在前
    pub fn transfers(&self) -> Vec<Transfer> {
        self.transfers.lock().unwrap().iter().rev().cloned().collect()
    }

    pub fn transfer(&self, id: &str) -> Option<Transfer> {
        self.transfers.lock().unwrap().iter().find(|t| t.id == id).cloned()
    }

    /// 收到过的文件，新的在前
    pub fn inbox(&self, limit: usize) -> Vec<InboxItem> {
        self.inbox.lock().unwrap().iter().rev().take(limit).cloned().collect()
    }

    fn track(&self, t: Transfer) {
        let mut list = self.transfers.lock().unwrap();
        list.push(t);
        // 太多了先删最早的已结束的，进行中的不删
        while list.len() > TRANSFERS_KEEP {
            match list.iter().position(Transfer::finished) {
                Some(i) => {
                    list.remove(i);
                }
                None => break,
            }
        }
    }

    fn update(&self, id: &str, f: impl FnOnce(&mut Transfer)) -> Option<Transfer> {
        let mut list = self.transfers.lock().unwrap();
        let t = list.iter_mut().find(|t| t.id == id)?;
        f(t);
        Some(t.clone())
    }

    fn end(&self, id: &str, result: Result<(String, Vec<String>)>) -> Option<Transfer> {
        self.update(id, |t| {
            t.finished_at = Some(now_secs());
            match result {
                Ok((dir, paths)) => {
                    t.state = TransferState::Done;
                    t.done = t.total;
                    t.dir = Some(dir);
                    t.paths = paths;
                }
                Err(e) => {
                    t.state = TransferState::Failed;
                    t.error = Some(format!("{e:#}"));
                }
            }
        })
    }

    /// 这次传输在开始传之前就失败了（比如连不上对方）
    pub fn fail(&self, id: &str, message: String) {
        self.end(id, Err(anyhow!(message)));
    }

    /// 登记一次发送，返回编号；真正开始发用 [`Files::run_send`]
    pub fn begin(&self, peer: &EndpointId, peer_name: &str, paths: &[PathBuf]) -> String {
        let names: Vec<String> = paths
            .iter()
            .map(|p| p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned()))
            .collect();
        self.begin_as(Direction::Send, peer, peer_name, label(&names))
    }

    /// 登记一次从对方拉文件，返回编号；真正开始拉用 [`Files::run_pull`]
    pub fn begin_pull(&self, peer: &EndpointId, peer_name: &str, remote_paths: &[String]) -> String {
        let names: Vec<String> = remote_paths
            .iter()
            .map(|p| p.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(p).to_string())
            .collect();
        self.begin_as(Direction::Receive, peer, peer_name, label(&names))
    }

    fn begin_as(&self, direction: Direction, peer: &EndpointId, peer_name: &str, label: String) -> String {
        let id = new_id();
        self.track(Transfer {
            id: id.clone(),
            direction,
            peer: peer.to_string(),
            peer_name: peer_name.to_string(),
            label,
            files: 0,
            total: 0,
            done: 0,
            state: TransferState::Connecting,
            error: None,
            dir: None,
            paths: Vec::new(),
            started_at: now_secs(),
            finished_at: None,
            lanes: 0,
        });
        id
    }

    /// 把本机的文件或文件夹推给对方，等对方收好，返回最后的结果
    pub async fn run_send(&self, id: &str, endpoint: &Endpoint, addr: EndpointAddr, me: &DeviceInfo, paths: Vec<PathBuf>) -> Transfer {
        let result = self.send_inner(id, endpoint, addr, me, paths).await;
        if let Err(e) = &result {
            info!(id, "发文件失败：{e:#}");
        }
        self.end(id, result).unwrap_or_else(|| panic!("传输记录 {id} 不见了"))
    }

    async fn send_inner(&self, id: &str, endpoint: &Endpoint, addr: EndpointAddr, me: &DeviceInfo, paths: Vec<PathBuf>) -> Result<(String, Vec<String>)> {
        let list = self.list_for(id, paths).await?;
        let conn = self.connection(endpoint, addr).await?;
        let (mut send, mut recv) = conn.open_bi().await.map_err(|e| anyhow!("{e}"))?;
        self.send_batch(id, &mut send, &mut recv, me, list, Some(&conn)).await
    }

    /// 把本机的文件推到对方电脑上指定的目录（目录在对方电脑上，不存在会建，同名文件会被盖掉）。
    /// 走已经认证过的终端连接，所以只有密码对了、对方点过同意或者勾了受信的设备才能用。
    pub async fn run_put(&self, id: &str, remote: &RemoteHost, me: &DeviceInfo, paths: Vec<PathBuf>, dest: String) -> Transfer {
        let result = async {
            let list = self.list_for(id, paths).await?;
            let (mut send, mut recv) = remote.open_stream(&Msg::PutFiles { dest }).await?;
            self.send_batch(id, &mut send, &mut recv, me, list, None).await
        }
        .await;
        if let Err(e) = &result {
            info!(id, "推文件到指定目录失败：{e:#}");
        }
        self.end(id, result).unwrap_or_else(|| panic!("传输记录 {id} 不见了"))
    }

    /// 列出要发的文件并记进传输记录
    async fn list_for(&self, id: &str, paths: Vec<PathBuf>) -> Result<Vec<(PathBuf, FileEntry)>> {
        let list = tokio::task::spawn_blocking(move || collect(&paths)).await??;
        let total = list.iter().map(|(_, e)| e.size).sum();
        let files = list.len();
        self.update(id, |t| {
            t.files = files;
            t.total = total;
        });
        Ok(list)
    }

    /// 在打开的流上发一批：报清单，等对方回要从哪里接着发，发完等对方核对放好
    /// `lanes` 有值就是这条流所在的连接：可以请对方并发分段收（对方是旧版本会自动退回一条流）
    async fn send_batch(
        &self,
        id: &str,
        send: &mut SendStream,
        recv: &mut RecvStream,
        me: &DeviceInfo,
        list: Vec<(PathBuf, FileEntry)>,
        lanes: Option<&Connection>,
    ) -> Result<(String, Vec<String>)> {
        let entries: Vec<FileEntry> = list.iter().map(|(_, e)| e.clone()).collect();
        let want_lanes = lanes.filter(|_| self.lanes_max > 0).map(|_| self.lanes_max);
        write_json(
            send,
            &FileMsg::Offer { id: id.to_string(), name: me.name.clone(), platform: me.platform.clone(), entries: entries.clone(), lanes: want_lanes },
        )
        .await?;
        self.update(id, |t| t.state = TransferState::Waiting);
        // 对方可能要弹窗问主人，等得比弹窗超时长一点
        let reply = timeout(APPROVE_TIMEOUT + Duration::from_secs(15), read_json::<_, FileMsg>(recv))
            .await
            .map_err(|_| anyhow!("对方一直没有回应"))??;
        let offsets = match reply {
            FileMsg::Accept { offsets } if offsets.len() == entries.len() => offsets,
            FileMsg::LaneAccept { lanes: n, segment, done } if done.len() == entries.len() => {
                let conn = lanes.ok_or_else(|| anyhow!("对方回了意外的消息：没请求并发分段"))?;
                return self.send_lanes(id, conn, send, recv, &list, n, segment, done).await;
            }
            FileMsg::Reject { message } => bail!("{message}"),
            FileMsg::Error { message } => bail!("对方出错：{message}"),
            other => bail!("对方回了意外的消息：{other:?}"),
        };
        let resumed = offsets.iter().zip(&entries).map(|(o, e)| (*o).min(e.size)).sum();
        self.update(id, |t| {
            t.state = TransferState::Running;
            t.done = resumed;
        });
        let streamed: Result<()> = async {
            for (index, ((local, entry), offset)) in list.iter().zip(offsets).enumerate() {
                if offset > entry.size {
                    bail!("对方要的续传位置不对");
                }
                let hash = self
                    .send_file(id, send, local, entry.size, offset)
                    .await
                    .with_context(|| format!("发 {} 失败", entry.path))?;
                write_json(send, &FileMsg::Done { index: index as u32, hash }).await?;
            }
            Ok(())
        }
        .await;
        if let Err(e) = streamed {
            // 对方可能先出了错（比如校验没过）就不收了，它的说法更准
            if let Ok(Ok(FileMsg::Error { message })) = timeout(Duration::from_secs(2), read_json::<_, FileMsg>(recv)).await {
                bail!("对方出错：{message}");
            }
            return Err(e);
        }
        let _ = send.finish();
        // 对方核对完、挪到正式位置才回
        match timeout(Duration::from_secs(120), read_json::<_, FileMsg>(recv))
            .await
            .map_err(|_| anyhow!("对方收完以后没有回应"))??
        {
            FileMsg::Saved { dir, paths } => Ok((dir, paths)),
            FileMsg::Error { message } => bail!("对方出错：{message}"),
            other => bail!("对方回了意外的消息：{other:?}"),
        }
    }

    /// 被连方：对面要拉走这些文件（路径在本机，`~` 开头从用户目录算，相对路径也从用户目录算起）。
    /// 调用前对面已经通过终端连接的认证。
    pub async fn serve_get(&self, peer: &EndpointId, peer_name: &str, send: &mut SendStream, recv: &mut RecvStream, me: &DeviceInfo, paths: Vec<String>) -> Result<()> {
        let paths: Vec<PathBuf> = paths.iter().map(|p| resolve_path(p)).collect();
        let id = self.begin(peer, peer_name, &paths);
        let result = async {
            if paths.is_empty() {
                bail!("没写要拉哪些文件");
            }
            let list = self.list_for(&id, paths).await?;
            self.send_batch(&id, send, recv, me, list, None).await
        }
        .await;
        if let Err(e) = &result {
            info!(%id, "被拉文件失败：{e:#}");
            let _ = write_json(send, &FileMsg::Error { message: format!("{e:#}") }).await;
        }
        self.end(&id, result);
        Ok(())
    }

    /// 本机去拉对方电脑上的文件，放进本机的 `to` 目录（不存在会建，同名文件会被盖掉）。
    pub async fn run_pull(&self, id: &str, remote: &RemoteHost, paths: Vec<String>, to: PathBuf) -> Transfer {
        let result = self.pull_inner(id, remote, paths, to).await;
        if let Err(e) = &result {
            info!(id, "拉文件失败：{e:#}");
        }
        self.end(id, result).unwrap_or_else(|| panic!("传输记录 {id} 不见了"))
    }

    async fn pull_inner(&self, id: &str, remote: &RemoteHost, paths: Vec<String>, to: PathBuf) -> Result<(String, Vec<String>)> {
        let (mut send, mut recv) = remote.open_stream(&Msg::GetFiles { paths }).await?;
        // 对方要先把文件夹走一遍列出清单，文件多时要等一会儿
        let first = timeout(Duration::from_secs(120), read_json::<_, FileMsg>(&mut recv))
            .await
            .map_err(|_| anyhow!("对方一直没有回应"))?
            .map_err(|e| anyhow!("对方没有回应，它的 P2PCLI 可能版本太旧、还不支持拉文件，请更新（{e:#}）"))?;
        let entries = match first {
            FileMsg::Offer { entries, .. } => entries,
            FileMsg::Error { message } => bail!("对方出错：{message}"),
            other => bail!("对方回了意外的消息：{other:?}"),
        };
        let plan = Plan::new(&entries)?;
        let target = Target::Dir { dir: to, notify: false };
        self.receive_batch(Some(id), remote.peer_id(), &remote.info.name, plan, entries, &mut send, &mut recv, target).await
    }

    /// 发一个文件：对方已经有的前 offset 字节只算哈希不再发，返回整个文件的哈希
    async fn send_file(&self, id: &str, send: &mut SendStream, local: &Path, size: u64, offset: u64) -> Result<String> {
        let mut file = tokio::fs::File::open(local).await?;
        if file.metadata().await?.len() != size {
            bail!("文件在发送过程中被改动了");
        }
        let mut hasher = blake3::Hasher::new();
        let mut buf = vec![0u8; CHUNK];
        let mut pos = 0u64;
        while pos < size {
            let want = (size - pos).min(CHUNK as u64) as usize;
            let n = file.read(&mut buf[..want]).await?;
            if n == 0 {
                bail!("文件在发送过程中被改动了");
            }
            hasher.update(&buf[..n]);
            if pos + n as u64 > offset {
                let skip = offset.saturating_sub(pos) as usize;
                write_data(send, &buf[skip..n]).await?;
                let sent = (n - skip) as u64;
                self.update(id, |t| t.done += sent);
            }
            pos += n as u64;
        }
        Ok(hasher.finalize().to_hex().to_string())
    }

    /// 复用还活着的连接；没有就新建。地址交给调用方查好
    async fn connection(&self, endpoint: &Endpoint, addr: EndpointAddr) -> Result<Connection> {
        let id = addr.id;
        let alive = self.conns.lock().unwrap().get(&id).filter(|c| c.close_reason().is_none()).cloned();
        if let Some(conn) = alive {
            return Ok(conn);
        }
        let conn = match timeout(CONNECT_TIMEOUT, endpoint.connect(addr, ALPN_FILE)).await {
            Err(_) => bail!("30 秒内没连上对方，对方可能不在线"),
            Ok(Err(e)) if is_unknown_protocol(&format!("{e:#}")) => bail!("对方的 P2PCLI 版本太旧，收不了文件，请先更新"),
            Ok(Err(e)) => bail!("连不上对方：{e}"),
            Ok(Ok(conn)) => conn,
        };
        self.conns.lock().unwrap().insert(id, conn.clone());
        Ok(conn)
    }

    /// 认识的设备直接收：受信的、同意过它连我的、我连过它的
    fn knows(&self, id: &EndpointId) -> bool {
        let key = id.to_string();
        self.host.trust.contains(id) || self.host.paired.contains(id) || self.recent.list().iter().any(|d| d.endpoint_id == key)
    }

    /// 被发文件的一方：一条连接上可以有好几批，每批一条流
    pub async fn handle(self: Arc<Self>, conn: Connection) {
        loop {
            tokio::select! {
                bi = conn.accept_bi() => {
                    let Ok((send, recv)) = bi else { break };
                    let this = self.clone();
                    let conn = conn.clone();
                    tokio::spawn(async move {
                        if let Err(e) = this.receive(&conn, send, recv).await {
                            debug!(remote = %conn.remote_id().fmt_short(), "收文件失败：{e:#}");
                        }
                    });
                }
                // 并发分段的车道：单向流，第一条消息说明属于哪次传输
                uni = conn.accept_uni() => {
                    let Ok(recv) = uni else { break };
                    let this = self.clone();
                    let conn = conn.clone();
                    tokio::spawn(async move {
                        if let Err(e) = this.receive_lane(&conn, recv).await {
                            debug!(remote = %conn.remote_id().fmt_short(), "收车道失败：{e:#}");
                        }
                    });
                }
            }
        }
    }

    async fn receive(&self, conn: &Connection, mut send: SendStream, mut recv: RecvStream) -> Result<()> {
        let remote = conn.remote_id();
        let FileMsg::Offer { id: offer_id, name, platform, entries, lanes } = timeout(Duration::from_secs(10), read_json(&mut recv)).await?? else {
            bail!("开场消息不对");
        };
        let name = clean_name(&name);
        let platform = clean_name(&platform);
        let plan = match Plan::new(&entries) {
            Ok(plan) => plan,
            Err(e) => return reply(&mut send, FileMsg::Reject { message: format!("{e:#}") }).await,
        };
        let total: u64 = entries.iter().map(|e| e.size).sum();
        if !self.knows(&remote) {
            let req = FileRequest {
                id: 0,
                remote: remote.to_string(),
                name: name.clone(),
                platform: platform.clone(),
                files: entries.len(),
                total,
                items: plan.tops.iter().take(5).map(|t| t.name.clone()).collect(),
            };
            if !self.host.ask_files(req, conn).await {
                info!(remote = %remote.fmt_short(), "拒收文件");
                return reply(&mut send, FileMsg::Reject { message: "对方拒绝接收".into() }).await;
            }
        }
        // 失败已经回给对方、记进传输记录了
        match lanes.filter(|n| *n > 0 && self.lanes_max > 0) {
            // 对方想并发分段，本机也开着：多条流一起收
            Some(offered) => {
                let _ = self.receive_lanes(remote, offer_id, &name, plan, entries, offered, &mut send, &mut recv).await;
            }
            None => {
                let _ = self.receive_batch(None, remote, &name, plan, entries, &mut send, &mut recv, Target::Inbox).await;
            }
        }
        Ok(())
    }

    /// 被连方：对面要把文件推到本机指定的目录（`~` 开头从用户目录算，相对路径也从用户目录算起，同名文件会被盖掉）。
    /// 调用前对面已经通过终端连接的认证，所以不再弹窗问。
    pub async fn serve_put(&self, peer: &EndpointId, send: &mut SendStream, recv: &mut RecvStream, dest: String) -> Result<()> {
        let FileMsg::Offer { name, entries, .. } = timeout(Duration::from_secs(10), read_json(recv)).await?? else {
            bail!("开场消息不对");
        };
        if dest.trim().is_empty() {
            return reply(send, FileMsg::Reject { message: "没写放到哪个目录".into() }).await;
        }
        let plan = match Plan::new(&entries) {
            Ok(plan) => plan,
            Err(e) => return reply(send, FileMsg::Reject { message: format!("{e:#}") }).await,
        };
        let target = Target::Dir { dir: resolve_path(&dest), notify: true };
        let _ = self.receive_batch(None, *peer, &name, plan, entries, send, recv, target).await;
        Ok(())
    }

    /// 收一批：先占住半截文件、告诉对方从哪里接着发，收完核对，再放到目标位置。
    /// `tid` 是已经登记好的传输编号（拉文件时有）；没有就在这里登记，也由这里收尾。
    /// 失败会回给对方；返回放在了哪里。
    #[allow(clippy::too_many_arguments)]
    async fn receive_batch(
        &self,
        tid: Option<&str>,
        remote: EndpointId,
        name: &str,
        plan: Plan,
        entries: Vec<FileEntry>,
        send: &mut SendStream,
        recv: &mut RecvStream,
        target: Target,
    ) -> Result<(String, Vec<String>)> {
        let name = clean_name(name);
        let total: u64 = entries.iter().map(|e| e.size).sum();
        let tops: Vec<String> = plan.tops.iter().map(|t| t.name.clone()).collect();

        let partial_dir = self.inbox_root.join(PARTIAL_DIR);
        if let Err(e) = tokio::fs::create_dir_all(&partial_dir).await {
            let message = format!("建收件目录失败：{}（{e}）", partial_dir.display());
            let _ = reply(send, FileMsg::Error { message: message.clone() }).await;
            bail!("{message}");
        }
        let keys: Vec<String> = entries.iter().map(|e| partial_key(&remote, e)).collect();
        let Some(_claim) = Claim::take(&self.receiving, &keys) else {
            let message = "这批文件正在传，等它传完再发".to_string();
            let _ = reply(send, FileMsg::Reject { message: message.clone() }).await;
            bail!("{message}");
        };
        let busy = self.receiving.lock().unwrap().clone();
        let dir = partial_dir.clone();
        let _ = tokio::task::spawn_blocking(move || clean_partials(&dir, &busy)).await;

        let mut offsets = Vec::with_capacity(entries.len());
        for (entry, key) in entries.iter().zip(&keys) {
            let path = partial_dir.join(key);
            let len = tokio::fs::metadata(&path).await.map_or(0, |m| m.len());
            if len > entry.size {
                let _ = tokio::fs::remove_file(&path).await;
                offsets.push(0);
            } else {
                offsets.push(len);
            }
        }
        let own = tid.is_none();
        let id = match tid {
            Some(id) => {
                self.update(id, |t| {
                    t.label = label(&tops);
                    t.files = entries.len();
                    t.total = total;
                    t.done = offsets.iter().sum();
                    t.state = TransferState::Running;
                });
                id.to_string()
            }
            None => {
                let id = new_id();
                self.track(Transfer {
                    id: id.clone(),
                    direction: Direction::Receive,
                    peer: remote.to_string(),
                    peer_name: name.clone(),
                    label: label(&tops),
                    files: entries.len(),
                    total,
                    done: offsets.iter().sum(),
                    state: TransferState::Running,
                    error: None,
                    dir: None,
                    paths: Vec::new(),
                    started_at: now_secs(),
                    finished_at: None,
                    lanes: 0,
                });
                id
            }
        };
        info!(remote = %remote.fmt_short(), %name, files = entries.len(), total, "开始收文件");

        let (dest, overwrite) = match &target {
            Target::Inbox => (self.inbox_root.join(safe_component(&name).unwrap_or_else(|| "未命名设备".into())), false),
            Target::Dir { dir, .. } => (dir.clone(), true),
        };
        let result: Result<(String, Vec<String>)> = async {
            write_json(send, &FileMsg::Accept { offsets: offsets.clone() }).await?;
            for (index, ((entry, key), offset)) in entries.iter().zip(&keys).zip(&offsets).enumerate() {
                self.receive_file(&id, recv, index, &partial_dir.join(key), entry.size, *offset)
                    .await
                    .with_context(|| format!("收 {} 失败", entry.path))?;
            }
            let shown = dest.display().to_string();
            let entries = entries.clone();
            let keys = keys.clone();
            let paths = tokio::task::spawn_blocking(move || place(&dest, &partial_dir, &plan, &keys, &entries, overwrite)).await??;
            Ok((shown, paths))
        }
        .await;

        self.conclude(&id, own, remote, name, entries.len(), total, target, result, send).await
    }

    /// 收完（或失败）以后的收尾：回给对方、记收件记录、通知界面、结束传输记录。
    /// 一条流收和并发分段收共用。
    #[allow(clippy::too_many_arguments)]
    async fn conclude(
        &self,
        id: &str,
        own: bool,
        remote: EndpointId,
        name: String,
        files: usize,
        total: u64,
        target: Target,
        result: Result<(String, Vec<String>)>,
        send: &mut SendStream,
    ) -> Result<(String, Vec<String>)> {
        match &result {
            Ok((dir, paths)) => {
                let _ = reply(send, FileMsg::Saved { dir: dir.clone(), paths: paths.clone() }).await;
                info!(remote = %remote.fmt_short(), %dir, "文件收好了");
                let received = HostEvent::FilesReceived {
                    from: remote.to_string(),
                    from_name: name.clone(),
                    files,
                    total,
                    dir: dir.clone(),
                    paths: paths.clone(),
                };
                match target {
                    Target::Inbox => {
                        self.remember(InboxItem {
                            id: id.to_string(),
                            from: remote.to_string(),
                            from_name: name,
                            at: now_secs(),
                            dir: dir.clone(),
                            paths: paths.clone(),
                            files,
                            total,
                        });
                        self.host.notify(received);
                    }
                    Target::Dir { notify: true, .. } => self.host.notify(received),
                    Target::Dir { .. } => {}
                }
            }
            Err(e) => {
                warn!(remote = %remote.fmt_short(), "收文件失败：{e:#}");
                let _ = reply(send, FileMsg::Error { message: format!("{e:#}") }).await;
            }
        }
        let outcome = match &result {
            Ok(v) => Ok(v.clone()),
            Err(e) => Err(anyhow!("{e:#}")),
        };
        if own {
            self.end(id, result);
        }
        outcome
    }

    /// 收一个文件写进半截文件；已经有的前 offset 字节先读出来算进哈希，最后和对方给的哈希核对
    async fn receive_file(&self, id: &str, recv: &mut RecvStream, index: usize, partial: &Path, size: u64, offset: u64) -> Result<()> {
        let mut file = tokio::fs::OpenOptions::new().create(true).read(true).write(true).truncate(false).open(partial).await?;
        let mut hasher = blake3::Hasher::new();
        let mut buf = vec![0u8; CHUNK];
        let mut pos = 0u64;
        while pos < offset {
            let want = (offset - pos).min(CHUNK as u64) as usize;
            let n = file.read(&mut buf[..want]).await?;
            if n == 0 {
                bail!("半截文件读不全");
            }
            hasher.update(&buf[..n]);
            pos += n as u64;
        }
        file.set_len(offset).await?;
        file.seek(SeekFrom::Start(offset)).await?;
        let mut left = size - offset;
        loop {
            match read_frame_as::<_, FileMsg>(recv).await? {
                Some(AnyFrame::Data(d)) => {
                    let n = d.len() as u64;
                    if n > left {
                        bail!("对方发来的比文件大小多");
                    }
                    file.write_all(&d).await?;
                    hasher.update(&d);
                    left -= n;
                    self.update(id, |t| t.done += n);
                }
                Some(AnyFrame::Msg(FileMsg::Done { index: i, hash })) => {
                    if i as usize != index || left != 0 {
                        bail!("文件没收全");
                    }
                    file.flush().await?;
                    drop(file);
                    if hasher.finalize().to_hex().as_str() != hash {
                        let _ = tokio::fs::remove_file(partial).await;
                        bail!("校验没通过，文件可能在传输中损坏，请重发");
                    }
                    return Ok(());
                }
                Some(AnyFrame::Msg(other)) => bail!("意外的消息：{other:?}"),
                None => bail!("对方中途断开"),
            }
        }
    }

    fn remember(&self, item: InboxItem) {
        let mut inbox = self.inbox.lock().unwrap();
        inbox.push(item);
        let extra = inbox.len().saturating_sub(INBOX_KEEP);
        inbox.drain(..extra);
        let saved = serde_json::to_vec_pretty(&*inbox)
            .map_err(anyhow::Error::from)
            .and_then(|b| write_private(&self.inbox_path, &b));
        if let Err(e) = saved {
            warn!("记收件记录失败：{e:#}");
        }
    }
}

/// 回一句话就结束这条流；对方收不收得到都算处理完了
async fn reply(send: &mut SendStream, msg: FileMsg) -> Result<()> {
    if write_json(send, &msg).await.is_ok() {
        let _ = send.finish();
    }
    Ok(())
}

fn new_id() -> String {
    format!("{:016x}", rand::random::<u64>())
}

fn label(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [first, rest @ ..] => format!("{first} 等 {} 项", rest.len() + 1),
    }
}

/// 半截文件按“谁发的、哪个文件、多大、什么时候改的”起名，同一个文件再发能接上
fn partial_key(remote: &EndpointId, e: &FileEntry) -> String {
    let mut h = blake3::Hasher::new();
    h.update(remote.as_bytes());
    h.update(e.path.as_bytes());
    h.update(&e.size.to_le_bytes());
    h.update(&e.modified.to_le_bytes());
    h.finalize().to_hex()[..32].to_string()
}

/// 占住这批要收的半截文件，收完或出错时放开
struct Claim<'a> {
    set: &'a Mutex<HashSet<String>>,
    keys: Vec<String>,
}

impl<'a> Claim<'a> {
    fn take(set: &'a Mutex<HashSet<String>>, keys: &[String]) -> Option<Self> {
        let mut busy = set.lock().unwrap();
        if keys.iter().any(|k| busy.contains(k)) {
            return None;
        }
        busy.extend(keys.iter().cloned());
        Some(Self { set, keys: keys.to_vec() })
    }
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        let mut busy = self.set.lock().unwrap();
        for k in &self.keys {
            busy.remove(k);
        }
    }
}

fn clean_partials(dir: &Path, busy: &HashSet<String>) {
    let Ok(items) = std::fs::read_dir(dir) else { return };
    for item in items.flatten() {
        let old = item
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > PARTIAL_KEEP);
        if old && !busy.contains(item.file_name().to_string_lossy().as_ref()) {
            let _ = std::fs::remove_file(item.path());
        }
    }
}

/// 收到的文件放哪
enum Target {
    /// 收件目录里按发送设备分文件夹，重名自动加编号，不盖已有的
    Inbox,
    /// 明确指定的目录，同名文件盖掉（和 scp、rsync 一样）
    Dir { dir: PathBuf, notify: bool },
}

/// 对方（或命令行）给的路径换成本机的：`~` 开头从用户目录算，相对路径也从用户目录算起
pub fn resolve_path(s: &str) -> PathBuf {
    let s = s.trim();
    let home = || std::env::home_dir().unwrap_or_else(|| PathBuf::from("."));
    if s == "~" {
        return home();
    }
    if let Some(rest) = s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\")) {
        return home().join(rest);
    }
    let p = PathBuf::from(s);
    if p.is_absolute() { p } else { home().join(p) }
}

/// 接收方对清单的处理结果：每个文件清理过的相对路径，以及第一层有哪些项
struct Plan {
    rels: Vec<Vec<String>>,
    tops: Vec<Top>,
}

struct Top {
    name: String,
    file: bool,
}

impl Plan {
    fn new(entries: &[FileEntry]) -> Result<Self> {
        if entries.is_empty() {
            bail!("没有文件");
        }
        if entries.len() > MAX_ENTRIES {
            bail!("一次最多发 {MAX_ENTRIES} 个文件");
        }
        let mut rels = Vec::with_capacity(entries.len());
        let mut files = HashSet::new();
        let mut tops: Vec<Top> = Vec::new();
        for e in entries {
            let parts: Vec<String> = e
                .path
                .split('/')
                .map(safe_component)
                .collect::<Option<_>>()
                .ok_or_else(|| anyhow!("文件名不合法：{}", e.path))?;
            // 按不分大小写比，Mac 和 Windows 的磁盘默认不分
            if !files.insert(parts.join("/").to_lowercase()) {
                bail!("有重名的文件：{}", e.path);
            }
            if !tops.iter().any(|t| t.name.to_lowercase() == parts[0].to_lowercase()) {
                tops.push(Top { name: parts[0].clone(), file: parts.len() == 1 });
            }
            rels.push(parts);
        }
        // 同一个名字不能既是文件又是文件夹
        for parts in &rels {
            for k in 1..parts.len() {
                if files.contains(&parts[..k].join("/").to_lowercase()) {
                    bail!("有重名的文件：{}", parts[..k].join("/"));
                }
            }
        }
        Ok(Self { rels, tops })
    }
}

/// 把对方给的一段名字清理成本机能用的：不许 . 和 ..，换掉各系统不认的字符
fn safe_component(s: &str) -> Option<String> {
    if s.is_empty() || s == "." || s == ".." {
        return None;
    }
    let mut out: String = s
        .chars()
        .map(|c| if c.is_control() || r#"<>:"|?*\"#.contains(c) { '_' } else { c })
        .collect();
    if cfg!(windows) {
        // Windows 不认结尾的点和空格，也不认 CON、NUL 这些设备名
        out = out.trim_end_matches(['.', ' ']).to_string();
        if out.is_empty() {
            return None;
        }
        let stem = out.split('.').next().unwrap_or_default().to_ascii_uppercase();
        let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit());
        if device {
            out.insert(0, '_');
        }
    }
    Some(out)
}

/// 收齐以后挪到正式位置；第一层重名就加 (1)、(2)，不覆盖已有的；overwrite 为真时按原名放，同名文件盖掉
fn place(dest: &Path, partial_dir: &Path, plan: &Plan, keys: &[String], entries: &[FileEntry], overwrite: bool) -> Result<Vec<String>> {
    std::fs::create_dir_all(dest).with_context(|| format!("建收件目录失败：{}", dest.display()))?;
    let mut renamed = HashMap::new();
    let mut paths = Vec::new();
    for top in &plan.tops {
        let name = if overwrite { top.name.clone() } else { unique_name(dest, &top.name, top.file) };
        paths.push(dest.join(&name).display().to_string());
        renamed.insert(top.name.to_lowercase(), name);
    }
    for ((parts, key), entry) in plan.rels.iter().zip(keys).zip(entries) {
        let mut target = dest.join(&renamed[&parts[0].to_lowercase()]);
        for p in &parts[1..] {
            target.push(p);
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let from = partial_dir.join(key);
        std::fs::rename(&from, &target)
            .or_else(|_| std::fs::copy(&from, &target).and_then(|_| std::fs::remove_file(&from)))
            .with_context(|| format!("放到 {} 失败", target.display()))?;
        #[cfg(unix)]
        if entry.exec {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&target)?.permissions().mode();
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode | 0o111))?;
        }
        #[cfg(not(unix))]
        let _ = entry;
    }
    Ok(paths)
}

fn unique_name(dir: &Path, name: &str, file: bool) -> String {
    let taken = |n: &str| dir.join(n).symlink_metadata().is_ok();
    if !taken(name) {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if file && i > 0 => name.split_at(i),
        _ => (name, ""),
    };
    (1..).map(|n| format!("{stem} ({n}){ext}")).find(|c| !taken(c)).expect("总能找到不重名的")
}

/// 列出要发的文件：文件夹展开，里面的符号链接和特殊文件跳过
fn collect(paths: &[PathBuf]) -> Result<Vec<(PathBuf, FileEntry)>> {
    let mut out = Vec::new();
    let mut tops = HashSet::new();
    for path in paths {
        let mut path = std::path::absolute(path)?;
        // . 和 .. 这种没有名字，换成真实路径
        if path.file_name().is_none() {
            path = std::fs::canonicalize(&path).with_context(|| format!("找不到 {}", path.display()))?;
        }
        let meta = std::fs::metadata(&path).with_context(|| format!("找不到 {}", path.display()))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| anyhow!("不能发整个磁盘：{}", path.display()))?;
        if !tops.insert(name.to_lowercase()) {
            bail!("有两项都叫 {name}，请分开发");
        }
        if meta.is_dir() {
            walk(&path, &name, &mut out)?;
        } else if meta.is_file() {
            out.push((path.clone(), entry(name, &meta)));
        } else {
            bail!("{} 不是普通文件", path.display());
        }
    }
    if out.is_empty() {
        bail!("没有可发的文件（文件夹是空的）");
    }
    Ok(out)
}

fn walk(dir: &Path, rel: &str, out: &mut Vec<(PathBuf, FileEntry)>) -> Result<()> {
    let mut items: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("读不了文件夹 {}", dir.display()))?
        .collect::<std::io::Result<_>>()?;
    items.sort_by_key(|e| e.file_name());
    for item in items {
        // DirEntry 的 metadata 不跟符号链接，链接本身既不是文件也不是文件夹，跳过
        let meta = item.metadata()?;
        let rel = format!("{rel}/{}", item.file_name().to_string_lossy());
        if meta.is_dir() {
            walk(&item.path(), &rel, out)?;
        } else if meta.is_file() {
            out.push((item.path(), entry(rel, &meta)));
        }
        if out.len() > MAX_ENTRIES {
            bail!("文件太多了，一次最多发 {MAX_ENTRIES} 个");
        }
    }
    Ok(())
}

fn entry(path: String, meta: &std::fs::Metadata) -> FileEntry {
    FileEntry {
        path,
        size: meta.len(),
        modified: meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs()),
        exec: is_exec(meta),
    }
}

#[cfg(unix)]
fn is_exec(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_exec(_meta: &std::fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(path: &str) -> FileEntry {
        FileEntry { path: path.into(), size: 1, modified: 0, exec: false }
    }

    #[test]
    fn plan_rejects_escape_and_duplicates() {
        assert!(Plan::new(&[e("../x")]).is_err());
        assert!(Plan::new(&[e("a/./b")]).is_err());
        assert!(Plan::new(&[e("a//b")]).is_err());
        assert!(Plan::new(&[e("A.txt"), e("a.txt")]).is_err());
        assert!(Plan::new(&[e("a"), e("a/b")]).is_err());
        let plan = Plan::new(&[e("dir/x"), e("dir/y/z"), e("one.txt")]).unwrap();
        let tops: Vec<_> = plan.tops.iter().map(|t| (t.name.as_str(), t.file)).collect();
        assert_eq!(tops, [("dir", false), ("one.txt", true)]);
    }

    #[test]
    fn components_are_cleaned() {
        assert_eq!(safe_component(r"a\..\b").as_deref(), Some("a_.._b"));
        assert_eq!(safe_component("a:b?").as_deref(), Some("a_b_"));
        assert_eq!(safe_component(".."), None);
    }

    #[test]
    fn unique_names_do_not_overwrite() {
        let dir = std::env::temp_dir().join(format!("p2pcli-unique-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("folder")).unwrap();
        std::fs::write(dir.join("a.txt"), b"").unwrap();
        assert_eq!(unique_name(&dir, "a.txt", true), "a (1).txt");
        assert_eq!(unique_name(&dir, "folder", false), "folder (1)");
        assert_eq!(unique_name(&dir, "new.txt", true), "new.txt");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn paths_resolve_from_home() {
        let home = std::env::home_dir().unwrap();
        assert_eq!(resolve_path("~"), home);
        assert_eq!(resolve_path("~/a/b"), home.join("a/b"));
        assert_eq!(resolve_path("  rel/x "), home.join("rel/x"));
        #[cfg(unix)]
        assert_eq!(resolve_path("/etc/hosts"), PathBuf::from("/etc/hosts"));
        #[cfg(windows)]
        assert_eq!(resolve_path(r"C:\Windows"), PathBuf::from(r"C:\Windows"));
    }

    #[test]
    fn labels() {
        assert_eq!(label(&["a".into()]), "a");
        assert_eq!(label(&["a".into(), "b".into(), "c".into()]), "a 等 3 项");
    }
}
