//! Windows.Graphics.Capture 抓屏：画面是 GPU 上的 BGRA 纹理，用 D3D11 视频处理器在 GPU 上
//! 缩放并转成 NV12（BT.709），再交给硬件编码器。整个过程画面都不离开显存。
//! 观看方能解 4:4:4 时，用着色器把画面转成 AYUV、按 H.265 Main 4:4:4 编（见 [`super::yuv444`]），码流本身就是 4:4:4；
//! 显卡的编码器不支持这个档次就照常编 4:2:0，告诉观看方的也是 4:2:0。
//! 抓屏、转换、编码都放在一个专门的线程里，COM 和 D3D 对象只在这个线程上用。
//!
//! 抓屏到编码是一条流水线：上一帧还在编码器里的时候，下一帧就能抓、转好送进去，
//! 帧率看编码器的吞吐，不看一帧要编多久。编码器里最多同时两帧；没人要帧的时候只先备一帧，
//! 不多编（编好的 P 帧不能扔，扔了后面的就解不出来）。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use p2pcli_core::host::BoxFuture;
use p2pcli_core::screen::{
    CaptureOptions, CaptureStats, ColorFormat, EncodedFrame, EncoderInfo, Primaries, ScreenCapture, VideoCodec, annexb_to_lengths, avcc_config,
    hvcc_config,
};
use tokio::sync::oneshot;
use tracing::{debug, info, warn};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{HMODULE, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709, DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P709, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Media::MediaFoundation::{IMFDXGIDeviceManager, MFCreateDXGIDeviceManager};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::core::{IInspectable, Interface};

use super::encode::Encoder;
use super::yuv444::AyuvConverter;

/// 老接口的颜色空间（位域）：第 1 位 RGB 范围（0 是全范围 0~255），第 2 位 YCbCr 矩阵（1 是 BT.709），
/// 第 4、5 位 YCbCr 的范围（1 是 16~235，2 是 0~255）。None 是输入的 RGB，Some(全范围) 是输出的 YCbCr
fn legacy_color_space(ycbcr_full_range: Option<bool>) -> D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
    const BT709: u32 = 1 << 2;
    let nominal = match ycbcr_full_range {
        None => 0,
        Some(false) => D3D11_VIDEO_PROCESSOR_NOMINAL_RANGE_16_235.0 as u32,
        Some(true) => D3D11_VIDEO_PROCESSOR_NOMINAL_RANGE_0_255.0 as u32,
    };
    D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: BT709 | (nominal << 4) }
}

/// 转好的 NV12 纹理轮着用几块：编码器里最多两帧在读，一块正在转，再留一块余量
pub(super) const NV12_POOL: usize = 4;
/// 要关键帧时最多等新画面这么久，等不到就拿上一帧重新编
const REUSE_AFTER: Duration = Duration::from_millis(100);
/// 最早送进编码器的那帧这么久还没出来，就当编码器卡住了，重建
const STALL: Duration = Duration::from_secs(2);

/// 流水线有多深
#[derive(Debug, Clone, Copy)]
pub(super) struct Depth {
    /// 编码器里最多同时几帧
    pub in_flight: usize,
    /// 没人要帧的时候最多先编好几帧等着
    pub prefetch: usize,
}

/// 平时用的：编码器里最多两帧，没人要时先备一帧
pub(super) const PIPELINED: Depth = Depth { in_flight: 2, prefetch: 1 };
/// 老的一路：有人要才抓一帧、编一帧、等它出来，测试里拿来比
#[cfg(test)]
pub(super) const SERIAL: Depth = Depth { in_flight: 1, prefetch: 0 };

/// 抓屏线程等的东西
pub(super) enum Ev {
    /// 要下一帧
    Next(oneshot::Sender<Result<EncodedFrame>>),
    Stop,
    /// 有新画面、编码器有事件、要关键帧了：醒过来看一眼
    Wake,
}

/// 抓屏线程和外面共用的状态
#[derive(Default)]
pub(super) struct Shared {
    pub want_key: AtomicBool,
    pub bitrate: AtomicU32,
    /// 最近交出去的那帧是不是 4:4:4。编码器建的时候才定下来，开不起来就是 4:2:0；
    /// 按交出去的帧记，不按正在编的记：被连方收到关键帧时读的颜色格式一定和这帧对得上
    pub chroma444: AtomicBool,
    pub source_frames: AtomicU64,
    pub encoded_frames: AtomicU64,
    pub skipped_frames: AtomicU64,
    pub busy_us: AtomicU64,
    pub max_us: AtomicU64,
    /// 送进编码器多少帧，测试里查有没有多编
    pub submitted: AtomicU64,
    /// 现在在用的编码器，观看方的诊断面板里显示
    pub encoder: Mutex<Option<EncoderInfo>>,
}

impl Shared {
    pub fn new(bitrate: u32) -> Arc<Self> {
        Arc::new(Shared { want_key: AtomicBool::new(true), bitrate: AtomicU32::new(bitrate), ..Default::default() })
    }

