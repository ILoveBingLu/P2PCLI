//! 远程画面：看对方电脑的屏幕，也可以用键盘鼠标操作它。终端是主体，这个是附带的功能。
//!
//! 画面只走直连（IPv6、IPv4、局域网）。连接落到中继上时被连方暂停发画面，直连恢复后自动接着发；
//! 中继只给终端这种小流量用，视频不能占它。
//!
//! 抓屏、编码、注入键鼠全在被连方用系统的硬件接口做（macOS 是 ScreenCaptureKit + VideoToolbox，
//! Windows 是 Windows.Graphics.Capture + 显卡的 H.264 编码器），这里只定协议和收发节奏，
//! 具体实现由桌面端通过 [`ScreenBackend`] 接进来。编码统一成 H.264（AVCC 格式，带 avcC 描述），
//! 观看方用 WebCodecs 硬件解码。
//!
//! 流上的帧：1 字节类型 + 4 字节长度（大端）+ 内容。类型 1 是 JSON 控制消息，类型 2 是一帧画面：
//! 8 字节序号 + 8 字节时间戳（微秒）+ 1 字节标志（第 0 位是关键帧）+ H.264 数据。
//!
//! 节奏：观看方每收到一帧就回一个确认。被连方只在“最早没确认的那帧排队不超过一点点”时才去要下一帧，
//! 等的这段时间抓到的画面直接被新的覆盖，编码器根本不碰，所以不会越积越卡。码率按确认回来的延迟自动调。

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use bytes::Bytes;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Notify, mpsc};
use tracing::{debug, info, warn};

use crate::path::{self, PathKind};

/// 能力名：两边都有才显示远程画面的按钮
pub const SCREEN_V1: &str = "screen_v1";

const KIND_MSG: u8 = 1;
const KIND_VIDEO: u8 = 2;
/// 一帧画面最大多少字节。4K 关键帧在高码率下也就几 MB
const MAX_SCREEN_FRAME: usize = 16 << 20;
/// 视频流的优先级比终端低：同一条连接上终端的字先走
pub const VIDEO_PRIORITY: i32 = -10;

/// 最早没确认的帧排队超过这么久就不再要新帧（在最小延迟之上）。
/// 笔记本 Wi-Fi 省电时确认会周期性晚到 100 多毫秒，上限要比这个抖动宽
const MAX_QUEUE: Duration = Duration::from_millis(200);
/// 一段时间里连最快的确认都排队超过这么久，才算真的堵了
const CONGESTED_QUEUE: Duration = Duration::from_millis(60);
/// 一段时间里至少有这么多次确认，才按最快的那次判断
const MIN_SAMPLES: usize = 3;
/// 多久读一次指针：60 帧每秒
const CURSOR_POLL: Duration = Duration::from_millis(16);
/// 最少隔这么久降一次码率
const DECREASE_GAP: Duration = Duration::from_millis(600);
/// 这么久没堵才往上加码率
const INCREASE_AFTER: Duration = Duration::from_secs(2);
const MIN_BITRATE: u32 = 300_000;

/// 画质档位。长边上限、帧率、码率上限
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    /// 清晰：原始分辨率（长边最多 3840，H.264 的上限附近），适合看小字
    Sharp,
    /// 均衡：长边最多 1920
    #[default]
    Balanced,
    /// 流畅：长边最多 1280、30 帧，网差时用
    Smooth,
}

impl Quality {
    pub fn max_long_edge(self) -> u32 {
        match self {
            Quality::Sharp => 3840,
            Quality::Balanced => 1920,
            Quality::Smooth => 1280,
        }
    }
    pub fn fps(self) -> u32 {
        match self {
            Quality::Sharp | Quality::Balanced => 60,
            Quality::Smooth => 30,
        }
    }
    pub fn max_bitrate(self) -> u32 {
        match self {
            Quality::Sharp => 40_000_000,
            Quality::Balanced => 16_000_000,
            Quality::Smooth => 4_000_000,
        }
    }
    pub fn start_bitrate(self) -> u32 {
        (self.max_bitrate() / 3).clamp(MIN_BITRATE, 8_000_000)
    }
}

