//! 一台设备上的总入口：既能被连，也能去连别人。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use iroh::endpoint::{Connection, presets};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode, RelayUrl, TransportAddr};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::client::{self, ConnectError, RemoteHost};
use crate::host::{Host, HostDelegate};
use crate::identity::{DeviceInfo, load_or_create_key};
use crate::lan::{Lan, NearbyDevice};
use crate::proto::RejectCode;
use crate::rendezvous::{RdvClient, RdvPush, RdvResponse, normalize_code, parse_rdv_server};
use crate::store::{RecentDevice, RecentStore, now_secs};
use crate::{ALPN_PUNCH, ALPN_TERM, v6};

/// IPv6 直连先跑这么久，还没通就同时走常规流程（中继加打洞），谁先通用谁
const V6_HEAD_START: Duration = Duration::from_millis(1500);
/// 被连方配合发包持续多久
const PUNCH_FOR: Duration = Duration::from_secs(5);
/// 同一台设备的发包请求，间隔太短的不理
const PUNCH_MIN_GAP: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Default)]
pub struct NodeConfig {
    pub data_dir: PathBuf,
    pub device_name: Option<String>,
    /// Absolute bundled tmux/psmux path supplied by the desktop application.
    pub session_program: Option<PathBuf>,
    /// 牵线服务器，见 [`parse_rdv_server`]
    pub server: Option<String>,
    /// HTTP proxy for WebSocket rendezvous and HTTP(S) relay traffic.
    pub proxy: Option<String>,
    /// 测试中继用：完全不走直连
    pub relay_only: bool,
    /// 测试打洞用：只绑这个本机地址（比如绕开隧道网卡）
    pub bind_ip: Option<std::net::IpAddr>,
    /// 自建中继地址；空着就用 iroh 官方的公共中继
    pub relays: Vec<String>,
    /// 测试用：不连 iroh 的公共地址查询和中继，只用明确给出的地址，结果不受外网影响
    pub offline: bool,
    /// 局域网发现：看到同一网络里的 P2PCLI 设备，按公钥直接连，不经过牵线服务器
    pub lan: bool,
    /// 让同一网络里的其他设备看到本机（要先开 lan）
    pub lan_visible: bool,
}

pub struct Node {
    config: NodeConfig,
    pub endpoint: Endpoint,
    pub me: DeviceInfo,
    pub host: Arc<Host>,
    pub rdv: Option<Arc<RdvClient>>,
    /// 局域网发现，没开或者没启动成功是 None
    pub lan: Option<Arc<Lan>>,
    pub recent: Arc<RecentStore>,
    /// 配合对方 IPv6 直连发过几次包，排查和测试用
    pub punches: Arc<std::sync::atomic::AtomicU64>,
}

impl Node {
    pub async fn start(cfg: NodeConfig, delegate: Arc<dyn HostDelegate>) -> Result<Arc<Self>> {
        let node = Self::prepare(cfg, delegate, None, None, None, None).await?;
        node.activate();
        Ok(node)
    }

    pub fn transport_changed(&self, cfg: &NodeConfig) -> bool {
        self.config.proxy != cfg.proxy || self.config.relays != cfg.relays ||
        self.config.bind_ip != cfg.bind_ip || self.config.offline != cfg.offline || self.config.relay_only != cfg.relay_only ||
        self.config.lan != cfg.lan || self.config.lan_visible != cfg.lan_visible
    }

    pub async fn prepare_update(&self, cfg: NodeConfig, delegate: Arc<dyn HostDelegate>) -> Result<Arc<Self>> {
        let (endpoint, lan) = if self.transport_changed(&cfg) { (None, None) } else { (Some(self.endpoint.clone()), self.lan.clone()) };
        Self::prepare(cfg, delegate, Some(self.host.clone()), endpoint, lan, Some(self.recent.clone())).await
    }

    pub fn activate(&self) {
        self.host.set_info(self.me.clone());
        if let Some(rdv) = &self.rdv { rdv.activate(); }
    }

    pub async fn stop_rendezvous(&self) {
        if let Some(rdv) = &self.rdv { rdv.stop().await; }
    }