    /// 来了一帧新画面；replaced：原来那帧还没被取走就被它盖掉了
    pub fn arrived(&self, replaced: bool) {
        self.source_frames.fetch_add(1, Ordering::Relaxed);
        if replaced {
            self.skipped_frames.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn stats(&self) -> CaptureStats {
        CaptureStats {
            source_frames: self.source_frames.load(Ordering::Relaxed),
            encoded_frames: self.encoded_frames.load(Ordering::Relaxed),
            skipped_frames: self.skipped_frames.load(Ordering::Relaxed),
            busy_us: self.busy_us.load(Ordering::Relaxed),
            max_us: self.max_us.swap(0, Ordering::Relaxed),
        }
    }
}

pub struct WinCapture {
    tx: Sender<Ev>,
    full_range: bool,
    shared: Arc<Shared>,
}

/// 在抓屏线程上建状态：真抓屏是 [`State::new`]，合成屏幕见 [`super::synthetic`]
pub(super) type MakeState = fn(&CaptureOptions, Arc<Shared>, Sender<Ev>) -> Result<State>;

impl WinCapture {
    pub fn start(opts: CaptureOptions) -> Result<Self> {
        Self::start_with(opts, State::new)
    }

    pub(super) fn start_with(opts: CaptureOptions, make: MakeState) -> Result<Self> {
        let (tx, rx) = channel();
        let (init_tx, init_rx) = channel();
        let shared = Shared::new(opts.bitrate);
        let (sh, wake) = (shared.clone(), tx.clone());
        let full_range = opts.full_range;
        std::thread::Builder::new()
            .name("p2pcli-screen".into())
            .spawn(move || worker(opts, init_tx, wake, rx, sh, make))
            .context("开不了抓屏线程")?;
        init_rx.recv_timeout(Duration::from_secs(10)).map_err(|_| anyhow!("抓屏没有及时开起来"))??;
        Ok(WinCapture { tx, full_range, shared })
    }
}

impl Drop for WinCapture {
    fn drop(&mut self) {
        let _ = self.tx.send(Ev::Stop);
    }
}

impl ScreenCapture for WinCapture {
    fn next_frame(&self) -> BoxFuture<Result<EncodedFrame>> {
        let (reply, rx) = oneshot::channel();
        let sent = self.tx.send(Ev::Next(reply));
        Box::pin(async move {
            sent.map_err(|_| anyhow!("抓屏线程已经结束"))?;
            rx.await.map_err(|_| anyhow!("抓屏线程已经结束"))?
        })
    }

    fn request_keyframe(&self) {
        self.shared.want_key.store(true, Ordering::Relaxed);
        let _ = self.tx.send(Ev::Wake);
    }

    fn set_bitrate(&self, bps: u32) {
        self.shared.bitrate.store(bps, Ordering::Relaxed);
    }

    fn color(&self) -> ColorFormat {
        // 4:4:4 一定是全范围
        if self.shared.chroma444.load(Ordering::Relaxed) {
            return ColorFormat { full_range: true, primaries: Primaries::Bt709, chroma444: true };
        }
        // Windows 抓到的画面已经是 sRGB，只有范围跟着观看方
        ColorFormat { full_range: self.full_range, primaries: Primaries::Bt709, ..Default::default() }
    }

    fn stats(&self) -> Option<CaptureStats> {
        Some(self.shared.stats())
    }

    fn encoder(&self) -> Option<EncoderInfo> {
        self.shared.encoder.lock().unwrap().clone()
    }
}

fn worker(opts: CaptureOptions, init: Sender<Result<()>>, tx: Sender<Ev>, rx: Receiver<Ev>, shared: Arc<Shared>, make: MakeState) {
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let mut state = match make(&opts, shared, tx) {
        Ok(s) => {
            let _ = init.send(Ok(()));
            s
        }
        Err(e) => {
            let _ = init.send(Err(e));
            return;
        }
    };
    state.run(&rx, &opts);
    state.stop();
    debug!("停止抓屏");
}

/// 长边超过这么多（5K、6K 屏的原生分辨率）只有 H.265 编得了；编码器开不了就缩到这么大
const LARGE_LONG_EDGE: u32 = 3840;

fn fit(w: u32, h: u32, max_long_edge: u32) -> (u32, u32) {
    let long = w.max(h).max(1);
    let scale = (max_long_edge as f64 / long as f64).min(1.0);
    let even = |v: f64| ((v.round() as u32) & !1).max(2);
    (even(w as f64 * scale), even(h as f64 * scale))
}

/// 把输入纹理变成编码器要的 NV12，按输入输出尺寸建
enum Converter {
    /// 普通 4:2:0：GPU 视频处理器缩放并转成 NV12
    Nv12 {
        processor: ID3D11VideoProcessor,
        input_view: ID3D11VideoProcessorInputView,
        outputs: Vec<(ID3D11Texture2D, ID3D11VideoProcessorOutputView)>,
        next: usize,
    },
    /// 4:4:4：着色器转成 AYUV
    Ayuv(AyuvConverter),
}

/// 画面从哪来
enum Source {
    /// 抓屏：新画面一到就把帧池取空、只留最新一帧放在 latest 里
    Wgc {
        winrt_device: IDirect3DDevice,
        pool: Direct3D11CaptureFramePool,
        session: GraphicsCaptureSession,
        latest: Arc<Mutex<Option<Direct3D11CaptureFrame>>>,
        size: SizeInt32,
    },
    /// 合成屏幕：定时往 latest 里放 (时刻, 第几帧)，取的时候把 frames 里对应的那张拷进输入纹理
    Synthetic(Synthetic),
}

pub(super) struct Synthetic {
    pub frames: Vec<ID3D11Texture2D>,
    pub size: (u32, u32),
    pub latest: Arc<Mutex<Option<(Instant, usize)>>>,
    /// 定时放画面的线程，停的时候一起停；测试自己管定时的话是空
    pub ticker: Option<super::synthetic::Ticker>,
}

/// 已经拷进输入纹理、还没送进编码器的一帧
#[derive(Clone, Copy)]
struct Staged {
    /// 时间戳，100 纳秒
    ts: i64,
    /// 从帧池取到它的时候
    taken: Instant,
}

/// 送进了编码器、还没出来的一帧
struct InFlight {
    ts: i64,
    taken: Instant,
    /// 后来要了关键帧：它编出来也不交出去
    discard: bool,
}

/// 最近一次见到的参数集：(VPS（只有 H.265 有）, SPS, PPS)
pub(super) type ParamSets = (Option<Vec<u8>>, Vec<u8>, Vec<u8>);

type Reply = oneshot::Sender<Result<EncodedFrame>>;

pub(super) struct State {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    video_device: ID3D11VideoDevice,
    video_ctx: ID3D11VideoContext,
    manager: IMFDXGIDeviceManager,
    source: Source,
    /// 编码器有事件时往这里叫醒自己
    tx: Sender<Ev>,
    shared: Arc<Shared>,
    depth: Depth,
    /// 时间戳从这里算：同一帧重新编码时时间戳也要往前走
    pub started: Instant,
    /// WGC 的纹理先拷到这里和它的尺寸：视频处理器、着色器只读自己建的纹理，不挑驱动；
    /// 屏幕不动时也拿它重新转、重新编关键帧
    input: Option<(ID3D11Texture2D, (u32, u32))>,
    converter: Option<Converter>,
    encoder: Option<Encoder>,
    sps_pps: Option<ParamSets>,
    /// 还试不试 4:4:4：开不起来、编不出来以后这一路一直用 4:2:0
    try_yuv444: bool,
    staged: Option<Staged>,
    in_flight: VecDeque<InFlight>,
    /// 编好了还没交出去的（最多一帧）和它是不是 4:4:4
    ready: VecDeque<(EncodedFrame, bool)>,
    /// 从什么时候开始要关键帧（还没送出去）
    key_since: Option<Instant>,
}

/// 默认的那块显卡（接着主屏幕的），要能做视频处理
pub(super) fn create_device() -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device = None;
    let mut ctx = None;
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut ctx),
        )
        .context("开不了显卡（D3D11）")?;
    }
    let device = device.ok_or_else(|| anyhow!("开不了显卡"))?;
    let ctx = ctx.ok_or_else(|| anyhow!("开不了显卡"))?;
    Ok((device, ctx))
}

