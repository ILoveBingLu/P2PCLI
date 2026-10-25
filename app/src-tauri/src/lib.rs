//! 桌面端：把核心库包成界面能调的命令，管弹窗、托盘和窗口。

mod update;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use p2pcli_core::client::{ConnectError, RemoteHost, RemoteInfo, TermEvent};
use p2pcli_core::host::{
    BoxFuture, Decision, HostDelegate, HostEvent, IncomingRequest, SessionSummary,
};
use p2pcli_core::node::{Node, NodeConfig};
use p2pcli_core::path::PathInfo;
use p2pcli_core::proto::RejectCode;
use p2pcli_core::rendezvous::{normalize_code, parse_server};
use p2pcli_core::lan::NearbyDevice;
use p2pcli_core::store::{RecentDevice, TrustedDevice};
use serde::{Deserialize, Serialize};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::{mpsc, oneshot};
use tracing::{info, warn};

/// 每批推给界面的终端输出上限，太碎会拖慢界面
const OUTPUT_BATCH: usize = 64 * 1024;
/// 后端定时扫描，窗口隐藏后也继续检测；界面通过现有状态事件获取变化。
const NEARBY_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Tauri externalBin is next to the app executable in both development and
/// installed builds (Contents/MacOS on macOS). It is selected at package time.
fn bundled_session_program() -> std::io::Result<Option<PathBuf>> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        let name = if cfg!(windows) { "psmux.exe" } else { "tmux" };
        Ok(Some(std::env::current_exe()?.with_file_name(name)))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    { Ok(None) }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Settings {
    #[serde(default)]
    proxy: Option<String>,
    #[serde(default)]
    server: Option<String>,
    #[serde(default)]
    device_name: Option<String>,
    #[serde(default)]
    relay: Option<String>,
    /// 不让局域网里的其他设备看到本机；默认看得到
    #[serde(default)]
    lan_hidden: bool,
}

fn settings_path(dir: &Path) -> PathBuf {
    dir.join("config.json")
}

fn load_settings(dir: &Path) -> Settings {
    std::fs::read(settings_path(dir))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// 编译时默认的牵线服务器，打包时可以用 P2PCLI_DEFAULT_SERVER 换掉
fn default_server() -> &'static str {
    option_env!("P2PCLI_DEFAULT_SERVER").unwrap_or(p2pcli_core::rendezvous::DEFAULT_SERVER)
}

/// 牵线服务器：环境变量 > 配置文件 > 编译时默认
fn effective_server(s: &Settings) -> Option<String> {
    std::env::var("P2PCLI_SERVER")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| s.server.clone())
        .or_else(|| Some(default_server().to_string()))
        .filter(|s| !s.trim().is_empty())
}

#[derive(Serialize)]
struct ServerPreset {
    address: &'static str,
    label: &'static str,
    note: &'static str,
}

/// 设置页可选的牵线服务器；打包时换了默认服务器而它不在列表里，就放在最前面
fn server_presets() -> Vec<ServerPreset> {
    let mut list: Vec<ServerPreset> = p2pcli_core::rendezvous::SERVER_PRESETS
        .iter()
        .map(|&(address, label, note)| ServerPreset { address, label, note })
        .collect();
    let default = default_server();
    if !default.trim().is_empty() && !list.iter().any(|p| p.address == default) {
        list.insert(0, ServerPreset { address: default, label: "默认服务器", note: "安装包内置的牵线服务器" });
    }
    list
}

/// 中继同样：环境变量 > 配置文件 > 编译时默认；都没有就用 iroh 公共中继
fn effective_relay(s: &Settings) -> Option<String> {
    std::env::var("P2PCLI_RELAY")
        .ok()
        .or_else(|| s.relay.clone())
        .or_else(|| option_env!("P2PCLI_DEFAULT_RELAY").map(String::from))
        .filter(|s| !s.trim().is_empty())
}

struct UiDelegate {
    app: AppHandle,
    pending: Mutex<HashMap<u64, oneshot::Sender<Decision>>>,
}

