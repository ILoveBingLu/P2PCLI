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
            Quality::Sharp => 60_000_000,
            Quality::Balanced => 16_000_000,
            Quality::Smooth => 4_000_000,
        }
    }
    /// 一开始用多少码率。清晰档直接拉满，不爬坡；只有真的堵了（确认回来的延迟一直在涨）才往下降。
    /// 别的档从三分之一起步，网络确认没堵再慢慢往上加
    pub fn start_bitrate(self) -> u32 {
        match self {
            Quality::Sharp => self.max_bitrate(),
            _ => (self.max_bitrate() / 3).clamp(MIN_BITRATE, 8_000_000),
        }
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
    /// 想用的编码格式；这台电脑编不了 H.265 时实现自己退回 H.264，观看方按解码配置里的格式解
    pub codec: VideoCodec,
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
    /// 观看方能硬件解码的格式。有 H.265 而被连方也能硬件编码就用 H.265，不然用 H.264；
    /// 老版本不带这项，就是 H.264
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub codecs: Vec<VideoCodec>,
}

/// 视频编码格式。H.265 同样画质码率省三到五成，同样码率更清楚；H.264 哪儿都能用，兜底
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    #[default]
    H264,
    H265,
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
/// Wi-Fi 省电、系统调度这类抖动只让一部分确认晚到，最快的那次照样快。
///
/// 一帧本身的传输时间要从延迟里扣掉，按链路能跑多快估（QUIC 的拥塞窗口 / 往返），不能只按编码码率估：
/// 码率压低以后按码率估的传输时间远比实际长，扣多了“最小延迟”就掉到真实往返的一半，
/// 之后每个小帧都像在排队，码率越降越低，最后贴着下限出不来（跨境 80 ms 的线路上实测过）。
/// 最小延迟也不会低于 QUIC 量到的最小往返
struct RateControl {
    bitrate: u32,
    max: u32,
    min_delay: Duration,
    /// QUIC 量到的最小往返：一帧的确认不可能比这还快
    floor: Duration,
    /// 链路大概能跑多快（拥塞窗口 / 往返），比特每秒；还不知道时为 0
    link_bps: u64,
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
            floor: Duration::ZERO,
            link_bps: 0,
            min_delay_at: now,
            last_decrease: now - DECREASE_GAP,
            last_congested: now,
            best_queue: None,
            samples: 0,
        }
    }

    fn set_quality(&mut self, quality: Quality) {
        self.max = quality.max_bitrate();
        // 切到清晰档马上拉满；切到别的档不超过它的上限
        self.bitrate = if quality == Quality::Sharp { quality.start_bitrate() } else { self.bitrate.min(self.max) };
    }

    /// QUIC 那边的链路情况：最小往返、拥塞窗口能撑起的速度
    fn set_link(&mut self, min_rtt: Duration, link_bps: u64) {
        self.floor = min_rtt;
        self.link_bps = link_bps;
        self.min_delay = self.min_delay.max(min_rtt);
    }

    /// 一帧被确认了：从发出到确认用了 delay，帧有 bytes 大
    fn on_ack(&mut self, delay: Duration, bytes: usize) {
        // 一帧本身要占的传输时间不算排队：按链路速度估，不知道链路速度时按码率估
        let rate = self.link_bps.max(self.bitrate.max(MIN_BITRATE) as u64);
        let send_time = Duration::from_secs_f64(bytes as f64 * 8.0 / rate as f64);
        let base = delay.saturating_sub(send_time.min(delay / 2));
        // 最小延迟 10 秒内没刷新就慢慢放宽，网络换路以后能跟上
        if base < self.min_delay || self.min_delay_at.elapsed() > Duration::from_secs(10) {
            self.min_delay = if base < self.min_delay { base } else { (self.min_delay + base) / 2 };
            self.min_delay_at = Instant::now();
        }
        self.min_delay = self.min_delay.max(self.floor);
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
            // 每个周期加 15%，从最低涨到均衡档的最高大概十几秒
            self.bitrate = (((self.bitrate as f64) * 1.15) as u32).min(self.max);
            self.last_congested = now - INCREASE_AFTER + Duration::from_millis(500);
            return Some(self.bitrate);
        }
        None
    }
}

