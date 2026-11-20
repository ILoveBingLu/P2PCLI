//! 远程画面：本机被看时的设置和同意，看别的设备时的窗口和收发。
//! 画面只走直连，中继只给终端用；抓屏、编解码都用硬件，见 p2pcli-screen 和 core 的 screen 模块。

use p2pcli_core::host::{BoxFuture, ScreenDecision, ScreenRequest};
use p2pcli_core::screen::{HostMsg, Quality, ScreenEvent, ScreenOpen, ScreenStarted, ViewerMsg};
use serde::Serialize;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::{mpsc, oneshot};
use tracing::{info, warn};

use crate::{AppState, UiDelegate, settings_path};

/// 画面窗口的标题栏高度，和主窗口、前端 .screen-toolbar 一致
#[cfg(target_os = "windows")]
const TITLEBAR_HEIGHT: u32 = 44;

/// 发给画面窗口的二进制消息：第一个字节是类型
const EV_JSON: u8 = 1;
const EV_FRAME: u8 = 2;

// ---------------------------------------------------------------- 本机被看

impl UiDelegate {
    pub(crate) fn approve_screen_request(&self, req: ScreenRequest) -> BoxFuture<ScreenDecision> {
        let deny = ScreenDecision { allow: false, control: false };
        let Some(state) = self.app.try_state::<AppState>() else {
            return Box::pin(async move { deny });
        };
        let (off, view_only) = {
            let s = state.settings.lock().unwrap();
            (s.screen_off, s.screen_view_only)
        };
        if off {
            info!("别的设备要看本机画面，设置里关掉了，直接拒绝");
            return Box::pin(async move { deny });
        }
        // 受信设备（一般是自己的另一台电脑）直接放行，界面上照样亮“正在被查看”
        if req.trusted {
            return Box::pin(async move { ScreenDecision { allow: true, control: !view_only } });
        }
        let (tx, rx) = oneshot::channel();
        self.pending_screens.lock().unwrap().insert(req.id, tx);
        crate::show_main(&self.app);
        #[cfg(desktop)]
        if let Some(w) = self.app.get_webview_window("main") {
            let _ = w.request_user_attention(Some(tauri::UserAttentionType::Critical));
        }
        let _ = self
            .app
            .notification()
            .builder()
            .title(format!("{} 想查看本机画面", req.name))
            .body("回到 P2PCLI 选择允许或拒绝")
            .show();
        let _ = self.app.emit("screen-request", ScreenRequestView { req, view_only });
        Box::pin(async move {
            let decision = rx.await.unwrap_or(deny);
            ScreenDecision { allow: decision.allow, control: decision.control && !view_only }
        })
    }
}

#[derive(Serialize, Clone)]
struct ScreenRequestView {
    #[serde(flatten)]
    req: ScreenRequest,
    /// 设置里只允许看：弹窗里不给“允许操作”
    view_only: bool,
}

#[tauri::command]
pub(crate) fn respond_screen(state: State<'_, AppState>, id: u64, allow: bool, control: bool) {
    if let Some(tx) = state.delegate.pending_screens.lock().unwrap().remove(&id) {
        let _ = tx.send(ScreenDecision { allow, control });
    }
}

/// 停止别人看本机画面；id 不写就全停
#[tauri::command]
pub(crate) fn stop_screen_share(state: State<'_, AppState>, id: Option<u64>) {
    state.node().host.stop_screen(id);
}

#[derive(Serialize)]
pub(crate) struct ScreenSettingsView {
    /// 这个系统能不能让别人看画面
    supported: bool,
    enabled: bool,
    view_only: bool,
    /// macOS 的录屏、辅助功能权限
    capture_permission: bool,
    control_permission: bool,
}

/// 本机能不能被看画面，和系统给没给录屏、辅助功能权限。手机只当连接的一方，没有这些
#[cfg(desktop)]
fn platform() -> (bool, bool, bool) {
    let perms = p2pcli_screen::permissions();
    (p2pcli_screen::backend().is_some(), perms.capture, perms.control)
}

#[cfg(mobile)]
fn platform() -> (bool, bool, bool) {
    (false, false, false)
}

