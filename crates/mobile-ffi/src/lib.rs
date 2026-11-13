//! 手机端（Android 的 Kotlin、iOS 的 Swift）调的 Rust 接口。
//!
//! 界面每个平台各写一套，连接、协议、会话、传文件都在这里共用一份。手机只当连接的一方：
//! 不能被别人连，没有本机终端。用 UniFFI 导出，绑定由 uniffi-bindgen 从编好的库生成。
//!
//! 用法：启动时建一个 [`P2p`]（会阻塞一小会儿，放在后台线程调），之后连接、开终端、发文件都调它。
//! 带 async 的方法在 Kotlin 里是 suspend 函数、在 Swift 里是 async 函数；它们在 Rust 自己的线程池上跑，
//! 不占界面线程。回调（[`AppListener`]、[`TerminalListener`]）也从 Rust 的线程调过来，界面要自己切回主线程。

mod pair;
mod settings;
mod types;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use p2pcli_core::client::{ConnectError, RemoteHost, TermEvent};
use p2pcli_core::host::{BoxFuture, Decision, FileRequest, HostDelegate, HostEvent, IncomingRequest};
use p2pcli_core::node::{Node, NodeConfig};
use p2pcli_core::rendezvous::{normalize_code, parse_server};
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, oneshot};
use tracing::{info, warn};

pub use pair::{PairInfo, parse_pair_link};
pub use settings::{MobileSettings, ServerPreset, SettingsView};
pub use types::*;

uniffi::setup_scaffolding!();

/// 每次交给界面的终端输出上限：太碎会拖慢界面，太大一次画不完
const OUTPUT_BATCH: usize = 64 * 1024;
/// 后台定时扫一遍局域网
const NEARBY_REFRESH: Duration = Duration::from_secs(30);

/// 启动参数，由界面按平台填
#[derive(Debug, Clone, uniffi::Record)]
pub struct MobileConfig {
    /// 放身份密钥、连接历史、日志的目录：Android 的 filesDir，iOS 的 Application Support
    pub data_dir: String,
    /// 收到的文件放哪；不填就放在 data_dir 下面
    pub inbox_dir: Option<String>,
    /// 对方看到的名字，比如手机型号；不填用系统给的
    pub device_name: Option<String>,
    /// 牵线服务器；不填用默认的
    pub server: Option<String>,
    /// 中继服务器，多个用逗号分开；不填用 iroh 公共中继
    pub relay: Option<String>,
    pub proxy: Option<String>,
    /// App 的版本号，连接时告诉对方
    pub app_version: Option<String>,
}

/// 界面要知道的后台变化。从 Rust 的线程调过来，界面自己切回主线程
#[uniffi::export(with_foreign)]
pub trait AppListener: Send + Sync {
    /// 设备列表、在线状态、牵线服务器状态有变化：界面重新调 overview()
    fn on_overview_changed(&self);
    /// 一条连接断了（对方关了、网络断了）
    fn on_disconnected(&self, conn_id: u64, reason: String);
    /// 传输进度有变化
    fn on_transfers_changed(&self, transfers: Vec<TransferView>);
    /// 没配对过的设备要发文件过来，界面问用户以后调 respond_files
    fn on_file_request(&self, request: FileRequestView);
    /// 收文件的弹窗还开着，对面走了或超时了：界面关掉它
    fn on_file_request_cancelled(&self, id: u64);
}

/// 一个终端的输出。从 Rust 的线程调过来
#[uniffi::export(with_foreign)]
pub trait TerminalListener: Send + Sync {
    /// 终端输出的原始字节，直接交给终端控件
    fn on_output(&self, data: Vec<u8>);
    /// 终端结束了：exit_code 有值是里面的程序退出了；reason 有值是连接断了
    fn on_closed(&self, exit_code: Option<u32>, reason: Option<String>);
}

enum TermCmd {
    Input(Vec<u8>),
    Resize(u16, u16),
    Close,
}

/// 手机不能被连：别人来连一律拒绝；别人发文件来交给界面问用户
struct MobileDelegate {
    listener: Arc<dyn AppListener>,
    pending_files: Mutex<HashMap<u64, oneshot::Sender<bool>>>,
}

