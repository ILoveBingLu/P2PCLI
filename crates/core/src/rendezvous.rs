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
use crate::rdv_ws::WsLink;
use crate::v6;

/// 多久看一次自己的地址有没有变，变了就报
const CHECK_ADDR_EVERY: Duration = Duration::from_secs(20);
/// 地址没变也至少这么久报一次，顺带保活（WebSocket 太久没数据会被中间设备断开）
const REPORT_AT_LEAST: Duration = Duration::from_secs(60);

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

/// 牵线服务器的两种接法
#[derive(Debug, Clone, PartialEq)]
pub enum RdvServer {
    /// 自建服务器，走 UDP（iroh），写法 `公钥@IP:端口`
    Quic(EndpointAddr),
    /// 走 WebSocket，可以放在 Cloudflare 这类只支持 HTTP 的平台，写法 `wss://域名`
    Ws(String),
}

/// 识别服务器地址写法：`wss://`、`ws://`、`https://` 开头的走 WebSocket，其余按 `公钥@IP:端口`
pub fn parse_rdv_server(s: &str) -> Result<RdvServer> {
    let s = s.trim();
    let ws = if let Some(rest) = s.strip_prefix("https://") {
        Some(format!("wss://{rest}"))
    } else if let Some(rest) = s.strip_prefix("http://") {
        Some(format!("ws://{rest}"))
    } else if s.starts_with("wss://") || s.starts_with("ws://") {
        Some(s.to_string())
    } else {
        None
    };
    let Some(mut url) = ws else {
        return Ok(RdvServer::Quic(parse_server(s)?));
    };
    // 只写了域名就补上默认路径
    let after_scheme = url.split_once("://").map(|(_, r)| r).unwrap_or("");
    match after_scheme.find('/') {
        None => url.push_str(crate::rdv_ws::WS_PATH),
        Some(i) if after_scheme[i..] == *"/" => {
            url.pop();
            url.push_str(crate::rdv_ws::WS_PATH);
        }
        _ => {}
    }
    let host = url.split_once("://").map(|(_, r)| r.split('/').next().unwrap_or("")).unwrap_or("");
    if host.is_empty() {
        bail!("牵线服务器地址里没有域名");
    }
    Ok(RdvServer::Ws(url))
}

/// 和牵线服务器的一条连接，UDP 或 WebSocket
#[derive(Clone)]
enum Link {
    Quic(Connection),
    Ws(Arc<WsLink>),
}

impl Link {
    async fn request(&self, req: &RdvRequest) -> Result<RdvResponse> {
        match self {
            Link::Quic(c) => request(c, req).await,
            Link::Ws(w) => w.request(req).await,
        }
    }

    async fn closed(&self) -> String {
        match self {
            Link::Quic(c) => c.closed().await.to_string(),
            Link::Ws(w) => w.closed().await,
        }
    }
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
    server: RdvServer,
    me: DeviceInfo,
    code_file: PathBuf,
    link: tokio::sync::Mutex<Option<Link>>,
    code: watch::Sender<Option<String>>,
    status: watch::Sender<RdvStatus>,
    last_addr: Mutex<Option<EndpointAddr>>,
    pushes: mpsc::UnboundedSender<RdvPush>,
}