fn screen_settings_view(state: &AppState) -> ScreenSettingsView {
    let s = state.settings.lock().unwrap();
    let (supported, capture_permission, control_permission) = platform();
    ScreenSettingsView { supported, enabled: !s.screen_off, view_only: s.screen_view_only, capture_permission, control_permission }
}

#[tauri::command]
pub(crate) fn screen_settings(state: State<'_, AppState>) -> ScreenSettingsView {
    screen_settings_view(&state)
}

#[tauri::command]
pub(crate) async fn set_screen_settings(state: State<'_, AppState>, enabled: bool, view_only: bool) -> Result<ScreenSettingsView, String> {
    let _gate = state.config_gate.lock().await;
    let s = {
        let mut s = state.settings.lock().unwrap().clone();
        s.screen_off = !enabled;
        s.screen_view_only = view_only;
        s
    };
    let bytes = serde_json::to_vec_pretty(&s).map_err(|e| e.to_string())?;
    p2pcli_core::store::write_private(&settings_path(&state.data_dir), &bytes).map_err(|e| format!("保存设置失败：{e:#}"))?;
    *state.settings.lock().unwrap() = s;
    let host = state.node().host.clone();
    if !enabled {
        host.stop_screen(None);
    } else if view_only {
        // 改成只许看：正在操作的立刻断开，要再看得重新打开（这回只给看）
        for v in host.screen_viewers().into_iter().filter(|v| v.control) {
            host.stop_screen(Some(v.id));
        }
    }
    Ok(screen_settings_view(&state))
}

/// 弹系统的授权提示（macOS）：capture 为真是录屏，否则是辅助功能
#[tauri::command]
pub(crate) fn screen_request_permission(state: State<'_, AppState>, capture: bool) -> ScreenSettingsView {
    #[cfg(desktop)]
    p2pcli_screen::request_permission(capture);
    #[cfg(mobile)]
    let _ = capture;
    screen_settings_view(&state)
}

// ---------------------------------------------------------------- 看别的设备

/// 对方支不支持远程画面（新版本、macOS 或 Windows）
#[tauri::command]
pub(crate) fn remote_supports_screen(state: State<'_, AppState>, conn_id: u64) -> bool {
    state.remote(conn_id).map(|r| r.supports_screen()).unwrap_or(false)
}

pub(crate) struct ScreenWindow {
    endpoint_id: String,
    device_name: String,
    platform: String,
}

#[derive(Serialize)]
pub(crate) struct ScreenWindowInfo {
    endpoint_id: String,
    device_name: String,
    /// 对方的系统：两边不一样时对调 ⌘ 和 Ctrl
    platform: String,
}

#[cfg(desktop)]
fn screen_label(endpoint_id: &str) -> String {
    let id: String = endpoint_id.chars().filter(|c| c.is_ascii_alphanumeric()).take(24).collect();
    format!("screen-{id}")
}

/// 打开（或叫到前面）某台设备的远程画面窗口。每台设备一个。
/// 必须是 async 命令：Windows 上在同步命令里建窗口会死锁（WebView2 的限制），新窗口卡住、关不掉
#[cfg(desktop)]
#[tauri::command]
pub(crate) async fn open_screen_window(app: AppHandle, state: State<'_, AppState>, endpoint_id: String, device_name: String, platform: String) -> Result<(), String> {
    let label = screen_label(&endpoint_id);
    if let Some(window) = app.get_webview_window(&label) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }
    state.screen_windows.lock().unwrap().insert(label.clone(), ScreenWindow { endpoint_id, device_name: device_name.clone(), platform });
    let builder = tauri::WebviewWindowBuilder::new(&app, &label, tauri::WebviewUrl::App("index.html?window=screen".into()))
        .title(format!("{device_name} — 远程画面"))
        .inner_size(1280.0, 800.0)
        .min_inner_size(480.0, 320.0);
    // 标题栏和主窗口一样自己画（工具栏就是标题栏），窗口按钮用系统原生的：
    // macOS 留着红绿灯，Windows 由 window-controls 插件画原生的最小化、最大化、关闭
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition::new(14.0, 24.0));
    #[cfg(target_os = "windows")]
    let builder = {
        use tauri_plugin_window_controls::WindowControlsBuilderExt;
        builder.theme(Some(tauri::Theme::Dark)).window_controls_height(TITLEBAR_HEIGHT)
    };
    let built = builder.build();
    if let Err(e) = built {
        state.screen_windows.lock().unwrap().remove(&label);
        return Err(format!("打不开远程画面窗口：{e}"));
    }
    Ok(())
}