impl HostDelegate for UiDelegate {
    fn approve(&self, req: IncomingRequest) -> BoxFuture<Decision> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(req.id, tx);
        show_main(&self.app);
        if let Some(w) = self.app.get_webview_window("main") {
            let _ = w.request_user_attention(Some(tauri::UserAttentionType::Critical));
        }
        let _ = self
            .app
            .notification()
            .builder()
            .title("有人请求使用你的终端")
            .body(format!("{}（{}）", req.name, platform_label(&req.platform)))
            .show();
        let _ = self.app.emit("host-request", &req);
        Box::pin(async move { rx.await.unwrap_or(Decision::Deny) })
    }

    fn event(&self, ev: HostEvent) {
        if let HostEvent::RequestCancelled { id } = &ev {
            self.pending.lock().unwrap().remove(id);
        }
        if let HostEvent::Connected { name, .. } = &ev {
            let _ = self
                .app
                .notification()
                .builder()
                .title("已有设备连上本机")
                .body(format!("{name} 正在使用你的终端"))
                .show();
        }
        let _ = self.app.emit("host-event", &ev);
    }
}

fn platform_label(p: &str) -> &str {
    match p {
        "macos" => "macOS",
        "windows" => "Windows",
        "linux" => "Linux",
        other => other,
    }
}

enum TermCmd {
    Input(Vec<u8>),
    Resize(u16, u16),
    Close,
}

struct AppState {
    node: RwLock<Arc<Node>>,
    config_gate: tokio::sync::Mutex<()>,
    delegate: Arc<UiDelegate>,
    data_dir: PathBuf,
    settings: Mutex<Settings>,
    remotes: Mutex<HashMap<u64, Arc<RemoteHost>>>,
    terms: Mutex<HashMap<u64, mpsc::UnboundedSender<TermCmd>>>,
    next: AtomicU64,
}

impl AppState {
    fn node(&self) -> Arc<Node> { self.node.read().unwrap().clone() }
    fn next(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    fn remote(&self, id: u64) -> Result<Arc<RemoteHost>, String> {
        self.remotes
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| "连接已断开".to_string())
    }
}

#[derive(Serialize)]
struct TrustedView {
    endpoint_id: String,
    #[serde(flatten)]
    device: TrustedDevice,
}

#[derive(Serialize)]
struct Overview {
    name: String,
    platform: String,
    endpoint_id: String,
    code: Option<String>,
    password: String,
    /// 配对密码固定了，重启也不变
    password_fixed: bool,
    /// none 表示没配牵线服务器
    server: serde_json::Value,
    sessions: Vec<SessionSummary>,
    recent: Vec<RecentDevice>,
    /// 局域网里发现的设备
    nearby: Vec<NearbyDevice>,
    trusted: Vec<TrustedView>,
}

#[tauri::command]
fn overview(state: State<'_, AppState>) -> Overview {
    let node = state.node();
    let server = match &node.rdv {
        None => serde_json::json!({ "state": "none" }),
        Some(r) => serde_json::to_value(r.status()).unwrap_or_default(),
    };
    Overview {
        name: node.me.name.clone(),
        platform: node.me.platform.clone(),
        endpoint_id: node.id().to_string(),
        code: node.rdv.as_ref().and_then(|r| r.code()),
        password: node.host.password(),
        password_fixed: node.host.password_fixed(),
        server,
        sessions: node.host.sessions(),
        recent: node.recent.list(),
        nearby: node.nearby(),
        trusted: node
            .host
            .trust
            .list()
            .into_iter()
            .map(|(endpoint_id, device)| TrustedView {
                endpoint_id,
                device,
            })
            .collect(),
    }
}

#[tauri::command]
async fn scan_nearby(state: State<'_, AppState>) -> Result<Vec<NearbyDevice>, String> {
    let node = state.node();
    let lan = node.lan.as_ref().ok_or_else(|| "局域网发现未启用".to_string())?;
    lan.scan(node.id()).await.map_err(|error| format!("{error:#}"))
}

#[tauri::command]
async fn diagnose(state: State<'_, AppState>) -> Result<p2pcli_core::diag::Diagnosis, String> {
    Ok(p2pcli_core::diag::diagnose(&state.node()).await)
}

#[tauri::command]
fn refresh_password(state: State<'_, AppState>) -> String {
    state.node().host.regenerate_password()
}