/// 被连方的一块屏幕
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DisplayInfo {
    /// 系统里的编号，转成字符串
    pub id: String,
    pub name: String,
    /// 像素尺寸
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

/// 解码器配置：WebCodecs 的 VideoDecoderConfig 直接能用
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VideoConfig {
    /// 比如 avc1.640033
    pub codec: String,
    pub width: u32,
    pub height: u32,
    /// avcC（AVCDecoderConfigurationRecord），base64
    pub description: String,
}

/// 编码好的一帧
#[derive(Debug, Clone)]
pub struct EncodedFrame {
    pub key: bool,
    /// 抓到这帧时的时间，微秒，只用来排查
    pub ts_us: u64,
    /// H.264，AVCC 格式（每个 NAL 前面 4 字节长度）
    pub data: Vec<u8>,
    /// 关键帧带上；尺寸或参数变了观看方要重新配置解码器
    pub config: Option<VideoConfig>,
}

/// 开始抓屏的参数
#[derive(Debug, Clone)]
pub struct CaptureOptions {
    pub display: String,
    pub max_long_edge: u32,
    pub fps: u32,
    pub bitrate: u32,
    /// 把鼠标指针画进画面。能单独读指针时为假：指针由观看方用本机的系统指针画，零延迟、跟手
    pub show_cursor: bool,
}

/// 鼠标指针的样子：RGBA（不预乘透明度），观看方拿它当本机指针的图案
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CursorShape {
    /// 图案变了编号就变
    pub id: u64,
    pub width: u32,
    pub height: u32,
    /// 热点（点击的那个点），按图片像素算
    pub hot_x: f64,
    pub hot_y: f64,
    /// 图片像素和屏幕上的点的比例：Retina 是 2
    pub scale: f64,
    /// base64 的 RGBA
    pub rgba: String,
}

/// 指针现在在哪、什么样
#[derive(Debug, Clone)]
pub struct CursorInfo {
    /// 相对当前屏幕的 0~1
    pub x: f64,
    pub y: f64,
    /// 在这块屏幕上、没被藏起来
    pub visible: bool,
    pub shape: Option<Arc<CursorShape>>,
}

/// 平台实现：桌面端在 macOS、Windows 上各接一个
pub trait ScreenBackend: Send + Sync + 'static {
    fn displays(&self) -> Result<Vec<DisplayInfo>>;
    /// 能不能抓屏；不能时返回给对方看的原因（比如没给屏幕录制权限），同时在本机弹系统的授权提示
    fn check_capture(&self) -> Result<()>;
    /// 能不能注入键盘鼠标；不能时返回原因（比如没给辅助功能权限）
    fn check_control(&self) -> Result<()>;
    fn start(&self, opts: CaptureOptions) -> Result<Arc<dyn ScreenCapture>>;
    /// 注入一个键鼠事件，坐标是相对 display 的 0~1
    fn inject(&self, display: &str, ev: &InputEvent) -> Result<()>;
    /// 单独读鼠标指针的位置和样子。读不了就是空，那时指针画在画面里
    fn cursor(&self, _display: &str) -> Option<CursorInfo> {
        None
    }
}

/// 一路正在抓的画面。丢掉就停止
pub trait ScreenCapture: Send + Sync {
    /// 等下一帧画面（上次取走以后屏幕有变化才会有），编码好返回。同一时间只会有一个调用
    fn next_frame(&self) -> crate::host::BoxFuture<Result<EncodedFrame>>;
    fn request_keyframe(&self);
    fn set_bitrate(&self, bps: u32);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

/// 键鼠事件。坐标是相对当前屏幕的 0~1；键是浏览器 KeyboardEvent.code（按物理位置，不管输入法和布局）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum InputEvent {
    Move { x: f64, y: f64 },
    Button { button: MouseButton, down: bool, x: f64, y: f64 },
    /// 滚轮，像素；dy 正数往下
    Wheel { dx: f64, dy: f64, x: f64, y: f64 },
    Key { code: String, down: bool },
    /// 窗口失焦时把按着的键和鼠标都松开，免得对面一直按着 Shift
    ReleaseAll,
}

/// 开始看画面的请求
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScreenOpen {
    /// 不写就是主屏
    #[serde(default)]
    pub display: Option<String>,
    #[serde(default)]
    pub quality: Quality,
    /// 想不想操作；被连方可能只给看
    #[serde(default)]
    pub control: bool,
}

/// 被连方同意以后回的第一条消息
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScreenStarted {
    pub displays: Vec<DisplayInfo>,
    pub display: String,
    /// 能不能操作：对方只给看、或者本机没给辅助功能权限时为假
    pub control: bool,
    /// 不能操作的原因
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_note: Option<String>,
}

/// 观看方发给被连方
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ViewerMsg {
    /// 收到了这一帧（和它之前的所有帧）
    Ack { seq: u64 },
    /// 解码出错或刚重新配置，要一个关键帧
    KeyFrame,
    Switch { display: String },
    SetQuality { quality: Quality },
    Input { ev: InputEvent },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    /// 连接走了中继：画面不走中继
    Relay,
    /// 还没确定走哪条路
    Connecting,
}

/// 被连方发给观看方的控制消息
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum HostMsg {
    Paused { reason: PauseReason },
    Resumed,
    /// 换了屏幕
    Switched { display: String },
    /// 每秒一次：当前码率、实际帧率
    Stats { bitrate: u32, fps: u32 },
    /// 指针挪了：相对当前屏幕的 0~1
    Cursor { x: f64, y: f64, visible: bool },
    /// 指针换样子了（箭头、文本光标、手形……）
    CursorShape { shape: CursorShape },
    /// 出错了，画面结束
    Error { message: String },
}