fn encoder_manager(device: &ID3D11Device) -> Result<IMFDXGIDeviceManager> {
    let mut token = 0u32;
    let mut manager = None;
    unsafe { MFCreateDXGIDeviceManager(&mut token, &mut manager)? };
    let manager = manager.ok_or_else(|| anyhow!("建不了编码器用的显卡管理器"))?;
    unsafe { manager.ResetDevice(device, token)? };
    Ok(manager)
}

impl State {
    fn new(opts: &CaptureOptions, shared: Arc<Shared>, tx: Sender<Ev>) -> Result<Self> {
        let monitor = super::find_monitor(&opts.display)?;
        let (device, ctx) = create_device()?;

        let dxgi: IDXGIDevice = device.cast()?;
        let inspectable: IInspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)? };
        let winrt_device: IDirect3DDevice = inspectable.cast()?;

        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item: GraphicsCaptureItem = unsafe { interop.CreateForMonitor(monitor.handle) }.context("抓不了这块屏幕")?;
        let size = item.Size()?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&winrt_device, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, size)?;
        // 新画面一到就把帧池取空、只留最新一帧：帧池只有两块缓冲，不取空的话后来的画面会被丢掉，
        // 观看方就会看到旧画面，还漏掉最后一次变化
        let latest: Arc<Mutex<Option<Direct3D11CaptureFrame>>> = Arc::default();
        let (slot, wake, counter) = (latest.clone(), tx.clone(), shared.clone());
        pool.FrameArrived(&TypedEventHandler::new(move |sender: windows::core::Ref<Direct3D11CaptureFramePool>, _| {
            let Some(pool) = sender.as_ref() else { return Ok(()) };
            let mut newest = None;
            while let Ok(f) = pool.TryGetNextFrame() {
                if let Some(old) = newest.replace(f) {
                    counter.arrived(true);
                    let _ = old.Close();
                }
            }
            if let Some(f) = newest {
                let old = slot.lock().unwrap().replace(f);
                counter.arrived(old.is_some());
                if let Some(old) = old {
                    let _ = old.Close();
                }
                let _ = wake.send(Ev::Wake);
            }
            Ok(())
        }))?;
        let session = pool.CreateCaptureSession(&item)?;
        // 能单独读指针时不画进画面：观看方用本机的系统指针画，跟手
        let _ = session.SetIsCursorCaptureEnabled(opts.show_cursor);
        // Windows 11 上去掉黄色边框；老系统不支持就算了
        let _ = session.SetIsBorderRequired(false);
        // 系统默认最多 60 帧。高刷屏要按目标帧率放开（Windows 11 24H2 起才有这个设置，老系统还是 60）。
        // 间隔比一帧短一成：正好卡在一帧的长度上时，稍早一点到的画面会被扔掉，帧率反而掉一截
        let interval = (10_000_000.0 * 0.9 / opts.fps.max(1) as f64) as i64;
        if let Err(e) = session.SetMinUpdateInterval(windows::Foundation::TimeSpan { Duration: interval }) {
            debug!("这个系统不能调抓屏的帧率上限（老于 Windows 11 24H2），最多 60 帧：{e}");
        }
        session.StartCapture().context("开始抓屏失败")?;
        info!(display = %opts.display, width = size.Width, height = size.Height, "开始抓屏");
        let source = Source::Wgc { winrt_device, pool, session, latest, size };
        Self::with_source(device, ctx, source, opts, shared, tx, PIPELINED)
    }

    /// 合成屏幕：画面由定时线程放进 latest
    pub fn synthetic(device: ID3D11Device, ctx: ID3D11DeviceContext, source: Synthetic, opts: &CaptureOptions, shared: Arc<Shared>, tx: Sender<Ev>, depth: Depth) -> Result<Self> {
        Self::with_source(device, ctx, Source::Synthetic(source), opts, shared, tx, depth)
    }

    fn with_source(device: ID3D11Device, ctx: ID3D11DeviceContext, source: Source, opts: &CaptureOptions, shared: Arc<Shared>, tx: Sender<Ev>, depth: Depth) -> Result<Self> {
        // 编码器在它自己的线程上也会用这块显卡
        if let Ok(mt) = device.cast::<ID3D11Multithread>() {
            let _ = unsafe { mt.SetMultithreadProtected(true) };
        }
        let video_device: ID3D11VideoDevice = device.cast().context("显卡不支持视频处理")?;
        let video_ctx: ID3D11VideoContext = ctx.cast().context("显卡不支持视频处理")?;
        let manager = encoder_manager(&device)?;
        Ok(State {
            device,
            ctx,
            video_device,
            video_ctx,
            manager,
            source,
            tx,
            shared,
            depth,
            started: Instant::now(),
            input: None,
            converter: None,
            encoder: None,
            sps_pps: None,
            try_yuv444: opts.yuv444,
            staged: None,
            in_flight: VecDeque::new(),
            ready: VecDeque::new(),
            key_since: None,
        })
    }

    fn stop(&mut self) {
        match &mut self.source {
            Source::Wgc { pool, session, .. } => {
                let _ = session.Close();
                let _ = pool.Close();
            }
            // 定时线程丢掉就停
            Source::Synthetic(s) => s.ticker = None,
        }
    }

    /// 主循环：等事件（要帧、新画面、编码器事件），每次醒来都收编码结果、交帧、能送就送新帧进编码器。
    /// 收到 Stop 或者出了修不好的错就返回；出错时把错误交给正在等（或者下一个来要）帧的人
    pub fn run(&mut self, rx: &Receiver<Ev>, opts: &CaptureOptions) {
        let mut pending: Option<Reply> = None;
        let result = loop {
            let first = match rx.recv_timeout(self.wait_time(pending.is_some())) {
                Ok(ev) => Some(ev),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break Ok(()),
            };
            let mut stop = false;
            for ev in first.into_iter().chain(std::iter::from_fn(|| rx.try_recv().ok())) {
                match ev {
                    Ev::Next(reply) => pending = Some(reply),
                    Ev::Stop => stop = true,
                    Ev::Wake => {}
                }
            }
            if stop {
                break Ok(());
            }
            if let Err(e) = self.step(&mut pending, opts) {
                break Err(e);
            }
        };
        let Err(e) = result else { return };
        let reply = pending.or_else(|| {
            loop {
                match rx.recv() {
                    Ok(Ev::Next(reply)) => break Some(reply),
                    Ok(Ev::Wake) => {}
                    Ok(Ev::Stop) | Err(_) => break None,
                }
            }
        });
        if let Some(reply) = reply {
            let _ = reply.send(Err(e));
        }
    }

    /// 最多睡多久：要关键帧时到点就拿上一帧重编，别的事都会把线程叫醒
    fn wait_time(&self, has_pending: bool) -> Duration {
        let mut wait = Duration::from_millis(200);
        if let Some(since) = self.key_since
            && self.input.is_some()
            && self.staged.is_none()
            && self.has_room(has_pending)
        {
            wait = wait.min((since + REUSE_AFTER).saturating_duration_since(Instant::now()));
        }
        if let Some(f) = self.in_flight.front() {
            wait = wait.min((f.taken + STALL).saturating_duration_since(Instant::now()));
        }
        wait.max(Duration::from_millis(1))
    }

    /// 还能不能再送一帧进编码器：编码器里不超过上限，编好没人要的不超过预备的那一帧
    fn has_room(&self, has_pending: bool) -> bool {
        let allowed = self.depth.prefetch + usize::from(has_pending);
        let live = self.ready.len() + self.in_flight.iter().filter(|f| !f.discard).count();
        live < allowed && self.in_flight.len() < self.depth.in_flight
    }

    fn step(&mut self, pending: &mut Option<Reply>, opts: &CaptureOptions) -> Result<()> {
        if pending.as_ref().is_some_and(|r| r.is_closed()) {
            // 对面已经不要了（停了、换屏了）
            *pending = None;
        }
        if self.shared.want_key.load(Ordering::Relaxed) {
            // 要关键帧：还没交出去的帧都不要了（观看方要的就是从关键帧重新开始），下一帧编成关键帧
            self.ready.clear();
            for f in &mut self.in_flight {
                f.discard = true;
            }
            self.key_since.get_or_insert_with(Instant::now);
        }
        if let Err(e) = self.pump(pending, opts) {
            self.recover(e)?;
            self.pump(pending, opts)?;
        }
        Ok(())
    }

    fn pump(&mut self, pending: &mut Option<Reply>, opts: &CaptureOptions) -> Result<()> {
        self.collect()?;
        self.deliver(pending);
        self.feed(pending.is_some(), opts)?;
        // 软件编码器是同步的，送进去就有结果
        self.collect()?;
        self.deliver(pending);
        Ok(())
    }

    /// 编码出错：4:4:4 时退回 4:2:0，拿输入纹理里留着的这帧接着编；4:2:0 也错就没办法了
    fn recover(&mut self, e: anyhow::Error) -> Result<()> {
        if !matches!(self.converter, Some(Converter::Ayuv(_))) {
            warn!("硬件编码出错：{e:#}");
            return Err(e);
        }
        warn!("4:4:4 编码出错，以后用 4:2:0：{e:#}");
        self.try_yuv444 = false;
        self.reset_encoder();
        self.restage();
        Ok(())
    }

    /// 转换器、编码器扔掉重建；编码器里的、编好没交出去的帧都不要了，下一帧是关键帧
    fn reset_encoder(&mut self) {
        self.converter = None;
        self.encoder = None;
        self.sps_pps = None;
        self.in_flight.clear();
        self.ready.clear();
        self.shared.want_key.store(true, Ordering::Relaxed);
        self.key_since.get_or_insert_with(Instant::now);
    }

    /// 输入纹理里的画面重新送一遍
    fn restage(&mut self) {
        if self.input.is_some() && self.staged.is_none() {
            let now = Instant::now();
            self.staged = Some(Staged { ts: self.ts(now), taken: now });
        }
    }

    fn ts(&self, at: Instant) -> i64 {
        (at.saturating_duration_since(self.started).as_nanos() / 100) as i64
    }

    /// 收编码器的结果：按时间戳对上是哪一帧，打包好放进 ready
    fn collect(&mut self) -> Result<()> {
        let Some(enc) = self.encoder.as_mut() else { return Ok(()) };
        enc.poll()?;
        while let Some((data, time)) = enc.take_output() {
            // 排在它前面没对上的，是编码器自己丢掉的帧
            let pos = time.and_then(|t| self.in_flight.iter().position(|f| f.ts == t)).unwrap_or(0);
            self.in_flight.drain(..pos.min(self.in_flight.len()));
            let entry = self.in_flight.pop_front();
            let ts = entry.as_ref().map(|f| f.ts).or(time).unwrap_or(0);
            let frame = package(enc, &mut self.sps_pps, &data, (ts / 10) as u64)?;
            let chroma444 = matches!(self.converter, Some(Converter::Ayuv(_)));
            match entry {
                Some(f) if f.discard => {}
                entry => {
                    if let Some(f) = entry {
                        let us = f.taken.elapsed().as_micros() as u64;
                        self.shared.busy_us.fetch_add(us, Ordering::Relaxed);
                        self.shared.max_us.fetch_max(us, Ordering::Relaxed);
                    }
                    self.ready.push_back((frame, chroma444));
                }
            }
        }
        if self.in_flight.front().is_some_and(|f| f.taken.elapsed() > STALL) {
            warn!(在编 = self.in_flight.len(), "硬件编码器好久没出结果，重建编码器");
            self.reset_encoder();
            self.restage();
        }
        Ok(())
    }

    /// 有人在等、有编好的帧就交出去
    fn deliver(&mut self, pending: &mut Option<Reply>) {
        if pending.is_none() {
            return;
        }
        let Some((frame, chroma444)) = self.ready.pop_front() else { return };
        // 先记下这帧的格式再交：对面拿到关键帧马上就读 color()
        self.shared.chroma444.store(chroma444, Ordering::Relaxed);
        match pending.take().unwrap().send(Ok(frame)) {
            Ok(()) => {
                self.shared.encoded_frames.fetch_add(1, Ordering::Relaxed);
            }
            // 对面刚好不要了：留着给下一个
            Err(Ok(frame)) => self.ready.push_front((frame, chroma444)),
            Err(Err(_)) => {}
        }
    }

    /// 有地方就送一帧进编码器：有新画面拿新的；要关键帧而屏幕一直不动，等一会儿就拿输入纹理里留着的上一帧重编
    fn feed(&mut self, has_pending: bool, opts: &CaptureOptions) -> Result<()> {
        if !self.has_room(has_pending) {
            return Ok(());
        }
        if let Some(staged) = self.take()? {
            // 还没送进去的那帧被新画面盖掉了
            if self.staged.replace(staged).is_some() {
                self.shared.skipped_frames.fetch_add(1, Ordering::Relaxed);
            }
        } else if self.key_since.is_some_and(|t| t.elapsed() >= REUSE_AFTER) {
            self.restage();
        }
        let Some(staged) = self.staged else { return Ok(()) };
        if self.converter.is_none() || self.encoder.is_none() {
            self.setup(opts, self.shared.bitrate.load(Ordering::Relaxed))?;
            self.shared.want_key.store(true, Ordering::Relaxed);
            let tx = self.tx.clone();
            let enc = self.encoder.as_mut().unwrap();
            enc.set_waker(move || {
                let _ = tx.send(Ev::Wake);
            });
            enc.poll()?;
        }
        // 新开的编码器还没要输入：画面先留着，它要了再送
        if !self.encoder.as_ref().unwrap().ready_for_input() {
            return Ok(());
        }
        let picture = self.convert()?;
        let key = self.shared.want_key.swap(false, Ordering::Relaxed);
        if key {
            self.key_since = None;
            self.ready.clear();
            for f in &mut self.in_flight {
                f.discard = true;
            }
        }
        let enc = self.encoder.as_mut().unwrap();
        enc.set_bitrate(self.shared.bitrate.load(Ordering::Relaxed));
        if let Err(e) = enc.submit(&self.ctx, &picture, staged.ts, key) {
            if key {
                self.shared.want_key.store(true, Ordering::Relaxed);
            }
            return Err(e);
        }
        self.staged = None;
        self.in_flight.push_back(InFlight { ts: staged.ts, taken: staged.taken, discard: false });
        self.shared.submitted.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// 取最新的一帧画面拷进输入纹理；没有新画面就是空
    fn take(&mut self) -> Result<Option<Staged>> {
        let taken = Instant::now();
        let (texture, src, at, frame) = match &mut self.source {
            Source::Wgc { winrt_device, pool, latest, size, .. } => {
                let Some(frame) = latest.lock().unwrap().take() else { return Ok(None) };
                let content = frame.ContentSize()?;
                if content.Width <= 0 || content.Height <= 0 {
                    return Ok(None);
                }
                if content != *size {
                    // 分辨率变了：帧池、转换器、编码器都按新尺寸重建
                    *size = content;
                    pool.Recreate(&*winrt_device, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, content)?;
                    self.input = None;
                    self.staged = None;
                    self.reset_encoder();
                    return Ok(None);
                }
                let surface = frame.Surface()?;
                let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
                let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };
                (texture, (content.Width as u32, content.Height as u32), taken, Some(frame))
            }
            Source::Synthetic(s) => {
                let Some((at, i)) = s.latest.lock().unwrap().take() else { return Ok(None) };
                (s.frames[i % s.frames.len()].clone(), s.size, at, None)
            }
        };
        if self.input.as_ref().is_none_or(|(_, size)| *size != src) {
            let input = texture_2d(&self.device, src.0, src.1, DXGI_FORMAT_B8G8R8A8_UNORM, D3D11_BIND_RENDER_TARGET.0 as u32 | D3D11_BIND_SHADER_RESOURCE.0 as u32)?;
            self.input = Some((input, src));
            self.staged = None;
            self.reset_encoder();
        }
        self.copy_in(&texture);
        if let Some(frame) = frame {
            let _ = frame.Close();
        }
        Ok(Some(Staged { ts: self.ts(at), taken }))
    }

    /// 换上新开的编码器，名字和是不是硬件记给观看方看
    fn set_encoder(&mut self, enc: Encoder) {
        // 名字是系统给编码器起的，像“NVIDIA HEVC Encoder MFT”，本身就带着格式
        let name = if enc.name.is_empty() { format!("Media Foundation {:?}", enc.format) } else { enc.name.clone() };
        *self.shared.encoder.lock().unwrap() = Some(EncoderInfo { name, hardware: enc.hardware });
        self.encoder = Some(enc);
    }

    /// 按输入尺寸建转换器和编码器。要 4:4:4 时先试三倍高的画面（只用硬件编码器，软件的编不动），
    /// 哪一步不行就退回 4:2:0，以后这一路不再试
    fn setup(&mut self, opts: &CaptureOptions, bitrate: u32) -> Result<()> {
        let (input, src) = self.input.clone().ok_or_else(|| anyhow!("还没有抓到画面"))?;
        let mut dst = fit(src.0, src.1, opts.max_long_edge);
        self.converter = None;
        self.encoder = None;
        self.sps_pps = None;
        self.in_flight.clear();
        self.ready.clear();
        // 真正的 4:4:4：显卡的 H.265 编码器支持 Main 4:4:4 档次才用，不支持就是 4:2:0
        if self.try_yuv444 {
            let ready = AyuvConverter::new(&self.device, &self.video_device, &self.video_ctx, &input, src, dst, opts.fps)
                .and_then(|conv| Ok((conv, Encoder::hevc444(&self.manager, dst.0, dst.1, opts.fps, bitrate)?)));
            match ready {
                Ok((conv, enc)) => {
                    info!(width = dst.0, height = dst.1, "远程画面用 4:4:4（H.265 Main 4:4:4）");
                    self.converter = Some(Converter::Ayuv(conv));
                    self.set_encoder(enc);
                    return Ok(());
                }
                Err(e) => {
                    warn!(width = dst.0, height = dst.1, "这块显卡编不了 4:4:4，用 4:2:0：{e:#}");
                    self.try_yuv444 = false;
                }
            }
        }
        // 5K、6K 的原生分辨率：只有 H.265 硬件编码器编得了（H.264 最多 4096 宽，观看方也多半解不了），开不了就缩到 3840
        let mut enc = None;
        if dst.0.max(dst.1) > LARGE_LONG_EDGE {
            match Encoder::new(&self.device, &self.manager, dst.0, dst.1, opts.fps, bitrate, opts.codec, opts.full_range, true) {
                Ok(e) if e.format == VideoCodec::H265 => enc = Some(e),
                Ok(_) => {
                    warn!(width = dst.0, height = dst.1, "没有 H.265 硬件编码器，编不了原生分辨率，缩到 {LARGE_LONG_EDGE}");
                    dst = fit(src.0, src.1, LARGE_LONG_EDGE);
                }
                Err(e) => {
                    warn!(width = dst.0, height = dst.1, "编不了原生分辨率，缩到 {LARGE_LONG_EDGE}：{e:#}");
                    dst = fit(src.0, src.1, LARGE_LONG_EDGE);
                }
            }
        }
        self.converter = Some(self.build_converter(&input, src, dst, opts.fps, opts.full_range)?);
        let enc = match enc {
            Some(e) => e,
            None => Encoder::new(&self.device, &self.manager, dst.0, dst.1, opts.fps, bitrate, opts.codec, opts.full_range, false)?,
        };
        debug!(hardware = enc.hardware, "编码器已换新");
        self.set_encoder(enc);
        Ok(())
    }

    /// full_range：转成全范围（0~255）的 NV12，不然是视频范围（16~235）
    fn build_converter(&self, input: &ID3D11Texture2D, src: (u32, u32), dst: (u32, u32), fps: u32, full_range: bool) -> Result<Converter> {
        let (processor, enumerator) = video_processor(&self.video_device, &self.video_ctx, src, dst, fps, Some(full_range))?;
        let input_view = vp_input_view(&self.video_device, &enumerator, input)?;
        let mut outputs = Vec::new();
        for _ in 0..NV12_POOL {
            let tex = encoder_texture(&self.device, dst.0, dst.1)?;
            let view = vp_output_view(&self.video_device, &enumerator, &tex)?;
            outputs.push((tex, view));
        }
        debug!(?src, ?dst, "GPU 缩放和转格式已就绪");
        Ok(Converter::Nv12 { processor, input_view, outputs, next: 0 })
    }

    /// 抓到的画面拷进输入纹理（也留作屏幕不动时重编关键帧用）
    fn copy_in(&mut self, frame: &ID3D11Texture2D) {
        let (input, src) = self.input.as_ref().unwrap();
        let region = D3D11_BOX { left: 0, top: 0, front: 0, right: src.0, bottom: src.1, back: 1 };
        unsafe { self.ctx.CopySubresourceRegion(input, 0, 0, 0, 0, frame, 0, Some(&region)) };
    }

    /// 在 GPU 上缩放并转成编码器要的 NV12，返回放结果的纹理
    fn convert(&mut self) -> Result<ID3D11Texture2D> {
        match self.converter.as_mut().unwrap() {
            Converter::Nv12 { processor, input_view, outputs, next } => {
                let i = *next;
                *next = (i + 1) % outputs.len();
                let (tex, view) = &outputs[i];
                vp_blt(&self.video_ctx, processor, input_view, view).context("GPU 转格式失败")?;
                Ok(tex.clone())
            }
            Converter::Ayuv(conv) => conv.convert(&self.ctx, &self.video_ctx),
        }
    }
}

