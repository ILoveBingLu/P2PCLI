//! Windows.Graphics.Capture 抓屏：画面是 GPU 上的 BGRA 纹理，用 D3D11 视频处理器在 GPU 上
//! 缩放并转成 NV12（BT.709、视频范围），再交给硬件编码器。整个过程画面都不离开显存。
//! 抓屏、转换、编码都放在一个专门的线程里，COM 和 D3D 对象只在这个线程上用。

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use p2pcli_core::host::BoxFuture;
use p2pcli_core::screen::{CaptureOptions, ColorFormat, EncodedFrame, Primaries, ScreenCapture, VideoCodec, annexb_to_lengths, avcc_config, hvcc_config};
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
    DXGI_FORMAT_B8G8R8A8_UNORM,
    DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Media::MediaFoundation::{IMFDXGIDeviceManager, MFCreateDXGIDeviceManager};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::core::{IInspectable, Interface};

use super::encode::Encoder;

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

/// 转好的 NV12 纹理轮着用几块：编码器可能还在读上一块
const NV12_POOL: usize = 3;
/// 要关键帧时最多等新画面这么久，等不到就拿上一帧重新编
const REUSE_AFTER: Duration = Duration::from_millis(100);

enum Cmd {
    Next(oneshot::Sender<Result<EncodedFrame>>),
    Stop,
}

pub struct WinCapture {
    tx: Sender<Cmd>,
    full_range: bool,
    want_key: Arc<AtomicBool>,
    bitrate: Arc<AtomicU32>,
}

impl WinCapture {
    pub fn start(opts: CaptureOptions) -> Result<Self> {
        let (tx, rx) = channel();
        let (init_tx, init_rx) = channel();
        let want_key = Arc::new(AtomicBool::new(true));
        let bitrate = Arc::new(AtomicU32::new(opts.bitrate));
        let (k, b) = (want_key.clone(), bitrate.clone());
        let full_range = opts.full_range;
        std::thread::Builder::new()
            .name("p2pcli-screen".into())
            .spawn(move || worker(opts, init_tx, rx, k, b))
            .context("开不了抓屏线程")?;
        init_rx.recv_timeout(Duration::from_secs(10)).map_err(|_| anyhow!("抓屏没有及时开起来"))??;
        Ok(WinCapture { tx, full_range, want_key, bitrate })
    }
}

impl Drop for WinCapture {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Stop);
    }
}

impl ScreenCapture for WinCapture {
    fn next_frame(&self) -> BoxFuture<Result<EncodedFrame>> {
        let (reply, rx) = oneshot::channel();
        let sent = self.tx.send(Cmd::Next(reply));
        Box::pin(async move {
            sent.map_err(|_| anyhow!("抓屏线程已经结束"))?;
            rx.await.map_err(|_| anyhow!("抓屏线程已经结束"))?
        })
    }

    fn request_keyframe(&self) {
        self.want_key.store(true, Ordering::Relaxed);
    }

    fn set_bitrate(&self, bps: u32) {
        self.bitrate.store(bps, Ordering::Relaxed);
    }

    fn color(&self) -> ColorFormat {
        // Windows 抓到的画面已经是 sRGB，只有范围跟着观看方
        ColorFormat { full_range: self.full_range, primaries: Primaries::Bt709 }
    }
}

fn worker(opts: CaptureOptions, init: Sender<Result<()>>, rx: Receiver<Cmd>, want_key: Arc<AtomicBool>, bitrate: Arc<AtomicU32>) {
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let mut state = match State::new(&opts) {
        Ok(s) => {
            let _ = init.send(Ok(()));
            s
        }
        Err(e) => {
            let _ = init.send(Err(e));
            return;
        }
    };
    while let Ok(cmd) = rx.recv() {
        match cmd {
            Cmd::Stop => break,
            Cmd::Next(reply) => {
                let result = state.next(&reply, &want_key, &bitrate, &opts);
                match result {
                    Ok(Some(frame)) => {
                        let _ = reply.send(Ok(frame));
                    }
                    // 对面已经不要了（停了、换屏了）
                    Ok(None) => {}
                    Err(e) => {
                        let _ = reply.send(Err(e));
                        break;
                    }
                }
            }
        }
    }
    state.stop();
    debug!("停止抓屏");
}

fn fit(w: u32, h: u32, max_long_edge: u32) -> (u32, u32) {
    let long = w.max(h).max(1);
    let scale = (max_long_edge as f64 / long as f64).min(1.0);
    let even = |v: f64| ((v.round() as u32) & !1).max(2);
    (even(w as f64 * scale), even(h as f64 * scale))
}