/// 选中那条路的最小往返，和拥塞窗口按平滑往返折算的速度
fn link_info(conn: &iroh::endpoint::Connection) -> Option<(Duration, u64)> {
    let paths = conn.paths();
    let p = paths.iter().find(|p| p.is_selected())?;
    let srtt = p.rtt();
    let controller = conn.congestion_state(p.id())?;
    let window = controller.window();
    let min_rtt = crate::quic::min_rtt(controller)?;
    if srtt.is_zero() {
        return None;
    }
    Some((min_rtt, (window as f64 * 8.0 / srtt.as_secs_f64()) as u64))
}

struct Inflight {
    seq: u64,
    sent: Instant,
    bytes: usize,
}

/// 被连方的诊断日志隔多久记一行
const HOST_DIAG_EVERY: Duration = Duration::from_secs(5);

/// 被连方的诊断：每 5 秒记一行线路、丢包、拥塞窗口和确认延迟，和观看方记的停顿对着看，
/// 判断卡顿是丢包重传、路径切换还是码率控制造成的
struct HostDiag {
    since: Instant,
    /// 上一次记录时选中路径的累计计数
    sent: u64,
    lost: u64,
    lost_bytes: u64,
    congestion: u64,
    remote: Option<String>,
    frames: u32,
    keys: u32,
    bytes: usize,
    max_frame: usize,
    max_ack: Duration,
    max_oldest: Duration,
    /// 想要新帧但因为排队太久没要的次数（按 250 毫秒一次的检查算）
    blocked: u32,
}

impl HostDiag {
    fn new(conn: &iroh::endpoint::Connection) -> Self {
        let mut d = HostDiag {
            since: Instant::now(),
            sent: 0,
            lost: 0,
            lost_bytes: 0,
            congestion: 0,
            remote: None,
            frames: 0,
            keys: 0,
            bytes: 0,
            max_frame: 0,
            max_ack: Duration::ZERO,
            max_oldest: Duration::ZERO,
            blocked: 0,
        };
        if let Some(p) = conn.paths().iter().find(|p| p.is_selected()) {
            let s = p.stats();
            (d.sent, d.lost, d.lost_bytes, d.congestion) = (s.sent_packets, s.lost_packets, s.lost_bytes, s.congestion_events);
        }
        d.remote = path::current(conn).remote;
        d
    }

    fn frame(&mut self, key: bool, bytes: usize) {
        self.frames += 1;
        self.keys += u32::from(key);
        self.bytes += bytes;
        self.max_frame = self.max_frame.max(bytes);
    }

