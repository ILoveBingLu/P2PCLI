//! 远程画面：手机看对方电脑的屏幕，用触屏操作它。
//!
//! 画面在对方电脑上硬件编码成 H.264/H.265（AVCC：每个 NAL 前面 4 字节长度），这里原样交给界面，
//! 界面用系统的硬件解码器解（iOS 的 VideoToolbox、Android 的 MediaCodec）。
//! 每收到一帧马上回确认，对方按确认回来的快慢调码率、决定要不要发下一帧，和桌面端一样。
//! 画面只走直连：连接落到中继上时对方暂停发画面（[`ScreenListener::on_paused`]），直连恢复后自动接着发。

use std::sync::Arc;

use base64::Engine;
use p2pcli_core::client::RemoteHost;
use p2pcli_core::screen::{self as core, HostMsg, InputEvent, PauseReason, ScreenEvent, ScreenOpen, ViewerMsg};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::P2pError;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ScreenQuality {
    /// 清晰：原始分辨率，看小字
    Sharp,
    /// 均衡：长边最多 1920
    Balanced,
    /// 流畅：长边最多 1280、30 帧，网差时用
    Smooth,
}

impl From<ScreenQuality> for core::Quality {
    fn from(q: ScreenQuality) -> Self {
        match q {
            ScreenQuality::Sharp => core::Quality::Sharp,
            ScreenQuality::Balanced => core::Quality::Balanced,
            ScreenQuality::Smooth => core::Quality::Smooth,
        }
    }
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ScreenCodec {
    H264,
    H265,
}

/// 对方电脑的一块屏幕
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[derive(Debug, Clone, uniffi::Record)]
pub struct ScreenDisplay {
    pub id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

/// 对方同意以后的情况
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[derive(Debug, Clone, uniffi::Record)]
pub struct ScreenStartedView {
    pub displays: Vec<ScreenDisplay>,
    /// 正在看的屏幕
    pub display: String,
    /// 能不能操作：对方只给看、或者对方电脑没给辅助功能权限时为假
    pub control: bool,
    /// 不能操作的原因
    pub control_note: Option<String>,
    /// 能直接输入文字（中文、表情）。对方是老版本时为假，那时只能按键（英文字母、数字、符号）
    pub text_input: bool,
}

/// 解码器配置：尺寸、参数变了会再来一次，界面要重新配置解码器
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[derive(Debug, Clone, uniffi::Record)]
pub struct ScreenVideoConfig {
    pub codec: ScreenCodec,
    /// WebCodecs 的写法，比如 avc1.640033，只用来显示
    pub codec_name: String,
    pub width: u32,
    pub height: u32,
    /// H.264 是 avcC、H.265 是 hvcC（解码器配置记录），原始字节
    pub description: Vec<u8>,
    /// 全范围（0~255）；假是视频范围（16~235）
    pub full_range: bool,
}

/// 鼠标指针的样子：RGBA（不预乘透明度）
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[derive(Debug, Clone, uniffi::Record)]
pub struct ScreenCursorShape {
    /// 图案变了编号就变
    pub id: u64,
    pub width: u32,
    pub height: u32,
    /// 热点（点击的那个点），按图片像素算
    pub hot_x: f64,
    pub hot_y: f64,
    /// 图片像素和屏幕上的点的比例：Retina 是 2
    pub scale: f64,
    pub rgba: Vec<u8>,
}

#[cfg_attr(feature = "serde", derive(serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ScreenMouseButton {
    Left,
    Right,
    Middle,
}

/// 发给对方的键鼠事件。坐标是相对当前屏幕的 0~1；
/// 键是浏览器 KeyboardEvent.code（按物理位置，比如 KeyA、Enter、Backspace、ControlLeft、MetaLeft）
#[cfg_attr(feature = "serde", derive(serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "k", rename_all = "snake_case"))]
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum ScreenInput {
    Move { x: f64, y: f64 },
    Button { button: ScreenMouseButton, down: bool, x: f64, y: f64 },
    /// 滚动，像素；dy 正数往下
    Wheel { dx: f64, dy: f64, x: f64, y: f64 },
    Key { code: String, down: bool },
    /// 直接输入文字；只在 [`ScreenStartedView::text_input`] 为真时发
    Text { text: String },
    /// 把按着的键和鼠标都松开
    ReleaseAll,
}

impl From<ScreenInput> for InputEvent {
    fn from(i: ScreenInput) -> Self {
        let button = |b| match b {
            ScreenMouseButton::Left => core::MouseButton::Left,
            ScreenMouseButton::Right => core::MouseButton::Right,
            ScreenMouseButton::Middle => core::MouseButton::Middle,
        };
        match i {
            ScreenInput::Move { x, y } => InputEvent::Move { x, y },
            ScreenInput::Button { button: b, down, x, y } => InputEvent::Button { button: button(b), down, x, y },
            ScreenInput::Wheel { dx, dy, x, y } => InputEvent::Wheel { dx, dy, x, y },
            ScreenInput::Key { code, down } => InputEvent::Key { code, down },
            ScreenInput::Text { text } => InputEvent::Text { text },
            ScreenInput::ReleaseAll => InputEvent::ReleaseAll,
        }
    }
}

/// 画面这一路的消息。从 Rust 的线程调过来，界面自己切回主线程（画面帧可以直接在这个线程喂给解码器）
#[uniffi::export(with_foreign)]
pub trait ScreenListener: Send + Sync {
    /// 解码器配置：第一帧之前、尺寸或参数变了时来
    fn on_config(&self, config: ScreenVideoConfig);
    /// 一帧画面，AVCC 格式；key 为真是关键帧
    fn on_frame(&self, key: bool, data: Vec<u8>);
    /// 指针挪了：相对当前屏幕的 0~1。对方能单独报指针时才有，这时画面里不画指针
    fn on_cursor(&self, x: f64, y: f64, visible: bool);
    /// 指针换样子了
    fn on_cursor_shape(&self, shape: ScreenCursorShape);
    /// 对方暂停发画面：relay 为真是连接走了中继（画面只走直连），假是还在找路
    fn on_paused(&self, relay: bool);
    fn on_resumed(&self);
    /// 换了屏幕
    fn on_switched(&self, display: String);
    /// 每秒一次：对方现在的码率、帧率
    fn on_stats(&self, bitrate: u32, fps: u32);
    /// 画面结束了：reason 有值是出错或连接断了，空是正常关掉
    fn on_closed(&self, reason: Option<String>);
}

/// 一路正在看的画面。丢掉或调 stop 就停
#[derive(uniffi::Object)]
pub struct ScreenView {
    started: ScreenStartedView,
    tx: mpsc::UnboundedSender<Cmd>,
}

enum Cmd {
    Msg(ViewerMsg),
    Stop,
}

#[uniffi::export]
impl ScreenView {
    pub fn started(&self) -> ScreenStartedView {
        self.started.clone()
    }