impl HostDelegate for MobileDelegate {
    fn approve(&self, _req: IncomingRequest) -> BoxFuture<Decision> {
        Box::pin(async { Decision::Deny })
    }

    fn event(&self, ev: HostEvent) {
        if let HostEvent::RequestCancelled { id } = ev {
            if self.pending_files.lock().unwrap().remove(&id).is_some() {
                self.listener.on_file_request_cancelled(id);
            }
        }
    }

    fn approve_files(&self, req: FileRequest) -> BoxFuture<bool> {
        let (tx, rx) = oneshot::channel();
        self.pending_files.lock().unwrap().insert(req.id, tx);
        self.listener.on_file_request(req.into());
        Box::pin(async move { rx.await.unwrap_or(false) })
    }
}

/// 一个 App 只建一个
#[derive(uniffi::Object)]
pub struct P2p {
    rt: Runtime,
    /// 改设置时整个换成新的节点：后台任务每次都从这里取当前的
    node: Arc<RwLock<Arc<Node>>>,
    /// App 给的启动参数；用户在设置里改过的项盖在它上面
    base: NodeConfig,
    /// 同一时间只换一次设置
    config_gate: tokio::sync::Mutex<()>,
    delegate: Arc<MobileDelegate>,
    listener: Arc<dyn AppListener>,
    remotes: Arc<Mutex<HashMap<u64, Arc<RemoteHost>>>>,
    next: AtomicU64,
}

#[uniffi::export]
impl P2p {
    /// 启动：读身份、连牵线服务器、开始局域网发现。会阻塞到网络起来，别在界面线程调
    #[uniffi::constructor]
    pub fn start(config: MobileConfig, listener: Arc<dyn AppListener>) -> Result<Arc<Self>, P2pError> {
        let data_dir = PathBuf::from(&config.data_dir);
        std::fs::create_dir_all(&data_dir).map_err(P2pError::failed)?;
        init_logging(&data_dir);
        let relays = config
            .relay
            .as_deref()
            .map(|r| r.split(',').map(|u| u.trim().to_string()).filter(|u| !u.is_empty()).collect())
            .unwrap_or_default();
        Self::start_with(
            NodeConfig {
                data_dir: data_dir.clone(),
                device_name: config.device_name.filter(|n| !n.trim().is_empty()),
                server: config
                    .server
                    .filter(|s| !s.trim().is_empty())
                    .or_else(|| Some(p2pcli_core::rendezvous::DEFAULT_SERVER.to_string())),
                proxy: config.proxy.filter(|p| !p.trim().is_empty()),
                relays,
                lan: true,
                inbox_dir: config.inbox_dir.map(PathBuf::from),
                app_version: config.app_version,
                ..Default::default()
            },
            listener,
        )
    }

    /// 首页要显示的全部内容：本机、牵线服务器、设备列表（连接历史、在线状态、局域网、备注合在一起）
    pub fn overview(&self) -> Overview {
        let node = self.node();
        let remarks = node.recent.remarks();
        let statuses: HashMap<_, _> = node.devices().into_iter().map(|d| (d.endpoint_id.clone(), d)).collect();
        let lan: HashMap<_, _> = node.nearby().into_iter().map(|d| (d.endpoint_id.clone(), d)).collect();
        let remark_of = |id: &str| remarks.get(id).map(|r| r.device.clone()).filter(|r| !r.trim().is_empty());
        let session_remark = |id: &str, session: &str| remarks.get(id).and_then(|r| r.sessions.get(session).cloned());

        let mut seen = std::collections::HashSet::new();
        let devices: Vec<DeviceView> = node
            .recent
            .list()
            .into_iter()
            // 同一台设备换过设备码会有几条记录，只留最近用的那条
            .filter(|d| seen.insert(d.endpoint_id.clone()))
            .map(|d| {
                let status = statuses.get(&d.endpoint_id);
                let id = d.endpoint_id.clone();
                DeviceView {
                    remark: remark_of(&id),
                    code: Some(d.code),
                    name: d.name,
                    platform: d.platform,
                    known: true,
                    presence: status.map(|s| if s.probing { Presence::Checking } else { s.state.into() }).unwrap_or(Presence::Checking),
                    path: status.and_then(|s| s.path.clone()).map(Into::into),
                    lan_online: lan.get(&id).is_some_and(|n| n.online),
                    sessions: status
                        .map(|s| s.sessions.iter().cloned().map(|x| { let r = session_remark(&id, &x.id); SessionView::new(x, r) }).collect())
                        .unwrap_or_default(),
                    sessions_live: status.is_some_and(|s| s.sessions_live),
                    endpoint_id: id,
                }
            })
            .collect();
        let nearby = node
            .nearby()
            .into_iter()
            .filter(|d| !seen.contains(&d.endpoint_id))
            .map(|d| DeviceView {
                remark: remark_of(&d.endpoint_id),
                endpoint_id: d.endpoint_id,
                code: None,
                name: d.name,
                platform: d.platform,
                known: false,
                presence: if d.online { Presence::Online } else { Presence::Offline },
                path: None,
                lan_online: d.online,
                sessions: Vec::new(),
                sessions_live: false,
            })
            .collect();
        Overview {
            name: node.me.name.clone(),
            endpoint_id: node.id().to_string(),
            server: node.rdv.as_ref().map(|r| r.status().into()).unwrap_or(ServerState::None),
            devices,
            nearby,
            inbox_dir: node.files.inbox_root().display().to_string(),
        }
    }