    fn log(&mut self, conn: &iroh::endpoint::Connection, bitrate: u32, inflight: usize) {
        let secs = self.since.elapsed().as_secs_f64().max(0.001);
        let info = path::current(conn);
        let selected = conn.paths().iter().find(|p| p.is_selected()).map(|p| p.stats());
        let (sent, lost, lost_bytes, congestion, cwnd, mtu, bw) = selected
            .as_ref()
            .map(|s| (s.sent_packets, s.lost_packets, s.lost_bytes, s.congestion_events, s.cwnd, s.current_mtu, s.bandwidth_estimate))
            .unwrap_or_default();
        if info.remote != self.remote {
            // 换了路：计数是按路算的，从新路的累计值重新起算
            info!(from = ?self.remote, to = ?info.remote, kind = ?info.kind, "远程画面诊断：线路切换");
            self.remote = info.remote.clone();
            (self.sent, self.lost, self.lost_bytes, self.congestion) = (sent, lost, lost_bytes, congestion);
        }
        let d_sent = sent.saturating_sub(self.sent);
        let d_lost = lost.saturating_sub(self.lost);
        info!(
            线路 = ?info.kind,
            延时 = ?info.rtt_ms,
            发包 = d_sent,
            丢包 = d_lost,
            丢包率 = format!("{:.2}%", if d_sent > 0 { d_lost as f64 * 100.0 / d_sent as f64 } else { 0.0 }),
            丢字节 = lost_bytes.saturating_sub(self.lost_bytes),
            拥塞事件 = congestion.saturating_sub(self.congestion),
            拥塞窗口 = format!("{}KB", cwnd / 1024),
            mtu,
            带宽估计 = bw.map(|b| format!("{:.2}Mbps", b as f64 / 1e6)),
            发帧率 = format!("{:.0}", self.frames as f64 / secs),
            发码率 = format!("{:.2}Mbps", self.bytes as f64 * 8.0 / secs / 1e6),
            目标码率 = format!("{:.2}Mbps", bitrate as f64 / 1e6),
            关键帧 = self.keys,
            最大帧 = format!("{}KB", self.max_frame / 1024),
            最慢确认 = format!("{}ms", self.max_ack.as_millis()),
            最久未确认 = format!("{}ms", self.max_oldest.as_millis()),
            在途帧 = inflight,
            排队没要帧 = self.blocked,
            "远程画面诊断"
        );
        (self.sent, self.lost, self.lost_bytes, self.congestion) = (sent, lost, lost_bytes, congestion);
        self.since = Instant::now();
        self.frames = 0;
        self.keys = 0;
        self.bytes = 0;
        self.max_frame = 0;
        self.max_ack = Duration::ZERO;
        self.max_oldest = Duration::ZERO;
        self.blocked = 0;
    }
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
    async fn start(backend: &Arc<dyn ScreenBackend>, display: &str, quality: Quality, bitrate: u32, show_cursor: bool, codec: VideoCodec) -> Result<Self> {
        let opts = CaptureOptions {
            display: display.to_string(),
            max_long_edge: quality.max_long_edge(),
            fps: quality.fps(),
            bitrate,
            show_cursor,
            codec,
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
    // 观看方能硬件解码 H.265 就请编码器用 H.265（这台编不了时实现自己退回 H.264）
    let codec = if open.codecs.contains(&VideoCodec::H265) { VideoCodec::H265 } else { VideoCodec::H264 };
    // 能单独读指针就不把指针画进画面：观看方用本机的系统指针画，跟手
    let separate_cursor = {
        let (backend, display) = (backend.clone(), display.clone());
        tokio::task::spawn_blocking(move || backend.cursor(&display).is_some()).await.unwrap_or(false)
    };
    let mut running = match Running::start(&backend, &display, quality, rate.bitrate, !separate_cursor, codec).await {
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
    let mut diag = HostDiag::new(&conn);
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
                diag.frame(frame.key, bytes);
                inflight.push_back(Inflight { seq, sent: Instant::now(), bytes });
                frames_this_second += 1;
            }
            msg = msg_rx.recv() => match msg {
                Some(ViewerMsg::Ack { seq: acked }) => {
                    while let Some(front) = inflight.front() {
                        if front.seq > acked { break; }
                        let f = inflight.pop_front().unwrap();
                        let delay = f.sent.elapsed();
                        diag.max_ack = diag.max_ack.max(delay);
                        rate.on_ack(delay, f.bytes);
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
                    drop(std::mem::replace(&mut running, match Running::start(&backend, &want, quality, rate.bitrate, !separate_cursor, codec).await {
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
                    drop(std::mem::replace(&mut running, match Running::start(&backend, &display, quality, rate.bitrate, !separate_cursor, codec).await {
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
                if let Some((min_rtt, link_bps)) = link_info(&conn) {
                    rate.set_link(min_rtt, link_bps);
                }
                let oldest = inflight.front().map(|f| f.sent.elapsed());
                if let Some(age) = oldest {
                    diag.max_oldest = diag.max_oldest.max(age);
                }
                if paused.is_none() && !rate.may_send(oldest) {
                    diag.blocked += 1;
                }
                if diag.since.elapsed() >= HOST_DIAG_EVERY {
                    diag.log(&conn, rate.bitrate, inflight.len());
                }
                if let Some(b) = rate.tick(oldest) {
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
    /// 每个 NAL 前面 4 字节长度的帧数据，不含参数集
    pub data: Vec<u8>,
    /// 只有 H.265 有
    pub vps: Option<Vec<u8>>,
    pub sps: Option<Vec<u8>>,
    pub pps: Option<Vec<u8>>,
    /// 含 IDR（H.265 还有 CRA 这类随机访问帧），是关键帧
    pub key: bool,
}

/// Annex B（00 00 00 01 分隔）转成每个 NAL 带 4 字节长度的格式，顺便把参数集挑出来。
/// Windows 的编码器吐 Annex B，WebCodecs 配了 description（avcC、hvcC）就要带长度的
pub fn annexb_to_avcc(data: &[u8]) -> Avcc {
    annexb_to_lengths(data, VideoCodec::H264)
}

pub fn annexb_to_lengths(data: &[u8], codec: VideoCodec) -> Avcc {
    let mut out = Avcc { data: Vec::with_capacity(data.len() + 16), ..Default::default() };
    for nal in split_annexb(data) {
        if nal.is_empty() {
            continue;
        }
        match codec {
            VideoCodec::H264 => match nal[0] & 0x1F {
                7 => { out.sps = Some(nal.to_vec()); continue; }
                8 => { out.pps = Some(nal.to_vec()); continue; }
                // 访问单元分隔符，WebCodecs 不需要
                9 => continue,
                5 => out.key = true,
                _ => {}
            },
            VideoCodec::H265 => match (nal[0] >> 1) & 0x3F {
                32 => { out.vps = Some(nal.to_vec()); continue; }
                33 => { out.sps = Some(nal.to_vec()); continue; }
                34 => { out.pps = Some(nal.to_vec()); continue; }
                35 => continue,
                // BLA、IDR、CRA：解码器能从这里开始解
                16..=21 => out.key = true,
                _ => {}
            },
        }
        out.data.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        out.data.extend_from_slice(nal);
    }
    out
}

/// 去掉防竞争字节（00 00 03 里的 03），得到 RBSP
fn rbsp(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut zeros = 0;
    for &b in nal {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

/// 从 VPS、SPS、PPS 拼出 hvcC（HEVCDecoderConfigurationRecord）和 WebCodecs 的 codec 字符串。
/// Windows 的 H.265 编码器只吐原始码流，没有现成的 hvcC。按 8 位 4:2:0 的 Main 写，硬件编码器出的就是这种
pub fn hvcc_config(vps: &[u8], sps: &[u8], pps: &[u8], width: u32, height: u32) -> Result<VideoConfig> {
    // SPS：2 字节 NAL 头，1 字节（VPS 编号、子层数、时域嵌套），后面 12 字节就是 general profile_tier_level，
    // 正好是 hvcC 里第 1~12 字节
    let r = rbsp(sps);
    if r.len() < 15 {
        bail!("H.265 的 SPS 太短");
    }
    let ptl = &r[3..15];
    let mut rec = vec![1u8];
    rec.extend_from_slice(ptl);
    rec.extend_from_slice(&[0xF0, 0x00, 0xFC, 0xFD, 0xF8, 0xF8, 0x00, 0x00, 0x0F, 3]);
    for (kind, nal) in [(32u8, vps), (33, sps), (34, pps)] {
        rec.push(0x80 | kind);
        rec.extend_from_slice(&1u16.to_be_bytes());
        rec.extend_from_slice(&(nal.len() as u16).to_be_bytes());
        rec.extend_from_slice(nal);
    }
    hevc_config(&rec, width, height)
}

/// 已经有 hvcC（macOS 的编码器给现成的），算出 codec 字符串（ISO/IEC 14496-15 附录 E）
pub fn hevc_config(hvcc: &[u8], width: u32, height: u32) -> Result<VideoConfig> {
    use base64::Engine as _;
    if hvcc.len() < 23 {
        bail!("hvcC 太短");
    }
    let space = ["", "A", "B", "C"][(hvcc[1] >> 6) as usize];
    let tier = if hvcc[1] & 0x20 != 0 { 'H' } else { 'L' };
    let profile = hvcc[1] & 0x1F;
    // 兼容标志按位倒过来写成十六进制
    let compat = u32::from_be_bytes(hvcc[2..6].try_into().unwrap()).reverse_bits();
    let level = hvcc[12];
    let mut constraints: Vec<u8> = hvcc[6..12].to_vec();
    while constraints.last() == Some(&0) {
        constraints.pop();
    }
    let mut codec = format!("hvc1.{space}{profile}.{compat:X}.{tier}{level}");
    for c in constraints {
        codec.push_str(&format!(".{c:X}"));
    }
    Ok(VideoConfig { codec, width, height, description: base64::engine::general_purpose::STANDARD.encode(hvcc) })
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
        let Avcc { data: avcc, sps: s, pps: p, key, .. } = annexb_to_avcc(&data);
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
    fn hevc_record_and_codec_string() {
        // 硬件编码器常见的 Main、4.1 级、只有逐行标志的参数集
        let vps = [0x40, 0x01, 0x0C, 0x01, 0xFF, 0xFF];
        let mut sps = vec![0x42, 0x01, 0x01, 0x01, 0x60, 0x00, 0x00, 0x00, 0x90, 0x00, 0x00, 0x00, 0x00, 0x00, 0x7B];
        sps.extend_from_slice(&[0xA0, 0x03]);
        let pps = [0x44, 0x01, 0xC1, 0x72];
        let cfg = hvcc_config(&vps, &sps, &pps, 1920, 1080).unwrap();
        assert_eq!(cfg.codec, "hvc1.1.6.L123.90");
        // 带长度的转换：参数集挑出来，IDR 算关键帧
        let mut data = Vec::new();
        for nal in [&vps[..], &sps[..], &pps[..], &[0x26, 0x01, 0xAF, 0x05][..], &[0x02, 0x01, 0xD0][..]] {
            data.extend_from_slice(&[0, 0, 0, 1]);
            data.extend_from_slice(nal);
        }
        let out = annexb_to_lengths(&data, VideoCodec::H265);
        assert!(out.key);
        assert_eq!(out.vps.as_deref(), Some(&vps[..]));
        assert_eq!(out.sps.as_deref(), Some(&sps[..]));
        assert_eq!(out.data.len(), 4 + 4 + 4 + 3);
        // 防竞争字节要去掉
        assert_eq!(rbsp(&[0, 0, 3, 1, 0, 0, 3, 0]), vec![0, 0, 1, 0, 0, 0]);
        // 老版本观看方不带格式：默认 H.264
        let open: ScreenOpen = serde_json::from_str(r#"{"quality":"balanced"}"#).unwrap();
        assert!(open.codecs.is_empty());
        let open: ScreenOpen = serde_json::from_str(r#"{"codecs":["h265","h264"]}"#).unwrap();
        assert_eq!(open.codecs, vec![VideoCodec::H265, VideoCodec::H264]);
    }

    /// macOS 硬件编码器给的真实 hvcC：从里面取出参数集，用 Windows 那边拼 hvcC 的办法再拼一次，结果要对得上
    #[test]
    fn hevc_record_matches_videotoolbox() {
        use base64::Engine as _;
        let real = base64::engine::general_purpose::STANDARD
            .decode("AQFgAAAAsAAAAAAAe/AA/P34+AAACwOgAAEAGkABDAP//wFgAAADALAAAAMAAAMAewAAGwJAoQABADBCAQMBYAAAAwCwAAADAAADAHsAAKADwIARB8uIG7kUjLn4T0L6hvVKagIaAgfwgEGiAAEAB0QBwHLwWyQ=")
            .unwrap();
        assert_eq!(hevc_config(&real, 1920, 1080).unwrap().codec, "hvc1.1.6.L123.B0");
        let mut nals = Vec::new();
        let mut i = 23;
        for _ in 0..real[22] {
            let n = u16::from_be_bytes([real[i + 1], real[i + 2]]) as usize;
            i += 3;
            for _ in 0..n {
                let len = u16::from_be_bytes([real[i], real[i + 1]]) as usize;
                nals.push(real[i + 2..i + 2 + len].to_vec());
                i += 2 + len;
            }
        }
        let rebuilt = hvcc_config(&nals[0], &nals[1], &nals[2], 1920, 1080).unwrap();
        assert_eq!(rebuilt.codec, "hvc1.1.6.L123.B0");
        let rebuilt = base64::engine::general_purpose::STANDARD.decode(rebuilt.description).unwrap();
        // profile、tier、level 那 12 字节和三组参数集都要一样
        assert_eq!(rebuilt[1..13], real[1..13]);
        assert_eq!(rebuilt[22..], real[22..]);
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
    /// 清晰档一开始就是最高码率，中途切过去也马上拉满；真的堵了照样往下降
    #[test]
    fn sharp_starts_at_full_rate() {
        let mut rate = RateControl::new(Quality::Sharp, Duration::from_millis(20));
        assert_eq!(rate.bitrate, Quality::Sharp.max_bitrate());
        let mut rate2 = RateControl::new(Quality::Balanced, Duration::from_millis(20));
        rate2.set_quality(Quality::Sharp);
        assert_eq!(rate2.bitrate, Quality::Sharp.max_bitrate());
        rate2.set_quality(Quality::Smooth);
        assert_eq!(rate2.bitrate, Quality::Smooth.max_bitrate());
        for _ in 0..5 {
            rate.on_ack(Duration::from_millis(400), 1000);
        }
        assert!(rate.tick(None).unwrap() < Quality::Sharp.max_bitrate());
    }

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

    /// 跨境 80 ms、链路很快、码率已经掉到下限：大帧的传输时间按链路速度扣，最小延迟不会掉到往返以下，
    /// 小帧不再像在排队，码率能涨回去
    #[test]
    fn low_bitrate_does_not_spiral_down() {
        let mut rate = RateControl::new(Quality::Balanced, Duration::from_millis(80));
        rate.bitrate = MIN_BITRATE;
        rate.set_link(Duration::from_millis(78), 50_000_000);
        for round in 0..30 {
            for i in 0..14 {
                // 偶尔有 30 KB 的大帧，链路 50 Mbps 只要 5 毫秒；小帧有 Wi-Fi 那种几十毫秒的抖动
                let (delay, bytes) = if i % 7 == 0 { (90, 30_000) } else { (82 + (i * 9 % 50), 2_000) };
                rate.on_ack(Duration::from_millis(delay as u64), bytes);
            }
            assert!(rate.min_delay >= Duration::from_millis(78), "第 {round} 轮最小延迟掉到了 {:?}", rate.min_delay);
            // 测试里不等真实时间：当作上次堵已经过去很久
            rate.last_congested = Instant::now() - INCREASE_AFTER;
            rate.tick(Some(Duration::from_millis(90)));
        }
        assert!(rate.bitrate > MIN_BITRATE * 10, "码率没涨起来：{}", rate.bitrate);
    }

    /// 不知道链路速度时（还没有确认）照旧按码率估，真堵了照样降
    #[test]
    fn link_floor_still_detects_real_queue() {
        let mut rate = RateControl::new(Quality::Balanced, Duration::from_millis(80));
        rate.set_link(Duration::from_millis(78), 50_000_000);
        let start = rate.bitrate;
        for _ in 0..6 {
            rate.on_ack(Duration::from_millis(400), 2_000);
        }
        assert!(rate.tick(None).unwrap() < start);
    }

    #[test]
    fn messages_are_tagged() {
        let json = serde_json::to_string(&ViewerMsg::Input { ev: InputEvent::Key { code: "KeyA".into(), down: true } }).unwrap();
        assert_eq!(json, r#"{"t":"input","ev":{"k":"key","code":"KeyA","down":true}}"#);
    }
}