#[cfg(mobile)]
#[tauri::command]
pub(crate) fn open_screen_window() -> Result<(), String> {
    Err(crate::MOBILE_UNSUPPORTED.into())
}

/// 画面窗口启动时问自己是哪台设备的
#[tauri::command]
pub(crate) fn screen_window_info(window: tauri::WebviewWindow, state: State<'_, AppState>) -> Result<ScreenWindowInfo, String> {
    let windows = state.screen_windows.lock().unwrap();
    let w = windows.get(window.label()).ok_or("这个画面窗口没有登记")?;
    Ok(ScreenWindowInfo { endpoint_id: w.endpoint_id.clone(), device_name: w.device_name.clone(), platform: w.platform.clone() })
}

#[derive(Serialize)]
pub(crate) struct ScreenOpened {
    screen_id: u64,
    started: ScreenStarted,
}

fn json_event(value: &serde_json::Value) -> InvokeResponseBody {
    let mut buf = vec![EV_JSON];
    buf.extend_from_slice(&serde_json::to_vec(value).unwrap_or_default());
    InvokeResponseBody::Raw(buf)
}

/// 开始看：对方点了同意才返回。画面、解码配置、状态都从 events 推给窗口
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn screen_open(
    app: AppHandle,
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    conn_id: u64,
    quality: Option<Quality>,
    control: bool,
    display: Option<String>,
    events: Channel<InvokeResponseBody>,
) -> Result<ScreenOpened, String> {
    let remote = state.remote(conn_id)?;
    let open = ScreenOpen { display, quality: quality.unwrap_or_default(), control };
    let (started, mut input, mut output) = remote.open_screen(open).await.map_err(|e| format!("{e:#}"))?;
    let screen_id = state.next();
    let (tx, mut rx) = mpsc::unbounded_channel::<ViewerMsg>();
    state.screens.lock().unwrap().insert(screen_id, ScreenEntry { window: window.label().to_string(), conn_id, tx: tx.clone() });
    info!(conn_id, screen_id, control = started.control, "开始看对方画面");

    // 发：确认、键鼠、换屏
    tauri::async_runtime::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if input.send(&msg).await.is_err() {
                break;
            }
        }
        input.close().await;
    });

    // 收：每帧马上回确认，再交给窗口去解码。这里只拿弱引用：窗口关掉（screen_close）后发送端没了，流就关上
    let ack = tx.downgrade();
    drop(tx);
    let remote = remote.clone();
    tauri::async_runtime::spawn(async move {
        let reason: Option<String> = loop {
            let ev = tokio::select! {
                ev = output.next() => ev,
                reason = remote.closed() => break Some(reason),
            };
            match ev {
                Ok(Some(ScreenEvent::Frame { seq, key, data, .. })) => {
                    match ack.upgrade() {
                        Some(tx) => { let _ = tx.send(ViewerMsg::Ack { seq }); }
                        None => break None,
                    }
                    let mut buf = Vec::with_capacity(10 + data.len());
                    buf.push(EV_FRAME);
                    buf.push(u8::from(key));
                    buf.extend_from_slice(&seq.to_be_bytes());
                    buf.extend_from_slice(&data);
                    if events.send(InvokeResponseBody::Raw(buf)).is_err() {
                        break None;
                    }
                }
                Ok(Some(ScreenEvent::Config(config))) => {
                    let _ = events.send(json_event(&serde_json::json!({ "t": "config", "config": config })));
                }
                Ok(Some(ScreenEvent::Host(HostMsg::Error { message }))) => break Some(message),
                Ok(Some(ScreenEvent::Host(msg))) => {
                    let _ = events.send(json_event(&serde_json::json!({ "t": "host", "msg": msg })));
                }
                Ok(None) => break None,
                Err(e) => {
                    warn!("收对方画面出错：{e:#}");
                    break Some(format!("{e:#}"));
                }
            }
        };
        let _ = events.send(json_event(&serde_json::json!({ "t": "closed", "reason": reason })));
        let state = app.state::<AppState>();
        state.screens.lock().unwrap().remove(&screen_id);
        release_held(&state, conn_id);
        info!(screen_id, "不再看对方画面");
    });
    Ok(ScreenOpened { screen_id, started })
}

