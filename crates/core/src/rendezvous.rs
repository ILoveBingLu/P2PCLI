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
use iroh::{Endpoint, EndpointAddr, EndpointId, TransportAddr, Watcher};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch};
use tokio::time::timeout;
use tracing::{debug, info};

use crate::ALPN_RDV;
use crate::identity::DeviceInfo;
use crate::proto::{read_json, write_json};
use crate::rdv_ws::WsLink;
use crate::v6;

/// 软件默认用的牵线服务器（自建服务器，藏在 CDN 后面走 WebSocket）；用户可以在设置里换成自己部署的
pub const DEFAULT_SERVER: &str = "wss://p2pcli.aiqji.cn";

/// 设置里可以直接选的牵线服务器：地址、名称、说明。也可以不选，自己填
pub const SERVER_PRESETS: &[(&str, &str, &str)] = &[
    (DEFAULT_SERVER, "国内服务器", "国内节点，在国内用连得最快"),
    ("wss://p2pcli.yscopy.cn", "Cloudflare", "海外节点，适合在国外用，国内访问有时不稳"),
];

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
    /// 自建服务器，走 UDP（iroh），写法 `公钥@IP:端口` 或 `公钥@域名:端口`，可以逗号分隔写多个；
    /// 域名每次连接时现解析，服务器换了 IP 改解析就行
    Quic { id: EndpointId, addrs: Vec<String> },
    /// 走 WebSocket，可以放在 Cloudflare 这类只支持 HTTP 的平台，写法 `wss://域名`
    Ws(String),
}

/// 识别服务器地址写法：`wss://`、`ws://`、`https://` 开头的走 WebSocket，其余按 `公钥@IP或域名:端口`
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
        return parse_quic_server(s);
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

fn parse_quic_server(s: &str) -> Result<RdvServer> {
    let (id, rest) = s.split_once('@').unwrap_or((s, ""));
    let id = EndpointId::from_str(id.trim()).context("牵线服务器公钥格式不对")?;
    let mut addrs = Vec::new();
    for a in rest.split(',').map(str::trim).filter(|a| !a.is_empty()) {
        if a.parse::<SocketAddr>().is_err() {
            let host_ok = a.rsplit_once(':').is_some_and(|(host, port)| {
                !host.is_empty()
                    && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
                    && port.parse::<u16>().is_ok_and(|p| p > 0)
            });
            if !host_ok {
                bail!("牵线服务器地址格式不对：{a}（要写成 IP:端口 或 域名:端口）");
            }
        }
        addrs.push(a.to_string());
    }
    Ok(RdvServer::Quic { id, addrs })
}