/// 观看方收到的东西
#[derive(Debug, Clone)]
pub enum ScreenEvent {
    Host(HostMsg),
    Config(VideoConfig),
    Frame { seq: u64, ts_us: u64, key: bool, data: Bytes },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum Wire {
    Host { msg: HostMsg },
    Config { config: VideoConfig },
}

async fn write_raw<W: AsyncWrite + Unpin>(w: &mut W, kind: u8, parts: &[&[u8]]) -> Result<()> {
    let len: usize = parts.iter().map(|p| p.len()).sum();
    if len > MAX_SCREEN_FRAME {
        bail!("画面帧太大：{len} 字节");
    }
    let mut buf = Vec::with_capacity(5 + len);
    buf.push(kind);
    buf.extend_from_slice(&(len as u32).to_be_bytes());
    for p in parts {
        buf.extend_from_slice(p);
    }
    w.write_all(&buf).await?;
    Ok(())
}

async fn write_json<W: AsyncWrite + Unpin, T: Serialize>(w: &mut W, v: &T) -> Result<()> {
    write_raw(w, KIND_MSG, &[&serde_json::to_vec(v)?]).await
}

async fn read_raw<R: AsyncRead + Unpin>(r: &mut R) -> Result<Option<(u8, Vec<u8>)>> {
    let mut head = [0u8; 5];
    match r.read_exact(&mut head).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_be_bytes(head[1..].try_into().unwrap()) as usize;
    if len > MAX_SCREEN_FRAME {
        bail!("画面帧太大：{len} 字节");
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await.context("画面帧没读完连接就断了")?;
    Ok(Some((head[0], body)))
}

async fn read_json<R: AsyncRead + Unpin, T: DeserializeOwned>(r: &mut R) -> Result<Option<T>> {
    match read_raw(r).await? {
        Some((KIND_MSG, body)) => Ok(Some(serde_json::from_slice(&body)?)),
        Some((k, _)) => bail!("这里应该是控制消息，收到类型 {k}"),
        None => Ok(None),
    }
}

/// 画面能不能走这条路：只认直连
fn direct(kind: PathKind) -> Result<(), PauseReason> {
    match kind {
        PathKind::Ipv4 | PathKind::Ipv6 => Ok(()),
        PathKind::Relay => Err(PauseReason::Relay),
        PathKind::Local | PathKind::Unknown => Err(PauseReason::Connecting),
    }
}

/// 观看方先查一下：走中继就不开
pub fn check_path(info: &path::PathInfo) -> Result<()> {
    match direct(info.kind) {
        Err(PauseReason::Relay) => bail!("relay: 现在这条连接走的是中继，远程画面只走直连。终端照常能用；直连接通后再打开画面"),
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------- 被连方

/// 一路画面在被连方的样子，给界面显示“谁在看本机画面”、随时断开
#[derive(Debug, Clone, Serialize)]
pub struct ScreenViewerSummary {
    pub id: u64,
    pub session: u64,
    pub remote: String,
    pub name: String,
    pub control: bool,
    pub since: u64,
}

/// 码率控制：按确认回来的延迟估排队，排队多了降，长时间不堵慢慢往上加。
///
/// 判断堵没堵看一段时间里排队最少的那次确认：真堵的时候队列压在每个包上，连最快的也慢；
/// Wi-Fi 省电、系统调度这类抖动只让一部分确认晚到，最快的那次照样快
struct RateControl {
    bitrate: u32,
    max: u32,
    min_delay: Duration,
    min_delay_at: Instant,
    last_decrease: Instant,
    last_congested: Instant,
    /// 这一段里排队最少的一次，和确认了几次
    best_queue: Option<Duration>,
    samples: usize,
}

impl RateControl {
    fn new(quality: Quality, rtt: Duration) -> Self {
        let now = Instant::now();
        RateControl {
            bitrate: quality.start_bitrate(),
            max: quality.max_bitrate(),
            min_delay: rtt,
            min_delay_at: now,
            last_decrease: now - DECREASE_GAP,
            last_congested: now,
            best_queue: None,
            samples: 0,
        }
    }

    fn set_quality(&mut self, quality: Quality) {
        self.max = quality.max_bitrate();
        self.bitrate = self.bitrate.min(self.max);
    }

    /// 一帧被确认了：从发出到确认用了 delay，帧有 bytes 大
    fn on_ack(&mut self, delay: Duration, bytes: usize) {
        // 一帧本身要占的传输时间不算排队：按当前码率估
        let send_time = Duration::from_secs_f64(bytes as f64 * 8.0 / self.bitrate.max(MIN_BITRATE) as f64);
        let base = delay.saturating_sub(send_time.min(delay / 2));
        // 最小延迟 10 秒内没刷新就慢慢放宽，网络换路以后能跟上
        if base < self.min_delay || self.min_delay_at.elapsed() > Duration::from_secs(10) {
            self.min_delay = if base < self.min_delay { base } else { (self.min_delay + base) / 2 };
            self.min_delay_at = Instant::now();
        }
        let queue = base.saturating_sub(self.min_delay);
        self.best_queue = Some(self.best_queue.map_or(queue, |q| q.min(queue)));
        self.samples += 1;
    }

    /// 最早没确认的帧发出去这么久了：已经超过最小延迟加上排队上限就别再要新帧
    fn may_send(&self, oldest_age: Option<Duration>) -> bool {
        oldest_age.is_none_or(|age| age < self.min_delay + MAX_QUEUE)
    }

    /// 定时调；返回新码率（有变化时）
    fn tick(&mut self, oldest_age: Option<Duration>) -> Option<u32> {
        let stalled = oldest_age.is_some_and(|age| age > self.min_delay + MAX_QUEUE * 2);
        let queue = self.best_queue.take().unwrap_or_default();
        let samples = std::mem::take(&mut self.samples);
        // 确认太少（屏幕几乎不动）时，要排得更久才算堵
        let congested = if samples >= MIN_SAMPLES { queue > CONGESTED_QUEUE } else { queue > CONGESTED_QUEUE * 3 };
        tracing::trace!(?queue, samples, stalled, ?oldest_age, min_delay = ?self.min_delay, bitrate = self.bitrate, "码率控制");
        let now = Instant::now();
        if congested || stalled {
            self.last_congested = now;
            if now - self.last_decrease >= DECREASE_GAP {
                self.last_decrease = now;
                let next = ((self.bitrate as f64) * 0.75) as u32;
                let next = next.max(MIN_BITRATE);
                if next != self.bitrate {
                    self.bitrate = next;
                    return Some(next);
                }
            }
            return None;
        }
        if now - self.last_congested >= INCREASE_AFTER && self.bitrate < self.max {
            // 每个周期加 8%，从最低涨到最高大概要半分钟
            self.bitrate = (((self.bitrate as f64) * 1.08) as u32).min(self.max);
            self.last_congested = now - INCREASE_AFTER + Duration::from_millis(500);
            return Some(self.bitrate);
        }
        None
    }
}

struct Inflight {
    seq: u64,
    sent: Instant,
    bytes: usize,
}

/// 抓屏任务：主循环要一帧就给它发个信号，它取一帧编码好送回来。
/// 单独跑是因为编码时不能被打断（打断了丢的是编码好的帧，后面的帧就解不出来了）
fn spawn_capture(capture: Arc<dyn ScreenCapture>, frames: mpsc::Sender<Result<EncodedFrame>>) -> (mpsc::Sender<()>, tokio::task::JoinHandle<()>) {
    let (demand_tx, mut demand_rx) = mpsc::channel::<()>(1);
    let task = tokio::spawn(async move {
        while demand_rx.recv().await.is_some() {
            let frame = capture.next_frame().await;
            let failed = frame.is_err();
            if frames.send(frame).await.is_err() || failed {
                break;
            }
        }
    });
    (demand_tx, task)
}

struct Running {
    capture: Arc<dyn ScreenCapture>,
    demand: mpsc::Sender<()>,
    task: tokio::task::JoinHandle<()>,
    frames: mpsc::Receiver<Result<EncodedFrame>>,
    /// 已经要了一帧，还没拿到
    waiting: bool,
}

impl Running {
    /// 开抓屏要等系统回应（最多几秒），放到阻塞线程上，不占异步运行时
    async fn start(backend: &Arc<dyn ScreenBackend>, display: &str, quality: Quality, bitrate: u32, show_cursor: bool) -> Result<Self> {
        let opts = CaptureOptions {
            display: display.to_string(),
            max_long_edge: quality.max_long_edge(),
            fps: quality.fps(),
            bitrate,
            show_cursor,
        };
        let backend = backend.clone();
        let capture = tokio::task::spawn_blocking(move || backend.start(opts)).await.map_err(|e| anyhow!("抓屏线程出错：{e}"))??;
        let (tx, frames) = mpsc::channel(1);
        let (demand, task) = spawn_capture(capture.clone(), tx);
        Ok(Running { capture, demand, task, frames, waiting: false })
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// 被连方：把画面发给观看方，收它的确认和键鼠事件。返回时画面结束。
/// `control` 为假时键鼠事件一律丢掉。`stop` 被通知就结束（本机的人点了断开）
#[allow(clippy::too_many_arguments)]
pub async fn serve<S, R>(
    conn: iroh::endpoint::Connection,
    backend: Arc<dyn ScreenBackend>,
    open: ScreenOpen,
    control: bool,
    control_note: Option<String>,
    mut send: S,
    mut recv: R,
    stop: Arc<Notify>,
) -> Result<()>
where
    S: AsyncWrite + Unpin + Send + 'static,
    R: AsyncRead + Unpin + Send + 'static,
{
    let displays = backend.displays()?;
    let mut display = open
        .display
        .as_ref()
        .and_then(|want| displays.iter().find(|d| &d.id == want))
        .or_else(|| displays.iter().find(|d| d.primary))
        .or(displays.first())
        .map(|d| d.id.clone())
        .ok_or_else(|| anyhow!("本机没有可以抓的屏幕"))?;
    let mut quality = open.quality;

    // 先把抓屏开起来，开不起来就按普通错误回给观看方，让它看到原因
    let rtt = Duration::from_millis(path::current(&conn).rtt_ms.unwrap_or(50));
    let mut rate = RateControl::new(quality, rtt);
    // 能单独读指针就不把指针画进画面：观看方用本机的系统指针画，跟手
    let separate_cursor = {
        let (backend, display) = (backend.clone(), display.clone());
        tokio::task::spawn_blocking(move || backend.cursor(&display).is_some()).await.unwrap_or(false)
    };
    let mut running = match Running::start(&backend, &display, quality, rate.bitrate, !separate_cursor).await {
        Ok(r) => r,
        Err(e) => {
            let _ = crate::proto::write_msg(&mut send, &crate::proto::Msg::Error { message: format!("{e:#}") }).await;
            let _ = send.shutdown().await;
            return Err(e);
        }
    };

    // 第一条用普通的控制消息格式回，观看方的 open_stream 按普通消息读
    crate::proto::write_msg(
        &mut send,
        &crate::proto::Msg::ScreenStarted {
            started: ScreenStarted { displays: displays.clone(), display: display.clone(), control, control_note },
        },
    )
    .await?;

    // 收：确认和换屏、换画质交给主循环；键鼠事件直接注入，不排在编码后面
    let (msg_tx, mut msg_rx) = mpsc::unbounded_channel::<ViewerMsg>();
    let current_display = Arc::new(std::sync::Mutex::new(display.clone()));
    let reader = {
        let backend = backend.clone();
        let current_display = current_display.clone();
        tokio::spawn(async move {
            loop {
                match read_json::<_, ViewerMsg>(&mut recv).await {
                    Ok(Some(ViewerMsg::Input { ev })) => {
                        if !control {
                            continue;
                        }
                        let display = current_display.lock().unwrap().clone();
                        if let Err(e) = backend.inject(&display, &ev) {
                            debug!("注入键鼠事件失败：{e:#}");
                        }
                    }
                    Ok(Some(msg)) => {
                        if msg_tx.send(msg).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        debug!("读观看方消息失败：{e:#}");
                        break;
                    }
                }
            }
            // 观看方走了：把按着的键松开
            if control {
                let display = current_display.lock().unwrap().clone();
                let _ = backend.inject(&display, &InputEvent::ReleaseAll);
            }
        })
    };

    // 发：单独的任务，写满时只卡它自己
    // 写之前再看一眼线路：已经排着的画面在切到中继以后也不发（恢复直连时主循环会从关键帧重新开始）
    let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(4);
    let writer_conn = conn.clone();
    let writer = tokio::spawn(async move {
        while let Some(buf) = out_rx.recv().await {
            if buf.first() == Some(&KIND_VIDEO) && direct(path::current(&writer_conn).kind).is_err() {
                continue;
            }
            if send.write_all(&buf).await.is_err() {
                return;
            }
        }
        let _ = send.shutdown().await;
    });
    // 指针：单独一个线程每 16 毫秒读一次（macOS 读指针样子要碰 AppKit，可能会阻塞一下），有变化才报
    let (cursor_tx, mut cursor_rx) = mpsc::channel::<CursorInfo>(4);
    if separate_cursor {
        let backend = backend.clone();
        let current_display = current_display.clone();
        std::thread::Builder::new()
            .name("p2pcli-cursor".into())
            .spawn(move || {
                let mut last: Option<(i64, i64, bool, u64)> = None;
                while !cursor_tx.is_closed() {
                    let display = current_display.lock().unwrap().clone();
                    if let Some(c) = backend.cursor(&display) {
                        // 位置按万分之一比较，抖一点点不报
                        let key = ((c.x * 10000.0) as i64, (c.y * 10000.0) as i64, c.visible, c.shape.as_ref().map_or(0, |s| s.id));
                        if last != Some(key) {
                            last = Some(key);
                            if cursor_tx.blocking_send(c).is_err() {
                                break;
                            }
                        }
                    }
                    std::thread::sleep(CURSOR_POLL);
                }
            })?;
    } else {
        drop(cursor_tx);
    }
    let mut last_shape: Option<u64> = None;
    let mut inflight: VecDeque<Inflight> = VecDeque::new();
    let mut seq: u64 = 0;
    let mut paused: Option<PauseReason> = None;
    let mut need_key = true;
    let mut last_config: Option<VideoConfig> = None;
    let mut frames_this_second = 0u32;
    let mut last_stats = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let encode = |kind: u8, parts: &[&[u8]]| -> Vec<u8> {
        let len: usize = parts.iter().map(|p| p.len()).sum();
        let mut buf = Vec::with_capacity(5 + len);
        buf.push(kind);
        buf.extend_from_slice(&(len as u32).to_be_bytes());
        for p in parts {
            buf.extend_from_slice(p);
        }
        buf
    };
    let json = |w: &Wire| encode(KIND_MSG, &[&serde_json::to_vec(w).unwrap_or_default()]);

    // 一开始就看一眼路
    if let Err(reason) = direct(path::current(&conn).kind) {
        paused = Some(reason);
        let _ = out_tx.send(json(&Wire::Host { msg: HostMsg::Paused { reason } })).await;
    }

    let result: Result<()> = loop {
        // 能发就要下一帧
        let oldest = inflight.front().map(|f| f.sent.elapsed());
        if paused.is_none() && !running.waiting && rate.may_send(oldest) {
            if need_key {
                running.capture.request_keyframe();
                need_key = false;
            }
            if running.demand.try_send(()).is_ok() {
                running.waiting = true;
            }
        }
        tokio::select! {
            frame = running.frames.recv() => {
                running.waiting = false;
                let frame = match frame {
                    Some(Ok(f)) => f,
                    Some(Err(e)) => break Err(e.context("抓屏或编码失败")),
                    None => break Err(anyhow!("抓屏停止了")),
                };
                if paused.is_some() {
                    // 暂停期间编出来的帧丢掉，恢复时从关键帧开始
                    need_key = true;
                    continue;
                }
                if let Some(config) = &frame.config
                    && last_config.as_ref() != Some(config)
                {
                    last_config = Some(config.clone());
                    if out_tx.send(json(&Wire::Config { config: config.clone() })).await.is_err() { break Ok(()); }
                }
                if last_config.is_none() {
                    // 还没有解码配置，这帧没法解，要关键帧
                    need_key = true;
                    continue;
                }
                seq += 1;
                let head_seq = seq.to_be_bytes();
                let ts = frame.ts_us.to_be_bytes();
                let flags = [u8::from(frame.key)];
                let buf = encode(KIND_VIDEO, &[&head_seq, &ts, &flags, &frame.data]);
                let bytes = buf.len();
                if out_tx.send(buf).await.is_err() { break Ok(()); }
                inflight.push_back(Inflight { seq, sent: Instant::now(), bytes });
                frames_this_second += 1;
            }
            msg = msg_rx.recv() => match msg {
                Some(ViewerMsg::Ack { seq: acked }) => {
                    while let Some(front) = inflight.front() {
                        if front.seq > acked { break; }
                        let f = inflight.pop_front().unwrap();
                        rate.on_ack(f.sent.elapsed(), f.bytes);
                    }
                }
                // 观看方解码出错或刚重建解码器：关键帧前面带上解码配置，让它重新配一遍
                Some(ViewerMsg::KeyFrame) => {
                    need_key = true;
                    last_config = None;
                }
                Some(ViewerMsg::Switch { display: want }) => {
                    if want == display { continue; }
                    if !backend.displays().map(|ds| ds.iter().any(|d| d.id == want)).unwrap_or(false) {
                        // 没有这块屏幕（可能刚拔掉）：告诉观看方还在看原来那块
                        let _ = out_tx.send(json(&Wire::Host { msg: HostMsg::Switched { display: display.clone() } })).await;
                        continue;
                    }
                    drop(std::mem::replace(&mut running, match Running::start(&backend, &want, quality, rate.bitrate, !separate_cursor).await {
                        Ok(r) => r,
                        Err(e) => break Err(e),
                    }));
                    display = want;
                    *current_display.lock().unwrap() = display.clone();
                    inflight.clear();
                    need_key = true;
                    last_config = None;
                    let _ = out_tx.send(json(&Wire::Host { msg: HostMsg::Switched { display: display.clone() } })).await;
                }
                Some(ViewerMsg::SetQuality { quality: q }) => {
                    if q == quality { continue; }
                    quality = q;
                    rate.set_quality(q);
                    drop(std::mem::replace(&mut running, match Running::start(&backend, &display, quality, rate.bitrate, !separate_cursor).await {
                        Ok(r) => r,
                        Err(e) => break Err(e),
                    }));
                    inflight.clear();
                    need_key = true;
                    last_config = None;
                }
                Some(ViewerMsg::Input { .. }) => {}
                None => break Ok(()),
            },
            _ = tick.tick() => {
                // 路：落到中继就暂停
                let now_path = direct(path::current(&conn).kind).err();
                if now_path != paused {
                    match now_path {
                        Some(reason) => {
                            info!(?reason, "远程画面暂停：现在不是直连");
                            let _ = out_tx.send(json(&Wire::Host { msg: HostMsg::Paused { reason } })).await;
                        }
                        None => {
                            info!("远程画面恢复：直连已接通");
                            need_key = true;
                            inflight.clear();
                            let _ = out_tx.send(json(&Wire::Host { msg: HostMsg::Resumed })).await;
                        }
                    }
                    paused = now_path;
                }
                if let Some(b) = rate.tick(inflight.front().map(|f| f.sent.elapsed())) {
                    debug!(bitrate = b, "远程画面调码率");
                    running.capture.set_bitrate(b);
                }
                if last_stats.elapsed() >= Duration::from_secs(1) {
                    let secs = last_stats.elapsed().as_secs_f64();
                    let fps = (frames_this_second as f64 / secs).round() as u32;
                    frames_this_second = 0;
                    last_stats = Instant::now();
                    let _ = out_tx.try_send(json(&Wire::Host { msg: HostMsg::Stats { bitrate: rate.bitrate, fps } }));
                }
            }
            Some(c) = cursor_rx.recv() => {
                // 暂停时画面整个不发，指针也不发
                if paused.is_some() { continue; }
                if let Some(shape) = &c.shape
                    && last_shape != Some(shape.id)
                {
                    last_shape = Some(shape.id);
                    if out_tx.send(json(&Wire::Host { msg: HostMsg::CursorShape { shape: (**shape).clone() } })).await.is_err() { break Ok(()); }
                }
                // 位置发不出去就丢掉，反正下一次的更新
                let _ = out_tx.try_send(json(&Wire::Host { msg: HostMsg::Cursor { x: c.x, y: c.y, visible: c.visible } }));
            }
            _ = stop.notified() => break Ok(()),
            _ = conn.closed() => break Ok(()),
        }
    };

    if let Err(e) = &result {
        warn!("远程画面出错：{e:#}");
        let _ = out_tx.send(json(&Wire::Host { msg: HostMsg::Error { message: format!("{e:#}") } })).await;
    }
    drop(running);
    drop(out_tx);
    let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
    reader.abort();
    if control {
        let _ = backend.inject(&display, &InputEvent::ReleaseAll);
    }
    Ok(())
}

// ---------------------------------------------------------------- 观看方

/// 观看方发消息的一头
pub struct ScreenInput<S> {
    send: S,
}

impl<S: AsyncWrite + Unpin> ScreenInput<S> {
    pub fn new(send: S) -> Self {
        ScreenInput { send }
    }
    pub async fn send(&mut self, msg: &ViewerMsg) -> Result<()> {
        write_json(&mut self.send, msg).await
    }
    pub async fn close(&mut self) {
        let _ = self.send.shutdown().await;
    }
}

/// 观看方收画面的一头
pub struct ScreenOutput<R> {
    recv: R,
}

impl<R: AsyncRead + Unpin> ScreenOutput<R> {
    pub fn new(recv: R) -> Self {
        ScreenOutput { recv }
    }

    pub async fn next(&mut self) -> Result<Option<ScreenEvent>> {
        let Some((kind, body)) = read_raw(&mut self.recv).await? else { return Ok(None) };
        match kind {
            KIND_MSG => Ok(Some(match serde_json::from_slice::<Wire>(&body)? {
                Wire::Host { msg } => ScreenEvent::Host(msg),
                Wire::Config { config } => ScreenEvent::Config(config),
            })),
            KIND_VIDEO => {
                if body.len() < 17 {
                    bail!("画面帧太短");
                }
                let seq = u64::from_be_bytes(body[0..8].try_into().unwrap());
                let ts_us = u64::from_be_bytes(body[8..16].try_into().unwrap());
                let key = body[16] & 1 == 1;
                let data = Bytes::from(body).slice(17..);
                Ok(Some(ScreenEvent::Frame { seq, ts_us, key, data }))
            }
            k => bail!("未知画面帧类型 {k}"),
        }
    }
}

// ---------------------------------------------------------------- H.264 小工具（各平台的编码器都用）

/// 从 SPS、PPS 拼出 avcC（AVCDecoderConfigurationRecord）和 WebCodecs 的 codec 字符串
pub fn avcc_config(sps: &[u8], pps: &[u8], width: u32, height: u32) -> Result<VideoConfig> {
    use base64::Engine as _;
    if sps.len() < 4 {
        bail!("SPS 太短");
    }
    let (profile, compat, level) = (sps[1], sps[2], sps[3]);
    let mut rec = vec![1, profile, compat, level, 0xFF, 0xE1];
    rec.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    rec.extend_from_slice(sps);
    rec.push(1);
    rec.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    rec.extend_from_slice(pps);
    // High 及以上的 profile 按规范要带 chroma_format 等字段；4:2:0、8 位
    if matches!(profile, 100 | 110 | 122 | 144) {
        rec.extend_from_slice(&[0xFC | 1, 0xF8, 0xF8, 0]);
    }
    Ok(VideoConfig {
        codec: format!("avc1.{profile:02x}{compat:02x}{level:02x}"),
        width,
        height,
        description: base64::engine::general_purpose::STANDARD.encode(rec),
    })
}

/// [`annexb_to_avcc`] 的结果
#[derive(Debug, Default)]
pub struct Avcc {
    /// AVCC 格式的帧数据，不含 SPS、PPS
    pub data: Vec<u8>,
    pub sps: Option<Vec<u8>>,
    pub pps: Option<Vec<u8>>,
    /// 含 IDR，是关键帧
    pub key: bool,
}

/// Annex B（00 00 00 01 分隔）转成 AVCC（4 字节长度），顺便把 SPS、PPS 挑出来。
/// Windows 的编码器吐 Annex B，WebCodecs 配了 description 就要 AVCC
pub fn annexb_to_avcc(data: &[u8]) -> Avcc {
    let mut out = Vec::with_capacity(data.len() + 16);
    let (mut sps, mut pps, mut key) = (None, None, false);
    for nal in split_annexb(data) {
        if nal.is_empty() {
            continue;
        }
        match nal[0] & 0x1F {
            7 => { sps = Some(nal.to_vec()); continue; }
            8 => { pps = Some(nal.to_vec()); continue; }
            // 访问单元分隔符，WebCodecs 不需要
            9 => continue,
            5 => key = true,
            _ => {}
        }
        out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        out.extend_from_slice(nal);
    }
    Avcc { data: out, sps, pps, key }
}

fn split_annexb(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push((i, i + 3));
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut nals = Vec::with_capacity(starts.len());
    for (n, &(_, body)) in starts.iter().enumerate() {
        let mut end = starts.get(n + 1).map(|&(s, _)| s).unwrap_or(data.len());
        // 4 字节起始码前面多一个 0，属于下一个起始码
        while end > body && data[end - 1] == 0 && n + 1 < starts.len() {
            end -= 1;
        }
        nals.push(&data[body..end]);
    }
    nals
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annexb_round_trip() {
        let sps = [0x67, 0x64, 0x00, 0x28, 0xAC];
        let pps = [0x68, 0xEE, 0x3C, 0x80];
        let idr = [0x65, 0x88, 0x84, 0x00, 0x00, 0x03, 0x01];
        let mut data = vec![0, 0, 0, 1, 9, 0x10];
        data.extend_from_slice(&[0, 0, 0, 1]);
        data.extend_from_slice(&sps);
        data.extend_from_slice(&[0, 0, 1]);
        data.extend_from_slice(&pps);
        data.extend_from_slice(&[0, 0, 0, 1]);
        data.extend_from_slice(&idr);
        let Avcc { data: avcc, sps: s, pps: p, key } = annexb_to_avcc(&data);
        assert!(key);
        assert_eq!(s.as_deref(), Some(&sps[..]));
        assert_eq!(p.as_deref(), Some(&pps[..]));
        let mut want = (idr.len() as u32).to_be_bytes().to_vec();
        want.extend_from_slice(&idr);
        assert_eq!(avcc, want);
        let cfg = avcc_config(&sps, &pps, 1920, 1080).unwrap();
        assert_eq!(cfg.codec, "avc1.640028");
    }

    #[test]
    fn only_direct_paths_carry_video() {
        assert!(direct(PathKind::Ipv4).is_ok());
        assert!(direct(PathKind::Ipv6).is_ok());
        assert_eq!(direct(PathKind::Relay), Err(PauseReason::Relay));
        assert!(check_path(&path::PathInfo { kind: PathKind::Relay, rtt_ms: None, remote: None }).is_err());
    }

    #[test]
    fn rate_backs_off_when_queue_grows() {
        let mut rate = RateControl::new(Quality::Balanced, Duration::from_millis(20));
        let start = rate.bitrate;
        rate.on_ack(Duration::from_millis(20), 1000);
        assert!(rate.tick(None).is_none());
        // 这一段里每次确认都排了很久：真的堵了
        for _ in 0..5 {
            rate.on_ack(Duration::from_millis(300), 1000);
        }
        assert!(rate.tick(None).unwrap() < start);
        // 刚降过，不会马上再降
        for _ in 0..5 {
            rate.on_ack(Duration::from_millis(300), 1000);
        }
        assert!(rate.tick(None).is_none());
        // 排队太久就别要新帧
        assert!(!rate.may_send(Some(Duration::from_millis(500))));
        assert!(rate.may_send(Some(Duration::from_millis(30))));
    }

    /// Wi-Fi 省电那种抖动：大部分确认按时到，隔一阵有几个晚到 100 多毫秒，不算堵
    #[test]
    fn jitter_is_not_congestion() {
        let mut rate = RateControl::new(Quality::Balanced, Duration::from_millis(70));
        let start = rate.bitrate;
        for round in 0..20 {
            for i in 0..14 {
                let late = round % 2 == 0 && i < 3;
                rate.on_ack(Duration::from_millis(if late { 190 } else { 72 }), 200);
            }
            let before = rate.bitrate;
            if let Some(b) = rate.tick(Some(Duration::from_millis(70))) {
                assert!(b >= before, "第 {round} 轮被当成堵了");
            }
        }
        assert!(rate.bitrate >= start);
    }

    #[test]
    fn messages_are_tagged() {
        let json = serde_json::to_string(&ViewerMsg::Input { ev: InputEvent::Key { code: "KeyA".into(), down: true } }).unwrap();
        assert_eq!(json, r#"{"t":"input","ev":{"k":"key","code":"KeyA","down":true}}"#);
    }
}