/// 缩放、转格式用的 GPU 视频处理器，按输入输出尺寸建
struct Converter {
    processor: ID3D11VideoProcessor,
    /// WGC 的纹理先拷到这里，视频处理器只读自己建的纹理，不挑驱动
    input: ID3D11Texture2D,
    input_view: ID3D11VideoProcessorInputView,
    outputs: Vec<(ID3D11Texture2D, ID3D11VideoProcessorOutputView)>,
    next: usize,
    src: (u32, u32),
    dst: (u32, u32),
}

struct State {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    video_device: ID3D11VideoDevice,
    video_ctx: ID3D11VideoContext,
    manager: IMFDXGIDeviceManager,
    winrt_device: IDirect3DDevice,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    frames: Receiver<()>,
    latest: Arc<Mutex<Option<Direct3D11CaptureFrame>>>,
    /// 时间戳从这里算：同一帧重新编码时时间戳也要往前走
    started: Instant,
    size: SizeInt32,
    converter: Option<Converter>,
    encoder: Option<Encoder>,
    /// 最近一次见到的参数集：(VPS（只有 H.265 有）, SPS, PPS)
    sps_pps: Option<(Option<Vec<u8>>, Vec<u8>, Vec<u8>)>,
}

impl State {
    fn new(opts: &CaptureOptions) -> Result<Self> {
        let monitor = super::find_monitor(&opts.display)?;
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
        // 编码器在它自己的线程上也会用这块显卡
        if let Ok(mt) = device.cast::<ID3D11Multithread>() {
            let _ = unsafe { mt.SetMultithreadProtected(true) };
        }
        let video_device: ID3D11VideoDevice = device.cast().context("显卡不支持视频处理")?;
        let video_ctx: ID3D11VideoContext = ctx.cast().context("显卡不支持视频处理")?;

        let mut token = 0u32;
        let mut manager = None;
        unsafe { MFCreateDXGIDeviceManager(&mut token, &mut manager)? };
        let manager = manager.ok_or_else(|| anyhow!("建不了编码器用的显卡管理器"))?;
        unsafe { manager.ResetDevice(&device, token)? };

        let dxgi: IDXGIDevice = device.cast()?;
        let inspectable: IInspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)? };
        let winrt_device: IDirect3DDevice = inspectable.cast()?;

        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item: GraphicsCaptureItem = unsafe { interop.CreateForMonitor(monitor.handle) }.context("抓不了这块屏幕")?;
        let size = item.Size()?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&winrt_device, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, size)?;
        // 新画面一到就把帧池取空、只留最新一帧：帧池只有两块缓冲，不取空的话后来的画面会被丢掉，
        // 观看方就会看到旧画面，还漏掉最后一次变化
        let (tx, frames) = channel();
        let latest: Arc<Mutex<Option<Direct3D11CaptureFrame>>> = Arc::default();
        let slot = latest.clone();
        pool.FrameArrived(&TypedEventHandler::new(move |sender: windows::core::Ref<Direct3D11CaptureFramePool>, _| {
            let Some(pool) = sender.as_ref() else { return Ok(()) };
            let mut newest = None;
            while let Ok(f) = pool.TryGetNextFrame() {
                if let Some(old) = newest.replace(f) {
                    let _ = old.Close();
                }
            }
            if let Some(f) = newest {
                if let Some(old) = slot.lock().unwrap().replace(f) {
                    let _ = old.Close();
                }
                let _ = tx.send(());
            }
            Ok(())
        }))?;
        let session = pool.CreateCaptureSession(&item)?;
        // 能单独读指针时不画进画面：观看方用本机的系统指针画，跟手
        let _ = session.SetIsCursorCaptureEnabled(opts.show_cursor);
        // Windows 11 上去掉黄色边框；老系统不支持就算了
        let _ = session.SetIsBorderRequired(false);
        session.StartCapture().context("开始抓屏失败")?;
        info!(display = %opts.display, width = size.Width, height = size.Height, "开始抓屏");
        Ok(State {
            device,
            ctx,
            video_device,
            video_ctx,
            manager,
            winrt_device,
            pool,
            session,
            frames,
            latest,
            started: Instant::now(),
            size,
            converter: None,
            encoder: None,
            sps_pps: None,
        })
    }

    fn stop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }

    /// 等屏幕有变化，抓最新的一帧，转好编码。对面不要了（reply 关了）就返回空。
    /// 要关键帧而屏幕一直不动时（恢复直连、观看方解码出错），拿 GPU 上留着的上一帧重新转、重新编
    fn next(&mut self, reply: &oneshot::Sender<Result<EncodedFrame>>, want_key: &AtomicBool, bitrate: &AtomicU32, opts: &CaptureOptions) -> Result<Option<EncodedFrame>> {
        loop {
            let can_reuse = want_key.load(Ordering::Relaxed) && self.converter.is_some();
            let wait = if can_reuse { REUSE_AFTER } else { Duration::from_millis(200) };
            let frame = match self.frames.recv_timeout(wait) {
                Ok(()) => {
                    while self.frames.try_recv().is_ok() {}
                    match self.latest.lock().unwrap().take() {
                        Some(f) => Some(f),
                        None => continue,
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if reply.is_closed() {
                        return Ok(None);
                    }
                    if !can_reuse {
                        continue;
                    }
                    None
                }
                Err(RecvTimeoutError::Disconnected) => bail!("抓屏停止了"),
            };
            let nv12 = match frame {
                Some(frame) => {
                    let content = frame.ContentSize()?;
                    if content.Width <= 0 || content.Height <= 0 {
                        continue;
                    }
                    if content != self.size {
                        // 分辨率变了：帧池、转换器、编码器都按新尺寸重建
                        self.size = content;
                        self.pool.Recreate(&self.winrt_device, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, content)?;
                        self.converter = None;
                        self.encoder = None;
                        continue;
                    }
                    let surface = frame.Surface()?;
                    let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
                    let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };
                    let src = (content.Width as u32, content.Height as u32);
                    let dst = fit(src.0, src.1, opts.max_long_edge);
                    if self.converter.as_ref().is_none_or(|c| c.src != src || c.dst != dst) {
                        self.converter = Some(self.build_converter(src, dst, opts.fps, opts.full_range)?);
                        self.encoder = None;
                    }
                    self.copy_in(&texture);
                    let _ = frame.Close();
                    self.blit()?
                }
                // 屏幕没动：转换器里还留着上一帧
                None => self.blit()?,
            };
            let dst = self.converter.as_ref().unwrap().dst;
            let ts = (self.started.elapsed().as_nanos() / 100) as i64;

            let mut key = want_key.swap(false, Ordering::Relaxed);
            if self.encoder.is_none() {
                let enc = Encoder::new(&self.device, &self.manager, dst.0, dst.1, opts.fps, bitrate.load(Ordering::Relaxed), opts.codec, opts.full_range)?;
                debug!(hardware = enc.hardware, "编码器已换新");
                self.encoder = Some(enc);
                self.sps_pps = None;
                key = true;
            }
            let enc = self.encoder.as_mut().unwrap();
            enc.set_bitrate(bitrate.load(Ordering::Relaxed));
            let data = match enc.encode(&self.ctx, &nv12, ts, key) {
                Ok(Some(d)) => d,
                // 编码器丢了这帧：等下一次屏幕变化
                Ok(None) => {
                    if key {
                        want_key.store(true, Ordering::Relaxed);
                    }
                    continue;
                }
                Err(e) => {
                    warn!("硬件编码出错，重建编码器：{e:#}");
                    self.encoder = None;
                    want_key.store(true, Ordering::Relaxed);
                    return Err(e);
                }
            };
            let codec = enc.format;
            let avcc = annexb_to_lengths(&data, codec);
            let complete = |vps: &Option<Vec<u8>>| codec == VideoCodec::H264 || vps.is_some();
            if let (Some(sps), Some(pps)) = (&avcc.sps, &avcc.pps)
                && complete(&avcc.vps)
            {
                self.sps_pps = Some((avcc.vps.clone(), sps.clone(), pps.clone()));
            } else if self.sps_pps.is_none()
                && let Some(header) = enc.sequence_header()
            {
                let h = annexb_to_lengths(header, codec);
                if let (Some(sps), Some(pps)) = (h.sps, h.pps)
                    && complete(&h.vps)
                {
                    self.sps_pps = Some((h.vps, sps, pps));
                }
            }
            let config = if avcc.key {
                let (vps, sps, pps) = self.sps_pps.as_ref().ok_or_else(|| anyhow!("编码器没给出参数集"))?;
                Some(match (codec, vps) {
                    (VideoCodec::H265, Some(vps)) => hvcc_config(vps, sps, pps, enc.width, enc.height)?,
                    _ => avcc_config(sps, pps, enc.width, enc.height)?,
                })
            } else {
                None
            };
            return Ok(Some(EncodedFrame { key: avcc.key, ts_us: (ts / 10) as u64, data: avcc.data, config }));
        }
    }

    /// full_range：转成全范围（0~255）的 NV12，不然是视频范围（16~235）
    fn build_converter(&self, src: (u32, u32), dst: (u32, u32), fps: u32, full_range: bool) -> Result<Converter> {
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
        let enumerator = unsafe { self.video_device.CreateVideoProcessorEnumerator(&content) }.context("显卡不支持缩放和转格式")?;
        let processor = unsafe { self.video_device.CreateVideoProcessor(&enumerator, 0)? };
        unsafe {
            self.video_ctx.VideoProcessorSetStreamFrameFormat(&processor, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
            self.video_ctx.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
            let src_rect = RECT { left: 0, top: 0, right: src.0 as i32, bottom: src.1 as i32 };
            let dst_rect = RECT { left: 0, top: 0, right: dst.0 as i32, bottom: dst.1 as i32 };
            self.video_ctx.VideoProcessorSetStreamSourceRect(&processor, 0, true, Some(&src_rect));
            self.video_ctx.VideoProcessorSetStreamDestRect(&processor, 0, true, Some(&dst_rect));
            self.video_ctx.VideoProcessorSetOutputTargetRect(&processor, true, Some(&dst_rect));
            if let Ok(ctx1) = self.video_ctx.cast::<ID3D11VideoContext1>() {
                ctx1.VideoProcessorSetStreamColorSpace1(&processor, 0, DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709);
                let out = if full_range { DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P709 } else { DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709 };
                ctx1.VideoProcessorSetOutputColorSpace1(&processor, out);
            } else {
                // 老驱动没有新接口：用老的设法，不设的话默认按 BT.601 转，和标的 BT.709 对不上，整体偏色
                self.video_ctx.VideoProcessorSetStreamColorSpace(&processor, 0, &legacy_color_space(None));
                self.video_ctx.VideoProcessorSetOutputColorSpace(&processor, &legacy_color_space(Some(full_range)));
            }
        }

        let input = texture(&self.device, src.0, src.1, DXGI_FORMAT_B8G8R8A8_UNORM, D3D11_BIND_RENDER_TARGET.0 as u32 | D3D11_BIND_SHADER_RESOURCE.0 as u32)?;
        let in_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: 0 } },
        };
        let mut input_view = None;
        unsafe { self.video_device.CreateVideoProcessorInputView(&input, &enumerator, &in_desc, Some(&mut input_view))? };
        let input_view = input_view.ok_or_else(|| anyhow!("建不了视频处理器的输入"))?;

        let out_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 } },
        };
        let mut outputs = Vec::new();
        for _ in 0..NV12_POOL {
            // 有的编码器要求纹理带“视频编码”绑定，不支持就去掉
            let tex = texture(&self.device, dst.0, dst.1, DXGI_FORMAT_NV12, D3D11_BIND_RENDER_TARGET.0 as u32 | D3D11_BIND_VIDEO_ENCODER.0 as u32)
                .or_else(|_| texture(&self.device, dst.0, dst.1, DXGI_FORMAT_NV12, D3D11_BIND_RENDER_TARGET.0 as u32))?;
            let mut view = None;
            unsafe { self.video_device.CreateVideoProcessorOutputView(&tex, &enumerator, &out_desc, Some(&mut view))? };
            outputs.push((tex, view.ok_or_else(|| anyhow!("建不了视频处理器的输出"))?));
        }
        debug!(?src, ?dst, "GPU 缩放和转格式已就绪");
        Ok(Converter { processor, input, input_view, outputs, next: 0, src, dst })
    }

    /// 抓到的画面拷进转换器自己的纹理（也留作屏幕不动时重编关键帧用）
    fn copy_in(&mut self, frame: &ID3D11Texture2D) {
        let c = self.converter.as_ref().unwrap();
        let region = D3D11_BOX { left: 0, top: 0, front: 0, right: c.src.0, bottom: c.src.1, back: 1 };
        unsafe { self.ctx.CopySubresourceRegion(&c.input, 0, 0, 0, 0, frame, 0, Some(&region)) };
    }

    /// 在 GPU 上缩放并转成 NV12，返回放结果的纹理
    fn blit(&mut self) -> Result<ID3D11Texture2D> {
        let c = self.converter.as_mut().unwrap();
        let i = c.next;
        c.next = (c.next + 1) % c.outputs.len();
        let (tex, view) = &c.outputs[i];
        let stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            pInputSurface: std::mem::ManuallyDrop::new(Some(c.input_view.clone())),
            ..Default::default()
        };
        let mut streams = [stream];
        let result = unsafe { self.video_ctx.VideoProcessorBlt(&c.processor, view, 0, &streams) };
        unsafe { std::mem::ManuallyDrop::drop(&mut streams[0].pInputSurface) };
        result.context("GPU 转格式失败")?;
        Ok(tex.clone())
    }
}

fn texture(device: &ID3D11Device, width: u32, height: u32, format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT, bind: u32) -> Result<ID3D11Texture2D> {
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