/// 编码器出的 Annex B 转成 AVCC；关键帧带上解码配置（宽高是编码画面的）。参数集记在 sps_pps 里
pub(super) fn package(enc: &Encoder, sps_pps: &mut Option<ParamSets>, data: &[u8], ts_us: u64) -> Result<EncodedFrame> {
    let codec = enc.format;
    let avcc = annexb_to_lengths(data, codec);
    let complete = |vps: &Option<Vec<u8>>| codec == VideoCodec::H264 || vps.is_some();
    if let (Some(sps), Some(pps)) = (&avcc.sps, &avcc.pps)
        && complete(&avcc.vps)
    {
        *sps_pps = Some((avcc.vps.clone(), sps.clone(), pps.clone()));
    } else if sps_pps.is_none()
        && let Some(header) = enc.sequence_header()
    {
        let h = annexb_to_lengths(header, codec);
        if let (Some(sps), Some(pps)) = (h.sps, h.pps)
            && complete(&h.vps)
        {
            *sps_pps = Some((h.vps, sps, pps));
        }
    }
    let config = if avcc.key {
        let (vps, sps, pps) = sps_pps.as_ref().ok_or_else(|| anyhow!("编码器没给出参数集"))?;
        Some(match (codec, vps) {
            (VideoCodec::H265, Some(vps)) => hvcc_config(vps, sps, pps, enc.width, enc.height)?,
            _ => avcc_config(sps, pps, enc.width, enc.height)?,
        })
    } else {
        None
    };
    Ok(EncodedFrame { key: avcc.key, ts_us, data: avcc.data, config })
}