    async fn prepare(cfg: NodeConfig, delegate: Arc<dyn HostDelegate>, shared_host: Option<Arc<Host>>, existing_endpoint: Option<Endpoint>, existing_lan: Option<Arc<Lan>>, recent: Option<Arc<RecentStore>>) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&cfg.data_dir)
            .with_context(|| format!("建数据目录失败：{}", cfg.data_dir.display()))?;
        let server = cfg.server.as_deref().filter(|s| !s.trim().is_empty()).map(parse_rdv_server).transpose()?;
        let key = load_or_create_key(&cfg.data_dir)?;
        let proxy = cfg.proxy.as_deref().map(crate::proxy::parse_proxy).transpose()?;
        let mut builder = Endpoint::builder(presets::N0)
            .secret_key(key)
            .alpns(vec![ALPN_TERM.to_vec()])
            .path_selector(std::sync::Arc::new(crate::path::Ipv6First));
        if let Some(proxy) = &proxy {
            builder = builder.proxy_url(proxy.clone());
        }
        if cfg.relay_only {
            builder = builder.clear_ip_transports();
        }
        let relays = cfg
            .relays
            .iter()
            .map(|u| u.trim())
            .filter(|u| !u.is_empty())
            .map(|u| u.parse::<RelayUrl>().map_err(|e| anyhow!("中继地址格式不对：{u}（{e}）")))
            .collect::<Result<Vec<_>>>()?;
        if !relays.is_empty() {
            builder = builder.relay_mode(RelayMode::custom(relays));
        }
        if cfg.offline {
            builder = builder.clear_address_lookup().relay_mode(RelayMode::Disabled);
        }
        if let Some(ip) = cfg.bind_ip {
            builder = builder
                .bind_addr(std::net::SocketAddr::new(ip, 0))
                .map_err(|e| anyhow!("绑定地址不对：{e}"))?;
        }
        let start_accept = existing_endpoint.is_none();
        let endpoint = match existing_endpoint {
            Some(endpoint) => endpoint,
            None => builder.bind().await.context("启动网络失败")?,
        };
        let me = DeviceInfo::local(cfg.device_name.clone());
        let lan = match existing_lan {
            Some(lan) => {
                lan.set_info(&me);
                Some(lan)
            }
            None if cfg.lan && !cfg.offline => match Lan::start(&endpoint, &me, cfg.lan_visible).await {
                Ok(lan) => Some(lan),
                Err(e) => {
                    warn!("局域网发现没启动：{e:#}");
                    None
                }
            },
            None => None,
        };
        let host = shared_host.unwrap_or_else(|| Arc::new(Host::new(me.clone(), &cfg.data_dir, &endpoint.id().to_string(), cfg.session_program.clone(), delegate)));

        if start_accept {
        let ep = endpoint.clone();
        let h = host.clone();
        tokio::spawn(async move {
            while let Some(incoming) = ep.accept().await {
                let h = h.clone();
                tokio::spawn(async move {
                    let conn = match incoming.accept() {
                        Ok(a) => match a.await {
                            Ok(c) => c,
                            Err(e) => return debug!("握手失败：{e}"),
                        },
                        Err(e) => return debug!("拒收连接：{e}"),
                    };
                    if conn.alpn() == ALPN_TERM {
                        h.handle(conn).await;
                    }
                });
            }
        });

        }

        let (push_tx, push_rx) = mpsc::unbounded_channel();
        let rdv = match server {
            Some(s) => Some(RdvClient::prepare(
                endpoint.clone(),
                s,
                me.clone(),
                &cfg.data_dir,
                push_tx,
                proxy,
            )),
            None => None,
        };
        let punches = Arc::new(std::sync::atomic::AtomicU64::new(0));
        spawn_punch_handler(endpoint.clone(), push_rx, punches.clone());

        Ok(Arc::new(Self {
            endpoint,
            me,
            host,
            rdv,
            lan,
            recent: recent.unwrap_or_else(|| Arc::new(RecentStore::open(&cfg.data_dir))),
            config: cfg,
            punches,
        }))
    }

    pub fn id(&self) -> iroh::EndpointId {
        self.endpoint.id()
    }

    /// 输设备码连
    pub async fn connect_code(&self, code: &str, password: Option<String>) -> Result<RemoteHost, ConnectError> {
        let code = normalize_code(code).ok_or_else(|| anyhow!("设备码是 9 位数字"))?;
        let rdv = self
            .rdv
            .as_ref()
            .ok_or_else(|| anyhow!("没有配置牵线服务器，没法用设备码连接"))?;
        let found = rdv.lookup(&code).await?;
        let (id, online, addr) = match found {
            RdvResponse::Found {
                endpoint_id,
                online,
                addr,
                ..
            } => (endpoint_id, online, addr),
            RdvResponse::NotFound => return Err(anyhow!("没有这个设备码").into()),
            RdvResponse::Error { message } => return Err(anyhow!("{message}").into()),
            other => return Err(anyhow!("牵线服务器回了意外的结果：{other:?}").into()),
        };
        if id == self.id() {
            return Err(anyhow!("这是本机的设备码").into());
        }
        if let Some(prev) = self.recent.get(&code)
            && prev.endpoint_id != id.to_string()
        {
            return Err(anyhow!("这个设备码对应的设备和上次不一样，可能有人冒充，已停止连接").into());
        }
        if !online {
            return Err(anyhow!("对方设备不在线").into());
        }
        let addr = addr.filter(|a| a.id == id).unwrap_or_else(|| EndpointAddr::new(id));
        debug!(?addr, "按设备码查到对方地址");
        let conn = self.dial(rdv, &code, addr).await?;
        let remote = self
            .with_saved_password(id, password, |pw| client::handshake(conn, &self.me, pw))
            .await?;
        if let Err(e) = self.recent.upsert(RecentDevice {
            code,
            endpoint_id: id.to_string(),
            name: remote.info.name.clone(),
            platform: remote.info.platform.clone(),
            last_used: now_secs(),
        }) {
            warn!("记最近设备失败：{e:#}");
        }
        Ok(remote)
    }

    /// 两边都有公网 IPv6 时，请对方同时往我这边发包，自己直接连对方的 IPv6；
    /// 1.5 秒还没通就并行走常规流程，哪条先通用哪条。
    async fn dial(&self, rdv: &RdvClient, code: &str, addr: EndpointAddr) -> Result<Connection> {
        let mine = v6::local_candidates(&self.endpoint);
        let theirs = v6::pick(addr.ip_addrs());
        if mine.is_empty() || theirs.is_empty() {
            debug!(mine = mine.len(), theirs = theirs.len(), "有一边没有公网 IPv6，走常规流程");
            return client::dial(&self.endpoint, addr).await;
        }
        if let Err(e) = rdv.punch(code, mine).await {
            debug!("没能请对方配合发包：{e:#}");
        }
        let direct = EndpointAddr::from_parts(addr.id, theirs.iter().copied().map(TransportAddr::Ip));
        let v6_try = self.endpoint.connect(direct, ALPN_TERM);
        let normal = async {
            tokio::time::sleep(V6_HEAD_START).await;
            client::dial(&self.endpoint, addr).await
        };
        tokio::pin!(v6_try, normal);
        let (mut v6_done, mut normal_err) = (false, None);
        loop {
            tokio::select! {
                r = &mut v6_try, if !v6_done => match r {
                    Ok(conn) => {
                        // iroh 可能自己补上中继地址先从中继通；照用，选路规则会在 IPv6 路打开后切过去
                        info!(path = ?crate::path::current(&conn).kind, "IPv6 直连这一路先连上");
                        return Ok(conn);
                    }
                    Err(e) => {
                        debug!("IPv6 直连没通：{e}");
                        v6_done = true;
                        if let Some(e) = normal_err.take() {
                            return Err(e);
                        }
                    }
                },
                r = &mut normal, if normal_err.is_none() => match r {
                    Ok(conn) => return Ok(conn),
                    Err(e) if v6_done => return Err(e),
                    Err(e) => normal_err = Some(e),
                },
            }
        }
    }

    /// 局域网里看到的设备
    pub fn nearby(&self) -> Vec<NearbyDevice> {
        self.lan.as_ref().map(|l| l.devices()).unwrap_or_default()
    }

    /// 按公钥连：局域网里发现的设备，或者连接历史里正好在局域网在线的设备。
    /// 地址交给 iroh 去查（局域网 mDNS 优先，也会查公共地址），不经过牵线服务器。
    pub async fn connect_endpoint(&self, id: EndpointId, password: Option<String>) -> Result<RemoteHost, ConnectError> {
        if id == self.id() {
            return Err(anyhow!("这是本机").into());
        }
        let remote = self
            .with_saved_password(id, password, |pw| client::connect(&self.endpoint, EndpointAddr::new(id), &self.me, pw))
            .await?;
        // 只更新已有的历史记录：局域网广播里的信息没法核实，不拿它新建记录
        if let Err(e) = self.recent.touch(&id.to_string(), &remote.info.name, &remote.info.platform) {
            warn!("更新连接历史失败：{e:#}");
        }
        Ok(remote)
    }

    /// 没填密码就用上次连上时记住的；连上了就记住这次的密码。
    /// 记住的密码被拒（对方换了密码）就忘掉它，按“需要密码”返回，让用户重新输。
    async fn with_saved_password<F, Fut>(&self, id: EndpointId, given: Option<String>, connect: F) -> Result<RemoteHost, ConnectError>
    where
        F: FnOnce(Option<String>) -> Fut,
        Fut: std::future::Future<Output = Result<RemoteHost, ConnectError>>,
    {
        let key = id.to_string();
        let saved = if given.is_none() { self.recent.password(&key) } else { None };
        let used_saved = saved.is_some();
        let password = given.or(saved);
        match connect(password.clone()).await {
            Ok(remote) => {
                if let Some(p) = &password
                    && let Err(e) = self.recent.save_password(&key, p)
                {
                    warn!("记住配对密码失败：{e:#}");
                }
                Ok(remote)
            }
            Err(ConnectError::Rejected(RejectCode::BadPassword)) if used_saved => {
                info!(id = %id.fmt_short(), "记住的配对密码对方不认了，忘掉它");
                if let Err(e) = self.recent.forget_password(&key) {
                    warn!("删除记住的配对密码失败：{e:#}");
                }
                Err(ConnectError::Rejected(RejectCode::NeedPassword))
            }
            Err(e) => Err(e),
        }
    }

    /// 直接用对方公钥（加可选地址）连，开发和测试用
    pub async fn connect_addr(&self, addr: EndpointAddr, password: Option<String>) -> Result<RemoteHost, ConnectError> {
        client::connect(&self.endpoint, addr, &self.me, password).await
    }

    /// 本机完整地址，给不走牵线服务器的测试用
    pub async fn ticket(&self) -> Result<String> {
        self.endpoint.online().await;
        let addr = self.endpoint.addr();
        let ips: Vec<String> = addr.ip_addrs().map(|a| a.to_string()).collect();
        if ips.is_empty() && addr.relay_urls().next().is_none() {
            bail!("还没拿到本机地址");
        }
        Ok(format!("{}@{}", addr.id, ips.join(",")))
    }

    pub async fn shutdown(&self) {
        self.stop_rendezvous().await;
        self.host.kick_all();
        self.endpoint.close().await;
    }
}

