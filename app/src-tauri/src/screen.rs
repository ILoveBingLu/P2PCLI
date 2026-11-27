//! 远程画面：本机被看时的设置和同意，看别的设备时的窗口和收发。
//! 画面只走直连，中继只给终端用；抓屏、编解码都用硬件，见 p2pcli-screen 和 core 的 screen 模块。

use p2pcli_core::host::{BoxFuture, ScreenDecision, ScreenRequest};
use p2pcli_core::screen::{HostMsg, Quality, ScreenEvent, ScreenOpen, ScreenStarted, ViewerMsg};
use serde::Serialize;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use std::sync::Arc;
use std::sync::atomic::Ordering;

use p2pcli_core::screen_clip::ViewerClip;

use crate::{AppState, UiDelegate, settings_path};

/// 画面窗口的标题栏高度，和主窗口、前端 .screen-toolbar 一致
#[cfg(target_os = "windows")]
const TITLEBAR_HEIGHT: u32 = 44;

/// 发给画面窗口的二进制消息：第一个字节是类型
const EV_JSON: u8 = 1;
const EV_FRAME: u8 = 2;
/// 一段解好的声音：[3, 0, 0, 0] + 48 kHz 立体声左右交错的 f32（小端）。前面补齐 4 字节，页面那边直接当 Float32Array 用
const EV_AUDIO: u8 = 3;

// ---------------------------------------------------------------- 本机被看

impl UiDelegate {
    pub(crate) fn approve_screen_request(&self, req: ScreenRequest) -> BoxFuture<ScreenDecision> {
        let deny = ScreenDecision { allow: false, control: false, privacy: false };
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
    /// 录屏是这次打开以后才允许的：要重开 P2PCLI 才生效
    capture_needs_restart: bool,
    /// 用户在第一次打开时的权限引导里点过“以后再说”或“完成”，不再自动弹
    guide_done: bool,
}

/// 打开 P2PCLI 时有没有录屏权限。录屏权限要重开才生效，拿它判断现在是不是该提示重开
#[cfg(desktop)]
static CAPTURE_AT_LAUNCH: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

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
    #[cfg(desktop)]
    let capture_needs_restart = capture_permission && !*CAPTURE_AT_LAUNCH.get_or_init(|| capture_permission);
    #[cfg(mobile)]
    let capture_needs_restart = false;
    ScreenSettingsView {
        supported,
        enabled: !s.screen_off,
        view_only: s.screen_view_only,
        capture_permission,
        control_permission,
        capture_needs_restart,
        guide_done: s.screen_guide_done,
    }
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

/// 直接打开系统设置里录屏或辅助功能那一页（macOS）：系统的授权提示只弹一次，被关掉以后从这里去
#[tauri::command]
pub(crate) fn screen_open_permission_settings(capture: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let pane = if capture { "Privacy_ScreenCapture" } else { "Privacy_Accessibility" };
        return std::process::Command::new("open")
            .arg(format!("x-apple.systempreferences:com.apple.preference.security?{pane}"))
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("打开系统设置失败：{e}"));
    }
    #[allow(unreachable_code)]
    {
        let _ = capture;
        Err("只有 macOS 要在系统设置里授权".into())
    }
}

/// 第一次打开时的权限引导：点过“以后再说”或“完成”就记下来，不再自动弹
#[tauri::command]
pub(crate) async fn screen_guide_done(state: State<'_, AppState>) -> Result<(), String> {
    let _gate = state.config_gate.lock().await;
    let s = {
        let mut s = state.settings.lock().unwrap().clone();
        s.screen_guide_done = true;
        s
    };
    let bytes = serde_json::to_vec_pretty(&s).map_err(|e| e.to_string())?;
    p2pcli_core::store::write_private(&settings_path(&state.data_dir), &bytes).map_err(|e| format!("保存设置失败：{e:#}"))?;
    *state.settings.lock().unwrap() = s;
    Ok(())
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
    #[cfg(target_os = "windows")]
    let builder = match crate::portable_webview_dir() {
        Some(dir) => builder.data_directory(dir),
        None => builder,
    };
    let built = builder.build();
    if let Err(e) = built {
        state.screen_windows.lock().unwrap().remove(&label);
        return Err(format!("打不开远程画面窗口：{e}"));
    }
    Ok(())
}