    /// 马上扫一遍局域网
    pub async fn scan_nearby(&self) -> Result<Vec<NearbyView>, P2pError> {
        let node = self.node();
        self.run(async move {
            let lan = node.lan.as_ref().ok_or_else(|| P2pError::failed("局域网发现没有启用"))?;
            let found = lan.scan(node.id()).await.map_err(P2pError::from_anyhow)?;
            Ok(found.into_iter().map(Into::into).collect())
        })
        .await
    }

    /// 连一台设备。target 可以是 9 位设备码、设备公钥（局域网里的设备），或者调试用的 公钥@地址。
    /// 对方要点同意，所以可能等比较久
    pub async fn connect(&self, target: String, password: Option<String>) -> Result<Connected, P2pError> {
        let node = self.node();
        let remotes = self.remotes.clone();
        let listener = self.listener.clone();
        let conn_id = self.next.fetch_add(1, Ordering::Relaxed);
        let handle = self.rt.handle().clone();
        self.run(async move {
            let password = password.map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
            let code = normalize_code(&target);
            let res = match &code {
                Some(c) => node.connect_code(c, password).await,
                None => match target.trim().parse::<iroh::EndpointId>() {
                    Ok(id) => node.connect_endpoint(id, password).await,
                    Err(_) => match parse_server(&target) {
                        Ok(addr) => node.connect_addr(addr, password).await,
                        Err(_) => return Err(P2pError::failed("设备码是 9 位数字")),
                    },
                },
            };
            let remote = Arc::new(res.map_err(|e| match e {
                ConnectError::Rejected(c) => P2pError::Rejected { reason: c.into(), detail: c.describe().into() },
                other => P2pError::failed(other),
            })?);
            remotes.lock().unwrap().insert(conn_id, remote.clone());
            node.adopt_remote(remote.clone());
            info!(conn_id, name = %remote.info.name, "已连上对方");
            // 断了告诉界面
            let watch = remote.clone();
            handle.spawn(async move {
                let reason = watch.closed().await;
                remotes.lock().unwrap().remove(&conn_id);
                listener.on_disconnected(conn_id, reason);
            });
            Ok(Connected {
                conn_id,
                endpoint_id: remote.info.endpoint_id.clone(),
                name: remote.info.name.clone(),
                platform: remote.info.platform.clone(),
                code,
                path: remote.path().into(),
            })
        })
        .await
    }

    pub fn disconnect(&self, conn_id: u64) {
        if let Some(r) = self.remotes.lock().unwrap().remove(&conn_id) {
            r.close();
        }
    }

    /// 现在走的是哪条路、延迟多少
    pub fn path(&self, conn_id: u64) -> Result<PathView, P2pError> {
        Ok(self.remote(conn_id)?.path().into())
    }