/// 把 `IP:端口`、`域名:端口` 解析成连得上的地址；一个都解析不出来才算失败
async fn resolve_quic_server(id: EndpointId, addrs: &[String]) -> Result<EndpointAddr> {
    let mut ips = Vec::new();
    for a in addrs {
        match tokio::net::lookup_host(a.as_str()).await {
            Ok(found) => ips.extend(found.map(TransportAddr::Ip)),
            Err(e) => debug!("解析牵线服务器地址 {a} 失败：{e}"),
        }
    }
    if ips.is_empty() && !addrs.is_empty() {
        bail!("解析不了牵线服务器地址：{}", addrs.join(", "));
    }
    Ok(EndpointAddr::from_parts(id, ips))
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
    task: Mutex<Option<tokio::task::AbortHandle>>,
    proxy: Option<url::Url>,
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
    pub fn prepare(
        endpoint: Endpoint,
        server: RdvServer,
        me: DeviceInfo,
        data_dir: &Path,
        pushes: mpsc::UnboundedSender<RdvPush>,
        proxy: Option<url::Url>,
    ) -> Arc<Self> {
        let code_file = data_dir.join("device_code.txt");
        let cached = std::fs::read_to_string(&code_file)
            .ok()
            .and_then(|s| normalize_code(&s));
        let this = Arc::new(Self {
            proxy,
            task: Mutex::new(None),
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
        this
    }

    pub fn activate(self: &Arc<Self>) {
        let mut task = self.task.lock().unwrap();
        if task.is_none() { *task = Some(tokio::spawn(self.clone().maintain()).abort_handle()); }
    }

    pub async fn stop(&self) {
        if let Some(task) = self.task.lock().unwrap().take() { task.abort(); }
        if let Some(link) = self.link.lock().await.take() {
            match link {
                Link::Quic(c) => c.close(0u32.into(), b"configuration changed"),
                Link::Ws(w) => w.close(),
            }
        }
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
            RdvServer::Quic { id, addrs } => {
                let conn = timeout(Duration::from_secs(20), async {
                    let addr = resolve_quic_server(*id, addrs).await?;
                    self.endpoint.connect(addr, ALPN_RDV).await.map_err(|e| anyhow!("连不上牵线服务器：{e}"))
                })
                .await
                .map_err(|_| anyhow!("连接牵线服务器超时"))??;
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
                    WsLink::connect(url, self.endpoint.secret_key(), self.pushes.clone(), self.proxy.as_ref()),
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
                    let mut addr = without_symmetric_v4(&self.endpoint, self.endpoint.addr());
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

/// 本机 IPv4 出口是对称型 NAT（向不同服务器测出的公网端口不一样）时，不报测出来的公网 IPv4。
/// 别人照这个地址连进来，包能到，可本机回包从另一个端口出去，对方的 NAT 不认，握手卡死；
/// 对方同时还在走能通的路（比如经 VPN 隧道的内网地址）时，这个先到的包反而把连接绑在了死路上。
/// 打洞由 iroh 在连上之后自己换地址协商，用不着这里报。
fn without_symmetric_v4(endpoint: &Endpoint, addr: EndpointAddr) -> EndpointAddr {
    let symmetric = endpoint.net_report().get().is_some_and(|r| r.mapping_varies_by_dest_ipv4 == Some(true));
    if !symmetric {
        return addr;
    }
    EndpointAddr::from_parts(
        addr.id,
        addr.addrs.into_iter().filter(|a| match a {
            TransportAddr::Ip(SocketAddr::V4(v4)) => {
                // 内网、本机、链路本地和运营商共享地址（100.64.0.0/10，Tailscale 这类也在这段）照报
                let ip = v4.ip();
                let shared = ip.octets()[0] == 100 && ip.octets()[1] & 0xc0 == 64;
                ip.is_private() || ip.is_loopback() || ip.is_link_local() || shared
            }
            _ => true,
        }),
    )
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
        let quic = |s: &str| match parse_rdv_server(s).unwrap() {
            RdvServer::Quic { id, addrs } => (id, addrs),
            other => panic!("应该是 UDP：{other:?}"),
        };
        assert_eq!(quic(&format!("{id}@192.0.2.10:7788")), (id, vec!["192.0.2.10:7788".to_string()]));
        assert_eq!(
            quic(&format!("{id}@rdv.example.com:7788, [::1]:7788")).1,
            vec!["rdv.example.com:7788".to_string(), "[::1]:7788".to_string()]
        );
        assert!(quic(&id.to_string()).1.is_empty());
        for bad in ["rdv.example.com", "rdv.example.com:0", "rdv.example.com:99999", "a b:7788", ":7788"] {
            assert!(parse_rdv_server(&format!("{id}@{bad}")).is_err(), "{bad} 应该报错");
        }
        assert!(parse_rdv_server("bad@rdv.example.com:7788").is_err());
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

    #[tokio::test]
    async fn resolve_server_hosts() {
        let id = iroh::SecretKey::generate().public();
        let a = resolve_quic_server(id, &["127.0.0.1:7788".into(), "localhost:7789".into()]).await.unwrap();
        assert!(a.ip_addrs().any(|s| *s == "127.0.0.1:7788".parse().unwrap()));
        assert!(a.ip_addrs().any(|s| s.port() == 7789 && s.ip().is_loopback()));
        assert!(resolve_quic_server(id, &["no-such-host.invalid:7788".into()]).await.is_err());
        assert_eq!(resolve_quic_server(id, &[]).await.unwrap().ip_addrs().count(), 0);
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
