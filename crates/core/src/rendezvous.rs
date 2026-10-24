//! 牵线：设备码和设备公钥的对照、在线状态、地址。
//!
//! 牵线服务器本身也是一个 iroh 节点，客户端用它的公钥连过去，
//! 所以不用证书和域名，通信也是加密的，服务器还能直接认出是哪台设备。
//! 牵线只交换地址，终端数据不经过它。

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use iroh::endpoint::Connection;
use iroh::{Endpoint, EndpointAddr, EndpointId, TransportAddr};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch};
use tokio::time::timeout;
use tracing::{debug, info};

use crate::ALPN_RDV;
use crate::identity::DeviceInfo;
use crate::proto::{read_json, write_json};
use crate::v6;

/// 多久把自己的地址报一次（顺带保活）
const REPORT_EVERY: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum RdvRequest {
    /// 登记本机，拿设备码；同一把钥匙永远拿到同一个码
    Register { name: String, platform: String },
    /// 报告本机当前地址
    Addr { addr: EndpointAddr },
    Lookup { code: String },
    /// 请服务器叫对方马上往这些 IPv6 地址发包（见 v6 模块）
    Punch { code: String, addrs: Vec<SocketAddr> },
}

/// 服务器主动推给设备的消息，走服务器开的单向流
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum RdvPush {
    /// 有设备要用 IPv6 直连过来，往它的地址发包把本机防火墙打开
    Punch { from: EndpointId, addrs: Vec<SocketAddr> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum RdvResponse {
    Registered {
        code: String,
    },
    Ok,
    Found {
        endpoint_id: EndpointId,
        name: String,
        platform: String,
        online: bool,
        addr: Option<EndpointAddr>,
    },
    NotFound,
    Error {
        message: String,
    },
}

/// 设备码统一成 9 位数字，用户输入时的空格、横杠都去掉
pub fn normalize_code(input: &str) -> Option<String> {
    let digits: String = input.chars().filter(|c| c.is_ascii_digit()).collect();
    let rest_ok = input
        .chars()
        .all(|c| c.is_ascii_digit() || c.is_whitespace() || c == '-');
    (digits.len() == 9 && rest_ok).then_some(digits)
}

/// 显示用：123 456 789
pub fn format_code(code: &str) -> String {
    if code.len() != 9 {
        return code.to_string();
    }
    format!("{} {} {}", &code[0..3], &code[3..6], &code[6..9])
}

/// 服务器地址写法：<公钥>@<ip:端口>[,<ip:端口>...]，只写公钥也行（靠 n0 的地址查询）
pub fn parse_server(s: &str) -> Result<EndpointAddr> {
    let s = s.trim();
    let (id, addrs) = match s.split_once('@') {
        Some((id, rest)) => (id, rest),
        None => (s, ""),
    };
    let id = EndpointId::from_str(id.trim()).context("牵线服务器公钥格式不对")?;
    let mut ips = Vec::new();
    for a in addrs.split(',').map(str::trim).filter(|a| !a.is_empty()) {
        let sock = a
            .parse()
            .with_context(|| format!("牵线服务器地址格式不对：{a}"))?;
        ips.push(TransportAddr::Ip(sock));
    }
    Ok(EndpointAddr::from_parts(id, ips))
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RdvStatus {
    Connecting,
    Online,
    Offline { error: String },
}

pub struct RdvClient {
    endpoint: Endpoint,
    server: EndpointAddr,
    me: DeviceInfo,
    code_file: PathBuf,
    conn: tokio::sync::Mutex<Option<Connection>>,
    code: watch::Sender<Option<String>>,
    status: watch::Sender<RdvStatus>,
    last_addr: Mutex<Option<EndpointAddr>>,
    pushes: mpsc::UnboundedSender<RdvPush>,
}

impl RdvClient {
    /// 起后台任务：连服务器、登记、定时报地址，断了自动重连
    pub fn spawn(
        endpoint: Endpoint,
        server: EndpointAddr,
        me: DeviceInfo,
        data_dir: &Path,
        pushes: mpsc::UnboundedSender<RdvPush>,
    ) -> Arc<Self> {
        let code_file = data_dir.join("device_code.txt");
        let cached = std::fs::read_to_string(&code_file)
            .ok()
            .and_then(|s| normalize_code(&s));
        let this = Arc::new(Self {
            endpoint,
            server,
            me,
            code_file,
            conn: tokio::sync::Mutex::new(None),
            code: watch::channel(cached).0,
            status: watch::channel(RdvStatus::Connecting).0,
            last_addr: Mutex::new(None),
            pushes,
        });
        tokio::spawn(this.clone().maintain());
        this
    }

    pub fn code(&self) -> Option<String> {
        self.code.borrow().clone()
    }

    pub fn watch_code(&self) -> watch::Receiver<Option<String>> {
        self.code.subscribe()
    }

    pub fn status(&self) -> RdvStatus {
        self.status.borrow().clone()
    }

    pub fn watch_status(&self) -> watch::Receiver<RdvStatus> {
        self.status.subscribe()
    }

    async fn maintain(self: Arc<Self>) {
        let mut backoff = Duration::from_secs(1);
        loop {
            match self.session().await {
                Ok(()) => backoff = Duration::from_secs(1),
                Err(e) => {
                    debug!("牵线服务器：{e:#}");
                    self.status.send_replace(RdvStatus::Offline {
                        error: format!("{e:#}"),
                    });
                }
            }
            *self.conn.lock().await = None;
            *self.last_addr.lock().unwrap() = None;
            if self.endpoint.is_closed() {
                return;
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    }

    async fn session(&self) -> Result<()> {
        self.status.send_replace(RdvStatus::Connecting);
        let conn = timeout(
            Duration::from_secs(20),
            self.endpoint.connect(self.server.clone(), ALPN_RDV),
        )
        .await
        .map_err(|_| anyhow!("连接牵线服务器超时"))?
        .map_err(|e| anyhow!("连不上牵线服务器：{e}"))?;
        let resp = request(
            &conn,
            &RdvRequest::Register {
                name: self.me.name.clone(),
                platform: self.me.platform.clone(),
            },
        )
        .await?;
        let RdvResponse::Registered { code } = resp else {
            bail!("登记失败：{resp:?}");
        };
        if self.code() != Some(code.clone()) {
            let _ = std::fs::write(&self.code_file, &code);
            self.code.send_replace(Some(code.clone()));
        }
        info!(code = %format_code(&code), "已登记到牵线服务器");
        *self.conn.lock().await = Some(conn.clone());
        self.status.send_replace(RdvStatus::Online);

        let mut tick = tokio::time::interval(REPORT_EVERY);
        loop {
            tokio::select! {
                e = conn.closed() => bail!("和牵线服务器断开：{e}"),
                uni = conn.accept_uni() => {
                    let mut recv = uni?;
                    match read_json::<_, RdvPush>(&mut recv).await {
                        Ok(push) => { let _ = self.pushes.send(push); }
                        Err(e) => debug!("服务器推送格式不对：{e:#}"),
                    }
                }
                _ = tick.tick() => {
                    // iroh 自己会报公网 IPv6；测试时放开的局域网 IPv6 它不报，这里统一补上
                    let mut addr = self.endpoint.addr();
                    for a in v6::local_candidates(&self.endpoint) {
                        addr = addr.with_ip_addr(a);
                    }
                    let changed = self.last_addr.lock().unwrap().as_ref() != Some(&addr);
                    // 地址没变也报，顺带让服务器知道还在线
                    request(&conn, &RdvRequest::Addr { addr: addr.clone() }).await?;
                    if changed {
                        *self.last_addr.lock().unwrap() = Some(addr);
                    }
                }
            }
        }
    }

    /// 按设备码查对方
    pub async fn lookup(&self, code: &str) -> Result<RdvResponse> {
        let conn = self.connection().await?;
        request(&conn, &RdvRequest::Lookup { code: code.into() }).await
    }

    /// 请对方同时往我这些 IPv6 地址发包
    pub async fn punch(&self, code: &str, addrs: Vec<SocketAddr>) -> Result<()> {
        let conn = self.connection().await?;
        match request(&conn, &RdvRequest::Punch { code: code.into(), addrs }).await? {
            RdvResponse::Ok => Ok(()),
            other => bail!("服务器没转达：{other:?}"),
        }
    }

    async fn connection(&self) -> Result<Connection> {
        let conn = self.conn.lock().await.clone();
        Ok(match conn {
            Some(c) => c,
            None => timeout(
                Duration::from_secs(20),
                self.endpoint.connect(self.server.clone(), ALPN_RDV),
            )
            .await
            .map_err(|_| anyhow!("连接牵线服务器超时"))?
            .map_err(|e| anyhow!("连不上牵线服务器：{e}"))?,
        })
    }
}

/// 一问一答，每次开一条新流
pub async fn request(conn: &Connection, req: &RdvRequest) -> Result<RdvResponse> {
    let (mut send, mut recv) = conn.open_bi().await?;
    write_json(&mut send, req).await?;
    send.finish()?;
    let resp = timeout(Duration::from_secs(15), read_json(&mut recv))
        .await
        .map_err(|_| anyhow!("牵线服务器没回应"))??;
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes() {
        assert_eq!(normalize_code("123 456 789").as_deref(), Some("123456789"));
        assert_eq!(normalize_code("123-456-789").as_deref(), Some("123456789"));
        assert_eq!(normalize_code("12345678"), None);
        assert_eq!(normalize_code("12345678a9"), None);
        assert_eq!(format_code("123456789"), "123 456 789");
    }

    #[test]
    fn server_addr() {
        let id = iroh::SecretKey::generate().public();
        let a = parse_server(&format!("{id}@192.0.2.10:7788,[::1]:7788")).unwrap();
        assert_eq!(a.id, id);
        assert_eq!(a.ip_addrs().count(), 2);
        assert_eq!(parse_server(&id.to_string()).unwrap().ip_addrs().count(), 0);
        assert!(parse_server("bad@1.2.3.4:5").is_err());
    }
}