/// 被连方：服务器转来“有人要 IPv6 直连”，就往对方地址发几个包，把本机防火墙打开。
fn spawn_punch_handler(
    endpoint: Endpoint,
    mut rx: mpsc::UnboundedReceiver<RdvPush>,
    punches: Arc<std::sync::atomic::AtomicU64>,
) {
    tokio::spawn(async move {
        let mut last: HashMap<EndpointId, Instant> = HashMap::new();
        while let Some(RdvPush::Punch { from, addrs }) = rx.recv().await {
            let now = Instant::now();
            if from == endpoint.id() || last.get(&from).is_some_and(|t| now.duration_since(*t) < PUNCH_MIN_GAP) {
                continue;
            }
            last.insert(from, now);
            let theirs = v6::pick(&addrs);
            if theirs.is_empty() || v6::local_candidates(&endpoint).is_empty() {
                continue;
            }
            info!(from = %from.fmt_short(), ?theirs, "对方要 IPv6 直连，先往它那边发包");
            punches.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let target = EndpointAddr::from_parts(from, theirs.into_iter().map(TransportAddr::Ip));
            let ep = endpoint.clone();
            tokio::spawn(async move {
                // 对方不收这个协议号，这条连接注定建不成；要的只是这几个包把本机防火墙打开
                let _ = tokio::time::timeout(PUNCH_FOR, ep.connect(target, ALPN_PUNCH)).await;
            });
        }
    });
}