/// 窗口发来的消息：键鼠、要关键帧、换屏、换画质
#[tauri::command]
pub(crate) fn screen_send(state: State<'_, AppState>, screen_id: u64, msgs: Vec<ViewerMsg>) {
    if let Some(ScreenEntry { tx, .. }) = state.screens.lock().unwrap().get(&screen_id) {
        for m in msgs {
            let _ = tx.send(m);
        }
    }
}

#[tauri::command]
pub(crate) fn screen_close(state: State<'_, AppState>, screen_id: u64) {
    // 丢掉发送端，对方读到流结束就停
    state.screens.lock().unwrap().remove(&screen_id);
}

/// 桌面端启动时挂上本机的远程画面实现
#[cfg(desktop)]
pub(crate) fn attach_backend(node: &p2pcli_core::node::Node) {
    if let Some(backend) = p2pcli_screen::backend() {
        node.host.set_screen_backend(backend);
    }
}

/// 一路正在看的画面
pub(crate) struct ScreenEntry {
    /// 哪个窗口在看：窗口关了就按窗口名停掉
    window: String,
    /// 走的哪条连接：画面开着时这条连接不断
    conn_id: u64,
    /// 发给对方的消息（确认、键鼠）；丢掉就停
    tx: mpsc::UnboundedSender<ViewerMsg>,
}

pub(crate) type ScreenSenders = std::collections::HashMap<u64, ScreenEntry>;

/// 画面窗口关掉了：丢掉它的发送端，对方读到流结束就停止抓屏
pub(crate) fn window_closed(state: &AppState, label: &str) {
    state.screen_windows.lock().unwrap().remove(label);
    state.screens.lock().unwrap().retain(|_, e| e.window != label);
}

/// 主窗口要断开这条连接时，画面还在用就先不断，等画面关了再断。
/// 远程控制从设备进，不依赖终端标签：关掉标签、离开设备页都不影响正在看的画面
pub(crate) fn hold_disconnect(state: &AppState, conn_id: u64) -> bool {
    let in_use = state.screens.lock().unwrap().values().any(|e| e.conn_id == conn_id);
    if in_use {
        state.screen_held.lock().unwrap().insert(conn_id);
    }
    in_use
}

/// 一路画面结束了：这条连接主窗口早就不要了、也没有别的画面在用，就真的断开
fn release_held(state: &AppState, conn_id: u64) {
    if state.screens.lock().unwrap().values().any(|e| e.conn_id == conn_id) {
        return;
    }
    if state.screen_held.lock().unwrap().remove(&conn_id) {
        info!(conn_id, "画面关了，断开主窗口早先要断的连接");
        crate::close_connection(state, conn_id);
    }
}

/// 画面窗口用哪条连接：已经在用的那条还活着就一直用它（同一台设备后来又连了新的也不换，
/// 免得画面重开、对方再点一次同意）；断了才换成这台设备现在最新的一条
#[tauri::command]
pub(crate) fn screen_connection(state: State<'_, AppState>, endpoint_id: String, current: Option<u64>) -> Option<u64> {
    let remotes = state.remotes.lock().unwrap();
    if let Some(id) = current
        && remotes.get(&id).is_some_and(|r| r.info.endpoint_id == endpoint_id && !r.is_closed())
    {
        return Some(id);
    }
    remotes.iter().filter(|(_, r)| r.info.endpoint_id == endpoint_id && !r.is_closed()).map(|(id, _)| *id).max()
}
pub(crate) type ScreenWindows = std::collections::HashMap<String, ScreenWindow>;
