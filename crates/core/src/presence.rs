//! 在线探测：连接历史里的设备，一打开软件就能看到在不在线、走哪条路、延迟多少；
//! 对方同意过一次的（配对过的），还能在后台拿到会话列表（只有编号和状态），不用先点连接。
//!
//! 走单独的协议号，和终端连接分开：被连方不弹窗、不记成一次连接。
//! 探测连接留着复用，下次不用重新握手，打洞找到的直连线路也一直热着，真要开终端时连得更快。

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

/// 多久探测一轮
const PROBE_EVERY: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(12);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum PresenceReq {
    Sessions,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum PresenceResp {
    /// 只有编号、创建时间、在不在用，不带目录和程序
    Sessions { sessions: Vec<TerminalSession> },
    /// 没配对过：只能看到在线和延迟，不给会话列表
    NotPaired,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
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
            // 对方的 TLS 握手里没有这个协议号，会回 no_application_protocol（警报码 120）
            let lower = text.to_lowercase();
            if lower.contains("application protocol") || lower.contains("alpn") || lower.contains("error 120") {
                Err(ProbeError::OldVersion)
            } else {
                Err(ProbeError::Unreachable(text))
            }
        }
    }
}

/// 在探测连接上问一次会话列表；None 表示对方没跟我配对过
pub async fn ask_sessions(conn: &Connection) -> Result<Option<Vec<TerminalSession>>> {
    let (mut send, mut recv) = conn.open_bi().await.map_err(|e| anyhow!("{e}"))?;
    write_json(&mut send, &PresenceReq::Sessions).await?;
    let resp: PresenceResp = timeout(REQUEST_TIMEOUT, read_json(&mut recv))
        .await
        .map_err(|_| anyhow!("对方没回应"))??;
    let _ = send.finish();
    Ok(match resp {
        PresenceResp::Sessions { sessions } => Some(sessions),
        PresenceResp::NotPaired => None,
    })
}

/// 一台设备的最新探测结果
#[derive(Clone)]
struct Seen {
    state: PresenceState,
    path: Option<PathInfo>,
    paired: Option<bool>,
    sessions: Option<(Vec<TerminalSession>, u64)>,
}

/// 连接方的后台探测：每 30 秒把连接历史里的设备都探一遍
pub struct Prober {
    endpoint: Endpoint,
    rdv: Option<Arc<RdvClient>>,
    recent: Arc<RecentStore>,
    conns: tokio::sync::Mutex<HashMap<EndpointId, Connection>>,
    seen: Mutex<HashMap<String, Seen>>,
    task: Mutex<Option<tokio::task::AbortHandle>>,
}

impl Prober {
    pub fn new(endpoint: Endpoint, rdv: Option<Arc<RdvClient>>, recent: Arc<RecentStore>) -> Arc<Self> {
        Arc::new(Self {
            endpoint,
            rdv,
            recent,
            conns: Default::default(),
            seen: Default::default(),
            task: Mutex::new(None),
        })
    }

    pub fn start(self: &Arc<Self>) {
        let mut task = self.task.lock().unwrap();
        if task.is_some() {
            return;
        }
        let this = self.clone();
        *task = Some(
            tokio::spawn(async move {
                // 刚启动时牵线服务器还没连上，稍等一下再探第一轮
                tokio::time::sleep(Duration::from_secs(2)).await;
                loop {
                    this.probe_all().await;
                    tokio::time::sleep(PROBE_EVERY).await;
                }
            })
            .abort_handle(),
        );
    }

    pub fn stop(&self) {
        if let Some(task) = self.task.lock().unwrap().take() {
            task.abort();
        }
    }

    /// 马上探一轮，比如刚连过、刚删过历史
    pub fn refresh_now(self: &Arc<Self>) {
        let this = self.clone();
        tokio::spawn(async move { this.probe_all().await });
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
        self.conns.lock().await.retain(|id, conn| {
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
                self.conns.lock().await.remove(&id);
                self.record(id, Seen { state: PresenceState::Offline, path: None, paired: None, sessions: None });
            }
        }
    }

    /// 复用还活着的探测连接；没有就先问牵线服务器在不在线、拿地址，再连
    async fn connection(&self, id: EndpointId, code: &str) -> Result<Connection, PresenceState> {
        if let Some(conn) = self.conns.lock().await.get(&id)
            && conn.close_reason().is_none()
        {
            return Ok(conn.clone());
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
                self.conns.lock().await.insert(id, conn.clone());
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
                DeviceStatus {
                    endpoint_id: key,
                    state: s.map_or(PresenceState::Checking, |s| s.state),
                    path: s.and_then(|s| s.path.clone()),
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