    /// 发一个键鼠事件。连续的移动会合并，发得再勤也不会积压
    pub fn send(&self, input: ScreenInput) {
        let _ = self.tx.send(Cmd::Msg(ViewerMsg::Input { ev: input.into() }));
    }

    /// 打一段字（手机键盘、粘贴）。modifiers 是按住的修饰键（ControlLeft、AltLeft、MetaLeft、ShiftLeft），
    /// 为空且对方能收文字时整段直接输入，中文、表情都行；否则一个字一个字按键（只有英文键盘上有的字能打出来），
    /// 带着修饰键就是快捷键，比如 MetaLeft + c。换行按回车、制表符按 Tab
    pub fn type_text(&self, text: String, modifiers: Vec<String>) {
        if modifiers.is_empty() && self.started.text_input && !text.contains(['\n', '\r', '\t', '\u{8}']) {
            let _ = self.tx.send(Cmd::Msg(ViewerMsg::Input { ev: InputEvent::Text { text } }));
            return;
        }
        let mut pending = String::new();
        for ch in text.chars() {
            let Some((code, shift)) = key_for_char(ch) else {
                // 英文键盘上没有的字：没有修饰键时能直接输入就攒着一起发
                if modifiers.is_empty() && self.started.text_input {
                    pending.push(ch);
                }
                continue;
            };
            if !pending.is_empty() {
                let _ = self.tx.send(Cmd::Msg(ViewerMsg::Input { ev: InputEvent::Text { text: std::mem::take(&mut pending) } }));
            }
            let mut held: Vec<String> = modifiers.clone();
            if shift && !held.iter().any(|m| m.starts_with("Shift")) {
                held.push("ShiftLeft".into());
            }
            self.stroke(code, &held);
        }
        if !pending.is_empty() {
            let _ = self.tx.send(Cmd::Msg(ViewerMsg::Input { ev: InputEvent::Text { text: pending } }));
        }
    }