#[tauri::command]
async fn check_update(app: AppHandle, state: State<'_, AppState>) -> Result<Option<update::UpdateView>, String> {
    let proxy = state.settings.lock().unwrap().proxy.clone();
    update::check(&app, proxy).await
}

#[tauri::command]
async fn install_update(app: AppHandle, progress: Channel<update::UpdateProgress>) -> Result<(), String> {
    update::install(&app, progress).await
}

#[tauri::command]
fn set_password_fixed(state: State<'_, AppState>, fixed: bool) -> Result<(), String> {
    state.node().host.set_password_fixed(fixed).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn respond_request(state: State<'_, AppState>, id: u64, allow: bool, trust: bool) {
    if let Some(tx) = state.delegate.pending.lock().unwrap().remove(&id) {
        let _ = tx.send(if allow {
            Decision::Allow { trust }
        } else {
            Decision::Deny
        });
    }
}

#[tauri::command]
fn kick(state: State<'_, AppState>, session: u64) -> bool {
    state.node().host.kick(session)
}

#[tauri::command]
fn remove_trusted(state: State<'_, AppState>, endpoint_id: String) -> Result<(), String> {
    state
        .node()
        .host
        .trust
        .remove(&endpoint_id)
        .map(|_| ())
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn forget_recent(state: State<'_, AppState>, code: String) -> Result<(), String> {
    state.node().recent.remove(&code).map_err(|e| format!("{e:#}"))
}

#[derive(Serialize)]
struct Connected {
    conn_id: u64,
    code: Option<String>,
    info: RemoteInfo,
    path: PathInfo,
}

#[derive(Serialize)]
struct ConnectFail {
    reject: Option<RejectCode>,
    message: String,
}

#[tauri::command]
async fn connect(
    app: AppHandle,
    state: State<'_, AppState>,
    target: String,
    password: Option<String>,
) -> Result<Connected, ConnectFail> {
    let password = password.map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
    let code = normalize_code(&target);
    let res = match &code {
        Some(c) => state.node().connect_code(c, password).await,
        // 局域网设备，或者在局域网在线的历史设备：按公钥连，不经过牵线服务器
        None if !target.contains('@') && target.trim().parse::<iroh::EndpointId>().is_ok() => {
            let id = target.trim().parse::<iroh::EndpointId>().expect("上面刚检查过");
            state.node().connect_endpoint(id, password).await
        }
        // 开发测试：直接填 公钥@地址
        None => match parse_server(&target) {
            Ok(addr) => state.node().connect_addr(addr, password).await,
            Err(_) => {
                return Err(ConnectFail {
                    reject: None,
                    message: "设备码是 9 位数字".into(),
                });
            }
        },
    };
    let remote = match res {
        Ok(r) => Arc::new(r),
        Err(ConnectError::Rejected(c)) => {
            return Err(ConnectFail {
                reject: Some(c),
                message: c.describe().into(),
            });
        }
        Err(e) => {
            return Err(ConnectFail {
                reject: None,
                message: e.to_string(),
            });
        }
    };
    let conn_id = state.next();
    state.remotes.lock().unwrap().insert(conn_id, remote.clone());
    info!(conn_id, name = %remote.info.name, "已连上对方");
    let watch = remote.clone();
    tauri::async_runtime::spawn(async move {
        let reason = watch.closed().await;
        app.state::<AppState>().remotes.lock().unwrap().remove(&conn_id);
        let _ = app.emit(
            "remote-closed",
            serde_json::json!({ "conn_id": conn_id, "reason": reason }),
        );
    });
    Ok(Connected {
        conn_id,
        code,
        path: remote.path(),
        info: remote.info.clone(),
    })
}

#[tauri::command]
fn disconnect(state: State<'_, AppState>, conn_id: u64) {
    if let Some(r) = state.remotes.lock().unwrap().remove(&conn_id) {
        r.close();
    }
}

#[tauri::command]
fn path_info(state: State<'_, AppState>, conn_id: u64) -> Result<PathInfo, String> {
    Ok(state.remote(conn_id)?.path())
}

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum TermStatus {
    Exited { code: Option<u32> },
    Closed { reason: Option<String> },
}

#[derive(Serialize)]
struct OpenedTerminal { term_id: u64, session_id: String }

#[tauri::command]
async fn list_terminals(state: State<'_, AppState>, conn_id: u64) -> Result<Vec<p2pcli_core::proto::TerminalSession>, String> {
    state.remote(conn_id)?.list_terminals().await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn end_terminal(state: State<'_, AppState>, conn_id: u64, session_id: String) -> Result<(), String> {
    state.remote(conn_id)?.end_terminal(session_id).await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn open_terminal(
    app: AppHandle,
    state: State<'_, AppState>,
    conn_id: u64,
    cols: u16,
    rows: u16,
    session_id: Option<String>,
    output: Channel<InvokeResponseBody>,
    status: Channel<TermStatus>,
) -> Result<OpenedTerminal, String> {
    let remote = state.remote(conn_id)?;
    let (mut input, mut out) = remote
        .attach_terminal(session_id, cols, rows)
        .await
        .map_err(|e| format!("{e:#}"))?;
    let session_id = out.session_id.clone();
    let term_id = state.next();
    let (tx, mut rx) = mpsc::unbounded_channel::<TermCmd>();
    state.terms.lock().unwrap().insert(term_id, tx);

    tauri::async_runtime::spawn(async move {
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

    // 读和转发分开：读帧不能被打断，转发时把攒下的输出合成一批
    let (chunk_tx, mut chunk_rx) = mpsc::unbounded_channel::<Result<TermEvent, String>>();
    tauri::async_runtime::spawn(async move {
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
    tauri::async_runtime::spawn(async move {
        let mut final_status = TermStatus::Closed { reason: None };
        'outer: while let Some(first) = chunk_rx.recv().await {
            let mut next = Some(first);
            let mut batch = Vec::new();
            while let Some(item) = next.take() {
                match item {
                    Ok(TermEvent::Output(d)) => batch.extend_from_slice(&d),
                    Ok(TermEvent::Exit(code)) => {
                        final_status = TermStatus::Exited { code };
                        let _ = output.send(InvokeResponseBody::Raw(batch));
                        break 'outer;
                    }
                    Err(reason) => {
                        final_status = TermStatus::Closed {
                            reason: Some(reason),
                        };
                        let _ = output.send(InvokeResponseBody::Raw(batch));
                        break 'outer;
                    }
                }
                if batch.len() < OUTPUT_BATCH {
                    next = chunk_rx.try_recv().ok();
                }
            }
            if output.send(InvokeResponseBody::Raw(batch)).is_err() {
                break;
            }
        }
        let _ = status.send(final_status);
        app.state::<AppState>().terms.lock().unwrap().remove(&term_id);
    });
    Ok(OpenedTerminal { term_id, session_id })
}

fn send_term(state: &AppState, term_id: u64, cmd: TermCmd) {
    if let Some(tx) = state.terms.lock().unwrap().get(&term_id) {
        let _ = tx.send(cmd);
    }
}

#[tauri::command]
fn term_input(state: State<'_, AppState>, term_id: u64, data: String, binary: bool) {
    let bytes = if binary {
        // xterm 的 onBinary 给的是每个字符一个字节
        data.chars().map(|c| c as u32 as u8).collect()
    } else {
        data.into_bytes()
    };
    send_term(&state, term_id, TermCmd::Input(bytes));
}

#[tauri::command]
fn term_resize(state: State<'_, AppState>, term_id: u64, cols: u16, rows: u16) {
    send_term(&state, term_id, TermCmd::Resize(cols, rows));
}

#[tauri::command]
fn term_close(state: State<'_, AppState>, term_id: u64) {
    send_term(&state, term_id, TermCmd::Close);
}

#[derive(Serialize)]
struct SettingsView {
    proxy: Option<String>,
    server: Option<String>,
    device_name: Option<String>,
    relay: Option<String>,
    effective_server: Option<String>,
    effective_relay: Option<String>,
    default_server: &'static str,
    server_presets: Vec<ServerPreset>,
    lan_visible: bool,
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> SettingsView {
    let s = state.settings.lock().unwrap().clone();
    SettingsView {
        proxy: s.proxy.clone(),
        effective_server: effective_server(&s),
        effective_relay: effective_relay(&s),
        default_server: default_server(),
        server_presets: server_presets(),
        lan_visible: !s.lan_hidden,
        server: s.server,
        device_name: s.device_name,
        relay: s.relay,
    }
}

/// Prepare a new runtime, persist, then atomically swap the active configuration.
#[tauri::command]
async fn save_settings(
    state: State<'_, AppState>,
    server: Option<String>,
    device_name: Option<String>,
    relay: Option<String>,
    proxy: Option<String>,
    lan_visible: Option<bool>,
) -> Result<(), String> {
    let _gate = state.config_gate.lock().await;
    let clean = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let s = Settings {
        proxy: clean(proxy),
        server: clean(server),
        device_name: clean(device_name),
        relay: clean(relay),
        lan_hidden: !lan_visible.unwrap_or(true),
    };
    if let Some(proxy) = &s.proxy {
        p2pcli_core::proxy::parse_proxy(proxy).map_err(|e| format!("{e:#}"))?;
    }
    if let Some(srv) = &s.server {
        p2pcli_core::rendezvous::parse_rdv_server(srv).map_err(|e| format!("{e:#}"))?;
    }
    if let Some(r) = &s.relay {
        for u in r.split(',') {
            u.trim()
                .parse::<iroh::RelayUrl>()
                .map_err(|e| format!("中继地址格式不对：{u}（{e}）"))?;
        }
    }
    let cfg = NodeConfig {
        data_dir: state.data_dir.clone(),
        session_program: bundled_session_program().map_err(|e| e.to_string())?,
        device_name: s.device_name.clone(),
        server: effective_server(&s),
        proxy: s.proxy.clone(),
        relays: effective_relay(&s).map(|r| r.split(',').map(|u| u.trim().to_string()).collect()).unwrap_or_default(),
        lan: true,
        lan_visible: !s.lan_hidden,
        ..Default::default()
    };
    let old = state.node();
    let transport_changed = old.transport_changed(&cfg);
    let next = old.prepare_update(cfg, state.delegate.clone()).await.map_err(|e| format!("应用网络配置失败：{e:#}"))?;
    let bytes = serde_json::to_vec_pretty(&s).map_err(|e| e.to_string())?;
    if let Err(e) = p2pcli_core::store::write_private(&settings_path(&state.data_dir), &bytes) {
        if transport_changed { next.endpoint.close().await; }
        return Err(format!("保存设置失败，当前配置未更改：{e:#}"));
    }
    old.stop_rendezvous().await;
    next.activate();
    *state.node.write().unwrap() = next;
    *state.settings.lock().unwrap() = s;
    if transport_changed { old.endpoint.close().await; }

    Ok(())
}

#[tauri::command]
fn restart(app: AppHandle) {
    app.restart();
}

/// 自测脚本配置，只有调试版认 P2PCLI_SELFTEST，发给用户的正式版永远返回空
#[tauri::command]
fn selftest_config() -> Option<serde_json::Value> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let path = std::env::var_os("P2PCLI_SELFTEST")?;
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

#[tauri::command]
fn selftest_report(path: String, report: serde_json::Value) -> Result<(), String> {
    if !cfg!(debug_assertions) {
        return Err("正式版不支持自测".into());
    }
    let bytes = serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn init_logging(dir: &Path) {
    let file = std::fs::File::create(dir.join("p2pcli.log")).ok();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "p2pcli_core=info,p2pcli_app_lib=info".into());
    let builder = tracing_subscriber::fmt().with_env_filter(filter).with_ansi(false);
    match file {
        Some(f) => {
            let _ = builder.with_writer(Mutex::new(f)).try_init();
        }
        None => {
            let _ = builder.try_init();
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(target_os = "windows")]
    let builder = builder.plugin(tauri_plugin_window_controls::init());

    let app = builder
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app);
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(update::PendingUpdate::default())
        .setup(|app| {
            // 数据目录可以用环境变量指定，方便一台机器上开两份做测试
            let data_dir = match std::env::var_os("P2PCLI_DATA_DIR") {
                Some(d) => PathBuf::from(d),
                None => app.path().app_data_dir()?,
            };
            std::fs::create_dir_all(&data_dir)?;
            init_logging(&data_dir);
            let settings = load_settings(&data_dir);
            let delegate = Arc::new(UiDelegate {
                app: app.handle().clone(),
                pending: Mutex::new(HashMap::new()),
            });
            let cfg = NodeConfig {
                data_dir: data_dir.clone(),
                session_program: bundled_session_program()?,
                device_name: settings.device_name.clone(),
                server: effective_server(&settings),
                proxy: settings.proxy.clone(),
                relays: effective_relay(&settings)
                    .map(|r| r.split(',').map(|u| u.trim().to_string()).collect())
                    .unwrap_or_default(),
                lan: true,
                lan_visible: !settings.lan_hidden,
                ..Default::default()
            };
            let node = tauri::async_runtime::block_on(Node::start(cfg, delegate.clone()))?;
            info!(id = %node.id(), "P2PCLI 已启动");

            app.manage(AppState {
                node: RwLock::new(node),
                config_gate: tokio::sync::Mutex::new(()),
                delegate,
                data_dir,
                settings: Mutex::new(settings),
                remotes: Mutex::new(HashMap::new()),
                terms: Mutex::new(HashMap::new()),
                next: AtomicU64::new(1),
            });

            update::auto_install_for_test(app.handle());

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut tick = tokio::time::interval(NEARBY_REFRESH_INTERVAL);
                // 睡眠恢复或扫描耗时较长时跳过错过的轮次，不集中补扫。
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    tick.tick().await;
                    // 每轮读取当前节点，配置更新后不再扫描旧网络实例。
                    let node = handle.state::<AppState>().node();
                    if let Some(lan) = &node.lan {
                        if let Err(error) = lan.scan(node.id()).await {
                            tracing::debug!("附近设备自动刷新失败：{error:#}");
                        }
                    }
                }
            });

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut previous = serde_json::Value::Null;
                let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
                loop {
                    tick.tick().await;
                    let node = handle.state::<AppState>().node();
                    let current = serde_json::json!({ "status": node.rdv.as_ref().map(|r| r.status()), "code": node.rdv.as_ref().and_then(|r| r.code()), "name": node.me.name, "lan": node.nearby() });
                    if current != previous { let _ = handle.emit("server-status", &current); previous = current; }
                }
            });

            // Windows 必须通过插件的 builder 创建窗口；平台配置禁用了自动创建。
            #[cfg(target_os = "windows")]
            {
                use tauri_plugin_window_controls::WindowControlsBuilderExt;

                let config = app
                    .config()
                    .app
                    .windows
                    .iter()
                    .find(|window| window.label == "main")
                    .ok_or_else(|| anyhow::anyhow!("缺少主窗口配置"))?;
                tauri::WebviewWindowBuilder::from_config(app, config)?
                    // 与 styles.css 的标题栏高度一致，控件由 Win32 层承载。
                    .window_controls_height(44)
                    .build()?;
            }

            let show = MenuItem::with_id(app, "show", "显示 P2PCLI", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let mut tray = TrayIconBuilder::with_id("main")
                .tooltip("P2PCLI")
                .menu(&menu)
                .on_menu_event(|app, ev| match ev.id().as_ref() {
                    "show" => show_main(app),
                    "quit" => {
                        let state = app.state::<AppState>();
                        tauri::async_runtime::block_on(state.node().shutdown());
                        app.exit(0);
                    }
                    _ => {}
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            if let Err(e) = tray.build(app) {
                warn!("托盘图标创建失败：{e}");
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // 关窗口只是藏起来，别人还要能连进来
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            overview,
            scan_nearby,
            diagnose,
            refresh_password,
            set_password_fixed,
            check_update,
            install_update,
            respond_request,
            kick,
            remove_trusted,
            forget_recent,
            connect,
            disconnect,
            path_info,
            open_terminal,
            list_terminals,
            end_terminal,
            term_input,
            term_resize,
            term_close,
            get_settings,
            save_settings,
            restart,
            selftest_config,
            selftest_report,
        ])
        .build(tauri::generate_context!())
        .expect("P2PCLI 启动失败");

    app.run(|app, event| {
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = event {
            show_main(app);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (app, event);
    });
}