/// 画面窗口进出全屏，返回现在是不是全屏。
/// macOS 用“简单全屏”（不新开桌面）并把菜单栏、程序坞整个藏掉：系统的全屏下鼠标碰到屏幕顶端会滑出菜单栏，
/// 正好盖住我们滑下来的工具栏，退不出全屏。别的系统就是普通全屏。
#[cfg(desktop)]
#[tauri::command]
pub(crate) fn screen_set_fullscreen(window: tauri::WebviewWindow, on: bool) -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        // 用户按绿灯进的是系统全屏，先退出来再说
        if !on && window.is_fullscreen().unwrap_or(false) {
            window.set_fullscreen(false).map_err(|e| e.to_string())?;
            return Ok(false);
        }
        window.set_simple_fullscreen(on).map_err(|e| e.to_string())?;
        if on {
            // 排在进简单全屏之后执行（它会把菜单栏设成“自动隐藏”），这里改成整个藏掉；退出时它自己恢复原样
            let _ = window.run_on_main_thread(|| {
                use objc2_app_kit::{NSApplication, NSApplicationPresentationOptions as P};
                if let Some(mtm) = objc2::MainThreadMarker::new() {
                    NSApplication::sharedApplication(mtm).setPresentationOptions(P::HideDock | P::HideMenuBar);
                }
            });
        }
        Ok(on)
    }
    #[cfg(not(target_os = "macos"))]
    {
        window.set_fullscreen(on).map_err(|e| e.to_string())?;
        Ok(on)
    }
}