/// GPU 视频处理器：src 缩放到 dst。ycbcr_full_range 是 Some 时输出 NV12（BT.709，值是不是全范围），
/// None 时输出 BGRA，只缩放不转格式
pub(super) fn video_processor(
    video_device: &ID3D11VideoDevice,
    video_ctx: &ID3D11VideoContext,
    src: (u32, u32),
    dst: (u32, u32),
    fps: u32,
    ycbcr_full_range: Option<bool>,
) -> Result<(ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator)> {
    let rate = DXGI_RATIONAL { Numerator: fps.max(1), Denominator: 1 };
    let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
        InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
        InputFrameRate: rate,
        InputWidth: src.0,
        InputHeight: src.1,
        OutputFrameRate: rate,
        OutputWidth: dst.0,
        OutputHeight: dst.1,
        Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
    };
    let enumerator = unsafe { video_device.CreateVideoProcessorEnumerator(&content) }.context("显卡不支持缩放和转格式")?;
    let processor = unsafe { video_device.CreateVideoProcessor(&enumerator, 0)? };
    unsafe {
        video_ctx.VideoProcessorSetStreamFrameFormat(&processor, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
        video_ctx.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
        let src_rect = RECT { left: 0, top: 0, right: src.0 as i32, bottom: src.1 as i32 };
        let dst_rect = RECT { left: 0, top: 0, right: dst.0 as i32, bottom: dst.1 as i32 };
        video_ctx.VideoProcessorSetStreamSourceRect(&processor, 0, true, Some(&src_rect));
        video_ctx.VideoProcessorSetStreamDestRect(&processor, 0, true, Some(&dst_rect));
        video_ctx.VideoProcessorSetOutputTargetRect(&processor, true, Some(&dst_rect));
        if let Ok(ctx1) = video_ctx.cast::<ID3D11VideoContext1>() {
            ctx1.VideoProcessorSetStreamColorSpace1(&processor, 0, DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709);
            let out = match ycbcr_full_range {
                None => DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
                Some(true) => DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P709,
                Some(false) => DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
            };
            ctx1.VideoProcessorSetOutputColorSpace1(&processor, out);
        } else {
            // 老驱动没有新接口：用老的设法，不设的话默认按 BT.601 转，和标的 BT.709 对不上，整体偏色
            video_ctx.VideoProcessorSetStreamColorSpace(&processor, 0, &legacy_color_space(None));
            video_ctx.VideoProcessorSetOutputColorSpace(&processor, &legacy_color_space(ycbcr_full_range));
        }
    }
    Ok((processor, enumerator))
}