    /// 按一下某个键（Escape、Tab、Backspace、ArrowLeft、F5……），modifiers 是同时按住的修饰键
    pub fn tap_key(&self, code: String, modifiers: Vec<String>) {
        self.stroke(&code, &modifiers);
    }

    /// 换屏幕
    pub fn switch_display(&self, display: String) {
        let _ = self.tx.send(Cmd::Msg(ViewerMsg::Switch { display }));
    }

    pub fn set_quality(&self, quality: ScreenQuality) {
        let _ = self.tx.send(Cmd::Msg(ViewerMsg::SetQuality { quality: quality.into() }));
    }

    /// 解码出错、刚重新配置解码器（比如切回前台重建了显示层）时要一个关键帧
    pub fn request_keyframe(&self) {
        let _ = self.tx.send(Cmd::Msg(ViewerMsg::KeyFrame));
    }

    /// 不看了。不叫 close：UniFFI 生成的对象自带 close
    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Msg(ViewerMsg::Input { ev: InputEvent::ReleaseAll }));
        let _ = self.tx.send(Cmd::Stop);
    }
}

impl ScreenView {
    /// 修饰键按下 → 键按下、松开 → 修饰键倒着松开
    fn stroke(&self, code: &str, modifiers: &[String]) {
        let key = |code: &str, down: bool| Cmd::Msg(ViewerMsg::Input { ev: InputEvent::Key { code: code.to_string(), down } });
        for m in modifiers {
            let _ = self.tx.send(key(m, true));
        }
        let _ = self.tx.send(key(code, true));
        let _ = self.tx.send(key(code, false));
        for m in modifiers.iter().rev() {
            let _ = self.tx.send(key(m, false));
        }
    }
}

/// 一个字在美式键盘上是哪个键、要不要按着 Shift
pub(crate) fn key_for_char(ch: char) -> Option<(&'static str, bool)> {
    const LETTERS: [&str; 26] = [
        "KeyA", "KeyB", "KeyC", "KeyD", "KeyE", "KeyF", "KeyG", "KeyH", "KeyI", "KeyJ", "KeyK", "KeyL", "KeyM", "KeyN", "KeyO",
        "KeyP", "KeyQ", "KeyR", "KeyS", "KeyT", "KeyU", "KeyV", "KeyW", "KeyX", "KeyY", "KeyZ",
    ];
    const DIGITS: [&str; 10] = ["Digit0", "Digit1", "Digit2", "Digit3", "Digit4", "Digit5", "Digit6", "Digit7", "Digit8", "Digit9"];
    Some(match ch {
        'a'..='z' => (LETTERS[(ch as u8 - b'a') as usize], false),
        'A'..='Z' => (LETTERS[(ch as u8 - b'A') as usize], true),
        '0'..='9' => (DIGITS[(ch as u8 - b'0') as usize], false),
        ' ' => ("Space", false),
        '\n' | '\r' => ("Enter", false),
        '\t' => ("Tab", false),
        '\u{8}' => ("Backspace", false),
        '-' => ("Minus", false),
        '_' => ("Minus", true),
        '=' => ("Equal", false),
        '+' => ("Equal", true),
        '[' => ("BracketLeft", false),
        '{' => ("BracketLeft", true),
        ']' => ("BracketRight", false),
        '}' => ("BracketRight", true),
        '\\' => ("Backslash", false),
        '|' => ("Backslash", true),
        ';' => ("Semicolon", false),
        ':' => ("Semicolon", true),
        '\'' => ("Quote", false),
        '"' => ("Quote", true),
        ',' => ("Comma", false),
        '<' => ("Comma", true),
        '.' => ("Period", false),
        '>' => ("Period", true),
        '/' => ("Slash", false),
        '?' => ("Slash", true),
        '`' => ("Backquote", false),
        '~' => ("Backquote", true),
        '!' => ("Digit1", true),
        '@' => ("Digit2", true),
        '#' => ("Digit3", true),
        '$' => ("Digit4", true),
        '%' => ("Digit5", true),
        '^' => ("Digit6", true),
        '&' => ("Digit7", true),
        '*' => ("Digit8", true),
        '(' => ("Digit9", true),
        ')' => ("Digit0", true),
        _ => return None,
    })
}