#[cfg(mobile)]
#[tauri::command]
pub(crate) fn screen_set_fullscreen() -> Result<bool, String> {
    Err(crate::MOBILE_UNSUPPORTED.into())
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
    codecs: Option<Vec<p2pcli_core::screen::VideoCodec>>,
    color_tags: Option<bool>,
    wide_gamut: Option<bool>,
    chroma444: Option<bool>,
    clipboard: Option<bool>,
    refresh_hz: Option<u32>,
    keyboard: Option<p2pcli_core::screen_keys::KeyboardKind>,
    translate_keys: Option<bool>,
    audio_on: Option<bool>,
    events: Channel<InvokeResponseBody>,
) -> Result<ScreenOpened, String> {
    let remote = state.remote(conn_id)?;
    let open = ScreenOpen {
        display,
        quality: quality.unwrap_or_default(),
        control,
        codecs: codecs.unwrap_or_default(),
        color_tags: color_tags.unwrap_or(false),
        wide_gamut: wide_gamut.unwrap_or(false),
        chroma444: chroma444.unwrap_or(false),
        clipboard: clipboard.unwrap_or(false),
        refresh_hz: refresh_hz.unwrap_or(0),
        keyboard,
        translate_keys: translate_keys.unwrap_or(false),
        // Opus 用纯 Rust 解，哪个平台都能放
        audio: true,
    };
    let (started, mut input, mut output) = remote.open_screen(open).await.map_err(|e| format!("{e:#}"))?;
    let screen_id = state.next();
    let (tx, mut rx) = mpsc::unbounded_channel::<ViewerMsg>();
    // 剪贴板互通：对方同意了、本机读写得了剪贴板（只有桌面端）
    #[cfg(desktop)]
    let platform = p2pcli_screen::clipboard();
    #[cfg(mobile)]
    let platform: Option<Arc<dyn p2pcli_core::screen_clip::ClipPlatform>> = None;
    let clip = match platform {
        Some(p) if started.clipboard => Some(ViewerClip::new(p, remote.clone(), started.clip_token)),
        _ => None,
    };
    state.screens.lock().unwrap().insert(
        screen_id,
        ScreenEntry {
            window: window.label().to_string(),
            conn_id,
            tx: tx.clone(),
            clip: clip.clone(),
            audio_token: started.audio_token,
            events: events.clone(),
            audio: None,
        },
    );
    if let Some(clip) = &clip {
        spawn_clip_poll(clip.clone(), tx.downgrade());
        // 剪贴板内容走中继传：窗口里提示会比较慢
        let events = events.clone();
        clip.on_relay(move || {
            let _ = events.send(json_event(&serde_json::json!({ "t": "notice", "kind": "clip_relay" })));
        });
    }
    // 对方的剪贴板清单按到达的先后放进本机剪贴板
    let (apply_tx, mut apply_rx) = mpsc::unbounded_channel::<p2pcli_core::screen_clip::ClipOffer>();
    if let Some(clip) = clip.clone() {
        tauri::async_runtime::spawn(async move {
            while let Some(offer) = apply_rx.recv().await {
                if !clip.enabled() {
                    continue;
                }
                let (sync, fetch) = (clip.sync.clone(), clip.fetcher());
                match tokio::task::spawn_blocking(move || sync.apply(offer, fetch)).await {
                    Ok(Err(e)) => warn!("放进对方的剪贴板内容失败：{e:#}"),
                    Err(e) => warn!("放进对方的剪贴板内容失败：{e}"),
                    Ok(Ok(())) => {}
                }
            }
        });
    }
    info!(conn_id, screen_id, control = started.control, audio = started.audio_token != 0, "开始看对方画面");
    if audio_on.unwrap_or(true) {
        set_audio(&state, screen_id, true);
    }

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
        // 诊断：每 5 秒记一行收到的帧率、码率、最长间隔，和对方报的、本机解码报的放在一起看
        let mut diag = Diag::default();
        let reason: Option<String> = loop {
            if diag.since.elapsed() >= DIAG_EVERY {
                let path = remote.path();
                diag.log(screen_id, &path);
            }
            let ev = tokio::select! {
                ev = output.next() => ev,
                reason = remote.closed() => break Some(reason),
            };
            match ev {
                Ok(Some(ScreenEvent::Frame { seq, key, data, .. })) => {
                    diag.frame(screen_id, seq, key, data.len());
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
                    diag.codec = config.codec.clone();
                    let _ = events.send(json_event(&serde_json::json!({ "t": "config", "config": config })));
                }
                Ok(Some(ScreenEvent::Host(HostMsg::Error { message }))) => break Some(message),
                // 对方在画面里复制了东西：清单放进本机剪贴板，粘贴时才传
                Ok(Some(ScreenEvent::Host(HostMsg::ClipOffer { offer }))) => {
                    let _ = apply_tx.send(offer);
                }
                // 对方那边有程序要粘贴本机剪贴板里的内容：开一条流送过去
                Ok(Some(ScreenEvent::Host(HostMsg::ClipWant { req, offer, what }))) => {
                    if let Some(clip) = clip.clone() {
                        tauri::async_runtime::spawn(async move {
                            if let Err(e) = clip.put(req, offer, what).await {
                                warn!("给对方剪贴板内容失败：{e:#}");
                            }
                        });
                    }
                }
                Ok(Some(ScreenEvent::Host(msg))) => {
                    if let HostMsg::Stats { bitrate, fps, .. } = msg {
                        diag.host = Some((fps, bitrate));
                    }
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
        // 主窗口早先要断这条连接：画面关了，别的也不用了就断
        crate::release_if_unused(&state, conn_id);
        info!(screen_id, "不再看对方画面");
    });
    Ok(ScreenOpened { screen_id, started })
}

/// 两帧之间隔这么久就单独记一行停顿
const STALL_LOG: std::time::Duration = std::time::Duration::from_millis(150);

/// 诊断日志隔多久记一次
const DIAG_EVERY: std::time::Duration = std::time::Duration::from_secs(5);

/// 观看端的诊断计数：卡在哪一环（对方编码、网络、本机解码）从这里看
struct Diag {
    since: std::time::Instant,
    last_frame: Option<std::time::Instant>,
    frames: u32,
    bytes: usize,
    keys: u32,
    max_gap: std::time::Duration,
    /// 两帧之间隔了 STALL_LOG 以上的次数
    stalls: u32,
    codec: String,
    host: Option<(u32, u32)>,
}

impl Default for Diag {
    fn default() -> Self {
        Diag { since: std::time::Instant::now(), last_frame: None, frames: 0, bytes: 0, keys: 0, max_gap: Default::default(), stalls: 0, codec: String::new(), host: None }
    }
}

impl Diag {
    fn frame(&mut self, screen_id: u64, seq: u64, key: bool, bytes: usize) {
        let now = std::time::Instant::now();
        if let Some(last) = self.last_frame {
            let gap = now - last;
            self.max_gap = self.max_gap.max(gap);
            // 明显的停顿数一下，跟着每 5 秒的统计一起记。画面不动时本来就没有帧，所以只算个数不逐条记
            if gap >= STALL_LOG {
                self.stalls += 1;
                debug!(screen_id, seq, 间隔 = format!("{}ms", gap.as_millis()), 关键帧 = key, 大小 = format!("{}KB", bytes / 1024), "画面停顿");
            }
        }
        self.last_frame = Some(now);
        self.frames += 1;
        self.bytes += bytes;
        self.keys += u32::from(key);
    }

    fn log(&mut self, screen_id: u64, path: &p2pcli_core::path::PathInfo) {
        let secs = self.since.elapsed().as_secs_f64();
        info!(
            screen_id,
            收到帧率 = format!("{:.0}", self.frames as f64 / secs),
            收到码率 = format!("{:.2}Mbps", self.bytes as f64 * 8.0 / secs / 1e6),
            关键帧 = self.keys,
            最长间隔 = format!("{}ms", self.max_gap.as_millis()),
            停顿次数 = self.stalls,
            对方帧率 = self.host.map(|h| h.0),
            对方码率 = self.host.map(|h| format!("{:.2}Mbps", h.1 as f64 / 1e6)),
            格式 = %self.codec,
            线路 = ?path.kind,
            延时 = ?path.rtt_ms,
            "画面统计"
        );
        let codec = std::mem::take(&mut self.codec);
        *self = Diag { codec, host: self.host, ..Default::default() };
    }
}

/// 画面窗口每 5 秒报一次本机解码的情况，和收到的放在一起看卡在哪
#[tauri::command]
pub(crate) fn screen_report(screen_id: u64, decoded: u32, errors: u32, queue: u32, hardware: bool, seconds: f64) {
    info!(
        screen_id,
        解出帧率 = format!("{:.0}", decoded as f64 / seconds.max(0.1)),
        解码出错 = errors,
        积压 = queue,
        硬件解码 = hardware,
        "画面解码统计"
    );
}

/// 本机剪贴板：每隔一会儿看一眼，变了就把清单发给对方；画面关了就停
fn spawn_clip_poll(clip: Arc<ViewerClip>, tx: mpsc::WeakUnboundedSender<ViewerMsg>) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(p2pcli_core::screen_clip::POLL).await;
            let Some(tx) = tx.upgrade() else { return };
            if !clip.enabled() {
                continue;
            }
            let sync = clip.sync.clone();
            match tokio::task::spawn_blocking(move || sync.poll()).await {
                Ok(Ok(Some(offer))) => {
                    let _ = tx.send(ViewerMsg::ClipOffer { offer });
                }
                Ok(Err(e)) => debug!("读本机剪贴板失败：{e:#}"),
                _ => {}
            }
        }
    });
}