pub(super) fn vp_input_view(video_device: &ID3D11VideoDevice, enumerator: &ID3D11VideoProcessorEnumerator, input: &ID3D11Texture2D) -> Result<ID3D11VideoProcessorInputView> {
    let desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
        FourCC: 0,
        ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: 0 } },
    };
    let mut view = None;
    unsafe { video_device.CreateVideoProcessorInputView(input, enumerator, &desc, Some(&mut view))? };
    view.ok_or_else(|| anyhow!("建不了视频处理器的输入"))
}

pub(super) fn vp_output_view(video_device: &ID3D11VideoDevice, enumerator: &ID3D11VideoProcessorEnumerator, output: &ID3D11Texture2D) -> Result<ID3D11VideoProcessorOutputView> {
    let desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
        ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 } },
    };
    let mut view = None;
    unsafe { video_device.CreateVideoProcessorOutputView(output, enumerator, &desc, Some(&mut view))? };
    view.ok_or_else(|| anyhow!("建不了视频处理器的输出"))
}

/// 视频处理器转一帧
pub(super) fn vp_blt(video_ctx: &ID3D11VideoContext, processor: &ID3D11VideoProcessor, input: &ID3D11VideoProcessorInputView, output: &ID3D11VideoProcessorOutputView) -> Result<()> {
    let stream = D3D11_VIDEO_PROCESSOR_STREAM {
        Enable: true.into(),
        pInputSurface: std::mem::ManuallyDrop::new(Some(input.clone())),
        ..Default::default()
    };
    let mut streams = [stream];
    let result = unsafe { video_ctx.VideoProcessorBlt(processor, output, 0, &streams) };
    unsafe { std::mem::ManuallyDrop::drop(&mut streams[0].pInputSurface) };
    Ok(result?)
}

/// 交给编码器的 NV12 纹理。有的编码器要求纹理带“视频编码”绑定，不支持就去掉
pub(super) fn encoder_texture(device: &ID3D11Device, width: u32, height: u32) -> Result<ID3D11Texture2D> {
    texture_2d(device, width, height, DXGI_FORMAT_NV12, D3D11_BIND_RENDER_TARGET.0 as u32 | D3D11_BIND_VIDEO_ENCODER.0 as u32)
        .or_else(|_| texture_2d(device, width, height, DXGI_FORMAT_NV12, D3D11_BIND_RENDER_TARGET.0 as u32))
}

pub(super) fn texture_2d(device: &ID3D11Device, width: u32, height: u32, format: DXGI_FORMAT, bind: u32) -> Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: bind,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut tex = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut tex))? };
    tex.ok_or_else(|| anyhow!("建不了纹理"))
}