fn started_view(s: core::ScreenStarted) -> ScreenStartedView {
    ScreenStartedView {
        displays: s
            .displays
            .into_iter()
            .map(|d| ScreenDisplay { id: d.id, name: d.name, width: d.width, height: d.height, primary: d.primary })
            .collect(),
        display: s.display,
        control: s.control,
        control_note: s.control_note,
        text_input: s.text_input,
    }
}

fn config_view(c: core::VideoConfig) -> Result<ScreenVideoConfig, String> {
    let description = base64::engine::general_purpose::STANDARD.decode(&c.description).map_err(|e| format!("解码器配置不对：{e}"))?;
    let codec = if c.codec.starts_with("hvc1") || c.codec.starts_with("hev1") { ScreenCodec::H265 } else { ScreenCodec::H264 };
    Ok(ScreenVideoConfig { codec, codec_name: c.codec, width: c.width, height: c.height, description, full_range: c.full_range })
}

fn shape_view(s: core::CursorShape) -> Option<ScreenCursorShape> {
    let rgba = base64::engine::general_purpose::STANDARD.decode(&s.rgba).ok()?;
    Some(ScreenCursorShape { id: s.id, width: s.width, height: s.height, hot_x: s.hot_x, hot_y: s.hot_y, scale: s.scale, rgba })
}