/// 窗口发来的消息：键鼠、要关键帧、换屏、换画质。
/// clipboard 为真（按了粘贴键）时，先看一眼本机剪贴板，刚复制了东西就先把清单发过去，
/// 排在这批键鼠前面：对方读到粘贴键时剪贴板已经是新的（不用等下一次定时看）
#[tauri::command]
pub(crate) fn screen_send(state: State<'_, AppState>, screen_id: u64, msgs: Vec<ViewerMsg>, clipboard: Option<bool>) {
    if let Some(ScreenEntry { tx, clip, .. }) = state.screens.lock().unwrap().get(&screen_id) {
        if clipboard == Some(true)
            && let Some(clip) = clip
            && clip.enabled()
        {
            match clip.sync.poll() {
                Ok(Some(offer)) => {
                    let _ = tx.send(ViewerMsg::ClipOffer { offer });
                }
                Ok(None) => {}
                Err(e) => debug!("读本机剪贴板失败：{e:#}"),
            }
        }
        for m in msgs {
            let _ = tx.send(m);
        }
    }
}

/// 观看窗口里开关剪贴板互通
#[tauri::command]
pub(crate) fn screen_set_clipboard(state: State<'_, AppState>, screen_id: u64, on: bool) {
    if let Some(ScreenEntry { clip: Some(clip), .. }) = state.screens.lock().unwrap().get(&screen_id) {
        clip.enabled.store(on, Ordering::Relaxed);
    }
}