    /// 对方电脑上的后台会话。顺便记下来，断开以后列表里还能显示
    pub async fn list_sessions(&self, conn_id: u64) -> Result<Vec<SessionView>, P2pError> {
        let remote = self.remote(conn_id)?;
        let node = self.node();
        self.run(async move {
            let sessions = remote.list_terminals().await.map_err(P2pError::from_anyhow)?;
            let id = remote.info.endpoint_id.clone();
            if let Err(e) = node.recent.save_sessions(&id, &sessions) {
                warn!("记会话列表失败：{e:#}");
            }
            let remarks = node.recent.remarks();
            let remark = |s: &str| remarks.get(&id).and_then(|r| r.sessions.get(s).cloned());
            Ok(sessions.into_iter().map(|s| { let r = remark(&s.id); SessionView::new(s, r) }).collect())
        })
        .await
    }

    /// 结束对方电脑上的一个会话，里面跑的程序会被关掉
    pub async fn end_session(&self, conn_id: u64, session_id: String) -> Result<(), P2pError> {
        let remote = self.remote(conn_id)?;
        self.run(async move { remote.end_terminal(session_id).await.map_err(P2pError::from_anyhow) }).await
    }

    /// 在 AI 的确认框里选一项
    pub async fn choose_agent_option(&self, conn_id: u64, session_id: String, approval_id: String, option_id: u64) -> Result<(), P2pError> {
        let remote = self.remote(conn_id)?;
        self.run(async move {
            remote
                .choose_agent_option(session_id, approval_id, option_id as usize)
                .await
                .map_err(P2pError::from_anyhow)
        })
        .await
    }

