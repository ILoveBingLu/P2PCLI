//! 在线探测：连接历史里的设备，一打开软件就能看到在不在线、走哪条路、延迟多少；
//! 对方同意过一次的（配对过的），还能在后台拿到会话列表（只有编号、状态和里面跑的是哪个 AI、在不在干活），不用先点连接。
//!
//! 走单独的协议号，和终端连接分开：被连方不弹窗、不记成一次连接。
//! 探测连接留着复用，下次不用重新握手，打洞找到的直连线路也一直热着，真要开终端时连得更快。
//!
//! 延迟是实时的：连接上每 2 秒发一个很小的 ping，QUIC 每收到确认就更新一次平滑往返时间，
//! 界面每次读状态都现读这个值；ping 连续失败或者对方关了连接，几秒内就标成离线。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow};
use futures_util::future::join_all;
use iroh::endpoint::Connection;
use iroh::{Endpoint, EndpointAddr, EndpointId};
use serde::{Deserialize, Serialize};
use tokio::time::timeout;
use tracing::debug;

use crate::ALPN_PRESENCE;
use crate::path::{self, PathInfo};
use crate::proto::{TerminalSession, read_json, write_json};
use crate::rendezvous::{RdvClient, RdvResponse};
use crate::store::{RecentStore, now_secs};