/// 开始看：对方点了同意才返回。之后画面、状态从 listener 来
pub(crate) async fn open(
    remote: Arc<RemoteHost>,
    handle: tokio::runtime::Handle,
    quality: ScreenQuality,
    control: bool,
    codecs: Vec<ScreenCodec>,
    listener: Arc<dyn ScreenListener>,
) -> Result<Arc<ScreenView>, P2pError> {
    if !remote.supports_screen() {
        return Err(P2pError::failed("对方的 P2PCLI 版本较旧或系统不支持远程画面，请更新对方的 P2PCLI"));
    }
    let open = ScreenOpen {
        display: None,
        quality: quality.into(),
        control,
        codecs: codecs
            .into_iter()
            .map(|c| match c {
                ScreenCodec::H264 => core::VideoCodec::H264,
                ScreenCodec::H265 => core::VideoCodec::H265,
            })
            .collect(),
        // 手机的解码器认 SPS 里的范围标记，用视频范围、sRGB、4:2:0 最稳
        color_tags: false,
        wide_gamut: false,
        yuv444: false,
    };
    let (started, mut input, mut output) = remote.open_screen(open).await.map_err(P2pError::from_anyhow)?;
    let started = started_view(started);
    info!(control = started.control, text = started.text_input, "开始看对方画面");
    let (tx, mut rx) = mpsc::unbounded_channel::<Cmd>();

    // 发：确认、键鼠、换屏。攒下来的连续移动只发最后一个
    handle.spawn(async move {
        'outer: while let Some(first) = rx.recv().await {
            let mut batch = vec![first];
            while let Ok(m) = rx.try_recv() {
                batch.push(m);
            }
            let mut out: Vec<ViewerMsg> = Vec::with_capacity(batch.len());
            for m in batch {
                let Cmd::Msg(m) = m else {
                    for m in out.drain(..) {
                        let _ = input.send(&m).await;
                    }
                    break 'outer;
                };
                let is_move = matches!(m, ViewerMsg::Input { ev: InputEvent::Move { .. } });
                if is_move && matches!(out.last(), Some(ViewerMsg::Input { ev: InputEvent::Move { .. } })) {
                    out.pop();
                }
                out.push(m);
            }
            for m in out {
                if input.send(&m).await.is_err() {
                    break 'outer;
                }
            }
        }
        input.close().await;
    });

    // 收：每帧马上回确认，再交给界面。界面释放了 ScreenView（发送端没了）就停
    let ack = tx.downgrade();
    let watch = remote.clone();
    handle.spawn(async move {
        let reason: Option<String> = loop {
            let ev = tokio::select! {
                ev = output.next() => ev,
                reason = watch.closed() => break Some(reason),
            };
            match ev {
                Ok(Some(ScreenEvent::Frame { seq, key, data, .. })) => {
                    match ack.upgrade() {
                        Some(tx) => {
                            let _ = tx.send(Cmd::Msg(ViewerMsg::Ack { seq }));
                        }
                        None => break None,
                    }
                    listener.on_frame(key, data.to_vec());
                }
                Ok(Some(ScreenEvent::Config(config))) => match config_view(config) {
                    Ok(c) => listener.on_config(c),
                    Err(e) => break Some(e),
                },
                Ok(Some(ScreenEvent::Host(msg))) => match msg {
                    HostMsg::Error { message } => break Some(message),
                    HostMsg::Paused { reason } => listener.on_paused(reason == PauseReason::Relay),
                    HostMsg::Resumed => listener.on_resumed(),
                    HostMsg::Switched { display } => listener.on_switched(display),
                    HostMsg::Stats { bitrate, fps } => listener.on_stats(bitrate, fps),
                    HostMsg::Cursor { x, y, visible } => listener.on_cursor(x, y, visible),
                    HostMsg::CursorShape { shape } => {
                        if let Some(s) = shape_view(shape) {
                            listener.on_cursor_shape(s);
                        }
                    }
                },
                Ok(None) => break None,
                Err(e) => {
                    warn!("收对方画面出错：{e:#}");
                    break Some(format!("{e:#}"));
                }
            }
        };
        info!("不再看对方画面");
        listener.on_closed(reason);
    });
    Ok(Arc::new(ScreenView { started, tx }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(text_input: bool) -> (ScreenView, mpsc::UnboundedReceiver<Cmd>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let started = ScreenStartedView { displays: vec![], display: "1".into(), control: true, control_note: None, text_input };
        (ScreenView { started, tx }, rx)
    }

    fn events(rx: &mut mpsc::UnboundedReceiver<Cmd>) -> Vec<InputEvent> {
        let mut out = vec![];
        while let Ok(Cmd::Msg(ViewerMsg::Input { ev })) = rx.try_recv() {
            out.push(ev);
        }
        out
    }

    fn key(code: &str, down: bool) -> InputEvent {
        InputEvent::Key { code: code.into(), down }
    }

    #[test]
    fn text_goes_straight_through_when_supported() {
        let (v, mut rx) = view(true);
        v.type_text("你好 ok".into(), vec![]);
        assert_eq!(events(&mut rx), vec![InputEvent::Text { text: "你好 ok".into() }]);
    }

    #[test]
    fn old_host_gets_key_strokes() {
        let (v, mut rx) = view(false);
        v.type_text("a!中".into(), vec![]);
        assert_eq!(
            events(&mut rx),
            vec![
                key("KeyA", true),
                key("KeyA", false),
                key("ShiftLeft", true),
                key("Digit1", true),
                key("Digit1", false),
                key("ShiftLeft", false),
            ]
        );
    }

    #[test]
    fn shortcut_and_newline_use_keys() {
        let (v, mut rx) = view(true);
        v.type_text("c".into(), vec!["MetaLeft".into()]);
        assert_eq!(events(&mut rx), vec![key("MetaLeft", true), key("KeyC", true), key("KeyC", false), key("MetaLeft", false)]);
        v.type_text("好\n".into(), vec![]);
        assert_eq!(events(&mut rx), vec![InputEvent::Text { text: "好".into() }, key("Enter", true), key("Enter", false)]);
        v.tap_key("Tab".into(), vec!["AltLeft".into()]);
        assert_eq!(events(&mut rx), vec![key("AltLeft", true), key("Tab", true), key("Tab", false), key("AltLeft", false)]);
    }
}