    /// 打开终端：session_id 为空是新建一个后台会话，不为空是接上已有的。
    /// 输出从 listener 送来；返回的 Terminal 用来打字、改尺寸、关掉
    pub async fn open_terminal(
        &self,
        conn_id: u64,
        session_id: Option<String>,
        cols: u16,
        rows: u16,
        listener: Arc<dyn TerminalListener>,
    ) -> Result<Arc<Terminal>, P2pError> {
        let remote = self.remote(conn_id)?;
        let handle = self.rt.handle().clone();
        self.run(async move {
            let (mut input, mut out) = remote.attach_terminal(session_id, cols, rows).await.map_err(P2pError::from_anyhow)?;
            let session_id = out.session_id.clone();
            let (tx, mut rx) = mpsc::unbounded_channel::<TermCmd>();

            // 打字、改尺寸
            handle.spawn(async move {
                while let Some(cmd) = rx.recv().await {
                    let ok = match cmd {
                        TermCmd::Input(b) => input.write(&b).await.is_ok(),
                        TermCmd::Resize(c, r) => input.resize(c, r).await.is_ok(),
                        TermCmd::Close => {
                            input.close();
                            false
                        }
                    };
                    if !ok {
                        break;
                    }
                }
            });

            // 读和交给界面分开：读帧不能被打断，交给界面时把攒下的输出合成一批
            let (chunk_tx, mut chunk_rx) = mpsc::unbounded_channel::<Result<TermEvent, String>>();
            handle.spawn(async move {
                loop {
                    let ev = match out.next().await {
                        Ok(Some(ev)) => Ok(ev),
                        Ok(None) => break,
                        Err(e) => Err(format!("{e:#}")),
                    };
                    let stop = !matches!(ev, Ok(TermEvent::Output(_)));
                    if chunk_tx.send(ev).is_err() || stop {
                        break;
                    }
                }
            });
            handle.spawn(async move {
                let (mut exit_code, mut reason) = (None, None);
                'outer: while let Some(first) = chunk_rx.recv().await {
                    let mut next = Some(first);
                    let mut batch = Vec::new();
                    while let Some(item) = next.take() {
                        match item {
                            Ok(TermEvent::Output(d)) => batch.extend_from_slice(&d),
                            Ok(TermEvent::Exit(code)) => {
                                exit_code = code;
                                if !batch.is_empty() { listener.on_output(batch); }
                                break 'outer;
                            }
                            Err(r) => {
                                reason = Some(r);
                                if !batch.is_empty() { listener.on_output(batch); }
                                break 'outer;
                            }
                        }
                        if batch.len() < OUTPUT_BATCH {
                            next = chunk_rx.try_recv().ok();
                        }
                    }
                    listener.on_output(batch);
                }
                listener.on_closed(exit_code, reason);
            });
            Ok(Arc::new(Terminal { session_id, tx }))
        })
        .await
    }

    /// 在后台给一台设备发文件，返回传输编号；进度从 AppListener.on_transfers_changed 来。
    /// target 是设备公钥、设备码或名字；paths 是本机路径（Android 要先把选好的文件复制到缓存目录）
    pub fn send_files(&self, target: String, paths: Vec<String>) -> Result<String, P2pError> {
        let _guard = self.rt.enter();
        let device = self.node().find_device(&target).map_err(P2pError::from_anyhow)?;
        self.node()
            .start_send(&device, paths.into_iter().map(PathBuf::from).collect())
            .map_err(P2pError::from_anyhow)
    }

    pub fn transfers(&self) -> Vec<TransferView> {
        self.node().files.transfers().into_iter().map(Into::into).collect()
    }

    /// 回答 on_file_request 问的那批文件收不收
    pub fn respond_files(&self, id: u64, accept: bool) {
        if let Some(tx) = self.delegate.pending_files.lock().unwrap().remove(&id) {
            let _ = tx.send(accept);
        }
    }

    /// 给设备起备注，空字符串是去掉备注
    pub fn set_device_remark(&self, endpoint_id: String, remark: String) -> Result<(), P2pError> {
        self.node().recent.set_device_remark(&endpoint_id, &remark).map(|_| ()).map_err(P2pError::from_anyhow)
    }

    /// 从连接历史里去掉一台设备
    pub fn forget_device(&self, endpoint_id: String) -> Result<(), P2pError> {
        let node = self.node();
        for d in node.recent.list().into_iter().filter(|d| d.endpoint_id == endpoint_id) {
            node.forget_recent(&d.code).map_err(P2pError::from_anyhow)?;
        }
        Ok(())
    }

    /// 后台重新探测一遍设备在不在线
    pub fn refresh_devices(&self) {
        let _guard = self.rt.enter();
        self.node().prober.refresh_now();
    }

    /// 设置页要显示的：用户改过的项、默认值、可选的牵线服务器
    pub fn settings(&self) -> SettingsView {
        let node = self.node();
        settings::view(&self.base, &settings::load(&self.base.data_dir), &node)
    }

    /// 保存设置并马上生效。改了代理、中继会换一套网络，已有的连接会断开（对方电脑上的会话还在）
    pub async fn save_settings(&self, settings: MobileSettings) -> Result<(), P2pError> {
        let _gate = self.config_gate.lock().await;
        let saved = settings::validate(settings)?;
        let cfg = settings::apply(&self.base, &saved);
        let old = self.node();
        let delegate: Arc<dyn HostDelegate> = self.delegate.clone();
        let data_dir = self.base.data_dir.clone();
        let cell = self.node.clone();
        self.run(async move {
            let transport_changed = old.transport_changed(&cfg);
            let next = old.prepare_update(cfg, delegate).await.map_err(|e| P2pError::failed(format!("应用网络配置失败：{e:#}")))?;
            if let Err(e) = settings::store(&data_dir, &saved) {
                if transport_changed {
                    next.endpoint.close().await;
                }
                return Err(P2pError::failed(format!("保存设置失败，当前配置未更改：{e:#}")));
            }
            old.stop_rendezvous().await;
            next.activate();
            *cell.write().unwrap() = next;
            if transport_changed {
                old.endpoint.close().await;
            }
            info!("设置已更新");
            Ok(())
        })
        .await?;
        self.listener.on_overview_changed();
        Ok(())
    }

    /// 退出前调：断开所有连接、停掉网络
    pub fn shutdown(&self) {
        for (_, r) in self.remotes.lock().unwrap().drain() {
            r.close();
        }
        self.rt.block_on(self.node().shutdown());
    }
}

impl P2p {
    /// 按给定的节点配置启动。App 走 [`P2p::start`]；测试用它起离线节点（不连外网的牵线服务器）
    #[doc(hidden)]
    pub fn start_with(mut base: NodeConfig, listener: Arc<dyn AppListener>) -> Result<Arc<Self>, P2pError> {
        // 手机不能被连，不在局域网里显示自己，别人点了也连不上
        base.lan_visible = false;
        let cfg = settings::apply(&base, &settings::load(&base.data_dir));
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("p2pcli")
            .enable_all()
            .build()
            .map_err(P2pError::failed)?;
        let delegate = Arc::new(MobileDelegate { listener: listener.clone(), pending_files: Mutex::new(HashMap::new()) });
        let node = rt.block_on(Node::start(cfg, delegate.clone())).map_err(P2pError::from_anyhow)?;
        info!(id = %node.id(), "P2PCLI 手机端已启动");
        let me = Arc::new(Self {
            rt,
            node: Arc::new(RwLock::new(node)),
            base,
            config_gate: tokio::sync::Mutex::new(()),
            delegate,
            listener,
            remotes: Arc::new(Mutex::new(HashMap::new())),
            next: AtomicU64::new(1),
        });
        me.watch_background();
        Ok(me)
    }