impl RdvClient {
    /// 起后台任务：连服务器、登记、定时报地址，断了自动重连
    pub fn spawn(
        endpoint: Endpoint,
        server: RdvServer,
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
            link: tokio::sync::Mutex::new(None),
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
            *self.link.lock().await = None;
            *self.last_addr.lock().unwrap() = None;
            if self.endpoint.is_closed() {
                return;
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    }

    /// 新开一条到牵线服务器的连接；服务器的推送都转到 pushes
    async fn open_link(&self) -> Result<Link> {
        match &self.server {
            RdvServer::Quic(addr) => {
                let conn = timeout(Duration::from_secs(20), self.endpoint.connect(addr.clone(), ALPN_RDV))
                    .await
                    .map_err(|_| anyhow!("连接牵线服务器超时"))?
                    .map_err(|e| anyhow!("连不上牵线服务器：{e}"))?;
                let c = conn.clone();
                let pushes = self.pushes.clone();
                tokio::spawn(async move {
                    while let Ok(mut recv) = c.accept_uni().await {
                        match read_json::<_, RdvPush>(&mut recv).await {
                            Ok(push) => {
                                let _ = pushes.send(push);
                            }
                            Err(e) => debug!("服务器推送格式不对：{e:#}"),
                        }
                    }
                });
                Ok(Link::Quic(conn))
            }
            RdvServer::Ws(url) => {
                let link = timeout(
                    Duration::from_secs(20),
                    WsLink::connect(url, self.endpoint.secret_key(), self.pushes.clone()),
                )
                .await
                .map_err(|_| anyhow!("连接牵线服务器超时"))??;
                Ok(Link::Ws(link))
            }
        }
    }

    async fn session(&self) -> Result<()> {
        self.status.send_replace(RdvStatus::Connecting);
        let link = self.open_link().await?;
        let resp = link
            .request(&RdvRequest::Register {
                name: self.me.name.clone(),
                platform: self.me.platform.clone(),
            })
            .await?;
        let RdvResponse::Registered { code } = resp else {
            bail!("登记失败：{resp:?}");
        };
        if self.code() != Some(code.clone()) {
            let _ = std::fs::write(&self.code_file, &code);
            self.code.send_replace(Some(code.clone()));
        }
        info!(code = %format_code(&code), "已登记到牵线服务器");
        *self.link.lock().await = Some(link.clone());
        self.status.send_replace(RdvStatus::Online);

        let mut tick = tokio::time::interval(CHECK_ADDR_EVERY);
        let mut last_sent: Option<tokio::time::Instant> = None;
        loop {
            tokio::select! {
                reason = link.closed() => bail!("和牵线服务器断开：{reason}"),
                _ = tick.tick() => {
                    // iroh 自己会报公网 IPv6；测试时放开的局域网 IPv6 它不报，这里统一补上
                    let mut addr = self.endpoint.addr();
                    for a in v6::local_candidates(&self.endpoint) {
                        addr = addr.with_ip_addr(a);
                    }
                    let changed = self.last_addr.lock().unwrap().as_ref() != Some(&addr);
                    let due = last_sent.is_none_or(|t| t.elapsed() >= REPORT_AT_LEAST);
                    if changed || due {
                        link.request(&RdvRequest::Addr { addr: addr.clone() }).await?;
                        last_sent = Some(tokio::time::Instant::now());
                        *self.last_addr.lock().unwrap() = Some(addr);
                    }
                }
            }
        }
    }

    /// 按设备码查对方
    pub async fn lookup(&self, code: &str) -> Result<RdvResponse> {
        self.current_link().await?.request(&RdvRequest::Lookup { code: code.into() }).await
    }

    /// 请对方同时往我这些 IPv6 地址发包
    pub async fn punch(&self, code: &str, addrs: Vec<SocketAddr>) -> Result<()> {
        match self
            .current_link()
            .await?
            .request(&RdvRequest::Punch { code: code.into(), addrs })
            .await?
        {
            RdvResponse::Ok => Ok(()),
            other => bail!("服务器没转达：{other:?}"),
        }
    }

    /// 登记用的那条连接还在就复用；正在登记就稍等一下；都不行再临时开一条
    async fn current_link(&self) -> Result<Link> {
        if let Some(l) = self.link.lock().await.clone() {
            return Ok(l);
        }
        let mut status = self.watch_status();
        // 只要个结果，别把状态的读锁带过下面的等待
        let settled = matches!(
            timeout(Duration::from_secs(5), status.wait_for(|s| !matches!(s, RdvStatus::Connecting))).await,
            Ok(Ok(_))
        );
        if settled && let Some(l) = self.link.lock().await.clone() {
            return Ok(l);
        }
        self.open_link().await
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
    fn rdv_server_kinds() {
        let id = iroh::SecretKey::generate().public();
        assert!(matches!(parse_rdv_server(&format!("{id}@192.0.2.10:7788")).unwrap(), RdvServer::Quic(_)));
        let ws = |s: &str| match parse_rdv_server(s).unwrap() {
            RdvServer::Ws(u) => u,
            other => panic!("应该是 WebSocket：{other:?}"),
        };
        assert_eq!(ws("wss://rdv.example.com"), "wss://rdv.example.com/v1/ws");
        assert_eq!(ws("https://rdv.example.com/"), "wss://rdv.example.com/v1/ws");
        assert_eq!(ws("ws://127.0.0.1:8787"), "ws://127.0.0.1:8787/v1/ws");
        assert_eq!(ws("wss://rdv.example.com/custom/ws"), "wss://rdv.example.com/custom/ws");
        assert!(parse_rdv_server("wss://").is_err());
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