/// 观看窗口里开关声音：关的时候把声音流关掉，对方就不录了，不白占带宽
#[tauri::command]
pub(crate) fn screen_set_audio(state: State<'_, AppState>, screen_id: u64, on: bool) {
    set_audio(&state, screen_id, on);
}

fn set_audio(state: &AppState, screen_id: u64, on: bool) {
    let mut screens = state.screens.lock().unwrap();
    let Some(entry) = screens.get_mut(&screen_id) else { return };
    if let Some(task) = entry.audio.take() {
        task.abort();
    }
    if !on || entry.audio_token == 0 {
        return;
    }
    let Ok(remote) = state.remote(entry.conn_id) else { return };
    let (token, events) = (entry.audio_token, entry.events.clone());
    entry.audio = Some(tauri::async_runtime::spawn(async move {
        if let Err(e) = play_audio(remote, token, events).await {
            warn!("收对方的声音出错：{e:#}");
        }
    }));
}

/// 收对方的声音、解成 PCM 交给窗口去放。任务被取消时收的流跟着丢掉，对方读到就停
async fn play_audio(remote: Arc<p2pcli_core::client::RemoteHost>, token: u64, events: Channel<InvokeResponseBody>) -> anyhow::Result<()> {
    let mut rx = remote.open_screen_audio(token).await?;
    let mut decoder = p2pcli_core::screen_audio::Decoder::new()?;
    info!("开始收对方的声音");
    while let Some((_ts, packet)) = rx.next().await? {
        let pcm = decoder.decode(&packet)?;
        let mut buf = Vec::with_capacity(4 + pcm.len() * 4);
        buf.extend_from_slice(&[EV_AUDIO, 0, 0, 0]);
        for v in pcm {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        if events.send(InvokeResponseBody::Raw(buf)).is_err() {
            break;
        }
    }
    info!("不再收对方的声音");
    Ok(())
}

#[tauri::command]
pub(crate) fn screen_close(state: State<'_, AppState>, screen_id: u64) {
    // 丢掉发送端，对方读到流结束就停
    state.screens.lock().unwrap().remove(&screen_id);
}

/// 桌面端启动时挂上本机的远程画面实现
#[cfg(desktop)]
pub(crate) fn attach_backend(node: &p2pcli_core::node::Node) {
    CAPTURE_AT_LAUNCH.get_or_init(|| p2pcli_screen::permissions().capture);
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
    /// 剪贴板互通（对方同意了才有）
    clip: Option<Arc<ViewerClip>>,
    /// 对方给的声音号：0 是对方不传声音
    audio_token: u64,
    /// 往窗口送画面、声音的通道
    events: Channel<InvokeResponseBody>,
    /// 正在收声音的任务（静音时没有）
    audio: Option<tauri::async_runtime::JoinHandle<()>>,
}

impl Drop for ScreenEntry {
    fn drop(&mut self) {
        // 不看了：声音也停
        if let Some(task) = self.audio.take() {
            task.abort();
        }
    }
}

pub(crate) type ScreenSenders = std::collections::HashMap<u64, ScreenEntry>;

/// 画面窗口关掉了：丢掉它的发送端，对方读到流结束就停止抓屏
pub(crate) fn window_closed(state: &AppState, label: &str) {
    state.screen_windows.lock().unwrap().remove(label);
    state.screens.lock().unwrap().retain(|_, e| e.window != label);
}

/// 这条连接上有没有正在看的画面。主窗口要断开时画面还在用就先不断，等画面关了再断：
/// 远程控制从设备进，不依赖终端标签，关掉标签、离开设备页都不影响正在看的画面
pub(crate) fn in_use(state: &AppState, conn_id: u64) -> bool {
    state.screens.lock().unwrap().values().any(|e| e.conn_id == conn_id)
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