    fn node(&self) -> Arc<Node> {
        self.node.read().unwrap().clone()
    }

    fn remote(&self, conn_id: u64) -> Result<Arc<RemoteHost>, P2pError> {
        self.remotes.lock().unwrap().get(&conn_id).cloned().ok_or(P2pError::Disconnected)
    }

    /// 放到自己的线程池上跑。界面那边用什么方式等结果都行（Kotlin 协程、Swift async），
    /// 不需要它们认识 tokio
    async fn run<T: Send + 'static>(&self, fut: impl std::future::Future<Output = Result<T, P2pError>> + Send + 'static) -> Result<T, P2pError> {
        self.rt.spawn(fut).await.map_err(P2pError::failed)?
    }

    /// 后台：传输进度、设备列表和牵线服务器状态有变化就告诉界面；定时扫局域网
    fn watch_background(&self) {
        let cell = self.node.clone();
        let listener = self.listener.clone();
        self.rt.spawn(async move {
            let mut previous = Vec::new();
            let mut tick = tokio::time::interval(Duration::from_millis(500));
            loop {
                tick.tick().await;
                let node = cell.read().unwrap().clone();
                let current: Vec<TransferView> = node.files.transfers().into_iter().map(Into::into).collect();
                if current != previous {
                    listener.on_transfers_changed(current.clone());
                    previous = current;
                }
            }
        });
        let cell = self.node.clone();
        let listener = self.listener.clone();
        self.rt.spawn(async move {
            let mut previous = String::new();
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            loop {
                tick.tick().await;
                let node = cell.read().unwrap().clone();
                // 只比较会变的部分，变了才叫界面重读
                let current = format!(
                    "{:?}|{:?}|{:?}|{:?}",
                    node.rdv.as_ref().map(|r| r.status()),
                    node.nearby(),
                    node.devices(),
                    node.recent.list().len()
                );
                if current != previous {
                    listener.on_overview_changed();
                    previous = current;
                }
            }
        });
        let cell = self.node.clone();
        self.rt.spawn(async move {
            let mut tick = tokio::time::interval(NEARBY_REFRESH);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                let node = cell.read().unwrap().clone();
                if let Some(lan) = &node.lan {
                    if let Err(e) = lan.scan(node.id()).await {
                        tracing::debug!("附近设备自动刷新失败：{e:#}");
                    }
                }
            }
        });
    }
}

/// 一个开着的终端
#[derive(uniffi::Object)]
pub struct Terminal {
    session_id: String,
    tx: mpsc::UnboundedSender<TermCmd>,
}

#[uniffi::export]
impl Terminal {
    /// 对方电脑上的会话编号：关掉终端以后还能用它接回来
    pub fn session_id(&self) -> String {
        self.session_id.clone()
    }

    /// 键盘输入、粘贴的内容，原样发过去
    pub fn write(&self, data: Vec<u8>) {
        let _ = self.tx.send(TermCmd::Input(data));
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let _ = self.tx.send(TermCmd::Resize(cols, rows));
    }

    /// 关掉手机上的这个终端；对方电脑上的会话接着跑。
    /// 不叫 close：UniFFI 生成的对象自带 close（释放 Rust 那边的对象）
    pub fn detach(&self) {
        let _ = self.tx.send(TermCmd::Close);
    }
}

/// 日志写到 data_dir/p2pcli.log，查问题用；只初始化一次
fn init_logging(data_dir: &std::path::Path) {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(data_dir.join("p2pcli.log")) else { return };
        let filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,iroh=warn,noq=warn"));
        let _ = tracing_subscriber::fmt().with_env_filter(filter).with_ansi(false).with_writer(Mutex::new(file)).try_init();
    });
}