/// 多久完整探测一轮：连不上的设备重试、刷新会话列表
const PROBE_EVERY: Duration = Duration::from_secs(10);
/// 连着的设备多久 ping 一次，延迟跟着刷新
const PING_EVERY: Duration = Duration::from_secs(2);
/// 连着、配对过的设备每隔几次 ping 顺带问一次会话列表：会话里 AI 的状态变了要尽快知道
const SESSIONS_EVERY_PINGS: u64 = 2;
const PING_TIMEOUT: Duration = Duration::from_secs(2);
/// ping 连续失败几次就当离线
const PING_FAILS_OFFLINE: u32 = 2;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(12);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum PresenceReq {
    Sessions,
    /// 只为让 QUIC 测一次往返时间，对方回 Pong
    Ping,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum PresenceResp {
    /// 只有编号、创建时间、在不在用，不带目录和程序
    Sessions { sessions: Vec<TerminalSession> },
    /// 没配对过：只能看到在线和延迟，不给会话列表
    NotPaired,
    Pong,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PresenceState {
    /// 还没探测过
    Checking,
    Online,
    Offline,
    /// 对方在线，但软件版本太旧，不支持探测
    OldVersion,
}

/// 界面上一台历史设备的状态
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DeviceStatus {
    pub endpoint_id: String,
    pub state: PresenceState,
    pub path: Option<PathInfo>,
    /// 对方认不认我：配对过才给会话列表；还没探测到是 None
    pub paired: Option<bool>,
    /// 会话列表：刚从对方拿到的，或者上次记住的
    pub sessions: Vec<TerminalSession>,
    /// true 表示刚从对方拿到；false 表示是记住的旧列表
    pub sessions_live: bool,
    /// 这份会话列表是什么时候拿到的
    pub sessions_at: Option<u64>,
}

#[derive(Debug)]
pub enum ProbeError {
    /// 对方不在线或者连不上
    Unreachable(String),
    /// 连上了，但对方不认识探测协议，是旧版本
    OldVersion,
}

/// 建一条探测连接
pub async fn connect(endpoint: &Endpoint, addr: EndpointAddr) -> Result<Connection, ProbeError> {
    match timeout(CONNECT_TIMEOUT, endpoint.connect(addr, ALPN_PRESENCE)).await {
        Err(_) => Err(ProbeError::Unreachable("连接超时".into())),
        Ok(Ok(conn)) => Ok(conn),
        Ok(Err(e)) => {
            let text = format!("{e:#}");
            if is_unknown_protocol(&text) {
                Err(ProbeError::OldVersion)
            } else {
                Err(ProbeError::Unreachable(text))
            }
        }
    }
}

/// 连上了但对方不认这个协议号：对方的 TLS 握手会回 no_application_protocol（警报码 120），说明是旧版本
pub(crate) fn is_unknown_protocol(error: &str) -> bool {
    let lower = error.to_lowercase();
    lower.contains("application protocol") || lower.contains("alpn") || lower.contains("error 120")
}

/// 在探测连接上问一次会话列表；None 表示对方没跟我配对过
pub async fn ask_sessions(conn: &Connection) -> Result<Option<Vec<TerminalSession>>> {
    // 打开流、写入、读取任意一步卡住都不能让旧会话状态永久冒充实时状态。
    timeout(REQUEST_TIMEOUT, async {
        let (mut send, mut recv) = conn.open_bi().await.map_err(|e| anyhow!("{e}"))?;
        write_json(&mut send, &PresenceReq::Sessions).await?;
        let resp: PresenceResp = read_json(&mut recv).await?;
        let _ = send.finish();
        Ok(match resp {
            PresenceResp::Sessions { sessions } => Some(sessions),
            PresenceResp::NotPaired => None,
            PresenceResp::Pong => return Err(anyhow!("对方回了意外的消息")),
        })
    })
    .await
    .map_err(|_| anyhow!("读取会话列表超时"))?
}

/// 在探测连接上 ping 一次；整个来回都算在超时里，网断了写也可能卡住
pub async fn ping(conn: &Connection) -> Result<()> {
    timeout(PING_TIMEOUT, async {
        let (mut send, mut recv) = conn.open_bi().await.map_err(|e| anyhow!("{e}"))?;
        write_json(&mut send, &PresenceReq::Ping).await?;
        let _ = send.finish();
        match read_json(&mut recv).await? {
            PresenceResp::Pong => Ok(()),
            _ => Err(anyhow!("对方回了意外的消息")),
        }
    })
    .await
    .map_err(|_| anyhow!("ping 超时"))?
}

/// 一台设备的最新探测结果
#[derive(Clone)]
struct Seen {
    state: PresenceState,
    path: Option<PathInfo>,
    paired: Option<bool>,
    sessions: Option<(Vec<TerminalSession>, u64)>,
}

/// 连接方的后台探测：每 10 秒把连接历史里的设备都探一遍，连着的每 2 秒 ping 一次、每 4 秒问一次会话列表
pub struct Prober {
    endpoint: Endpoint,
    rdv: Option<Arc<RdvClient>>,
    recent: Arc<RecentStore>,
    conns: Mutex<HashMap<EndpointId, Connection>>,
    /// 每台设备 ping 连续失败了几次
    fails: Mutex<HashMap<EndpointId, u32>>,
    seen: Mutex<HashMap<String, Seen>>,
    tasks: Mutex<Vec<tokio::task::AbortHandle>>,
    /// 建的时候所在的 tokio 运行时。界面上点“刷新状态”是同步命令，跑在主线程上，
    /// 那里没有运行时，直接 tokio::spawn 会 panic，Windows 上整个软件闪退
    runtime: tokio::runtime::Handle,
}

impl Prober {
    pub fn new(endpoint: Endpoint, rdv: Option<Arc<RdvClient>>, recent: Arc<RecentStore>) -> Arc<Self> {
        Arc::new(Self {
            endpoint,
            rdv,
            recent,
            conns: Default::default(),
            fails: Default::default(),
            seen: Default::default(),
            tasks: Mutex::new(Vec::new()),
            runtime: tokio::runtime::Handle::current(),
        })
    }

    pub fn start(self: &Arc<Self>) {
        let mut tasks = self.tasks.lock().unwrap();
        if !tasks.is_empty() {
            return;
        }
        let this = self.clone();
        tasks.push(
            self.runtime.spawn(async move {
                // 刚启动时牵线服务器还没连上，稍等一下再探第一轮
                tokio::time::sleep(Duration::from_secs(2)).await;
                loop {
                    this.probe_all().await;
                    tokio::time::sleep(PROBE_EVERY).await;
                }
            })
            .abort_handle(),
        );
        let this = self.clone();
        tasks.push(
            self.runtime.spawn(async move {
                let mut tick = tokio::time::interval(PING_EVERY);
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                for round in 0u64.. {
                    tick.tick().await;
                    this.ping_all(round % SESSIONS_EVERY_PINGS == 0).await;
                }
            })
            .abort_handle(),
        );
    }

    pub fn stop(&self) {
        for task in self.tasks.lock().unwrap().drain(..) {
            task.abort();
        }
    }

    /// 给每条连着的探测连接 ping 一次；连不上了就马上标成离线，不等下一轮完整探测。
    /// with_sessions 时，ping 通的配对设备再问一次会话列表
    async fn ping_all(&self, with_sessions: bool) {
        let conns: Vec<(EndpointId, Connection)> =
            self.conns.lock().unwrap().iter().map(|(id, c)| (*id, c.clone())).collect();
        join_all(conns.into_iter().map(|(id, conn)| async move {
            let ok = conn.close_reason().is_none() && ping(&conn).await.is_ok();
            if ok {
                self.fails.lock().unwrap().remove(&id);
                if with_sessions {
                    self.refresh_sessions(id, &conn).await;
                }
                return;
            }
            let mut fails = self.fails.lock().unwrap();
            let n = fails.entry(id).or_default();
            *n += 1;
            if conn.close_reason().is_some() || *n >= PING_FAILS_OFFLINE {
                fails.remove(&id);
                drop(fails);
                debug!(id = %id.fmt_short(), "ping 不通，标成离线");
                conn.close(0u32.into(), b"unreachable");
                self.conns.lock().unwrap().remove(&id);
                self.record(id, Seen { state: PresenceState::Offline, path: None, paired: None, sessions: None });
            }
        }))
        .await;
    }

    /// 只更新会话列表，在线状态和线路留给 ping 和完整探测
    async fn refresh_sessions(&self, id: EndpointId, conn: &Connection) {
        let key = id.to_string();
        let paired = self.seen.lock().unwrap().get(&key).is_some_and(|s| s.state == PresenceState::Online && s.paired == Some(true));
        if !paired {
            return;
        }
        let result = ask_sessions(conn).await;
        if let Some(seen) = self.seen.lock().unwrap().get_mut(&key) {
            match result {
                Ok(Some(list)) => seen.sessions = Some((list, now_secs())),
                // 查询失败或对方不支持实时会话时，回退到缓存并标记为非实时，
                // 不能让最后一次“干活中”无限期冒充当前状态。
                Ok(None) => {
                    seen.paired = Some(false);
                    seen.sessions = None;
                }
                Err(_) => seen.sessions = None,
            }
        }
    }

    /// 马上探一轮，比如刚连过、刚删过历史；哪个线程调都行
    pub fn refresh_now(self: &Arc<Self>) {
        let this = self.clone();
        self.runtime.spawn(async move { this.probe_all().await });
    }

    /// 连接历史里的设备，每台只算一次（同一台可能换过设备码）
    fn devices(&self) -> Vec<(EndpointId, String)> {
        let mut out: Vec<(EndpointId, String)> = Vec::new();
        for d in self.recent.list() {
            let Ok(id) = d.endpoint_id.parse::<EndpointId>() else { continue };
            if id != self.endpoint.id() && !out.iter().any(|(i, _)| *i == id) {
                out.push((id, d.code));
            }
        }
        out
    }

    async fn probe_all(&self) {
        let devices = self.devices();
        // 已经不在历史里的设备，探测连接也关掉
        self.conns.lock().unwrap().retain(|id, conn| {
            let keep = devices.iter().any(|(d, _)| d == id);
            if !keep {
                conn.close(0u32.into(), b"forgotten");
            }
            keep
        });
        join_all(devices.into_iter().map(|(id, code)| self.probe_one(id, code))).await;
    }

    async fn probe_one(&self, id: EndpointId, code: String) {
        let conn = match self.connection(id, &code).await {
            Ok(conn) => conn,
            Err(state) => return self.record(id, Seen { state, path: None, paired: None, sessions: None }),
        };
        match ask_sessions(&conn).await {
            Ok(sessions) => {
                if let Some(list) = &sessions
                    && let Err(e) = self.recent.save_sessions(&id.to_string(), list)
                {
                    debug!("记会话列表失败：{e:#}");
                }
                self.record(id, Seen {
                    state: PresenceState::Online,
                    path: Some(path::current(&conn)),
                    paired: Some(sessions.is_some()),
                    sessions: sessions.map(|s| (s, now_secs())),
                });
            }
            Err(e) => {
                // 连接可能刚断，丢掉它，下一轮重新建
                debug!(id = %id.fmt_short(), "探测请求失败：{e:#}");
                self.conns.lock().unwrap().remove(&id);
                self.record(id, Seen { state: PresenceState::Offline, path: None, paired: None, sessions: None });
            }
        }
    }

    /// 复用还活着的探测连接；没有就先问牵线服务器在不在线、拿地址，再连
    async fn connection(&self, id: EndpointId, code: &str) -> Result<Connection, PresenceState> {
        let alive = self.conns.lock().unwrap().get(&id).filter(|c| c.close_reason().is_none()).cloned();
        if let Some(conn) = alive {
            return Ok(conn);
        }
        let mut addr = EndpointAddr::new(id);
        if let Some(rdv) = &self.rdv
            && let Ok(Ok(found)) = timeout(Duration::from_secs(8), rdv.lookup(code)).await
        {
            match found {
                RdvResponse::Found { endpoint_id, online, addr: found, .. } if endpoint_id == id => {
                    if !online {
                        return Err(PresenceState::Offline);
                    }
                    if let Some(found) = found.filter(|a| a.id == id) {
                        addr = found;
                    }
                }
                // 设备码对不上这台设备，或者查无此码：照样按公钥试，局域网里可能找得到
                _ => {}
            }
        }
        match connect(&self.endpoint, addr).await {
            Ok(conn) => {
                self.fails.lock().unwrap().remove(&id);
                self.conns.lock().unwrap().insert(id, conn.clone());
                Ok(conn)
            }
            Err(ProbeError::OldVersion) => Err(PresenceState::OldVersion),
            Err(ProbeError::Unreachable(e)) => {
                debug!(id = %id.fmt_short(), "探测连不上：{e}");
                Err(PresenceState::Offline)
            }
        }
    }

    fn record(&self, id: EndpointId, seen: Seen) {
        self.seen.lock().unwrap().insert(id.to_string(), seen);
    }

    /// 连接历史里每台设备现在的状态；会话列表没有新的就用记住的
    pub fn statuses(&self) -> Vec<DeviceStatus> {
        let conns = self.conns.lock().unwrap();
        let seen = self.seen.lock().unwrap();
        self.devices()
            .into_iter()
            .map(|(id, _)| {
                let key = id.to_string();
                let s = seen.get(&key);
                let (sessions, live, at) = match s.and_then(|s| s.sessions.clone()) {
                    Some((list, at)) => (list, true, Some(at)),
                    None => match self.recent.sessions(&key) {
                        Some(c) => (c.sessions, false, Some(c.at)),
                        None => (Vec::new(), false, None),
                    },
                };
                let state = s.map_or(PresenceState::Checking, |s| s.state);
                // 在线就现读连接上的往返时间，是实时的
                let live_path = (state == PresenceState::Online)
                    .then(|| conns.get(&id).filter(|c| c.close_reason().is_none()).map(path::current))
                    .flatten();
                DeviceStatus {
                    endpoint_id: key,
                    state,
                    path: live_path.or_else(|| s.and_then(|s| s.path.clone())),
                    paired: s.and_then(|s| s.paired),
                    sessions,
                    sessions_live: live,
                    sessions_at: at,
                }
            })
            .collect()
    }
}

impl Drop for Prober {
    fn drop(&mut self) {
        self.stop();
    }
}
